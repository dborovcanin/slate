use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

/// How long `read_key` waits for input before returning `None` so the event
/// loop can run idle work (autosave, background results, animations).
const IDLE_POLL: Duration = Duration::from_millis(100);

const DEFAULT_ROWS: u32 = 24;
const DEFAULT_COLS: u32 = 80;

static TERMINAL_ROWS: AtomicU32 = AtomicU32::new(DEFAULT_ROWS);
static TERMINAL_COLS: AtomicU32 = AtomicU32::new(DEFAULT_COLS);
static RESIZED: AtomicBool = AtomicBool::new(false);

thread_local! {
    // Keys decoded from a single event that expands to several (Alt+x is
    // delivered as Esc followed by x).
    static PENDING: RefCell<VecDeque<Key>> = const { RefCell::new(VecDeque::new()) };
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

/// Last known terminal size as `(rows, cols)`. Updated at session start and on
/// resize events; defaults to 24x80 when no terminal session is active.
pub fn terminal_size() -> (usize, usize) {
    (
        TERMINAL_ROWS.load(Ordering::Relaxed) as usize,
        TERMINAL_COLS.load(Ordering::Relaxed) as usize,
    )
}

pub fn set_terminal_size(rows: u16, cols: u16) {
    TERMINAL_ROWS.store(u32::from(rows.max(1)), Ordering::Relaxed);
    TERMINAL_COLS.store(u32::from(cols.max(1)), Ordering::Relaxed);
}

pub fn take_resize() -> bool {
    RESIZED.swap(false, Ordering::Relaxed)
}

/// Waits up to `IDLE_POLL` for the next key. Returns `None` on timeout or for
/// events that do not map to a key (resizes set the resize flag instead).
pub fn read_key() -> Result<Option<Key>, String> {
    if let Some(key) = PENDING.with(|pending| pending.borrow_mut().pop_front()) {
        return Ok(Some(key));
    }
    let ready = event::poll(IDLE_POLL).map_err(|e| format!("Failed to poll input: {e}"))?;
    if !ready {
        return Ok(None);
    }
    let event = event::read().map_err(|e| format!("Failed to read input: {e}"))?;
    match event {
        Event::Key(key_event) => {
            let mut keys = map_key_event(key_event);
            if keys.is_empty() {
                return Ok(None);
            }
            let first = keys.remove(0);
            if !keys.is_empty() {
                PENDING.with(|pending| pending.borrow_mut().extend(keys));
            }
            Ok(Some(first))
        }
        Event::Paste(text) => Ok(Some(Key::Paste(text))),
        Event::Resize(cols, rows) => {
            set_terminal_size(rows, cols);
            RESIZED.store(true, Ordering::Relaxed);
            Ok(None)
        }
        _ => Ok(None),
    }
}

/// Translates a crossterm key event into editor keys. Most events map to one
/// key; Alt+key expands to `Esc` followed by the key so fast `Esc j` typing
/// (which terminals deliver as a single Alt sequence) is not lost.
fn map_key_event(event: KeyEvent) -> Vec<Key> {
    if event.kind == KeyEventKind::Release {
        return Vec::new();
    }
    let mods = event.modifiers;
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let alt = mods.contains(KeyModifiers::ALT);
    let shift = mods.contains(KeyModifiers::SHIFT);

    let key = match event.code {
        KeyCode::Char(c) if ctrl => match ctrl_char(c) {
            // ^H is what most terminals send for Ctrl+Backspace.
            'h' => Key::CtrlBackspace,
            other => Key::Ctrl(other),
        },
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter if shift => Key::ShiftEnter,
        KeyCode::Enter => Key::Enter,
        KeyCode::Backspace if ctrl || alt => return vec![Key::CtrlBackspace],
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete if ctrl => Key::CtrlDelete,
        KeyCode::Delete => Key::Delete,
        KeyCode::Tab if shift => Key::BackTab,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Esc => Key::Esc,
        KeyCode::Up => Key::ArrowUp,
        KeyCode::Down => Key::ArrowDown,
        KeyCode::Left if ctrl => Key::CtrlArrowLeft,
        KeyCode::Right if ctrl => Key::CtrlArrowRight,
        KeyCode::Left => Key::ArrowLeft,
        KeyCode::Right => Key::ArrowRight,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        _ => return Vec::new(),
    };
    if alt {
        vec![Key::Esc, key]
    } else {
        vec![key]
    }
}

/// Normalizes the character of a Ctrl chord. Legacy terminals encode
/// Ctrl+\ ] ^ _ as bytes 0x1C..0x1F, which crossterm reports as Ctrl+4..7.
fn ctrl_char(c: char) -> char {
    match c {
        '4' => '\\',
        '5' => ']',
        '6' => '^',
        '7' => '_',
        other => other.to_ascii_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> Vec<Key> {
        map_key_event(KeyEvent::new(code, modifiers))
    }

    #[test]
    fn maps_plain_and_shifted_chars() {
        assert_eq!(press(KeyCode::Char('j'), KeyModifiers::NONE), vec![Key::Char('j')]);
        assert_eq!(press(KeyCode::Char('J'), KeyModifiers::SHIFT), vec![Key::Char('J')]);
    }

    #[test]
    fn maps_ctrl_chords_including_legacy_punctuation() {
        assert_eq!(press(KeyCode::Char('p'), KeyModifiers::CONTROL), vec![Key::Ctrl('p')]);
        assert_eq!(press(KeyCode::Char('5'), KeyModifiers::CONTROL), vec![Key::Ctrl(']')]);
        assert_eq!(press(KeyCode::Char(']'), KeyModifiers::CONTROL), vec![Key::Ctrl(']')]);
        assert_eq!(press(KeyCode::Char('4'), KeyModifiers::CONTROL), vec![Key::Ctrl('\\')]);
    }

    #[test]
    fn maps_ctrl_backspace_variants() {
        assert_eq!(press(KeyCode::Char('h'), KeyModifiers::CONTROL), vec![Key::CtrlBackspace]);
        assert_eq!(press(KeyCode::Backspace, KeyModifiers::CONTROL), vec![Key::CtrlBackspace]);
        assert_eq!(press(KeyCode::Backspace, KeyModifiers::ALT), vec![Key::CtrlBackspace]);
        assert_eq!(press(KeyCode::Backspace, KeyModifiers::NONE), vec![Key::Backspace]);
    }

    #[test]
    fn maps_shift_enter_and_back_tab() {
        assert_eq!(press(KeyCode::Enter, KeyModifiers::SHIFT), vec![Key::ShiftEnter]);
        assert_eq!(press(KeyCode::BackTab, KeyModifiers::SHIFT), vec![Key::BackTab]);
        assert_eq!(press(KeyCode::Tab, KeyModifiers::SHIFT), vec![Key::BackTab]);
    }

    #[test]
    fn maps_ctrl_arrows_and_delete() {
        assert_eq!(press(KeyCode::Left, KeyModifiers::CONTROL), vec![Key::CtrlArrowLeft]);
        assert_eq!(press(KeyCode::Right, KeyModifiers::CONTROL), vec![Key::CtrlArrowRight]);
        assert_eq!(press(KeyCode::Delete, KeyModifiers::CONTROL), vec![Key::CtrlDelete]);
    }

    #[test]
    fn alt_chord_expands_to_escape_then_key() {
        assert_eq!(
            press(KeyCode::Char('j'), KeyModifiers::ALT),
            vec![Key::Esc, Key::Char('j')]
        );
    }

    #[test]
    fn ignores_release_events_and_unmapped_keys() {
        let mut release = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert!(map_key_event(release).is_empty());
        assert!(press(KeyCode::F(5), KeyModifiers::NONE).is_empty());
    }
}
