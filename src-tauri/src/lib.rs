mod commands;
mod config;
pub mod editor_core;
#[cfg(feature = "imap")]
mod imap;
mod ipc;
mod startup_log;
mod storage;
#[cfg(unix)]
mod terminal;

use app_core::AppCore;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ipc::server;
use std::io::IsTerminal as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use ulid::Ulid;

#[cfg(unix)]
use terminal::TerminalOptions;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Gui,
    #[cfg(unix)]
    Terminal,
    ImapSync,
    Append,
}

const MARKDOWN_NOTE_ID_PREFIX: &str = "mdfile:";

pub(crate) struct StartupMarkdownFileState {
    startup_file: Mutex<Option<PathBuf>>,
}

impl StartupMarkdownFileState {
    fn new(startup_file: Option<PathBuf>) -> Self {
        Self {
            startup_file: Mutex::new(startup_file),
        }
    }

    pub(crate) fn take_startup_file(&self) -> Option<PathBuf> {
        self.startup_file.lock().ok()?.take()
    }
}

pub(crate) fn note_id_for_markdown_file(path: &Path) -> String {
    let encoded = URL_SAFE_NO_PAD.encode(path.to_string_lossy().as_bytes());
    format!("{MARKDOWN_NOTE_ID_PREFIX}{encoded}")
}

pub(crate) fn markdown_file_path_from_note_id(note_id: &str) -> Option<PathBuf> {
    let encoded = note_id.strip_prefix(MARKDOWN_NOTE_ID_PREFIX)?;
    let decoded = URL_SAFE_NO_PAD.decode(encoded).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let path = PathBuf::from(decoded);
    if !path.is_absolute() || !is_supported_markdown_path(&path) {
        return None;
    }
    Some(path)
}

fn is_supported_markdown_path(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "md" | "markdown" | "mdown" | "mkd"
    )
}

fn resolve_markdown_file_path(raw: &str) -> Result<PathBuf, String> {
    let candidate = PathBuf::from(raw);
    let absolute = if candidate.is_absolute() {
        candidate
    } else {
        std::env::current_dir()
            .map_err(|e| format!("Failed to resolve current directory: {e}"))?
            .join(candidate)
    };

    if !is_supported_markdown_path(&absolute) {
        return Err(format!(
            "Unsupported file extension for '{}'; expected markdown (.md/.markdown/.mdown/.mkd)",
            absolute.display()
        ));
    }

    if absolute.exists() {
        std::fs::canonicalize(&absolute).map_err(|e| {
            format!(
                "Failed to canonicalize markdown file '{}': {e}",
                absolute.display()
            )
        })
    } else {
        Ok(absolute)
    }
}

fn stdin_is_tty() -> bool {
    std::io::stdin().is_terminal()
}

#[cfg(not(unix))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TerminalOptions {
    create_new: bool,
    note_id: Option<String>,
    list_only: bool,
}

fn run_gui(startup_markdown_file: Option<PathBuf>) -> Result<(), String> {
    let core = AppCore::open_default()?;
    if let Err(err) = config::ensure_config_file() {
        eprintln!("Config: {err}");
    }
    maybe_start_background_imap_sync();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(core)
        .manage(StartupMarkdownFileState::new(startup_markdown_file))
        .invoke_handler(tauri::generate_handler![
            commands::notes::get_or_create_note,
            commands::notes::save_note,
            commands::notes::get_note,
            commands::notes::create_note,
            commands::notes::list_notes_meta,
            commands::notes::search_notes_content,
            commands::notes::rebuild_note_search_index,
            commands::notes::get_note_meta,
            commands::notes::get_note_revision,
            commands::notes::delete_note,
            commands::notes::set_note_modules,
            commands::notes::lock_note_access,
            commands::notes::unlock_note_access,
            commands::notes::encrypt_note,
            commands::notes::decrypt_note,
            commands::reminders::list_note_reminders,
            commands::reminders::upsert_note_reminder,
            commands::reminders::delete_note_reminder,
            commands::reminders::move_note_reminder_line,
            commands::reminders::mark_note_reminder_notified,
            commands::reminders::send_system_notification,
            commands::calc::evaluate_lines,
            commands::calc::evaluate_note_context,
            commands::calc::sync_note_lines,
            commands::calc::evaluate_note_context_delta,
            commands::perf::append_startup_log,
            commands::config::get_theme_config,
            commands::config::get_runtime_flags,
            commands::export::export_to_file,
            commands::clipboard::read_clipboard_text,
        ])
        .setup(|app| {
            server::start_ipc_server(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .map_err(|e| format!("Error while running tauri application: {e}"))?;

    server::cleanup_socket();
    Ok(())
}

fn print_help() {
    println!(
        "slate usage:
  slate [--gui|--terminal] [--new] [--id <note-id>] [--list]
  slate <file.md>
  slate append [--id <note-id>]
  slate imap-sync

Modes:
  --gui       Force Tauri GUI mode
  --terminal  Force terminal editor mode (no window UI, Unix only)
  <file.md>   Open a markdown file in GUI mode and sync edits back to disk
  append      Append stdin to a note and exit
  imap-sync   Pull messages from configured IMAP inbox once

Terminal options:
  --new       Create and edit a new note
  --id <id>   Open a specific note id
  --list      List notes and exit

Append mode:
  cmd | slate append
  cmd | slate append --id <note-id>

When no explicit mode is passed:
  - terminal mode is used if [editor].terminal_mode = true and stdin is a TTY
  - otherwise GUI mode is used."
    );
}

#[cfg(unix)]
fn parse_args(
    args: &[String],
    config_terminal_mode: bool,
    stdin_tty: bool,
) -> Result<(Mode, TerminalOptions, Option<PathBuf>), String> {
    let mut force_gui = false;
    let mut force_terminal = false;
    let mut force_imap = false;
    let mut force_append = false;
    let mut opts = TerminalOptions::default();
    let mut saw_id_flag = false;
    let mut startup_markdown_file: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_help();
                return Err(String::new());
            }
            "--gui" => force_gui = true,
            "--terminal" | "-t" => force_terminal = true,
            "imap-sync" | "--imap-sync" => force_imap = true,
            "append" | "--append" => force_append = true,
            "--new" => {
                opts.create_new = true;
                force_terminal = true;
            }
            "--list" => {
                opts.list_only = true;
                force_terminal = true;
            }
            "--id" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--id requires a note id".to_string())?
                    .clone();
                opts.note_id = Some(value);
                saw_id_flag = true;
                i += 1;
            }
            candidate => {
                if !candidate.starts_with('-') && startup_markdown_file.is_none() {
                    match resolve_markdown_file_path(candidate) {
                        Ok(path) => {
                            startup_markdown_file = Some(path);
                            force_gui = true;
                            i += 1;
                            continue;
                        }
                        Err(_) => {}
                    }
                }
                let unknown = candidate;
                return Err(format!(
                    "Unknown argument: {unknown}. Use --help for usage."
                ));
            }
        }
        i += 1;
    }

    if saw_id_flag && !force_append {
        force_terminal = true;
    }
    if startup_markdown_file.is_some() {
        if force_append || force_imap || force_terminal {
            return Err(
                "Cannot combine markdown file open with append, imap-sync, or terminal flags"
                    .to_string(),
            );
        }
        if opts.create_new || opts.list_only || saw_id_flag {
            return Err(
                "Cannot combine markdown file open with terminal note selection flags".to_string(),
            );
        }
    }

    if force_append && (force_gui || force_imap || force_terminal) {
        return Err("Cannot combine append with GUI, terminal, or imap-sync flags".to_string());
    }
    if force_imap && (force_gui || force_terminal) {
        return Err("Cannot combine imap-sync with GUI or terminal flags".to_string());
    }
    if force_gui && force_terminal {
        return Err("Cannot combine --gui with terminal flags".to_string());
    }

    let mode = if force_append {
        Mode::Append
    } else if force_imap {
        Mode::ImapSync
    } else if force_gui {
        Mode::Gui
    } else if force_terminal {
        Mode::Terminal
    } else if config_terminal_mode && stdin_tty {
        Mode::Terminal
    } else {
        Mode::Gui
    };

    if mode == Mode::Terminal && !stdin_tty && !opts.list_only {
        return Err(
            "Terminal edit mode requires a TTY. Run inside a terminal emulator or pass --gui."
                .to_string(),
        );
    }
    if mode == Mode::Append && stdin_tty {
        return Err(
            "Append mode expects piped stdin (example: cmd | slate append [--id <note-id>])."
                .to_string(),
        );
    }

    Ok((mode, opts, startup_markdown_file))
}

#[cfg(not(unix))]
fn parse_args(
    args: &[String],
    _config_terminal_mode: bool,
    stdin_tty: bool,
) -> Result<(Mode, TerminalOptions, Option<PathBuf>), String> {
    let mut imap = false;
    let mut append = false;
    let mut opts = TerminalOptions::default();
    let mut startup_markdown_file: Option<PathBuf> = None;
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_help();
                return Err(String::new());
            }
            "--gui" => {}
            "imap-sync" | "--imap-sync" => imap = true,
            "append" | "--append" => append = true,
            "--id" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--id requires a note id".to_string())?
                    .clone();
                opts.note_id = Some(value);
                i += 1;
            }
            candidate => {
                if !candidate.starts_with('-') && startup_markdown_file.is_none() {
                    match resolve_markdown_file_path(candidate) {
                        Ok(path) => {
                            startup_markdown_file = Some(path);
                            i += 1;
                            continue;
                        }
                        Err(_) => {}
                    }
                }
                let unknown = candidate;
                return Err(format!(
                    "Unknown argument: {unknown}. Use --help for usage."
                ));
            }
        }
        i += 1;
    }
    if startup_markdown_file.is_some() && (append || imap || opts.note_id.is_some()) {
        return Err(
            "Cannot combine markdown file open with append, imap-sync, or --id".to_string(),
        );
    }
    if append && imap {
        return Err("Cannot combine append with imap-sync".to_string());
    }
    if append && stdin_tty {
        return Err(
            "Append mode expects piped stdin (example: cmd | slate append [--id <note-id>])."
                .to_string(),
        );
    }
    if append {
        return Ok((Mode::Append, opts, None));
    }
    if imap {
        return Ok((Mode::ImapSync, opts, None));
    }
    Ok((Mode::Gui, opts, startup_markdown_file))
}

fn select_append_note_id(
    db: &app_core::storage::Db,
    requested_id: Option<&str>,
) -> Result<String, String> {
    if let Some(id) = requested_id {
        return Ok(id.to_string());
    }

    let special = crate::config::load_special_notes_config();
    if let Some(note) = db.get_most_recent_note_excluding_prefix(&special.email_note_prefix)? {
        return Ok(note.id);
    }

    if let Some(note) = db.get_most_recent_note()? {
        return Ok(note.id);
    }

    Ok(Ulid::new().to_string())
}

fn run_append(note_id: Option<&str>) -> Result<(), String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("Failed reading stdin: {e}"))?;
    if input.is_empty() {
        return Err("Append mode received empty stdin input".to_string());
    }

    let core = AppCore::open_default()?;
    let target_id = select_append_note_id(core.db(), note_id)?;
    core.db().append_note_body(&target_id, &input)?;

    let bytes = input.as_bytes().len();
    println!(
        "Appended {bytes} byte{} to note {target_id}",
        if bytes == 1 { "" } else { "s" }
    );
    Ok(())
}

#[cfg(unix)]
fn run_terminal(opts: &TerminalOptions, theme: &config::ThemeConfig) -> Result<(), String> {
    if let Err(err) = config::ensure_config_file() {
        eprintln!("Config: {err}");
    }
    maybe_start_background_imap_sync();
    let core = AppCore::open_default()?;
    terminal::run_terminal_session(core.db(), theme, opts)
}

#[cfg(feature = "imap")]
fn run_imap_sync() -> Result<(), String> {
    if let Err(err) = config::ensure_config_file() {
        eprintln!("Config: {err}");
    }
    let imap_cfg = config::load_imap_config();
    let special = config::load_special_notes_config();
    imap::run_imap_sync(imap_cfg, special).map(|_| ())
}

#[cfg(feature = "imap")]
fn notify_imap_new_mail(
    appended: usize,
    imap_cfg: &config::ImapConfig,
    special: &config::SpecialNotesConfig,
) {
    if appended == 0 {
        return;
    }
    let note_id = config::resolve_email_note_id(special, time::OffsetDateTime::now_utc());
    let title = if appended == 1 {
        "New email in Slate"
    } else {
        "New emails in Slate"
    };
    let body = format!(
        "{} new email{} synced from {} to {}",
        appended,
        if appended == 1 { "" } else { "s" },
        imap_cfg.folder,
        note_id
    );
    let _ = commands::reminders::send_system_notification(title.to_string(), body);
}

#[cfg(feature = "imap")]
fn maybe_start_background_imap_sync() {
    let imap_cfg = config::load_imap_config();
    if !imap_cfg.auto_sync_on_startup {
        return;
    }
    if let Err(err) = imap_cfg.validate_runtime() {
        eprintln!("IMAP background sync disabled: {err}");
        return;
    }
    let special = config::load_special_notes_config();
    let poll_seconds = imap_cfg.poll_seconds.max(10);

    thread::spawn(move || loop {
        if let Ok(summary) = imap::run_imap_sync_silent(imap_cfg.clone(), special.clone()) {
            notify_imap_new_mail(summary.appended, &imap_cfg, &special);
        }
        thread::sleep(Duration::from_secs(poll_seconds));
    });
}

#[cfg(not(feature = "imap"))]
fn maybe_start_background_imap_sync() {}

#[cfg(not(feature = "imap"))]
fn run_imap_sync() -> Result<(), String> {
    Err("IMAP support not compiled in (missing 'imap' feature)".to_string())
}

pub fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = config::load_theme_config();
    match parse_args(&args, cfg.terminal_mode, stdin_is_tty()) {
        Ok((Mode::Gui, _, startup_markdown_file)) => {
            if let Err(err) = run_gui(startup_markdown_file) {
                eprintln!("{err}");
                std::process::exit(1);
            }
        }
        Ok((Mode::ImapSync, _, _)) => {
            if let Err(err) = run_imap_sync() {
                eprintln!("{err}");
                std::process::exit(1);
            }
        }
        Ok((Mode::Append, opts, _)) => {
            if let Err(err) = run_append(opts.note_id.as_deref()) {
                eprintln!("{err}");
                std::process::exit(1);
            }
        }
        #[cfg(unix)]
        Ok((Mode::Terminal, opts, _)) => {
            if let Err(err) = run_terminal(&opts, &cfg) {
                eprintln!("{err}");
                std::process::exit(1);
            }
        }
        Err(err) => {
            if err.is_empty() {
                std::process::exit(0);
            }
            eprintln!("{err}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    #[test]
    fn parse_defaults_to_gui_when_terminal_not_enabled() {
        let (mode, opts, startup_file) = parse_args(&[], false, true).expect("parsed");
        assert_eq!(mode, Mode::Gui);
        assert_eq!(opts, TerminalOptions::default());
        assert!(startup_file.is_none());
    }

    #[test]
    fn parse_uses_terminal_when_enabled_in_config_and_tty() {
        let (mode, _, _) = parse_args(&[], true, true).expect("parsed");
        assert_eq!(mode, Mode::Terminal);
    }

    #[test]
    fn parse_forces_gui_over_config() {
        let (mode, _, _) = parse_args(&["--gui".to_string()], true, true).expect("parsed");
        assert_eq!(mode, Mode::Gui);
    }

    #[test]
    fn parse_collects_terminal_options() {
        let (mode, opts, _) = parse_args(
            &[
                "--terminal".to_string(),
                "--new".to_string(),
                "--id".to_string(),
                "abc".to_string(),
            ],
            false,
            true,
        )
        .expect("parsed");
        assert_eq!(mode, Mode::Terminal);
        assert!(opts.create_new);
        assert_eq!(opts.note_id.as_deref(), Some("abc"));
    }

    #[test]
    fn parse_allows_terminal_list_without_tty() {
        let (mode, opts, _) = parse_args(&["--list".to_string()], false, false).expect("parsed");
        assert_eq!(mode, Mode::Terminal);
        assert!(opts.list_only);
    }

    #[test]
    fn parse_supports_imap_mode() {
        let (mode, _, _) = parse_args(&["imap-sync".to_string()], false, true).expect("parsed");
        assert_eq!(mode, Mode::ImapSync);
    }

    #[test]
    fn parse_supports_append_mode_with_note_id() {
        let (mode, opts, _) = parse_args(
            &["--id".to_string(), "n1".to_string(), "append".to_string()],
            false,
            false,
        )
        .expect("parsed");
        assert_eq!(mode, Mode::Append);
        assert_eq!(opts.note_id.as_deref(), Some("n1"));
        assert!(!opts.list_only);
        assert!(!opts.create_new);
    }

    #[test]
    fn parse_rejects_append_with_terminal_flags() {
        let err = parse_args(
            &["append".to_string(), "--terminal".to_string()],
            false,
            false,
        )
        .expect_err("expected err");
        assert!(err.contains("Cannot combine append"));
    }

    #[test]
    fn parse_rejects_imap_with_terminal_flags() {
        let err = parse_args(
            &["imap-sync".to_string(), "--terminal".to_string()],
            false,
            true,
        )
        .expect_err("expected err");
        assert!(err.contains("Cannot combine imap-sync"));
    }

    #[test]
    fn parse_rejects_terminal_edit_without_tty() {
        let err = parse_args(&["--terminal".to_string()], false, false).expect_err("expected err");
        assert!(err.contains("requires a TTY"));
    }

    #[test]
    fn parse_rejects_append_without_piped_stdin() {
        let err = parse_args(&["append".to_string()], false, true).expect_err("expected err");
        assert!(err.contains("expects piped stdin"));
    }

    #[test]
    fn parse_supports_opening_markdown_file_in_gui_mode() {
        let (mode, opts, startup_file) =
            parse_args(&["notes.md".to_string()], false, true).expect("parsed");
        assert_eq!(mode, Mode::Gui);
        assert_eq!(opts, TerminalOptions::default());
        assert_eq!(
            startup_file,
            Some(std::env::current_dir().expect("cwd").join("notes.md"))
        );
    }

    #[test]
    fn parse_rejects_markdown_file_with_terminal_flag() {
        let err = parse_args(
            &["--terminal".to_string(), "notes.md".to_string()],
            false,
            true,
        )
        .expect_err("expected err");
        assert!(err.contains("Cannot combine markdown file open"));
    }
}
