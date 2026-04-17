use crate::config::{self, ThemeConfig};

#[tauri::command]
pub fn get_theme_config() -> ThemeConfig {
    config::load_theme_config()
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RuntimeFlags {
    pub plain_text_mode: bool,
    pub backend_detach: bool,
    pub calc_disable: bool,
    pub markdown_disable: bool,
    pub folding_disable: bool,
    pub notify_disable: bool,
    pub autocomplete_disable: bool,
}

fn env_flag(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

#[tauri::command]
pub fn get_runtime_flags() -> RuntimeFlags {
    RuntimeFlags {
        plain_text_mode: env_flag("PLAIN_TEXT_MODE"),
        backend_detach: env_flag("BACKEND_DETACH"),
        calc_disable: env_flag("CALC_DISABLE"),
        markdown_disable: env_flag("MARKDOWN_DISABLE"),
        folding_disable: env_flag("FOLDING_DISABLE"),
        notify_disable: env_flag("NOTIFY_DISABLE"),
        autocomplete_disable: env_flag("AUTOCOMPLETE_DISABLE"),
    }
}
