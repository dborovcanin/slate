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
use ipc::server;
use std::io::IsTerminal as _;
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use terminal::TerminalOptions;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Gui,
    #[cfg(unix)]
    Terminal,
    ImapSync,
}

fn stdin_is_tty() -> bool {
    std::io::stdin().is_terminal()
}

fn run_gui() -> Result<(), String> {
    let core = AppCore::open_default()?;
    if let Err(err) = config::ensure_config_file() {
        eprintln!("Config: {err}");
    }
    maybe_start_background_imap_sync();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(core)
        .invoke_handler(tauri::generate_handler![
            commands::notes::get_or_create_note,
            commands::notes::save_note,
            commands::notes::get_note,
            commands::notes::create_note,
            commands::notes::list_notes,
            commands::notes::list_notes_meta,
            commands::notes::get_note_meta,
            commands::notes::delete_note,
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
        "note usage:
  note [--gui|--terminal] [--new] [--id <note-id>] [--list]
  note imap-sync

Modes:
  --gui       Force Tauri GUI mode
  --terminal  Force terminal editor mode (no window UI, Unix only)
  imap-sync   Pull messages from configured IMAP inbox once

Terminal options:
  --new       Create and edit a new note
  --id <id>   Open a specific note id
  --list      List notes and exit

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
) -> Result<(Mode, TerminalOptions), String> {
    let mut force_gui = false;
    let mut force_terminal = false;
    let mut force_imap = false;
    let mut opts = TerminalOptions::default();

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
                force_terminal = true;
                i += 1;
            }
            unknown => {
                return Err(format!(
                    "Unknown argument: {unknown}. Use --help for usage."
                ));
            }
        }
        i += 1;
    }

    if force_imap && (force_gui || force_terminal) {
        return Err("Cannot combine imap-sync with GUI or terminal flags".to_string());
    }
    if force_gui && force_terminal {
        return Err("Cannot combine --gui with terminal flags".to_string());
    }

    let mode = if force_imap {
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

    Ok((mode, opts))
}

#[cfg(not(unix))]
fn parse_args(
    args: &[String],
    _config_terminal_mode: bool,
    _stdin_tty: bool,
) -> Result<(Mode, ()), String> {
    let mut imap = false;
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => {
                print_help();
                return Err(String::new());
            }
            "--gui" => {}
            "imap-sync" | "--imap-sync" => imap = true,
            unknown => {
                return Err(format!(
                    "Unknown argument: {unknown}. Use --help for usage."
                ));
            }
        }
    }
    if imap {
        Ok((Mode::ImapSync, ()))
    } else {
        Ok((Mode::Gui, ()))
    }
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
        Ok((Mode::Gui, _)) => {
            if let Err(err) = run_gui() {
                eprintln!("{err}");
                std::process::exit(1);
            }
        }
        Ok((Mode::ImapSync, _)) => {
            if let Err(err) = run_imap_sync() {
                eprintln!("{err}");
                std::process::exit(1);
            }
        }
        #[cfg(unix)]
        Ok((Mode::Terminal, opts)) => {
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
        let (mode, opts) = parse_args(&[], false, true).expect("parsed");
        assert_eq!(mode, Mode::Gui);
        assert_eq!(opts, TerminalOptions::default());
    }

    #[test]
    fn parse_uses_terminal_when_enabled_in_config_and_tty() {
        let (mode, _) = parse_args(&[], true, true).expect("parsed");
        assert_eq!(mode, Mode::Terminal);
    }

    #[test]
    fn parse_forces_gui_over_config() {
        let (mode, _) = parse_args(&["--gui".to_string()], true, true).expect("parsed");
        assert_eq!(mode, Mode::Gui);
    }

    #[test]
    fn parse_collects_terminal_options() {
        let (mode, opts) = parse_args(
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
        let (mode, opts) = parse_args(&["--list".to_string()], false, false).expect("parsed");
        assert_eq!(mode, Mode::Terminal);
        assert!(opts.list_only);
    }

    #[test]
    fn parse_supports_imap_mode() {
        let (mode, _) = parse_args(&["imap-sync".to_string()], false, true).expect("parsed");
        assert_eq!(mode, Mode::ImapSync);
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
}
