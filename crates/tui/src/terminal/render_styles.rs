use crate::terminal::canvas::cell_style;
pub use note_session::variables::VariableNames;
use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct CharStyle {
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) dim: bool,
    pub(super) strikethrough: bool,
    pub(super) underline: bool,
    pub(super) reverse: bool,
    pub(super) fg: Option<Color>,
    pub(super) bg: Option<Color>,
}

impl CharStyle {
    pub(super) fn to_style(self) -> Style {
        let mut modifiers = Modifier::empty();
        modifiers.set(Modifier::BOLD, self.bold);
        modifiers.set(Modifier::DIM, self.dim);
        modifiers.set(Modifier::ITALIC, self.italic);
        modifiers.set(Modifier::UNDERLINED, self.underline);
        modifiers.set(Modifier::REVERSED, self.reverse);
        modifiers.set(Modifier::CROSSED_OUT, self.strikethrough);
        cell_style(self.fg, self.bg, modifiers)
    }
}
