use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Compute a deterministic 64-bit hash for a line. `DefaultHasher` uses
/// SipHash-1-3 with fixed keys, so the result is stable across calls within
/// the same process and across builds.
pub fn hash_line(line: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    line.hash(&mut hasher);
    hasher.finish()
}

pub fn hash_lines(lines: &[String]) -> Vec<u64> {
    lines.iter().map(|line| hash_line(line)).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalcSegment {
    pub expr: String,
    pub from_col: usize,
    pub to_col: usize,
    pub from_byte: usize,
    pub to_byte: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableFormulaSegment {
    /// Byte range of the formula expression inside the cell (excludes pipes/padding).
    pub from_byte: usize,
    pub to_byte: usize,
    /// Char range of the formula expression.
    pub from_char: usize,
    pub to_char: usize,
    /// Char range of the enclosing cell, including the leading and trailing pipes.
    /// `cell_left_pipe_char` is the column of the `|` to the left of the cell;
    /// `cell_right_pipe_char` is the column of the `|` to the right.
    pub cell_left_pipe_char: usize,
    pub cell_right_pipe_char: usize,
    /// 0-based cell index within the row.
    pub cell_index: usize,
    /// Builtin formula labels invoked inside this cell, in left-to-right order.
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalcTrailerRefresh {
    pub eq_byte_idx: usize,
    pub new_tail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineCalcResult {
    pub line_idx: usize,
    pub result: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncrementalCalcPlan {
    pub base_results: Vec<LineCalcResult>,
    pub eval_from: usize,
    pub eval_to: usize,
    pub eval_lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalcEvalScopeDecision {
    pub touches_any_assignment: bool,
    pub touches_builtin_formula: bool,
    pub can_use_partial: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitMarkerLoc {
    pub doc_pos: usize,
    pub line_idx: usize,
    pub offset_in_line: usize,
    pub last_literal: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalcRefreshChange {
    pub line_idx: usize,
    pub from: usize,
    pub to: usize,
    pub insert: String,
    pub new_literal: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CalcRefreshPlan {
    pub changes: Vec<CalcRefreshChange>,
    pub prune: Vec<usize>,
    pub synced_lines: Vec<usize>,
}

fn char_to_byte_idx(text: &str, char_idx: usize) -> usize {
    if char_idx == 0 {
        return 0;
    }
    text.char_indices()
        .nth(char_idx)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

fn count_chars(text: &str, upto_byte: usize) -> usize {
    text[..upto_byte.min(text.len())].chars().count()
}

fn consume_ascii_whitespace(bytes: &[u8], mut idx: usize) -> usize {
    while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
        idx += 1;
    }
    idx
}

fn parse_ordered_list_marker(bytes: &[u8], start: usize) -> Option<usize> {
    let len = bytes.len();
    let mut i = start;

    let digit_start = i;
    while i < len && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == digit_start || i >= len || bytes[i] != b'.' {
        return None;
    }

    i += 1;
    if i < len && bytes[i].is_ascii_digit() {
        while i < len {
            while i < len && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < len && bytes[i] == b'.' && i + 1 < len && bytes[i + 1].is_ascii_digit() {
                i += 1;
                continue;
            }
            break;
        }
    }

    if i >= len || !bytes[i].is_ascii_whitespace() {
        return None;
    }

    Some(consume_ascii_whitespace(bytes, i))
}

fn list_body_byte_range(line: &str) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut i = consume_ascii_whitespace(bytes, 0);
    if i >= len {
        return None;
    }

    if i + 2 <= len && &bytes[i..i + 2] == b"->" {
        let marker_end = i + 2;
        if marker_end >= len || !bytes[marker_end].is_ascii_whitespace() {
            return None;
        }
        i = consume_ascii_whitespace(bytes, marker_end);
    } else if matches!(bytes[i], b'-' | b'*' | b'+') {
        let marker_end = i + 1;
        if marker_end >= len || !bytes[marker_end].is_ascii_whitespace() {
            return None;
        }
        i = consume_ascii_whitespace(bytes, marker_end);
    } else if let Some(next) = parse_ordered_list_marker(bytes, i) {
        i = next;
    } else {
        return None;
    }

    if i + 3 < len
        && bytes[i] == b'['
        && (bytes[i + 1] == b' ' || bytes[i + 1] == b'x' || bytes[i + 1] == b'X')
        && bytes[i + 2] == b']'
        && bytes[i + 3].is_ascii_whitespace()
    {
        i = consume_ascii_whitespace(bytes, i + 3);
    }

    if i >= len {
        return None;
    }

    let mut end = len;
    while end > i && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if i >= end {
        return None;
    }

    Some((i, end))
}

fn split3(s: &str, delim: char) -> Option<(&str, &str, &str)> {
    let mut it = s.split(delim);
    let a = it.next()?;
    let b = it.next()?;
    let c = it.next()?;
    if it.next().is_some() {
        return None;
    }
    Some((a, b, c))
}

fn parse_u32_len(part: &str, min_len: usize, max_len: usize) -> Option<u32> {
    if part.len() < min_len || part.len() > max_len || !part.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    part.parse::<u32>().ok()
}

fn looks_like_date_with_delim(s: &str, delim: char) -> bool {
    let Some((a, b, c)) = split3(s, delim) else {
        return false;
    };

    if parse_u32_len(a, 4, 4).is_some() {
        if let (Some(month), Some(day)) = (parse_u32_len(b, 1, 2), parse_u32_len(c, 1, 2)) {
            if (1..=12).contains(&month) && (1..=31).contains(&day) {
                return true;
            }
        }
    }

    if parse_u32_len(c, 4, 4).is_some() {
        if let (Some(first), Some(second)) = (parse_u32_len(a, 1, 2), parse_u32_len(b, 1, 2)) {
            let dmy = (1..=31).contains(&first) && (1..=12).contains(&second);
            let mdy = (1..=12).contains(&first) && (1..=31).contains(&second);
            if dmy || mdy {
                return true;
            }
        }
    }

    false
}

fn looks_like_date(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed.contains(' ') {
        return false;
    }

    looks_like_date_with_delim(trimmed, '-')
        || looks_like_date_with_delim(trimmed, '.')
        || looks_like_date_with_delim(trimmed, '/')
}

pub fn builtin_formula_label(text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }

    let without_equals = text.strip_prefix('=').unwrap_or(text).trim();
    if without_equals.is_empty() {
        return None;
    }

    let compact = without_equals
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    let token = compact.strip_suffix("()").unwrap_or(compact.as_str());

    let normalized = match token {
        "sum_row" => "sum_row",
        "avg_row" => "avg_row",
        "sum_col" | "sum_column" => "sum_col",
        "avg_col" | "avg_column" => "avg_col",
        _ => return None,
    };

    Some(format!("{normalized}()"))
}

pub fn is_builtin_formula(text: &str) -> bool {
    builtin_formula_label(text).is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BuiltinFormulaCallSpan {
    start: usize,
    end: usize,
}

fn find_builtin_formula_call_spans(text: &str) -> Vec<BuiltinFormulaCallSpan> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut idx = 0usize;

    while idx < bytes.len() {
        if !bytes[idx].is_ascii_alphabetic() && bytes[idx] != b'=' {
            idx += 1;
            continue;
        }

        let mut start = idx;
        let mut cursor = idx;
        if bytes[cursor] == b'=' {
            if cursor > 0
                && (bytes[cursor - 1].is_ascii_alphanumeric()
                    || bytes[cursor - 1] == b'_'
                    || matches!(bytes[cursor - 1], b':' | b'!' | b'<' | b'>' | b'='))
            {
                idx += 1;
                continue;
            }
            cursor += 1;
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if cursor >= bytes.len() || !bytes[cursor].is_ascii_alphabetic() {
                idx += 1;
                continue;
            }
        } else {
            if cursor > 0
                && (bytes[cursor - 1].is_ascii_alphanumeric() || bytes[cursor - 1] == b'_')
            {
                idx += 1;
                continue;
            }
            start = cursor;
        }

        let ident_start = cursor;
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
        {
            cursor += 1;
        }

        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] != b'(' {
            idx = ident_start.saturating_add(1);
            continue;
        }
        cursor += 1;

        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] != b')' {
            idx = ident_start.saturating_add(1);
            continue;
        }
        cursor += 1;

        if cursor < bytes.len() && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
        {
            idx = ident_start.saturating_add(1);
            continue;
        }

        if builtin_formula_label(&text[start..cursor]).is_some() {
            spans.push(BuiltinFormulaCallSpan { start, end: cursor });
            idx = cursor;
            continue;
        }

        idx = ident_start.saturating_add(1);
    }

    spans
}

pub fn builtin_formula_labels_in_text(text: &str) -> Vec<String> {
    find_builtin_formula_call_spans(text)
        .into_iter()
        .filter_map(|span| builtin_formula_label(&text[span.start..span.end]))
        .collect()
}

fn has_numeric_or_space_math_neighbors(bytes: &[u8], idx: usize) -> bool {
    let left = idx.checked_sub(1).and_then(|i| bytes.get(i)).copied();
    let right = bytes.get(idx + 1).copied();
    let is_mathish_neighbor = |b: u8| {
        b.is_ascii_digit()
            || b.is_ascii_whitespace()
            || matches!(
                b,
                b'(' | b')' | b'=' | b'+' | b'-' | b'*' | b'/' | b'^' | b'%'
            )
    };
    left.is_some_and(is_mathish_neighbor) || right.is_some_and(is_mathish_neighbor)
}

pub fn has_calc_signal(text: &str) -> bool {
    let trimmed = text.trim();
    if is_builtin_formula(trimmed) {
        return true;
    }

    if looks_like_date(text) {
        return false;
    }

    let bytes = text.as_bytes();
    if bytes
        .iter()
        .any(|&b| matches!(b, b'+' | b'*' | b'^' | b'%' | b'('))
    {
        return true;
    }

    if bytes.iter().enumerate().any(|(idx, &b)| {
        matches!(b, b'-' | b'/') && has_numeric_or_space_math_neighbors(bytes, idx)
    }) {
        return true;
    }

    let has_digit = text.bytes().any(|b| b.is_ascii_digit());
    let has_alpha = text.bytes().any(|b| b.is_ascii_alphabetic());
    if has_digit && (text.contains(" to ") || text.contains(" in ")) {
        return true;
    }

    // Keep mixed digit+alpha shorthand like `5km`, but avoid prose such as
    // `Task 5 update` being interpreted as a formula.
    has_digit && has_alpha && !trimmed.contains(char::is_whitespace)
}

pub fn contains_assignment_operator(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() < 2 {
        return false;
    }

    for i in 0..bytes.len() - 1 {
        if bytes[i] != b':' || bytes[i + 1] != b'=' {
            continue;
        }
        if i > 0 && matches!(bytes[i - 1], b':' | b'!' | b'<' | b'>' | b'=') {
            continue;
        }
        if i + 2 < bytes.len() && bytes[i + 2] == b'=' {
            continue;
        }
        return true;
    }

    false
}

fn segment_from_byte_range(
    line: &str,
    expr: &str,
    from_byte: usize,
    to_byte: usize,
) -> CalcSegment {
    CalcSegment {
        expr: expr.to_string(),
        from_col: count_chars(line, from_byte),
        to_col: count_chars(line, to_byte),
        from_byte,
        to_byte,
    }
}

fn is_table_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.ends_with('|')
}

pub fn find_single_calc_table_cell(line: &str) -> Option<CalcSegment> {
    if !is_table_line(line) {
        return None;
    }

    let mut pipes = Vec::new();
    for (idx, b) in line.as_bytes().iter().enumerate() {
        if *b == b'|' {
            pipes.push(idx);
        }
    }
    if pipes.len() < 2 {
        return None;
    }

    let mut formula_candidates: Vec<CalcSegment> = Vec::new();
    let mut candidates: Vec<CalcSegment> = Vec::new();

    for pair in pipes.windows(2) {
        let start = pair[0] + 1;
        let end = pair[1];
        if start >= end {
            continue;
        }

        let raw = &line[start..end];
        let trimmed = raw.trim();
        if trimmed.is_empty() || !has_calc_signal(trimmed) {
            continue;
        }

        let leading_ws = raw.len() - raw.trim_start().len();
        let trailing_ws = raw.len() - raw.trim_end().len();
        let from_byte = start + leading_ws;
        let to_byte = end.saturating_sub(trailing_ws);
        if from_byte >= to_byte {
            continue;
        }

        let segment = segment_from_byte_range(line, trimmed, from_byte, to_byte);
        if is_builtin_formula(trimmed) {
            formula_candidates.push(segment);
        } else {
            candidates.push(segment);
        }
    }

    if formula_candidates.len() == 1 {
        return formula_candidates.into_iter().next();
    }
    if formula_candidates.len() > 1 {
        return None;
    }
    if candidates.len() != 1 {
        return None;
    }
    candidates.into_iter().next()
}

pub fn find_list_calc_segment(line: &str) -> Option<CalcSegment> {
    let (from_byte, to_byte) = list_body_byte_range(line)?;
    let expr = line[from_byte..to_byte].trim();
    if expr.is_empty() || !has_calc_signal(expr) {
        return None;
    }
    Some(segment_from_byte_range(line, expr, from_byte, to_byte))
}

pub fn find_calc_segment(line: &str) -> Option<CalcSegment> {
    find_single_calc_table_cell(line).or_else(|| find_list_calc_segment(line))
}

pub fn line_for_calc_evaluation(line: &str) -> String {
    if let Some(segment) = find_calc_segment(line) {
        return segment.expr;
    }

    if is_table_line(line) {
        return String::new();
    }

    if let Some((from_byte, to_byte)) = list_body_byte_range(line) {
        let value = line[from_byte..to_byte].trim();
        if has_calc_signal(value) {
            return value.to_string();
        }
        return String::new();
    }

    line.to_string()
}

pub fn line_uses_assignment_ghost_prefix(line: &str) -> bool {
    let eval_target = line_for_calc_evaluation(line);
    let trimmed = eval_target.trim();
    if !trimmed.is_empty() {
        return contains_assignment_operator(trimmed);
    }
    contains_assignment_operator(line)
}

pub fn contains_variable_assignment(lines: &[String]) -> bool {
    lines.iter().any(|line| contains_assignment_operator(line))
}

pub fn contains_builtin_formula(lines: &[String]) -> bool {
    lines.iter().any(|line| {
        let eval_target = line_for_calc_evaluation(line);
        let trimmed = eval_target.trim();
        !trimmed.is_empty() && !builtin_formula_labels_in_text(trimmed).is_empty()
    })
}

pub fn decide_eval_scope(
    eval_lines: &[String],
    prev_changed_lines: &[String],
    has_prev: bool,
    variables_enabled: bool,
) -> CalcEvalScopeDecision {
    let prev_changed_had_assignment = contains_variable_assignment(prev_changed_lines);
    let prev_changed_had_builtin_formula = contains_builtin_formula(prev_changed_lines);
    decide_eval_scope_with_flags(
        eval_lines,
        prev_changed_had_assignment,
        prev_changed_had_builtin_formula,
        has_prev,
        variables_enabled,
    )
}

pub fn decide_eval_scope_with_flags(
    eval_lines: &[String],
    prev_changed_had_assignment: bool,
    prev_changed_had_builtin_formula: bool,
    has_prev: bool,
    variables_enabled: bool,
) -> CalcEvalScopeDecision {
    let touches_any_assignment = variables_enabled
        && (contains_variable_assignment(eval_lines) || prev_changed_had_assignment);
    let touches_builtin_formula =
        contains_builtin_formula(eval_lines) || prev_changed_had_builtin_formula;
    let can_use_partial = has_prev && !touches_any_assignment && !touches_builtin_formula;
    CalcEvalScopeDecision {
        touches_any_assignment,
        touches_builtin_formula,
        can_use_partial,
    }
}

pub fn should_schedule_calc_eval(
    doc_line_count: usize,
    max_eval_lines: usize,
    has_global_syntax: bool,
    touches_calc_expression: bool,
    visible_has_calc_syntax: bool,
) -> bool {
    if doc_line_count > max_eval_lines {
        return false;
    }
    if has_global_syntax {
        return true;
    }
    if touches_calc_expression {
        return true;
    }
    visible_has_calc_syntax
}

pub fn should_attempt_calc_trailer_refresh(
    prev_line_hash: u64,
    next_line_hash: u64,
    prev_result: Option<&str>,
    line_is_selected: bool,
) -> bool {
    if line_is_selected {
        return false;
    }
    if prev_line_hash != next_line_hash {
        return false;
    }
    prev_result.is_none()
}

pub fn find_table_formula_segment(line: &str) -> Option<TableFormulaSegment> {
    find_table_formula_segments(line).into_iter().next()
}

/// Return every formula cell in the row, ordered left-to-right.
pub fn find_table_formula_segments(line: &str) -> Vec<TableFormulaSegment> {
    if !is_table_line(line) {
        return Vec::new();
    }

    let mut pipes = Vec::new();
    for (idx, b) in line.as_bytes().iter().enumerate() {
        if *b == b'|' {
            pipes.push(idx);
        }
    }
    if pipes.len() < 2 {
        return Vec::new();
    }

    let mut out = Vec::new();
    for (cell_index, pair) in pipes.windows(2).enumerate() {
        let left_pipe = pair[0];
        let right_pipe = pair[1];
        if right_pipe <= left_pipe + 1 {
            continue;
        }

        let cell_start = left_pipe + 1;
        let raw = &line[cell_start..right_pipe];
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }

        let labels = builtin_formula_labels_in_text(trimmed);
        if labels.is_empty() {
            continue;
        }

        let leading_ws = raw.len() - raw.trim_start().len();
        let trailing_ws = raw.len() - raw.trim_end().len();
        let from_byte = cell_start + leading_ws;
        let to_byte = right_pipe.saturating_sub(trailing_ws);
        if from_byte >= to_byte {
            continue;
        }

        out.push(TableFormulaSegment {
            from_byte,
            to_byte,
            from_char: count_chars(line, from_byte),
            to_char: count_chars(line, to_byte),
            cell_left_pipe_char: count_chars(line, left_pipe),
            cell_right_pipe_char: count_chars(line, right_pipe),
            cell_index,
            labels,
        });
    }

    out
}

pub fn format_formula_display_value(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut cleaned = trimmed.to_string();
    if let Some(rest) = cleaned.strip_prefix('≈') {
        cleaned = rest.trim_start().to_string();
    }
    if let Some(rest) = cleaned.strip_prefix('~') {
        cleaned = rest.trim_start().to_string();
    }
    let lowered = cleaned.to_ascii_lowercase();
    for prefix in ["approximately ", "approx. ", "approx ", "about "] {
        if lowered.starts_with(prefix) {
            cleaned = cleaned[prefix.len()..].trim_start().to_string();
            break;
        }
    }
    if cleaned.is_empty() {
        cleaned = trimmed.to_string();
    }

    let mut parts = cleaned.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim_start();
    let numeric = first.replace(',', "");

    let Ok(value) = numeric.parse::<f64>() else {
        return cleaned;
    };
    if !value.is_finite() {
        return cleaned;
    }

    let rounded = (value * 100.0).round() / 100.0;
    let mut out = format!("{rounded:.2}");
    while out.contains('.') && out.ends_with('0') {
        out.pop();
    }
    if out.ends_with('.') {
        out.pop();
    }

    if rest.is_empty() {
        out
    } else {
        format!("{out} {rest}")
    }
}

pub fn compute_calc_trailer_refresh(
    line: &str,
    new_result: &str,
    is_cursor_line: bool,
    cursor_col: usize,
) -> Option<CalcTrailerRefresh> {
    let eq_idx = line.rfind(" = ")?;
    let current_literal = &line[eq_idx + 3..];
    if current_literal == new_result {
        return None;
    }
    if is_cursor_line {
        let eq_char_idx = line[..eq_idx].chars().count();
        if cursor_col >= eq_char_idx {
            return None;
        }
    }
    Some(CalcTrailerRefresh {
        eq_byte_idx: eq_idx,
        new_tail: format!(" = {new_result}"),
    })
}

fn shared_prefix_len_hashed(a: &[u64], b: &[u64]) -> usize {
    let max = a.len().min(b.len());
    let mut i = 0;
    while i < max && a[i] == b[i] {
        i += 1;
    }
    i
}

fn shared_suffix_len_hashed(a: &[u64], b: &[u64], prefix_len: usize) -> usize {
    let max = a.len().min(b.len()).saturating_sub(prefix_len);
    let mut i = 0;
    while i < max && a[a.len() - 1 - i] == b[b.len() - 1 - i] {
        i += 1;
    }
    i
}

pub fn plan_incremental_calc_from_hashes(
    prev_hashes: &[u64],
    prev_results: &[Option<String>],
    next_lines: &[String],
    next_hashes: &[u64],
) -> IncrementalCalcPlan {
    if prev_hashes.is_empty() {
        return IncrementalCalcPlan {
            base_results: Vec::new(),
            eval_from: 0,
            eval_to: next_lines.len(),
            eval_lines: next_lines.to_vec(),
        };
    }

    let prefix = shared_prefix_len_hashed(prev_hashes, next_hashes);
    let suffix = shared_suffix_len_hashed(prev_hashes, next_hashes, prefix);
    let next_len = next_lines.len();
    let prev_len = prev_hashes.len();
    let changed_from = prefix;
    let changed_to = next_len.saturating_sub(suffix);

    let mut base_results: Vec<LineCalcResult> = Vec::new();

    for i in 0..prefix {
        if let Some(Some(result)) = prev_results.get(i) {
            base_results.push(LineCalcResult {
                line_idx: i,
                result: result.clone(),
            });
        }
    }

    for i in 0..suffix {
        let prev_idx = prev_len - suffix + i;
        let next_idx = next_len - suffix + i;
        if let Some(Some(result)) = prev_results.get(prev_idx) {
            base_results.push(LineCalcResult {
                line_idx: next_idx,
                result: result.clone(),
            });
        }
    }

    IncrementalCalcPlan {
        base_results,
        eval_from: changed_from,
        eval_to: changed_to,
        eval_lines: next_lines[changed_from..changed_to].to_vec(),
    }
}

pub fn plan_incremental_calc(
    prev_lines: &[String],
    prev_results: &[Option<String>],
    next_lines: &[String],
) -> IncrementalCalcPlan {
    let prev_hashes = hash_lines(prev_lines);
    let next_hashes = hash_lines(next_lines);
    plan_incremental_calc_from_hashes(&prev_hashes, prev_results, next_lines, &next_hashes)
}

fn line_slice_by_char(text: &str, from_col: usize, to_col: usize) -> String {
    let from = char_to_byte_idx(text, from_col);
    let to = char_to_byte_idx(text, to_col.max(from_col));
    text[from..to].to_string()
}

pub fn compute_calc_refresh(
    markers: &[CommitMarkerLoc],
    lines: &[String],
    line_starts: &[usize],
    next_results: &[Option<String>],
    selection_from: usize,
    selection_to: usize,
) -> CalcRefreshPlan {
    let mut changes = Vec::new();
    let mut prune = Vec::new();
    let mut synced_lines = Vec::new();

    for marker in markers {
        let Some(line_text) = lines.get(marker.line_idx) else {
            prune.push(marker.doc_pos);
            continue;
        };

        if line_slice_by_char(line_text, marker.offset_in_line, marker.offset_in_line + 3) != " = "
        {
            prune.push(marker.doc_pos);
            continue;
        }

        let current_literal = line_slice_by_char(line_text, marker.offset_in_line + 3, usize::MAX);
        if current_literal != marker.last_literal {
            prune.push(marker.doc_pos);
            continue;
        }

        let Some(next_result) = next_results
            .get(marker.line_idx)
            .and_then(|value| value.as_ref())
        else {
            continue;
        };

        if current_literal == *next_result {
            synced_lines.push(marker.line_idx);
            continue;
        }

        let line_start = *line_starts.get(marker.line_idx).unwrap_or(&0);
        let trailer_from = line_start + marker.offset_in_line;
        let trailer_to = line_start + line_text.chars().count();
        if selection_from <= trailer_to && selection_to >= trailer_from {
            continue;
        }

        changes.push(CalcRefreshChange {
            line_idx: marker.line_idx,
            from: trailer_from,
            to: trailer_to,
            insert: format!(" = {next_result}"),
            new_literal: next_result.clone(),
        });
        synced_lines.push(marker.line_idx);
    }

    CalcRefreshPlan {
        changes,
        prune,
        synced_lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_formula_segment_detects_avg_col() {
        let line = "| dsad | dsdsd | =avg_col() | 1.91 | |";
        let seg = find_table_formula_segment(line).expect("formula");
        assert_eq!(seg.labels, vec!["avg_col()".to_string()]);
        assert_eq!(&line[seg.from_byte..seg.to_byte], "=avg_col()");
        assert_eq!(seg.cell_index, 2);
    }

    #[test]
    fn find_table_formula_segments_returns_each_formula_cell_left_to_right() {
        let line = "| =sum_col() | x | =sum_row() |";
        let segments = find_table_formula_segments(line);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].labels, vec!["sum_col()".to_string()]);
        assert_eq!(segments[0].cell_index, 0);
        assert_eq!(segments[1].labels, vec!["sum_row()".to_string()]);
        assert_eq!(segments[1].cell_index, 2);
    }

    #[test]
    fn list_body_calc_segment_detects_checklist() {
        let line = "- [ ]  2+2   ";
        let seg = find_list_calc_segment(line).expect("segment");
        assert_eq!(seg.expr, "2+2");
    }

    #[test]
    fn list_body_calc_segment_ignores_ordered_list_prose_with_hyphenated_words_and_slashes() {
        let line = "5. Dedup: Unify the text-object methods, undo/redo, and VimIntent line-range";
        assert!(find_list_calc_segment(line).is_none());
        assert_eq!(line_for_calc_evaluation(line), "");
    }

    #[test]
    fn list_body_calc_segment_keeps_division_expressions() {
        let line = "- total / 2";
        let seg = find_list_calc_segment(line).expect("segment");
        assert_eq!(seg.expr, "total / 2");
    }

    #[test]
    fn builtin_formula_labels_in_text_detects_multiple_calls_in_expression_order() {
        let labels = builtin_formula_labels_in_text("sum_col() * a + avg_col() - sum_row()");
        assert_eq!(
            labels,
            vec![
                "sum_col()".to_string(),
                "avg_col()".to_string(),
                "sum_row()".to_string()
            ]
        );
    }

    #[test]
    fn contains_builtin_formula_detects_formula_calls_inside_expressions() {
        let lines = vec![
            "| value |".to_string(),
            "| --- |".to_string(),
            "| sum_col() * a + 5 |".to_string(),
        ];
        assert!(contains_builtin_formula(&lines));
    }

    #[test]
    fn find_table_formula_segment_detects_chained_formula_expression_cell() {
        let line = "| sum_col() * a + avg_col() |";
        let seg = find_table_formula_segment(line).expect("formula");
        assert_eq!(seg.labels, vec!["sum_col()", "avg_col()"]);
        assert_eq!(
            &line[seg.from_byte..seg.to_byte],
            "sum_col() * a + avg_col()"
        );
    }

    #[test]
    fn line_eval_target_prefers_segment() {
        assert_eq!(
            line_for_calc_evaluation("| name | =avg_col() |"),
            "=avg_col()".to_string()
        );
        assert_eq!(line_for_calc_evaluation("| a | b |"), "".to_string());
    }

    #[test]
    fn line_eval_target_ignores_plain_numeric_list_and_table_literals() {
        assert_eq!(line_for_calc_evaluation("- [x] 6"), "");
        assert_eq!(line_for_calc_evaluation("- 6"), "");
        assert_eq!(line_for_calc_evaluation("| label | 6 |"), "");
    }

    #[test]
    fn trailer_refresh_respects_cursor_guard() {
        let refresh = compute_calc_trailer_refresh("2+2 = 4", "5", true, 5);
        assert!(refresh.is_none());
    }

    #[test]
    fn incremental_plan_reuses_prefix_and_suffix() {
        let prev = vec!["1+1".to_string(), "2+2".to_string(), "3+3".to_string()];
        let next = vec!["1+1".to_string(), "20+2".to_string(), "3+3".to_string()];
        let prev_results = vec![
            Some("2".to_string()),
            Some("4".to_string()),
            Some("6".to_string()),
        ];
        let plan = plan_incremental_calc(&prev, &prev_results, &next);
        assert_eq!(plan.eval_from, 1);
        assert_eq!(plan.eval_to, 2);
        assert_eq!(plan.eval_lines, vec!["20+2".to_string()]);
        assert_eq!(plan.base_results.len(), 2);
    }

    #[test]
    fn format_formula_value_rounds_and_drops_approx_prefixes() {
        assert_eq!(format_formula_display_value("≈ 12.345"), "12.35");
        assert_eq!(format_formula_display_value("about 5.0 kg"), "5 kg");
    }

    #[test]
    fn decide_eval_scope_prefers_partial_only_without_global_dependencies() {
        let eval_lines = vec!["2 + 2".to_string()];
        let prev_changed = vec!["plain text".to_string()];
        let decision = decide_eval_scope(&eval_lines, &prev_changed, true, true);
        assert!(decision.can_use_partial);
        assert!(!decision.touches_any_assignment);
        assert!(!decision.touches_builtin_formula);
    }

    #[test]
    fn decide_eval_scope_detects_assignment_and_formula_dependencies() {
        let eval_assignment = vec!["a := 2 + 2".to_string()];
        let from_assignment =
            decide_eval_scope_with_flags(&eval_assignment, false, false, true, true);
        assert!(from_assignment.touches_any_assignment);
        assert!(!from_assignment.can_use_partial);

        let eval_formula = vec!["sum_col() + 1".to_string()];
        let from_formula = decide_eval_scope_with_flags(&eval_formula, false, false, true, true);
        assert!(from_formula.touches_builtin_formula);
        assert!(!from_formula.can_use_partial);
    }

    #[test]
    fn should_schedule_calc_eval_follows_shared_trigger_rules() {
        assert!(should_schedule_calc_eval(10, 100, true, false, false));
        assert!(should_schedule_calc_eval(10, 100, false, true, false));
        assert!(should_schedule_calc_eval(10, 100, false, false, true));
        assert!(!should_schedule_calc_eval(200, 100, true, true, true));
        assert!(!should_schedule_calc_eval(10, 100, false, false, false));
    }

    #[test]
    fn should_attempt_calc_trailer_refresh_requires_unselected_synced_prev_none_line() {
        assert!(should_attempt_calc_trailer_refresh(1, 1, None, false));
        assert!(!should_attempt_calc_trailer_refresh(1, 2, None, false));
        assert!(!should_attempt_calc_trailer_refresh(1, 1, Some("4"), false));
        assert!(!should_attempt_calc_trailer_refresh(1, 1, None, true));
    }
}
