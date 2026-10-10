//! Note history: stored versions of the open note, a preview of what
//! restoring one would change, and restore. Versions, diffs and the
//! "text before restore stays as a version" rule come from `app-core`.
use crate::note_view::NoteHost;
use crate::window::SlateWindow;
use app_core::history::{line_changes, LineChange};
use app_core::storage::NoteVersion;
use editor_core::types::{EditOperation, TextChange};
use gpui::{div, prelude::*, px, AnyElement, Context, FontWeight, KeyDownEvent, SharedString};

/// Lines of unchanged text kept around each change in the preview.
const CONTEXT_LINES: usize = 3;
/// Longest text preview, in lines.
const PREVIEW_MAX_LINES: usize = 400;

pub struct History {
    pub note_id: String,
    pub title: String,
    pub versions: Vec<NoteVersion>,
    /// 0 is the current text; `n` is `versions[n - 1]`.
    pub selected: usize,
    pub show_text: bool,
    pub preview: Preview,
    pub confirm_restore: bool,
}

pub enum Preview {
    Current,
    Changes(Vec<LineChange>),
    Text(Vec<String>),
    Unavailable(String),
}

impl History {
    pub fn open(host: &NoteHost) -> Result<Self, String> {
        if host.locked() {
            return Err("unlock the note to see its history".into());
        }
        let note_id = host.note_id().to_string();
        let versions = host.db.list_note_history(&note_id)?;
        let mut history = Self {
            title: host.title.clone(),
            note_id,
            versions,
            selected: 0,
            show_text: false,
            preview: Preview::Current,
            confirm_restore: false,
        };
        history.refresh(host);
        Ok(history)
    }

    fn version(&self) -> Option<&NoteVersion> {
        self.selected
            .checked_sub(1)
            .and_then(|i| self.versions.get(i))
    }

    fn refresh(&mut self, host: &NoteHost) {
        let Some(version) = self.version() else {
            self.preview = Preview::Current;
            return;
        };
        let text = match host.db.note_version_text(&self.note_id, version.id) {
            Ok(text) => text,
            Err(err) => {
                self.preview = Preview::Unavailable(err);
                return;
            }
        };
        self.preview = if self.show_text {
            Preview::Text(
                text.split('\n')
                    .take(PREVIEW_MAX_LINES)
                    .map(str::to_string)
                    .collect(),
            )
        } else {
            let current = host.doc.lines().join("\n");
            Preview::Changes(line_changes(&current, &text, CONTEXT_LINES))
        };
    }

    fn step(&mut self, host: &NoteHost, delta: isize) {
        let max = self.versions.len();
        self.selected = (self.selected as isize + delta).clamp(0, max as isize) as usize;
        self.confirm_restore = false;
        self.refresh(host);
    }
}

/// Replace the whole note text with a stored version, as one undoable edit.
/// The text before is kept as a version of its own.
fn restore(win: &mut SlateWindow, version_id: i64, cx: &mut Context<SlateWindow>) {
    let note_id = win.host.note_id().to_string();
    let text = match win.host.db.note_version_text(&note_id, version_id) {
        Ok(text) => text,
        Err(err) => return win.set_status(format!("restore failed: {err}")),
    };
    if let Err(err) = win.host.save() {
        return win.set_status(format!("save failed: {err}"));
    }
    if let Err(err) = win.host.db.end_history_session(&note_id) {
        return win.set_status(format!("restore failed: {err}"));
    }
    let len = win.host.doc.lines().join("\n").len();
    let op = EditOperation {
        changes: vec![TextChange {
            from: 0,
            to: len,
            insert: text,
        }],
        selection: None,
    };
    let before = win.snapshot_cursor();
    let outcome = win.host.apply_operation(&op);
    win.after_input(before, outcome, cx);
    win.save(cx);
    win.set_status("restored · the text before is now a version");
}

pub fn on_key(win: &mut SlateWindow, ev: &KeyDownEvent, cx: &mut Context<SlateWindow>) -> bool {
    let k = &ev.keystroke;
    let key = k.key.as_str();
    let ch = k.key_char.as_deref().unwrap_or("");
    let crate::overlays::Overlay::History(h) = &mut win.overlay else {
        return false;
    };
    let host = &win.host;
    match (key, ch) {
        ("escape", _) | ("q", _) | ("h", _) | ("left", _) | ("-", _) => {
            if h.confirm_restore {
                h.confirm_restore = false;
            } else {
                win.overlay = crate::overlays::Overlay::None;
            }
        }
        ("down", _) | ("j", _) => h.step(host, 1),
        ("up", _) | ("k", _) => h.step(host, -1),
        ("g", _) if !k.modifiers.shift => h.step(host, -(h.selected as isize)),
        ("g", _) => h.step(host, h.versions.len() as isize),
        ("tab", _) | ("t", _) => {
            h.show_text = !h.show_text;
            h.refresh(host);
        }
        ("enter", _) | ("l", _) | ("right", _) => match h.version().map(|v| v.id) {
            None => win.overlay = crate::overlays::Overlay::None,
            Some(id) if h.confirm_restore => {
                win.overlay = crate::overlays::Overlay::None;
                restore(win, id, cx);
            }
            Some(_) => h.confirm_restore = true,
        },
        ("y", _) if h.confirm_restore => {
            if let Some(id) = h.version().map(|v| v.id) {
                win.overlay = crate::overlays::Overlay::None;
                restore(win, id, cx);
            }
        }
        _ => {}
    }
    cx.notify();
    true
}

/// `2026-10-10T12:41:00Z` as `10.10.2026. 12:41`.
fn label(saved_at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(saved_at)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%d.%m.%Y. %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| saved_at.to_string())
}

fn age(saved_at: &str) -> String {
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(saved_at) else {
        return String::new();
    };
    let secs = (chrono::Utc::now() - t.with_timezone(&chrono::Utc))
        .num_seconds()
        .max(0);
    match secs {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

pub fn render(win: &SlateWindow, h: &History, cx: &mut Context<SlateWindow>) -> AnyElement {
    let t = win.theme;
    let mono = win.fonts.mono.clone();
    let rows = std::iter::once((
        "Current".to_string(),
        String::new(),
        String::new(),
        "now".to_string(),
    ))
    .chain(h.versions.iter().map(|v| {
        (
            label(&v.saved_at),
            format!("+{}", v.lines_added),
            format!("−{}", v.lines_removed),
            age(&v.saved_at),
        )
    }))
    .enumerate()
    .map(|(i, (name, add, del, ago))| {
        let on = i == h.selected;
        div()
            .id(SharedString::from(format!("ver-{i}")))
            .flex()
            .items_center()
            .gap(px(10.0))
            .h(px(32.0))
            .px(px(10.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .when(on, |d| d.bg(t.active).border_1().border_color(t.blue))
            .hover(|s| s.bg(t.active))
            .on_click(cx.listener(move |this, _, _, cx| {
                let host = &this.host;
                if let crate::overlays::Overlay::History(h) = &mut this.overlay {
                    h.step(host, i as isize - h.selected as isize);
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex_1()
                    .font_family(mono.clone())
                    .text_size(px(12.5))
                    .when(on, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .text_color(if on { t.heading } else { t.text })
                    .child(name),
            )
            .child(
                div()
                    .w(px(34.0))
                    .font_family(mono.clone())
                    .text_size(px(12.0))
                    .text_color(t.blue)
                    .child(add),
            )
            .child(
                div()
                    .w(px(34.0))
                    .font_family(mono.clone())
                    .text_size(px(12.0))
                    .text_color(t.amber)
                    .child(del),
            )
            .child(
                div()
                    .w(px(30.0))
                    .text_size(px(11.5))
                    .text_color(t.faint)
                    .child(ago),
            )
    });

    let (heading, sub) = match h.version() {
        None => (
            "Current version".to_string(),
            format!("{} · {} lines", h.title, win.host.doc.lines().len()),
        ),
        Some(v) => (
            label(&v.saved_at),
            if h.confirm_restore {
                "Press Enter or y again to replace the current text".to_string()
            } else {
                "Restoring replaces the current text with this version".to_string()
            },
        ),
    };
    let tab = |name: &'static str, on: bool, text: bool, cx: &mut Context<SlateWindow>| {
        div()
            .id(name)
            .px(px(12.0))
            .py(px(4.0))
            .rounded(px(5.0))
            .text_size(px(12.5))
            .cursor_pointer()
            .when(on, |d| d.bg(t.active).text_color(t.heading))
            .when(!on, |d| d.text_color(t.muted))
            .on_click(cx.listener(move |this, _, _, cx| {
                let host = &this.host;
                if let crate::overlays::Overlay::History(h) = &mut this.overlay {
                    h.show_text = text;
                    h.refresh(host);
                }
                cx.notify();
            }))
            .child(name)
    };
    let preview = match &h.preview {
        Preview::Current => vec![div()
            .px(px(18.0))
            .py(px(24.0))
            .text_color(t.muted)
            .child("This is the note as it is now. Pick an older version to see what restoring it would change.")
            .into_any_element()],
        Preview::Unavailable(err) => vec![div().p(px(18.0)).text_color(t.amber).child(err.clone()).into_any_element()],
        Preview::Text(lines) => lines
            .iter()
            .map(|l| diff_row("", l, t.text, None, &t, &mono))
            .collect(),
        Preview::Changes(changes) if changes.is_empty() => vec![div()
            .px(px(18.0))
            .py(px(24.0))
            .text_color(t.muted)
            .child("Same as the current text.")
            .into_any_element()],
        Preview::Changes(changes) => changes
            .iter()
            .map(|c| match c {
                LineChange::Same(l) => diff_row("", l, t.muted, None, &t, &mono),
                LineChange::Added(l) => diff_row("+", l, t.text, Some(t.blue.opacity(0.14)), &t, &mono),
                LineChange::Removed(l) => diff_row("−", l, t.text, Some(t.amber.opacity(0.14)), &t, &mono),
                LineChange::Skipped(n) => diff_row("⋯", &format!("{n} unchanged lines"), t.faint, None, &t, &mono),
            })
            .collect(),
    };

    div()
        .absolute()
        .occlude()
        .top(px(38.0))
        .bottom(px(28.0))
        .left_0()
        .right_0()
        .bg(t.bg)
        .flex()
        .child(
            div()
                .w(px(380.0))
                .flex_none()
                .h_full()
                .p(px(10.0))
                .bg(t.panel)
                .border_r_1()
                .border_color(t.border)
                .flex()
                .flex_col()
                .gap(px(1.0))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .px(px(8.0))
                        .pb(px(8.0))
                        .text_size(px(11.0))
                        .text_color(t.muted)
                        .child("VERSIONS")
                        .child("lines added / removed"),
                )
                .child(div().id("versions").flex_1().overflow_y_scroll().children(rows)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(12.0))
                        .px(px(18.0))
                        .py(px(12.0))
                        .border_b_1()
                        .border_color(t.border)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(200.0))
                                .child(div().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(t.heading).child(heading))
                                .child(div().text_size(px(12.0)).text_color(t.muted).child(sub)),
                        )
                        .child(
                            div()
                                .flex()
                                .p(px(2.0))
                                .rounded(px(7.0))
                                .bg(t.panel)
                                .border_1()
                                .border_color(t.border)
                                .child(tab("Changes", !h.show_text, false, cx))
                                .child(tab("Text", h.show_text, true, cx)),
                        )
                        .when(h.version().is_some(), |d| {
                            let id = h.version().map(|v| v.id);
                            d.child(
                                div()
                                    .id("restore")
                                    .h(px(30.0))
                                    .px(px(14.0))
                                    .flex()
                                    .items_center()
                                    .rounded(px(6.0))
                                    .cursor_pointer()
                                    .bg(if h.confirm_restore { t.amber } else { t.blue })
                                    .text_color(t.on_accent)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(px(12.5))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let Some(id) = id else { return };
                                        let confirmed = matches!(
                                            &this.overlay,
                                            crate::overlays::Overlay::History(h) if h.confirm_restore
                                        );
                                        if confirmed {
                                            this.overlay = crate::overlays::Overlay::None;
                                            restore(this, id, cx);
                                        } else if let crate::overlays::Overlay::History(h) = &mut this.overlay {
                                            h.confirm_restore = true;
                                        }
                                        cx.notify();
                                    }))
                                    .child(if h.confirm_restore { "Confirm restore" } else { "Restore this version" }),
                            )
                        }),
                )
                .child(
                    div()
                        .id("preview")
                        .flex_1()
                        .overflow_y_scroll()
                        .py(px(10.0))
                        .font_family(mono.clone())
                        .text_size(px(13.0))
                        .line_height(px(22.0))
                        .children(preview),
                ),
        )
        .into_any_element()
}

fn diff_row(
    sign: &str,
    text: &str,
    color: gpui::Hsla,
    bg: Option<gpui::Hsla>,
    t: &crate::theme::Theme,
    mono: &SharedString,
) -> AnyElement {
    let sign_color = match sign {
        "+" => t.blue,
        "−" => t.amber,
        _ => t.faint,
    };
    div()
        .flex()
        .font_family(mono.clone())
        .when_some(bg, |d, bg| d.bg(bg))
        .text_color(color)
        .child(
            div()
                .w(px(34.0))
                .flex_none()
                .flex()
                .justify_center()
                .text_color(sign_color)
                .child(sign.to_string()),
        )
        .child(
            div()
                .min_w_0()
                .whitespace_nowrap()
                .pr(px(18.0))
                .child(if text.is_empty() {
                    " ".to_string()
                } else {
                    text.to_string()
                }),
        )
        .into_any_element()
}
