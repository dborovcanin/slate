#![cfg(unix)]

use app_core::AppCore;
use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};

#[path = "../commands/backup.rs"]
pub mod backup_impl;
#[path = "../config/mod.rs"]
mod config;
#[path = "../editor_core/mod.rs"]
pub mod editor_core;
#[path = "../commands/export/mod.rs"]
pub mod export_impl;
#[path = "../startup_log.rs"]
mod startup_log;
#[path = "../storage/mod.rs"]
mod storage;
#[path = "../terminal/mod.rs"]
mod terminal;

mod commands {
    pub use super::backup_impl as backup;
    pub use super::export_impl as export;
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

fn print_help() {
    println!("Slate TUI");
    println!();
    println!("Usage: slight [OPTIONS] [file]");
    println!();
    println!("Options:");
    println!("  --new, -n           Create a new note on startup");
    println!("  --id <note-id>      Open a specific note id");
    println!("  --list, -l          List notes and exit");
    println!("  --help, -h          Show this help");
}

fn parse_terminal_args(args: &[String]) -> Result<terminal::TerminalOptions, String> {
    let mut opts = terminal::TerminalOptions::default();
    let mut startup_file: Option<PathBuf> = None;
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            "--new" | "-n" => opts.create_new = true,
            "--list" | "-l" => opts.list_only = true,
            "--id" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--id requires a note id".to_string())?
                    .clone();
                opts.note_id = Some(value);
                i += 1;
            }
            "--terminal" | "-t" => {}
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

    if let Some(path) = startup_file.take() {
        if opts.create_new || opts.list_only || opts.note_id.is_some() {
            return Err("Cannot combine file open with --new, --list, or --id".to_string());
        }
        opts.note_id = Some(note_id_for_file(&path));
    }

    if !std::io::stdin().is_terminal() && !opts.list_only {
        return Err(
            "Terminal edit mode requires a TTY. Run inside a terminal emulator.".to_string(),
        );
    }

    Ok(opts)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let theme = config::load_theme_config();
    let opts = match parse_terminal_args(&args) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    };

    if let Err(err) = config::ensure_config_file() {
        eprintln!("Config: {err}");
    }

    let core = match AppCore::open_default() {
        Ok(value) => value,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    };

    if let Err(err) = terminal::run_terminal_session(core.db(), &theme, &opts) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
