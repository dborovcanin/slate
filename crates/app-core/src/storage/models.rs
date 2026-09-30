use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteAccessMode {
    None,
    Encrypted,
}

impl Default for NoteAccessMode {
    fn default() -> Self {
        Self::None
    }
}

/// Stored per note as JSON in `notes.modules_json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteModules {
    pub math: bool,
    pub table: bool,
    pub variables: bool,
    pub style: bool,
    pub cross_note: bool,
}

impl Default for NoteModules {
    fn default() -> Self {
        Self {
            math: true,
            table: true,
            variables: true,
            style: true,
            cross_note: true,
        }
    }
}

/// The outcome of a write, without the body the caller just sent.
/// `updated_at` is the revision token used for optimistic concurrency.
#[derive(Debug, Clone)]
pub struct NoteRevision {
    pub id: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct Note {
    pub id: String,
    pub body: String,
    /// Title set by hand, which edits to the text no longer change.
    pub pinned_title: Option<String>,
    pub modules: NoteModules,
    pub access_mode: NoteAccessMode,
    pub is_unlocked: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct NoteSummary {
    pub id: String,
    pub title: String,
    pub body_prefix: String,
    pub access_mode: NoteAccessMode,
    pub is_unlocked: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct NoteSearchResult {
    pub id: String,
    pub title: String,
    pub snippet: String,
    pub line_number: usize,
    pub rank: f64,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct Reminder {
    pub note_id: String,
    pub line_number: i64,
    pub remind_at_ms: i64,
    pub display_at: String,
    pub line_text: String,
    pub reminded_at_ms: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Note counts for collection browsing: every note, notes in no collection,
/// and notes per collection id (collections without notes are absent).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CollectionCounts {
    pub total: usize,
    pub unsorted: usize,
    pub per_collection: std::collections::HashMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A stored older version of a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteVersion {
    pub id: i64,
    /// When this version was saved (RFC 3339).
    pub saved_at: String,
    /// Lines the editing session after this version added and removed.
    pub lines_added: usize,
    pub lines_removed: usize,
}
