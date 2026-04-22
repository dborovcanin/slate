use js_sys::{Array, Object, Reflect};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::calc_plan::{
    self, CalcRefreshPlan, CalcSegment, CommitMarkerLoc, IncrementalCalcPlan, TableFormulaSegment,
};
use crate::command_catalog;
use crate::command_history;
use crate::context::ResolvedContext;
use crate::folding;
use crate::format::format_markdown;
use crate::markdown_tokens::{
    self, CodeToken, InlineMarkerComponentRange, InlineToken, MarkdownAnalyzeResult,
    MarkdownAnalyzedLine, MarkdownLineInfo,
};
use crate::substitute;
use crate::table;
use crate::text_rules::{
    convert_line_to_list, rewrite_line_with_checklist_toggle_suffix, run_doc_change_rules,
    run_enter_rules, run_tab_rules, run_table_boundary_edit_rules, run_table_cell_navigation_rules,
    run_table_header_delete_column_rule, run_table_pipe_insert_column_rule, ListKind,
    TabRuleOptions, TableBoundaryEditOptions, TextRuleOptions,
};
use crate::types::{
    CommandExecutionResult, CommandMode, EditorContextSnapshot, SelectionSnapshot, TextRange,
};
use crate::vim::{self, VimContext, VimIntent, VimKey, VimMode, VimState};

#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

fn build_context(
    text: &str,
    selection_anchor: usize,
    selection_head: usize,
    has_changed_range: bool,
    changed_from: usize,
    changed_to: usize,
) -> ResolvedContext {
    ResolvedContext::from_parts(
        text,
        SelectionSnapshot {
            anchor: selection_anchor,
            head: selection_head,
        },
        if has_changed_range {
            Some(TextRange {
                from: changed_from,
                to: changed_to,
            })
        } else {
            None
        },
    )
}

/// Run document-change text rules. Returns a structured JsValue {changes, selection?} or null.
#[wasm_bindgen]
pub fn wasm_run_doc_change_rules(
    text: &str,
    selection_anchor: usize,
    selection_head: usize,
    has_changed_range: bool,
    changed_from: usize,
    changed_to: usize,
    markdown_autoformat: bool,
    checklist_auto_reorder: bool,
) -> Option<JsValue> {
    let ctx = build_context(
        text,
        selection_anchor,
        selection_head,
        has_changed_range,
        changed_from,
        changed_to,
    );
    let options = TextRuleOptions {
        markdown_autoformat,
        checklist_auto_reorder,
    };
    let op = run_doc_change_rules(&ctx, options)?;
    Some(edit_operation_to_js(&op))
}

/// Run enter-key text rules. Returns a structured JsValue or null.
#[wasm_bindgen]
pub fn wasm_run_enter_rules(
    text: &str,
    selection_anchor: usize,
    selection_head: usize,
    has_changed_range: bool,
    changed_from: usize,
    changed_to: usize,
    markdown_autoformat: bool,
) -> Option<JsValue> {
    let ctx = build_context(
        text,
        selection_anchor,
        selection_head,
        has_changed_range,
        changed_from,
        changed_to,
    );
    let options = TextRuleOptions {
        markdown_autoformat,
        checklist_auto_reorder: true,
    };
    let op = run_enter_rules(&ctx, options)?;
    Some(edit_operation_to_js(&op))
}

/// Run tab-key text rules. Returns a structured JsValue or null.
#[wasm_bindgen]
pub fn wasm_run_tab_rules(
    text: &str,
    selection_anchor: usize,
    selection_head: usize,
    has_changed_range: bool,
    changed_from: usize,
    changed_to: usize,
    markdown_autoformat: bool,
    outdent: bool,
) -> Option<JsValue> {
    let ctx = build_context(
        text,
        selection_anchor,
        selection_head,
        has_changed_range,
        changed_from,
        changed_to,
    );
    let options = TabRuleOptions {
        markdown_autoformat,
        outdent,
    };
    let op = run_tab_rules(&ctx, options)?;
    Some(edit_operation_to_js(&op))
}

/// Run table cell navigation rules (Tab/Shift+Tab in tables). Returns a structured JsValue or null.
#[wasm_bindgen]
pub fn wasm_run_table_cell_navigation_rules(
    text: &str,
    selection_anchor: usize,
    selection_head: usize,
    has_changed_range: bool,
    changed_from: usize,
    changed_to: usize,
    markdown_autoformat: bool,
    outdent: bool,
) -> Option<JsValue> {
    let ctx = build_context(
        text,
        selection_anchor,
        selection_head,
        has_changed_range,
        changed_from,
        changed_to,
    );
    let options = TabRuleOptions {
        markdown_autoformat,
        outdent,
    };
    let op = run_table_cell_navigation_rules(&ctx, options)?;
    Some(edit_operation_to_js(&op))
}

/// Insert a new column when `|` is typed in the table header row.
/// Returns a structured JsValue or null when not applicable.
#[wasm_bindgen]
pub fn wasm_run_table_pipe_insert_column_rule(
    text: &str,
    selection_anchor: usize,
    selection_head: usize,
) -> Option<JsValue> {
    let ctx = build_context(text, selection_anchor, selection_head, false, 0, 0);
    let op = run_table_pipe_insert_column_rule(&ctx)?;
    Some(edit_operation_to_js(&op))
}

/// Delete a column when Ctrl+Backspace is pressed inside an empty header cell.
/// Returns a structured JsValue or null when not applicable.
#[wasm_bindgen]
pub fn wasm_run_table_header_delete_column_rule(
    text: &str,
    selection_anchor: usize,
    selection_head: usize,
) -> Option<JsValue> {
    let ctx = build_context(text, selection_anchor, selection_head, false, 0, 0);
    let op = run_table_header_delete_column_rule(&ctx)?;
    Some(edit_operation_to_js(&op))
}

/// Run table boundary edit rules (Backspace/Delete and explicit merge commands).
/// Returns a structured JsValue or null.
#[wasm_bindgen]
pub fn wasm_run_table_boundary_edit_rules(
    text: &str,
    selection_anchor: usize,
    selection_head: usize,
    has_changed_range: bool,
    changed_from: usize,
    changed_to: usize,
    markdown_autoformat: bool,
    backward: bool,
    structural_merge: bool,
) -> Option<JsValue> {
    let ctx = build_context(
        text,
        selection_anchor,
        selection_head,
        has_changed_range,
        changed_from,
        changed_to,
    );
    let options = TableBoundaryEditOptions {
        markdown_autoformat,
        backward,
        structural_merge,
    };
    let op = run_table_boundary_edit_rules(&ctx, options)?;
    Some(edit_operation_to_js(&op))
}

/// Rewrite a list line that ends with /x toggle suffix. Returns the rewritten line or null.
#[wasm_bindgen]
pub fn wasm_rewrite_line_with_checklist_toggle_suffix(line_text: &str) -> Option<String> {
    rewrite_line_with_checklist_toggle_suffix(line_text)
}

/// Convert a single line to the target list kind.
/// Returns the converted line, or null if kind is unknown or line is empty.
#[wasm_bindgen]
pub fn wasm_convert_line_to_list(line: &str, kind: &str, ordered_index: usize) -> Option<String> {
    let list_kind = match kind {
        "checklist" => ListKind::Checklist,
        "unordered" => ListKind::Unordered,
        "ordered" => ListKind::Ordered,
        _ => return None,
    };
    let (text, _changed) = convert_line_to_list(line, list_kind, ordered_index);
    Some(text)
}

/// Format a markdown document.
#[wasm_bindgen]
pub fn wasm_format_markdown(text: &str) -> String {
    format_markdown(text)
}

/// Format a markdown table block represented as row lines.
#[wasm_bindgen]
pub fn wasm_format_table_lines(lines: Vec<String>) -> JsValue {
    let formatted = table::format_table_lines(&lines);
    let out = Array::new();
    for line in formatted {
        out.push(&JsValue::from_str(&line));
    }
    out.into()
}

fn parse_mode(mode: &str) -> Option<CommandMode> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "vim" => Some(CommandMode::Vim),
        "editor" => Some(CommandMode::Editor),
        _ => None,
    }
}

#[wasm_bindgen]
pub fn wasm_normalize_command(raw_input: &str) -> String {
    command_catalog::normalize_command(raw_input)
}

#[wasm_bindgen]
pub fn wasm_list_command_suggestions(mode: &str, raw_input: &str) -> String {
    let Some(mode) = parse_mode(mode) else {
        return "[]".to_string();
    };
    let suggestions = command_catalog::list_command_suggestions(mode, raw_input);
    serde_json::to_string(&suggestions).unwrap_or_else(|_| "[]".to_string())
}

#[wasm_bindgen]
pub fn wasm_resolve_command(mode: &str, raw_input: &str) -> Option<String> {
    let mode = parse_mode(mode)?;
    let command = command_catalog::resolve_command(mode, raw_input)?;
    Some(command.value.to_string())
}

#[wasm_bindgen]
pub fn wasm_parse_note_security_command(raw_input: &str) -> Option<JsValue> {
    let parsed = command_catalog::parse_note_security_command(raw_input)?;
    let out = Object::new();
    let _ = set_prop(&out, "action", JsValue::from_str(parsed.action.as_str()));
    let _ = set_prop(
        &out,
        "password",
        JsValue::from_str(parsed.password.as_str()),
    );
    Some(out.into())
}

#[wasm_bindgen]
pub fn wasm_try_execute_vim_substitute(
    text: &str,
    selection_anchor: usize,
    selection_head: usize,
    raw_input: &str,
    mode: &str,
) -> Option<JsValue> {
    let mode = parse_mode(mode)?;
    let snapshot = EditorContextSnapshot {
        text: text.to_string(),
        selection: SelectionSnapshot {
            anchor: selection_anchor,
            head: selection_head,
        },
        changed_range: None,
    };
    let result = substitute::try_execute_vim_substitute(&snapshot, raw_input, mode)?;
    Some(command_execution_result_to_js(&result))
}

#[wasm_bindgen]
pub fn wasm_command_history_sanitize(raw_command: &str) -> String {
    command_history::sanitize_command(raw_command)
}

#[wasm_bindgen]
pub fn wasm_command_history_remember(
    history: JsValue,
    raw_command: &str,
    max_entries: usize,
) -> Option<JsValue> {
    let mut history = js_strings(history)?;
    command_history::remember_command(&mut history, raw_command, max_entries);
    let out = Array::new();
    for command in &history {
        out.push(&JsValue::from_str(command));
    }
    Some(out.into())
}

#[wasm_bindgen]
pub fn wasm_command_history_prev(history: JsValue, current_index: i32) -> Option<JsValue> {
    let history = js_strings(history)?;
    let current_index = if current_index < 0 {
        None
    } else {
        Some(current_index as usize)
    };
    let step = command_history::cycle_prev(&history, current_index)?;
    Some(command_history_step_to_js(&step))
}

#[wasm_bindgen]
pub fn wasm_command_history_next(history: JsValue, current_index: i32) -> Option<JsValue> {
    let history = js_strings(history)?;
    let current_index = if current_index < 0 {
        None
    } else {
        Some(current_index as usize)
    };
    let step = command_history::cycle_next(&history, current_index)?;
    Some(command_history_step_to_js(&step))
}

fn set_prop(obj: &Object, key: &str, value: JsValue) -> bool {
    Reflect::set(obj, &JsValue::from_str(key), &value).is_ok()
}

fn edit_operation_to_js(op: &crate::types::EditOperation) -> JsValue {
    let out = Object::new();
    let changes = Array::new();
    for change in &op.changes {
        let item = Object::new();
        let _ = set_prop(&item, "from", JsValue::from_f64(change.from as f64));
        let _ = set_prop(&item, "to", JsValue::from_f64(change.to as f64));
        let _ = set_prop(&item, "insert", JsValue::from_str(&change.insert));
        changes.push(&item.into());
    }
    let _ = set_prop(&out, "changes", changes.into());
    if let Some(sel) = &op.selection {
        let s = Object::new();
        let _ = set_prop(&s, "anchor", JsValue::from_f64(sel.anchor as f64));
        if let Some(h) = sel.head {
            let _ = set_prop(&s, "head", JsValue::from_f64(h as f64));
        }
        let _ = set_prop(&out, "selection", s.into());
    }
    out.into()
}

fn calc_segment_to_js(segment: &CalcSegment) -> JsValue {
    let out = Object::new();
    let _ = set_prop(&out, "expr", JsValue::from_str(&segment.expr));
    let _ = set_prop(&out, "fromCol", JsValue::from_f64(segment.from_col as f64));
    let _ = set_prop(&out, "toCol", JsValue::from_f64(segment.to_col as f64));
    let _ = set_prop(
        &out,
        "fromByte",
        JsValue::from_f64(segment.from_byte as f64),
    );
    let _ = set_prop(&out, "toByte", JsValue::from_f64(segment.to_byte as f64));
    out.into()
}

fn table_formula_segment_to_js(segment: &TableFormulaSegment) -> JsValue {
    let out = Object::new();
    let _ = set_prop(
        &out,
        "fromByte",
        JsValue::from_f64(segment.from_byte as f64),
    );
    let _ = set_prop(&out, "toByte", JsValue::from_f64(segment.to_byte as f64));
    let _ = set_prop(
        &out,
        "fromChar",
        JsValue::from_f64(segment.from_char as f64),
    );
    let _ = set_prop(&out, "toChar", JsValue::from_f64(segment.to_char as f64));
    let _ = set_prop(
        &out,
        "cellLeftPipeChar",
        JsValue::from_f64(segment.cell_left_pipe_char as f64),
    );
    let _ = set_prop(
        &out,
        "cellRightPipeChar",
        JsValue::from_f64(segment.cell_right_pipe_char as f64),
    );
    let _ = set_prop(
        &out,
        "cellIndex",
        JsValue::from_f64(segment.cell_index as f64),
    );
    let labels = Array::new();
    for label in &segment.labels {
        labels.push(&JsValue::from_str(label));
    }
    let _ = set_prop(&out, "labels", labels.into());
    out.into()
}

fn js_strings(value: JsValue) -> Option<Vec<String>> {
    let array: Array = value.dyn_into().ok()?;
    let mut out = Vec::with_capacity(array.length() as usize);
    for value in array.iter() {
        out.push(value.as_string()?);
    }
    Some(out)
}

fn js_optional_strings(value: JsValue) -> Option<Vec<Option<String>>> {
    let array: Array = value.dyn_into().ok()?;
    let mut out = Vec::with_capacity(array.length() as usize);
    for value in array.iter() {
        if value.is_null() || value.is_undefined() {
            out.push(None);
        } else {
            out.push(Some(value.as_string()?));
        }
    }
    Some(out)
}

fn js_usize(value: JsValue) -> Option<Vec<usize>> {
    let array: Array = value.dyn_into().ok()?;
    let mut out = Vec::with_capacity(array.length() as usize);
    for value in array.iter() {
        let number = value.as_f64()?;
        if !number.is_finite() || number < 0.0 || number.fract() != 0.0 {
            return None;
        }
        out.push(number as usize);
    }
    Some(out)
}

fn js_prop_usize(value: &JsValue, key: &str) -> Option<usize> {
    let raw = Reflect::get(value, &JsValue::from_str(key)).ok()?;
    let number = raw.as_f64()?;
    if !number.is_finite() || number < 0.0 || number.fract() != 0.0 {
        return None;
    }
    Some(number as usize)
}

fn js_prop_string(value: &JsValue, key: &str) -> Option<String> {
    Reflect::get(value, &JsValue::from_str(key))
        .ok()?
        .as_string()
}

fn js_commit_markers(value: JsValue) -> Option<Vec<CommitMarkerLoc>> {
    let array: Array = value.dyn_into().ok()?;
    let mut out = Vec::with_capacity(array.length() as usize);
    for value in array.iter() {
        out.push(CommitMarkerLoc {
            doc_pos: js_prop_usize(&value, "docPos")?,
            line_idx: js_prop_usize(&value, "lineIdx")?,
            offset_in_line: js_prop_usize(&value, "offsetInLine")?,
            last_literal: js_prop_string(&value, "lastLiteral")?,
        });
    }
    Some(out)
}

fn incremental_plan_to_js(plan: &IncrementalCalcPlan) -> JsValue {
    let out = Object::new();
    let base_results = Array::new();
    for entry in &plan.base_results {
        let item = Object::new();
        let _ = set_prop(&item, "lineIdx", JsValue::from_f64(entry.line_idx as f64));
        let _ = set_prop(&item, "result", JsValue::from_str(&entry.result));
        base_results.push(&item);
    }
    let eval_lines = Array::new();
    for line in &plan.eval_lines {
        eval_lines.push(&JsValue::from_str(line));
    }
    let _ = set_prop(&out, "baseResults", base_results.into());
    let _ = set_prop(&out, "evalFrom", JsValue::from_f64(plan.eval_from as f64));
    let _ = set_prop(&out, "evalTo", JsValue::from_f64(plan.eval_to as f64));
    let _ = set_prop(&out, "evalLines", eval_lines.into());
    out.into()
}

fn calc_refresh_plan_to_js(plan: &CalcRefreshPlan) -> JsValue {
    let out = Object::new();
    let changes = Array::new();
    for change in &plan.changes {
        let item = Object::new();
        let _ = set_prop(&item, "lineIdx", JsValue::from_f64(change.line_idx as f64));
        let _ = set_prop(&item, "from", JsValue::from_f64(change.from as f64));
        let _ = set_prop(&item, "to", JsValue::from_f64(change.to as f64));
        let _ = set_prop(&item, "insert", JsValue::from_str(&change.insert));
        let _ = set_prop(&item, "newLiteral", JsValue::from_str(&change.new_literal));
        changes.push(&item);
    }

    let prune = Array::new();
    for idx in &plan.prune {
        prune.push(&JsValue::from_f64(*idx as f64));
    }

    let synced = Array::new();
    for idx in &plan.synced_lines {
        synced.push(&JsValue::from_f64(*idx as f64));
    }

    let _ = set_prop(&out, "changes", changes.into());
    let _ = set_prop(&out, "prune", prune.into());
    let _ = set_prop(&out, "syncedLines", synced.into());
    out.into()
}

fn markdown_line_info_to_js(info: &MarkdownLineInfo) -> JsValue {
    let out = Object::new();
    let heading_level = info
        .heading_level
        .map(|v| JsValue::from_f64(v as f64))
        .unwrap_or(JsValue::NULL);
    let heading_marker_end = info
        .heading_marker_end
        .map(|v| JsValue::from_f64(v as f64))
        .unwrap_or(JsValue::NULL);
    let quote_marker_end = info
        .quote_marker_end
        .map(|v| JsValue::from_f64(v as f64))
        .unwrap_or(JsValue::NULL);
    let list_marker_end = info
        .list_marker_end
        .map(|v| JsValue::from_f64(v as f64))
        .unwrap_or(JsValue::NULL);
    let checklist_marker_start = info
        .checklist_marker_start
        .map(|v| JsValue::from_f64(v as f64))
        .unwrap_or(JsValue::NULL);
    let checklist_marker_end = info
        .checklist_marker_end
        .map(|v| JsValue::from_f64(v as f64))
        .unwrap_or(JsValue::NULL);
    let checklist_content_start = info
        .checklist_content_start
        .map(|v| JsValue::from_f64(v as f64))
        .unwrap_or(JsValue::NULL);

    let _ = set_prop(&out, "headingLevel", heading_level);
    let _ = set_prop(&out, "headingMarkerEnd", heading_marker_end);
    let _ = set_prop(&out, "quoteMarkerEnd", quote_marker_end);
    let _ = set_prop(&out, "listMarkerEnd", list_marker_end);
    let _ = set_prop(&out, "checklistMarkerStart", checklist_marker_start);
    let _ = set_prop(&out, "checklistMarkerEnd", checklist_marker_end);
    let _ = set_prop(&out, "checklistContentStart", checklist_content_start);
    let _ = set_prop(
        &out,
        "checklistChecked",
        JsValue::from_bool(info.checklist_checked),
    );
    let _ = set_prop(
        &out,
        "isHorizontalRule",
        JsValue::from_bool(info.is_horizontal_rule),
    );
    let _ = set_prop(&out, "isCodeFence", JsValue::from_bool(info.is_code_fence));
    out.into()
}

fn inline_token_to_js(token: &InlineToken) -> JsValue {
    let out = Object::new();
    let _ = set_prop(&out, "from", JsValue::from_f64(token.from as f64));
    let _ = set_prop(&out, "to", JsValue::from_f64(token.to as f64));
    let _ = set_prop(&out, "type", JsValue::from_str(token.kind.as_str()));
    out.into()
}

fn code_token_to_js(token: &CodeToken) -> JsValue {
    let out = Object::new();
    let _ = set_prop(&out, "from", JsValue::from_f64(token.from as f64));
    let _ = set_prop(&out, "to", JsValue::from_f64(token.to as f64));
    let _ = set_prop(&out, "type", JsValue::from_str(token.kind.as_str()));
    out.into()
}

fn inline_tokens_to_js(tokens: &[InlineToken]) -> JsValue {
    let out = Array::new();
    for token in tokens {
        out.push(&inline_token_to_js(token));
    }
    out.into()
}

fn code_tokens_to_js(tokens: &[CodeToken]) -> JsValue {
    let out = Array::new();
    for token in tokens {
        out.push(&code_token_to_js(token));
    }
    out.into()
}

fn inline_marker_component_range_to_js(range: &InlineMarkerComponentRange) -> JsValue {
    let out = Object::new();
    let _ = set_prop(&out, "from", JsValue::from_f64(range.from as f64));
    let _ = set_prop(&out, "to", JsValue::from_f64(range.to as f64));
    out.into()
}

fn inline_marker_component_ranges_to_js(ranges: &[InlineMarkerComponentRange]) -> JsValue {
    let out = Array::new();
    for range in ranges {
        out.push(&inline_marker_component_range_to_js(range));
    }
    out.into()
}

fn fold_range_to_js_1_based(range: &folding::FoldRange) -> JsValue {
    let out = Object::new();
    let _ = set_prop(
        &out,
        "startLine",
        JsValue::from_f64((range.start_line + 1) as f64),
    );
    let _ = set_prop(
        &out,
        "endLine",
        JsValue::from_f64((range.end_line + 1) as f64),
    );
    let _ = set_prop(&out, "kind", JsValue::from_str(range.kind.as_str()));
    out.into()
}

fn command_history_step_to_js(step: &command_history::CommandHistoryStep) -> JsValue {
    let out = Object::new();
    let _ = set_prop(&out, "index", JsValue::from_f64(step.index as f64));
    let _ = set_prop(&out, "command", JsValue::from_str(&step.command));
    out.into()
}

fn command_execution_result_to_js(result: &CommandExecutionResult) -> JsValue {
    let out = Object::new();
    let _ = set_prop(&out, "message", JsValue::from_str(&result.message));
    let operations = Array::new();
    for operation in &result.operations {
        operations.push(&edit_operation_to_js(operation));
    }
    let _ = set_prop(&out, "operations", operations.into());
    let _ = set_prop(
        &out,
        "clipboardText",
        result
            .clipboard_text
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
    );
    let _ = set_prop(
        &out,
        "quitRequested",
        JsValue::from_bool(result.quit_requested),
    );
    out.into()
}

fn markdown_analyzed_line_to_js(line: &MarkdownAnalyzedLine) -> JsValue {
    let out = Object::new();
    let _ = set_prop(&out, "info", markdown_line_info_to_js(&line.info));
    let _ = set_prop(&out, "inCodeBlock", JsValue::from_bool(line.in_code_block));
    let _ = set_prop(
        &out,
        "codeFenceLang",
        line.code_fence_lang
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
    );
    let _ = set_prop(
        &out,
        "inlineTokens",
        inline_tokens_to_js(&line.inline_tokens),
    );
    let _ = set_prop(&out, "codeTokens", code_tokens_to_js(&line.code_tokens));
    out.into()
}

fn markdown_analyze_result_to_js(result: &MarkdownAnalyzeResult) -> JsValue {
    let out = Object::new();
    let lines = Array::new();
    for line in &result.lines {
        lines.push(&markdown_analyzed_line_to_js(line));
    }
    let _ = set_prop(&out, "lines", lines.into());
    let _ = set_prop(
        &out,
        "finalInCodeBlock",
        JsValue::from_bool(result.final_in_code_block),
    );
    let _ = set_prop(
        &out,
        "finalCodeFenceLang",
        result
            .final_code_fence_lang
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
    );
    out.into()
}

#[wasm_bindgen]
pub fn wasm_markdown_classify_line(line_text: &str) -> JsValue {
    markdown_line_info_to_js(&markdown_tokens::classify_markdown_line(line_text))
}

#[wasm_bindgen]
pub fn wasm_markdown_find_inline_tokens(line_text: &str) -> JsValue {
    inline_tokens_to_js(&markdown_tokens::tokenize_inline_markdown(line_text))
}

#[wasm_bindgen]
pub fn wasm_markdown_inline_marker_component_ranges(line_text: &str) -> JsValue {
    inline_marker_component_ranges_to_js(&markdown_tokens::inline_marker_component_ranges(
        line_text,
    ))
}

#[wasm_bindgen]
pub fn wasm_markdown_tokenize_code_line(line_text: &str, lang: Option<String>) -> JsValue {
    code_tokens_to_js(&markdown_tokens::tokenize_code_line(
        line_text,
        lang.as_deref(),
    ))
}

#[wasm_bindgen]
pub fn wasm_markdown_is_code_fence(line_text: &str) -> bool {
    markdown_tokens::is_code_fence(line_text)
}

#[wasm_bindgen]
pub fn wasm_markdown_parse_fence_language(line_text: &str) -> Option<String> {
    markdown_tokens::parse_fence_language(line_text)
}

#[wasm_bindgen]
pub fn wasm_markdown_analyze_lines(
    lines: JsValue,
    start_in_code_block: bool,
    start_code_fence_lang: Option<String>,
) -> Option<JsValue> {
    let lines = js_strings(lines)?;
    let result = markdown_tokens::analyze_lines(
        &lines,
        start_in_code_block,
        start_code_fence_lang.as_deref(),
    );
    Some(markdown_analyze_result_to_js(&result))
}

#[wasm_bindgen]
pub fn wasm_markdown_build_fold_ranges_ui(lines: JsValue) -> Option<JsValue> {
    let lines = js_strings(lines)?;
    let ranges = folding::build_fold_ranges_ui(&lines);
    let out = Array::new();
    for range in &ranges {
        out.push(&fold_range_to_js_1_based(range));
    }
    Some(out.into())
}

#[wasm_bindgen]
pub fn wasm_calc_find_single_table_cell(line_text: &str) -> Option<JsValue> {
    let segment: CalcSegment = calc_plan::find_single_calc_table_cell(line_text)?;
    Some(calc_segment_to_js(&segment))
}

#[wasm_bindgen]
pub fn wasm_calc_find_list_segment(line_text: &str) -> Option<JsValue> {
    let segment: CalcSegment = calc_plan::find_list_calc_segment(line_text)?;
    Some(calc_segment_to_js(&segment))
}

#[wasm_bindgen]
pub fn wasm_calc_find_segment(line_text: &str) -> Option<JsValue> {
    let segment: CalcSegment = calc_plan::find_calc_segment(line_text)?;
    Some(calc_segment_to_js(&segment))
}

#[wasm_bindgen]
pub fn wasm_calc_find_table_formula_segment(line_text: &str) -> Option<JsValue> {
    let segment: TableFormulaSegment = calc_plan::find_table_formula_segment(line_text)?;
    Some(table_formula_segment_to_js(&segment))
}

#[wasm_bindgen]
pub fn wasm_calc_find_table_formula_segments(line_text: &str) -> JsValue {
    let segments = calc_plan::find_table_formula_segments(line_text);
    let out = Array::new();
    for segment in &segments {
        out.push(&table_formula_segment_to_js(segment));
    }
    out.into()
}

#[wasm_bindgen]
pub fn wasm_calc_line_for_eval(line_text: &str) -> String {
    calc_plan::line_for_calc_evaluation(line_text)
}

#[wasm_bindgen]
pub fn wasm_calc_is_builtin_formula(text: &str) -> bool {
    calc_plan::is_builtin_formula(text)
}

#[wasm_bindgen]
pub fn wasm_calc_builtin_formula_label(text: &str) -> Option<String> {
    calc_plan::builtin_formula_label(text)
}

#[wasm_bindgen]
pub fn wasm_calc_builtin_formula_labels(text: &str) -> JsValue {
    calc_plan::builtin_formula_labels_in_text(text)
        .into_iter()
        .map(|label| JsValue::from_str(&label))
        .collect::<Array>()
        .into()
}

#[wasm_bindgen]
pub fn wasm_calc_format_formula_display_value(raw: &str) -> String {
    calc_plan::format_formula_display_value(raw)
}

#[wasm_bindgen]
pub fn wasm_calc_line_uses_assignment_prefix(line_text: &str) -> bool {
    calc_plan::line_uses_assignment_ghost_prefix(line_text)
}

#[wasm_bindgen]
pub fn wasm_calc_contains_variable_assignment(lines: JsValue) -> Option<bool> {
    let lines = js_strings(lines)?;
    Some(calc_plan::contains_variable_assignment(&lines))
}

#[wasm_bindgen]
pub fn wasm_calc_contains_builtin_formula(lines: JsValue) -> Option<bool> {
    let lines = js_strings(lines)?;
    Some(calc_plan::contains_builtin_formula(&lines))
}

#[wasm_bindgen]
pub fn wasm_calc_plan_incremental(
    prev_lines: JsValue,
    prev_results: JsValue,
    next_lines: JsValue,
) -> Option<JsValue> {
    let prev_lines = js_strings(prev_lines)?;
    let prev_results = js_optional_strings(prev_results)?;
    let next_lines = js_strings(next_lines)?;
    let plan: IncrementalCalcPlan =
        calc_plan::plan_incremental_calc(&prev_lines, &prev_results, &next_lines);
    Some(incremental_plan_to_js(&plan))
}

#[wasm_bindgen]
pub fn wasm_calc_compute_refresh(
    markers: JsValue,
    lines: JsValue,
    line_starts: JsValue,
    next_results: JsValue,
    selection_from: usize,
    selection_to: usize,
) -> Option<JsValue> {
    let markers = js_commit_markers(markers)?;
    let lines = js_strings(lines)?;
    let line_starts = js_usize(line_starts)?;
    let next_results = js_optional_strings(next_results)?;
    let plan: CalcRefreshPlan = calc_plan::compute_calc_refresh(
        &markers,
        &lines,
        &line_starts,
        &next_results,
        selection_from,
        selection_to,
    );
    Some(calc_refresh_plan_to_js(&plan))
}

fn mode_from_id(id: u32) -> Option<VimMode> {
    match id {
        0 => Some(VimMode::Insert),
        1 => Some(VimMode::Normal),
        2 => Some(VimMode::Visual),
        3 => Some(VimMode::VisualLine),
        _ => None,
    }
}

fn mode_to_id(mode: VimMode) -> u32 {
    match mode {
        VimMode::Insert => 0,
        VimMode::Normal => 1,
        VimMode::Visual => 2,
        VimMode::VisualLine => 3,
    }
}

fn decode_key(kind: u32, key_char: u32) -> Option<VimKey> {
    match kind {
        0 => Some(VimKey::Esc),
        1 => Some(VimKey::Enter),
        2 => Some(VimKey::Tab),
        3 => Some(VimKey::Backspace),
        4 => Some(VimKey::Delete),
        5 => Some(VimKey::ArrowUp),
        6 => Some(VimKey::ArrowDown),
        7 => Some(VimKey::ArrowLeft),
        8 => Some(VimKey::ArrowRight),
        9 => char::from_u32(key_char).map(VimKey::Char),
        10 => char::from_u32(key_char).map(|ch| VimKey::Ctrl(ch.to_ascii_lowercase())),
        _ => None,
    }
}

fn intent_to_id(intent: VimIntent) -> u32 {
    match intent {
        VimIntent::MoveLeft => 0,
        VimIntent::MoveRight => 1,
        VimIntent::MoveUp => 2,
        VimIntent::MoveDown => 3,
        VimIntent::MoveWordForward => 4,
        VimIntent::MoveWordBackward => 5,
        VimIntent::MoveLineStart => 6,
        VimIntent::MoveLineEnd => 7,
        VimIntent::MoveDocStart => 8,
        VimIntent::MoveDocEnd => 9,
        VimIntent::MoveToLine => 10,
        VimIntent::EnterInsert => 11,
        VimIntent::AppendInsert => 12,
        VimIntent::InsertLineStart => 13,
        VimIntent::AppendLineEnd => 14,
        VimIntent::OpenLineBelow => 15,
        VimIntent::OpenLineAbove => 16,
        VimIntent::EnterVisual => 17,
        VimIntent::EnterVisualLine => 18,
        VimIntent::ExitVisual => 19,
        VimIntent::DeleteLine => 20,
        VimIntent::YankLine => 21,
        VimIntent::DeleteToLineStart => 22,
        VimIntent::DeleteToLineEnd => 23,
        VimIntent::YankToLineStart => 24,
        VimIntent::YankToLineEnd => 25,
        VimIntent::DeleteChar => 26,
        VimIntent::PasteAfter => 27,
        VimIntent::Undo => 28,
        VimIntent::Redo => 29,
        VimIntent::OpenCommandBar => 30,
        VimIntent::OpenSearch => 31,
        VimIntent::SearchNext => 32,
        VimIntent::SearchPrev => 33,
        VimIntent::DeleteInsideWord => 34,
        VimIntent::DeleteAroundWord => 35,
        VimIntent::YankInsideWord => 36,
        VimIntent::YankAroundWord => 37,
        VimIntent::DeleteInsidePipe => 38,
        VimIntent::DeleteAroundPipe => 39,
        VimIntent::YankInsidePipe => 40,
        VimIntent::YankAroundPipe => 41,
        VimIntent::Swallow => 42,
        VimIntent::DeleteWordForward => 43,
        VimIntent::DeleteWordBackward => 44,
        VimIntent::YankWordForward => 45,
        VimIntent::YankWordBackward => 46,
    }
}

fn encode_empty_step(mode: VimMode) -> Box<[u32]> {
    vec![0, mode_to_id(mode), 0].into_boxed_slice()
}

fn encode_step(step: &vim::VimStep) -> Box<[u32]> {
    let mut encoded = Vec::with_capacity(3 + step.actions.len() * 2);
    encoded.push(u32::from(step.handled));
    encoded.push(mode_to_id(step.state.mode));
    encoded.push(step.actions.len() as u32);
    for action in &step.actions {
        encoded.push(intent_to_id(action.intent));
        encoded.push(action.count.max(1) as u32);
    }
    encoded.into_boxed_slice()
}

#[wasm_bindgen]
pub struct WasmVimSession {
    state: VimState,
}

#[wasm_bindgen]
impl WasmVimSession {
    #[wasm_bindgen(constructor)]
    pub fn new(initial_mode: u32) -> WasmVimSession {
        let mut state = VimState::default();
        state.mode = mode_from_id(initial_mode).unwrap_or(VimMode::Normal);
        WasmVimSession { state }
    }

    pub fn step(
        &mut self,
        key_kind: u32,
        key_char: u32,
        has_search_matches: bool,
        line_count: usize,
    ) -> Box<[u32]> {
        let Some(key) = decode_key(key_kind, key_char) else {
            return encode_empty_step(self.state.mode);
        };

        let context = VimContext {
            has_search_matches,
            line_count,
        };
        let step = vim::step(&self.state, key, &context);
        self.state = step.state.clone();
        encode_step(&step)
    }
}
