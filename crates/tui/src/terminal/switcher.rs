use std::cmp::min;

use super::canvas::{contrast_fg_for_bg, draw_framed_surface, draw_row_at_styled, TextStyle};
use super::render::RenderPalette;
use app_core::storage::{Collection, NoteAccessMode};
use ratatui::buffer::Buffer;

fn fill_box_interior(
    buf: &mut Buffer,
    row: usize,
    col: usize,
    width: usize,
    height: usize,
    style: TextStyle,
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
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct CollectionMeta {
    pub id: Option<String>,
    pub name: String,
    pub description: String,
    pub is_clear: bool,
    pub encrypted: bool,
}

pub(crate) fn note_identity_label(note_id: &str) -> String {
    if let Some(path) = crate::file_path_from_note_id(note_id) {
        path.display().to_string()
    } else {
        note_id.to_string()
    }
}

pub fn load_note_meta_filtered(
    db: &crate::storage::Db,
    active_note_id: Option<&str>,
    collection_id: Option<&str>,
) -> Result<Vec<NoteMeta>, String> {
    let note_sources = app_core::note_sources::NoteSourceService::new(db.clone());
    Ok(note_sources
        .list_notes_meta_filtered(active_note_id, collection_id)?
        .into_iter()
        .map(|n| NoteMeta {
            title: n.title,
            id: n.id,
            access_mode: n.access_mode,
            is_unlocked: n.is_unlocked,
            updated_at: n.updated_at,
        })
        .collect())
}

pub fn load_collection_meta(db: &crate::storage::Db) -> Result<Vec<CollectionMeta>, String> {
    let mut items = vec![CollectionMeta {
        id: None,
        name: "All collections".to_string(),
        description: "Clear active session collection".to_string(),
        is_clear: true,
        encrypted: false,
    }];
    let collections = db.list_collections()?;
    items.extend(collections.into_iter().map(collection_to_meta));
    Ok(items)
}

pub fn collection_to_meta(collection: Collection) -> CollectionMeta {
    CollectionMeta {
        id: Some(collection.id),
        name: collection.name,
        description: collection.description,
        is_clear: false,
        encrypted: collection.encrypted,
    }
}

pub fn print_note_list(db: &crate::storage::Db) -> Result<(), String> {
    let note_sources = app_core::note_sources::NoteSourceService::new(db.clone());
    let notes = note_sources.list_notes_meta(None)?;
    if notes.is_empty() {
        println!("No notes");
        return Ok(());
    }
    for (idx, note) in notes.into_iter().enumerate() {
        let note_label = note_identity_label(&note.id);
        let badge = match note.access_mode {
            NoteAccessMode::None => "",
            NoteAccessMode::Encrypted => "[encrypted at rest] ",
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

pub struct WebSearchView<'a> {
    pub query: &'a str,
    pub results: &'a [app_core::web_search::WebSearchItem],
    pub answer: Option<&'a str>,
    pub summary: Option<&'a str>,
    pub answer_card: Option<&'a app_core::web_search::WebSearchAnswerCard>,
    pub selected: usize,
    pub pending: bool,
    pub error: Option<&'a str>,
}

pub struct CollectionEditView<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub default_tags: &'a str,
    pub selected_field: usize,
}

pub fn draw_collection_edit_dialog(
    view: &CollectionEditView,
    buf: &mut Buffer,
    rows: usize,
    cols: usize,
    palette: RenderPalette,
) {
    let box_w = min(cols.saturating_sub(4).max(48), 88);
    let box_h = min(rows.saturating_sub(4).max(10), 12);
    let x = (cols.saturating_sub(box_w)) / 2 + 1;
    let y = (rows.saturating_sub(box_h)) / 2 + 1;
    let surface_bg = palette.surface_bg();
    let normal_style = TextStyle {
        fg: Some(palette.variable),
        bg: Some(surface_bg),
        ..Default::default()
    };
    let label_style = TextStyle {
        fg: Some(palette.code_comment),
        bg: Some(surface_bg),
        dim: true,
        ..Default::default()
    };
    let active_bg = palette.primary();
    let active_style = TextStyle {
        fg: Some(contrast_fg_for_bg(active_bg)),
        bg: Some(active_bg),
        bold: true,
        ..Default::default()
    };

    draw_framed_surface(
        buf,
        y,
        x,
        box_w,
        box_h,
        surface_bg,
        palette.primary(),
        false,
        Some("Edit collection"),
        None,
    );
    fill_box_interior(buf, y, x, box_w, box_h, normal_style);

    draw_row_at_styled(
        buf,
        y + 1,
        x + 1,
        box_w.saturating_sub(2),
        " Edit collection",
        TextStyle {
            fg: Some(palette.primary()),
            bg: Some(surface_bg),
            bold: true,
            ..Default::default()
        },
    );
    draw_row_at_styled(
        buf,
        y + 2,
        x + 1,
        box_w.saturating_sub(2),
        " Tab/Shift+Tab move field  Enter save  Esc cancel",
        label_style,
    );

    let fields = [
        format!(" Name: {}", view.name),
        format!(" Description: {}", view.description),
        format!(" Default tags: {}", view.default_tags),
    ];
    for (idx, text) in fields.iter().enumerate() {
        let style = if idx == view.selected_field {
            active_style
        } else {
            normal_style
        };
        draw_row_at_styled(
            buf,
            y + 4 + idx,
            x + 1,
            box_w.saturating_sub(2),
            text,
            style,
        );
    }
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

pub(crate) fn web_search_box_geometry(rows: usize, cols: usize) -> (usize, usize, usize, usize) {
    let box_w = min(cols.saturating_sub(4).max(40), 86);
    let box_h = min(rows.saturating_sub(4).max(10), 18);
    let x = (cols.saturating_sub(box_w)) / 2 + 1;
    let y = (rows.saturating_sub(box_h)) / 2 + 1;
    (x, y, box_w, box_h)
}

pub fn draw_web_search(
    view: &WebSearchView,
    buf: &mut Buffer,
    rows: usize,
    cols: usize,
    palette: RenderPalette,
) {
    let (x, y, box_w, box_h) = web_search_box_geometry(rows, cols);
    let surface_bg = palette.surface_bg();

    let prompt_style = TextStyle {
        fg: Some(palette.primary()),
        bg: Some(surface_bg),
        bold: true,
        ..Default::default()
    };
    let label_style = TextStyle {
        fg: Some(palette.code_comment),
        bg: Some(surface_bg),
        dim: true,
        ..Default::default()
    };
    let row_style = TextStyle {
        fg: Some(palette.variable),
        bg: Some(surface_bg),
        ..Default::default()
    };
    let selected_bg = palette.primary();
    let selected_style = TextStyle {
        fg: Some(contrast_fg_for_bg(selected_bg)),
        bg: Some(selected_bg),
        bold: true,
        ..Default::default()
    };
    let snippet_style = TextStyle {
        fg: Some(palette.code_comment),
        bg: Some(surface_bg),
        dim: true,
        ..Default::default()
    };
    let error_style = TextStyle {
        fg: Some(palette.search_current),
        bg: Some(surface_bg),
        bold: true,
        ..Default::default()
    };

    draw_framed_surface(
        buf,
        y,
        x,
        box_w,
        box_h,
        surface_bg,
        palette.primary(),
        false,
        Some("Web search"),
        None,
    );
    fill_box_interior(buf, y, x, box_w, box_h, row_style);

    draw_prompt(
        buf,
        y + 1,
        x + 1,
        box_w.saturating_sub(2),
        view.query,
        "search the web",
        prompt_style,
        palette,
    );

    let status_label_row = y + 2;
    if view.pending {
        draw_row_at_styled(
            buf,
            status_label_row,
            x + 1,
            box_w.saturating_sub(2),
            " searching...",
            label_style,
        );
    } else if let Some(err) = view.error {
        let err_text = format!(" error: {err}");
        draw_row_at_styled(
            buf,
            status_label_row,
            x + 1,
            box_w.saturating_sub(2),
            &err_text,
            error_style,
        );
    } else if view.results.is_empty()
        && view.answer.is_none()
        && view.summary.is_none()
        && view.answer_card.is_none()
    {
        if !view.query.is_empty() {
            draw_row_at_styled(
                buf,
                status_label_row,
                x + 1,
                box_w.saturating_sub(2),
                " press Enter to search",
                label_style,
            );
        } else {
            draw_row_at_styled(
                buf,
                status_label_row,
                x + 1,
                box_w.saturating_sub(2),
                " type a query and press Enter",
                label_style,
            );
        }
    } else {
        let count_text = if view.answer_card.is_some() {
            format!(" answer card + {} link results:", view.results.len())
        } else if view.answer.is_some() {
            format!(" answer + {} link results:", view.results.len())
        } else if view.summary.is_some() {
            format!(" summary + {} link results:", view.results.len())
        } else {
            format!(" link results ({}):", view.results.len())
        };
        draw_row_at_styled(
            buf,
            status_label_row,
            x + 1,
            box_w.saturating_sub(2),
            &count_text,
            label_style,
        );
    }

    let preview_lines_count = 5;
    let layout_rows = 4; // prompt + label + preview + borders
    let max_rows = box_h.saturating_sub(layout_rows + preview_lines_count);
    let mut start = 0usize;
    if view.selected >= max_rows && max_rows > 0 {
        start = view.selected + 1 - max_rows;
    }

    for i in 0..max_rows {
        let row = status_label_row + 1 + i;
        if let Some(item) = view.results.get(start + i) {
            let marker = " ";
            let text = format!("{marker} {} ({})", item.title, item.url);
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
        }
    }

    // Keep the shared answer card visible; otherwise preview selected result text.
    let preview_start_row = y + box_h.saturating_sub(preview_lines_count + 1);
    let (preview_label, preview, preview_source) = if let Some(card) = view.answer_card {
        (
            format!(" {}:", card.title.to_ascii_lowercase()),
            Some(card.text.as_str()),
            card.sources.first(),
        )
    } else if let Some(answer) = view.answer {
        (" answer:".to_string(), Some(answer), None)
    } else {
        let selected_text = view
            .results
            .get(view.selected)
            .map(|item| item.snippet.as_str())
            .filter(|snippet| !snippet.trim().is_empty());
        if selected_text.is_some() {
            (" result text:".to_string(), selected_text, None)
        } else if view.summary.is_some() {
            (" summary:".to_string(), view.summary, None)
        } else {
            (
                " result text:".to_string(),
                view.results
                    .iter()
                    .map(|item| item.snippet.as_str())
                    .find(|snippet| !snippet.trim().is_empty()),
                None,
            )
        }
    };
    if let Some(preview) = preview {
        draw_row_at_styled(
            buf,
            preview_start_row,
            x + 1,
            box_w.saturating_sub(2),
            &preview_label,
            label_style,
        );
        let snippet_text = sanitize_preview_text(preview);
        let source_lines = usize::from(preview_source.is_some());
        let wrapped = wrap_preview_lines(
            &snippet_text,
            box_w.saturating_sub(4),
            preview_lines_count.saturating_sub(1 + source_lines),
        );
        for (idx, line) in wrapped.iter().enumerate() {
            let text = format!("  {line}");
            draw_row_at_styled(
                buf,
                preview_start_row + idx + 1,
                x + 1,
                box_w.saturating_sub(2),
                &text,
                snippet_style,
            );
        }
        if let Some(source) = preview_source {
            let source_text = format!(" source: {} ({})", source.title, source.url);
            draw_row_at_styled(
                buf,
                preview_start_row + preview_lines_count.saturating_sub(1),
                x + 1,
                box_w.saturating_sub(2),
                &source_text,
                label_style,
            );
        }
    }

    // Footnote
    let footnote_row = y + box_h.saturating_sub(1);
    draw_row_at_styled(
        buf,
        footnote_row,
        x + 1,
        box_w.saturating_sub(2),
        " Enter: open | Shift+Enter: insert link | Esc: close",
        label_style,
    );
}

pub fn draw_delete_confirm(
    note_title: &str,
    requires_password: bool,
    password_len: usize,
    buf: &mut Buffer,
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
    let target_h = if requires_password { 4 } else { 3 };
    let box_h = target_h.min(rows.saturating_sub(2).max(target_h));
    let x = (cols.saturating_sub(box_w)) / 2 + 1;
    let y = (rows.saturating_sub(box_h)) / 2 + 1;
    let surface_bg = palette.surface_bg();

    let message_style = TextStyle {
        fg: Some(palette.primary()),
        bg: Some(surface_bg),
        bold: true,
        ..Default::default()
    };

    draw_framed_surface(
        buf,
        y,
        x,
        box_w,
        box_h,
        surface_bg,
        palette.primary(),
        true,
        Some("Delete note"),
        Some(hint.trim()),
    );

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
}

/// Password prompt for a locked note; `collection` names the encrypted
/// collection whose password it needs.
pub fn draw_open_confirm(
    note_title: &str,
    collection: Option<&str>,
    password_len: usize,
    buf: &mut Buffer,
    rows: usize,
    cols: usize,
    palette: RenderPalette,
) {
    let title = match collection {
        Some(collection) => format!("Unlock {}", truncate_title_for_confirm(collection)),
        None => "Unlock note".to_string(),
    };
    draw_confirm(
        &title,
        &format!(" Open \"{}\" ", truncate_title_for_confirm(note_title)),
        Some(password_len),
        " Enter confirm, Esc cancel ",
        buf,
        rows,
        cols,
        palette,
    );
}

/// Masked password dialog: `title` on the frame, `message` above the field.
pub fn draw_password_dialog(
    title: &str,
    message: &str,
    password_len: usize,
    buf: &mut Buffer,
    rows: usize,
    cols: usize,
    palette: RenderPalette,
) {
    draw_confirm(
        title,
        &format!(" {message} "),
        Some(password_len),
        " Enter confirm, Esc cancel ",
        buf,
        rows,
        cols,
        palette,
    );
}

fn draw_confirm(
    title: &str,
    message: &str,
    password_len: Option<usize>,
    hint: &str,
    buf: &mut Buffer,
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
    let target_h = if password_prompt.is_some() { 4 } else { 3 };
    let box_h = target_h.min(rows.saturating_sub(2).max(target_h));
    let x = (cols.saturating_sub(box_w)) / 2 + 1;
    let y = (rows.saturating_sub(box_h)) / 2 + 1;
    let surface_bg = palette.surface_bg();

    let message_style = TextStyle {
        fg: Some(palette.primary()),
        bg: Some(surface_bg),
        bold: true,
        ..Default::default()
    };

    draw_framed_surface(
        buf,
        y,
        x,
        box_w,
        box_h,
        surface_bg,
        palette.primary(),
        true,
        Some(title),
        Some(hint.trim()),
    );

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

/// Text before the query in overlay prompts; the cursor sits after it.
pub const PROMPT_PREFIX: &str = " › ";

/// Draws an overlay prompt row: accent `›`, the query, and a dim placeholder
/// while the query is empty.
#[allow(clippy::too_many_arguments)]
fn draw_prompt(
    buf: &mut Buffer,
    row: usize,
    col: usize,
    width: usize,
    query: &str,
    placeholder: &str,
    prompt_style: TextStyle,
    palette: RenderPalette,
) {
    draw_row_at_styled(buf, row, col, width, PROMPT_PREFIX, prompt_style);
    let text_col = col + PROMPT_PREFIX.chars().count();
    let text_width = width.saturating_sub(PROMPT_PREFIX.chars().count());
    if query.is_empty() {
        let hint = TextStyle {
            fg: Some(palette.code_comment),
            bg: prompt_style.bg,
            dim: true,
            ..Default::default()
        };
        draw_row_at_styled(buf, row, text_col, text_width, placeholder, hint);
    } else {
        let text = TextStyle {
            bold: false,
            ..prompt_style
        };
        draw_row_at_styled(buf, row, text_col, text_width, query, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::canvas::test_support::{buffer_text, screen};
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
    fn note_identity_label_decodes_markdown_file_note_id_to_path() {
        let path = std::env::temp_dir().join("switcher-mdfile-test.md");
        let note_id = crate::note_id_for_file(&path);
        assert_eq!(note_identity_label(&note_id), path.display().to_string());
        assert_eq!(note_identity_label("n1"), "n1".to_string());
    }

    #[test]
    fn draw_web_search_prioritizes_direct_answer_over_selected_snippet() {
        let result = app_core::web_search::WebSearchItem {
            title: "Conversion result".to_string(),
            url: "https://example.com/conversion".to_string(),
            snippet: "ordinary result snippet".to_string(),
            markdown_link: "[Conversion result](https://example.com/conversion)".to_string(),
        };
        let view = WebSearchView {
            query: "35cm to inches",
            results: &[result],
            answer: Some("35 cm = 13.7795 inches"),
            summary: Some("fallback summary"),
            answer_card: None,
            selected: 0,
            pending: false,
            error: None,
        };
        let mut buf = screen(24, 100);
        draw_web_search(&view, &mut buf, 24, 100, RenderPalette::default());

        assert!(buffer_text(&buf).contains("answer:"));
        assert!(buffer_text(&buf).contains("35 cm = 13.7795 inches"));
        assert!(!buffer_text(&buf).contains("ordinary result snippet"));
    }

    #[test]
    fn draw_web_search_shows_shared_answer_card_with_source() {
        let card = app_core::web_search::WebSearchAnswerCard {
            title: "Best result".to_string(),
            text: "The tallest building in Europe is the Lakhta Center.".to_string(),
            sources: vec![app_core::web_search::WebSearchSource {
                title: "List of tallest buildings in Europe".to_string(),
                url: "https://example.com/tallest-buildings".to_string(),
            }],
        };
        let view = WebSearchView {
            query: "tallest building in Europe",
            results: &[],
            answer: None,
            summary: None,
            answer_card: Some(&card),
            selected: 0,
            pending: false,
            error: None,
        };
        let mut buf = screen(24, 100);
        draw_web_search(&view, &mut buf, 24, 100, RenderPalette::default());

        assert!(buffer_text(&buf).contains("best result:"));
        assert!(buffer_text(&buf).contains("The tallest building in Europe is the Lakhta Center."));
        assert!(buffer_text(&buf).contains("source: List of tallest buildings in Europe"));
        assert!(buffer_text(&buf).contains("https://example.com/tallest-buildings"));
    }

    #[test]
    fn draw_web_search_shows_selected_result_text_when_no_direct_answer_exists() {
        let result = app_core::web_search::WebSearchItem {
            title: "Novak Djokovic".to_string(),
            url: "https://example.com/novak".to_string(),
            snippet: "Novak Djokovic is a Serbian tennis player.".to_string(),
            markdown_link: "[Novak Djokovic](https://example.com/novak)".to_string(),
        };
        let view = WebSearchView {
            query: "Novak",
            results: &[result],
            answer: None,
            summary: Some("fallback summary"),
            answer_card: None,
            selected: 0,
            pending: false,
            error: None,
        };
        let mut buf = screen(24, 100);
        draw_web_search(&view, &mut buf, 24, 100, RenderPalette::default());

        assert!(buffer_text(&buf).contains("result text:"));
        assert!(buffer_text(&buf).contains("Novak Djokovic is a Serbian tennis player."));
        assert!(!buffer_text(&buf).contains("fallback summary"));
    }

    #[test]
    fn draw_web_search_falls_back_to_another_useful_result_text() {
        let results = vec![
            app_core::web_search::WebSearchItem {
                title: "Novak".to_string(),
                url: "https://example.com/novak".to_string(),
                snippet: String::new(),
                markdown_link: "[Novak](https://example.com/novak)".to_string(),
            },
            app_core::web_search::WebSearchItem {
                title: "Novak Djokovic".to_string(),
                url: "https://example.com/djokovic".to_string(),
                snippet: "A Serbian professional tennis player.".to_string(),
                markdown_link: "[Novak Djokovic](https://example.com/djokovic)".to_string(),
            },
        ];
        let view = WebSearchView {
            query: "Novak",
            results: &results,
            answer: None,
            summary: None,
            answer_card: None,
            selected: 0,
            pending: false,
            error: None,
        };
        let mut buf = screen(24, 100);
        draw_web_search(&view, &mut buf, 24, 100, RenderPalette::default());

        assert!(buffer_text(&buf).contains("result text:"));
        assert!(buffer_text(&buf).contains("A Serbian professional tennis player."));
    }
}
