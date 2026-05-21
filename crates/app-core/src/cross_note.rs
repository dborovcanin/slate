use crate::calc::{CrossNoteRef, ExternVar, VariableIndexEntry};
use rustc_hash::{FxHashMap, FxHashSet};

/// Tracks variable exports and cross-note dependencies across all open notes.
///
/// Maintained in `AppCore`. Updated after each note evaluation; drives
/// reactive re-evaluation of dependent notes when exported values change.
#[derive(Debug, Default)]
pub struct CrossNoteVarIndex {
    /// short_id (lowercase) → (var_normalized → resolved f64 value)
    exports: FxHashMap<String, FxHashMap<String, f64>>,
    /// short_id (lowercase) → variable entries (for autocomplete)
    export_entries: FxHashMap<String, Vec<VariableIndexEntry>>,
    /// note_id → set of short_ids this note depends on
    deps: FxHashMap<String, FxHashSet<String>>,
    /// short_id (lowercase) → full note_id (for dependency propagation)
    short_id_to_note_id: FxHashMap<String, String>,
    /// short_ids for which a fast name-scan has been done this session.
    /// Prevents re-hitting the DB for autocomplete on notes with zero variables.
    name_scan_attempted_ids: FxHashSet<String>,
    /// short_ids for which a full CalcEngine eval has been done this session.
    /// Prevents re-evaluating a dep note when building extern_vars for ghost eval.
    full_eval_attempted_ids: FxHashSet<String>,
}

impl CrossNoteVarIndex {
    /// Register the mapping from a note's short_id to its full note_id.
    /// Should be called once per note on open/creation.
    pub fn register_note(&mut self, note_id: &str, short_id: &str) {
        self.short_id_to_note_id
            .insert(short_id.to_ascii_lowercase(), note_id.to_string());
    }

    /// Update exported variable values for a note after evaluation.
    /// Returns `true` if any exported value changed (used to gate dependent re-eval).
    pub fn update_exports(
        &mut self,
        short_id: &str,
        entries: &[VariableIndexEntry],
        values: &FxHashMap<String, f64>,
    ) -> bool {
        let short_id = short_id.to_ascii_lowercase();
        let new_exports: FxHashMap<String, f64> = entries
            .iter()
            .filter_map(|e| values.get(&e.normalized).map(|&v| (e.normalized.clone(), v)))
            .collect();
        let changed = self
            .exports
            .get(&short_id)
            .map(|old| old != &new_exports)
            .unwrap_or(!new_exports.is_empty());
        self.exports.insert(short_id.clone(), new_exports);
        self.export_entries.insert(short_id, entries.to_vec());
        changed
    }

    /// Update the dependency set for a note (which short_ids it references).
    pub fn update_deps(&mut self, note_id: &str, refs: &[CrossNoteRef]) {
        let dep_short_ids: FxHashSet<String> =
            refs.iter().map(|r| r.note_short_id.clone()).collect();
        if dep_short_ids.is_empty() {
            self.deps.remove(note_id);
        } else {
            self.deps.insert(note_id.to_string(), dep_short_ids);
        }
    }

    /// Build the `extern_vars` input for a note's evaluation from its current deps.
    pub fn extern_vars_for(&self, note_id: &str) -> Vec<ExternVar> {
        let Some(dep_ids) = self.deps.get(note_id) else {
            return Vec::new();
        };
        let mut result = Vec::new();
        for short_id in dep_ids {
            if let Some(exports) = self.exports.get(short_id.as_str()) {
                for (var_normalized, &value) in exports {
                    result.push(ExternVar {
                        note_short_id: short_id.clone(),
                        var_normalized: var_normalized.clone(),
                        value,
                    });
                }
            }
        }
        result
    }

    /// Return the full note_ids of all notes that depend on `short_id`.
    /// Used to find which notes to re-evaluate after a note's exports change.
    pub fn dependents_of(&self, short_id: &str) -> Vec<String> {
        let short_id_lower = short_id.to_ascii_lowercase();
        self.deps
            .iter()
            .filter(|(_, dep_ids)| dep_ids.contains(&short_id_lower))
            .map(|(note_id, _)| note_id.clone())
            .collect()
    }

    /// Return exported variable entries for `short_id` (for cross-note autocomplete).
    pub fn exports_for_short_id(&self, short_id: &str) -> &[VariableIndexEntry] {
        self.export_entries
            .get(&short_id.to_ascii_lowercase())
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Update autocomplete entries without touching the f64 value map.
    /// Use after a fast name-only scan when values aren't available yet.
    pub fn update_entries_only(&mut self, short_id: &str, entries: &[VariableIndexEntry]) {
        self.export_entries
            .insert(short_id.to_ascii_lowercase(), entries.to_vec());
    }

    // --- Name-scan tracking (fast path for autocomplete) ---

    pub fn mark_name_scan_attempted(&mut self, short_id: &str) {
        self.name_scan_attempted_ids
            .insert(short_id.to_ascii_lowercase());
    }

    pub fn was_name_scan_attempted(&self, short_id: &str) -> bool {
        self.name_scan_attempted_ids
            .contains(&short_id.to_ascii_lowercase())
    }

    // --- Full-eval tracking (fast path for ghost-eval preload) ---

    pub fn mark_full_eval_attempted(&mut self, short_id: &str) {
        self.full_eval_attempted_ids
            .insert(short_id.to_ascii_lowercase());
    }

    pub fn was_full_eval_attempted(&self, short_id: &str) -> bool {
        self.full_eval_attempted_ids
            .contains(&short_id.to_ascii_lowercase())
    }
}
