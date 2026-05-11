use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

fn sanitize_mode(mode: &str) -> String {
    mode.chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
        .collect::<String>()
        .to_ascii_lowercase()
}

pub fn startup_log_path(mode: &str) -> PathBuf {
    let mode = sanitize_mode(mode);
    let perf = crate::config::load_perf_config();
    startup_log_path_with_config(mode.as_str(), &perf)
}

pub fn append_startup_log_line(mode: &str, line: &str) -> Result<(), String> {
    let perf = crate::config::load_perf_config();
    if !perf.enabled {
        return Ok(());
    }
    let mode = sanitize_mode(mode);
    let path = startup_log_path_with_config(mode.as_str(), &perf);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("open startup log '{}': {e}", path.display()))?;
    writeln!(file, "{line}").map_err(|e| format!("write startup log '{}': {e}", path.display()))?;
    Ok(())
}

fn startup_log_path_with_config(mode: &str, perf: &crate::config::PerfConfig) -> PathBuf {
    match mode_group(mode) {
        ModeGroup::Ui => {
            if perf.ui_log_path.trim().is_empty() {
                std::env::temp_dir().join("slate-log-ui.log")
            } else {
                PathBuf::from(perf.ui_log_path.as_str())
            }
        }
        ModeGroup::Tui => {
            if perf.tui_log_path.trim().is_empty() {
                std::env::temp_dir().join("slate-log-tui.log")
            } else {
                PathBuf::from(perf.tui_log_path.as_str())
            }
        }
        ModeGroup::Misc => std::env::temp_dir().join("slate-log-misc.log"),
    }
}

enum ModeGroup {
    Ui,
    Tui,
    Misc,
}

fn mode_group(mode: &str) -> ModeGroup {
    match mode {
        "ui" | "gui" | "ui_perf" | "gui_perf" => ModeGroup::Ui,
        "tui" | "terminal" | "tui_perf" | "terminal_perf" => ModeGroup::Tui,
        _ => ModeGroup::Misc,
    }
}
