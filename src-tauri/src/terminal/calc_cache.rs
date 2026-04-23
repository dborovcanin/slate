use app_core::calc::CalcEngine;

/// Calc evaluation state kept between keystrokes to enable incremental updates.
pub struct CalcCache {
    pub engine: CalcEngine,
    pub results: Vec<Option<String>>,
    pub cell_results: Vec<Vec<(usize, String)>>,
    pub variable_names: Vec<String>,
    pub prev_line_metadata: Vec<crate::editor_core::calc_plan::LineMetadata>,
    pub stale: bool,
    pub cached_has_builtin_formula: bool,
    pub cached_has_variable_assignment: bool,
}
