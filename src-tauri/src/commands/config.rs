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
        // Use namespaced env vars to avoid accidental collisions with
        // unrelated shell/application variables.
        plain_text_mode: env_flag("SLATE_PLAIN_TEXT_MODE"),
        backend_detach: env_flag("SLATE_BACKEND_DETACH"),
        calc_disable: env_flag("SLATE_CALC_DISABLE"),
        markdown_disable: env_flag("SLATE_MARKDOWN_DISABLE"),
        folding_disable: env_flag("SLATE_FOLDING_DISABLE"),
        notify_disable: env_flag("SLATE_NOTIFY_DISABLE"),
        autocomplete_disable: env_flag("SLATE_AUTOCOMPLETE_DISABLE"),
    }
}
