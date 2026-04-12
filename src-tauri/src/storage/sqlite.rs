use rusqlite::{Connection, OptionalExtension};
use std::path::PathBuf;
use std::sync::Mutex;
use time::OffsetDateTime;

use super::models::Note;

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

        let exists: bool = conn
            .query_row("SELECT 1 FROM notes WHERE id = ?1", [id], |_| Ok(true))
            .optional()
            .map_err(|e| e.to_string())?
            .unwrap_or(false);

        if exists {
            conn.execute(
                "UPDATE notes SET body = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![body, now, id],
            )
            .map_err(|e| e.to_string())?;
        } else {
            conn.execute(
                "INSERT INTO notes (id, body, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![id, body, now, now],
            )
            .map_err(|e| e.to_string())?;
        }

        load_note(&conn, id)?.ok_or_else(|| "Note not found after save".to_string())
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

    pub fn delete_note(&self, id: &str) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        let changed = conn
            .execute("DELETE FROM notes WHERE id = ?1", [id])
            .map_err(|e| e.to_string())?;
        Ok(changed > 0)
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

fn now_iso() -> String {
    let now = OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .unwrap()
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
}
