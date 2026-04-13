use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VimMode {
    Insert,
    Normal,
    Visual,
    VisualLine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VimPending {
    Delete,
    Yank,
    Go,
    DeleteInner,
    DeleteAround,
    YankInner,
    YankAround,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VimState {
    #[serde(default = "default_mode")]
    pub mode: VimMode,
    #[serde(default)]
    pub count_buffer: String,
    #[serde(default)]
    pub pending: Option<VimPending>,
}

fn default_mode() -> VimMode {
    VimMode::Normal
}

impl Default for VimState {
    fn default() -> Self {
        Self {
            mode: VimMode::Normal,
            count_buffer: String::new(),
            pending: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VimKey {
    Esc,
    Enter,
    Tab,
    Backspace,
    Delete,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Char(char),
    Ctrl(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VimIntent {
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveWordForward,
    MoveWordBackward,
    MoveLineStart,
    MoveLineEnd,
    MoveDocStart,
    MoveDocEnd,
    MoveToLine,
    EnterInsert,
    AppendInsert,
    InsertLineStart,
    AppendLineEnd,
    OpenLineBelow,
    OpenLineAbove,
    EnterVisual,
    EnterVisualLine,
    ExitVisual,
    DeleteLine,
    YankLine,
    DeleteToLineStart,
    DeleteToLineEnd,
    YankToLineStart,
    YankToLineEnd,
    DeleteChar,
    PasteAfter,
    Undo,
    Redo,
    OpenCommandBar,
    OpenSearch,
    SearchNext,
    SearchPrev,
    DeleteInsideWord,
    DeleteAroundWord,
    YankInsideWord,
    YankAroundWord,
    DeleteInsidePipe,
    DeleteAroundPipe,
    YankInsidePipe,
    YankAroundPipe,
    Swallow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VimContext {
    #[serde(default)]
    pub has_search_matches: bool,
    #[serde(default)]
    pub line_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VimAction {
    pub intent: VimIntent,
    #[serde(default)]
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VimStep {
    pub state: VimState,
    pub actions: Vec<VimAction>,
    pub handled: bool,
}

fn make_action(intent: VimIntent, count: usize) -> VimAction {
    VimAction {
        intent,
        count: count.max(1),
    }
}

fn has_count(state: &VimState) -> bool {
    !state.count_buffer.is_empty()
}

fn consume_count(state: &mut VimState) -> usize {
    let count = state.count_buffer.parse::<usize>().ok().unwrap_or(1).max(1);
    state.count_buffer.clear();
    count
}

pub fn parse_key_token(token: &str) -> Option<VimKey> {
    let token = token.trim();
    if token == "esc" {
        return Some(VimKey::Esc);
    }
    if token == "enter" {
        return Some(VimKey::Enter);
    }
    if token == "tab" {
        return Some(VimKey::Tab);
    }
    if token == "backspace" {
        return Some(VimKey::Backspace);
    }
    if token == "delete" {
        return Some(VimKey::Delete);
    }
    if token == "arrow_up" {
        return Some(VimKey::ArrowUp);
    }
    if token == "arrow_down" {
        return Some(VimKey::ArrowDown);
    }
    if token == "arrow_left" {
        return Some(VimKey::ArrowLeft);
    }
    if token == "arrow_right" {
        return Some(VimKey::ArrowRight);
    }
    if let Some(ch) = token
        .strip_prefix("char:")
        .and_then(|raw| raw.chars().next())
    {
        return Some(VimKey::Char(ch));
    }
    if let Some(ch) = token
        .strip_prefix("ctrl:")
        .and_then(|raw| raw.chars().next())
    {
        return Some(VimKey::Ctrl(ch.to_ascii_lowercase()));
    }
    None
}

pub fn step(state: &VimState, key: VimKey, ctx: &VimContext) -> VimStep {
    let mut next = state.clone();
    let mut actions: Vec<VimAction> = Vec::new();
    let mut handled = false;

    if next.mode == VimMode::Insert {
        if key == VimKey::Esc {
            next.mode = VimMode::Normal;
            next.pending = None;
            next.count_buffer.clear();
            handled = true;
        }
        return VimStep {
            state: next,
            actions,
            handled,
        };
    }

    if key == VimKey::Esc {
        next.mode = VimMode::Normal;
        next.pending = None;
        next.count_buffer.clear();
        actions.push(make_action(VimIntent::ExitVisual, 1));
        handled = true;
        return VimStep {
            state: next,
            actions,
            handled,
        };
    }

    if matches!(next.mode, VimMode::Visual | VimMode::VisualLine) {
        match key {
            VimKey::Char('v') if next.mode == VimMode::Visual => {
                next.mode = VimMode::Normal;
                actions.push(make_action(VimIntent::ExitVisual, 1));
                handled = true;
            }
            VimKey::Char('V') if next.mode == VimMode::VisualLine => {
                next.mode = VimMode::Normal;
                actions.push(make_action(VimIntent::ExitVisual, 1));
                handled = true;
            }
            VimKey::ArrowLeft | VimKey::Char('h') => {
                actions.push(make_action(VimIntent::MoveLeft, 1));
                handled = true;
            }
            VimKey::ArrowRight | VimKey::Char('l') => {
                actions.push(make_action(VimIntent::MoveRight, 1));
                handled = true;
            }
            VimKey::ArrowUp | VimKey::Char('k') => {
                actions.push(make_action(VimIntent::MoveUp, 1));
                handled = true;
            }
            VimKey::ArrowDown | VimKey::Char('j') => {
                actions.push(make_action(VimIntent::MoveDown, 1));
                handled = true;
            }
            _ => {}
        }

        return VimStep {
            state: next,
            actions,
            handled,
        };
    }

    if let VimKey::Char(digit) = key {
        if digit.is_ascii_digit() && next.pending.is_none() {
            if digit == '0' && !has_count(&next) && next.pending.is_none() {
                actions.push(make_action(VimIntent::MoveLineStart, 1));
                handled = true;
                return VimStep {
                    state: next,
                    actions,
                    handled,
                };
            }
            next.count_buffer.push(digit);
            handled = true;
            return VimStep {
                state: next,
                actions,
                handled,
            };
        }
    }

    if let Some(pending) = next.pending.take() {
        match (pending, key) {
            (VimPending::Delete, VimKey::Char('d')) => {
                let count = consume_count(&mut next);
                actions.push(make_action(VimIntent::DeleteLine, count));
                handled = true;
                return VimStep {
                    state: next,
                    actions,
                    handled,
                };
            }
            (VimPending::Delete, VimKey::Char('0')) => {
                let count = consume_count(&mut next);
                actions.push(make_action(VimIntent::DeleteToLineStart, count));
                handled = true;
                return VimStep {
                    state: next,
                    actions,
                    handled,
                };
            }
            (VimPending::Delete, VimKey::Char('$')) => {
                let count = consume_count(&mut next);
                actions.push(make_action(VimIntent::DeleteToLineEnd, count));
                handled = true;
                return VimStep {
                    state: next,
                    actions,
                    handled,
                };
            }
            (VimPending::Delete, VimKey::Char('i')) => {
                next.pending = Some(VimPending::DeleteInner);
                handled = true;
                return VimStep {
                    state: next,
                    actions,
                    handled,
                };
            }
            (VimPending::Delete, VimKey::Char('a')) => {
                next.pending = Some(VimPending::DeleteAround);
                handled = true;
                return VimStep {
                    state: next,
                    actions,
                    handled,
                };
            }
            (VimPending::Yank, VimKey::Char('y')) => {
                let count = consume_count(&mut next);
                actions.push(make_action(VimIntent::YankLine, count));
                handled = true;
                return VimStep {
                    state: next,
                    actions,
                    handled,
                };
            }
            (VimPending::Yank, VimKey::Char('0')) => {
                let count = consume_count(&mut next);
                actions.push(make_action(VimIntent::YankToLineStart, count));
                handled = true;
                return VimStep {
                    state: next,
                    actions,
                    handled,
                };
            }
            (VimPending::Yank, VimKey::Char('$')) => {
                let count = consume_count(&mut next);
                actions.push(make_action(VimIntent::YankToLineEnd, count));
                handled = true;
                return VimStep {
                    state: next,
                    actions,
                    handled,
                };
            }
            (VimPending::Yank, VimKey::Char('i')) => {
                next.pending = Some(VimPending::YankInner);
                handled = true;
                return VimStep {
                    state: next,
                    actions,
                    handled,
                };
            }
            (VimPending::Yank, VimKey::Char('a')) => {
                next.pending = Some(VimPending::YankAround);
                handled = true;
                return VimStep {
                    state: next,
                    actions,
                    handled,
                };
            }
            (VimPending::Go, VimKey::Char('g')) => {
                let count = consume_count(&mut next);
                if count > 1 {
                    actions.push(make_action(
                        VimIntent::MoveToLine,
                        count.min(ctx.line_count.max(1)),
                    ));
                } else {
                    actions.push(make_action(VimIntent::MoveDocStart, 1));
                }
                handled = true;
                return VimStep {
                    state: next,
                    actions,
                    handled,
                };
            }
            (VimPending::DeleteInner, VimKey::Char('w')) => {
                let count = consume_count(&mut next);
                actions.push(make_action(VimIntent::DeleteInsideWord, count));
                handled = true;
                return VimStep {
                    state: next,
                    actions,
                    handled,
                };
            }
            (VimPending::DeleteAround, VimKey::Char('w')) => {
                let count = consume_count(&mut next);
                actions.push(make_action(VimIntent::DeleteAroundWord, count));
                handled = true;
                return VimStep {
                    state: next,
                    actions,
                    handled,
                };
            }
            (VimPending::YankInner, VimKey::Char('w')) => {
                let count = consume_count(&mut next);
                actions.push(make_action(VimIntent::YankInsideWord, count));
                handled = true;
                return VimStep {
                    state: next,
                    actions,
                    handled,
                };
            }
            (VimPending::YankAround, VimKey::Char('w')) => {
                let count = consume_count(&mut next);
                actions.push(make_action(VimIntent::YankAroundWord, count));
                handled = true;
                return VimStep {
                    state: next,
                    actions,
                    handled,
                };
            }
            (VimPending::DeleteInner, VimKey::Char('|')) => {
                let count = consume_count(&mut next);
                actions.push(make_action(VimIntent::DeleteInsidePipe, count));
                handled = true;
                return VimStep {
                    state: next,
                    actions,
                    handled,
                };
            }
            (VimPending::DeleteAround, VimKey::Char('|')) => {
                let count = consume_count(&mut next);
                actions.push(make_action(VimIntent::DeleteAroundPipe, count));
                handled = true;
                return VimStep {
                    state: next,
                    actions,
                    handled,
                };
            }
            (VimPending::YankInner, VimKey::Char('|')) => {
                let count = consume_count(&mut next);
                actions.push(make_action(VimIntent::YankInsidePipe, count));
                handled = true;
                return VimStep {
                    state: next,
                    actions,
                    handled,
                };
            }
            (VimPending::YankAround, VimKey::Char('|')) => {
                let count = consume_count(&mut next);
                actions.push(make_action(VimIntent::YankAroundPipe, count));
                handled = true;
                return VimStep {
                    state: next,
                    actions,
                    handled,
                };
            }
            _ => {}
        }
    }

    match key {
        VimKey::Ctrl('r') => {
            let count = consume_count(&mut next);
            actions.push(make_action(VimIntent::Redo, count));
            handled = true;
        }
        VimKey::Ctrl('f') => {
            actions.push(make_action(VimIntent::OpenSearch, 1));
            handled = true;
        }
        VimKey::Ctrl('e') => {
            actions.push(make_action(VimIntent::OpenCommandBar, 1));
            handled = true;
        }
        VimKey::Char(':') => {
            actions.push(make_action(VimIntent::OpenCommandBar, 1));
            handled = true;
        }
        VimKey::Char('/') => {
            actions.push(make_action(VimIntent::OpenSearch, 1));
            handled = true;
        }
        VimKey::Char('n')
            if next.pending.is_none() && !has_count(&next) && ctx.has_search_matches =>
        {
            actions.push(make_action(VimIntent::SearchNext, 1));
            handled = true;
        }
        VimKey::Char('N')
            if next.pending.is_none() && !has_count(&next) && ctx.has_search_matches =>
        {
            actions.push(make_action(VimIntent::SearchPrev, 1));
            handled = true;
        }
        VimKey::Char('v') => {
            next.mode = VimMode::Visual;
            actions.push(make_action(VimIntent::EnterVisual, 1));
            handled = true;
        }
        VimKey::Char('V') => {
            next.mode = VimMode::VisualLine;
            actions.push(make_action(VimIntent::EnterVisualLine, 1));
            handled = true;
        }
        VimKey::Char('i') => {
            next.mode = VimMode::Insert;
            actions.push(make_action(VimIntent::EnterInsert, 1));
            handled = true;
        }
        VimKey::Char('a') => {
            next.mode = VimMode::Insert;
            actions.push(make_action(VimIntent::AppendInsert, 1));
            handled = true;
        }
        VimKey::Char('I') => {
            next.mode = VimMode::Insert;
            actions.push(make_action(VimIntent::InsertLineStart, 1));
            handled = true;
        }
        VimKey::Char('A') => {
            next.mode = VimMode::Insert;
            actions.push(make_action(VimIntent::AppendLineEnd, 1));
            handled = true;
        }
        VimKey::Char('o') => {
            next.mode = VimMode::Insert;
            actions.push(make_action(VimIntent::OpenLineBelow, 1));
            handled = true;
        }
        VimKey::Char('O') => {
            next.mode = VimMode::Insert;
            actions.push(make_action(VimIntent::OpenLineAbove, 1));
            handled = true;
        }
        VimKey::ArrowLeft | VimKey::Char('h') => {
            let count = consume_count(&mut next);
            actions.push(make_action(VimIntent::MoveLeft, count));
            handled = true;
        }
        VimKey::ArrowRight | VimKey::Char('l') => {
            let count = consume_count(&mut next);
            actions.push(make_action(VimIntent::MoveRight, count));
            handled = true;
        }
        VimKey::ArrowUp | VimKey::Char('k') => {
            let count = consume_count(&mut next);
            actions.push(make_action(VimIntent::MoveUp, count));
            handled = true;
        }
        VimKey::ArrowDown | VimKey::Char('j') => {
            let count = consume_count(&mut next);
            actions.push(make_action(VimIntent::MoveDown, count));
            handled = true;
        }
        VimKey::Char('w') => {
            let count = consume_count(&mut next);
            actions.push(make_action(VimIntent::MoveWordForward, count));
            handled = true;
        }
        VimKey::Char('b') => {
            let count = consume_count(&mut next);
            actions.push(make_action(VimIntent::MoveWordBackward, count));
            handled = true;
        }
        VimKey::Char('$') => {
            actions.push(make_action(VimIntent::MoveLineEnd, 1));
            next.count_buffer.clear();
            handled = true;
        }
        VimKey::Char('G') => {
            let count = consume_count(&mut next);
            if count > 1 {
                actions.push(make_action(
                    VimIntent::MoveToLine,
                    count.min(ctx.line_count.max(1)),
                ));
            } else {
                actions.push(make_action(VimIntent::MoveDocEnd, 1));
            }
            handled = true;
        }
        VimKey::Char('x') => {
            let count = consume_count(&mut next);
            actions.push(make_action(VimIntent::DeleteChar, count));
            handled = true;
        }
        VimKey::Char('p') => {
            let count = consume_count(&mut next);
            actions.push(make_action(VimIntent::PasteAfter, count));
            handled = true;
        }
        VimKey::Char('u') => {
            let count = consume_count(&mut next);
            actions.push(make_action(VimIntent::Undo, count));
            handled = true;
        }
        VimKey::Char('d') => {
            next.pending = Some(VimPending::Delete);
            handled = true;
        }
        VimKey::Char('y') => {
            next.pending = Some(VimPending::Yank);
            handled = true;
        }
        VimKey::Char('g') => {
            next.pending = Some(VimPending::Go);
            handled = true;
        }
        VimKey::Enter | VimKey::Tab | VimKey::Backspace | VimKey::Delete => {
            actions.push(make_action(VimIntent::Swallow, 1));
            handled = true;
        }
        VimKey::Char(ch) if ch.is_ascii_graphic() || ch == ' ' => {
            actions.push(make_action(VimIntent::Swallow, 1));
            handled = true;
        }
        _ => {}
    }

    VimStep {
        state: next,
        actions,
        handled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step_token(state: &VimState, token: &str) -> VimStep {
        let key = parse_key_token(token).expect("token");
        step(
            state,
            key,
            &VimContext {
                has_search_matches: true,
                line_count: 200,
            },
        )
    }

    #[test]
    fn dd_uses_count_prefix() {
        let start = VimState::default();
        let one = step_token(&start, "char:2");
        let two = step_token(&one.state, "char:d");
        let three = step_token(&two.state, "char:d");
        assert_eq!(three.actions.len(), 1);
        assert_eq!(three.actions[0].intent, VimIntent::DeleteLine);
        assert_eq!(three.actions[0].count, 2);
    }

    #[test]
    fn gg_and_counted_gg_are_supported() {
        let start = VimState::default();
        let one = step_token(&start, "char:g");
        let two = step_token(&one.state, "char:g");
        assert_eq!(two.actions[0].intent, VimIntent::MoveDocStart);
        assert_eq!(two.actions[0].count, 1);

        let start = VimState::default();
        let one = step_token(&start, "char:3");
        let two = step_token(&one.state, "char:g");
        let three = step_token(&two.state, "char:g");
        assert_eq!(three.actions[0].intent, VimIntent::MoveToLine);
        assert_eq!(three.actions[0].count, 3);
    }

    #[test]
    fn colon_opens_command_bar() {
        let step = step_token(&VimState::default(), "char::");
        assert!(step.handled);
        assert_eq!(step.actions[0].intent, VimIntent::OpenCommandBar);
    }

    #[test]
    fn insert_mode_exits_on_escape() {
        let state = VimState {
            mode: VimMode::Insert,
            ..VimState::default()
        };
        let step = step_token(&state, "esc");
        assert!(step.handled);
        assert_eq!(step.state.mode, VimMode::Normal);
    }

    #[test]
    fn di_pipe_emits_delete_inside_pipe() {
        let one = step_token(&VimState::default(), "char:d");
        let two = step_token(&one.state, "char:i");
        let three = step_token(&two.state, "char:|");
        assert_eq!(three.actions.len(), 1);
        assert_eq!(three.actions[0].intent, VimIntent::DeleteInsidePipe);
        assert_eq!(three.actions[0].count, 1);
    }

    #[test]
    fn daw_emits_delete_around_word() {
        let one = step_token(&VimState::default(), "char:d");
        let two = step_token(&one.state, "char:a");
        let three = step_token(&two.state, "char:w");
        assert_eq!(three.actions.len(), 1);
        assert_eq!(three.actions[0].intent, VimIntent::DeleteAroundWord);
        assert_eq!(three.actions[0].count, 1);
    }

    #[test]
    fn yaw_emits_yank_around_word() {
        let one = step_token(&VimState::default(), "char:y");
        let two = step_token(&one.state, "char:a");
        let three = step_token(&two.state, "char:w");
        assert_eq!(three.actions.len(), 1);
        assert_eq!(three.actions[0].intent, VimIntent::YankAroundWord);
        assert_eq!(three.actions[0].count, 1);
    }

    #[test]
    fn d0_and_dollar_emit_line_range_deletes() {
        let one = step_token(&VimState::default(), "char:d");
        let two = step_token(&one.state, "char:0");
        assert_eq!(two.actions.len(), 1);
        assert_eq!(two.actions[0].intent, VimIntent::DeleteToLineStart);

        let one = step_token(&VimState::default(), "char:d");
        let two = step_token(&one.state, "char:$");
        assert_eq!(two.actions.len(), 1);
        assert_eq!(two.actions[0].intent, VimIntent::DeleteToLineEnd);
    }
}
