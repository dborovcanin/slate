use directories::ProjectDirs;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use time::{Month, OffsetDateTime, UtcOffset};

const DEFAULT_COLOR_SCHEME: &str = "gruvbox-light";
const DEFAULT_ACCENT: &str = "auto";
const DEFAULT_ICONS: IconStyle = IconStyle::Nerd;
const DEFAULT_VIM_MODE: bool = false;
const DEFAULT_WRAP: bool = true;
const DEFAULT_DAILY_NOTE_PREFIX: &str = "daily";
const DEFAULT_DAILY_TEMPLATE: &str = "# {date}\n\n";
const DEFAULT_MARKDOWN_AUTOFORMAT: bool = true;
const DEFAULT_CHECKLIST_AUTO_REORDER: bool = true;
const DEFAULT_AUTOSAVE: bool = true;
const DEFAULT_RELOAD_OUTSIDE_CHANGES: bool = true;
const DEFAULT_DATE_FORMAT: &str = "%Y-%m-%d";
const DEFAULT_DATE_TIME_FORMAT: &str = "%Y-%m-%d %H:%M";
const DEFAULT_FORMAT_ON_SAVE: bool = false;
const DEFAULT_VARIABLE_AUTOCOMPLETE_MIN_CHARS: u8 = 3;
const DEFAULT_MODULE_MATH_ENABLED: bool = true;
const DEFAULT_MODULE_TABLE_ENABLED: bool = true;
const DEFAULT_MODULE_VARIABLES_ENABLED: bool = true;
const DEFAULT_MODULE_STYLE_ENABLED: bool = true;
const DEFAULT_MODULE_CROSS_NOTE_ENABLED: bool = true;
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
const DEFAULT_PERF_LOG_PATH: &str = "";
const DEFAULT_BACKGROUND_TASKS_ENABLED: bool = true;
const DEFAULT_MCP_ENABLED: bool = false;
const DEFAULT_MCP_ALLOW_DELETE: bool = false;
const MIN_IMAP_MAX_BYTES: usize = 1024;
const MAX_IMAP_MAX_BYTES: usize = 64 * 1024 * 1024;
const MIN_IMAP_POLL_SECONDS: u64 = 10;
const MAX_IMAP_POLL_SECONDS: u64 = 24 * 60 * 60;
const MIN_IMAP_INITIAL_SYNC_MAX_MESSAGES: u32 = 1;
const MAX_IMAP_INITIAL_SYNC_MAX_MESSAGES: u32 = 100_000;
const MIN_IMAP_INITIAL_SYNC_PAST_DAYS: u16 = 0;
const MAX_IMAP_INITIAL_SYNC_PAST_DAYS: u16 = 3650;
const DEFAULT_WEB_SEARCH_PROVIDER: &str = "auto";
const DEFAULT_WEB_SEARCH_API_KEY: &str = "";
const DEFAULT_WEB_SEARCH_ENGINE_ID: &str = "";
const DEFAULT_WEB_SEARCH_MAX_RESULTS: usize = 10;
const MIN_WEB_SEARCH_MAX_RESULTS: usize = 1;
const MAX_WEB_SEARCH_MAX_RESULTS: usize = 10;
const DEFAULT_TERMINAL_IMAGE_MAX_ROWS: usize = 15;
const MIN_TERMINAL_IMAGE_MAX_ROWS: usize = 1;
const MAX_TERMINAL_IMAGE_MAX_ROWS: usize = 100;
const DEFAULT_CONFIG: &str = r##"# Slate configuration
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
# Date format tokens:
#   %Y, %y, %m, %d, %b, %B, %H, %M

[theme]
# Visual theme palette.
color_scheme = "gruvbox-light"
# Accent palette token or custom hex.
#   auto, amber, sage, rose, plum, cobalt, slate
#   custom hex also works, e.g. #4f7bd9
accent = "auto"
# Icon glyphs in lists and the browser.
#   nerd (needs a Nerd Font), unicode, ascii
icons = "nerd"

[editor]
# Enable markdown helpers while typing (list continuation, table alignment).
markdown_autoformat = true
# Move checked checklist items to bottom and unchecked to top.
checklist_auto_reorder = true
# Persist edits automatically (idle flush + save on exit/switch).
autosave = true
# Load changes made to the open note outside this window (slate mcp, slate
# append, slate capture, IMAP sync, another Slate) while it has no unsaved
# edits. Checks the stored note about once a second while idle.
reload_outside_changes = true
# Run :format before every save.
format_on_save = false
# Start the editor in Vim normal mode.
vim_mode = false
# Soft-wrap long lines. Tables and code blocks always scroll horizontally.
wrap = true
# Date format used by :date and date picker insert.
date_format = "%Y-%m-%d"
# Date+time format used by :date (with time) and :remind.
date_time_format = "%Y-%m-%d %H:%M"
# Minimum typed chars before variable autocomplete suggestions appear.
# Range: 1..8
variable_autocomplete_min_chars = 3

[editor.modules]
# Default per-note modules for newly created notes.
# Runtime behavior uses the active note's modules.
math = true
table = true
variables = true
style = true
cross_note = true

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

[daily]
# Prefix for daily note ids (`slate today`, `:today`, `slate capture`).
note_prefix = "daily"
# Body of a new daily note. {date} is replaced with the date formatted by
# [editor] date_format.
template = "# {date}\n\n"

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
# Optional explicit log path. Empty means OS temp dir:
# Linux -> /tmp/slate-log.log
log_path = ""

[web_search]
# Online search provider:
#   "auto"       pick the best configured provider, then fall back (default)
#   "brave"      Brave Search API, needs api_key
#   "google"     legacy Google Custom Search, needs api_key + search_engine_id
#   "duckduckgo" key-free only; no API key is used
# "auto" uses Google when api_key and search_engine_id are both set, Brave when
# only api_key is set, and DuckDuckGo otherwise. Every keyed provider falls
# back to DuckDuckGo if its request fails, so search keeps working either way.
# DuckDuckGo has no search API: results are scraped and it now answers many
# requests with an anti-bot challenge, so set an api_key for reliable results.
provider = "auto"
# Brave Search API key, or Google Custom Search API key
api_key = ""
# Google is available only to existing Custom Search JSON API customers and
# that API is scheduled to shut down on 2027-01-01.
# Google Custom Search Engine ID / cx (only needed for Google)
search_engine_id = ""
# Maximum search results to return (1..10, default 10)
max_results = 10

[terminal]
# Inline image rendering mode:
#   "auto"       detect best graphics protocol (sixel, kitty, iterm2), fall back if unsupported (default)
#   "sixel"      force Sixel graphics (e.g. foot)
#   "kitty"      force Kitty graphics protocol (e.g. kitty, ghostty)
#   "iterm2"     force iTerm2 graphics protocol (e.g. iTerm2, wezterm)
#   "halfblocks" unicode halfblock characters fallback
#   "off"        text placeholder [image: alt] only
images = "auto"
# Maximum terminal row height for an inline image. Range: 1..100
image_max_rows = 15

[mcp]
# Let `slate mcp` serve notes to MCP clients (AI assistants and bots) over
# stdio. Clients can list, search, read, create, edit, rename and archive
# notes and manage collections; they cannot open encrypted notes. Off by
# default.
enabled = false
# Also let clients delete notes for good (with their history). Without it
# they can only move notes to the Archive collection.
allow_delete = false

[startup]
# Enable non-critical startup work asynchronously after first paint/edit
# (prewarm/hydration/background sync loops).
background_tasks_enabled = true
"##;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorModulesConfig {
    pub math: bool,
    pub table: bool,
    pub variables: bool,
    pub style: bool,
    pub cross_note: bool,
}

impl Default for EditorModulesConfig {
    fn default() -> Self {
        Self {
            math: DEFAULT_MODULE_MATH_ENABLED,
            table: DEFAULT_MODULE_TABLE_ENABLED,
            variables: DEFAULT_MODULE_VARIABLES_ENABLED,
            style: DEFAULT_MODULE_STYLE_ENABLED,
            cross_note: DEFAULT_MODULE_CROSS_NOTE_ENABLED,
        }
    }
}

/// Which glyph set the terminal UI draws icons with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IconStyle {
    /// Nerd Font private-use glyphs.
    #[default]
    Nerd,
    /// Symbols from standard Unicode blocks.
    Unicode,
    /// Plain ASCII markers.
    Ascii,
}

impl IconStyle {
    fn parse(value: Option<&str>) -> Self {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("unicode") => Self::Unicode,
            Some("ascii" | "none" | "plain") => Self::Ascii,
            Some("nerd" | "nerd-font" | "nerdfont") => Self::Nerd,
            _ => DEFAULT_ICONS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeConfig {
    pub color_scheme: String,
    pub accent: String,
    pub icons: IconStyle,
    pub markdown_autoformat: bool,
    pub checklist_auto_reorder: bool,
    pub autosave: bool,
    /// Take in changes made to the open note elsewhere while idle.
    pub reload_outside_changes: bool,
    pub format_on_save: bool,
    pub vim_mode: bool,
    pub wrap: bool,
    pub background_tasks_enabled: bool,
    pub date_format: String,
    pub date_time_format: String,
    pub variables_autocomplete_min_chars: u8,
    pub default_modules: EditorModulesConfig,
    /// `[editor.security]`: encryption of new notes.
    pub security: NoteSecurityConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyNotesConfig {
    pub note_prefix: String,
    pub template: String,
}

impl Default for DailyNotesConfig {
    fn default() -> Self {
        Self {
            note_prefix: DEFAULT_DAILY_NOTE_PREFIX.to_string(),
            template: DEFAULT_DAILY_TEMPLATE.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerfConfig {
    pub enabled: bool,
    pub log_path: String,
}

impl Default for PerfConfig {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_PERF_ENABLED,
            log_path: DEFAULT_PERF_LOG_PATH.to_string(),
        }
    }
}

/// `[mcp]`: the `slate mcp` server for MCP clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConfig {
    pub enabled: bool,
    /// Clients may delete notes, not only archive them.
    pub allow_delete: bool,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_MCP_ENABLED,
            allow_delete: DEFAULT_MCP_ALLOW_DELETE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSearchConfig {
    pub provider: String,
    pub api_key: String,
    pub search_engine_id: String,
    pub max_results: usize,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            provider: DEFAULT_WEB_SEARCH_PROVIDER.to_string(),
            api_key: DEFAULT_WEB_SEARCH_API_KEY.to_string(),
            search_engine_id: DEFAULT_WEB_SEARCH_ENGINE_ID.to_string(),
            max_results: DEFAULT_WEB_SEARCH_MAX_RESULTS,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TerminalImagesMode {
    #[default]
    Auto,
    Sixel,
    Kitty,
    Iterm2,
    Halfblocks,
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalImagesConfig {
    pub mode: TerminalImagesMode,
    pub max_rows: usize,
}

impl Default for TerminalImagesConfig {
    fn default() -> Self {
        Self {
            mode: TerminalImagesMode::default(),
            max_rows: DEFAULT_TERMINAL_IMAGE_MAX_ROWS,
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
            icons: DEFAULT_ICONS,
            markdown_autoformat: DEFAULT_MARKDOWN_AUTOFORMAT,
            checklist_auto_reorder: DEFAULT_CHECKLIST_AUTO_REORDER,
            autosave: DEFAULT_AUTOSAVE,
            reload_outside_changes: DEFAULT_RELOAD_OUTSIDE_CHANGES,
            format_on_save: DEFAULT_FORMAT_ON_SAVE,
            vim_mode: DEFAULT_VIM_MODE,
            wrap: DEFAULT_WRAP,
            background_tasks_enabled: DEFAULT_BACKGROUND_TASKS_ENABLED,
            date_format: DEFAULT_DATE_FORMAT.to_string(),
            date_time_format: DEFAULT_DATE_TIME_FORMAT.to_string(),
            variables_autocomplete_min_chars: DEFAULT_VARIABLE_AUTOCOMPLETE_MIN_CHARS,
            default_modules: EditorModulesConfig::default(),
            security: NoteSecurityConfig::default(),
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
    daily: DailySection,
    #[serde(default)]
    imap: ImapSection,
    #[serde(default)]
    perf: PerfSection,
    #[serde(default)]
    web_search: WebSearchSection,
    #[serde(default)]
    terminal: TerminalSection,
    #[serde(default)]
    startup: StartupSection,
    #[serde(default)]
    mcp: McpSection,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct TerminalSection {
    images: Option<String>,
    image_max_rows: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct WebSearchSection {
    provider: Option<String>,
    api_key: Option<String>,
    search_engine_id: Option<String>,
    max_results: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ThemeSection {
    color_scheme: Option<String>,
    accent: Option<String>,
    icons: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct EditorSection {
    markdown_autoformat: Option<bool>,
    checklist_auto_reorder: Option<bool>,
    autosave: Option<bool>,
    reload_outside_changes: Option<bool>,
    format_on_save: Option<bool>,
    vim_mode: Option<bool>,
    wrap: Option<bool>,
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
    cross_note: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct SecuritySection {
    encrypt_notes: Option<bool>,
    password_env: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct DailySection {
    note_prefix: Option<String>,
    template: Option<String>,
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
    log_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct StartupSection {
    background_tasks_enabled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct McpSection {
    enabled: Option<bool>,
    allow_delete: Option<bool>,
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

pub fn load_daily_notes_config() -> DailyNotesConfig {
    let Ok(path) = ensure_config_file() else {
        return DailyNotesConfig::default();
    };
    fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|text| parse_daily_notes_config(&text))
        .unwrap_or_else(|err| {
            eprintln!("Config: failed to parse {}: {err}", path.display());
            DailyNotesConfig::default()
        })
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

pub fn load_web_search_config() -> WebSearchConfig {
    let path = match ensure_config_file() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("Config: {err}");
            return WebSearchConfig::default();
        }
    };

    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("Config: failed to read {}: {err}", path.display());
            return WebSearchConfig::default();
        }
    };

    match parse_web_search_config(&text) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Config: failed to parse {}: {err}", path.display());
            WebSearchConfig::default()
        }
    }
}

/// Reads `[mcp]`. Unlike the other loaders, a config that cannot be read or
/// parsed is an error rather than the defaults, so the server never starts
/// from a config the user did not mean.
pub fn load_mcp_config() -> Result<(McpConfig, PathBuf), String> {
    let path = ensure_config_file()?;
    let text =
        fs::read_to_string(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let config =
        parse_mcp_config(&text).map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
    Ok((config, path))
}

pub fn load_terminal_images_config() -> TerminalImagesConfig {
    let path = match ensure_config_file() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("Config: {err}");
            return TerminalImagesConfig::default();
        }
    };

    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("Config: failed to read {}: {err}", path.display());
            return TerminalImagesConfig::default();
        }
    };

    match parse_terminal_images_config(&text) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Config: failed to parse {}: {err}", path.display());
            TerminalImagesConfig::default()
        }
    }
}

fn parse_theme_config(text: &str) -> Result<ThemeConfig, String> {
    let raw: FileConfig = toml::from_str(text).map_err(|e| e.to_string())?;
    let date_format = normalize_date_format(raw.editor.date_format);
    Ok(ThemeConfig {
        color_scheme: normalize_name(raw.theme.color_scheme, DEFAULT_COLOR_SCHEME),
        accent: normalize_name(raw.theme.accent, DEFAULT_ACCENT),
        icons: IconStyle::parse(raw.theme.icons.as_deref()),
        markdown_autoformat: raw
            .editor
            .markdown_autoformat
            .unwrap_or(DEFAULT_MARKDOWN_AUTOFORMAT),
        checklist_auto_reorder: raw
            .editor
            .checklist_auto_reorder
            .unwrap_or(DEFAULT_CHECKLIST_AUTO_REORDER),
        autosave: raw.editor.autosave.unwrap_or(DEFAULT_AUTOSAVE),
        reload_outside_changes: raw
            .editor
            .reload_outside_changes
            .unwrap_or(DEFAULT_RELOAD_OUTSIDE_CHANGES),
        format_on_save: raw.editor.format_on_save.unwrap_or(DEFAULT_FORMAT_ON_SAVE),
        vim_mode: raw.editor.vim_mode.unwrap_or(DEFAULT_VIM_MODE),
        wrap: raw.editor.wrap.unwrap_or(DEFAULT_WRAP),
        background_tasks_enabled: raw
            .startup
            .background_tasks_enabled
            .unwrap_or(DEFAULT_BACKGROUND_TASKS_ENABLED),
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
            cross_note: raw
                .editor
                .modules
                .cross_note
                .unwrap_or(DEFAULT_MODULE_CROSS_NOTE_ENABLED),
        },
        security: NoteSecurityConfig {
            encrypt_notes: raw
                .editor
                .security
                .encrypt_notes
                .unwrap_or(DEFAULT_ENCRYPT_NOTES),
            password_env: normalize_nonempty(
                raw.editor.security.password_env,
                DEFAULT_NOTES_PASSWORD_ENV,
            ),
        },
    })
}

fn parse_daily_notes_config(text: &str) -> Result<DailyNotesConfig, String> {
    let raw: FileConfig = toml::from_str(text).map_err(|e| e.to_string())?;
    Ok(DailyNotesConfig {
        note_prefix: normalize_nonempty(raw.daily.note_prefix, DEFAULT_DAILY_NOTE_PREFIX),
        template: raw
            .daily
            .template
            .unwrap_or_else(|| DEFAULT_DAILY_TEMPLATE.to_string()),
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
        log_path: normalize_optional_path(raw.perf.log_path),
    })
}

fn parse_mcp_config(text: &str) -> Result<McpConfig, String> {
    let raw: FileConfig = toml::from_str(text).map_err(|e| e.to_string())?;
    Ok(McpConfig {
        enabled: raw.mcp.enabled.unwrap_or(DEFAULT_MCP_ENABLED),
        allow_delete: raw.mcp.allow_delete.unwrap_or(DEFAULT_MCP_ALLOW_DELETE),
    })
}

fn parse_web_search_config(text: &str) -> Result<WebSearchConfig, String> {
    let raw: FileConfig = toml::from_str(text).map_err(|e| e.to_string())?;
    Ok(WebSearchConfig {
        provider: normalize_name(raw.web_search.provider, DEFAULT_WEB_SEARCH_PROVIDER),
        api_key: normalize_optional_path(raw.web_search.api_key),
        search_engine_id: normalize_optional_path(raw.web_search.search_engine_id),
        max_results: normalize_web_search_max_results(raw.web_search.max_results),
    })
}

pub fn parse_terminal_images_config(text: &str) -> Result<TerminalImagesConfig, String> {
    let raw: FileConfig = toml::from_str(text).map_err(|e| e.to_string())?;
    let mode = normalize_terminal_images_mode(raw.terminal.images.as_deref());
    let max_rows = raw
        .terminal
        .image_max_rows
        .map(|r| r.clamp(MIN_TERMINAL_IMAGE_MAX_ROWS, MAX_TERMINAL_IMAGE_MAX_ROWS))
        .unwrap_or(DEFAULT_TERMINAL_IMAGE_MAX_ROWS);
    Ok(TerminalImagesConfig { mode, max_rows })
}

fn normalize_terminal_images_mode(mode: Option<&str>) -> TerminalImagesMode {
    match mode.map(|mode| mode.trim().to_ascii_lowercase()).as_deref() {
        Some("sixel") => TerminalImagesMode::Sixel,
        Some("kitty") => TerminalImagesMode::Kitty,
        Some("iterm2") => TerminalImagesMode::Iterm2,
        Some("halfblocks") => TerminalImagesMode::Halfblocks,
        Some("off") => TerminalImagesMode::Off,
        _ => TerminalImagesMode::Auto,
    }
}

fn normalize_web_search_max_results(value: Option<usize>) -> usize {
    value
        .map(|v| v.clamp(MIN_WEB_SEARCH_MAX_RESULTS, MAX_WEB_SEARCH_MAX_RESULTS))
        .unwrap_or(DEFAULT_WEB_SEARCH_MAX_RESULTS)
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

/// Note id of the email note for the local day of `now_utc`; the host passes
/// the local `offset`.
pub fn resolve_email_note_id(
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

            [editor]
            markdown_autoformat = false
            checklist_auto_reorder = false
            autosave = false
            format_on_save = true
            vim_mode = true
            wrap = false
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
        assert!(!cfg.markdown_autoformat);
        assert!(!cfg.checklist_auto_reorder);
        assert!(!cfg.autosave);
        assert!(cfg.format_on_save);
        assert!(cfg.vim_mode);
        assert!(!cfg.wrap);
        assert!(cfg.background_tasks_enabled);
        assert_eq!(cfg.date_format, "%d.%m.%Y");
        assert_eq!(cfg.date_time_format, "%d.%m.%Y. %H:%M");
        assert_eq!(cfg.variables_autocomplete_min_chars, 5);
        assert!(!cfg.default_modules.math);
        assert!(cfg.default_modules.table);
        assert!(!cfg.default_modules.variables);
        assert!(cfg.default_modules.style);
        assert!(cfg.default_modules.cross_note);
        assert!(!cfg.security.encrypt_notes);
        assert_eq!(cfg.security.password_env, "SLATE_NOTES_PASSWORD");
    }

    #[test]
    fn uses_defaults_when_theme_values_are_missing() {
        let cfg = parse_theme_config("").expect("empty config parsed");
        assert_eq!(cfg, ThemeConfig::default());
    }

    #[test]
    fn generated_default_config_includes_all_note_modules() {
        let cfg = parse_theme_config(DEFAULT_CONFIG).expect("default config parsed");
        assert_eq!(cfg.default_modules, EditorModulesConfig::default());
        assert!(
            DEFAULT_CONFIG.contains("cross_note = true"),
            "generated config must expose the cross-note default"
        );
    }

    #[test]
    fn normalize_name_uses_fallback_on_empty() {
        assert_eq!(normalize_name(Some("   ".to_string()), "dark"), "dark");
        assert_eq!(normalize_name(None, "plain"), "plain");
    }

    #[test]
    fn parses_icon_style_with_nerd_default() {
        let cfg = parse_theme_config("[theme]\nicons = 'Unicode'").expect("config parsed");
        assert_eq!(cfg.icons, IconStyle::Unicode);
        let cfg = parse_theme_config("[theme]\nicons = 'ascii'").expect("config parsed");
        assert_eq!(cfg.icons, IconStyle::Ascii);
        let cfg = parse_theme_config("[theme]\nicons = 'bogus'").expect("config parsed");
        assert_eq!(cfg.icons, IconStyle::Nerd);
        let cfg = parse_theme_config("").expect("config parsed");
        assert_eq!(cfg.icons, IconStyle::Nerd);
    }

    #[test]
    fn defaults_vim_mode_to_false() {
        let cfg = parse_theme_config("[theme]\ncolor_scheme = 'dark'").expect("config parsed");
        assert!(cfg.markdown_autoformat);
        assert!(cfg.checklist_auto_reorder);
        assert!(cfg.autosave);
        assert!(!cfg.format_on_save);
        assert!(!cfg.vim_mode);
        assert!(cfg.wrap);
        assert!(cfg.background_tasks_enabled);
        assert_eq!(cfg.date_format, "%Y-%m-%d");
        assert_eq!(cfg.date_time_format, "%Y-%m-%d %H:%M");
        assert_eq!(cfg.accent, "auto");
        assert_eq!(cfg.variables_autocomplete_min_chars, 3);
        assert!(cfg.default_modules.math);
        assert!(cfg.default_modules.table);
        assert!(cfg.default_modules.variables);
        assert!(cfg.default_modules.style);
        assert!(cfg.default_modules.cross_note);
        assert!(!cfg.security.encrypt_notes);
        assert_eq!(cfg.security.password_env, "SLATE_NOTES_PASSWORD");
    }

    #[test]
    fn parses_markdown_autoformat_override() {
        let cfg = parse_theme_config("[editor]\nmarkdown_autoformat = false").expect("config");
        assert!(!cfg.markdown_autoformat);
    }

    #[test]
    fn reload_outside_changes_is_on_unless_turned_off() {
        assert!(
            parse_theme_config("")
                .expect("config")
                .reload_outside_changes
        );
        assert!(
            parse_theme_config(DEFAULT_CONFIG)
                .expect("config")
                .reload_outside_changes
        );
        let cfg = parse_theme_config("[editor]\nreload_outside_changes = false").expect("config");
        assert!(!cfg.reload_outside_changes);
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
    fn parses_background_tasks_enabled_override() {
        let cfg = parse_theme_config(
            r#"
            [startup]
            background_tasks_enabled = false
            "#,
        )
        .expect("config");
        assert!(!cfg.background_tasks_enabled);
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
        assert!(cfg.security.encrypt_notes);
        assert_eq!(cfg.security.password_env, "APP_NOTES_PASSWORD");
    }

    #[test]
    fn default_note_encryption_password_needs_encryption_and_env() {
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
    fn parses_daily_notes_section_and_defaults() {
        assert_eq!(
            parse_daily_notes_config("").expect("defaults"),
            DailyNotesConfig::default()
        );
        assert_eq!(
            parse_daily_notes_config(DEFAULT_CONFIG).expect("generated config"),
            DailyNotesConfig::default()
        );
        let parsed = parse_daily_notes_config(
            "[daily]\nnote_prefix = \"journal\"\ntemplate = \"## {date}\\n- \"",
        )
        .expect("daily config");
        assert_eq!(parsed.note_prefix, "journal");
        assert_eq!(parsed.template, "## {date}\n- ");
        assert_eq!(
            parse_daily_notes_config("[daily]\nnote_prefix = \"  \"")
                .expect("blank prefix")
                .note_prefix,
            "daily"
        );
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
            log_path = "/tmp/slate-log.log"
            "#,
        )
        .expect("perf config parsed");
        assert!(parsed.enabled);
        assert_eq!(parsed.log_path, "/tmp/slate-log.log");
    }

    #[test]
    fn resolve_email_note_id_uses_prefix_and_local_day() {
        let special = SpecialNotesConfig {
            email_note_prefix: "inbox-email".to_string(),
            email_rotation: "daily-local".to_string(),
        };
        let now = OffsetDateTime::from_unix_timestamp(1_714_516_200).expect("fixed ts");
        let offset = UtcOffset::from_hms(2, 0, 0).expect("offset");
        let note_id = resolve_email_note_id(&special, now, offset);
        assert_eq!(note_id, "inbox-email-2024-05-01");
    }

    #[test]
    fn mcp_is_off_unless_enabled() {
        assert_eq!(parse_mcp_config("").expect("parsed"), McpConfig::default());
        assert!(!McpConfig::default().enabled);
        let defaults = parse_mcp_config(DEFAULT_CONFIG).expect("parsed");
        assert!(!defaults.enabled);
        assert!(!defaults.allow_delete);
        let parsed =
            parse_mcp_config("[mcp]\nenabled = true\nallow_delete = true\n").expect("parsed");
        assert!(parsed.enabled);
        assert!(parsed.allow_delete);
    }

    #[test]
    fn parses_web_search_section_and_defaults() {
        let defaults = parse_web_search_config("").expect("web_search config parsed");
        assert_eq!(defaults, WebSearchConfig::default());

        let parsed = parse_web_search_config(
            r#"
            [web_search]
            provider = "google"
            api_key = "test_key"
            search_engine_id = "test_cx"
            max_results = 8
            "#,
        )
        .expect("web_search config parsed");
        assert_eq!(parsed.provider, "google");
        assert_eq!(parsed.api_key, "test_key");
        assert_eq!(parsed.search_engine_id, "test_cx");
        assert_eq!(parsed.max_results, 8);
    }

    #[test]
    fn parses_terminal_images_section_and_defaults() {
        let defaults = parse_terminal_images_config("").expect("terminal images config parsed");
        assert_eq!(defaults, TerminalImagesConfig::default());
        assert_eq!(defaults.mode, TerminalImagesMode::Auto);
        assert_eq!(defaults.max_rows, 15);

        let parsed = parse_terminal_images_config(
            r#"
            [terminal]
            images = "sixel"
            image_max_rows = 20
            "#,
        )
        .expect("terminal images parsed");
        assert_eq!(parsed.mode, TerminalImagesMode::Sixel);
        assert_eq!(parsed.max_rows, 20);

        let invalid_mode = parse_terminal_images_config(
            r#"
            [terminal]
            images = "unknown_mode"
            image_max_rows = 500
            "#,
        )
        .expect("invalid mode parsed with fallback");
        assert_eq!(invalid_mode.mode, TerminalImagesMode::Auto);
        assert_eq!(invalid_mode.max_rows, 100);
    }
}
