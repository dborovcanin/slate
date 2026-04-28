use std::cmp::min;

use super::ansi::{contrast_fg_for_bg, draw_box_border, draw_row_at_styled, AnsiStyle};
use super::render::RenderPalette;
use app_core::storage::{NoteAccessMode, NoteSearchResult};

const OVERLAY_SURFACE_BG: u8 = 236;
const CONTENT_SEARCH_MIN_H: usize = 9;
const CONTENT_SEARCH_MAX_H: usize = 14;
const CONTENT_SEARCH_PREVIEW_LINES: usize = 3;

fn fill_box_interior(
    buf: &mut String,
    row: usize,
    col: usize,
    width: usize,
    height: usize,
    style: AnsiStyle,
) {
    if width < 2 || height < 2 {
        return;
    }
    for dy in 1..height.saturating_sub(1) {
        draw_row_at_styled(buf, row + dy, col + 1, width.saturating_sub(2), "", style);
    }
}
#[derive(Debug, Clone)]
pub struct NoteMeta {
    pub id: String,
    pub title: String,
    pub access_mode: NoteAccessMode,
    pub is_unlocked: bool,
}

fn note_identity_label(note_id: &str) -> String {
    if let Some(path) = crate::markdown_file_path_from_note_id(note_id) {
        path.display().to_string()
    } else {
        note_id.to_string()
    }
}

pub fn load_note_meta(db: &crate::storage::Db) -> Result<Vec<NoteMeta>, String> {
    Ok(db
        .list_notes_meta()?
        .into_iter()
        .filter(|n| !crate::is_markdown_file_note_id(&n.id))
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
    let notes = db
        .list_notes_meta()?
        .into_iter()
        .filter(|note| !crate::is_markdown_file_note_id(&note.id))
        .collect::<Vec<_>>();
    if notes.is_empty() {
        println!("No notes");
        return Ok(());
    }
    for (idx, note) in notes.into_iter().enumerate() {
        let note_label = note_identity_label(&note.id);
        let badge = match note.access_mode {
            NoteAccessMode::None => "",
            NoteAccessMode::Locked => "[lock 󰌾] ",
            NoteAccessMode::Encrypted => "[enc 󰕥] ",
        };
        println!("{:>3}. {}  {}{}", idx + 1, note_label, badge, note.title);
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
        bg: Some(OVERLAY_SURFACE_BG),
        ..Default::default()
    };
    let prompt_style = AnsiStyle {
        fg: Some(palette.primary()),
        bg: Some(OVERLAY_SURFACE_BG),
        bold: true,
        ..Default::default()
    };
    let label_style = AnsiStyle {
        fg: Some(palette.code_comment),
        bg: Some(OVERLAY_SURFACE_BG),
        dim: true,
        ..Default::default()
    };
    let row_style = AnsiStyle {
        fg: Some(palette.variable),
        bg: Some(OVERLAY_SURFACE_BG),
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
    fill_box_interior(buf, y, x, box_w, box_h, row_style);

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
            let note_label = note_identity_label(&item.id);
            let marker = if start + i == view.selected { ">" } else { " " };
            let text = if let Some(badge) = access_badge(item) {
                format!("{marker} {}  {} {}", note_label, badge, item.title)
            } else {
                format!("{marker} {}  {}", note_label, item.title)
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

pub(crate) fn content_search_box_geometry(
    rows: usize,
    cols: usize,
) -> (usize, usize, usize, usize) {
    let box_w = min(cols.saturating_sub(4).max(30), 72);
    let box_h = min(
        rows.saturating_sub(4).max(CONTENT_SEARCH_MIN_H),
        CONTENT_SEARCH_MAX_H,
    );
    let x = (cols.saturating_sub(box_w)) / 2 + 1;
    let y = (rows.saturating_sub(box_h)) / 2 + 1;
    (x, y, box_w, box_h)
}

fn sanitize_preview_text(snippet: &str) -> String {
    let mut out = String::with_capacity(snippet.len());
    let mut chars = snippet.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '[' && chars.peek() == Some(&'[') {
            chars.next();
            continue;
        }
        if ch == ']' && chars.peek() == Some(&']') {
            chars.next();
            continue;
        }
        let safe = if matches!(ch, '\n' | '\r' | '\t') || ch.is_control() {
            ' '
        } else {
            ch
        };
        out.push(safe);
    }
    out
}

fn wrap_preview_lines(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    if width == 0 || max_lines == 0 {
        return Vec::new();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut idx = 0usize;

    while idx < chars.len() && out.len() < max_lines {
        let end = (idx + width).min(chars.len());
        out.push(chars[idx..end].iter().collect::<String>());
        idx = end;
    }

    if idx < chars.len() {
        if let Some(last) = out.last_mut() {
            if width <= 3 {
                *last = ".".repeat(width);
            } else {
                let truncated: String = last.chars().take(width.saturating_sub(3)).collect();
                *last = format!("{truncated}...");
            }
        }
    }

    out
}

pub fn draw_content_search(
    view: &ContentSearchView,
    buf: &mut String,
    rows: usize,
    cols: usize,
    palette: RenderPalette,
) {
    let (x, y, box_w, box_h) = content_search_box_geometry(rows, cols);

    let border_style = AnsiStyle {
        fg: Some(palette.code_type),
        bg: Some(OVERLAY_SURFACE_BG),
        ..Default::default()
    };
    let prompt_style = AnsiStyle {
        fg: Some(palette.primary()),
        bg: Some(OVERLAY_SURFACE_BG),
        bold: true,
        ..Default::default()
    };
    let label_style = AnsiStyle {
        fg: Some(palette.code_comment),
        bg: Some(OVERLAY_SURFACE_BG),
        dim: true,
        ..Default::default()
    };
    let row_style = AnsiStyle {
        fg: Some(palette.variable),
        bg: Some(OVERLAY_SURFACE_BG),
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
        bg: Some(OVERLAY_SURFACE_BG),
        dim: true,
        ..Default::default()
    };

    draw_box_border(buf, y, x, box_w, box_h, border_style);
    fill_box_interior(buf, y, x, box_w, box_h, row_style);

    let prompt = format!(" content: {}", view.query);
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

    // Reserve preview rows at the bottom; result rows fill the rest.
    let max_rows = box_h.saturating_sub(4 + CONTENT_SEARCH_PREVIEW_LINES); // prompt + label + preview + borders
    let mut start = 0usize;
    if view.selected >= max_rows {
        start = view.selected + 1 - max_rows;
    }

    for i in 0..max_rows {
        let row = y + 3 + i;
        if let Some(result) = view.results.get(start + i) {
            let marker = if start + i == view.selected { ">" } else { " " };
            let text = format!("{marker} L{}  {}", result.line_number.max(1), result.title);
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

    // Preview area: fixed 3 lines at the bottom of the dialog.
    let preview_start_row = y + box_h.saturating_sub(CONTENT_SEARCH_PREVIEW_LINES + 1);
    let snippet_inner_w = box_w.saturating_sub(2);
    for i in 0..CONTENT_SEARCH_PREVIEW_LINES {
        draw_row_at_styled(
            buf,
            preview_start_row + i,
            x + 1,
            snippet_inner_w,
            "",
            snippet_style,
        );
    }

    let selected_result = view.results.get(view.selected);
    if let Some(result) = selected_result {
        let prefix = format!(" L{} ", result.line_number.max(1));
        let snippet = sanitize_preview_text(result.snippet.trim());
        let preview_text = if snippet.trim().is_empty() {
            format!("{prefix}no snippet preview")
        } else {
            format!("{prefix}{snippet}")
        };
        let lines =
            wrap_preview_lines(&preview_text, snippet_inner_w, CONTENT_SEARCH_PREVIEW_LINES);
        for (idx, line) in lines.into_iter().enumerate() {
            draw_row_at_styled(
                buf,
                preview_start_row + idx,
                x + 1,
                snippet_inner_w,
                &line,
                snippet_style,
            );
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::render::RenderPalette;

    #[test]
    fn sanitize_preview_text_removes_markers_and_controls() {
        let input = "[[foo]]\nbar\tbaz\rqux";
        assert_eq!(sanitize_preview_text(input), "foo bar baz qux");
    }

    #[test]
    fn wrap_preview_lines_truncates_with_ascii_ellipsis() {
        let lines = wrap_preview_lines("abcdefghijkl", 4, 2);
        assert_eq!(lines, vec!["abcd".to_string(), "e...".to_string()]);
    }

    #[test]
    fn draw_switcher_prompt_uses_primary_color_instead_of_keyword_color() {
        let palette = RenderPalette {
            code_keyword: 33,
            primary: 201,
            ..RenderPalette::default()
        };
        let view = SwitcherView {
            query: "abc",
            items: &[],
            matches: &[],
            selected: 0,
        };
        let mut buf = String::new();
        draw_switcher(&view, &mut buf, 24, 80, palette);

        assert!(buf.contains("38;5;201"), "prompt should use accent primary");
        assert!(
            !buf.contains("38;5;33"),
            "prompt should not use keyword color"
        );
    }

    #[test]
    fn note_identity_label_decodes_markdown_file_note_id_to_path() {
        let path = std::env::temp_dir().join("switcher-mdfile-test.md");
        let note_id = crate::note_id_for_markdown_file(&path);
        assert_eq!(note_identity_label(&note_id), path.display().to_string());
        assert_eq!(note_identity_label("n1"), "n1".to_string());
    }

    #[test]
    fn draw_content_search_prompt_uses_primary_color_instead_of_keyword_color() {
        let palette = RenderPalette {
            code_keyword: 33,
            primary: 201,
            ..RenderPalette::default()
        };
        let view = ContentSearchView {
            query: "abc",
            results: &[],
            selected: 0,
        };
        let mut buf = String::new();
        draw_content_search(&view, &mut buf, 24, 80, palette);

        assert!(buf.contains("38;5;201"), "prompt should use accent primary");
        assert!(
            !buf.contains("38;5;33"),
            "prompt should not use keyword color"
        );
    }
}
