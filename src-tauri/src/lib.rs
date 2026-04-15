mod commands;
mod config;
pub mod editor_core;
mod ipc;
mod startup_log;
mod storage;
#[cfg(unix)]
mod terminal;

use app_core::AppCore;
use ipc::server;
use std::io::IsTerminal as _;

#[cfg(unix)]
use terminal::TerminalOptions;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Gui,
    #[cfg(unix)]
    Terminal,
}

fn stdin_is_tty() -> bool {
    std::io::stdin().is_terminal()
}

fn run_gui() -> Result<(), String> {
    let core = AppCore::open_default()?;
    if let Err(err) = config::ensure_config_file() {
        eprintln!("Config: {err}");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(core)
        .invoke_handler(tauri::generate_handler![
            commands::notes::get_or_create_note,
            commands::notes::save_note,
            commands::notes::create_note,
            commands::notes::list_notes,
            commands::notes::delete_note,
            commands::reminders::list_note_reminders,
            commands::reminders::upsert_note_reminder,
            commands::reminders::delete_note_reminder,
            commands::reminders::move_note_reminder_line,
            commands::reminders::mark_note_reminder_notified,
            commands::reminders::send_system_notification,
            commands::calc::evaluate_lines,
            commands::calc::evaluate_note_context,
            commands::perf::append_startup_log,
            commands::config::get_theme_config,
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

Modes:
  --gui       Force Tauri GUI mode
  --terminal  Force terminal editor mode (no window UI, Unix only)

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

    if force_gui && force_terminal {
        return Err("Cannot combine --gui with terminal flags".to_string());
    }

    let mode = if force_gui {
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
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => {
                print_help();
                return Err(String::new());
            }
            "--gui" => {}
            unknown => {
                return Err(format!(
                    "Unknown argument: {unknown}. Use --help for usage."
                ));
            }
        }
    }
    Ok((Mode::Gui, ()))
}

#[cfg(unix)]
fn run_terminal(opts: &TerminalOptions, theme: &config::ThemeConfig) -> Result<(), String> {
    if let Err(err) = config::ensure_config_file() {
        eprintln!("Config: {err}");
    }
    let core = AppCore::open_default()?;
    terminal::run_terminal_session(core.db(), theme, opts)
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
    fn parse_rejects_terminal_edit_without_tty() {
        let err = parse_args(&["--terminal".to_string()], false, false).expect_err("expected err");
        assert!(err.contains("requires a TTY"));
    }
}
