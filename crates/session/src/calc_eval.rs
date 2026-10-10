use app_core::calc::{
    CalcEngine, ExternVar, NoteContextCache, NoteEvaluationOptions, NoteEvaluationResult,
    TableCellEvaluation,
};
pub struct CalcData {
    /// The line `line_results[0]` and `cell_results[0]` belong to; nonzero
    /// for a range evaluation, which returns only its own lines.
    pub first_line: usize,
    pub line_results: Vec<Option<String>>,
    pub cell_results: Vec<Vec<TableCellEvaluation>>,
    pub variable_names: Vec<String>,
}

impl CalcData {
    pub fn line_result(&self, line: usize) -> Option<String> {
        line.checked_sub(self.first_line)
            .and_then(|idx| self.line_results.get(idx))
            .cloned()
            .flatten()
    }

    pub fn cell_result(&self, line: usize) -> Vec<TableCellEvaluation> {
        line.checked_sub(self.first_line)
            .and_then(|idx| self.cell_results.get(idx))
            .cloned()
            .unwrap_or_default()
    }
}

pub fn compute_calc_data(
    engine: &CalcEngine,
    lines: &[String],
    variables_enabled: bool,
    cross_note_enabled: bool,
    table_enabled: bool,
    eval_range: Option<(usize, usize)>,
    extern_vars: Vec<ExternVar>,
) -> CalcData {
    let result = engine.evaluate_note_context(
        lines,
        NoteEvaluationOptions {
            variables_enabled,
            cross_note_enabled,
            table_enabled,
            eval_range,
            extern_vars,
            ..Default::default()
        },
    );
    calc_data_from_result(result)
}

/// Evaluates only `eval_lines`; table formula cells elsewhere read their
/// `table_cell_seeds` value instead of being evaluated again.
pub fn compute_calc_data_for_lines(
    engine: &CalcEngine,
    lines: &[String],
    options: NoteEvaluationOptions,
    eval_lines: Vec<usize>,
    table_cell_seeds: rustc_hash::FxHashMap<(usize, usize), String>,
) -> CalcData {
    let result = engine.evaluate_note_context(
        lines,
        NoteEvaluationOptions {
            eval_lines: Some(eval_lines),
            table_cell_seeds,
            ..options
        },
    );
    calc_data_from_result(result)
}

/// `compute_calc_data` that reuses whole-note preparation from `cache` while
/// the note is unchanged, for repeated range evaluations such as scrolling.
#[allow(clippy::too_many_arguments)]
pub fn compute_calc_data_cached(
    engine: &CalcEngine,
    lines: &[String],
    variables_enabled: bool,
    cross_note_enabled: bool,
    table_enabled: bool,
    eval_range: Option<(usize, usize)>,
    extern_vars: Vec<ExternVar>,
    cache: &mut NoteContextCache,
    text_generation: u64,
    omit_note_wide_results: bool,
) -> CalcData {
    let result = engine.evaluate_note_context_cached(
        lines,
        NoteEvaluationOptions {
            variables_enabled,
            cross_note_enabled,
            table_enabled,
            eval_range,
            extern_vars,
            text_generation: Some(text_generation),
            omit_note_wide_results,
            ..Default::default()
        },
        cache,
    );
    calc_data_from_result(result)
}

pub fn calc_data_from_result(result: NoteEvaluationResult) -> CalcData {
    let mut variable_names = result
        .variables
        .into_iter()
        .map(|entry| entry.normalized)
        .collect::<Vec<_>>();
    variable_names.sort();
    variable_names.dedup();

    let cell_results = result.table_cell_results;

    CalcData {
        first_line: result.first_line,
        line_results: result.line_results,
        cell_results,
        variable_names,
    }
}
