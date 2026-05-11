use crate::table;
use regex::Regex;
use rustc_hash::{FxHashMap, FxHashSet, FxHasher};
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

/// Compute a deterministic 64-bit hash for a line using FxHasher — a fast,
/// non-cryptographic hasher that is stable within a process run.
pub fn hash_line(line: &str) -> u64 {
    let mut hasher = FxHasher::default();
    line.hash(&mut hasher);
    hasher.finish()
}

pub fn hash_lines(lines: &[String]) -> Vec<u64> {
    lines.iter().map(|line| hash_line(line)).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineMetadata {
    pub hash: u64,
    pub assignment_name: Option<String>,
    pub has_assignment: bool,
    pub has_builtin_formula: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CalcSignalFlags {
    pub has_variable_assignment: bool,
    pub has_builtin_formula: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalcFeatureMask {
    pub math_enabled: bool,
    pub table_enabled: bool,
    pub variables_enabled: bool,
}

impl Default for CalcFeatureMask {
    fn default() -> Self {
        Self {
            math_enabled: true,
            table_enabled: true,
            variables_enabled: true,
        }
    }
}

impl CalcFeatureMask {
    fn variables_active(self) -> bool {
        self.math_enabled && self.variables_enabled
    }

    fn table_active(self) -> bool {
        self.math_enabled && self.table_enabled
    }
}

pub fn line_metadata(line: &String) -> LineMetadata {
    line_metadata_with_mask(line, CalcFeatureMask::default())
}

pub fn line_metadata_with_mask(line: &String, mask: CalcFeatureMask) -> LineMetadata {
    LineMetadata {
        hash: hash_line(line),
        assignment_name: assignment_name_with_mask(line, mask),
        has_assignment: contains_variable_assignment_with_mask(std::slice::from_ref(line), mask),
        has_builtin_formula: line_has_builtin_formula_with_mask(line, mask),
    }
}

pub fn line_metadata_for_lines(lines: &[String]) -> Vec<LineMetadata> {
    line_metadata_for_lines_with_mask(lines, CalcFeatureMask::default())
}

pub fn line_metadata_for_lines_with_mask(
    lines: &[String],
    mask: CalcFeatureMask,
) -> Vec<LineMetadata> {
    lines
        .iter()
        .map(|line| LineMetadata {
            hash: hash_line(line),
            assignment_name: assignment_name_with_mask(line, mask),
            has_assignment: contains_variable_assignment_with_mask(
                std::slice::from_ref(line),
                mask,
            ),
            has_builtin_formula: line_has_builtin_formula_with_mask(line, mask),
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalcSegment {
    pub expr: String,
    pub from_col: usize,
    pub to_col: usize,
    pub from_byte: usize,
    pub to_byte: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
pub struct LineCalcResult {
    pub line_idx: usize,
    pub result: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncrementalCalcPlan {
    pub base_results: Vec<LineCalcResult>,
    pub eval_from: usize,
    pub eval_to: usize,
    pub eval_lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalcEvalScopeDecision {
    pub touches_any_assignment: bool,
    pub touches_builtin_formula: bool,
    pub can_use_partial: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalcEvalWindowDecision {
    pub eval_from: usize,
    pub eval_to: usize,
    pub touches_any_assignment: bool,
    pub touches_builtin_formula: bool,
    pub can_use_partial: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitMarkerLoc {
    pub doc_pos: usize,
    pub line_idx: usize,
    pub offset_in_line: usize,
    pub last_literal: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalcRefreshChange {
    pub line_idx: usize,
    pub from: usize,
    pub to: usize,
    pub insert: String,
    pub new_literal: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
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

    let without_prefix = text
        .strip_prefix(":=")
        .or_else(|| text.strip_prefix('='))
        .unwrap_or(text)
        .trim();
    if without_prefix.is_empty() {
        return None;
    }

    let compact = without_prefix
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
        // Require a word character (alphanumeric or _) before := so that a
        // leading `:=` table formula prefix is not mistaken for an assignment.
        let has_name_before = (0..i)
            .rev()
            .find(|&j| !bytes[j].is_ascii_whitespace())
            .map(|j| bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_')
            .unwrap_or(false);
        if !has_name_before {
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
    table::is_table_line(line)
}

fn find_single_calc_table_cell_range(line: &str) -> Option<(usize, usize)> {
    if !is_table_line(line) {
        return None;
    }

    let pipes = table::table_pipe_positions(line);
    if pipes.len() < 2 {
        return None;
    }

    let mut formula_candidates: Vec<(usize, usize)> = Vec::new();
    let mut candidates: Vec<(usize, usize)> = Vec::new();

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

        // A cell starting with := (with non-empty content after) is an
        // explicit formula cell — treat it like a builtin formula for the
        // purpose of detection priority.
        let is_colon_eq_formula = trimmed
            .strip_prefix(":=")
            .map(|rest| !rest.trim_start().is_empty())
            .unwrap_or(false);

        if is_builtin_formula(trimmed) || is_colon_eq_formula {
            formula_candidates.push((from_byte, to_byte));
        } else if has_calc_signal(trimmed) {
            candidates.push((from_byte, to_byte));
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

pub fn find_single_calc_table_cell(line: &str) -> Option<CalcSegment> {
    let (from_byte, to_byte) = find_single_calc_table_cell_range(line)?;
    let expr = line[from_byte..to_byte].trim();
    Some(segment_from_byte_range(line, expr, from_byte, to_byte))
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

fn line_for_calc_evaluation_slice_with_mask(line: &str, mask: CalcFeatureMask) -> Option<&str> {
    if !mask.math_enabled {
        return None;
    }

    if is_table_line(line) && !mask.table_active() {
        return None;
    }

    if let Some((from_byte, to_byte)) = find_single_calc_table_cell_range(line) {
        return Some(line[from_byte..to_byte].trim());
    }

    if is_table_line(line) {
        return None;
    }

    if let Some((from_byte, to_byte)) = list_body_byte_range(line) {
        let value = line[from_byte..to_byte].trim();
        if has_calc_signal(value) {
            return Some(value);
        }
        return None;
    }

    Some(line)
}

fn line_has_builtin_formula_with_mask(line: &str, mask: CalcFeatureMask) -> bool {
    let Some(eval_target) = line_for_calc_evaluation_slice_with_mask(line, mask) else {
        return false;
    };
    let trimmed = eval_target.trim();
    !trimmed.is_empty() && !builtin_formula_labels_in_text(trimmed).is_empty()
}

pub fn line_for_calc_evaluation(line: &str) -> String {
    line_for_calc_evaluation_with_mask(line, CalcFeatureMask::default())
}

pub fn line_for_calc_evaluation_with_mask(line: &str, mask: CalcFeatureMask) -> String {
    if let Some(eval_target) = line_for_calc_evaluation_slice_with_mask(line, mask) {
        return eval_target.to_string();
    }
    String::new()
}

pub fn line_uses_assignment_ghost_prefix(line: &str) -> bool {
    if let Some(eval_target) =
        line_for_calc_evaluation_slice_with_mask(line, CalcFeatureMask::default())
    {
        let trimmed = eval_target.trim();
        if !trimmed.is_empty() {
            return contains_assignment_operator(trimmed);
        }
    }
    contains_assignment_operator(line)
}

pub fn contains_variable_assignment(lines: &[String]) -> bool {
    contains_variable_assignment_with_mask(lines, CalcFeatureMask::default())
}

pub fn contains_variable_assignment_with_mask(lines: &[String], mask: CalcFeatureMask) -> bool {
    if !mask.variables_active() {
        return false;
    }

    lines.iter().any(|line| {
        if is_table_line(line) && !mask.table_active() {
            return false;
        }
        contains_assignment_operator(line)
    })
}

pub fn contains_builtin_formula(lines: &[String]) -> bool {
    contains_builtin_formula_with_mask(lines, CalcFeatureMask::default())
}

pub fn contains_builtin_formula_with_mask(lines: &[String], mask: CalcFeatureMask) -> bool {
    lines
        .iter()
        .any(|line| line_has_builtin_formula_with_mask(line, mask))
}

pub fn detect_calc_signal_flags(lines: &[String]) -> CalcSignalFlags {
    detect_calc_signal_flags_with_mask(lines, CalcFeatureMask::default())
}

pub fn detect_calc_signal_flags_with_mask(
    lines: &[String],
    mask: CalcFeatureMask,
) -> CalcSignalFlags {
    if !mask.math_enabled {
        return CalcSignalFlags::default();
    }

    let mut flags = CalcSignalFlags::default();
    for line in lines {
        if !flags.has_variable_assignment
            && contains_variable_assignment_with_mask(std::slice::from_ref(line), mask)
        {
            flags.has_variable_assignment = true;
        }
        if !flags.has_builtin_formula && line_has_builtin_formula_with_mask(line, mask) {
            flags.has_builtin_formula = true;
        }
        if flags.has_variable_assignment && flags.has_builtin_formula {
            break;
        }
    }
    flags
}

pub fn decide_eval_scope(
    eval_lines: &[String],
    prev_changed_lines: &[String],
    has_prev: bool,
    variables_enabled: bool,
) -> CalcEvalScopeDecision {
    decide_eval_scope_with_mask(
        eval_lines,
        prev_changed_lines,
        has_prev,
        CalcFeatureMask {
            math_enabled: true,
            table_enabled: true,
            variables_enabled,
        },
    )
}

pub fn decide_eval_scope_with_mask(
    eval_lines: &[String],
    prev_changed_lines: &[String],
    has_prev: bool,
    mask: CalcFeatureMask,
) -> CalcEvalScopeDecision {
    let prev_changed_had_assignment =
        contains_variable_assignment_with_mask(prev_changed_lines, mask);
    let prev_changed_had_builtin_formula =
        contains_builtin_formula_with_mask(prev_changed_lines, mask);
    let touches_any_assignment = mask.variables_active()
        && (contains_variable_assignment_with_mask(eval_lines, mask)
            || prev_changed_had_assignment);
    let touches_builtin_formula =
        contains_builtin_formula_with_mask(eval_lines, mask) || prev_changed_had_builtin_formula;
    let can_use_partial = has_prev && !touches_any_assignment && !touches_builtin_formula;
    CalcEvalScopeDecision {
        touches_any_assignment,
        touches_builtin_formula,
        can_use_partial,
    }
}

pub fn decide_eval_scope_with_flags(
    eval_lines: &[String],
    prev_changed_had_assignment: bool,
    prev_changed_had_builtin_formula: bool,
    has_prev: bool,
    variables_enabled: bool,
) -> CalcEvalScopeDecision {
    let mask = CalcFeatureMask {
        math_enabled: true,
        table_enabled: true,
        variables_enabled,
    };
    let touches_any_assignment = mask.variables_active()
        && (contains_variable_assignment_with_mask(eval_lines, mask)
            || prev_changed_had_assignment);
    let touches_builtin_formula =
        contains_builtin_formula_with_mask(eval_lines, mask) || prev_changed_had_builtin_formula;
    let can_use_partial = has_prev && !touches_any_assignment && !touches_builtin_formula;
    CalcEvalScopeDecision {
        touches_any_assignment,
        touches_builtin_formula,
        can_use_partial,
    }
}

fn collapse_spaces(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_valid_variable_name(name: &str) -> bool {
    let mut has_word = false;
    let mut has_alpha_or_underscore = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            has_word = true;
            if ch.is_ascii_alphabetic() || ch == '_' {
                has_alpha_or_underscore = true;
            }
            continue;
        }
        if ch == ' ' {
            continue;
        }
        return false;
    }
    has_word && has_alpha_or_underscore
}

fn strip_applied_result_tail(input: &str) -> &str {
    if let Some(idx) = input.rfind(" = ") {
        let left = input[..idx].trim();
        if !left.is_empty() {
            return left;
        }
    }
    input
}

fn parse_variable_assignment_name_rhs(input: &str) -> Option<(String, String)> {
    let idx = input.find(":=")?;
    let left = collapse_spaces(input[..idx].trim());
    if left.is_empty() || !is_valid_variable_name(&left) {
        return None;
    }
    let right = strip_applied_result_tail(input[idx + 2..].trim())
        .trim()
        .to_string();
    if right.is_empty() {
        return None;
    }
    Some((left.to_ascii_lowercase(), right))
}

pub fn assignment_name(line: &str) -> Option<String> {
    assignment_name_with_mask(line, CalcFeatureMask::default())
}

pub fn assignment_name_with_mask(line: &str, mask: CalcFeatureMask) -> Option<String> {
    if !mask.variables_active() {
        return None;
    }

    if is_table_line(line) && !mask.table_active() {
        return None;
    }

    if let Some(eval_target) = line_for_calc_evaluation_slice_with_mask(line, mask) {
        if let Some((name, _)) = parse_variable_assignment_name_rhs(eval_target.trim()) {
            return Some(name);
        }
    }

    if mask.table_active() && is_table_line(line) {
        let mut pipes = Vec::new();
        for (idx, b) in line.as_bytes().iter().enumerate() {
            if *b == b'|' {
                pipes.push(idx);
            }
        }
        for pair in pipes.windows(2) {
            let start = pair[0] + 1;
            let end = pair[1];
            if start >= end {
                continue;
            }
            let candidate = line[start..end].trim();
            if let Some((name, _)) = parse_variable_assignment_name_rhs(candidate) {
                return Some(name);
            }
        }
    }

    if let Some((from_byte, to_byte)) = list_body_byte_range(line) {
        let candidate = line[from_byte..to_byte].trim();
        if let Some((name, _)) = parse_variable_assignment_name_rhs(candidate) {
            return Some(name);
        }
    }

    parse_variable_assignment_name_rhs(line.trim()).map(|(name, _)| name)
}

pub fn collect_assignment_names(lines: &[String]) -> Vec<String> {
    collect_assignment_names_with_mask(lines, CalcFeatureMask::default())
}

pub fn collect_assignment_names_with_mask(lines: &[String], mask: CalcFeatureMask) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: FxHashSet<String> = FxHashSet::default();
    for line in lines {
        let Some(name) = assignment_name_with_mask(line, mask) else {
            continue;
        };
        if seen.insert(name.clone()) {
            out.push(name);
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct VariableDependencyGraph {
    assignment_name_by_line: Vec<Option<String>>,
    assignment_rhs_refs_by_line: Vec<FxHashSet<String>>,
    assignment_line_by_name: FxHashMap<String, usize>,
    variable_dependents: FxHashMap<String, FxHashSet<String>>,
    variable_consumer_lines: FxHashMap<String, FxHashSet<usize>>,
    line_variable_refs: Vec<FxHashSet<String>>,
    has_duplicate_assignment_names: bool,
}

fn collect_identifier_refs(text: &str) -> FxHashSet<String> {
    let bytes = text.as_bytes();
    let mut out = FxHashSet::default();
    let mut idx = 0usize;
    while idx < bytes.len() {
        let ch = bytes[idx];
        let starts_ident = ch.is_ascii_alphabetic() || ch == b'_';
        if !starts_ident {
            idx += 1;
            continue;
        }
        let start = idx;
        idx += 1;
        while idx < bytes.len() {
            let next = bytes[idx];
            if next.is_ascii_alphanumeric() || next == b'_' {
                idx += 1;
            } else {
                break;
            }
        }
        out.insert(text[start..idx].to_ascii_lowercase());
    }
    out
}

fn line_assignment_def(line: &str, mask: CalcFeatureMask) -> (Option<String>, FxHashSet<String>) {
    let Some(eval_target) = line_for_calc_evaluation_slice_with_mask(line, mask) else {
        return (None, FxHashSet::default());
    };
    let Some((name, rhs)) = parse_variable_assignment_name_rhs(eval_target.trim()) else {
        return (None, FxHashSet::default());
    };
    let refs = collect_identifier_refs(rhs.as_str())
        .into_iter()
        .filter(|candidate| candidate != &name)
        .collect::<FxHashSet<_>>();
    (Some(name), refs)
}

fn line_variable_refs(line: &str, mask: CalcFeatureMask) -> FxHashSet<String> {
    if mask.table_active() && is_table_line(line) {
        let formula_segments = find_table_formula_segments(line);
        if !formula_segments.is_empty() {
            let mut refs = FxHashSet::default();
            for segment in formula_segments {
                let segment_text = line[segment.from_byte..segment.to_byte].trim();
                if segment_text.is_empty() {
                    continue;
                }
                let assignment_rhs =
                    parse_variable_assignment_name_rhs(segment_text).map(|(_, rhs)| rhs);
                let expression_for_refs = assignment_rhs.as_deref().unwrap_or(segment_text);
                refs.extend(collect_identifier_refs(expression_for_refs));
            }
            return refs;
        }
    }

    let eval_target = line_for_calc_evaluation_with_mask(line, mask);
    let trimmed = eval_target.trim();
    if trimmed.is_empty() {
        return FxHashSet::default();
    }
    let assignment_rhs = parse_variable_assignment_name_rhs(trimmed).map(|(_, rhs)| rhs);
    let expression_for_refs = assignment_rhs.as_deref().unwrap_or(trimmed);
    collect_identifier_refs(expression_for_refs)
}

fn rebuild_dependency_maps(graph: &mut VariableDependencyGraph) {
    graph.assignment_line_by_name.clear();
    graph.variable_dependents.clear();
    graph.variable_consumer_lines.clear();
    graph.has_duplicate_assignment_names = false;

    let mut assignment_name_counts: FxHashMap<String, usize> = FxHashMap::default();
    for (line_idx, maybe_name) in graph.assignment_name_by_line.iter().enumerate() {
        let Some(name) = maybe_name.as_ref() else {
            continue;
        };
        let count = assignment_name_counts.entry(name.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            graph.has_duplicate_assignment_names = true;
        }
        graph.assignment_line_by_name.insert(name.clone(), line_idx);
    }

    for (name, line_idx) in &graph.assignment_line_by_name {
        let refs = graph
            .assignment_rhs_refs_by_line
            .get(*line_idx)
            .cloned()
            .unwrap_or_default();
        for referenced in refs {
            if referenced == *name {
                continue;
            }
            graph
                .variable_dependents
                .entry(referenced)
                .or_default()
                .insert(name.clone());
        }
    }

    for (line_idx, refs) in graph.line_variable_refs.iter().enumerate() {
        for referenced in refs {
            graph
                .variable_consumer_lines
                .entry(referenced.clone())
                .or_default()
                .insert(line_idx);
        }
    }
}

fn add_dependency_edges(
    graph: &mut VariableDependencyGraph,
    dependent: &str,
    refs: &FxHashSet<String>,
) {
    for referenced in refs {
        if referenced == dependent {
            continue;
        }
        graph
            .variable_dependents
            .entry(referenced.clone())
            .or_default()
            .insert(dependent.to_string());
    }
}

fn remove_dependency_edges(
    graph: &mut VariableDependencyGraph,
    dependent: &str,
    refs: &FxHashSet<String>,
) {
    for referenced in refs {
        let mut empty = false;
        if let Some(dependents) = graph.variable_dependents.get_mut(referenced) {
            dependents.remove(dependent);
            empty = dependents.is_empty();
        }
        if empty {
            graph.variable_dependents.remove(referenced);
        }
    }
}

fn add_line_variable_refs(
    graph: &mut VariableDependencyGraph,
    line_idx: usize,
    refs: &FxHashSet<String>,
) {
    for referenced in refs {
        graph
            .variable_consumer_lines
            .entry(referenced.clone())
            .or_default()
            .insert(line_idx);
    }
}

fn remove_line_variable_refs(
    graph: &mut VariableDependencyGraph,
    line_idx: usize,
    refs: &FxHashSet<String>,
) {
    for referenced in refs {
        let mut empty = false;
        if let Some(consumers) = graph.variable_consumer_lines.get_mut(referenced) {
            consumers.remove(&line_idx);
            empty = consumers.is_empty();
        }
        if empty {
            graph.variable_consumer_lines.remove(referenced);
        }
    }
}

fn try_patch_variable_dependency_graph_in_place(
    graph: &mut VariableDependencyGraph,
    lines: &[String],
    changed_from: usize,
    changed_to: usize,
    mask: CalcFeatureMask,
) -> bool {
    if graph.has_duplicate_assignment_names
        || graph.assignment_name_by_line.len() != lines.len()
        || graph.assignment_rhs_refs_by_line.len() != lines.len()
        || graph.line_variable_refs.len() != lines.len()
    {
        return false;
    }

    let from = changed_from.min(lines.len());
    let to = changed_to.min(lines.len()).max(from);
    if from == to {
        return true;
    }

    for line_idx in from..to {
        let Some(line) = lines.get(line_idx) else {
            return false;
        };

        let prev_name = graph.assignment_name_by_line[line_idx].clone();
        let prev_rhs_refs = graph.assignment_rhs_refs_by_line[line_idx].clone();
        let prev_line_refs = graph.line_variable_refs[line_idx].clone();

        if let Some(name) = prev_name.as_ref() {
            remove_dependency_edges(graph, name, &prev_rhs_refs);
            graph.assignment_line_by_name.remove(name);
        }
        remove_line_variable_refs(graph, line_idx, &prev_line_refs);

        let (next_name, next_rhs_refs) = line_assignment_def(line, mask);
        let next_line_refs = line_variable_refs(line, mask);

        if let Some(name) = next_name.as_ref() {
            if let Some(existing_line) = graph.assignment_line_by_name.get(name).copied() {
                if existing_line != line_idx {
                    return false;
                }
            }
            graph.assignment_line_by_name.insert(name.clone(), line_idx);
            add_dependency_edges(graph, name, &next_rhs_refs);
        }
        add_line_variable_refs(graph, line_idx, &next_line_refs);

        graph.assignment_name_by_line[line_idx] = next_name;
        graph.assignment_rhs_refs_by_line[line_idx] = next_rhs_refs;
        graph.line_variable_refs[line_idx] = next_line_refs;
    }

    true
}

pub fn build_variable_dependency_graph(
    lines: &[String],
    mask: CalcFeatureMask,
) -> Option<VariableDependencyGraph> {
    if !mask.variables_active() {
        return None;
    }

    let mut assignment_name_by_line = vec![None; lines.len()];
    let mut assignment_rhs_refs_by_line: Vec<FxHashSet<String>> =
        vec![FxHashSet::default(); lines.len()];
    let mut line_variable_refs_cache: Vec<FxHashSet<String>> =
        vec![FxHashSet::default(); lines.len()];
    for (line_idx, line) in lines.iter().enumerate() {
        let (name, rhs_refs) = line_assignment_def(line, mask);
        assignment_name_by_line[line_idx] = name;
        assignment_rhs_refs_by_line[line_idx] = rhs_refs;
        line_variable_refs_cache[line_idx] = line_variable_refs(line, mask);
    }
    if !assignment_name_by_line.iter().any(|name| name.is_some()) {
        return None;
    }

    let mut graph = VariableDependencyGraph {
        assignment_name_by_line,
        assignment_rhs_refs_by_line,
        assignment_line_by_name: FxHashMap::default(),
        variable_dependents: FxHashMap::default(),
        variable_consumer_lines: FxHashMap::default(),
        line_variable_refs: line_variable_refs_cache,
        has_duplicate_assignment_names: false,
    };
    rebuild_dependency_maps(&mut graph);
    Some(graph)
}

pub fn sync_variable_dependency_graph(
    graph: &mut Option<VariableDependencyGraph>,
    lines: &[String],
    changed_from: usize,
    changed_to: usize,
    mask: CalcFeatureMask,
) {
    if !mask.variables_active() {
        *graph = None;
        return;
    }

    let can_patch_in_place = graph
        .as_mut()
        .map(|cached| {
            try_patch_variable_dependency_graph_in_place(
                cached,
                lines,
                changed_from,
                changed_to,
                mask,
            )
        })
        .unwrap_or(false);

    if !can_patch_in_place {
        *graph = build_variable_dependency_graph(lines, mask);
    }
}

pub fn variable_names_from_dependency_graph(graph: &VariableDependencyGraph) -> Vec<String> {
    let mut names = graph
        .assignment_line_by_name
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn variable_dependency_window_from_graph(
    graph: &VariableDependencyGraph,
    changed_from: usize,
    changed_to: usize,
    prev_changed_assignment_names: &[String],
    prev_changed_had_assignment: bool,
) -> Option<(usize, usize)> {
    let line_count = graph.assignment_name_by_line.len();
    let mut changed_variables: FxHashSet<String> = FxHashSet::default();
    let from = changed_from.min(line_count);
    let to = changed_to.min(line_count).max(from);

    for line_idx in from..to {
        if let Some(name) = graph
            .assignment_name_by_line
            .get(line_idx)
            .and_then(|name| name.as_ref())
        {
            changed_variables.insert(name.clone());
        }
    }
    for name in prev_changed_assignment_names {
        if !name.is_empty() {
            changed_variables.insert(name.to_ascii_lowercase());
        }
    }

    if changed_variables.is_empty() {
        if prev_changed_had_assignment {
            return Some((0, line_count));
        }
        return None;
    }

    let mut affected_variables = changed_variables.clone();
    let mut stack: Vec<String> = changed_variables.into_iter().collect();
    while let Some(variable) = stack.pop() {
        let Some(dependents) = graph.variable_dependents.get(&variable) else {
            continue;
        };
        for dependent in dependents {
            if affected_variables.insert(dependent.clone()) {
                stack.push(dependent.clone());
            }
        }
    }

    let mut min_line = from;
    let mut max_line_exclusive = to;
    let mut found_any = from < to;

    for variable in &affected_variables {
        if let Some(line_idx) = graph.assignment_line_by_name.get(variable).copied() {
            min_line = min_line.min(line_idx);
            max_line_exclusive = max_line_exclusive.max(line_idx.saturating_add(1));
            found_any = true;
        }
    }

    for variable in &affected_variables {
        let Some(consumers) = graph.variable_consumer_lines.get(variable) else {
            continue;
        };
        for line_idx in consumers {
            min_line = min_line.min(*line_idx);
            max_line_exclusive = max_line_exclusive.max(line_idx.saturating_add(1));
            found_any = true;
        }
    }

    if found_any {
        Some((min_line, max_line_exclusive))
    } else {
        None
    }
}

static TABLE_COORD_REF_RE: OnceLock<Regex> = OnceLock::new();

fn table_coord_ref_regex() -> &'static Regex {
    TABLE_COORD_REF_RE.get_or_init(|| {
        Regex::new(r"\(\s*(\d+)\s*,\s*(\d+)\s*\)").expect("table coordinate regex is valid")
    })
}

fn table_coordinate_refs(expression: &str) -> Vec<(usize, usize)> {
    let regex = table_coord_ref_regex();
    let mut refs = Vec::new();
    for captures in regex.captures_iter(expression) {
        let row = captures
            .get(1)
            .and_then(|m| m.as_str().parse::<usize>().ok());
        let col = captures
            .get(2)
            .and_then(|m| m.as_str().parse::<usize>().ok());
        if let (Some(row), Some(col)) = (row, col) {
            refs.push((row, col));
        }
    }
    refs
}

fn split_table_cells(line: &str) -> Vec<String> {
    table::split_table_cells(line)
}

fn is_table_delimiter_cell(cell: &str) -> bool {
    let trimmed = cell.trim();
    if trimmed.is_empty() {
        return true;
    }
    let without_left = trimmed.strip_prefix(':').unwrap_or(trimmed);
    let core = without_left.strip_suffix(':').unwrap_or(without_left);
    core.len() >= 3 && core.bytes().all(|byte| byte == b'-')
}

fn is_table_delimiter_row(cells: &[String]) -> bool {
    !cells.is_empty() && cells.iter().all(|cell| is_table_delimiter_cell(cell))
}

#[derive(Debug, Clone)]
struct TableDataRows {
    rows: Vec<Vec<usize>>,
    row_for_line: FxHashMap<usize, usize>,
    col_count_for_line: FxHashMap<usize, usize>,
}

fn table_data_rows(lines: &[String], table_start: usize, table_end: usize) -> TableDataRows {
    let mut delimiter_row: Option<usize> = None;
    for row_idx in table_start..=table_end {
        let Some(line) = lines.get(row_idx) else {
            continue;
        };
        let cells = split_table_cells(line);
        if is_table_delimiter_row(&cells) {
            delimiter_row = Some(row_idx);
            break;
        }
    }
    let Some(data_start) = delimiter_row.map(|row| row.saturating_add(1)) else {
        return TableDataRows {
            rows: Vec::new(),
            row_for_line: FxHashMap::default(),
            col_count_for_line: FxHashMap::default(),
        };
    };
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut row_for_line: FxHashMap<usize, usize> = FxHashMap::default();
    let mut col_count_for_line: FxHashMap<usize, usize> = FxHashMap::default();
    for row_idx in data_start..=table_end {
        let Some(line) = lines.get(row_idx) else {
            continue;
        };
        let cells = split_table_cells(line);
        col_count_for_line.insert(row_idx, cells.len());
        if is_table_delimiter_row(&cells) {
            continue;
        }
        if table::is_table_continuation_line(line) {
            if rows.is_empty() {
                rows.push(vec![row_idx]);
                row_for_line.insert(row_idx, 1);
            } else {
                let logical_row = rows.len();
                rows[logical_row - 1].push(row_idx);
                row_for_line.insert(row_idx, logical_row);
            }
        } else {
            rows.push(vec![row_idx]);
            row_for_line.insert(row_idx, rows.len());
        }
    }
    TableDataRows {
        rows,
        row_for_line,
        col_count_for_line,
    }
}

#[derive(Debug, Clone)]
struct TableFormulaLineInfo {
    line_idx: usize,
    has_row_formula: bool,
    has_col_formula: bool,
}

#[derive(Debug, Clone)]
struct TableFormulaDependencyBlock {
    table_start: usize,
    table_end: usize,
    formula_lines: Vec<TableFormulaLineInfo>,
    data_rows: TableDataRows,
    formula_nodes: FxHashMap<(usize, usize), usize>,
    reverse_refs: FxHashMap<(usize, usize), FxHashSet<(usize, usize)>>,
    nodes_with_coords: FxHashSet<(usize, usize)>,
}

#[derive(Debug, Clone)]
pub struct TableFormulaDependencyIndex {
    line_count: usize,
    blocks: Vec<TableFormulaDependencyBlock>,
}

#[derive(Debug, Clone)]
pub struct CalcDependencyIndex {
    mask: CalcFeatureMask,
    variable_graph: Option<VariableDependencyGraph>,
    table_formula_index: Option<TableFormulaDependencyIndex>,
}

fn build_table_formula_dependency_block(
    lines: &[String],
    table_start: usize,
    table_end: usize,
) -> Option<TableFormulaDependencyBlock> {
    let data_rows = table_data_rows(lines, table_start, table_end);
    let mut formula_lines = Vec::new();
    let mut formula_nodes: FxHashMap<(usize, usize), usize> = FxHashMap::default();
    let mut reverse_refs: FxHashMap<(usize, usize), FxHashSet<(usize, usize)>> =
        FxHashMap::default();
    let mut nodes_with_coords: FxHashSet<(usize, usize)> = FxHashSet::default();

    for line_idx in table_start..=table_end {
        let Some(line) = lines.get(line_idx) else {
            continue;
        };
        let segments = find_table_formula_segments(line);
        if segments.is_empty() {
            continue;
        }
        let has_row_formula = segments.iter().any(|segment| {
            segment
                .labels
                .iter()
                .any(|label| label == "sum_row()" || label == "avg_row()")
        });
        let has_col_formula = segments.iter().any(|segment| {
            segment
                .labels
                .iter()
                .any(|label| label == "sum_col()" || label == "avg_col()")
        });
        formula_lines.push(TableFormulaLineInfo {
            line_idx,
            has_row_formula,
            has_col_formula,
        });
    }

    if !data_rows.rows.is_empty() {
        for (logical_idx, row_line_idxs) in data_rows.rows.iter().enumerate() {
            let row_1based = logical_idx.saturating_add(1);
            for row_line_idx in row_line_idxs {
                let line = &lines[*row_line_idx];
                for segment in find_table_formula_segments(line) {
                    let node = (row_1based, segment.cell_index.saturating_add(1));
                    formula_nodes.insert(node, *row_line_idx);
                    let expression = line[segment.from_byte..segment.to_byte].trim();
                    let refs = table_coordinate_refs(expression);
                    if !refs.is_empty() {
                        nodes_with_coords.insert(node);
                    }
                    for reference in refs {
                        reverse_refs.entry(reference).or_default().insert(node);
                    }
                }
            }
        }
    }

    if formula_lines.is_empty() && formula_nodes.is_empty() {
        return None;
    }

    Some(TableFormulaDependencyBlock {
        table_start,
        table_end,
        formula_lines,
        data_rows,
        formula_nodes,
        reverse_refs,
        nodes_with_coords,
    })
}

pub fn build_table_formula_dependency_index(
    lines: &[String],
    mask: CalcFeatureMask,
) -> Option<TableFormulaDependencyIndex> {
    if !mask.table_active() {
        return None;
    }

    let mut blocks = Vec::new();
    let mut line_idx = 0usize;
    while line_idx < lines.len() {
        if !is_table_line(&lines[line_idx]) {
            line_idx = line_idx.saturating_add(1);
            continue;
        }
        let Some((table_start, table_end)) = table_block_range(lines, line_idx) else {
            line_idx = line_idx.saturating_add(1);
            continue;
        };
        if let Some(block) = build_table_formula_dependency_block(lines, table_start, table_end) {
            blocks.push(block);
        }
        line_idx = table_end.saturating_add(1);
    }

    if blocks.is_empty() {
        None
    } else {
        Some(TableFormulaDependencyIndex {
            line_count: lines.len(),
            blocks,
        })
    }
}

pub fn sync_table_formula_dependency_index(
    index: &mut Option<TableFormulaDependencyIndex>,
    lines: &[String],
    changed_from: usize,
    changed_to: usize,
    mask: CalcFeatureMask,
) {
    if !mask.table_active() {
        *index = None;
        return;
    }

    let needs_rebuild = index
        .as_ref()
        .map(|cached| {
            cached.line_count != lines.len()
                || table_range_maybe_impacts_formulas(lines, changed_from, changed_to, mask)
        })
        .unwrap_or(true);
    if needs_rebuild {
        *index = build_table_formula_dependency_index(lines, mask);
    }
}

pub fn build_calc_dependency_index(
    lines: &[String],
    mask: CalcFeatureMask,
) -> Option<CalcDependencyIndex> {
    if !mask.math_enabled {
        return None;
    }
    let variable_graph = build_variable_dependency_graph(lines, mask);
    let table_formula_index = build_table_formula_dependency_index(lines, mask);
    if variable_graph.is_none() && table_formula_index.is_none() {
        None
    } else {
        Some(CalcDependencyIndex {
            mask,
            variable_graph,
            table_formula_index,
        })
    }
}

pub fn sync_calc_dependency_index(
    index: &mut Option<CalcDependencyIndex>,
    lines: &[String],
    changed_from: usize,
    changed_to: usize,
    mask: CalcFeatureMask,
) {
    if !mask.math_enabled {
        *index = None;
        return;
    }

    if index
        .as_ref()
        .map(|cached| cached.mask != mask)
        .unwrap_or(true)
    {
        *index = build_calc_dependency_index(lines, mask);
        return;
    }

    if let Some(cached) = index.as_mut() {
        sync_variable_dependency_graph(
            &mut cached.variable_graph,
            lines,
            changed_from,
            changed_to,
            mask,
        );
        sync_table_formula_dependency_index(
            &mut cached.table_formula_index,
            lines,
            changed_from,
            changed_to,
            mask,
        );
        if cached.variable_graph.is_none() && cached.table_formula_index.is_none() {
            *index = None;
        }
    } else {
        *index = build_calc_dependency_index(lines, mask);
    }
}

pub fn variable_names_from_calc_dependency_index(
    index: Option<&CalcDependencyIndex>,
) -> Vec<String> {
    index
        .and_then(|cached| cached.variable_graph.as_ref())
        .map(variable_names_from_dependency_graph)
        .unwrap_or_default()
}

fn coordinate_formula_dependency_window_in_block(
    block: &TableFormulaDependencyBlock,
    changed_from: usize,
    changed_to: usize,
) -> Option<(usize, usize)> {
    if changed_from >= changed_to
        || block.table_end.saturating_add(1) <= changed_from
        || changed_to <= block.table_start
        || block.data_rows.rows.is_empty()
        || block.formula_nodes.is_empty()
    {
        None
    } else {
        let mut changed_cells: FxHashSet<(usize, usize)> = FxHashSet::default();
        let mut structure_changed = false;
        for idx in
            changed_from.max(block.table_start)..changed_to.min(block.table_end.saturating_add(1))
        {
            let Some(row_1based) = block.data_rows.row_for_line.get(&idx).copied() else {
                structure_changed = true;
                continue;
            };
            let col_count = block
                .data_rows
                .col_count_for_line
                .get(&idx)
                .copied()
                .unwrap_or(0);
            if col_count == 0 {
                structure_changed = true;
                continue;
            }
            for col_1based in 1..=col_count {
                changed_cells.insert((row_1based, col_1based));
            }
        }

        let mut affected_nodes: FxHashSet<(usize, usize)> = FxHashSet::default();
        let mut stack: Vec<(usize, usize)> = Vec::new();
        if structure_changed {
            for node in &block.nodes_with_coords {
                if affected_nodes.insert(*node) {
                    stack.push(*node);
                }
            }
        } else {
            for changed in &changed_cells {
                if let Some(dependents) = block.reverse_refs.get(changed) {
                    for dependent in dependents {
                        if affected_nodes.insert(*dependent) {
                            stack.push(*dependent);
                        }
                    }
                }
            }
        }

        while let Some(node) = stack.pop() {
            if let Some(dependents) = block.reverse_refs.get(&node) {
                for dependent in dependents {
                    if affected_nodes.insert(*dependent) {
                        stack.push(*dependent);
                    }
                }
            }
        }

        let mut min_line = usize::MAX;
        let mut max_line_exclusive = 0usize;
        for node in &affected_nodes {
            if let Some(dep_line_idx) = block.formula_nodes.get(node) {
                min_line = min_line.min(*dep_line_idx);
                max_line_exclusive = max_line_exclusive.max(dep_line_idx.saturating_add(1));
            }
        }
        if min_line == usize::MAX {
            None
        } else {
            Some((min_line, max_line_exclusive))
        }
    }
}

fn table_range_maybe_impacts_formulas(
    lines: &[String],
    from: usize,
    to: usize,
    mask: CalcFeatureMask,
) -> bool {
    if !mask.table_active() {
        return false;
    }
    if from >= to {
        return false;
    }
    lines
        .get(from..to)
        .map(|slice| slice.iter().any(|line| is_table_line(line)))
        .unwrap_or(false)
}

fn formula_dependency_window(
    lines: &[String],
    changed_from: usize,
    changed_to: usize,
    mask: CalcFeatureMask,
) -> Option<(usize, usize)> {
    formula_dependency_window_with_cached_index(None, lines, changed_from, changed_to, mask)
}

fn formula_dependency_window_with_cached_index(
    table_index: Option<&TableFormulaDependencyIndex>,
    lines: &[String],
    changed_from: usize,
    changed_to: usize,
    mask: CalcFeatureMask,
) -> Option<(usize, usize)> {
    if !mask.table_active() {
        return None;
    }
    if changed_from >= changed_to {
        return None;
    }

    if let Some(index) = table_index {
        let mut min_line = usize::MAX;
        let mut max_line_exclusive = 0usize;
        for block in &index.blocks {
            if block.table_end.saturating_add(1) <= changed_from || changed_to <= block.table_start
            {
                continue;
            }

            let mut block_builtin_from = usize::MAX;
            let mut block_builtin_to = 0usize;
            for info in &block.formula_lines {
                let mut impacted = info.line_idx >= changed_from && info.line_idx < changed_to;
                if !impacted && info.has_col_formula {
                    let dep_from = block.table_start;
                    let dep_to = info.line_idx;
                    impacted = dep_from < dep_to && dep_from < changed_to && changed_from < dep_to;
                }
                if !impacted && info.has_row_formula {
                    impacted = info.line_idx >= changed_from && info.line_idx < changed_to;
                }
                if impacted {
                    block_builtin_from = block_builtin_from.min(info.line_idx);
                    block_builtin_to = block_builtin_to.max(info.line_idx.saturating_add(1));
                }
            }

            let builtin_window = if block_builtin_from == usize::MAX {
                None
            } else {
                Some((block_builtin_from, block_builtin_to))
            };
            let coord_window =
                coordinate_formula_dependency_window_in_block(block, changed_from, changed_to);

            let block_window = match (builtin_window, coord_window) {
                (Some((from_a, to_a)), Some((from_b, to_b))) => {
                    Some((from_a.min(from_b), to_a.max(to_b)))
                }
                (Some(window), None) | (None, Some(window)) => Some(window),
                (None, None) => None,
            };
            if let Some((from, to)) = block_window {
                min_line = min_line.min(from);
                max_line_exclusive = max_line_exclusive.max(to);
            }
        }

        if min_line == usize::MAX {
            return None;
        }
        return Some((min_line, max_line_exclusive));
    }

    let mut min_line = usize::MAX;
    let mut max_line_exclusive = 0usize;
    for line_idx in 0..lines.len() {
        let line = &lines[line_idx];
        if !is_table_line(line) {
            continue;
        }

        let segments = find_table_formula_segments(line);
        if segments.is_empty() {
            continue;
        }
        let has_row_formula = segments.iter().any(|segment| {
            segment
                .labels
                .iter()
                .any(|label| label == "sum_row()" || label == "avg_row()")
        });
        let has_col_formula = segments.iter().any(|segment| {
            segment
                .labels
                .iter()
                .any(|label| label == "sum_col()" || label == "avg_col()")
        });

        let mut impacted = line_idx >= changed_from && line_idx < changed_to;
        if !impacted && has_col_formula {
            let Some((table_start, _)) = table_block_range(lines, line_idx) else {
                continue;
            };
            let dep_from = table_start;
            let dep_to = line_idx;
            impacted = dep_from < dep_to && dep_from < changed_to && changed_from < dep_to;
        }
        if !impacted && has_row_formula {
            impacted = line_idx >= changed_from && line_idx < changed_to;
        }

        if impacted {
            min_line = min_line.min(line_idx);
            max_line_exclusive = max_line_exclusive.max(line_idx.saturating_add(1));
        }
    }

    let builtin_window = if min_line == usize::MAX {
        None
    } else {
        Some((min_line, max_line_exclusive))
    };
    let coord_window = {
        let mut min_coord = usize::MAX;
        let mut max_coord = 0usize;
        let mut line_idx = 0usize;
        while line_idx < lines.len() {
            if !is_table_line(&lines[line_idx]) {
                line_idx = line_idx.saturating_add(1);
                continue;
            }
            let Some((table_start, table_end)) = table_block_range(lines, line_idx) else {
                line_idx = line_idx.saturating_add(1);
                continue;
            };
            line_idx = table_end.saturating_add(1);
            let Some(block) = build_table_formula_dependency_block(lines, table_start, table_end)
            else {
                continue;
            };
            if let Some((from, to)) =
                coordinate_formula_dependency_window_in_block(&block, changed_from, changed_to)
            {
                min_coord = min_coord.min(from);
                max_coord = max_coord.max(to);
            }
        }
        if min_coord == usize::MAX {
            None
        } else {
            Some((min_coord, max_coord))
        }
    };

    match (builtin_window, coord_window) {
        (Some((from_a, to_a)), Some((from_b, to_b))) => Some((from_a.min(from_b), to_a.max(to_b))),
        (Some(window), None) | (None, Some(window)) => Some(window),
        (None, None) => None,
    }
}

fn table_block_range(lines: &[String], line_idx: usize) -> Option<(usize, usize)> {
    if lines
        .get(line_idx)
        .map(|line| !is_table_line(line))
        .unwrap_or(true)
    {
        return None;
    }

    let mut start = line_idx;
    while start > 0 {
        let prev = start - 1;
        if !is_table_line(lines.get(prev)?) {
            break;
        }
        start = prev;
    }

    let mut end = line_idx;
    while end + 1 < lines.len() {
        if !is_table_line(lines.get(end + 1)?) {
            break;
        }
        end += 1;
    }

    Some((start, end))
}

fn variable_dependency_window(
    lines: &[String],
    changed_from: usize,
    changed_to: usize,
    prev_changed_assignment_names: &[String],
    prev_changed_had_assignment: bool,
    mask: CalcFeatureMask,
) -> Option<(usize, usize)> {
    let Some(graph) = build_variable_dependency_graph(lines, mask) else {
        if prev_changed_had_assignment {
            return Some((0, lines.len()));
        }
        return None;
    };
    variable_dependency_window_from_graph(
        &graph,
        changed_from,
        changed_to,
        prev_changed_assignment_names,
        prev_changed_had_assignment,
    )
}

fn variable_dependency_window_with_cached_graph(
    graph: Option<&VariableDependencyGraph>,
    lines: &[String],
    changed_from: usize,
    changed_to: usize,
    prev_changed_assignment_names: &[String],
    prev_changed_had_assignment: bool,
    mask: CalcFeatureMask,
) -> Option<(usize, usize)> {
    if let Some(cached) = graph {
        return variable_dependency_window_from_graph(
            cached,
            changed_from,
            changed_to,
            prev_changed_assignment_names,
            prev_changed_had_assignment,
        );
    }
    variable_dependency_window(
        lines,
        changed_from,
        changed_to,
        prev_changed_assignment_names,
        prev_changed_had_assignment,
        mask,
    )
}

pub fn decide_eval_window(
    lines: &[String],
    changed_from: usize,
    changed_to: usize,
    prev_changed_lines: &[String],
    has_prev: bool,
    variables_enabled: bool,
) -> CalcEvalWindowDecision {
    decide_eval_window_with_mask(
        lines,
        changed_from,
        changed_to,
        prev_changed_lines,
        has_prev,
        CalcFeatureMask {
            math_enabled: true,
            table_enabled: true,
            variables_enabled,
        },
    )
}

pub fn decide_eval_window_with_mask(
    lines: &[String],
    changed_from: usize,
    changed_to: usize,
    prev_changed_lines: &[String],
    has_prev: bool,
    mask: CalcFeatureMask,
) -> CalcEvalWindowDecision {
    let prev_changed_assignment_names =
        collect_assignment_names_with_mask(prev_changed_lines, mask);
    let prev_changed_had_assignment = !prev_changed_assignment_names.is_empty()
        || contains_variable_assignment_with_mask(prev_changed_lines, mask);
    let prev_changed_had_builtin_formula =
        contains_builtin_formula_with_mask(prev_changed_lines, mask);

    let line_count = lines.len();
    let mut eval_from = changed_from.min(line_count);
    let mut eval_to = changed_to.min(line_count).max(eval_from);

    let changed_lines: &[String] = lines.get(eval_from..eval_to).unwrap_or(&[]);
    let touches_any_assignment = mask.variables_active()
        && (contains_variable_assignment_with_mask(changed_lines, mask)
            || prev_changed_had_assignment);
    let touches_builtin_formula =
        contains_builtin_formula_with_mask(changed_lines, mask) || prev_changed_had_builtin_formula;

    if !has_prev {
        return CalcEvalWindowDecision {
            eval_from: 0,
            eval_to: line_count,
            touches_any_assignment,
            touches_builtin_formula,
            can_use_partial: false,
        };
    }

    if touches_any_assignment {
        if let Some((from, to)) = variable_dependency_window(
            lines,
            eval_from,
            eval_to,
            &prev_changed_assignment_names,
            prev_changed_had_assignment,
            mask,
        ) {
            eval_from = eval_from.min(from);
            eval_to = eval_to.max(to);
        } else if prev_changed_had_assignment {
            eval_from = 0;
            eval_to = line_count;
        }
    }

    let maybe_formula_deps = touches_builtin_formula
        || prev_changed_had_builtin_formula
        || table_range_maybe_impacts_formulas(lines, eval_from, eval_to, mask);
    if maybe_formula_deps {
        if let Some((from, to)) = formula_dependency_window(lines, eval_from, eval_to, mask) {
            eval_from = eval_from.min(from);
            eval_to = eval_to.max(to);
        } else if touches_builtin_formula || prev_changed_had_builtin_formula {
            eval_from = 0;
            eval_to = line_count;
        }
    }

    CalcEvalWindowDecision {
        eval_from,
        eval_to,
        touches_any_assignment,
        touches_builtin_formula,
        can_use_partial: true,
    }
}

pub fn decide_eval_window_with_flags(
    lines: &[String],
    changed_from: usize,
    changed_to: usize,
    prev_changed_assignment_names: &[String],
    prev_changed_had_assignment: bool,
    prev_changed_had_builtin_formula: bool,
    has_prev: bool,
    variables_enabled: bool,
    table_enabled: bool,
) -> CalcEvalWindowDecision {
    decide_eval_window_with_cached_variable_graph_and_flags(
        None,
        lines,
        changed_from,
        changed_to,
        prev_changed_assignment_names,
        prev_changed_had_assignment,
        prev_changed_had_builtin_formula,
        has_prev,
        variables_enabled,
        table_enabled,
    )
}

pub fn decide_eval_window_with_cached_variable_graph_and_flags(
    variable_graph: Option<&VariableDependencyGraph>,
    lines: &[String],
    changed_from: usize,
    changed_to: usize,
    prev_changed_assignment_names: &[String],
    prev_changed_had_assignment: bool,
    prev_changed_had_builtin_formula: bool,
    has_prev: bool,
    variables_enabled: bool,
    table_enabled: bool,
) -> CalcEvalWindowDecision {
    decide_eval_window_with_cached_dependency_indexes_and_flags(
        variable_graph,
        None,
        lines,
        changed_from,
        changed_to,
        prev_changed_assignment_names,
        prev_changed_had_assignment,
        prev_changed_had_builtin_formula,
        has_prev,
        variables_enabled,
        table_enabled,
    )
}

pub fn decide_eval_window_with_cached_calc_dependency_index_and_flags(
    calc_dependency_index: Option<&CalcDependencyIndex>,
    lines: &[String],
    changed_from: usize,
    changed_to: usize,
    prev_changed_assignment_names: &[String],
    prev_changed_had_assignment: bool,
    prev_changed_had_builtin_formula: bool,
    has_prev: bool,
    variables_enabled: bool,
    table_enabled: bool,
) -> CalcEvalWindowDecision {
    decide_eval_window_with_cached_dependency_indexes_and_flags(
        calc_dependency_index.and_then(|cached| cached.variable_graph.as_ref()),
        calc_dependency_index.and_then(|cached| cached.table_formula_index.as_ref()),
        lines,
        changed_from,
        changed_to,
        prev_changed_assignment_names,
        prev_changed_had_assignment,
        prev_changed_had_builtin_formula,
        has_prev,
        variables_enabled,
        table_enabled,
    )
}

pub fn decide_eval_window_with_cached_dependency_indexes_and_flags(
    variable_graph: Option<&VariableDependencyGraph>,
    table_formula_index: Option<&TableFormulaDependencyIndex>,
    lines: &[String],
    changed_from: usize,
    changed_to: usize,
    prev_changed_assignment_names: &[String],
    prev_changed_had_assignment: bool,
    prev_changed_had_builtin_formula: bool,
    has_prev: bool,
    variables_enabled: bool,
    table_enabled: bool,
) -> CalcEvalWindowDecision {
    let line_count = lines.len();
    let mut eval_from = changed_from.min(line_count);
    let mut eval_to = changed_to.min(line_count).max(eval_from);
    let mask = CalcFeatureMask {
        math_enabled: true,
        table_enabled,
        variables_enabled,
    };

    let changed_lines: &[String] = lines.get(eval_from..eval_to).unwrap_or(&[]);
    let touches_any_assignment = mask.variables_active()
        && (contains_variable_assignment_with_mask(changed_lines, mask)
            || prev_changed_had_assignment);
    let touches_builtin_formula =
        contains_builtin_formula_with_mask(changed_lines, mask) || prev_changed_had_builtin_formula;

    if !has_prev {
        return CalcEvalWindowDecision {
            eval_from: 0,
            eval_to: line_count,
            touches_any_assignment,
            touches_builtin_formula,
            can_use_partial: false,
        };
    }

    if touches_any_assignment {
        if let Some((from, to)) = variable_dependency_window_with_cached_graph(
            variable_graph,
            lines,
            eval_from,
            eval_to,
            prev_changed_assignment_names,
            prev_changed_had_assignment,
            mask,
        ) {
            eval_from = eval_from.min(from);
            eval_to = eval_to.max(to);
        } else if prev_changed_had_assignment {
            eval_from = 0;
            eval_to = line_count;
        }
    }

    let maybe_formula_deps = touches_builtin_formula
        || prev_changed_had_builtin_formula
        || table_range_maybe_impacts_formulas(lines, eval_from, eval_to, mask);
    if maybe_formula_deps {
        if let Some((from, to)) = formula_dependency_window_with_cached_index(
            table_formula_index,
            lines,
            eval_from,
            eval_to,
            mask,
        ) {
            eval_from = eval_from.min(from);
            eval_to = eval_to.max(to);
        } else if touches_builtin_formula || prev_changed_had_builtin_formula {
            eval_from = 0;
            eval_to = line_count;
        }
    }

    CalcEvalWindowDecision {
        eval_from,
        eval_to,
        touches_any_assignment,
        touches_builtin_formula,
        can_use_partial: true,
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

    let pipes = table::table_pipe_positions(line);
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
        let is_colon_eq_formula = trimmed
            .strip_prefix(":=")
            .map(|rest| !rest.trim_start().is_empty())
            .unwrap_or(false);
        if labels.is_empty() && !is_colon_eq_formula {
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

pub fn plan_incremental_calc_from_line_metadata(
    prev_line_metadata: &[LineMetadata],
    prev_results: &[Option<String>],
    next_lines: &[String],
    next_line_metadata: &[LineMetadata],
) -> IncrementalCalcPlan {
    let prev_hashes: Vec<u64> = prev_line_metadata.iter().map(|m| m.hash).collect();
    let next_hashes: Vec<u64> = next_line_metadata.iter().map(|m| m.hash).collect();
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
    fn contains_assignment_operator_requires_word_char_before_colon_eq() {
        // Variable assignments — must match.
        assert!(contains_assignment_operator("x := 5"));
        assert!(contains_assignment_operator("total cost := 12"));
        assert!(contains_assignment_operator("| total := 12 |"));
        // := formula prefix — must NOT match.
        assert!(!contains_assignment_operator(":=sum_col()"));
        assert!(!contains_assignment_operator(":=5+var"));
        assert!(!contains_assignment_operator(":=var"));
        assert!(!contains_assignment_operator("| :=sum_col() |"));
        assert!(!contains_assignment_operator("  := 5"));
    }

    #[test]
    fn find_table_formula_segments_detects_colon_eq_prefix_cells() {
        let line = "| label | :=sum_col()+3 |";
        let segs = find_table_formula_segments(line);
        assert_eq!(segs.len(), 1);
        let seg = &segs[0];
        assert_eq!(&line[seg.from_byte..seg.to_byte], ":=sum_col()+3");
        assert_eq!(seg.cell_index, 1);
        // sum_col() label is still extracted from the expression text.
        assert_eq!(seg.labels, vec!["sum_col()"]);
    }

    #[test]
    fn find_table_formula_segments_detects_colon_eq_prefix_without_builtin() {
        let line = "| a | :=5+var |";
        let segs = find_table_formula_segments(line);
        assert_eq!(segs.len(), 1);
        let seg = &segs[0];
        assert_eq!(&line[seg.from_byte..seg.to_byte], ":=5+var");
        assert!(seg.labels.is_empty());
    }

    #[test]
    fn table_formula_segment_detects_avg_col() {
        let line = "| dsad | dsdsd | :=avg_col() | 1.91 | |";
        let seg = find_table_formula_segment(line).expect("formula");
        assert_eq!(seg.labels, vec!["avg_col()".to_string()]);
        assert_eq!(&line[seg.from_byte..seg.to_byte], ":=avg_col()");
        assert_eq!(seg.cell_index, 2);
    }

    #[test]
    fn find_table_formula_segments_returns_each_formula_cell_left_to_right() {
        let line = "| :=sum_col() | x | :=sum_row() |";
        let segments = find_table_formula_segments(line);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].labels, vec!["sum_col()".to_string()]);
        assert_eq!(segments[0].cell_index, 0);
        assert_eq!(segments[1].labels, vec!["sum_row()".to_string()]);
        assert_eq!(segments[1].cell_index, 2);
    }

    #[test]
    fn find_single_calc_table_cell_ignores_escaped_pipe_in_cell_content() {
        let line = "| left \\| right | 2+2 |";
        let seg = find_single_calc_table_cell(line).expect("calc segment");
        assert_eq!(seg.expr, "2+2");
    }

    #[test]
    fn find_table_formula_segments_ignores_escaped_pipe_in_non_formula_cell() {
        let line = "| note with \\| pipe | :=sum_col() |";
        let segments = find_table_formula_segments(line);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].cell_index, 1);
        assert_eq!(
            &line[segments[0].from_byte..segments[0].to_byte],
            ":=sum_col()"
        );
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
    fn detect_calc_signal_flags_reports_assignment_and_builtin_formula() {
        let lines = vec![
            "x := 2".to_string(),
            "| value | sum_col() * x |".to_string(),
        ];
        let flags = detect_calc_signal_flags(&lines);
        assert!(flags.has_variable_assignment);
        assert!(flags.has_builtin_formula);
    }

    #[test]
    fn detect_calc_signal_flags_ignores_plain_numeric_table_and_list_literals() {
        let lines = vec!["- [x] 6".to_string(), "| label | 6 |".to_string()];
        let flags = detect_calc_signal_flags(&lines);
        assert!(!flags.has_variable_assignment);
        assert!(!flags.has_builtin_formula);
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
            line_for_calc_evaluation("| name | :=avg_col() |"),
            ":=avg_col()".to_string()
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

    #[test]
    fn decide_eval_window_expands_for_variable_dependents() {
        let lines = vec![
            "a := 1".to_string(),
            "b := a + 1".to_string(),
            "c := b + 1".to_string(),
            "c".to_string(),
            "x := 9".to_string(),
            "x".to_string(),
        ];
        let prev_changed = vec!["a := 0".to_string()];
        let decision = decide_eval_window(&lines, 0, 1, &prev_changed, true, true);
        assert!(decision.can_use_partial);
        assert_eq!(decision.eval_from, 0);
        assert_eq!(decision.eval_to, 4);
    }

    #[test]
    fn decide_eval_window_tracks_removed_assignment_dependencies() {
        let lines = vec![
            "plain text".to_string(),
            "b := a + 1".to_string(),
            "b".to_string(),
            "x := 3".to_string(),
            "x".to_string(),
        ];
        let prev_changed = vec!["a := 1".to_string()];
        let decision = decide_eval_window(&lines, 0, 1, &prev_changed, true, true);
        assert!(decision.can_use_partial);
        assert_eq!(decision.eval_from, 0);
        assert_eq!(decision.eval_to, 3);
    }

    #[test]
    fn decide_eval_window_expands_table_column_formula_dependents() {
        let lines = vec![
            "| item | value | total |".to_string(),
            "| ---- | ----- | ----- |".to_string(),
            "| a | 1 | 1 |".to_string(),
            "| b | 2 | 2 |".to_string(),
            "| total |  | :=sum_col() |".to_string(),
            "| grand |  | :=sum_col() |".to_string(),
        ];
        let decision = decide_eval_window(&lines, 3, 4, &[], true, true);
        assert!(decision.can_use_partial);
        assert_eq!(decision.eval_from, 3);
        assert_eq!(decision.eval_to, 6);
    }

    #[test]
    fn decide_eval_window_expands_table_formula_with_variable_dependency() {
        let lines = vec![
            "var := 0.5".to_string(),
            "| item | value | total |".to_string(),
            "| ---- | ----- | ----- |".to_string(),
            "| a | 10 | :=sum_col() * var |".to_string(),
        ];
        let prev_changed = vec!["var := 0.4".to_string()];
        let decision = decide_eval_window(&lines, 0, 1, &prev_changed, true, true);
        assert!(decision.can_use_partial);
        assert_eq!(decision.eval_from, 0);
        assert_eq!(decision.eval_to, 4);
    }

    #[test]
    fn decide_eval_window_expands_multi_formula_row_with_variable_dependency() {
        let lines = vec![
            "var := 0.5".to_string(),
            "| item | a | b |".to_string(),
            "| ---- | - | - |".to_string(),
            "| row1 | 10 | 20 |".to_string(),
            "| total | :=sum_col() | :=sum_col() * var |".to_string(),
        ];
        let prev_changed = vec!["var := 0.4".to_string()];
        let decision = decide_eval_window(&lines, 0, 1, &prev_changed, true, true);
        assert!(decision.can_use_partial);
        assert_eq!(decision.eval_from, 0);
        assert_eq!(decision.eval_to, 5);
    }

    #[test]
    fn decide_eval_window_with_table_module_off_does_not_expand_for_table_formula_dependencies() {
        let lines = vec![
            "var := 0.5".to_string(),
            "| item | value | total |".to_string(),
            "| ---- | ----- | ----- |".to_string(),
            "| a | 10 | :=sum_col() * var |".to_string(),
        ];
        let prev_changed = vec!["var := 0.4".to_string()];
        let decision = decide_eval_window_with_mask(
            &lines,
            0,
            1,
            &prev_changed,
            true,
            CalcFeatureMask {
                math_enabled: true,
                table_enabled: false,
                variables_enabled: true,
            },
        );
        assert!(decision.can_use_partial);
        assert_eq!(decision.eval_from, 0);
        assert_eq!(decision.eval_to, 1);
    }

    #[test]
    fn decide_eval_window_expands_for_table_coordinate_reference_dependents() {
        let lines = vec![
            "| item | value | total |".to_string(),
            "| --- | --- | --- |".to_string(),
            "| a | 10 | |".to_string(),
            "| b | 20 | |".to_string(),
            "| c | 0 | :=(1,2) + (2,2) |".to_string(),
        ];
        // Edit first data row line; formula row should be included in eval window.
        let decision =
            decide_eval_window_with_mask(&lines, 2, 3, &[], true, CalcFeatureMask::default());
        assert!(decision.can_use_partial);
        assert_eq!(decision.eval_from, 2);
        assert_eq!(decision.eval_to, 5);
    }

    #[test]
    fn decide_eval_window_expands_transitively_for_table_coordinate_reference_dependents() {
        let lines = vec![
            "| item | value | total |".to_string(),
            "| --- | --- | --- |".to_string(),
            "| a | 1 | :=(2,3) |".to_string(),
            "| b | 2 | :=(1,3) + 1 |".to_string(),
            "| c | 3 | :=(2,3) + 1 |".to_string(),
        ];
        // Change row 1; row 2 depends on row 1, and row 3 depends on row 2.
        let decision =
            decide_eval_window_with_mask(&lines, 2, 3, &[], true, CalcFeatureMask::default());
        assert_eq!(decision.eval_from, 2);
        assert_eq!(decision.eval_to, 5);
    }

    #[test]
    fn cached_variable_dependency_graph_matches_uncached_window_decision_after_line_edit() {
        let mut lines = vec![
            "a := 1".to_string(),
            "b := a + 2".to_string(),
            "c := b + 3".to_string(),
            "c".to_string(),
        ];
        let mask = CalcFeatureMask::default();
        let mut graph = build_variable_dependency_graph(&lines, mask);

        lines[1] = "b := z + 2".to_string();
        sync_variable_dependency_graph(&mut graph, &lines, 1, 2, mask);

        let prev_changed_assignment_names = vec!["b".to_string()];
        let decision_cached = decide_eval_window_with_cached_variable_graph_and_flags(
            graph.as_ref(),
            &lines,
            1,
            2,
            &prev_changed_assignment_names,
            true,
            false,
            true,
            true,
            true,
        );
        let decision_uncached = decide_eval_window_with_flags(
            &lines,
            1,
            2,
            &prev_changed_assignment_names,
            true,
            false,
            true,
            true,
            true,
        );
        assert_eq!(decision_cached, decision_uncached);
    }

    #[test]
    fn sync_variable_dependency_graph_rebuilds_when_document_shape_changes() {
        let mut lines = vec![
            "a := 1".to_string(),
            "b := a + 2".to_string(),
            "b".to_string(),
        ];
        let mask = CalcFeatureMask::default();
        let mut graph = build_variable_dependency_graph(&lines, mask);
        lines.insert(1, "x := 7".to_string());

        sync_variable_dependency_graph(&mut graph, &lines, 1, 2, mask);

        let prev_changed_assignment_names = vec!["x".to_string()];
        let decision_cached = decide_eval_window_with_cached_variable_graph_and_flags(
            graph.as_ref(),
            &lines,
            1,
            2,
            &prev_changed_assignment_names,
            true,
            false,
            true,
            true,
            true,
        );
        let decision_uncached = decide_eval_window_with_flags(
            &lines,
            1,
            2,
            &prev_changed_assignment_names,
            true,
            false,
            true,
            true,
            true,
        );
        assert_eq!(decision_cached, decision_uncached);
    }

    #[test]
    fn cached_table_dependency_index_matches_uncached_eval_window_decision() {
        let lines = vec![
            "| item | value | total |".to_string(),
            "| --- | --- | --- |".to_string(),
            "| a | 10 | |".to_string(),
            "| b | 20 | |".to_string(),
            "| c | 0 | :=(1,2) + (2,2) |".to_string(),
        ];
        let mask = CalcFeatureMask::default();
        let table_index = build_table_formula_dependency_index(&lines, mask);

        let decision_cached = decide_eval_window_with_cached_dependency_indexes_and_flags(
            None,
            table_index.as_ref(),
            &lines,
            2,
            3,
            &[],
            false,
            false,
            true,
            true,
            true,
        );
        let decision_uncached =
            decide_eval_window_with_flags(&lines, 2, 3, &[], false, false, true, true, true);
        assert_eq!(decision_cached, decision_uncached);
    }

    #[test]
    fn assignment_name_extracts_normalized_name_from_list_and_table_lines() {
        assert_eq!(
            assignment_name("- [ ] Total Cost := 12"),
            Some("total cost".to_string())
        );
        assert_eq!(
            assignment_name("| item | total_cost := 12 |"),
            Some("total_cost".to_string())
        );
        assert_eq!(assignment_name("not assignment"), None);
    }
}
