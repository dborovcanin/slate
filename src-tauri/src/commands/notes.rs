use tauri::State;

use crate::storage::{Db, Note};

#[tauri::command]
pub fn get_or_create_note(db: State<'_, Db>) -> Result<Note, String> {
    if let Some(note) = db.get_most_recent_note()? {
        return Ok(note);
    }
    let id = ulid::Ulid::new().to_string();
    db.save_note(&id, "")
}

#[tauri::command]
pub fn save_note(db: State<'_, Db>, id: String, body: String) -> Result<Note, String> {
    db.save_note(&id, &body)
}

#[tauri::command]
pub fn create_note(db: State<'_, Db>) -> Result<Note, String> {
    let id = ulid::Ulid::new().to_string();
    db.save_note(&id, "")
}

#[tauri::command]
pub fn list_notes(db: State<'_, Db>) -> Result<Vec<Note>, String> {
    db.list_notes()
}

#[tauri::command]
pub fn delete_note(db: State<'_, Db>, id: String) -> Result<bool, String> {
    db.delete_note(&id)
}
