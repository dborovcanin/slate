use wasm_bindgen::prelude::*;

use crate::format::format_markdown;
use crate::text_rules::{
    run_doc_change_rules, run_enter_rules, run_tab_rules, run_table_cell_navigation_rules,
    rewrite_line_with_checklist_toggle_suffix, TabRuleOptions, TextRuleOptions,
};
use crate::types::EditorContextSnapshot;

#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

/// Run document-change text rules. Returns JSON-encoded EditOperation or null.
#[wasm_bindgen]
pub fn wasm_run_doc_change_rules(snapshot_json: &str, markdown_autoformat: bool) -> Option<String> {
    let snapshot: EditorContextSnapshot = serde_json::from_str(snapshot_json).ok()?;
    let options = TextRuleOptions { markdown_autoformat };
    let op = run_doc_change_rules(&snapshot, options)?;
    serde_json::to_string(&op).ok()
}

/// Run enter-key text rules. Returns JSON-encoded EditOperation or null.
#[wasm_bindgen]
pub fn wasm_run_enter_rules(snapshot_json: &str, markdown_autoformat: bool) -> Option<String> {
    let snapshot: EditorContextSnapshot = serde_json::from_str(snapshot_json).ok()?;
    let options = TextRuleOptions { markdown_autoformat };
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
    let options = TabRuleOptions { markdown_autoformat, outdent };
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
    let options = TabRuleOptions { markdown_autoformat, outdent };
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
