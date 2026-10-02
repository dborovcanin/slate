use crate::calc::{CrossNoteRef, ExternVar, VariableIndexEntry};
use rustc_hash::{FxHashMap, FxHashSet};

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
    /// Notes for which a full CalcEngine eval has been done this session.
    /// Prevents re-evaluating a dep note when building extern_vars for ghost eval.
    full_eval_attempted_ids: FxHashSet<String>,
    /// Notes currently being evaluated on a background thread.
    /// Guards against concurrent duplicate evals when multiple notes share a dep.
    eval_in_flight_ids: FxHashSet<String>,
}

impl CrossNoteVarIndex {
    /// Update exported variable values for a note after evaluation.
    /// Returns `true` if any exported value changed (used to gate dependent re-eval).
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
        changed
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

    pub fn mark_full_eval_attempted(&mut self, note_id: &str) {
        self.eval_in_flight_ids.remove(note_id);
        self.full_eval_attempted_ids.insert(note_id.to_string());
    }

    pub fn was_full_eval_attempted(&self, note_id: &str) -> bool {
        self.full_eval_attempted_ids.contains(note_id)
    }

    /// Returns true if the full eval is already done OR currently in progress on
    /// another thread. Used by the sync preload path to avoid duplicating work
    /// that the autocomplete background thread is already handling.
    pub fn is_eval_done_or_in_flight(&self, note_id: &str) -> bool {
        self.full_eval_attempted_ids.contains(note_id) || self.eval_in_flight_ids.contains(note_id)
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
