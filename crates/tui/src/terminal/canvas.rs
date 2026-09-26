//! Drawing primitives over a ratatui `Buffer`. Rows and columns are 1-based
//! to match the screen layout constants used by the renderer; writes outside
//! the buffer are clipped.

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};

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

/// Truncates or space-pads `text` to exactly `width` chars.
pub fn pad_right(text: &str, width: usize) -> String {
    let mut out: String = text.chars().take(width).collect();
    let current = out.chars().count();
    if current < width {
        out.push_str(&" ".repeat(width - current));
    }
    out
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
    let end = x.saturating_add(u16::try_from(width).unwrap_or(u16::MAX)).min(area.right());
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

pub fn draw_framed_surface(
    buf: &mut Buffer,
    row: usize,
    col: usize,
    width: usize,
    height: usize,
    bg: u8,
    border_fg: u8,
    border_bold: bool,
) {
    if width == 0 || height == 0 {
        return;
    }
    let fill_style = TextStyle {
        bg: Some(bg),
        ..Default::default()
    };
    for dy in 0..height {
        draw_row_at_styled(buf, row + dy, col, width, "", fill_style);
    }
    let border_style = TextStyle {
        fg: Some(border_fg),
        bg: Some(bg),
        bold: border_bold,
        ..Default::default()
    };
    draw_box_border(buf, row, col, width, height, border_style);
}

pub fn draw_box_border(
    buf: &mut Buffer,
    row: usize,
    col: usize,
    width: usize,
    height: usize,
    style: TextStyle,
) {
    if width == 0 || height == 0 {
        return;
    }
    let style = style.to_style();
    let row = row.max(1);
    let col = col.max(1);
    if width == 1 || height == 1 {
        for dx in 0..width {
            put_char(buf, row, col + dx, '─', style);
        }
        return;
    }

    put_char(buf, row, col, '┌', style);
    for dx in 1..width.saturating_sub(1) {
        put_char(buf, row, col + dx, '─', style);
    }
    put_char(buf, row, col + width - 1, '┐', style);

    let bottom = row + height - 1;
    put_char(buf, bottom, col, '└', style);
    for dx in 1..width.saturating_sub(1) {
        put_char(buf, bottom, col + dx, '─', style);
    }
    put_char(buf, bottom, col + width - 1, '┘', style);

    for dy in 1..height.saturating_sub(1) {
        put_char(buf, row + dy, col, '│', style);
        put_char(buf, row + dy, col + width - 1, '│', style);
    }
}

fn cell_x(origin: u16, col: usize) -> Option<u16> {
    origin.checked_add(u16::try_from(col.checked_sub(1)?).ok()?)
}

fn cell_y(origin: u16, row: usize) -> Option<u16> {
    origin.checked_add(u16::try_from(row.checked_sub(1)?).ok()?)
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
            cell.symbol() == symbol && cell.fg == Color::Indexed(fg) && cell.bg == Color::Indexed(bg)
        })
    }

    pub fn has_fg(buf: &Buffer, fg: u8) -> bool {
        has_cell(buf, |cell| cell.fg == Color::Indexed(fg))
    }
}
