use crate::editor_core::markdown_tokens;
use crate::terminal::canvas::contrast_fg_for_bg;
use ratatui::buffer::Buffer;
use ratatui::style::Style;
pub use crate::terminal::markdown_view::collapse_markdown_line_for_cursor_with_formatting_boundary_exit;
use crate::terminal::markdown_view::{hidden_line_prefix_marker_ranges, normalize_hidden_ranges};
use crate::terminal::render_styles::{
    apply_code_token_styles, apply_inline_token_styles, apply_line_styles_from_info,
    apply_variable_styles, CharStyle,
};
pub use crate::terminal::theme::RenderPalette;
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Arc;

pub const TAB_WIDTH: usize = 4;
const INLINE_TOKEN_CACHE_MAX_ENTRIES: usize = 512;

struct InlineTokenCache {
    entries: FxHashMap<String, Arc<Vec<markdown_tokens::InlineToken>>>,
    order: VecDeque<String>,
}

impl InlineTokenCache {
    fn new() -> Self {
        Self {
            entries: FxHashMap::default(),
            order: VecDeque::new(),
        }
    }

    fn get(&mut self, text: &str) -> Arc<Vec<markdown_tokens::InlineToken>> {
        if let Some(tokens) = self.entries.get(text) {
            return Arc::clone(tokens);
        }
        let tokens = Arc::new(markdown_tokens::tokenize_inline_markdown(text));
        self.entries.insert(text.to_string(), Arc::clone(&tokens));
        self.order.push_back(text.to_string());
        while self.entries.len() > INLINE_TOKEN_CACHE_MAX_ENTRIES {
            let Some(evict_key) = self.order.pop_front() else {
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
    render_as_plain_code: bool,
    forced_code_lang: Option<String>,
    palette: RenderPalette,
}

impl RenderContext {
    #[cfg(test)]
    pub fn new() -> Self {
        Self {
            in_code_block: false,
            code_fence_lang: None,
            render_as_plain_code: false,
            forced_code_lang: None,
            palette: RenderPalette::default(),
        }
    }

    #[cfg(test)]
    pub fn new_with_palette(palette: RenderPalette) -> Self {
        Self {
            in_code_block: false,
            code_fence_lang: None,
            render_as_plain_code: false,
            forced_code_lang: None,
            palette,
        }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub fn with_fence_state(
        in_code_block: bool,
        code_fence_lang: Option<String>,
        palette: RenderPalette,
    ) -> Self {
        Self::with_syntax_mode(in_code_block, code_fence_lang, false, None, palette)
    }

    pub fn with_syntax_mode(
        in_code_block: bool,
        code_fence_lang: Option<String>,
        render_as_plain_code: bool,
        forced_code_lang: Option<String>,
        palette: RenderPalette,
    ) -> Self {
        Self {
            in_code_block,
            code_fence_lang,
            render_as_plain_code,
            forced_code_lang,
            palette,
        }
    }

    /// Skip ahead through `lines` without rendering — just track code fence state.
    pub fn advance_lines(&mut self, lines: &[String]) {
        if self.render_as_plain_code {
            return;
        }
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
        if self.render_as_plain_code {
            return;
        }
        let mut state = markdown_tokens::FenceState {
            in_code_block: self.in_code_block,
            code_fence_lang: self.code_fence_lang.clone(),
        };
        markdown_tokens::advance_fence_state(&mut state, text);
        self.in_code_block = state.in_code_block;
        self.code_fence_lang = state.code_fence_lang;
    }

    /// Renders one line into exactly `width` cells of `buf` starting at
    /// (`x`, `y`), showing display columns from `window_col` onward.
    #[allow(clippy::too_many_arguments)]
    pub fn render_line(
        &mut self,
        text: &str,
        width: usize,
        window_col: usize,
        deco: &LineDecorations<'_>,
        buf: &mut Buffer,
        x: u16,
        y: u16,
    ) {
        let LineDecorations {
            calc_ghost,
            reminder_ghost,
            reminder_strikethrough,
            search_ranges,
            current_search_ranges,
            variable_names,
            dim_ranges,
            selection_ranges: reverse_ranges,
            accent_ranges: red_ranges,
            underline_ranges,
            active_cursor_col,
        } = *deco;
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        let is_table_row = text.trim_start().starts_with('|');
        let is_table_continuation_line =
            crate::editor_core::table::is_table_continuation_line(text);
        let info = if self.render_as_plain_code {
            None
        } else {
            Some(markdown_tokens::classify_markdown_line(text))
        };
        let is_code_block_line = !self.render_as_plain_code
            && (self.in_code_block || info.as_ref().is_some_and(|i| i.is_code_fence));
        let base_style = CharStyle {
            fg: Some(self.palette.text_fg()),
            bg: Some(if is_code_block_line {
                self.palette.code_block_bg
            } else {
                self.palette.surface_bg()
            }),
            ..Default::default()
        };
        let mut styles = vec![base_style; len];
        let mut hidden_ranges: Vec<(usize, usize)> = Vec::new();
        if self.render_as_plain_code {
            if let Some(lang) = self.forced_code_lang.as_deref() {
                let code_tokens = markdown_tokens::tokenize_code_line(text, Some(lang));
                apply_code_token_styles(&code_tokens, &mut styles, self.palette);
            }
            apply_variable_styles(&chars, &mut styles, variable_names, self.palette.variable);
        } else if is_table_continuation_line {
            // `|>` is a structural continuation marker, not editable cell content.
            if let Some(style) = styles.get_mut(1) {
                style.dim = true;
                style.italic = true;
                style.fg = Some(self.palette.code_comment);
            }
            if text.chars().nth(2).is_some_and(|ch| ch == ' ') {
                if let Some(style) = styles.get_mut(2) {
                    style.dim = true;
                    style.italic = true;
                    style.fg = Some(self.palette.code_comment);
                }
            }
        }

        if self.render_as_plain_code {
            // Skip markdown semantic styling when a file has a fixed syntax mode.
        } else if info.as_ref().is_some_and(|line| line.is_code_fence) {
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
            let code_tokens =
                markdown_tokens::tokenize_code_line(text, self.code_fence_lang.as_deref());
            apply_code_token_styles(&code_tokens, &mut styles, self.palette);
        } else {
            let info = info.as_ref().expect("markdown info present");
            apply_line_styles_from_info(info, &mut styles);
            hidden_ranges.extend(hidden_line_prefix_marker_ranges(
                info,
                len,
                active_cursor_col.is_some(),
            ));
            let inline_tokens = cached_inline_tokens(text);
            // Inside table rows, single-asterisk Emphasis (`*x*`) is ambiguous
            // with formula markers (`value*`, `value***`) and can leak across
            // cell boundaries — always filter it.
            //
            // Strong (bold, `**x**`) uses double asterisks that wrap content;
            // this is visually unambiguous when the token stays within one cell.
            // Allow it unless the token spans a `|` character (cross-cell).
            let filtered_tokens: Vec<markdown_tokens::InlineToken>;
            let inline_tokens_to_apply: &[markdown_tokens::InlineToken] = if is_table_row {
                filtered_tokens = inline_tokens
                    .iter()
                    .filter(|t| {
                        if matches!(t.kind, markdown_tokens::InlineTokenType::Emphasis) {
                            return false;
                        }
                        if matches!(t.kind, markdown_tokens::InlineTokenType::Strong) {
                            // Drop bold that spans a pipe (cross-cell ambiguity).
                            // Scan the chars slice directly — no Vec allocation.
                            let from = t.from.min(len);
                            let to = t.to.min(len);
                            return !chars[from..to].iter().any(|&c| c == '|');
                        }
                        true
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
                s.fg = Some(self.palette.code_comment);
            }
        }
        if !self.render_as_plain_code && is_table_row && text.contains('*') {
            apply_table_formula_marker_styles(&chars, &mut styles, self.palette.code_comment);
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

        for &(start, end) in red_ranges {
            for s in styles.iter_mut().take(end.min(len)).skip(start.min(len)) {
                s.fg = Some(self.palette.primary);
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
        let selection_bg = selection_bg_for_surface(self.palette.surface_bg());
        let selection_fg = contrast_fg_for_bg(selection_bg);
        let selection_style = (!reverse_ranges.is_empty()).then_some(SelectionStyle {
            bg: selection_bg,
            fg: selection_fg,
            plain_fg: self.palette.text_fg(),
        });

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

        let mut painter = RowPainter {
            buf,
            x,
            y,
            emitted: 0,
            stream_col: 0,
            width,
            window_col,
            window_end: window_col.saturating_add(width),
            selection_ranges: reverse_ranges,
            selection_style,
            style_cache: None,
        };
        painter.paint_line(
            &chars,
            &styles,
            &hidden_ranges,
            Ghosts {
                calc: calc_ghost,
                calc_prefix,
                reminder: reminder_ghost,
                reminder_strikethrough,
                fg: self.palette.code_comment,
            },
            base_style,
        );
    }
}

/// Per-line overlays and highlights passed to `RenderContext::render_line`.
/// Ranges are char indices into the line text.
#[derive(Clone, Copy, Default)]
pub struct LineDecorations<'a> {
    pub calc_ghost: Option<&'a str>,
    pub reminder_ghost: Option<&'a str>,
    pub reminder_strikethrough: bool,
    pub search_ranges: &'a [(usize, usize)],
    pub current_search_ranges: &'a [(usize, usize)],
    pub variable_names: &'a [String],
    pub dim_ranges: &'a [(usize, usize)],
    pub selection_ranges: &'a [(usize, usize)],
    pub accent_ranges: &'a [(usize, usize)],
    pub underline_ranges: &'a [(usize, usize)],
    /// Cursor column when this is the cursor line; reveals markers near it.
    pub active_cursor_col: Option<usize>,
}

fn selection_bg_for_surface(surface_bg: u8) -> u8 {
    if contrast_fg_for_bg(surface_bg) == 16 {
        236
    } else {
        252
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

fn apply_table_formula_marker_styles(chars: &[char], styles: &mut [CharStyle], marker_color: u8) {
    let len = chars.len();
    let mut i = 0usize;
    while i < len {
        if chars[i] != '*' {
            i += 1;
            continue;
        }
        let start = i;
        while i < len && chars[i] == '*' {
            i += 1;
        }
        let end = i;
        let prev = if start > 0 {
            Some(chars[start - 1])
        } else {
            None
        };
        let next = if end < len { Some(chars[end]) } else { None };
        let prev_ok = prev
            .map(|ch| ch.is_ascii_digit() || ch == '.' || ch == ')')
            .unwrap_or(false);
        let next_ok = next
            .map(|ch| ch == '|' || ch.is_whitespace())
            .unwrap_or(true);
        if prev_ok && next_ok {
            for s in styles.iter_mut().take(end).skip(start) {
                s.dim = true;
                s.fg = Some(marker_color);
            }
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

// --- Output ---

#[derive(Clone, Copy)]
struct SelectionStyle {
    bg: u8,
    fg: u8,
    plain_fg: u8,
}

#[derive(Clone, Copy)]
struct Ghosts<'a> {
    calc: Option<&'a str>,
    calc_prefix: &'a str,
    reminder: Option<&'a str>,
    reminder_strikethrough: bool,
    fg: u8,
}

fn selection_intersects_cell(ranges: &[(usize, usize)], start: usize, end: usize) -> bool {
    ranges
        .iter()
        .any(|&(range_start, range_end)| range_start < end && start < range_end)
}

fn apply_selection_overlay(style: &mut CharStyle, selection: SelectionStyle) {
    style.bg = Some(selection.bg);
    style.reverse = false;
    if style.fg.is_none() || style.fg == Some(selection.plain_fg) {
        style.fg = Some(selection.fg);
    }
}

/// Writes a horizontally scrolled window of styled chars into one buffer row.
/// `stream_col` tracks display columns of the full line; only columns inside
/// `[window_col, window_end)` produce cells, and exactly `width` cells are
/// written in total (the remainder is padded with the base style).
struct RowPainter<'a, 'b> {
    buf: &'a mut Buffer,
    x: u16,
    y: u16,
    emitted: usize,
    stream_col: usize,
    width: usize,
    window_col: usize,
    window_end: usize,
    selection_ranges: &'b [(usize, usize)],
    selection_style: Option<SelectionStyle>,
    /// Last converted style; consecutive cells usually share one.
    style_cache: Option<(CharStyle, Style)>,
}

impl RowPainter<'_, '_> {
    fn paint_line(
        &mut self,
        chars: &[char],
        styles: &[CharStyle],
        hidden_ranges: &[(usize, usize)],
        ghosts: Ghosts<'_>,
        base_style: CharStyle,
    ) {
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
            if ch == '\t' {
                let tab_spaces = TAB_WIDTH - (self.stream_col % TAB_WIDTH);
                for _ in 0..tab_spaces {
                    self.emit(' ', styles[i]);
                }
            } else {
                self.emit(ch, styles[i]);
            }
        }

        if let Some(ghost) = ghosts.calc {
            let ghost_style = CharStyle {
                dim: true,
                italic: true,
                fg: Some(ghosts.fg),
                bg: base_style.bg,
                ..Default::default()
            };
            for ch in ghosts.calc_prefix.chars().chain(ghost.chars()) {
                self.emit(ch, ghost_style);
            }
        }

        if let Some(ghost) = ghosts.reminder {
            let reminder_style = CharStyle {
                dim: true,
                italic: true,
                strikethrough: ghosts.reminder_strikethrough,
                fg: Some(ghosts.fg),
                bg: base_style.bg,
                ..Default::default()
            };
            let reminder_prefix = if ghosts.calc.is_some() { "  " } else { " " };
            for ch in reminder_prefix.chars().chain(ghost.chars()) {
                self.emit(ch, reminder_style);
            }
        }

        while self.emitted < self.width {
            let mut filler_style = base_style;
            if let Some(selection) = self.selection_style {
                if selection_intersects_cell(
                    self.selection_ranges,
                    self.stream_col,
                    self.stream_col + 1,
                ) {
                    apply_selection_overlay(&mut filler_style, selection);
                }
            }
            self.put(' ', 1, filler_style);
            self.stream_col += 1;
        }
    }

    fn emit(&mut self, ch: char, style: CharStyle) {
        use unicode_width::UnicodeWidthChar;
        let ch_width = ch.width().unwrap_or(0).max(1);
        let ch_end = self.stream_col + ch_width;
        let mut style = style;
        if let Some(selection) = self.selection_style {
            if selection_intersects_cell(self.selection_ranges, self.stream_col, ch_end) {
                apply_selection_overlay(&mut style, selection);
            }
        }

        // Portion of this char's columns inside [window_col, window_end).
        let visible_start = self.stream_col.max(self.window_col);
        let visible_end = ch_end.min(self.window_end);
        if visible_start < visible_end && self.emitted < self.width {
            if self.stream_col >= self.window_col
                && ch_end <= self.window_end
                && self.emitted + ch_width <= self.width
            {
                self.put(ch, ch_width, style);
            } else {
                // Wide char clips a window boundary: fill the visible slots.
                let slots = (visible_end - visible_start).min(self.width - self.emitted);
                for _ in 0..slots {
                    self.put(' ', 1, style);
                }
            }
        }
        self.stream_col = ch_end;
    }

    /// Writes `ch` spanning `cells` columns at the current position.
    fn put(&mut self, ch: char, cells: usize, style: CharStyle) {
        let area = self.buf.area;
        if self.y < area.bottom() && self.x < area.right() {
            let style = match self.style_cache {
                Some((cached, converted)) if cached == style => converted,
                _ => {
                    let converted = style.to_style();
                    self.style_cache = Some((style, converted));
                    converted
                }
            };
            self.buf[(self.x, self.y)].set_char(ch).set_style(style);
            // Cells covered by a wide char are reset so the diff skips them.
            for offset in 1..cells {
                let cx = self.x.saturating_add(offset as u16);
                if cx < area.right() {
                    self.buf[(cx, self.y)].reset();
                    self.buf[(cx, self.y)].set_style(style);
                }
            }
        }
        self.x = self.x.saturating_add(cells as u16);
        self.emitted += cells;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::canvas::test_support::row_text;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier};

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

    struct Rendered {
        text: String,
        buf: Buffer,
    }

    impl Rendered {
        fn any_cell(&self, pred: impl Fn(&ratatui::buffer::Cell) -> bool) -> bool {
            self.buf.content().iter().any(pred)
        }
    }

    fn render(
        ctx: &mut RenderContext,
        text: &str,
        width: usize,
        window_col: usize,
        deco: LineDecorations<'_>,
    ) -> Rendered {
        let mut buf = Buffer::empty(Rect::new(0, 0, width as u16, 1));
        ctx.render_line(text, width, window_col, &deco, &mut buf, 0, 0);
        Rendered {
            text: row_text(&buf, 0),
            buf,
        }
    }

    fn render_plain(ctx: &mut RenderContext, text: &str, width: usize) -> Rendered {
        render(ctx, text, width, 0, LineDecorations::default())
    }

    fn with_cursor(cursor: Option<usize>) -> LineDecorations<'static> {
        LineDecorations {
            active_cursor_col: cursor,
            ..LineDecorations::default()
        }
    }

    fn fg(index: u8) -> Color {
        Color::Indexed(index)
    }

    #[test]
    fn render_plain_pads_to_width() {
        let mut ctx = RenderContext::new();
        let out = render_plain(&mut ctx, "hi", 10);
        assert_eq!(out.text, "hi        ");
    }

    #[test]
    fn render_window_skips_prefix_columns() {
        let mut ctx = RenderContext::new();
        let out = render(&mut ctx, "abcdef", 4, 2, LineDecorations::default());
        assert_eq!(out.text, "cdef");
    }

    #[test]
    fn render_window_respects_tab_expansion() {
        let mut ctx = RenderContext::new();
        let out = render(&mut ctx, "\tabcd", 4, 2, LineDecorations::default());
        assert_eq!(out.text, "  ab");
    }

    #[test]
    fn render_calc_ghost_appended() {
        let mut ctx = RenderContext::new();
        let deco = LineDecorations {
            calc_ghost: Some("4"),
            ..LineDecorations::default()
        };
        assert!(render(&mut ctx, "2+2", 30, 0, deco).text.contains("→ 4"));
    }

    #[test]
    fn render_assignment_calc_ghost_uses_equals_prefix() {
        let mut ctx = RenderContext::new();
        let deco = LineDecorations {
            calc_ghost: Some("4"),
            ..LineDecorations::default()
        };
        assert!(render(&mut ctx, "value := 2 + 2", 30, 0, deco).text.contains("= 4"));
    }

    #[test]
    fn render_formula_explanation_ghost_does_not_add_default_arrow_prefix() {
        let mut ctx = RenderContext::new();
        let deco = LineDecorations {
            calc_ghost: Some("* ➜ avg_col()"),
            ..LineDecorations::default()
        };
        let visible = render(&mut ctx, "| a | 4* |", 40, 0, deco).text;
        assert!(visible.contains(" * ➜ avg_col()"));
        assert!(!visible.contains("→ * ➜ avg_col()"));
    }

    #[test]
    fn render_reminder_ghost_is_appended_without_calc_prefix() {
        let mut ctx = RenderContext::new();
        let deco = LineDecorations {
            reminder_ghost: Some("⏰ 23.04.2026. 14:34"),
            ..LineDecorations::default()
        };
        let visible = render(&mut ctx, "- [ ] task", 40, 0, deco).text;
        assert!(visible.contains(" ⏰ 23.04.2026. 14:34"));
        assert!(!visible.contains("→ ⏰"));
    }

    #[test]
    fn render_expired_reminder_ghost_uses_strikethrough_style() {
        let mut ctx = RenderContext::new();
        let deco = LineDecorations {
            reminder_ghost: Some("⏰ 23.04.2026. 14:34"),
            reminder_strikethrough: true,
            ..LineDecorations::default()
        };
        let out = render(&mut ctx, "task", 40, 0, deco);
        assert!(out.any_cell(|cell| cell
            .modifier
            .contains(Modifier::DIM | Modifier::ITALIC | Modifier::CROSSED_OUT)));
    }

    #[test]
    fn render_line_with_dim_ranges_dims_marker_character() {
        let mut ctx = RenderContext::new();
        let deco = LineDecorations {
            dim_ranges: &[(3, 4)],
            ..LineDecorations::default()
        };
        let out = render(&mut ctx, "abc*", 12, 0, deco);
        assert!(out.text.starts_with("abc*"));
        assert!(out.buf[(3, 0)].modifier.contains(Modifier::DIM));
        assert!(!out.buf[(0, 0)].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn render_visual_selection_uses_background_without_reverse_video() {
        let palette = RenderPalette {
            surface_bg: 252,
            text_fg: 16,
            ..RenderPalette::default()
        };
        let mut ctx = RenderContext::new_with_palette(palette);
        let deco = LineDecorations {
            selection_ranges: &[(0, 5)],
            ..LineDecorations::default()
        };
        let out = render(&mut ctx, "alpha beta", 16, 0, deco);

        assert!(out.text.starts_with("alpha beta"));
        assert_eq!(out.buf[(0, 0)].bg, fg(236));
        assert_eq!(out.buf[(0, 0)].fg, fg(231));
        assert!(!out.any_cell(|cell| cell.modifier.contains(Modifier::REVERSED)));
    }

    #[test]
    fn render_visual_selection_paints_selected_filler_cells() {
        let mut ctx = RenderContext::new();
        let deco = LineDecorations {
            selection_ranges: &[(0, usize::MAX)],
            ..LineDecorations::default()
        };
        let out = render(&mut ctx, "", 4, 0, deco);

        assert_eq!(out.text, "    ");
        assert!(out.buf.content().iter().all(|cell| cell.bg == fg(252)));
    }

    #[test]
    fn render_full_row_visual_selection_keeps_calc_ghost_visible() {
        let mut ctx = RenderContext::new();
        let deco = LineDecorations {
            calc_ghost: Some("155"),
            selection_ranges: &[(0, usize::MAX)],
            ..LineDecorations::default()
        };
        let out = render(&mut ctx, "subtotal := 155", 40, 0, deco);

        assert!(out.text.contains("subtotal := 155 = 155"));
        assert!(out.any_cell(|cell| cell.bg == fg(252)));
    }

    #[test]
    fn render_table_formula_markers_use_ghost_style() {
        let mut ctx = RenderContext::new();
        let palette = RenderPalette::default();
        let out = render_plain(&mut ctx, "| a | 88* | 1455.86** |", 80);
        assert!(out.any_cell(|cell| cell.modifier.contains(Modifier::DIM)
            && cell.fg == fg(palette.code_comment)));
        assert!(out.text.contains("88*"));
        assert!(out.text.contains("1455.86**"));
    }

    #[test]
    fn render_code_block_uses_gray_background_with_syntax_colors() {
        let mut ctx = RenderContext::new();
        let palette = RenderPalette::default();
        let _ = render_plain(&mut ctx, "```rust", 60);
        let out = render_plain(&mut ctx, "let total = 42 // note", 60);
        assert!(out.any_cell(|cell| cell.bg == fg(palette.code_block_bg)));
        assert!(out.any_cell(|cell| cell.fg == fg(palette.code_keyword)));
        assert!(out.any_cell(|cell| cell.fg == fg(palette.code_number)));
    }

    #[test]
    fn render_expands_tabs_into_spaces() {
        let mut ctx = RenderContext::new();
        let out = render_plain(&mut ctx, "a\tb", 12);
        assert!(out.text.starts_with("a   b"));
        assert_eq!(out.text.len(), 12);
    }

    fn is_bold_with_fg(cell: &ratatui::buffer::Cell, color: u8) -> bool {
        cell.modifier.contains(Modifier::BOLD) && cell.fg == fg(color)
    }

    #[test]
    fn render_highlights_variables_in_bold_with_distinct_color() {
        let mut ctx = RenderContext::new();
        let palette = RenderPalette::default();
        let vars = vec!["subtotal".to_string(), "tax rate".to_string()];
        let deco = LineDecorations {
            variable_names: &vars,
            ..LineDecorations::default()
        };
        let out = render(&mut ctx, "total = subtotal + tax rate", 80, 0, deco);
        // "subtotal" starts at column 8.
        assert!(is_bold_with_fg(&out.buf[(8, 0)], palette.variable));
        assert!(!is_bold_with_fg(&out.buf[(0, 0)], palette.variable));
    }

    #[test]
    fn render_variable_highlighting_is_case_insensitive() {
        let mut ctx = RenderContext::new();
        let palette = RenderPalette::default();
        let vars = vec!["daily".to_string()];
        let deco = LineDecorations {
            variable_names: &vars,
            ..LineDecorations::default()
        };

        let exact = render(&mut ctx, "daily = 1", 40, 0, deco);
        assert!(is_bold_with_fg(&exact.buf[(0, 0)], palette.variable));

        let different_case = render(&mut ctx, "Daily note", 40, 0, deco);
        assert!(is_bold_with_fg(&different_case.buf[(0, 0)], palette.variable));
    }

    fn assert_search_colors(palette: RenderPalette) {
        let mut ctx = RenderContext::new_with_palette(palette);
        let deco = LineDecorations {
            search_ranges: &[(0, 5)],
            current_search_ranges: &[(11, 16)],
            ..LineDecorations::default()
        };
        let out = render(&mut ctx, "alpha beta alpha", 40, 0, deco);
        let other = &out.buf[(0, 0)];
        assert_eq!(other.fg, fg(palette.search_match));
        assert!(!other.modifier.contains(Modifier::BOLD));
        assert!(is_bold_with_fg(&out.buf[(11, 0)], palette.search_current));
    }

    #[test]
    fn render_search_uses_distinct_colors_for_current_and_other_matches() {
        assert_search_colors(RenderPalette::default());
    }

    #[test]
    fn render_supports_custom_palette_for_search_highlights() {
        assert_search_colors(RenderPalette {
            search_match: 135,
            search_current: 196,
            ..RenderPalette::default()
        });
    }

    #[test]
    fn render_hides_inline_markers_off_cursor_and_reveals_on_marker_boundaries() {
        let mut ctx = RenderContext::new();
        let hidden = render(&mut ctx, "**bold**", 8, 0, with_cursor(None)).text;
        assert!(hidden.starts_with("bold"));
        assert!(!hidden.contains('*'));

        let reveal_open = render(&mut ctx, "**bold**", 8, 0, with_cursor(Some(1))).text;
        assert_eq!(reveal_open, "**bold**");

        let reveal_close = render(&mut ctx, "**bold**", 8, 0, with_cursor(Some(6))).text;
        assert_eq!(reveal_close, "**bold**");
    }

    #[test]
    fn render_hides_heading_marker_off_cursor_line_and_reveals_on_active_line() {
        let mut ctx = RenderContext::new();
        let hidden = render(&mut ctx, "# Heading", 9, 0, with_cursor(None)).text;
        assert!(hidden.starts_with("Heading"));
        assert!(!hidden.contains('#'));

        let revealed = render(&mut ctx, "# Heading", 9, 0, with_cursor(Some(0))).text;
        assert_eq!(revealed, "# Heading");
    }

    #[test]
    fn render_reveals_only_active_inline_component_on_cursor_line() {
        let mut ctx = RenderContext::new();

        let strong = render(&mut ctx, "**bold** and `code`", 32, 0, with_cursor(Some(3))).text;
        assert!(strong.starts_with("**bold** and code"));
        assert!(!strong.contains("`code`"));

        let code = render(&mut ctx, "**bold** and `code`", 32, 0, with_cursor(Some(14))).text;
        assert!(code.starts_with("bold and `code`"));
        assert!(!code.contains("**bold**"));
    }
}
