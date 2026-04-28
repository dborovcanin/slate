use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use pbkdf2::pbkdf2_hmac;
use rusqlite::{Connection, OptionalExtension};
use sha2::Sha256;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use subtle::ConstantTimeEq;
use time::OffsetDateTime;

use super::models::{Note, NoteAccessMode, NoteModules, NoteSearchResult, NoteSummary, Reminder};
use super::note_access::{NoteAccessGrant, NoteAccessService};

const DEFAULT_NOTE_MODULES_JSON: &str =
    r#"{"math":true,"table":true,"variables":true,"style":true}"#;
const PASSWORD_SALT_LEN: usize = 16;
const ENCRYPTION_SALT_LEN: usize = 16;
const ENCRYPTION_NONCE_LEN: usize = 12;
const PASSWORD_HASH_LEN: usize = 32;
const PBKDF2_ITERATIONS: u32 = 200_000;
const NOTE_TITLE_MAX_CHARS: usize = 60;
const SEARCH_QUERY_MAX_TERMS: usize = 8;
const SEARCH_LIMIT_MAX: usize = 100;
const LEGACY_MARKDOWN_FILE_NOTE_ID_SQL_PREFIX: &str = "mdfile:%";

#[derive(Debug, Clone)]
struct NoteSecurityRow {
    access_mode: NoteAccessMode,
    password_salt: Option<Vec<u8>>,
    password_hash: Option<Vec<u8>>,
    encryption_salt: Option<Vec<u8>>,
    encryption_nonce: Option<Vec<u8>>,
    encrypted_body: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
struct UnlockedEncryptedNote {
    key: [u8; 32],
    encryption_salt: Vec<u8>,
}

const SQLITE_POOL_SIZE: usize = 4;

struct SqlitePool {
    connections: Mutex<Vec<Connection>>,
    available: Condvar,
}

struct SqlitePoolGuard<'a> {
    pool: &'a SqlitePool,
    connection: Option<Connection>,
}

impl SqlitePool {
    fn configure_connection(conn: &Connection) -> Result<(), String> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;",
        )
        .map_err(|e| format!("Failed to set pragmas: {e}"))
    }

    fn new(path: &Path, pool_size: usize) -> Result<Self, String> {
        let first = Connection::open(path).map_err(|e| format!("Failed to open DB: {e}"))?;
        Self::configure_connection(&first)?;

        let schema = include_str!("../../migrations/0001_init.sql");
        first
            .execute_batch(schema)
            .map_err(|e| format!("Failed to initialize schema: {e}"))?;
        apply_pending_migrations(&first)?;
        seed_note_search_index_if_empty(&first)?;
        check_and_heal_search_index(&first)?;

        let mut connections = Vec::with_capacity(pool_size.max(1));
        connections.push(first);
        for _ in 1..pool_size.max(1) {
            let conn = Connection::open(path).map_err(|e| format!("Failed to open DB: {e}"))?;
            Self::configure_connection(&conn)?;
            connections.push(conn);
        }
        Ok(Self {
            connections: Mutex::new(connections),
            available: Condvar::new(),
        })
    }

    fn lock(&self) -> Result<SqlitePoolGuard<'_>, String> {
        let mut guard = self
            .connections
            .lock()
            .map_err(|_| "db pool lock poisoned".to_string())?;
        loop {
            if let Some(connection) = guard.pop() {
                return Ok(SqlitePoolGuard {
                    pool: self,
                    connection: Some(connection),
                });
            }
            guard = self
                .available
                .wait(guard)
                .map_err(|_| "db pool lock poisoned".to_string())?;
        }
    }
}

impl Deref for SqlitePoolGuard<'_> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.connection
            .as_ref()
            .expect("sqlite pool guard missing connection")
    }
}

impl DerefMut for SqlitePoolGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.connection
            .as_mut()
            .expect("sqlite pool guard missing connection")
    }
}

impl Drop for SqlitePoolGuard<'_> {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            if let Ok(mut guard) = self.pool.connections.lock() {
                guard.push(connection);
                self.pool.available.notify_one();
            }
        }
    }
}

pub struct Db {
    conn: Arc<SqlitePool>,
    note_access: Arc<NoteAccessService>,
}

impl Clone for Db {
    fn clone(&self) -> Self {
        Self {
            conn: Arc::clone(&self.conn),
            note_access: Arc::clone(&self.note_access),
        }
    }
}

impl Db {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        let conn = SqlitePool::new(&path, SQLITE_POOL_SIZE)?;

        Ok(Self {
            conn: Arc::new(conn),
            note_access: Arc::new(NoteAccessService::new()),
        })
    }

    #[allow(dead_code)]
    pub fn get_note(&self, id: &str) -> Result<Option<Note>, String> {
        let conn = self.conn.lock().unwrap();
        self.load_note_with_access(&conn, id)
    }

    pub fn save_note(&self, id: &str, body: &str) -> Result<Note, String> {
        let conn = self.conn.lock().unwrap();
        let now = now_iso();
        let note_title = derive_note_title_from_body(body);

        if let Some(security) = self.load_note_security(&conn, id)? {
            match security.access_mode {
                NoteAccessMode::None => {}
                NoteAccessMode::Locked => {
                    if !self.is_note_unlocked(id) {
                        return Err("note is locked; unlock first".to_string());
                    }
                }
                NoteAccessMode::Encrypted => {
                    let persisted = self
                        .load_note_persistence_row(&conn, id)?
                        .ok_or_else(|| "Note not found".to_string())?;
                    let encryption = self
                        .unlocked_encryption_for(id)
                        .ok_or_else(|| "note is locked; unlock first".to_string())?;
                    let encrypted = encrypt_note_body_with_key(body, &encryption.key)?;
                    conn.execute(
                        "UPDATE notes
                         SET body = '',
                             note_title = ?2,
                             encrypted_body = ?3,
                             encryption_salt = ?4,
                             encryption_nonce = ?5,
                             updated_at = ?6
                         WHERE id = ?1",
                        rusqlite::params![
                            id,
                            note_title,
                            encrypted.ciphertext,
                            encryption.encryption_salt,
                            encrypted.nonce,
                            &now,
                        ],
                    )
                    .map_err(|e| e.to_string())?;
                    return Ok(Note {
                        id: id.to_string(),
                        body: body.to_string(),
                        modules: parse_note_modules_json(persisted.modules_json),
                        access_mode: NoteAccessMode::Encrypted,
                        is_unlocked: true,
                        created_at: persisted.created_at,
                        updated_at: now,
                    });
                }
            }
        }

        conn.execute(
            "INSERT INTO notes (id, body, note_title, modules_json, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                 body = excluded.body,
                 note_title = excluded.note_title,
                 updated_at = excluded.updated_at",
            rusqlite::params![id, body, note_title, DEFAULT_NOTE_MODULES_JSON, now, now],
        )
        .map_err(|e| e.to_string())?;

        self.load_note_with_access(&conn, id)?
            .ok_or_else(|| "Note not found after save".to_string())
    }

    pub fn create_note_with_defaults(
        &self,
        id: &str,
        modules: NoteModules,
        default_encryption_password: Option<&str>,
    ) -> Result<Note, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let now = now_iso();
        let note_title = derive_note_title_from_body("");
        let modules_json = serde_json::to_string(&modules)
            .map_err(|e| format!("Failed to encode note modules: {e}"))?;

        let mut unlocked_encryption: Option<UnlockedEncryptedNote> = None;
        if let Some(raw_password) = default_encryption_password {
            let password = normalize_password(raw_password)?;
            let mut encryption_salt = [0u8; ENCRYPTION_SALT_LEN];
            fill_random_bytes(&mut encryption_salt)?;
            let encryption_key = derive_encryption_key(&password, &encryption_salt);
            let encrypted = encrypt_note_body_with_key("", &encryption_key)?;
            let (password_salt, password_hash) = password_hash_pair(&password)?;
            tx.execute(
                "INSERT INTO notes (
                    id,
                    body,
                    note_title,
                    modules_json,
                    access_mode,
                    password_salt,
                    password_hash,
                    encryption_salt,
                    encryption_nonce,
                    encrypted_body,
                    created_at,
                    updated_at
                ) VALUES (?1, '', ?2, ?3, 'encrypted', ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    id,
                    note_title,
                    modules_json,
                    password_salt,
                    password_hash,
                    encryption_salt.to_vec(),
                    encrypted.nonce,
                    encrypted.ciphertext,
                    now,
                    now,
                ],
            )
            .map_err(|e| e.to_string())?;
            unlocked_encryption = Some(UnlockedEncryptedNote {
                key: encryption_key,
                encryption_salt: encryption_salt.to_vec(),
            });
        } else {
            tx.execute(
                "INSERT INTO notes (id, body, note_title, modules_json, created_at, updated_at)
                 VALUES (?1, '', ?2, ?3, ?4, ?5)",
                rusqlite::params![id, note_title, modules_json, now, now],
            )
            .map_err(|e| e.to_string())?;
        }

        tx.commit().map_err(|e| e.to_string())?;
        if let Some(session) = unlocked_encryption {
            self.note_access
                .unlock_encrypted(id, session.key, &session.encryption_salt);
        }
        self.load_note_with_access(&conn, id)?
            .ok_or_else(|| "Note not found after create".to_string())
    }

    pub fn set_note_modules(&self, id: &str, modules: NoteModules) -> Result<Note, String> {
        let conn = self.conn.lock().unwrap();
        let now = now_iso();
        let modules_json = serde_json::to_string(&modules)
            .map_err(|e| format!("Failed to encode note modules: {e}"))?;

        let changed = conn
            .execute(
                "UPDATE notes
                 SET modules_json = ?2, updated_at = ?3
                 WHERE id = ?1",
                rusqlite::params![id, modules_json, now],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err("Note not found".to_string());
        }

        self.load_note_with_access(&conn, id)?
            .ok_or_else(|| "Note not found after module update".to_string())
    }

    pub fn lock_note(&self, id: &str, password: &str) -> Result<Note, String> {
        let password = normalize_password(password)?;
        let conn = self.conn.lock().unwrap();
        let security = self
            .load_note_security(&conn, id)?
            .ok_or_else(|| "Note not found".to_string())?;
        if is_note_protected(security.access_mode) && !self.is_note_unlocked(id) {
            return Err("note is locked; unlock first".to_string());
        }
        let body = self
            .load_note_plain_body_for_access(&conn, id, &security)?
            .ok_or_else(|| "Note not found".to_string())?;
        let note_title = derive_note_title_from_body(&body);
        let now = now_iso();
        let (password_salt, password_hash) = password_hash_pair(&password)?;

        conn.execute(
            "UPDATE notes
             SET body = ?2,
                 note_title = ?3,
                 access_mode = 'locked',
                 password_salt = ?4,
                 password_hash = ?5,
                 encryption_salt = NULL,
                 encryption_nonce = NULL,
                 encrypted_body = NULL,
                 updated_at = ?6
             WHERE id = ?1",
            rusqlite::params![id, body, note_title, password_salt, password_hash, now],
        )
        .map_err(|e| e.to_string())?;

        self.note_access.clear(id);
        self.load_note_with_access(&conn, id)?
            .ok_or_else(|| "Note not found after lock".to_string())
    }

    pub fn unlock_note(&self, id: &str, password: &str) -> Result<Note, String> {
        let password = normalize_password(password)?;
        let conn = self.conn.lock().unwrap();
        let security = self
            .load_note_security(&conn, id)?
            .ok_or_else(|| "Note not found".to_string())?;
        if security.access_mode == NoteAccessMode::None {
            return self
                .load_note_with_access(&conn, id)?
                .ok_or_else(|| "Note not found".to_string());
        }
        verify_password(&security, &password)?;
        match security.access_mode {
            NoteAccessMode::None => {}
            NoteAccessMode::Locked => {
                self.note_access.unlock_locked(id);
            }
            NoteAccessMode::Encrypted => {
                let encryption_salt = security
                    .encryption_salt
                    .as_ref()
                    .ok_or_else(|| "encrypted note salt missing".to_string())?;
                let encryption_key = derive_encryption_key(&password, encryption_salt);
                self.note_access
                    .unlock_encrypted(id, encryption_key, encryption_salt);
            }
        }
        match self.load_note_with_access(&conn, id) {
            Ok(Some(note)) => Ok(note),
            Ok(None) => Err("Note not found after unlock".to_string()),
            Err(error) => {
                self.note_access.clear(id);
                Err(error)
            }
        }
    }

    pub fn encrypt_note(&self, id: &str, password: &str) -> Result<Note, String> {
        let password = normalize_password(password)?;
        let conn = self.conn.lock().unwrap();
        let security = self
            .load_note_security(&conn, id)?
            .ok_or_else(|| "Note not found".to_string())?;
        if is_note_protected(security.access_mode) && !self.is_note_unlocked(id) {
            return Err("note is locked; unlock first".to_string());
        }
        let body = self
            .load_note_plain_body_for_access(&conn, id, &security)?
            .ok_or_else(|| "Note not found".to_string())?;
        let note_title = derive_note_title_from_body(&body);
        let mut encryption_salt = [0u8; ENCRYPTION_SALT_LEN];
        fill_random_bytes(&mut encryption_salt)?;
        let encryption_key = derive_encryption_key(&password, &encryption_salt);
        let encrypted = encrypt_note_body_with_key(&body, &encryption_key)?;
        let (password_salt, password_hash) = password_hash_pair(&password)?;
        let now = now_iso();

        conn.execute(
            "UPDATE notes
             SET body = '',
                 note_title = ?2,
                 access_mode = 'encrypted',
                 password_salt = ?3,
                 password_hash = ?4,
                 encryption_salt = ?5,
                 encryption_nonce = ?6,
                 encrypted_body = ?7,
                 updated_at = ?8
             WHERE id = ?1",
            rusqlite::params![
                id,
                note_title,
                password_salt,
                password_hash,
                encryption_salt.to_vec(),
                encrypted.nonce,
                encrypted.ciphertext,
                now
            ],
        )
        .map_err(|e| e.to_string())?;

        // Keep the just-encrypted note open in this session so autosave continues
        // to write encrypted-at-rest payloads with the same unlocked key.
        self.note_access
            .unlock_encrypted(id, encryption_key, &encryption_salt);
        match self.load_note_with_access(&conn, id) {
            Ok(Some(note)) => Ok(note),
            Ok(None) => {
                self.note_access.clear(id);
                Err("Note not found after encrypt".to_string())
            }
            Err(error) => {
                self.note_access.clear(id);
                Err(error)
            }
        }
    }

    pub fn decrypt_note(&self, id: &str, password: &str) -> Result<Note, String> {
        let password = normalize_password(password)?;
        let conn = self.conn.lock().unwrap();
        let security = self
            .load_note_security(&conn, id)?
            .ok_or_else(|| "Note not found".to_string())?;
        if security.access_mode == NoteAccessMode::None {
            return self
                .load_note_with_access(&conn, id)?
                .ok_or_else(|| "Note not found".to_string());
        }
        verify_password(&security, &password)?;
        let body = self
            .load_note_plain_body_for_access_with_password(&conn, id, &security, &password)?
            .ok_or_else(|| "Note not found".to_string())?;
        let note_title = derive_note_title_from_body(&body);
        let now = now_iso();
        conn.execute(
            "UPDATE notes
             SET body = ?2,
                 note_title = ?3,
                 access_mode = 'none',
                 password_salt = NULL,
                 password_hash = NULL,
                 encryption_salt = NULL,
                 encryption_nonce = NULL,
                 encrypted_body = NULL,
                 updated_at = ?4
             WHERE id = ?1",
            rusqlite::params![id, body, note_title, now],
        )
        .map_err(|e| e.to_string())?;
        self.note_access.clear(id);
        self.load_note_with_access(&conn, id)?
            .ok_or_else(|| "Note not found after decrypt".to_string())
    }

    pub fn append_note_body(&self, id: &str, body_suffix: &str) -> Result<Note, String> {
        let conn = self.conn.lock().unwrap();
        let now = now_iso();

        let security = self.load_note_security(&conn, id)?;
        if let Some(security) = security {
            match security.access_mode {
                NoteAccessMode::None => {}
                NoteAccessMode::Locked => {
                    if !self.is_note_unlocked(id) {
                        return Err("note is locked; unlock first".to_string());
                    }
                }
                NoteAccessMode::Encrypted => {
                    let persisted = self
                        .load_note_persistence_row(&conn, id)?
                        .ok_or_else(|| "Note not found".to_string())?;
                    let encryption = self
                        .unlocked_encryption_for(id)
                        .ok_or_else(|| "note is locked; unlock first".to_string())?;
                    let current = self
                        .load_note_plain_body_for_access(&conn, id, &security)?
                        .unwrap_or_default();
                    let next_body = if current.is_empty() {
                        body_suffix.to_string()
                    } else if body_suffix.is_empty() {
                        current
                    } else if current.ends_with('\n') {
                        format!("{current}{body_suffix}")
                    } else {
                        format!("{current}\n{body_suffix}")
                    };
                    let note_title = derive_note_title_from_body(&next_body);
                    let encrypted = encrypt_note_body_with_key(&next_body, &encryption.key)?;
                    conn.execute(
                        "UPDATE notes
                         SET body = '',
                             note_title = ?2,
                             encrypted_body = ?3,
                             encryption_salt = ?4,
                             encryption_nonce = ?5,
                             updated_at = ?6
                         WHERE id = ?1",
                        rusqlite::params![
                            id,
                            note_title,
                            encrypted.ciphertext,
                            encryption.encryption_salt,
                            encrypted.nonce,
                            &now,
                        ],
                    )
                    .map_err(|e| e.to_string())?;
                    return Ok(Note {
                        id: id.to_string(),
                        body: next_body,
                        modules: parse_note_modules_json(persisted.modules_json),
                        access_mode: NoteAccessMode::Encrypted,
                        is_unlocked: true,
                        created_at: persisted.created_at,
                        updated_at: now,
                    });
                }
            }
        }

        let current = self
            .load_note_row(&conn, id)?
            .map(|row| row.body)
            .unwrap_or_default();
        let next_body = if current.is_empty() {
            body_suffix.to_string()
        } else if body_suffix.is_empty() {
            current
        } else if current.ends_with('\n') {
            format!("{current}{body_suffix}")
        } else {
            format!("{current}\n{body_suffix}")
        };
        let note_title = derive_note_title_from_body(&next_body);

        conn.execute(
            "INSERT INTO notes (id, body, note_title, modules_json, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                 body = excluded.body,
                 note_title = excluded.note_title,
                 updated_at = excluded.updated_at",
            rusqlite::params![
                id,
                next_body,
                note_title,
                DEFAULT_NOTE_MODULES_JSON,
                now,
                now
            ],
        )
        .map_err(|e| e.to_string())?;

        self.load_note_with_access(&conn, id)?
            .ok_or_else(|| "Note not found after append".to_string())
    }

    pub fn prepend_note_with_ingest_event(
        &self,
        source: &str,
        message_id: Option<&str>,
        note_id: &str,
        body_prefix: &str,
        raw_payload: &[u8],
        body_truncated: bool,
        message_truncated: bool,
    ) -> Result<Option<Note>, String> {
        self.write_note_with_ingest_event(
            source,
            message_id,
            note_id,
            body_prefix,
            raw_payload,
            body_truncated,
            message_truncated,
        )
    }

    fn write_note_with_ingest_event(
        &self,
        source: &str,
        message_id: Option<&str>,
        note_id: &str,
        body: &str,
        raw_payload: &[u8],
        body_truncated: bool,
        message_truncated: bool,
    ) -> Result<Option<Note>, String> {
        let mut conn = self.conn.lock().unwrap();
        if let Some(security) = self.load_note_security(&conn, note_id)? {
            if security.access_mode != NoteAccessMode::None {
                return Err("cannot ingest into protected note".to_string());
            }
        }
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

        let existing_body = tx
            .query_row("SELECT body FROM notes WHERE id = ?1", [note_id], |row| {
                row.get::<_, String>(0)
            })
            .optional()
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        let next_body = if existing_body.is_empty() {
            body.to_string()
        } else if body.is_empty() {
            existing_body
        } else if body.ends_with('\n') {
            format!("{body}{existing_body}")
        } else {
            format!("{body}\n{existing_body}")
        };
        let note_title = derive_note_title_from_body(&next_body);

        tx.execute(
            "INSERT INTO notes (id, body, note_title, modules_json, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                 body = excluded.body,
                 note_title = excluded.note_title,
                 updated_at = excluded.updated_at",
            rusqlite::params![
                note_id,
                next_body,
                note_title,
                DEFAULT_NOTE_MODULES_JSON,
                now,
                now
            ],
        )
        .map_err(|e| e.to_string())?;

        tx.commit().map_err(|e| e.to_string())?;
        let note = self
            .load_note_with_access(&conn, note_id)?
            .ok_or_else(|| "Note not found after write".to_string())?;
        Ok(Some(note))
    }

    pub fn get_most_recent_note(&self) -> Result<Option<Note>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id FROM notes ORDER BY updated_at DESC LIMIT 1")
            .map_err(|e| e.to_string())?;

        let id = stmt
            .query_row([], |row| row.get::<_, String>(0))
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(id) = id else {
            return Ok(None);
        };
        self.load_note_with_access(&conn, &id)
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
                "SELECT id
                 FROM notes
                 WHERE id NOT LIKE ?1 ESCAPE '\\'
                 ORDER BY updated_at DESC
                 LIMIT 1",
            )
            .map_err(|e| e.to_string())?;

        let pattern = format!("{}-%", escape_like_pattern(trimmed));
        let id = stmt
            .query_row([pattern], |row| row.get::<_, String>(0))
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(id) = id else {
            return Ok(None);
        };
        self.load_note_with_access(&conn, &id)
    }

    pub fn list_notes(&self) -> Result<Vec<Note>, String> {
        let conn = self.conn.lock().unwrap();
        let rows = self.load_note_access_rows(&conn)?;
        let mut notes = Vec::with_capacity(rows.len());
        for row in &rows {
            notes.push(self.note_from_access_row(row)?);
        }
        Ok(notes)
    }

    pub fn list_notes_meta(&self) -> Result<Vec<NoteSummary>, String> {
        let conn = self.conn.lock().unwrap();
        let rows = self.load_note_summary_rows(&conn)?;
        let mut notes = Vec::with_capacity(rows.len());
        for row in &rows {
            notes.push(self.note_summary_from_row(&conn, row)?);
        }

        Ok(notes)
    }

    pub fn get_note_meta(&self, id: &str) -> Result<Option<NoteSummary>, String> {
        let conn = self.conn.lock().unwrap();
        let Some(row) = self.load_note_summary_row(&conn, id)? else {
            return Ok(None);
        };
        self.note_summary_from_row(&conn, &row).map(Some)
    }

    pub fn get_note_updated_at(&self, id: &str) -> Result<Option<String>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT updated_at
                 FROM notes
                 WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let updated_at = stmt
            .query_row([id], |row| row.get::<_, String>(0))
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(updated_at)
    }

    pub fn search_notes_content(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<NoteSearchResult>, String> {
        let search_terms = parse_search_terms(query);
        let fts_query = build_fts_query_from_terms(&search_terms);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }
        let bounded_limit = limit.clamp(1, SEARCH_LIMIT_MAX) as i64;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT n.id, n.note_title, n.body,
                        snippet(notes_fts, 2, '[[', ']]', '…', 16),
                        bm25(notes_fts, 0.0, 10.0, 1.0),
                        n.updated_at
                 FROM notes_fts
                 JOIN notes n ON n.rowid = notes_fts.rowid
                 WHERE notes_fts MATCH ?1
                   AND n.access_mode = 'none'
                 ORDER BY bm25(notes_fts, 0.0, 10.0, 1.0), n.updated_at DESC
                 LIMIT ?2",
            )
            .map_err(|e| e.to_string())?;
        let results = stmt
            .query_map(rusqlite::params![fts_query, bounded_limit], |row| {
                let body = row.get::<_, Option<String>>(2)?.unwrap_or_default();
                let snippet = row.get::<_, Option<String>>(3)?.unwrap_or_default();
                Ok(NoteSearchResult {
                    id: row.get(0)?,
                    title: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    line_number: search_result_line_number(&body, &snippet, &search_terms),
                    snippet,
                    rank: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(results)
    }

    pub fn rebuild_note_search_index(&self) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        rebuild_note_search_index_inner(&conn)
    }

    pub fn delete_note(&self, id: &str, password: Option<&str>) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        if let Some(security) = self.load_note_security(&conn, id)? {
            if is_note_protected(security.access_mode) {
                let provided = password.ok_or_else(|| {
                    "password required to delete locked/encrypted note".to_string()
                })?;
                let normalized = normalize_password(provided)?;
                verify_password(&security, &normalized)?;
            }
        }
        let changed = conn
            .execute("DELETE FROM notes WHERE id = ?1", [id])
            .map_err(|e| e.to_string())?;
        if changed > 0 {
            self.note_access.clear(id);
        }
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

    fn is_note_unlocked(&self, id: &str) -> bool {
        self.note_access.is_unlocked(id)
    }

    fn unlocked_encryption_for(&self, id: &str) -> Option<UnlockedEncryptedNote> {
        match self.note_access.session(id)? {
            NoteAccessGrant::Encrypted {
                key,
                encryption_salt,
            } => Some(UnlockedEncryptedNote {
                key,
                encryption_salt,
            }),
            NoteAccessGrant::Locked => None,
        }
    }

    fn load_note_security(
        &self,
        conn: &Connection,
        id: &str,
    ) -> Result<Option<NoteSecurityRow>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT access_mode, password_salt, password_hash, encryption_salt, encryption_nonce, encrypted_body
                 FROM notes WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let row = stmt
            .query_row([id], |row| {
                Ok(NoteSecurityRow {
                    access_mode: parse_note_access_mode(row.get::<_, Option<String>>(0)?),
                    password_salt: row.get(1)?,
                    password_hash: row.get(2)?,
                    encryption_salt: row.get(3)?,
                    encryption_nonce: row.get(4)?,
                    encrypted_body: row.get(5)?,
                })
            })
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(row)
    }

    fn load_note_row(&self, conn: &Connection, id: &str) -> Result<Option<NoteRow>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT body
                 FROM notes
                 WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let note = stmt
            .query_row([id], |row| Ok(NoteRow { body: row.get(0)? }))
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(note)
    }

    fn load_note_persistence_row(
        &self,
        conn: &Connection,
        id: &str,
    ) -> Result<Option<NotePersistenceRow>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT modules_json, created_at
                 FROM notes
                 WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let row = stmt
            .query_row([id], |row| {
                Ok(NotePersistenceRow {
                    modules_json: row.get(0)?,
                    created_at: row.get(1)?,
                })
            })
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(row)
    }

    fn load_note_summary_row(
        &self,
        conn: &Connection,
        id: &str,
    ) -> Result<Option<NoteSummaryRow>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, note_title, substr(body, 1, 200), access_mode, updated_at
                 FROM notes
                 WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let row = stmt
            .query_row([id], |row| {
                Ok(NoteSummaryRow {
                    id: row.get(0)?,
                    note_title: row.get(1)?,
                    body_prefix: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    access_mode: parse_note_access_mode(row.get::<_, Option<String>>(3)?),
                    updated_at: row.get(4)?,
                })
            })
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(row)
    }

    fn load_note_summary_rows(&self, conn: &Connection) -> Result<Vec<NoteSummaryRow>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, note_title, substr(body, 1, 200), access_mode, updated_at
                 FROM notes
                 ORDER BY updated_at DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(NoteSummaryRow {
                    id: row.get(0)?,
                    note_title: row.get(1)?,
                    body_prefix: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    access_mode: parse_note_access_mode(row.get::<_, Option<String>>(3)?),
                    updated_at: row.get(4)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    }

    fn load_note_encrypted_payload_row(
        &self,
        conn: &Connection,
        id: &str,
    ) -> Result<Option<NoteEncryptedPayloadRow>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT encryption_salt, encryption_nonce, encrypted_body
                 FROM notes
                 WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let row = stmt
            .query_row([id], |row| {
                Ok(NoteEncryptedPayloadRow {
                    encryption_salt: row.get(0)?,
                    encryption_nonce: row.get(1)?,
                    encrypted_body: row.get(2)?,
                })
            })
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(row)
    }

    fn load_note_access_row(
        &self,
        conn: &Connection,
        id: &str,
    ) -> Result<Option<NoteAccessRow>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, body, modules_json, access_mode,
                        encryption_salt, encryption_nonce, encrypted_body, created_at, updated_at
                 FROM notes
                 WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let note = stmt
            .query_row([id], |row| {
                Ok(NoteAccessRow {
                    id: row.get(0)?,
                    body: row.get(1)?,
                    modules_json: row.get(2)?,
                    access_mode: parse_note_access_mode(row.get::<_, Option<String>>(3)?),
                    encryption_salt: row.get(4)?,
                    encryption_nonce: row.get(5)?,
                    encrypted_body: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            })
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(note)
    }

    fn load_note_access_rows(&self, conn: &Connection) -> Result<Vec<NoteAccessRow>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, body, modules_json, access_mode,
                        encryption_salt, encryption_nonce, encrypted_body, created_at, updated_at
                 FROM notes
                 ORDER BY updated_at DESC",
            )
            .map_err(|e| e.to_string())?;
        let notes = stmt
            .query_map([], |row| {
                Ok(NoteAccessRow {
                    id: row.get(0)?,
                    body: row.get(1)?,
                    modules_json: row.get(2)?,
                    access_mode: parse_note_access_mode(row.get::<_, Option<String>>(3)?),
                    encryption_salt: row.get(4)?,
                    encryption_nonce: row.get(5)?,
                    encrypted_body: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(notes)
    }

    fn note_from_access_row(&self, row: &NoteAccessRow) -> Result<Note, String> {
        let is_unlocked =
            !is_note_protected(row.access_mode) || self.is_note_unlocked(row.id.as_str());
        let body = if is_unlocked {
            self.load_plain_body_from_access_row(row)?
        } else {
            String::new()
        };
        Ok(Note {
            id: row.id.clone(),
            body,
            modules: parse_note_modules_json(row.modules_json.clone()),
            access_mode: row.access_mode,
            is_unlocked,
            created_at: row.created_at.clone(),
            updated_at: row.updated_at.clone(),
        })
    }

    fn note_summary_from_row(
        &self,
        conn: &Connection,
        row: &NoteSummaryRow,
    ) -> Result<NoteSummary, String> {
        let mut is_unlocked =
            !is_note_protected(row.access_mode) || self.is_note_unlocked(row.id.as_str());
        let body_prefix = if is_unlocked {
            match row.access_mode {
                NoteAccessMode::None | NoteAccessMode::Locked => row.body_prefix.clone(),
                NoteAccessMode::Encrypted => {
                    let Some(encryption) = self.unlocked_encryption_for(row.id.as_str()) else {
                        is_unlocked = false;
                        return Ok(NoteSummary {
                            id: row.id.clone(),
                            title: normalize_stored_title(row.note_title.clone())
                                .unwrap_or_else(|| "Untitled".to_string()),
                            body_prefix: "[locked]".to_string(),
                            access_mode: row.access_mode,
                            is_unlocked,
                            updated_at: row.updated_at.clone(),
                        });
                    };
                    let payload = self
                        .load_note_encrypted_payload_row(conn, row.id.as_str())?
                        .ok_or_else(|| "Note not found".to_string())?;
                    let payload_salt = payload
                        .encryption_salt
                        .as_ref()
                        .ok_or_else(|| "encrypted note salt missing".to_string())?;
                    if payload_salt.as_slice() != encryption.encryption_salt.as_slice() {
                        self.note_access.clear(row.id.as_str());
                        is_unlocked = false;
                        "[locked]".to_string()
                    } else {
                        match decrypt_note_body_with_key(
                            payload
                                .encrypted_body
                                .as_ref()
                                .ok_or_else(|| "encrypted note payload missing".to_string())?,
                            payload
                                .encryption_nonce
                                .as_ref()
                                .ok_or_else(|| "encrypted note nonce missing".to_string())?,
                            &encryption.key,
                        ) {
                            Ok(decrypted) => decrypted.chars().take(200).collect(),
                            Err(_) => {
                                self.note_access.clear(row.id.as_str());
                                is_unlocked = false;
                                "[locked]".to_string()
                            }
                        }
                    }
                }
            }
        } else {
            "[locked]".to_string()
        };
        let title = match normalize_stored_title(row.note_title.clone()) {
            Some(value) => value,
            None if is_unlocked => derive_note_title_from_body(&body_prefix),
            None => "Untitled".to_string(),
        };
        Ok(NoteSummary {
            id: row.id.clone(),
            title,
            body_prefix,
            access_mode: row.access_mode,
            is_unlocked,
            updated_at: row.updated_at.clone(),
        })
    }

    fn load_plain_body_from_access_row(&self, row: &NoteAccessRow) -> Result<String, String> {
        match row.access_mode {
            NoteAccessMode::None | NoteAccessMode::Locked => Ok(row.body.clone()),
            NoteAccessMode::Encrypted => {
                let encryption = self
                    .unlocked_encryption_for(row.id.as_str())
                    .ok_or_else(|| "note is locked; unlock first".to_string())?;
                let row_salt = row
                    .encryption_salt
                    .as_ref()
                    .ok_or_else(|| "encrypted note salt missing".to_string())?;
                if row_salt.as_slice() != encryption.encryption_salt.as_slice() {
                    self.note_access.clear(row.id.as_str());
                    return Err("note is locked; unlock first".to_string());
                }
                match decrypt_note_body_with_key(
                    row.encrypted_body
                        .as_ref()
                        .ok_or_else(|| "encrypted note payload missing".to_string())?,
                    row.encryption_nonce
                        .as_ref()
                        .ok_or_else(|| "encrypted note nonce missing".to_string())?,
                    &encryption.key,
                ) {
                    Ok(body) => Ok(body),
                    Err(_) => {
                        self.note_access.clear(row.id.as_str());
                        Err("note is locked; unlock first".to_string())
                    }
                }
            }
        }
    }

    fn load_note_plain_body_for_access(
        &self,
        conn: &Connection,
        id: &str,
        security: &NoteSecurityRow,
    ) -> Result<Option<String>, String> {
        if !is_note_protected(security.access_mode) {
            let body = self.load_note_row(conn, id)?.map(|row| row.body);
            return Ok(body);
        }

        let Some(access_grant) = self.note_access.session(id) else {
            return Ok(None);
        };
        match security.access_mode {
            NoteAccessMode::None => self.load_note_row(conn, id).map(|row| row.map(|r| r.body)),
            NoteAccessMode::Locked => self.load_note_row(conn, id).map(|row| row.map(|r| r.body)),
            NoteAccessMode::Encrypted => {
                let NoteAccessGrant::Encrypted {
                    key,
                    encryption_salt,
                } = access_grant
                else {
                    return Ok(None);
                };
                let security_salt = security
                    .encryption_salt
                    .as_ref()
                    .ok_or_else(|| "encrypted note salt missing".to_string())?;
                if security_salt.as_slice() != encryption_salt.as_slice() {
                    self.note_access.clear(id);
                    return Ok(None);
                }
                let decrypted = decrypt_note_body_with_key(
                    security
                        .encrypted_body
                        .as_ref()
                        .ok_or_else(|| "encrypted note payload missing".to_string())?,
                    security
                        .encryption_nonce
                        .as_ref()
                        .ok_or_else(|| "encrypted note nonce missing".to_string())?,
                    &key,
                );
                match decrypted {
                    Ok(body) => Ok(Some(body)),
                    Err(_) => {
                        self.note_access.clear(id);
                        Ok(None)
                    }
                }
            }
        }
    }

    fn load_note_plain_body_for_access_with_password(
        &self,
        conn: &Connection,
        id: &str,
        security: &NoteSecurityRow,
        password: &str,
    ) -> Result<Option<String>, String> {
        let Some(row) = self.load_note_row(conn, id)? else {
            return Ok(None);
        };
        let body = match security.access_mode {
            NoteAccessMode::None => row.body,
            NoteAccessMode::Locked => row.body,
            NoteAccessMode::Encrypted => decrypt_note_body(
                security
                    .encrypted_body
                    .as_ref()
                    .ok_or_else(|| "encrypted note payload missing".to_string())?,
                security
                    .encryption_salt
                    .as_ref()
                    .ok_or_else(|| "encrypted note salt missing".to_string())?,
                security
                    .encryption_nonce
                    .as_ref()
                    .ok_or_else(|| "encrypted note nonce missing".to_string())?,
                password,
            )?,
        };
        Ok(Some(body))
    }

    fn load_note_with_access(&self, conn: &Connection, id: &str) -> Result<Option<Note>, String> {
        let Some(row) = self.load_note_access_row(conn, id)? else {
            return Ok(None);
        };
        self.note_from_access_row(&row).map(Some)
    }
}

#[derive(Debug, Clone)]
struct NoteRow {
    body: String,
}

#[derive(Debug, Clone)]
struct NotePersistenceRow {
    modules_json: Option<String>,
    created_at: String,
}

#[derive(Debug, Clone)]
struct NoteSummaryRow {
    id: String,
    note_title: Option<String>,
    body_prefix: String,
    access_mode: NoteAccessMode,
    updated_at: String,
}

#[derive(Debug, Clone)]
struct NoteEncryptedPayloadRow {
    encryption_salt: Option<Vec<u8>>,
    encryption_nonce: Option<Vec<u8>>,
    encrypted_body: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
struct NoteAccessRow {
    id: String,
    body: String,
    modules_json: Option<String>,
    access_mode: NoteAccessMode,
    encryption_salt: Option<Vec<u8>>,
    encryption_nonce: Option<Vec<u8>>,
    encrypted_body: Option<Vec<u8>>,
    created_at: String,
    updated_at: String,
}

fn parse_note_modules_json(value: Option<String>) -> NoteModules {
    let Some(raw) = value else {
        return NoteModules::default();
    };
    match serde_json::from_str::<NoteModules>(&raw) {
        Ok(modules) => modules,
        Err(_) => NoteModules::default(),
    }
}

fn parse_note_access_mode(value: Option<String>) -> NoteAccessMode {
    match value
        .as_deref()
        .map(str::trim)
        .unwrap_or("none")
        .to_ascii_lowercase()
        .as_str()
    {
        "locked" => NoteAccessMode::Locked,
        "encrypted" => NoteAccessMode::Encrypted,
        _ => NoteAccessMode::None,
    }
}

fn normalize_stored_title(value: Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToString::to_string)
}

fn derive_note_title_from_body(body: &str) -> String {
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut out = String::new();
        for (idx, ch) in trimmed.chars().enumerate() {
            if idx >= NOTE_TITLE_MAX_CHARS {
                out.push_str("...");
                return out;
            }
            out.push(ch);
        }
        return out;
    }
    "Untitled".to_string()
}

fn is_note_protected(mode: NoteAccessMode) -> bool {
    !matches!(mode, NoteAccessMode::None)
}

fn normalize_password(password: &str) -> Result<String, String> {
    if password.trim().is_empty() {
        return Err("password must not be empty".to_string());
    }
    if password != password.trim() {
        return Err("password must not start or end with whitespace".to_string());
    }
    Ok(password.to_string())
}

fn fill_random_bytes(target: &mut [u8]) -> Result<(), String> {
    getrandom::fill(target).map_err(|e| format!("Failed to gather secure random bytes: {e}"))
}

fn derive_password_hash(password: &str, salt: &[u8]) -> [u8; PASSWORD_HASH_LEN] {
    let mut out = [0u8; PASSWORD_HASH_LEN];
    pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, PBKDF2_ITERATIONS, &mut out);
    out
}

fn password_hash_pair(password: &str) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut salt = [0u8; PASSWORD_SALT_LEN];
    fill_random_bytes(&mut salt)?;
    let hash = derive_password_hash(password, &salt);
    Ok((salt.to_vec(), hash.to_vec()))
}

fn verify_password(security: &NoteSecurityRow, password: &str) -> Result<(), String> {
    let Some(salt) = security.password_salt.as_ref() else {
        return Err("note password metadata missing".to_string());
    };
    let Some(expected_hash) = security.password_hash.as_ref() else {
        return Err("note password metadata missing".to_string());
    };
    if expected_hash.len() != PASSWORD_HASH_LEN {
        return Err("note password metadata invalid".to_string());
    }
    let actual = derive_password_hash(password, salt);
    if actual.ct_eq(expected_hash.as_slice()).into() {
        Ok(())
    } else {
        Err("invalid password".to_string())
    }
}

#[derive(Debug, Clone)]
struct EncryptedBody {
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
}

fn derive_encryption_key(password: &str, salt: &[u8]) -> [u8; 32] {
    let mut key = [0u8; 32];
    pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, PBKDF2_ITERATIONS, &mut key);
    key
}

fn encrypt_note_body_with_key(body: &str, key: &[u8; 32]) -> Result<EncryptedBody, String> {
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|e| format!("Failed to init cipher: {e}"))?;

    let mut nonce_bytes = [0u8; ENCRYPTION_NONCE_LEN];
    fill_random_bytes(&mut nonce_bytes)?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, body.as_bytes())
        .map_err(|_| "Failed to encrypt note body".to_string())?;

    Ok(EncryptedBody {
        nonce: nonce_bytes.to_vec(),
        ciphertext,
    })
}

fn decrypt_note_body_with_key(
    ciphertext: &[u8],
    nonce: &[u8],
    key: &[u8; 32],
) -> Result<String, String> {
    if nonce.len() != ENCRYPTION_NONCE_LEN {
        return Err("encrypted note nonce invalid".to_string());
    }
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|e| format!("Failed to init cipher: {e}"))?;
    let plain = cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| "invalid password".to_string())?;
    String::from_utf8(plain).map_err(|_| "encrypted note content invalid UTF-8".to_string())
}

fn decrypt_note_body(
    ciphertext: &[u8],
    salt: &[u8],
    nonce: &[u8],
    password: &str,
) -> Result<String, String> {
    let key = derive_encryption_key(password, salt);
    decrypt_note_body_with_key(ciphertext, nonce, &key)
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

fn apply_pending_migrations(conn: &Connection) -> Result<(), String> {
    let needs_fts_title: bool = conn
        .query_row(
            "SELECT COUNT(1) FROM sqlite_master \
             WHERE type='table' AND name='notes_fts' AND sql NOT LIKE '%note_title%'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;

    if needs_fts_title {
        conn.execute_batch(include_str!("../../migrations/0002_fts_title.sql"))
            .map_err(|e| format!("Migration 0002 (fts_title) failed: {e}"))?;
    }

    let needs_fts_prefix: bool = conn
        .query_row(
            "SELECT COUNT(1) FROM sqlite_master \
             WHERE type='table' AND name='notes_fts' \
               AND (sql IS NULL OR LOWER(sql) NOT LIKE '%prefix%')",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;

    if needs_fts_prefix {
        conn.execute_batch(include_str!("../../migrations/0003_fts_prefix.sql"))
            .map_err(|e| format!("Migration 0003 (fts_prefix) failed: {e}"))?;
    }
    cleanup_legacy_markdown_file_notes(conn)?;
    Ok(())
}

fn cleanup_legacy_markdown_file_notes(conn: &Connection) -> Result<(), String> {
    let deleted = conn
        .execute(
            "DELETE FROM notes WHERE id LIKE ?1",
            [LEGACY_MARKDOWN_FILE_NOTE_ID_SQL_PREFIX],
        )
        .map_err(|e| format!("Migration 0004 (legacy_markdown_cleanup) failed: {e}"))?;
    if deleted > 0 {
        eprintln!("cleaned up {deleted} legacy markdown-file note row(s)");
    }
    Ok(())
}

fn rebuild_note_search_index_inner(conn: &Connection) -> Result<(), String> {
    conn.execute("DELETE FROM notes_fts", [])
        .map_err(|e| format!("Failed to clear FTS index: {e}"))?;
    conn.execute(
        "INSERT INTO notes_fts(rowid, note_id, note_title, body)
         SELECT rowid, id, note_title, body FROM notes WHERE access_mode = 'none'",
        [],
    )
    .map_err(|e| format!("Failed to rebuild FTS index: {e}"))?;
    Ok(())
}

fn check_and_heal_search_index(conn: &Connection) -> Result<(), String> {
    let notes_count: i64 = conn
        .query_row(
            "SELECT COUNT(1) FROM notes WHERE access_mode = 'none'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to count searchable notes: {e}"))?;
    let indexed_count: i64 = conn
        .query_row("SELECT COUNT(1) FROM notes_fts", [], |row| row.get(0))
        .map_err(|e| format!("Failed to count FTS index rows: {e}"))?;

    if notes_count != indexed_count {
        eprintln!(
            "note search index drift detected ({notes_count} notes, {indexed_count} indexed): rebuilding"
        );
        rebuild_note_search_index_inner(conn)?;
    }
    Ok(())
}

fn seed_note_search_index_if_empty(conn: &Connection) -> Result<(), String> {
    let indexed_rows: i64 = conn
        .query_row("SELECT COUNT(1) FROM notes_fts", [], |row| row.get(0))
        .map_err(|e| format!("Failed to inspect note search index: {e}"))?;
    if indexed_rows > 0 {
        return Ok(());
    }

    conn.execute(
        "INSERT INTO notes_fts(rowid, note_id, note_title, body)
         SELECT rowid, id, note_title, body
         FROM notes
         WHERE access_mode = 'none'",
        [],
    )
    .map_err(|e| format!("Failed to seed note search index: {e}"))?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SearchTerm {
    Phrase(String),
    Prefix(String),
}

fn is_search_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '-'
}

fn parse_search_terms(raw: &str) -> Vec<SearchTerm> {
    let mut terms = Vec::new();
    let mut chars = raw.chars().peekable();

    while chars.peek().is_some() {
        while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
            chars.next();
        }
        if terms.len() >= SEARCH_QUERY_MAX_TERMS {
            break;
        }
        match chars.peek() {
            None => break,
            Some(&'"') => {
                chars.next();
                let mut phrase = String::new();
                for ch in chars.by_ref() {
                    if ch == '"' {
                        break;
                    }
                    if is_search_char(ch) || ch.is_whitespace() {
                        phrase.push(ch);
                    }
                }
                let phrase = phrase.trim().to_string();
                if !phrase.is_empty() {
                    terms.push(SearchTerm::Phrase(phrase));
                }
            }
            Some(_) => {
                let mut token = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch.is_whitespace() || ch == '"' {
                        break;
                    }
                    chars.next();
                    if is_search_char(ch) {
                        token.push(ch);
                    }
                }
                if !token.is_empty() {
                    terms.push(SearchTerm::Prefix(token));
                }
            }
        }
    }
    terms
}

fn build_fts_query_from_terms(terms: &[SearchTerm]) -> String {
    terms
        .iter()
        .map(|term| match term {
            SearchTerm::Phrase(phrase) => format!("\"{}\"", phrase.replace('"', "\"\"")),
            SearchTerm::Prefix(prefix) => format!("\"{}\"*", prefix.replace('"', "\"\"")),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
fn build_fts_query(raw: &str) -> String {
    build_fts_query_from_terms(&parse_search_terms(raw))
}

fn snippet_fragments(snippet: &str) -> Vec<String> {
    let cleaned = snippet
        .replace("[[", "")
        .replace("]]", "")
        .replace('\n', " ");
    let mut parts = cleaned
        .split('…')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| part.to_lowercase())
        .collect::<Vec<_>>();
    parts.sort_by_key(|part| std::cmp::Reverse(part.len()));
    parts
}

fn line_matches_term(line: &str, term: &SearchTerm) -> bool {
    match term {
        SearchTerm::Phrase(phrase) => line.contains(&phrase.to_lowercase()),
        SearchTerm::Prefix(prefix) => line.contains(&prefix.to_lowercase()),
    }
}

fn search_result_line_number(body: &str, snippet: &str, terms: &[SearchTerm]) -> usize {
    if body.is_empty() {
        return 1;
    }

    let lines = body.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return 1;
    }

    let snippet_parts = snippet_fragments(snippet);
    if !snippet_parts.is_empty() {
        if let Some((idx, _)) = lines.iter().enumerate().find(|(_, line)| {
            let lower = line.to_lowercase();
            snippet_parts.iter().any(|part| lower.contains(part))
        }) {
            return idx + 1;
        }
    }

    if !terms.is_empty() {
        if let Some((idx, _)) = lines.iter().enumerate().find(|(_, line)| {
            let lower = line.to_lowercase();
            terms.iter().all(|term| line_matches_term(&lower, term))
        }) {
            return idx + 1;
        }
        if let Some((idx, _)) = lines.iter().enumerate().find(|(_, line)| {
            let lower = line.to_lowercase();
            terms.iter().any(|term| line_matches_term(&lower, term))
        }) {
            return idx + 1;
        }
    }

    1
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

        assert!(db.delete_note("n1", None).expect("delete succeeds"));
        assert!(!db.delete_note("n1", None).expect("second delete succeeds"));
        assert!(db.get_note("n1").expect("lookup succeeds").is_none());

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn password_normalization_rejects_surrounding_whitespace() {
        let err = normalize_password(" pass123").expect_err("leading whitespace should fail");
        assert!(err.contains("start or end with whitespace"));

        let err = normalize_password("pass123 ").expect_err("trailing whitespace should fail");
        assert!(err.contains("start or end with whitespace"));

        let err = normalize_password("   ").expect_err("all-whitespace should fail");
        assert!(err.contains("must not be empty"));

        assert_eq!(
            normalize_password("pass 123").expect("interior whitespace is allowed"),
            "pass 123".to_string()
        );
    }

    #[test]
    fn lock_unlock_requires_password_and_redacts_locked_reads() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        db.save_note("n1", "top secret").expect("seed note");
        let locked = db.lock_note("n1", "pass123").expect("lock note");
        assert_eq!(locked.access_mode, NoteAccessMode::Locked);
        assert!(!locked.is_unlocked);
        assert_eq!(locked.body, "");

        let listed_locked = db.list_notes_meta().expect("list meta");
        assert_eq!(listed_locked.len(), 1);
        assert_eq!(listed_locked[0].title, "top secret");
        assert_eq!(listed_locked[0].body_prefix, "[locked]");
        assert_eq!(listed_locked[0].access_mode, NoteAccessMode::Locked);
        assert!(!listed_locked[0].is_unlocked);

        let save_err = db
            .save_note("n1", "should fail")
            .expect_err("save should fail while locked");
        assert!(save_err.contains("unlock first"));

        let wrong = db
            .unlock_note("n1", "wrong")
            .expect_err("wrong password should fail");
        assert!(wrong.contains("invalid password"));

        let unlocked = db.unlock_note("n1", "pass123").expect("unlock note");
        assert_eq!(unlocked.access_mode, NoteAccessMode::Locked);
        assert!(unlocked.is_unlocked);
        assert_eq!(unlocked.body, "top secret");

        let listed_unlocked = db.list_notes_meta().expect("list meta unlocked");
        assert_eq!(listed_unlocked.len(), 1);
        assert_eq!(listed_unlocked[0].body_prefix, "top secret");
        assert_eq!(listed_unlocked[0].access_mode, NoteAccessMode::Locked);
        assert!(listed_unlocked[0].is_unlocked);

        let saved = db
            .save_note("n1", "top secret updated")
            .expect("save after unlock");
        assert_eq!(saved.body, "top secret updated");
        assert!(saved.is_unlocked);

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn encrypt_decrypt_keeps_ciphertext_at_rest_until_decrypted() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        db.save_note("n1", "classified line").expect("seed note");
        let encrypted = db.encrypt_note("n1", "enc-pass").expect("encrypt note");
        assert_eq!(encrypted.access_mode, NoteAccessMode::Encrypted);
        assert!(encrypted.is_unlocked);
        assert_eq!(encrypted.body, "classified line");

        let listed_encrypted = db.list_notes_meta().expect("list meta encrypted");
        assert_eq!(listed_encrypted.len(), 1);
        assert_eq!(listed_encrypted[0].title, "classified line");
        assert_eq!(listed_encrypted[0].body_prefix, "classified line");
        assert_eq!(listed_encrypted[0].access_mode, NoteAccessMode::Encrypted);
        assert!(listed_encrypted[0].is_unlocked);

        let conn = Connection::open(path.clone()).expect("inspect db");
        let stored_before_unlock: String = conn
            .query_row("SELECT body FROM notes WHERE id = 'n1'", [], |row| {
                row.get(0)
            })
            .expect("body query");
        let encrypted_payload_before_unlock: Option<Vec<u8>> = conn
            .query_row(
                "SELECT encrypted_body FROM notes WHERE id = 'n1'",
                [],
                |row| row.get(0),
            )
            .expect("cipher query");
        assert_eq!(stored_before_unlock, "");
        assert!(encrypted_payload_before_unlock.is_some());
        drop(conn);

        let unlocked = db.unlock_note("n1", "enc-pass").expect("unlock encrypted");
        assert_eq!(unlocked.access_mode, NoteAccessMode::Encrypted);
        assert!(unlocked.is_unlocked);
        assert_eq!(unlocked.body, "classified line");

        let listed_unlocked = db.list_notes_meta().expect("list meta unlocked");
        assert_eq!(listed_unlocked.len(), 1);
        assert_eq!(listed_unlocked[0].body_prefix, "classified line");
        assert_eq!(listed_unlocked[0].access_mode, NoteAccessMode::Encrypted);
        assert!(listed_unlocked[0].is_unlocked);

        db.save_note("n1", "classified line updated")
            .expect("save encrypted unlocked note");
        let conn = Connection::open(path.clone()).expect("inspect db after save");
        let stored_after_save: String = conn
            .query_row("SELECT body FROM notes WHERE id = 'n1'", [], |row| {
                row.get(0)
            })
            .expect("body query");
        let encrypted_payload_after_save: Option<Vec<u8>> = conn
            .query_row(
                "SELECT encrypted_body FROM notes WHERE id = 'n1'",
                [],
                |row| row.get(0),
            )
            .expect("cipher query");
        assert_eq!(stored_after_save, "");
        assert!(encrypted_payload_after_save.is_some());
        drop(conn);

        let decrypted = db.decrypt_note("n1", "enc-pass").expect("decrypt note");
        assert_eq!(decrypted.access_mode, NoteAccessMode::None);
        assert!(decrypted.is_unlocked);
        assert_eq!(decrypted.body, "classified line updated");

        let conn = Connection::open(path.clone()).expect("inspect db after decrypt");
        let stored_after_decrypt: String = conn
            .query_row("SELECT body FROM notes WHERE id = 'n1'", [], |row| {
                row.get(0)
            })
            .expect("body query");
        let encrypted_payload_after_decrypt: Option<Vec<u8>> = conn
            .query_row(
                "SELECT encrypted_body FROM notes WHERE id = 'n1'",
                [],
                |row| row.get(0),
            )
            .expect("cipher query");
        assert_eq!(stored_after_decrypt, "classified line updated");
        assert!(encrypted_payload_after_decrypt.is_none());

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn create_note_with_defaults_rolls_back_when_password_is_invalid() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        let err = db
            .create_note_with_defaults("n1", NoteModules::default(), Some(" pass123"))
            .expect_err("invalid password should fail");
        assert!(err.contains("start or end with whitespace"));
        assert!(db.get_note("n1").expect("lookup succeeds").is_none());

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn create_note_with_defaults_can_create_encrypted_note_atomically() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        let created = db
            .create_note_with_defaults(
                "n1",
                NoteModules {
                    math: false,
                    table: true,
                    variables: false,
                    style: true,
                },
                Some("enc-pass"),
            )
            .expect("create note with defaults");
        assert_eq!(created.id, "n1");
        assert_eq!(created.access_mode, NoteAccessMode::Encrypted);
        assert!(created.is_unlocked);
        assert_eq!(created.body, "");
        assert!(!created.modules.math);
        assert!(created.modules.table);
        assert!(!created.modules.variables);
        assert!(created.modules.style);

        let conn = Connection::open(path.clone()).expect("inspect db");
        let encrypted_payload: Option<Vec<u8>> = conn
            .query_row(
                "SELECT encrypted_body FROM notes WHERE id = 'n1'",
                [],
                |row| row.get(0),
            )
            .expect("cipher query");
        assert!(encrypted_payload.is_some());

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn deleting_protected_note_requires_password() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        db.save_note("locked", "top secret")
            .expect("seed locked note");
        db.lock_note("locked", "lock-pass").expect("lock note");

        let missing_password = db
            .delete_note("locked", None)
            .expect_err("delete should require password");
        assert!(missing_password.contains("password required"));

        let wrong_password = db
            .delete_note("locked", Some("wrong"))
            .expect_err("wrong password should fail");
        assert!(wrong_password.contains("invalid password"));

        assert!(db
            .delete_note("locked", Some("lock-pass"))
            .expect("delete locked note succeeds"));

        db.save_note("encrypted", "classified")
            .expect("seed encrypted note");
        db.encrypt_note("encrypted", "enc-pass")
            .expect("encrypt note");

        let missing_password = db
            .delete_note("encrypted", None)
            .expect_err("delete should require password");
        assert!(missing_password.contains("password required"));

        let wrong_password = db
            .delete_note("encrypted", Some("wrong"))
            .expect_err("wrong password should fail");
        assert!(wrong_password.contains("invalid password"));

        assert!(db
            .delete_note("encrypted", Some("enc-pass"))
            .expect("delete encrypted note succeeds"));

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn notes_default_modules_and_persist_explicit_module_updates() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        let note = db.save_note("n1", "hello").expect("note saved");
        assert_eq!(note.id, "n1");
        assert_eq!(note.body, "hello");
        assert_eq!(note.modules, NoteModules::default());

        let updated = db
            .set_note_modules(
                "n1",
                NoteModules {
                    math: false,
                    table: true,
                    variables: false,
                    style: true,
                },
            )
            .expect("module update succeeds");
        assert!(!updated.modules.math);
        assert!(updated.modules.table);
        assert!(!updated.modules.variables);
        assert!(updated.modules.style);

        let fetched = db
            .get_note("n1")
            .expect("lookup succeeds")
            .expect("note exists");
        assert_eq!(fetched.modules, updated.modules);

        // Body saves should keep previously selected modules unchanged.
        let body_updated = db
            .save_note("n1", "updated body")
            .expect("body update succeeds");
        assert_eq!(body_updated.modules, updated.modules);

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
    fn search_notes_content_matches_plain_notes_and_skips_protected_notes() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        db.save_note("plain-a", "alpha budget meeting notes")
            .expect("save plain note a");
        db.save_note("plain-b", "roadmap has alpha milestone")
            .expect("save plain note b");
        db.save_note("locked-a", "alpha locked secret")
            .expect("save locked note");
        db.lock_note("locked-a", "lock-pass").expect("lock note");
        db.save_note("enc-a", "alpha encrypted secret")
            .expect("save encrypted note");
        db.encrypt_note("enc-a", "enc-pass").expect("encrypt note");

        let hits = db
            .search_notes_content("alpha", 20)
            .expect("search succeeds");
        let ids: Vec<&str> = hits.iter().map(|note| note.id.as_str()).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"plain-a"));
        assert!(ids.contains(&"plain-b"));
        assert!(!ids.contains(&"locked-a"));
        assert!(!ids.contains(&"enc-a"));

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn search_notes_content_seeds_index_for_pre_fts_databases() {
        let path = temp_db_path();
        let conn = Connection::open(path.clone()).expect("legacy db opens");
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE notes (
                id TEXT PRIMARY KEY,
                body TEXT NOT NULL DEFAULT '',
                note_title TEXT NOT NULL DEFAULT '',
                modules_json TEXT NOT NULL DEFAULT '{\"math\":true,\"table\":true,\"variables\":true,\"style\":true}',
                access_mode TEXT NOT NULL DEFAULT 'none',
                password_salt BLOB,
                password_hash BLOB,
                encryption_salt BLOB,
                encryption_nonce BLOB,
                encrypted_body BLOB,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
             );
             INSERT INTO notes (id, body, note_title, modules_json, access_mode, created_at, updated_at)
             VALUES ('legacy-note', 'alpha from legacy schema', 'legacy', '{\"math\":true,\"table\":true,\"variables\":true,\"style\":true}', 'none', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');",
        )
        .expect("legacy schema created");
        drop(conn);

        let db = Db::open(path.clone()).expect("db opens with search index bootstrap");
        let hits = db
            .search_notes_content("alpha", 20)
            .expect("search succeeds");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "legacy-note");

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn open_db_migration_cleans_legacy_markdown_file_notes() {
        let path = temp_db_path();
        let conn = Connection::open(path.clone()).expect("legacy db opens");
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE notes (
                id TEXT PRIMARY KEY,
                body TEXT NOT NULL DEFAULT '',
                note_title TEXT NOT NULL DEFAULT '',
                modules_json TEXT NOT NULL DEFAULT '{\"math\":true,\"table\":true,\"variables\":true,\"style\":true}',
                access_mode TEXT NOT NULL DEFAULT 'none',
                password_salt BLOB,
                password_hash BLOB,
                encryption_salt BLOB,
                encryption_nonce BLOB,
                encrypted_body BLOB,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
             );
             INSERT INTO notes (id, body, note_title, modules_json, access_mode, created_at, updated_at)
             VALUES
               ('mdfile:legacy', 'legacy markdown row marker', 'legacy mdfile', '{\"math\":true,\"table\":true,\"variables\":true,\"style\":true}', 'none', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'),
               ('n1', 'survivor row marker', 'regular', '{\"math\":true,\"table\":true,\"variables\":true,\"style\":true}', 'none', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');",
        )
        .expect("legacy schema created");
        drop(conn);

        let db = Db::open(path.clone()).expect("db opens and runs cleanup migration");
        let listed = db.list_notes_meta().expect("list meta succeeds");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "n1");

        let legacy_hits = db
            .search_notes_content("legacy markdown row marker", 20)
            .expect("search succeeds");
        assert!(
            legacy_hits.is_empty(),
            "legacy mdfile row should be removed"
        );
        let survivor_hits = db
            .search_notes_content("survivor row marker", 20)
            .expect("search succeeds");
        assert_eq!(survivor_hits.len(), 1);
        assert_eq!(survivor_hits[0].id, "n1");
        drop(db);

        let check = Connection::open(path.clone()).expect("db reopens for verification");
        let mdfile_rows: i64 = check
            .query_row(
                "SELECT COUNT(1) FROM notes WHERE id LIKE 'mdfile:%'",
                [],
                |row| row.get(0),
            )
            .expect("mdfile row count query succeeds");
        assert_eq!(mdfile_rows, 0);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn search_notes_content_migrates_fts_to_prefix_index() {
        let path = temp_db_path();
        let conn = Connection::open(path.clone()).expect("legacy db opens");
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE notes (
                id TEXT PRIMARY KEY,
                body TEXT NOT NULL DEFAULT '',
                note_title TEXT NOT NULL DEFAULT '',
                modules_json TEXT NOT NULL DEFAULT '{\"math\":true,\"table\":true,\"variables\":true,\"style\":true}',
                access_mode TEXT NOT NULL DEFAULT 'none',
                password_salt BLOB,
                password_hash BLOB,
                encryption_salt BLOB,
                encryption_nonce BLOB,
                encrypted_body BLOB,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
             );
             CREATE VIRTUAL TABLE notes_fts USING fts5(
                note_id UNINDEXED,
                note_title,
                body,
                tokenize = 'unicode61'
             );
             CREATE TRIGGER notes_fts_ai
             AFTER INSERT ON notes
             BEGIN
                 INSERT INTO notes_fts(rowid, note_id, note_title, body)
                 SELECT new.rowid, new.id, new.note_title, new.body
                 WHERE new.access_mode = 'none';
             END;
             CREATE TRIGGER notes_fts_ad
             AFTER DELETE ON notes
             BEGIN
                 DELETE FROM notes_fts WHERE rowid = old.rowid;
             END;
             CREATE TRIGGER notes_fts_au
             AFTER UPDATE ON notes
             BEGIN
                 DELETE FROM notes_fts WHERE rowid = old.rowid;
                 INSERT INTO notes_fts(rowid, note_id, note_title, body)
                 SELECT new.rowid, new.id, new.note_title, new.body
                 WHERE new.access_mode = 'none';
             END;
             INSERT INTO notes (id, body, note_title, modules_json, access_mode, created_at, updated_at)
             VALUES ('legacy-note', 'alpha from legacy fts', 'legacy', '{\"math\":true,\"table\":true,\"variables\":true,\"style\":true}', 'none', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');",
        )
        .expect("legacy schema with non-prefix fts created");
        drop(conn);

        let db = Db::open(path.clone()).expect("db opens and migrates fts");
        let hits = db
            .search_notes_content("alpha", 20)
            .expect("search succeeds");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "legacy-note");
        drop(db);

        let check = Connection::open(path.clone()).expect("db reopens");
        let sql: String = check
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='notes_fts'",
                [],
                |row| row.get(0),
            )
            .expect("fts schema sql exists");
        assert!(
            sql.to_lowercase().contains("prefix"),
            "notes_fts should include prefix index after migration, schema: {sql}"
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn search_notes_content_returns_snippet_with_highlight_markers() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        db.save_note("n1", "the quarterly budget review is scheduled for monday")
            .expect("save note");

        let hits = db
            .search_notes_content("budget", 10)
            .expect("search succeeds");
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].line_number, 1,
            "single-line match should resolve to line 1"
        );
        assert!(
            hits[0].snippet.contains("[[") && hits[0].snippet.contains("]]"),
            "snippet should contain highlight markers, got: {:?}",
            hits[0].snippet
        );
        assert!(
            hits[0].snippet.contains("budget") || hits[0].snippet.contains("[[budget]]"),
            "snippet should include the matched term"
        );

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn search_notes_content_returns_line_number_for_multiline_match() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        db.save_note(
            "n1",
            "intro line\nmiddle line\nneedle appears here\ntrailing line",
        )
        .expect("save note");

        let hits = db
            .search_notes_content("needle", 10)
            .expect("search succeeds");
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].line_number, 3,
            "line number should point to matched content line"
        );

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn search_notes_content_phrase_query_matches_exact_sequence() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        db.save_note("match", "the quick brown fox jumps over the lazy dog")
            .expect("save note");
        db.save_note("no-match", "quick fox brown jumps dog lazy")
            .expect("save note");

        let hits = db
            .search_notes_content("\"quick brown fox\"", 10)
            .expect("search succeeds");
        let ids: Vec<&str> = hits.iter().map(|r| r.id.as_str()).collect();
        assert!(
            ids.contains(&"match"),
            "phrase should match contiguous sequence"
        );
        assert!(
            !ids.contains(&"no-match"),
            "phrase should not match non-contiguous tokens"
        );

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn search_notes_content_title_hit_ranks_above_body_only_hit() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        // note with "rocket" only in body
        db.save_note(
            "body-only",
            "the concept of a rocket propulsion system is complex",
        )
        .expect("save body-only note");
        thread::sleep(Duration::from_millis(5));
        // note with "rocket" in title (first line becomes title)
        db.save_note("title-hit", "Rocket science overview\nsome content here")
            .expect("save title-hit note");

        let hits = db
            .search_notes_content("rocket", 10)
            .expect("search succeeds");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "title-hit", "title match should rank first");

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rebuild_note_search_index_restores_results_after_index_reset() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        db.save_note("n1", "restore me after rebuild")
            .expect("save note");

        // Manually corrupt the index
        {
            let conn = db.conn.lock().unwrap();
            conn.execute("DELETE FROM notes_fts", [])
                .expect("corrupt index");
        }

        let empty = db
            .search_notes_content("restore", 10)
            .expect("search on empty index");
        assert!(empty.is_empty(), "index should be empty after manual clear");

        db.rebuild_note_search_index().expect("rebuild succeeds");

        let hits = db
            .search_notes_content("restore", 10)
            .expect("search after rebuild");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "n1");

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn search_index_excludes_note_after_lock_and_encrypt_transitions() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        db.save_note("will-lock", "canary lock content")
            .expect("save note");
        db.save_note("will-encrypt", "canary encrypt content")
            .expect("save note");

        // Both notes indexed initially
        let before = db.search_notes_content("canary", 10).expect("search");
        assert_eq!(before.len(), 2);

        db.lock_note("will-lock", "pass1").expect("lock note");
        let after_lock = db
            .search_notes_content("canary", 10)
            .expect("search after lock");
        assert_eq!(after_lock.len(), 1);
        assert_eq!(after_lock[0].id, "will-encrypt");

        db.encrypt_note("will-encrypt", "pass2")
            .expect("encrypt note");
        let after_encrypt = db
            .search_notes_content("canary", 10)
            .expect("search after encrypt");
        assert!(
            after_encrypt.is_empty(),
            "both protected notes must be removed from index"
        );

        // Verify neither title nor snippet leaks protected content
        assert!(
            !after_lock.iter().any(|r| r.snippet.contains("lock")),
            "locked note content must not appear in snippets"
        );

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn search_index_restores_note_after_decrypt() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        db.save_note("encme", "canary decrypt content")
            .expect("save note");
        db.encrypt_note("encme", "pass1").expect("encrypt note");

        let encrypted = db
            .search_notes_content("canary", 10)
            .expect("search after encrypt");
        assert!(
            encrypted.is_empty(),
            "encrypted note should not be searchable"
        );

        db.decrypt_note("encme", "pass1").expect("decrypt note");
        let decrypted = db
            .search_notes_content("canary", 10)
            .expect("search after decrypt");
        assert_eq!(decrypted.len(), 1);
        assert_eq!(decrypted[0].id, "encme");

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn search_locked_note_stays_unsearchable_after_session_unlock() {
        // unlock_note for locked notes is in-session only — access_mode stays 'locked' in DB
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        db.save_note("lockme", "canary locked content")
            .expect("save note");
        db.lock_note("lockme", "pass1").expect("lock note");
        db.unlock_note("lockme", "pass1").expect("session unlock");

        let hits = db.search_notes_content("canary", 10).expect("search");
        assert!(
            hits.is_empty(),
            "locked note should remain unsearchable even after session unlock"
        );

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn build_fts_query_phrase_and_prefix_parsing() {
        assert_eq!(build_fts_query("hello world"), "\"hello\"* \"world\"*");
        assert_eq!(build_fts_query("\"exact phrase\""), "\"exact phrase\"");
        assert_eq!(
            build_fts_query("\"exact phrase\" prefix"),
            "\"exact phrase\" \"prefix\"*"
        );
        assert_eq!(build_fts_query(""), "");
        assert_eq!(build_fts_query("   "), "");
        // punctuation stripped from unquoted tokens
        assert_eq!(build_fts_query("hello!world"), "\"helloworld\"*");
    }

    #[test]
    fn note_updated_at_revision_lookup() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        assert_eq!(
            db.get_note_updated_at("missing")
                .expect("missing lookup succeeds"),
            None
        );

        let created = db.save_note("n1", "hello").expect("create note");
        let created_revision = db
            .get_note_updated_at("n1")
            .expect("revision lookup")
            .expect("revision present");
        assert_eq!(created_revision, created.updated_at);

        thread::sleep(Duration::from_millis(5));
        let updated = db.save_note("n1", "hello again").expect("update note");
        let updated_revision = db
            .get_note_updated_at("n1")
            .expect("revision lookup")
            .expect("revision present");
        assert_eq!(updated_revision, updated.updated_at);
        assert!(updated_revision >= created_revision);

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

        db.delete_note("n1", None).expect("delete note");
        let after_delete = db
            .list_reminders("n1")
            .expect("list reminders after delete");
        assert!(after_delete.is_empty());

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
    fn prepend_note_with_ingest_event_is_atomic_keeps_latest_at_top_and_dedups() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        let first = db
            .prepend_note_with_ingest_event(
                "imap",
                Some("<older@id>"),
                "inbox-email-2026-04-17",
                "# older",
                b"raw older",
                false,
                false,
            )
            .expect("first ingest succeeds")
            .expect("note written");
        assert_eq!(first.body, "# older");

        let second = db
            .prepend_note_with_ingest_event(
                "imap",
                Some("<newer@id>"),
                "inbox-email-2026-04-17",
                "# newer",
                b"raw newer",
                false,
                false,
            )
            .expect("second ingest succeeds")
            .expect("note written");
        assert_eq!(second.body, "# newer\n# older");

        let duplicate = db
            .prepend_note_with_ingest_event(
                "imap",
                Some("<newer@id>"),
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
        assert_eq!(note.body, "# newer\n# older");

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
