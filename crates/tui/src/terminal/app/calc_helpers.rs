use super::VariableCompletionPrefix;
use crate::storage::Db;
#[cfg(test)]
use crate::terminal::text_utils::line_display_cols;
use app_core::calc::{
    CalcEngine, ExternVar, NoteContextCache, NoteEvaluationOptions, NoteEvaluationResult,
    TableCellEvaluation, VariableIndexEntry,
};
use app_core::cross_note::CrossNoteVarIndex;
use std::sync::{Arc, Mutex};

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
    pub(super) cell_results: Vec<Vec<TableCellEvaluation>>,
    pub(super) variable_names: Vec<String>,
}

pub(super) fn compute_calc_data(
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

/// `compute_calc_data` that reuses whole-note preparation from `cache` while
/// the note is unchanged, for repeated range evaluations such as scrolling.
#[allow(clippy::too_many_arguments)]
pub(super) fn compute_calc_data_cached(
    engine: &CalcEngine,
    lines: &[String],
    variables_enabled: bool,
    cross_note_enabled: bool,
    table_enabled: bool,
    eval_range: Option<(usize, usize)>,
    extern_vars: Vec<ExternVar>,
    cache: &mut NoteContextCache,
) -> CalcData {
    let result = engine.evaluate_note_context_cached(
        lines,
        NoteEvaluationOptions {
            variables_enabled,
            cross_note_enabled,
            table_enabled,
            eval_range,
            extern_vars,
            ..Default::default()
        },
        cache,
    );
    calc_data_from_result(result)
}

fn calc_data_from_result(result: NoteEvaluationResult) -> CalcData {
    let mut variable_names = result
        .variables
        .into_iter()
        .map(|entry| entry.normalized)
        .collect::<Vec<_>>();
    variable_names.sort();
    variable_names.dedup();

    let cell_results = result.table_cell_results;

    CalcData {
        line_results: result.line_results,
        cell_results,
        variable_names,
    }
}

/// Full-note eval that also reads/writes the shared cross-note variable index.
/// Use this instead of `compute_calc_data` for whole-document recomputes so that
/// cross-note variable references resolve correctly and exported variables stay
/// visible to other notes.
pub(super) fn compute_calc_data_for_note(
    engine: &CalcEngine,
    lines: &[String],
    variables_enabled: bool,
    cross_note_enabled: bool,
    table_enabled: bool,
    note_id: &str,
    short_id: &str,
    cross_note_var_index: &Arc<Mutex<CrossNoteVarIndex>>,
) -> CalcData {
    let has_cross_note_syntax =
        cross_note_enabled && !short_id.is_empty() && lines.iter().any(|l| l.contains("[["));

    let (extern_vars, precomputed_refs) = if has_cross_note_syntax {
        // Scan outside the lock: TUI runs on a single event-loop thread so
        // no concurrent eval can race update_deps for the same note_id.
        let refs = app_core::calc::scan_cross_note_refs(lines);
        let extern_vars = if let Ok(mut index) = cross_note_var_index.lock() {
            index.register_note(note_id, short_id);
            index.update_deps(note_id, &refs);
            index.extern_vars_for(note_id)
        } else {
            Vec::new()
        };
        (extern_vars, Some(refs))
    } else {
        (Vec::new(), None)
    };

    let result = engine.evaluate_note_context(
        lines,
        NoteEvaluationOptions {
            variables_enabled,
            cross_note_enabled,
            table_enabled,
            eval_range: None,
            extern_vars,
            precomputed_refs,
        },
    );

    if cross_note_enabled && !short_id.is_empty() {
        if let Ok(mut index) = cross_note_var_index.lock() {
            index.update_exports(short_id, &result.variables, &result.variable_values);
            index.update_deps(note_id, &result.cross_note_refs);
        }
    }

    let mut variable_names = result
        .variables
        .into_iter()
        .map(|entry| entry.normalized)
        .collect::<Vec<_>>();
    variable_names.sort();
    variable_names.dedup();

    CalcData {
        line_results: result.line_results,
        cell_results: result.table_cell_results,
        variable_names,
    }
}

#[cfg(test)]
pub(super) fn compute_calc_results(
    lines: &[String],
    variables_enabled: bool,
) -> Vec<Option<String>> {
    let engine = CalcEngine::new();
    compute_calc_data(
        &engine,
        lines,
        variables_enabled,
        true,
        true,
        None,
        Vec::new(),
    )
    .line_results
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

/// Candidate queries for a completion prefix, longest first: the whole run
/// (variable names may contain spaces, e.g. `tax ra` -> `tax rate`) and then
/// each shorter run starting after a space (`then pri` -> `pri`).
pub(super) fn variable_completion_candidates(
    prefix: &VariableCompletionPrefix,
) -> Vec<VariableCompletionPrefix> {
    let chars: Vec<char> = prefix.query.chars().collect();
    let mut candidates = vec![VariableCompletionPrefix {
        from_col: prefix.from_col,
        to_col: prefix.to_col,
        query: prefix.query.clone(),
    }];
    for (idx, ch) in chars.iter().enumerate() {
        if *ch == ' ' && idx + 1 < chars.len() && chars[idx + 1] != ' ' {
            candidates.push(VariableCompletionPrefix {
                from_col: prefix.from_col + idx + 1,
                to_col: prefix.to_col,
                query: chars[idx + 1..].iter().collect(),
            });
        }
    }
    candidates
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

/// Returns the 8-char wiki-link short ID for a DB note, or `""` for file notes.
pub(super) fn tui_note_short_id(note_id: &str) -> &str {
    if note_id.starts_with("mdfile:") || note_id.len() < 8 {
        return "";
    }
    &note_id[..8]
}

/// Returns variable name entries for `short_id` for autocomplete suggestions.
/// Uses a fast text scan (no expression evaluation) so the first call is cheap.
/// Values are NOT populated by this function — use `preload_cross_note_dep_value`
/// for ghost-eval correctness.
pub(super) fn cross_note_exports_for_autocomplete(
    short_id: &str,
    cross_note_var_index: &Arc<Mutex<CrossNoteVarIndex>>,
    _engine: &CalcEngine,
    db: &Db,
) -> Vec<VariableIndexEntry> {
    // Fast path: name scan already done for this short_id.
    if let Ok(index) = cross_note_var_index.lock() {
        if index.was_name_scan_attempted(short_id) {
            return index.exports_for_short_id(short_id).to_vec();
        }
    }

    // Slow path: load the note body and do a name-only scan (no eval).
    let note_sources = app_core::note_sources::NoteSourceService::new(db.clone());
    let note = match note_sources.resolve_wiki_link_note(short_id) {
        Ok(Some(n)) => n,
        _ => {
            if let Ok(mut index) = cross_note_var_index.lock() {
                index.mark_name_scan_attempted(short_id);
            }
            return Vec::new();
        }
    };
    let lines: Vec<String> = note.body.split('\n').map(|l| l.to_string()).collect();
    let entries = app_core::calc::scan_variable_assignments(&lines);
    if let Ok(mut index) = cross_note_var_index.lock() {
        index.register_note(&note.id, short_id);
        index.update_entries_only(short_id, &entries);
        index.mark_name_scan_attempted(short_id);
    }
    entries
}

/// Ensures the f64 export values for `short_id` are in the index by running a
/// full CalcEngine eval if not already done this session. Called from
/// `preload_cross_note_deps` before each recompute so ghost eval has values.
pub(super) fn preload_cross_note_dep_value(
    short_id: &str,
    cross_note_var_index: &Arc<Mutex<CrossNoteVarIndex>>,
    engine: &CalcEngine,
    db: &Db,
) {
    if let Ok(index) = cross_note_var_index.lock() {
        if index.was_full_eval_attempted(short_id) {
            return;
        }
    }

    let note_sources = app_core::note_sources::NoteSourceService::new(db.clone());
    let note = match note_sources.resolve_wiki_link_note(short_id) {
        Ok(Some(n)) => n,
        _ => {
            if let Ok(mut index) = cross_note_var_index.lock() {
                index.mark_full_eval_attempted(short_id);
            }
            return;
        }
    };
    let lines: Vec<String> = note.body.split('\n').map(|l| l.to_string()).collect();
    let result = engine.evaluate_note_context(
        &lines,
        NoteEvaluationOptions {
            variables_enabled: true,
            table_enabled: false,
            ..Default::default()
        },
    );
    if let Ok(mut index) = cross_note_var_index.lock() {
        index.register_note(&note.id, short_id);
        index.update_exports(short_id, &result.variables, &result.variable_values);
        index.mark_name_scan_attempted(short_id); // name scan implied by full eval
        index.mark_full_eval_attempted(short_id);
    }
}

/// Values for the `[[SHORTID]].var` references in `lines`, loading each
/// linked note on first use, so the first calc pass after opening a note
/// already shows cross-note results.
pub(super) fn startup_cross_note_extern_vars(
    db: &Db,
    engine: &CalcEngine,
    cross_note_var_index: &Arc<Mutex<CrossNoteVarIndex>>,
    note_id: &str,
    lines: &[String],
) -> Vec<ExternVar> {
    let refs = app_core::calc::scan_cross_note_refs(lines);
    if refs.is_empty() {
        return Vec::new();
    }
    let short_ids: rustc_hash::FxHashSet<&str> =
        refs.iter().map(|r| r.note_short_id.as_str()).collect();
    for short_id in short_ids {
        preload_cross_note_dep_value(short_id, cross_note_var_index, engine, db);
    }
    match cross_note_var_index.lock() {
        Ok(mut index) => {
            index.update_deps(note_id, &refs);
            index.extern_vars_for(note_id)
        }
        Err(_) => Vec::new(),
    }
}

/// If the text before `cursor_col` ends with `[[SHORTID]].partial`, return
/// `(short_id, from_col_of_partial, partial_query)`.
/// `from_col_of_partial` is the char index right after the dot.
/// Returns `(short_id, bracket_col, from_col, partial)`.
/// `bracket_col` is the char column of the opening `[[` — used to anchor the
/// autocomplete popup visually under the full `[[id]].` expression.
/// `from_col` is the start of the partial var name — used for text replacement.
pub(super) fn extract_cross_note_completion_prefix(
    line_text: &str,
    cursor_col: usize,
) -> Option<(String, usize, usize, String)> {
    use regex::Regex;
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"\[\[([A-Za-z0-9]{8})\]\]\.([A-Za-z0-9_][A-Za-z0-9_ ]*)?$").unwrap()
    });

    let chars: Vec<char> = line_text.chars().collect();
    let col = cursor_col.min(chars.len());
    let text_before: String = chars[..col].iter().collect();
    let m = re.captures(&text_before)?;
    let full_match_start_byte = m.get(0)?.start();
    let bracket_col = text_before[..full_match_start_byte].chars().count();

    let short_id = m.get(1)?.as_str().to_string();
    let partial_raw = m.get(2).map(|g| g.as_str()).unwrap_or("");
    let partial = partial_raw.trim().to_lowercase();

    let partial_chars = partial_raw.chars().count();
    let from_col = col.saturating_sub(partial_chars);

    Some((short_id, bracket_col, from_col, partial))
}
