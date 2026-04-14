use wasm_bindgen::prelude::*;

use crate::calc_plan::{
    self, CalcRefreshPlan, CalcSegment, CommitMarkerLoc, IncrementalCalcPlan, TableFormulaSegment,
};
use crate::command_catalog;
use crate::format::format_markdown;
use crate::text_rules::{
    rewrite_line_with_checklist_toggle_suffix, run_doc_change_rules, run_enter_rules,
    run_tab_rules, run_table_cell_navigation_rules, TabRuleOptions, TextRuleOptions,
};
use crate::types::{CommandMode, EditorContextSnapshot};
use crate::vim::{self, VimContext, VimIntent, VimKey, VimMode, VimState};

#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

/// Run document-change text rules. Returns JSON-encoded EditOperation or null.
#[wasm_bindgen]
pub fn wasm_run_doc_change_rules(snapshot_json: &str, markdown_autoformat: bool) -> Option<String> {
    let snapshot: EditorContextSnapshot = serde_json::from_str(snapshot_json).ok()?;
    let options = TextRuleOptions {
        markdown_autoformat,
    };
    let op = run_doc_change_rules(&snapshot, options)?;
    serde_json::to_string(&op).ok()
}

/// Run enter-key text rules. Returns JSON-encoded EditOperation or null.
#[wasm_bindgen]
pub fn wasm_run_enter_rules(snapshot_json: &str, markdown_autoformat: bool) -> Option<String> {
    let snapshot: EditorContextSnapshot = serde_json::from_str(snapshot_json).ok()?;
    let options = TextRuleOptions {
        markdown_autoformat,
    };
    let op = run_enter_rules(&snapshot, options)?;
    serde_json::to_string(&op).ok()
}

/// Run tab-key text rules. Returns JSON-encoded EditOperation or null.
#[wasm_bindgen]
pub fn wasm_run_tab_rules(
    snapshot_json: &str,
    markdown_autoformat: bool,
    outdent: bool,
) -> Option<String> {
    let snapshot: EditorContextSnapshot = serde_json::from_str(snapshot_json).ok()?;
    let options = TabRuleOptions {
        markdown_autoformat,
        outdent,
    };
    let op = run_tab_rules(&snapshot, options)?;
    serde_json::to_string(&op).ok()
}

/// Run table cell navigation rules (Tab/Shift+Tab in tables). Returns JSON-encoded EditOperation or null.
#[wasm_bindgen]
pub fn wasm_run_table_cell_navigation_rules(
    snapshot_json: &str,
    markdown_autoformat: bool,
    outdent: bool,
) -> Option<String> {
    let snapshot: EditorContextSnapshot = serde_json::from_str(snapshot_json).ok()?;
    let options = TabRuleOptions {
        markdown_autoformat,
        outdent,
    };
    let op = run_table_cell_navigation_rules(&snapshot, options)?;
    serde_json::to_string(&op).ok()
}

/// Rewrite a list line that ends with /x toggle suffix. Returns the rewritten line or null.
#[wasm_bindgen]
pub fn wasm_rewrite_line_with_checklist_toggle_suffix(line_text: &str) -> Option<String> {
    rewrite_line_with_checklist_toggle_suffix(line_text)
}

/// Format a markdown document.
#[wasm_bindgen]
pub fn wasm_format_markdown(text: &str) -> String {
    format_markdown(text)
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
pub fn wasm_calc_find_single_table_cell(line_text: &str) -> Option<String> {
    let segment: CalcSegment = calc_plan::find_single_calc_table_cell(line_text)?;
    serde_json::to_string(&segment).ok()
}

#[wasm_bindgen]
pub fn wasm_calc_find_list_segment(line_text: &str) -> Option<String> {
    let segment: CalcSegment = calc_plan::find_list_calc_segment(line_text)?;
    serde_json::to_string(&segment).ok()
}

#[wasm_bindgen]
pub fn wasm_calc_find_segment(line_text: &str) -> Option<String> {
    let segment: CalcSegment = calc_plan::find_calc_segment(line_text)?;
    serde_json::to_string(&segment).ok()
}

#[wasm_bindgen]
pub fn wasm_calc_find_table_formula_segment(line_text: &str) -> Option<String> {
    let segment: TableFormulaSegment = calc_plan::find_table_formula_segment(line_text)?;
    serde_json::to_string(&segment).ok()
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
pub fn wasm_calc_format_formula_display_value(raw: &str) -> String {
    calc_plan::format_formula_display_value(raw)
}

#[wasm_bindgen]
pub fn wasm_calc_line_uses_assignment_prefix(line_text: &str) -> bool {
    calc_plan::line_uses_assignment_ghost_prefix(line_text)
}

#[wasm_bindgen]
pub fn wasm_calc_contains_variable_assignment(lines_json: &str) -> Option<bool> {
    let lines: Vec<String> = serde_json::from_str(lines_json).ok()?;
    Some(calc_plan::contains_variable_assignment(&lines))
}

#[wasm_bindgen]
pub fn wasm_calc_contains_builtin_formula(lines_json: &str) -> Option<bool> {
    let lines: Vec<String> = serde_json::from_str(lines_json).ok()?;
    Some(calc_plan::contains_builtin_formula(&lines))
}

#[wasm_bindgen]
pub fn wasm_calc_plan_incremental(
    prev_lines_json: &str,
    prev_results_json: &str,
    next_lines_json: &str,
) -> Option<String> {
    let prev_lines: Vec<String> = serde_json::from_str(prev_lines_json).ok()?;
    let prev_results: Vec<Option<String>> = serde_json::from_str(prev_results_json).ok()?;
    let next_lines: Vec<String> = serde_json::from_str(next_lines_json).ok()?;
    let plan: IncrementalCalcPlan =
        calc_plan::plan_incremental_calc(&prev_lines, &prev_results, &next_lines);
    serde_json::to_string(&plan).ok()
}

#[wasm_bindgen]
pub fn wasm_calc_compute_refresh(
    markers_json: &str,
    lines_json: &str,
    line_starts_json: &str,
    next_results_json: &str,
    selection_from: usize,
    selection_to: usize,
) -> Option<String> {
    let markers: Vec<CommitMarkerLoc> = serde_json::from_str(markers_json).ok()?;
    let lines: Vec<String> = serde_json::from_str(lines_json).ok()?;
    let line_starts: Vec<usize> = serde_json::from_str(line_starts_json).ok()?;
    let next_results: Vec<Option<String>> = serde_json::from_str(next_results_json).ok()?;
    let plan: CalcRefreshPlan = calc_plan::compute_calc_refresh(
        &markers,
        &lines,
        &line_starts,
        &next_results,
        selection_from,
        selection_to,
    );
    serde_json::to_string(&plan).ok()
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
