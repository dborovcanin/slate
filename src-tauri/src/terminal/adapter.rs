use super::input::Key;
use crate::editor_core::engine::EditorEngine;
use crate::editor_core::vim::{VimContext, VimKey, VimState, VimStep};

pub struct TerminalVimAdapter;

impl TerminalVimAdapter {
    pub fn to_vim_key(key: &Key) -> Option<VimKey> {
        match key {
            Key::Esc => Some(VimKey::Esc),
            Key::Enter => Some(VimKey::Enter),
            Key::Tab => Some(VimKey::Tab),
            Key::Backspace => Some(VimKey::Backspace),
            Key::Delete => Some(VimKey::Delete),
            Key::ArrowUp => Some(VimKey::ArrowUp),
            Key::ArrowDown => Some(VimKey::ArrowDown),
            Key::ArrowLeft => Some(VimKey::ArrowLeft),
            Key::ArrowRight => Some(VimKey::ArrowRight),
            Key::Home => Some(VimKey::Char('0')),
            Key::End => Some(VimKey::Char('$')),
            Key::Char(ch) => Some(VimKey::Char(*ch)),
            Key::Ctrl(ch) => Some(VimKey::Ctrl(*ch)),
            _ => None,
        }
    }

    pub fn step(state: &VimState, key: &Key, context: &VimContext) -> Option<VimStep> {
        let vim_key = Self::to_vim_key(key)?;
        Some(EditorEngine::step_vim(state, vim_key, context))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_vim_key_maps_terminal_input_subset() {
        assert_eq!(TerminalVimAdapter::to_vim_key(&Key::Esc), Some(VimKey::Esc));
        assert_eq!(
            TerminalVimAdapter::to_vim_key(&Key::Char('j')),
            Some(VimKey::Char('j'))
        );
        assert_eq!(
            TerminalVimAdapter::to_vim_key(&Key::Ctrl('w')),
            Some(VimKey::Ctrl('w'))
        );
        assert_eq!(
            TerminalVimAdapter::to_vim_key(&Key::Home),
            Some(VimKey::Char('0'))
        );
        assert_eq!(
            TerminalVimAdapter::to_vim_key(&Key::End),
            Some(VimKey::Char('$'))
        );
    }

    #[test]
    fn to_vim_key_ignores_non_vim_terminal_inputs() {
        assert_eq!(
            TerminalVimAdapter::to_vim_key(&Key::Paste("x".into())),
            None
        );
        assert_eq!(TerminalVimAdapter::to_vim_key(&Key::CtrlArrowLeft), None);
        assert_eq!(TerminalVimAdapter::to_vim_key(&Key::CtrlDelete), None);
    }

    #[test]
    fn step_delegates_to_shared_engine() {
        let state = VimState::default();
        let context = VimContext {
            has_search_matches: false,
            line_count: 10,
        };
        let step = TerminalVimAdapter::step(&state, &Key::Esc, &context).expect("step");
        assert_eq!(step.state.mode, crate::editor_core::vim::VimMode::Normal);
    }
}
