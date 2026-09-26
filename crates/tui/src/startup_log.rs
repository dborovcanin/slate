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
    if !is_app_mode(mode) {
        return std::env::temp_dir().join("slate-log-misc.log");
    }
    if perf.log_path.trim().is_empty() {
        std::env::temp_dir().join("slate-log.log")
    } else {
        PathBuf::from(perf.log_path.as_str())
    }
}

fn is_app_mode(mode: &str) -> bool {
    matches!(mode, "tui" | "tui_perf")
}
