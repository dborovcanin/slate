#[cfg(not(test))]
use std::io::{self, IsTerminal as _, Write};
use std::process::{Command, Stdio};

#[cfg(not(test))]
use base64::Engine as _;

#[cfg_attr(test, allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardWriteBackend {
    Arboard,
    Tmux,
    WlCopy,
    Xclip,
    Xsel,
    Pbcopy,
    ClipExe,
    Osc52,
}

impl ClipboardWriteBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Arboard => "native",
            Self::Tmux => "tmux",
            Self::WlCopy => "wl-copy",
            Self::Xclip => "xclip",
            Self::Xsel => "xsel",
            Self::Pbcopy => "pbcopy",
            Self::ClipExe => "clip.exe",
            Self::Osc52 => "osc52",
        }
    }
}

#[cfg(test)]
pub fn copy_text_to_clipboard(_text: &str) -> Option<ClipboardWriteBackend> {
    None
}

#[cfg(not(test))]
pub fn copy_text_to_clipboard(text: &str) -> Option<ClipboardWriteBackend> {
    if text.is_empty() {
        return None;
    }

    if let Some(backend) = write_clipboard_via_commands(text) {
        return Some(backend);
    }

    if write_clipboard_via_osc52(text) {
        return Some(ClipboardWriteBackend::Osc52);
    }

    if let Ok(mut ctx) = arboard::Clipboard::new() {
        if ctx.set_text(text.to_string()).is_ok() {
            return Some(ClipboardWriteBackend::Arboard);
        }
    }

    None
}

pub fn read_clipboard_via_commands() -> Option<String> {
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

/// Image formats a note can hold, most preferred first. SVG is left out: notes
/// refuse it.
const CLIPBOARD_IMAGE_TYPES: [&str; 6] = [
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
    "image/bmp",
    "image/avif",
];

fn is_plain_text_type(mime: &str) -> bool {
    mime.starts_with("text/plain") || matches!(mime, "UTF8_STRING" | "STRING" | "TEXT")
}

/// The image type to read from a clipboard offering `types`. With
/// `prefer_text`, none when plain text is also offered: a spreadsheet copy
/// offers both, and its text is what a paste means.
fn pick_clipboard_image_type<'a>(types: &[&'a str], prefer_text: bool) -> Option<&'a str> {
    if prefer_text && types.iter().any(|mime| is_plain_text_type(mime)) {
        return None;
    }
    CLIPBOARD_IMAGE_TYPES
        .iter()
        .find_map(|wanted| types.iter().copied().find(|mime| mime == wanted))
}

/// Image bytes on the system clipboard. Terminals paste only text, so a
/// clipboard holding just an image (a screenshot tool's copy) never reaches
/// slate as a paste and has to be read here.
pub fn read_clipboard_image_via_commands(prefer_text: bool) -> Option<Vec<u8>> {
    let commands: [(&str, &[&str], &[&str]); 2] = [
        ("wl-paste", &["--list-types"], &["--type"]),
        (
            "xclip",
            &["-selection", "clipboard", "-target", "TARGETS", "-out"],
            &["-selection", "clipboard", "-out", "-target"],
        ),
    ];
    for (bin, list_args, read_args) in commands {
        let Some(listing) = run_clipboard_read_command(bin, list_args) else {
            continue;
        };
        let types = listing.lines().map(str::trim).collect::<Vec<_>>();
        let Some(mime) = pick_clipboard_image_type(&types, prefer_text) else {
            return None;
        };
        let output = Command::new(bin)
            .args(read_args)
            .arg(mime)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        return (output.status.success() && !output.stdout.is_empty()).then_some(output.stdout);
    }
    None
}

#[cfg(not(test))]
fn write_terminal_sequence(sequence: &str) -> bool {
    if io::stdout().is_terminal() {
        let mut out = io::stdout();
        if write!(out, "{sequence}").and_then(|_| out.flush()).is_ok() {
            return true;
        }
    }

    #[cfg(unix)]
    {
        if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
            if tty
                .write_all(sequence.as_bytes())
                .and_then(|_| tty.flush())
                .is_ok()
            {
                return true;
            }
        }
    }

    false
}

#[cfg(not(test))]
fn run_clipboard_write_command(bin: &str, args: &[&str], text: &str) -> bool {
    let mut child = match Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };

    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(text.as_bytes()).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
    } else {
        return false;
    }

    matches!(child.wait(), Ok(status) if status.success())
}

#[cfg(not(test))]
fn run_clipboard_write_command_with_arg(bin: &str, args: &[&str], text: &str) -> bool {
    matches!(
        Command::new(bin)
            .args(args)
            .arg(text)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status(),
        Ok(status) if status.success()
    )
}

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

#[cfg(not(test))]
fn write_clipboard_via_tmux(text: &str) -> bool {
    if run_clipboard_write_command("tmux", &["load-buffer", "-w", "-"], text) {
        return true;
    }
    if run_clipboard_write_command_with_arg("tmux", &["set-buffer", "-w", "--"], text) {
        return true;
    }
    if run_clipboard_write_command("tmux", &["load-buffer", "-"], text) {
        return true;
    }
    run_clipboard_write_command_with_arg("tmux", &["set-buffer", "--"], text)
}

#[cfg(not(test))]
fn write_clipboard_via_commands(text: &str) -> Option<ClipboardWriteBackend> {
    if run_clipboard_write_command("wl-copy", &[], text) {
        return Some(ClipboardWriteBackend::WlCopy);
    }
    if run_clipboard_write_command("xclip", &["-selection", "clipboard"], text) {
        return Some(ClipboardWriteBackend::Xclip);
    }
    if run_clipboard_write_command("xsel", &["--clipboard", "--input"], text) {
        return Some(ClipboardWriteBackend::Xsel);
    }
    if run_clipboard_write_command("pbcopy", &[], text) {
        return Some(ClipboardWriteBackend::Pbcopy);
    }
    if run_clipboard_write_command("clip.exe", &[], text) {
        return Some(ClipboardWriteBackend::ClipExe);
    }
    if run_clipboard_write_command("clip", &[], text) {
        return Some(ClipboardWriteBackend::ClipExe);
    }
    if std::env::var_os("TMUX").is_some() && write_clipboard_via_tmux(text) {
        return Some(ClipboardWriteBackend::Tmux);
    }
    None
}

#[cfg(not(test))]
fn build_osc52_sequence(encoded: &str, terminator: &str) -> String {
    if std::env::var_os("TMUX").is_some() {
        return format!("\x1bPtmux;\x1b\x1b]52;c;{encoded}{terminator}\x1b\\");
    }
    if std::env::var_os("STY").is_some() {
        return format!("\x1bP\x1b]52;c;{encoded}{terminator}\x1b\\");
    }
    format!("\x1b]52;c;{encoded}{terminator}")
}

#[cfg(not(test))]
fn write_clipboard_via_osc52(text: &str) -> bool {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let bel = build_osc52_sequence(&encoded, "\x07");
    let st = build_osc52_sequence(&encoded, "\x1b\\");

    let wrote_bel = write_terminal_sequence(&bel);
    let wrote_st = write_terminal_sequence(&st);
    wrote_bel || wrote_st
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_clipboard_image_type_takes_an_image_only_clipboard() {
        assert_eq!(
            pick_clipboard_image_type(&["image/png"], true),
            Some("image/png")
        );
        assert_eq!(
            pick_clipboard_image_type(&["TARGETS", "TIMESTAMP", "image/jpeg"], true),
            Some("image/jpeg")
        );
    }

    #[test]
    fn pick_clipboard_image_type_prefers_png_and_skips_svg() {
        assert_eq!(
            pick_clipboard_image_type(&["image/svg+xml", "image/webp", "image/png"], true),
            Some("image/png")
        );
        assert_eq!(pick_clipboard_image_type(&["image/svg+xml"], true), None);
    }

    #[test]
    fn pick_clipboard_image_type_leaves_text_to_a_text_paste() {
        let types = ["text/plain;charset=utf-8", "image/png"];
        assert_eq!(pick_clipboard_image_type(&types, true), None);
        assert_eq!(pick_clipboard_image_type(&types, false), Some("image/png"));
        assert_eq!(
            pick_clipboard_image_type(&["UTF8_STRING", "image/png"], true),
            None
        );
        // A browser's image copy carries HTML, not plain text.
        assert_eq!(
            pick_clipboard_image_type(&["text/html", "image/png"], true),
            Some("image/png")
        );
    }
}
