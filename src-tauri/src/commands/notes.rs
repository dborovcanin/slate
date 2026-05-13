use app_core::storage::{Collection, Note, NoteModules, NoteSearchResult, NoteSummary};
use app_core::AppCore;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use serde::Serialize;
use std::path::Path;
use tauri::{AppHandle, Emitter, Manager, State};

const NOTE_CHANGED_EVENT: &str = "slate://note-changed";
const DEFAULT_CONTENT_SEARCH_LIMIT: usize = 60;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedNoteImage {
    pub image_id: String,
    pub markdown_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedNoteImagePath {
    pub source: String,
    pub path: Option<String>,
}

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
    let startup_markdown = app.state::<crate::StartupFileState>().take_startup_file();
    if let Some(path) = startup_markdown {
        let note_id = crate::note_id_for_file(&path);
        return core
            .note_sources()
            .open_note_by_id(&note_id)?
            .ok_or_else(|| "Failed to load file note".to_string());
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
pub fn create_note_with_context(
    core: State<'_, AppCore>,
    app: AppHandle,
    working_collection_id: Option<String>,
) -> Result<Note, String> {
    let id = ulid::Ulid::new().to_string();
    let (modules, default_password) = note_defaults_from_config()?;
    let note = core.db().create_note_with_context(
        &id,
        modules,
        default_password.as_deref(),
        working_collection_id.as_deref(),
    )?;
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
pub fn list_notes_meta_filtered(
    core: State<'_, AppCore>,
    active_id: Option<String>,
    collection_id: Option<String>,
) -> Result<Vec<NoteSummary>, String> {
    core.note_sources()
        .list_notes_meta_filtered(active_id.as_deref(), collection_id.as_deref())
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
pub async fn search_notes_content_filtered(
    core: State<'_, AppCore>,
    query: String,
    limit: Option<usize>,
    collection_id: Option<String>,
) -> Result<Vec<NoteSearchResult>, String> {
    let db = core.db().clone();
    let limit = limit.unwrap_or(DEFAULT_CONTENT_SEARCH_LIMIT);
    tauri::async_runtime::spawn_blocking(move || {
        db.search_notes_content_filtered(&query, limit, collection_id.as_deref())
    })
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
pub fn list_collections(core: State<'_, AppCore>) -> Result<Vec<Collection>, String> {
    core.db().list_collections()
}

#[tauri::command]
pub fn create_collection(
    core: State<'_, AppCore>,
    name: String,
    description: Option<String>,
) -> Result<Collection, String> {
    core.db()
        .create_collection(&name, description.as_deref().unwrap_or(""))
}

#[tauri::command]
pub fn rename_collection(
    core: State<'_, AppCore>,
    id: String,
    name: String,
) -> Result<Collection, String> {
    core.db().rename_collection(&id, &name)
}

#[tauri::command]
pub fn update_collection_description(
    core: State<'_, AppCore>,
    id: String,
    description: String,
) -> Result<Collection, String> {
    core.db().update_collection_description(&id, &description)
}

#[tauri::command]
pub fn delete_collection(core: State<'_, AppCore>, id: String) -> Result<bool, String> {
    core.db().delete_collection(&id)
}

#[tauri::command]
pub fn purge_collection(core: State<'_, AppCore>, id: String) -> Result<usize, String> {
    core.db().purge_collection(&id)
}

#[tauri::command]
pub fn list_collection_default_tags(
    core: State<'_, AppCore>,
    collection_id: String,
) -> Result<Vec<String>, String> {
    core.db().list_collection_default_tags(&collection_id)
}

#[tauri::command]
pub fn set_collection_default_tags(
    core: State<'_, AppCore>,
    collection_id: String,
    tag_names: Vec<String>,
) -> Result<Vec<String>, String> {
    core.db()
        .set_collection_default_tags(&collection_id, &tag_names)
}

#[tauri::command]
pub fn list_note_tags(core: State<'_, AppCore>, note_id: String) -> Result<Vec<String>, String> {
    let capabilities = core.note_sources().capabilities_for_note_id(&note_id);
    if !capabilities.can_module_persist {
        return Err("note tags are not supported for file-backed notes".to_string());
    }
    core.db().list_note_tags(&note_id)
}

#[tauri::command]
pub fn set_note_tags(
    core: State<'_, AppCore>,
    note_id: String,
    tag_names: Vec<String>,
) -> Result<Vec<String>, String> {
    let capabilities = core.note_sources().capabilities_for_note_id(&note_id);
    if !capabilities.can_module_persist {
        return Err("note tags are not supported for file-backed notes".to_string());
    }
    core.db().set_note_tags(&note_id, &tag_names)
}

#[tauri::command]
pub fn get_note_collection_ids(
    core: State<'_, AppCore>,
    note_id: String,
) -> Result<Vec<String>, String> {
    let capabilities = core.note_sources().capabilities_for_note_id(&note_id);
    if !capabilities.can_module_persist {
        return Err("collections are not supported for file-backed notes".to_string());
    }
    core.db().get_note_collection_ids(&note_id)
}

#[tauri::command]
pub fn set_note_collections(
    core: State<'_, AppCore>,
    note_id: String,
    collection_ids: Vec<String>,
) -> Result<Vec<String>, String> {
    let capabilities = core.note_sources().capabilities_for_note_id(&note_id);
    if !capabilities.can_module_persist {
        return Err("collections are not supported for file-backed notes".to_string());
    }
    core.db().set_note_collections(&note_id, &collection_ids)
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
pub fn resolve_wiki_link_headings(
    core: State<'_, AppCore>,
    short_id: String,
) -> Result<Vec<String>, String> {
    let Some(note) = core.note_sources().resolve_wiki_link_note(&short_id)? else {
        return Ok(Vec::new());
    };
    Ok(crate::editor_core::markdown_tokens::extract_markdown_headings(&note.body))
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

#[tauri::command]
pub fn import_note_image(
    core: State<'_, AppCore>,
    note_id: String,
    file_name: Option<String>,
    mime_type: Option<String>,
    bytes_base64: String,
) -> Result<ImportedNoteImage, String> {
    let image_bytes = decode_image_bytes(&bytes_base64)?;
    let imported = core.note_sources().import_image_bytes_by_id(
        &note_id,
        file_name.as_deref(),
        mime_type.as_deref(),
        &image_bytes,
    )?;
    Ok(ImportedNoteImage {
        image_id: imported.image_id,
        markdown_path: imported.markdown_path,
    })
}

#[tauri::command]
pub fn import_note_image_from_path(
    core: State<'_, AppCore>,
    note_id: String,
    file_path: String,
) -> Result<ImportedNoteImage, String> {
    let imported = core
        .note_sources()
        .import_image_path_by_id(&note_id, Path::new(&file_path))?;
    Ok(ImportedNoteImage {
        image_id: imported.image_id,
        markdown_path: imported.markdown_path,
    })
}

#[tauri::command]
pub fn import_note_image_from_clipboard(
    core: State<'_, AppCore>,
    note_id: String,
) -> Result<Option<ImportedNoteImage>, String> {
    if let Some(path) = crate::commands::clipboard::read_clipboard_image_file_path() {
        let imported = core
            .note_sources()
            .import_image_path_by_id(&note_id, &path)?;
        return Ok(Some(ImportedNoteImage {
            image_id: imported.image_id,
            markdown_path: imported.markdown_path,
        }));
    }

    if let Some((image_bytes, mime_type)) = crate::commands::clipboard::read_clipboard_image_bytes()
    {
        let file_name = clipboard_image_file_name_for_mime(mime_type);
        let imported = core.note_sources().import_image_bytes_by_id(
            &note_id,
            Some(file_name),
            Some(mime_type),
            &image_bytes,
        )?;
        return Ok(Some(ImportedNoteImage {
            image_id: imported.image_id,
            markdown_path: imported.markdown_path,
        }));
    }

    Ok(None)
}

#[tauri::command]
pub fn resolve_note_image_paths(
    core: State<'_, AppCore>,
    note_id: String,
    sources: Vec<String>,
) -> Result<Vec<ResolvedNoteImagePath>, String> {
    let mut out = Vec::with_capacity(sources.len());
    for source in sources {
        let resolved = core
            .note_sources()
            .resolve_image_markdown_source_by_id(&note_id, &source)?;
        out.push(ResolvedNoteImagePath {
            source,
            path: resolved,
        });
    }
    Ok(out)
}

#[tauri::command]
pub fn reserve_note_image(
    core: State<'_, AppCore>,
    note_id: String,
    file_name: Option<String>,
    mime_type: Option<String>,
) -> Result<ImportedNoteImage, String> {
    let reserved = core.note_sources().reserve_image_placeholder_by_id(
        &note_id,
        file_name.as_deref(),
        mime_type.as_deref(),
    )?;
    Ok(ImportedNoteImage {
        image_id: reserved.image_id,
        markdown_path: reserved.markdown_path,
    })
}

#[tauri::command]
pub fn write_note_image(
    core: State<'_, AppCore>,
    note_id: String,
    image_id: String,
    file_name: Option<String>,
    mime_type: Option<String>,
    bytes_base64: String,
) -> Result<(), String> {
    let image_bytes = decode_image_bytes(&bytes_base64)?;
    core.note_sources().write_image_bytes_to_placeholder_by_id(
        &note_id,
        &image_id,
        file_name.as_deref(),
        mime_type.as_deref(),
        &image_bytes,
    )
}

#[tauri::command]
pub fn write_note_image_from_path(
    core: State<'_, AppCore>,
    note_id: String,
    image_id: String,
    file_path: String,
) -> Result<(), String> {
    core.note_sources().write_image_path_to_placeholder_by_id(
        &note_id,
        &image_id,
        Path::new(&file_path),
    )
}

#[tauri::command]
pub fn delete_note_image(
    core: State<'_, AppCore>,
    note_id: String,
    image_id: String,
) -> Result<bool, String> {
    core.note_sources()
        .delete_image_placeholder_by_id(&note_id, &image_id)
}

pub(crate) fn decode_image_bytes(encoded: &str) -> Result<Vec<u8>, String> {
    let payload = encoded
        .trim()
        .split_once(',')
        .map(|(_, data)| data)
        .unwrap_or(encoded)
        .trim();
    if payload.is_empty() {
        return Err("Image payload is empty".to_string());
    }
    let decoded = BASE64_STANDARD
        .decode(payload)
        .map_err(|e| format!("Invalid base64 image payload: {e}"))?;
    if decoded.is_empty() {
        return Err("Decoded image payload is empty".to_string());
    }
    Ok(decoded)
}

fn clipboard_image_file_name_for_mime(mime_type: &str) -> &'static str {
    match mime_type.to_ascii_lowercase().as_str() {
        "image/jpeg" | "image/jpg" => "clipboard-image.jpg",
        "image/webp" => "clipboard-image.webp",
        "image/gif" => "clipboard-image.gif",
        "image/bmp" => "clipboard-image.bmp",
        "image/svg+xml" => "clipboard-image.svg",
        "image/avif" => "clipboard-image.avif",
        _ => "clipboard-image.png",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_image_bytes_accepts_data_url_prefix() {
        let decoded = decode_image_bytes("data:image/png;base64,aGVsbG8=").expect("decode");
        assert_eq!(decoded, b"hello");
    }
}
