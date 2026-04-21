use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteModules {
    pub math: bool,
    pub table: bool,
    pub variables: bool,
    pub style: bool,
}

impl Default for NoteModules {
    fn default() -> Self {
        Self {
            math: true,
            table: true,
            variables: true,
            style: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub body: String,
    pub modules: NoteModules,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteSummary {
    pub id: String,
    pub body_prefix: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reminder {
    pub note_id: String,
    pub line_number: i64,
    pub remind_at_ms: i64,
    pub display_at: String,
    pub line_text: String,
    pub notified_at_ms: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}
