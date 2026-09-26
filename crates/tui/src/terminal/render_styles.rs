use crate::terminal::canvas::cell_style;
use ratatui::style::{Modifier, Style};

use crate::editor_core::markdown_tokens::{self, CodeTokenType, InlineTokenType, MarkdownLineInfo};
use crate::terminal::markdown_view::{
    image_hidden_token_ranges, should_reveal_inline_marker, wiki_link_hidden_token_ranges,
};
use crate::terminal::theme::RenderPalette;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct CharStyle {
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) dim: bool,
    pub(super) strikethrough: bool,
    pub(super) underline: bool,
    pub(super) reverse: bool,
    pub(super) fg: Option<u8>,
    pub(super) bg: Option<u8>,
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

fn is_variable_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn has_variable_word_boundaries(bytes: &[u8], start: usize, end: usize) -> bool {
    let left_ok = start == 0 || !is_variable_word_byte(bytes[start - 1]);
    let right_ok = end == bytes.len() || !is_variable_word_byte(bytes[end]);
    left_ok && right_ok
}

fn find_variable_ranges(text: &str, variable_names: &[String]) -> Vec<(usize, usize)> {
    if text.is_empty() || variable_names.is_empty() {
        return Vec::new();
    }

    let bytes = text.as_bytes();
    // Keep byte indices stable (for style slicing) while matching variables
    // case-insensitively, matching normalized variable names.
    let lower_bytes = bytes
        .iter()
        .map(|byte| byte.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let mut matches: Vec<(usize, usize)> = Vec::new();

    for raw in variable_names {
        let needle_text = raw.trim();
        if needle_text.is_empty() {
            continue;
        }
        let needle = needle_text
            .as_bytes()
            .iter()
            .map(|byte| byte.to_ascii_lowercase())
            .collect::<Vec<_>>();
        if needle.len() > bytes.len() {
            continue;
        }

        let mut idx = 0usize;
        while idx + needle.len() <= bytes.len() {
            let end = idx + needle.len();
            if lower_bytes[idx..end] == needle[..] && has_variable_word_boundaries(bytes, idx, end)
            {
                matches.push((idx, end));
            }
            idx += 1;
        }
    }

    if matches.len() <= 1 {
        return matches;
    }

    // Prefer left-most ranges; for overlaps at same start, keep longer match.
    matches.sort_by(|a, b| a.0.cmp(&b.0).then((b.1 - b.0).cmp(&(a.1 - a.0))));

    let mut deduped = Vec::new();
    for candidate in matches {
        let Some(last) = deduped.last() else {
            deduped.push(candidate);
            continue;
        };
        if candidate.0 < last.1 {
            continue;
        }
        deduped.push(candidate);
    }

    deduped
}

pub(super) fn apply_variable_styles(
    chars: &[char],
    styles: &mut [CharStyle],
    variable_names: &[String],
    variable_color: u8,
) {
    if chars.is_empty() || variable_names.is_empty() {
        return;
    }

    let text: String = chars.iter().collect();
    for (start, end) in find_variable_ranges(&text, variable_names) {
        for style in styles.iter_mut().take(end).skip(start) {
            style.fg = Some(variable_color);
            style.bold = true;
            style.dim = false;
        }
    }
}

pub(super) fn apply_line_styles_from_info(info: &MarkdownLineInfo, styles: &mut [CharStyle]) {
    let len = styles.len();

    if let (Some(level), Some(marker_end)) = (info.heading_level, info.heading_marker_end) {
        let _ = level;
        for (idx, style) in styles.iter_mut().enumerate() {
            if idx < marker_end.min(len) {
                style.dim = true;
            } else {
                style.bold = true;
            }
        }
        return;
    }

    if let Some(marker_end) = info.quote_marker_end {
        for (idx, style) in styles.iter_mut().enumerate() {
            if idx < marker_end.min(len) {
                style.dim = true;
            } else {
                style.italic = true;
            }
        }
        return;
    }

    if info.is_horizontal_rule {
        for style in styles.iter_mut() {
            style.dim = true;
        }
        return;
    }

    if let Some(marker_end) = info.checklist_marker_end {
        for style in styles.iter_mut().take(marker_end.min(len)) {
            style.dim = true;
        }
        if info.checklist_checked {
            for style in styles.iter_mut().skip(marker_end.min(len)) {
                style.strikethrough = true;
                style.dim = true;
            }
        }
        return;
    }

    if let Some(marker_end) = info.list_marker_end {
        for style in styles.iter_mut().take(marker_end.min(len)) {
            style.dim = true;
        }
    }
}

pub(super) fn apply_inline_token_styles(
    tokens: &[markdown_tokens::InlineToken],
    styles: &mut [CharStyle],
    hidden_ranges: &mut Vec<(usize, usize)>,
    active_cursor_col: Option<usize>,
) {
    let len = styles.len();
    let component_ranges = markdown_tokens::inline_marker_component_ranges_from_tokens(tokens);
    hidden_ranges.extend(image_hidden_token_ranges(tokens, active_cursor_col));
    hidden_ranges.extend(wiki_link_hidden_token_ranges(tokens, active_cursor_col));
    for (index, token) in tokens.iter().enumerate() {
        let from = token.from.min(len);
        let to = token.to.min(len);
        if to <= from {
            continue;
        }

        if markdown_tokens::is_inline_marker_token_kind(token.kind)
            && !should_reveal_inline_marker(tokens, &component_ranges, index, active_cursor_col)
        {
            hidden_ranges.push((from, to));
            continue;
        }

        for style in styles.iter_mut().take(to).skip(from) {
            match token.kind {
                InlineTokenType::Strong => style.bold = true,
                InlineTokenType::Emphasis => style.italic = true,
                InlineTokenType::Strikethrough => style.strikethrough = true,
                InlineTokenType::Code | InlineTokenType::CodeMarker => style.dim = true,
                InlineTokenType::ImageAlt => {
                    style.italic = true;
                    style.bold = true;
                }
                InlineTokenType::ImageSrc | InlineTokenType::ImageMarker => style.dim = true,
                InlineTokenType::LinkText => {
                    style.bold = true;
                    style.underline = true;
                }
                InlineTokenType::LinkUrl | InlineTokenType::LinkMarker => style.dim = true,
                InlineTokenType::WikiLinkTitle => {
                    style.bold = true;
                    style.underline = true;
                }
                InlineTokenType::WikiLinkMarker
                | InlineTokenType::WikiLinkId
                | InlineTokenType::WikiLinkSep
                | InlineTokenType::WikiLinkAnchor => style.dim = true,
            }
        }
    }
}

pub(super) fn apply_code_token_styles(
    tokens: &[markdown_tokens::CodeToken],
    styles: &mut [CharStyle],
    palette: RenderPalette,
) {
    let len = styles.len();
    for token in tokens {
        let from = token.from.min(len);
        let to = token.to.min(len);
        if to <= from {
            continue;
        }

        for style in styles.iter_mut().take(to).skip(from) {
            match token.kind {
                CodeTokenType::Keyword => {
                    style.fg = Some(palette.code_keyword);
                    style.dim = false;
                }
                CodeTokenType::String => {
                    style.fg = Some(palette.code_string);
                    style.dim = false;
                }
                CodeTokenType::Number => {
                    style.fg = Some(palette.code_number);
                    style.dim = false;
                }
                CodeTokenType::Comment => {
                    style.fg = Some(palette.code_comment);
                    style.dim = true;
                }
                CodeTokenType::Function => {
                    style.fg = Some(palette.code_function);
                    style.dim = false;
                }
                CodeTokenType::Type => {
                    style.fg = Some(palette.code_type);
                    style.dim = false;
                }
            }
        }
    }
}
