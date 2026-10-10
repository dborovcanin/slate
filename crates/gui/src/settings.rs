//! Preferences the desktop app remembers between runs. They live in
//! `gui-settings.json` in the Slate data directory, next to `notes.db`.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Where the command line appears.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandBarStyle {
    /// A popup over the editor.
    #[default]
    Popup,
    /// A line at the bottom of the window, as in the terminal app.
    Bottom,
}

pub const DEFAULT_FONT_SIZE: f32 = 14.0;
pub const MIN_FONT_SIZE: f32 = 9.0;
pub const MAX_FONT_SIZE: f32 = 32.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub command_bar: CommandBarStyle,
    /// Vim keys (true) or standard editing (false).
    pub vim: bool,
    pub theme: crate::theme::ThemeMode,
    pub sidebar: bool,
    /// Editor text size in pixels (`Ctrl+=`, `Ctrl+-`, `Ctrl+0`).
    pub font_size: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            command_bar: CommandBarStyle::default(),
            vim: true,
            theme: Default::default(),
            sidebar: true,
            font_size: DEFAULT_FONT_SIZE,
        }
    }
}

impl Settings {
    pub fn path() -> Option<PathBuf> {
        Some(app_core::data_dir().ok()?.join("gui-settings.json"))
    }

    /// Missing or unreadable settings fall back to the defaults.
    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    pub fn parse(text: &str) -> Self {
        serde_json::from_str(text).unwrap_or_default()
    }

    /// Best effort: a failed write only means the choice is not remembered.
    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_tolerate_missing_fields() {
        let s = Settings {
            command_bar: CommandBarStyle::Bottom,
            vim: false,
            theme: crate::theme::ThemeMode::Light,
            sidebar: false,
            font_size: 18.0,
        };
        let text = serde_json::to_string(&s).unwrap();
        assert!(text.contains("\"bottom\""));
        assert_eq!(Settings::parse(&text), s);
        // Older files and hand edits: unknown or missing keys are fine.
        let partial = Settings::parse(r#"{"command_bar":"bottom","extra":1}"#);
        assert_eq!(partial.command_bar, CommandBarStyle::Bottom);
        assert!(partial.vim && partial.sidebar);
        assert_eq!(partial.font_size, DEFAULT_FONT_SIZE);
        assert_eq!(partial.theme, crate::theme::ThemeMode::Config);
        assert_eq!(Settings::parse("not json"), Settings::default());
    }
}
