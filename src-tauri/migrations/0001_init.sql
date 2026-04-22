CREATE TABLE IF NOT EXISTS notes (
    id TEXT PRIMARY KEY,
    body TEXT NOT NULL DEFAULT '',
    note_title TEXT NOT NULL DEFAULT '',
    modules_json TEXT NOT NULL DEFAULT '{"math":true,"table":true,"variables":true,"style":true}',
    access_mode TEXT NOT NULL DEFAULT 'none',
    password_salt BLOB,
    password_hash BLOB,
    encryption_salt BLOB,
    encryption_nonce BLOB,
    encrypted_body BLOB,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_notes_updated ON notes(updated_at DESC);

CREATE TABLE IF NOT EXISTS reminders (
    note_id TEXT NOT NULL,
    line_number INTEGER NOT NULL CHECK(line_number > 0),
    remind_at_ms INTEGER NOT NULL,
    display_at TEXT NOT NULL,
    line_text TEXT NOT NULL DEFAULT '',
    notified_at_ms INTEGER,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (note_id, line_number),
    FOREIGN KEY (note_id) REFERENCES notes(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_reminders_note_line ON reminders(note_id, line_number);
CREATE INDEX IF NOT EXISTS idx_reminders_due ON reminders(remind_at_ms);
