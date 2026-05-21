use app_core::calc::{
    start_eval_generation, CalcEngine, NoteEvaluationOptions, NoteEvaluationResult,
    VariableIndexEntry,
};
use app_core::AppCore;
use std::sync::Arc;
use tauri::State;

/// Returns the 8-char wiki-link short ID for a DB note, or `""` for file notes.
fn note_short_id(note_id: &str) -> &str {
    if note_id.starts_with("mdfile:") || note_id.len() < 8 {
        return "";
    }
    &note_id[..8]
}

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
        ..Default::default()
    };
    let generation = start_eval_generation();
    let fallback_lines = lines.clone();
    let fallback_options = options.clone();
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
    let entry = cache.entry(note_id).or_insert_with(|| Arc::new(Vec::new()));
    let vec = Arc::make_mut(entry);
    let clamped_to = to.min(vec.len());
    let clamped_from = from.min(clamped_to);
    vec.splice(clamped_from..clamped_to, changed_lines);
    Ok(())
}

/// Evaluate the cached lines for a note, sending only a partial range.
/// Requires a prior `sync_note_lines` call that seeded the cache.
/// Automatically injects cross-note variable values from the shared index and
/// updates the index with this note's latest exports after evaluation.
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

    let short_id = note_short_id(&note_id).to_string();

    // Snapshot extern vars from the cross-note index before entering spawn_blocking.
    let vars_enabled = variables_enabled.unwrap_or(true);
    let extern_vars = if vars_enabled && !short_id.is_empty() {
        // Pre-load any referenced notes not yet in the index so ghost eval
        // works without requiring the user to visit the source note first.
        let refs = app_core::calc::scan_cross_note_refs(&lines);
        for r in &refs {
            let already = core
                .cross_note_var_index
                .lock()
                .ok()
                .map(|index| index.was_full_eval_attempted(&r.note_short_id))
                .unwrap_or(true);
            if !already {
                if let Ok(Some(dep)) = core
                    .note_sources()
                    .resolve_wiki_link_note(&r.note_short_id)
                {
                    let dep_lines: Vec<String> =
                        dep.body.split('\n').map(|l| l.to_string()).collect();
                    let dep_options = NoteEvaluationOptions {
                        variables_enabled: true,
                        table_enabled: false,
                        ..Default::default()
                    };
                    core.evaluate_note_with_cross_refs(
                        &dep.id,
                        &r.note_short_id,
                        &dep_lines,
                        dep_options,
                    );
                }
                if let Ok(mut index) = core.cross_note_var_index.lock() {
                    index.mark_name_scan_attempted(&r.note_short_id);
                    index.mark_full_eval_attempted(&r.note_short_id);
                }
            }
        }

        if let Ok(mut index) = core.cross_note_var_index.lock() {
            index.register_note(&note_id, &short_id);
            index.extern_vars_for(&note_id)
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    let eval_range = resolve_eval_range(lines.len(), eval_from, eval_to);
    let options = NoteEvaluationOptions {
        variables_enabled: vars_enabled,
        table_enabled: table_enabled.unwrap_or(true),
        eval_range,
        extern_vars,
    };
    let generation = start_eval_generation();
    let fallback_lines = Arc::clone(&lines);
    let fallback_options = options.clone();
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

    // Update the cross-note index with this note's latest exports and deps.
    if vars_enabled && !short_id.is_empty() {
        if let Ok(mut index) = core.cross_note_var_index.lock() {
            index.update_exports(&short_id, &result.variables, &result.variable_values);
            index.update_deps(&note_id, &result.cross_note_refs);
        }
    }

    Ok(result)
}

/// Return exported variable entries for a note identified by its 8-char short ID.
/// Used to populate cross-note variable autocomplete suggestions in the editor.
/// If the note has never been evaluated this session, loads and evaluates it on demand.
#[tauri::command]
pub fn get_cross_note_vars(
    core: State<'_, AppCore>,
    short_id: String,
) -> Vec<VariableIndexEntry> {
    // Fast path: name scan already done (covers notes with zero variables too).
    let already = core
        .cross_note_var_index
        .lock()
        .ok()
        .map(|index| index.was_name_scan_attempted(&short_id))
        .unwrap_or(false);
    if already {
        return core.cross_note_exports_for_autocomplete(&short_id);
    }

    // Slow path: load note body and do a fast name-only scan (no CalcEngine eval).
    let note = match core.note_sources().resolve_wiki_link_note(&short_id) {
        Ok(Some(n)) => n,
        _ => {
            if let Ok(mut index) = core.cross_note_var_index.lock() {
                index.mark_name_scan_attempted(&short_id);
            }
            return Vec::new();
        }
    };
    let lines: Vec<String> = note.body.split('\n').map(|l| l.to_string()).collect();
    let entries = app_core::calc::scan_variable_assignments(&lines);
    if let Ok(mut index) = core.cross_note_var_index.lock() {
        index.register_note(&note.id, &short_id);
        index.update_entries_only(&short_id, &entries);
        index.mark_name_scan_attempted(&short_id);
    }
    entries
}
