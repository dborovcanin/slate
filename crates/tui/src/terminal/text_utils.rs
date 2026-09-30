use std::cmp::min;

use unicode_width::UnicodeWidthChar;

use super::render;

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
        .map(|line| app_core::note_sources::title_text_for_line(line))
        .find(|line| !line.is_empty())
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
    let mut visible = 0usize;
    for ch in text.chars() {
        if ch == '\t' {
            visible += render::TAB_WIDTH - (visible % render::TAB_WIDTH);
        } else {
            visible += ch.width().unwrap_or(0);
        }
    }
    visible
}

/// Returns `(prefix_cols, total_cols)` in a single pass over the chars.
/// Equivalent to calling `display_cols_for_prefix(text, prefix_chars)` and
/// `line_display_cols(text)` separately but avoids the double iteration.
pub fn display_cols_prefix_and_total(text: &str, prefix_chars: usize) -> (usize, usize) {
    let mut visible = 0usize;
    let mut prefix_cols = None;
    for (idx, ch) in text.chars().enumerate() {
        if idx == prefix_chars {
            prefix_cols = Some(visible);
        }
        if ch == '\t' {
            visible += render::TAB_WIDTH - (visible % render::TAB_WIDTH);
        } else {
            visible += ch.width().unwrap_or(0);
        }
    }
    let total = visible;
    (prefix_cols.unwrap_or(total), total)
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

/// Char ranges in `line` of non-overlapping matches of `query_lower` (an
/// already lowercased query), compared case-insensitively.
pub fn case_insensitive_matches(line: &str, query_lower: &str) -> Vec<(usize, usize)> {
    let mut matches = Vec::new();
    if query_lower.is_empty() {
        return matches;
    }
    if line.is_ascii() && query_lower.is_ascii() {
        // ASCII lowercasing keeps byte offsets, which are char offsets here.
        let lower = line.to_ascii_lowercase();
        let mut start = 0;
        while let Some(pos) = lower[start..].find(query_lower) {
            let from = start + pos;
            matches.push((from, from + query_lower.len()));
            start = from + query_lower.len();
        }
        return matches;
    }
    // Lowercasing can change a char's length (`İ` becomes two chars), so
    // keep, for every lowered char, the index of the char it came from.
    let query: Vec<char> = query_lower.chars().collect();
    let lowered: Vec<(char, usize)> = line
        .chars()
        .enumerate()
        .flat_map(|(idx, ch)| ch.to_lowercase().map(move |lower| (lower, idx)))
        .collect();
    let mut start = 0;
    while start + query.len() <= lowered.len() {
        let window = &lowered[start..start + query.len()];
        if window.iter().map(|(ch, _)| *ch).eq(query.iter().copied()) {
            matches.push((window[0].1, window[query.len() - 1].1 + 1));
            start += query.len();
        } else {
            start += 1;
        }
    }
    matches
}
