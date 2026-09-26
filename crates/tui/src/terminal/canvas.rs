//! Drawing primitives over a ratatui `Buffer`. Rows and columns are 1-based
//! to match the screen layout constants used by the renderer; writes outside
//! the buffer are clipped.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Widget};

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

/// Draws a dialog surface: drop shadow, cleared interior filled with `bg`,
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
    draw_shadow(buf, area, bg);
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
        block = block.title(Line::from(format!(" {title} ")).style(border.add_modifier(Modifier::BOLD)));
    }
    if let Some(footer) = footer.filter(|footer| !footer.is_empty()) {
        let hint = cell_style(Some(border_fg), Some(bg), Modifier::DIM);
        block = block.title_bottom(Line::from(format!(" {footer} ")).style(hint).right_aligned());
    }
    block.render(area, buf);
}

/// Horizontal rule across a framed surface at `row`, joining its side
/// borders (`├──┤`), with an optional dim label near the left.
pub fn draw_separator(
    buf: &mut Buffer,
    row: usize,
    col: usize,
    width: usize,
    bg: u8,
    border_fg: u8,
    label: Option<&str>,
) {
    if width < 2 {
        return;
    }
    let line = cell_style(Some(border_fg), Some(bg), Modifier::empty());
    put_char(buf, row, col, '├', line);
    for dx in 1..width - 1 {
        put_char(buf, row, col + dx, '─', line);
    }
    put_char(buf, row, col + width - 1, '┤', line);
    if let Some(label) = label.filter(|label| !label.is_empty()) {
        let text = format!(" {label} ");
        let dim = cell_style(Some(border_fg), Some(bg), Modifier::DIM);
        put_str_width(buf, row, col + 2, &text, width.saturating_sub(4), dim);
    }
}

/// Darkens the cells one column right of and one row below `area`.
fn draw_shadow(buf: &mut Buffer, area: Rect, surface_bg: u8) {
    let shadow = if contrast_fg_for_bg(surface_bg) == 16 {
        Color::Indexed(247)
    } else {
        Color::Indexed(233)
    };
    let bounds = buf.area;
    let right = area.right();
    let bottom = area.bottom();
    let mut shade = |x: u16, y: u16| {
        if x < bounds.right() && y < bounds.bottom() {
            let cell = &mut buf[(x, y)];
            cell.set_bg(shadow);
            cell.modifier.insert(Modifier::DIM);
        }
    };
    for y in area.y.saturating_add(1)..=bottom {
        shade(right, y);
    }
    for x in area.x.saturating_add(1)..=right {
        shade(x, bottom);
    }
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
