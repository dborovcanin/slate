mod commands;
mod config;
pub mod editor_core;
#[cfg(feature = "imap")]
mod imap;
mod startup_log;
mod storage;
mod terminal;

#[cfg(feature = "imap")]
use app_core::storage::Db;
use app_core::AppCore;
use std::io::IsTerminal as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};
#[cfg(feature = "imap")]
use std::thread;
#[cfg(feature = "imap")]
use std::time::Duration;
use ulid::Ulid;

#[cfg(test)]
use app_core::storage::NoteAccessMode;

use terminal::TerminalOptions;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    Terminal,
    /// Open today's daily note.
    Today,
    ImapSync,
    Append,
    /// Append a timestamped entry to today's daily note; `None` reads stdin.
    Capture(Option<String>),
}

pub(crate) fn note_id_for_file(path: &Path) -> String {
    app_core::note_sources::note_id_for_markdown_file(path)
}

pub(crate) fn file_path_from_note_id(note_id: &str) -> Option<PathBuf> {
    app_core::note_sources::markdown_file_path_from_note_id(note_id)
}

fn resolve_text_file_path(raw: &str) -> Result<PathBuf, String> {
    let cwd =
        std::env::current_dir().map_err(|e| format!("Failed to resolve current directory: {e}"))?;
    app_core::note_sources::resolve_markdown_file_path(raw, &cwd)
}

fn stdin_is_tty() -> bool {
    std::io::stdin().is_terminal()
}

fn print_help() {
    println!(
        "slate usage:
  slate [--new] [--id <note-id>] [--list]
  slate <file>
  slate today
  slate capture [text...]
  slate append [--id <note-id>]
  slate imap-sync

Modes:
  <file>      Open a text/code file
  today       Open today's daily note (created from [daily] template)
  capture     Add a timestamped entry to today's daily note and exit
              (text from arguments, or piped stdin)
  append      Append stdin to a note and exit
  imap-sync   Pull messages from configured IMAP inbox once

Options:
  --new, -n   Create and edit a new note
  --id <id>   Open a specific note id
  --list, -l  List notes and exit

Append mode:
  cmd | slate append
  cmd | slate append --id <note-id>"
    );
}

fn parse_args(args: &[String], stdin_tty: bool) -> Result<(Mode, TerminalOptions), String> {
    let mut force_imap = false;
    let mut force_append = false;
    let mut force_today = false;
    let mut capture: Option<Option<String>> = None;
    let mut saw_note_flag = false;
    let mut opts = TerminalOptions::default();
    let mut startup_file: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_help();
                return Err(String::new());
            }
            "imap-sync" | "--imap-sync" => force_imap = true,
            "today" => force_today = true,
            "capture" => {
                // Everything after `capture` is the text to capture.
                let text = args[i + 1..].join(" ");
                capture = Some((!text.trim().is_empty()).then_some(text));
                break;
            }
            "append" | "--append" => force_append = true,
            "--new" | "-n" => {
                opts.create_new = true;
                saw_note_flag = true;
            }
            "--list" | "-l" => {
                opts.list_only = true;
                saw_note_flag = true;
            }
            "--id" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--id requires a note id".to_string())?
                    .clone();
                opts.note_id = Some(value);
                i += 1;
            }
            candidate => {
                if !candidate.starts_with('-') && startup_file.is_none() {
                    if let Ok(path) = resolve_text_file_path(candidate) {
                        startup_file = Some(path);
                        i += 1;
                        continue;
                    }
                }
                return Err(format!(
                    "Unknown argument: {candidate}. Use --help for usage."
                ));
            }
        }
        i += 1;
    }

    if startup_file.is_some() {
        if force_append || force_imap {
            return Err("Cannot combine file open with append or imap-sync flags".to_string());
        }
        if saw_note_flag || opts.note_id.is_some() {
            return Err("Cannot combine file open with note selection flags".to_string());
        }
    }
    if force_append && (force_imap || saw_note_flag) {
        return Err("Cannot combine append with imap-sync or note selection flags".to_string());
    }
    if force_imap && (saw_note_flag || opts.note_id.is_some()) {
        return Err("Cannot combine imap-sync with note selection flags".to_string());
    }

    let subcommands = [force_append, force_imap, force_today, capture.is_some()]
        .iter()
        .filter(|set| **set)
        .count();
    if subcommands > 1 {
        return Err("Use only one of append, imap-sync, today, capture".to_string());
    }
    if (force_today || capture.is_some())
        && (saw_note_flag || opts.note_id.is_some() || startup_file.is_some())
    {
        return Err("today and capture cannot be combined with note selection".to_string());
    }
    if let Some(text) = capture {
        if text.is_none() && stdin_tty {
            return Err(
                "Nothing to capture: pass text (slate capture buy milk) or pipe it in.".to_string(),
            );
        }
        return Ok((Mode::Capture(text), opts));
    }

    let mode = if force_append {
        Mode::Append
    } else if force_imap {
        Mode::ImapSync
    } else if force_today {
        Mode::Today
    } else {
        Mode::Terminal
    };

    if matches!(mode, Mode::Terminal | Mode::Today) && !stdin_tty && !opts.list_only {
        return Err(
            "Terminal edit mode requires a TTY. Run inside a terminal emulator.".to_string(),
        );
    }
    if mode == Mode::Append && stdin_tty {
        return Err(
            "Append mode expects piped stdin (example: cmd | slate append [--id <note-id>])."
                .to_string(),
        );
    }

    if let Some(path) = startup_file {
        opts.note_id = Some(note_id_for_file(&path));
    }

    Ok((mode, opts))
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

/// Resolves today's daily note (creating it when missing) and returns its id.
fn ensure_today_note(
    db: &app_core::storage::Db,
    theme: &config::ThemeConfig,
) -> Result<String, String> {
    let stamp = terminal::local_stamp();
    let label = terminal::daily_date_label(stamp, &theme.date_format);
    let daily = config::load_daily_notes_config();
    Ok(app_core::daily::ensure_daily_note(db, &daily, stamp, &label)?.id)
}

fn run_capture(text: Option<String>, theme: &config::ThemeConfig) -> Result<(), String> {
    let text = match text {
        Some(text) => text,
        None => {
            let mut input = String::new();
            std::io::stdin()
                .read_to_string(&mut input)
                .map_err(|e| format!("Failed reading stdin: {e}"))?;
            input
        }
    };
    let core = AppCore::open_default()?;
    let stamp = terminal::local_stamp();
    let label = terminal::daily_date_label(stamp, &theme.date_format);
    let daily = config::load_daily_notes_config();
    let note = app_core::daily::capture_to_daily_note(core.db(), &daily, stamp, &label, &text)?;
    println!("Captured to {}", note.id);
    Ok(())
}

fn run_terminal(opts: &TerminalOptions, theme: &config::ThemeConfig) -> Result<(), String> {
    if let Err(err) = config::ensure_config_file() {
        eprintln!("Config: {err}");
    }
    let core = AppCore::open_default()?;
    maybe_start_background_imap_sync(theme.background_tasks_enabled, core.db().clone());
    terminal::run_terminal_session(core.db(), theme, opts, core.cross_note_var_index_arc())?;
    // Apply any staged restore that was not handled in-session (e.g. after a crash).
    drop(core);
    if let Err(e) = crate::commands::backup::apply_staged_restore_if_pending() {
        eprintln!("slate: staged restore cleanup failed: {e}");
    }
    Ok(())
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
    let _ = terminal::send_system_notification(title, &body);
}

#[cfg(feature = "imap")]
fn maybe_start_background_imap_sync(background_tasks_enabled: bool, db: Db) {
    if !background_tasks_enabled {
        return;
    }
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
        if let Ok(summary) = imap::run_imap_sync_silent(imap_cfg.clone(), special.clone(), &db) {
            notify_imap_new_mail(summary.appended, &imap_cfg, &special);
        }
        thread::sleep(Duration::from_secs(poll_seconds));
    });
}

#[cfg(not(feature = "imap"))]
fn maybe_start_background_imap_sync(_background_tasks_enabled: bool, _db: app_core::storage::Db) {}

#[cfg(not(feature = "imap"))]
fn run_imap_sync() -> Result<(), String> {
    Err("IMAP support not compiled in (missing 'imap' feature)".to_string())
}

pub fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = config::load_theme_config();
    let result = match parse_args(&args, stdin_is_tty()) {
        Ok((Mode::ImapSync, _)) => run_imap_sync(),
        Ok((Mode::Append, opts)) => run_append(opts.note_id.as_deref()),
        Ok((Mode::Capture(text), _)) => run_capture(text, &cfg),
        Ok((Mode::Today, mut opts)) => AppCore::open_default()
            .and_then(|core| ensure_today_note(core.db(), &cfg))
            .and_then(|id| {
                opts.note_id = Some(id);
                opts.open_at_end = true;
                run_terminal(&opts, &cfg)
            }),
        Ok((Mode::Terminal, opts)) => run_terminal(&opts, &cfg),
        Err(err) => {
            if err.is_empty() {
                std::process::exit(0);
            }
            eprintln!("{err}");
            std::process::exit(2);
        }
    };
    if let Err(err) = result {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn parse_defaults_to_terminal() {
        let (mode, opts) = parse_args(&[], true).expect("parsed");
        assert_eq!(mode, Mode::Terminal);
        assert_eq!(opts, TerminalOptions::default());
    }

    #[test]
    fn parse_collects_terminal_options() {
        let (mode, opts) = parse_args(&args(&["--new", "--id", "abc"]), true).expect("parsed");
        assert_eq!(mode, Mode::Terminal);
        assert!(opts.create_new);
        assert_eq!(opts.note_id.as_deref(), Some("abc"));
    }

    #[test]
    fn parse_allows_list_without_tty() {
        let (mode, opts) = parse_args(&args(&["--list"]), false).expect("parsed");
        assert_eq!(mode, Mode::Terminal);
        assert!(opts.list_only);
    }

    #[test]
    fn parse_supports_imap_mode() {
        let (mode, _) = parse_args(&args(&["imap-sync"]), true).expect("parsed");
        assert_eq!(mode, Mode::ImapSync);
    }

    #[test]
    fn parse_supports_append_mode_with_note_id() {
        let (mode, opts) = parse_args(&args(&["--id", "n1", "append"]), false).expect("parsed");
        assert_eq!(mode, Mode::Append);
        assert_eq!(opts.note_id.as_deref(), Some("n1"));
        assert!(!opts.list_only);
        assert!(!opts.create_new);
    }

    #[test]
    fn parse_rejects_append_with_note_selection_flags() {
        let err = parse_args(&args(&["append", "--new"]), false).expect_err("expected err");
        assert!(err.contains("Cannot combine append"));
    }

    #[test]
    fn parse_rejects_imap_with_note_selection_flags() {
        let err = parse_args(&args(&["imap-sync", "--list"]), true).expect_err("expected err");
        assert!(err.contains("Cannot combine imap-sync"));
    }

    #[test]
    fn parse_rejects_terminal_edit_without_tty() {
        let err = parse_args(&[], false).expect_err("expected err");
        assert!(err.contains("requires a TTY"));
    }

    #[test]
    fn parse_rejects_append_without_piped_stdin() {
        let err = parse_args(&args(&["append"]), true).expect_err("expected err");
        assert!(err.contains("expects piped stdin"));
    }

    #[test]
    fn parse_opens_markdown_file_as_note() {
        let (mode, opts) = parse_args(&args(&["notes.md"]), true).expect("parsed");
        assert_eq!(mode, Mode::Terminal);
        let expected_path = std::env::current_dir().expect("cwd").join("notes.md");
        let expected_note_id = note_id_for_file(&expected_path);
        assert_eq!(opts.note_id.as_deref(), Some(expected_note_id.as_str()));
    }

    #[test]
    fn parse_rejects_file_with_note_selection_flags() {
        let err = parse_args(&args(&["notes.md", "--new"]), true).expect_err("expected err");
        assert!(err.contains("Cannot combine file open"));
    }

    #[test]
    fn parse_today_opens_terminal_mode() {
        let (mode, _) = parse_args(&args(&["today"]), true).expect("parsed");
        assert_eq!(mode, Mode::Today);
        let err = parse_args(&args(&["today"]), false).expect_err("needs tty");
        assert!(err.contains("requires a TTY"));
    }

    #[test]
    fn parse_capture_takes_the_remaining_args_as_text() {
        let (mode, _) =
            parse_args(&args(&["capture", "call", "--new", "Ana"]), true).expect("parsed");
        assert_eq!(mode, Mode::Capture(Some("call --new Ana".to_string())));
    }

    #[test]
    fn parse_capture_without_text_reads_piped_stdin() {
        let (mode, _) = parse_args(&args(&["capture"]), false).expect("parsed");
        assert_eq!(mode, Mode::Capture(None));
        let err = parse_args(&args(&["capture"]), true).expect_err("tty without text");
        assert!(err.contains("Nothing to capture"));
    }

    #[test]
    fn parse_rejects_combining_subcommands_or_note_selection() {
        assert!(parse_args(&args(&["append", "capture", "x"]), false).is_err());
        assert!(parse_args(&args(&["today", "--new"]), true).is_err());
        assert!(parse_args(&args(&["--id", "n1", "capture", "x"]), true).is_err());
    }

    #[test]
    fn markdown_file_note_roundtrip_reads_file_contents() {
        let db_path = std::env::temp_dir().join(format!("slate-mdfile-test-{}.db", Ulid::new()));
        let db = app_core::storage::Db::open(db_path.clone()).expect("db opens");
        let note_sources = app_core::note_sources::NoteSourceService::new(db.clone());
        let path = std::env::temp_dir().join(format!("slate-mdfile-note-{}.md", Ulid::new()));
        fs::write(&path, "hello\nworld").expect("seed markdown file");
        let note_id = note_id_for_file(&path);

        let note = note_sources
            .open_note_by_id(&note_id)
            .expect("load markdown note")
            .expect("note exists");
        assert_eq!(note.id, note_id);
        assert_eq!(note.body, "hello\nworld");
        assert_eq!(note.access_mode, NoteAccessMode::None);
        assert!(note.is_unlocked);
        assert!(!note.updated_at.is_empty());

        let _ = fs::remove_file(path);
        drop(db);
        let _ = fs::remove_file(db_path);
    }

    #[test]
    fn markdown_file_revision_is_none_when_file_missing() {
        let db_path = std::env::temp_dir().join(format!("slate-mdfile-test-{}.db", Ulid::new()));
        let db = app_core::storage::Db::open(db_path.clone()).expect("db opens");
        let note_sources = app_core::note_sources::NoteSourceService::new(db.clone());
        let path = std::env::temp_dir().join(format!("slate-missing-mdfile-{}.md", Ulid::new()));
        let note_id = note_id_for_file(&path);
        let revision = note_sources
            .get_note_revision_by_id(&note_id)
            .expect("revision lookup should succeed");
        assert_eq!(revision, None);
        drop(db);
        let _ = fs::remove_file(db_path);
    }
}
