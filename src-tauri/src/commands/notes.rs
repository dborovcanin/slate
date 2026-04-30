use app_core::storage::{Note, NoteModules, NoteSearchResult, NoteSummary};
use app_core::AppCore;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

const NOTE_CHANGED_EVENT: &str = "slate://note-changed";
const DEFAULT_CONTENT_SEARCH_LIMIT: usize = 60;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NoteChangedEvent {
    id: String,
    updated_at: Option<String>,
    deleted: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedWikiLink {
    pub short_id: String,
    pub summary: Option<NoteSummary>,
}

fn emit_note_changed(app: &AppHandle, id: &str, updated_at: Option<String>, deleted: bool) {
    let payload = NoteChangedEvent {
        id: id.to_string(),
        updated_at,
        deleted,
    };
    if let Err(error) = app.emit(NOTE_CHANGED_EVENT, payload) {
        eprintln!("failed to emit note change event: {error}");
    }
}

fn note_defaults_from_config() -> Result<(NoteModules, Option<String>), String> {
    let cfg = app_core::config::load_theme_config();
    let security = app_core::config::note_security_config_from_theme(&cfg);
    let default_password = app_core::config::resolve_default_note_encryption_password(&security)?;
    let modules = NoteModules {
        math: cfg.default_modules.math,
        table: cfg.default_modules.table,
        variables: cfg.default_modules.variables,
        style: cfg.default_modules.style,
    };
    Ok((modules, default_password))
}

#[tauri::command]
pub fn get_or_create_note(core: State<'_, AppCore>, app: AppHandle) -> Result<Note, String> {
    let startup_markdown = app
        .state::<crate::StartupMarkdownFileState>()
        .take_startup_file();
    if let Some(path) = startup_markdown {
        let note_id = crate::note_id_for_markdown_file(&path);
        return core
            .note_sources()
            .open_note_by_id(&note_id)?
            .ok_or_else(|| "Failed to load markdown file note".to_string());
    }

    let special = app_core::config::load_special_notes_config();
    if let Some(note) = core
        .db()
        .get_most_recent_note_excluding_prefix(&special.email_note_prefix)?
    {
        return Ok(note);
    }

    if let Some(note) = core.db().get_most_recent_note()? {
        return Ok(note);
    }
    let id = ulid::Ulid::new().to_string();
    let (modules, default_password) = note_defaults_from_config()?;
    let note = core
        .db()
        .create_note_with_defaults(&id, modules, default_password.as_deref())?;
    emit_note_changed(&app, note.id.as_str(), Some(note.updated_at.clone()), false);
    Ok(note)
}

#[tauri::command]
pub fn save_note(
    core: State<'_, AppCore>,
    app: AppHandle,
    id: String,
    body: String,
    expected_revision: Option<String>,
    force: Option<bool>,
) -> Result<Note, String> {
    let note = core.note_sources().save_note_by_id(
        &id,
        &body,
        app_core::note_sources::SaveOptions {
            expected_revision,
            force: force.unwrap_or(false),
        },
    )?;
    emit_note_changed(&app, note.id.as_str(), Some(note.updated_at.clone()), false);
    Ok(note)
}

#[tauri::command]
pub fn get_note(core: State<'_, AppCore>, id: String) -> Result<Option<Note>, String> {
    core.note_sources().open_note_by_id(&id)
}

#[tauri::command]
pub fn create_note(core: State<'_, AppCore>, app: AppHandle) -> Result<Note, String> {
    let id = ulid::Ulid::new().to_string();
    let (modules, default_password) = note_defaults_from_config()?;
    let note = core
        .db()
        .create_note_with_defaults(&id, modules, default_password.as_deref())?;
    emit_note_changed(&app, note.id.as_str(), Some(note.updated_at.clone()), false);
    Ok(note)
}

#[tauri::command]
pub fn list_notes_meta(
    core: State<'_, AppCore>,
    active_id: Option<String>,
) -> Result<Vec<NoteSummary>, String> {
    core.note_sources().list_notes_meta(active_id.as_deref())
}

#[tauri::command]
pub async fn search_notes_content(
    core: State<'_, AppCore>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<NoteSearchResult>, String> {
    let db = core.db().clone();
    let limit = limit.unwrap_or(DEFAULT_CONTENT_SEARCH_LIMIT);
    tauri::async_runtime::spawn_blocking(move || db.search_notes_content(&query, limit))
        .await
        .map_err(|e| format!("content search worker failed: {e}"))?
}

#[tauri::command]
pub async fn rebuild_note_search_index(core: State<'_, AppCore>) -> Result<(), String> {
    let db = core.db().clone();
    tauri::async_runtime::spawn_blocking(move || db.rebuild_note_search_index())
        .await
        .map_err(|e| format!("index rebuild worker failed: {e}"))?
}

#[tauri::command]
pub fn get_note_meta(core: State<'_, AppCore>, id: String) -> Result<Option<NoteSummary>, String> {
    core.note_sources().get_note_meta_by_id(&id)
}

#[tauri::command]
pub fn get_note_revision(core: State<'_, AppCore>, id: String) -> Result<Option<String>, String> {
    core.note_sources().get_note_revision_by_id(&id)
}

#[tauri::command]
pub fn delete_note(
    core: State<'_, AppCore>,
    app: AppHandle,
    id: String,
    password: Option<String>,
) -> Result<bool, String> {
    let capabilities = core.note_sources().capabilities_for_note_id(&id);
    if !capabilities.can_delete {
        return Err("delete is not supported for file-backed notes".to_string());
    }
    let deleted = core.db().delete_note(&id, password.as_deref())?;
    if deleted {
        emit_note_changed(&app, &id, None, true);
    }
    Ok(deleted)
}

#[tauri::command]
pub fn set_note_modules(
    core: State<'_, AppCore>,
    app: AppHandle,
    id: String,
    modules: NoteModules,
) -> Result<Note, String> {
    let capabilities = core.note_sources().capabilities_for_note_id(&id);
    if !capabilities.can_module_persist {
        return Err("module updates are not supported for file-backed notes".to_string());
    }
    let note = core.db().set_note_modules(&id, modules)?;
    emit_note_changed(&app, note.id.as_str(), Some(note.updated_at.clone()), false);
    Ok(note)
}

#[tauri::command]
pub fn lock_note_access(
    core: State<'_, AppCore>,
    app: AppHandle,
    id: String,
    password: String,
) -> Result<Note, String> {
    let capabilities = core.note_sources().capabilities_for_note_id(&id);
    if !capabilities.can_lock {
        return Err("note locking is not supported for file-backed notes".to_string());
    }
    let note = core.db().lock_note(&id, &password)?;
    emit_note_changed(&app, note.id.as_str(), Some(note.updated_at.clone()), false);
    Ok(note)
}

#[tauri::command]
pub fn unlock_note_access(
    core: State<'_, AppCore>,
    app: AppHandle,
    id: String,
    password: String,
) -> Result<Note, String> {
    let capabilities = core.note_sources().capabilities_for_note_id(&id);
    if !capabilities.can_lock {
        return Err("note locking is not supported for file-backed notes".to_string());
    }
    let note = core.db().unlock_note(&id, &password)?;
    emit_note_changed(&app, note.id.as_str(), Some(note.updated_at.clone()), false);
    Ok(note)
}

#[tauri::command]
pub fn encrypt_note(
    core: State<'_, AppCore>,
    app: AppHandle,
    id: String,
    password: String,
) -> Result<Note, String> {
    let capabilities = core.note_sources().capabilities_for_note_id(&id);
    if !capabilities.can_encrypt {
        return Err("note encryption is not supported for file-backed notes".to_string());
    }
    let note = core.db().encrypt_note(&id, &password)?;
    emit_note_changed(&app, note.id.as_str(), Some(note.updated_at.clone()), false);
    Ok(note)
}

#[tauri::command]
pub fn resolve_wiki_link(
    core: State<'_, AppCore>,
    short_id: String,
) -> Result<Option<NoteSummary>, String> {
    core.note_sources().resolve_wiki_link(&short_id)
}

#[tauri::command]
pub fn resolve_wiki_links(
    core: State<'_, AppCore>,
    short_ids: Vec<String>,
) -> Result<Vec<ResolvedWikiLink>, String> {
    let resolved = core.note_sources().resolve_wiki_links(&short_ids)?;
    Ok(resolved
        .into_iter()
        .map(|(short_id, summary)| ResolvedWikiLink { short_id, summary })
        .collect())
}

#[tauri::command]
pub fn decrypt_note(
    core: State<'_, AppCore>,
    app: AppHandle,
    id: String,
    password: String,
) -> Result<Note, String> {
    let capabilities = core.note_sources().capabilities_for_note_id(&id);
    if !capabilities.can_encrypt {
        return Err("note encryption is not supported for file-backed notes".to_string());
    }
    let note = core.db().decrypt_note(&id, &password)?;
    emit_note_changed(&app, note.id.as_str(), Some(note.updated_at.clone()), false);
    Ok(note)
}
