use std::path::PathBuf;
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

fn run_clipboard_read_binary_command(bin: &str, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new(bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }
    Some(output.stdout)
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

fn is_likely_image_file(path: &PathBuf) -> bool {
    let Some(ext) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "avif"
    )
}

pub(crate) fn read_clipboard_image_file_path() -> Option<PathBuf> {
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        if let Ok(paths) = clipboard.get().file_list() {
            for path in paths {
                if path.exists() && path.is_file() && is_likely_image_file(&path) {
                    return Some(path);
                }
            }
        }
    }
    None
}

pub(crate) fn read_clipboard_image_bytes() -> Option<(Vec<u8>, &'static str)> {
    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    {
        let linux_commands: [(&str, &[&str], &str); 6] = [
            (
                "wl-paste",
                &["--no-newline", "--type", "image/png"],
                "image/png",
            ),
            (
                "wl-paste",
                &["--no-newline", "--type", "image/jpeg"],
                "image/jpeg",
            ),
            (
                "wl-paste",
                &["--no-newline", "--type", "image/webp"],
                "image/webp",
            ),
            (
                "xclip",
                &["-selection", "clipboard", "-t", "image/png", "-o"],
                "image/png",
            ),
            (
                "xclip",
                &["-selection", "clipboard", "-t", "image/jpeg", "-o"],
                "image/jpeg",
            ),
            (
                "xclip",
                &["-selection", "clipboard", "-t", "image/webp", "-o"],
                "image/webp",
            ),
        ];
        for (bin, args, mime) in linux_commands {
            if let Some(bytes) = run_clipboard_read_binary_command(bin, args) {
                return Some((bytes, mime));
            }
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
