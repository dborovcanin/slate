use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NoteAccessMode {
    None,
    Locked,
    Encrypted,
}

impl Default for NoteAccessMode {
    fn default() -> Self {
        Self::None
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteModules {
    pub math: bool,
    pub table: bool,
    pub variables: bool,
    pub style: bool,
    #[serde(default = "default_true")]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub body: String,
    pub modules: NoteModules,
    pub access_mode: NoteAccessMode,
    pub is_unlocked: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteSummary {
    pub id: String,
    pub title: String,
    pub body_prefix: String,
    pub access_mode: NoteAccessMode,
    pub is_unlocked: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteSearchResult {
    pub id: String,
    pub title: String,
    pub snippet: String,
    pub line_number: usize,
    pub rank: f64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tag {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
}
