//! The main window: title bar, notes sidebar, editor and status bar.
//!
//! Painting only. Line content, styles and calc values come from
//! `NoteHost::lines`; this file lays them out.
use crate::note_view::{LineKind, LineView, NoteHost, Run, TableCell};
use crate::theme::Theme;
use gpui::{
    div, list, prelude::*, px, AnyElement, Context, FontWeight, ListAlignment, ListState,
    MouseButton, SharedString, StyledText, Window,
};
use std::ops::Range;

const MENUS: [&str; 6] = ["File", "Edit", "View", "Format", "Calc", "Help"];
/// Sidebar entries; the rest of the notes are one search away once it exists.
const SIDEBAR_NOTES: usize = 200;

pub struct Fonts {
    pub sans: SharedString,
    pub mono: SharedString,
}

pub struct SlateWindow {
    host: NoteHost,
    theme: Theme,
    fonts: Fonts,
    lines: Vec<LineView>,
    list: ListState,
}

impl SlateWindow {
    pub fn new(host: NoteHost, theme: Theme, fonts: Fonts) -> Self {
        let count = host.doc.lines().len();
        let lines = host.lines(0, count);
        Self {
            host,
            theme,
            fonts,
            lines,
            list: ListState::new(count, ListAlignment::Top, px(600.0)),
        }
    }

    fn reload_lines(&mut self) {
        let count = self.host.doc.lines().len();
        self.lines = self.host.lines(0, count);
        self.list.reset(count);
    }

    fn open_note(&mut self, id: &str, cx: &mut Context<Self>) {
        if id == self.host.note_id() {
            return;
        }
        if let Err(err) = self.host.switch_to(id) {
            eprintln!("slate-gui: {err}");
            return;
        }
        self.reload_lines();
        cx.notify();
    }

    /// Restyle only the lines whose cursor state changed.
    fn move_cursor_to(&mut self, line: usize, cx: &mut Context<Self>) {
        let old = self.host.doc.cursor_line;
        self.host.set_cursor_line(line);
        let new = self.host.doc.cursor_line;
        if old == new {
            return;
        }
        for ix in [old, new] {
            if let (Some(slot), Some(view)) =
                (self.lines.get_mut(ix), self.host.lines(ix, ix + 1).pop())
            {
                *slot = view;
            }
        }
        cx.notify();
    }

    fn title_bar(&self) -> impl IntoElement {
        let t = &self.theme;
        div()
            .h(px(38.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(14.0))
            .bg(t.panel)
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .size(px(14.0))
                    .rounded(px(3.0))
                    .border_2()
                    .border_color(t.blue),
            )
            .child(
                div()
                    .flex()
                    .gap(px(2.0))
                    .children(MENUS.iter().map(|label| {
                        div()
                            .px(px(9.0))
                            .py(px(4.0))
                            .rounded(px(5.0))
                            .text_size(px(12.5))
                            .text_color(t.muted)
                            .hover(|s| s.bg(t.active))
                            .child(*label)
                    })),
            )
            .child(div().w(px(1.0)).h(px(16.0)).bg(t.border))
            .child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .text_size(px(12.5))
                    .text_color(t.muted)
                    .child("Notes")
                    .child(div().text_color(t.faint).child("/"))
                    .child(
                        div()
                            .text_color(t.text)
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.host.title.clone()),
                    ),
            )
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let current = self.host.note_id().to_string();
        let items = self.host.notes.iter().take(SIDEBAR_NOTES).map(|note| {
            let active = note.id == current;
            let id = note.id.clone();
            div()
                .id(SharedString::from(format!("note-{}", note.id)))
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .when(active, |d| d.bg(t.active))
                .hover(|s| s.bg(t.active))
                .on_click(cx.listener(move |this, _, _, cx| this.open_note(&id, cx)))
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(if active { t.heading } else { t.text })
                        .when(active, |d| d.font_weight(FontWeight::SEMIBOLD))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(if note.title.is_empty() {
                            "Untitled".to_string()
                        } else {
                            note.title.clone()
                        }),
                )
        });
        div()
            .id("sidebar")
            .w(px(248.0))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .bg(t.panel)
            .border_r_1()
            .border_color(t.border)
            .p(px(10.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .px(px(8.0))
                    .pb(px(8.0))
                    .text_size(px(11.0))
                    .text_color(t.muted)
                    .child("NOTES")
                    .child(
                        div()
                            .text_color(t.faint)
                            .child(format!("{}", self.host.notes.len())),
                    ),
            )
            .children(items)
    }

    fn status_bar(&self) -> impl IntoElement {
        let t = &self.theme;
        let m = self.host.modules;
        let line = self.host.doc.cursor_line;
        let ghost = self
            .lines
            .get(line)
            .and_then(|l| l.ghost.as_deref())
            .map(|g| g.trim().trim_start_matches(['=', '→']).trim().to_string());
        let chip = |label: &'static str| div().px(px(7.0)).rounded(px(9.0)).bg(t.chip).child(label);
        div()
            .h(px(28.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(6.0))
            .bg(t.panel)
            .border_t_1()
            .border_color(t.border)
            .text_size(px(11.5))
            .text_color(t.muted)
            .child(
                div()
                    .px(px(9.0))
                    .rounded(px(4.0))
                    .bg(t.blue)
                    .text_color(t.on_accent)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("NORMAL"),
            )
            .child(
                div()
                    .px(px(6.0))
                    .text_color(t.text)
                    .child(self.host.title.clone())
                    .when(self.host.session.dirty(), |d| {
                        d.child(div().text_color(t.amber).child(" ●"))
                    }),
            )
            .when(m.math, |d| d.child(chip("math")))
            .when(m.variables, |d| d.child(chip("variables")))
            .when(m.table, |d| d.child(chip("table")))
            .child(div().flex_1())
            .when_some(ghost, |d, g| {
                d.child(
                    div()
                        .px(px(10.0))
                        .font_family(self.fonts.mono.clone())
                        .text_color(t.amber)
                        .child(format!("= {g}")),
                )
            })
            .child(
                div()
                    .px(px(10.0))
                    .border_l_1()
                    .border_color(t.border)
                    .font_family(self.fonts.mono.clone())
                    .child(format!("{}:{}", line + 1, self.host.doc.cursor_col + 1)),
            )
    }

    fn render_line(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(line) = self.lines.get(ix) else {
            return div().into_any_element();
        };
        let t = self.theme;
        let cursor = self.host.doc.cursor_line;
        let is_cursor = ix == cursor;
        let number = if is_cursor {
            ix + 1
        } else {
            ix.abs_diff(cursor)
        };
        let gutter = div()
            .w(px(38.0))
            .flex_none()
            .pr(px(12.0))
            .flex()
            .justify_end()
            .text_size(px(12.0))
            .text_color(if is_cursor { t.text } else { t.faint })
            .child(number.to_string());
        let body: AnyElement = match &line.kind {
            LineKind::Heading(level) => {
                let size = match level {
                    1 => 26.0,
                    2 => 18.0,
                    _ => 16.0,
                };
                div()
                    .pt(px(if *level <= 2 { 8.0 } else { 4.0 }))
                    .font_family(self.fonts.sans.clone())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(size))
                    .line_height(px(size * 1.4))
                    .text_color(t.heading)
                    .child(styled_text(&line.runs, &t))
                    .into_any_element()
            }
            LineKind::Checklist { checked } => div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(checkbox(*checked, &t))
                .child(
                    div()
                        .when(*checked, |d| d.text_color(t.faint).line_through())
                        .child(styled_text(&line.runs, &t)),
                )
                .when_some(line.ghost.clone(), |d, g| d.child(ghost(g, &t)))
                .into_any_element(),
            LineKind::TableRow { cells, header } => table_row(cells, *header, &t),
            // The header row already draws the rule under it.
            LineKind::TableDelimiter => div().into_any_element(),
            LineKind::Text => div()
                .flex()
                .child(styled_text(&line.runs, &t))
                .when_some(line.ghost.clone(), |d, g| d.child(ghost(g, &t)))
                .into_any_element(),
        };
        div()
            .id(("line", ix))
            .flex()
            .items_center()
            .w_full()
            .max_w(px(900.0))
            .map(|d| match line.kind {
                LineKind::TableDelimiter => d.h(px(0.0)).overflow_hidden(),
                _ => d.min_h(px(26.0)),
            })
            .when(is_cursor, |d| d.bg(t.cursorline))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| this.move_cursor_to(ix, cx)),
            )
            .child(gutter)
            .child(div().flex_1().min_w_0().child(body))
            .into_any_element()
    }
}

fn styled_text(runs: &[Run], theme: &Theme) -> StyledText {
    let mut text = String::new();
    let mut highlights: Vec<(Range<usize>, gpui::HighlightStyle)> = Vec::new();
    for run in runs {
        let start = text.len();
        text.push_str(&run.text);
        highlights.push((start..text.len(), theme.highlight(run.style)));
    }
    StyledText::new(text).with_highlights(highlights)
}

fn ghost(text: String, t: &Theme) -> impl IntoElement {
    div()
        .pl(px(24.0))
        .text_color(t.amber)
        .child(text.trim().to_string())
}

fn checkbox(checked: bool, t: &Theme) -> impl IntoElement {
    let b = div().size(px(13.0)).rounded(px(3.0)).flex_none();
    if checked {
        b.bg(t.blue)
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(10.0))
            .text_color(t.bg)
            .child("✓")
    } else {
        b.border_1().border_color(t.muted)
    }
}

fn table_row(cells: &[TableCell], header: bool, t: &Theme) -> AnyElement {
    div()
        .w_full()
        .flex()
        .border_b_1()
        .border_color(t.border)
        .when(header, |d| {
            d.text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD)
        })
        .children(cells.iter().enumerate().map(|(i, cell)| {
            let color = if header {
                t.muted
            } else if cell.formula {
                t.amber
            } else {
                t.text
            };
            div()
                .flex_1()
                .min_w_0()
                .px(px(12.0))
                .py(px(3.0))
                .text_color(color)
                .when(i > 0, |d| d.flex().justify_end())
                .overflow_hidden()
                .whitespace_nowrap()
                .child(cell.text.clone())
        }))
        .into_any_element()
}

impl Render for SlateWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let editor = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .pt(px(20.0))
            .font_family(self.fonts.mono.clone())
            .text_size(px(14.0))
            .line_height(px(26.0))
            .text_color(t.text)
            .child(
                list(
                    self.list.clone(),
                    cx.processor(|this, ix, _window, cx| this.render_line(ix, cx)),
                )
                .size_full(),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.bg)
            .text_color(t.text)
            .font_family(self.fonts.sans.clone())
            .text_size(px(13.0))
            .child(self.title_bar())
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.sidebar(cx))
                    .child(editor),
            )
            .child(self.status_bar())
    }
}
