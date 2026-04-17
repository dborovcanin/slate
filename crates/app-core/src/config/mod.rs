use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use time::{Month, OffsetDateTime, UtcOffset};

const DEFAULT_COLOR_SCHEME: &str = "gruvbox-light";
const DEFAULT_BACKGROUND: &str = "plain";
const DEFAULT_FONT: &str = "jetbrains-mono";
const DEFAULT_FONT_SIZE: u8 = 14;
const MIN_FONT_SIZE: u8 = 11;
const MAX_FONT_SIZE: u8 = 28;
const DEFAULT_VIM_MODE: bool = false;
const DEFAULT_TERMINAL_MODE: bool = false;
const DEFAULT_MARKDOWN_AUTOFORMAT: bool = true;
const DEFAULT_CHECKLIST_AUTO_REORDER: bool = true;
const DEFAULT_DATE_FORMAT: &str = "%Y-%m-%d";
const DEFAULT_DATE_TIME_FORMAT: &str = "%Y-%m-%d %H:%M";
const DEFAULT_FORMAT_ON_SAVE: bool = false;
const DEFAULT_VARIABLES_ENABLED: bool = true;
const DEFAULT_VARIABLE_AUTOCOMPLETE_MIN_CHARS: u8 = 3;
const MIN_VARIABLE_AUTOCOMPLETE_MIN_CHARS: u8 = 1;
const MAX_VARIABLE_AUTOCOMPLETE_MIN_CHARS: u8 = 8;
const DEFAULT_EMAIL_NOTE_PREFIX: &str = "inbox-email";
const DEFAULT_EMAIL_ROTATION: &str = "daily-local";
const DEFAULT_IMAP_HOST: &str = "imap.example.com";
const DEFAULT_IMAP_PORT: u16 = 993;
const DEFAULT_IMAP_USERNAME: &str = "";
const DEFAULT_IMAP_PASSWORD_ENV: &str = "SLATE_IMAP_PASSWORD";
const DEFAULT_IMAP_FOLDER: &str = "INBOX";
const DEFAULT_IMAP_POLL_SECONDS: u64 = 60;
const DEFAULT_IMAP_MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_IMAP_MAX_BODY_BYTES: usize = 512 * 1024;
const MIN_IMAP_MAX_BYTES: usize = 1024;
const MAX_IMAP_MAX_BYTES: usize = 64 * 1024 * 1024;
const MIN_IMAP_POLL_SECONDS: u64 = 10;
const MAX_IMAP_POLL_SECONDS: u64 = 24 * 60 * 60;
const DEFAULT_CONFIG: &str = r#"# Note configuration
#
# Color schemes:
#   catppuccin-mocha, catppuccin-latte, gruvbox-dark, gruvbox-light,
#   dracula, dark, white, solarized-dark, solarized-light,
#   nord, tokyo-night, one-dark
#
# Backgrounds:
#   plain, lines, squares, dots, diagonal
#
# Fonts:
#   jetbrains-mono, fira-code, cascadia-code, iosevka, hack, source-code-pro
#
# Date format tokens:
#   %Y, %y, %m, %d, %b, %B, %H, %M
# variables.autocomplete_min_chars range:
#   1..8

[theme]
color_scheme = "gruvbox-light"
background = "plain"
font = "jetbrains-mono"
font_size = 14

[editor]
# Enable markdown editing helpers (list continuation, table alignment, etc.)
markdown_autoformat = true
# Automatically move checked checklist items to bottom and unchecked to top
checklist_auto_reorder = true
# Run :format before save (Ctrl+S and save flush path)
format_on_save = false
# Start in terminal mode by default when launched from a TTY
terminal_mode = false
# Enable Vim keybindings in GUI editor
vim_mode = false
# Date format used by :date and date picker insert
date_format = "%Y-%m-%d"
# Date+time format used by :date (when time is included) and :notify
date_time_format = "%Y-%m-%d %H:%M"

[editor.variables]
# Enable note-local reactive variables
enabled = true
# Minimum typed characters to show variable completion suggestions
autocomplete_min_chars = 3

[special_notes]
# Prefix for date-partitioned email inbox notes
email_note_prefix = "inbox-email"
# Rotation strategy for email captures
email_rotation = "daily-local"

[imap]
# IMAP server host and implicit TLS port (993 by default)
host = "imap.example.com"
port = 993
# Account username/login for IMAP
username = ""
# Environment variable containing IMAP account password or app password
password_env = "SLATE_IMAP_PASSWORD"
# Folder to pull (typically INBOX)
folder = "INBOX"
# Poll interval for `slate imap-sync --watch`
poll_seconds = 60
# Maximum accepted raw message payload size in bytes
max_message_bytes = 8388608
# Maximum stored message body size in bytes (truncated in note body when exceeded)
max_body_bytes = 524288
"#;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThemeConfig {
    pub color_scheme: String,
    pub background: String,
    pub font: String,
    pub font_size: u8,
    pub markdown_autoformat: bool,
    pub checklist_auto_reorder: bool,
    pub format_on_save: bool,
    pub terminal_mode: bool,
    pub vim_mode: bool,
    pub date_format: String,
    pub date_time_format: String,
    pub variables_enabled: bool,
    pub variables_autocomplete_min_chars: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpecialNotesConfig {
    pub email_note_prefix: String,
    pub email_rotation: String,
}

impl Default for SpecialNotesConfig {
    fn default() -> Self {
        Self {
            email_note_prefix: DEFAULT_EMAIL_NOTE_PREFIX.to_string(),
            email_rotation: DEFAULT_EMAIL_ROTATION.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImapConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password_env: String,
    pub folder: String,
    pub poll_seconds: u64,
    pub max_message_bytes: usize,
    pub max_body_bytes: usize,
}

impl Default for ImapConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_IMAP_HOST.to_string(),
            port: DEFAULT_IMAP_PORT,
            username: DEFAULT_IMAP_USERNAME.to_string(),
            password_env: DEFAULT_IMAP_PASSWORD_ENV.to_string(),
            folder: DEFAULT_IMAP_FOLDER.to_string(),
            poll_seconds: DEFAULT_IMAP_POLL_SECONDS,
            max_message_bytes: DEFAULT_IMAP_MAX_MESSAGE_BYTES,
            max_body_bytes: DEFAULT_IMAP_MAX_BODY_BYTES,
        }
    }
}

impl ImapConfig {
    pub fn validate_runtime(&self) -> Result<(), String> {
        if self.host.trim().is_empty() {
            return Err("imap.host must not be empty".to_string());
        }
        if self.port == 0 {
            return Err("imap.port must be greater than 0".to_string());
        }
        if self.username.trim().is_empty() {
            return Err("imap.username must not be empty".to_string());
        }
        if self.password_env.trim().is_empty() {
            return Err("imap.password_env must not be empty".to_string());
        }
        if std::env::var(self.password_env.trim())
            .map(|value| value.trim().is_empty())
            .unwrap_or(true)
        {
            return Err(format!(
                "IMAP password env var '{}' is missing or empty",
                self.password_env
            ));
        }
        if self.folder.trim().is_empty() {
            return Err("imap.folder must not be empty".to_string());
        }
        if self.poll_seconds < MIN_IMAP_POLL_SECONDS {
            return Err(format!(
                "imap.poll_seconds must be >= {MIN_IMAP_POLL_SECONDS}"
            ));
        }
        if self.max_body_bytes > self.max_message_bytes {
            return Err(
                "imap.max_body_bytes must be less than or equal to imap.max_message_bytes"
                    .to_string(),
            );
        }
        Ok(())
    }
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            color_scheme: DEFAULT_COLOR_SCHEME.to_string(),
            background: DEFAULT_BACKGROUND.to_string(),
            font: DEFAULT_FONT.to_string(),
            font_size: DEFAULT_FONT_SIZE,
            markdown_autoformat: DEFAULT_MARKDOWN_AUTOFORMAT,
            checklist_auto_reorder: DEFAULT_CHECKLIST_AUTO_REORDER,
            format_on_save: DEFAULT_FORMAT_ON_SAVE,
            terminal_mode: DEFAULT_TERMINAL_MODE,
            vim_mode: DEFAULT_VIM_MODE,
            date_format: DEFAULT_DATE_FORMAT.to_string(),
            date_time_format: DEFAULT_DATE_TIME_FORMAT.to_string(),
            variables_enabled: DEFAULT_VARIABLES_ENABLED,
            variables_autocomplete_min_chars: DEFAULT_VARIABLE_AUTOCOMPLETE_MIN_CHARS,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
struct FileConfig {
    #[serde(default)]
    theme: ThemeSection,
    #[serde(default)]
    editor: EditorSection,
    #[serde(default)]
    special_notes: SpecialNotesSection,
    #[serde(default)]
    imap: ImapSection,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ThemeSection {
    color_scheme: Option<String>,
    background: Option<String>,
    font: Option<String>,
    font_size: Option<u16>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct EditorSection {
    markdown_autoformat: Option<bool>,
    checklist_auto_reorder: Option<bool>,
    format_on_save: Option<bool>,
    terminal_mode: Option<bool>,
    vim_mode: Option<bool>,
    date_format: Option<String>,
    date_time_format: Option<String>,
    #[serde(default)]
    variables: VariablesSection,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct VariablesSection {
    enabled: Option<bool>,
    autocomplete_min_chars: Option<u16>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct SpecialNotesSection {
    email_note_prefix: Option<String>,
    email_rotation: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ImapSection {
    host: Option<String>,
    port: Option<u16>,
    username: Option<String>,
    password_env: Option<String>,
    folder: Option<String>,
    poll_seconds: Option<u64>,
    max_message_bytes: Option<u64>,
    max_body_bytes: Option<u64>,
}

pub fn ensure_config_file() -> Result<PathBuf, String> {
    let path = config_file_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create config dir: {e}"))?;
    }

    if !path.exists() {
        fs::write(&path, DEFAULT_CONFIG)
            .map_err(|e| format!("Failed to write config file: {e}"))?;
    }

    Ok(path)
}

pub fn load_theme_config() -> ThemeConfig {
    let path = match ensure_config_file() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("Config: {err}");
            return ThemeConfig::default();
        }
    };

    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("Config: failed to read {}: {err}", path.display());
            return ThemeConfig::default();
        }
    };

    match parse_theme_config(&text) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Config: failed to parse {}: {err}", path.display());
            ThemeConfig::default()
        }
    }
}

pub fn load_special_notes_config() -> SpecialNotesConfig {
    let path = match ensure_config_file() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("Config: {err}");
            return SpecialNotesConfig::default();
        }
    };

    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("Config: failed to read {}: {err}", path.display());
            return SpecialNotesConfig::default();
        }
    };

    match parse_special_notes_config(&text) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Config: failed to parse {}: {err}", path.display());
            SpecialNotesConfig::default()
        }
    }
}

pub fn load_imap_config() -> ImapConfig {
    let path = match ensure_config_file() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("Config: {err}");
            return ImapConfig::default();
        }
    };

    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("Config: failed to read {}: {err}", path.display());
            return ImapConfig::default();
        }
    };

    match parse_imap_config(&text) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Config: failed to parse {}: {err}", path.display());
            ImapConfig::default()
        }
    }
}

fn parse_theme_config(text: &str) -> Result<ThemeConfig, String> {
    let raw: FileConfig = toml::from_str(text).map_err(|e| e.to_string())?;
    let date_format = normalize_date_format(raw.editor.date_format);
    Ok(ThemeConfig {
        color_scheme: normalize_name(raw.theme.color_scheme, DEFAULT_COLOR_SCHEME),
        background: normalize_name(raw.theme.background, DEFAULT_BACKGROUND),
        font: normalize_name(raw.theme.font, DEFAULT_FONT),
        font_size: normalize_font_size(raw.theme.font_size),
        markdown_autoformat: raw
            .editor
            .markdown_autoformat
            .unwrap_or(DEFAULT_MARKDOWN_AUTOFORMAT),
        checklist_auto_reorder: raw
            .editor
            .checklist_auto_reorder
            .unwrap_or(DEFAULT_CHECKLIST_AUTO_REORDER),
        format_on_save: raw.editor.format_on_save.unwrap_or(DEFAULT_FORMAT_ON_SAVE),
        terminal_mode: raw.editor.terminal_mode.unwrap_or(DEFAULT_TERMINAL_MODE),
        vim_mode: raw.editor.vim_mode.unwrap_or(DEFAULT_VIM_MODE),
        date_time_format: normalize_date_time_format(raw.editor.date_time_format, &date_format),
        date_format,
        variables_enabled: raw
            .editor
            .variables
            .enabled
            .unwrap_or(DEFAULT_VARIABLES_ENABLED),
        variables_autocomplete_min_chars: normalize_variable_autocomplete_min_chars(
            raw.editor.variables.autocomplete_min_chars,
        ),
    })
}

fn parse_special_notes_config(text: &str) -> Result<SpecialNotesConfig, String> {
    let raw: FileConfig = toml::from_str(text).map_err(|e| e.to_string())?;
    Ok(SpecialNotesConfig {
        email_note_prefix: normalize_nonempty(
            raw.special_notes.email_note_prefix,
            DEFAULT_EMAIL_NOTE_PREFIX,
        ),
        email_rotation: normalize_email_rotation(raw.special_notes.email_rotation),
    })
}

fn parse_imap_config(text: &str) -> Result<ImapConfig, String> {
    let raw: FileConfig = toml::from_str(text).map_err(|e| e.to_string())?;
    Ok(ImapConfig {
        host: normalize_nonempty(raw.imap.host, DEFAULT_IMAP_HOST),
        port: raw.imap.port.unwrap_or(DEFAULT_IMAP_PORT),
        username: normalize_nonempty(raw.imap.username, DEFAULT_IMAP_USERNAME),
        password_env: normalize_nonempty(raw.imap.password_env, DEFAULT_IMAP_PASSWORD_ENV),
        folder: normalize_nonempty(raw.imap.folder, DEFAULT_IMAP_FOLDER),
        poll_seconds: normalize_imap_poll_seconds(raw.imap.poll_seconds),
        max_message_bytes: normalize_imap_max_bytes(
            raw.imap.max_message_bytes,
            DEFAULT_IMAP_MAX_MESSAGE_BYTES,
        ),
        max_body_bytes: normalize_imap_max_bytes(raw.imap.max_body_bytes, DEFAULT_IMAP_MAX_BODY_BYTES),
    })
}

fn normalize_name(value: Option<String>, fallback: &str) -> String {
    let name = value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(fallback)
        .to_lowercase()
        .replace('_', "-")
        .replace(' ', "-");

    if name.is_empty() {
        fallback.to_string()
    } else {
        name
    }
}

fn normalize_nonempty(value: Option<String>, fallback: &str) -> String {
    value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn normalize_font_size(value: Option<u16>) -> u8 {
    let size = value
        .map(|v| v.clamp(MIN_FONT_SIZE as u16, MAX_FONT_SIZE as u16))
        .unwrap_or(DEFAULT_FONT_SIZE as u16);

    size as u8
}

fn normalize_date_format(value: Option<String>) -> String {
    let trimmed = value.as_deref().map(str::trim).unwrap_or("");
    if trimmed.is_empty() {
        DEFAULT_DATE_FORMAT.to_string()
    } else {
        trimmed.to_string()
    }
}

fn normalize_date_time_format(value: Option<String>, date_format: &str) -> String {
    let trimmed = value.as_deref().map(str::trim).unwrap_or("");
    if trimmed.is_empty() {
        format!("{date_format} %H:%M")
    } else {
        trimmed.to_string()
    }
}

fn normalize_variable_autocomplete_min_chars(value: Option<u16>) -> u8 {
    value
        .map(|v| {
            v.clamp(
                MIN_VARIABLE_AUTOCOMPLETE_MIN_CHARS as u16,
                MAX_VARIABLE_AUTOCOMPLETE_MIN_CHARS as u16,
            ) as u8
        })
        .unwrap_or(DEFAULT_VARIABLE_AUTOCOMPLETE_MIN_CHARS)
}

fn normalize_imap_max_bytes(value: Option<u64>, fallback: usize) -> usize {
    value
        .map(|raw| raw.clamp(MIN_IMAP_MAX_BYTES as u64, MAX_IMAP_MAX_BYTES as u64) as usize)
        .unwrap_or(fallback)
}

fn normalize_imap_poll_seconds(value: Option<u64>) -> u64 {
    value
        .map(|raw| raw.clamp(MIN_IMAP_POLL_SECONDS, MAX_IMAP_POLL_SECONDS))
        .unwrap_or(DEFAULT_IMAP_POLL_SECONDS)
}

fn normalize_email_rotation(value: Option<String>) -> String {
    let normalized = value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(DEFAULT_EMAIL_ROTATION)
        .to_lowercase();
    match normalized.as_str() {
        "daily-local" => normalized,
        _ => DEFAULT_EMAIL_ROTATION.to_string(),
    }
}

pub fn resolve_email_note_id(special: &SpecialNotesConfig, now_utc: OffsetDateTime) -> String {
    let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    resolve_email_note_id_with_offset(special, now_utc, offset)
}

fn resolve_email_note_id_with_offset(
    special: &SpecialNotesConfig,
    now_utc: OffsetDateTime,
    offset: UtcOffset,
) -> String {
    let local = now_utc.to_offset(offset);
    let month_number = match local.month() {
        Month::January => 1,
        Month::February => 2,
        Month::March => 3,
        Month::April => 4,
        Month::May => 5,
        Month::June => 6,
        Month::July => 7,
        Month::August => 8,
        Month::September => 9,
        Month::October => 10,
        Month::November => 11,
        Month::December => 12,
    };
    format!(
        "{}-{:04}-{:02}-{:02}",
        special.email_note_prefix,
        local.year(),
        month_number,
        local.day()
    )
}

fn config_file_path() -> Result<PathBuf, String> {
    let dirs = ProjectDirs::from("io", "github", "slate")
        .ok_or_else(|| "Failed to determine app config directory".to_string())?;
    Ok(dirs.config_dir().join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_theme_section() {
        let cfg = parse_theme_config(
            r#"
            [theme]
            color_scheme = "Gruvbox Dark"
            background = "squares"
            font = "Fira Code"
            font_size = 18

            [editor]
            markdown_autoformat = false
            checklist_auto_reorder = false
            format_on_save = true
            terminal_mode = true
            vim_mode = true
            date_format = "%d.%m.%Y"
            date_time_format = "%d.%m.%Y. %H:%M"

            [editor.variables]
            enabled = false
            autocomplete_min_chars = 5
            "#,
        )
        .expect("config parsed");

        assert_eq!(cfg.color_scheme, "gruvbox-dark");
        assert_eq!(cfg.background, "squares");
        assert_eq!(cfg.font, "fira-code");
        assert_eq!(cfg.font_size, 18);
        assert!(!cfg.markdown_autoformat);
        assert!(!cfg.checklist_auto_reorder);
        assert!(cfg.format_on_save);
        assert!(cfg.terminal_mode);
        assert!(cfg.vim_mode);
        assert_eq!(cfg.date_format, "%d.%m.%Y");
        assert_eq!(cfg.date_time_format, "%d.%m.%Y. %H:%M");
        assert!(!cfg.variables_enabled);
        assert_eq!(cfg.variables_autocomplete_min_chars, 5);
    }

    #[test]
    fn uses_defaults_when_theme_values_are_missing() {
        let cfg = parse_theme_config("").expect("empty config parsed");
        assert_eq!(cfg, ThemeConfig::default());
    }

    #[test]
    fn normalize_name_uses_fallback_on_empty() {
        assert_eq!(normalize_name(Some("   ".to_string()), "dark"), "dark");
        assert_eq!(normalize_name(None, "plain"), "plain");
    }

    #[test]
    fn normalize_font_size_clamps_value() {
        assert_eq!(normalize_font_size(Some(5)), 11);
        assert_eq!(normalize_font_size(Some(200)), 28);
        assert_eq!(normalize_font_size(Some(16)), 16);
        assert_eq!(normalize_font_size(None), 14);
    }

    #[test]
    fn defaults_vim_mode_to_false() {
        let cfg = parse_theme_config("[theme]\ncolor_scheme = 'dark'").expect("config parsed");
        assert!(cfg.markdown_autoformat);
        assert!(cfg.checklist_auto_reorder);
        assert!(!cfg.format_on_save);
        assert!(!cfg.terminal_mode);
        assert!(!cfg.vim_mode);
        assert_eq!(cfg.date_format, "%Y-%m-%d");
        assert_eq!(cfg.date_time_format, "%Y-%m-%d %H:%M");
        assert!(cfg.variables_enabled);
        assert_eq!(cfg.variables_autocomplete_min_chars, 3);
    }

    #[test]
    fn parses_markdown_autoformat_override() {
        let cfg = parse_theme_config("[editor]\nmarkdown_autoformat = false").expect("config");
        assert!(!cfg.markdown_autoformat);
    }

    #[test]
    fn parses_checklist_auto_reorder_override() {
        let cfg = parse_theme_config("[editor]\nchecklist_auto_reorder = false").expect("config");
        assert!(!cfg.checklist_auto_reorder);
    }

    #[test]
    fn parses_terminal_mode_override() {
        let cfg = parse_theme_config("[editor]\nterminal_mode = true").expect("config");
        assert!(cfg.terminal_mode);
    }

    #[test]
    fn normalize_date_format_uses_default_for_blank() {
        assert_eq!(normalize_date_format(None), "%Y-%m-%d");
        assert_eq!(normalize_date_format(Some(" ".to_string())), "%Y-%m-%d");
        assert_eq!(
            normalize_date_format(Some("%m/%d/%Y".to_string())),
            "%m/%d/%Y"
        );
    }

    #[test]
    fn normalize_date_time_format_defaults_from_date_format() {
        assert_eq!(
            normalize_date_time_format(None, "%d.%m.%Y"),
            "%d.%m.%Y %H:%M"
        );
        assert_eq!(
            normalize_date_time_format(Some(" ".to_string()), "%d.%m.%Y"),
            "%d.%m.%Y %H:%M"
        );
        assert_eq!(
            normalize_date_time_format(Some("%d.%m.%Y. %H:%M".to_string()), "%Y-%m-%d"),
            "%d.%m.%Y. %H:%M"
        );
    }

    #[test]
    fn normalize_variable_autocomplete_min_chars_clamps_value() {
        assert_eq!(normalize_variable_autocomplete_min_chars(None), 3);
        assert_eq!(normalize_variable_autocomplete_min_chars(Some(0)), 1);
        assert_eq!(normalize_variable_autocomplete_min_chars(Some(99)), 8);
        assert_eq!(normalize_variable_autocomplete_min_chars(Some(4)), 4);
    }

    #[test]
    fn parses_imap_and_special_notes_sections() {
        let special = parse_special_notes_config(
            r#"
            [special_notes]
            email_note_prefix = "mailbox"
            email_rotation = "daily-local"
            "#,
        )
        .expect("special notes parsed");
        assert_eq!(special.email_note_prefix, "mailbox");
        assert_eq!(special.email_rotation, "daily-local");

        let imap = parse_imap_config(
            r#"
            [imap]
            host = "imap.test.local"
            port = 993
            username = "relay"
            password_env = "IMAP_PASS"
            folder = "INBOX"
            poll_seconds = 30
            max_message_bytes = 4096
            max_body_bytes = 2048
            "#,
        )
        .expect("imap parsed");
        assert_eq!(imap.host, "imap.test.local");
        assert_eq!(imap.port, 993);
        assert_eq!(imap.username, "relay");
        assert_eq!(imap.password_env, "IMAP_PASS");
        assert_eq!(imap.folder, "INBOX");
        assert_eq!(imap.poll_seconds, 30);
        assert_eq!(imap.max_message_bytes, 4096);
        assert_eq!(imap.max_body_bytes, 2048);
    }

    #[test]
    fn resolve_email_note_id_uses_prefix_and_local_day() {
        let special = SpecialNotesConfig {
            email_note_prefix: "inbox-email".to_string(),
            email_rotation: "daily-local".to_string(),
        };
        let now = OffsetDateTime::from_unix_timestamp(1_714_516_200).expect("fixed ts");
        let offset = UtcOffset::from_hms(2, 0, 0).expect("offset");
        let note_id = resolve_email_note_id_with_offset(&special, now, offset);
        assert_eq!(note_id, "inbox-email-2024-05-01");
    }
}
