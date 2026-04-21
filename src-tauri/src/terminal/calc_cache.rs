use app_core::calc::CalcEngine;

/// Calc evaluation state kept between keystrokes to enable incremental updates.
pub struct CalcCache {
    pub engine: CalcEngine,
    pub results: Vec<Option<String>>,
    pub cell_results: Vec<Vec<(usize, String)>>,
    pub variable_names: Vec<String>,
    pub prev_line_hashes: Vec<u64>,
    pub prev_line_has_assignment: Vec<bool>,
    pub prev_line_has_builtin_formula: Vec<bool>,
    pub stale: bool,
    pub cached_has_builtin_formula: bool,
    pub cached_has_variable_assignment: bool,
}
