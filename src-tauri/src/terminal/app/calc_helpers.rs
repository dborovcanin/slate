use super::VariableCompletionPrefix;
#[cfg(test)]
use crate::terminal::text_utils::line_display_cols;
use app_core::calc::CalcEngine;

#[cfg(test)]
pub(super) fn calc_ghost_prefix(text: &str, calc_ghost: Option<&str>) -> &'static str {
    if calc_ghost
        .map(|ghost| ghost.trim_start().starts_with('*'))
        .unwrap_or(false)
    {
        " "
    } else if contains_assignment_operator(text) {
        " = "
    } else {
        " → "
    }
}

#[cfg(test)]
pub(super) fn rendered_line_display_cols(text: &str, calc_ghost: Option<&str>) -> usize {
    let mut width = line_display_cols(text);
    if let Some(ghost) = calc_ghost {
        width += calc_ghost_prefix(text, Some(ghost)).chars().count();
        width += ghost.chars().count();
    }
    width
}

pub(super) struct CalcData {
    pub(super) line_results: Vec<Option<String>>,
    pub(super) cell_results: Vec<Vec<(usize, String)>>,
    pub(super) variable_names: Vec<String>,
}

pub(super) fn compute_calc_data(
    engine: &CalcEngine,
    lines: &[String],
    variables_enabled: bool,
    eval_range: Option<(usize, usize)>,
) -> CalcData {
    let result = engine.evaluate_note_context(
        lines,
        app_core::calc::NoteEvaluationOptions {
            variables_enabled,
            eval_range,
        },
    );
    let mut variable_names = result
        .variables
        .into_iter()
        .map(|entry| entry.normalized)
        .collect::<Vec<_>>();
    variable_names.sort();
    variable_names.dedup();

    let cell_results = result
        .table_cell_results
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|entry| (entry.cell_index, entry.value))
                .collect()
        })
        .collect();

    CalcData {
        line_results: result.line_results,
        cell_results,
        variable_names,
    }
}

#[cfg(test)]
pub(super) fn compute_calc_results(
    lines: &[String],
    variables_enabled: bool,
) -> Vec<Option<String>> {
    let engine = CalcEngine::new();
    compute_calc_data(&engine, lines, variables_enabled, None).line_results
}

/// Decide whether an already-eligible line's trailing ` = <literal>` should
/// be rewritten to `new_result`. The caller is responsible for the eligibility
/// gate (line unchanged since last recompute AND previous backend result was
/// `None`, i.e. trailer was in sync). This helper only handles the per-line
/// mechanics: locate the trailer, short-circuit when already in sync, and
/// enforce the cursor guard so we never yank text from under the caret.
///
/// Returns `Some((eq_byte_idx, new_tail))` so the caller can run
/// `line.replace_range(eq_byte_idx.., &new_tail)`, or `None` to leave the
/// line untouched.
pub(super) fn compute_calc_trailer_refresh(
    line: &str,
    new_result: &str,
    is_cursor_line: bool,
    cursor_col: usize,
) -> Option<(usize, String)> {
    let refresh = crate::editor_core::calc_plan::compute_calc_trailer_refresh(
        line,
        new_result,
        is_cursor_line,
        cursor_col,
    )?;
    Some((refresh.eq_byte_idx, refresh.new_tail))
}

pub(super) fn find_calc_segment_range(text: &str) -> Option<(usize, usize)> {
    let segment = crate::editor_core::calc_plan::find_calc_segment(text)?;
    Some((segment.from_byte, segment.to_byte))
}

pub(super) fn variable_query_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == ' '
}

pub(super) fn extract_variable_completion_prefix(
    line_text: &str,
    cursor_col: usize,
) -> Option<VariableCompletionPrefix> {
    let chars = line_text.chars().collect::<Vec<_>>();
    let col = cursor_col.min(chars.len());
    let mut from = col;

    while from > 0 && variable_query_char(chars[from - 1]) {
        from -= 1;
    }

    while from < col && chars[from] == ' ' {
        from += 1;
    }

    if from >= col {
        return None;
    }

    let query = chars[from..col].iter().collect::<String>();
    if query.is_empty() || query.ends_with(' ') {
        return None;
    }

    Some(VariableCompletionPrefix {
        from_col: from,
        to_col: col,
        query,
    })
}

pub(super) fn build_variable_suggestions(
    variable_names: &[String],
    query: &str,
    min_chars: usize,
    max_suggestions: usize,
) -> Vec<String> {
    let normalized_query = query.trim().to_lowercase();
    if normalized_query.chars().count() < min_chars {
        return Vec::new();
    }

    let mut matches = variable_names
        .iter()
        .filter(|name| name.starts_with(&normalized_query) && **name != normalized_query)
        .cloned()
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    matches.truncate(max_suggestions.max(1));
    matches
}

pub(super) fn contains_assignment_operator(text: &str) -> bool {
    crate::editor_core::calc_plan::contains_assignment_operator(text)
}
