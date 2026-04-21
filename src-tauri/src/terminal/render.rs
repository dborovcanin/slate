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

    #[cfg(test)]
    pub fn new_with_palette(palette: RenderPalette) -> Self {
        Self {
            in_code_block: false,
            code_fence_lang: None,
            palette,
        }
    }

    pub fn with_fence_state(
        in_code_block: bool,
        code_fence_lang: Option<String>,
        palette: RenderPalette,
    ) -> Self {
        Self {
            in_code_block,
            code_fence_lang,
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

    #[allow(dead_code)]
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
        self.render_line_window_with_reminder_cursor(
            text,
            width,
            window_col,
            calc_ghost,
            reminder_ghost,
            reminder_strikethrough,
            search_ranges,
            current_search_ranges,
            variable_names,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_line_window_with_reminder_cursor(
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
        active_cursor_col: Option<usize>,
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
            active_cursor_col,
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
            None,
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
        active_cursor_col: Option<usize>,
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
            active_cursor_col,
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
        active_cursor_col: Option<usize>,
    ) -> String {
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        let mut styles = vec![CharStyle::default(); len];
        let mut hidden_ranges: Vec<(usize, usize)> = Vec::new();

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
            hidden_ranges.extend(hidden_line_prefix_marker_ranges(
                &info,
                len,
                active_cursor_col.is_some(),
            ));
            let inline_tokens = markdown_tokens::tokenize_inline_markdown(text);
            // Markdown emphasis (`*x*`, `***x***`) inside table rows is
            // ambiguous with our formula markers (`value*`, `value***`) and
            // would otherwise leak italic/bold across cell boundaries.
            // Skip inline emphasis tokens for table rows; keep code, links,
            // and strikethrough (which are not asterisk-based).
            let is_table_row = text.trim_start().starts_with('|');
            let filtered_tokens: Vec<markdown_tokens::InlineToken>;
            let inline_tokens_to_apply: &[markdown_tokens::InlineToken] = if is_table_row {
                filtered_tokens = inline_tokens
                    .iter()
                    .filter(|t| {
                        !matches!(
                            t.kind,
                            markdown_tokens::InlineTokenType::Strong
                                | markdown_tokens::InlineTokenType::Emphasis
                        )
                    })
                    .cloned()
                    .collect();
                &filtered_tokens
            } else {
                &inline_tokens
            };
            apply_inline_token_styles(
                inline_tokens_to_apply,
                &mut styles,
                &mut hidden_ranges,
                active_cursor_col,
            );
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

        hidden_ranges = normalize_hidden_ranges(hidden_ranges, len);

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
            &hidden_ranges,
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

fn hidden_line_prefix_marker_ranges(
    info: &MarkdownLineInfo,
    len: usize,
    reveal_prefix: bool,
) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    if reveal_prefix {
        return ranges;
    }
    if let Some(marker_end) = info.heading_marker_end {
        let to = marker_end.min(len);
        if to > 0 {
            ranges.push((0, to));
        }
    }
    if let Some(marker_end) = info.quote_marker_end {
        let to = marker_end.min(len);
        if to > 0 {
            ranges.push((0, to));
        }
    }
    ranges
}

fn normalize_hidden_ranges(mut ranges: Vec<(usize, usize)>, len: usize) -> Vec<(usize, usize)> {
    if ranges.is_empty() || len == 0 {
        return Vec::new();
    }
    for range in &mut ranges {
        range.0 = range.0.min(len);
        range.1 = range.1.min(len);
    }
    ranges.retain(|(from, to)| to > from);
    if ranges.is_empty() {
        return Vec::new();
    }
    ranges.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (from, to) in ranges {
        if let Some(last) = merged.last_mut() {
            if from <= last.1 {
                if to > last.1 {
                    last.1 = to;
                }
                continue;
            }
        }
        merged.push((from, to));
    }
    merged
}

fn should_reveal_inline_marker(
    tokens: &[markdown_tokens::InlineToken],
    component_ranges: &[markdown_tokens::InlineMarkerComponentRange],
    marker_index: usize,
    active_cursor_col: Option<usize>,
) -> bool {
    let Some(cursor_col) = active_cursor_col else {
        return false;
    };
    let Some(marker) = tokens.get(marker_index) else {
        return false;
    };
    if !markdown_tokens::is_inline_marker_token_kind(marker.kind) {
        return false;
    }
    let Some((from, to)) = component_ranges
        .iter()
        .find(|range| marker.from >= range.from && marker.to <= range.to)
        .map(|range| (range.from, range.to))
    else {
        return false;
    };
    cursor_col >= from && cursor_col <= to
}

fn apply_inline_token_styles(
    tokens: &[markdown_tokens::InlineToken],
    styles: &mut [CharStyle],
    hidden_ranges: &mut Vec<(usize, usize)>,
    active_cursor_col: Option<usize>,
) {
    let len = styles.len();
    let component_ranges = markdown_tokens::inline_marker_component_ranges_from_tokens(tokens);
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
                InlineTokenType::LinkText => style.bold = true,
                InlineTokenType::LinkUrl | InlineTokenType::LinkMarker => style.dim = true,
            }
        }
    }
}

fn hidden_ranges_for_markdown_line(
    text: &str,
    active_cursor_col: Option<usize>,
) -> Vec<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    if len == 0 {
        return Vec::new();
    }
    let info = markdown_tokens::classify_markdown_line(text);
    let mut hidden_ranges =
        hidden_line_prefix_marker_ranges(&info, len, active_cursor_col.is_some());
    let inline_tokens = markdown_tokens::tokenize_inline_markdown(text);
    let is_table_row = text.trim_start().starts_with('|');
    let filtered_tokens: Vec<markdown_tokens::InlineToken>;
    let inline_tokens_to_apply: &[markdown_tokens::InlineToken] = if is_table_row {
        filtered_tokens = inline_tokens
            .iter()
            .filter(|t| {
                !matches!(
                    t.kind,
                    markdown_tokens::InlineTokenType::Strong
                        | markdown_tokens::InlineTokenType::Emphasis
                )
            })
            .cloned()
            .collect();
        &filtered_tokens
    } else {
        &inline_tokens
    };
    let mut dummy_styles = vec![CharStyle::default(); len];
    apply_inline_token_styles(
        inline_tokens_to_apply,
        &mut dummy_styles,
        &mut hidden_ranges,
        active_cursor_col,
    );
    normalize_hidden_ranges(hidden_ranges, len)
}

pub fn collapse_markdown_line_for_cursor(text: &str, cursor_col: usize) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    if len == 0 {
        return (String::new(), 0);
    }
    let clamped_cursor = cursor_col.min(len);
    let hidden_ranges = hidden_ranges_for_markdown_line(text, Some(clamped_cursor));
    if hidden_ranges.is_empty() {
        return (text.to_string(), clamped_cursor);
    }

    let mut mapped_cursor = clamped_cursor;
    for (from, to) in &hidden_ranges {
        if *from >= clamped_cursor {
            break;
        }
        let removed = if *to <= clamped_cursor {
            to - from
        } else {
            clamped_cursor.saturating_sub(*from)
        };
        mapped_cursor = mapped_cursor.saturating_sub(removed);
    }

    let mut out = String::with_capacity(chars.len());
    let mut hidden_iter = hidden_ranges.iter().peekable();
    for (idx, ch) in chars.iter().enumerate() {
        while let Some((_, end)) = hidden_iter.peek() {
            if idx >= *end {
                hidden_iter.next();
            } else {
                break;
            }
        }
        if let Some((start, end)) = hidden_iter.peek() {
            if idx >= *start && idx < *end {
                continue;
            }
        }
        out.push(*ch);
    }
    let out_len = out.chars().count();
    (out, mapped_cursor.min(out_len))
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
        &[],
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
    use unicode_width::UnicodeWidthChar;
    let ch_width = ch.width().unwrap_or(0).max(1);
    let ch_end = *stream_col + ch_width;

    // Compute which portion of this char's columns fall inside [window_col, window_end).
    let visible_start = (*stream_col).max(window_col);
    let visible_end = ch_end.min(window_end);

    if visible_start < visible_end && *emitted < width {
        let slots = (visible_end - visible_start).min(width - *emitted);
        if style != *current {
            style.write_ansi(buf);
            *current = style;
        }
        if *stream_col >= window_col && ch_end <= window_end && *emitted + ch_width <= width {
            buf.push(ch);
            *emitted += ch_width;
        } else {
            // Wide char clips a window boundary — fill the visible slots with spaces.
            for _ in 0..slots {
                buf.push(' ');
            }
            *emitted += slots;
        }
    }

    *stream_col += ch_width;
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
    hidden_ranges: &[(usize, usize)],
) -> String {
    let mut buf = String::with_capacity(width * 4);
    let mut current = CharStyle::default();
    let mut stream_col = 0usize;
    let mut emitted = 0usize;
    let window_end = window_col.saturating_add(width);

    let mut hidden_iter = hidden_ranges.iter().peekable();
    for (i, &ch) in chars.iter().enumerate() {
        while let Some((_, end)) = hidden_iter.peek() {
            if i >= *end {
                hidden_iter.next();
            } else {
                break;
            }
        }
        if let Some((start, end)) = hidden_iter.peek() {
            if i >= *start && i < *end {
                continue;
            }
        }
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

    #[test]
    fn render_hides_inline_markers_off_cursor_and_reveals_on_marker_boundaries() {
        let mut ctx = RenderContext::new();
        let hidden = ctx.render_line_window_with_reminder_cursor(
            "**bold**",
            8,
            0,
            None,
            None,
            false,
            &[],
            &[],
            &[],
            None,
        );
        let hidden_visible = strip_ansi(&hidden);
        assert!(hidden_visible.starts_with("bold"));
        assert!(!hidden_visible.contains('*'));

        let reveal_open = ctx.render_line_window_with_reminder_cursor(
            "**bold**",
            8,
            0,
            None,
            None,
            false,
            &[],
            &[],
            &[],
            Some(1),
        );
        assert_eq!(strip_ansi(&reveal_open), "**bold**");

        let reveal_close = ctx.render_line_window_with_reminder_cursor(
            "**bold**",
            8,
            0,
            None,
            None,
            false,
            &[],
            &[],
            &[],
            Some(6),
        );
        assert_eq!(strip_ansi(&reveal_close), "**bold**");
    }

    #[test]
    fn render_hides_heading_marker_off_cursor_line_and_reveals_on_active_line() {
        let mut ctx = RenderContext::new();
        let hidden = ctx.render_line_window_with_reminder_cursor(
            "# Heading",
            9,
            0,
            None,
            None,
            false,
            &[],
            &[],
            &[],
            None,
        );
        let hidden_visible = strip_ansi(&hidden);
        assert!(hidden_visible.starts_with("Heading"));
        assert!(!hidden_visible.contains('#'));

        let revealed = ctx.render_line_window_with_reminder_cursor(
            "# Heading",
            9,
            0,
            None,
            None,
            false,
            &[],
            &[],
            &[],
            Some(0),
        );
        assert_eq!(strip_ansi(&revealed), "# Heading");
    }

    #[test]
    fn render_reveals_only_active_inline_component_on_cursor_line() {
        let mut ctx = RenderContext::new();

        let reveal_strong = ctx.render_line_window_with_reminder_cursor(
            "**bold** and `code`",
            32,
            0,
            None,
            None,
            false,
            &[],
            &[],
            &[],
            Some(3),
        );
        let strong_visible = strip_ansi(&reveal_strong);
        assert!(strong_visible.starts_with("**bold** and code"));
        assert!(!strong_visible.contains("`code`"));

        let reveal_code = ctx.render_line_window_with_reminder_cursor(
            "**bold** and `code`",
            32,
            0,
            None,
            None,
            false,
            &[],
            &[],
            &[],
            Some(14),
        );
        let code_visible = strip_ansi(&reveal_code);
        assert!(code_visible.starts_with("bold and `code`"));
        assert!(!code_visible.contains("**bold**"));
    }

    #[test]
    fn collapse_markdown_line_for_cursor_removes_hidden_marker_gaps_and_maps_cursor() {
        let (collapsed, mapped_col) = collapse_markdown_line_for_cursor("`code` tail **bold**", 10);
        assert_eq!(collapsed, "code tail bold");
        assert_eq!(mapped_col, 8);
    }

    #[test]
    fn collapse_markdown_line_for_cursor_does_not_collapse_mid_word_underscores() {
        let (collapsed, mapped_col) = collapse_markdown_line_for_cursor("this_Is_my_Word", 7);
        assert_eq!(collapsed, "this_Is_my_Word");
        assert_eq!(mapped_col, 7);
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
