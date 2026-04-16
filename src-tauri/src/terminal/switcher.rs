use std::cmp::min;

use super::ansi::{contrast_fg_for_bg, draw_row_at_styled, AnsiStyle};
use super::render::{self, RenderPalette};
use crate::storage::Note;

#[derive(Debug, Clone)]
pub struct NoteMeta {
    pub id: String,
    pub title: String,
}

pub fn load_note_meta(db: &crate::storage::Db) -> Result<Vec<NoteMeta>, String> {
    Ok(db
        .list_notes()?
        .into_iter()
        .map(|n| NoteMeta {
            id: n.id.clone(),
            title: note_title(&n),
        })
        .collect())
}

pub fn note_title(note: &Note) -> String {
    let first = note
        .body
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("Untitled")
        .trim();
    if first.chars().count() > 60 {
        let truncated: String = first.chars().take(60).collect();
        format!("{truncated}...")
    } else {
        first.to_string()
    }
}

pub fn print_note_list(db: &crate::storage::Db) -> Result<(), String> {
    let notes = db.list_notes()?;
    if notes.is_empty() {
        println!("No notes");
        return Ok(());
    }
    for (idx, note) in notes.iter().enumerate() {
        println!("{:>3}. {}  {}", idx + 1, note.id, note_title(note));
    }
    Ok(())
}

pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q = query.to_lowercase();
    let t = text.to_lowercase();
    let q_chars: Vec<char> = q.chars().collect();
    let t_chars: Vec<char> = t.chars().collect();
    let raw_chars: Vec<char> = text.chars().collect();

    let mut qi = 0usize;
    let mut score = 0i32;
    let mut prev_match = -2isize;
    for (ti, ch) in t_chars.iter().enumerate() {
        if qi >= q_chars.len() {
            break;
        }
        if *ch == q_chars[qi] {
            score += if prev_match == ti as isize - 1 { 2 } else { 1 };
            if ti == 0
                || raw_chars
                    .get(ti - 1)
                    .map(|c| c.is_whitespace())
                    .unwrap_or(false)
            {
                score += 1;
            }
            prev_match = ti as isize;
            qi += 1;
        }
    }

    if qi == q_chars.len() {
        Some(score)
    } else {
        None
    }
}

/// View data required to render the switcher overlay.
pub struct SwitcherView<'a> {
    pub query: &'a str,
    pub items: &'a [NoteMeta],
    pub matches: &'a [usize],
    pub selected: usize,
}

pub fn draw_switcher(
    view: &SwitcherView,
    buf: &mut String,
    rows: usize,
    cols: usize,
    palette: RenderPalette,
) {
    let box_w = min(cols.saturating_sub(4).max(30), 72);
    let box_h = min(rows.saturating_sub(4).max(8), 14);
    let x = (cols.saturating_sub(box_w)) / 2 + 1;
    let y = (rows.saturating_sub(box_h)) / 2 + 1;

    let border_style = AnsiStyle {
        fg: Some(palette.code_type),
        ..Default::default()
    };
    let prompt_style = AnsiStyle {
        fg: Some(palette.code_keyword),
        bold: true,
        ..Default::default()
    };
    let label_style = AnsiStyle {
        fg: Some(palette.code_comment),
        dim: true,
        ..Default::default()
    };
    let row_style = AnsiStyle {
        fg: Some(palette.variable),
        ..Default::default()
    };
    let selected_bg = palette.search_current;
    let selected_style = AnsiStyle {
        fg: Some(contrast_fg_for_bg(selected_bg)),
        bg: Some(selected_bg),
        bold: true,
        ..Default::default()
    };

    // Border
    border_style.write_to(buf);
    for dx in 0..box_w {
        let ch_top = if dx == 0 || dx + 1 == box_w { '+' } else { '-' };
        buf.push_str(&super::ansi::goto(y, x + dx));
        buf.push(ch_top);
        buf.push_str(&super::ansi::goto(y + box_h - 1, x + dx));
        buf.push(ch_top);
    }
    for dy in 1..box_h.saturating_sub(1) {
        buf.push_str(&super::ansi::goto(y + dy, x));
        buf.push('|');
        buf.push_str(&super::ansi::goto(y + dy, x + box_w - 1));
        buf.push('|');
    }
    buf.push_str(render::RESET);

    let prompt = format!(" search: {}", view.query);
    draw_row_at_styled(
        buf,
        y + 1,
        x + 1,
        box_w.saturating_sub(2),
        &prompt,
        prompt_style,
    );
    draw_row_at_styled(
        buf,
        y + 2,
        x + 1,
        box_w.saturating_sub(2),
        " results:",
        label_style,
    );

    let max_rows = box_h.saturating_sub(4);
    let mut start = 0usize;
    if view.selected >= max_rows {
        start = view.selected + 1 - max_rows;
    }

    for i in 0..max_rows {
        let row = y + 3 + i;
        if let Some(match_idx) = view.matches.get(start + i).copied() {
            let item = &view.items[match_idx];
            let marker = if start + i == view.selected {
                ">"
            } else {
                " "
            };
            let text = format!("{marker} {}  {}", item.id, item.title);
            if start + i == view.selected {
                draw_row_at_styled(
                    buf,
                    row,
                    x + 1,
                    box_w.saturating_sub(2),
                    &text,
                    selected_style,
                );
            } else {
                draw_row_at_styled(buf, row, x + 1, box_w.saturating_sub(2), &text, row_style);
            }
        } else {
            draw_row_at_styled(buf, row, x + 1, box_w.saturating_sub(2), "", row_style);
        }
    }
}
