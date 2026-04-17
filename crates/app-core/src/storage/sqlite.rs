use rusqlite::{Connection, OptionalExtension};
use std::path::PathBuf;
use std::sync::Mutex;
use time::OffsetDateTime;

use super::models::{Note, NoteSummary, Reminder};

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        let conn = Connection::open(&path).map_err(|e| format!("Failed to open DB: {e}"))?;

        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;",
        )
        .map_err(|e| format!("Failed to set pragmas: {e}"))?;

        let migration = include_str!("../../migrations/0001_init.sql");
        conn.execute_batch(migration)
            .map_err(|e| format!("Failed to run migration: {e}"))?;
        ensure_reminders_schema(&conn)?;
        ensure_ingest_schema(&conn)?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    #[allow(dead_code)]
    pub fn get_note(&self, id: &str) -> Result<Option<Note>, String> {
        let conn = self.conn.lock().unwrap();
        load_note(&conn, id)
    }

    pub fn save_note(&self, id: &str, body: &str) -> Result<Note, String> {
        let conn = self.conn.lock().unwrap();
        let now = now_iso();

        conn.execute(
            "INSERT INTO notes (id, body, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at",
            rusqlite::params![id, body, now, now],
        )
        .map_err(|e| e.to_string())?;

        load_note(&conn, id)?.ok_or_else(|| "Note not found after save".to_string())
    }

    pub fn append_note_body(&self, id: &str, body_suffix: &str) -> Result<Note, String> {
        let conn = self.conn.lock().unwrap();
        let now = now_iso();

        conn.execute(
            "INSERT INTO notes (id, body, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                 body = CASE
                     WHEN notes.body = '' THEN excluded.body
                     WHEN excluded.body = '' THEN notes.body
                     WHEN substr(notes.body, -1, 1) = char(10) THEN notes.body || excluded.body
                     ELSE notes.body || char(10) || excluded.body
                 END,
                 updated_at = excluded.updated_at",
            rusqlite::params![id, body_suffix, now, now],
        )
        .map_err(|e| e.to_string())?;

        load_note(&conn, id)?.ok_or_else(|| "Note not found after append".to_string())
    }

    pub fn append_note_with_ingest_event(
        &self,
        source: &str,
        message_id: Option<&str>,
        note_id: &str,
        body_suffix: &str,
        raw_payload: &[u8],
        body_truncated: bool,
        message_truncated: bool,
    ) -> Result<Option<Note>, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let now = now_iso();

        let inserted = tx
            .execute(
                "INSERT OR IGNORE INTO ingest_events (
                    source,
                    message_id,
                    note_id,
                    received_at,
                    raw_payload,
                    body_truncated,
                    message_truncated
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    source,
                    message_id,
                    note_id,
                    now,
                    raw_payload,
                    if body_truncated { 1 } else { 0 },
                    if message_truncated { 1 } else { 0 },
                ],
            )
            .map_err(|e| e.to_string())?;
        if message_id.is_some() && inserted == 0 {
            tx.commit().map_err(|e| e.to_string())?;
            return Ok(None);
        }

        tx.execute(
            "INSERT INTO notes (id, body, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                 body = CASE
                     WHEN notes.body = '' THEN excluded.body
                     WHEN excluded.body = '' THEN notes.body
                     WHEN substr(notes.body, -1, 1) = char(10) THEN notes.body || excluded.body
                     ELSE notes.body || char(10) || excluded.body
                 END,
                 updated_at = excluded.updated_at",
            rusqlite::params![note_id, body_suffix, now, now],
        )
        .map_err(|e| e.to_string())?;

        let mut stmt = tx
            .prepare("SELECT id, body, created_at, updated_at FROM notes WHERE id = ?1")
            .map_err(|e| e.to_string())?;
        let note = stmt
            .query_row([note_id], |row| {
                Ok(Note {
                    id: row.get(0)?,
                    body: row.get(1)?,
                    created_at: row.get(2)?,
                    updated_at: row.get(3)?,
                })
            })
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Note not found after append".to_string())?;
        drop(stmt);

        tx.commit().map_err(|e| e.to_string())?;
        Ok(Some(note))
    }

    pub fn get_most_recent_note(&self) -> Result<Option<Note>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, body, created_at, updated_at FROM notes ORDER BY updated_at DESC LIMIT 1",
            )
            .map_err(|e| e.to_string())?;

        let note = stmt
            .query_row([], |row| {
                Ok(Note {
                    id: row.get(0)?,
                    body: row.get(1)?,
                    created_at: row.get(2)?,
                    updated_at: row.get(3)?,
                })
            })
            .optional()
            .map_err(|e| e.to_string())?;

        Ok(note)
    }

    pub fn get_most_recent_note_excluding_prefix(
        &self,
        prefix: &str,
    ) -> Result<Option<Note>, String> {
        let trimmed = prefix.trim();
        if trimmed.is_empty() {
            return self.get_most_recent_note();
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, body, created_at, updated_at
                 FROM notes
                 WHERE id NOT LIKE ?1 ESCAPE '\\'
                 ORDER BY updated_at DESC
                 LIMIT 1",
            )
            .map_err(|e| e.to_string())?;

        let pattern = format!("{}-%", escape_like_pattern(trimmed));
        let note = stmt
            .query_row([pattern], |row| {
                Ok(Note {
                    id: row.get(0)?,
                    body: row.get(1)?,
                    created_at: row.get(2)?,
                    updated_at: row.get(3)?,
                })
            })
            .optional()
            .map_err(|e| e.to_string())?;

        Ok(note)
    }

    pub fn list_notes(&self) -> Result<Vec<Note>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, body, created_at, updated_at FROM notes ORDER BY updated_at DESC")
            .map_err(|e| e.to_string())?;

        let notes = stmt
            .query_map([], |row| {
                Ok(Note {
                    id: row.get(0)?,
                    body: row.get(1)?,
                    created_at: row.get(2)?,
                    updated_at: row.get(3)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        Ok(notes)
    }

    pub fn list_notes_meta(&self) -> Result<Vec<NoteSummary>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, substr(body, 1, 200) FROM notes ORDER BY updated_at DESC")
            .map_err(|e| e.to_string())?;

        let notes = stmt
            .query_map([], |row| {
                Ok(NoteSummary {
                    id: row.get(0)?,
                    body_prefix: row.get(1)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        Ok(notes)
    }

    pub fn delete_note(&self, id: &str) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        let changed = conn
            .execute("DELETE FROM notes WHERE id = ?1", [id])
            .map_err(|e| e.to_string())?;
        Ok(changed > 0)
    }

    pub fn list_reminders(&self, note_id: &str) -> Result<Vec<Reminder>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT note_id, line_number, remind_at_ms, display_at, line_text, notified_at_ms, created_at, updated_at
                 FROM reminders
                 WHERE note_id = ?1
                 ORDER BY line_number ASC",
            )
            .map_err(|e| e.to_string())?;

        let reminders = stmt
            .query_map([note_id], |row| {
                Ok(Reminder {
                    note_id: row.get(0)?,
                    line_number: row.get(1)?,
                    remind_at_ms: row.get(2)?,
                    display_at: row.get(3)?,
                    line_text: row.get(4)?,
                    notified_at_ms: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        Ok(reminders)
    }

    pub fn upsert_reminder(
        &self,
        note_id: &str,
        line_number: i64,
        remind_at_ms: i64,
        display_at: &str,
        line_text: &str,
    ) -> Result<Reminder, String> {
        let conn = self.conn.lock().unwrap();
        let now = now_iso();

        conn.execute(
            "INSERT INTO reminders (
                note_id, line_number, remind_at_ms, display_at, line_text, notified_at_ms, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7)
            ON CONFLICT(note_id, line_number) DO UPDATE SET
                remind_at_ms = excluded.remind_at_ms,
                display_at = excluded.display_at,
                line_text = excluded.line_text,
                notified_at_ms = NULL,
                updated_at = excluded.updated_at",
            rusqlite::params![
                note_id,
                line_number,
                remind_at_ms,
                display_at,
                line_text,
                now,
                now
            ],
        )
        .map_err(|e| e.to_string())?;

        load_reminder(&conn, note_id, line_number)?
            .ok_or_else(|| "Reminder not found after upsert".to_string())
    }

    pub fn mark_reminder_notified(
        &self,
        note_id: &str,
        line_number: i64,
        notified_at_ms: i64,
    ) -> Result<Option<Reminder>, String> {
        let conn = self.conn.lock().unwrap();
        let now = now_iso();
        conn.execute(
            "UPDATE reminders
             SET notified_at_ms = ?3, updated_at = ?4
             WHERE note_id = ?1 AND line_number = ?2",
            rusqlite::params![note_id, line_number, notified_at_ms, now],
        )
        .map_err(|e| e.to_string())?;

        load_reminder(&conn, note_id, line_number)
    }

    pub fn delete_reminder(&self, note_id: &str, line_number: i64) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        let changed = conn
            .execute(
                "DELETE FROM reminders WHERE note_id = ?1 AND line_number = ?2",
                rusqlite::params![note_id, line_number],
            )
            .map_err(|e| e.to_string())?;
        Ok(changed > 0)
    }

    pub fn move_reminder_line(
        &self,
        note_id: &str,
        from_line_number: i64,
        to_line_number: i64,
        line_text: &str,
    ) -> Result<bool, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let now = now_iso();

        let exists = tx
            .query_row(
                "SELECT 1 FROM reminders WHERE note_id = ?1 AND line_number = ?2",
                rusqlite::params![note_id, from_line_number],
                |_| Ok(()),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .is_some();
        if !exists {
            tx.commit().map_err(|e| e.to_string())?;
            return Ok(false);
        }

        if from_line_number != to_line_number {
            tx.execute(
                "DELETE FROM reminders WHERE note_id = ?1 AND line_number = ?2",
                rusqlite::params![note_id, to_line_number],
            )
            .map_err(|e| e.to_string())?;
        }

        let changed = tx
            .execute(
                "UPDATE reminders
                 SET line_number = ?3, line_text = ?4, updated_at = ?5
                 WHERE note_id = ?1 AND line_number = ?2",
                rusqlite::params![note_id, from_line_number, to_line_number, line_text, now],
            )
            .map_err(|e| e.to_string())?;

        tx.commit().map_err(|e| e.to_string())?;
        Ok(changed > 0)
    }

    pub fn has_ingest_message_id(&self, source: &str, message_id: &str) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT 1
                 FROM ingest_events
                 WHERE source = ?1 AND message_id = ?2
                 LIMIT 1",
            )
            .map_err(|e| e.to_string())?;
        let found = stmt
            .query_row(rusqlite::params![source, message_id], |_| Ok(()))
            .optional()
            .map_err(|e| e.to_string())?
            .is_some();
        Ok(found)
    }

    pub fn record_ingest_event(
        &self,
        source: &str,
        message_id: Option<&str>,
        note_id: &str,
        raw_payload: &[u8],
        body_truncated: bool,
        message_truncated: bool,
    ) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        let now = now_iso();

        let changed = conn
            .execute(
                "INSERT OR IGNORE INTO ingest_events (
                    source,
                    message_id,
                    note_id,
                    received_at,
                    raw_payload,
                    body_truncated,
                    message_truncated
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    source,
                    message_id,
                    note_id,
                    now,
                    raw_payload,
                    if body_truncated { 1 } else { 0 },
                    if message_truncated { 1 } else { 0 },
                ],
            )
            .map_err(|e| e.to_string())?;
        Ok(changed > 0)
    }

    pub fn get_ingest_offset(&self, source_key: &str) -> Result<Option<i64>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT last_uid
                 FROM ingest_offsets
                 WHERE source_key = ?1",
            )
            .map_err(|e| e.to_string())?;
        let offset = stmt
            .query_row([source_key], |row| row.get::<_, i64>(0))
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(offset)
    }

    pub fn set_ingest_offset(&self, source_key: &str, last_uid: i64) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let now = now_iso();
        conn.execute(
            "INSERT INTO ingest_offsets (source_key, last_uid, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(source_key) DO UPDATE SET
                 last_uid = excluded.last_uid,
                 updated_at = excluded.updated_at",
            rusqlite::params![source_key, last_uid, now],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn load_note(conn: &Connection, id: &str) -> Result<Option<Note>, String> {
    let mut stmt = conn
        .prepare("SELECT id, body, created_at, updated_at FROM notes WHERE id = ?1")
        .map_err(|e| e.to_string())?;

    let note = stmt
        .query_row([id], |row| {
            Ok(Note {
                id: row.get(0)?,
                body: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
            })
        })
        .optional()
        .map_err(|e| e.to_string())?;

    Ok(note)
}

fn load_reminder(
    conn: &Connection,
    note_id: &str,
    line_number: i64,
) -> Result<Option<Reminder>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT note_id, line_number, remind_at_ms, display_at, line_text, notified_at_ms, created_at, updated_at
             FROM reminders
             WHERE note_id = ?1 AND line_number = ?2",
        )
        .map_err(|e| e.to_string())?;

    let reminder = stmt
        .query_row(rusqlite::params![note_id, line_number], |row| {
            Ok(Reminder {
                note_id: row.get(0)?,
                line_number: row.get(1)?,
                remind_at_ms: row.get(2)?,
                display_at: row.get(3)?,
                line_text: row.get(4)?,
                notified_at_ms: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })
        .optional()
        .map_err(|e| e.to_string())?;

    Ok(reminder)
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| format!("Failed to inspect schema for {table}: {e}"))?;

    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let name: String = row.get(1).map_err(|e| e.to_string())?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn ensure_column(
    conn: &Connection,
    table: &str,
    column: &str,
    sql_def: &str,
) -> Result<(), String> {
    if has_column(conn, table, column)? {
        return Ok(());
    }
    conn.execute_batch(&format!(
        "ALTER TABLE {table} ADD COLUMN {column} {sql_def}"
    ))
    .map_err(|e| format!("Failed to add column {table}.{column}: {e}"))?;
    Ok(())
}

fn ensure_reminders_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS reminders (
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
        CREATE INDEX IF NOT EXISTS idx_reminders_due ON reminders(remind_at_ms);",
    )
    .map_err(|e| format!("Failed to ensure reminders schema: {e}"))?;

    // Legacy DBs may have an older reminders table shape. Add missing columns in-place.
    ensure_column(
        conn,
        "reminders",
        "remind_at_ms",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(conn, "reminders", "display_at", "TEXT NOT NULL DEFAULT ''")?;
    ensure_column(conn, "reminders", "line_text", "TEXT NOT NULL DEFAULT ''")?;
    ensure_column(conn, "reminders", "notified_at_ms", "INTEGER")?;
    ensure_column(conn, "reminders", "created_at", "TEXT NOT NULL DEFAULT ''")?;
    ensure_column(conn, "reminders", "updated_at", "TEXT NOT NULL DEFAULT ''")?;
    Ok(())
}

fn ensure_ingest_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS ingest_events (
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
        );",
    )
    .map_err(|e| format!("Failed to ensure ingest schema: {e}"))?;
    Ok(())
}

fn now_iso() -> String {
    let now = OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}

fn escape_like_pattern(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch == '\\' || ch == '%' || ch == '_' {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::thread;
    use std::time::Duration;

    fn temp_db_path() -> PathBuf {
        std::env::temp_dir().join(format!("note-test-{}.db", ulid::Ulid::new()))
    }

    #[test]
    fn save_update_and_delete_note() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        let note = db.save_note("n1", "hello").expect("note saved");
        assert_eq!(note.id, "n1");
        assert_eq!(note.body, "hello");

        let updated = db.save_note("n1", "updated").expect("note updated");
        assert_eq!(updated.id, "n1");
        assert_eq!(updated.body, "updated");
        assert!(updated.updated_at >= note.updated_at);

        assert_eq!(
            db.get_note("n1").expect("lookup succeeds").map(|n| n.body),
            Some("updated".to_string())
        );

        assert!(db.delete_note("n1").expect("delete succeeds"));
        assert!(!db.delete_note("n1").expect("second delete succeeds"));
        assert!(db.get_note("n1").expect("lookup succeeds").is_none());

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn list_notes_and_most_recent_follow_updated_at() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        db.save_note("a", "first").expect("save first");
        thread::sleep(Duration::from_millis(5));
        db.save_note("b", "second").expect("save second");
        thread::sleep(Duration::from_millis(5));
        db.save_note("a", "first updated").expect("update first");

        let listed = db.list_notes().expect("list succeeds");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, "a");
        assert_eq!(listed[1].id, "b");

        let most_recent = db
            .get_most_recent_note()
            .expect("most recent succeeds")
            .expect("note exists");
        assert_eq!(most_recent.id, "a");
        assert_eq!(most_recent.body, "first updated");

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn most_recent_note_can_skip_prefixed_special_notes() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        db.save_note("work-note", "normal").expect("save normal");
        thread::sleep(Duration::from_millis(5));
        db.save_note("inbox-email-2026-04-17", "special")
            .expect("save special");

        let most_recent = db
            .get_most_recent_note()
            .expect("most recent succeeds")
            .expect("note exists");
        assert_eq!(most_recent.id, "inbox-email-2026-04-17");

        let preferred = db
            .get_most_recent_note_excluding_prefix("inbox-email")
            .expect("most recent excluding prefix succeeds")
            .expect("note exists");
        assert_eq!(preferred.id, "work-note");

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn reminders_are_upserted_notified_and_deleted_with_note() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        db.save_note("n1", "hello").expect("save note");

        let first = db
            .upsert_reminder("n1", 3, 1_800_000_000_000, "14.01.2027. 10:00", "line 3")
            .expect("upsert reminder");
        assert_eq!(first.line_number, 3);
        assert_eq!(first.remind_at_ms, 1_800_000_000_000);
        assert!(first.notified_at_ms.is_none());

        let updated = db
            .upsert_reminder(
                "n1",
                3,
                1_900_000_000_000,
                "13.03.2030. 10:00",
                "line 3 changed",
            )
            .expect("upsert reminder update");
        assert_eq!(updated.line_number, 3);
        assert_eq!(updated.remind_at_ms, 1_900_000_000_000);
        assert_eq!(updated.display_at, "13.03.2030. 10:00");
        assert_eq!(updated.line_text, "line 3 changed");
        assert!(updated.notified_at_ms.is_none());

        let list = db.list_reminders("n1").expect("list reminders");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].line_number, 3);

        let notified = db
            .mark_reminder_notified("n1", 3, 1_900_000_100_000)
            .expect("mark notified")
            .expect("reminder exists");
        assert_eq!(notified.notified_at_ms, Some(1_900_000_100_000));

        assert!(db
            .move_reminder_line("n1", 3, 5, "line 5 changed")
            .expect("move reminder"));
        let after_move = db.list_reminders("n1").expect("list reminders after move");
        assert_eq!(after_move.len(), 1);
        assert_eq!(after_move[0].line_number, 5);
        assert_eq!(after_move[0].line_text, "line 5 changed");
        assert_eq!(after_move[0].notified_at_ms, Some(1_900_000_100_000));

        assert!(!db
            .move_reminder_line("n1", 3, 9, "missing")
            .expect("move missing reminder"));

        assert!(db.delete_reminder("n1", 5).expect("delete reminder"));
        assert!(!db.delete_reminder("n1", 5).expect("second delete reminder"));

        let after_line_delete = db
            .list_reminders("n1")
            .expect("list reminders after line delete");
        assert!(after_line_delete.is_empty());

        db.delete_note("n1").expect("delete note");
        let after_delete = db
            .list_reminders("n1")
            .expect("list reminders after delete");
        assert!(after_delete.is_empty());

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn open_upgrades_legacy_reminders_schema() {
        let path = temp_db_path();
        let conn = Connection::open(path.clone()).expect("legacy db opens");
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE notes (
                id TEXT PRIMARY KEY,
                body TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
             );
             CREATE TABLE reminders (
                note_id TEXT NOT NULL,
                line_number INTEGER NOT NULL CHECK(line_number > 0),
                remind_at_ms INTEGER NOT NULL,
                display_at TEXT NOT NULL,
                notified_at_ms INTEGER,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (note_id, line_number),
                FOREIGN KEY (note_id) REFERENCES notes(id) ON DELETE CASCADE
             );",
        )
        .expect("legacy schema created");
        drop(conn);

        let db = Db::open(path.clone()).expect("db opens with upgrade");
        db.save_note("n1", "hello").expect("note saved");
        let reminder = db
            .upsert_reminder("n1", 1, 1_900_000_000_000, "13.03.2030. 10:00", "line 1")
            .expect("upsert reminder works");
        assert_eq!(reminder.line_text, "line 1");

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn append_note_body_creates_and_appends_with_newline_boundary() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        let created = db
            .append_note_body("n1", "first block")
            .expect("append creates note");
        assert_eq!(created.body, "first block");

        let appended = db
            .append_note_body("n1", "second block")
            .expect("append updates note");
        assert_eq!(appended.body, "first block\nsecond block");

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn ingest_event_dedup_uses_source_and_message_id() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        let inserted = db
            .record_ingest_event(
                "smtp",
                Some("<a@b>"),
                "inbox-email-2026-04-17",
                b"raw",
                false,
                false,
            )
            .expect("inserted");
        assert!(inserted);
        assert!(db
            .has_ingest_message_id("smtp", "<a@b>")
            .expect("dedup lookup"));

        let duplicate = db
            .record_ingest_event(
                "smtp",
                Some("<a@b>"),
                "inbox-email-2026-04-17",
                b"raw-duplicate",
                false,
                false,
            )
            .expect("duplicate insert checked");
        assert!(!duplicate);

        let other_source = db
            .record_ingest_event(
                "imap",
                Some("<a@b>"),
                "inbox-email-2026-04-17",
                b"raw-imap",
                false,
                false,
            )
            .expect("other source insert");
        assert!(other_source);

        let no_message_id_1 = db
            .record_ingest_event(
                "smtp",
                None,
                "inbox-email-2026-04-17",
                b"raw-1",
                false,
                false,
            )
            .expect("no message id insert 1");
        let no_message_id_2 = db
            .record_ingest_event(
                "smtp",
                None,
                "inbox-email-2026-04-17",
                b"raw-2",
                false,
                false,
            )
            .expect("no message id insert 2");
        assert!(no_message_id_1);
        assert!(no_message_id_2);

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn append_note_with_ingest_event_is_atomic_and_dedups() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        let first = db
            .append_note_with_ingest_event(
                "smtp",
                Some("<abc@id>"),
                "inbox-email-2026-04-17",
                "# hello",
                b"raw message",
                false,
                false,
            )
            .expect("first ingest succeeds")
            .expect("note appended");
        assert_eq!(first.body, "# hello");

        let duplicate = db
            .append_note_with_ingest_event(
                "smtp",
                Some("<abc@id>"),
                "inbox-email-2026-04-17",
                "# duplicate",
                b"raw duplicate",
                false,
                false,
            )
            .expect("duplicate ingest checked");
        assert!(duplicate.is_none());

        let note = db
            .get_note("inbox-email-2026-04-17")
            .expect("note lookup")
            .expect("note present");
        assert_eq!(note.body, "# hello");

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn ingest_offsets_can_be_set_and_read() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        assert!(db
            .get_ingest_offset("imap:example:user:inbox")
            .expect("lookup")
            .is_none());

        db.set_ingest_offset("imap:example:user:inbox", 42)
            .expect("set offset");
        assert_eq!(
            db.get_ingest_offset("imap:example:user:inbox")
                .expect("lookup"),
            Some(42)
        );

        db.set_ingest_offset("imap:example:user:inbox", 77)
            .expect("update offset");
        assert_eq!(
            db.get_ingest_offset("imap:example:user:inbox")
                .expect("lookup"),
            Some(77)
        );

        drop(db);
        let _ = fs::remove_file(path);
    }
}
