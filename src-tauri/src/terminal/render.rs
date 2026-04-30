use crate::editor_core::markdown_tokens;
pub use crate::terminal::markdown_view::collapse_markdown_line_for_cursor;
use crate::terminal::markdown_view::{hidden_line_prefix_marker_ranges, normalize_hidden_ranges};
use crate::terminal::render_styles::{
    apply_code_token_styles, apply_inline_token_styles, apply_line_styles_from_info,
    apply_variable_styles, CharStyle,
};
pub use crate::terminal::theme::RenderPalette;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

pub const RESET: &str = "\x1b[0m";
pub const TAB_WIDTH: usize = 4;
const INLINE_TOKEN_CACHE_MAX_ENTRIES: usize = 4096;

struct InlineTokenCache {
    entries: HashMap<String, (Arc<Vec<markdown_tokens::InlineToken>>, u64)>,
    tick: u64,
}

impl InlineTokenCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            tick: 0,
        }
    }

    fn get(&mut self, text: &str) -> Arc<Vec<markdown_tokens::InlineToken>> {
        self.tick = self.tick.wrapping_add(1);
        if let Some((tokens, last_used)) = self.entries.get_mut(text) {
            *last_used = self.tick;
            return Arc::clone(tokens);
        }
        let tokens = Arc::new(markdown_tokens::tokenize_inline_markdown(text));
        self.entries
            .insert(text.to_string(), (Arc::clone(&tokens), self.tick));
        while self.entries.len() > INLINE_TOKEN_CACHE_MAX_ENTRIES {
            let Some(evict_key) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, last_used))| *last_used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.entries.remove(evict_key.as_str());
        }
        tokens
    }
}

thread_local! {
    static INLINE_TOKEN_CACHE: RefCell<InlineTokenCache> = RefCell::new(InlineTokenCache::new());
}

fn cached_inline_tokens(text: &str) -> Arc<Vec<markdown_tokens::InlineToken>> {
    INLINE_TOKEN_CACHE.with(|cache| cache.borrow_mut().get(text))
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
        underline_ranges: &[(usize, usize)],
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
            let inline_tokens = cached_inline_tokens(text);
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
                inline_tokens.as_ref()
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

        for &(start, end) in underline_ranges {
            for s in styles.iter_mut().take(end.min(len)).skip(start.min(len)) {
                s.underline = true;
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
