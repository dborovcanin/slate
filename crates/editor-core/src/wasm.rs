use js_sys::Array;
use serde::{Deserialize, Serialize};
use serde_wasm_bindgen::{from_value as js_from_value, to_value as js_to_value};
use wasm_bindgen::prelude::*;

use crate::calc_plan::{
    self, CalcEvalScopeDecision, CalcEvalWindowDecision, CalcRefreshPlan, CalcSegment,
    CommitMarkerLoc, IncrementalCalcPlan, TableFormulaSegment,
};
use crate::command_catalog::CommandId;
use crate::command_history;
use crate::context::ResolvedContext;
use crate::engine::{
    CommandDispatchKind, EditorEngine, HostClipWatchAction, HostCommandPlan, HostFoldAction,
    ModuleState,
};
use crate::folding;
use crate::format::format_markdown;
use crate::markdown_tokens::{
    self, CodeToken, InlineMarkerComponentRange, InlineToken, MarkdownAnalyzeResult,
    MarkdownLineInfo, WikiLinkMatch,
};
use crate::math_commands;
use crate::substitute;
use crate::table;
use crate::text_rules::{
    convert_line_to_list, rewrite_line_with_checklist_toggle_suffix, run_doc_change_rules,
    run_enter_rules, run_tab_rules, run_table_boundary_edit_rules, run_table_cell_navigation_rules,
    run_table_header_delete_column_rule, run_table_multiline_break_rule,
    run_table_pipe_insert_column_rule, ListKind, TabRuleOptions, TableBoundaryEditOptions,
    TextRuleOptions,
};
use crate::types::{
    CommandExecutionResult, CommandMode, EditorContextSnapshot, SelectionSnapshot, TextRange,
};
use crate::vim::{self, VimContext, VimIntent, VimKey, VimMode, VimState};
use crate::vim_actions::{self, VimActionExecutionResult, VimRegisterMode, VimRegisterValue};

#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

fn build_context<'a>(
    text: &'a str,
    selection_anchor: usize,
    selection_head: usize,
    has_changed_range: bool,
    changed_from: usize,
    changed_to: usize,
) -> ResolvedContext<'a> {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MarkdownTransactionKind {
    DocChange,
    Enter,
    Tab,
    TableCellNavigation,
    TablePipeInsertColumn,
    TableHeaderDeleteColumn,
    TableBoundaryEdit,
    TableMultilineBreak,
}

const fn default_true() -> bool {
    true
}

const fn default_false() -> bool {
    false
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarkdownTransactionRequest {
    kind: MarkdownTransactionKind,
    #[serde(default = "default_true")]
    markdown_autoformat: bool,
    #[serde(default = "default_true")]
    checklist_auto_reorder: bool,
    #[serde(default = "default_false")]
    outdent: bool,
    #[serde(default = "default_true")]
    backward: bool,
    #[serde(default = "default_false")]
    structural_merge: bool,
    #[serde(default = "default_true")]
    table_enabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MarkdownTransactionResultWire {
    kind: MarkdownTransactionKind,
    operation: crate::types::EditOperation,
}

fn to_js_value<T: Serialize + ?Sized>(value: &T) -> Option<JsValue> {
    js_to_value(value).ok()
}

fn from_js_value<T: for<'de> Deserialize<'de>>(value: JsValue) -> Option<T> {
    js_from_value(value).ok()
}

fn run_markdown_transaction(
    ctx: &ResolvedContext<'_>,
    request: MarkdownTransactionRequest,
) -> Option<crate::types::EditOperation> {
    match request.kind {
        MarkdownTransactionKind::DocChange => run_doc_change_rules(
            ctx,
            TextRuleOptions {
                markdown_autoformat: request.markdown_autoformat,
                checklist_auto_reorder: request.checklist_auto_reorder,
                table_enabled: request.table_enabled,
            },
        ),
        MarkdownTransactionKind::Enter => run_enter_rules(
            ctx,
            TextRuleOptions {
                markdown_autoformat: request.markdown_autoformat,
                checklist_auto_reorder: true,
                table_enabled: request.table_enabled,
            },
        ),
        MarkdownTransactionKind::Tab => run_tab_rules(
            ctx,
            TabRuleOptions {
                markdown_autoformat: request.markdown_autoformat,
                outdent: request.outdent,
                table_enabled: request.table_enabled,
            },
        ),
        MarkdownTransactionKind::TableCellNavigation => run_table_cell_navigation_rules(
            ctx,
            TabRuleOptions {
                markdown_autoformat: request.markdown_autoformat,
                outdent: request.outdent,
                table_enabled: request.table_enabled,
            },
        ),
        MarkdownTransactionKind::TablePipeInsertColumn => run_table_pipe_insert_column_rule(ctx),
        MarkdownTransactionKind::TableHeaderDeleteColumn => {
            run_table_header_delete_column_rule(ctx)
        }
        MarkdownTransactionKind::TableBoundaryEdit => run_table_boundary_edit_rules(
            ctx,
            TableBoundaryEditOptions {
                markdown_autoformat: request.markdown_autoformat,
                backward: request.backward,
                structural_merge: request.structural_merge,
                table_enabled: request.table_enabled,
            },
        ),
        MarkdownTransactionKind::TableMultilineBreak => {
            run_table_multiline_break_rule(ctx, request.table_enabled)
        }
    }
}

fn js_markdown_transaction_requests(value: JsValue) -> Option<Vec<MarkdownTransactionRequest>> {
    from_js_value(value)
}

/// Run one-or-more markdown transactions through a single wasm boundary crossing.
/// Returns {kind, operation} for the first transaction that produced an edit operation.
#[wasm_bindgen]
pub fn wasm_run_markdown_transaction_batch(
    text: &str,
    selection_anchor: usize,
    selection_head: usize,
    has_changed_range: bool,
    changed_from: usize,
    changed_to: usize,
    transactions: JsValue,
) -> Option<JsValue> {
    let requests = js_markdown_transaction_requests(transactions)?;
    if requests.is_empty() {
        return None;
    }

    let ctx = build_context(
        text,
        selection_anchor,
        selection_head,
        has_changed_range,
        changed_from,
        changed_to,
    );
    for request in requests {
        let Some(operation) = run_markdown_transaction(&ctx, request) else {
            continue;
        };
        return to_js_value(&MarkdownTransactionResultWire {
            kind: request.kind,
            operation,
        });
    }
    None
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
    to_js_value(&formatted).unwrap_or_else(|| Array::new().into())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TableCursorCellInfoWire {
    column_index: usize,
    column_count: usize,
    left_pipe: usize,
    right_pipe: usize,
    trim_start: usize,
    trim_end: usize,
    edit_start: usize,
    navigation_anchor: usize,
    logical_row_index: Option<usize>,
    logical_row_count: usize,
    is_continuation_row: bool,
}

#[wasm_bindgen]
pub fn wasm_table_cursor_cell_info(
    block_lines: Vec<String>,
    line_index: usize,
    col: usize,
) -> Option<JsValue> {
    let info = table::table_cell_cursor_info_in_document(&block_lines, line_index, col)?;
    to_js_value(&TableCursorCellInfoWire {
        column_index: info.column_index,
        column_count: info.column_count,
        left_pipe: info.left_pipe,
        right_pipe: info.right_pipe,
        trim_start: info.trim_start,
        trim_end: info.trim_end,
        edit_start: info.edit_start(),
        navigation_anchor: info.navigation_anchor(),
        logical_row_index: info.logical_row_index,
        logical_row_count: info.logical_row_count,
        is_continuation_row: info.is_continuation_row,
    })
}

fn parse_mode(mode: &str) -> Option<CommandMode> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "vim" => Some(CommandMode::Vim),
        "editor" => Some(CommandMode::Editor),
        _ => None,
    }
}

fn command_dispatch_kind_to_str(kind: CommandDispatchKind) -> &'static str {
    match kind {
        CommandDispatchKind::Core => "core",
        CommandDispatchKind::HostDate => "host_date",
        CommandDispatchKind::HostNotify => "host_notify",
        CommandDispatchKind::HostNotifyDelete => "host_notify_delete",
        CommandDispatchKind::HostWrite => "host_write",
        CommandDispatchKind::HostModule => "host_module",
        CommandDispatchKind::HostFold => "host_fold",
        CommandDispatchKind::HostClipWatch => "host_clip_watch",
        CommandDispatchKind::HostNoteSecurity => "host_note_security",
        CommandDispatchKind::Quit => "quit",
    }
}

fn module_command_value(command_id: CommandId) -> Option<&'static str> {
    match command_id {
        CommandId::ModuleStatus => Some("module status"),
        CommandId::ModuleOnMath => Some("module math on"),
        CommandId::ModuleOffMath => Some("module math off"),
        CommandId::ModuleToggleMath => Some("module math toggle"),
        CommandId::ModuleOnTable => Some("module table on"),
        CommandId::ModuleOffTable => Some("module table off"),
        CommandId::ModuleToggleTable => Some("module table toggle"),
        CommandId::ModuleOnVariables => Some("module variables on"),
        CommandId::ModuleOffVariables => Some("module variables off"),
        CommandId::ModuleToggleVariables => Some("module variables toggle"),
        CommandId::ModuleOnStyle => Some("module style on"),
        CommandId::ModuleOffStyle => Some("module style off"),
        CommandId::ModuleToggleStyle => Some("module style toggle"),
        _ => None,
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum HostCommandPlanWire {
    Date,
    Notify,
    NotifyDelete,
    Write { quit: bool, force: bool },
    Module { command: String },
    Fold { action: String },
    ClipWatch { action: String },
    NoteSecurity { action: String, password: String },
    Quit { force: bool },
}

fn host_command_plan_to_js(plan: HostCommandPlan) -> Option<JsValue> {
    let wire = match plan {
        HostCommandPlan::Date => HostCommandPlanWire::Date,
        HostCommandPlan::Notify => HostCommandPlanWire::Notify,
        HostCommandPlan::NotifyDelete => HostCommandPlanWire::NotifyDelete,
        HostCommandPlan::Write { quit, force } => HostCommandPlanWire::Write { quit, force },
        HostCommandPlan::Module { command_id } => HostCommandPlanWire::Module {
            command: module_command_value(command_id)?.to_string(),
        },
        HostCommandPlan::Fold { action } => HostCommandPlanWire::Fold {
            action: match action {
                HostFoldAction::Fold => "fold",
                HostFoldAction::Unfold => "unfold",
                HostFoldAction::Toggle => "toggle",
            }
            .to_string(),
        },
        HostCommandPlan::ClipWatch { action } => HostCommandPlanWire::ClipWatch {
            action: match action {
                HostClipWatchAction::Start => "start",
                HostClipWatchAction::Stop => "stop",
            }
            .to_string(),
        },
        HostCommandPlan::NoteSecurity { action, password } => HostCommandPlanWire::NoteSecurity {
            action: action.as_str().to_string(),
            password,
        },
        HostCommandPlan::Quit { force } => HostCommandPlanWire::Quit { force },
    };
    to_js_value(&wire)
}

#[derive(Debug, Serialize)]
struct ParsedNoteSecurityCommandWire<'a> {
    action: &'a str,
    password: &'a str,
}

#[wasm_bindgen]
pub fn wasm_normalize_command(raw_input: &str) -> String {
    EditorEngine::normalize_command(raw_input)
}

#[wasm_bindgen]
pub fn wasm_list_command_suggestions(mode: &str, raw_input: &str) -> Option<JsValue> {
    let mode = parse_mode(mode)?;
    let suggestions = EditorEngine::list_command_suggestions(mode, raw_input);
    to_js_value(&suggestions)
}

#[wasm_bindgen]
pub fn wasm_resolve_command(mode: &str, raw_input: &str) -> Option<String> {
    let mode = parse_mode(mode)?;
    let command = EditorEngine::resolve_command(mode, raw_input)?;
    Some(command.value.to_string())
}

#[wasm_bindgen]
pub fn wasm_classify_command_dispatch(mode: &str, raw_input: &str) -> Option<String> {
    let mode = parse_mode(mode)?;
    let kind = EditorEngine::classify_command_dispatch(mode, raw_input)?;
    Some(command_dispatch_kind_to_str(kind).to_string())
}

#[wasm_bindgen]
pub fn wasm_plan_host_command(mode: &str, raw_input: &str) -> Option<JsValue> {
    let mode = parse_mode(mode)?;
    let plan = EditorEngine::plan_host_command(mode, raw_input)?;
    host_command_plan_to_js(plan)
}

#[wasm_bindgen]
pub fn wasm_execute_math_command(
    text: &str,
    selection_anchor: usize,
    selection_head: usize,
    raw_input: &str,
    mode: &str,
) -> Option<JsValue> {
    let mode = parse_mode(mode)?;
    let command = EditorEngine::resolve_command(mode, raw_input)?;
    let snapshot = EditorContextSnapshot {
        text: text.to_string(),
        selection: SelectionSnapshot {
            anchor: selection_anchor,
            head: selection_head,
        },
        changed_range: None,
    };
    let result = math_commands::execute_math_command(&snapshot, command.id)?;
    command_execution_result_to_js(&result)
}

#[wasm_bindgen]
pub fn wasm_parse_note_security_command(raw_input: &str) -> Option<JsValue> {
    let parsed = EditorEngine::parse_note_security_command(raw_input)?;
    to_js_value(&ParsedNoteSecurityCommandWire {
        action: parsed.action.as_str(),
        password: parsed.password.as_str(),
    })
}

#[wasm_bindgen]
pub fn wasm_plan_module_command(
    mode: &str,
    raw_input: &str,
    math: bool,
    table: bool,
    variables: bool,
    style: bool,
) -> Option<JsValue> {
    let mode = parse_mode(mode)?;
    let command = EditorEngine::resolve_command(mode, raw_input)?;
    let current = ModuleState {
        math,
        table,
        variables,
        style,
    };
    let plan = EditorEngine::plan_module_command(command.id, current)?;
    to_js_value(&plan)
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
    command_execution_result_to_js(&result)
}

#[wasm_bindgen]
pub fn wasm_execute_vim_action(
    text: &str,
    selection_anchor: usize,
    selection_head: usize,
    intent: JsValue,
    count: usize,
    register_text: &str,
    register_mode: &str,
) -> Option<JsValue> {
    let intent: VimIntent = from_js_value(intent)?;
    let snapshot = EditorContextSnapshot {
        text: text.to_string(),
        selection: SelectionSnapshot {
            anchor: selection_anchor,
            head: selection_head,
        },
        changed_range: None,
    };
    let register_mode = parse_vim_register_mode(register_mode);
    let register = register_mode.map(|mode| VimRegisterValue {
        text: register_text.to_string(),
        mode,
    });
    let result = vim_actions::execute_vim_action(&snapshot, intent, count, register.as_ref())?;
    vim_action_execution_result_to_js(&result)
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
    to_js_value(&history)
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
    to_js_value(&step)
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
    to_js_value(&step)
}

fn command_execution_result_to_js(result: &CommandExecutionResult) -> Option<JsValue> {
    to_js_value(result)
}

fn calc_segment_to_js(segment: &CalcSegment) -> Option<JsValue> {
    to_js_value(segment)
}

fn table_formula_segment_to_js(segment: &TableFormulaSegment) -> Option<JsValue> {
    to_js_value(segment)
}

fn js_strings(value: JsValue) -> Option<Vec<String>> {
    from_js_value(value)
}

fn js_optional_strings(value: JsValue) -> Option<Vec<Option<String>>> {
    from_js_value(value)
}

fn js_usize(value: JsValue) -> Option<Vec<usize>> {
    from_js_value(value)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FoldRangeWire {
    start_line: usize,
    end_line: usize,
    kind: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FoldLineEditWire {
    old_start_line: usize,
    old_line_span: usize,
    new_line_span: usize,
    old_line_text: String,
    new_line_text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FoldRangeOutWire<'a> {
    start_line: usize,
    end_line: usize,
    kind: &'a str,
}

fn parse_fold_kind(value: &str) -> Option<folding::FoldKind> {
    match value {
        "heading" => Some(folding::FoldKind::Heading),
        "fence" => Some(folding::FoldKind::Fence),
        "list" => Some(folding::FoldKind::List),
        "table" => Some(folding::FoldKind::Table),
        "paragraph" => Some(folding::FoldKind::Paragraph),
        _ => None,
    }
}

fn js_fold_ranges_1_based(value: JsValue) -> Option<Vec<folding::FoldRange>> {
    let raw_ranges: Vec<FoldRangeWire> = from_js_value(value)?;
    let mut out = Vec::with_capacity(raw_ranges.len());
    for range in raw_ranges {
        let start_line_1 = range.start_line;
        let end_line_1 = range.end_line;
        if start_line_1 == 0 || end_line_1 == 0 {
            return None;
        }
        let kind = parse_fold_kind(&range.kind)?;
        let start_line = start_line_1 - 1;
        let end_line = end_line_1 - 1;
        if end_line <= start_line {
            continue;
        }
        out.push(folding::FoldRange {
            start_line,
            end_line,
            kind,
        });
    }
    Some(out)
}

fn js_fold_line_edits_1_based(value: JsValue) -> Option<Vec<folding::FoldLineEdit>> {
    let raw_edits: Vec<FoldLineEditWire> = from_js_value(value)?;
    let mut out = Vec::with_capacity(raw_edits.len());
    for edit in raw_edits {
        let old_start_line_1 = edit.old_start_line;
        if old_start_line_1 == 0 {
            return None;
        }
        let old_line_span = edit.old_line_span.max(1);
        let new_line_span = edit.new_line_span.max(1);
        out.push(folding::FoldLineEdit {
            old_start_line: old_start_line_1 - 1,
            old_line_span,
            new_line_span,
            old_line_text: edit.old_line_text,
            new_line_text: edit.new_line_text,
        });
    }
    Some(out)
}

fn js_commit_markers(value: JsValue) -> Option<Vec<CommitMarkerLoc>> {
    from_js_value(value)
}

fn incremental_plan_to_js(plan: &IncrementalCalcPlan) -> Option<JsValue> {
    to_js_value(plan)
}

fn calc_refresh_plan_to_js(plan: &CalcRefreshPlan) -> Option<JsValue> {
    to_js_value(plan)
}

fn calc_eval_scope_decision_to_js(decision: &CalcEvalScopeDecision) -> Option<JsValue> {
    to_js_value(decision)
}

fn calc_eval_window_decision_to_js(decision: &CalcEvalWindowDecision) -> Option<JsValue> {
    to_js_value(decision)
}

fn markdown_line_info_to_js(info: &MarkdownLineInfo) -> Option<JsValue> {
    to_js_value(info)
}

fn inline_tokens_to_js(tokens: &[InlineToken]) -> Option<JsValue> {
    to_js_value(tokens)
}

fn code_tokens_to_js(tokens: &[CodeToken]) -> Option<JsValue> {
    to_js_value(tokens)
}

fn inline_marker_component_ranges_to_js(ranges: &[InlineMarkerComponentRange]) -> Option<JsValue> {
    to_js_value(ranges)
}

fn fold_ranges_to_js_1_based(ranges: &[folding::FoldRange]) -> Option<JsValue> {
    let wire: Vec<FoldRangeOutWire<'_>> = ranges
        .iter()
        .map(|range| FoldRangeOutWire {
            start_line: range.start_line + 1,
            end_line: range.end_line + 1,
            kind: range.kind.as_str(),
        })
        .collect();
    to_js_value(&wire)
}

fn markdown_analyze_result_to_js(result: &MarkdownAnalyzeResult) -> Option<JsValue> {
    to_js_value(result)
}

fn wiki_link_match_to_js(entry: &WikiLinkMatch) -> Option<JsValue> {
    to_js_value(entry)
}

fn parse_vim_register_mode(value: &str) -> Option<VimRegisterMode> {
    match value {
        "charwise" => Some(VimRegisterMode::Charwise),
        "linewise" => Some(VimRegisterMode::Linewise),
        _ => None,
    }
}

fn vim_action_execution_result_to_js(result: &VimActionExecutionResult) -> Option<JsValue> {
    to_js_value(result)
}

#[wasm_bindgen]
pub fn wasm_markdown_classify_line(line_text: &str) -> JsValue {
    markdown_line_info_to_js(&markdown_tokens::classify_markdown_line(line_text))
        .unwrap_or(JsValue::NULL)
}

#[wasm_bindgen]
pub fn wasm_markdown_find_inline_tokens(line_text: &str) -> JsValue {
    inline_tokens_to_js(&markdown_tokens::tokenize_inline_markdown(line_text))
        .unwrap_or_else(|| Array::new().into())
}

#[wasm_bindgen]
pub fn wasm_markdown_find_image_matches(line_text: &str) -> JsValue {
    to_js_value(&markdown_tokens::find_markdown_image_matches(line_text))
        .unwrap_or_else(|| Array::new().into())
}

#[wasm_bindgen]
pub fn wasm_markdown_wiki_link_at_cursor(line_text: &str, cursor_col: usize) -> JsValue {
    markdown_tokens::wiki_link_at_cursor(line_text, cursor_col)
        .as_ref()
        .and_then(wiki_link_match_to_js)
        .unwrap_or(JsValue::NULL)
}

#[wasm_bindgen]
pub fn wasm_markdown_inline_marker_component_ranges(line_text: &str) -> JsValue {
    inline_marker_component_ranges_to_js(&markdown_tokens::inline_marker_component_ranges(
        line_text,
    ))
    .unwrap_or_else(|| Array::new().into())
}

#[wasm_bindgen]
pub fn wasm_markdown_tokenize_code_line(line_text: &str, lang: Option<String>) -> JsValue {
    code_tokens_to_js(&markdown_tokens::tokenize_code_line(
        line_text,
        lang.as_deref(),
    ))
    .unwrap_or_else(|| Array::new().into())
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
    markdown_analyze_result_to_js(&result)
}

#[wasm_bindgen]
pub fn wasm_markdown_build_fold_ranges_ui(lines: JsValue) -> Option<JsValue> {
    let lines = js_strings(lines)?;
    let ranges = folding::build_fold_ranges_ui(&lines);
    fold_ranges_to_js_1_based(&ranges)
}

#[wasm_bindgen]
pub fn wasm_markdown_fold_map_ranges_ui(
    ranges: JsValue,
    edits: JsValue,
    new_line_count: usize,
) -> Option<JsValue> {
    let ranges = js_fold_ranges_1_based(ranges)?;
    let edits = js_fold_line_edits_1_based(edits)?;
    let mapped = folding::map_ranges_through_line_edits(&ranges, &edits, new_line_count.max(1));
    fold_ranges_to_js_1_based(&mapped)
}

#[wasm_bindgen]
pub fn wasm_markdown_fold_edits_require_rebuild_ui(edits: JsValue) -> Option<bool> {
    let edits = js_fold_line_edits_1_based(edits)?;
    Some(folding::edits_require_rebuild(&edits))
}

#[wasm_bindgen]
pub fn wasm_calc_find_single_table_cell(line_text: &str) -> Option<JsValue> {
    let segment: CalcSegment = calc_plan::find_single_calc_table_cell(line_text)?;
    calc_segment_to_js(&segment)
}

#[wasm_bindgen]
pub fn wasm_calc_find_list_segment(line_text: &str) -> Option<JsValue> {
    let segment: CalcSegment = calc_plan::find_list_calc_segment(line_text)?;
    calc_segment_to_js(&segment)
}

#[wasm_bindgen]
pub fn wasm_calc_find_segment(line_text: &str) -> Option<JsValue> {
    let segment: CalcSegment = calc_plan::find_calc_segment(line_text)?;
    calc_segment_to_js(&segment)
}

#[wasm_bindgen]
pub fn wasm_calc_find_table_formula_segment(line_text: &str) -> Option<JsValue> {
    let segment: TableFormulaSegment = calc_plan::find_table_formula_segment(line_text)?;
    table_formula_segment_to_js(&segment)
}

#[wasm_bindgen]
pub fn wasm_calc_find_table_formula_segments(line_text: &str) -> JsValue {
    let segments = calc_plan::find_table_formula_segments(line_text);
    to_js_value(&segments).unwrap_or_else(|| Array::new().into())
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
    let labels = calc_plan::builtin_formula_labels_in_text(text);
    to_js_value(&labels).unwrap_or_else(|| Array::new().into())
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
pub fn wasm_calc_decide_eval_scope(
    eval_lines: JsValue,
    prev_changed_lines: JsValue,
    has_prev: bool,
    variables_enabled: bool,
    table_enabled: bool,
) -> Option<JsValue> {
    let eval_lines = js_strings(eval_lines)?;
    let prev_changed_lines = js_strings(prev_changed_lines)?;
    let decision = calc_plan::decide_eval_scope_with_mask(
        &eval_lines,
        &prev_changed_lines,
        has_prev,
        calc_plan::CalcFeatureMask {
            math_enabled: true,
            table_enabled,
            variables_enabled,
        },
    );
    calc_eval_scope_decision_to_js(&decision)
}

#[wasm_bindgen]
pub fn wasm_calc_decide_eval_window(
    lines: JsValue,
    changed_from: usize,
    changed_to: usize,
    prev_changed_lines: JsValue,
    has_prev: bool,
    variables_enabled: bool,
    table_enabled: bool,
) -> Option<JsValue> {
    let lines = js_strings(lines)?;
    let prev_changed_lines = js_strings(prev_changed_lines)?;
    let decision = calc_plan::decide_eval_window_with_mask(
        &lines,
        changed_from,
        changed_to,
        &prev_changed_lines,
        has_prev,
        calc_plan::CalcFeatureMask {
            math_enabled: true,
            table_enabled,
            variables_enabled,
        },
    );
    calc_eval_window_decision_to_js(&decision)
}

#[wasm_bindgen]
pub fn wasm_calc_should_schedule_eval(
    doc_line_count: usize,
    max_eval_lines: usize,
    has_global_syntax: bool,
    touches_calc_expression: bool,
    visible_has_calc_syntax: bool,
) -> bool {
    calc_plan::should_schedule_calc_eval(
        doc_line_count,
        max_eval_lines,
        has_global_syntax,
        touches_calc_expression,
        visible_has_calc_syntax,
    )
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
    incremental_plan_to_js(&plan)
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
    calc_refresh_plan_to_js(&plan)
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WasmVimStep {
    mode: VimMode,
    actions: Vec<vim::VimAction>,
    handled: bool,
}

fn encode_empty_step(mode: VimMode) -> JsValue {
    let empty = WasmVimStep {
        mode,
        actions: Vec::new(),
        handled: false,
    };
    to_js_value(&empty).unwrap_or(JsValue::NULL)
}

fn encode_step(step: &vim::VimStep) -> JsValue {
    let encoded = WasmVimStep {
        mode: step.state.mode,
        actions: step.actions.clone(),
        handled: step.handled,
    };
    to_js_value(&encoded).unwrap_or(JsValue::NULL)
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
    ) -> JsValue {
        let Some(key) = decode_key(key_kind, key_char) else {
            return encode_empty_step(self.state.mode);
        };

        let context = VimContext {
            has_search_matches,
            line_count,
        };
        let step = EditorEngine::step_vim(&self.state, key, &context);
        self.state = step.state.clone();
        encode_step(&step)
    }
}
