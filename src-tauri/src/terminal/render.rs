use std::fmt::Write as _;

use crate::editor_core::markdown_tokens::{self, CodeTokenType, InlineTokenType, MarkdownLineInfo};

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const TAB_WIDTH: usize = 4;
const FG_CODE_KEYWORD: u8 = 81;
const FG_CODE_STRING: u8 = 114;
const FG_CODE_NUMBER: u8 = 215;
const FG_CODE_COMMENT: u8 = 244;
const FG_CODE_FUNCTION: u8 = 74;
const FG_CODE_TYPE: u8 = 183;
const FG_VARIABLE: u8 = 179;
const FG_SEARCH_MATCH: u8 = 141;
const FG_SEARCH_CURRENT: u8 = 203;

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
}

impl RenderContext {
    pub fn new() -> Self {
        Self {
            in_code_block: false,
            code_fence_lang: None,
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
    pub fn render_line(
        &mut self,
        text: &str,
        width: usize,
        calc_ghost: Option<&str>,
        search_ranges: &[(usize, usize)],
        current_search_ranges: &[(usize, usize)],
        variable_names: &[String],
    ) -> String {
        self.render_line_with_dim_ranges(
            text,
            width,
            calc_ghost,
            search_ranges,
            current_search_ranges,
            variable_names,
            &[],
            &[],
        )
    }

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
            let code_tokens = markdown_tokens::tokenize_code_line(text, self.code_fence_lang.as_deref());
            apply_code_token_styles(&code_tokens, &mut styles);
        } else {
            apply_line_styles_from_info(&info, &mut styles);
            let inline_tokens = markdown_tokens::tokenize_inline_markdown(text);
            apply_inline_token_styles(&inline_tokens, &mut styles);
            apply_variable_styles(&chars, &mut styles, variable_names);
        }

        for &(start, end) in dim_ranges {
            for s in styles.iter_mut().take(end.min(len)).skip(start.min(len)) {
                s.dim = true;
            }
        }

        for &(start, end) in search_ranges {
            for s in styles.iter_mut().take(end.min(len)).skip(start) {
                s.fg = Some(FG_SEARCH_MATCH);
            }
        }

        for &(start, end) in current_search_ranges {
            for s in styles.iter_mut().take(end.min(len)).skip(start) {
                s.bold = true;
                s.fg = Some(FG_SEARCH_CURRENT);
            }
        }

        for &(start, end) in reverse_ranges {
            for s in styles.iter_mut().take(end.min(len)).skip(start) {
                s.reverse = true;
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

        build_ansi_output(&chars, &styles, width, calc_ghost, calc_prefix)
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

    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut matches: Vec<(usize, usize)> = Vec::new();

    for raw in variable_names {
        let needle_text = raw.trim().to_ascii_lowercase();
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

fn apply_variable_styles(chars: &[char], styles: &mut [CharStyle], variable_names: &[String]) {
    if chars.is_empty() || variable_names.is_empty() {
        return;
    }

    let text: String = chars.iter().collect();
    for (start, end) in find_variable_ranges(&text, variable_names) {
        for style in styles.iter_mut().take(end).skip(start) {
            style.fg = Some(FG_VARIABLE);
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

fn apply_code_token_styles(tokens: &[markdown_tokens::CodeToken], styles: &mut [CharStyle]) {
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
                    style.fg = Some(FG_CODE_KEYWORD);
                    style.dim = false;
                }
                CodeTokenType::String => {
                    style.fg = Some(FG_CODE_STRING);
                    style.dim = false;
                }
                CodeTokenType::Number => {
                    style.fg = Some(FG_CODE_NUMBER);
                    style.dim = false;
                }
                CodeTokenType::Comment => {
                    style.fg = Some(FG_CODE_COMMENT);
                    style.dim = true;
                }
                CodeTokenType::Function => {
                    style.fg = Some(FG_CODE_FUNCTION);
                    style.dim = false;
                }
                CodeTokenType::Type => {
                    style.fg = Some(FG_CODE_TYPE);
                    style.dim = false;
                }
            }
        }
    }
}

// --- Output ---

fn build_ansi_output(
    chars: &[char],
    styles: &[CharStyle],
    width: usize,
    calc_ghost: Option<&str>,
    calc_prefix: &str,
) -> String {
    let mut buf = String::with_capacity(width * 4);
    let mut current = CharStyle::default();
    let mut visible = 0;

    for (i, &ch) in chars.iter().enumerate() {
        if visible >= width {
            break;
        }
        let s = styles[i];
        if s != current {
            s.write_ansi(&mut buf);
            current = s;
        }
        if ch == '\t' {
            let tab_spaces = TAB_WIDTH - (visible % TAB_WIDTH);
            for _ in 0..tab_spaces {
                if visible >= width {
                    break;
                }
                buf.push(' ');
                visible += 1;
            }
        } else {
            buf.push(ch);
            visible += 1;
        }
    }

    if let Some(ghost) = calc_ghost {
        if visible < width {
            let ghost_style = CharStyle {
                dim: true,
                italic: true,
                ..Default::default()
            };
            if current != ghost_style {
                ghost_style.write_ansi(&mut buf);
                current = ghost_style;
            }
            for ch in calc_prefix.chars().chain(ghost.chars()) {
                if visible >= width {
                    break;
                }
                buf.push(ch);
                visible += 1;
            }
        }
    }

    if !current.is_plain() {
        buf.push_str(RESET);
    }
    while visible < width {
        buf.push(' ');
        visible += 1;
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
    fn render_line_with_dim_ranges_dims_marker_character() {
        let mut ctx = RenderContext::new();
        let out = ctx.render_line_with_dim_ranges("abc*", 12, None, &[], &[], &[], &[(3, 4)], &[]);
        assert!(out.contains("\x1b[0;2m*"));
    }

    #[test]
    fn render_code_block_adds_syntax_color_sequences() {
        let mut ctx = RenderContext::new();
        let _ = ctx.render_line("```rust", 60, None, &[], &[], &[]);
        let out = ctx.render_line("let total = 42 // note", 60, None, &[], &[], &[]);
        assert!(out.contains("38;5;81"));
        assert!(out.contains("38;5;215"));
        assert!(out.contains("38;5;244"));
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
        let vars = vec!["subtotal".to_string(), "tax rate".to_string()];
        let out = ctx.render_line("total = subtotal + tax rate", 80, None, &[], &[], &vars);
        assert!(out.contains(&format!("0;1;38;5;{FG_VARIABLE}")));
    }

    #[test]
    fn render_search_uses_distinct_colors_for_current_and_other_matches() {
        let mut ctx = RenderContext::new();
        let out = ctx.render_line(
            "alpha beta alpha",
            40,
            None,
            &[(0, 5)],
            &[(11, 16)],
            &[],
        );
        assert!(out.contains(&format!("0;38;5;{FG_SEARCH_MATCH}")));
        assert!(out.contains(&format!("0;1;38;5;{FG_SEARCH_CURRENT}")));
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
