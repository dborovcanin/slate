//! Popup pickers drawn with the collection browser's list style: the note
//! switcher (`Ctrl+P`), its full-text search and the collection picker
//! (`Ctrl+G`). A query bar tops the list and key hints sit at the bottom.
//! State and keys live in the app.

use std::cmp::min;

use ratatui::buffer::Buffer;
use ratatui::style::Modifier;

use super::browser::{
    draw_centered_hint, draw_query_bar, draw_rows, query_bar_cursor, Hover, Look, Pane, Row,
};
use super::canvas::{cell_style, draw_framed_surface, draw_key_hints, put_char};

pub struct PickerView<'a, F: Fn(usize) -> Row> {
    pub look: Look<'a>,
    /// Frame title, e.g. `Notes · in Work`.
    pub title: &'a str,
    /// Dim label in the bottom border, e.g. `3/29`.
    pub footer: String,
    pub query: &'a str,
    /// Char index of the query cursor; `usize::MAX` means at the end.
    pub cursor: usize,
    pub placeholder: &'a str,
    pub len: usize,
    /// Highlighted row, if any.
    pub selected: Option<usize>,
    pub row_at: F,
    /// Shown in place of an empty list.
    pub empty_hint: &'a str,
    pub hints: &'a [(&'a str, &'a str)],
}

/// Top-left cell, width and height of the popup on a `rows` x `cols` screen.
fn popup_rect(rows: usize, cols: usize) -> (usize, usize, usize, usize) {
    let width = min(cols.saturating_sub(4), 76).max(min(cols, 30));
    let height = min(rows.saturating_sub(4), 20).max(min(rows, 8));
    let row = rows.saturating_sub(height) / 2 + 1;
    let col = cols.saturating_sub(width) / 2 + 1;
    (row, col, width, height)
}

/// The popup's inside, between the side borders.
fn inner_pane(col: usize, width: usize) -> Pane {
    Pane {
        col: col + 1,
        width: width.saturating_sub(2),
    }
}

pub fn draw_picker<F: Fn(usize) -> Row>(
    view: &PickerView<F>,
    buf: &mut Buffer,
    rows: usize,
    cols: usize,
) {
    let look = &view.look;
    let palette = look.palette;
    let (top, col, width, height) = popup_rect(rows, cols);
    draw_framed_surface(
        buf,
        top,
        col,
        width,
        height,
        palette.surface_bg(),
        palette.primary(),
        false,
        Some(view.title),
        Some(&view.footer),
    );
    if height < 6 {
        return;
    }
    let pane = inner_pane(col, width);
    draw_query_bar(
        look,
        buf,
        pane,
        top + 1,
        view.query,
        view.cursor,
        view.placeholder,
    );
    // Join the rule under the query bar to the frame.
    let rule = cell_style(
        Some(palette.primary()),
        Some(palette.surface_bg()),
        Modifier::empty(),
    );
    put_char(buf, top + 2, col, '├', rule);
    put_char(buf, top + 2, col + width - 1, '┤', rule);
    let list_top = top + 3;
    let list_height = height - 5;
    draw_rows(
        buf,
        palette,
        pane,
        list_top,
        list_height,
        view.len,
        &view.row_at,
        view.selected.map(|pos| (pos, Hover::Focused)),
    );
    if view.len == 0 {
        draw_centered_hint(buf, palette, pane, list_top + 1, view.empty_hint);
    }
    let bg = palette.surface_bg();
    draw_key_hints(
        buf,
        top + height - 2,
        pane.col + 1,
        pane.width.saturating_sub(2),
        view.hints,
        cell_style(Some(palette.primary()), Some(bg), Modifier::BOLD),
        cell_style(Some(palette.code_comment), Some(bg), Modifier::empty()),
    );
}

/// Screen cell of the query cursor of a picker on a `rows` x `cols` screen.
pub fn picker_query_cursor(rows: usize, cols: usize, query: &str, cursor: usize) -> (usize, usize) {
    let (top, col, width, _) = popup_rect(rows, cols);
    query_bar_cursor(inner_pane(col, width), top + 1, query, cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_is_centered_and_fits_small_screens() {
        let (row, col, width, height) = popup_rect(24, 80);
        assert_eq!((width, height), (76, 20));
        assert_eq!((row, col), (3, 3));
        let (row, col, width, height) = popup_rect(10, 30);
        assert!(row + height - 1 <= 10 && col + width - 1 <= 30);
    }
}
