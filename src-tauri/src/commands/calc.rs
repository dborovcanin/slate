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

/// Sync a range of lines into the server-side note cache.
/// `from..to` (exclusive) is replaced with `changed_lines`.
/// Pass `from=0, to=usize::MAX` (or any to > current len) to replace all.
#[tauri::command]
pub fn sync_note_lines(
    core: State<'_, AppCore>,
    note_id: String,
    from: usize,
    to: usize,
    changed_lines: Vec<String>,
) -> Result<(), String> {
    let mut cache = core
        .note_line_cache
        .lock()
        .map_err(|_| "line cache lock poisoned".to_string())?;
    let entry = cache.entry(note_id).or_insert_with(Vec::new);
    let clamped_to = to.min(entry.len());
    let clamped_from = from.min(clamped_to);
    entry.splice(clamped_from..clamped_to, changed_lines);
    Ok(())
}

/// Evaluate the cached lines for a note, sending only a partial range.
/// Requires a prior `sync_note_lines` call that seeded the cache.
#[tauri::command]
pub fn evaluate_note_context_delta(
    core: State<'_, AppCore>,
    note_id: String,
    variables_enabled: Option<bool>,
    eval_from: Option<usize>,
    eval_to: Option<usize>,
) -> Result<NoteEvaluationResult, String> {
    // Snapshot lines under the mutex, then release it before evaluation so
    // expensive calc work doesn't block other cache updates.
    let lines = {
        let cache = core
            .note_line_cache
            .lock()
            .map_err(|_| "line cache lock poisoned".to_string())?;
        cache
            .get(&note_id)
            .cloned()
            .ok_or_else(|| format!("no cached lines for note '{note_id}'"))?
    };
    let line_count = lines.len();
    let eval_range = match (eval_from, eval_to) {
        (Some(from), Some(to)) if to >= from => Some((from.min(line_count), to.min(line_count))),
        _ => None,
    };
    let options = NoteEvaluationOptions {
        variables_enabled: variables_enabled.unwrap_or(true),
        eval_range,
    };
    Ok(core.calc_engine().evaluate_note_context(&lines, options))
}
