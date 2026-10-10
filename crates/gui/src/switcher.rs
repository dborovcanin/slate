//! The note switcher (`Ctrl+P`) and the collection picker (`Ctrl+G`), with
//! the same keys as the terminal app.
use crate::note_view::NoteHost;
use crate::overlays::{self, Overlay};
use crate::window::SlateWindow;
use app_core::storage::{NoteAccessMode, NoteSummary};
use gpui::{div, prelude::*, px, AnyElement, Context, KeyDownEvent, SharedString};

const MAX_ROWS: usize = 12;

/// Score of `query` as a case-insensitive subsequence of `text`, higher for
/// earlier and tighter matches; `None` when it does not match.
pub fn fuzzy(query: &str, text: &str) -> Option<i64> {
    let query: Vec<char> = query.to_lowercase().chars().collect();
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0i64;
    let (mut qi, mut last) = (0, None::<usize>);
    for (ti, ch) in text.iter().enumerate() {
        if qi < query.len() && *ch == query[qi] {
            score += 10;
            if last == Some(ti.wrapping_sub(1)) {
                score += 15; // consecutive
            }
            if ti == 0 || !text[ti - 1].is_alphanumeric() {
                score += 8; // start of a word
            }
            score -= (ti as i64).min(20);
            last = Some(ti);
            qi += 1;
        }
    }
    (qi == query.len()).then_some(score)
}

pub struct Switcher {
    pub query: String,
    pub selected: usize,
    /// Every note, or only the working collection's.
    pub all: bool,
    items: Vec<NoteSummary>,
    /// `Tab`: search the text of the notes instead of their titles.
    pub text: bool,
    found: Vec<NoteSummary>,
    snippets: std::collections::HashMap<String, String>,
}

impl Switcher {
    pub fn open(host: &NoteHost) -> Self {
        let mut s = Self {
            query: String::new(),
            selected: 0,
            all: host.working.is_none(),
            items: Vec::new(),
            text: false,
            found: Vec::new(),
            snippets: Default::default(),
        };
        s.load(host);
        s
    }

    fn load(&mut self, host: &NoteHost) {
        self.items = if self.all || host.working.is_none() {
            host.db.list_notes_meta().unwrap_or_default()
        } else {
            host.notes.clone()
        };
        self.selected = 0;
    }

    /// Run the full-text search for the query (text mode).
    fn search_text(&mut self, host: &NoteHost) {
        self.found.clear();
        self.snippets.clear();
        let collection = if self.all {
            None
        } else {
            host.working.as_ref().map(|(id, _)| id.as_str())
        };
        let Ok(results) = host
            .db
            .search_notes_content_filtered(self.query.trim(), 50, collection)
        else {
            return;
        };
        for r in results {
            // One row per note, at its first matching line.
            if self.found.iter().any(|n| n.id == r.id) {
                continue;
            }
            if let Ok(Some(meta)) = host.db.get_note_meta(&r.id) {
                if !r.snippet.trim().is_empty() {
                    self.snippets
                        .insert(r.id.clone(), r.snippet.trim().to_string());
                }
                self.found.push(meta);
            }
        }
    }

    /// The query changed: refresh what depends on it.
    fn query_changed(&mut self, host: &NoteHost) {
        self.selected = 0;
        if self.text {
            self.search_text(host);
        }
    }

    /// Notes matching the query, best first (recent first when empty).
    pub fn matches(&self) -> Vec<&NoteSummary> {
        if self.text {
            return self.found.iter().collect();
        }
        let mut scored: Vec<(i64, usize, &NoteSummary)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, n)| fuzzy(&self.query, display_title(n)).map(|s| (s, i, n)))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.into_iter().map(|(_, _, n)| n).collect()
    }
}

fn display_title(n: &NoteSummary) -> &str {
    if n.title.is_empty() {
        "Untitled"
    } else {
        &n.title
    }
}

fn locked(n: &NoteSummary) -> bool {
    n.access_mode != NoteAccessMode::None && !n.is_unlocked
}

pub fn open_switcher(win: &mut SlateWindow, cx: &mut Context<SlateWindow>) {
    win.overlay = Overlay::Switcher(Switcher::open(&win.host));
    cx.notify();
}

fn open_selected(win: &mut SlateWindow, cx: &mut Context<SlateWindow>) {
    let Overlay::Switcher(s) = &win.overlay else {
        return;
    };
    let Some(id) = s.matches().get(s.selected).map(|n| n.id.clone()) else {
        return;
    };
    win.overlay = Overlay::None;
    win.open_note(&id, cx);
}

pub fn on_switcher_key(
    win: &mut SlateWindow,
    ev: &KeyDownEvent,
    cx: &mut Context<SlateWindow>,
) -> bool {
    let k = &ev.keystroke;
    let ctrl = k.modifiers.control || k.modifiers.platform;
    let typed = k.key_char.clone().filter(|s| !s.is_empty() && !ctrl);
    let Overlay::Switcher(s) = &mut win.overlay else {
        return false;
    };
    let host = &win.host;
    let count = s.matches().len();
    match (k.key.as_str(), ctrl) {
        ("escape", _) | ("p", true) => win.overlay = Overlay::None,
        ("up", _) => s.selected = s.selected.saturating_sub(1),
        ("down", _) => s.selected = (s.selected + 1).min(count.saturating_sub(1)),
        ("enter", _) => {
            open_selected(win, cx);
            return true;
        }
        // Tab: title search and full-text search take turns.
        ("tab", false) => {
            s.text = !s.text;
            s.query_changed(host);
        }
        ("backspace", false) => {
            s.query.pop();
            s.query_changed(host);
        }
        // Ctrl+W / Ctrl+Backspace: delete the last word of the query.
        ("w", true) | ("backspace", true) => {
            let trimmed = s.query.trim_end().len();
            let cut = s.query[..trimmed].rfind(' ').map_or(0, |i| i + 1);
            s.query.truncate(cut);
            s.query_changed(host);
        }
        ("l", true) => {
            s.all = !s.all;
            s.load(host);
            s.query_changed(host);
        }
        ("n", true) => {
            win.overlay = Overlay::None;
            crate::commands::new_note(win, cx);
            return true;
        }
        ("g", true) => {
            open_picker(win, cx);
            return true;
        }
        ("r", true) => {
            // History of the selected note: open it, then its history.
            let id = s.matches().get(s.selected).map(|n| n.id.clone());
            if let Some(id) = id {
                win.overlay = Overlay::None;
                win.open_note(&id, cx);
                overlays::open_history(win, cx);
            }
            return true;
        }
        _ => {
            if let Some(text) = typed {
                s.query.push_str(&text);
                s.query_changed(host);
            }
        }
    }
    cx.notify();
    true
}

pub struct Picker {
    pub query: String,
    pub selected: usize,
    /// `(id, name, note count)`; `None` is "All notes".
    entries: Vec<(Option<String>, String, usize)>,
}

impl Picker {
    pub fn open(host: &NoteHost) -> Result<Self, String> {
        let counts = host.db.collection_note_counts()?;
        let mut entries = vec![(None, "All notes".to_string(), counts.total)];
        for c in host.db.list_collections()? {
            let n = counts.per_collection.get(&c.id).copied().unwrap_or(0);
            entries.push((Some(c.id), c.name, n));
        }
        let selected = host
            .working
            .as_ref()
            .and_then(|(id, _)| entries.iter().position(|e| e.0.as_deref() == Some(id)))
            .unwrap_or(0);
        Ok(Self {
            query: String::new(),
            selected,
            entries,
        })
    }

    fn visible(&self) -> Vec<&(Option<String>, String, usize)> {
        self.entries
            .iter()
            .filter(|e| fuzzy(&self.query, &e.1).is_some())
            .collect()
    }
}

pub fn open_picker(win: &mut SlateWindow, cx: &mut Context<SlateWindow>) {
    match Picker::open(&win.host) {
        Ok(p) => win.overlay = Overlay::Picker(p),
        Err(err) => win.set_status(err),
    }
    cx.notify();
}

fn choose(win: &mut SlateWindow, cx: &mut Context<SlateWindow>) {
    let Overlay::Picker(p) = &win.overlay else {
        return;
    };
    let Some((id, name, _)) = p.visible().get(p.selected).map(|e| (*e).clone()) else {
        return;
    };
    win.overlay = Overlay::None;
    let message = match &id {
        Some(_) => format!("working collection: {name}"),
        None => "working collection cleared".to_string(),
    };
    win.host.set_working(id.map(|id| (id, name)));
    win.set_status(message);
    cx.notify();
}

pub fn on_picker_key(
    win: &mut SlateWindow,
    ev: &KeyDownEvent,
    cx: &mut Context<SlateWindow>,
) -> bool {
    let k = &ev.keystroke;
    let ctrl = k.modifiers.control || k.modifiers.platform;
    let typed = k.key_char.clone().filter(|s| !s.is_empty() && !ctrl);
    let Overlay::Picker(p) = &mut win.overlay else {
        return false;
    };
    let count = p.visible().len();
    match (k.key.as_str(), ctrl) {
        ("escape", _) | ("g", true) | ("p", true) => win.overlay = Overlay::None,
        ("up", _) => p.selected = p.selected.saturating_sub(1),
        ("down", _) => p.selected = (p.selected + 1).min(count.saturating_sub(1)),
        ("enter", _) => {
            choose(win, cx);
            return true;
        }
        ("backspace", _) => {
            p.query.pop();
            p.selected = 0;
        }
        _ => {
            if let Some(text) = typed {
                p.query.push_str(&text);
                p.selected = 0;
            }
        }
    }
    cx.notify();
    true
}

/// One popup row: main text, dim detail on the right.
struct Row {
    id: SharedString,
    label: String,
    detail: String,
    locked: bool,
}

#[allow(clippy::too_many_arguments)]
fn popup(
    win: &SlateWindow,
    title: &str,
    query: &str,
    rows: Vec<Row>,
    selected: usize,
    hints: &[(&str, &str)],
    on_pick: impl Fn(usize) -> Box<dyn Fn(&mut SlateWindow, &mut Context<SlateWindow>)> + 'static,
    cx: &mut Context<SlateWindow>,
) -> AnyElement {
    let t = win.theme;
    let total = rows.len();
    let body = rows.into_iter().take(MAX_ROWS).enumerate().map(|(i, r)| {
        let pick = on_pick(i);
        div()
            .id(r.id)
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(12.0))
            .py(px(7.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .when(i == selected, |d| d.bg(t.active))
            .hover(|s| s.bg(t.active))
            .on_click(cx.listener(move |this, _, _, cx| pick(this, cx)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(if i == selected { t.heading } else { t.text })
                    .child(format!("{}{}", if r.locked { "🔒 " } else { "" }, r.label)),
            )
            .child(
                div()
                    .flex_none()
                    .max_w(px(280.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(11.5))
                    .text_color(t.faint)
                    .child(r.detail),
            )
    });
    div()
        .absolute()
        .occlude()
        .top(px(96.0))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .child(
            div()
                .w(px(640.0))
                .max_w_full()
                .bg(t.panel)
                .border_1()
                .border_color(t.border)
                .rounded(px(10.0))
                .shadow_lg()
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .h(px(52.0))
                        .px(px(16.0))
                        .border_b_1()
                        .border_color(t.border)
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(t.muted)
                                .child(title.to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(16.0))
                                .text_color(t.heading)
                                .child(format!("{query}▏")),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(t.faint)
                                .child(format!("{total}")),
                        ),
                )
                .child(div().p(px(6.0)).flex().flex_col().children(body))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_x(px(18.0))
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
                                .child(div().text_color(t.text).child(k.to_string()))
                                .child(v.to_string())
                        })),
                ),
        )
        .into_any_element()
}

pub fn render_switcher(
    win: &SlateWindow,
    s: &Switcher,
    cx: &mut Context<SlateWindow>,
) -> AnyElement {
    let matches = s.matches();
    let ids: Vec<String> = matches
        .iter()
        .take(MAX_ROWS)
        .map(|n| n.id.clone())
        .collect();
    let rows = matches
        .iter()
        .map(|n| Row {
            id: SharedString::from(format!("sw-{}", n.id)),
            label: display_title(n).to_string(),
            detail: s.snippets.get(&n.id).cloned().unwrap_or_default(),
            locked: locked(n),
        })
        .collect();
    let scope = match (&win.host.working, s.all) {
        (Some((_, name)), false) => name.clone(),
        _ => "All notes".to_string(),
    };
    let scope = if s.text {
        format!("{scope} · text")
    } else {
        scope
    };
    popup(
        win,
        &scope,
        &s.query,
        rows,
        s.selected,
        &[
            ("Enter", "open"),
            ("Tab", "search text"),
            ("Ctrl+N", "new"),
            ("Ctrl+G", "collections"),
            ("Ctrl+R", "history"),
            ("Ctrl+L", "scope"),
            ("Esc", "close"),
        ],
        move |i| {
            let id = ids.get(i).cloned();
            Box::new(move |win, cx| {
                if let Some(id) = &id {
                    win.overlay = Overlay::None;
                    win.open_note(id, cx);
                }
            })
        },
        cx,
    )
}

pub fn render_picker(win: &SlateWindow, p: &Picker, cx: &mut Context<SlateWindow>) -> AnyElement {
    let visible = p.visible();
    let picks: Vec<(Option<String>, String)> = visible
        .iter()
        .take(MAX_ROWS)
        .map(|e| (e.0.clone(), e.1.clone()))
        .collect();
    let rows = visible
        .iter()
        .map(|e| Row {
            id: SharedString::from(format!("pk-{}", e.1)),
            label: e.1.clone(),
            detail: format!("{} notes", e.2),
            locked: false,
        })
        .collect();
    popup(
        win,
        "Collection",
        &p.query,
        rows,
        p.selected,
        &[("Enter", "set working collection"), ("Esc", "close")],
        move |i| {
            let pick = picks.get(i).cloned();
            Box::new(move |win, cx| {
                if let Some((id, name)) = &pick {
                    win.overlay = Overlay::None;
                    win.host
                        .set_working(id.clone().map(|id| (id, name.clone())));
                    win.set_status(match id {
                        Some(_) => format!("working collection: {name}"),
                        None => "working collection cleared".to_string(),
                    });
                    cx.notify();
                }
            })
        },
        cx,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_matches_subsequences_and_ranks_tight_matches_first() {
        assert!(fuzzy("", "anything").is_some());
        assert!(fuzzy("lsb", "Lisbon trip").is_some());
        assert!(fuzzy("xyz", "Lisbon trip").is_none());
        let tight = fuzzy("lis", "Lisbon trip").unwrap();
        let loose = fuzzy("lis", "Lost in space").unwrap();
        assert!(tight > loose);
        // Word starts beat the middle of a word.
        assert!(fuzzy("tr", "Lisbon trip").unwrap() > fuzzy("tr", "Lisbon atrium").unwrap());
        // Case does not matter.
        assert_eq!(fuzzy("LIS", "lisbon"), fuzzy("lis", "LISBON"));
    }

    #[test]
    fn tab_switches_between_title_and_text_search() {
        let dir = std::env::temp_dir().join(format!("slate-gui-switch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = app_core::storage::Db::open(dir.join("notes.db")).unwrap();
        for (id, body) in [
            ("a", "# Garden\nroses and tulips\nmore tulips"),
            ("b", "# Budget\nrent"),
        ] {
            db.create_note_with_context(id, Default::default(), None, None)
                .unwrap();
            db.save_note(id, body).unwrap();
        }
        let host = NoteHost::open(db, Some("b")).unwrap();
        let mut s = Switcher::open(&host);
        s.query = "tulips".into();
        assert!(s.matches().is_empty(), "no title contains it");
        s.text = true;
        s.query_changed(&host);
        let ids: Vec<&str> = s.matches().iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["a"], "one row per note");
        assert!(s.snippets.contains_key("a"));
        s.text = false;
        s.query_changed(&host);
        assert!(s.matches().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
