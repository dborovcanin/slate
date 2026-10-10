//! Collection browser: collections, their notes and a preview, as in the
//! terminal's `Ctrl+B` browser. Listings come from `app-core` storage.
use crate::note_view::NoteHost;
use crate::overlays::{self, Overlay, PromptKind};
use crate::window::SlateWindow;
use app_core::storage::{NoteAccessMode, NoteSummary};
use gpui::{div, prelude::*, px, AnyElement, Context, FontWeight, KeyDownEvent, SharedString};

const PREVIEW_LINES: usize = 40;
const NOTE_ROWS: usize = 500;

#[derive(Clone, PartialEq)]
pub enum Scope {
    All,
    Unsorted,
    Collection(String),
}

pub struct Entry {
    pub scope: Scope,
    pub name: String,
    pub count: usize,
    pub encrypted: bool,
    pub unlocked: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Collections,
    Notes,
}

/// What the keyboard is doing in the browser.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Browse,
    /// `/`: narrow the note list by title.
    Filter,
    /// `Ctrl+F`: search the text of the notes.
    Search,
}

pub struct Browser {
    pub mode: Mode,
    pub query: String,
    /// The open collection's notes before filtering or searching.
    scope_notes: Vec<NoteSummary>,
    /// Text found by a search, by note.
    pub snippets: std::collections::HashMap<String, String>,
    pub entries: Vec<Entry>,
    pub scope: usize,
    pub notes: Vec<NoteSummary>,
    pub note: usize,
    pub column: Column,
    pub preview: Vec<String>,
    pub preview_note: String,
}

impl Browser {
    pub fn open(host: &NoteHost) -> Result<Self, String> {
        let db = &host.db;
        let counts = db.collection_note_counts()?;
        let mut entries = vec![
            Entry {
                scope: Scope::All,
                name: "All notes".into(),
                count: counts.total,
                encrypted: false,
                unlocked: true,
            },
            Entry {
                scope: Scope::Unsorted,
                name: "Unsorted".into(),
                count: counts.unsorted,
                encrypted: false,
                unlocked: true,
            },
        ];
        for c in db.list_collections()? {
            entries.push(Entry {
                count: counts.per_collection.get(&c.id).copied().unwrap_or(0),
                unlocked: !c.encrypted || db.is_collection_unlocked(&c.id),
                encrypted: c.encrypted,
                name: c.name,
                scope: Scope::Collection(c.id),
            });
        }
        let mut browser = Self {
            mode: Mode::Browse,
            query: String::new(),
            scope_notes: Vec::new(),
            snippets: Default::default(),
            entries,
            scope: 0,
            notes: Vec::new(),
            note: 0,
            column: Column::Notes,
            preview: Vec::new(),
            preview_note: String::new(),
        };
        browser.load_notes(host);
        // Start on the open note.
        if let Some(i) = browser.notes.iter().position(|n| n.id == host.note_id()) {
            browser.note = i;
        }
        browser.refresh_preview(host);
        Ok(browser)
    }

    fn load_notes(&mut self, host: &NoteHost) {
        let entry = &self.entries[self.scope];
        let notes = match &entry.scope {
            Scope::All => host.db.list_notes_meta(),
            Scope::Unsorted => host.db.list_notes_meta_unsorted(),
            Scope::Collection(id) if entry.unlocked => host.db.list_notes_meta_filtered(Some(id)),
            Scope::Collection(_) => Ok(Vec::new()),
        };
        self.scope_notes = notes
            .unwrap_or_default()
            .into_iter()
            .take(NOTE_ROWS)
            .collect();
        self.mode = Mode::Browse;
        self.query.clear();
        self.snippets.clear();
        self.notes = self.scope_notes.clone();
        self.note = 0;
    }

    /// Recompute the note list for the current mode and query.
    fn apply_query(&mut self, host: &NoteHost) {
        self.snippets.clear();
        let query = self.query.trim().to_string();
        self.notes = match self.mode {
            _ if query.is_empty() => self.scope_notes.clone(),
            Mode::Browse => self.scope_notes.clone(),
            Mode::Filter => {
                let mut scored: Vec<_> = self
                    .scope_notes
                    .iter()
                    .filter_map(|n| {
                        let title = if n.title.is_empty() {
                            "Untitled"
                        } else {
                            &n.title
                        };
                        crate::switcher::fuzzy(&query, title).map(|score| (score, n.clone()))
                    })
                    .collect();
                scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
                scored.into_iter().map(|(_, n)| n).collect()
            }
            Mode::Search => {
                let collection = match &self.entries[self.scope].scope {
                    Scope::Collection(id) => Some(id.as_str()),
                    _ => None,
                };
                let hits =
                    crate::sidebar_search::search_in(host, &self.scope_notes, collection, &query);
                let mut notes = Vec::new();
                for hit in hits {
                    if let Ok(Some(meta)) = host.db.get_note_meta(&hit.id) {
                        if let Some(snippet) = hit.snippet {
                            self.snippets.insert(hit.id.clone(), snippet);
                        }
                        notes.push(meta);
                    }
                }
                notes
            }
        };
        self.note = 0;
        self.refresh_preview(host);
    }

    fn refresh_preview(&mut self, host: &NoteHost) {
        self.preview.clear();
        self.preview_note.clear();
        let entry = &self.entries[self.scope];
        if !entry.unlocked {
            self.preview = vec!["This collection is encrypted. Press Enter to unlock it.".into()];
            return;
        }
        let Some(note) = self.notes.get(self.note) else {
            return;
        };
        self.preview_note = note.title.clone();
        if note.access_mode != NoteAccessMode::None && !note.is_unlocked {
            self.preview = vec!["Encrypted note. Press Enter to unlock it.".into()];
            return;
        }
        if note.id == host.note_id() {
            self.preview = host
                .doc
                .lines()
                .iter()
                .take(PREVIEW_LINES)
                .cloned()
                .collect();
            return;
        }
        if let Ok(Some(found)) = host.db.get_note(&note.id) {
            self.preview = found
                .body
                .split('\n')
                .take(PREVIEW_LINES)
                .map(str::to_string)
                .collect();
        }
    }

    fn pick_scope(&mut self, host: &NoteHost, i: usize) {
        self.scope = i.min(self.entries.len() - 1);
        self.load_notes(host);
        self.refresh_preview(host);
    }

    fn pick_note(&mut self, host: &NoteHost, i: usize) {
        self.note = i.min(self.notes.len().saturating_sub(1));
        self.refresh_preview(host);
    }
}

/// Open the highlighted note, or unlock the collection it is in.
fn open_selected(win: &mut SlateWindow, cx: &mut Context<SlateWindow>) {
    let Overlay::Browser(b) = &win.overlay else {
        return;
    };
    let entry = &b.entries[b.scope];
    if !entry.unlocked {
        if let Scope::Collection(id) = &entry.scope {
            let id = id.clone();
            overlays::open_prompt(win, PromptKind::UnlockCollection, &id, cx);
        }
        return;
    }
    let Some(id) = b.notes.get(b.note).map(|n| n.id.clone()) else {
        return;
    };
    win.overlay = Overlay::None;
    win.open_note(&id, cx);
}

/// A new note in the open collection.
fn new_note_here(win: &mut SlateWindow, cx: &mut Context<SlateWindow>) {
    let collection = match &win.overlay {
        Overlay::Browser(b) => match &b.entries[b.scope].scope {
            Scope::Collection(id) => Some(id.clone()),
            _ => None,
        },
        _ => None,
    };
    let id = ulid::Ulid::new().to_string();
    let created = win.host.db.create_note_with_context(
        &id,
        app_core::storage::NoteModules::default(),
        None,
        collection.as_deref(),
    );
    match created {
        Ok(_) => {
            win.host.refresh_notes();
            win.overlay = Overlay::None;
            win.open_note(&id, cx);
        }
        Err(err) => win.set_status(err),
    }
}

pub fn on_key(win: &mut SlateWindow, ev: &KeyDownEvent, cx: &mut Context<SlateWindow>) -> bool {
    let k = &ev.keystroke;
    let key = k.key.as_str();
    let Overlay::Browser(b) = &mut win.overlay else {
        return false;
    };
    let host = &win.host;
    // Typing a filter or a search: keys edit the query, `Up`/`Down` move.
    if b.mode != Mode::Browse && !k.modifiers.control && !k.modifiers.platform {
        match key {
            "escape" => {
                b.mode = Mode::Browse;
                b.query.clear();
                b.apply_query(host);
            }
            "enter" => return open_and_notify(win, cx),
            "down" => b.pick_note(host, b.note + 1),
            "up" => b.pick_note(host, b.note.saturating_sub(1)),
            "backspace" => {
                b.query.pop();
                b.apply_query(host);
            }
            _ => {
                if let Some(ch) = k
                    .key_char
                    .as_deref()
                    .filter(|c| !c.chars().any(char::is_control))
                {
                    b.query.push_str(ch);
                    b.apply_query(host);
                }
            }
        }
        cx.notify();
        return true;
    }
    if (key == "f" || key == "/") && k.modifiers.control {
        b.mode = Mode::Search;
        b.query.clear();
        b.column = Column::Notes;
        cx.notify();
        return true;
    }
    if key == "/" && !k.modifiers.control {
        b.mode = Mode::Filter;
        b.query.clear();
        b.column = Column::Notes;
        cx.notify();
        return true;
    }
    // `Shift+H` or `Ctrl+R` on a note: its history, as in the terminal.
    if wants_history(k) {
        return open_selected_history(win, cx);
    }
    if key == "w" && !k.modifiers.control {
        let working = match &b.entries[b.scope].scope {
            Scope::Collection(id) => Some((id.clone(), b.entries[b.scope].name.clone())),
            _ => None,
        };
        let label = working
            .as_ref()
            .map_or("All notes".to_string(), |(_, n)| n.clone());
        win.host.set_working(working);
        win.set_status(format!("working collection: {label}"));
        cx.notify();
        return true;
    }
    match key {
        "escape" | "q" => win.overlay = Overlay::None,
        "down" | "j" => match b.column {
            Column::Collections => b.pick_scope(host, b.scope + 1),
            Column::Notes => b.pick_note(host, b.note + 1),
        },
        "up" | "k" => match b.column {
            Column::Collections => b.pick_scope(host, b.scope.saturating_sub(1)),
            Column::Notes => b.pick_note(host, b.note.saturating_sub(1)),
        },
        "left" | "h" => match b.column {
            Column::Notes => b.column = Column::Collections,
            Column::Collections => {}
        },
        "right" | "l" => match b.column {
            Column::Collections => b.column = Column::Notes,
            Column::Notes => return open_and_notify(win, cx),
        },
        "enter" => return open_and_notify(win, cx),
        "n" => new_note_here(win, cx),
        "c" => overlays::open_prompt(win, PromptKind::NewCollection, "", cx),
        _ => {}
    }
    cx.notify();
    true
}

/// `Shift+H` or `Ctrl+R`: the history of the highlighted note.
fn wants_history(k: &gpui::Keystroke) -> bool {
    let m = k.modifiers;
    (k.key == "h" && m.shift && !m.control && !m.alt) || (k.key == "r" && m.control && !m.shift)
}

/// History of the highlighted note: it becomes the open note first, since
/// restoring a version replaces the open note's text.
fn open_selected_history(win: &mut SlateWindow, cx: &mut Context<SlateWindow>) -> bool {
    let Overlay::Browser(b) = &win.overlay else {
        return true;
    };
    if !b.entries[b.scope].unlocked {
        return open_and_notify(win, cx);
    }
    let Some(id) = b.notes.get(b.note).map(|n| n.id.clone()) else {
        return true;
    };
    win.overlay = Overlay::None;
    win.open_note(&id, cx);
    overlays::open_history(win, cx);
    true
}

fn open_and_notify(win: &mut SlateWindow, cx: &mut Context<SlateWindow>) -> bool {
    open_selected(win, cx);
    cx.notify();
    true
}

pub fn render(win: &SlateWindow, b: &Browser, cx: &mut Context<SlateWindow>) -> AnyElement {
    let t = win.theme;
    let mono = win.fonts.mono.clone();
    let focus_ring = |on: bool| if on { t.blue } else { t.border };
    let collections = b.entries.iter().enumerate().map(|(i, e)| {
        let on = i == b.scope;
        div()
            .id(SharedString::from(format!("col-{i}")))
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(10.0))
            .py(px(7.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .when(on, |d| d.bg(t.active))
            .when(on && b.column == Column::Collections, |d| {
                d.border_1().border_color(t.blue)
            })
            .hover(|s| s.bg(t.active))
            .on_click(cx.listener(move |this, _, _, cx| {
                let host = &this.host;
                if let Overlay::Browser(b) = &mut this.overlay {
                    b.column = Column::Collections;
                    b.pick_scope(host, i);
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex_1()
                    .text_color(if on { t.heading } else { t.text })
                    .child(format!(
                        "{}{}",
                        if e.encrypted { "🔒 " } else { "" },
                        e.name
                    )),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(t.faint)
                    .child(e.count.to_string()),
            )
    });
    let notes = b.notes.iter().enumerate().map(|(i, n)| {
        let on = i == b.note;
        let locked = n.access_mode != NoteAccessMode::None && !n.is_unlocked;
        div()
            .id(SharedString::from(format!("bn-{i}")))
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(10.0))
            .py(px(7.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .when(on, |d| d.bg(t.active))
            .when(on && b.column == Column::Notes, |d| {
                d.border_1().border_color(t.blue)
            })
            .hover(|s| s.bg(t.active))
            .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| {
                let host = &this.host;
                if let Overlay::Browser(b) = &mut this.overlay {
                    b.column = Column::Notes;
                    b.pick_note(host, i);
                }
                if ev.click_count() >= 2 {
                    open_selected(this, cx);
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(if on { t.heading } else { t.text })
                            .child(format!(
                                "{}{}",
                                if locked { "🔒 " } else { "" },
                                if n.title.is_empty() {
                                    "Untitled"
                                } else {
                                    &n.title
                                }
                            )),
                    )
                    .when_some(b.snippets.get(&n.id).cloned(), |d, snippet| {
                        d.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(t.faint)
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(snippet),
                        )
                    }),
            )
    });
    let hints = [
        ("h j k l", "move"),
        ("Enter", "open"),
        ("/", "filter"),
        ("Ctrl+F", "search text"),
        ("H", "history"),
        ("w", "working collection"),
        ("n", "new note"),
        ("c", "new collection"),
        ("Esc", "close"),
    ];
    div()
        .absolute()
        .occlude()
        .top(px(38.0))
        .bottom(px(28.0))
        .left_0()
        .right_0()
        .flex()
        .flex_col()
        .bg(t.bg)
        .child(
            div()
                .w_full()
                .h_full()
                .flex()
                .flex_col()
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .h(px(44.0))
                        .px(px(16.0))
                        .border_b_1()
                        .border_color(t.border)
                        .text_size(px(12.5))
                        .text_color(t.muted)
                        .child("Collections")
                        .child(div().text_color(t.faint).child("/"))
                        .child(
                            div()
                                .text_color(t.heading)
                                .font_weight(FontWeight::MEDIUM)
                                .child(b.entries[b.scope].name.clone()),
                        )
                        .when(b.mode != Mode::Browse, |d| {
                            d.child(div().flex_1()).child(
                                div()
                                    .w(px(320.0))
                                    .px(px(10.0))
                                    .py(px(4.0))
                                    .rounded(px(6.0))
                                    .bg(t.bg)
                                    .border_1()
                                    .border_color(t.blue)
                                    .text_color(t.text)
                                    .child(format!(
                                        "{} {}▏",
                                        if b.mode == Mode::Filter {
                                            "Filter"
                                        } else {
                                            "Search text"
                                        },
                                        b.query
                                    )),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .child(
                            div()
                                .id("collections")
                                .w(px(230.0))
                                .flex_none()
                                .overflow_y_scroll()
                                .p(px(8.0))
                                .border_r_1()
                                .border_color(focus_ring(b.column == Column::Collections))
                                .children(collections),
                        )
                        .child(
                            div()
                                .id("browser-notes")
                                .w(px(380.0))
                                .flex_none()
                                .overflow_y_scroll()
                                .p(px(8.0))
                                .border_r_1()
                                .border_color(focus_ring(b.column == Column::Notes))
                                .children(notes),
                        )
                        .child(
                            div()
                                .id("browser-preview")
                                .flex_1()
                                .min_w_0()
                                .overflow_y_scroll()
                                .px(px(22.0))
                                .py(px(18.0))
                                .font_family(mono)
                                .text_size(px(13.0))
                                .line_height(px(22.0))
                                .text_color(t.muted)
                                .when(!b.preview_note.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .pb(px(8.0))
                                            .font_family(win.fonts.sans.clone())
                                            .text_size(px(20.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(t.heading)
                                            .child(b.preview_note.clone()),
                                    )
                                })
                                .children(b.preview.iter().map(|l| {
                                    div().whitespace_nowrap().child(if l.is_empty() {
                                        " ".to_string()
                                    } else {
                                        l.clone()
                                    })
                                })),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(18.0))
                        .px(px(16.0))
                        .py(px(9.0))
                        .border_t_1()
                        .border_color(t.border)
                        .text_size(px(11.5))
                        .text_color(t.muted)
                        .children(hints.iter().map(|(k, v)| {
                            div()
                                .flex()
                                .gap(px(5.0))
                                .child(div().text_color(t.text).child(*k))
                                .child(*v)
                        })),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_and_text_search_narrow_the_note_list() {
        let dir = std::env::temp_dir().join(format!("slate-gui-browser-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = app_core::storage::Db::open(dir.join("notes.db")).unwrap();
        for (id, body) in [
            ("a", "# Lisbon trip\nflights"),
            ("b", "# Budget\nthe lisbon hotel costs 300"),
            ("c", "# Garden\nroses"),
        ] {
            db.create_note_with_context(id, Default::default(), None, None)
                .unwrap();
            db.save_note(id, body).unwrap();
        }
        let host = NoteHost::open(db, Some("c")).unwrap();
        let mut b = Browser::open(&host).unwrap();
        let all = b.notes.len();
        assert!(all >= 3);
        b.mode = Mode::Filter;
        b.query = "gard".into();
        b.apply_query(&host);
        assert_eq!(b.notes.len(), 1);
        assert_eq!(b.notes[0].id, "c");
        b.mode = Mode::Search;
        b.query = "lisbon".into();
        b.apply_query(&host);
        let ids: Vec<&str> = b.notes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
        assert!(b.snippets.contains_key("b"));
        b.query.clear();
        b.apply_query(&host);
        assert_eq!(b.notes.len(), all);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn history_keys() {
        for (spec, yes) in [
            ("shift-h", true),
            ("ctrl-r", true),
            ("h", false),
            ("shift-j", false),
            ("ctrl-shift-r", false),
        ] {
            let k = gpui::Keystroke::parse(spec).unwrap();
            assert_eq!(wants_history(&k), yes, "{spec}");
        }
    }
}
