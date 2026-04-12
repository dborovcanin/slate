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

    pub fn get_note(&self, id: &str) -> Result<Option<Note>, String> {
        let conn = self.conn.lock().unwrap();
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

        self.get_note(id)?
            .ok_or_else(|| "Note not found after save".to_string())
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

fn now_iso() -> String {
    let now = OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}
