use app_core::calc::CalcEngine;

/// Calc evaluation state kept between keystrokes to enable incremental updates.
pub struct CalcCache {
    pub engine: CalcEngine,
    pub results: Vec<Option<String>>,
    pub cell_results: Vec<Vec<app_core::calc::TableCellEvaluation>>,
    pub variable_names: crate::terminal::render::VariableNames,
    /// Whole-note calc preparation reused across viewport range evaluations.
    pub range_context: app_core::calc::NoteContextCache,
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
