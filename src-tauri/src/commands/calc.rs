use tauri::State;

use crate::calc::engine::{CalcEngine, NoteEvaluationOptions, NoteEvaluationResult};

#[tauri::command]
pub fn evaluate_lines(engine: State<'_, CalcEngine>, lines: Vec<String>) -> Vec<Option<String>> {
    engine.evaluate_lines(&lines)
}

#[tauri::command]
pub fn evaluate_note_context(
    engine: State<'_, CalcEngine>,
    lines: Vec<String>,
    variables_enabled: Option<bool>,
) -> NoteEvaluationResult {
    let options = NoteEvaluationOptions {
        variables_enabled: variables_enabled.unwrap_or(true),
    };
    engine.evaluate_note_context(&lines, options)
}
