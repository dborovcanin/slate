use std::cmp::min;

use unicode_width::UnicodeWidthChar;

use super::render;

pub fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

pub fn split_lines(body: &str) -> Vec<String> {
    if body.is_empty() {
        vec![String::new()]
    } else {
        body.split('\n').map(|l| l.to_string()).collect()
    }
}

pub fn join_lines(lines: &[String]) -> String {
    if lines.len() == 1 && lines[0].is_empty() {
        String::new()
    } else {
        lines.join("\n")
    }
}

pub fn line_char_len(text: &str) -> usize {
    text.chars().count()
}

pub fn byte_index(text: &str, char_idx: usize) -> usize {
    if char_idx == 0 {
        return 0;
    }
    text.char_indices()
        .nth(char_idx)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

pub fn remove_char_at(text: &mut String, char_idx: usize) {
    let start = byte_index(text, char_idx);
    let end = byte_index(text, char_idx + 1);
    if start < end && end <= text.len() {
        text.replace_range(start..end, "");
    }
}

pub fn derive_title_from_lines(lines: &[String]) -> String {
    let line = lines
        .iter()
        .find(|l| !l.trim().is_empty())
        .map(|s| s.trim())
        .unwrap_or("Untitled");
    if line.chars().count() > 70 {
        let truncated: String = line.chars().take(70).collect();
        format!("{truncated}...")
    } else {
        line.to_string()
    }
}

pub fn display_cols_for_prefix(text: &str, prefix_chars: usize) -> usize {
    let mut visible = 0usize;
    for ch in text.chars().take(prefix_chars) {
        if ch == '\t' {
            let tab = render::TAB_WIDTH - (visible % render::TAB_WIDTH);
            visible += tab;
        } else {
            visible += ch.width().unwrap_or(0);
        }
    }
    visible
}

pub fn line_display_cols(text: &str) -> usize {
    display_cols_for_prefix(text, line_char_len(text))
}

/// Map a logical cursor char-column to the display char-column.
///
/// When `clamp_to_last_char` is true (normal/visual modes), the cursor is
/// clamped to the last character instead of one-past-end.
pub fn cursor_render_char_col(text: &str, cursor_col: usize, clamp_to_last_char: bool) -> usize {
    let line_len = line_char_len(text);
    let clamped_col = min(cursor_col, line_len);
    if clamp_to_last_char && line_len > 0 && clamped_col == line_len {
        line_len - 1
    } else {
        clamped_col
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LineViewport {
    pub has_left_overflow: bool,
    pub has_right_overflow: bool,
    pub text_window_col: usize,
    pub text_width: usize,
}

pub fn compute_line_viewport(
    line_width: usize,
    scroll_col: usize,
    available_cols: usize,
) -> LineViewport {
    if available_cols == 0 {
        return LineViewport::default();
    }

    let has_left_overflow = scroll_col > 0;
    let has_right_overflow = line_width > scroll_col.saturating_add(available_cols);
    let reserved = (has_left_overflow as usize) + (has_right_overflow as usize);
    let text_width = available_cols.saturating_sub(reserved);
    let text_window_col = scroll_col.saturating_add(has_left_overflow as usize);

    LineViewport {
        has_left_overflow,
        has_right_overflow,
        text_window_col,
        text_width,
    }
}

pub fn viewport_col_for_display_col(
    display_col: usize,
    line_width: usize,
    scroll_col: usize,
    available_cols: usize,
) -> usize {
    let viewport = compute_line_viewport(line_width, scroll_col, available_cols);
    if viewport.text_width == 0 {
        return 0;
    }

    let text_rel = display_col
        .saturating_sub(viewport.text_window_col)
        .min(viewport.text_width.saturating_sub(1));
    (viewport.has_left_overflow as usize).saturating_add(text_rel)
}
