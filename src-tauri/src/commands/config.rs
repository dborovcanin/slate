use crate::config::{self, ThemeConfig};

#[tauri::command]
pub fn get_theme_config() -> ThemeConfig {
    config::load_theme_config()
}
