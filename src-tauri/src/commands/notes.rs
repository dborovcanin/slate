use app_core::storage::{Note, NoteModules, NoteSummary};
use app_core::AppCore;
use tauri::State;

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
pub fn get_or_create_note(core: State<'_, AppCore>) -> Result<Note, String> {
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
    core.db()
        .create_note_with_defaults(&id, modules, default_password.as_deref())
}

#[tauri::command]
pub fn save_note(core: State<'_, AppCore>, id: String, body: String) -> Result<Note, String> {
    core.db().save_note(&id, &body)
}

#[tauri::command]
pub fn get_note(core: State<'_, AppCore>, id: String) -> Result<Option<Note>, String> {
    core.db().get_note(&id)
}

#[tauri::command]
pub fn create_note(core: State<'_, AppCore>) -> Result<Note, String> {
    let id = ulid::Ulid::new().to_string();
    let (modules, default_password) = note_defaults_from_config()?;
    core.db()
        .create_note_with_defaults(&id, modules, default_password.as_deref())
}

#[tauri::command]
pub fn list_notes(core: State<'_, AppCore>) -> Result<Vec<Note>, String> {
    core.db().list_notes()
}

#[tauri::command]
pub fn list_notes_meta(core: State<'_, AppCore>) -> Result<Vec<NoteSummary>, String> {
    core.db().list_notes_meta()
}

#[tauri::command]
pub fn get_note_meta(core: State<'_, AppCore>, id: String) -> Result<Option<NoteSummary>, String> {
    core.db().get_note_meta(&id)
}

#[tauri::command]
pub fn delete_note(
    core: State<'_, AppCore>,
    id: String,
    password: Option<String>,
) -> Result<bool, String> {
    core.db().delete_note(&id, password.as_deref())
}

#[tauri::command]
pub fn set_note_modules(
    core: State<'_, AppCore>,
    id: String,
    modules: NoteModules,
) -> Result<Note, String> {
    core.db().set_note_modules(&id, modules)
}

#[tauri::command]
pub fn lock_note_access(
    core: State<'_, AppCore>,
    id: String,
    password: String,
) -> Result<Note, String> {
    core.db().lock_note(&id, &password)
}

#[tauri::command]
pub fn unlock_note_access(
    core: State<'_, AppCore>,
    id: String,
    password: String,
) -> Result<Note, String> {
    core.db().unlock_note(&id, &password)
}

#[tauri::command]
pub fn encrypt_note(
    core: State<'_, AppCore>,
    id: String,
    password: String,
) -> Result<Note, String> {
    core.db().encrypt_note(&id, &password)
}

#[tauri::command]
pub fn decrypt_note(
    core: State<'_, AppCore>,
    id: String,
    password: String,
) -> Result<Note, String> {
    core.db().decrypt_note(&id, &password)
}
