use aho_corasick::AhoCorasick;
use regex::Regex;
use table_syntax::{is_table_line, split_table_cells, table_pipe_positions};
use rustc_hash::{FxHashMap, FxHashSet, FxHasher};
use std::borrow::Cow;
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::cmp::Reverse;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

pub struct CalcEngine;

/// A variable value imported from another note for cross-note calc evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct ExternVar {
    /// 8-char lowercase alphanumeric short ID of the source note.
    pub note_short_id: String,
    /// Normalized (lowercased, spaces collapsed) variable name.
    pub var_normalized: String,
    pub value: f64,
}

/// A cross-note variable reference found while scanning a note's lines.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct CrossNoteRef {
    /// 8-char lowercase alphanumeric short ID of the referenced note.
    pub note_short_id: String,
    /// Normalized variable name referenced from the other note.
    pub var_normalized: String,
    /// 1-based line number where the reference appears.
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NoteEvaluationOptions {
    pub variables_enabled: bool,
    pub table_enabled: bool,
    /// Controls whether `[[SHORTID]].var_name` cross-note references are scanned
    /// and substituted. When `false`, cross-note refs are left as-is (no ghost output).
    pub cross_note_enabled: bool,
    /// Optional half-open range `[from, to)` of line indices (0-based) to evaluate.
    /// When `None`, evaluates every line. Variable resolution always considers the
    /// full document so that a restricted evaluation still sees vars defined elsewhere.
    /// Positions outside the range are returned as `None` in `line_results`.
    pub eval_range: Option<(usize, usize)>,
    /// Values from other notes to substitute for `[[SHORTID]].var_name` references.
    /// Only used when `cross_note_enabled` is true.
    pub extern_vars: Vec<ExternVar>,
    /// Pre-scanned cross-note refs for this note. When `Some`, the engine skips
    /// its own `scan_cross_note_refs` call and uses this directly. Pass the result
    /// of a prior `scan_cross_note_refs` call to avoid rescanning on every eval.
    pub precomputed_refs: Option<Vec<CrossNoteRef>>,
}

impl Default for NoteEvaluationOptions {
    fn default() -> Self {
        Self {
            variables_enabled: true,
            table_enabled: true,
            cross_note_enabled: true,
            eval_range: None,
            extern_vars: Vec::new(),
            precomputed_refs: None,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoteEvaluationResult {
    pub line_results: Vec<Option<String>>,
    pub variables: Vec<VariableIndexEntry>,
    pub diagnostics: Option<Vec<NoteEvaluationDiagnostic>>,
    /// Per-line list of (cell_index, value) for every formula cell that produced
    /// a result. Lines without table formulas have an empty inner Vec.
    /// Within a row, cells are evaluated left-to-right; rows are evaluated
    /// top-to-bottom and earlier results are visible to subsequent formulas.
    #[serde(default)]
    pub table_cell_results: Vec<Vec<TableCellEvaluation>>,
    /// Resolved numeric values for each note-local variable (normalized name → f64).
    /// Populated only when `variables_enabled` is true. Used for cross-note export.
    #[serde(default, skip_serializing_if = "rustc_hash::FxHashMap::is_empty")]
    pub variable_values: rustc_hash::FxHashMap<String, f64>,
    /// Cross-note variable references found in this note's lines.
    /// Used by the caller to maintain the dependency graph.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cross_note_refs: Vec<CrossNoteRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TableCellEvaluation {
    pub cell_index: usize,
    pub value: String,
    pub error_kind: Option<TableCellErrorKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TableCellErrorKind {
    OutOfBounds,
    NonNumeric,
    SelfReference,
    Cycle,
    Unknown,
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

/// A variable definition being resolved: its references are substituted
/// left to right, pausing while an unresolved one is resolved first.
struct ResolveFrame {
    normalized: String,
    name: String,
    line: usize,
    expression: String,
    matches: Vec<MatchSpan>,
    /// Index into `matches` of the reference being substituted.
    next: usize,
    /// Byte offset in `expression` copied into `substituted` so far.
    cursor: usize,
    substituted: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolveState {
    Resolving,
    Resolved,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormulaOp {
    Sum,
    Avg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormulaScope {
    Row,
    Column,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FormulaSpec {
    op: FormulaOp,
    scope: FormulaScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LineExpression {
    expression: String,
    table_cell_index: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FormulaCallSpan {
    start: usize,
    end: usize,
    spec: FormulaSpec,
}

#[derive(Debug, Clone)]
struct TableBlockInfo {
    data_rows: Vec<Vec<usize>>,
    row_lookup: FxHashMap<usize, usize>,
}

#[derive(Default)]
struct TableEvalCache {
    split_cells: FxHashMap<usize, Arc<Vec<String>>>,
    block_by_line: FxHashMap<usize, Option<TableBlockInfo>>,
}

impl TableEvalCache {
    fn cells_for_line<'a>(
        &'a mut self,
        lines: &[String],
        line_idx: usize,
    ) -> Option<&'a Arc<Vec<String>>> {
        if !self.split_cells.contains_key(&line_idx) {
            let line = lines.get(line_idx)?;
            self.split_cells
                .insert(line_idx, Arc::new(split_table_cells(line)));
        }
        self.split_cells.get(&line_idx)
    }

    fn block_for_line(&mut self, lines: &[String], line_idx: usize) -> Option<TableBlockInfo> {
        if let Some(cached) = self.block_by_line.get(&line_idx) {
            return cached.clone();
        }
        let Some((start, end)) = table_syntax::table_block_bounds(lines, line_idx) else {
            self.block_by_line.insert(line_idx, None);
            return None;
        };

        let mut delimiter_row: Option<usize> = None;
        let mut data_rows: Vec<Vec<usize>> = Vec::new();
        let mut row_lookup: FxHashMap<usize, usize> = FxHashMap::default();
        let after_header = table_syntax::after_header_row(end + 1 - start, |idx| {
            table_syntax::is_table_continuation_line(&lines[start + idx])
        })
        .map(|idx| start + idx);
        for row_idx in start..=end {
            let row_cells = self.cells_for_line(lines, row_idx)?.clone();
            if delimiter_row.is_none()
                && table_syntax::is_delimiter_row_at(&row_cells, Some(row_idx) == after_header)
            {
                delimiter_row = Some(row_idx);
                continue;
            }
            if delimiter_row.is_some() {
                if table_syntax::is_table_continuation_line(&lines[row_idx]) {
                    if let Some(last) = data_rows.last_mut() {
                        last.push(row_idx);
                        row_lookup.insert(row_idx, data_rows.len().saturating_sub(1));
                    } else {
                        data_rows.push(vec![row_idx]);
                        row_lookup.insert(row_idx, 0);
                    }
                } else {
                    let logical_idx = data_rows.len();
                    data_rows.push(vec![row_idx]);
                    row_lookup.insert(row_idx, logical_idx);
                }
            }
        }

        let info = TableBlockInfo {
            data_rows,
            row_lookup,
        };
        for row_idx in start..=end {
            self.block_by_line.insert(row_idx, Some(info.clone()));
        }
        Some(info)
    }
}

struct VariableResolver<'a> {
    defs: &'a FxHashMap<String, VariableDefinition>,
    variable_matcher: Option<VariableMatcher>,
    states: FxHashMap<String, ResolveState>,
    values: FxHashMap<String, String>,
    diagnostics: Vec<NoteEvaluationDiagnostic>,
    raw_eval_cache: FxHashMap<String, Option<String>>,
}

/// Variables already resolved against one set of definitions.
#[derive(Default)]
struct ResolvedVariables {
    states: FxHashMap<String, ResolveState>,
    values: FxHashMap<String, String>,
}

/// The whole-document part of a note evaluation: cross-note substitution,
/// variable definitions and the variables resolved so far. Evaluating another
/// range of the same note state reuses it instead of rescanning every line.
struct PreparedNoteContext {
    /// Identifies the options and extern values; a change rebuilds everything.
    options_key: u64,
    /// Hash of each line it was prepared from, to find what an edit changed.
    line_hashes: Vec<u64>,
    /// The note with cross-note references replaced by their values; `None`
    /// when nothing was substituted and the original lines apply.
    eval_lines: Option<Vec<String>>,
    lines_with_unresolved: FxHashSet<usize>,
    cross_note_refs: Vec<CrossNoteRef>,
    /// The definition each line makes, if any (empty when variables are off).
    line_defs: Vec<Option<VariableDefinition>>,
    defs: FxHashMap<String, VariableDefinition>,
    variables: Vec<VariableIndexEntry>,
    names_sorted: Vec<String>,
    variable_matcher: Option<VariableMatcher>,
    resolved: ResolvedVariables,
    /// Scratch copy of the evaluated lines that table formulas write their
    /// values into; rows written by an evaluation are restored after it.
    working_lines: Option<Vec<String>>,
}

/// Reusable state for `CalcEngine::evaluate_note_context_cached`.
#[derive(Default)]
pub struct NoteContextCache(Option<PreparedNoteContext>);

static VARIABLE_MATCHER_CACHE: OnceLock<Mutex<FxHashMap<u64, VariableMatcher>>> = OnceLock::new();
static TABLE_COORD_REF_RE: OnceLock<Regex> = OnceLock::new();
static CROSS_NOTE_REF_RE: OnceLock<Regex> = OnceLock::new();

static EVAL_GENERATION: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static ACTIVE_EVAL_GENERATION: Cell<u64> = const { Cell::new(0) };
}

pub fn start_eval_generation() -> u64 {
    EVAL_GENERATION.fetch_add(1, Ordering::SeqCst) + 1
}

pub fn current_eval_generation() -> u64 {
    EVAL_GENERATION.load(Ordering::Relaxed)
}

fn with_eval_generation<R>(generation: u64, f: impl FnOnce() -> R) -> R {
    ACTIVE_EVAL_GENERATION.with(|active| {
        let previous = active.replace(generation);
        let result = f();
        active.set(previous);
        result
    })
}

struct GenerationInterrupt;
impl fend_core::Interrupt for GenerationInterrupt {
    fn should_interrupt(&self) -> bool {
        ACTIVE_EVAL_GENERATION.with(|active| {
            let expected = active.get();
            expected != 0 && EVAL_GENERATION.load(Ordering::Relaxed) != expected
        })
    }
}

static GENERATION_INTERRUPT: GenerationInterrupt = GenerationInterrupt;

const TABLE_REF_ERROR_OUT_OF_BOUNDS: &str = "!ERROR#out_of_bounds";
const TABLE_REF_ERROR_NON_NUMERIC: &str = "!ERROR#non_numeric";
const TABLE_REF_ERROR_SELF_REFERENCE: &str = "!ERROR#self_reference";
const TABLE_REF_ERROR_CYCLE: &str = "!ERROR#cycle";

fn table_cell_error_kind(value: &str) -> Option<TableCellErrorKind> {
    match value {
        TABLE_REF_ERROR_OUT_OF_BOUNDS => Some(TableCellErrorKind::OutOfBounds),
        TABLE_REF_ERROR_NON_NUMERIC => Some(TableCellErrorKind::NonNumeric),
        TABLE_REF_ERROR_SELF_REFERENCE => Some(TableCellErrorKind::SelfReference),
        TABLE_REF_ERROR_CYCLE => Some(TableCellErrorKind::Cycle),
        _ => value
            .strip_prefix("!ERROR#")
            .map(|_| TableCellErrorKind::Unknown),
    }
}

fn table_ref_error_code(value: &str) -> Option<String> {
    value.strip_prefix("!ERROR#").map(ToOwned::to_owned)
}

impl<'a> VariableResolver<'a> {
    /// A resolver that starts from `resolved`, the states and values an
    /// earlier evaluation of the same definitions left behind.
    fn with_resolved(
        defs: &'a FxHashMap<String, VariableDefinition>,
        variable_matcher: Option<VariableMatcher>,
        resolved: ResolvedVariables,
    ) -> Self {
        Self {
            defs,
            variable_matcher,
            states: resolved.states,
            values: resolved.values,
            diagnostics: Vec::new(),
            raw_eval_cache: FxHashMap::default(),
        }
    }

    fn into_resolved(self) -> ResolvedVariables {
        ResolvedVariables {
            states: self.states,
            values: self.values,
        }
    }

    fn find_matches(&self, expression: &str) -> Vec<MatchSpan> {
        let Some(matcher) = &self.variable_matcher else {
            return Vec::new();
        };
        matcher
            .find(expression)
            .into_iter()
            .map(|(start, end)| MatchSpan {
                normalized: expression[start..end].to_ascii_lowercase(),
                start,
                end,
            })
            .collect()
    }

    fn expression_references_variable(&self, expression: &str) -> bool {
        self.variable_matcher
            .as_ref()
            .is_some_and(|matcher| matcher.is_match(expression))
    }

    fn eval_raw(&mut self, expr: &str, ctx: &mut fend_core::Context) -> Option<String> {
        if let Some(cached) = self.raw_eval_cache.get(expr) {
            return cached.clone();
        }
        let result = evaluate_raw_expression(expr, ctx);
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

    /// Resolves a variable and, first, everything its definition depends on.
    /// Dependencies are walked with an explicit stack, not recursion, so long
    /// chains (`total 2 := total 1 + x`, ...) cannot overflow the stack.
    fn resolve(&mut self, normalized: &str, ctx: &mut fend_core::Context) -> Option<String> {
        if let Some(known) = self.known_resolution(normalized) {
            return known;
        }
        let mut stack = vec![self.start_resolving(normalized)?];
        // Result of the dependency that was just resolved, for the frame on top.
        let mut dependency: Option<Option<String>> = None;
        loop {
            let frame = stack.last_mut().expect("frame to resolve");
            if let Some(result) = dependency.take() {
                let span = &frame.matches[frame.next];
                let Some(value) = result else {
                    let (name, line) = (frame.normalized.clone(), frame.line);
                    let message = format!("unresolved variable '{}'", span.normalized);
                    stack.pop();
                    self.push_diagnostic("unresolved-variable", line + 1, message);
                    self.states.insert(name, ResolveState::Failed);
                    if stack.is_empty() {
                        return None;
                    }
                    dependency = Some(None);
                    continue;
                };
                frame.substituted.push_str(&value);
                frame.cursor = span.end;
                frame.next += 1;
            }

            if let Some(span) = frame.matches.get(frame.next) {
                frame
                    .substituted
                    .push_str(&frame.expression[frame.cursor..span.start]);
                let name = span.normalized.clone();
                match self.known_resolution(&name) {
                    Some(known) => dependency = Some(known),
                    None => match self.start_resolving(&name) {
                        Some(next) => stack.push(next),
                        None => dependency = Some(None),
                    },
                }
                continue;
            }

            let mut frame = stack.pop().expect("frame to finish");
            frame
                .substituted
                .push_str(&frame.expression[frame.cursor..]);
            let result = self.finish_resolving(&frame, ctx);
            if stack.is_empty() {
                return result;
            }
            dependency = Some(result);
        }
    }

    /// The outcome for a variable that was already visited, or `None` when it
    /// still needs resolving. Reaching one that is mid-resolution is a cycle.
    fn known_resolution(&mut self, normalized: &str) -> Option<Option<String>> {
        let state = self.states.get(normalized).copied()?;
        Some(match state {
            ResolveState::Resolved => self.values.get(normalized).cloned(),
            ResolveState::Failed => None,
            ResolveState::Resolving => {
                if let Some(def) = self.defs.get(normalized) {
                    let (line, name) = (def.line, def.name.clone());
                    self.push_diagnostic(
                        "cycle",
                        line + 1,
                        format!("cycle detected for variable '{name}'"),
                    );
                }
                self.states
                    .insert(normalized.to_string(), ResolveState::Failed);
                None
            }
        })
    }

    /// Marks a defined variable as resolving and lists the variables its
    /// expression references; `None` when the name has no definition.
    fn start_resolving(&mut self, normalized: &str) -> Option<ResolveFrame> {
        let def = self.defs.get(normalized)?;
        let frame = ResolveFrame {
            normalized: normalized.to_string(),
            name: def.name.clone(),
            line: def.line,
            expression: def.expression.clone(),
            matches: self.find_matches(&def.expression),
            next: 0,
            cursor: 0,
            substituted: String::with_capacity(def.expression.len()),
        };
        self.states
            .insert(normalized.to_string(), ResolveState::Resolving);
        Some(frame)
    }

    /// Evaluates a definition whose references are all substituted.
    fn finish_resolving(
        &mut self,
        frame: &ResolveFrame,
        ctx: &mut fend_core::Context,
    ) -> Option<String> {
        let (kind, message) = match self.eval_raw(&frame.substituted, ctx) {
            None => (
                "invalid-expression",
                format!(
                    "failed to evaluate expression for variable '{}'",
                    frame.name
                ),
            ),
            Some(raw_value) => match extract_first_number(&raw_value) {
                Some(numeric) => {
                    let formatted = format_number(numeric);
                    self.states
                        .insert(frame.normalized.clone(), ResolveState::Resolved);
                    self.values
                        .insert(frame.normalized.clone(), formatted.clone());
                    return Some(formatted);
                }
                None => (
                    "non-numeric",
                    format!(
                        "variable '{}' did not evaluate to a numeric value",
                        frame.name
                    ),
                ),
            },
        };
        self.push_diagnostic(kind, frame.line + 1, message);
        self.states
            .insert(frame.normalized.clone(), ResolveState::Failed);
        None
    }

    fn substitute_runtime(
        &mut self,
        expression: &str,
        owner_line: Option<usize>,
        ctx: &mut fend_core::Context,
    ) -> Option<String> {
        let matches = self.find_matches(expression);
        if matches.is_empty() {
            return Some(expression.to_string());
        }

        let mut out = String::with_capacity(expression.len());
        let mut cursor = 0usize;
        for span in matches {
            out.push_str(&expression[cursor..span.start]);
            let Some(value) = self.resolve(&span.normalized, ctx) else {
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

    fn numeric_values(&self) -> FxHashMap<String, f64> {
        self.values
            .iter()
            .filter_map(|(k, v)| extract_first_number(v).map(|n| (k.clone(), n)))
            .collect()
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

fn variable_matcher_cache_key(names_sorted: &[String]) -> u64 {
    let mut hasher = FxHasher::default();
    names_sorted.len().hash(&mut hasher);
    for name in names_sorted {
        name.hash(&mut hasher);
        '\u{1f}'.hash(&mut hasher);
    }
    hasher.finish()
}

fn cached_variable_matcher(names_sorted: &[String]) -> Option<VariableMatcher> {
    if names_sorted.is_empty() {
        return None;
    }

    let key = variable_matcher_cache_key(names_sorted);
    let cache = VARIABLE_MATCHER_CACHE.get_or_init(|| Mutex::new(FxHashMap::default()));
    if let Ok(guard) = cache.lock() {
        if let Some(hit) = guard.get(&key) {
            return Some(hit.clone());
        }
    }

    let compiled = VariableMatcher::new(names_sorted)?;
    if let Ok(mut guard) = cache.lock() {
        if guard.len() >= 256 {
            guard.clear();
        }
        guard.insert(key, compiled.clone());
    }
    Some(compiled)
}

impl CalcEngine {
    pub fn new() -> Self {
        Self
    }

    pub fn evaluate(&self, input: &str) -> Option<String> {
        self.evaluate_with_generation(input, current_eval_generation())
    }

    pub fn evaluate_with_generation(&self, input: &str, generation: u64) -> Option<String> {
        with_eval_generation(generation, || {
            let mut ctx = new_context();
            evaluate_single(input, &mut ctx)
        })
    }

    pub fn evaluate_lines(&self, lines: &[String]) -> Vec<Option<String>> {
        self.evaluate_lines_with_generation(lines, current_eval_generation())
    }

    pub fn evaluate_lines_with_generation(
        &self,
        lines: &[String],
        generation: u64,
    ) -> Vec<Option<String>> {
        with_eval_generation(generation, || {
            let mut ctx = new_context();
            lines
                .iter()
                .map(|line| evaluate_single(line, &mut ctx))
                .collect()
        })
    }

    pub fn evaluate_note_context(
        &self,
        lines: &[String],
        options: NoteEvaluationOptions,
    ) -> NoteEvaluationResult {
        self.evaluate_note_context_with_generation(lines, options, current_eval_generation())
    }

    pub fn evaluate_note_context_with_generation(
        &self,
        lines: &[String],
        options: NoteEvaluationOptions,
        generation: u64,
    ) -> NoteEvaluationResult {
        with_eval_generation(generation, || {
            self.evaluate_note_context_inner(lines, options)
        })
    }

    /// Like `evaluate_note_context`, but keeps the whole-document preparation
    /// (cross-note substitution, variable definitions, resolved variables) in
    /// `cache` and reuses it while `lines` and the options stay the same, so
    /// evaluating successive ranges of an unchanged note only costs the range.
    /// Diagnostics then cover only variables resolved by this call.
    pub fn evaluate_note_context_cached(
        &self,
        lines: &[String],
        options: NoteEvaluationOptions,
        cache: &mut NoteContextCache,
    ) -> NoteEvaluationResult {
        let generation = current_eval_generation();
        with_eval_generation(generation, || {
            let options_key = note_options_key(&options);
            let line_hashes: Vec<u64> = lines.iter().map(|line| hash_line(line)).collect();
            let mut prepared = match cache.0.take() {
                Some(mut prepared) if prepared.options_key == options_key => {
                    if prepared.line_hashes != line_hashes {
                        update_prepared_note_context(&mut prepared, lines, &options, line_hashes);
                    }
                    prepared
                }
                _ => prepare_note_context(lines, &options, options_key, line_hashes),
            };
            let result = evaluate_prepared(lines, &mut prepared, options);
            // An interrupted evaluation may have recorded failures that are
            // not real; only keep state from a completed one.
            if current_eval_generation() == generation {
                cache.0 = Some(prepared);
            }
            result
        })
    }

    fn evaluate_note_context_inner(
        &self,
        lines: &[String],
        options: NoteEvaluationOptions,
    ) -> NoteEvaluationResult {
        let mut prepared = prepare_note_context(lines, &options, 0, Vec::new());
        evaluate_prepared(lines, &mut prepared, options)
    }
}

/// Identifies the options a `PreparedNoteContext` was built with.
fn note_options_key(options: &NoteEvaluationOptions) -> u64 {
    let mut hasher = FxHasher::default();
    (
        options.variables_enabled,
        options.table_enabled,
        options.cross_note_enabled,
    )
        .hash(&mut hasher);
    // Order-independent: the caller may list extern values in any order.
    let mut externs = 0u64;
    for var in &options.extern_vars {
        let mut entry = FxHasher::default();
        (&var.note_short_id, &var.var_normalized, var.value.to_bits()).hash(&mut entry);
        externs = externs.wrapping_add(entry.finish());
    }
    externs.hash(&mut hasher);
    hasher.finish()
}

fn hash_line(line: &str) -> u64 {
    let mut hasher = FxHasher::default();
    line.hash(&mut hasher);
    hasher.finish()
}

fn extern_value_map(options: &NoteEvaluationOptions) -> FxHashMap<(String, String), f64> {
    options
        .extern_vars
        .iter()
        .map(|ev| {
            (
                (ev.note_short_id.clone(), ev.var_normalized.clone()),
                ev.value,
            )
        })
        .collect()
}

/// Longest-first names, so leftmost-first regex alternation picks the
/// longest match.
fn names_longest_first(defs: &FxHashMap<String, VariableDefinition>) -> Vec<String> {
    let mut names: Vec<String> = defs.keys().cloned().collect();
    names.sort_by_key(|name| (Reverse(name.len()), name.clone()));
    names
}

/// The definitions in effect: the last assignment of each name wins.
fn definitions_from_lines(
    line_defs: &[Option<VariableDefinition>],
) -> FxHashMap<String, VariableDefinition> {
    let mut defs = FxHashMap::default();
    for def in line_defs.iter().flatten() {
        defs.insert(def.normalized.clone(), def.clone());
    }
    defs
}

fn prepare_note_context(
    lines: &[String],
    options: &NoteEvaluationOptions,
    options_key: u64,
    line_hashes: Vec<u64>,
) -> PreparedNoteContext {
    let cross_note_refs = if options.cross_note_enabled {
        options
            .precomputed_refs
            .clone()
            .unwrap_or_else(|| scan_cross_note_refs(lines))
    } else {
        Vec::new()
    };

    // Substitute known cross-note values. Only lines holding a reference can
    // change, and the rest of the note is copied only if one does.
    let mut eval_lines: Option<Vec<String>> = None;
    let mut lines_with_unresolved = FxHashSet::default();
    if options.cross_note_enabled && !options.extern_vars.is_empty() {
        let extern_map = extern_value_map(options);
        let mut previous_line = None;
        for reference in &cross_note_refs {
            let idx = reference.line - 1;
            if previous_line == Some(idx) || idx >= lines.len() {
                continue;
            }
            previous_line = Some(idx);
            let (new_line, has_unresolved) = preprocess_line_cross_note(&lines[idx], &extern_map);
            if has_unresolved {
                lines_with_unresolved.insert(idx);
            }
            if let Cow::Owned(rewritten) = new_line {
                eval_lines.get_or_insert_with(|| lines.to_vec())[idx] = rewritten;
            }
        }
    }

    let line_defs: Vec<Option<VariableDefinition>> = if options.variables_enabled {
        eval_lines
            .as_deref()
            .unwrap_or(lines)
            .iter()
            .enumerate()
            .map(|(idx, line)| line_variable_definition(line, idx, options.table_enabled))
            .collect()
    } else {
        Vec::new()
    };
    let defs = definitions_from_lines(&line_defs);
    let variables = variable_index_from_definitions(&defs);
    let names_sorted = names_longest_first(&defs);
    let variable_matcher = cached_variable_matcher(&names_sorted);

    PreparedNoteContext {
        options_key,
        line_hashes,
        eval_lines,
        lines_with_unresolved,
        cross_note_refs,
        line_defs,
        defs,
        variables,
        names_sorted,
        variable_matcher,
        resolved: ResolvedVariables::default(),
        working_lines: None,
    }
}

/// Brings `prepared` up to date with `lines` after an edit, redoing the
/// per-line work only for the lines that changed.
fn update_prepared_note_context(
    prepared: &mut PreparedNoteContext,
    lines: &[String],
    options: &NoteEvaluationOptions,
    line_hashes: Vec<u64>,
) {
    let Some((from, old_to, new_to)) = changed_line_span(&prepared.line_hashes, &line_hashes)
    else {
        prepared.line_hashes = line_hashes;
        return;
    };
    let delta = new_to as isize - old_to as isize;
    let shift = |idx: usize| idx.saturating_add_signed(delta);

    if options.cross_note_enabled {
        let mut refs = Vec::with_capacity(prepared.cross_note_refs.len());
        refs.extend(
            prepared
                .cross_note_refs
                .iter()
                .filter(|r| r.line - 1 < from)
                .cloned(),
        );
        refs.extend(
            scan_cross_note_refs(&lines[from..new_to])
                .into_iter()
                .map(|r| CrossNoteRef {
                    line: r.line + from,
                    ..r
                }),
        );
        refs.extend(
            prepared
                .cross_note_refs
                .iter()
                .filter(|r| r.line > old_to)
                .map(|r| CrossNoteRef {
                    line: shift(r.line - 1) + 1,
                    ..r.clone()
                }),
        );
        prepared.cross_note_refs = refs;
    }

    if options.cross_note_enabled && !options.extern_vars.is_empty() {
        let extern_map = extern_value_map(options);
        prepared.lines_with_unresolved = prepared
            .lines_with_unresolved
            .iter()
            .filter(|&&idx| idx < from || idx >= old_to)
            .map(|&idx| if idx < from { idx } else { shift(idx) })
            .collect();
        let mut span = Vec::with_capacity(new_to - from);
        let mut any_rewritten = false;
        for (offset, line) in lines[from..new_to].iter().enumerate() {
            let (new_line, has_unresolved) = preprocess_line_cross_note(line, &extern_map);
            if has_unresolved {
                prepared.lines_with_unresolved.insert(from + offset);
            }
            any_rewritten |= matches!(new_line, Cow::Owned(_));
            span.push(new_line.into_owned());
        }
        match prepared.eval_lines.as_mut() {
            Some(eval_lines) => {
                eval_lines.splice(from..old_to, span);
            }
            None if any_rewritten => {
                let mut eval_lines = lines.to_vec();
                eval_lines.splice(from..new_to, span);
                prepared.eval_lines = Some(eval_lines);
            }
            None => {}
        }
    }

    if options.variables_enabled {
        let eval_lines = prepared.eval_lines.as_deref().unwrap_or(lines);
        let span_defs: Vec<Option<VariableDefinition>> = (from..new_to)
            .map(|idx| line_variable_definition(&eval_lines[idx], idx, options.table_enabled))
            .collect();
        let definitions_changed = prepared.line_defs[from..old_to]
            .iter()
            .chain(&span_defs)
            .any(Option::is_some);
        prepared.line_defs.splice(from..old_to, span_defs);
        if delta != 0 {
            for (idx, def) in prepared.line_defs.iter_mut().enumerate().skip(new_to) {
                if let Some(def) = def {
                    def.line = idx;
                }
            }
        }
        if definitions_changed {
            prepared.defs = definitions_from_lines(&prepared.line_defs);
            prepared.variables = variable_index_from_definitions(&prepared.defs);
            let names_sorted = names_longest_first(&prepared.defs);
            if names_sorted != prepared.names_sorted {
                prepared.variable_matcher = cached_variable_matcher(&names_sorted);
                prepared.names_sorted = names_sorted;
            }
        } else if delta != 0 {
            // Same definitions, only moved: shift the lines of those after
            // the edit. The index stays sorted by name, then line.
            for def in prepared.defs.values_mut() {
                if def.line >= old_to {
                    def.line = shift(def.line);
                }
            }
            for entry in &mut prepared.variables {
                if entry.line > old_to {
                    entry.line = shift(entry.line - 1) + 1;
                }
            }
        }
    }

    // Any value may depend on the changed lines; resolve again on demand.
    prepared.resolved = ResolvedVariables::default();
    if let Some(working) = prepared.working_lines.as_mut() {
        let eval_lines = prepared.eval_lines.as_deref().unwrap_or(lines);
        working.splice(from..old_to, eval_lines[from..new_to].iter().cloned());
    }
    prepared.line_hashes = line_hashes;
}

/// The span that differs between two versions of a note, from their line
/// hashes: old lines `[from, old_to)` became new lines `[from, new_to)`.
fn changed_line_span(old: &[u64], new: &[u64]) -> Option<(usize, usize, usize)> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    if prefix == old.len() && prefix == new.len() {
        return None;
    }
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    Some((prefix, old.len() - suffix, new.len() - suffix))
}

fn evaluate_prepared(
    lines: &[String],
    prepared: &mut PreparedNoteContext,
    options: NoteEvaluationOptions,
) -> NoteEvaluationResult {
    let mut ctx = new_context();
    let PreparedNoteContext {
        eval_lines: prepared_lines,
        lines_with_unresolved,
        cross_note_refs,
        defs,
        variables,
        variable_matcher,
        resolved,
        working_lines,
        ..
    } = prepared;
    let eval_lines: &[String] = prepared_lines.as_deref().unwrap_or(lines);
    let defs: &FxHashMap<String, VariableDefinition> = defs;

    let line_count = eval_lines.len();
    let (eval_from, eval_to) = match options.eval_range {
        Some((from, to)) => (from.min(line_count), to.min(line_count)),
        None => (0, line_count),
    };
    let mut resolver =
        VariableResolver::with_resolved(defs, variable_matcher.clone(), std::mem::take(resolved));
    let is_full_eval = eval_from == 0 && eval_to == line_count;
    if options.variables_enabled && is_full_eval {
        let mut names: Vec<String> = defs.keys().cloned().collect();
        names.sort();
        for normalized in names {
            let _ = resolver.resolve(&normalized, &mut ctx);
        }
    }

    let mut line_results: Vec<Option<String>> = vec![None; line_count];
    let mut table_cell_results: Vec<Vec<TableCellEvaluation>> = vec![Vec::new(); line_count];

    // Working copy of the document for formula substitution, allocated only
    // when a table formula writes a value back and kept between evaluations
    // of the same note state; `written` rows are restored at the end.
    let mut written: Vec<usize> = Vec::new();
    // Cache and recursion guard for table-cell formula evaluation by (line, cell).
    let mut table_formula_cache: FxHashMap<(usize, usize), String> = FxHashMap::default();
    let mut table_formula_stack: Vec<(usize, usize)> = Vec::new();
    let mut table_eval_cache = TableEvalCache::default();
    let mut table_diagnostics: Vec<NoteEvaluationDiagnostic> = Vec::new();

    // Helper: get a mutable reference to working_lines, cloning from `eval_lines`
    // on first access. Call this only when a write is needed.
    macro_rules! ensure_working {
        () => {{
            working_lines.get_or_insert_with(|| eval_lines.to_vec())
        }};
    }

    // Read a line from working_lines if allocated, otherwise from eval_lines.
    macro_rules! read_line {
        ($i:expr) => {
            working_lines
                .as_ref()
                .map(|w| w[$i].as_str())
                .unwrap_or(&eval_lines[$i])
        };
    }

    for idx in eval_from..eval_to {
        if lines_with_unresolved.contains(&idx) {
            continue;
        }
        // Borrow the current line text per probe; each `read_line!` borrow
        // ends with the expression so it doesn't conflict with later
        // `ensure_working!()` mutation of `working_lines`.
        let table_segments = if options.table_enabled && is_table_line(read_line!(idx)) {
            table_expression_segments(read_line!(idx), true)
        } else {
            Vec::new()
        };

        // Multi-cell table evaluation: walk every formula cell L→R.
        // Triggered by any builtin call OR an explicit := prefix.
        if table_segments.iter().any(|(expr, _)| {
            find_builtin_formula_calls(expr).first().is_some()
                || expr
                    .trim()
                    .strip_prefix(":=")
                    .map(|rest| !rest.trim_start().is_empty())
                    .unwrap_or(false)
        }) {
            let working = ensure_working!();
            let mut first_value: Option<String> = None;
            for (expression, cell_idx) in table_segments {
                let value = evaluate_table_formula(
                    working,
                    idx,
                    &expression,
                    Some(cell_idx),
                    options.variables_enabled,
                    if options.variables_enabled {
                        Some(&mut resolver)
                    } else {
                        None
                    },
                    &mut table_eval_cache,
                    &mut table_formula_cache,
                    &mut table_formula_stack,
                    &mut ctx,
                );
                let Some(value) = value else { continue };
                if let Some(code) = table_ref_error_code(&value) {
                    table_diagnostics.push(NoteEvaluationDiagnostic {
                        kind: format!("table-ref-{code}"),
                        line: idx + 1,
                        message: format!("table formula cell {} returned {}", cell_idx + 1, value),
                    });
                }

                if first_value.is_none() {
                    first_value = Some(value.clone());
                }
                table_cell_results[idx].push(TableCellEvaluation {
                    cell_index: cell_idx,
                    value: value.clone(),
                    error_kind: table_cell_error_kind(&value),
                });

                if let Some(updated) = substitute_table_cell_value(&working[idx], cell_idx, &value)
                {
                    working[idx] = updated;
                    written.push(idx);
                    table_eval_cache.split_cells.remove(&idx);
                }
            }
            line_results[idx] = first_value;
            continue;
        }

        // Single-expression path (preserves original behavior).
        let Some(line_expr) = extract_line_expression(read_line!(idx), options.table_enabled)
        else {
            continue;
        };
        let expression = line_expr.expression.as_str();

        let result = if options.variables_enabled {
            if let Some(value) = evaluate_table_formula(
                working_lines.as_deref().unwrap_or(eval_lines),
                idx,
                expression,
                line_expr.table_cell_index,
                true,
                Some(&mut resolver),
                &mut table_eval_cache,
                &mut table_formula_cache,
                &mut table_formula_stack,
                &mut ctx,
            ) {
                if let Some(code) = table_ref_error_code(&value) {
                    table_diagnostics.push(NoteEvaluationDiagnostic {
                        kind: format!("table-ref-{code}"),
                        line: idx + 1,
                        message: format!("table formula returned {}", value),
                    });
                }
                Some(value)
            } else if let Some((_name, normalized, rhs)) = parse_variable_assignment(expression) {
                let resolved = resolver.resolve(&normalized, &mut ctx);
                if assignment_rhs_is_plain_numeric_literal(&rhs) {
                    None
                } else {
                    resolved
                }
            } else {
                evaluate_expression_with_variables(expression, &mut resolver, &mut ctx)
            }
        } else {
            if let Some(value) = evaluate_table_formula(
                working_lines.as_deref().unwrap_or(eval_lines),
                idx,
                expression,
                line_expr.table_cell_index,
                false,
                None,
                &mut table_eval_cache,
                &mut table_formula_cache,
                &mut table_formula_stack,
                &mut ctx,
            ) {
                if let Some(code) = table_ref_error_code(&value) {
                    table_diagnostics.push(NoteEvaluationDiagnostic {
                        kind: format!("table-ref-{code}"),
                        line: idx + 1,
                        message: format!("table formula returned {}", value),
                    });
                }
                Some(value)
            } else {
                evaluate_single(expression, &mut ctx)
            }
        };

        line_results[idx] = result;
    }

    if let Some(working) = working_lines.as_mut() {
        for idx in written {
            working[idx].clone_from(&eval_lines[idx]);
        }
    }

    let mut diagnostics = resolver.diagnostics().unwrap_or_default();
    for diag in table_diagnostics {
        if !diagnostics.iter().any(|existing| {
            existing.kind == diag.kind
                && existing.line == diag.line
                && existing.message == diag.message
        }) {
            diagnostics.push(diag);
        }
    }

    let variable_values = if options.variables_enabled {
        resolver.numeric_values()
    } else {
        FxHashMap::default()
    };
    *resolved = resolver.into_resolved();

    NoteEvaluationResult {
        line_results,
        variables: variables.clone(),
        diagnostics: if diagnostics.is_empty() {
            None
        } else {
            Some(diagnostics)
        },
        table_cell_results,
        variable_values,
        cross_note_refs: cross_note_refs.clone(),
    }
}

/// Replace the contents of `cell_index` in a pipe-table line with `value`,
/// preserving surrounding pipes and a single leading/trailing space.
/// Returns the updated line, or None if the line does not contain enough
/// pipes to address the cell.
fn substitute_table_cell_value(line: &str, cell_index: usize, value: &str) -> Option<String> {
    let pipes = table_pipe_positions(line);
    let left_pipe = *pipes.get(cell_index)?;
    let right_pipe = *pipes.get(cell_index + 1)?;
    if right_pipe <= left_pipe + 1 {
        return None;
    }
    let mut out = String::with_capacity(line.len() + value.len());
    out.push_str(&line[..=left_pipe]);
    out.push(' ');
    out.push_str(value);
    out.push(' ');
    out.push_str(&line[right_pipe..]);
    Some(out)
}

/// Like `table_expression_segment` but returns *every* candidate formula cell
/// in the row (left-to-right). When no formula cells exist, falls back to the
/// single non-formula candidate to preserve existing single-line behavior.
fn table_expression_segments(line: &str, allow_assignments: bool) -> Vec<(String, usize)> {
    if !is_table_line(line) {
        return Vec::new();
    }

    let cells = split_table_cells(line);
    if cells.is_empty() {
        return Vec::new();
    }

    let mut formula_segments: Vec<(String, usize)> = Vec::new();
    let mut other_candidates: Vec<(String, usize)> = Vec::new();
    for (cell_idx, cell) in cells.iter().enumerate() {
        let trimmed = cell.trim();
        if trimmed.is_empty() {
            continue;
        }

        // A cell is a "formula cell" if it contains any builtin formula call
        // (e.g. `sum_col() + 3`) or starts with the explicit `:=` prefix
        // (e.g. `:=5*sum_col()+var` or `:=var`).
        let is_colon_eq = trimmed
            .strip_prefix(":=")
            .map(|rest| !rest.trim_start().is_empty())
            .unwrap_or(false);
        if !find_builtin_formula_calls(trimmed).is_empty() || is_colon_eq {
            formula_segments.push((trimmed.to_string(), cell_idx));
            continue;
        }

        let qualifies =
            has_calc_signal(trimmed) || (allow_assignments && looks_like_assignment(trimmed));
        if !qualifies {
            continue;
        }
        other_candidates.push((trimmed.to_string(), cell_idx));
    }

    if !formula_segments.is_empty() {
        return formula_segments;
    }

    if other_candidates.len() == 1 {
        return other_candidates;
    }
    Vec::new()
}

fn evaluate_expression_with_variables(
    raw_input: &str,
    resolver: &mut VariableResolver<'_>,
    ctx: &mut fend_core::Context,
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
        resolver.substitute_runtime(expr, None, ctx)?
    } else {
        expr.to_string()
    };

    let text = match resolver.eval_raw(&substituted, ctx) {
        Some(text) => text,
        None => evaluate_leading_expression(&substituted, |prefix| resolver.eval_raw(prefix, ctx))?,
    };
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

fn evaluate_single(input: &str, ctx: &mut fend_core::Context) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !has_calc_signal(trimmed) {
        return None;
    }

    let (expr, applied_result) = split_applied_result(trimmed);
    let text = match evaluate_raw_expression(expr, ctx) {
        Some(text) => text,
        None => evaluate_leading_expression(expr, |prefix| evaluate_raw_expression(prefix, ctx))?,
    };
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

/// Most attempts `evaluate_leading_expression` makes on one line.
const MAX_LABEL_PREFIX_ATTEMPTS: usize = 8;

/// Evaluates the calculation at the start of a line that begins with a
/// number and ends in a text label, e.g. `100 - 20 groceries` -> `80`. Only
/// called after the whole line failed to evaluate. Tries the prefixes that end
/// right before a word starting with a letter, longest first, and accepts the
/// first one that has a calculation in it and evaluates to something other
/// than itself (so `3 days ago` and `2 kids and 3 dogs` stay silent).
fn evaluate_leading_expression(
    expr: &str,
    mut evaluate: impl FnMut(&str) -> Option<String>,
) -> Option<String> {
    let trimmed = expr.trim();
    if !trimmed.starts_with(|ch: char| ch.is_ascii_digit()) {
        return None;
    }
    let mut label_starts: Vec<usize> = trimmed
        .char_indices()
        .zip(trimmed.chars().skip(1))
        .filter(|((_, ch), next)| ch.is_whitespace() && next.is_alphabetic())
        .map(|((idx, _), _)| idx)
        .collect();
    label_starts.reverse();
    label_starts
        .into_iter()
        .map(|end| trimmed[..end].trim_end())
        .filter(|prefix| has_calc_signal(prefix))
        .take(MAX_LABEL_PREFIX_ATTEMPTS)
        .find_map(|prefix| evaluate(prefix).filter(|value| value != prefix))
}

fn evaluate_raw_expression(expr: &str, ctx: &mut fend_core::Context) -> Option<String> {
    match fend_core::evaluate_with_interrupt(expr, ctx, &GENERATION_INTERRUPT) {
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

fn assignment_rhs_is_plain_numeric_literal(rhs: &str) -> bool {
    parse_plain_numeric_literal(rhs).is_some()
}

fn parse_plain_numeric_literal(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let cleaned: String = trimmed
        .chars()
        .filter(|ch| !matches!(ch, ',' | '_'))
        .collect();
    if cleaned.is_empty() {
        return None;
    }

    let mut seen_digit = false;
    let mut seen_dot = false;
    for (idx, ch) in cleaned.chars().enumerate() {
        if ch.is_ascii_digit() {
            seen_digit = true;
            continue;
        }
        if ch == '.' && !seen_dot {
            seen_dot = true;
            continue;
        }
        if matches!(ch, '+' | '-') && idx == 0 {
            continue;
        }
        return None;
    }
    if !seen_digit {
        return None;
    }

    let value = cleaned.parse::<f64>().ok()?;
    if value.is_finite() {
        Some(value)
    } else {
        None
    }
}

fn parse_builtin_formula(expression: &str) -> Option<FormulaSpec> {
    let trimmed = expression.trim();
    if trimmed.is_empty() {
        return None;
    }

    let without_prefix = trimmed
        .strip_prefix(":=")
        .or_else(|| trimmed.strip_prefix('='))
        .unwrap_or(trimmed)
        .trim();
    let compact = without_prefix
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    let token = compact.strip_suffix("()").unwrap_or(compact.as_str());

    match token {
        "sum_row" => Some(FormulaSpec {
            op: FormulaOp::Sum,
            scope: FormulaScope::Row,
        }),
        "avg_row" => Some(FormulaSpec {
            op: FormulaOp::Avg,
            scope: FormulaScope::Row,
        }),
        "sum_col" | "sum_column" => Some(FormulaSpec {
            op: FormulaOp::Sum,
            scope: FormulaScope::Column,
        }),
        "avg_col" | "avg_column" => Some(FormulaSpec {
            op: FormulaOp::Avg,
            scope: FormulaScope::Column,
        }),
        _ => None,
    }
}

fn table_coordinate_ref_regex() -> &'static Regex {
    TABLE_COORD_REF_RE.get_or_init(|| {
        Regex::new(r"\(\s*(\d+)\s*,\s*(\d+)\s*\)")
            .expect("table coordinate reference regex is valid")
    })
}

fn logical_row_cell_text(
    lines: &[String],
    logical_row: &[usize],
    col: usize,
    table_eval_cache: &mut TableEvalCache,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for line_idx in logical_row {
        let row_cells = table_eval_cache.cells_for_line(lines, *line_idx)?.clone();
        let Some(cell) = row_cells.get(col) else {
            continue;
        };
        let trimmed = cell.trim();
        if trimmed.is_empty() {
            continue;
        }
        parts.push(trimmed.to_string());
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join(" "))
}

fn logical_row_primary_cell(
    lines: &[String],
    logical_row: &[usize],
    col: usize,
    table_eval_cache: &mut TableEvalCache,
) -> Option<(usize, String)> {
    for line_idx in logical_row {
        let row_cells = table_eval_cache.cells_for_line(lines, *line_idx)?.clone();
        let Some(cell) = row_cells.get(col) else {
            continue;
        };
        let trimmed = cell.trim();
        if trimmed.is_empty() {
            continue;
        }
        return Some((*line_idx, trimmed.to_string()));
    }
    None
}

fn resolve_table_coordinate_value(
    lines: &[String],
    line_idx: usize,
    formula_col: usize,
    row_1based: usize,
    col_1based: usize,
    variables_enabled: bool,
    mut resolver: Option<&mut VariableResolver<'_>>,
    table_eval_cache: &mut TableEvalCache,
    table_formula_cache: &mut FxHashMap<(usize, usize), String>,
    table_formula_stack: &mut Vec<(usize, usize)>,
    ctx: &mut fend_core::Context,
) -> Result<String, &'static str> {
    if row_1based == 0 || col_1based == 0 {
        return Err(TABLE_REF_ERROR_OUT_OF_BOUNDS);
    }

    let table_block = table_eval_cache
        .block_for_line(lines, line_idx)
        .ok_or(TABLE_REF_ERROR_OUT_OF_BOUNDS)?;
    let data_rows = &table_block.data_rows;
    let target_logical_row = data_rows
        .get(row_1based.saturating_sub(1))
        .ok_or(TABLE_REF_ERROR_OUT_OF_BOUNDS)?;
    let target_col = col_1based.saturating_sub(1);

    let current_logical_row = table_block.row_lookup.get(&line_idx).copied();
    let target_logical_idx = row_1based.saturating_sub(1);
    if current_logical_row == Some(target_logical_idx) && target_col == formula_col {
        return Err(TABLE_REF_ERROR_SELF_REFERENCE);
    }

    let target_text =
        logical_row_cell_text(lines, target_logical_row, target_col, table_eval_cache)
            .ok_or(TABLE_REF_ERROR_NON_NUMERIC)?;
    let (target_line, target_raw) =
        logical_row_primary_cell(lines, target_logical_row, target_col, table_eval_cache)
            .ok_or(TABLE_REF_ERROR_NON_NUMERIC)?;
    if target_raw.is_empty() {
        return Err(TABLE_REF_ERROR_NON_NUMERIC);
    }

    if let Some(value) = parse_plain_numeric_literal(&target_text) {
        return Ok(format_number(value));
    }

    // If the target is itself a table formula cell, evaluate it recursively
    // so references can chain across formulas.
    if let Some(value) = evaluate_table_formula(
        lines,
        target_line,
        &target_raw,
        Some(target_col),
        variables_enabled,
        resolver.as_deref_mut(),
        table_eval_cache,
        table_formula_cache,
        table_formula_stack,
        ctx,
    ) {
        if value == TABLE_REF_ERROR_OUT_OF_BOUNDS
            || value == TABLE_REF_ERROR_NON_NUMERIC
            || value == TABLE_REF_ERROR_SELF_REFERENCE
            || value == TABLE_REF_ERROR_CYCLE
        {
            return Err(if value == TABLE_REF_ERROR_OUT_OF_BOUNDS {
                TABLE_REF_ERROR_OUT_OF_BOUNDS
            } else if value == TABLE_REF_ERROR_NON_NUMERIC {
                TABLE_REF_ERROR_NON_NUMERIC
            } else if value == TABLE_REF_ERROR_CYCLE {
                TABLE_REF_ERROR_CYCLE
            } else {
                TABLE_REF_ERROR_SELF_REFERENCE
            });
        }
        let numeric = extract_first_number(&value).ok_or(TABLE_REF_ERROR_NON_NUMERIC)?;
        return Ok(format_number(numeric));
    }

    let evaluated = if variables_enabled {
        let resolver = resolver.as_deref_mut().ok_or(TABLE_REF_ERROR_NON_NUMERIC)?;
        evaluate_formula_term_with_variables(&target_raw, resolver, ctx)
    } else {
        evaluate_formula_term(&target_raw, ctx)
    }
    .ok_or(TABLE_REF_ERROR_NON_NUMERIC)?;

    let numeric = extract_first_number(&evaluated).ok_or(TABLE_REF_ERROR_NON_NUMERIC)?;
    Ok(format_number(numeric))
}

fn substitute_table_coordinate_references(
    lines: &[String],
    line_idx: usize,
    formula_col: usize,
    expression: &str,
    variables_enabled: bool,
    mut resolver: Option<&mut VariableResolver<'_>>,
    table_eval_cache: &mut TableEvalCache,
    table_formula_cache: &mut FxHashMap<(usize, usize), String>,
    table_formula_stack: &mut Vec<(usize, usize)>,
    ctx: &mut fend_core::Context,
) -> Result<String, &'static str> {
    let regex = table_coordinate_ref_regex();
    let mut rewritten = String::with_capacity(expression.len());
    let mut cursor = 0usize;
    let mut replaced_any = false;

    for captures in regex.captures_iter(expression) {
        let Some(m) = captures.get(0) else { continue };
        rewritten.push_str(&expression[cursor..m.start()]);

        let row_1based = captures
            .get(1)
            .and_then(|m| m.as_str().parse::<usize>().ok())
            .ok_or(TABLE_REF_ERROR_OUT_OF_BOUNDS)?;
        let col_1based = captures
            .get(2)
            .and_then(|m| m.as_str().parse::<usize>().ok())
            .ok_or(TABLE_REF_ERROR_OUT_OF_BOUNDS)?;

        let value = resolve_table_coordinate_value(
            lines,
            line_idx,
            formula_col,
            row_1based,
            col_1based,
            variables_enabled,
            resolver.as_deref_mut(),
            table_eval_cache,
            table_formula_cache,
            table_formula_stack,
            ctx,
        )?;
        rewritten.push_str(&value);
        cursor = m.end();
        replaced_any = true;
    }

    if !replaced_any {
        return Ok(expression.to_string());
    }

    rewritten.push_str(&expression[cursor..]);
    Ok(rewritten)
}

fn collect_table_formula_terms(
    lines: &[String],
    line_idx: usize,
    formula_col: usize,
    spec: FormulaSpec,
    table_eval_cache: &mut TableEvalCache,
) -> Option<Vec<String>> {
    let current_cells = table_eval_cache.cells_for_line(lines, line_idx)?.clone();
    if table_syntax::is_delimiter_row_at(&current_cells, false) {
        return None;
    }
    if formula_col >= current_cells.len() {
        return None;
    }
    let table_block = table_eval_cache.block_for_line(lines, line_idx)?;
    let current_logical_row = table_block.row_lookup.get(&line_idx).copied();
    let mut terms = Vec::new();

    match spec.scope {
        FormulaScope::Row => {
            if let Some(row_idx) = current_logical_row {
                let row_lines = table_block.data_rows.get(row_idx)?;
                for col in 0..formula_col {
                    let Some(cell) = logical_row_cell_text(lines, row_lines, col, table_eval_cache)
                    else {
                        continue;
                    };
                    let trimmed = cell.trim();
                    if trimmed.is_empty() || table_syntax::delimiter_cell_dashes(&cell).is_some() {
                        continue;
                    }
                    if !trimmed.chars().any(|c| c.is_ascii_digit()) {
                        continue;
                    }
                    terms.push(cell);
                }
            } else {
                for cell in current_cells.iter().take(formula_col) {
                    let trimmed = cell.trim();
                    if trimmed.is_empty() || table_syntax::delimiter_cell_dashes(cell).is_some() {
                        continue;
                    }
                    if !trimmed.chars().any(|c| c.is_ascii_digit()) {
                        continue;
                    }
                    terms.push(cell.clone());
                }
            }
        }
        FormulaScope::Column => {
            for (logical_idx, row_lines) in table_block.data_rows.iter().enumerate() {
                if Some(logical_idx) >= current_logical_row {
                    break;
                }
                let Some(cell) =
                    logical_row_cell_text(lines, row_lines, formula_col, table_eval_cache)
                else {
                    continue;
                };
                if cell.trim().is_empty() || parse_builtin_formula(&cell).is_some() {
                    continue;
                }
                terms.push(cell);
            }
        }
    }

    Some(terms)
}

fn reduce_formula_values(
    values: &[String],
    op: FormulaOp,
    ctx: &mut fend_core::Context,
) -> Option<String> {
    if values.is_empty() {
        return None;
    }

    // Fast path: if all values are plain numbers, do arithmetic directly.
    let numeric: Option<Vec<f64>> = values
        .iter()
        .map(|v| parse_plain_numeric_literal(v))
        .collect();
    if let Some(nums) = numeric {
        let sum: f64 = nums.iter().sum();
        let result = match op {
            FormulaOp::Sum => sum,
            FormulaOp::Avg => sum / nums.len() as f64,
        };
        return Some(format_number(result));
    }

    let mut acc = values[0].clone();
    for value in values.iter().skip(1) {
        let expr = format!("({acc}) + ({value})");
        acc = evaluate_raw_expression(&expr, ctx)?;
    }

    if op == FormulaOp::Avg && values.len() > 1 {
        let avg_expr = format!("({acc}) / {}", values.len());
        acc = evaluate_raw_expression(&avg_expr, ctx)?;
    }

    Some(acc)
}

fn evaluate_formula_term(term: &str, ctx: &mut fend_core::Context) -> Option<String> {
    let trimmed = term.trim();
    if trimmed.is_empty() {
        return None;
    }
    evaluate_raw_expression(trimmed, ctx)
}

fn evaluate_formula_term_with_variables(
    term: &str,
    resolver: &mut VariableResolver<'_>,
    ctx: &mut fend_core::Context,
) -> Option<String> {
    let trimmed = term.trim();
    if trimmed.is_empty() {
        return None;
    }

    let substituted = if resolver.expression_references_variable(trimmed) {
        resolver.substitute_runtime(trimmed, None, ctx)?
    } else {
        trimmed.to_string()
    };

    resolver.eval_raw(&substituted, ctx)
}

fn find_builtin_formula_calls(expression: &str) -> Vec<FormulaCallSpan> {
    let bytes = expression.as_bytes();
    let mut calls = Vec::new();
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

        let candidate = &expression[start..cursor];
        if let Some(spec) = parse_builtin_formula(candidate) {
            calls.push(FormulaCallSpan {
                start,
                end: cursor,
                spec,
            });
            idx = cursor;
            continue;
        }

        idx = ident_start.saturating_add(1);
    }

    calls
}

fn evaluate_table_formula_call(
    lines: &[String],
    line_idx: usize,
    formula_col: usize,
    spec: FormulaSpec,
    variables_enabled: bool,
    resolver: Option<&mut VariableResolver<'_>>,
    table_eval_cache: &mut TableEvalCache,
    ctx: &mut fend_core::Context,
) -> Option<String> {
    let terms = collect_table_formula_terms(lines, line_idx, formula_col, spec, table_eval_cache)?;

    let mut values = Vec::new();
    if variables_enabled {
        let resolver = resolver?;
        for term in terms {
            if let Some(value) = evaluate_formula_term_with_variables(&term, resolver, ctx) {
                values.push(value);
            }
        }
    } else {
        for term in terms {
            if let Some(value) = evaluate_formula_term(&term, ctx) {
                values.push(value);
            }
        }
    }

    reduce_formula_values(&values, spec.op, ctx)
}

fn evaluate_table_formula(
    lines: &[String],
    line_idx: usize,
    expression: &str,
    table_cell_index: Option<usize>,
    variables_enabled: bool,
    mut resolver: Option<&mut VariableResolver<'_>>,
    table_eval_cache: &mut TableEvalCache,
    table_formula_cache: &mut FxHashMap<(usize, usize), String>,
    table_formula_stack: &mut Vec<(usize, usize)>,
    ctx: &mut fend_core::Context,
) -> Option<String> {
    let formula_col = table_cell_index?;

    let key = (line_idx, formula_col);
    if let Some(cached) = table_formula_cache.get(&key) {
        return Some(cached.clone());
    }
    if table_formula_stack.contains(&key) {
        return Some(TABLE_REF_ERROR_CYCLE.to_string());
    }
    table_formula_stack.push(key);

    let result = (|| {
        // Strip the leading `:=` formula prefix if present. This keeps fend
        // from ever seeing the `:=` characters and lets callers write either
        // `:=sum_col()` or `:=5*sum_col()+var` uniformly.
        let trimmed = expression.trim();
        let (expression, had_prefix) = if let Some(rest) = trimmed.strip_prefix(":=") {
            (rest.trim_start(), true)
        } else {
            (trimmed, false)
        };

        if expression.is_empty() {
            return None;
        }

        let expression = match substitute_table_coordinate_references(
            lines,
            line_idx,
            formula_col,
            expression,
            variables_enabled,
            resolver.as_deref_mut(),
            table_eval_cache,
            table_formula_cache,
            table_formula_stack,
            ctx,
        ) {
            Ok(value) => value,
            Err(error) => return Some(error.to_string()),
        };

        let calls = find_builtin_formula_calls(&expression);

        // When no builtin calls exist but the cell had an explicit := prefix,
        // evaluate the stripped expression as a plain arithmetic/variable expression.
        if calls.is_empty() {
            return if !had_prefix {
                None
            } else if variables_enabled {
                // Explicit `:=` cells should evaluate even when the substituted
                // expression becomes a plain literal (e.g. `:=(1,2)` -> `10`).
                let resolver = resolver?;
                let substituted = if resolver.expression_references_variable(&expression) {
                    resolver.substitute_runtime(&expression, None, ctx)?
                } else {
                    expression.clone()
                };
                evaluate_raw_expression(&substituted, ctx)
            } else {
                evaluate_raw_expression(&expression, ctx)
            };
        }

        let mut rewritten = String::with_capacity(expression.len() + calls.len() * 4);
        let mut cursor = 0usize;
        for call in calls {
            rewritten.push_str(&expression[cursor..call.start]);
            let value = evaluate_table_formula_call(
                lines,
                line_idx,
                formula_col,
                call.spec,
                variables_enabled,
                resolver.as_deref_mut(),
                table_eval_cache,
                ctx,
            )?;
            rewritten.push('(');
            rewritten.push_str(&value);
            rewritten.push(')');
            cursor = call.end;
        }
        rewritten.push_str(&expression[cursor..]);

        if variables_enabled {
            let resolver = resolver?;
            evaluate_expression_with_variables(&rewritten, resolver, ctx)
        } else {
            evaluate_single(&rewritten, ctx)
        }
    })();

    table_formula_stack.pop();
    if let Some(value) = result.clone() {
        table_formula_cache.insert(key, value);
    }
    result
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

fn table_expression_segment(line: &str, allow_assignments: bool) -> Option<(String, usize)> {
    if !is_table_line(line) {
        return None;
    }

    let cells = split_table_cells(line);
    if cells.is_empty() {
        return None;
    }

    let mut formula_candidates: Vec<(String, usize)> = Vec::new();
    let mut candidates: Vec<(String, usize)> = Vec::new();
    for (cell_idx, cell) in cells.iter().enumerate() {
        let trimmed = cell.trim();
        if trimmed.is_empty() {
            continue;
        }

        if parse_builtin_formula(trimmed).is_some() {
            formula_candidates.push((trimmed.to_string(), cell_idx));
            continue;
        }

        let qualifies =
            has_calc_signal(trimmed) || (allow_assignments && looks_like_assignment(trimmed));
        if !qualifies {
            continue;
        }

        candidates.push((trimmed.to_string(), cell_idx));
    }

    if formula_candidates.len() == 1 {
        return formula_candidates.pop();
    }
    if !formula_candidates.is_empty() {
        return None;
    }

    if candidates.len() != 1 {
        return None;
    }

    candidates.pop()
}

fn extract_line_expression(line: &str, table_enabled: bool) -> Option<LineExpression> {
    if table_enabled && is_table_line(line) {
        let (expression, table_cell_index) = table_expression_segment(line, true)?;
        return Some(LineExpression {
            expression,
            table_cell_index: Some(table_cell_index),
        });
    }

    if let Some(body) = list_body_segment(line) {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            return None;
        }
        return Some(LineExpression {
            expression: trimmed.to_string(),
            table_cell_index: None,
        });
    }

    let trimmed = line.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(LineExpression {
            expression: trimmed.to_string(),
            table_cell_index: None,
        })
    }
}

/// The variable a line assigns, if it is an assignment.
fn line_variable_definition(
    line: &str,
    line_idx: usize,
    table_enabled: bool,
) -> Option<VariableDefinition> {
    let line_expr = extract_line_expression(line, table_enabled)?;
    let (name, normalized, rhs) = parse_variable_assignment(&line_expr.expression)?;
    Some(VariableDefinition {
        name,
        normalized,
        expression: rhs,
        line: line_idx,
    })
}

fn variable_index_from_definitions(
    defs: &FxHashMap<String, VariableDefinition>,
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

fn cross_note_ref_regex() -> &'static Regex {
    CROSS_NOTE_REF_RE.get_or_init(|| {
        // Matches [[SHORTID]].var_name where SHORTID is 8 alphanumeric chars.
        // Var name: starts and ends with [A-Za-z0-9_], allows internal spaces.
        Regex::new(r"\[\[([A-Za-z0-9]{8})\]\]\.([A-Za-z0-9_](?:[A-Za-z0-9_ ]*[A-Za-z0-9_])?)")
            .expect("cross-note ref regex is valid")
    })
}

/// Scan lines for all `[[SHORTID]].var_name` references (1-based line numbers).
/// Fast variable-assignment scan: finds `name :=` lines without evaluating
/// expressions. Returns one `VariableIndexEntry` per unique normalized name
/// (last definition wins, matching runtime precedence). Much cheaper than a
/// full `CalcEngine::evaluate_note_context` call; use for autocomplete where
/// only names are needed, not values.
pub fn scan_variable_assignments(lines: &[String]) -> Vec<VariableIndexEntry> {
    let mut seen: FxHashMap<String, VariableIndexEntry> = FxHashMap::default();
    for (line_idx, line) in lines.iter().enumerate() {
        if let Some((name, normalized, _rhs)) = parse_variable_assignment(line.trim()) {
            seen.insert(
                normalized.clone(),
                VariableIndexEntry {
                    name,
                    normalized,
                    line: line_idx + 1,
                },
            );
        }
    }
    seen.into_values().collect()
}

pub fn scan_cross_note_refs(lines: &[String]) -> Vec<CrossNoteRef> {
    let re = cross_note_ref_regex();
    let mut refs = Vec::new();
    for (line_idx, line) in lines.iter().enumerate() {
        for cap in re.captures_iter(line) {
            let short_id = cap[1].to_ascii_lowercase();
            let raw_name = cap[2].trim();
            let var_normalized = raw_name
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase();
            if var_normalized.is_empty() {
                continue;
            }
            refs.push(CrossNoteRef {
                note_short_id: short_id,
                var_normalized,
                line: line_idx + 1,
            });
        }
    }
    refs
}

/// Replace `[[SHORTID]].var_name` tokens in a line with their resolved numeric values.
/// Returns (preprocessed_line, has_unresolved) where has_unresolved is true if any
/// ref in the line could not be resolved from extern_vars.
fn preprocess_line_cross_note<'a>(
    line: &'a str,
    extern_map: &FxHashMap<(String, String), f64>,
) -> (Cow<'a, str>, bool) {
    // The overwhelming majority of lines in a note carry no cross-note ref, and
    // this runs over the whole document on every evaluation. Skip the regex and
    // the allocation for them.
    if !line.contains("[[") {
        return (Cow::Borrowed(line), false);
    }

    let re = cross_note_ref_regex();
    let mut has_unresolved = false;
    let mut cursor = 0usize;
    let mut out: Option<String> = None;

    for cap in re.captures_iter(line) {
        let full = cap.get(0).unwrap();
        let short_id = cap[1].to_ascii_lowercase();
        let raw_name = cap[2].trim();
        let var_normalized = raw_name
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();

        let key = (short_id, var_normalized);
        let Some(&value) = extern_map.get(&key) else {
            // Unresolved refs are left verbatim, so they cost no rewrite.
            has_unresolved = true;
            continue;
        };

        let out = out.get_or_insert_with(|| String::with_capacity(line.len()));
        out.push_str(&line[cursor..full.start()]);
        out.push_str(&format_number(value));
        cursor = full.end();
    }

    match out {
        Some(mut out) => {
            out.push_str(&line[cursor..]);
            (Cow::Owned(out), has_unresolved)
        }
        // Every ref was unresolved (or there were none): the line is unchanged.
        None => (Cow::Borrowed(line), has_unresolved),
    }
}

/// Finds variable names in expressions: ASCII case-insensitive, on word
/// boundaries, left-most first and the longest name among those starting at
/// the same place. A single automaton over all names; a regex alternation of
/// thousands of names degrades badly as the note grows.
#[derive(Clone)]
struct VariableMatcher(Arc<AhoCorasick>);

impl VariableMatcher {
    fn new(names: &[String]) -> Option<Self> {
        let names: Vec<&str> = names
            .iter()
            .map(String::as_str)
            .filter(|n| !n.is_empty())
            .collect();
        if names.is_empty() {
            return None;
        }
        AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(names)
            .ok()
            .map(|automaton| Self(Arc::new(automaton)))
    }

    /// Byte ranges of the variable names in `text`, left to right.
    fn find(&self, text: &str) -> Vec<(usize, usize)> {
        let bytes = text.as_bytes();
        let mut matches: Vec<(usize, usize)> = self
            .0
            .find_overlapping_iter(text)
            .map(|found| (found.start(), found.end()))
            .filter(|&(start, end)| on_word_boundaries(bytes, start, end))
            .collect();
        matches.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        let mut picked: Vec<(usize, usize)> = Vec::with_capacity(matches.len());
        for candidate in matches {
            if picked.last().is_some_and(|last| candidate.0 < last.1) {
                continue;
            }
            picked.push(candidate);
        }
        picked
    }

    fn is_match(&self, text: &str) -> bool {
        let bytes = text.as_bytes();
        self.0
            .find_overlapping_iter(text)
            .any(|found| on_word_boundaries(bytes, found.start(), found.end()))
    }
}

/// `start..end` is not glued to an ASCII word character on either side.
fn on_word_boundaries(bytes: &[u8], start: usize, end: usize) -> bool {
    let is_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    (start == 0 || !is_word(bytes[start - 1])) && (end == bytes.len() || !is_word(bytes[end]))
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

    let bytes = s.as_bytes();
    let mut has_digit = false;
    let mut has_alpha = false;
    let mut has_space = false;

    for (idx, &b) in bytes.iter().enumerate() {
        match b {
            b'+' | b'*' | b'^' | b'%' | b'(' => return true,
            b'-' | b'/' if has_numeric_or_space_math_neighbors(bytes, idx) => return true,
            b if b.is_ascii_digit() => has_digit = true,
            b if b.is_ascii_alphabetic() => has_alpha = true,
            b if b.is_ascii_whitespace() => has_space = true,
            _ => {}
        }
    }

    if has_digit && has_space && (s.contains(" to ") || s.contains(" in ")) {
        return true;
    }

    // Mixed digit+alpha shorthand like `5km` — only when no whitespace.
    has_digit && has_alpha && !has_space
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
    fn evaluates_leading_expression_before_a_text_label() {
        let engine = CalcEngine::new();
        assert_eq!(engine.evaluate("100 - 20 groceries"), Some("80".to_string()));
        assert_eq!(engine.evaluate("12 * 3 apples for the party"), Some("36".to_string()));
        assert_eq!(engine.evaluate("1,200 + 300 rent and power"), Some("1500".to_string()));
    }

    #[test]
    fn leading_expression_ignores_prose_and_plain_counts() {
        let engine = CalcEngine::new();
        assert_eq!(engine.evaluate("3 items bought"), None);
        assert_eq!(engine.evaluate("2 kids and 3 dogs"), None);
        assert_eq!(engine.evaluate("3 days ago"), None);
        assert_eq!(engine.evaluate("groceries 100 - 20"), None, "must start with a number");
    }

    #[test]
    fn leading_expression_in_note_context_respects_applied_results() {
        let engine = CalcEngine::new();
        let lines = vec![
            "100 - 20 groceries".to_string(),
            "100 - 20 groceries = 80".to_string(),
            "budget := 500".to_string(),
            "2 * 50 snacks".to_string(),
        ];
        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(result.line_results[0].as_deref(), Some("80"));
        assert_eq!(result.line_results[1], None, "already applied");
        assert_eq!(result.line_results[3].as_deref(), Some("100"));
    }

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
    fn evaluates_decimal_addition_expression_precisely() {
        let engine = CalcEngine::new();
        assert_eq!(
            engine.evaluate("3 + 346 + 5.4 + 105.03 + 2347 + 5 + 389.06 + 126 + 34.4"),
            Some("3360.89".to_string())
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
            vec![None, Some("2".to_string()), Some("12".to_string())]
        );

        let vars = result
            .variables
            .iter()
            .map(|entry| entry.normalized.clone())
            .collect::<Vec<_>>();
        assert_eq!(vars, vec!["subtotal", "tax"]);
    }

    #[test]
    fn note_eval_assignment_with_decimals_matches_expected_sum() {
        let engine = CalcEngine::new();
        let lines = vec![
            "val := 3 + 346 + 5.4 + 105.03 + 2347 + 5 + 389.06 + 126 + 34.4".to_string(),
            "val".to_string(),
        ];

        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(
            result.line_results,
            vec![Some("3360.89".to_string()), Some("3360.89".to_string())]
        );
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
            vec![None, Some("6".to_string()), Some("7".to_string())]
        );
    }

    #[test]
    fn note_eval_ignores_ordered_list_prose_with_hyphenated_words_and_slashes() {
        let engine = CalcEngine::new();
        let lines = vec![
            "5. Dedup: Unify the text-object methods, undo/redo, and VimIntent line-range"
                .to_string(),
        ];

        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(result.line_results, vec![None]);
    }

    #[test]
    fn note_eval_keeps_division_signal_in_list_items() {
        let engine = CalcEngine::new();
        let lines = vec!["- 12 / 3".to_string()];

        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(result.line_results, vec![Some("4".to_string())]);
    }

    #[test]
    fn note_eval_table_allows_chained_column_formula_with_variable() {
        let engine = CalcEngine::new();
        let lines = vec![
            "a := 22.5".to_string(),
            "| value | note |".to_string(),
            "| --- | --- |".to_string(),
            "| 2 | ok |".to_string(),
            "| 4 | ok |".to_string(),
            "| 6 | ok |".to_string(),
            "| sum_col() * a + 5 | done |".to_string(),
        ];

        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(result.line_results[6], Some("275".to_string()));
    }

    #[test]
    fn note_eval_table_allows_multiple_formula_calls_in_same_cell() {
        let engine = CalcEngine::new();
        let lines = vec![
            "| value |".to_string(),
            "| --- |".to_string(),
            "| 2 |".to_string(),
            "| 4 |".to_string(),
            "| 6 |".to_string(),
            "| sum_col() + avg_col() |".to_string(),
        ];

        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(result.line_results[5], Some("16".to_string()));
    }

    #[test]
    fn note_eval_table_formula_works_when_other_cell_contains_escaped_pipe() {
        let engine = CalcEngine::new();
        let lines = vec![
            "| note | value |".to_string(),
            "| --- | --- |".to_string(),
            "| left \\| right | 2 |".to_string(),
            "| ok | 3 |".to_string(),
            "| total | :=sum_col() |".to_string(),
        ];

        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(result.line_results[4], Some("5".to_string()));
    }

    #[test]
    fn note_eval_table_allows_multiple_row_formula_calls_in_same_cell() {
        let engine = CalcEngine::new();
        let lines = vec![
            "| left | right | total |".to_string(),
            "| --- | --- | --- |".to_string(),
            "| 2 | 4 | sum_row() + avg_row() |".to_string(),
        ];

        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(result.line_results[2], Some("9".to_string()));
    }

    #[test]
    fn note_eval_table_avg_col_formula_uses_rows_above_same_column() {
        let engine = CalcEngine::new();
        let lines = vec![
            "| value |".to_string(),
            "| --- |".to_string(),
            "| 10 |".to_string(),
            "| 20 |".to_string(),
            "| :=avg_col() |".to_string(),
        ];

        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(result.line_results[4], Some("15".to_string()));
    }

    #[test]
    fn note_eval_table_sum_row_formula_uses_cells_left_of_formula_cell() {
        let engine = CalcEngine::new();
        let lines = vec![
            "| left | right | total |".to_string(),
            "| --- | --- | --- |".to_string(),
            "| 2 | 3 | :=sum_row() |".to_string(),
        ];

        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(result.line_results[2], Some("5".to_string()));
    }

    #[test]
    fn note_eval_table_avg_col_formula_in_middle_column_with_trailing_cells() {
        let engine = CalcEngine::new();
        let lines = vec![
            "| first | last | grade | height | time |".to_string(),
            "| ----- | ---- | ----- | ------ | ---- |".to_string(),
            "| Dusan | Boro | 5 | 1.94 | 12h 30min |".to_string(),
            "| Dusan | Peric | 5 | 1.88 | 12h 30min |".to_string(),
            "| Dusan | Boro | 4 | 1.98 | 13h 15min |".to_string(),
            "| dsad | dsdsd | :=avg_col() | 1.91 | |".to_string(),
        ];

        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        let value = result.line_results[5].as_deref().unwrap_or("");
        let numeric = extract_first_number(value).unwrap_or(f64::NAN);
        assert!((numeric - (14.0 / 3.0)).abs() < 1e-6);
    }

    #[test]
    fn note_eval_table_cell_combines_builtin_with_arithmetic_and_variable() {
        let engine = CalcEngine::new();
        let lines = vec![
            "a := 3.4".to_string(),
            "| value |".to_string(),
            "| --- |".to_string(),
            "| 1 |".to_string(),
            "| 2 |".to_string(),
            "| :=sum_col() + 3 + a |".to_string(),
        ];
        let res = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        let cells = &res.table_cell_results[5];
        let val = cells
            .iter()
            .find(|c| c.cell_index == 0)
            .map(|c| c.value.clone())
            .unwrap_or_default();
        let n = extract_first_number(&val).unwrap_or(f64::NAN);
        assert!((n - (3.0 + 3.0 + 3.4)).abs() < 1e-6, "got {}", val);
    }

    #[test]
    fn note_eval_table_colon_eq_prefix_arithmetic_before_builtin_with_variable() {
        let engine = CalcEngine::new();
        let lines = vec![
            "var := 3.4".to_string(),
            "| value |".to_string(),
            "| --- |".to_string(),
            "| 1 |".to_string(),
            "| 2 |".to_string(),
            "| :=5*sum_col()+var |".to_string(),
        ];
        let res = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        let val = res.table_cell_results[5]
            .iter()
            .find(|c| c.cell_index == 0)
            .map(|c| c.value.clone())
            .unwrap_or_default();
        let n = extract_first_number(&val).unwrap_or(f64::NAN);
        // sum_col() = 3, expression = 5*3 + 3.4 = 18.4
        assert!((n - (5.0 * 3.0 + 3.4)).abs() < 1e-6, "got {val}");
    }

    #[test]
    fn note_eval_table_colon_eq_prefix_expression_no_builtin() {
        let engine = CalcEngine::new();
        let lines = vec![
            "multiplier := 4".to_string(),
            "| a | b | total |".to_string(),
            "| --- | --- | --- |".to_string(),
            "| 3 | 7 | :=multiplier * 2 + 1 |".to_string(),
        ];
        let res = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        let val = res.table_cell_results[3]
            .iter()
            .find(|c| c.cell_index == 2)
            .map(|c| c.value.clone())
            .unwrap_or_default();
        let n = extract_first_number(&val).unwrap_or(f64::NAN);
        assert!((n - 9.0).abs() < 1e-6, "got {val}");
    }

    #[test]
    fn note_eval_table_colon_eq_prefix_variable_only() {
        let engine = CalcEngine::new();
        let lines = vec![
            "rate := 7.5".to_string(),
            "| label | value |".to_string(),
            "| --- | --- |".to_string(),
            "| fee | :=rate |".to_string(),
        ];
        let res = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        let val = res.table_cell_results[3]
            .iter()
            .find(|c| c.cell_index == 1)
            .map(|c| c.value.clone())
            .unwrap_or_default();
        let n = extract_first_number(&val).unwrap_or(f64::NAN);
        assert!((n - 7.5).abs() < 1e-6, "got {val}");
    }

    #[test]
    fn note_eval_table_coordinate_reference_uses_1_based_data_rows() {
        let engine = CalcEngine::new();
        let lines = vec![
            "| item | value | total |".to_string(),
            "| --- | --- | --- |".to_string(),
            "| a | 10 | |".to_string(),
            "| b | 20 | |".to_string(),
            "| c | 0 | :=(1,2) + (2,2) |".to_string(),
        ];
        let res = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(
            res.table_cell_results[4]
                .iter()
                .find(|c| c.cell_index == 2)
                .map(|c| c.value.clone()),
            Some("30".to_string())
        );
    }

    #[test]
    fn note_eval_table_coordinate_reference_accepts_short_gfm_delimiter() {
        let engine = CalcEngine::new();
        let lines = vec![
            "|item|value|total|".to_string(),
            "|-|:--|--:|".to_string(),
            "|a|10||".to_string(),
            "|b|20||".to_string(),
            "|c|0|:=(1,2) + (2,2)|".to_string(),
        ];
        let res = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(
            res.table_cell_results[4]
                .iter()
                .find(|c| c.cell_index == 2)
                .map(|c| c.value.clone()),
            Some("30".to_string())
        );
    }

    #[test]
    fn note_eval_table_coordinate_reference_counts_logical_rows_with_continuations() {
        let engine = CalcEngine::new();
        let lines = vec![
            "| item | value | total |".to_string(),
            "| --- | --- | --- |".to_string(),
            "| a | 10 | |".to_string(),
            "|> details | | |".to_string(),
            "| b | 20 | :=(1,2) + (2,2) |".to_string(),
        ];
        let res = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(
            res.table_cell_results[4]
                .iter()
                .find(|c| c.cell_index == 2)
                .map(|c| c.value.clone()),
            Some("30".to_string())
        );
    }

    #[test]
    fn note_eval_table_coordinate_reference_without_arithmetic_evaluates() {
        let engine = CalcEngine::new();
        let lines = vec![
            "| item | value | total |".to_string(),
            "| --- | --- | --- |".to_string(),
            "| a | 10 | |".to_string(),
            "| b | 20 | :=(1,2) |".to_string(),
        ];
        let res = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(
            res.table_cell_results[3]
                .iter()
                .find(|c| c.cell_index == 2)
                .map(|c| c.value.clone()),
            Some("10".to_string())
        );
    }

    #[test]
    fn note_eval_table_coordinate_reference_reports_out_of_bounds() {
        let engine = CalcEngine::new();
        let lines = vec![
            "| item | value | total |".to_string(),
            "| --- | --- | --- |".to_string(),
            "| a | 10 | |".to_string(),
            "| b | 20 | :=(3,2) |".to_string(),
        ];
        let res = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(
            res.table_cell_results[3]
                .iter()
                .find(|c| c.cell_index == 2)
                .map(|c| c.value.clone()),
            Some(TABLE_REF_ERROR_OUT_OF_BOUNDS.to_string())
        );
    }

    #[test]
    fn note_eval_table_coordinate_reference_reports_non_numeric() {
        let engine = CalcEngine::new();
        let lines = vec![
            "| item | value | total |".to_string(),
            "| --- | --- | --- |".to_string(),
            "| abc | 10 | |".to_string(),
            "| b | 20 | :=(1,1) |".to_string(),
        ];
        let res = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(
            res.table_cell_results[3]
                .iter()
                .find(|c| c.cell_index == 2)
                .map(|c| c.value.clone()),
            Some(TABLE_REF_ERROR_NON_NUMERIC.to_string())
        );
    }

    #[test]
    fn note_eval_table_coordinate_reference_reports_self_reference() {
        let engine = CalcEngine::new();
        let lines = vec![
            "| item | value | total |".to_string(),
            "| --- | --- | --- |".to_string(),
            "| a | 10 | :=(1,3) + 1 |".to_string(),
        ];
        let res = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(
            res.table_cell_results[2]
                .iter()
                .find(|c| c.cell_index == 2)
                .map(|c| c.value.clone()),
            Some(TABLE_REF_ERROR_SELF_REFERENCE.to_string())
        );
    }

    #[test]
    fn note_eval_table_coordinate_reference_resolves_formula_target_cell() {
        let engine = CalcEngine::new();
        let lines = vec![
            "| item | value | total |".to_string(),
            "| --- | --- | --- |".to_string(),
            "| a | 10 | :=sum_row() |".to_string(),
            "| b | 20 | :=(1,3) + 5 |".to_string(),
        ];
        let res = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(
            res.table_cell_results[3]
                .iter()
                .find(|c| c.cell_index == 2)
                .map(|c| c.value.clone()),
            Some("15".to_string())
        );
    }

    #[test]
    fn note_eval_table_coordinate_reference_reports_indirect_self_reference_cycle() {
        let engine = CalcEngine::new();
        let lines = vec![
            "| item | value | total |".to_string(),
            "| --- | --- | --- |".to_string(),
            "| a | 1 | :=(2,3) |".to_string(),
            "| b | 2 | :=(1,3) |".to_string(),
        ];
        let res = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(
            res.table_cell_results[2]
                .iter()
                .find(|c| c.cell_index == 2)
                .map(|c| c.value.clone()),
            Some(TABLE_REF_ERROR_CYCLE.to_string())
        );
        assert_eq!(
            res.table_cell_results[3]
                .iter()
                .find(|c| c.cell_index == 2)
                .map(|c| c.value.clone()),
            Some(TABLE_REF_ERROR_CYCLE.to_string())
        );
    }

    #[test]
    fn note_eval_table_coordinate_reference_emits_structured_diagnostics() {
        let engine = CalcEngine::new();
        let lines = vec![
            "| item | value | total |".to_string(),
            "| --- | --- | --- |".to_string(),
            "| a | text | |".to_string(),
            "| b | 2 | :=(1,2) |".to_string(),
        ];
        let res = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        let diagnostics = res.diagnostics.unwrap_or_default();
        assert!(diagnostics.iter().any(|d| {
            d.kind == "table-ref-non_numeric"
                && d.line == 4
                && d.message.contains(TABLE_REF_ERROR_NON_NUMERIC)
        }));
    }

    #[test]
    fn note_eval_table_coordinate_reference_accepts_whitespace_variants() {
        let engine = CalcEngine::new();
        let lines = vec![
            "| item | value | total |".to_string(),
            "| --- | --- | --- |".to_string(),
            "| a | 10 | |".to_string(),
            "| b | 20 | :=( 1 , 2 ) + (2, 2) |".to_string(),
        ];
        let res = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(
            res.table_cell_results[3]
                .iter()
                .find(|c| c.cell_index == 2)
                .map(|c| c.value.clone()),
            Some("30".to_string())
        );
    }

    #[test]
    fn note_eval_table_coordinate_reference_very_large_index_is_out_of_bounds() {
        let engine = CalcEngine::new();
        let lines = vec![
            "| item | value | total |".to_string(),
            "| --- | --- | --- |".to_string(),
            "| a | 10 | |".to_string(),
            "| b | 20 | :=(999999999999999999999999, 2) |".to_string(),
        ];
        let res = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(
            res.table_cell_results[3]
                .iter()
                .find(|c| c.cell_index == 2)
                .map(|c| c.value.clone()),
            Some(TABLE_REF_ERROR_OUT_OF_BOUNDS.to_string())
        );
    }

    #[test]
    fn note_eval_table_formulas_are_ignored_when_table_module_is_off() {
        let engine = CalcEngine::new();
        let lines = vec![
            "var := 0.5".to_string(),
            "| value |".to_string(),
            "| --- |".to_string(),
            "| 10 |".to_string(),
            "| :=sum_col() * var |".to_string(),
        ];
        let result = engine.evaluate_note_context(
            &lines,
            NoteEvaluationOptions {
                variables_enabled: true,
                table_enabled: false,
                eval_range: None,
                ..Default::default()
            },
        );
        assert_eq!(result.line_results[4], None);
        assert!(result.table_cell_results[4].is_empty());
    }

    #[test]
    fn note_eval_table_colon_eq_prefix_does_not_define_variable() {
        let engine = CalcEngine::new();
        let lines = vec!["| :=5+3 |".to_string()];
        let res = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        // The := prefix is a formula marker, not a variable assignment.
        assert!(res.variables.is_empty());
    }

    #[test]
    fn note_eval_table_cell_combines_row_formula_with_arithmetic() {
        let engine = CalcEngine::new();
        let lines = vec![
            "k := 10".to_string(),
            "| a | b | total |".to_string(),
            "| --- | --- | --- |".to_string(),
            "| 2 | 3 | :=sum_row() * 2 + k |".to_string(),
        ];
        let res = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        let val = res.table_cell_results[3]
            .iter()
            .find(|c| c.cell_index == 2)
            .map(|c| c.value.clone())
            .unwrap_or_default();
        let n = extract_first_number(&val).unwrap_or(f64::NAN);
        assert!((n - ((2.0 + 3.0) * 2.0 + 10.0)).abs() < 1e-6, "got {}", val);
    }

    #[test]
    fn note_eval_table_recognizes_compound_formula_cell_alongside_simple_ones() {
        let engine = CalcEngine::new();
        let lines = vec![
            "| g | h | t |".to_string(),
            "| --- | --- | --- |".to_string(),
            "| 4 | 1.94 |  |".to_string(),
            "| 5 | 1.98 |  |".to_string(),
            "| :=avg_col() | :=avg_col()*a+2 | :=sum_row() |".to_string(),
            "a := 4".to_string(),
        ];
        let res = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        let cells = &res.table_cell_results[4];
        let by_idx: FxHashMap<usize, String> = cells
            .iter()
            .map(|c| (c.cell_index, c.value.clone()))
            .collect();
        let avg_g = extract_first_number(by_idx.get(&0).cloned().unwrap_or_default().as_str())
            .unwrap_or(f64::NAN);
        let compound = extract_first_number(by_idx.get(&1).cloned().unwrap_or_default().as_str())
            .unwrap_or(f64::NAN);
        let total = extract_first_number(by_idx.get(&2).cloned().unwrap_or_default().as_str())
            .unwrap_or(f64::NAN);
        assert!((avg_g - 4.5).abs() < 1e-6, "avg_g = {}", avg_g);
        assert!(
            (compound - (1.96 * 4.0 + 2.0)).abs() < 1e-6,
            "compound = {}",
            compound
        );
        assert!(
            (total - (4.5 + (1.96 * 4.0 + 2.0))).abs() < 1e-6,
            "total = {}",
            total
        );
    }

    #[test]
    fn note_eval_table_two_avg_cols_then_sum_row_in_same_row() {
        let engine = CalcEngine::new();
        let lines = vec![
            "| name  | last | grade | height | total |".to_string(),
            "| ----- | ---- | ----- | ------ | ----- |".to_string(),
            "| Dusan | B    | 5     | 1.94   |       |".to_string(),
            "| Dusan | B    | 4     | 1.98   |       |".to_string(),
            "| asds  | s    | :=avg_col() | :=avg_col() | :=sum_row() |".to_string(),
        ];
        let res = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        let cells = &res.table_cell_results[4];
        let by_idx: FxHashMap<usize, String> = cells
            .iter()
            .map(|c| (c.cell_index, c.value.clone()))
            .collect();
        eprintln!("cells: {:?}", cells);
        let avg_grade = by_idx.get(&2).cloned().unwrap_or_default();
        let avg_height = by_idx.get(&3).cloned().unwrap_or_default();
        let total = by_idx.get(&4).cloned().unwrap_or_default();
        let n_grade = extract_first_number(&avg_grade).unwrap_or(f64::NAN);
        let n_height = extract_first_number(&avg_height).unwrap_or(f64::NAN);
        let n_total = extract_first_number(&total).unwrap_or(f64::NAN);
        assert!((n_grade - 4.5).abs() < 1e-6, "grade avg = {}", avg_grade);
        assert!(
            (n_height - 1.96).abs() < 1e-6,
            "height avg = {}",
            avg_height
        );
        assert!((n_total - (4.5 + 1.96)).abs() < 1e-6, "sum_row = {}", total);
    }

    #[test]
    fn note_eval_table_avg_col_formula_reacts_when_rows_are_added_above_formula() {
        let engine = CalcEngine::new();
        let lines_before = vec![
            "| value |".to_string(),
            "| --- |".to_string(),
            "| 4 |".to_string(),
            "| 6 |".to_string(),
            "| :=avg_col() |".to_string(),
        ];
        let before = engine.evaluate_note_context(&lines_before, NoteEvaluationOptions::default());
        assert_eq!(before.line_results[4].as_deref(), Some("5"));

        let lines_after = vec![
            "| value |".to_string(),
            "| --- |".to_string(),
            "| 4 |".to_string(),
            "| 6 |".to_string(),
            "| 10 |".to_string(),
            "| :=avg_col() |".to_string(),
        ];
        let after = engine.evaluate_note_context(&lines_after, NoteEvaluationOptions::default());
        let value = after.line_results[5].as_deref().unwrap_or("");
        let numeric = extract_first_number(value).unwrap_or(f64::NAN);
        assert!((numeric - (20.0 / 3.0)).abs() < 1e-6);
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
                table_enabled: true,
                eval_range: None,
                ..Default::default()
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
        assert_eq!(result.line_results, vec![None, Some("24".to_string())]);
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
            vec![None, Some("24".to_string()), Some("25".to_string())]
        );
    }

    #[test]
    fn note_eval_hides_literal_assignment_ghost_but_keeps_variable_value() {
        let engine = CalcEngine::new();
        let lines = vec!["x := 213323".to_string(), "x + 2".to_string()];

        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(result.line_results, vec![None, Some("213325".to_string())]);
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
                table_enabled: true,
                eval_range: Some((1, 3)),
                ..Default::default()
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
                table_enabled: true,
                eval_range: Some((2, 3)),
                ..Default::default()
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

    #[test]
    fn cached_range_evaluation_matches_uncached_and_follows_edits() {
        let engine = CalcEngine::new();
        let mut lines: Vec<String> = vec![
            "rate := [[abcd1234]].rate".to_string(),
            "base := 10".to_string(),
        ];
        for i in 0..40 {
            lines.push(format!("v{i} := base * {i} + rate"));
            lines.push(format!("v{i} * 2"));
            lines.push("| a | b |".to_string());
            lines.push("| --- | --- |".to_string());
            lines.push(format!("| {i} | :=(1,1) * base |"));
        }
        let options = |range: Option<(usize, usize)>, rate: f64| NoteEvaluationOptions {
            variables_enabled: true,
            table_enabled: true,
            cross_note_enabled: true,
            eval_range: range,
            extern_vars: vec![ExternVar {
                note_short_id: "abcd1234".to_string(),
                var_normalized: "rate".to_string(),
                value: rate,
            }],
            precomputed_refs: None,
        };
        let mut cache = NoteContextCache::default();
        let check = |lines: &[String], cache: &mut NoteContextCache, rate: f64| {
            for from in (0..lines.len()).step_by(37) {
                let range = Some((from, (from + 50).min(lines.len())));
                let cached =
                    engine.evaluate_note_context_cached(lines, options(range, rate), cache);
                let fresh = engine.evaluate_note_context(lines, options(range, rate));
                assert_eq!(cached.line_results, fresh.line_results, "range {range:?}");
                assert_eq!(cached.table_cell_results, fresh.table_cell_results);
            }
        };
        check(&lines, &mut cache, 0.5);
        lines[1] = "base := 20".to_string();
        check(&lines, &mut cache, 0.5);
        check(&lines, &mut cache, 2.0);
        assert_eq!(
            engine
                .evaluate_note_context_cached(&lines, options(Some((8, 9)), 2.0), &mut cache)
                .line_results[8]
                .as_deref(),
            Some("44")
        );
    }

    #[test]
    fn cached_evaluation_matches_fresh_evaluation_across_random_edits() {
        let pool = [
            "base := 10",
            "rate := [[abcd1234]].rate",
            "base := 3",
            "total := base * rate",
            "total + 1",
            "[[abcd1234]].missing * 2",
            "[[abcd1234]].rate * base",
            "4 + 5",
            "prose line",
            "",
            "| a | b |",
            "| --- | --- |",
            "| 2 | :=(1,1) * base |",
            "| sum | :=sum_col() |",
        ];
        let engine = CalcEngine::new();
        let options = |range: (usize, usize)| NoteEvaluationOptions {
            variables_enabled: true,
            table_enabled: true,
            cross_note_enabled: true,
            eval_range: Some(range),
            extern_vars: vec![ExternVar {
                note_short_id: "abcd1234".to_string(),
                var_normalized: "rate".to_string(),
                value: 0.5,
            }],
            precomputed_refs: None,
        };
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = |bound: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % bound.max(1) as u64) as usize
        };
        let mut lines: Vec<String> = (0..40).map(|i| pool[i % pool.len()].to_string()).collect();
        let mut cache = NoteContextCache::default();
        for step in 0..300 {
            let at = next(lines.len() + 1);
            match next(3) {
                0 => {
                    for _ in 0..1 + next(3) {
                        lines.insert(at.min(lines.len()), pool[next(pool.len())].to_string());
                    }
                }
                1 if lines.len() > 1 => {
                    let start = at.min(lines.len() - 1);
                    let end = (start + 1 + next(3)).min(lines.len());
                    lines.drain(start..end);
                }
                _ if !lines.is_empty() => {
                    let idx = at.min(lines.len() - 1);
                    lines[idx] = pool[next(pool.len())].to_string();
                }
                _ => {}
            }
            for _ in 0..2 {
                let from = next(lines.len());
                let range = (from, (from + 1 + next(15)).min(lines.len()));
                let cached =
                    engine.evaluate_note_context_cached(&lines, options(range), &mut cache);
                let fresh = engine.evaluate_note_context(&lines, options(range));
                assert_eq!(
                    cached.line_results, fresh.line_results,
                    "step {step} {range:?}"
                );
                assert_eq!(
                    cached.table_cell_results, fresh.table_cell_results,
                    "step {step}"
                );
                assert_eq!(cached.variables, fresh.variables, "step {step}");
                assert_eq!(cached.cross_note_refs, fresh.cross_note_refs, "step {step}");
            }
        }
    }

    #[test]
    fn variable_matcher_matches_the_word_bounded_regex_it_replaced() {
        let mut names: Vec<String> = [
            "tax", "tax rate", "rate", "a", "ab", "x_1", "total 2", "total", "b2b",
        ]
        .iter()
        .map(|n| n.to_string())
        .collect();
        names.sort_by_key(|name| (Reverse(name.len()), name.clone()));
        let escaped: Vec<String> = names.iter().map(|n| regex::escape(n)).collect();
        let reference = regex::RegexBuilder::new(&format!(r"\b(?:{})\b", escaped.join("|")))
            .unicode(false)
            .case_insensitive(true)
            .build()
            .unwrap();
        let matcher = VariableMatcher::new(&names).unwrap();
        let pieces = [
            "tax", "TAX", "rate", " ", "+", "_", "1", "2", "total", "a", "b", "é", "(", "Rate",
            "x_1", "ab", "b2b", "*",
        ];
        let mut seed = 0x1234_5678_u64;
        for _ in 0..5_000 {
            let mut text = String::new();
            for _ in 0..(seed % 9) {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                text.push_str(pieces[(seed % pieces.len() as u64) as usize]);
            }
            seed = seed.wrapping_add(1);
            let expected: Vec<(usize, usize)> = reference
                .find_iter(&text)
                .map(|m| (m.start(), m.end()))
                .collect();
            assert_eq!(matcher.find(&text), expected, "{text:?}");
            assert_eq!(
                matcher.is_match(&text),
                reference.is_match(&text),
                "{text:?}"
            );
        }
    }

    #[test]
    fn long_variable_chain_resolves_without_deep_recursion() {
        // A range evaluation resolves the chain on demand from its last link.
        // Run on a small stack so a recursive resolver would overflow here.
        let handle = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let links = 20_000;
                let mut lines = vec!["total 0 := 1".to_string()];
                for i in 1..links {
                    lines.push(format!("total {i} := total {} + 1", i - 1));
                }
                lines.push(format!("total {}", links - 1));
                let last = lines.len() - 1;
                CalcEngine::new().evaluate_note_context(
                    &lines,
                    NoteEvaluationOptions {
                        variables_enabled: true,
                        eval_range: Some((last, last + 1)),
                        ..Default::default()
                    },
                )
            })
            .expect("spawn evaluator");
        let result = handle.join().expect("evaluation did not overflow");
        assert_eq!(
            result.line_results.last().unwrap().as_deref(),
            Some("20000")
        );
    }

    #[test]
    fn variable_cycle_reports_diagnostics_and_no_value() {
        let lines = vec![
            "a := b + 1".to_string(),
            "b := a + 1".to_string(),
            "a".to_string(),
        ];
        let result = CalcEngine::new().evaluate_note_context(
            &lines,
            NoteEvaluationOptions {
                variables_enabled: true,
                ..Default::default()
            },
        );
        assert_eq!(result.line_results[2], None);
        let kinds: Vec<(&str, usize)> = result
            .diagnostics
            .iter()
            .flatten()
            .map(|d| (d.kind.as_str(), d.line))
            .collect();
        assert!(kinds.contains(&("cycle", 1)), "{kinds:?}");
        assert!(kinds.contains(&("unresolved-variable", 1)), "{kinds:?}");
        assert!(kinds.contains(&("unresolved-variable", 2)), "{kinds:?}");
    }

    // --- Cross-note variable tests ---

    #[test]
    fn scan_cross_note_refs_basic() {
        let lines = vec![
            "see [[ABCD1234]].monthly_income + 100".to_string(),
            "plain line".to_string(),
            "[[ABCD1234]].total cost + [[ZZZZZZZZ]].rate".to_string(),
        ];
        let refs = scan_cross_note_refs(&lines);
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].note_short_id, "abcd1234");
        assert_eq!(refs[0].var_normalized, "monthly_income");
        assert_eq!(refs[0].line, 1);
        // refs[1] = [[ABCD1234]].total cost, refs[2] = [[ZZZZZZZZ]].rate (both on line 3)
        assert_eq!(refs[1].note_short_id, "abcd1234");
        assert_eq!(refs[1].var_normalized, "total cost");
        assert_eq!(refs[1].line, 3);
        assert_eq!(refs[2].note_short_id, "zzzzzzzz");
        assert_eq!(refs[2].var_normalized, "rate");
        assert_eq!(refs[2].line, 3);
    }

    #[test]
    fn cross_note_ref_substituted_and_evaluated() {
        let engine = CalcEngine::new();
        let lines = vec!["[[ABCD1234]].budget + 500".to_string()];
        let options = NoteEvaluationOptions {
            extern_vars: vec![ExternVar {
                note_short_id: "abcd1234".to_string(),
                var_normalized: "budget".to_string(),
                value: 1000.0,
            }],
            ..Default::default()
        };
        let result = engine.evaluate_note_context(&lines, options);
        assert_eq!(result.line_results[0], Some("1500".to_string()));
        assert_eq!(result.cross_note_refs.len(), 1);
        assert_eq!(result.cross_note_refs[0].var_normalized, "budget");
    }

    #[test]
    fn cross_note_ref_unresolved_produces_no_output() {
        let engine = CalcEngine::new();
        let lines = vec!["[[ABCD1234]].missing + 500".to_string()];
        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(result.line_results[0], None);
        assert_eq!(result.cross_note_refs.len(), 1);
    }

    #[test]
    fn preprocess_line_cross_note_borrows_lines_it_does_not_rewrite() {
        let mut extern_map: FxHashMap<(String, String), f64> = FxHashMap::default();
        extern_map.insert(("abcd1234".to_string(), "budget".to_string()), 90.0);

        // No ref at all: no scan, no allocation.
        let (plain, unresolved) = preprocess_line_cross_note("just some prose", &extern_map);
        assert!(matches!(plain, Cow::Borrowed(_)));
        assert!(!unresolved);

        // A ref that cannot be resolved is left verbatim, so still no rewrite.
        let (missing, unresolved) =
            preprocess_line_cross_note("[[ABCD1234]].nope + 1", &extern_map);
        assert!(matches!(missing, Cow::Borrowed(_)));
        assert!(unresolved);
        assert_eq!(missing, "[[ABCD1234]].nope + 1");

        // A resolvable ref is substituted, which does require an owned line.
        let (rewritten, unresolved) =
            preprocess_line_cross_note("[[ABCD1234]].budget + 1", &extern_map);
        assert!(matches!(rewritten, Cow::Owned(_)));
        assert!(!unresolved);
        assert!(rewritten.contains("90"), "got: {rewritten}");
        assert!(rewritten.ends_with(" + 1"), "got: {rewritten}");
    }

    #[test]
    fn cross_note_ref_mixed_with_local_variable() {
        let engine = CalcEngine::new();
        let lines = vec!["x := 10".to_string(), "[[ABCD1234]].budget + x".to_string()];
        let options = NoteEvaluationOptions {
            extern_vars: vec![ExternVar {
                note_short_id: "abcd1234".to_string(),
                var_normalized: "budget".to_string(),
                value: 90.0,
            }],
            ..Default::default()
        };
        let result = engine.evaluate_note_context(&lines, options);
        assert_eq!(result.line_results[1], Some("100".to_string()));
    }

    #[test]
    fn variable_values_exported_in_result() {
        let engine = CalcEngine::new();
        let lines = vec!["x := 42".to_string(), "y := 8".to_string()];
        let result = engine.evaluate_note_context(&lines, NoteEvaluationOptions::default());
        assert_eq!(result.variable_values.get("x"), Some(&42.0));
        assert_eq!(result.variable_values.get("y"), Some(&8.0));
    }
}
