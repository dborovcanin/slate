use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use time::{Month, OffsetDateTime, UtcOffset};

const DEFAULT_COLOR_SCHEME: &str = "gruvbox-light";
const DEFAULT_ACCENT: &str = "auto";
const DEFAULT_BACKGROUND: &str = "plain";
const DEFAULT_FONT: &str = "jetbrains-mono";
const DEFAULT_FONT_SIZE: u8 = 14;
const DEFAULT_ANIMATION_MODE: &str = "fast";
const DEFAULT_ANIMATION_STYLE: &str = "pop-up";
const MIN_FONT_SIZE: u8 = 11;
const MAX_FONT_SIZE: u8 = 28;
const DEFAULT_VIM_MODE: bool = false;
const DEFAULT_TERMINAL_MODE: bool = false;
const DEFAULT_MARKDOWN_AUTOFORMAT: bool = true;
const DEFAULT_CHECKLIST_AUTO_REORDER: bool = true;
const DEFAULT_AUTOSAVE: bool = true;
const DEFAULT_DATE_FORMAT: &str = "%Y-%m-%d";
const DEFAULT_DATE_TIME_FORMAT: &str = "%Y-%m-%d %H:%M";
const DEFAULT_FORMAT_ON_SAVE: bool = false;
const DEFAULT_VARIABLE_AUTOCOMPLETE_MIN_CHARS: u8 = 3;
const DEFAULT_MODULE_MATH_ENABLED: bool = true;
const DEFAULT_MODULE_TABLE_ENABLED: bool = true;
const DEFAULT_MODULE_VARIABLES_ENABLED: bool = true;
const DEFAULT_MODULE_STYLE_ENABLED: bool = true;
const DEFAULT_ENCRYPT_NOTES: bool = false;
const DEFAULT_NOTES_PASSWORD_ENV: &str = "SLATE_NOTES_PASSWORD";
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
const DEFAULT_IMAP_AUTO_SYNC_ON_STARTUP: bool = false;
const DEFAULT_IMAP_INITIAL_SYNC_MAX_MESSAGES: u32 = 200;
const DEFAULT_IMAP_INITIAL_SYNC_PAST_DAYS: u16 = 1;
const DEFAULT_IMAP_MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_IMAP_MAX_BODY_BYTES: usize = 512 * 1024;
const DEFAULT_PERF_ENABLED: bool = false;
const DEFAULT_PERF_UI_LOG_PATH: &str = "";
const DEFAULT_PERF_TUI_LOG_PATH: &str = "";
const MIN_IMAP_MAX_BYTES: usize = 1024;
const MAX_IMAP_MAX_BYTES: usize = 64 * 1024 * 1024;
const MIN_IMAP_POLL_SECONDS: u64 = 10;
const MAX_IMAP_POLL_SECONDS: u64 = 24 * 60 * 60;
const MIN_IMAP_INITIAL_SYNC_MAX_MESSAGES: u32 = 1;
const MAX_IMAP_INITIAL_SYNC_MAX_MESSAGES: u32 = 100_000;
const MIN_IMAP_INITIAL_SYNC_PAST_DAYS: u16 = 0;
const MAX_IMAP_INITIAL_SYNC_PAST_DAYS: u16 = 3650;
const DEFAULT_CONFIG: &str = r#"# Slate configuration
#
# All settings are optional. Unknown keys are ignored.
# Update only values you want to override.
#
# Color schemes:
#   slate-light, slate-dark,
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
# Animation modes:
#   fast, fade, smooth, spring
#   (compat: none disables all animations)
#
# Animation styles:
#   slide-up, pop-up, none
#
# Date format tokens:
#   %Y, %y, %m, %d, %b, %B, %H, %M

[theme]
# Visual theme palette.
color_scheme = "gruvbox-light"
# Accent palette token or custom hex.
#   auto, amber, sage, rose, plum, cobalt, slate
#   custom hex also works, e.g. #4f7bd9
accent = "auto"
# Canvas background pattern.
background = "plain"
# UI/editor font family token.
font = "jetbrains-mono"
# UI/editor font size in px. Range: 11..28
font_size = 14
# Motion speed profile.
animation_mode = "fast"
# Motion entry style for popups/surfaces.
animation_style = "pop-up"

[editor]
# Enable markdown helpers while typing (list continuation, table alignment).
markdown_autoformat = true
# Move checked checklist items to bottom and unchecked to top.
checklist_auto_reorder = true
# Persist edits automatically in GUI and terminal modes.
autosave = true
# Run :format before every save.
format_on_save = false
# Default to terminal runtime when launched from a TTY.
terminal_mode = false
# Enable Vim keybindings in GUI.
vim_mode = false
# Date format used by :date and date picker insert.
date_format = "%Y-%m-%d"
# Date+time format used by :date (with time) and :notify.
date_time_format = "%Y-%m-%d %H:%M"
# Minimum typed chars before variable autocomplete suggestions appear.
# Range: 1..8
variable_autocomplete_min_chars = 3

[editor.modules]
# Default per-note modules for newly created notes.
# Runtime behavior uses the active note's modules in both GUI and TUI.
math = true
table = true
variables = true
style = true

[editor.security]
# Encrypt newly created notes at rest by default.
encrypt_notes = false
# Environment variable with default note encryption password.
password_env = "SLATE_NOTES_PASSWORD"

[special_notes]
# Prefix for date-partitioned email inbox notes.
email_note_prefix = "inbox-email"
# Rotation strategy for email capture notes.
email_rotation = "daily-local"

[imap]
# IMAP server host.
host = "imap.example.com"
# IMAP TLS port.
port = 993
# IMAP account username/login.
username = ""
# Environment variable containing IMAP password/app password.
password_env = "SLATE_IMAP_PASSWORD"
# Mailbox folder to sync.
folder = "INBOX"
# Poll interval in seconds. Range: 10..86400
poll_seconds = 60
# Start background IMAP polling on app startup.
auto_sync_on_startup = false
# First sync window before UID checkpoint exists. Range: 1..100000
initial_sync_max_messages = 200
# Sync recency filter in days. 0 disables date filter. Range: 0..3650
initial_sync_past_days = 1
# Maximum accepted raw message payload size in bytes. Range: 1024..67108864
max_message_bytes = 8388608
# Maximum stored message body bytes. Must be <= max_message_bytes.
# Range: 1024..67108864
max_body_bytes = 524288

[perf]
# Enable runtime perf/startup logging and perf check runners.
enabled = false
# Optional explicit log paths. Empty means OS temp dir:
# Linux -> /tmp/slate-log-ui.log and /tmp/slate-log-tui.log
ui_log_path = ""
tui_log_path = ""
"#;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EditorModulesConfig {
    pub math: bool,
    pub table: bool,
    pub variables: bool,
    pub style: bool,
}

impl Default for EditorModulesConfig {
    fn default() -> Self {
        Self {
            math: DEFAULT_MODULE_MATH_ENABLED,
            table: DEFAULT_MODULE_TABLE_ENABLED,
            variables: DEFAULT_MODULE_VARIABLES_ENABLED,
            style: DEFAULT_MODULE_STYLE_ENABLED,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThemeConfig {
    pub color_scheme: String,
    pub accent: String,
    pub background: String,
    pub font: String,
    pub font_size: u8,
    pub animation_mode: String,
    pub animation_style: String,
    pub markdown_autoformat: bool,
    pub checklist_auto_reorder: bool,
    pub autosave: bool,
    pub format_on_save: bool,
    pub terminal_mode: bool,
    pub vim_mode: bool,
    pub date_format: String,
    pub date_time_format: String,
    pub variables_autocomplete_min_chars: u8,
    pub default_modules: EditorModulesConfig,
    // Compatibility-only fields. Runtime note security should be accessed
    // via NoteSecurityConfig helpers to avoid mixing with visual/editor prefs.
    pub encrypt_notes: bool,
    pub notes_password_env: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteSecurityConfig {
    pub encrypt_notes: bool,
    pub password_env: String,
}

impl Default for NoteSecurityConfig {
    fn default() -> Self {
        Self {
            encrypt_notes: DEFAULT_ENCRYPT_NOTES,
            password_env: DEFAULT_NOTES_PASSWORD_ENV.to_string(),
        }
    }
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
    pub auto_sync_on_startup: bool,
    pub initial_sync_max_messages: u32,
    pub initial_sync_past_days: u16,
    pub max_message_bytes: usize,
    pub max_body_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PerfConfig {
    pub enabled: bool,
    pub ui_log_path: String,
    pub tui_log_path: String,
}

impl Default for PerfConfig {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_PERF_ENABLED,
            ui_log_path: DEFAULT_PERF_UI_LOG_PATH.to_string(),
            tui_log_path: DEFAULT_PERF_TUI_LOG_PATH.to_string(),
        }
    }
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
            auto_sync_on_startup: DEFAULT_IMAP_AUTO_SYNC_ON_STARTUP,
            initial_sync_max_messages: DEFAULT_IMAP_INITIAL_SYNC_MAX_MESSAGES,
            initial_sync_past_days: DEFAULT_IMAP_INITIAL_SYNC_PAST_DAYS,
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
        if self.initial_sync_max_messages == 0 {
            return Err("imap.initial_sync_max_messages must be greater than 0".to_string());
        }
        if self.initial_sync_past_days > MAX_IMAP_INITIAL_SYNC_PAST_DAYS {
            return Err(format!(
                "imap.initial_sync_past_days must be <= {MAX_IMAP_INITIAL_SYNC_PAST_DAYS}"
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
            accent: DEFAULT_ACCENT.to_string(),
            background: DEFAULT_BACKGROUND.to_string(),
            font: DEFAULT_FONT.to_string(),
            font_size: DEFAULT_FONT_SIZE,
            animation_mode: DEFAULT_ANIMATION_MODE.to_string(),
            animation_style: DEFAULT_ANIMATION_STYLE.to_string(),
            markdown_autoformat: DEFAULT_MARKDOWN_AUTOFORMAT,
            checklist_auto_reorder: DEFAULT_CHECKLIST_AUTO_REORDER,
            autosave: DEFAULT_AUTOSAVE,
            format_on_save: DEFAULT_FORMAT_ON_SAVE,
            terminal_mode: DEFAULT_TERMINAL_MODE,
            vim_mode: DEFAULT_VIM_MODE,
            date_format: DEFAULT_DATE_FORMAT.to_string(),
            date_time_format: DEFAULT_DATE_TIME_FORMAT.to_string(),
            variables_autocomplete_min_chars: DEFAULT_VARIABLE_AUTOCOMPLETE_MIN_CHARS,
            default_modules: EditorModulesConfig::default(),
            encrypt_notes: DEFAULT_ENCRYPT_NOTES,
            notes_password_env: DEFAULT_NOTES_PASSWORD_ENV.to_string(),
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
    #[serde(default)]
    perf: PerfSection,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ThemeSection {
    color_scheme: Option<String>,
    accent: Option<String>,
    background: Option<String>,
    font: Option<String>,
    font_size: Option<u16>,
    animation_mode: Option<String>,
    animation_style: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct EditorSection {
    markdown_autoformat: Option<bool>,
    checklist_auto_reorder: Option<bool>,
    autosave: Option<bool>,
    format_on_save: Option<bool>,
    terminal_mode: Option<bool>,
    vim_mode: Option<bool>,
    date_format: Option<String>,
    date_time_format: Option<String>,
    variable_autocomplete_min_chars: Option<u16>,
    #[serde(default)]
    modules: ModulesSection,
    #[serde(default)]
    security: SecuritySection,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ModulesSection {
    math: Option<bool>,
    table: Option<bool>,
    variables: Option<bool>,
    style: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct SecuritySection {
    encrypt_notes: Option<bool>,
    password_env: Option<String>,
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
    auto_sync_on_startup: Option<bool>,
    initial_sync_max_messages: Option<u32>,
    initial_sync_past_days: Option<u16>,
    max_message_bytes: Option<u64>,
    max_body_bytes: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PerfSection {
    enabled: Option<bool>,
    ui_log_path: Option<String>,
    tui_log_path: Option<String>,
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

pub fn note_security_config_from_theme(theme: &ThemeConfig) -> NoteSecurityConfig {
    NoteSecurityConfig {
        encrypt_notes: theme.encrypt_notes,
        password_env: theme.notes_password_env.clone(),
    }
}

pub fn load_note_security_config() -> NoteSecurityConfig {
    note_security_config_from_theme(&load_theme_config())
}

pub fn resolve_default_note_encryption_password(
    security: &NoteSecurityConfig,
) -> Result<Option<String>, String> {
    if !security.encrypt_notes {
        return Ok(None);
    }
    let env_name = security.password_env.trim();
    if env_name.is_empty() {
        return Err("config editor.security.password_env must not be empty".to_string());
    }
    let password = std::env::var(env_name).map_err(|_| {
        format!("global note encryption enabled but env var '{env_name}' is missing")
    })?;
    if password.trim().is_empty() {
        return Err(format!(
            "global note encryption enabled but env var '{env_name}' is empty"
        ));
    }
    Ok(Some(password))
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

pub fn load_perf_config() -> PerfConfig {
    let path = match ensure_config_file() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("Config: {err}");
            return PerfConfig::default();
        }
    };

    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("Config: failed to read {}: {err}", path.display());
            return PerfConfig::default();
        }
    };

    match parse_perf_config(&text) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Config: failed to parse {}: {err}", path.display());
            PerfConfig::default()
        }
    }
}

fn parse_theme_config(text: &str) -> Result<ThemeConfig, String> {
    let raw: FileConfig = toml::from_str(text).map_err(|e| e.to_string())?;
    let date_format = normalize_date_format(raw.editor.date_format);
    let (animation_mode, animation_style) =
        normalize_animation_pair(raw.theme.animation_mode, raw.theme.animation_style);
    Ok(ThemeConfig {
        color_scheme: normalize_name(raw.theme.color_scheme, DEFAULT_COLOR_SCHEME),
        accent: normalize_name(raw.theme.accent, DEFAULT_ACCENT),
        background: normalize_name(raw.theme.background, DEFAULT_BACKGROUND),
        font: normalize_name(raw.theme.font, DEFAULT_FONT),
        font_size: normalize_font_size(raw.theme.font_size),
        animation_mode,
        animation_style,
        markdown_autoformat: raw
            .editor
            .markdown_autoformat
            .unwrap_or(DEFAULT_MARKDOWN_AUTOFORMAT),
        checklist_auto_reorder: raw
            .editor
            .checklist_auto_reorder
            .unwrap_or(DEFAULT_CHECKLIST_AUTO_REORDER),
        autosave: raw.editor.autosave.unwrap_or(DEFAULT_AUTOSAVE),
        format_on_save: raw.editor.format_on_save.unwrap_or(DEFAULT_FORMAT_ON_SAVE),
        terminal_mode: raw.editor.terminal_mode.unwrap_or(DEFAULT_TERMINAL_MODE),
        vim_mode: raw.editor.vim_mode.unwrap_or(DEFAULT_VIM_MODE),
        date_time_format: normalize_date_time_format(raw.editor.date_time_format, &date_format),
        date_format,
        variables_autocomplete_min_chars: normalize_variable_autocomplete_min_chars(
            raw.editor.variable_autocomplete_min_chars,
        ),
        default_modules: EditorModulesConfig {
            math: raw
                .editor
                .modules
                .math
                .unwrap_or(DEFAULT_MODULE_MATH_ENABLED),
            table: raw
                .editor
                .modules
                .table
                .unwrap_or(DEFAULT_MODULE_TABLE_ENABLED),
            variables: raw
                .editor
                .modules
                .variables
                .unwrap_or(DEFAULT_MODULE_VARIABLES_ENABLED),
            style: raw
                .editor
                .modules
                .style
                .unwrap_or(DEFAULT_MODULE_STYLE_ENABLED),
        },
        encrypt_notes: raw
            .editor
            .security
            .encrypt_notes
            .unwrap_or(DEFAULT_ENCRYPT_NOTES),
        notes_password_env: normalize_nonempty(
            raw.editor.security.password_env,
            DEFAULT_NOTES_PASSWORD_ENV,
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
        auto_sync_on_startup: raw
            .imap
            .auto_sync_on_startup
            .unwrap_or(DEFAULT_IMAP_AUTO_SYNC_ON_STARTUP),
        initial_sync_max_messages: normalize_imap_initial_sync_max_messages(
            raw.imap.initial_sync_max_messages,
        ),
        initial_sync_past_days: normalize_imap_initial_sync_past_days(
            raw.imap.initial_sync_past_days,
        ),
        max_message_bytes: normalize_imap_max_bytes(
            raw.imap.max_message_bytes,
            DEFAULT_IMAP_MAX_MESSAGE_BYTES,
        ),
        max_body_bytes: normalize_imap_max_bytes(
            raw.imap.max_body_bytes,
            DEFAULT_IMAP_MAX_BODY_BYTES,
        ),
    })
}

fn parse_perf_config(text: &str) -> Result<PerfConfig, String> {
    let raw: FileConfig = toml::from_str(text).map_err(|e| e.to_string())?;
    Ok(PerfConfig {
        enabled: raw.perf.enabled.unwrap_or(DEFAULT_PERF_ENABLED),
        ui_log_path: normalize_optional_path(raw.perf.ui_log_path),
        tui_log_path: normalize_optional_path(raw.perf.tui_log_path),
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

fn normalize_optional_path(value: Option<String>) -> String {
    value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("")
        .to_string()
}

fn normalize_font_size(value: Option<u16>) -> u8 {
    let size = value
        .map(|v| v.clamp(MIN_FONT_SIZE as u16, MAX_FONT_SIZE as u16))
        .unwrap_or(DEFAULT_FONT_SIZE as u16);

    size as u8
}

fn normalize_animation_token(value: Option<String>) -> String {
    value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("")
        .to_lowercase()
        .replace('_', "-")
        .replace(' ', "-")
}

fn normalize_animation_mode(value: Option<String>) -> String {
    let normalized = normalize_animation_token(value);
    if normalized.is_empty() {
        return DEFAULT_ANIMATION_MODE.to_string();
    }

    match normalized.as_str() {
        "none" | "fast" | "fade" | "smooth" | "spring" => normalized,
        "sping" => "spring".to_string(),
        _ => DEFAULT_ANIMATION_MODE.to_string(),
    }
}

fn normalize_animation_style(value: Option<String>) -> String {
    let normalized = normalize_animation_token(value);
    if normalized.is_empty() {
        return DEFAULT_ANIMATION_STYLE.to_string();
    }

    match normalized.as_str() {
        "slide-up" | "from-bottom" | "bottom" => "slide-up".to_string(),
        "pop-up" | "popup" => "pop-up".to_string(),
        "none" => "none".to_string(),
        // Legacy: old style value now maps to no-motion style.
        "fade" => "none".to_string(),
        _ => DEFAULT_ANIMATION_STYLE.to_string(),
    }
}

fn normalize_animation_pair(mode: Option<String>, style: Option<String>) -> (String, String) {
    (
        normalize_animation_mode(mode),
        normalize_animation_style(style),
    )
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

fn normalize_imap_initial_sync_max_messages(value: Option<u32>) -> u32 {
    value
        .map(|raw| {
            raw.clamp(
                MIN_IMAP_INITIAL_SYNC_MAX_MESSAGES,
                MAX_IMAP_INITIAL_SYNC_MAX_MESSAGES,
            )
        })
        .unwrap_or(DEFAULT_IMAP_INITIAL_SYNC_MAX_MESSAGES)
}

fn normalize_imap_initial_sync_past_days(value: Option<u16>) -> u16 {
    value
        .map(|raw| {
            raw.clamp(
                MIN_IMAP_INITIAL_SYNC_PAST_DAYS,
                MAX_IMAP_INITIAL_SYNC_PAST_DAYS,
            )
        })
        .unwrap_or(DEFAULT_IMAP_INITIAL_SYNC_PAST_DAYS)
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
            accent = "rose"
            background = "squares"
            font = "Fira Code"
            font_size = 18
            animation_mode = "smooth"
            animation_style = "from bottom"

            [editor]
            markdown_autoformat = false
            checklist_auto_reorder = false
            autosave = false
            format_on_save = true
            terminal_mode = true
            vim_mode = true
            date_format = "%d.%m.%Y"
            date_time_format = "%d.%m.%Y. %H:%M"
            variable_autocomplete_min_chars = 5

            [editor.modules]
            math = false
            table = true
            variables = false
            style = true
            "#,
        )
        .expect("config parsed");

        assert_eq!(cfg.color_scheme, "gruvbox-dark");
        assert_eq!(cfg.accent, "rose");
        assert_eq!(cfg.background, "squares");
        assert_eq!(cfg.font, "fira-code");
        assert_eq!(cfg.font_size, 18);
        assert_eq!(cfg.animation_mode, "smooth");
        assert_eq!(cfg.animation_style, "slide-up");
        assert!(!cfg.markdown_autoformat);
        assert!(!cfg.checklist_auto_reorder);
        assert!(!cfg.autosave);
        assert!(cfg.format_on_save);
        assert!(cfg.terminal_mode);
        assert!(cfg.vim_mode);
        assert_eq!(cfg.date_format, "%d.%m.%Y");
        assert_eq!(cfg.date_time_format, "%d.%m.%Y. %H:%M");
        assert_eq!(cfg.variables_autocomplete_min_chars, 5);
        assert!(!cfg.default_modules.math);
        assert!(cfg.default_modules.table);
        assert!(!cfg.default_modules.variables);
        assert!(cfg.default_modules.style);
        assert!(!cfg.encrypt_notes);
        assert_eq!(cfg.notes_password_env, "SLATE_NOTES_PASSWORD");
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
    fn normalize_animation_mode_uses_supported_values() {
        assert_eq!(normalize_animation_mode(Some("fade".to_string())), "fade");
        assert_eq!(normalize_animation_mode(Some("none".to_string())), "none");
        assert_eq!(
            normalize_animation_mode(Some("smooth".to_string())),
            "smooth"
        );
        assert_eq!(
            normalize_animation_mode(Some("sping".to_string())),
            "spring"
        );
        assert_eq!(
            normalize_animation_mode(Some("sprinG".to_string())),
            "spring"
        );
        assert_eq!(normalize_animation_mode(Some("ultra".to_string())), "fast");
        assert_eq!(normalize_animation_mode(None), "fast");
    }

    #[test]
    fn normalize_animation_style_uses_supported_values() {
        assert_eq!(
            normalize_animation_style(Some("from_bottom".to_string())),
            "slide-up"
        );
        assert_eq!(
            normalize_animation_style(Some("bottom".to_string())),
            "slide-up"
        );
        assert_eq!(
            normalize_animation_style(Some("popUP".to_string())),
            "pop-up"
        );
        assert_eq!(normalize_animation_style(Some("fAde".to_string())), "none");
        assert_eq!(
            normalize_animation_style(Some("ultra".to_string())),
            "pop-up"
        );
        assert_eq!(normalize_animation_style(None), "pop-up");
    }

    #[test]
    fn normalize_animation_pair_combines_mode_and_style() {
        assert_eq!(
            normalize_animation_pair(Some("fade".to_string()), Some("slide-up".to_string())),
            ("fade".to_string(), "slide-up".to_string())
        );
        assert_eq!(
            normalize_animation_pair(Some("spring".to_string()), Some("from_bottom".to_string())),
            ("spring".to_string(), "slide-up".to_string())
        );
        assert_eq!(
            normalize_animation_pair(Some("fast".to_string()), Some("fade".to_string())),
            ("fast".to_string(), "none".to_string())
        );
    }

    #[test]
    fn defaults_vim_mode_to_false() {
        let cfg = parse_theme_config("[theme]\ncolor_scheme = 'dark'").expect("config parsed");
        assert!(cfg.markdown_autoformat);
        assert!(cfg.checklist_auto_reorder);
        assert!(cfg.autosave);
        assert!(!cfg.format_on_save);
        assert!(!cfg.terminal_mode);
        assert!(!cfg.vim_mode);
        assert_eq!(cfg.animation_mode, "fast");
        assert_eq!(cfg.animation_style, "pop-up");
        assert_eq!(cfg.date_format, "%Y-%m-%d");
        assert_eq!(cfg.date_time_format, "%Y-%m-%d %H:%M");
        assert_eq!(cfg.accent, "auto");
        assert_eq!(cfg.variables_autocomplete_min_chars, 3);
        assert!(cfg.default_modules.math);
        assert!(cfg.default_modules.table);
        assert!(cfg.default_modules.variables);
        assert!(cfg.default_modules.style);
        assert!(!cfg.encrypt_notes);
        assert_eq!(cfg.notes_password_env, "SLATE_NOTES_PASSWORD");
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
    fn parses_autosave_override() {
        let cfg = parse_theme_config("[editor]\nautosave = false").expect("config");
        assert!(!cfg.autosave);
    }

    #[test]
    fn parses_terminal_mode_override() {
        let cfg = parse_theme_config("[editor]\nterminal_mode = true").expect("config");
        assert!(cfg.terminal_mode);
    }

    #[test]
    fn parses_editor_security_section() {
        let cfg = parse_theme_config(
            r#"
            [editor.security]
            encrypt_notes = true
            password_env = "APP_NOTES_PASSWORD"
            "#,
        )
        .expect("config");
        assert!(cfg.encrypt_notes);
        assert_eq!(cfg.notes_password_env, "APP_NOTES_PASSWORD");
    }

    #[test]
    fn note_security_config_helpers_extract_and_validate() {
        let cfg = parse_theme_config(
            r#"
            [editor.security]
            encrypt_notes = true
            password_env = "APP_NOTES_PASSWORD"
            "#,
        )
        .expect("config");
        let security = note_security_config_from_theme(&cfg);
        assert!(security.encrypt_notes);
        assert_eq!(security.password_env, "APP_NOTES_PASSWORD");

        let disabled = NoteSecurityConfig {
            encrypt_notes: false,
            password_env: "IGNORED".to_string(),
        };
        assert_eq!(
            resolve_default_note_encryption_password(&disabled).expect("disabled mode"),
            None
        );

        let invalid_env = NoteSecurityConfig {
            encrypt_notes: true,
            password_env: "   ".to_string(),
        };
        let err = resolve_default_note_encryption_password(&invalid_env)
            .expect_err("blank env name should fail");
        assert!(err.contains("must not be empty"));
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
            auto_sync_on_startup = true
            initial_sync_max_messages = 75
            initial_sync_past_days = 3
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
        assert!(imap.auto_sync_on_startup);
        assert_eq!(imap.initial_sync_max_messages, 75);
        assert_eq!(imap.initial_sync_past_days, 3);
        assert_eq!(imap.max_message_bytes, 4096);
        assert_eq!(imap.max_body_bytes, 2048);
    }

    #[test]
    fn imap_defaults_initial_sync_past_days_to_one() {
        let imap = parse_imap_config("[imap]\n").expect("imap parsed");
        assert_eq!(imap.initial_sync_past_days, 1);
    }

    #[test]
    fn parses_perf_section_and_defaults() {
        let defaults = parse_perf_config("").expect("perf config parsed");
        assert_eq!(defaults, PerfConfig::default());

        let parsed = parse_perf_config(
            r#"
            [perf]
            enabled = true
            ui_log_path = "/tmp/slate-log-ui.log"
            tui_log_path = "/tmp/slate-log-tui.log"
            "#,
        )
        .expect("perf config parsed");
        assert!(parsed.enabled);
        assert_eq!(parsed.ui_log_path, "/tmp/slate-log-ui.log");
        assert_eq!(parsed.tui_log_path, "/tmp/slate-log-tui.log");
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
