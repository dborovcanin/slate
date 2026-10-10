//! Platform keystrokes to editor commands, for vim and standard editing.
//!
//! Vim mode hands keys to the shared vim engine unchanged. Standard mode
//! keeps the session in insert mode and maps the usual desktop shortcuts onto
//! the same session operations.
use editor_core::vim::VimKey;
use gpui::Keystroke;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditingMode {
    Vim,
    Standard,
}

/// What a keystroke asks the window to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyCommand {
    /// Send to the session's input pipeline.
    Vim(VimKey),
    /// Move the cursor; `select` extends the selection (Shift).
    Move { key: VimKey, select: bool },
    Home { select: bool },
    End { select: bool },
    Undo,
    Redo,
    Copy,
    Cut,
    Paste,
    SelectAll,
    Save,
    CommandPalette,
    CollectionBrowser,
    History,
    Find,
    Bold,
    Italic,
    NewNote,
    ToggleSidebar,
    Quit,
    /// Escape closes overlays first, then clears a selection.
    Escape,
    Ignore,
}

fn named_key(key: &str) -> Option<VimKey> {
    Some(match key {
        "escape" => VimKey::Esc,
        "enter" => VimKey::Enter,
        "tab" => VimKey::Tab,
        "backspace" => VimKey::Backspace,
        "delete" => VimKey::Delete,
        "up" => VimKey::ArrowUp,
        "down" => VimKey::ArrowDown,
        "left" => VimKey::ArrowLeft,
        "right" => VimKey::ArrowRight,
        _ => return None,
    })
}

/// The typed character, if the keystroke types one.
fn typed_char(k: &Keystroke) -> Option<char> {
    if let Some(text) = &k.key_char {
        let mut chars = text.chars();
        if let (Some(ch), None) = (chars.next(), chars.next()) {
            return Some(ch);
        }
    }
    let mut chars = k.key.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) if k.key == "space" || !ch.is_control() => Some(ch),
        _ if k.key == "space" => Some(' '),
        _ => None,
    }
}

/// Shortcuts both modes share (Ctrl with a letter, function keys).
fn app_shortcut(k: &Keystroke) -> Option<KeyCommand> {
    let m = k.modifiers;
    if k.key == "f1" {
        return Some(KeyCommand::CommandPalette);
    }
    if !(m.control || m.platform) {
        return None;
    }
    Some(match (k.key.as_str(), m.shift) {
        ("p", true) => KeyCommand::CommandPalette,
        ("s", false) => KeyCommand::Save,
        ("o", false) => KeyCommand::CollectionBrowser,
        ("h", true) => KeyCommand::History,
        ("n", false) => KeyCommand::NewNote,
        ("\\", false) => KeyCommand::ToggleSidebar,
        ("q", false) => KeyCommand::Quit,
        _ => return None,
    })
}

pub fn map(k: &Keystroke, mode: EditingMode) -> KeyCommand {
    if let Some(cmd) = app_shortcut(k) {
        return cmd;
    }
    let m = k.modifiers;
    match mode {
        EditingMode::Vim => {
            if m.control {
                return match k.key.chars().next() {
                    Some(ch) if k.key.len() == 1 => KeyCommand::Vim(VimKey::Ctrl(ch)),
                    _ => KeyCommand::Ignore,
                };
            }
            if let Some(key) = named_key(&k.key) {
                return KeyCommand::Vim(key);
            }
            typed_char(k).map_or(KeyCommand::Ignore, |ch| KeyCommand::Vim(VimKey::Char(ch)))
        }
        EditingMode::Standard => {
            if m.control || m.platform {
                return match (k.key.as_str(), m.shift) {
                    ("z", false) => KeyCommand::Undo,
                    ("z", true) | ("y", false) => KeyCommand::Redo,
                    ("c", false) => KeyCommand::Copy,
                    ("x", false) => KeyCommand::Cut,
                    ("v", false) => KeyCommand::Paste,
                    ("a", false) => KeyCommand::SelectAll,
                    ("f", false) => KeyCommand::Find,
                    ("b", false) => KeyCommand::Bold,
                    ("i", false) => KeyCommand::Italic,
                    _ => KeyCommand::Ignore,
                };
            }
            match k.key.as_str() {
                "escape" => KeyCommand::Escape,
                "home" => KeyCommand::Home { select: m.shift },
                "end" => KeyCommand::End { select: m.shift },
                "up" | "down" | "left" | "right" => KeyCommand::Move {
                    key: named_key(&k.key).expect("arrow"),
                    select: m.shift,
                },
                key => match named_key(key) {
                    Some(key) => KeyCommand::Vim(key),
                    None => typed_char(k)
                        .map_or(KeyCommand::Ignore, |ch| KeyCommand::Vim(VimKey::Char(ch))),
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(spec: &str) -> Keystroke {
        let mut k = Keystroke::parse(spec).expect("keystroke");
        if k.key.chars().count() == 1 && !k.modifiers.control {
            let ch = if k.modifiers.shift {
                k.key.to_uppercase()
            } else {
                k.key.clone()
            };
            k.key_char = Some(ch);
        }
        k
    }

    #[test]
    fn vim_mode_sends_keys_to_the_engine() {
        assert_eq!(map(&key("j"), EditingMode::Vim), KeyCommand::Vim(VimKey::Char('j')));
        assert_eq!(map(&key("shift-g"), EditingMode::Vim), KeyCommand::Vim(VimKey::Char('G')));
        assert_eq!(map(&key("escape"), EditingMode::Vim), KeyCommand::Vim(VimKey::Esc));
        assert_eq!(map(&key("ctrl-r"), EditingMode::Vim), KeyCommand::Vim(VimKey::Ctrl('r')));
    }

    #[test]
    fn standard_mode_maps_desktop_shortcuts() {
        let s = EditingMode::Standard;
        assert_eq!(map(&key("ctrl-z"), s), KeyCommand::Undo);
        assert_eq!(map(&key("ctrl-shift-z"), s), KeyCommand::Redo);
        assert_eq!(map(&key("ctrl-v"), s), KeyCommand::Paste);
        assert_eq!(
            map(&key("shift-left"), s),
            KeyCommand::Move {
                key: VimKey::ArrowLeft,
                select: true
            }
        );
        assert_eq!(map(&key("a"), s), KeyCommand::Vim(VimKey::Char('a')));
        assert_eq!(map(&key("enter"), s), KeyCommand::Vim(VimKey::Enter));
    }

    #[test]
    fn app_shortcuts_work_in_both_modes() {
        for mode in [EditingMode::Vim, EditingMode::Standard] {
            assert_eq!(map(&key("ctrl-shift-p"), mode), KeyCommand::CommandPalette);
            assert_eq!(map(&key("ctrl-s"), mode), KeyCommand::Save);
            assert_eq!(map(&key("ctrl-o"), mode), KeyCommand::CollectionBrowser);
        }
    }
}
