use std::fmt::Write as _;

use super::render;

#[derive(Clone, Copy, Default)]
pub struct AnsiStyle {
    pub fg: Option<u8>,
    pub bg: Option<u8>,
    pub bold: bool,
    pub dim: bool,
    pub reverse: bool,
}

impl AnsiStyle {
    pub fn write_to(self, buf: &mut String) {
        buf.push_str("\x1b[0");
        if self.bold {
            buf.push_str(";1");
        }
        if self.dim {
            buf.push_str(";2");
        }
        if self.reverse {
            buf.push_str(";7");
        }
        if let Some(fg) = self.fg {
            let _ = write!(buf, ";38;5;{fg}");
        }
        if let Some(bg) = self.bg {
            let _ = write!(buf, ";48;5;{bg}");
        }
        buf.push('m');
    }
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

pub fn goto(row: usize, col: usize) -> String {
    format!("\x1b[{};{}H", row.max(1), col.max(1))
}

pub fn pad_right(text: &str, width: usize) -> String {
    let mut out: String = text.chars().take(width).collect();
    let current = out.chars().count();
    if current < width {
        out.push_str(&" ".repeat(width - current));
    }
    out
}

pub fn draw_row_at_styled(
    buf: &mut String,
    row: usize,
    col: usize,
    width: usize,
    text: &str,
    style: AnsiStyle,
) {
    buf.push_str(&goto(row, col));
    style.write_to(buf);
    buf.push_str(&pad_right(text, width));
    buf.push_str(render::RESET);
}

pub fn draw_box_border(
    buf: &mut String,
    row: usize,
    col: usize,
    width: usize,
    height: usize,
    style: AnsiStyle,
) {
    if width == 0 || height == 0 {
        return;
    }
    let row = row.max(1);
    let col = col.max(1);
    style.write_to(buf);
    if width == 1 || height == 1 {
        for dx in 0..width {
            buf.push_str(&goto(row, col + dx));
            buf.push('─');
        }
        buf.push_str(render::RESET);
        return;
    }

    buf.push_str(&goto(row, col));
    buf.push('┌');
    for dx in 1..width.saturating_sub(1) {
        buf.push_str(&goto(row, col + dx));
        buf.push('─');
    }
    buf.push_str(&goto(row, col + width - 1));
    buf.push('┐');

    let bottom = row + height - 1;
    buf.push_str(&goto(bottom, col));
    buf.push('└');
    for dx in 1..width.saturating_sub(1) {
        buf.push_str(&goto(bottom, col + dx));
        buf.push('─');
    }
    buf.push_str(&goto(bottom, col + width - 1));
    buf.push('┘');

    for dy in 1..height.saturating_sub(1) {
        buf.push_str(&goto(row + dy, col));
        buf.push('│');
        buf.push_str(&goto(row + dy, col + width - 1));
        buf.push('│');
    }
    buf.push_str(render::RESET);
}
