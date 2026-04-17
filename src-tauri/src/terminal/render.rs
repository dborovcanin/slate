use std::fmt::Write as _;

use crate::editor_core::markdown_tokens::{self, CodeTokenType, InlineTokenType, MarkdownLineInfo};

pub const RESET: &str = "\x1b[0m";
pub const TAB_WIDTH: usize = 4;

#[derive(Clone, Copy)]
pub struct RenderPalette {
    pub code_keyword: u8,
    pub code_string: u8,
    pub code_number: u8,
    pub code_comment: u8,
    pub code_function: u8,
    pub code_type: u8,
    pub variable: u8,
    pub search_match: u8,
    pub search_current: u8,
}

impl Default for RenderPalette {
    fn default() -> Self {
        Self {
            code_keyword: 81,
            code_string: 114,
            code_number: 215,
            code_comment: 244,
            code_function: 74,
            code_type: 183,
            variable: 179,
            search_match: 141,
            search_current: 203,
        }
    }
}

fn normalize_color_scheme(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(['_', ' '], "-")
}

impl RenderPalette {
    pub fn for_color_scheme(color_scheme: &str) -> Self {
        match normalize_color_scheme(color_scheme).as_str() {
            "catppuccin-mocha" => Self {
                code_keyword: 111,
                code_string: 150,
                code_number: 217,
                code_comment: 103,
                code_function: 117,
                code_type: 183,
                variable: 180,
                search_match: 147,
                search_current: 211,
            },
            "catppuccin-latte" => Self {
                code_keyword: 33,
                code_string: 29,
                code_number: 166,
                code_comment: 102,
                code_function: 25,
                code_type: 61,
                variable: 130,
                search_match: 69,
                search_current: 160,
            },
            "gruvbox-dark" => Self {
                code_keyword: 214,
                code_string: 142,
                code_number: 208,
                code_comment: 245,
                code_function: 109,
                code_type: 175,
                variable: 172,
                search_match: 179,
                search_current: 167,
            },
            "gruvbox-light" => Self {
                code_keyword: 130,
                code_string: 64,
                code_number: 166,
                code_comment: 102,
                code_function: 25,
                code_type: 95,
                variable: 94,
                search_match: 136,
                search_current: 160,
            },
            "dracula" => Self {
                code_keyword: 177,
                code_string: 114,
                code_number: 221,
                code_comment: 103,
                code_function: 117,
                code_type: 183,
                variable: 222,
                search_match: 141,
                search_current: 204,
            },
            "dark" => Self {
                code_keyword: 75,
                code_string: 114,
                code_number: 215,
                code_comment: 244,
                code_function: 74,
                code_type: 153,
                variable: 152,
                search_match: 111,
                search_current: 203,
            },
            "white" => Self {
                code_keyword: 26,
                code_string: 28,
                code_number: 166,
                code_comment: 102,
                code_function: 31,
                code_type: 61,
                variable: 24,
                search_match: 69,
                search_current: 160,
            },
            "solarized-dark" => Self {
                code_keyword: 136,
                code_string: 64,
                code_number: 166,
                code_comment: 102,
                code_function: 37,
                code_type: 61,
                variable: 109,
                search_match: 144,
                search_current: 166,
            },
            "solarized-light" => Self {
                code_keyword: 166,
                code_string: 64,
                code_number: 130,
                code_comment: 102,
                code_function: 31,
                code_type: 60,
                variable: 65,
                search_match: 137,
                search_current: 160,
            },
            "nord" => Self {
                code_keyword: 110,
                code_string: 150,
                code_number: 180,
                code_comment: 102,
                code_function: 81,
                code_type: 146,
                variable: 152,
                search_match: 109,
                search_current: 203,
            },
            "tokyo-night" => Self {
                code_keyword: 111,
                code_string: 114,
                code_number: 216,
                code_comment: 103,
                code_function: 75,
                code_type: 147,
                variable: 153,
                search_match: 147,
                search_current: 204,
            },
            "one-dark" => Self {
                code_keyword: 75,
                code_string: 114,
                code_number: 180,
                code_comment: 245,
                code_function: 74,
                code_type: 176,
                variable: 152,
                search_match: 111,
                search_current: 203,
            },
            _ => Self::default(),
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct CharStyle {
    bold: bool,
    italic: bool,
    dim: bool,
    strikethrough: bool,
    reverse: bool,
    fg: Option<u8>,
}

impl CharStyle {
    fn write_ansi(&self, buf: &mut String) {
        // Emit a single combined SGR sequence: \x1b[0;1;2;...m
        buf.push_str("\x1b[0");
        if self.bold {
            buf.push_str(";1");
        }
        if self.dim {
            buf.push_str(";2");
        }
        if self.italic {
            buf.push_str(";3");
        }
        if self.reverse {
            buf.push_str(";7");
        }
        if self.strikethrough {
            buf.push_str(";9");
        }
        if let Some(color) = self.fg {
            let _ = write!(buf, ";38;5;{color}");
        }
        buf.push('m');
    }

    fn is_plain(&self) -> bool {
        !self.bold
            && !self.italic
            && !self.dim
            && !self.strikethrough
            && !self.reverse
            && self.fg.is_none()
    }
}

pub struct RenderContext {
    in_code_block: bool,
    code_fence_lang: Option<String>,
    palette: RenderPalette,
}

impl RenderContext {
    #[cfg(test)]
    pub fn new() -> Self {
        Self {
            in_code_block: false,
            code_fence_lang: None,
            palette: RenderPalette::default(),
        }
    }

    pub fn new_with_palette(palette: RenderPalette) -> Self {
        Self {
            in_code_block: false,
            code_fence_lang: None,
            palette,
        }
    }

    /// Skip ahead through `lines` without rendering — just track code fence state.
    pub fn advance_lines(&mut self, lines: &[String]) {
        let mut state = markdown_tokens::FenceState {
            in_code_block: self.in_code_block,
            code_fence_lang: self.code_fence_lang.clone(),
        };
        for line in lines {
            markdown_tokens::advance_fence_state(&mut state, line);
        }
        self.in_code_block = state.in_code_block;
        self.code_fence_lang = state.code_fence_lang;
    }

    #[cfg(test)]
    pub fn advance_line(&mut self, text: &str) {
        let mut state = markdown_tokens::FenceState {
            in_code_block: self.in_code_block,
            code_fence_lang: self.code_fence_lang.clone(),
        };
        markdown_tokens::advance_fence_state(&mut state, text);
        self.in_code_block = state.in_code_block;
        self.code_fence_lang = state.code_fence_lang;
    }

    /// Render a single line with ANSI markdown formatting.
    /// Returns an ANSI string occupying exactly `width` visible characters.
    #[allow(dead_code)]
    pub fn render_line(
        &mut self,
        text: &str,
        width: usize,
        calc_ghost: Option<&str>,
        search_ranges: &[(usize, usize)],
        current_search_ranges: &[(usize, usize)],
        variable_names: &[String],
    ) -> String {
        self.render_line_window(
            text,
            width,
            0,
            calc_ghost,
            search_ranges,
            current_search_ranges,
            variable_names,
        )
    }

    pub fn render_line_window(
        &mut self,
        text: &str,
        width: usize,
        window_col: usize,
        calc_ghost: Option<&str>,
        search_ranges: &[(usize, usize)],
        current_search_ranges: &[(usize, usize)],
        variable_names: &[String],
    ) -> String {
        self.render_line_with_dim_ranges_window(
            text,
            width,
            window_col,
            calc_ghost,
            search_ranges,
            current_search_ranges,
            variable_names,
            &[],
            &[],
        )
    }

    pub fn render_line_window_with_reminder(
        &mut self,
        text: &str,
        width: usize,
        window_col: usize,
        calc_ghost: Option<&str>,
        reminder_ghost: Option<&str>,
        reminder_strikethrough: bool,
        search_ranges: &[(usize, usize)],
        current_search_ranges: &[(usize, usize)],
        variable_names: &[String],
    ) -> String {
        self.render_line_with_dim_ranges_window_with_reminder(
            text,
            width,
            window_col,
            calc_ghost,
            reminder_ghost,
            reminder_strikethrough,
            search_ranges,
            current_search_ranges,
            variable_names,
            &[],
            &[],
        )
    }

    #[allow(dead_code)]
    pub fn render_line_with_dim_ranges(
        &mut self,
        text: &str,
        width: usize,
        calc_ghost: Option<&str>,
        search_ranges: &[(usize, usize)],
        current_search_ranges: &[(usize, usize)],
        variable_names: &[String],
        dim_ranges: &[(usize, usize)],
        reverse_ranges: &[(usize, usize)],
    ) -> String {
        self.render_line_with_dim_ranges_window(
            text,
            width,
            0,
            calc_ghost,
            search_ranges,
            current_search_ranges,
            variable_names,
            dim_ranges,
            reverse_ranges,
        )
    }

    pub fn render_line_with_dim_ranges_window(
        &mut self,
        text: &str,
        width: usize,
        window_col: usize,
        calc_ghost: Option<&str>,
        search_ranges: &[(usize, usize)],
        current_search_ranges: &[(usize, usize)],
        variable_names: &[String],
        dim_ranges: &[(usize, usize)],
        reverse_ranges: &[(usize, usize)],
    ) -> String {
        self.render_line_with_dim_ranges_window_with_reminder(
            text,
            width,
            window_col,
            calc_ghost,
            None,
            false,
            search_ranges,
            current_search_ranges,
            variable_names,
            dim_ranges,
            reverse_ranges,
        )
    }

    pub fn render_line_with_dim_ranges_window_with_reminder(
        &mut self,
        text: &str,
        width: usize,
        window_col: usize,
        calc_ghost: Option<&str>,
        reminder_ghost: Option<&str>,
        reminder_strikethrough: bool,
        search_ranges: &[(usize, usize)],
        current_search_ranges: &[(usize, usize)],
        variable_names: &[String],
        dim_ranges: &[(usize, usize)],
        reverse_ranges: &[(usize, usize)],
    ) -> String {
        self.render_line_full(
            text,
            width,
            window_col,
            calc_ghost,
            reminder_ghost,
            reminder_strikethrough,
            search_ranges,
            current_search_ranges,
            variable_names,
            dim_ranges,
            reverse_ranges,
            &[],
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_line_full(
        &mut self,
        text: &str,
        width: usize,
        window_col: usize,
        calc_ghost: Option<&str>,
        reminder_ghost: Option<&str>,
        reminder_strikethrough: bool,
        search_ranges: &[(usize, usize)],
        current_search_ranges: &[(usize, usize)],
        variable_names: &[String],
        dim_ranges: &[(usize, usize)],
        reverse_ranges: &[(usize, usize)],
        red_ranges: &[(usize, usize)],
    ) -> String {
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        let mut styles = vec![CharStyle::default(); len];

        let info = markdown_tokens::classify_markdown_line(text);

        if info.is_code_fence {
            for s in &mut styles {
                s.dim = true;
            }
            let mut state = markdown_tokens::FenceState {
                in_code_block: self.in_code_block,
                code_fence_lang: self.code_fence_lang.clone(),
            };
            markdown_tokens::advance_fence_state(&mut state, text);
            self.in_code_block = state.in_code_block;
            self.code_fence_lang = state.code_fence_lang;
        } else if self.in_code_block {
            for style in &mut styles {
                style.dim = true;
                style.fg = None;
            }
            let code_tokens =
                markdown_tokens::tokenize_code_line(text, self.code_fence_lang.as_deref());
            apply_code_token_styles(&code_tokens, &mut styles, self.palette);
        } else {
            apply_line_styles_from_info(&info, &mut styles);
            let inline_tokens = markdown_tokens::tokenize_inline_markdown(text);
            apply_inline_token_styles(&inline_tokens, &mut styles);
            apply_variable_styles(&chars, &mut styles, variable_names, self.palette.variable);
        }

        for &(start, end) in dim_ranges {
            for s in styles.iter_mut().take(end.min(len)).skip(start.min(len)) {
                s.dim = true;
            }
        }

        for &(start, end) in search_ranges {
            for s in styles.iter_mut().take(end.min(len)).skip(start) {
                s.fg = Some(self.palette.search_match);
            }
        }

        for &(start, end) in current_search_ranges {
            for s in styles.iter_mut().take(end.min(len)).skip(start) {
                s.bold = true;
                s.fg = Some(self.palette.search_current);
            }
        }

        for &(start, end) in reverse_ranges {
            for s in styles.iter_mut().take(end.min(len)).skip(start) {
                s.reverse = true;
            }
        }

        for &(start, end) in red_ranges {
            for s in styles.iter_mut().take(end.min(len)).skip(start.min(len)) {
                s.fg = Some(196);
                s.bold = true;
                s.dim = false;
            }
        }

        let calc_prefix = if calc_ghost
            .map(|ghost| ghost.trim_start().starts_with('*'))
            .unwrap_or(false)
        {
            " "
        } else if contains_assignment_operator(text) {
            " = "
        } else {
            " → "
        };

        build_ansi_output_window(
            &chars,
            &styles,
            window_col,
            width,
            calc_ghost,
            calc_prefix,
            reminder_ghost,
            reminder_strikethrough,
        )
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
    let mut matches: Vec<(usize, usize)> = Vec::new();

    for raw in variable_names {
        let needle_text = raw.trim();
        if needle_text.is_empty() {
            continue;
        }
        let needle = needle_text.as_bytes();
        if needle.len() > bytes.len() {
            continue;
        }

        let mut idx = 0usize;
        while idx + needle.len() <= bytes.len() {
            let end = idx + needle.len();
            if &bytes[idx..end] == needle && has_variable_word_boundaries(bytes, idx, end) {
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

fn contains_assignment_operator(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() < 2 {
        return false;
    }

    for i in 0..bytes.len() - 1 {
        if bytes[i] != b':' || bytes[i + 1] != b'=' {
            continue;
        }

        if i > 0 && matches!(bytes[i - 1], b':' | b'!' | b'<' | b'>' | b'=') {
            continue;
        }
        if i + 2 < bytes.len() && bytes[i + 2] == b'=' {
            continue;
        }

        return true;
    }

    false
}

fn apply_variable_styles(
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

#[cfg(test)]
fn is_code_fence(text: &str) -> bool {
    markdown_tokens::is_code_fence(text)
}

#[cfg(test)]
fn fence_language(text: &str) -> Option<String> {
    markdown_tokens::parse_fence_language(text)
}

#[cfg(test)]
fn heading_marker_end(text: &str) -> Option<(usize, usize)> {
    let info = markdown_tokens::classify_markdown_line(text);
    Some((info.heading_level?, info.heading_marker_end?))
}

#[cfg(test)]
fn is_horizontal_rule(text: &str) -> bool {
    markdown_tokens::is_horizontal_rule(text)
}

#[cfg(test)]
pub fn list_marker_end(text: &str) -> Option<usize> {
    markdown_tokens::list_marker_end(text)
}

fn apply_line_styles_from_info(info: &MarkdownLineInfo, styles: &mut [CharStyle]) {
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

fn apply_inline_token_styles(tokens: &[markdown_tokens::InlineToken], styles: &mut [CharStyle]) {
    let len = styles.len();
    for token in tokens {
        let from = token.from.min(len);
        let to = token.to.min(len);
        if to <= from {
            continue;
        }

        for style in styles.iter_mut().take(to).skip(from) {
            match token.kind {
                InlineTokenType::Strong => style.bold = true,
                InlineTokenType::Emphasis => style.italic = true,
                InlineTokenType::Strikethrough => style.strikethrough = true,
                InlineTokenType::Code | InlineTokenType::CodeMarker => style.dim = true,
                InlineTokenType::LinkText => style.bold = true,
                InlineTokenType::LinkUrl | InlineTokenType::LinkMarker => style.dim = true,
            }
        }
    }
}

fn apply_code_token_styles(
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

// --- Output ---

#[allow(dead_code)]
fn build_ansi_output(
    chars: &[char],
    styles: &[CharStyle],
    width: usize,
    calc_ghost: Option<&str>,
    calc_prefix: &str,
) -> String {
    build_ansi_output_window(
        chars,
        styles,
        0,
        width,
        calc_ghost,
        calc_prefix,
        None,
        false,
    )
}

fn emit_window_cell(
    buf: &mut String,
    current: &mut CharStyle,
    style: CharStyle,
    ch: char,
    stream_col: &mut usize,
    emitted: &mut usize,
    window_col: usize,
    window_end: usize,
    width: usize,
) {
    if *stream_col >= window_col && *stream_col < window_end && *emitted < width {
        if style != *current {
            style.write_ansi(buf);
            *current = style;
        }
        buf.push(ch);
        *emitted += 1;
    }
    *stream_col += 1;
}

fn build_ansi_output_window(
    chars: &[char],
    styles: &[CharStyle],
    window_col: usize,
    width: usize,
    calc_ghost: Option<&str>,
    calc_prefix: &str,
    reminder_ghost: Option<&str>,
    reminder_strikethrough: bool,
) -> String {
    let mut buf = String::with_capacity(width * 4);
    let mut current = CharStyle::default();
    let mut stream_col = 0usize;
    let mut emitted = 0usize;
    let window_end = window_col.saturating_add(width);

    for (i, &ch) in chars.iter().enumerate() {
        let s = styles[i];
        if ch == '\t' {
            let tab_spaces = TAB_WIDTH - (stream_col % TAB_WIDTH);
            for _ in 0..tab_spaces {
                emit_window_cell(
                    &mut buf,
                    &mut current,
                    s,
                    ' ',
                    &mut stream_col,
                    &mut emitted,
                    window_col,
                    window_end,
                    width,
                );
            }
        } else {
            emit_window_cell(
                &mut buf,
                &mut current,
                s,
                ch,
                &mut stream_col,
                &mut emitted,
                window_col,
                window_end,
                width,
            );
        }
    }

    if let Some(ghost) = calc_ghost {
        let ghost_style = CharStyle {
            dim: true,
            italic: true,
            ..Default::default()
        };
        for ch in calc_prefix.chars().chain(ghost.chars()) {
            emit_window_cell(
                &mut buf,
                &mut current,
                ghost_style,
                ch,
                &mut stream_col,
                &mut emitted,
                window_col,
                window_end,
                width,
            );
        }
    }

    if let Some(ghost) = reminder_ghost {
        let reminder_style = CharStyle {
            dim: true,
            italic: true,
            strikethrough: reminder_strikethrough,
            ..Default::default()
        };
        let reminder_prefix = if calc_ghost.is_some() { "  " } else { " " };
        for ch in reminder_prefix.chars().chain(ghost.chars()) {
            emit_window_cell(
                &mut buf,
                &mut current,
                reminder_style,
                ch,
                &mut stream_col,
                &mut emitted,
                window_col,
                window_end,
                width,
            );
        }
    }

    if !current.is_plain() {
        buf.push_str(RESET);
    }
    while emitted < width {
        buf.push(' ');
        emitted += 1;
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_detected() {
        assert_eq!(heading_marker_end("# Hello"), Some((1, 2)));
        assert_eq!(heading_marker_end("  ### Foo"), Some((3, 6)));
        assert_eq!(heading_marker_end("Not a heading"), None);
        assert_eq!(heading_marker_end("#nospace"), None);
    }

    #[test]
    fn list_marker_detected() {
        assert_eq!(list_marker_end("- item"), Some(2));
        assert_eq!(list_marker_end("  * item"), Some(4));
        assert_eq!(list_marker_end("1. item"), Some(3));
        assert_eq!(list_marker_end("1.1 item"), Some(4));
        assert_eq!(list_marker_end("  -> item"), Some(5));
        assert_eq!(list_marker_end("10. item"), Some(4));
        assert_eq!(list_marker_end("plain text"), None);
    }

    #[test]
    fn horizontal_rule_detected() {
        assert!(is_horizontal_rule("---"));
        assert!(is_horizontal_rule("- - -"));
        assert!(is_horizontal_rule("***"));
        assert!(!is_horizontal_rule("--"));
        assert!(!is_horizontal_rule("hello"));
    }

    #[test]
    fn code_fence_detected() {
        assert!(is_code_fence("```"));
        assert!(is_code_fence("  ```rust"));
        assert!(!is_code_fence("hello"));
        assert_eq!(fence_language("```rust"), Some("rust".to_string()));
        assert_eq!(fence_language("```typescript"), Some("ts".to_string()));
    }

    #[test]
    fn render_context_tracks_fences() {
        let mut ctx = RenderContext::new();
        ctx.advance_line("normal line");
        assert!(!ctx.in_code_block);
        ctx.advance_line("```rust");
        assert!(ctx.in_code_block);
        assert_eq!(ctx.code_fence_lang.as_deref(), Some("rust"));
        ctx.advance_line("code line");
        assert!(ctx.in_code_block);
        ctx.advance_line("```");
        assert!(!ctx.in_code_block);
        assert_eq!(ctx.code_fence_lang, None);
    }

    #[test]
    fn render_plain_pads_to_width() {
        let mut ctx = RenderContext::new();
        let out = ctx.render_line("hi", 10, None, &[], &[], &[]);
        // "hi" + 8 spaces = 10 visible chars (plus potential ANSI reset)
        let visible: String = strip_ansi(&out);
        assert_eq!(visible.len(), 10);
        assert!(visible.starts_with("hi"));
    }

    #[test]
    fn render_window_skips_prefix_columns() {
        let mut ctx = RenderContext::new();
        let out = ctx.render_line_window("abcdef", 4, 2, None, &[], &[], &[]);
        assert_eq!(strip_ansi(&out), "cdef");
    }

    #[test]
    fn render_window_respects_tab_expansion() {
        let mut ctx = RenderContext::new();
        let out = ctx.render_line_window("\tabcd", 4, 2, None, &[], &[], &[]);
        assert_eq!(strip_ansi(&out), "  ab");
    }

    #[test]
    fn render_calc_ghost_appended() {
        let mut ctx = RenderContext::new();
        let out = ctx.render_line("2+2", 30, Some("4"), &[], &[], &[]);
        let visible = strip_ansi(&out);
        assert!(visible.contains("→ 4"));
    }

    #[test]
    fn render_assignment_calc_ghost_uses_equals_prefix() {
        let mut ctx = RenderContext::new();
        let out = ctx.render_line("value := 2 + 2", 30, Some("4"), &[], &[], &[]);
        let visible = strip_ansi(&out);
        assert!(visible.contains("= 4"));
    }

    #[test]
    fn render_formula_explanation_ghost_does_not_add_default_arrow_prefix() {
        let mut ctx = RenderContext::new();
        let out = ctx.render_line("| a | 4* |", 40, Some("* ➜ avg_col()"), &[], &[], &[]);
        let visible = strip_ansi(&out);
        assert!(visible.contains(" * ➜ avg_col()"));
        assert!(!visible.contains("→ * ➜ avg_col()"));
    }

    #[test]
    fn render_reminder_ghost_is_appended_without_calc_prefix() {
        let mut ctx = RenderContext::new();
        let out = ctx.render_line_window_with_reminder(
            "- [ ] task",
            40,
            0,
            None,
            Some("⏰ 23.04.2026. 14:34"),
            false,
            &[],
            &[],
            &[],
        );
        let visible = strip_ansi(&out);
        assert!(visible.contains(" ⏰ 23.04.2026. 14:34"));
        assert!(!visible.contains("→ ⏰"));
    }

    #[test]
    fn render_expired_reminder_ghost_uses_strikethrough_style() {
        let mut ctx = RenderContext::new();
        let out = ctx.render_line_window_with_reminder(
            "task",
            40,
            0,
            None,
            Some("⏰ 23.04.2026. 14:34"),
            true,
            &[],
            &[],
            &[],
        );
        assert!(out.contains("\x1b[0;2;3;9m"));
    }

    #[test]
    fn render_line_with_dim_ranges_dims_marker_character() {
        let mut ctx = RenderContext::new();
        let out = ctx.render_line_with_dim_ranges("abc*", 12, None, &[], &[], &[], &[(3, 4)], &[]);
        assert!(out.contains("\x1b[0;2m*"));
    }

    #[test]
    fn render_code_block_adds_syntax_color_sequences() {
        let mut ctx = RenderContext::new();
        let palette = RenderPalette::default();
        let _ = ctx.render_line("```rust", 60, None, &[], &[], &[]);
        let out = ctx.render_line("let total = 42 // note", 60, None, &[], &[], &[]);
        assert!(out.contains(&format!("38;5;{}", palette.code_keyword)));
        assert!(out.contains(&format!("38;5;{}", palette.code_number)));
        assert!(out.contains(&format!("38;5;{}", palette.code_comment)));
    }

    #[test]
    fn render_expands_tabs_into_spaces() {
        let mut ctx = RenderContext::new();
        let out = ctx.render_line("a\tb", 12, None, &[], &[], &[]);
        let visible = strip_ansi(&out);
        assert!(visible.starts_with("a   b"));
        assert_eq!(visible.len(), 12);
    }

    #[test]
    fn render_highlights_variables_in_bold_with_distinct_color() {
        let mut ctx = RenderContext::new();
        let palette = RenderPalette::default();
        let vars = vec!["subtotal".to_string(), "tax rate".to_string()];
        let out = ctx.render_line("total = subtotal + tax rate", 80, None, &[], &[], &vars);
        assert!(out.contains(&format!("0;1;38;5;{}", palette.variable)));
    }

    #[test]
    fn render_variable_highlighting_is_case_sensitive() {
        let mut ctx = RenderContext::new();
        let palette = RenderPalette::default();
        let vars = vec!["daily".to_string()];

        let exact = ctx.render_line("daily = 1", 40, None, &[], &[], &vars);
        assert!(exact.contains(&format!("0;1;38;5;{}", palette.variable)));

        let different_case = ctx.render_line("Daily note", 40, None, &[], &[], &vars);
        assert!(!different_case.contains(&format!("0;1;38;5;{}", palette.variable)));
    }

    #[test]
    fn render_search_uses_distinct_colors_for_current_and_other_matches() {
        let mut ctx = RenderContext::new();
        let palette = RenderPalette::default();
        let out = ctx.render_line("alpha beta alpha", 40, None, &[(0, 5)], &[(11, 16)], &[]);
        assert!(out.contains(&format!("0;38;5;{}", palette.search_match)));
        assert!(out.contains(&format!("0;1;38;5;{}", palette.search_current)));
    }

    #[test]
    fn render_supports_custom_palette_for_search_highlights() {
        let custom = RenderPalette {
            search_match: 135,
            search_current: 196,
            ..RenderPalette::default()
        };
        let mut ctx = RenderContext::new_with_palette(custom);
        let out = ctx.render_line("alpha beta alpha", 40, None, &[(0, 5)], &[(11, 16)], &[]);
        assert!(out.contains("0;38;5;135"));
        assert!(out.contains("0;1;38;5;196"));
    }

    #[test]
    fn render_palette_supports_named_color_schemes() {
        let palette = RenderPalette::for_color_scheme("gruvbox-dark");
        assert_eq!(palette.code_keyword, 214);
        assert_eq!(palette.search_current, 167);

        let normalized = RenderPalette::for_color_scheme("Gruvbox Dark");
        assert_eq!(normalized.code_keyword, 214);
    }

    #[test]
    fn render_palette_falls_back_to_default_for_unknown_scheme() {
        let palette = RenderPalette::for_color_scheme("unknown-scheme");
        let default_palette = RenderPalette::default();
        assert_eq!(palette.code_keyword, default_palette.code_keyword);
        assert_eq!(palette.search_match, default_palette.search_match);
    }

    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut in_esc = false;
        for ch in s.chars() {
            if ch == '\x1b' {
                in_esc = true;
            } else if in_esc {
                if ch.is_ascii_alphabetic() {
                    in_esc = false;
                }
            } else {
                out.push(ch);
            }
        }
        out
    }
}
