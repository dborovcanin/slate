CREATE TABLE IF NOT EXISTS notes (
    id TEXT PRIMARY KEY,
    body TEXT NOT NULL DEFAULT '',
    note_title TEXT NOT NULL DEFAULT '',
    modules_json TEXT NOT NULL DEFAULT '{"math":true,"table":true,"variables":true,"style":true,"cross_note":true}',
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

CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
    note_id UNINDEXED,
    note_title,
    body,
    tokenize = 'unicode61',
    prefix = '2 3'
);

CREATE TRIGGER IF NOT EXISTS notes_fts_ai
AFTER INSERT ON notes
BEGIN
    INSERT INTO notes_fts(rowid, note_id, note_title, body)
    SELECT new.rowid, new.id, new.note_title, new.body
    WHERE new.access_mode = 'none';
END;

CREATE TRIGGER IF NOT EXISTS notes_fts_ad
AFTER DELETE ON notes
BEGIN
    DELETE FROM notes_fts WHERE rowid = old.rowid;
END;

CREATE TRIGGER IF NOT EXISTS notes_fts_au
AFTER UPDATE ON notes
BEGIN
    DELETE FROM notes_fts WHERE rowid = old.rowid;
    INSERT INTO notes_fts(rowid, note_id, note_title, body)
    SELECT new.rowid, new.id, new.note_title, new.body
    WHERE new.access_mode = 'none';
END;

INSERT INTO notes (
    id,
    body,
    note_title,
    modules_json,
    access_mode,
    created_at,
    updated_at
)
SELECT
    'welcome',
    '# Welcome to Slate

This note demonstrates modules and editing features.

## Headings

### Heading 3
#### Heading 4
##### Heading 5
###### Heading 6

## List Types

- Unordered item
- Another unordered item

1. Ordered item one
2. Ordered item two

- [ ] Checklist item
- [x] Completed checklist item

## Variables

salary := 4200
rent := 1300
tax_rate := 0.2
net := salary - rent
tax_due := net * tax_rate

## Table Formulas

| Item   | Price | Qty | Total |
| ------ | ----: | --: | ----: |
| Coffee |  3.50 |   2 | :=(1,2) * (1,3) |
| Snacks |  5.00 |   1 | :=(2,2) * (2,3) |
| Fruit  |  2.25 |   4 | :=(3,2) * (3,3) |
|        |       | Sum | :=sum_col() |',
    'Welcome to Slate',
    '{"math":true,"table":true,"variables":true,"style":true,"cross_note":true}',
    'none',
    strftime('%Y-%m-%dT%H:%M:%fZ','now'),
    strftime('%Y-%m-%dT%H:%M:%fZ','now')
WHERE NOT EXISTS (
    SELECT 1 FROM notes WHERE id = 'welcome'
)
AND NOT EXISTS (
    SELECT 1 FROM notes
);

CREATE TABLE IF NOT EXISTS reminders (
    note_id TEXT NOT NULL,
    line_number INTEGER NOT NULL CHECK(line_number > 0),
    remind_at_ms INTEGER NOT NULL,
    display_at TEXT NOT NULL,
    line_text TEXT NOT NULL DEFAULT '',
    reminded_at_ms INTEGER,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (note_id, line_number),
    FOREIGN KEY (note_id) REFERENCES notes(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_reminders_note_line ON reminders(note_id, line_number);
CREATE INDEX IF NOT EXISTS idx_reminders_due ON reminders(remind_at_ms);

CREATE TABLE IF NOT EXISTS ingest_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source TEXT NOT NULL,
    message_id TEXT,
    note_id TEXT NOT NULL,
    received_at TEXT NOT NULL,
    raw_payload BLOB NOT NULL,
    body_truncated INTEGER NOT NULL DEFAULT 0,
    message_truncated INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_ingest_events_received_at ON ingest_events(received_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS idx_ingest_events_source_message_id
    ON ingest_events(source, message_id)
    WHERE message_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS ingest_offsets (
    source_key TEXT PRIMARY KEY,
    last_uid INTEGER NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS note_images (
    id TEXT PRIMARY KEY,
    note_id TEXT NOT NULL,
    file_name TEXT,
    mime_type TEXT NOT NULL,
    image_bytes BLOB,
    byte_len INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'pending',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (note_id) REFERENCES notes(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_note_images_note_updated
    ON note_images(note_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS collections (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    normalized_name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_collections_normalized_name
    ON collections(normalized_name);
CREATE INDEX IF NOT EXISTS idx_collections_name
    ON collections(name);

CREATE TABLE IF NOT EXISTS tags (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    normalized_name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_tags_normalized_name
    ON tags(normalized_name);
CREATE INDEX IF NOT EXISTS idx_tags_name
    ON tags(name);

CREATE TABLE IF NOT EXISTS note_collections (
    note_id TEXT NOT NULL,
    collection_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (note_id, collection_id),
    FOREIGN KEY (note_id) REFERENCES notes(id) ON DELETE CASCADE,
    FOREIGN KEY (collection_id) REFERENCES collections(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_note_collections_collection_note
    ON note_collections(collection_id, note_id);

CREATE TABLE IF NOT EXISTS note_tags (
    note_id TEXT NOT NULL,
    tag_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (note_id, tag_id),
    FOREIGN KEY (note_id) REFERENCES notes(id) ON DELETE CASCADE,
    FOREIGN KEY (tag_id) REFERENCES tags(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_note_tags_tag_note
    ON note_tags(tag_id, note_id);

CREATE TABLE IF NOT EXISTS collection_default_tags (
    collection_id TEXT NOT NULL,
    tag_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (collection_id, tag_id),
    FOREIGN KEY (collection_id) REFERENCES collections(id) ON DELETE CASCADE,
    FOREIGN KEY (tag_id) REFERENCES tags(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_collection_default_tags_tag_collection
    ON collection_default_tags(tag_id, collection_id);
