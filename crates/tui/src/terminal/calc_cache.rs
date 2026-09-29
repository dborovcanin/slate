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

/// Calc evaluation state kept between keystrokes to enable incremental updates.
pub struct CalcCache {
    pub engine: CalcEngine,
    pub results: Vec<Option<String>>,
    pub cell_results: Vec<Vec<app_core::calc::TableCellEvaluation>>,
    pub variable_names: crate::terminal::render::VariableNames,
    /// Whole-note calc preparation reused across viewport range evaluations.
    pub range_context: app_core::calc::NoteContextCache,
    /// Line splices applied to `line_metadata` since `results` last matched
    /// it, so viewport notes can shift results instead of re-evaluating.
    pub pending_result_splices: Vec<ResultSplice>,
    pub calc_dependency_index: Option<crate::editor_core::calc_plan::CalcDependencyIndex>,
    /// Metadata for current `lines`, incrementally patched on edits.
    pub line_metadata: Vec<crate::editor_core::calc_plan::LineMetadata>,
    /// Snapshot aligned with `results` from the last recompute.
    pub prev_line_metadata: Vec<crate::editor_core::calc_plan::LineMetadata>,
    pub stale: bool,
    pub cached_has_builtin_formula: bool,
    /// Some line looks like a calculation (see `CalcSignalFlags::has_expression`).
    pub cached_has_expression: bool,
    pub cached_has_variable_assignment: bool,
    pub pathological_window_streak: usize,
    pub forced_full_recompute_remaining: usize,
}
