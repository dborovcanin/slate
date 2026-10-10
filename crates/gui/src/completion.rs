//! Autocomplete while typing, the same flows as the terminal: variable and
//! formula-helper names, `[[ID]].variable`, and `[[` note and heading links.
//! The candidate rules are `editor_core::completion`; this module gathers
//! the candidates from the open note and the note list and applies picks.
use crate::note_view::NoteHost;
use editor_core::completion::{self, CrossNoteCompletionPrefix};
use editor_core::markdown_tokens;
use editor_core::vim::VimMode;
use note_session::input::InputOutcome;
use std::collections::HashMap;

/// Variable popups list a few names; link popups scroll.
const VARIABLE_MAX: usize = 3;
pub const WIKI_VISIBLE: usize = 12;

#[derive(Debug, Clone)]
pub struct Item {
    /// What the popup shows.
    pub label: String,
    /// The linked note, for link items.
    pub note_id: Option<String>,
    pub heading: Option<String>,
    lower: String,
}

impl Item {
    fn name(name: String) -> Self {
        Self {
            label: name,
            note_id: None,
            heading: None,
            lower: String::new(),
        }
    }
    fn note(id: String, title: String) -> Self {
        Self {
            lower: title.to_lowercase(),
            label: title,
            note_id: Some(id),
            heading: None,
        }
    }
    fn heading(id: &str, heading: String) -> Self {
        Self {
            lower: heading.to_lowercase(),
            label: heading.clone(),
            note_id: Some(id.to_string()),
            heading: Some(heading),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Kind {
    Variable {
        from: usize,
        to: usize,
    },
    Wiki {
        /// Column of the opening `[[`.
        from: usize,
        /// A note was picked and its `#` heading choice is open.
        pending: Option<String>,
        notes: Vec<Item>,
        headings: HashMap<String, Vec<Item>>,
    },
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub kind: Kind,
    pub items: Vec<Item>,
    pub selected: usize,
    pub line: usize,
    /// Column the popup hangs from.
    pub anchor_col: usize,
}

impl Completion {
    pub fn is_wiki(&self) -> bool {
        matches!(self.kind, Kind::Wiki { .. })
    }

    pub fn step(&mut self, delta: isize) {
        let len = self.items.len() as isize;
        if len > 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
        }
    }

    /// The visible slice and the index of its first item.
    pub fn window(&self, max: usize) -> (usize, &[Item]) {
        let visible = self.items.len().min(max);
        let start = (self.selected + 1).saturating_sub(visible);
        (start, &self.items[start..start + visible])
    }
}

/// What a pick leaves behind.
pub enum Picked {
    /// Done; the popup closes.
    Closed(InputOutcome),
    /// A note was linked; headings of that note are offered next.
    Headings(InputOutcome),
}

fn line_of(host: &NoteHost) -> &str {
    host.doc
        .lines()
        .get(host.doc.cursor_line)
        .map_or("", String::as_str)
}

fn char_range(line: &str, from: usize, to: usize) -> String {
    line.chars()
        .skip(from)
        .take(to.saturating_sub(from))
        .collect()
}

impl NoteHost {
    /// Variable names to complete at the cursor, if any.
    pub fn variable_completion(
        &self,
        min_chars: usize,
    ) -> Option<completion::VariableAutocompleteState> {
        if !self.modules.math || self.input.mode() != VimMode::Insert {
            return None;
        }
        let line = line_of(self);
        let col = self.doc.cursor_col;
        if let Some(CrossNoteCompletionPrefix {
            note_id,
            bracket_col,
            from_col,
            partial,
        }) = completion::extract_cross_note_completion_prefix(line, col)
        {
            if !self.modules.variables {
                return None;
            }
            let exports =
                app_core::cross_note::exports_for_autocomplete(&note_id, &self.index, &self.db);
            let suggestions = completion::cross_note_suggestions(
                exports
                    .iter()
                    .map(|e| (e.normalized.as_str(), e.name.as_str())),
                &partial,
            );
            return (!suggestions.is_empty()).then_some(completion::VariableAutocompleteState {
                popup_anchor_col: bracket_col,
                from_col,
                to_col: col,
                query: partial,
                suggestions,
            });
        }
        completion::local_variable_completion(
            line,
            col,
            self.modules.table,
            self.modules.variables,
            &self.session.calc().variable_names,
            min_chars,
            VARIABLE_MAX,
        )
    }

    fn note_items(&self) -> Vec<Item> {
        self.notes
            .iter()
            .filter(|n| n.access_mode == app_core::storage::NoteAccessMode::None || n.is_unlocked)
            .map(|n| {
                let title = if n.title.is_empty() {
                    "Untitled".to_string()
                } else {
                    n.title.clone()
                };
                Item::note(n.id.clone(), title)
            })
            .collect()
    }

    fn heading_items(&self, note_id: &str) -> Vec<Item> {
        let Ok(Some(note)) = self.db.get_note(note_id) else {
            return Vec::new();
        };
        markdown_tokens::extract_markdown_headings(&note.body)
            .into_iter()
            .map(|h| Item::heading(note_id, h))
            .collect()
    }

    fn replace_chars(
        &mut self,
        range: std::ops::Range<usize>,
        text: &str,
        cursor_col: usize,
    ) -> InputOutcome {
        let line = self.doc.cursor_line;
        self.run_input(|session, doc, input, cx| {
            session.replace_line_chars(doc, input, line, range, text, cursor_col, cx)
        })
    }
}

/// The popup for the variable name being typed, keeping the highlighted
/// pick while the same names are still offered.
pub fn variable(
    host: &NoteHost,
    min_chars: usize,
    previous: Option<&Completion>,
) -> Option<Completion> {
    let state = host.variable_completion(min_chars)?;
    let items: Vec<Item> = state.suggestions.into_iter().map(Item::name).collect();
    if items.is_empty() {
        return None;
    }
    let selected = previous
        .filter(|p| !p.is_wiki() && p.line == host.doc.cursor_line)
        .and_then(|p| p.items.get(p.selected))
        .and_then(|pick| items.iter().position(|i| i.label == pick.label))
        .unwrap_or(0);
    Some(Completion {
        kind: Kind::Variable {
            from: state.from_col,
            to: state.to_col,
        },
        items,
        selected,
        line: host.doc.cursor_line,
        anchor_col: state.popup_anchor_col,
    })
}

fn wiki_popup(host: &NoteHost, from: usize, query: &str) -> Completion {
    let notes = host.note_items();
    let mut popup = Completion {
        kind: Kind::Wiki {
            from,
            pending: None,
            notes: notes.clone(),
            headings: HashMap::new(),
        },
        items: notes,
        selected: 0,
        line: host.doc.cursor_line,
        anchor_col: from,
    };
    refresh_wiki(host, &mut popup, query);
    popup
}

/// `[[` was just typed with the cursor between the brackets.
pub fn open_wiki(host: &NoteHost) -> Completion {
    wiki_popup(host, host.doc.cursor_col.saturating_sub(2), "")
}

/// `#` typed inside a `[[link]]`: offer that note's headings.
pub fn open_wiki_at_cursor(host: &NoteHost) -> Option<Completion> {
    let line = line_of(host);
    let link = markdown_tokens::wiki_link_at_cursor(line, host.doc.cursor_col)?;
    if host.doc.cursor_col < link.from + 2 {
        return None;
    }
    let query = char_range(line, link.from + 2, host.doc.cursor_col);
    completion::parse_wiki_link_query(&query)?;
    Some(wiki_popup(host, link.from, &query))
}

fn refresh_wiki(host: &NoteHost, popup: &mut Completion, query: &str) {
    let Kind::Wiki {
        notes, headings, ..
    } = &mut popup.kind
    else {
        return;
    };
    let Some((target, heading)) = completion::parse_wiki_link_query(query) else {
        return;
    };
    popup.items = match heading {
        Some(value) => {
            let all = headings
                .entry(target.to_string())
                .or_insert_with(|| host.heading_items(target));
            if value.is_empty() {
                all.clone()
            } else {
                completion::filter_wiki_suggestions(all, value, |i| &i.lower)
            }
        }
        None if target.is_empty() => notes.clone(),
        None => completion::filter_wiki_suggestions(notes, target, |i| &i.lower),
    };
    popup.selected = 0;
}

/// Bring an open link popup up to date after an edit; `false` closes it.
pub fn refresh(host: &NoteHost, popup: &mut Completion) -> bool {
    let Kind::Wiki { from, .. } = popup.kind else {
        return true;
    };
    if popup.line != host.doc.cursor_line || host.doc.cursor_col < from + 2 {
        return false;
    }
    let query = char_range(line_of(host), from + 2, host.doc.cursor_col);
    if completion::parse_wiki_link_query(&query).is_none() {
        return false;
    }
    popup.anchor_col = from;
    refresh_wiki(host, popup, &query);
    true
}

/// Insert the highlighted pick.
pub fn accept(host: &mut NoteHost, popup: &mut Completion) -> Option<Picked> {
    let pick = popup.items.get(popup.selected)?.clone();
    let line = line_of(host).to_string();
    match popup.kind.clone() {
        Kind::Variable { from, to } => {
            let from = from.min(host.doc.cursor_col);
            let to = to.min(line.chars().count());
            if from > to {
                return None;
            }
            let end = from + pick.label.chars().count();
            Some(Picked::Closed(host.replace_chars(
                from..to,
                &pick.label,
                end,
            )))
        }
        Kind::Wiki { from, .. } => {
            let note_id = pick.note_id.clone()?;
            // A note without headings has nothing more to offer: finish
            // the link instead of leaving an empty heading step open.
            let finish = pick.heading.is_none() && host.heading_items(&note_id).is_empty();
            let replacement = match (&pick.heading, finish) {
                (Some(h), _) => format!("[[{note_id}#{h}]]"),
                (None, true) => format!("[[{note_id}]]"),
                (None, false) => format!("[[{note_id}#]]"),
            };
            // The link ends at the next `]]` after the cursor.
            let chars: Vec<char> = line.chars().collect();
            let mut end = host.doc.cursor_col;
            while end + 1 < chars.len() {
                if chars[end] == ']' && chars[end + 1] == ']' {
                    end += 2;
                    break;
                }
                end += 1;
            }
            let cursor = if pick.heading.is_some() || finish {
                from + replacement.chars().count()
            } else {
                from + 2 + note_id.chars().count() + 1
            };
            let outcome = host.replace_chars(from..end, &replacement, cursor);
            if pick.heading.is_some() || finish {
                Some(Picked::Closed(outcome))
            } else {
                if let Kind::Wiki { pending, .. } = &mut popup.kind {
                    *pending = Some(note_id.clone());
                }
                let query = format!("{note_id}#");
                refresh_wiki(host, popup, &query);
                Some(Picked::Headings(outcome))
            }
        }
    }
}

/// Close a link popup that was left on the heading step: drop the empty
/// `#` that picking the note added.
pub fn cancel(host: &mut NoteHost, popup: &Completion) -> Option<InputOutcome> {
    let Kind::Wiki {
        from,
        pending: Some(note_id),
        ..
    } = &popup.kind
    else {
        return None;
    };
    let line_ix = popup.line;
    let line = host.doc.lines().get(line_ix)?.clone();
    let chars: Vec<char> = line.chars().collect();
    let hash = from + 2 + note_id.chars().count();
    if chars.get(hash) != Some(&'#') {
        return None;
    }
    let mut end = hash;
    while end + 1 < chars.len() && !(chars[end] == ']' && chars[end + 1] == ']') {
        end += 1;
    }
    if end + 1 >= chars.len() || end == hash || line_ix != host.doc.cursor_line {
        return None;
    }
    Some(host.replace_chars(hash..end, "", hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(notes: &[(&str, &str)]) -> (NoteHost, std::path::PathBuf) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "slate-gui-complete-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = app_core::storage::Db::open(dir.join("notes.db")).unwrap();
        for (id, body) in notes {
            db.create_note_with_context(id, Default::default(), None, None)
                .unwrap();
            db.save_note(id, body).unwrap();
        }
        (NoteHost::open(db, Some("src")).unwrap(), dir)
    }

    fn type_link(h: &mut NoteHost) -> Completion {
        h.input.vim.mode = VimMode::Insert;
        h.doc.cursor_line = 0;
        h.doc.cursor_col = 0;
        for ch in "see ".chars().chain(['[', '[']) {
            h.handle_key(editor_core::vim::VimKey::Char(ch));
        }
        open_wiki(h)
    }

    #[test]
    fn picking_a_note_without_headings_finishes_the_link() {
        let (mut h, dir) = host(&[("src", ""), ("plain", "just text, no headings")]);
        let mut popup = type_link(&mut h);
        popup.selected = popup
            .items
            .iter()
            .position(|i| i.note_id.as_deref() == Some("plain"))
            .expect("note offered");
        let picked = accept(&mut h, &mut popup).expect("pick");
        assert!(matches!(picked, Picked::Closed(_)), "no heading step");
        assert_eq!(h.doc.lines()[0], "see [[plain]]");
        assert_eq!(h.doc.cursor_col, "see [[plain]]".chars().count());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn picking_a_note_with_headings_offers_them() {
        let (mut h, dir) = host(&[("src", ""), ("doc", "# Title\n## Part one\ntext")]);
        let mut popup = type_link(&mut h);
        popup.selected = popup
            .items
            .iter()
            .position(|i| i.note_id.as_deref() == Some("doc"))
            .expect("note offered");
        let picked = accept(&mut h, &mut popup).expect("pick");
        assert!(matches!(picked, Picked::Headings(_)));
        assert_eq!(h.doc.lines()[0], "see [[doc#]]");
        assert!(popup.items.iter().all(|i| i.heading.is_some()));
        assert!(!popup.items.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
