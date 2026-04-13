use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::HashMap;

pub struct CalcEngine;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteEvaluationOptions {
    pub variables_enabled: bool,
    /// Optional half-open range `[from, to)` of line indices (0-based) to evaluate.
    /// When `None`, evaluates every line. Variable resolution always considers the
    /// full document so that a restricted evaluation still sees vars defined elsewhere.
    /// Positions outside the range are returned as `None` in `line_results`.
    pub eval_range: Option<(usize, usize)>,
}

impl Default for NoteEvaluationOptions {
    fn default() -> Self {
        Self {
            variables_enabled: true,
            eval_range: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VariableIndexEntry {
    pub name: String,
    pub normalized: String,
    pub line: usize, // 1-based
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteEvaluationDiagnostic {
    pub kind: String,
    pub line: usize, // 1-based
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteEvaluationResult {
    pub line_results: Vec<Option<String>>,
    pub variables: Vec<VariableIndexEntry>,
    pub diagnostics: Option<Vec<NoteEvaluationDiagnostic>>,
}

#[derive(Debug, Clone)]
struct VariableDefinition {
    name: String,
    normalized: String,
    expression: String,
    line: usize, // 0-based
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MatchSpan {
    normalized: String,
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolveState {
    Resolving,
    Resolved,
    Failed,
}

struct VariableResolver<'a> {
    defs: &'a HashMap<String, VariableDefinition>,
    variable_regex: Option<Regex>,
    states: HashMap<String, ResolveState>,
    values: HashMap<String, String>,
    diagnostics: Vec<NoteEvaluationDiagnostic>,
    raw_eval_cache: HashMap<String, Option<String>>,
}

struct NoInterrupt;
impl fend_core::Interrupt for NoInterrupt {
    fn should_interrupt(&self) -> bool {
        false
    }
}

static NO_INTERRUPT: NoInterrupt = NoInterrupt;

impl<'a> VariableResolver<'a> {
    fn new(defs: &'a HashMap<String, VariableDefinition>) -> Self {
        let mut names_sorted: Vec<String> = defs.keys().cloned().collect();
        // Longest-first so leftmost-first regex alternation picks the longest match.
        names_sorted.sort_by_key(|name| (Reverse(name.len()), name.clone()));
        let variable_regex = build_variable_regex(&names_sorted);

        Self {
            defs,
            variable_regex,
            states: HashMap::new(),
            values: HashMap::new(),
            diagnostics: Vec::new(),
            raw_eval_cache: HashMap::new(),
        }
    }

    fn find_matches(&self, expression: &str) -> Vec<MatchSpan> {
        let Some(regex) = &self.variable_regex else {
            return Vec::new();
        };
        regex
            .find_iter(expression)
            .map(|m| MatchSpan {
                normalized: expression[m.start()..m.end()].to_ascii_lowercase(),
                start: m.start(),
                end: m.end(),
            })
            .collect()
    }

    fn expression_references_variable(&self, expression: &str) -> bool {
        self.variable_regex
            .as_ref()
            .map(|r| r.is_match(expression))
            .unwrap_or(false)
    }

    fn eval_raw(&mut self, expr: &str) -> Option<String> {
        if let Some(cached) = self.raw_eval_cache.get(expr) {
            return cached.clone();
        }
        let result = evaluate_raw_expression(expr);
        self.raw_eval_cache.insert(expr.to_string(), result.clone());
        result
    }

    fn diagnostics(&self) -> Option<Vec<NoteEvaluationDiagnostic>> {
        if self.diagnostics.is_empty() {
            None
        } else {
            Some(self.diagnostics.clone())
        }
    }

    fn resolve(&mut self, normalized: &str) -> Option<String> {
        if let Some(state) = self.states.get(normalized).copied() {
            return match state {
                ResolveState::Resolved => self.values.get(normalized).cloned(),
                ResolveState::Failed => None,
                ResolveState::Resolving => {
                    if let Some(def) = self.defs.get(normalized) {
                        self.push_diagnostic(
                            "cycle",
                            def.line + 1,
                            format!("cycle detected for variable '{}'", def.name),
                        );
                    }
                    self.states
                        .insert(normalized.to_string(), ResolveState::Failed);
                    None
                }
            };
        }

        let def = self.defs.get(normalized)?.clone();
        self.states
            .insert(normalized.to_string(), ResolveState::Resolving);

        let substituted = match self.substitute_runtime(&def.expression, Some(def.line)) {
            Some(value) => value,
            None => {
                self.states
                    .insert(normalized.to_string(), ResolveState::Failed);
                return None;
            }
        };

        let raw_value = match self.eval_raw(&substituted) {
            Some(value) => value,
            None => {
                self.push_diagnostic(
                    "invalid-expression",
                    def.line + 1,
                    format!("failed to evaluate expression for variable '{}'", def.name),
                );
                self.states
                    .insert(normalized.to_string(), ResolveState::Failed);
                return None;
            }
        };

        let numeric = match extract_first_number(&raw_value) {
            Some(value) => value,
            None => {
                self.push_diagnostic(
                    "non-numeric",
                    def.line + 1,
                    format!(
                        "variable '{}' did not evaluate to a numeric value",
                        def.name
                    ),
                );
                self.states
                    .insert(normalized.to_string(), ResolveState::Failed);
                return None;
            }
        };

        let formatted = format_number(numeric);
        self.states
            .insert(normalized.to_string(), ResolveState::Resolved);
        self.values
            .insert(normalized.to_string(), formatted.clone());
        Some(formatted)
    }

    fn substitute_runtime(
        &mut self,
        expression: &str,
        owner_line: Option<usize>,
    ) -> Option<String> {
        let matches = self.find_matches(expression);
        if matches.is_empty() {
            return Some(expression.to_string());
        }

        let mut out = String::with_capacity(expression.len());
        let mut cursor = 0usize;
        for span in matches {
            out.push_str(&expression[cursor..span.start]);
            let Some(value) = self.resolve(&span.normalized) else {
                if let Some(line) = owner_line {
                    self.push_diagnostic(
                        "unresolved-variable",
                        line + 1,
                        format!("unresolved variable '{}'", span.normalized),
                    );
                }
                return None;
            };

            out.push_str(&value);
            cursor = span.end;
        }
        out.push_str(&expression[cursor..]);

        Some(out)
    }

    fn push_diagnostic(&mut self, kind: &str, line: usize, message: String) {
        if self
            .diagnostics
            .iter()
            .any(|d| d.kind == kind && d.line == line && d.message == message)
        {
            return;
        }

        self.diagnostics.push(NoteEvaluationDiagnostic {
            kind: kind.to_string(),
            line,
            message,
        });
    }
}

impl CalcEngine {
    pub fn new() -> Self {
        Self
    }

    pub fn evaluate(&self, input: &str) -> Option<String> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return None;
        }
        if !has_calc_signal(trimmed) {
            return None;
        }

        let (expr, applied_result) = split_applied_result(trimmed);
        let text = evaluate_raw_expression(expr)?;
        if text == expr {
            return None;
        }
        if applied_result
            .map(|applied| applied.trim() == text)
            .unwrap_or(false)
        {
            return None;
        }

        Some(text)
    }

    pub fn evaluate_lines(&self, lines: &[String]) -> Vec<Option<String>> {
        lines.iter().map(|line| self.evaluate(line)).collect()
    }

    pub fn evaluate_note_context(
        &self,
        lines: &[String],
        options: NoteEvaluationOptions,
    ) -> NoteEvaluationResult {
        let defs = if options.variables_enabled {
            collect_variable_definitions(lines)
        } else {
            HashMap::new()
        };
        let variables = variable_index_from_definitions(&defs);

        let line_count = lines.len();
        let (eval_from, eval_to) = match options.eval_range {
            Some((from, to)) => (from.min(line_count), to.min(line_count)),
            None => (0, line_count),
        };
        let mut resolver = VariableResolver::new(&defs);
        let is_full_eval = eval_from == 0 && eval_to == line_count;
        if options.variables_enabled && is_full_eval {
            let mut names: Vec<String> = defs.keys().cloned().collect();
            names.sort();
            for normalized in names {
                let _ = resolver.resolve(&normalized);
            }
        }

        let mut line_results: Vec<Option<String>> = vec![None; line_count];
        for (idx, line) in lines
            .iter()
            .enumerate()
            .skip(eval_from)
            .take(eval_to.saturating_sub(eval_from))
        {
            let Some(expression) = expression_for_ghost_eval(line) else {
                continue;
            };

            let result = if options.variables_enabled {
                if let Some((_name, normalized, _rhs)) = parse_variable_assignment(&expression) {
                    resolver.resolve(&normalized)
                } else {
                    evaluate_expression_with_variables(&expression, &mut resolver)
                }
            } else {
                self.evaluate(&expression)
            };

            line_results[idx] = result;
        }

        NoteEvaluationResult {
            line_results,
            variables,
            diagnostics: resolver.diagnostics(),
        }
    }
}

fn evaluate_expression_with_variables(
    raw_input: &str,
    resolver: &mut VariableResolver<'_>,
) -> Option<String> {
    let trimmed = raw_input.trim();
    if trimmed.is_empty() || looks_like_date(trimmed) {
        return None;
    }

    let (expr, applied_result) = split_applied_result(trimmed);
    let has_var_refs = resolver.expression_references_variable(expr);
    if !has_calc_signal(expr) && !has_var_refs {
        return None;
    }

    let substituted = if has_var_refs {
        resolver.substitute_runtime(expr, None)?
    } else {
        expr.to_string()
    };

    let text = resolver.eval_raw(&substituted)?;
    if text == expr {
        return None;
    }
    if applied_result
        .map(|applied| applied.trim() == text)
        .unwrap_or(false)
    {
        return None;
    }

    Some(text)
}

fn evaluate_raw_expression(expr: &str) -> Option<String> {
    let mut ctx = new_context();
    match fend_core::evaluate_with_interrupt(expr, &mut ctx, &NO_INTERRUPT) {
        Ok(result) => Some(result.get_main_result().to_string()),
        Err(_) => None,
    }
}

fn new_context() -> fend_core::Context {
    let mut ctx = fend_core::Context::new();
    ctx.set_random_u32_fn(|| {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        RandomState::new().build_hasher().finish() as u32
    });
    ctx
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

fn looks_like_assignment(text: &str) -> bool {
    parse_variable_assignment(text).is_some()
}

fn parse_variable_assignment(text: &str) -> Option<(String, String, String)> {
    let idx = text.find(":=")?;
    let left = collapse_spaces(text[..idx].trim());
    let right = text[idx + 2..].trim();
    let (rhs_expr, _applied) = split_applied_result(right);
    let right = rhs_expr.trim();

    if left.is_empty() || right.is_empty() || !is_valid_variable_name(&left) {
        return None;
    }

    Some((left.clone(), left.to_lowercase(), right.to_string()))
}

fn is_table_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.ends_with('|')
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

fn list_body_segment(line: &str) -> Option<String> {
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

    Some(line[i..end].to_string())
}

fn table_expression_segment(line: &str, allow_assignments: bool) -> Option<String> {
    if !is_table_line(line) {
        return None;
    }

    let bytes = line.as_bytes();
    let mut pipes = Vec::new();
    for (idx, b) in bytes.iter().enumerate() {
        if *b == b'|' {
            pipes.push(idx);
        }
    }
    if pipes.len() < 2 {
        return None;
    }

    let mut candidates = Vec::new();
    for pair in pipes.windows(2) {
        let start = pair[0] + 1;
        let end = pair[1];
        if start >= end {
            continue;
        }

        let raw = &line[start..end];
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }

        let qualifies =
            has_calc_signal(trimmed) || (allow_assignments && looks_like_assignment(trimmed));
        if !qualifies {
            continue;
        }

        candidates.push(trimmed.to_string());
    }

    if candidates.len() != 1 {
        return None;
    }

    candidates.pop()
}

fn expression_for_variable_scan(line: &str) -> Option<String> {
    if is_table_line(line) {
        return table_expression_segment(line, true);
    }

    if let Some(body) = list_body_segment(line) {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            return None;
        }
        return Some(trimmed.to_string());
    }

    let trimmed = line.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn expression_for_ghost_eval(line: &str) -> Option<String> {
    if is_table_line(line) {
        return table_expression_segment(line, true);
    }

    if let Some(body) = list_body_segment(line) {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            return None;
        }
        return Some(trimmed.to_string());
    }

    let trimmed = line.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn collect_variable_definitions(lines: &[String]) -> HashMap<String, VariableDefinition> {
    let mut defs = HashMap::new();

    for (line_idx, line) in lines.iter().enumerate() {
        let Some(expression) = expression_for_variable_scan(line) else {
            continue;
        };

        let Some((name, normalized, rhs)) = parse_variable_assignment(&expression) else {
            continue;
        };

        defs.insert(
            normalized.clone(),
            VariableDefinition {
                name,
                normalized,
                expression: rhs,
                line: line_idx,
            },
        );
    }

    defs
}

fn variable_index_from_definitions(
    defs: &HashMap<String, VariableDefinition>,
) -> Vec<VariableIndexEntry> {
    let mut entries = defs
        .values()
        .map(|def| VariableIndexEntry {
            name: def.name.clone(),
            normalized: def.normalized.clone(),
            line: def.line + 1,
        })
        .collect::<Vec<_>>();

    entries.sort_by(|a, b| a.normalized.cmp(&b.normalized).then(a.line.cmp(&b.line)));
    entries
}

fn build_variable_regex(names_sorted: &[String]) -> Option<Regex> {
    let escaped: Vec<String> = names_sorted
        .iter()
        .filter(|n| !n.is_empty())
        .map(|n| regex::escape(n))
        .collect();
    if escaped.is_empty() {
        return None;
    }
    // unicode(false) makes \b use ASCII word-char semantics ([A-Za-z0-9_]),
    // matching the previous hand-rolled boundary check exactly.
    let pattern = format!(r"\b(?:{})\b", escaped.join("|"));
    RegexBuilder::new(&pattern)
        .unicode(false)
        .case_insensitive(true)
        .build()
        .ok()
}

fn format_number(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    if (value - value.round()).abs() < 1e-9 {
        return format!("{}", value.round() as i64);
    }

    format!("{value:.10}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn extract_first_number(text: &str) -> Option<f64> {
    let cleaned = text.replace(',', "");
    cleaned
        .split(|c: char| !c.is_ascii_digit() && c != '.' && c != '-' && c != '+')
        .filter_map(|token| {
            let token = token.trim();
            if token.is_empty() || matches!(token, "+" | "-" | ".") {
                return None;
            }
            token.parse::<f64>().ok().filter(|v| v.is_finite())
        })
        .next()
}

fn has_calc_signal(s: &str) -> bool {
    if looks_like_date(s) {
        return false;
    }

    s.bytes().any(|b| {
        matches!(
            b,
            b'+' | b'-' | b'*' | b'/' | b'^' | b'%' | b'(' | b'0'..=b'9'
        )
    }) || s.contains(" to ")
        || s.contains(" in ")
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

    // YYYY-MM-DD
    if parse_u32_len(a, 4, 4).is_some() {
        if let (Some(month), Some(day)) = (parse_u32_len(b, 1, 2), parse_u32_len(c, 1, 2)) {
            if (1..=12).contains(&month) && (1..=31).contains(&day) {
                return true;
            }
        }
    }

    // DD/MM/YYYY or MM/DD/YYYY (accept either to avoid locale coupling).
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

fn split_applied_result(s: &str) -> (&str, Option<&str>) {
    // if line ends with " = <something>", evaluate just the left side
    if let Some(idx) = s.rfind(" = ") {
        let left = s[..idx].trim();
        if !left.is_empty() {
            let right = s[idx + 3..].trim();
            if !right.is_empty() {
                return (left, Some(right));
            }
            return (left, None);
        }
    }
    (s, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_basic_arithmetic() {
        let engine = CalcEngine::new();
        assert_eq!(engine.evaluate("200 * 1.19"), Some("238".to_string()));
    }

    #[test]
    fn ignores_non_expression_lines() {
        let engine = CalcEngine::new();
        assert_eq!(engine.evaluate("todo buy milk"), None);
        assert_eq!(engine.evaluate(""), None);
    }

    #[test]
    fn reevaluates_lines_with_applied_result() {
        let engine = CalcEngine::new();
        assert_eq!(engine.evaluate("2 + 2 = 4"), None);
        assert_eq!(engine.evaluate("2 + 2 = 5"), Some("4".to_string()));
        assert_eq!(split_applied_result("2 + 2 = 4"), ("2 + 2", Some("4")));
    }

    #[test]
    fn evaluates_multiple_lines() {
        let engine = CalcEngine::new();
        let lines = vec![
            "2+2".to_string(),
            "not calc".to_string(),
            "10/2".to_string(),
        ];
        let results = engine.evaluate_lines(&lines);
        assert_eq!(
            results,
            vec![Some("4".to_string()), None, Some("5".to_string())]
        );
    }

    #[test]
    fn ignores_date_like_lines() {
        let engine = CalcEngine::new();
        assert_eq!(engine.evaluate("2026-07-03"), None);
        assert_eq!(engine.evaluate("03.07.2026"), None);
        assert_eq!(engine.evaluate("07/03/2026"), None);
        assert_eq!(engine.evaluate("7/3/2026"), None);
    }

    #[test]
    fn date_detection_does_not_hide_non_date_expressions() {
        assert!(looks_like_date("2026-07-03"));
        assert!(looks_like_date("03.07.2026"));
        assert!(looks_like_date("07/03/2026"));
        assert!(!looks_like_date("2026-07-03 + 2"));
        assert!(!looks_like_date("2 + 2"));
    }

    #[test]
    fn eval_is_line_local_without_cross_line_state() {
        let engine = CalcEngine::new();
        // Setting a variable in one line should not impact another line evaluation call.
        let _ = engine.evaluate("x = 10");
        assert_eq!(engine.evaluate("x + 2"), None);
    }

    #[test]
    fn note_eval_resolves_variables_reactively() {
        let engine = CalcEngine::new();
        let lines = vec![
            "Subtotal := 10".to_string(),
            "tax := subtotal * 0.2".to_string(),
            "subtotal + tax".to_string(),
        ];

        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(
            result.line_results,
            vec![
                Some("10".to_string()),
                Some("2".to_string()),
                Some("12".to_string())
            ]
        );

        let vars = result
            .variables
            .iter()
            .map(|entry| entry.normalized.clone())
            .collect::<Vec<_>>();
        assert_eq!(vars, vec!["subtotal", "tax"]);
    }

    #[test]
    fn note_eval_assignment_lines_hide_unresolved_values() {
        let engine = CalcEngine::new();
        let lines = vec!["x := y + 1".to_string(), "x + 1".to_string()];

        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(result.line_results, vec![None, None]);
        assert!(result.diagnostics.is_some());
    }

    #[test]
    fn note_eval_detects_cycles() {
        let engine = CalcEngine::new();
        let lines = vec![
            "a := b + 1".to_string(),
            "b := a + 1".to_string(),
            "a + b".to_string(),
        ];

        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(result.line_results, vec![None, None, None]);

        let diagnostics = result.diagnostics.unwrap_or_default();
        assert!(diagnostics.iter().any(|d| d.kind == "cycle"));
    }

    #[test]
    fn note_eval_supports_list_and_table_context_with_variables() {
        let engine = CalcEngine::new();
        let lines = vec![
            "base := 4".to_string(),
            "- base + 2".to_string(),
            "| item | base + 3 |".to_string(),
        ];

        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(
            result.line_results,
            vec![Some("4".to_string()), Some("6".to_string()), Some("7".to_string())]
        );
    }

    #[test]
    fn note_eval_conversion_assignment_is_numeric_only() {
        let engine = CalcEngine::new();
        let lines = vec!["len := 3 m to km".to_string(), "len + 0".to_string()];

        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        let value = result.line_results[1].clone().unwrap_or_default();
        assert!(!value.is_empty());
        assert!(!value.chars().any(|ch| ch.is_ascii_alphabetic()));
    }

    #[test]
    fn note_eval_can_disable_variables() {
        let engine = CalcEngine::new();
        let lines = vec!["x := 10".to_string(), "x + 1".to_string()];

        let result = engine.evaluate_note_context(
            &lines,
            NoteEvaluationOptions {
                variables_enabled: false,
                eval_range: None,
            },
        );

        assert_eq!(result.line_results, vec![None, None]);
        assert!(result.variables.is_empty());
    }

    #[test]
    fn note_eval_accepts_assignment_without_spaces_around_colon_equals() {
        let engine = CalcEngine::new();
        let lines = vec!["len:=4 km to m".to_string(), "len + 1".to_string()];

        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(result.line_results[0], Some("4000".to_string()));
        assert_eq!(result.line_results[1], Some("4001".to_string()));
        assert!(result
            .variables
            .iter()
            .any(|entry| entry.normalized == "len" && entry.line == 1));
    }

    #[test]
    fn note_eval_table_assignment_line_emits_assigned_value() {
        let engine = CalcEngine::new();
        let lines = vec![
            "| total := 12 |".to_string(),
            "| value | total + 12 |".to_string(),
        ];

        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(
            result.line_results,
            vec![Some("12".to_string()), Some("24".to_string())]
        );
    }

    #[test]
    fn note_eval_ignores_assignment_trailer_literal_in_variable_rhs() {
        let engine = CalcEngine::new();
        let lines = vec![
            "total := 12 = 12".to_string(),
            "value := total + 12 = 24".to_string(),
            "value + 1".to_string(),
        ];

        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(
            result.line_results,
            vec![
                Some("12".to_string()),
                Some("24".to_string()),
                Some("25".to_string())
            ]
        );
    }

    #[test]
    fn note_eval_partial_range_only_evaluates_requested_lines() {
        let engine = CalcEngine::new();
        let lines = vec![
            "2 + 2".to_string(),
            "10 / 5".to_string(),
            "3 * 3".to_string(),
            "100 - 1".to_string(),
        ];

        let result = engine.evaluate_note_context(
            &lines,
            NoteEvaluationOptions {
                variables_enabled: true,
                eval_range: Some((1, 3)),
            },
        );

        assert_eq!(
            result.line_results,
            vec![None, Some("2".to_string()), Some("9".to_string()), None]
        );
    }

    #[test]
    fn note_eval_partial_range_still_resolves_variables_defined_outside_range() {
        let engine = CalcEngine::new();
        let lines = vec![
            "base := 10".to_string(),
            "base + 1".to_string(),
            "base + 2".to_string(),
            "base + 3".to_string(),
        ];

        // Only re-evaluate line index 2; it should still see `base` defined at line 0.
        let result = engine.evaluate_note_context(
            &lines,
            NoteEvaluationOptions {
                variables_enabled: true,
                eval_range: Some((2, 3)),
            },
        );

        assert_eq!(
            result.line_results,
            vec![None, None, Some("12".to_string()), None]
        );
        assert_eq!(result.variables.len(), 1);
    }

    #[test]
    fn note_eval_does_not_treat_plain_colon_as_assignment() {
        let engine = CalcEngine::new();
        let lines = vec!["len: 4 km to m".to_string(), "len + 1".to_string()];

        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert!(result.variables.is_empty());
        assert_eq!(result.line_results[1], None);
    }
}
