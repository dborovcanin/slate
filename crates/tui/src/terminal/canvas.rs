//! Drawing primitives over a ratatui `Buffer`. Rows and columns are 1-based
//! to match the screen layout constants used by the renderer; writes outside
//! the buffer are clipped.

use super::render::RenderPalette;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Widget};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// A fully specified cell style: unset colors mean the terminal default, and
/// unset attributes are cleared rather than inherited from the cell.
#[derive(Clone, Copy, Default)]
pub struct TextStyle {
    pub fg: Option<u8>,
    pub bg: Option<u8>,
    pub bold: bool,
    pub dim: bool,
    pub reverse: bool,
}

impl TextStyle {
    pub fn to_style(self) -> Style {
        let mut modifiers = Modifier::empty();
        modifiers.set(Modifier::BOLD, self.bold);
        modifiers.set(Modifier::DIM, self.dim);
        modifiers.set(Modifier::REVERSED, self.reverse);
        cell_style(self.fg, self.bg, modifiers)
    }
}

/// Builds a style that replaces every attribute of the cell it is applied to.
pub fn cell_style(fg: Option<u8>, bg: Option<u8>, modifiers: Modifier) -> Style {
    Style::reset()
        .fg(fg.map_or(Color::Reset, Color::Indexed))
        .bg(bg.map_or(Color::Reset, Color::Indexed))
        .remove_modifier(Modifier::all())
        .add_modifier(modifiers)
}

pub fn ansi_256_rgb(index: u8) -> (u8, u8, u8) {
    if index < 16 {
        const ANSI16: [(u8, u8, u8); 16] = [
            (0, 0, 0),
            (128, 0, 0),
            (0, 128, 0),
            (128, 128, 0),
            (0, 0, 128),
            (128, 0, 128),
            (0, 128, 128),
            (192, 192, 192),
            (128, 128, 128),
            (255, 0, 0),
            (0, 255, 0),
            (255, 255, 0),
            (0, 0, 255),
            (255, 0, 255),
            (0, 255, 255),
            (255, 255, 255),
        ];
        return ANSI16[index as usize];
    }

    if index <= 231 {
        let idx = index - 16;
        let r = idx / 36;
        let g = (idx % 36) / 6;
        let b = idx % 6;
        let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
        return (level(r), level(g), level(b));
    }

    let gray = 8 + (index - 232) * 10;
    (gray, gray, gray)
}

pub fn contrast_fg_for_bg(bg: u8) -> u8 {
    let (r, g, b) = ansi_256_rgb(bg);
    let luminance = (299u32 * r as u32 + 587u32 * g as u32 + 114u32 * b as u32) / 1000u32;
    if luminance >= 140 {
        16
    } else {
        231
    }
}

/// Writes `text` starting at (`row`, `col`), clipped to `max_width` cells and
/// to the buffer. Returns the column after the last written cell.
pub fn put_str_width(
    buf: &mut Buffer,
    row: usize,
    col: usize,
    text: &str,
    max_width: usize,
    style: Style,
) -> usize {
    let area = buf.area;
    let (Some(x), Some(y)) = (cell_x(area.x, col), cell_y(area.y, row)) else {
        return col;
    };
    if y >= area.bottom() || x >= area.right() {
        return col;
    }
    let (next_x, _) = buf.set_stringn(x, y, text, max_width, style);
    col + usize::from(next_x - x)
}

pub fn put_str(buf: &mut Buffer, row: usize, col: usize, text: &str, style: Style) -> usize {
    put_str_width(buf, row, col, text, usize::MAX, style)
}

/// Fills `width` cells at (`row`, `col`) with `style`, then writes `text`
/// over them (truncated to `width`).
pub fn draw_row_at_styled(
    buf: &mut Buffer,
    row: usize,
    col: usize,
    width: usize,
    text: &str,
    style: TextStyle,
) {
    let style = style.to_style();
    fill(buf, row, col, width, style);
    put_str_width(buf, row, col, text, width, style);
}

/// Sets `width` cells starting at (`row`, `col`) to blanks in `style`.
pub fn fill(buf: &mut Buffer, row: usize, col: usize, width: usize, style: Style) {
    let area = buf.area;
    let (Some(x), Some(y)) = (cell_x(area.x, col), cell_y(area.y, row)) else {
        return;
    };
    if y >= area.bottom() {
        return;
    }
    let end = x
        .saturating_add(u16::try_from(width).unwrap_or(u16::MAX))
        .min(area.right());
    for cx in x..end {
        buf[(cx, y)].set_char(' ').set_style(style);
    }
}

/// Writes one character cell; ignored when outside the buffer.
pub fn put_char(buf: &mut Buffer, row: usize, col: usize, ch: char, style: Style) {
    let area = buf.area;
    let (Some(x), Some(y)) = (cell_x(area.x, col), cell_y(area.y, row)) else {
        return;
    };
    if x < area.right() && y < area.bottom() {
        buf[(x, y)].set_char(ch).set_style(style);
    }
}

/// Draws `key label` hint pairs from (`row`, `col`), keys and labels in their
/// own styles, two cells apart. Pairs that do not fit in `width` are dropped
/// from the end, so list the important ones first. Returns the column after
/// the last drawn pair.
pub fn draw_key_hints(
    buf: &mut Buffer,
    row: usize,
    col: usize,
    width: usize,
    hints: &[(&str, &str)],
    key_style: Style,
    label_style: Style,
) -> usize {
    let end = col + width;
    let mut at = col;
    for (idx, (key, label)) in hints.iter().enumerate() {
        let gap = if idx == 0 { 0 } else { 2 };
        let pair_width = gap + key.width() + 1 + label.width();
        if at + pair_width > end {
            break;
        }
        at += gap;
        at = put_str(buf, row, at, key, key_style);
        at = put_str(buf, row, at + 1, label, label_style);
    }
    at
}

/// Draws a dialog surface: cleared interior filled with `bg`,
/// rounded border in `border_fg`, and optional `title` (top border) and
/// `footer` (bottom border, typically key hints).
#[allow(clippy::too_many_arguments)]
pub fn draw_framed_surface(
    buf: &mut Buffer,
    row: usize,
    col: usize,
    width: usize,
    height: usize,
    bg: u8,
    border_fg: u8,
    border_bold: bool,
    title: Option<&str>,
    footer: Option<&str>,
) {
    let Some(area) = cell_rect(buf, row, col, width, height) else {
        return;
    };
    Clear.render(area, buf);
    let surface = cell_style(None, Some(bg), Modifier::empty());
    let border = TextStyle {
        fg: Some(border_fg),
        bg: Some(bg),
        bold: border_bold,
        ..Default::default()
    }
    .to_style();
    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border)
        .style(surface);
    if let Some(title) = title.filter(|title| !title.is_empty()) {
        block = block
            .title(Line::from(format!(" {title} ")).style(border.add_modifier(Modifier::BOLD)));
    }
    if let Some(footer) = footer.filter(|footer| !footer.is_empty()) {
        let hint = cell_style(Some(border_fg), Some(bg), Modifier::DIM);
        block = block.title_bottom(
            Line::from(format!(" {footer} "))
                .style(hint)
                .right_aligned(),
        );
    }
    block.render(area, buf);
}

/// Buffer rectangle for a 1-based (`row`, `col`) box, clipped to the buffer.
fn cell_rect(buf: &Buffer, row: usize, col: usize, width: usize, height: usize) -> Option<Rect> {
    if width == 0 || height == 0 {
        return None;
    }
    let x = cell_x(buf.area.x, col.max(1))?;
    let y = cell_y(buf.area.y, row.max(1))?;
    let width = u16::try_from(width).unwrap_or(u16::MAX);
    let height = u16::try_from(height).unwrap_or(u16::MAX);
    let rect = Rect::new(x, y, width, height).intersection(buf.area);
    (!rect.is_empty()).then_some(rect)
}

fn cell_x(origin: u16, col: usize) -> Option<u16> {
    origin.checked_add(u16::try_from(col.checked_sub(1)?).ok()?)
}

fn cell_y(origin: u16, row: usize) -> Option<u16> {
    origin.checked_add(u16::try_from(row.checked_sub(1)?).ok()?)
}

/// The part of `text` shown in a `width`-cell input with the cursor at char
/// `cursor`, and the cursor's cell offset in it.
pub fn scrolled_input(text: &str, cursor: usize, width: usize) -> (String, usize) {
    let at = super::text_input::cursor(text, cursor);
    let before_width: usize = text.chars().take(at).map(|c| c.width().unwrap_or(0)).sum();
    // Keep the cursor visible in long input by scrolling the text left.
    let skip_width = before_width.saturating_sub(width.saturating_sub(1));
    let mut skipped = 0;
    let visible: String = text
        .chars()
        .skip_while(|c| {
            let skip = skipped < skip_width;
            skipped += c.width().unwrap_or(0);
            skip
        })
        .collect();
    (visible, before_width - skip_width.min(before_width))
}

/// A one-line input in a frame, like the browser's prompts: `title` on the
/// top border, `hint` on the bottom one.
pub struct InputBox<'a> {
    pub title: String,
    pub text: &'a str,
    /// Char index; `usize::MAX` means at the end.
    pub cursor: usize,
    /// Shown as one `•` per character.
    pub password: bool,
    pub hint: &'a str,
}

impl InputBox<'_> {
    fn shown(&self) -> String {
        if self.password {
            "•".repeat(self.text.chars().count())
        } else {
            self.text.to_string()
        }
    }
}

/// Draws `input` with its top-left cell at `row`, `col`; returns the screen
/// cell of its cursor.
pub fn draw_input_box(
    buf: &mut Buffer,
    row: usize,
    col: usize,
    width: usize,
    palette: &RenderPalette,
    input: &InputBox,
) -> (usize, usize) {
    let bg = palette.surface_bg();
    draw_framed_surface(
        buf,
        row,
        col,
        width,
        3,
        bg,
        palette.primary(),
        true,
        Some(&input.title),
        Some(input.hint),
    );
    let inner = width.saturating_sub(4);
    let (visible, offset) = scrolled_input(&input.shown(), input.cursor, inner);
    let style = cell_style(Some(palette.text_fg()), Some(bg), Modifier::empty());
    put_str_width(buf, row + 1, col + 2, &visible, inner, style);
    (row + 1, col + 2 + offset)
}

/// Top-left cell and width of an input box centred on a `rows` x `cols`
/// screen.
fn centered_input_box_rect(rows: usize, cols: usize) -> (usize, usize, usize) {
    let width = cols.saturating_sub(4).min(60).max(cols.min(24));
    (
        rows.saturating_sub(3) / 2 + 1,
        cols.saturating_sub(width) / 2 + 1,
        width,
    )
}

/// Draws `input` centred on a `rows` x `cols` screen.
pub fn draw_centered_input_box(
    buf: &mut Buffer,
    rows: usize,
    cols: usize,
    palette: &RenderPalette,
    input: &InputBox,
) {
    let (row, col, width) = centered_input_box_rect(rows, cols);
    draw_input_box(buf, row, col, width, palette, input);
}

/// Screen cell of the cursor of `input` drawn centred on a `rows` x `cols`
/// screen.
pub fn centered_input_box_cursor(rows: usize, cols: usize, input: &InputBox) -> (usize, usize) {
    let (row, col, width) = centered_input_box_rect(rows, cols);
    let (_, offset) = scrolled_input(&input.shown(), input.cursor, width.saturating_sub(4));
    (row + 1, col + 2 + offset)
}

#[cfg(test)]
mod tests {
    use super::test_support::{row_text, screen};
    use super::*;

    #[test]
    fn key_hints_drop_pairs_that_do_not_fit() {
        let mut buf = screen(1, 20);
        let style = Style::default();
        let end = draw_key_hints(
            &mut buf,
            1,
            1,
            20,
            &[("y", "copy"), ("x", "cut"), ("p", "paste")],
            style.add_modifier(Modifier::BOLD),
            style,
        );
        assert_eq!(row_text(&buf, 0).trim_end(), "y copy  x cut");
        assert_eq!(end, 14);
        assert!(buf[(0, 0)].modifier.contains(Modifier::BOLD));
        assert!(!buf[(2, 0)].modifier.contains(Modifier::BOLD));
    }
}

#[cfg(test)]
pub mod test_support {
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use unicode_width::UnicodeWidthStr;

    /// Text of one buffer row, skipping cells hidden behind wide characters.
    pub fn row_text(buf: &Buffer, y: u16) -> String {
        let area = buf.area;
        let mut row = String::new();
        let mut x = area.left();
        while x < area.right() {
            let symbol = buf[(x, y)].symbol();
            row.push_str(symbol);
            x += (UnicodeWidthStr::width(symbol) as u16).max(1);
        }
        row
    }

    pub fn rows_text(buf: &Buffer) -> Vec<String> {
        (buf.area.top()..buf.area.bottom())
            .map(|y| row_text(buf, y))
            .collect()
    }

    /// Blank buffer covering a `rows` x `cols` screen.
    pub fn screen(rows: u16, cols: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, cols, rows))
    }

    /// Whole-buffer text, rows joined with newlines.
    pub fn buffer_text(buf: &Buffer) -> String {
        rows_text(buf).join("\n")
    }

    pub fn has_cell(buf: &Buffer, pred: impl Fn(&Cell) -> bool) -> bool {
        buf.content().iter().any(pred)
    }

    /// True when some cell shows `symbol` with the given 256-color fg and bg.
    pub fn has_styled_symbol(buf: &Buffer, symbol: &str, fg: u8, bg: u8) -> bool {
        has_cell(buf, |cell| {
            cell.symbol() == symbol
                && cell.fg == Color::Indexed(fg)
                && cell.bg == Color::Indexed(bg)
        })
    }
}
