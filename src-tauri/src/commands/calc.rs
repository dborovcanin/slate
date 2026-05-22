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
    // Snapshot the line cache Arc (O(1) clone, no data copy).
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
    let vars_enabled = variables_enabled.unwrap_or(true);
    let table_enabled_val = table_enabled.unwrap_or(true);
    // cross_note_enabled is not yet forwarded from the UI; default to true.
    let cross_note_enabled = true;
    let eval_range = resolve_eval_range(lines.len(), eval_from, eval_to);
    let generation = start_eval_generation();

    // Clone the two shared handles needed inside spawn_blocking.
    // note_sources owns the DB connection (blocking I/O); var_index is Arc<Mutex<...>>.
    // Neither can be borrowed across the await point via State<'_>.
    let note_sources = core.note_sources().clone();
    let var_index = core.cross_note_var_index_arc();
    let var_index_post = Arc::clone(&var_index);

    let fallback_lines = Arc::clone(&lines);
    // Keep copies for the post-eval index update after the closure moves them.
    let note_id_post = note_id.clone();
    let short_id_post = short_id.clone();

    // All blocking work — dep pre-loading (DB I/O + CalcEngine) and the main
    // evaluation — runs together in one spawn_blocking so the async executor
    // is never stalled.
    let result = tauri::async_runtime::spawn_blocking(move || {
        // Quick check: if no line contains "[[", there are no cross-note refs at all.
        // Avoids the full regex scan for the common case.
        let has_cross_note_syntax =
            cross_note_enabled && !short_id.is_empty() && lines.iter().any(|l| l.contains("[["));

        let (extern_vars, refs) = if has_cross_note_syntax {
            let refs = app_core::calc::scan_cross_note_refs(&lines);

            for r in &refs {
                let claimed = var_index
                    .lock()
                    .ok()
                    .map(|mut idx| idx.try_claim_eval(&r.note_short_id))
                    .unwrap_or(false);
                if claimed {
                    if let Ok(Some(dep)) = note_sources.resolve_wiki_link_note(&r.note_short_id) {
                        let dep_lines: Vec<String> =
                            dep.body.split('\n').map(|l| l.to_string()).collect();
                        // Pre-scan dep lines so the engine skips its own internal scan.
                        let dep_refs = app_core::calc::scan_cross_note_refs(&dep_lines);
                        let dep_options = NoteEvaluationOptions {
                            variables_enabled: true,
                            table_enabled: false,
                            precomputed_refs: Some(dep_refs),
                            ..Default::default()
                        };
                        let dep_result =
                            CalcEngine::new().evaluate_note_context(&dep_lines, dep_options);
                        if let Ok(mut idx) = var_index.lock() {
                            idx.register_note(&dep.id, &r.note_short_id);
                            idx.update_exports(
                                &r.note_short_id,
                                &dep_result.variables,
                                &dep_result.variable_values,
                            );
                            idx.mark_name_scan_attempted(&r.note_short_id);
                            idx.mark_full_eval_attempted(&r.note_short_id);
                            idx.update_deps(&dep.id, &dep_result.cross_note_refs);
                        }
                    } else if let Ok(mut idx) = var_index.lock() {
                        idx.mark_name_scan_attempted(&r.note_short_id);
                        idx.mark_full_eval_attempted(&r.note_short_id);
                    }
                }
            }

            let extern_vars = if let Ok(mut idx) = var_index.lock() {
                idx.register_note(&note_id, &short_id);
                idx.update_deps(&note_id, &refs);
                idx.extern_vars_for(&note_id)
            } else {
                Vec::new()
            };
            (extern_vars, refs)
        } else {
            (Vec::new(), Vec::new())
        };

        let options = NoteEvaluationOptions {
            variables_enabled: vars_enabled,
            cross_note_enabled,
            table_enabled: table_enabled_val,
            eval_range,
            extern_vars,
            // Pass the pre-scanned refs so the engine doesn't rescan the same lines.
            precomputed_refs: if has_cross_note_syntax { Some(refs) } else { None },
        };
        CalcEngine::new().evaluate_note_context_with_generation(&lines, options, generation)
    })
    .await
    .unwrap_or_else(|_| {
        // spawn_blocking panicked — fall back to a plain eval with no extern vars.
        let options = NoteEvaluationOptions {
            variables_enabled: vars_enabled,
            cross_note_enabled,
            table_enabled: table_enabled_val,
            eval_range,
            extern_vars: Vec::new(),
            ..Default::default()
        };
        CalcEngine::new().evaluate_note_context_with_generation(&fallback_lines, options, generation)
    });

    // Update the cross-note index with this note's latest exports and deps.
    if cross_note_enabled && !short_id_post.is_empty() {
        if let Ok(mut idx) = var_index_post.lock() {
            idx.update_exports(&short_id_post, &result.variables, &result.variable_values);
            idx.update_deps(&note_id_post, &result.cross_note_refs);
        }
    }

    Ok(result)
}

/// Return exported variable entries for a note identified by its 8-char short ID.
/// Used to populate cross-note variable autocomplete suggestions in the editor.
/// Fully evaluates the note so that when the user selects a suggestion and the
/// editor re-evaluates, the dep note's values are already in the index (no extra
/// DB load on Enter).
#[tauri::command]
pub async fn get_cross_note_vars(
    core: State<'_, AppCore>,
    short_id: String,
) -> Result<Vec<VariableIndexEntry>, String> {
    // Fast path: already fully evaluated this session.
    let already = core
        .cross_note_var_index
        .lock()
        .ok()
        .map(|index| index.was_full_eval_attempted(&short_id))
        .unwrap_or(false);
    if already {
        return Ok(core.cross_note_exports_for_autocomplete(&short_id));
    }

    // Slow path: run a full CalcEngine eval so that variable *values* are indexed
    // before the user presses Enter (no extra DB load at that point). This runs
    // inside spawn_blocking so the UI thread is not stalled. Large dep notes pay
    // the eval cost once per session; subsequent calls hit the fast path above.
    let note_sources = core.note_sources().clone();
    let var_index = core.cross_note_var_index_arc();
    let entries = tauri::async_runtime::spawn_blocking(move || {
        let note = match note_sources.resolve_wiki_link_note(&short_id) {
            Ok(Some(n)) => n,
            _ => {
                if let Ok(mut index) = var_index.lock() {
                    index.mark_name_scan_attempted(&short_id);
                    index.mark_full_eval_attempted(&short_id);
                }
                return Vec::new();
            }
        };
        let lines: Vec<String> = note.body.split('\n').map(|l| l.to_string()).collect();
        let result = CalcEngine::new().evaluate_note_context(
            &lines,
            NoteEvaluationOptions {
                variables_enabled: true,
                table_enabled: false,
                ..Default::default()
            },
        );
        if let Ok(mut index) = var_index.lock() {
            index.register_note(&note.id, &short_id);
            index.update_exports(&short_id, &result.variables, &result.variable_values);
            index.update_deps(&note.id, &result.cross_note_refs);
            index.mark_name_scan_attempted(&short_id);
            index.mark_full_eval_attempted(&short_id);
        }
        result.variables
    })
    .await
    .unwrap_or_default();
    Ok(entries)
}
