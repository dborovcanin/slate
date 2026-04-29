use app_core::calc::{
    start_eval_generation, CalcEngine, NoteEvaluationOptions, NoteEvaluationResult,
};
use app_core::AppCore;
use std::sync::Arc;
use tauri::State;

fn resolve_eval_range(
    line_count: usize,
    eval_from: Option<usize>,
    eval_to: Option<usize>,
) -> Option<(usize, usize)> {
    match (eval_from, eval_to) {
        (Some(from), Some(to)) if to >= from => Some((from.min(line_count), to.min(line_count))),
        _ => None,
    }
}

#[tauri::command]
pub async fn evaluate_lines(lines: Vec<String>) -> Result<Vec<Option<String>>, String> {
    let generation = start_eval_generation();
    let fallback_lines = lines.clone();
    let result = match tauri::async_runtime::spawn_blocking(move || {
        CalcEngine::new().evaluate_lines_with_generation(&lines, generation)
    })
    .await
    {
        Ok(result) => result,
        Err(_) => CalcEngine::new().evaluate_lines_with_generation(&fallback_lines, generation),
    };
    Ok(result)
}

#[tauri::command]
pub async fn evaluate_note_context(
    lines: Vec<String>,
    variables_enabled: Option<bool>,
    table_enabled: Option<bool>,
    eval_from: Option<usize>,
    eval_to: Option<usize>,
) -> Result<NoteEvaluationResult, String> {
    let eval_range = resolve_eval_range(lines.len(), eval_from, eval_to);
    let options = NoteEvaluationOptions {
        variables_enabled: variables_enabled.unwrap_or(true),
        table_enabled: table_enabled.unwrap_or(true),
        eval_range,
    };
    let generation = start_eval_generation();
    let fallback_lines = lines.clone();
    let fallback_options = options;
    let result = match tauri::async_runtime::spawn_blocking(move || {
        CalcEngine::new().evaluate_note_context_with_generation(&lines, options, generation)
    })
    .await
    {
        Ok(result) => result,
        Err(_) => CalcEngine::new().evaluate_note_context_with_generation(
            &fallback_lines,
            fallback_options,
            generation,
        ),
    };
    Ok(result)
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
    let entry = cache
        .entry(note_id)
        .or_insert_with(|| Arc::new(Vec::new()));
    let vec = Arc::make_mut(entry);
    let clamped_to = to.min(vec.len());
    let clamped_from = from.min(clamped_to);
    vec.splice(clamped_from..clamped_to, changed_lines);
    Ok(())
}

/// Evaluate the cached lines for a note, sending only a partial range.
/// Requires a prior `sync_note_lines` call that seeded the cache.
#[tauri::command]
pub async fn evaluate_note_context_delta(
    core: State<'_, AppCore>,
    note_id: String,
    variables_enabled: Option<bool>,
    table_enabled: Option<bool>,
    eval_from: Option<usize>,
    eval_to: Option<usize>,
) -> Result<NoteEvaluationResult, String> {
    // Snapshot the Arc under the mutex, then release it before evaluation so
    // expensive calc work doesn't block other cache updates. The Arc clone is
    // O(1) — it does not copy the line data.
    let lines: Arc<Vec<String>> = {
        let cache = core
            .note_line_cache
            .lock()
            .map_err(|_| "line cache lock poisoned".to_string())?;
        cache
            .get(&note_id)
            .cloned()
            .ok_or_else(|| format!("no cached lines for note '{note_id}'"))?
    };
    let eval_range = resolve_eval_range(lines.len(), eval_from, eval_to);
    let options = NoteEvaluationOptions {
        variables_enabled: variables_enabled.unwrap_or(true),
        table_enabled: table_enabled.unwrap_or(true),
        eval_range,
    };
    let generation = start_eval_generation();
    let fallback_lines = Arc::clone(&lines);
    let fallback_options = options;
    match tauri::async_runtime::spawn_blocking(move || {
        CalcEngine::new().evaluate_note_context_with_generation(&lines, options, generation)
    })
    .await
    {
        Ok(result) => Ok(result),
        Err(_) => Ok(CalcEngine::new().evaluate_note_context_with_generation(
            &fallback_lines,
            fallback_options,
            generation,
        )),
    }
}
