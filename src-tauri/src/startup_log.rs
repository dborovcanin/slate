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
    let suffix = match mode.as_str() {
        "ui" | "gui" => "ui",
        "tui" | "terminal" => "tui",
        _ if mode.is_empty() => "misc",
        other => other,
    };
    std::env::temp_dir().join(format!("note-startup-{suffix}.log"))
}

pub fn append_startup_log_line(mode: &str, line: &str) -> Result<(), String> {
    let path = startup_log_path(mode);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("open startup log '{}': {e}", path.display()))?;
    writeln!(file, "{line}").map_err(|e| format!("write startup log '{}': {e}", path.display()))?;
    Ok(())
}
