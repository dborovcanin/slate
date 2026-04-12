use tauri::State;

use crate::calc::engine::CalcEngine;

#[tauri::command]
pub fn evaluate_lines(
    engine: State<'_, CalcEngine>,
    lines: Vec<String>,
) -> Vec<Option<String>> {
    engine.evaluate_lines(&lines)
}
