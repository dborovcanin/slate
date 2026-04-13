use crate::config::ThemeConfig;
use crate::storage::Db;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalOptions {
    pub create_new: bool,
    pub note_id: Option<String>,
    pub list_only: bool,
}

pub fn run_terminal_session(
    _db: &Db,
    _config: &ThemeConfig,
    _opts: &TerminalOptions,
) -> Result<(), String> {
    Err("Terminal mode is currently supported only on Unix-like platforms.".to_string())
}
