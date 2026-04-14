use app_core::storage::Note;
use app_core::AppCore;
use tauri::State;

#[tauri::command]
pub fn get_or_create_note(core: State<'_, AppCore>) -> Result<Note, String> {
    if let Some(note) = core.db().get_most_recent_note()? {
        return Ok(note);
    }
    let id = ulid::Ulid::new().to_string();
    core.db().save_note(&id, "")
}

#[tauri::command]
pub fn save_note(core: State<'_, AppCore>, id: String, body: String) -> Result<Note, String> {
    core.db().save_note(&id, &body)
}

#[tauri::command]
pub fn create_note(core: State<'_, AppCore>) -> Result<Note, String> {
    let id = ulid::Ulid::new().to_string();
    core.db().save_note(&id, "")
}

#[tauri::command]
pub fn list_notes(core: State<'_, AppCore>) -> Result<Vec<Note>, String> {
    core.db().list_notes()
}

#[tauri::command]
pub fn delete_note(core: State<'_, AppCore>, id: String) -> Result<bool, String> {
    core.db().delete_note(&id)
}
