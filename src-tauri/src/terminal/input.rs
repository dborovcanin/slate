use std::io::{self, Write};
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, Ordering};

static SIGWINCH_FIRED: AtomicBool = AtomicBool::new(false);

extern "C" fn sigwinch_handler(_: libc::c_int) {
    SIGWINCH_FIRED.store(true, Ordering::Relaxed);
}

pub fn take_resize() -> bool {
    SIGWINCH_FIRED.swap(false, Ordering::Relaxed)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Paste(String),
    Enter,
    ShiftEnter,
    Backspace,
    Delete,
    CtrlDelete,
    CtrlBackspace,
    Tab,
    BackTab,
    Esc,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    CtrlArrowLeft,
    CtrlArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    Ctrl(char),
}

pub fn terminal_size() -> (usize, usize) {
    let mut ws = MaybeUninit::<libc::winsize>::zeroed();
    let ok = unsafe {
        libc::ioctl(
            libc::STDOUT_FILENO,
            libc::TIOCGWINSZ,
            ws.as_mut_ptr() as *mut libc::c_void,
        )
    };
    if ok == 0 {
        let ws = unsafe { ws.assume_init() };
        let rows = usize::from(ws.ws_row.max(1));
        let cols = usize::from(ws.ws_col.max(1));
        (rows, cols)
    } else {
        (24, 80)
    }
}

pub fn read_key() -> Result<Option<Key>, String> {
    let Some(first) = read_byte()? else {
        return Ok(None);
    };

    if first == b'\x1b' {
        return parse_escape_sequence();
    }
    if first == b'\r' || first == b'\n' {
        return Ok(Some(Key::Enter));
    }
    if first == b'\t' {
        return Ok(Some(Key::Tab));
    }
    if first == 127 || first == 8 {
        let erase = terminal_erase_byte().unwrap_or(127);
        return Ok(Some(classify_backspace_byte(first, erase)));
    }
    if (1..=26).contains(&first) {
        let c = (b'a' + (first - 1)) as char;
        return Ok(Some(Key::Ctrl(c)));
    }
    // Map Ctrl+\ (28), Ctrl+] (29), Ctrl+^ (30), Ctrl+_ (31)
    // Ctrl+[ (27) is already handled as ESC above.
    if (28..=31).contains(&first) {
        let c = (b'\\' + (first - 28)) as char;
        return Ok(Some(Key::Ctrl(c)));
    }
    if first.is_ascii() {
        return Ok(Some(Key::Char(first as char)));
    }

    let needed = utf8_continuation_count(first);
    if needed == 0 {
        return Ok(None);
    }

    let mut bytes = vec![first];
    for _ in 0..needed {
        if let Some(b) = read_byte()? {
            bytes.push(b);
        } else {
            return Ok(None);
        }
    }
    if let Ok(text) = std::str::from_utf8(&bytes) {
        if let Some(ch) = text.chars().next() {
            return Ok(Some(Key::Char(ch)));
        }
    }
    Ok(None)
}

pub struct TerminalGuard {
    original: libc::termios,
}

impl TerminalGuard {
    pub fn enter() -> Result<Self, String> {
        let mut term = MaybeUninit::<libc::termios>::zeroed();
        let ok = unsafe { libc::tcgetattr(libc::STDIN_FILENO, term.as_mut_ptr()) };
        if ok != 0 {
            return Err(format!(
                "Failed to read terminal attributes: {}",
                io::Error::last_os_error()
            ));
        }
        let original = unsafe { term.assume_init() };
        let mut raw = original;

        raw.c_iflag &= !(libc::BRKINT | libc::ICRNL | libc::INPCK | libc::ISTRIP | libc::IXON);
        raw.c_oflag &= !(libc::OPOST);
        raw.c_cflag |= libc::CS8;
        raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
        raw.c_cc[libc::VMIN] = 0;
        raw.c_cc[libc::VTIME] = 1;

        let ok = unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) };
        if ok != 0 {
            return Err(format!(
                "Failed to enable raw terminal mode: {}",
                io::Error::last_os_error()
            ));
        }

        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = sigwinch_handler as *const () as libc::sighandler_t;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigaction(libc::SIGWINCH, &sa, std::ptr::null_mut());
        }

        let mut out = io::stdout();
        out.write_all(b"\x1b[?1049h\x1b[?2004h\x1b[?7l\x1b[?25l\x1b[H\x1b[2J")
            .and_then(|_| out.flush())
            .map_err(|e| format!("Failed to initialize terminal screen: {e}"))?;

        Ok(Self { original })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.original) };
        let mut out = io::stdout();
        let _ = out.write_all(b"\x1b[0m\x1b[?7h\x1b[?2004l\x1b[?25h\x1b[?1049l\x1b[0 q");
        let _ = out.flush();
    }
}

fn read_byte() -> Result<Option<u8>, String> {
    let mut buf = [0u8; 1];
    let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr() as *mut libc::c_void, 1) };
    if n == 0 {
        return Ok(None);
    }
    if n < 0 {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::WouldBlock
            || err.kind() == io::ErrorKind::Interrupted
        {
            return Ok(None);
        }
        return Err(format!("Failed to read stdin: {err}"));
    }
    Ok(Some(buf[0]))
}

fn utf8_continuation_count(first: u8) -> usize {
    if first & 0b1110_0000 == 0b1100_0000 {
        1
    } else if first & 0b1111_0000 == 0b1110_0000 {
        2
    } else if first & 0b1111_1000 == 0b1111_0000 {
        3
    } else {
        0
    }
}

fn read_bracketed_paste_payload() -> Result<String, String> {
    const END: &[u8] = b"\x1b[201~";
    let mut payload = Vec::new();
    let mut idle_ticks = 0usize;

    loop {
        match read_byte()? {
            Some(byte) => {
                idle_ticks = 0;
                payload.push(byte);
                if payload.len() >= END.len() && payload.ends_with(END) {
                    payload.truncate(payload.len() - END.len());
                    break;
                }
            }
            None => {
                idle_ticks += 1;
                if idle_ticks >= 8 {
                    break;
                }
            }
        }
    }

    Ok(String::from_utf8_lossy(&payload).into_owned())
}

fn parse_escape_sequence() -> Result<Option<Key>, String> {
    let Some(second) = read_byte()? else {
        return Ok(Some(Key::Esc));
    };
    if second == 127 || second == 8 {
        return Ok(Some(Key::CtrlBackspace));
    }
    if second != b'[' && second != b'O' {
        return Ok(Some(Key::Esc));
    }

    let mut seq = Vec::new();
    loop {
        let Some(b) = read_byte()? else {
            break;
        };
        seq.push(b);
        if b.is_ascii_alphabetic() || b == b'~' {
            break;
        }
    }

    if seq.is_empty() {
        return Ok(Some(Key::Esc));
    }

    let last = seq[seq.len() - 1];
    if seq.len() == 1 {
        match last {
            b'A' => return Ok(Some(Key::ArrowUp)),
            b'B' => return Ok(Some(Key::ArrowDown)),
            b'C' => return Ok(Some(Key::ArrowRight)),
            b'D' => return Ok(Some(Key::ArrowLeft)),
            b'H' => return Ok(Some(Key::Home)),
            b'F' => return Ok(Some(Key::End)),
            b'Z' => return Ok(Some(Key::BackTab)),
            _ => return Ok(Some(Key::Esc)),
        }
    } else {
        let s = std::str::from_utf8(&seq).unwrap_or("");
        if s == "200~" {
            let pasted = read_bracketed_paste_payload()?;
            return Ok(Some(Key::Paste(pasted)));
        }
        if s == "201~" {
            return Ok(None);
        }
        if let Some(key) = parse_csi_key(s) {
            return Ok(Some(key));
        }
    }

    Ok(Some(Key::Esc))
}

fn terminal_erase_byte() -> Option<u8> {
    let mut term = MaybeUninit::<libc::termios>::zeroed();
    let ok = unsafe { libc::tcgetattr(libc::STDIN_FILENO, term.as_mut_ptr()) };
    if ok != 0 {
        return None;
    }
    let term = unsafe { term.assume_init() };
    Some(term.c_cc[libc::VERASE] as u8)
}

fn classify_backspace_byte(byte: u8, erase: u8) -> Key {
    if byte == erase {
        Key::Backspace
    } else {
        Key::CtrlBackspace
    }
}

fn parse_csi_key(s: &str) -> Option<Key> {
    match s {
        "13;2u" | "27;2;13~" => return Some(Key::ShiftEnter),
        "1;5C" | "5C" => return Some(Key::CtrlArrowRight),
        "1;5D" | "5D" => return Some(Key::CtrlArrowLeft),
        "1~" | "7~" => return Some(Key::Home),
        "4~" | "8~" => return Some(Key::End),
        "3~" => return Some(Key::Delete),
        "3;5~" => return Some(Key::CtrlDelete),
        "5~" => return Some(Key::PageUp),
        "6~" => return Some(Key::PageDown),
        _ => {}
    }
    if is_ctrl_backspace_csi_sequence(s) {
        return Some(Key::CtrlBackspace);
    }
    None
}

fn is_ctrl_backspace_csi_sequence(s: &str) -> bool {
    let Some(body) = s.strip_suffix('u').or_else(|| s.strip_suffix('~')) else {
        return false;
    };
    if let Some(code) = body.strip_prefix("27;5;") {
        return code == "8" || code == "127";
    }
    let mut fields = body.split(';');
    let Some(codepoint) = fields.next() else {
        return false;
    };
    let Some(mods) = fields.next() else {
        return false;
    };
    let primary_mod = mods.split(':').next().unwrap_or("");
    (codepoint == "8" || codepoint == "127") && primary_mod == "5"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_backspace_byte_uses_erase_value() {
        assert_eq!(classify_backspace_byte(127, 127), Key::Backspace);
        assert_eq!(classify_backspace_byte(8, 127), Key::CtrlBackspace);
        assert_eq!(classify_backspace_byte(8, 8), Key::Backspace);
        assert_eq!(classify_backspace_byte(127, 8), Key::CtrlBackspace);
    }

    #[test]
    fn parse_csi_key_maps_common_ctrl_backspace_variants() {
        assert_eq!(parse_csi_key("127;5u"), Some(Key::CtrlBackspace));
        assert_eq!(parse_csi_key("8;5u"), Some(Key::CtrlBackspace));
        assert_eq!(parse_csi_key("127;5:1u"), Some(Key::CtrlBackspace));
        assert_eq!(parse_csi_key("8;5:1u"), Some(Key::CtrlBackspace));
        assert_eq!(parse_csi_key("127;5~"), Some(Key::CtrlBackspace));
        assert_eq!(parse_csi_key("8;5~"), Some(Key::CtrlBackspace));
        assert_eq!(parse_csi_key("27;5;8~"), Some(Key::CtrlBackspace));
        assert_eq!(parse_csi_key("27;5;127~"), Some(Key::CtrlBackspace));
    }

    #[test]
    fn parse_csi_key_maps_shift_enter_variants() {
        assert_eq!(parse_csi_key("13;2u"), Some(Key::ShiftEnter));
        assert_eq!(parse_csi_key("27;2;13~"), Some(Key::ShiftEnter));
    }
}
