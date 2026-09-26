//! Transitional adapter: paints the legacy ANSI frame string (cursor moves +
//! SGR styles + text) into a ratatui `Buffer`. Only the escape subset the
//! renderer emits is understood: `ESC[{row};{col}H` and `ESC[...m` with codes
//! 0/1/2/3/4/7/9 and 256-color `38;5;n` / `48;5;n`. Removed once the renderer
//! writes into the buffer directly.

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};

pub fn paint(frame: &str, buf: &mut Buffer) {
    let area = buf.area;
    let mut style = Style::reset();
    // 0-based cell position; `None` until the first cursor move.
    let mut pos: Option<(u16, u16)> = None;
    let mut run = String::new();
    let mut rest = frame;

    while !rest.is_empty() {
        if let Some(after_esc) = rest.strip_prefix("\x1b[") {
            flush_run(buf, &mut run, &mut pos, style);
            let end = after_esc
                .find(|ch: char| ch.is_ascii_alphabetic())
                .unwrap_or(after_esc.len());
            let params = &after_esc[..end];
            match after_esc[end..].chars().next() {
                Some('H') => pos = parse_goto(params, area.x, area.y),
                Some('m') => style = apply_sgr(params),
                _ => {}
            }
            rest = &after_esc[(end + 1).min(after_esc.len())..];
            continue;
        }
        let next_esc = rest.find('\x1b').unwrap_or(rest.len());
        let (text, tail) = if next_esc == 0 {
            // Lone ESC without '[': skip it.
            ("", &rest[1..])
        } else {
            rest.split_at(next_esc)
        };
        run.push_str(text);
        rest = tail;
    }
    flush_run(buf, &mut run, &mut pos, style);
}

fn flush_run(buf: &mut Buffer, run: &mut String, pos: &mut Option<(u16, u16)>, style: Style) {
    if run.is_empty() {
        return;
    }
    if let Some((x, y)) = *pos {
        let area = buf.area;
        if y < area.bottom() && x < area.right() {
            let max_width = usize::from(area.right() - x);
            let (next_x, _) = buf.set_stringn(x, y, run.as_str(), max_width, style);
            *pos = Some((next_x, y));
        }
    }
    run.clear();
}

fn parse_goto(params: &str, origin_x: u16, origin_y: u16) -> Option<(u16, u16)> {
    let (row, col) = params.split_once(';').unwrap_or((params, "1"));
    let row: u16 = row.parse().unwrap_or(1).max(1);
    let col: u16 = col.parse().unwrap_or(1).max(1);
    Some((origin_x + col - 1, origin_y + row - 1))
}

fn apply_sgr(params: &str) -> Style {
    let mut style = Style::reset();
    let mut codes = params.split(';').map(|code| code.parse::<u16>().unwrap_or(0));
    while let Some(code) = codes.next() {
        match code {
            0 => style = Style::reset(),
            1 => style = style.add_modifier(Modifier::BOLD),
            2 => style = style.add_modifier(Modifier::DIM),
            3 => style = style.add_modifier(Modifier::ITALIC),
            4 => style = style.add_modifier(Modifier::UNDERLINED),
            7 => style = style.add_modifier(Modifier::REVERSED),
            9 => style = style.add_modifier(Modifier::CROSSED_OUT),
            38 | 48 => {
                if codes.next() == Some(5) {
                    if let Some(index) = codes.next() {
                        let color = Color::Indexed(index.min(255) as u8);
                        style = if code == 38 {
                            style.fg(color)
                        } else {
                            style.bg(color)
                        };
                    }
                }
            }
            _ => {}
        }
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn paints_text_at_goto_positions_with_styles() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 2));
        paint("\x1b[2;3H\x1b[0;1;38;5;4;48;5;7mab\x1b[0mc", &mut buf);
        assert_eq!(buf[(2, 1)].symbol(), "a");
        assert_eq!(buf[(3, 1)].symbol(), "b");
        assert_eq!(buf[(4, 1)].symbol(), "c");
        assert_eq!(buf[(2, 1)].fg, Color::Indexed(4));
        assert_eq!(buf[(2, 1)].bg, Color::Indexed(7));
        assert!(buf[(2, 1)].modifier.contains(Modifier::BOLD));
        assert_eq!(buf[(4, 1)].fg, Color::Reset);
    }

    #[test]
    fn clips_text_at_the_right_edge_and_ignores_rows_below() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 1));
        paint("\x1b[1;3Hxyz\x1b[2;1Hhidden", &mut buf);
        assert_eq!(buf[(2, 0)].symbol(), "x");
        assert_eq!(buf[(3, 0)].symbol(), "y");
    }

    #[test]
    fn wide_chars_occupy_two_cells() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 6, 1));
        paint("\x1b[1;1H⏰x", &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "⏰");
        assert_eq!(buf[(2, 0)].symbol(), "x");
    }
}
