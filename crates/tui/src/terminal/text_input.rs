//! Single-line text editing for prompts (command bar, in-note search).
//! The cursor is a char index; `usize::MAX` means "at the end", so callers
//! that replace the whole text can park the cursor without measuring it.

use super::input::Key;

/// Cursor position clamped to the text.
pub fn cursor(text: &str, cursor: usize) -> usize {
    cursor.min(text.chars().count())
}

/// Applies an editing or cursor-movement key. Returns `true` when the key was
/// handled (text or cursor may have changed), `false` for keys it ignores.
pub fn apply_key(text: &mut String, cursor_pos: &mut usize, key: &Key) -> bool {
    let len = text.chars().count();
    let at = cursor(text, *cursor_pos);
    match key {
        Key::Char(ch) => {
            text.insert(byte_index(text, at), *ch);
            *cursor_pos = at + 1;
        }
        Key::Paste(pasted) => {
            let clean: String = pasted.chars().filter(|c| *c != '\n' && *c != '\r').collect();
            text.insert_str(byte_index(text, at), &clean);
            *cursor_pos = at + clean.chars().count();
        }
        Key::Backspace => {
            if at > 0 {
                let from = byte_index(text, at - 1);
                text.replace_range(from..byte_index(text, at), "");
                *cursor_pos = at - 1;
            } else {
                *cursor_pos = 0;
            }
        }
        Key::Delete => {
            if at < len {
                let from = byte_index(text, at);
                text.replace_range(from..byte_index(text, at + 1), "");
            }
            *cursor_pos = at;
        }
        Key::Ctrl('w') | Key::CtrlBackspace => {
            let start = word_start_before(text, at);
            let from = byte_index(text, start);
            text.replace_range(from..byte_index(text, at), "");
            *cursor_pos = start;
        }
        Key::CtrlDelete => {
            let end = word_end_after(text, at);
            let from = byte_index(text, at);
            text.replace_range(from..byte_index(text, end), "");
            *cursor_pos = at;
        }
        Key::ArrowLeft => *cursor_pos = at.saturating_sub(1),
        Key::ArrowRight => *cursor_pos = (at + 1).min(len),
        Key::CtrlArrowLeft => *cursor_pos = word_start_before(text, at),
        Key::CtrlArrowRight => *cursor_pos = word_end_after(text, at),
        Key::Home | Key::Ctrl('a') => *cursor_pos = 0,
        Key::End | Key::Ctrl('e') => *cursor_pos = len,
        Key::Ctrl('u') => {
            text.replace_range(..byte_index(text, at), "");
            *cursor_pos = 0;
        }
        Key::Ctrl('k') => {
            text.truncate(byte_index(text, at));
            *cursor_pos = at;
        }
        _ => return false,
    }
    true
}

fn byte_index(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map_or(text.len(), |(idx, _)| idx)
}

/// Start of the word before `at`, skipping whitespace first (Ctrl+W).
fn word_start_before(text: &str, at: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut idx = at.min(chars.len());
    while idx > 0 && chars[idx - 1].is_whitespace() {
        idx -= 1;
    }
    while idx > 0 && !chars[idx - 1].is_whitespace() {
        idx -= 1;
    }
    idx
}

/// End of the word at or after `at`, skipping whitespace first.
fn word_end_after(text: &str, at: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut idx = at.min(chars.len());
    while idx < chars.len() && chars[idx].is_whitespace() {
        idx += 1;
    }
    while idx < chars.len() && !chars[idx].is_whitespace() {
        idx += 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(text: &str, cursor_pos: usize, keys: &[Key]) -> (String, usize) {
        let mut text = text.to_string();
        let mut cursor_pos = cursor_pos;
        for key in keys {
            apply_key(&mut text, &mut cursor_pos, key);
        }
        let at = cursor(&text, cursor_pos);
        (text, at)
    }

    #[test]
    fn end_sentinel_inserts_at_the_end() {
        assert_eq!(edit("note", usize::MAX, &[Key::Char('s')]), ("notes".into(), 5));
    }

    #[test]
    fn arrows_move_and_typing_inserts_mid_text() {
        let keys = [Key::ArrowLeft, Key::ArrowLeft, Key::Char('X')];
        assert_eq!(edit("note lock", usize::MAX, &keys), ("note loXck".into(), 8));
    }

    #[test]
    fn backspace_and_delete_act_around_the_cursor() {
        assert_eq!(edit("abcd", 2, &[Key::Backspace]), ("acd".into(), 1));
        assert_eq!(edit("abcd", 2, &[Key::Delete]), ("abd".into(), 2));
        assert_eq!(edit("abcd", 0, &[Key::Backspace]), ("abcd".into(), 0));
        assert_eq!(edit("abcd", 4, &[Key::Delete]), ("abcd".into(), 4));
    }

    #[test]
    fn home_end_and_word_jumps() {
        assert_eq!(edit("note lock now", 6, &[Key::Home]).1, 0);
        assert_eq!(edit("note lock now", 6, &[Key::Ctrl('e')]).1, 13);
        assert_eq!(edit("note lock now", 13, &[Key::CtrlArrowLeft]).1, 10);
        assert_eq!(edit("note lock now", 0, &[Key::CtrlArrowRight]).1, 4);
    }

    #[test]
    fn word_deletes_respect_the_cursor() {
        assert_eq!(edit("note lock now", 9, &[Key::Ctrl('w')]), ("note  now".into(), 5));
        assert_eq!(edit("note lock now", 4, &[Key::CtrlDelete]), ("note now".into(), 4));
        assert_eq!(edit("note lock", 5, &[Key::Ctrl('u')]), ("lock".into(), 0));
        assert_eq!(edit("note lock", 4, &[Key::Ctrl('k')]), ("note".into(), 4));
    }

    #[test]
    fn handles_multibyte_chars() {
        assert_eq!(edit("čaš", 1, &[Key::Char('ž')]), ("čžaš".into(), 2));
        assert_eq!(edit("čaš", 3, &[Key::Backspace]), ("ča".into(), 2));
    }

    #[test]
    fn paste_inserts_without_newlines() {
        assert_eq!(edit("ab", 1, &[Key::Paste("x\ny".into())]), ("axyb".into(), 3));
    }

    #[test]
    fn ignores_unrelated_keys() {
        let mut text = "a".to_string();
        let mut cursor_pos = 0;
        assert!(!apply_key(&mut text, &mut cursor_pos, &Key::Tab));
    }
}
