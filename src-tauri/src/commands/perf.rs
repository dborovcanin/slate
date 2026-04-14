#[tauri::command]
pub fn append_startup_log(mode: String, line: String) -> Result<(), String> {
    crate::startup_log::append_startup_log_line(&mode, &line)
}
