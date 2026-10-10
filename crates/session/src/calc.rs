use app_core::calc::CalcEngine;

/// One replacement of old lines `[start, start + old_span)` by `new_span`
/// lines, applied to a line list of length `old_len`.
#[derive(Debug, Clone, Copy)]
pub struct ResultSplice {
    pub start: usize,
    pub old_span: usize,
    pub new_span: usize,
    pub old_len: usize,
    /// The removed lines assigned a variable or held a builtin formula.
    pub removed_affects_calc: bool,
}

pub struct CalcState {
    pub engine: CalcEngine,
    pub results: Vec<Option<String>>,
    pub cell_results: Vec<Vec<app_core::calc::TableCellEvaluation>>,
    pub variable_names: crate::variables::VariableNames,
    /// Whole-note calc preparation reused across viewport range evaluations.
    pub range_context: app_core::calc::NoteContextCache,
    /// Line splices applied to `line_metadata` since `results` last matched
    /// it, so viewport notes can shift results instead of re-evaluating.
    pub pending_result_splices: Vec<ResultSplice>,
    /// Cross-note refs from the last scan and the hashes of the lines they
    /// were scanned from, so later scans only read changed lines.
    pub cross_note_refs_scan: Option<(Vec<u64>, Vec<app_core::calc::CrossNoteRef>)>,
    /// The editor's `text_generation` that `cross_note_refs_scan` matches,
    /// if known; then it is reused without rehashing the note.
    pub cross_note_refs_generation: Option<u64>,
    pub calc_dependency_index: Option<editor_core::calc_plan::CalcDependencyIndex>,
    /// Metadata for current `lines`, incrementally patched on edits.
    pub line_metadata: Vec<editor_core::calc_plan::LineMetadata>,
    /// Snapshot aligned with `results` from the last recompute.
    pub prev_line_metadata: Vec<editor_core::calc_plan::LineMetadata>,
    pub stale: bool,
    pub cached_has_builtin_formula: bool,
    /// Some line looks like a calculation (see `CalcSignalFlags::has_expression`).
    pub cached_has_expression: bool,
    pub cached_has_variable_assignment: bool,
    pub pathological_window_streak: usize,
    pub forced_full_recompute_remaining: usize,
}

impl Default for CalcState {
    fn default() -> Self {
        Self {
            engine: CalcEngine::new(),
            results: Default::default(),
            cell_results: Default::default(),
            variable_names: Default::default(),
            range_context: Default::default(),
            pending_result_splices: Default::default(),
            cross_note_refs_scan: Default::default(),
            cross_note_refs_generation: Default::default(),
            calc_dependency_index: Default::default(),
            line_metadata: Default::default(),
            prev_line_metadata: Default::default(),
            stale: Default::default(),
            cached_has_builtin_formula: Default::default(),
            cached_has_expression: Default::default(),
            cached_has_variable_assignment: Default::default(),
            pathological_window_streak: Default::default(),
            forced_full_recompute_remaining: Default::default(),
        }
    }
}
