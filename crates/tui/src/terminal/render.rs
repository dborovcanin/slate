use crate::editor_core::markdown_tokens;
use crate::terminal::canvas::contrast_fg_for_bg;
pub use crate::terminal::markdown_view::collapse_markdown_line_for_cursor_with_formatting_boundary_exit;
use crate::terminal::markdown_view::{hidden_line_prefix_marker_ranges, normalize_hidden_ranges};
pub use crate::terminal::render_styles::VariableNames;
use crate::terminal::render_styles::{
    apply_code_token_styles, apply_inline_token_styles, apply_line_styles_from_info,
    apply_variable_styles, CharStyle,
};
pub use crate::terminal::theme::RenderPalette;
use ratatui::buffer::Buffer;
use ratatui::style::Style;
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

#[derive(Clone)]
pub struct RenderContext {
    fence: markdown_tokens::FenceState,
    render_as_plain_code: bool,
    forced_code_lang: Option<String>,
    palette: RenderPalette,
}

impl RenderContext {
    #[cfg(test)]
    pub fn new() -> Self {
        Self {
            fence: markdown_tokens::FenceState::default(),
            render_as_plain_code: false,
            forced_code_lang: None,
            palette: RenderPalette::default(),
        }
    }

    #[cfg(test)]
    pub fn new_with_palette(palette: RenderPalette) -> Self {
        Self {
            fence: markdown_tokens::FenceState::default(),
            render_as_plain_code: false,
            forced_code_lang: None,
            palette,
        }
    }

    pub fn with_syntax_mode(
        fence: markdown_tokens::FenceState,
        render_as_plain_code: bool,
        forced_code_lang: Option<String>,
        palette: RenderPalette,
    ) -> Self {
        Self {
            fence,
            render_as_plain_code,
            forced_code_lang,
            palette,
        }
    }

    /// Skip ahead through `lines` without rendering — just track code fence state.
    /// Whether the next rendered line sits inside a fenced code block.
    pub fn in_code_block(&self) -> bool {
        self.fence.in_code_block
    }

    /// Whether every line renders as plain code (code files, large notes).
    pub fn renders_plain_code(&self) -> bool {
        self.render_as_plain_code
    }

    pub fn advance_lines(&mut self, lines: &[String]) {
        if self.render_as_plain_code {
            return;
        }
        for line in lines {
            markdown_tokens::advance_fence_state(&mut self.fence, line);
        }
    }

    #[cfg(test)]
    pub fn advance_line(&mut self, text: &str) {
        if self.render_as_plain_code {
            return;
        }
        markdown_tokens::advance_fence_state(&mut self.fence, text);
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
        let line = self.style_line(text, deco);
        let mut painter = RowPainter {
            buf,
            x,
            y,
            emitted: 0,
            stream_col: 0,
            width,
            window_col,
            window_end: window_col.saturating_add(width),
            selection_ranges: deco.selection_ranges,
            selection_style: line.selection_style,
            style_cache: None,
        };
        painter.paint_line(
            &line.chars,
            &line.styles,
            &line.hidden_ranges,
            line.ghosts(deco, self.palette.code_comment),
            line.base_style,
        );
    }

    /// Soft-wraps one line into rows of `width` cells. Skips the first
    /// `skip_rows` rows, paints at most `max_rows` rows into `target` (when
    /// given) and reports the total row
    /// count plus the cell of the `track_char`-th visible character, which
    /// callers use to place the cursor. With `target` = `None` it only measures.
    #[allow(clippy::too_many_arguments)]
    pub fn render_line_wrapped(
        &mut self,
        text: &str,
        width: usize,
        skip_rows: usize,
        max_rows: usize,
        track_char: Option<usize>,
        deco: &LineDecorations<'_>,
        target: Option<(&mut Buffer, u16, u16)>,
    ) -> WrapOutcome {
        let line = self.style_line(text, deco);
        let (cells, visible_count) = build_wrap_cells(
            &line,
            line.ghosts(deco, self.palette.code_comment),
            deco.selection_ranges,
        );
        let width = width.max(1);
        let layout = layout_wrap_rows(&cells, width);
        let tracked = track_char
            .map(|ordinal| locate_ordinal(&cells, &layout, ordinal.min(visible_count), width));
        let mut rows = layout.rows.len();
        if let Some((row, _)) = tracked {
            rows = rows.max(row + 1);
        }
        if let Some((buf, x, y)) = target {
            paint_wrap_rows(
                buf,
                x,
                y,
                width,
                skip_rows,
                max_rows,
                &cells,
                &layout,
                &line,
                deco.selection_ranges,
            );
        }
        WrapOutcome { rows, tracked }
    }

    /// Cell position (row, col) of every char of a soft-wrapped line, plus
    /// the end-of-line slot at index `chars.len()`. Hidden chars map to `None`.
    /// Returns the positions and the number of rows.
    pub fn wrap_char_positions(
        &mut self,
        text: &str,
        width: usize,
        deco: &LineDecorations<'_>,
    ) -> (Vec<Option<(usize, usize)>>, usize) {
        let line = self.style_line(text, deco);
        let (cells, visible_count) = build_wrap_cells(
            &line,
            line.ghosts(deco, self.palette.code_comment),
            deco.selection_ranges,
        );
        let width = width.max(1);
        let layout = layout_wrap_rows(&cells, width);
        let mut positions = vec![None; line.chars.len() + 1];
        let mut hidden = vec![false; line.chars.len()];
        for &(start, end) in &line.hidden_ranges {
            for flag in hidden.iter_mut().take(end).skip(start) {
                *flag = true;
            }
        }
        let mut ordinal = 0usize;
        for (idx, is_hidden) in hidden.iter().enumerate() {
            if !is_hidden {
                positions[idx] = Some(locate_ordinal(&cells, &layout, ordinal, width));
                ordinal += 1;
            }
        }
        let end = locate_ordinal(&cells, &layout, visible_count, width);
        positions[line.chars.len()] = Some(end);
        let rows = layout.rows.len().max(end.0 + 1);
        (positions, rows)
    }

    /// Computes per-char styles, hidden marker ranges and ghost prefix for a
    /// line, advancing fenced-code state.
    fn style_line(&mut self, text: &str, deco: &LineDecorations<'_>) -> StyledLine {
        let LineDecorations {
            calc_ghost,
            reminder_ghost: _,
            reminder_strikethrough: _,
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
        // A fence line here is one that opens a block or closes the open one.
        let is_fence_line = info.is_some() && markdown_tokens::is_fence_line(&self.fence, text);
        let is_code_block_line =
            !self.render_as_plain_code && (self.fence.in_code_block || is_fence_line);
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
        } else if is_fence_line {
            for s in &mut styles {
                s.dim = true;
            }
            markdown_tokens::advance_fence_state(&mut self.fence, text);
        } else if self.fence.in_code_block {
            let code_tokens =
                markdown_tokens::tokenize_code_line(text, self.fence.code_fence_lang.as_deref());
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

        StyledLine {
            chars,
            styles,
            hidden_ranges,
            base_style,
            selection_style,
            calc_prefix,
        }
    }
}

/// A line after styling, before it is laid out into cells.
struct StyledLine {
    chars: Vec<char>,
    styles: Vec<CharStyle>,
    hidden_ranges: Vec<(usize, usize)>,
    base_style: CharStyle,
    selection_style: Option<SelectionStyle>,
    calc_prefix: &'static str,
}

impl StyledLine {
    fn ghosts<'a>(&self, deco: &LineDecorations<'a>, fg: u8) -> Ghosts<'a> {
        Ghosts {
            calc: deco.calc_ghost,
            calc_prefix: self.calc_prefix,
            reminder: deco.reminder_ghost,
            reminder_strikethrough: deco.reminder_strikethrough,
            fg,
        }
    }
}

/// Result of `RenderContext::render_line_wrapped`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WrapOutcome {
    /// Rows the whole line needs (may exceed the painted `max_rows`).
    pub rows: usize,
    /// (row, col) cell offset of the tracked character, relative to the line.
    pub tracked: Option<(usize, usize)>,
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
    pub variable_names: Option<&'a VariableNames>,
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

/// One display cell of a soft-wrapped line.
struct WrapCell {
    ch: char,
    width: usize,
    style: CharStyle,
    /// Index among visible source chars; `None` for tab padding and ghosts.
    ordinal: Option<usize>,
}

/// Row boundaries of a wrapped line as half-open cell index ranges. Cells
/// between rows (a space the break consumed) belong to no row.
struct WrapLayout {
    rows: Vec<(usize, usize)>,
}

/// Expands a styled line into display cells: hidden markers dropped, tabs
/// expanded, ghosts appended, selection applied. Returns the cells and the
/// number of visible source chars.
fn build_wrap_cells(
    line: &StyledLine,
    ghosts: Ghosts<'_>,
    selection_ranges: &[(usize, usize)],
) -> (Vec<WrapCell>, usize) {
    use unicode_width::UnicodeWidthChar;
    let mut cells = Vec::with_capacity(line.chars.len() + 16);
    let mut stream_col = 0usize;
    let mut ordinal = 0usize;
    let push = |cells: &mut Vec<WrapCell>,
                stream_col: &mut usize,
                ch: char,
                style: CharStyle,
                ord: Option<usize>| {
        let width = ch.width().unwrap_or(0).max(1);
        let mut style = style;
        if let Some(selection) = line.selection_style {
            if selection_intersects_cell(selection_ranges, *stream_col, *stream_col + width) {
                apply_selection_overlay(&mut style, selection);
            }
        }
        cells.push(WrapCell {
            ch,
            width,
            style,
            ordinal: ord,
        });
        *stream_col += width;
    };

    let mut hidden_iter = line.hidden_ranges.iter().peekable();
    for (i, &ch) in line.chars.iter().enumerate() {
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
        let style = line.styles[i];
        if ch == '\t' {
            let tab_spaces = TAB_WIDTH - (stream_col % TAB_WIDTH);
            for n in 0..tab_spaces {
                push(
                    &mut cells,
                    &mut stream_col,
                    ' ',
                    style,
                    (n == 0).then_some(ordinal),
                );
            }
        } else {
            push(&mut cells, &mut stream_col, ch, style, Some(ordinal));
        }
        ordinal += 1;
    }

    let base_bg = line.base_style.bg;
    if let Some(ghost) = ghosts.calc {
        let style = CharStyle {
            dim: true,
            italic: true,
            fg: Some(ghosts.fg),
            bg: base_bg,
            ..Default::default()
        };
        for ch in ghosts.calc_prefix.chars().chain(ghost.chars()) {
            push(&mut cells, &mut stream_col, ch, style, None);
        }
    }
    if let Some(ghost) = ghosts.reminder {
        let style = CharStyle {
            dim: true,
            italic: true,
            strikethrough: ghosts.reminder_strikethrough,
            fg: Some(ghosts.fg),
            bg: base_bg,
            ..Default::default()
        };
        let prefix = if ghosts.calc.is_some() { "  " } else { " " };
        for ch in prefix.chars().chain(ghost.chars()) {
            push(&mut cells, &mut stream_col, ch, style, None);
        }
    }
    (cells, ordinal)
}

/// Breaks cells into rows of at most `width` columns, preferring to break
/// after a space. A space that would overflow a row is consumed by the
/// break; words longer than a row are split; a wide char never straddles.
fn layout_wrap_rows(cells: &[WrapCell], width: usize) -> WrapLayout {
    let mut rows = Vec::new();
    let mut row_start = 0usize;
    let mut row_width = 0usize;
    let mut last_break: Option<usize> = None;
    let mut i = 0usize;
    while i < cells.len() {
        let cell_width = cells[i].width;
        if row_width + cell_width > width && row_width > 0 {
            if cells[i].ch == ' ' {
                rows.push((row_start, i));
                i += 1;
                row_start = i;
                row_width = 0;
                last_break = None;
                continue;
            }
            match last_break.filter(|&at| at > row_start) {
                Some(at) => {
                    rows.push((row_start, at));
                    row_start = at;
                    row_width = cells[at..i].iter().map(|cell| cell.width).sum();
                    last_break = cells[at..i]
                        .iter()
                        .rposition(|cell| cell.ch == ' ')
                        .map(|offset| at + offset + 1);
                }
                None => {
                    rows.push((row_start, i));
                    row_start = i;
                    row_width = 0;
                    last_break = None;
                }
            }
            continue;
        }
        row_width += cell_width;
        if cells[i].ch == ' ' {
            last_break = Some(i + 1);
        }
        i += 1;
    }
    rows.push((row_start, cells.len()));
    WrapLayout { rows }
}

/// Cell position of the `ordinal`-th visible char; `ordinal` equal to the
/// visible count means the slot just after the last visible char.
fn locate_ordinal(
    cells: &[WrapCell],
    layout: &WrapLayout,
    ordinal: usize,
    width: usize,
) -> (usize, usize) {
    // Slot after the char preceding `ordinal`, used when `ordinal` itself has
    // no cell (end of line) or was consumed by a break.
    let mut after_prev = (0usize, 0usize);
    for (row, &(start, end)) in layout.rows.iter().enumerate() {
        let mut col = 0usize;
        for cell in &cells[start..end] {
            match cell.ordinal {
                Some(ord) if ord == ordinal => return (row, col),
                Some(ord) if ord > ordinal => return after_prev,
                Some(_) => after_prev = (row, col + cell.width),
                None => {}
            }
            col += cell.width;
        }
        // A consumed break space right after this row.
        if let Some(cell) = cells
            .get(end)
            .filter(|_| layout.rows.get(row + 1).is_some_and(|next| next.0 > end))
        {
            match cell.ordinal {
                Some(ord) if ord == ordinal => return (row, col.min(width - 1)),
                Some(_) => after_prev = (row, col.min(width - 1)),
                None => {}
            }
        }
    }
    if after_prev.1 >= width {
        (after_prev.0 + 1, 0)
    } else {
        after_prev
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_wrap_rows(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: usize,
    skip_rows: usize,
    max_rows: usize,
    cells: &[WrapCell],
    layout: &WrapLayout,
    line: &StyledLine,
    selection_ranges: &[(usize, usize)],
) {
    let area = buf.area;
    let last_row = layout.rows.len().saturating_sub(1);
    for (row, &(start, end)) in layout
        .rows
        .iter()
        .enumerate()
        .skip(skip_rows)
        .take(max_rows)
    {
        let row_y = y.saturating_add((row - skip_rows) as u16);
        if row_y >= area.bottom() {
            break;
        }
        let mut col = 0usize;
        for cell in &cells[start..end] {
            let cell_x = x.saturating_add(col as u16);
            if cell_x < area.right() && col + cell.width <= width {
                let style = cell.style.to_style();
                buf[(cell_x, row_y)].set_char(cell.ch).set_style(style);
                for offset in 1..cell.width {
                    let cx = cell_x.saturating_add(offset as u16);
                    if cx < area.right() {
                        buf[(cx, row_y)].reset();
                        buf[(cx, row_y)].set_style(style);
                    }
                }
            }
            col += cell.width;
        }
        // Pad the row. Past the end of a fully selected line the padding
        // keeps the selection background (linewise selection).
        let mut filler = line.base_style;
        if row == last_row {
            if let Some(selection) = line.selection_style {
                let line_cols: usize = cells.iter().map(|cell| cell.width).sum();
                if selection_intersects_cell(selection_ranges, line_cols, line_cols + 1) {
                    apply_selection_overlay(&mut filler, selection);
                }
            }
        }
        let filler = filler.to_style();
        while col < width {
            let cell_x = x.saturating_add(col as u16);
            if cell_x >= area.right() {
                break;
            }
            buf[(cell_x, row_y)].set_char(' ').set_style(filler);
            col += 1;
        }
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
        assert!(!ctx.in_code_block());
        ctx.advance_line("```rust");
        assert!(ctx.in_code_block());
        assert_eq!(ctx.fence.code_fence_lang.as_deref(), Some("rust"));
        ctx.advance_line("code line");
        assert!(ctx.in_code_block());
        ctx.advance_line("```");
        assert!(!ctx.in_code_block());
        assert_eq!(ctx.fence.code_fence_lang, None);

        // A shorter fence inside a longer one is code.
        ctx.advance_line("````md");
        ctx.advance_line("```");
        assert!(ctx.in_code_block());
        ctx.advance_line("````");
        assert!(!ctx.in_code_block());
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
        assert!(render(&mut ctx, "value := 2 + 2", 30, 0, deco)
            .text
            .contains("= 4"));
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
        assert!(out.any_cell(
            |cell| cell.modifier.contains(Modifier::DIM) && cell.fg == fg(palette.code_comment)
        ));
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
        let vars = VariableNames::new(vec!["subtotal".to_string(), "tax rate".to_string()]);
        let deco = LineDecorations {
            variable_names: Some(&vars),
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
        let vars = VariableNames::new(vec!["daily".to_string()]);
        let deco = LineDecorations {
            variable_names: Some(&vars),
            ..LineDecorations::default()
        };

        let exact = render(&mut ctx, "daily = 1", 40, 0, deco);
        assert!(is_bold_with_fg(&exact.buf[(0, 0)], palette.variable));

        let different_case = render(&mut ctx, "Daily note", 40, 0, deco);
        assert!(is_bold_with_fg(
            &different_case.buf[(0, 0)],
            palette.variable
        ));
    }

    #[test]
    fn render_variable_highlighting_aligns_after_non_ascii_text() {
        let mut ctx = RenderContext::new();
        let palette = RenderPalette::default();
        let vars = VariableNames::new(vec!["total".to_string()]);
        let deco = LineDecorations {
            variable_names: Some(&vars),
            ..LineDecorations::default()
        };
        // "é" is two bytes but one cell; "total" occupies columns 4..9.
        let out = render(&mut ctx, "é = total + 1", 40, 0, deco);
        assert!(!is_bold_with_fg(&out.buf[(3, 0)], palette.variable));
        assert!(is_bold_with_fg(&out.buf[(4, 0)], palette.variable));
        assert!(is_bold_with_fg(&out.buf[(8, 0)], palette.variable));
        assert!(!is_bold_with_fg(&out.buf[(9, 0)], palette.variable));
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

        let code = render(
            &mut ctx,
            "**bold** and `code`",
            32,
            0,
            with_cursor(Some(14)),
        )
        .text;
        assert!(code.starts_with("bold and `code`"));
        assert!(!code.contains("**bold**"));
    }

    fn render_wrapped(
        ctx: &mut RenderContext,
        text: &str,
        width: usize,
        track: Option<usize>,
        deco: LineDecorations<'_>,
    ) -> (Vec<String>, WrapOutcome) {
        let mut buf = Buffer::empty(Rect::new(0, 0, width as u16, 8));
        let outcome =
            ctx.render_line_wrapped(text, width, 0, 8, track, &deco, Some((&mut buf, 0, 0)));
        let rows = (0..outcome.rows.min(8) as u16)
            .map(|y| row_text(&buf, y).trim_end().to_string())
            .collect();
        (rows, outcome)
    }

    #[test]
    fn wrap_breaks_after_spaces_and_consumes_the_overflowing_space() {
        let mut ctx = RenderContext::new();
        let (rows, outcome) = render_wrapped(
            &mut ctx,
            "hello world foo",
            11,
            None,
            LineDecorations::default(),
        );
        assert_eq!(rows, vec!["hello world", "foo"]);
        assert_eq!(outcome.rows, 2);
    }

    #[test]
    fn wrap_keeps_words_whole_when_a_break_exists() {
        let mut ctx = RenderContext::new();
        let (rows, _) = render_wrapped(
            &mut ctx,
            "alpha beta gamma",
            12,
            None,
            LineDecorations::default(),
        );
        assert_eq!(rows, vec!["alpha beta", "gamma"]);
    }

    #[test]
    fn wrap_splits_words_longer_than_a_row() {
        let mut ctx = RenderContext::new();
        let (rows, _) = render_wrapped(&mut ctx, "abcdefghij", 4, None, LineDecorations::default());
        assert_eq!(rows, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn wrap_never_splits_a_wide_char_across_rows() {
        let mut ctx = RenderContext::new();
        let (rows, _) = render_wrapped(&mut ctx, "ab漢字", 3, None, LineDecorations::default());
        assert_eq!(rows, vec!["ab", "漢", "字"]);
    }

    #[test]
    fn wrap_measures_hidden_markers_out_of_the_width() {
        let mut ctx = RenderContext::new();
        let (rows, outcome) = render_wrapped(
            &mut ctx,
            "**bold** text",
            9,
            None,
            LineDecorations::default(),
        );
        assert_eq!(rows, vec!["bold text"]);
        assert_eq!(outcome.rows, 1);
    }

    #[test]
    fn wrap_includes_calc_ghost_in_the_layout() {
        let mut ctx = RenderContext::new();
        let deco = LineDecorations {
            calc_ghost: Some("40"),
            ..LineDecorations::default()
        };
        let (rows, _) = render_wrapped(&mut ctx, "x + 4", 6, None, deco);
        assert_eq!(rows, vec!["x + 4", "→ 40"]);
    }

    #[test]
    fn wrap_tracks_cursor_on_continuation_rows() {
        let mut ctx = RenderContext::new();
        // 'f' of "foo" is visible char 12.
        let (_, outcome) = render_wrapped(
            &mut ctx,
            "hello world foo",
            11,
            Some(12),
            LineDecorations::default(),
        );
        assert_eq!(outcome.tracked, Some((1, 0)));
        // The consumed break space stays on the first row, clamped to its edge.
        let (_, outcome) = render_wrapped(
            &mut ctx,
            "hello world foo",
            11,
            Some(11),
            LineDecorations::default(),
        );
        assert_eq!(outcome.tracked, Some((0, 10)));
    }

    #[test]
    fn wrap_end_of_full_row_moves_cursor_to_a_new_row() {
        let mut ctx = RenderContext::new();
        let (_, outcome) = render_wrapped(&mut ctx, "abcd", 4, Some(4), LineDecorations::default());
        assert_eq!(outcome.tracked, Some((1, 0)));
        assert_eq!(outcome.rows, 2);
        let (_, outcome) = render_wrapped(&mut ctx, "abc", 4, Some(3), LineDecorations::default());
        assert_eq!(outcome.tracked, Some((0, 3)));
        assert_eq!(outcome.rows, 1);
    }

    #[test]
    fn wrap_tracks_visible_chars_not_hidden_markers() {
        let mut ctx = RenderContext::new();
        // Off-cursor markers are hidden: visible text is "bold text", 't' is 5.
        let (_, outcome) = render_wrapped(
            &mut ctx,
            "**bold** text",
            6,
            Some(5),
            LineDecorations::default(),
        );
        assert_eq!(outcome.tracked, Some((1, 0)));
    }

    #[test]
    fn wrap_measure_mode_reports_rows_without_painting() {
        let mut ctx = RenderContext::new();
        let outcome = ctx.render_line_wrapped(
            "abcdefghij",
            4,
            0,
            8,
            None,
            &LineDecorations::default(),
            None,
        );
        assert_eq!(outcome.rows, 3);
    }

    #[test]
    fn wrap_empty_line_takes_one_row() {
        let mut ctx = RenderContext::new();
        let (_, outcome) = render_wrapped(&mut ctx, "", 10, Some(0), LineDecorations::default());
        assert_eq!(outcome.rows, 1);
        assert_eq!(outcome.tracked, Some((0, 0)));
    }

    #[test]
    fn wrap_char_positions_map_source_chars_to_cells() {
        let mut ctx = RenderContext::new();
        let (positions, rows) =
            ctx.wrap_char_positions("hello world foo", 11, &LineDecorations::default());
        assert_eq!(rows, 2);
        assert_eq!(positions[0], Some((0, 0)));
        assert_eq!(positions[12], Some((1, 0)));
        assert_eq!(positions[15], Some((1, 3)));
    }

    #[test]
    fn wrap_char_positions_mark_hidden_markers() {
        let mut ctx = RenderContext::new();
        let (positions, _) = ctx.wrap_char_positions("**bold** x", 20, &LineDecorations::default());
        assert_eq!(positions[0], None);
        assert_eq!(positions[2], Some((0, 0)));
        assert_eq!(positions[9], Some((0, 5)));
    }

    #[test]
    fn wrap_skip_rows_paints_from_a_later_row() {
        let mut ctx = RenderContext::new();
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
        ctx.render_line_wrapped(
            "abcdefghij",
            4,
            1,
            2,
            None,
            &LineDecorations::default(),
            Some((&mut buf, 0, 0)),
        );
        assert_eq!(row_text(&buf, 0), "efgh");
        assert_eq!(row_text(&buf, 1), "ij  ");
    }
}
