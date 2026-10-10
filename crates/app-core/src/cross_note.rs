use crate::calc::{
    scan_cross_note_refs, CalcEngine, CrossNoteRef, ExternVar, NoteEvaluationOptions,
    VariableIndexEntry,
};
use crate::storage::Db;
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::Mutex;

/// How many notes deep [`load_note_exports`] follows references; a longer
/// chain leaves its far end unresolved.
const MAX_DEPENDENCY_DEPTH: usize = 16;

/// Variable names exported by `note_id`, for autocomplete. A fast text scan
/// (no evaluation) on first use; values are not filled in.
pub fn exports_for_autocomplete(
    note_id: &str,
    index: &Mutex<CrossNoteVarIndex>,
    db: &Db,
) -> Vec<VariableIndexEntry> {
    if let Ok(index) = index.lock() {
        if index.was_name_scan_attempted(note_id) {
            return index.exports_for_note(note_id).to_vec();
        }
    }
    let Ok(Some(note)) = db.get_note(note_id) else {
        if let Ok(mut index) = index.lock() {
            index.mark_name_scan_attempted(note_id);
        }
        return Vec::new();
    };
    let lines: Vec<String> = note.body.split('\n').map(|l| l.to_string()).collect();
    let entries = crate::calc::scan_variable_assignments(&lines);
    if let Ok(mut index) = index.lock() {
        index.update_entries_only(note_id, &entries);
        index.mark_name_scan_attempted(note_id);
    }
    entries
}

/// Tracks variable exports and cross-note dependencies across all open notes.
/// Every key is a full note id, as written in `[[id]].var`.
///
/// Maintained in `AppCore`. Updated after each note evaluation; drives
/// reactive re-evaluation of dependent notes when exported values change.
#[derive(Debug, Default)]
pub struct CrossNoteVarIndex {
    /// note_id → (var_normalized → resolved f64 value)
    exports: FxHashMap<String, FxHashMap<String, f64>>,
    /// note_id → variable entries (for autocomplete)
    export_entries: FxHashMap<String, Vec<VariableIndexEntry>>,
    /// note_id → set of note ids this note depends on
    deps: FxHashMap<String, FxHashSet<String>>,
    /// Notes for which a fast name-scan has been done this session.
    /// Prevents re-hitting the DB for autocomplete on notes with zero variables.
    name_scan_attempted_ids: FxHashSet<String>,
    /// Notes evaluated for their exports, with the stored revision they were
    /// read at (`None` when the note was missing). Cleared for a note when
    /// a note it depends on exports new values.
    evaluated: FxHashMap<String, Option<String>>,
    /// Notes currently being evaluated on a background thread.
    /// Guards against concurrent duplicate evals when multiple notes share a dep.
    eval_in_flight_ids: FxHashSet<String>,
    /// Bumped by [`Self::reset`]; a load started before it publishes nothing.
    epoch: u64,
}

impl CrossNoteVarIndex {
    /// Forgets everything, as after the database was replaced. Loads still
    /// running from before cannot write their now stale values back.
    pub fn reset(&mut self) {
        *self = Self {
            epoch: self.epoch.wrapping_add(1),
            ..Self::default()
        };
    }

    /// Forgets calculated values after exchange rates change. Keep names and
    /// dependencies, and fence loads that began with the previous values.
    pub fn invalidate_calculations(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.exports.clear();
        self.evaluated.clear();
        self.eval_in_flight_ids.clear();
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Update exported variable values for a note after evaluation.
    /// Returns `true` if any exported value changed (used to gate dependent re-eval).
    /// Notes depending on `note_id`, directly or not, are evaluated again
    /// the next time they are needed when this changes a value.
    pub fn update_exports(
        &mut self,
        note_id: &str,
        entries: &[VariableIndexEntry],
        values: &FxHashMap<String, f64>,
    ) -> bool {
        let new_exports: FxHashMap<String, f64> = entries
            .iter()
            .filter_map(|e| {
                values
                    .get(&e.normalized)
                    .map(|&v| (e.normalized.clone(), v))
            })
            .collect();
        let changed = self
            .exports
            .get(note_id)
            .map(|old| old != &new_exports)
            .unwrap_or(!new_exports.is_empty());
        self.exports.insert(note_id.to_string(), new_exports);
        self.export_entries
            .insert(note_id.to_string(), entries.to_vec());
        if changed {
            self.invalidate_dependents(note_id);
        }
        changed
    }

    /// Forgets the evaluation of every note that reads `note_id`'s values,
    /// directly or through other notes.
    pub fn invalidate_dependents(&mut self, note_id: &str) {
        let mut pending = vec![note_id.to_string()];
        let mut seen: FxHashSet<String> = FxHashSet::default();
        while let Some(changed) = pending.pop() {
            for (dependent, deps) in &self.deps {
                if deps.contains(&changed) && seen.insert(dependent.clone()) {
                    pending.push(dependent.clone());
                }
            }
        }
        for dependent in seen {
            self.evaluated.remove(&dependent);
        }
    }

    /// Forgets the evaluation of `note_id`, and of the notes reading its
    /// values, when its stored revision is no longer `revision`.
    pub fn invalidate_if_stale(&mut self, note_id: &str, revision: Option<&str>) {
        let stale = self
            .evaluated
            .get(note_id)
            .is_some_and(|evaluated| evaluated.as_deref() != revision);
        if stale {
            self.evaluated.remove(note_id);
            self.invalidate_dependents(note_id);
        }
    }

    /// Update the dependency set for a note (which notes it references).
    pub fn update_deps(&mut self, note_id: &str, refs: &[CrossNoteRef]) {
        let dep_ids: FxHashSet<String> = refs.iter().map(|r| r.note_id.clone()).collect();
        if dep_ids.is_empty() {
            self.deps.remove(note_id);
        } else {
            self.deps.insert(note_id.to_string(), dep_ids);
        }
    }

    /// Build the `extern_vars` input for a note's evaluation from its current deps.
    pub fn extern_vars_for(&self, note_id: &str) -> Vec<ExternVar> {
        let Some(dep_ids) = self.deps.get(note_id) else {
            return Vec::new();
        };
        let mut result = Vec::new();
        for dep_id in dep_ids {
            if let Some(exports) = self.exports.get(dep_id.as_str()) {
                for (var_normalized, &value) in exports {
                    result.push(ExternVar {
                        note_id: dep_id.clone(),
                        var_normalized: var_normalized.clone(),
                        value,
                    });
                }
            }
        }
        result
    }

    /// Snapshot values for refs without publishing the caller's dependencies.
    pub fn extern_vars_for_refs(&self, refs: &[CrossNoteRef]) -> Vec<ExternVar> {
        let ids: FxHashSet<&str> = refs
            .iter()
            .map(|reference| reference.note_id.as_str())
            .collect();
        let mut result = Vec::new();
        for id in ids {
            if let Some(exports) = self.exports.get(id) {
                for (name, &value) in exports {
                    result.push(ExternVar {
                        note_id: id.to_owned(),
                        var_normalized: name.clone(),
                        value,
                    });
                }
            }
        }
        result
    }

    /// Return exported variable entries for `note_id` (for cross-note autocomplete).
    pub fn exports_for_note(&self, note_id: &str) -> &[VariableIndexEntry] {
        self.export_entries
            .get(note_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Update autocomplete entries without touching the f64 value map.
    /// Use after a fast name-only scan when values aren't available yet.
    pub fn update_entries_only(&mut self, note_id: &str, entries: &[VariableIndexEntry]) {
        self.export_entries
            .insert(note_id.to_string(), entries.to_vec());
    }

    // --- Name-scan tracking (fast path for autocomplete) ---

    pub fn mark_name_scan_attempted(&mut self, note_id: &str) {
        self.name_scan_attempted_ids.insert(note_id.to_string());
    }

    pub fn was_name_scan_attempted(&self, note_id: &str) -> bool {
        self.name_scan_attempted_ids.contains(note_id)
    }

    // --- Full-eval tracking (fast path for ghost-eval preload) ---

    /// Records that `note_id` no longer exists: its values are dropped and
    /// the notes reading them are evaluated again without them.
    pub fn forget_missing_note(&mut self, note_id: &str) {
        let had_values = self
            .exports
            .remove(note_id)
            .is_some_and(|values| !values.is_empty());
        self.export_entries.remove(note_id);
        self.deps.remove(note_id);
        if had_values {
            self.invalidate_dependents(note_id);
        }
        self.mark_evaluated(note_id, None);
    }

    /// Gives up an in-flight claim without recording an evaluation, so the
    /// note is tried again next time (after a failed read).
    pub fn release_claim(&mut self, note_id: &str) {
        self.eval_in_flight_ids.remove(note_id);
    }

    /// Records that `note_id` was evaluated from its stored text at
    /// `revision` (`None` when it does not exist).
    pub fn mark_evaluated(&mut self, note_id: &str, revision: Option<String>) {
        self.eval_in_flight_ids.remove(note_id);
        self.evaluated.insert(note_id.to_string(), revision);
    }

    pub fn was_full_eval_attempted(&self, note_id: &str) -> bool {
        self.evaluated.contains_key(note_id)
    }

    /// Returns true if the full eval is already done OR currently in progress on
    /// another thread. Used by the sync preload path to avoid duplicating work
    /// that the autocomplete background thread is already handling.
    pub fn is_eval_done_or_in_flight(&self, note_id: &str) -> bool {
        self.evaluated.contains_key(note_id) || self.eval_in_flight_ids.contains(note_id)
    }

    /// Atomically checks whether a full eval is already done or in progress,
    /// and if not, marks the note as in-flight. Returns `true` if the
    /// caller successfully claimed the eval slot and should proceed with the
    /// evaluation. Returns `false` if another caller already has it.
    pub fn try_claim_eval(&mut self, note_id: &str) -> bool {
        if self.is_eval_done_or_in_flight(note_id) {
            return false;
        }
        self.eval_in_flight_ids.insert(note_id.to_string());
        true
    }
}

/// Makes sure the index holds `note_id`'s exported values: evaluates its
/// stored text, after the notes it references (up to
/// `MAX_DEPENDENCY_DEPTH` deep, skipping cycles) so its own cross-note
/// values resolve. A note already evaluated is not read again.
pub fn load_note_exports(
    db: &Db,
    engine: &CalcEngine,
    index: &Mutex<CrossNoteVarIndex>,
    note_id: &str,
) {
    let Ok(epoch) = index.lock().map(|index| index.epoch) else {
        return;
    };
    load_from_epoch(db, engine, index, note_id, epoch);
}

/// [`load_note_exports`] for a load that began at `epoch`.
/// Load exports only while the dispatch epoch remains current, including publication.
pub fn load_from_epoch(
    db: &Db,
    engine: &CalcEngine,
    index: &Mutex<CrossNoteVarIndex>,
    note_id: &str,
    epoch: u64,
) {
    let mut visiting = Vec::new();
    load_with_dependencies(db, engine, index, note_id, epoch, &mut visiting);
}

/// The index, unless it was reset since `epoch`.
fn index_at<'a>(
    index: &'a Mutex<CrossNoteVarIndex>,
    epoch: u64,
) -> Option<std::sync::MutexGuard<'a, CrossNoteVarIndex>> {
    index.lock().ok().filter(|index| index.epoch == epoch)
}

fn load_with_dependencies(
    db: &Db,
    engine: &CalcEngine,
    index: &Mutex<CrossNoteVarIndex>,
    note_id: &str,
    epoch: u64,
    visiting: &mut Vec<String>,
) {
    let evaluated = index_at(index, epoch)
        .map(|index| index.was_full_eval_attempted(note_id))
        .unwrap_or(true);
    if evaluated
        || visiting.len() >= MAX_DEPENDENCY_DEPTH
        || visiting.iter().any(|id| id == note_id)
    {
        return;
    }
    let note = match db.get_note(note_id) {
        Ok(Some(note)) => note,
        Ok(None) => {
            if let Some(mut index) = index_at(index, epoch) {
                index.forget_missing_note(note_id);
            }
            return;
        }
        // A failed read is not an answer: leave the note to be tried again.
        Err(_) => {
            if let Some(mut index) = index_at(index, epoch) {
                index.release_claim(note_id);
            }
            return;
        }
    };
    let lines: Vec<String> = note.body.split('\n').map(str::to_string).collect();
    let refs = scan_cross_note_refs(&lines);
    visiting.push(note_id.to_string());
    let mut dep_ids: Vec<&str> = refs.iter().map(|r| r.note_id.as_str()).collect();
    dep_ids.sort_unstable();
    dep_ids.dedup();
    for dep_id in dep_ids {
        load_with_dependencies(db, engine, index, dep_id, epoch, visiting);
    }
    visiting.pop();

    let extern_vars = match index_at(index, epoch) {
        Some(mut index) => {
            index.update_deps(note_id, &refs);
            index.extern_vars_for(note_id)
        }
        None => return,
    };
    let result = engine.evaluate_note_context(
        &lines,
        NoteEvaluationOptions {
            variables_enabled: true,
            cross_note_enabled: true,
            extern_vars,
            precomputed_refs: Some(refs),
            ..Default::default()
        },
    );
    if let Some(mut index) = index_at(index, epoch) {
        index.update_exports(note_id, &result.variables, &result.variable_values);
        index.mark_name_scan_attempted(note_id);
        index.mark_evaluated(note_id, Some(note.updated_at));
    }
}

/// Re-checks the stored revision of each note `lines` reference, and of
/// the notes those read from, so values from notes changed elsewhere since
/// they were evaluated are read again. One revision lookup per note.
pub fn refresh_referenced_notes(db: &Db, index: &Mutex<CrossNoteVarIndex>, lines: &[String]) {
    let mut pending: Vec<String> = scan_cross_note_refs(lines)
        .into_iter()
        .map(|r| r.note_id)
        .collect();
    let mut seen: FxHashSet<String> = FxHashSet::default();
    while let Some(note_id) = pending.pop() {
        if !seen.insert(note_id.clone()) {
            continue;
        }
        let revision = db.get_note_updated_at(&note_id).ok().flatten();
        let Ok(mut index) = index.lock() else {
            return;
        };
        index.invalidate_if_stale(&note_id, revision.as_deref());
        if let Some(deps) = index.deps.get(&note_id) {
            pending.extend(deps.iter().cloned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (Db, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("cross-note-{}.db", ulid::Ulid::new()));
        (Db::open(path.clone()).expect("db opens"), path)
    }

    fn cleanup(path: &std::path::Path) {
        for suffix in ["", "-wal", "-shm"] {
            let mut file = path.as_os_str().to_owned();
            file.push(suffix);
            let _ = std::fs::remove_file(file);
        }
    }

    fn export(index: &Mutex<CrossNoteVarIndex>, note_id: &str, var: &str) -> Option<f64> {
        let index = index.lock().expect("index");
        index
            .exports
            .get(note_id)
            .and_then(|values| values.get(var))
            .copied()
    }

    #[test]
    fn a_referenced_note_is_evaluated_with_its_own_references() {
        let (db, path) = temp_db();
        db.save_note("note-a", "x := 5").expect("a");
        db.save_note("note-b", "y := [[note-a]].x + 1").expect("b");
        let index = Mutex::new(CrossNoteVarIndex::default());
        let engine = CalcEngine::new();

        // Note C reads B; B is loaded with A's value.
        load_note_exports(&db, &engine, &index, "note-b");
        assert_eq!(export(&index, "note-b", "y"), Some(6.0));
        assert_eq!(export(&index, "note-a", "x"), Some(5.0));

        drop(db);
        cleanup(&path);
    }

    #[test]
    fn new_values_upstream_are_picked_up_downstream() {
        let (db, path) = temp_db();
        db.save_note("note-a", "x := 5").expect("a");
        db.save_note("note-b", "y := [[note-a]].x + 1").expect("b");
        let index = Mutex::new(CrossNoteVarIndex::default());
        let engine = CalcEngine::new();
        load_note_exports(&db, &engine, &index, "note-b");

        // Editing A in this session updates its exports...
        let edited = vec!["x := 10".to_string()];
        let result = engine.evaluate_note_context(
            &edited,
            NoteEvaluationOptions {
                variables_enabled: true,
                ..Default::default()
            },
        );
        index.lock().expect("index").update_exports(
            "note-a",
            &result.variables,
            &result.variable_values,
        );
        // ...so B is evaluated again when next needed.
        load_note_exports(&db, &engine, &index, "note-b");
        assert_eq!(export(&index, "note-b", "y"), Some(11.0));

        // A changed elsewhere: opening a note that reads B notices it.
        db.save_note("note-a", "x := 20").expect("external edit");
        refresh_referenced_notes(&db, &index, &["[[note-b]].y".to_string()]);
        load_note_exports(&db, &engine, &index, "note-b");
        assert_eq!(export(&index, "note-b", "y"), Some(21.0));

        drop(db);
        cleanup(&path);
    }

    #[test]
    fn a_deleted_dependency_stops_contributing() {
        let (db, path) = temp_db();
        db.save_note("note-a", "x := 5").expect("a");
        db.save_note("note-b", "y := [[note-a]].x + 1").expect("b");
        let index = Mutex::new(CrossNoteVarIndex::default());
        let engine = CalcEngine::new();
        load_note_exports(&db, &engine, &index, "note-b");
        assert_eq!(export(&index, "note-b", "y"), Some(6.0));

        db.delete_note("note-a", None).expect("delete a");
        refresh_referenced_notes(&db, &index, &["[[note-b]].y".to_string()]);
        load_note_exports(&db, &engine, &index, "note-b");
        assert_eq!(export(&index, "note-a", "x"), None);
        assert_eq!(export(&index, "note-b", "y"), None);

        drop(db);
        cleanup(&path);
    }

    #[test]
    fn invalidating_calculations_preserves_names_and_fences_old_loads() {
        let (db, path) = temp_db();
        db.save_note("note-a", "x := 5").unwrap();
        let index = Mutex::new(CrossNoteVarIndex::default());
        let engine = CalcEngine::new();
        load_note_exports(&db, &engine, &index, "note-a");
        let old_epoch = index.lock().unwrap().epoch();
        index.lock().unwrap().invalidate_calculations();
        assert!(!index.lock().unwrap().exports_for_note("note-a").is_empty());
        assert_eq!(export(&index, "note-a", "x"), None);
        load_from_epoch(&db, &engine, &index, "note-a", old_epoch);
        assert_eq!(export(&index, "note-a", "x"), None);
        db.save_note("note-a", "x := 10").unwrap();
        load_note_exports(&db, &engine, &index, "note-a");
        assert_eq!(export(&index, "note-a", "x"), Some(10.0));
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn a_load_from_before_a_reset_publishes_nothing() {
        let (db, path) = temp_db();
        db.save_note("note-a", "x := 5").expect("a");
        let index = Mutex::new(CrossNoteVarIndex::default());
        let started = index.lock().expect("index").epoch();
        // The database is replaced while the load runs.
        index.lock().expect("index").reset();
        load_from_epoch(&db, &CalcEngine::new(), &index, "note-a", started);
        assert_eq!(export(&index, "note-a", "x"), None);
        assert!(!index
            .lock()
            .expect("index")
            .was_full_eval_attempted("note-a"));

        load_note_exports(&db, &CalcEngine::new(), &index, "note-a");
        assert_eq!(export(&index, "note-a", "x"), Some(5.0));

        drop(db);
        cleanup(&path);
    }

    #[test]
    fn cyclic_references_terminate() {
        let (db, path) = temp_db();
        db.save_note("note-a", "x := [[note-b]].y + 1").expect("a");
        db.save_note("note-b", "y := [[note-a]].x + 1\nz := 3")
            .expect("b");
        let index = Mutex::new(CrossNoteVarIndex::default());
        load_note_exports(&db, &CalcEngine::new(), &index, "note-a");
        assert_eq!(export(&index, "note-b", "z"), Some(3.0));

        drop(db);
        cleanup(&path);
    }
}
