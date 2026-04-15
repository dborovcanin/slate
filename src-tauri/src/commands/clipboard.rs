use std::process::{Command, Stdio};

fn run_clipboard_read_command(bin: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim_end_matches('\n').trim_end_matches('\r');
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn read_clipboard_via_commands() -> Option<String> {
    if let Some(text) = run_clipboard_read_command("wl-paste", &["-n"]) {
        return Some(text);
    }
    if let Some(text) = run_clipboard_read_command("xclip", &["-selection", "clipboard", "-o"]) {
        return Some(text);
    }
    if let Some(text) = run_clipboard_read_command("xsel", &["--clipboard", "--output"]) {
        return Some(text);
    }
    if let Some(text) = run_clipboard_read_command("pbpaste", &[]) {
        return Some(text);
    }
    if let Some(text) = run_clipboard_read_command(
        "powershell",
        &["-NoProfile", "-Command", "Get-Clipboard -Raw"],
    ) {
        return Some(text);
    }
    if let Some(text) =
        run_clipboard_read_command("pwsh", &["-NoProfile", "-Command", "Get-Clipboard -Raw"])
    {
        return Some(text);
    }
    if std::env::var_os("TMUX").is_some() {
        if let Some(text) = run_clipboard_read_command("tmux", &["save-buffer", "-"]) {
            return Some(text);
        }
    }
    None
}

#[tauri::command]
pub fn read_clipboard_text() -> Option<String> {
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        if let Ok(text) = clipboard.get_text() {
            let trimmed = text.trim_end_matches('\n').trim_end_matches('\r');
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    read_clipboard_via_commands()
}
