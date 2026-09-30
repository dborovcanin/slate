//! Collection browser: a three-column view in the style of yazi. The left
//! column shows the parent level, the middle one the list being navigated
//! (collections, or the notes of one collection) and the right one a preview
//! of the hovered entry. This module holds the view state, list navigation
//! and drawing; database work lives in `app::browser`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use app_core::history::LineChange;
use app_core::storage::{NoteAccessMode, NoteSummary, NoteVersion};
use ratatui::buffer::Buffer;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::canvas::{
    cell_style, contrast_fg_for_bg, draw_framed_surface, draw_input_box, fill, put_str_width,
    scrolled_input, InputBox,
};
use super::icons::Icons;
use super::render::{LineDecorations, RenderContext, RenderPalette};
use super::switcher::fuzzy_score;
use super::text_utils::case_insensitive_matches;
use ratatui::style::Modifier;

/// Characters of a note body loaded for its preview.
pub const PREVIEW_BODY_CHARS: usize = 6_000;

/// What a notes list shows: every note, notes in no collection, or one
/// collection's notes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    All,
    Unsorted,
    Collection(String),
}

impl Scope {
    pub fn collection_id(&self) -> Option<&str> {
        match self {
            Scope::Collection(id) => Some(id),
            Scope::All | Scope::Unsorted => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CollectionEntry {
    pub scope: Scope,
    pub name: String,
    pub description: String,
    pub count: usize,
    /// Its notes are encrypted with its key.
    pub encrypted: bool,
}

#[derive(Debug, Clone)]
pub struct NoteEntry {
    pub id: String,
    pub title: String,
    pub access_mode: NoteAccessMode,
    pub is_unlocked: bool,
    pub updated_at: String,
    pub is_daily: bool,
}

impl NoteEntry {
    pub fn from_summary(summary: NoteSummary, daily_prefix: &str) -> Self {
        Self {
            is_daily: app_core::daily::is_daily_note_id(daily_prefix, &summary.id),
            id: summary.id,
            title: summary.title,
            access_mode: summary.access_mode,
            is_unlocked: summary.is_unlocked,
            updated_at: summary.updated_at,
        }
    }

    pub fn is_locked(&self) -> bool {
        self.access_mode != NoteAccessMode::None && !self.is_unlocked
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Collections,
    Notes,
    /// Full-text search over the notes of one scope.
    Search,
    /// Stored versions of one note.
    History,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    #[default]
    Modified,
    Title,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Modified => "modified",
            SortKey::Title => "title",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipOp {
    Copy,
    Cut,
}

/// Notes yanked (`y`) or cut (`x`), waiting for `p`.
#[derive(Debug, Clone)]
pub struct Clipboard {
    pub op: ClipOp,
    pub note_ids: Vec<String>,
    pub source: Scope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    Filter,
    NewNote,
    NewCollection,
    RenameNote {
        note_id: String,
    },
    RenameCollection {
        collection_id: String,
    },
    Unlock {
        note_id: String,
        title: String,
        /// 1-based line to open the note at (search hits).
        line: Option<usize>,
        /// Encrypted collection whose password unlocks the note.
        collection: Option<String>,
        /// Rename the note once unlocked instead of opening it.
        rename: bool,
    },
    UnlockCollection {
        collection_id: String,
        name: String,
    },
    /// New password for a collection; asked twice, `first` holds the first
    /// entry while it is repeated.
    EncryptCollection {
        collection_id: String,
        name: String,
        first: Option<String>,
    },
    DecryptCollection {
        collection_id: String,
        name: String,
    },
}

impl PromptKind {
    fn title(&self) -> &'static str {
        match self {
            PromptKind::Filter => "Filter",
            PromptKind::NewNote => "New note",
            PromptKind::NewCollection => "New collection",
            PromptKind::RenameNote { .. } => "Rename note",
            PromptKind::RenameCollection { .. } => "Rename collection",
            PromptKind::Unlock { .. } | PromptKind::UnlockCollection { .. } => "Password",
            PromptKind::EncryptCollection { first: None, .. } => "New password",
            PromptKind::EncryptCollection { .. } => "Repeat password",
            PromptKind::DecryptCollection { .. } => "Password to decrypt",
        }
    }

    /// Typed text is a password: shown masked and used untrimmed.
    pub fn is_password(&self) -> bool {
        matches!(
            self,
            PromptKind::Unlock { .. }
                | PromptKind::UnlockCollection { .. }
                | PromptKind::EncryptCollection { .. }
                | PromptKind::DecryptCollection { .. }
        )
    }
}

#[derive(Debug, Clone)]
pub struct Prompt {
    pub kind: PromptKind,
    pub text: String,
    /// Char index; `usize::MAX` means at the end.
    pub cursor: usize,
}

#[derive(Debug, Clone)]
pub enum Confirm {
    DeleteNotes {
        note_ids: Vec<String>,
        label: String,
    },
    RestoreVersion {
        version_id: i64,
        label: String,
    },
    DeleteCollection {
        collection_id: String,
        name: String,
    },
}

/// A reversible membership change, for `u`.
#[derive(Debug, Clone, Default)]
pub struct MembershipUndo {
    pub added: Vec<(String, Vec<String>)>,
    pub removed: Vec<(String, Vec<String>)>,
    pub label: String,
}

/// A note whose text matches the search, at its first matching line.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub note: NoteEntry,
    /// 1-based line of the match.
    pub line_number: usize,
}

/// Content search state while the search level is open.
#[derive(Debug, Clone)]
pub struct ContentSearch {
    pub scope: Scope,
    /// Level to return to on `Esc`.
    pub return_level: Level,
    pub query: String,
    /// Char index; `usize::MAX` means at the end.
    pub cursor: usize,
    pub hits: Vec<SearchHit>,
    /// Hit picked with the arrows; a new query or new hits clear it.
    pub selected: Option<usize>,
    /// Query the current hits belong to.
    pub searched: String,
    /// The query changed and waits for the debounce before it runs.
    pub pending: bool,
    pub running: bool,
    pub error: Option<String>,
}

impl ContentSearch {
    pub fn new(scope: Scope, return_level: Level) -> Self {
        Self {
            scope,
            return_level,
            query: String::new(),
            cursor: usize::MAX,
            hits: Vec::new(),
            selected: None,
            searched: String::new(),
            pending: false,
            running: false,
            error: None,
        }
    }

    pub fn hovered(&self) -> Option<&SearchHit> {
        self.hits.get(self.selected?)
    }
}

/// Lowercased words and quoted phrases of a search query, for highlighting.
pub fn search_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for (idx, part) in query.split('"').enumerate() {
        if idx % 2 == 1 {
            let phrase = part.trim().to_lowercase();
            if !phrase.is_empty() {
                terms.push(phrase);
            }
        } else {
            terms.extend(part.split_whitespace().map(str::to_lowercase));
        }
    }
    terms.sort_by_key(|term| std::cmp::Reverse(term.chars().count()));
    terms.dedup();
    terms
}

/// Versions of one note, listed under its current text.
#[derive(Debug, Clone)]
pub struct HistoryView {
    pub note: NoteEntry,
    /// Level to return to on `h`/`Esc`.
    pub return_level: Level,
    pub versions: Vec<NoteVersion>,
    /// 0 is the current text; `i` is `versions[i - 1]`.
    pub selected: usize,
    /// Show the version's text instead of what restoring it would change.
    pub show_text: bool,
}

impl HistoryView {
    pub fn selected_version(&self) -> Option<&NoteVersion> {
        self.selected
            .checked_sub(1)
            .and_then(|index| self.versions.get(index))
    }
}

/// Preview of a stored version.
#[derive(Debug, Clone)]
pub enum VersionPreview {
    /// What restoring the version would change in the current text.
    Changes(Vec<LineChange>),
    Text(Vec<String>),
    /// The version cannot be read (locked note, broken history).
    Unavailable(String),
}

#[derive(Debug, Clone, Default)]
pub enum Preview {
    #[default]
    Empty,
    /// Notes of the hovered collection.
    Notes {
        description: String,
        notes: Arc<Vec<NoteEntry>>,
    },
    /// Body and metadata of the hovered note.
    Note {
        note_id: String,
        /// The first lines of the body.
        lines: Vec<String>,
        /// 0-based line to scroll to and highlight (search hits).
        focus_line: Option<usize>,
        /// RFC 3339 creation time, when known.
        created_at: Option<String>,
        collections: Vec<String>,
        tags: Vec<String>,
        locked: bool,
    },
    Version {
        version_id: i64,
        content: VersionPreview,
    },
}

#[derive(Debug, Clone, Default)]
pub struct BrowserState {
    pub level: Option<Level>,
    pub collections: Vec<CollectionEntry>,
    pub collection_matches: Vec<usize>,
    pub collection_cursor: usize,
    pub collection_filter: String,
    /// Scope of `notes`; set while the notes level is open.
    pub scope: Option<Scope>,
    pub notes: Vec<NoteEntry>,
    pub note_matches: Vec<usize>,
    pub note_cursor: usize,
    pub note_filter: String,
    pub marked: HashSet<String>,
    pub clipboard: Option<Clipboard>,
    pub prompt: Option<Prompt>,
    pub confirm: Option<Confirm>,
    pub undo: Option<MembershipUndo>,
    pub sort: SortKey,
    pub preview: Preview,
    /// Notes lists of collections previewed so far; cleared on any change.
    pub scope_cache: HashMap<Scope, Arc<Vec<NoteEntry>>>,
    pub message: Option<String>,
    pub pending_g: bool,
    pub search: Option<ContentSearch>,
    pub history: Option<HistoryView>,
}

impl BrowserState {
    pub fn level(&self) -> Level {
        self.level.unwrap_or(Level::Collections)
    }

    pub fn hovered_collection(&self) -> Option<&CollectionEntry> {
        self.collection_matches
            .get(self.collection_cursor)
            .and_then(|idx| self.collections.get(*idx))
    }

    pub fn hovered_note(&self) -> Option<&NoteEntry> {
        self.note_matches
            .get(self.note_cursor)
            .and_then(|idx| self.notes.get(*idx))
    }

    /// The note the preview shows: the hovered note or search hit.
    pub fn focused_note(&self) -> Option<&NoteEntry> {
        match self.level() {
            Level::Collections => None,
            Level::Notes => self.hovered_note(),
            Level::Search => self.search.as_ref()?.hovered().map(|hit| &hit.note),
            Level::History => self.history.as_ref().map(|history| &history.note),
        }
    }

    pub fn scope_name(&self, scope: &Scope) -> String {
        self.collections
            .iter()
            .find(|entry| &entry.scope == scope)
            .map(|entry| entry.name.clone())
            .unwrap_or_else(|| "?".to_string())
    }

    pub fn collection_name(&self, collection_id: &str) -> Option<&str> {
        self.collections
            .iter()
            .find(|entry| entry.scope.collection_id() == Some(collection_id))
            .map(|entry| entry.name.as_str())
    }

    fn active_filter(&self) -> &str {
        match self.level() {
            Level::Collections => &self.collection_filter,
            Level::Notes => &self.note_filter,
            Level::Search | Level::History => "",
        }
    }

    pub fn set_active_filter(&mut self, filter: String) {
        match self.level() {
            Level::Collections => {
                self.collection_filter = filter;
                self.recompute_collection_matches();
            }
            Level::Notes => {
                self.note_filter = filter;
                self.recompute_note_matches();
            }
            Level::Search | Level::History => {}
        }
    }

    pub fn has_filter(&self) -> bool {
        !self.active_filter().trim().is_empty()
    }

    pub fn recompute_collection_matches(&mut self) {
        let hovered = self.hovered_collection().map(|entry| entry.scope.clone());
        self.collection_matches =
            filtered_indices(&self.collections, &self.collection_filter, |e| &e.name);
        self.collection_cursor = hovered
            .and_then(|scope| {
                self.collection_matches
                    .iter()
                    .position(|idx| self.collections[*idx].scope == scope)
            })
            .unwrap_or(0);
    }

    pub fn recompute_note_matches(&mut self) {
        let hovered = self.hovered_note().map(|note| note.id.clone());
        self.note_matches = filtered_indices(&self.notes, &self.note_filter, |n| &n.title);
        self.note_cursor = hovered.and_then(|id| self.note_position(&id)).unwrap_or(0);
    }

    pub fn note_position(&self, note_id: &str) -> Option<usize> {
        self.note_matches
            .iter()
            .position(|idx| self.notes[*idx].id == note_id)
    }

    pub fn sort_notes(notes: &mut [NoteEntry], sort: SortKey) {
        match sort {
            SortKey::Modified => notes.sort_by(|a, b| b.updated_at.cmp(&a.updated_at)),
            SortKey::Title => notes.sort_by_cached_key(|note| note.title.to_lowercase()),
        }
    }

    fn list_len(&self) -> usize {
        match self.level() {
            Level::Collections => self.collection_matches.len(),
            Level::Notes => self.note_matches.len(),
            Level::Search => self.search.as_ref().map_or(0, |search| search.hits.len()),
            Level::History => self
                .history
                .as_ref()
                .map_or(0, |history| history.versions.len() + 1),
        }
    }

    fn cursor_mut(&mut self) -> &mut usize {
        match (self.level(), self.search.as_mut(), self.history.as_mut()) {
            (Level::History, _, Some(history)) => &mut history.selected,
            (Level::Notes, ..) => &mut self.note_cursor,
            _ => &mut self.collection_cursor,
        }
    }

    /// Moves the cursor by `delta` rows, clamped to the list.
    pub fn move_cursor(&mut self, delta: isize) {
        if let (Level::Search, Some(search)) = (self.level(), self.search.as_mut()) {
            search.selected = step_selection(search.selected, delta, search.hits.len());
            return;
        }
        let len = self.list_len();
        let cursor = self.cursor_mut();
        *cursor = if len == 0 {
            0
        } else {
            cursor.saturating_add_signed(delta).min(len - 1)
        };
    }

    pub fn move_to_end(&mut self, end: bool) {
        if let (Level::Search, Some(search)) = (self.level(), self.search.as_mut()) {
            let len = search.hits.len();
            search.selected = (len > 0).then(|| if end { len - 1 } else { 0 });
            return;
        }
        let len = self.list_len();
        *self.cursor_mut() = if end { len.saturating_sub(1) } else { 0 };
    }

    /// Marked notes in list order, or the hovered note when none is marked.
    pub fn selected_note_ids(&self) -> Vec<String> {
        if self.marked.is_empty() {
            return self
                .hovered_note()
                .map(|note| vec![note.id.clone()])
                .unwrap_or_default();
        }
        self.note_matches
            .iter()
            .map(|idx| &self.notes[*idx])
            .filter(|note| self.marked.contains(&note.id))
            .map(|note| note.id.clone())
            .collect()
    }

    pub fn toggle_mark_hovered(&mut self) {
        if let Some(id) = self.hovered_note().map(|note| note.id.clone()) {
            if !self.marked.remove(&id) {
                self.marked.insert(id);
            }
        }
    }

    pub fn mark_all_visible(&mut self) {
        let all_marked = self
            .note_matches
            .iter()
            .all(|idx| self.marked.contains(&self.notes[*idx].id));
        if all_marked {
            self.marked.clear();
        } else {
            for idx in &self.note_matches {
                self.marked.insert(self.notes[*idx].id.clone());
            }
        }
    }
}

fn filtered_indices<T>(items: &[T], filter: &str, key: impl Fn(&T) -> &str) -> Vec<usize> {
    let filter = filter.trim();
    if filter.is_empty() {
        return (0..items.len()).collect();
    }
    let mut scored: Vec<(usize, i32)> = items
        .iter()
        .enumerate()
        .filter_map(|(idx, item)| fuzzy_score(filter, key(item)).map(|score| (idx, score)))
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.into_iter().map(|(idx, _)| idx).collect()
}

use app_core::storage::timestamp_epoch;

/// Short age of `updated_at`: `now`, `5m`, `3h`, `2d`, or after a week the
/// local date in `date_format` (the `[editor]` pattern).
pub fn age_label(updated_at: &str, now_epoch: i64, date_format: &str) -> String {
    let Some(epoch) = timestamp_epoch(updated_at) else {
        return String::new();
    };
    let age = now_epoch.saturating_sub(epoch).max(0);
    match age {
        0..60 => "now".to_string(),
        60..3_600 => format!("{}m", age / 60),
        3_600..86_400 => format!("{}h", age / 3_600),
        86_400..604_800 => format!("{}d", age / 86_400),
        _ => local_date(epoch, date_format),
    }
}

/// Local time of a Unix timestamp in an `[editor]` date pattern.
fn local_date(epoch: i64, pattern: &str) -> String {
    super::date_picker::format_epoch_local(epoch, pattern).unwrap_or_default()
}

/// `text` cut to `width` display cells, ending in `…` when shortened.
pub(crate) fn fit_width(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > width - 1 {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// Colors, icons and the facts row styling depends on; shared by the
/// browser and the note and collection pickers.
#[derive(Clone, Copy)]
pub struct Look<'a> {
    pub palette: RenderPalette,
    pub icons: &'static Icons,
    pub working_collection_id: Option<&'a str>,
    pub active_note_id: &'a str,
    pub now_epoch: i64,
    /// `[editor] date_format` and `date_time_format`.
    pub date_format: &'a str,
    pub date_time_format: &'a str,
}

pub struct BrowserView<'a> {
    pub state: &'a BrowserState,
    pub look: Look<'a>,
}

#[derive(Clone, Copy)]
pub(crate) struct Pane {
    pub col: usize,
    pub width: usize,
}

/// Parent, current and preview panes for a screen `cols` wide; narrow
/// screens drop the parent pane, then the preview.
fn pane_layout(cols: usize) -> (Option<Pane>, Pane, Option<Pane>) {
    if cols < 40 {
        return (
            None,
            Pane {
                col: 1,
                width: cols,
            },
            None,
        );
    }
    if cols < 80 {
        let current = cols / 2;
        return (
            None,
            Pane {
                col: 1,
                width: current,
            },
            Some(Pane {
                col: current + 2,
                width: cols - current - 1,
            }),
        );
    }
    // yazi's 1:4:3 ratio, with a readable minimum for the parent column.
    let parent = (cols / 8).clamp(16, 30);
    let rest = cols - parent - 2;
    let current = rest * 4 / 7;
    (
        Some(Pane {
            col: 1,
            width: parent,
        }),
        Pane {
            col: parent + 2,
            width: current,
        },
        Some(Pane {
            col: parent + current + 3,
            width: rest - current,
        }),
    )
}

/// One list row: optional marker, icon, text and a dim right-aligned label.
pub(crate) struct Row {
    pub marker: Option<u8>,
    pub icon: &'static str,
    pub icon_fg: u8,
    pub text: String,
    pub text_fg: u8,
    pub bold: bool,
    pub right: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hover {
    Focused,
    Dim,
}

fn scroll_start(cursor: usize, len: usize, height: usize) -> usize {
    if len <= height {
        0
    } else {
        cursor.saturating_sub(height / 2).min(len - height)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_rows(
    buf: &mut Buffer,
    palette: RenderPalette,
    pane: Pane,
    top: usize,
    height: usize,
    len: usize,
    row_at: impl Fn(usize) -> Row,
    cursor: Option<(usize, Hover)>,
) {
    let bg = palette.surface_bg();
    let start = cursor.map_or(0, |(cursor, _)| scroll_start(cursor, len, height));
    // Rows are built only for the visible window, so long lists cost the
    // same to draw as short ones.
    for (offset, idx) in (start..len).take(height).enumerate() {
        let row = row_at(idx);
        let screen_row = top + offset;
        let hover = cursor.and_then(|(cursor, hover)| (cursor == idx).then_some(hover));
        let row_bg = match hover {
            Some(Hover::Focused) => palette.primary(),
            Some(Hover::Dim) => palette.code_block_bg,
            None => bg,
        };
        let on_accent = hover == Some(Hover::Focused);
        let fg_on = |fg: u8| {
            if on_accent {
                contrast_fg_for_bg(row_bg)
            } else {
                fg
            }
        };
        let base = cell_style(
            Some(fg_on(palette.text_fg())),
            Some(row_bg),
            Modifier::empty(),
        );
        fill(buf, screen_row, pane.col, pane.width, base);

        let mut col = pane.col;
        let marker_style = cell_style(
            Some(row.marker.unwrap_or(palette.text_fg())),
            Some(bg),
            Modifier::empty(),
        );
        put_str_width(
            buf,
            screen_row,
            col,
            if row.marker.is_some() { "▌" } else { " " },
            1,
            if row.marker.is_some() {
                marker_style
            } else {
                base
            },
        );
        col += 1;
        let inner = pane.width.saturating_sub(2);
        let right_width = row.right.width();
        let icon_style = cell_style(Some(fg_on(row.icon_fg)), Some(row_bg), Modifier::empty());
        col = put_str_width(buf, screen_row, col, row.icon, 1, icon_style);
        col = put_str_width(buf, screen_row, col, " ", 1, base);
        let text_room = inner.saturating_sub(2 + if right_width > 0 { right_width + 1 } else { 0 });
        let mut text_mods = Modifier::empty();
        if row.bold || on_accent {
            text_mods |= Modifier::BOLD;
        }
        put_str_width(
            buf,
            screen_row,
            col,
            &fit_width(&row.text, text_room),
            text_room,
            cell_style(Some(fg_on(row.text_fg)), Some(row_bg), text_mods),
        );
        if right_width > 0 && right_width + 3 <= inner {
            let dim = cell_style(
                Some(fg_on(palette.code_comment)),
                Some(row_bg),
                Modifier::empty(),
            );
            put_str_width(
                buf,
                screen_row,
                pane.col + pane.width - 1 - right_width,
                &row.right,
                right_width,
                dim,
            );
        }
    }
}

/// Icon and its color for a note: open, protected, daily or plain.
pub(crate) fn note_icon(look: &Look, note: &NoteEntry) -> (&'static str, u8) {
    let icons = look.icons;
    let palette = look.palette;
    if note.id == look.active_note_id {
        (icons.active, palette.primary())
    } else if note.access_mode == NoteAccessMode::Encrypted {
        (icons.encrypted, palette.code_keyword)
    } else if note.is_daily {
        (icons.daily_note, palette.code_number)
    } else {
        (icons.note, palette.text_fg())
    }
}

/// A note row with its icon, title and age.
pub(crate) fn plain_note_row(look: &Look, note: &NoteEntry) -> Row {
    let (icon, icon_fg) = note_icon(look, note);
    Row {
        marker: None,
        icon,
        icon_fg,
        text: note.title.clone(),
        text_fg: look.palette.text_fg(),
        bold: false,
        right: age_label(&note.updated_at, look.now_epoch, look.date_format),
    }
}

/// A collection row with its icon and note count; the working collection
/// gets a marker and a pin.
pub(crate) fn collection_entry_row(look: &Look, entry: &CollectionEntry, open: bool) -> Row {
    let icons = look.icons;
    let palette = look.palette;
    let (icon, icon_fg) = match &entry.scope {
        Scope::All => (icons.library, palette.code_type),
        Scope::Unsorted => (icons.inbox, palette.code_number),
        Scope::Collection(_) if entry.encrypted => (icons.encrypted, palette.code_keyword),
        Scope::Collection(_) if open => (icons.collection_open, palette.code_function),
        Scope::Collection(_) => (icons.collection, palette.code_function),
    };
    let working = match (&entry.scope, look.working_collection_id) {
        (Scope::Collection(id), Some(working)) => id == working,
        _ => false,
    };
    let mut right = entry.count.to_string();
    if working {
        right = format!("{} {right}", icons.working);
    }
    Row {
        marker: working.then_some(palette.primary()),
        icon,
        icon_fg,
        text: entry.name.clone(),
        text_fg: palette.code_function,
        bold: true,
        right,
    }
}

/// Row for the `pos`-th visible collection.
fn collection_row(view: &BrowserView, pos: usize, open_scope: Option<&Scope>) -> Row {
    let entry = &view.state.collections[view.state.collection_matches[pos]];
    collection_entry_row(&view.look, entry, open_scope == Some(&entry.scope))
}

/// A note row with the browser's marks and clipboard markers.
fn note_row(view: &BrowserView, note: &NoteEntry) -> Row {
    let palette = view.look.palette;
    let state = view.state;
    let mut row = plain_note_row(&view.look, note);
    let clip = state
        .clipboard
        .as_ref()
        .filter(|clip| clip.note_ids.contains(&note.id));
    row.marker = if state.marked.contains(&note.id) {
        Some(palette.code_string)
    } else {
        clip.map(|clip| match clip.op {
            ClipOp::Copy => palette.code_number,
            ClipOp::Cut => palette.code_keyword,
        })
    };
    if let Some(clip) = clip {
        let glyph = match clip.op {
            ClipOp::Copy => view.look.icons.copied,
            ClipOp::Cut => view.look.icons.cut,
        };
        row.right = format!("{glyph} {}", row.right);
    }
    row
}

pub(crate) fn draw_separator_column(
    buf: &mut Buffer,
    palette: RenderPalette,
    col: usize,
    top: usize,
    height: usize,
) {
    let style = cell_style(
        Some(palette.code_comment),
        Some(palette.surface_bg()),
        Modifier::DIM,
    );
    for row in top..top + height {
        put_str_width(buf, row, col, "│", 1, style);
    }
}

pub(crate) fn draw_centered_hint(
    buf: &mut Buffer,
    palette: RenderPalette,
    pane: Pane,
    row: usize,
    text: &str,
) {
    let style = cell_style(
        Some(palette.code_comment),
        Some(palette.surface_bg()),
        Modifier::empty(),
    );
    let text = fit_width(text, pane.width.saturating_sub(2));
    let col = pane.col + pane.width.saturating_sub(text.width()) / 2;
    put_str_width(buf, row, col, &text, pane.width, style);
}

/// Clears rows `1..rows` to the surface color.
pub(crate) fn draw_screen_base(look: &Look, buf: &mut Buffer, rows: usize, cols: usize) {
    let palette = look.palette;
    let base = cell_style(
        Some(palette.text_fg()),
        Some(palette.surface_bg()),
        Modifier::empty(),
    );
    for row in 1..rows {
        fill(buf, row, 1, cols, base);
    }
}

/// Header row: ` Slate › crumb` on the left and a dim `right` label.
pub(crate) fn draw_header_bar(
    look: &Look,
    buf: &mut Buffer,
    cols: usize,
    crumb: &str,
    right: &str,
) {
    let palette = look.palette;
    let icons = look.icons;
    let bg = palette.surface_bg();
    fill(
        buf,
        1,
        1,
        cols,
        cell_style(None, Some(bg), Modifier::empty()),
    );
    let accent = cell_style(Some(palette.primary()), Some(bg), Modifier::BOLD);
    let dim = cell_style(Some(palette.code_comment), Some(bg), Modifier::empty());
    let text = cell_style(Some(palette.text_fg()), Some(bg), Modifier::BOLD);
    let right_width = right.width();
    let left_room = cols.saturating_sub(right_width + 1);

    let mut col = put_str_width(
        buf,
        1,
        1,
        &format!(" {} ", icons.library),
        left_room,
        accent,
    );
    col = put_str_width(
        buf,
        1,
        col,
        "Slate",
        left_room.saturating_sub(col - 1),
        accent,
    );
    col = put_str_width(
        buf,
        1,
        col,
        &format!(" {} ", icons.separator),
        left_room.saturating_sub(col - 1),
        dim,
    );
    put_str_width(buf, 1, col, crumb, left_room.saturating_sub(col - 1), text);
    if right_width < cols {
        put_str_width(buf, 1, cols + 1 - right_width, right, right_width, dim);
    }
}

fn draw_header(view: &BrowserView, buf: &mut Buffer, cols: usize) {
    let state = view.state;
    let icons = view.look.icons;
    let (position, total) = match (state.level(), state.search.as_ref(), state.history.as_ref()) {
        (Level::Search, Some(search), _) => (search.selected, search.hits.len()),
        (Level::History, _, Some(history)) => (Some(history.selected), history.versions.len() + 1),
        (Level::Notes, ..) => (Some(state.note_cursor), state.note_matches.len()),
        _ => (
            Some(state.collection_cursor),
            state.collection_matches.len(),
        ),
    };
    let mut right = String::new();
    if let Some(search) = state.search.as_ref() {
        if search.running || search.pending {
            right.push_str("searching…  ");
        }
    }
    if state.has_filter() {
        right.push_str(&format!(
            "{} {}  ",
            icons.filter,
            state.active_filter().trim()
        ));
    }
    if !state.marked.is_empty() {
        right.push_str(&format!("{} {}  ", icons.marked, state.marked.len()));
    }
    if state.level() == Level::Notes {
        right.push_str(&format!("{}  ", state.sort.label()));
    }
    right.push_str(&position_label(position, total));
    right.push(' ');
    let crumb = match (state.level(), state.scope.as_ref(), state.search.as_ref()) {
        (Level::History, scope, _) => format!(
            "{}{} {} history",
            scope
                .map(|scope| format!("{} {} ", state.scope_name(scope), icons.separator))
                .unwrap_or_default(),
            state
                .history
                .as_ref()
                .map_or("", |history| history.note.title.as_str()),
            icons.separator
        ),
        (Level::Search, _, Some(search)) => format!(
            "{} {} {} search",
            state.scope_name(&search.scope),
            icons.separator,
            icons.filter
        ),
        (Level::Notes, Some(scope), _) => state.scope_name(scope),
        _ => "Collections".to_string(),
    };
    draw_header_bar(&view.look, buf, cols, &crumb, &right);
}

/// `3/12`, or `0/0` for an empty list.
/// `3/29`, or just `29` when nothing is selected.
pub(crate) fn position_label(position: Option<usize>, total: usize) -> String {
    match position {
        Some(position) => format!("{}/{}", (position + 1).min(total), total),
        None => total.to_string(),
    }
}

/// Moves an optional list selection by `delta` rows, clamped to the list.
/// With nothing selected, moving down picks the first row and up the last.
pub(crate) fn step_selection(selected: Option<usize>, delta: isize, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match selected {
        Some(pos) => pos.saturating_add_signed(delta).min(len - 1),
        None if delta < 0 => len - 1,
        None => 0,
    })
}

/// Title, metadata and the start of the body of `note`, from `preview`.
/// A focused line (search hit) is scrolled into view and `terms` are
/// highlighted.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_note_preview(
    look: &Look,
    buf: &mut Buffer,
    pane: Pane,
    top: usize,
    height: usize,
    note: &NoteEntry,
    preview: &Preview,
    terms: &[String],
) {
    let palette = look.palette;
    let icons = look.icons;
    let bg = palette.surface_bg();
    let Preview::Note {
        note_id,
        lines,
        focus_line,
        created_at,
        collections,
        tags,
        locked,
    } = preview
    else {
        return;
    };
    if *note_id != note.id {
        return;
    }
    let inner = pane.width.saturating_sub(2);
    let col = pane.col + 1;
    let dim = cell_style(Some(palette.code_comment), Some(bg), Modifier::empty());
    let mut row = top;
    let bottom = top + height;

    let (icon, icon_fg) = note_icon(look, note);
    put_str_width(
        buf,
        row,
        col,
        icon,
        1,
        cell_style(Some(icon_fg), Some(bg), Modifier::empty()),
    );
    put_str_width(
        buf,
        row,
        col + 2,
        &fit_width(&note.title, inner.saturating_sub(2)),
        inner.saturating_sub(2),
        cell_style(Some(palette.primary()), Some(bg), Modifier::BOLD),
    );
    row += 1;

    if let Some(epoch) = timestamp_epoch(&note.updated_at) {
        let age = look.now_epoch.saturating_sub(epoch);
        let when = local_date(epoch, look.date_time_format);
        let text = if age < 60 {
            format!("{} modified just now · {when}", icons.clock)
        } else if age < 604_800 {
            let label = age_label(&note.updated_at, look.now_epoch, look.date_format);
            format!("{} modified {label} ago · {when}", icons.clock)
        } else {
            format!("{} modified {when}", icons.clock)
        };
        put_str_width(buf, row, col, &fit_width(&text, inner), inner, dim);
        row += 1;
    }
    if let Some(epoch) = created_at.as_deref().and_then(timestamp_epoch) {
        if row < bottom {
            let text = format!(
                "{} added {}",
                icons.added,
                local_date(epoch, look.date_time_format)
            );
            put_str_width(buf, row, col, &fit_width(&text, inner), inner, dim);
            row += 1;
        }
    }
    if !collections.is_empty() && row < bottom {
        let text = format!("{} {}", icons.collection, collections.join(", "));
        put_str_width(
            buf,
            row,
            col,
            &fit_width(&text, inner),
            inner,
            cell_style(Some(palette.code_function), Some(bg), Modifier::empty()),
        );
        row += 1;
    }
    if !tags.is_empty() && row < bottom {
        let text = tags
            .iter()
            .map(|tag| format!("{} {tag}", icons.tag))
            .collect::<Vec<_>>()
            .join("  ");
        put_str_width(
            buf,
            row,
            col,
            &fit_width(&text, inner),
            inner,
            cell_style(Some(palette.code_string), Some(bg), Modifier::empty()),
        );
        row += 1;
    }
    if row < bottom {
        put_str_width(buf, row, col, &"─".repeat(inner), inner, dim);
        row += 1;
    }
    if *locked {
        if row < bottom {
            let text = format!("{} locked · Enter to unlock", icons.locked);
            put_str_width(buf, row, col, &fit_width(&text, inner), inner, dim);
        }
        return;
    }
    let mut ctx = RenderContext::with_syntax_mode(false, None, false, None, palette);
    // A search hit scrolls its line to about a third of the way down.
    let skip = focus_line.map_or(0, |focus| {
        focus
            .saturating_sub(bottom.saturating_sub(row) / 3)
            .min(lines.len().saturating_sub(1))
    });
    ctx.advance_lines(&lines[..skip]);
    let area = buf.area;
    for (line_idx, line) in lines.iter().enumerate().skip(skip) {
        if row >= bottom {
            break;
        }
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        for term in terms {
            for range in case_insensitive_matches(line, term) {
                if !ranges.iter().any(|r| r.0 < range.1 && range.0 < r.1) {
                    ranges.push(range);
                }
            }
        }
        ranges.sort_unstable();
        let is_focus = *focus_line == Some(line_idx);
        let deco = LineDecorations {
            calc_ghost: None,
            reminder_ghost: None,
            reminder_strikethrough: false,
            search_ranges: if is_focus { &[] } else { &ranges },
            current_search_ranges: if is_focus { &ranges } else { &[] },
            variable_names: None,
            dim_ranges: &[],
            selection_ranges: &[],
            accent_ranges: &[],
            underline_ranges: &[],
            active_cursor_col: None,
        };
        let (Ok(x), Ok(y)) = (u16::try_from(col - 1), u16::try_from(row - 1)) else {
            break;
        };
        if y >= area.bottom() || x >= area.right() {
            break;
        }
        ctx.render_line(line, inner, 0, &deco, buf, area.x + x, area.y + y);
        row += 1;
    }
}

/// A collection's description and the notes in it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_collection_preview(
    look: &Look,
    buf: &mut Buffer,
    pane: Pane,
    top: usize,
    height: usize,
    description: &str,
    notes: &[NoteEntry],
) {
    let palette = look.palette;
    let mut row = top;
    if !description.trim().is_empty() {
        let dim = cell_style(
            Some(palette.code_comment),
            Some(palette.surface_bg()),
            Modifier::ITALIC,
        );
        let inner = pane.width.saturating_sub(2);
        put_str_width(
            buf,
            row,
            pane.col + 1,
            &fit_width(description, inner),
            inner,
            dim,
        );
        row += 2;
    }
    if notes.is_empty() {
        draw_centered_hint(buf, palette, pane, row + 1, "empty");
    } else {
        draw_rows(
            buf,
            palette,
            pane,
            row,
            (top + height).saturating_sub(row),
            notes.len(),
            |idx| plain_note_row(look, &notes[idx]),
            None,
        );
    }
}

/// Query line at the top of `pane` with a rule under it. Returns the cursor
/// cell.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_query_bar(
    look: &Look,
    buf: &mut Buffer,
    pane: Pane,
    top: usize,
    query: &str,
    cursor: usize,
    placeholder: &str,
) -> (usize, usize) {
    let palette = look.palette;
    let bg = palette.surface_bg();
    let inner = query_bar_width(pane);
    let col = pane.col + 1;
    put_str_width(
        buf,
        top,
        col,
        look.icons.filter,
        1,
        cell_style(Some(palette.primary()), Some(bg), Modifier::BOLD),
    );
    let text_col = col + 2;
    let (visible, cursor_offset) = scrolled_input(query, cursor, inner);
    if query.is_empty() {
        put_str_width(
            buf,
            top,
            text_col,
            placeholder,
            inner,
            cell_style(Some(palette.code_comment), Some(bg), Modifier::empty()),
        );
    } else {
        put_str_width(
            buf,
            top,
            text_col,
            &visible,
            inner,
            cell_style(Some(palette.text_fg()), Some(bg), Modifier::BOLD),
        );
    }
    let rule = cell_style(Some(palette.primary()), Some(bg), Modifier::empty());
    put_str_width(
        buf,
        top + 1,
        pane.col,
        &"─".repeat(pane.width),
        pane.width,
        rule,
    );
    (top, text_col + cursor_offset)
}

fn query_bar_width(pane: Pane) -> usize {
    pane.width.saturating_sub(4)
}

/// Screen cell of the cursor in a query bar drawn by [`draw_query_bar`].
pub(crate) fn query_bar_cursor(
    pane: Pane,
    top: usize,
    query: &str,
    cursor: usize,
) -> (usize, usize) {
    let (_, offset) = scrolled_input(query, cursor, query_bar_width(pane));
    (top, pane.col + 3 + offset)
}

/// Row `pos` of a history list: the current text, then the stored versions.
fn version_row(look: &Look, history: &HistoryView, pos: usize) -> Row {
    let palette = look.palette;
    let Some(version) = pos.checked_sub(1).and_then(|i| history.versions.get(i)) else {
        let (icon, icon_fg) = note_icon(look, &history.note);
        return Row {
            marker: None,
            icon,
            icon_fg,
            text: "Current".to_string(),
            text_fg: palette.text_fg(),
            bold: true,
            right: age_label(&history.note.updated_at, look.now_epoch, look.date_format),
        };
    };
    let saved = timestamp_epoch(&version.saved_at);
    let when = saved
        .map(|epoch| local_date(epoch, look.date_time_format))
        .unwrap_or_else(|| version.saved_at.clone());
    let age = age_label(&version.saved_at, look.now_epoch, look.date_format);
    Row {
        marker: None,
        icon: look.icons.clock,
        icon_fg: palette.code_function,
        text: when,
        text_fg: palette.text_fg(),
        bold: false,
        right: format!("+{} −{}  {age}", version.lines_added, version.lines_removed),
    }
}

/// Preview of a stored version: what restoring it would change, or its text.
#[allow(clippy::too_many_arguments)]
fn draw_version_preview(
    look: &Look,
    buf: &mut Buffer,
    pane: Pane,
    top: usize,
    height: usize,
    history: &HistoryView,
    content: &VersionPreview,
) {
    let palette = look.palette;
    let icons = look.icons;
    let bg = palette.surface_bg();
    let inner = pane.width.saturating_sub(2);
    let col = pane.col + 1;
    let bottom = top + height;
    let dim = cell_style(Some(palette.code_comment), Some(bg), Modifier::empty());
    let mut row = top;

    let (icon, icon_fg) = note_icon(look, &history.note);
    put_str_width(
        buf,
        row,
        col,
        icon,
        1,
        cell_style(Some(icon_fg), Some(bg), Modifier::empty()),
    );
    put_str_width(
        buf,
        row,
        col + 2,
        &fit_width(&history.note.title, inner.saturating_sub(2)),
        inner.saturating_sub(2),
        cell_style(Some(palette.primary()), Some(bg), Modifier::BOLD),
    );
    row += 1;
    let subtitle = match content {
        VersionPreview::Changes(_) => format!(
            "{} restoring changes  − current  + this version",
            icons.clock
        ),
        VersionPreview::Text(_) => format!("{} text of this version", icons.clock),
        VersionPreview::Unavailable(_) => format!("{} version", icons.clock),
    };
    put_str_width(buf, row, col, &fit_width(&subtitle, inner), inner, dim);
    row += 1;
    put_str_width(buf, row, col, &"─".repeat(inner), inner, dim);
    row += 1;

    match content {
        VersionPreview::Unavailable(message) => {
            put_str_width(buf, row, col, &fit_width(message, inner), inner, dim);
        }
        VersionPreview::Changes(changes) if changes.is_empty() => {
            put_str_width(
                buf,
                row,
                col,
                &fit_width("same as the current text", inner),
                inner,
                dim,
            );
        }
        VersionPreview::Changes(changes) => {
            for change in changes {
                if row >= bottom {
                    break;
                }
                let (text, fg) = match change {
                    LineChange::Same(line) => (format!("  {line}"), palette.text_fg()),
                    LineChange::Added(line) => (format!("+ {line}"), palette.code_string),
                    LineChange::Removed(line) => (format!("- {line}"), palette.code_keyword),
                    LineChange::Skipped(count) => (
                        format!(
                            "⋯ {count} unchanged line{}",
                            if *count == 1 { "" } else { "s" }
                        ),
                        palette.code_comment,
                    ),
                };
                put_str_width(
                    buf,
                    row,
                    col,
                    &fit_width(&text, inner),
                    inner,
                    cell_style(Some(fg), Some(bg), Modifier::empty()),
                );
                row += 1;
            }
        }
        VersionPreview::Text(lines) => {
            let mut ctx = RenderContext::with_syntax_mode(false, None, false, None, palette);
            let deco = LineDecorations {
                calc_ghost: None,
                reminder_ghost: None,
                reminder_strikethrough: false,
                search_ranges: &[],
                current_search_ranges: &[],
                variable_names: None,
                dim_ranges: &[],
                selection_ranges: &[],
                accent_ranges: &[],
                underline_ranges: &[],
                active_cursor_col: None,
            };
            let area = buf.area;
            for line in lines {
                if row >= bottom {
                    break;
                }
                let (Ok(x), Ok(y)) = (u16::try_from(col - 1), u16::try_from(row - 1)) else {
                    break;
                };
                if y >= area.bottom() || x >= area.right() {
                    break;
                }
                ctx.render_line(line, inner, 0, &deco, buf, area.x + x, area.y + y);
                row += 1;
            }
        }
    }
}

/// Draws the browser over rows `1..rows` (the status bar row is left to the
/// caller). Returns the cursor cell while a prompt or search is open.
pub fn draw_browser(
    view: &BrowserView,
    buf: &mut Buffer,
    rows: usize,
    cols: usize,
) -> Option<(usize, usize)> {
    let look = &view.look;
    let palette = look.palette;
    let state = view.state;
    draw_screen_base(look, buf, rows, cols);
    draw_header(view, buf, cols);

    let top = 2;
    let height = rows.saturating_sub(2);
    if height == 0 {
        return None;
    }
    let (parent, current, preview) = pane_layout(cols);
    if let Some(parent) = parent {
        draw_separator_column(buf, palette, parent.col + parent.width, top, height);
    }
    if let Some(preview) = preview {
        draw_separator_column(buf, palette, preview.col - 1, top, height);
    }
    let draw_parent = |buf: &mut Buffer, open: Option<&Scope>| {
        let Some(parent) = parent else {
            return;
        };
        let hovered = open.and_then(|scope| {
            state
                .collection_matches
                .iter()
                .position(|idx| &state.collections[*idx].scope == scope)
        });
        draw_rows(
            buf,
            palette,
            parent,
            top,
            height,
            state.collection_matches.len(),
            |pos| collection_row(view, pos, open),
            hovered.map(|pos| (pos, Hover::Dim)),
        );
    };

    let mut search_cursor = None;
    match state.level() {
        Level::Collections => {
            if let Some(parent) = parent {
                let root = |_| Row {
                    marker: None,
                    icon: look.icons.library,
                    icon_fg: palette.code_type,
                    text: "Slate".to_string(),
                    text_fg: palette.text_fg(),
                    bold: true,
                    right: String::new(),
                };
                draw_rows(
                    buf,
                    palette,
                    parent,
                    top,
                    height,
                    1,
                    root,
                    Some((0, Hover::Dim)),
                );
            }
            let len = state.collection_matches.len();
            draw_rows(
                buf,
                palette,
                current,
                top,
                height,
                len,
                |pos| collection_row(view, pos, None),
                Some((state.collection_cursor, Hover::Focused)),
            );
            if len == 0 {
                draw_centered_hint(buf, palette, current, top + 1, "no match");
            }
            if let (Some(pane), Preview::Notes { description, notes }) = (preview, &state.preview) {
                draw_collection_preview(look, buf, pane, top, height, description, notes);
            }
        }
        Level::Notes => {
            draw_parent(buf, state.scope.as_ref());
            let len = state.note_matches.len();
            draw_rows(
                buf,
                palette,
                current,
                top,
                height,
                len,
                |pos| note_row(view, &state.notes[state.note_matches[pos]]),
                Some((state.note_cursor, Hover::Focused)),
            );
            if len == 0 {
                let hint = if state.has_filter() {
                    "no match"
                } else {
                    "empty · a new note · p paste"
                };
                draw_centered_hint(buf, palette, current, top + 1, hint);
            }
            if let (Some(pane), Some(note)) = (preview, state.focused_note()) {
                draw_note_preview(look, buf, pane, top, height, note, &state.preview, &[]);
            }
        }
        Level::Search => {
            let search = state.search.as_ref()?;
            draw_parent(buf, Some(&search.scope));
            search_cursor = Some(draw_query_bar(
                look,
                buf,
                current,
                top,
                &search.query,
                search.cursor,
                "search note text",
            ));
            let list_top = top + 2;
            draw_rows(
                buf,
                palette,
                current,
                list_top,
                height.saturating_sub(2),
                search.hits.len(),
                |pos| {
                    let hit = &search.hits[pos];
                    let mut row = plain_note_row(look, &hit.note);
                    row.right = format!(":{}", hit.line_number);
                    row
                },
                search.selected.map(|pos| (pos, Hover::Focused)),
            );
            if search.hits.is_empty() {
                let hint = if let Some(error) = search.error.as_deref() {
                    error
                } else if search.query.trim().is_empty() {
                    "type to search note text"
                } else if search.pending || search.running {
                    "searching…"
                } else {
                    "no matches"
                };
                draw_centered_hint(buf, palette, current, list_top + 1, hint);
            }
            if let (Some(pane), Some(note)) = (preview, state.focused_note()) {
                let terms = search_terms(&search.searched);
                draw_note_preview(look, buf, pane, top, height, note, &state.preview, &terms);
            }
        }
        Level::History => {
            let history = state.history.as_ref()?;
            if let Some(parent) = parent {
                let open = state.note_position(&history.note.id);
                draw_rows(
                    buf,
                    palette,
                    parent,
                    top,
                    height,
                    state.note_matches.len(),
                    |pos| plain_note_row(look, &state.notes[state.note_matches[pos]]),
                    open.map(|pos| (pos, Hover::Dim)),
                );
            }
            draw_rows(
                buf,
                palette,
                current,
                top,
                height,
                history.versions.len() + 1,
                |pos| version_row(look, history, pos),
                Some((history.selected, Hover::Focused)),
            );
            if history.versions.is_empty() {
                draw_centered_hint(buf, palette, current, top + 2, "no older versions yet");
            }
            if let Some(pane) = preview {
                match &state.preview {
                    Preview::Version { content, .. } => {
                        draw_version_preview(look, buf, pane, top, height, history, content)
                    }
                    other => {
                        draw_note_preview(look, buf, pane, top, height, &history.note, other, &[])
                    }
                }
            }
        }
    }

    if let Some(confirm) = state.confirm.as_ref() {
        draw_confirm(view, buf, rows, cols, confirm);
        return None;
    }
    match state.prompt.as_ref() {
        Some(prompt) => draw_prompt(view, buf, current, top, prompt),
        None => search_cursor,
    }
}

fn draw_prompt(
    view: &BrowserView,
    buf: &mut Buffer,
    pane: Pane,
    top: usize,
    prompt: &Prompt,
) -> Option<(usize, usize)> {
    let palette = view.look.palette;
    let width = pane.width.clamp(20, 60);
    let col = pane.col;
    let row = top;
    let locked = view.look.icons.locked;
    let title = match &prompt.kind {
        PromptKind::Unlock {
            title,
            collection: Some(collection),
            ..
        } => format!("{locked} {collection} · {title}"),
        PromptKind::Unlock { title, .. } => format!("{locked} {title}"),
        PromptKind::UnlockCollection { name, .. } => format!("{locked} {name}"),
        PromptKind::EncryptCollection { name, .. } | PromptKind::DecryptCollection { name, .. } => {
            format!("{locked} {name} · {}", prompt.kind.title())
        }
        kind => kind.title().to_string(),
    };
    Some(draw_input_box(
        buf,
        row,
        col,
        width,
        &palette,
        &InputBox {
            title,
            text: &prompt.text,
            cursor: prompt.cursor,
            password: prompt.kind.is_password(),
            hint: "Enter ok · Esc cancel",
        },
    ))
}

fn draw_confirm(view: &BrowserView, buf: &mut Buffer, rows: usize, cols: usize, confirm: &Confirm) {
    let palette = view.look.palette;
    let (title, question, detail) = match confirm {
        Confirm::DeleteNotes { note_ids, label } => (
            "Delete",
            if note_ids.len() == 1 {
                format!("Delete \"{label}\"?")
            } else {
                format!("Delete {} notes?", note_ids.len())
            },
            "This removes the notes permanently.".to_string(),
        ),
        Confirm::RestoreVersion { label, .. } => (
            "Restore",
            format!("Restore the version from {label}?"),
            "The current text stays in the history.".to_string(),
        ),
        Confirm::DeleteCollection { name, .. } => (
            "Delete collection",
            format!("Delete collection \"{name}\"?"),
            "Its notes stay; only the grouping goes.".to_string(),
        ),
    };
    let width =
        (question.width().max(detail.width()) + 6).clamp(30, cols.saturating_sub(4).max(30));
    let height = 5;
    let row = rows.saturating_sub(height) / 2 + 1;
    let col = cols.saturating_sub(width) / 2 + 1;
    let bg = palette.surface_bg();
    draw_framed_surface(
        buf,
        row,
        col,
        width,
        height,
        bg,
        palette.code_keyword,
        true,
        Some(title),
        Some("y confirm · n cancel"),
    );
    let inner = width.saturating_sub(4);
    put_str_width(
        buf,
        row + 1,
        col + 2,
        &fit_width(&question, inner),
        inner,
        cell_style(Some(palette.text_fg()), Some(bg), Modifier::BOLD),
    );
    put_str_width(
        buf,
        row + 3,
        col + 2,
        &fit_width(&detail, inner),
        inner,
        cell_style(Some(palette.code_comment), Some(bg), Modifier::empty()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_selection_starts_from_the_ends_and_clamps() {
        assert_eq!(step_selection(None, 1, 3), Some(0));
        assert_eq!(step_selection(None, -1, 3), Some(2));
        assert_eq!(step_selection(Some(1), 5, 3), Some(2));
        assert_eq!(step_selection(Some(1), -5, 3), Some(0));
        assert_eq!(step_selection(None, 1, 0), None);
        assert_eq!(position_label(None, 3), "3");
        assert_eq!(position_label(Some(0), 3), "1/3");
    }

    fn note(id: &str, title: &str, updated_at: &str) -> NoteEntry {
        NoteEntry {
            id: id.to_string(),
            title: title.to_string(),
            access_mode: NoteAccessMode::None,
            is_unlocked: true,
            updated_at: updated_at.to_string(),
            is_daily: false,
        }
    }

    #[test]
    fn age_labels_step_from_minutes_to_dates() {
        let now = timestamp_epoch("2026-09-30T12:00:00Z").unwrap();
        let format = "%d.%m.%Y";
        assert_eq!(age_label("2026-09-30T11:59:30Z", now, format), "now");
        assert_eq!(age_label("2026-09-30T11:55:00Z", now, format), "5m");
        assert_eq!(age_label("2026-09-30T09:00:00Z", now, format), "3h");
        assert_eq!(age_label("2026-09-28T12:00:00Z", now, format), "2d");
        // Older notes show the local date in the configured format; noon UTC
        // is the same calendar day in every common time zone.
        assert_eq!(age_label("2026-08-01T12:00:00Z", now, format), "01.08.2026");
    }

    #[test]
    fn filter_keeps_hovered_note_and_selection_prefers_marks() {
        let mut state = BrowserState {
            level: Some(Level::Notes),
            notes: vec![
                note("a", "Budget", "2026-09-30T10:00:00Z"),
                note("b", "Meeting notes", "2026-09-29T10:00:00Z"),
                note("c", "Budget review", "2026-09-28T10:00:00Z"),
            ],
            ..Default::default()
        };
        state.recompute_note_matches();
        state.move_cursor(2);
        assert_eq!(state.hovered_note().map(|n| n.id.as_str()), Some("c"));
        state.set_active_filter("bud".to_string());
        assert_eq!(state.note_matches.len(), 2);
        assert_eq!(state.hovered_note().map(|n| n.id.as_str()), Some("c"));
        assert_eq!(state.selected_note_ids(), vec!["c".to_string()]);

        state.set_active_filter(String::new());
        state.mark_all_visible();
        assert_eq!(state.selected_note_ids(), vec!["a", "b", "c"]);
        state.mark_all_visible();
        assert!(state.marked.is_empty());

        state.move_cursor(-10);
        assert_eq!(state.note_cursor, 0);
        state.move_to_end(true);
        assert_eq!(state.note_cursor, 2);
    }

    #[test]
    fn sorts_notes_by_title_or_recency() {
        let mut notes = vec![
            note("a", "beta", "2026-09-28T10:00:00Z"),
            note("b", "Alpha", "2026-09-30T10:00:00Z"),
        ];
        BrowserState::sort_notes(&mut notes, SortKey::Title);
        assert_eq!(notes[0].id, "b");
        BrowserState::sort_notes(&mut notes, SortKey::Modified);
        assert_eq!(notes[0].id, "b");
        notes[0].updated_at = "2026-09-01T10:00:00Z".to_string();
        BrowserState::sort_notes(&mut notes, SortKey::Modified);
        assert_eq!(notes[0].id, "a");
    }

    #[test]
    fn search_terms_split_words_keep_phrases_longest_first() {
        assert_eq!(
            search_terms(r#"Rent "food budget" rent x"#),
            vec!["food budget", "rent", "x"]
        );
        assert!(search_terms("  ").is_empty());
    }

    #[test]
    fn fit_width_truncates_with_ellipsis() {
        assert_eq!(fit_width("hello", 10), "hello");
        assert_eq!(fit_width("hello world", 6), "hello…");
        assert_eq!(fit_width("hello", 0), "");
    }

    #[test]
    fn layout_drops_panes_on_narrow_screens() {
        let (parent, current, preview) = pane_layout(120);
        let (parent, preview) = (parent.unwrap(), preview.unwrap());
        assert_eq!(parent.col, 1);
        assert_eq!(current.col, parent.width + 2);
        assert_eq!(preview.col + preview.width - 1, 120);
        assert!(current.width > preview.width);

        let (parent, _, preview) = pane_layout(60);
        assert!(parent.is_none() && preview.is_some());
        let (parent, current, preview) = pane_layout(30);
        assert!(parent.is_none() && preview.is_none());
        assert_eq!(current.width, 30);
    }
}
