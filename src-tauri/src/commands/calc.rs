use app_core::calc::{NoteEvaluationOptions, NoteEvaluationResult};
use app_core::AppCore;
use tauri::State;

#[tauri::command]
pub fn evaluate_lines(core: State<'_, AppCore>, lines: Vec<String>) -> Vec<Option<String>> {
    core.calc_engine().evaluate_lines(&lines)
}

#[tauri::command]
pub fn evaluate_note_context(
    core: State<'_, AppCore>,
    lines: Vec<String>,
    variables_enabled: Option<bool>,
    eval_from: Option<usize>,
    eval_to: Option<usize>,
) -> NoteEvaluationResult {
    let eval_range = match (eval_from, eval_to) {
        (Some(from), Some(to)) if to >= from => Some((from, to)),
        _ => None,
    };
    let options = NoteEvaluationOptions {
        variables_enabled: variables_enabled.unwrap_or(true),
        eval_range,
    };
    core.calc_engine().evaluate_note_context(&lines, options)
}
