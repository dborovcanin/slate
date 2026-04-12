use std::fs;

#[tauri::command]
pub fn export_to_file(path: String, content: String) -> Result<(), String> {
    fs::write(&path, &content).map_err(|e| format!("Failed to write file: {e}"))
}
