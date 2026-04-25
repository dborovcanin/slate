use std::cmp::min;

use super::ansi::{contrast_fg_for_bg, draw_box_border, draw_row_at_styled, goto, AnsiStyle};
use super::render::{RenderPalette, RESET};
use app_core::storage::{NoteAccessMode, NoteSearchResult};
#[derive(Debug, Clone)]
pub struct NoteMeta {
    pub id: String,
    pub title: String,
    pub access_mode: NoteAccessMode,
    pub is_unlocked: bool,
}

pub fn load_note_meta(db: &crate::storage::Db) -> Result<Vec<NoteMeta>, String> {
    Ok(db
        .list_notes_meta()?
        .into_iter()
        .map(|n| NoteMeta {
            title: n.title,
            id: n.id,
            access_mode: n.access_mode,
            is_unlocked: n.is_unlocked,
        })
        .collect())
}

fn access_badge(note: &NoteMeta) -> Option<&'static str> {
    match note.access_mode {
        NoteAccessMode::None => None,
        NoteAccessMode::Locked => Some("[lock 󰌾]"),
        NoteAccessMode::Encrypted => Some("[enc 󰕥]"),
    }
}

pub fn print_note_list(db: &crate::storage::Db) -> Result<(), String> {
    let notes = db.list_notes_meta()?;
    if notes.is_empty() {
        println!("No notes");
        return Ok(());
    }
    for (idx, note) in notes.into_iter().enumerate() {
        let badge = match note.access_mode {
            NoteAccessMode::None => "",
            NoteAccessMode::Locked => "[lock 󰌾] ",
            NoteAccessMode::Encrypted => "[enc 󰕥] ",
        };
        println!("{:>3}. {}  {}{}", idx + 1, note.id, badge, note.title);
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
    let selected_bg = palette.primary();
    let selected_style = AnsiStyle {
        fg: Some(contrast_fg_for_bg(selected_bg)),
        bg: Some(selected_bg),
        bold: true,
        ..Default::default()
    };

    draw_box_border(buf, y, x, box_w, box_h, border_style);

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
            let marker = if start + i == view.selected { ">" } else { " " };
            let text = if let Some(badge) = access_badge(item) {
                format!("{marker} {}  {} {}", item.id, badge, item.title)
            } else {
                format!("{marker} {}  {}", item.id, item.title)
            };
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

pub struct ContentSearchView<'a> {
    pub query: &'a str,
    pub results: &'a [NoteSearchResult],
    pub selected: usize,
}

fn snippet_segments(snippet: &str) -> Vec<(String, bool)> {
    let mut segments: Vec<(String, bool)> = Vec::new();
    let mut current = String::new();
    let mut in_match = false;
    let mut chars = snippet.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '[' && chars.peek() == Some(&'[') {
            chars.next();
            if !current.is_empty() {
                segments.push((std::mem::take(&mut current), in_match));
            }
            in_match = true;
        } else if ch == ']' && chars.peek() == Some(&']') {
            chars.next();
            if !current.is_empty() {
                segments.push((std::mem::take(&mut current), in_match));
            }
            in_match = false;
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        segments.push((current, in_match));
    }
    segments
}

fn draw_snippet_row(
    buf: &mut String,
    row: usize,
    col: usize,
    width: usize,
    snippet: &str,
    normal_style: AnsiStyle,
    match_style: AnsiStyle,
) {
    buf.push_str(&goto(row, col));
    let mut remaining = width;
    for (text, is_match) in snippet_segments(snippet) {
        if remaining == 0 {
            break;
        }
        let style = if is_match { match_style } else { normal_style };
        style.write_to(buf);
        let chars: Vec<char> = text.chars().collect();
        let take = chars.len().min(remaining);
        for ch in &chars[..take] {
            buf.push(*ch);
        }
        remaining -= take;
    }
    if remaining > 0 {
        normal_style.write_to(buf);
        for _ in 0..remaining {
            buf.push(' ');
        }
    }
    buf.push_str(RESET);
}

pub fn draw_content_search(
    view: &ContentSearchView,
    buf: &mut String,
    rows: usize,
    cols: usize,
    palette: RenderPalette,
) {
    let box_w = min(cols.saturating_sub(4).max(30), 72);
    // Extra height vs title switcher: +1 for snippet row
    let box_h = min(rows.saturating_sub(4).max(9), 16);
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
    let selected_bg = palette.primary();
    let selected_style = AnsiStyle {
        fg: Some(contrast_fg_for_bg(selected_bg)),
        bg: Some(selected_bg),
        bold: true,
        ..Default::default()
    };
    let snippet_style = AnsiStyle {
        fg: Some(palette.code_comment),
        dim: true,
        ..Default::default()
    };
    let snippet_match_style = AnsiStyle {
        fg: Some(palette.primary()),
        bold: true,
        ..Default::default()
    };

    draw_box_border(buf, y, x, box_w, box_h, border_style);

    let prompt = format!(" content: {}", view.query);
    draw_row_at_styled(buf, y + 1, x + 1, box_w.saturating_sub(2), &prompt, prompt_style);
    draw_row_at_styled(buf, y + 2, x + 1, box_w.saturating_sub(2), " results:", label_style);

    // Reserve last inner row for snippet; result rows fill the rest
    let inner_h = box_h.saturating_sub(2); // rows inside border
    let max_rows = inner_h.saturating_sub(4); // prompt(1) + label(1) + snippet(1) + gap(1)
    let mut start = 0usize;
    if view.selected >= max_rows {
        start = view.selected + 1 - max_rows;
    }

    for i in 0..max_rows {
        let row = y + 3 + i;
        if let Some(result) = view.results.get(start + i) {
            let marker = if start + i == view.selected { ">" } else { " " };
            let text = format!("{marker} {}", result.title);
            if start + i == view.selected {
                draw_row_at_styled(buf, row, x + 1, box_w.saturating_sub(2), &text, selected_style);
            } else {
                draw_row_at_styled(buf, row, x + 1, box_w.saturating_sub(2), &text, row_style);
            }
        } else {
            draw_row_at_styled(buf, row, x + 1, box_w.saturating_sub(2), "", row_style);
        }
    }

    // Snippet row: show match context for selected result
    let snippet_row = y + 3 + max_rows;
    let snippet = view
        .results
        .get(view.selected)
        .map(|r| r.snippet.as_str())
        .unwrap_or("");
    let snippet_prefix = " ";
    let snippet_inner_w = box_w.saturating_sub(2);
    if snippet.is_empty() {
        draw_row_at_styled(buf, snippet_row, x + 1, snippet_inner_w, "", snippet_style);
    } else {
        // prefix " " then rendered snippet
        draw_row_at_styled(buf, snippet_row, x + 1, 1, snippet_prefix, snippet_style);
        draw_snippet_row(
            buf,
            snippet_row,
            x + 1 + snippet_prefix.len(),
            snippet_inner_w.saturating_sub(snippet_prefix.len()),
            snippet,
            snippet_style,
            snippet_match_style,
        );
    }
}

pub fn draw_delete_confirm(
    note_title: &str,
    requires_password: bool,
    password_len: usize,
    buf: &mut String,
    rows: usize,
    cols: usize,
    palette: RenderPalette,
) {
    let title = truncate_title_for_confirm(note_title);
    let message = format!(" Delete \"{title}\"? ");
    let password_prompt = if requires_password {
        let masked = if password_len == 0 {
            "<required>".to_string()
        } else {
            "*".repeat(password_len.min(32))
        };
        format!(" Password: {masked} ")
    } else {
        String::new()
    };
    let hint = if requires_password {
        " Enter confirm, Esc cancel "
    } else {
        " Enter/Y confirm, Esc/N cancel "
    };
    let mut inner_w = message.chars().count().max(hint.chars().count()).max(30);
    if requires_password {
        inner_w = inner_w.max(password_prompt.chars().count());
    }
    let box_w = (inner_w + 2).min(cols.saturating_sub(4).max(24));
    let target_h = if requires_password { 6 } else { 5 };
    let box_h = target_h.min(rows.saturating_sub(2).max(target_h));
    let x = (cols.saturating_sub(box_w)) / 2 + 1;
    let y = (rows.saturating_sub(box_h)) / 2 + 1;

    let border_style = AnsiStyle {
        fg: Some(palette.primary()),
        bold: true,
        ..Default::default()
    };
    let message_style = AnsiStyle {
        fg: Some(palette.primary()),
        bold: true,
        ..Default::default()
    };
    let hint_style = AnsiStyle {
        fg: Some(palette.code_comment),
        dim: true,
        ..Default::default()
    };

    draw_box_border(buf, y, x, box_w, box_h, border_style);

    draw_row_at_styled(
        buf,
        y + 1,
        x + 1,
        box_w.saturating_sub(2),
        &message,
        message_style,
    );
    if requires_password {
        draw_row_at_styled(
            buf,
            y + 2,
            x + 1,
            box_w.saturating_sub(2),
            &password_prompt,
            message_style,
        );
    }
    draw_row_at_styled(
        buf,
        y + if requires_password { 3 } else { 2 },
        x + 1,
        box_w.saturating_sub(2),
        &hint,
        hint_style,
    );
}

pub fn draw_open_confirm(
    note_title: &str,
    password_len: usize,
    buf: &mut String,
    rows: usize,
    cols: usize,
    palette: RenderPalette,
) {
    draw_confirm(
        &format!(" Open \"{}\" ", truncate_title_for_confirm(note_title)),
        Some(password_len),
        " Enter confirm, Esc cancel ",
        buf,
        rows,
        cols,
        palette,
    );
}

fn draw_confirm(
    message: &str,
    password_len: Option<usize>,
    hint: &str,
    buf: &mut String,
    rows: usize,
    cols: usize,
    palette: RenderPalette,
) {
    let password_prompt = password_len.map(|len| {
        let masked = if len == 0 {
            "<required>".to_string()
        } else {
            "*".repeat(len.min(32))
        };
        format!(" Password: {masked} ")
    });
    let mut inner_w = message.chars().count().max(hint.chars().count()).max(30);
    if let Some(prompt) = password_prompt.as_ref() {
        inner_w = inner_w.max(prompt.chars().count());
    }
    let box_w = (inner_w + 2).min(cols.saturating_sub(4).max(24));
    let target_h = if password_prompt.is_some() { 6 } else { 5 };
    let box_h = target_h.min(rows.saturating_sub(2).max(target_h));
    let x = (cols.saturating_sub(box_w)) / 2 + 1;
    let y = (rows.saturating_sub(box_h)) / 2 + 1;

    let border_style = AnsiStyle {
        fg: Some(palette.primary()),
        bold: true,
        ..Default::default()
    };
    let message_style = AnsiStyle {
        fg: Some(palette.primary()),
        bold: true,
        ..Default::default()
    };
    let hint_style = AnsiStyle {
        fg: Some(palette.code_comment),
        dim: true,
        ..Default::default()
    };

    draw_box_border(buf, y, x, box_w, box_h, border_style);

    draw_row_at_styled(
        buf,
        y + 1,
        x + 1,
        box_w.saturating_sub(2),
        message,
        message_style,
    );
    if let Some(prompt) = password_prompt.as_ref() {
        draw_row_at_styled(
            buf,
            y + 2,
            x + 1,
            box_w.saturating_sub(2),
            prompt,
            message_style,
        );
    }
    draw_row_at_styled(
        buf,
        y + if password_prompt.is_some() { 3 } else { 2 },
        x + 1,
        box_w.saturating_sub(2),
        hint,
        hint_style,
    );
}

fn truncate_title_for_confirm(value: &str) -> String {
    const MAX_CHARS: usize = 48;
    let mut out = String::new();
    for (idx, ch) in value.chars().enumerate() {
        if idx >= MAX_CHARS {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}
