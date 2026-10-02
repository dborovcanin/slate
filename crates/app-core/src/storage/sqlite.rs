use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use rusqlite::{Connection, OptionalExtension};
use rustc_hash::FxHashMap;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use time::OffsetDateTime;

use super::models::{
    Collection, CollectionCounts, Note, NoteAccessMode, NoteModules, NoteRevision,
    NoteSearchResult, NoteSummary, NoteVersion, Reminder,
};
use super::note_access::{NoteAccessGrant, NoteAccessService};

#[path = "encryption.rs"]
mod encryption;
#[path = "history_store.rs"]
mod history_store;
use crate::note_sources::derive_note_title_from_body;
use encryption::{
    decrypt_bytes_with_key, decrypt_note_body_with_key, encrypt_bytes_with_key,
    ensure_collection_protects_no_notes, normalize_password, NOTE_LOCKED,
};

const DEFAULT_NOTE_MODULES_JSON: &str =
    r#"{"math":true,"table":true,"variables":true,"style":true,"cross_note":true}"#;
/// Listed in place of an encrypted note's title until it is unlocked.
pub const ENCRYPTED_NOTE_TITLE: &str = "Encrypted note";
const SEARCH_QUERY_MAX_TERMS: usize = 8;
const SEARCH_LIMIT_MAX: usize = 100;
// Body prefix fetched per search result for per-line match expansion. Caps the
// data pulled across the rusqlite FFI and the Rust allocation overhead when
// searching massive notes. Matches beyond this prefix are not reported.
const SEARCH_BODY_PREFIX_CHARS: i64 = 8192;
// Maximum results emitted per matching note. Prevents a single dense note
// (e.g. generated lorem-ipsum) from dominating the result list.
const SEARCH_MAX_RESULTS_PER_NOTE: usize = 5;

/// How a note is encrypted; see the `encryption` module.
#[derive(Debug, Clone)]
struct NoteSecurityRow {
    access_mode: NoteAccessMode,
    encryption_salt: Option<Vec<u8>>,
    wrapped_key: Option<Vec<u8>>,
    key_collection_id: Option<String>,
    encryption_nonce: Option<Vec<u8>>,
    encrypted_body: Option<Vec<u8>>,
}

const SQLITE_POOL_SIZE: usize = 4;
/// The `user_version` the last step of `migrate` records.
const SCHEMA_VERSION: i64 = 4;
/// How long a restore waits for in-flight database work before giving up.
const RESTORE_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, Default)]
pub struct DbOpenMetrics {
    pub primary_open_ms: f64,
    pub primary_configure_ms: f64,
    pub schema_init_ms: f64,
    pub total_ms: f64,
}

struct SqlitePoolState {
    connections: Vec<Connection>,
    created: usize,
    /// Set while a restore replaces the database file: no connection is
    /// handed out until it finishes.
    restoring: bool,
}

struct SqlitePool {
    state: Mutex<SqlitePoolState>,
    available: Condvar,
    db_path: PathBuf,
    max_size: usize,
}

struct SqlitePoolGuard<'a> {
    pool: &'a SqlitePool,
    connection: Option<Connection>,
}

impl SqlitePool {
    fn configure_connection(conn: &Connection) -> Result<(), String> {
        // cache_size in negative form = KB of page cache per connection. 8 MB
        // (-8192) is generous for a desktop notes app: trivial RAM cost and
        // measurably faster FTS + metadata listings on large databases.
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             PRAGMA cache_size = -8192;
             PRAGMA busy_timeout = 5000;",
        )
        .map_err(|e| format!("Failed to set pragmas: {e}"))
    }

    fn open_configured_connection(path: &Path) -> Result<Connection, String> {
        let conn = Connection::open(path).map_err(|e| format!("Failed to open DB: {e}"))?;
        Self::configure_connection(&conn)?;
        Ok(conn)
    }

    fn new(path: &Path, pool_size: usize) -> Result<(Self, DbOpenMetrics), String> {
        let total_started = std::time::Instant::now();

        let open_started = std::time::Instant::now();
        let first = Connection::open(path).map_err(|e| format!("Failed to open DB: {e}"))?;
        let primary_open_ms = open_started.elapsed().as_secs_f64() * 1000.0;

        let configure_started = std::time::Instant::now();
        Self::configure_connection(&first)?;
        let primary_configure_ms = configure_started.elapsed().as_secs_f64() * 1000.0;

        let schema_started = std::time::Instant::now();
        initialize_schema(&first)?;
        let schema_init_ms = schema_started.elapsed().as_secs_f64() * 1000.0;

        // FTS consistency/repair runs lazily on the first search request via
        // `ensure_search_index_checked`, keeping startup lean.
        let mut connections = Vec::with_capacity(pool_size.max(1));
        connections.push(first);
        let total_ms = total_started.elapsed().as_secs_f64() * 1000.0;
        Ok((
            Self {
                state: Mutex::new(SqlitePoolState {
                    connections,
                    created: 1,
                    restoring: false,
                }),
                available: Condvar::new(),
                db_path: path.to_path_buf(),
                max_size: pool_size.max(1),
            },
            DbOpenMetrics {
                primary_open_ms,
                primary_configure_ms,
                schema_init_ms,
                total_ms,
            },
        ))
    }

    fn lock(&self) -> Result<SqlitePoolGuard<'_>, String> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| "db pool lock poisoned".to_string())?;
        loop {
            if guard.restoring {
                guard = self
                    .available
                    .wait(guard)
                    .map_err(|_| "db pool lock poisoned".to_string())?;
                continue;
            }
            if let Some(connection) = guard.connections.pop() {
                return Ok(SqlitePoolGuard {
                    pool: self,
                    connection: Some(connection),
                });
            }

            if guard.created < self.max_size {
                guard.created += 1;
                drop(guard);

                match Self::open_configured_connection(self.db_path.as_path()) {
                    Ok(connection) => {
                        return Ok(SqlitePoolGuard {
                            pool: self,
                            connection: Some(connection),
                        });
                    }
                    Err(error) => {
                        let mut retry_guard = self
                            .state
                            .lock()
                            .map_err(|_| "db pool lock poisoned".to_string())?;
                        retry_guard.created = retry_guard.created.saturating_sub(1);
                        self.available.notify_one();
                        return Err(error);
                    }
                }
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
            if let Ok(mut guard) = self.pool.state.lock() {
                guard.connections.push(connection);
                // A restore waiting for the pool to drain shares this condvar.
                self.pool.available.notify_all();
            }
        }
    }
}

pub struct Db {
    conn: Arc<SqlitePool>,
    note_access: Arc<NoteAccessService>,
    search_index_checked: Arc<Mutex<bool>>,
}

impl Clone for Db {
    fn clone(&self) -> Self {
        Self {
            conn: Arc::clone(&self.conn),
            note_access: Arc::clone(&self.note_access),
            search_index_checked: Arc::clone(&self.search_index_checked),
        }
    }
}

impl Db {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        let (conn, _) = SqlitePool::new(&path, SQLITE_POOL_SIZE)?;

        Ok(Self {
            conn: Arc::new(conn),
            note_access: Arc::new(NoteAccessService::new()),
            search_index_checked: Arc::new(Mutex::new(false)),
        })
    }

    pub fn open_with_metrics(path: PathBuf) -> Result<(Self, DbOpenMetrics), String> {
        let (conn, metrics) = SqlitePool::new(&path, SQLITE_POOL_SIZE)?;

        Ok((
            Self {
                conn: Arc::new(conn),
                note_access: Arc::new(NoteAccessService::new()),
                search_index_checked: Arc::new(Mutex::new(false)),
            },
            metrics,
        ))
    }

    pub fn backup_to_sqlite_file(&self, target: &Path) -> Result<(), String> {
        if target.exists() {
            return Err(format!(
                "backup sqlite target already exists: {}",
                target.display()
            ));
        }
        let Some(target_text) = target.to_str() else {
            return Err(format!(
                "backup sqlite target is not valid UTF-8: {}",
                target.display()
            ));
        };
        let conn = self.conn.lock()?;
        conn.execute("VACUUM INTO ?1", [target_text])
            .map_err(|e| format!("Failed to snapshot database '{}': {e}", target.display()))?;
        Ok(())
    }

    /// Replace the live database with a staged SQLite file, without
    /// restarting.
    ///
    /// The staged file is checked and migrated before anything else, as
    /// [`replace_database_file`] does. New database work then waits while work
    /// already in flight finishes, every connection is closed, the files are
    /// swapped and the pool reopens on the restored database. If in-flight
    /// work does not finish within `RESTORE_DRAIN_TIMEOUT`, or any step
    /// fails, the live database stays as it was. Unlocked note keys are
    /// forgotten: they belonged to the replaced notes.
    pub fn restore_from_sqlite_file(&self, staged_file: &Path) -> Result<(), String> {
        prepare_restore_file(staged_file).inspect_err(|_| {
            let _ = std::fs::remove_file(staged_file);
        })?;

        let pool = &self.conn;
        let mut state = pool
            .state
            .lock()
            .map_err(|_| "db pool mutex poisoned".to_string())?;
        state.restoring = true;
        let deadline = Instant::now() + RESTORE_DRAIN_TIMEOUT;
        while state.connections.len() < state.created {
            let now = Instant::now();
            if now >= deadline {
                state.restoring = false;
                pool.available.notify_all();
                let _ = std::fs::remove_file(staged_file);
                return Err("database is busy; restore cancelled".to_string());
            }
            state = pool
                .available
                .wait_timeout(state, deadline - now)
                .map_err(|_| "db pool mutex poisoned".to_string())?
                .0;
        }

        // Closing the last connection checkpoints the WAL into the file.
        state.connections.clear();
        state.created = 0;
        let swapped = swap_in_database_file(&pool.db_path, staged_file);
        // Reopen whichever database is now live: the restored one, or the
        // original after a failed swap.
        let reopened = SqlitePool::open_configured_connection(&pool.db_path).and_then(|conn| {
            initialize_schema(&conn)?;
            state.connections.push(conn);
            state.created = 1;
            Ok(())
        });
        state.restoring = false;
        pool.available.notify_all();
        drop(state);

        swapped?;
        reopened.map_err(|e| format!("failed to reopen db after restore: {e}"))?;
        if let Ok(mut checked) = self.search_index_checked.lock() {
            *checked = false;
        }
        self.note_access.clear_all();
        Ok(())
    }

    fn ensure_search_index_checked(&self) -> Result<(), String> {
        let mut checked = self
            .search_index_checked
            .lock()
            .map_err(|_| "search index check lock poisoned".to_string())?;
        if *checked {
            return Ok(());
        }
        let conn = self.conn.lock()?;
        check_and_heal_search_index(&conn)?;
        *checked = true;
        Ok(())
    }

    pub fn get_note(&self, id: &str) -> Result<Option<Note>, String> {
        let conn = self.conn.lock()?;
        self.load_note_with_access(&conn, id)
    }

    /// Persist a body and return just the new revision.
    ///
    /// Callers that are writing back what they already hold in memory (the
    /// editors' autosave paths) only need `updated_at` for optimistic
    /// concurrency, so this skips the read-back that `save_note` performs to
    /// assemble a full `Note`.
    pub fn save_note_revision(&self, id: &str, body: &str) -> Result<NoteRevision, String> {
        self.save_note_revision_if(id, body, None)
    }

    /// [`Self::save_note_revision`] that only writes while the stored
    /// revision is still `expected_revision` (`None` writes unconditionally).
    /// The check and the write share one write transaction, so two writers
    /// holding the same revision cannot both succeed.
    pub fn save_note_revision_if(
        &self,
        id: &str,
        body: &str,
        expected_revision: Option<&str>,
    ) -> Result<NoteRevision, String> {
        let mut conn = self.conn.lock()?;
        let tx = begin_write(&mut conn)?;
        ensure_note_revision(&tx, id, expected_revision)?;
        let now = now_iso();
        let note_title = derive_note_title_from_body(body);

        if let Some(security) = self.load_note_security(&tx, id)? {
            match security.access_mode {
                NoteAccessMode::None => {}
                NoteAccessMode::Encrypted => {
                    let encryption = self
                        .unlocked_encryption_for(id)
                        .ok_or_else(|| NOTE_LOCKED.to_string())?;
                    self.record_history(&tx, id, &security, body)?;
                    self.write_encrypted_body(&tx, id, &encryption, body, &now)?;
                    tx.commit().map_err(|e| e.to_string())?;
                    return Ok(NoteRevision {
                        id: id.to_string(),
                        updated_at: now,
                    });
                }
            }
            self.record_history(&tx, id, &security, body)?;
        }

        tx.execute(
            "INSERT INTO notes (id, body, note_title, modules_json, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                 body = excluded.body,
                 note_title = CASE WHEN title_pinned = 1 THEN note_title ELSE excluded.note_title END,
                 updated_at = excluded.updated_at",
            rusqlite::params![id, body, note_title, DEFAULT_NOTE_MODULES_JSON, now, now],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;

        Ok(NoteRevision {
            id: id.to_string(),
            updated_at: now,
        })
    }

    pub fn save_note(&self, id: &str, body: &str) -> Result<Note, String> {
        self.save_note_if(id, body, None)
    }

    /// [`Self::save_note`] with the revision check of
    /// [`Self::save_note_revision_if`].
    pub fn save_note_if(
        &self,
        id: &str,
        body: &str,
        expected_revision: Option<&str>,
    ) -> Result<Note, String> {
        let mut conn = self.conn.lock()?;
        let tx = begin_write(&mut conn)?;
        ensure_note_revision(&tx, id, expected_revision)?;
        let now = now_iso();
        let note_title = derive_note_title_from_body(body);

        if let Some(security) = self.load_note_security(&tx, id)? {
            match security.access_mode {
                NoteAccessMode::None => {}
                NoteAccessMode::Encrypted => {
                    let persisted = self
                        .load_note_persistence_row(&tx, id)?
                        .ok_or_else(|| "Note not found".to_string())?;
                    let encryption = self
                        .unlocked_encryption_for(id)
                        .ok_or_else(|| NOTE_LOCKED.to_string())?;
                    self.record_history(&tx, id, &security, body)?;
                    self.write_encrypted_body(&tx, id, &encryption, body, &now)?;
                    tx.commit().map_err(|e| e.to_string())?;
                    return Ok(Note {
                        id: id.to_string(),
                        body: body.to_string(),
                        pinned_title: persisted.pinned_title,
                        modules: parse_note_modules_json(&persisted.modules_json),
                        access_mode: NoteAccessMode::Encrypted,
                        is_unlocked: true,
                        created_at: persisted.created_at,
                        updated_at: now,
                    });
                }
            }
            self.record_history(&tx, id, &security, body)?;
        }

        tx.execute(
            "INSERT INTO notes (id, body, note_title, modules_json, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                 body = excluded.body,
                 note_title = CASE WHEN title_pinned = 1 THEN note_title ELSE excluded.note_title END,
                 updated_at = excluded.updated_at",
            rusqlite::params![id, body, note_title, DEFAULT_NOTE_MODULES_JSON, now, now],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;

        self.load_note_with_access(&conn, id)?
            .ok_or_else(|| "Note not found after save".to_string())
    }

    #[cfg(test)]
    pub fn create_note_with_defaults(
        &self,
        id: &str,
        modules: NoteModules,
        default_encryption_password: Option<&str>,
    ) -> Result<Note, String> {
        self.create_note_with_context(id, modules, default_encryption_password, None)
    }

    pub fn create_note_with_context(
        &self,
        id: &str,
        modules: NoteModules,
        default_encryption_password: Option<&str>,
        working_collection_id: Option<&str>,
    ) -> Result<Note, String> {
        let mut conn = self.conn.lock()?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let now = now_iso();
        let note_title = derive_note_title_from_body("");
        let modules_json = serde_json::to_string(&modules)
            .map_err(|e| format!("Failed to encode note modules: {e}"))?;

        tx.execute(
            "INSERT INTO notes (id, body, note_title, modules_json, created_at, updated_at)
             VALUES (?1, '', ?2, ?3, ?4, ?5)",
            rusqlite::params![id, note_title, modules_json, now, now],
        )
        .map_err(|e| e.to_string())?;

        if let Some(collection_id) = working_collection_id {
            let collection_exists: Option<String> = tx
                .query_row(
                    "SELECT id FROM collections WHERE id = ?1",
                    [collection_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            if collection_exists.is_none() {
                return Err(format!("collection not found: {collection_id}"));
            }

            tx.execute(
                "INSERT OR IGNORE INTO note_collections (note_id, collection_id, created_at)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![id, collection_id, now],
            )
            .map_err(|e| e.to_string())?;

            tx.execute(
                "INSERT OR IGNORE INTO note_tags (note_id, tag_id, created_at)
                 SELECT ?1, tag_id, ?2
                 FROM collection_default_tags
                 WHERE collection_id = ?3",
                rusqlite::params![id, now, collection_id],
            )
            .map_err(|e| e.to_string())?;
        }

        // An encrypted working collection encrypts the note with its key;
        // otherwise a configured default password encrypts it on its own.
        let mut unlocks = match working_collection_id {
            Some(collection_id) => {
                self.seal_for_collection(&tx, collection_id, &[id.to_string()], false)?
            }
            None => Vec::new(),
        };
        if let (true, Some(password)) = (unlocks.is_empty(), default_encryption_password) {
            unlocks.push((
                id.to_string(),
                self.seal_new_note_with_password(&tx, id, password)?,
            ));
        }
        tx.commit().map_err(|e| e.to_string())?;
        self.apply_unlocks(unlocks);
        self.load_note_with_access(&conn, id)?
            .ok_or_else(|| "Note not found after create".to_string())
    }

    pub fn set_note_modules(&self, id: &str, modules: NoteModules) -> Result<Note, String> {
        let conn = self.conn.lock()?;
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

    /// Pins `title` as the note's title, like a file name: edits to the text
    /// no longer change it. An empty title unpins it, so it follows the first
    /// line again. An encrypted note must be unlocked. The text is not
    /// touched, so the revision stays the same for an open editor.
    pub fn set_note_title(&self, id: &str, title: &str) -> Result<(), String> {
        let conn = self.conn.lock()?;
        let security = self
            .load_note_security(&conn, id)?
            .ok_or_else(|| "Note not found".to_string())?;
        if is_note_protected(security.access_mode) && !self.is_note_unlocked(id) {
            return Err(NOTE_LOCKED.to_string());
        }
        let title = title.trim();
        let (title, pinned) = if !title.is_empty() {
            (title.to_string(), true)
        } else if security.access_mode == NoteAccessMode::Encrypted {
            // Unpinned, an encrypted note's title is only known unlocked.
            (String::new(), false)
        } else {
            let body = self
                .load_note_row(&conn, id)?
                .map(|row| row.body)
                .unwrap_or_default();
            (derive_note_title_from_body(&body), false)
        };
        conn.execute(
            "UPDATE notes SET note_title = ?2, title_pinned = ?3 WHERE id = ?1",
            rusqlite::params![id, title, pinned],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn append_note_body(&self, id: &str, body_suffix: &str) -> Result<Note, String> {
        let conn = self.conn.lock()?;
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let now = now_iso();

        let security = self.load_note_security(&tx, id)?;
        if let Some(security) = &security {
            match security.access_mode {
                NoteAccessMode::None => {}
                NoteAccessMode::Encrypted => {
                    let persisted = self
                        .load_note_persistence_row(&tx, id)?
                        .ok_or_else(|| "Note not found".to_string())?;
                    let encryption = self
                        .unlocked_encryption_for(id)
                        .ok_or_else(|| NOTE_LOCKED.to_string())?;
                    let current = self
                        .load_note_plain_body_for_access(&tx, id, security)?
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
                    self.record_history(&tx, id, security, &next_body)?;
                    self.write_encrypted_body(&tx, id, &encryption, &next_body, &now)?;
                    tx.commit().map_err(|e| e.to_string())?;
                    return Ok(Note {
                        id: id.to_string(),
                        body: next_body,
                        pinned_title: persisted.pinned_title,
                        modules: parse_note_modules_json(&persisted.modules_json),
                        access_mode: NoteAccessMode::Encrypted,
                        is_unlocked: true,
                        created_at: persisted.created_at,
                        updated_at: now,
                    });
                }
            }
        }

        let current = self
            .load_note_row(&tx, id)?
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
        if let Some(security) = &security {
            self.record_history(&tx, id, security, &next_body)?;
        }

        tx.execute(
            "INSERT INTO notes (id, body, note_title, modules_json, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                 body = excluded.body,
                 note_title = CASE WHEN title_pinned = 1 THEN note_title ELSE excluded.note_title END,
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
        tx.commit().map_err(|e| e.to_string())?;

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
        let mut conn = self.conn.lock()?;
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
        if let Some(security) = self.load_note_security(&tx, note_id)? {
            self.record_history(&tx, note_id, &security, &next_body)?;
        }

        tx.execute(
            "INSERT INTO notes (id, body, note_title, modules_json, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                 body = excluded.body,
                 note_title = CASE WHEN title_pinned = 1 THEN note_title ELSE excluded.note_title END,
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
        let conn = self.conn.lock()?;
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

        let conn = self.conn.lock()?;
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
        let conn = self.conn.lock()?;
        let rows = self.load_note_access_rows(&conn)?;
        let mut notes = Vec::with_capacity(rows.len());
        for row in &rows {
            notes.push(self.note_from_access_row(row)?);
        }
        Ok(notes)
    }

    pub fn list_notes_meta(&self) -> Result<Vec<NoteSummary>, String> {
        let rows = {
            let conn = self.conn.lock()?;
            self.load_note_summary_rows(&conn)?
        };
        let mut notes = Vec::with_capacity(rows.len());
        for row in &rows {
            notes.push(self.note_summary_from_row(row)?);
        }

        Ok(notes)
    }

    pub fn get_note_meta(&self, id: &str) -> Result<Option<NoteSummary>, String> {
        let row = {
            let conn = self.conn.lock()?;
            self.load_note_summary_row(&conn, id)?
        };
        match row {
            Some(row) => self.note_summary_from_row(&row).map(Some),
            None => Ok(None),
        }
    }

    pub fn get_note_body_preview(
        &self,
        id: &str,
        heading: Option<&str>,
    ) -> Result<Option<String>, String> {
        struct Row {
            access_mode: NoteAccessMode,
            body: String,
        }
        let parse_row = |r: &rusqlite::Row<'_>| {
            Ok(Row {
                access_mode: parse_note_access_mode(r.get::<_, Option<String>>(0)?),
                body: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            })
        };
        let row = {
            let conn = self.conn.lock()?;
            if let Some(heading) = heading {
                conn.query_row(
                    "SELECT access_mode, \
                     CASE WHEN INSTR(lower(body), lower(?2)) = 0 \
                     THEN substr(body, 1, 600) \
                     ELSE substr(body, MAX(1, INSTR(lower(body), lower(?2)) - 50), 2000) \
                     END \
                     FROM notes WHERE id = ?1",
                    rusqlite::params![id, heading],
                    parse_row,
                )
            } else {
                conn.query_row(
                    "SELECT access_mode, substr(body, 1, 600) FROM notes WHERE id = ?1",
                    [id],
                    parse_row,
                )
            }
            .optional()
            .map_err(|e| e.to_string())?
        };
        let Some(row) = row else {
            return Ok(None);
        };
        if matches!(row.access_mode, NoteAccessMode::Encrypted)
            || (is_note_protected(row.access_mode) && !self.is_note_unlocked(id))
        {
            return Ok(Some("[locked]".to_string()));
        }
        if let Some(heading) = heading {
            if let Some(section) = extract_heading_section(&row.body, heading) {
                return Ok(Some(section));
            }
        }
        Ok(Some(row.body))
    }

    /// First `max_chars` characters of a note body for previews, or
    /// `"[locked]"` while the note is protected.
    /// Older versions of a note, newest first.
    pub fn list_note_history(&self, id: &str) -> Result<Vec<NoteVersion>, String> {
        let conn = self.conn.lock()?;
        history_store::list(&conn, id)
    }

    /// Makes the next save of a note start a new history version, so the
    /// text before it (for example before a restore) is kept.
    pub fn end_history_session(&self, id: &str) -> Result<(), String> {
        let conn = self.conn.lock()?;
        history_store::end_session(&conn, id)
    }

    /// The text of a stored version of a note. Protected notes must be
    /// unlocked.
    pub fn note_version_text(&self, id: &str, version_id: i64) -> Result<String, String> {
        let conn = self.conn.lock()?;
        let security = self
            .load_note_security(&conn, id)?
            .ok_or_else(|| "Note not found".to_string())?;
        let current = self
            .load_note_plain_body_for_access(&conn, id, &security)?
            .ok_or_else(|| NOTE_LOCKED.to_string())?;
        let key = self.history_key(id, &security);
        history_store::version_text(&conn, id, version_id, &current, key.as_ref())
    }

    /// Records the stored body of `id` as a history version before `new_body`
    /// replaces it. Call inside the transaction that writes `new_body`.
    fn record_history(
        &self,
        conn: &Connection,
        id: &str,
        security: &NoteSecurityRow,
        new_body: &str,
    ) -> Result<(), String> {
        let Some(old_body) = self.load_note_plain_body_for_access(conn, id, security)? else {
            return Ok(());
        };
        let saved_at: String = conn
            .query_row("SELECT updated_at FROM notes WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .map_err(|e| e.to_string())?;
        let key = self.history_key(id, security);
        history_store::record(conn, id, &old_body, &saved_at, new_body, key.as_ref())
    }

    /// Key the history of an encrypted note is sealed with, while unlocked.
    fn history_key(&self, id: &str, security: &NoteSecurityRow) -> Option<[u8; 32]> {
        if security.access_mode != NoteAccessMode::Encrypted {
            return None;
        }
        self.unlocked_encryption_for(id)
            .map(|unlocked| unlocked.key)
    }

    /// When the note was created (RFC 3339), for note info displays.
    pub fn get_note_created_at(&self, id: &str) -> Result<Option<String>, String> {
        let conn = self.conn.lock()?;
        conn.query_row("SELECT created_at FROM notes WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())
    }

    pub fn get_note_body_head(&self, id: &str, max_chars: usize) -> Result<Option<String>, String> {
        let row = {
            let conn = self.conn.lock()?;
            conn.query_row(
                "SELECT access_mode, substr(body, 1, ?2) FROM notes WHERE id = ?1",
                rusqlite::params![id, max_chars as i64],
                |r| {
                    Ok((
                        parse_note_access_mode(r.get::<_, Option<String>>(0)?),
                        r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?
        };
        Ok(row.map(|(access_mode, body)| {
            if matches!(access_mode, NoteAccessMode::Encrypted)
                || (is_note_protected(access_mode) && !self.is_note_unlocked(id))
            {
                "[locked]".to_string()
            } else {
                body
            }
        }))
    }

    pub fn list_notes_meta_filtered(
        &self,
        collection_id: Option<&str>,
    ) -> Result<Vec<NoteSummary>, String> {
        let rows = {
            let conn = self.conn.lock()?;
            if let Some(collection_id) = collection_id {
                self.load_note_summary_rows_for_collection(&conn, collection_id)?
            } else {
                self.load_note_summary_rows(&conn)?
            }
        };
        let mut notes = Vec::with_capacity(rows.len());
        for row in &rows {
            notes.push(self.note_summary_from_row(row)?);
        }
        Ok(notes)
    }

    pub fn list_collections(&self) -> Result<Vec<Collection>, String> {
        let conn = self.conn.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, description, created_at, updated_at,
                         EXISTS(SELECT 1 FROM collection_keys k WHERE k.collection_id = collections.id)
                 FROM collections
                 ORDER BY name COLLATE NOCASE ASC, created_at ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Collection {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    encrypted: row.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    }

    pub fn get_collection(&self, id: &str) -> Result<Option<Collection>, String> {
        let conn = self.conn.lock()?;
        conn.query_row(
            "SELECT id, name, description, created_at, updated_at,
                     EXISTS(SELECT 1 FROM collection_keys k WHERE k.collection_id = collections.id)
             FROM collections
             WHERE id = ?1",
            [id],
            |row| {
                Ok(Collection {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    encrypted: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    pub fn get_collection_by_name(&self, name: &str) -> Result<Option<Collection>, String> {
        let normalized = normalize_identifier_name(name)?;
        let conn = self.conn.lock()?;
        conn.query_row(
            "SELECT id, name, description, created_at, updated_at,
                     EXISTS(SELECT 1 FROM collection_keys k WHERE k.collection_id = collections.id)
             FROM collections
             WHERE normalized_name = ?1",
            [normalized],
            |row| {
                Ok(Collection {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    encrypted: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    pub fn create_collection(&self, name: &str, description: &str) -> Result<Collection, String> {
        let normalized_name = normalize_identifier_name(name)?;
        let normalized_display_name = normalize_display_name(name)?;
        let mut conn = self.conn.lock()?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let id = ulid::Ulid::new().to_string();
        let now = now_iso();
        tx.execute(
            "INSERT INTO collections (id, name, normalized_name, description, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                id,
                normalized_display_name,
                normalized_name,
                description.trim(),
                now,
                now
            ],
        )
        .map_err(map_unique_constraint_error)?;
        tx.commit().map_err(|e| e.to_string())?;
        self.get_collection(&id)?
            .ok_or_else(|| "collection not found after create".to_string())
    }

    pub fn rename_collection(&self, id: &str, name: &str) -> Result<Collection, String> {
        let normalized_name = normalize_identifier_name(name)?;
        let normalized_display_name = normalize_display_name(name)?;
        let conn = self.conn.lock()?;
        let now = now_iso();
        let changed = conn
            .execute(
                "UPDATE collections
                 SET name = ?2, normalized_name = ?3, updated_at = ?4
                 WHERE id = ?1",
                rusqlite::params![id, normalized_display_name, normalized_name, now],
            )
            .map_err(map_unique_constraint_error)?;
        if changed == 0 {
            return Err("collection not found".to_string());
        }
        self.get_collection(id)?
            .ok_or_else(|| "collection not found after rename".to_string())
    }

    pub fn update_collection_description(
        &self,
        id: &str,
        description: &str,
    ) -> Result<Collection, String> {
        let conn = self.conn.lock()?;
        let now = now_iso();
        let changed = conn
            .execute(
                "UPDATE collections
                 SET description = ?2, updated_at = ?3
                 WHERE id = ?1",
                rusqlite::params![id, description.trim(), now],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err("collection not found".to_string());
        }
        self.get_collection(id)?
            .ok_or_else(|| "collection not found after update".to_string())
    }

    pub fn delete_collection(&self, id: &str) -> Result<bool, String> {
        let conn = self.conn.lock()?;
        ensure_collection_protects_no_notes(&conn, id)?;
        let changed = conn
            .execute("DELETE FROM collections WHERE id = ?1", [id])
            .map_err(|e| e.to_string())?;
        Ok(changed > 0)
    }

    pub fn purge_collection(&self, id: &str) -> Result<usize, String> {
        let mut conn = self.conn.lock()?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;

        let exists: Option<String> = tx
            .query_row("SELECT id FROM collections WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        if exists.is_none() {
            return Err("collection not found".to_string());
        }

        let mut stmt = tx
            .prepare(
                "SELECT DISTINCT note_id
                 FROM note_collections
                 WHERE collection_id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let note_ids = stmt
            .query_map([id], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        drop(stmt);

        let mut deleted_notes = 0usize;
        for note_id in &note_ids {
            let changed = tx
                .execute("DELETE FROM notes WHERE id = ?1", [note_id])
                .map_err(|e| e.to_string())?;
            deleted_notes += changed as usize;
        }
        ensure_collection_protects_no_notes(&tx, id)?;

        let changed = tx
            .execute("DELETE FROM collections WHERE id = ?1", [id])
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err("collection not found".to_string());
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(deleted_notes)
    }

    pub fn list_collection_default_tags(&self, collection_id: &str) -> Result<Vec<String>, String> {
        let conn = self.conn.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT t.name
                 FROM collection_default_tags cdt
                 JOIN tags t ON t.id = cdt.tag_id
                 WHERE cdt.collection_id = ?1
                 ORDER BY t.name COLLATE NOCASE ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([collection_id], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    }

    pub fn set_collection_default_tags(
        &self,
        collection_id: &str,
        tag_names: &[String],
    ) -> Result<Vec<String>, String> {
        let normalized = normalize_tag_name_list(tag_names)?;
        let mut conn = self.conn.lock()?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;

        let collection_exists: Option<String> = tx
            .query_row(
                "SELECT id FROM collections WHERE id = ?1",
                [collection_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if collection_exists.is_none() {
            return Err("collection not found".to_string());
        }

        let now = now_iso();
        let mut tag_ids = Vec::with_capacity(normalized.len());
        for (display_name, normalized_name) in &normalized {
            let existing_id: Option<String> = tx
                .query_row(
                    "SELECT id FROM tags WHERE normalized_name = ?1",
                    [normalized_name],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            let tag_id = if let Some(id) = existing_id {
                id
            } else {
                let new_id = ulid::Ulid::new().to_string();
                tx.execute(
                    "INSERT INTO tags (id, name, normalized_name, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![new_id, display_name, normalized_name, now, now],
                )
                .map_err(map_unique_constraint_error)?;
                new_id
            };
            tag_ids.push(tag_id);
        }

        tx.execute(
            "DELETE FROM collection_default_tags WHERE collection_id = ?1",
            [collection_id],
        )
        .map_err(|e| e.to_string())?;
        for tag_id in &tag_ids {
            tx.execute(
                "INSERT OR IGNORE INTO collection_default_tags (collection_id, tag_id, created_at)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![collection_id, tag_id, now],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        self.list_collection_default_tags(collection_id)
    }

    pub fn list_note_tags(&self, note_id: &str) -> Result<Vec<String>, String> {
        let conn = self.conn.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT t.name
                 FROM note_tags nt
                 JOIN tags t ON t.id = nt.tag_id
                 WHERE nt.note_id = ?1
                 ORDER BY t.name COLLATE NOCASE ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([note_id], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    }

    pub fn get_note_collection_ids(&self, note_id: &str) -> Result<Vec<String>, String> {
        let conn = self.conn.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT collection_id
                 FROM note_collections
                 WHERE note_id = ?1
                 ORDER BY collection_id ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([note_id], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    }

    pub fn set_note_collections(
        &self,
        note_id: &str,
        collection_ids: &[String],
    ) -> Result<Vec<String>, String> {
        let mut conn = self.conn.lock()?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let note_exists: Option<String> = tx
            .query_row("SELECT id FROM notes WHERE id = ?1", [note_id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        if note_exists.is_none() {
            return Err("note not found".to_string());
        }

        let mut deduped = Vec::<String>::new();
        for value in collection_ids {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !deduped.iter().any(|existing| existing == trimmed) {
                deduped.push(trimmed.to_string());
            }
        }

        for collection_id in &deduped {
            let exists: Option<String> = tx
                .query_row(
                    "SELECT id FROM collections WHERE id = ?1",
                    [collection_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            if exists.is_none() {
                return Err(format!("collection not found: {collection_id}"));
            }
        }

        let now = now_iso();
        tx.execute("DELETE FROM note_collections WHERE note_id = ?1", [note_id])
            .map_err(|e| e.to_string())?;
        let mut unlocks = Vec::new();
        for collection_id in &deduped {
            tx.execute(
                "INSERT OR IGNORE INTO note_collections (note_id, collection_id, created_at)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![note_id, collection_id, now],
            )
            .map_err(|e| e.to_string())?;
            unlocks.extend(self.seal_for_collection(
                &tx,
                collection_id,
                &[note_id.to_string()],
                true,
            )?);
        }
        tx.commit().map_err(|e| e.to_string())?;
        self.apply_unlocks(unlocks);
        self.get_note_collection_ids(note_id)
    }

    /// Note counts for the collection browser, in one pass over the tables.
    pub fn collection_note_counts(&self) -> Result<CollectionCounts, String> {
        let conn = self.conn.lock()?;
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        let unsorted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notes n
                 WHERE NOT EXISTS (SELECT 1 FROM note_collections nc WHERE nc.note_id = n.id)",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT collection_id, COUNT(*) FROM note_collections GROUP BY collection_id")
            .map_err(|e| e.to_string())?;
        let per_collection = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        Ok(CollectionCounts {
            total: total as usize,
            unsorted: unsorted as usize,
            per_collection,
        })
    }

    /// Notes that belong to no collection, most recently updated first.
    pub fn list_notes_meta_unsorted(&self) -> Result<Vec<NoteSummary>, String> {
        let rows = {
            let conn = self.conn.lock()?;
            let mut stmt = conn
                .prepare(
                    "SELECT n.id, n.note_title, substr(n.body, 1, 200), n.access_mode, n.updated_at,
                            n.wrapped_key, n.encryption_nonce, n.encrypted_body, n.title_pinned
                     FROM notes n
                     WHERE NOT EXISTS (SELECT 1 FROM note_collections nc WHERE nc.note_id = n.id)
                     ORDER BY n.updated_at DESC",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], map_note_summary_row)
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            rows
        };
        rows.iter()
            .map(|row| self.note_summary_from_row(row))
            .collect()
    }

    /// Adds each note to the collection, keeping its other memberships.
    /// Returns how many notes were not already members.
    pub fn add_notes_to_collection(
        &self,
        collection_id: &str,
        note_ids: &[String],
    ) -> Result<usize, String> {
        let mut conn = self.conn.lock()?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let exists: Option<String> = tx
            .query_row(
                "SELECT id FROM collections WHERE id = ?1",
                [collection_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if exists.is_none() {
            return Err("collection not found".to_string());
        }
        let now = now_iso();
        let mut added = Vec::new();
        for note_id in note_ids {
            let inserted = tx
                .execute(
                    "INSERT OR IGNORE INTO note_collections (note_id, collection_id, created_at)
                     SELECT id, ?2, ?3 FROM notes WHERE id = ?1",
                    rusqlite::params![note_id, collection_id, now],
                )
                .map_err(|e| e.to_string())?;
            if inserted > 0 {
                added.push(note_id.clone());
            }
        }
        let unlocks = self.seal_for_collection(&tx, collection_id, &added, true)?;
        tx.commit().map_err(|e| e.to_string())?;
        self.apply_unlocks(unlocks);
        Ok(added.len())
    }

    /// Removes each note from the collection; the notes themselves stay.
    /// Returns how many memberships were removed.
    pub fn remove_notes_from_collection(
        &self,
        collection_id: &str,
        note_ids: &[String],
    ) -> Result<usize, String> {
        let mut conn = self.conn.lock()?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut removed = 0usize;
        for note_id in note_ids {
            removed += tx
                .execute(
                    "DELETE FROM note_collections WHERE note_id = ?1 AND collection_id = ?2",
                    rusqlite::params![note_id, collection_id],
                )
                .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(removed)
    }

    pub fn get_note_updated_at(&self, id: &str) -> Result<Option<String>, String> {
        let conn = self.conn.lock()?;
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

    #[cfg(test)]
    pub fn search_notes_content(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<NoteSearchResult>, String> {
        self.search_notes_content_filtered(query, limit, None)
    }

    pub fn search_notes_content_filtered(
        &self,
        query: &str,
        limit: usize,
        collection_id: Option<&str>,
    ) -> Result<Vec<NoteSearchResult>, String> {
        let search_terms = parse_search_terms(query);
        let fts_query = build_fts_query_from_terms(&search_terms);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }
        self.ensure_search_index_checked()?;
        let bounded_limit = limit.clamp(1, SEARCH_LIMIT_MAX) as i64;
        let conn = self.conn.lock()?;
        let results = if let Some(collection_id) = collection_id {
            let mut stmt = conn
                .prepare(
                    "SELECT n.id, n.note_title, substr(n.body, 1, ?4),
                            snippet(notes_fts, 2, '[[', ']]', '…', 16),
                            bm25(notes_fts, 0.0, 3.0, 1.0),
                            n.updated_at
                     FROM notes_fts
                     JOIN notes n ON n.rowid = notes_fts.rowid
                     JOIN note_collections nc ON nc.note_id = n.id
                     WHERE notes_fts MATCH ?1
                       AND n.access_mode = 'none'
                       AND nc.collection_id = ?2
                     ORDER BY bm25(notes_fts, 0.0, 3.0, 1.0), n.updated_at DESC
                     LIMIT ?3",
                )
                .map_err(|e| e.to_string())?;
            let raw_rows = stmt
                .query_map(
                    rusqlite::params![
                        fts_query,
                        collection_id,
                        bounded_limit,
                        SEARCH_BODY_PREFIX_CHARS
                    ],
                    |row| {
                        Ok(RawSearchRow {
                            id: row.get(0)?,
                            title: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                            body: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                            fts_snippet: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                            rank: row.get(4)?,
                            updated_at: row.get(5)?,
                        })
                    },
                )
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            raw_rows
                .into_iter()
                .flat_map(|row| expand_search_row(row, &search_terms))
                .collect()
        } else {
            let mut stmt = conn
                .prepare(
                    "SELECT n.id, n.note_title, substr(n.body, 1, ?3),
                            snippet(notes_fts, 2, '[[', ']]', '…', 16),
                            bm25(notes_fts, 0.0, 3.0, 1.0),
                            n.updated_at
                     FROM notes_fts
                     JOIN notes n ON n.rowid = notes_fts.rowid
                     WHERE notes_fts MATCH ?1
                       AND n.access_mode = 'none'
                     ORDER BY bm25(notes_fts, 0.0, 3.0, 1.0), n.updated_at DESC
                     LIMIT ?2",
                )
                .map_err(|e| e.to_string())?;
            let raw_rows = stmt
                .query_map(
                    rusqlite::params![fts_query, bounded_limit, SEARCH_BODY_PREFIX_CHARS],
                    |row| {
                        Ok(RawSearchRow {
                            id: row.get(0)?,
                            title: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                            body: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                            fts_snippet: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                            rank: row.get(4)?,
                            updated_at: row.get(5)?,
                        })
                    },
                )
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            raw_rows
                .into_iter()
                .flat_map(|row| expand_search_row(row, &search_terms))
                .collect()
        };
        Ok(results)
    }

    #[cfg(test)]
    pub fn rebuild_note_search_index(&self) -> Result<(), String> {
        let conn = self.conn.lock()?;
        rebuild_note_search_index_inner(&conn)
    }

    pub fn delete_note(&self, id: &str, password: Option<&str>) -> Result<bool, String> {
        let conn = self.conn.lock()?;
        if let Some(security) = self.load_note_security(&conn, id)? {
            if is_note_protected(security.access_mode) {
                let provided = password.ok_or_else(|| {
                    "password required to delete locked/encrypted note".to_string()
                })?;
                let normalized = normalize_password(provided)?;
                self.note_key_with_password(&conn, &security, &normalized)?;
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
        let conn = self.conn.lock()?;
        query_reminders(&conn, note_id)
    }

    /// Moves the note's reminders to where [`crate::reminders::place_reminders`]
    /// puts them in `lines`, in one transaction, and returns the placed ones
    /// as stored now. A reminder whose line is gone stays stored as it was,
    /// so undoing the deletion brings it back, until a placed reminder needs
    /// its line number. Writes nothing when no reminder moved.
    pub fn reconcile_reminders(
        &self,
        note_id: &str,
        lines: &[String],
    ) -> Result<Vec<Reminder>, String> {
        let mut conn = self.conn.lock()?;
        let reminders = query_reminders(&conn, note_id)?;
        if reminders.is_empty() {
            return Ok(Vec::new());
        }
        let placed = crate::reminders::place_reminders(&reminders, lines);
        let moves: Vec<(usize, usize)> = placed
            .iter()
            .enumerate()
            .filter_map(|(idx, line)| line.map(|line| (idx, line)))
            .filter(|&(idx, line)| {
                let reminder = &reminders[idx];
                reminder.line_number != line as i64 || lines[line - 1] != reminder.line_text
            })
            .collect();

        if !moves.is_empty() {
            let tx = conn.transaction().map_err(|e| e.to_string())?;
            let now = now_iso();
            // Park every moving reminder past all used line numbers first,
            // so moves can swap or chain without colliding.
            let highest = reminders
                .iter()
                .map(|reminder| reminder.line_number)
                .max()
                .unwrap_or(0)
                .max(lines.len() as i64);
            for (offset, &(idx, _)) in moves.iter().enumerate() {
                tx.execute(
                    "UPDATE reminders SET line_number = ?3 WHERE note_id = ?1 AND line_number = ?2",
                    rusqlite::params![
                        note_id,
                        reminders[idx].line_number,
                        highest + 1 + offset as i64
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
            for (offset, &(_, line)) in moves.iter().enumerate() {
                // Only an unplaced reminder can still hold this line number.
                tx.execute(
                    "DELETE FROM reminders WHERE note_id = ?1 AND line_number = ?2",
                    rusqlite::params![note_id, line as i64],
                )
                .map_err(|e| e.to_string())?;
                tx.execute(
                    "UPDATE reminders SET line_number = ?3, line_text = ?4, updated_at = ?5
                     WHERE note_id = ?1 AND line_number = ?2",
                    rusqlite::params![
                        note_id,
                        highest + 1 + offset as i64,
                        line as i64,
                        lines[line - 1],
                        now
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
            tx.commit().map_err(|e| e.to_string())?;
        }

        Ok(reminders
            .into_iter()
            .zip(placed)
            .filter_map(|(mut reminder, line)| {
                let line = line?;
                reminder.line_number = line as i64;
                reminder.line_text = lines[line - 1].clone();
                Some(reminder)
            })
            .collect())
    }

    pub fn upsert_reminder(
        &self,
        note_id: &str,
        line_number: i64,
        remind_at_ms: i64,
        display_at: &str,
        line_text: &str,
    ) -> Result<Reminder, String> {
        let conn = self.conn.lock()?;
        let now = now_iso();

        conn.execute(
            "INSERT INTO reminders (
                note_id, line_number, remind_at_ms, display_at, line_text, reminded_at_ms, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7)
            ON CONFLICT(note_id, line_number) DO UPDATE SET
                remind_at_ms = excluded.remind_at_ms,
                display_at = excluded.display_at,
                line_text = excluded.line_text,
                reminded_at_ms = NULL,
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

    pub fn mark_reminder_reminded(
        &self,
        note_id: &str,
        line_number: i64,
        reminded_at_ms: i64,
    ) -> Result<Option<Reminder>, String> {
        let conn = self.conn.lock()?;
        let now = now_iso();
        conn.execute(
            "UPDATE reminders
             SET reminded_at_ms = ?3, updated_at = ?4
             WHERE note_id = ?1 AND line_number = ?2",
            rusqlite::params![note_id, line_number, reminded_at_ms, now],
        )
        .map_err(|e| e.to_string())?;

        load_reminder(&conn, note_id, line_number)
    }

    pub fn delete_reminder(&self, note_id: &str, line_number: i64) -> Result<bool, String> {
        let conn = self.conn.lock()?;
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
        let mut conn = self.conn.lock()?;
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

    pub fn get_ingest_offset(&self, source_key: &str) -> Result<Option<i64>, String> {
        let conn = self.conn.lock()?;
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
        let conn = self.conn.lock()?;
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

    pub fn reserve_note_image(
        &self,
        note_id: &str,
        file_name: Option<&str>,
        mime_type: Option<&str>,
    ) -> Result<String, String> {
        let conn = self.conn.lock()?;
        self.ensure_note_allows_image_mutation(&conn, note_id)?;
        let image_id = ulid::Ulid::new().to_string();
        let now = now_iso();
        let effective_mime = select_image_mime_type(file_name, mime_type);
        conn.execute(
            "INSERT INTO note_images (
                id, note_id, file_name, mime_type, image_bytes, byte_len, status, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, NULL, 0, 'pending', ?5, ?6)",
            rusqlite::params![image_id, note_id, file_name, effective_mime, now, now],
        )
        .map_err(|e| e.to_string())?;
        Ok(image_id)
    }

    pub fn write_note_image_bytes(
        &self,
        note_id: &str,
        image_id: &str,
        file_name: Option<&str>,
        mime_type: Option<&str>,
        image_bytes: &[u8],
    ) -> Result<(), String> {
        if image_bytes.is_empty() {
            return Err("Image payload is empty".to_string());
        }
        let conn = self.conn.lock()?;
        self.ensure_note_allows_image_mutation(&conn, note_id)?;
        let effective_mime = select_image_mime_type(file_name, mime_type);
        let now = now_iso();
        let changed = conn
            .execute(
                "UPDATE note_images
                 SET file_name = COALESCE(?3, file_name),
                     mime_type = ?4,
                     image_bytes = ?5,
                     byte_len = ?6,
                     status = 'ready',
                     updated_at = ?7
                 WHERE id = ?1 AND note_id = ?2",
                rusqlite::params![
                    image_id,
                    note_id,
                    file_name,
                    effective_mime,
                    image_bytes,
                    image_bytes.len() as i64,
                    now,
                ],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err("image placeholder not found".to_string());
        }
        Ok(())
    }

    pub fn resolve_note_image_data_url(
        &self,
        note_id: &str,
        image_id: &str,
    ) -> Result<Option<String>, String> {
        Ok(self
            .read_note_image_bytes(note_id, image_id)?
            .map(|(mime, bytes)| format!("data:{mime};base64,{}", BASE64_STANDARD.encode(bytes))))
    }

    /// MIME type and bytes of a ready stored image.
    pub fn read_note_image_bytes(
        &self,
        note_id: &str,
        image_id: &str,
    ) -> Result<Option<(String, Vec<u8>)>, String> {
        let conn = self.conn.lock()?;
        let row = conn
            .query_row(
                "SELECT mime_type, image_bytes
                 FROM note_images
                 WHERE id = ?1 AND note_id = ?2 AND status = 'ready'",
                rusqlite::params![image_id, note_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<Vec<u8>>>(1)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((mime_type, Some(bytes))) = row else {
            return Ok(None);
        };
        if bytes.is_empty() {
            return Ok(None);
        }
        let mime = normalize_mime_value(mime_type.unwrap_or_else(|| "image/png".to_string()));
        Ok(Some((mime, bytes)))
    }

    /// `(updated_at, byte_len)` of a ready stored image, read without loading
    /// the image bytes so callers can cheaply detect replacements.
    pub fn note_image_stamp(
        &self,
        note_id: &str,
        image_id: &str,
    ) -> Result<Option<(String, i64)>, String> {
        let conn = self.conn.lock()?;
        conn.query_row(
            "SELECT updated_at, byte_len
             FROM note_images
             WHERE id = ?1 AND note_id = ?2 AND status = 'ready'",
            rusqlite::params![image_id, note_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    row.get::<_, Option<i64>>(1)?.unwrap_or_default(),
                ))
            },
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    fn ensure_note_allows_image_mutation(
        &self,
        conn: &Connection,
        note_id: &str,
    ) -> Result<(), String> {
        let security = self
            .load_note_security(conn, note_id)?
            .ok_or_else(|| "Note not found".to_string())?;
        if security.access_mode != NoteAccessMode::None && !self.is_note_unlocked(note_id) {
            return Err("note is locked; unlock first".to_string());
        }
        Ok(())
    }

    fn is_note_unlocked(&self, id: &str) -> bool {
        self.note_access.is_unlocked(id)
    }

    fn unlocked_encryption_for(&self, id: &str) -> Option<NoteAccessGrant> {
        self.note_access.session(id)
    }

    fn load_note_security(
        &self,
        conn: &Connection,
        id: &str,
    ) -> Result<Option<NoteSecurityRow>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT access_mode, encryption_salt, wrapped_key, key_collection_id,
                        encryption_nonce, encrypted_body
                 FROM notes WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let row = stmt
            .query_row([id], |row| {
                Ok(NoteSecurityRow {
                    access_mode: parse_note_access_mode(row.get::<_, Option<String>>(0)?),
                    encryption_salt: row.get(1)?,
                    wrapped_key: row.get(2)?,
                    key_collection_id: row.get(3)?,
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
                "SELECT modules_json, created_at, CASE WHEN title_pinned = 1 THEN note_title END
                 FROM notes
                 WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let row = stmt
            .query_row([id], |row| {
                Ok(NotePersistenceRow {
                    modules_json: row.get(0)?,
                    created_at: row.get(1)?,
                    pinned_title: row.get(2)?,
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
                "SELECT id, note_title, substr(body, 1, 200), access_mode, updated_at,
                        wrapped_key, encryption_nonce, encrypted_body, title_pinned
                 FROM notes
                 WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let row = stmt
            .query_row([id], map_note_summary_row)
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(row)
    }

    fn load_note_summary_rows(&self, conn: &Connection) -> Result<Vec<NoteSummaryRow>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, note_title, substr(body, 1, 200), access_mode, updated_at,
                        wrapped_key, encryption_nonce, encrypted_body, title_pinned
                 FROM notes
                 ORDER BY updated_at DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], map_note_summary_row)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    }

    fn load_note_summary_rows_for_collection(
        &self,
        conn: &Connection,
        collection_id: &str,
    ) -> Result<Vec<NoteSummaryRow>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT n.id, n.note_title, substr(n.body, 1, 200), n.access_mode, n.updated_at,
                        n.wrapped_key, n.encryption_nonce, n.encrypted_body, n.title_pinned
                 FROM notes n
                 JOIN note_collections nc ON nc.note_id = n.id
                 WHERE nc.collection_id = ?1
                 ORDER BY n.updated_at DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([collection_id], map_note_summary_row)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    }

    fn load_note_access_row(
        &self,
        conn: &Connection,
        id: &str,
    ) -> Result<Option<NoteAccessRow>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, body, modules_json, access_mode,
                        wrapped_key, encryption_nonce, encrypted_body, created_at, updated_at,
                        CASE WHEN title_pinned = 1 THEN note_title END
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
                    wrapped_key: row.get(4)?,
                    encryption_nonce: row.get(5)?,
                    encrypted_body: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                    pinned_title: row.get(9)?,
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
                        wrapped_key, encryption_nonce, encrypted_body, created_at, updated_at,
                        CASE WHEN title_pinned = 1 THEN note_title END
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
                    wrapped_key: row.get(4)?,
                    encryption_nonce: row.get(5)?,
                    encrypted_body: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                    pinned_title: row.get(9)?,
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
            pinned_title: row.pinned_title.clone(),
            modules: parse_note_modules_json(&row.modules_json),
            access_mode: row.access_mode,
            is_unlocked,
            created_at: row.created_at.clone(),
            updated_at: row.updated_at.clone(),
        })
    }

    fn note_summary_from_row(&self, row: &NoteSummaryRow) -> Result<NoteSummary, String> {
        let summary = |title: String, body_prefix: String, is_unlocked: bool| NoteSummary {
            id: row.id.clone(),
            title,
            body_prefix,
            access_mode: row.access_mode,
            is_unlocked,
            updated_at: row.updated_at.clone(),
        };
        match row.access_mode {
            NoteAccessMode::None => Ok(summary(
                row.note_title.clone(),
                row.body_prefix.clone(),
                true,
            )),
            NoteAccessMode::Encrypted => {
                // A pinned title is stored in the clear; otherwise the title
                // is part of the text and only known unlocked.
                let pinned = row.title_pinned.then(|| row.note_title.clone());
                Ok(match self.decrypt_summary_body(row)? {
                    Some(body) => summary(
                        pinned.unwrap_or_else(|| derive_note_title_from_body(&body)),
                        body.chars().take(200).collect(),
                        true,
                    ),
                    None => summary(
                        pinned.unwrap_or_else(|| ENCRYPTED_NOTE_TITLE.to_string()),
                        "[locked]".to_string(),
                        false,
                    ),
                })
            }
        }
    }

    /// The text of an encrypted summary row while its note is unlocked; a
    /// key that no longer fits ends the unlock.
    fn decrypt_summary_body(&self, row: &NoteSummaryRow) -> Result<Option<String>, String> {
        let Some(encryption) = self.unlocked_encryption_for(row.id.as_str()) else {
            return Ok(None);
        };
        if row.wrapped_key.as_deref() != Some(encryption.wrapped_key.as_slice()) {
            self.note_access.clear(row.id.as_str());
            return Ok(None);
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
            Ok(body) => Ok(Some(body)),
            Err(_) => {
                self.note_access.clear(row.id.as_str());
                Ok(None)
            }
        }
    }

    fn load_plain_body_from_access_row(&self, row: &NoteAccessRow) -> Result<String, String> {
        match row.access_mode {
            NoteAccessMode::None => Ok(row.body.clone()),
            NoteAccessMode::Encrypted => {
                let encryption = self
                    .unlocked_encryption_for(row.id.as_str())
                    .ok_or_else(|| NOTE_LOCKED.to_string())?;
                if row.wrapped_key.as_deref() != Some(encryption.wrapped_key.as_slice()) {
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

        let Some(NoteAccessGrant { key, wrapped_key }) = self.note_access.session(id) else {
            return Ok(None);
        };
        if security.wrapped_key.as_deref() != Some(wrapped_key.as_slice()) {
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
    modules_json: String,
    created_at: String,
    pinned_title: Option<String>,
}

#[derive(Debug, Clone)]
struct NoteSummaryRow {
    id: String,
    /// The shown title: pinned, derived from the text, or empty for an
    /// encrypted note whose title is not pinned.
    note_title: String,
    title_pinned: bool,
    body_prefix: String,
    access_mode: NoteAccessMode,
    updated_at: String,
    wrapped_key: Option<Vec<u8>>,
    encryption_nonce: Option<Vec<u8>>,
    encrypted_body: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
struct NoteAccessRow {
    id: String,
    body: String,
    modules_json: String,
    access_mode: NoteAccessMode,
    wrapped_key: Option<Vec<u8>>,
    encryption_nonce: Option<Vec<u8>>,
    encrypted_body: Option<Vec<u8>>,
    created_at: String,
    updated_at: String,
    pinned_title: Option<String>,
}

/// Stored module flags; unreadable JSON falls back to the defaults.
fn parse_note_modules_json(raw: &str) -> NoteModules {
    serde_json::from_str::<NoteModules>(raw).unwrap_or_default()
}

fn map_note_summary_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteSummaryRow> {
    Ok(NoteSummaryRow {
        id: row.get(0)?,
        note_title: row.get(1)?,
        body_prefix: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        access_mode: parse_note_access_mode(row.get::<_, Option<String>>(3)?),
        updated_at: row.get(4)?,
        wrapped_key: row.get(5)?,
        encryption_nonce: row.get(6)?,
        encrypted_body: row.get(7)?,
        title_pinned: row.get(8)?,
    })
}

fn extract_heading_section(body: &str, heading: &str) -> Option<String> {
    let heading_lower = heading.to_lowercase();
    let mut in_section = false;
    let mut section_level = 0usize;
    let mut lines = Vec::new();
    for line in body.lines() {
        if line.starts_with('#') {
            let level = line.chars().take_while(|&c| c == '#').count();
            if in_section {
                if level <= section_level {
                    break;
                }
                // Sub-heading within the section — include stripped text
                lines.push(line.trim_start_matches('#').trim().to_string());
                if lines.len() >= 15 {
                    break;
                }
            } else {
                let title = line.trim_start_matches('#').trim();
                if title.to_lowercase().contains(&heading_lower) {
                    in_section = true;
                    section_level = level;
                    lines.push(title.to_string());
                }
            }
        } else if in_section {
            lines.push(line.to_string());
            if lines.len() >= 15 {
                break;
            }
        }
    }
    if in_section && !lines.is_empty() {
        Some(lines.join("\n"))
    } else {
        None
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
        "encrypted" => NoteAccessMode::Encrypted,
        _ => NoteAccessMode::None,
    }
}

fn normalize_mime_value(value: String) -> String {
    let trimmed = value.trim().to_ascii_lowercase();
    if trimmed.starts_with("image/") && trimmed.len() > "image/".len() {
        trimmed
    } else {
        "image/png".to_string()
    }
}

fn select_image_mime_type(file_name: Option<&str>, mime_type: Option<&str>) -> String {
    if let Some(raw) = mime_type {
        let normalized = normalize_mime_value(raw.to_string());
        if normalized != "image/png" || raw.trim().eq_ignore_ascii_case("image/png") {
            return normalized;
        }
    }
    if let Some(name) = file_name {
        let ext = Path::new(name)
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .trim_start_matches('.')
            .to_ascii_lowercase();
        let from_ext = match ext.as_str() {
            "jpg" | "jpeg" => Some("image/jpeg"),
            "png" => Some("image/png"),
            "gif" => Some("image/gif"),
            "webp" => Some("image/webp"),
            "bmp" => Some("image/bmp"),
            "svg" | "svgz" => Some("image/svg+xml"),
            "avif" => Some("image/avif"),
            _ => None,
        };
        if let Some(value) = from_ext {
            return value.to_string();
        }
    }
    "image/png".to_string()
}

fn normalize_display_name(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("name must not be empty".to_string());
    }
    Ok(trimmed.to_string())
}

fn normalize_identifier_name(value: &str) -> Result<String, String> {
    let display = normalize_display_name(value)?;
    Ok(display.to_ascii_lowercase())
}

fn normalize_tag_name_list(raw: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut out: Vec<(String, String)> = Vec::new();
    for name in raw {
        let display = normalize_display_name(name)?;
        let normalized = display.to_ascii_lowercase();
        if !out.iter().any(|(_, existing)| existing == &normalized) {
            out.push((display, normalized));
        }
    }
    Ok(out)
}

fn map_unique_constraint_error(error: rusqlite::Error) -> String {
    let text = error.to_string();
    if text.contains("collections.normalized_name")
        || text.contains("idx_collections_normalized_name")
    {
        return "collection name already exists".to_string();
    }
    if text.contains("tags.normalized_name") || text.contains("idx_tags_normalized_name") {
        return "tag name already exists".to_string();
    }
    text
}

/// Creates missing tables and brings the data up to date.
fn initialize_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!("../../migrations/0001_init.sql"))
        .map_err(|e| format!("Failed to initialize schema: {e}"))?;
    migrate(conn)
}

/// One-off data changes, each run once per database and recorded in
/// `user_version`.
fn migrate(conn: &Connection) -> Result<(), String> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    if version < 1 {
        // App-only locks are gone. A locked note's text was never encrypted,
        // so it becomes a plain note. Encrypted notes stop storing their
        // title, which is part of the encrypted text.
        conn.execute_batch(
            "BEGIN;
             UPDATE notes SET access_mode = 'none' WHERE access_mode = 'locked';
             UPDATE notes SET note_title = '' WHERE access_mode = 'encrypted';
             PRAGMA user_version = 1;
             COMMIT;",
        )
        .map_err(|e| format!("Failed to migrate database: {e}"))?;
    }
    // New databases get the columns below from the schema, older ones here.
    if version < 2 {
        // Titles can be pinned by hand.
        let add_column = if has_column(conn, "notes", "title_pinned")? {
            ""
        } else {
            "ALTER TABLE notes ADD COLUMN title_pinned INTEGER NOT NULL DEFAULT 0;"
        };
        conn.execute_batch(&format!(
            "BEGIN; {add_column} PRAGMA user_version = 2; COMMIT;"
        ))
        .map_err(|e| format!("Failed to migrate database: {e}"))?;
    }
    if version < 3 {
        // Encrypted notes keep a random key wrapped by their password or by
        // an encrypted collection's key. A note encrypted the old way, with
        // a key derived straight from its password, cannot be converted
        // without that password.
        let columns = if has_column(conn, "notes", "wrapped_key")? {
            ""
        } else {
            let old: Option<(String, String)> = conn
                .query_row(
                    "SELECT id, note_title FROM notes WHERE access_mode = 'encrypted' LIMIT 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            if let Some((id, title)) = old {
                let name = if title.is_empty() { id } else { title };
                return Err(format!(
                    "note \"{name}\" is encrypted in an old format: decrypt it with the \
                     previous Slate version (:note decrypt <password>), then start this one"
                ));
            }
            "ALTER TABLE notes ADD COLUMN wrapped_key BLOB;
             ALTER TABLE notes ADD COLUMN key_collection_id TEXT REFERENCES collections(id);
             ALTER TABLE notes DROP COLUMN password_salt;
             ALTER TABLE notes DROP COLUMN password_hash;"
        };
        conn.execute_batch(&format!(
            "BEGIN; {columns}
             CREATE INDEX IF NOT EXISTS idx_notes_key_collection ON notes(key_collection_id);
             PRAGMA user_version = 3; COMMIT;"
        ))
        .map_err(|e| format!("Failed to migrate database: {e}"))?;
    }
    if version < 4 {
        // Wiki links name the whole note id instead of its first 8 characters.
        rewrite_short_wiki_links(conn).map_err(|e| format!("Failed to migrate database: {e}"))?;
    }
    Ok(())
}

/// Rewrites links that name the first 8 characters of a note id
/// (`[[01KP0YD0]]`, `[[01KP0YD0#Heading]]`, `[[01KP0YD0]].var`) to the whole
/// id. A prefix several notes share goes to the most recently updated one,
/// the note such a link opened before. Encrypted bodies stay as they are.
fn rewrite_short_wiki_links(conn: &Connection) -> Result<(), String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let mut by_prefix: FxHashMap<String, String> = FxHashMap::default();
    {
        let mut stmt = tx
            .prepare("SELECT id FROM notes ORDER BY updated_at DESC, id ASC")
            .map_err(|e| e.to_string())?;
        let ids = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        for id in ids {
            let id = id.map_err(|e| e.to_string())?;
            if let Some(prefix) = id.get(..8) {
                // A note whose whole id is the prefix keeps its own links.
                let exact = id.len() == 8;
                let key = prefix.to_ascii_lowercase();
                if exact || !by_prefix.contains_key(&key) {
                    by_prefix.insert(key, id);
                }
            }
        }
    }

    let short_link =
        regex::Regex::new(r"\[\[([A-Za-z0-9]{8})([\]#|])").map_err(|e| e.to_string())?;
    let rewrites = {
        let mut stmt = tx
            .prepare(
                "SELECT id, body FROM notes
                 WHERE access_mode = 'none' AND instr(body, '[[') > 0",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        let mut rewrites = Vec::new();
        for row in rows {
            let (id, body) = row.map_err(|e| e.to_string())?;
            let rewritten = short_link.replace_all(&body, |caps: &regex::Captures<'_>| {
                match by_prefix.get(&caps[1].to_ascii_lowercase()) {
                    Some(full_id) => format!("[[{full_id}{}", &caps[2]),
                    None => caps[0].to_string(),
                }
            });
            if rewritten != body {
                rewrites.push((id, rewritten.into_owned()));
            }
        }
        rewrites
    };
    for (id, body) in rewrites {
        tx.execute(
            "UPDATE notes
             SET body = ?2,
                 note_title = CASE WHEN title_pinned = 1 THEN note_title ELSE ?3 END
             WHERE id = ?1",
            rusqlite::params![id, body, derive_note_title_from_body(&body)],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.execute_batch("PRAGMA user_version = 4;")
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

/// Error for a save whose expected revision is no longer the stored one.
const NOTE_REVISION_CONFLICT: &str = "note changed since last load; use :w! to force save";

/// Starts a transaction that holds the write lock from its first statement,
/// so what it reads cannot change before it writes.
fn begin_write(conn: &mut Connection) -> Result<rusqlite::Transaction<'_>, String> {
    conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())
}

/// Fails with [`NOTE_REVISION_CONFLICT`] unless the note's stored revision is
/// `expected` (a missing note has none); `None` accepts any revision.
fn ensure_note_revision(conn: &Connection, id: &str, expected: Option<&str>) -> Result<(), String> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let current: Option<String> = conn
        .query_row("SELECT updated_at FROM notes WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())?;
    if current.as_deref() == Some(expected) {
        Ok(())
    } else {
        Err(NOTE_REVISION_CONFLICT.to_string())
    }
}

/// Path of a file SQLite keeps next to `db` (`-wal`, `-shm`).
fn sidecar_path(db: &Path, suffix: &str) -> PathBuf {
    let mut path = db.as_os_str().to_owned();
    path.push(suffix);
    PathBuf::from(path)
}

/// Where a restore keeps the database it replaced.
pub fn database_before_restore_path(db: &Path) -> PathBuf {
    sidecar_path(db, ".before-restore")
}

/// Checks that `path` is an intact Slate database this version can open and
/// brings it up to the current schema, so a restore never swaps in a file the
/// app would then fail on.
fn prepare_restore_file(path: &Path) -> Result<(), String> {
    let invalid = |detail: String| format!("not a valid Slate backup: {detail}");
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| invalid(e.to_string()))?;
    let check: String = conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|e| invalid(e.to_string()))?;
    if check != "ok" {
        return Err(invalid(check));
    }
    let has_notes: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'notes')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| invalid(e.to_string()))?;
    if !has_notes {
        return Err(invalid("it has no notes table".to_string()));
    }
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| invalid(e.to_string()))?;
    if version > SCHEMA_VERSION {
        return Err("backup was made by a newer Slate version".to_string());
    }
    initialize_schema(&conn).map_err(invalid)?;
    conn.close().map_err(|(_, e)| invalid(e.to_string()))
}

/// Renames the database at `from`, with its WAL, to `to`, replacing what
/// was there. A leftover shared-memory file is dropped; SQLite rebuilds it.
fn move_database_file(from: &Path, to: &Path) -> std::io::Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let target = sidecar_path(to, suffix);
        if target.exists() {
            std::fs::remove_file(target)?;
        }
    }
    if from.exists() {
        std::fs::rename(from, to)?;
    }
    let wal = sidecar_path(from, "-wal");
    if wal.exists() {
        std::fs::rename(wal, sidecar_path(to, "-wal"))?;
    }
    let _ = std::fs::remove_file(sidecar_path(from, "-shm"));
    Ok(())
}

/// Replaces the database at `live` with the backup at `staged`, keeping the
/// replaced one at [`database_before_restore_path`]. The backup is checked
/// and migrated first; when that or the swap fails, `live` is left as it was.
/// `staged` is consumed either way. No connection to `live` may be open.
pub fn replace_database_file(live: &Path, staged: &Path) -> Result<(), String> {
    if let Err(error) = prepare_restore_file(staged) {
        let _ = std::fs::remove_file(staged);
        return Err(error);
    }
    swap_in_database_file(live, staged)
}

/// The swap step of [`replace_database_file`], for a backup already prepared.
fn swap_in_database_file(live: &Path, staged: &Path) -> Result<(), String> {
    let result = (|| {
        let previous = database_before_restore_path(live);
        move_database_file(live, &previous)
            .map_err(|e| format!("failed to set the current database aside: {e}"))?;
        if let Err(e) = std::fs::rename(staged, live) {
            let _ = move_database_file(&previous, live);
            return Err(format!("failed to swap database file during restore: {e}"));
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(staged);
    result
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT COUNT(1) FROM pragma_table_info(?1) WHERE name = ?2",
        [table, column],
        |row| row.get::<_, i64>(0).map(|count| count > 0),
    )
    .map_err(|e| e.to_string())
}

fn is_note_protected(mode: NoteAccessMode) -> bool {
    !matches!(mode, NoteAccessMode::None)
}

fn query_reminders(conn: &Connection, note_id: &str) -> Result<Vec<Reminder>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT note_id, line_number, remind_at_ms, display_at, line_text, reminded_at_ms, created_at, updated_at
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
                reminded_at_ms: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(reminders)
}

fn load_reminder(
    conn: &Connection,
    note_id: &str,
    line_number: i64,
) -> Result<Option<Reminder>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT note_id, line_number, remind_at_ms, display_at, line_text, reminded_at_ms, created_at, updated_at
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
                reminded_at_ms: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })
        .optional()
        .map_err(|e| e.to_string())?;

    Ok(reminder)
}

/// Seconds since the Unix epoch of a stored RFC 3339 timestamp.
pub fn timestamp_epoch(value: &str) -> Option<i64> {
    OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(OffsetDateTime::unix_timestamp)
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
                    if is_search_char(ch) {
                        chars.next();
                        token.push(ch);
                        continue;
                    }
                    // Treat punctuation as a token boundary instead of silently
                    // stripping and merging neighboring terms (e.g. "it's" ->
                    // "it" + "s", not "its").
                    chars.next();
                    break;
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

// Raw SQL row before per-line expansion.
struct RawSearchRow {
    id: String,
    title: String,
    body: String,
    fts_snippet: String,
    rank: f64,
    updated_at: String,
}

// Expand one SQL row (one note) into up to SEARCH_MAX_RESULTS_PER_NOTE results,
// one per matching line. Falls back to the FTS5 snippet at line 1 when no
// per-line match is found in the body prefix (e.g. title-only hit).
fn expand_search_row(row: RawSearchRow, terms: &[SearchTerm]) -> Vec<NoteSearchResult> {
    let mut results = Vec::new();
    if !terms.is_empty() {
        // Lowercase each term once instead of allocating per-line per-term.
        let lowered_terms: Vec<String> = terms
            .iter()
            .map(|term| match term {
                SearchTerm::Phrase(phrase) => phrase.to_lowercase(),
                SearchTerm::Prefix(prefix) => prefix.to_lowercase(),
            })
            .collect();
        for (idx, line) in row.body.lines().enumerate() {
            if results.len() >= SEARCH_MAX_RESULTS_PER_NOTE {
                break;
            }
            let lower = line.to_lowercase();
            if lowered_terms.iter().all(|needle| lower.contains(needle)) {
                results.push(NoteSearchResult {
                    id: row.id.clone(),
                    title: row.title.clone(),
                    snippet: line.to_string(),
                    line_number: idx + 1,
                    rank: row.rank,
                    updated_at: row.updated_at.clone(),
                });
            }
        }
    }
    if results.is_empty() {
        results.push(NoteSearchResult {
            id: row.id,
            title: row.title,
            snippet: row.fts_snippet,
            line_number: 1,
            rank: row.rank,
            updated_at: row.updated_at,
        });
    }
    results
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

    fn remove_db_files(path: &Path) {
        for suffix in ["", "-wal", "-shm", ".before-restore"] {
            let _ = fs::remove_file(sidecar_path(path, suffix));
        }
    }

    fn reminder_lines(db: &Db, note_id: &str) -> Vec<(i64, String)> {
        db.list_reminders(note_id)
            .expect("list reminders")
            .into_iter()
            .map(|r| (r.line_number, r.line_text))
            .collect()
    }

    fn owned(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn reconcile_reminders_swaps_lines_in_one_pass() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        db.save_note("n1", "a\nb").expect("note");
        db.upsert_reminder("n1", 1, 1, "d", "a").expect("a");
        db.upsert_reminder("n1", 2, 2, "d", "b").expect("b");

        let placed = db
            .reconcile_reminders("n1", &owned(&["b", "a"]))
            .expect("reconcile");
        assert_eq!(placed.len(), 2);
        assert_eq!(
            reminder_lines(&db, "n1"),
            vec![(1, "b".to_string()), (2, "a".to_string())]
        );
        // The remind times travelled with their lines.
        let by_line = db.list_reminders("n1").expect("list");
        assert_eq!(by_line[0].remind_at_ms, 2);
        assert_eq!(by_line[1].remind_at_ms, 1);

        drop(db);
        remove_db_files(&path);
    }

    #[test]
    fn reconcile_reminders_keeps_unplaced_reminders_until_their_line_is_needed() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        db.save_note("n1", "buy milk\npay rent").expect("note");
        db.upsert_reminder("n1", 1, 1, "d", "buy milk")
            .expect("milk");
        db.upsert_reminder("n1", 2, 2, "d", "pay rent")
            .expect("rent");

        // "buy milk" replaced by another line: its reminder stays stored,
        // unplaced, so an undo can bring it back.
        let placed = db
            .reconcile_reminders("n1", &owned(&["call Ana", "pay rent"]))
            .expect("reconcile");
        assert_eq!(placed.len(), 1);
        assert_eq!(placed[0].line_text, "pay rent");
        assert_eq!(
            reminder_lines(&db, "n1"),
            vec![(1, "buy milk".to_string()), (2, "pay rent".to_string())]
        );

        // Once "pay rent" moves onto line 1, the unplaced reminder gives way.
        let placed = db
            .reconcile_reminders("n1", &owned(&["pay rent"]))
            .expect("reconcile");
        assert_eq!(placed.len(), 1);
        assert_eq!(reminder_lines(&db, "n1"), vec![(1, "pay rent".to_string())]);

        drop(db);
        remove_db_files(&path);
    }

    #[test]
    fn fresh_database_records_the_latest_schema_version() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        let version: i64 = db
            .conn
            .lock()
            .expect("conn")
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("version");
        assert_eq!(version, SCHEMA_VERSION);
        drop(db);
        remove_db_files(&path);
    }

    #[test]
    fn restore_rejects_invalid_backup_and_keeps_live_database() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        db.save_note("n1", "keep me").expect("seed");

        let staged = temp_db_path();
        fs::write(&staged, b"definitely not sqlite").expect("garbage");
        let error = db.restore_from_sqlite_file(&staged).expect_err("rejected");
        assert!(error.contains("not a valid Slate backup"), "{error}");
        assert!(!staged.exists(), "a rejected backup is discarded");
        assert_eq!(
            db.get_note("n1").expect("read").expect("note").body,
            "keep me"
        );

        // A database without Slate's tables is rejected too.
        Connection::open(&staged)
            .expect("other db")
            .execute_batch("CREATE TABLE other (x);")
            .expect("other table");
        let error = db.restore_from_sqlite_file(&staged).expect_err("rejected");
        assert!(error.contains("no notes table"), "{error}");

        // So is a backup from a newer schema.
        let newer = Db::open(staged.clone()).expect("newer db");
        newer.save_note("n2", "future").expect("seed newer");
        drop(newer);
        Connection::open(&staged)
            .expect("raw")
            .execute_batch(&format!("PRAGMA user_version = {};", SCHEMA_VERSION + 1))
            .expect("bump version");
        let error = db.restore_from_sqlite_file(&staged).expect_err("rejected");
        assert!(error.contains("newer Slate"), "{error}");
        assert_eq!(
            db.get_note("n1").expect("read").expect("note").body,
            "keep me"
        );

        drop(db);
        remove_db_files(&path);
        remove_db_files(&staged);
    }

    #[test]
    fn restore_swaps_in_backup_and_keeps_replaced_database() {
        let source_path = temp_db_path();
        let source = Db::open(source_path.clone()).expect("source opens");
        source
            .save_note("backup-note", "from backup")
            .expect("seed");
        let staged = temp_db_path();
        source.backup_to_sqlite_file(&staged).expect("snapshot");
        drop(source);

        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        db.save_note("live-note", "replaced").expect("seed live");
        db.save_note("secret", "plan").expect("seed secret");
        db.encrypt_note("secret", "pass").expect("encrypt");
        assert!(db.note_access.is_unlocked("secret"));

        db.restore_from_sqlite_file(&staged).expect("restore");
        assert!(!staged.exists());
        assert_eq!(
            db.get_note("backup-note")
                .expect("read")
                .expect("note")
                .body,
            "from backup"
        );
        assert!(db.get_note("live-note").expect("read").is_none());
        assert!(!db.note_access.is_unlocked("secret"));
        db.save_note("after", "writes land in the restored db")
            .expect("write after restore");

        let previous = Db::open(database_before_restore_path(&path)).expect("previous opens");
        assert_eq!(
            previous
                .get_note("live-note")
                .expect("read")
                .expect("note")
                .body,
            "replaced"
        );
        drop(previous);

        drop(db);
        remove_db_files(&path);
        remove_db_files(&source_path);
    }

    #[test]
    fn restore_waits_for_connections_in_use() {
        let source_path = temp_db_path();
        let source = Db::open(source_path.clone()).expect("source opens");
        source
            .save_note("backup-note", "from backup")
            .expect("seed");
        let staged = temp_db_path();
        source.backup_to_sqlite_file(&staged).expect("snapshot");
        drop(source);

        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        db.save_note("live-note", "replaced").expect("seed live");

        let in_use = db.conn.lock().expect("checkout");
        let restore = {
            let db = db.clone();
            let staged = staged.clone();
            std::thread::spawn(move || db.restore_from_sqlite_file(&staged))
        };
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            !restore.is_finished(),
            "restore waits for the checked-out connection"
        );
        drop(in_use);
        restore.join().expect("restore thread").expect("restore");

        assert!(db.get_note("live-note").expect("read").is_none());
        let state = db.conn.state.lock().expect("pool state");
        assert_eq!(state.connections.len(), state.created);
        assert!(state.created <= db.conn.max_size);
        drop(state);

        drop(db);
        remove_db_files(&path);
        remove_db_files(&source_path);
    }

    #[test]
    fn save_note_revision_persists_body_and_matches_save_note() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        let created = db.save_note_revision("n1", "hello").expect("note saved");
        assert_eq!(created.id, "n1");
        assert_eq!(
            db.get_note("n1").expect("lookup").map(|n| n.body),
            Some("hello".to_string())
        );

        let updated = db
            .save_note_revision("n1", "updated")
            .expect("note updated");
        assert!(updated.updated_at >= created.updated_at);
        assert_eq!(
            db.get_note("n1").expect("lookup").map(|n| n.body),
            Some("updated".to_string())
        );
        // The revision is the one a subsequent conflict check will compare against.
        assert_eq!(
            db.get_note_updated_at("n1").expect("revision"),
            Some(updated.updated_at)
        );

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn save_note_revision_encrypts_body_and_refuses_locked_notes() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        db.save_note("enc", "plain start").expect("seed note");
        db.encrypt_note("enc", "pass").expect("encrypt note");
        db.unlock_note("enc", "pass").expect("unlock note");
        db.save_note_revision("enc", "secret body")
            .expect("encrypted save");

        // Body column stays empty; ciphertext carries the content.
        let stored: String = Connection::open(&path)
            .expect("raw connection")
            .query_row("SELECT body FROM notes WHERE id = 'enc'", [], |r| r.get(0))
            .expect("row");
        assert_eq!(stored, "");
        assert_eq!(
            db.get_note("enc").expect("lookup").map(|n| n.body),
            Some("secret body".to_string())
        );

        db.note_access.clear("enc");
        assert!(
            db.save_note_revision("enc", "attempt").is_err(),
            "a locked encrypted note must reject writes on the revision path too"
        );

        drop(db);
        let _ = fs::remove_file(path);
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
    fn note_image_round_trip_uses_data_url() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        db.save_note("n1", "image note").expect("seed note");

        let image_id = db
            .reserve_note_image("n1", Some("clip.png"), Some("image/png"))
            .expect("reserve image");
        db.write_note_image_bytes(
            "n1",
            &image_id,
            Some("clip.png"),
            Some("image/png"),
            b"hello-image",
        )
        .expect("write image bytes");
        let resolved = db
            .resolve_note_image_data_url("n1", &image_id)
            .expect("resolve")
            .expect("exists");
        assert!(resolved.starts_with("data:image/png;base64,"));

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn note_image_write_requires_reserved_placeholder() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        db.save_note("n1", "image note").expect("seed note");

        let err = db
            .write_note_image_bytes(
                "n1",
                "missing-image-id",
                Some("clip.png"),
                Some("image/png"),
                b"hello-image",
            )
            .expect_err("missing placeholder should fail");
        assert!(err.contains("placeholder"));

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn note_image_delete_clears_resolve() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        db.save_note("n1", "image note").expect("seed note");

        let image_id = db
            .reserve_note_image("n1", Some("clip.png"), Some("image/png"))
            .expect("reserve image");
        db.write_note_image_bytes("n1", &image_id, None, None, b"hello-image")
            .expect("write image bytes");
        assert!(db
            .resolve_note_image_data_url("n1", &image_id)
            .expect("resolve")
            .is_some());

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
    fn pinned_titles_survive_edits_and_unpin_to_follow_the_first_line() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        db.save_note("n1", "Draft\nbody").expect("seed note");
        let revision = db.get_note_updated_at("n1").expect("revision");
        db.set_note_title("n1", "  Budget 2026 ")
            .expect("pin title");
        assert_eq!(
            db.get_note_updated_at("n1").expect("revision"),
            revision,
            "an open editor's revision stays valid"
        );
        let before = db.get_note("n1").expect("lookup").expect("exists");
        assert_eq!(before.pinned_title.as_deref(), Some("Budget 2026"));
        assert_eq!(before.body, "Draft\nbody", "renaming leaves the text alone");

        db.save_note("n1", "Other heading\nbody").expect("edit");
        db.save_note_revision("n1", "Third\nbody")
            .expect("autosave");
        let listed = db.list_notes_meta().expect("list meta");
        assert_eq!(listed[0].title, "Budget 2026");
        assert_eq!(
            db.search_notes_content("budget", 10).expect("search").len(),
            1
        );

        db.set_note_title("n1", "").expect("unpin");
        let note = db.get_note("n1").expect("lookup").expect("exists");
        assert_eq!(note.pinned_title, None);
        assert_eq!(db.list_notes_meta().expect("list")[0].title, "Third");
        db.save_note("n1", "Fourth\nbody").expect("edit");
        assert_eq!(db.list_notes_meta().expect("list")[0].title, "Fourth");

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn encrypting_pins_the_title_and_an_unpinned_one_stays_out_of_storage() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        let stored_title = || -> String {
            Connection::open(&path)
                .expect("raw connection")
                .query_row("SELECT note_title FROM notes WHERE id = 'n1'", [], |r| {
                    r.get(0)
                })
                .expect("row")
        };

        db.save_note("n1", "Plan\ntop secret").expect("seed note");
        db.encrypt_note("n1", "pass123").expect("encrypt note");
        db.save_note("n1", "Changed\ntop secret")
            .expect("edit encrypted");
        assert_eq!(stored_title(), "Plan", "encrypting pins the title");
        db.note_access.clear("n1");
        let listed = db.list_notes_meta().expect("list meta locked");
        assert_eq!(listed[0].title, "Plan");
        assert!(!listed[0].is_unlocked);

        // Renaming needs the password; unpinned, the title is hidden.
        assert!(db
            .set_note_title("n1", "Bank")
            .expect_err("locked")
            .contains("unlock first"));
        db.unlock_note("n1", "pass123").expect("unlock");
        db.set_note_title("n1", "Bank").expect("rename");
        assert_eq!(db.list_notes_meta().expect("list")[0].title, "Bank");
        db.set_note_title("n1", "").expect("unpin");
        assert_eq!(stored_title(), "");
        db.note_access.clear("n1");
        let listed = db.list_notes_meta().expect("list");
        assert_eq!(listed[0].title, ENCRYPTED_NOTE_TITLE);
        assert_eq!(listed[0].body_prefix, "[locked]");
        let linked = db.get_note_meta("n1").expect("resolve").expect("found");
        assert_eq!(linked.title, ENCRYPTED_NOTE_TITLE);
        assert!(db
            .save_note("n1", "should fail")
            .expect_err("save should fail while locked")
            .contains("unlock first"));

        db.unlock_note("n1", "pass123").expect("unlock note");
        db.save_note("n1", "Renamed\ntop secret")
            .expect("save unlocked");
        assert_eq!(stored_title(), "");
        assert_eq!(db.list_notes_meta().expect("list")[0].title, "Renamed");

        // Decrypted, an unpinned title follows the first line again.
        db.decrypt_note("n1", "pass123").expect("decrypt");
        assert_eq!(stored_title(), "Renamed");

        drop(db);
        let _ = fs::remove_file(path);
    }

    /// Turns the notes table back into its shape before note keys were
    /// wrapped (migration 3), recorded as `version`.
    fn downgrade_to_password_derived_keys(path: &Path, version: i64) {
        Connection::open(path)
            .expect("raw connection")
            .execute_batch(&format!(
                "DROP INDEX idx_notes_key_collection;
                 ALTER TABLE notes DROP COLUMN wrapped_key;
                 ALTER TABLE notes DROP COLUMN key_collection_id;
                 ALTER TABLE notes ADD COLUMN password_salt BLOB;
                 ALTER TABLE notes ADD COLUMN password_hash BLOB;
                 PRAGMA user_version = {version};"
            ))
            .expect("downgrade");
    }

    #[test]
    fn migration_rewrites_short_wiki_links_to_full_ids() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        db.save_note("01KP0YD099X9TQENYJQ1SE9X8V", "Older")
            .expect("seed older");
        db.save_note("01KP0YD0PZ75YKEFBJ162G23AV", "Newer")
            .expect("seed newer");
        db.save_note("01KP0YD1GJJDZA09429K2NYF7M", "Plan\n# Goals")
            .expect("seed plan");
        db.save_note(
            "src",
            "[[01kp0yd1]] and [[01KP0YD1#Goals|goals]]\n\
             total := [[01KP0YD0]].rate + 1\n\
             [[ZZZZZZZZ]] [[01KP0YD1GJJDZA09429K2NYF7M]] `[[01KP0YD1]]x`",
        )
        .expect("seed links");
        drop(db);
        let conn = Connection::open(&path).expect("raw connection");
        conn.execute_batch(
            "UPDATE notes SET updated_at = '2026-01-01T00:00:00Z'
             WHERE id = '01KP0YD099X9TQENYJQ1SE9X8V';
             UPDATE notes SET updated_at = '2026-02-01T00:00:00Z'
             WHERE id = '01KP0YD0PZ75YKEFBJ162G23AV';
             PRAGMA user_version = 3;",
        )
        .expect("downgrade");
        drop(conn);

        let db = Db::open(path.clone()).expect("db reopens");
        let body = db.get_note("src").expect("lookup").expect("exists").body;
        assert_eq!(
            body,
            "[[01KP0YD1GJJDZA09429K2NYF7M]] and [[01KP0YD1GJJDZA09429K2NYF7M#Goals|goals]]\n\
             total := [[01KP0YD0PZ75YKEFBJ162G23AV]].rate + 1\n\
             [[ZZZZZZZZ]] [[01KP0YD1GJJDZA09429K2NYF7M]] `[[01KP0YD1GJJDZA09429K2NYF7M]]x`"
        );

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn migrations_turn_app_locks_into_plain_notes_and_wrap_keys() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        db.save_note("locked", "was locked").expect("seed locked");
        drop(db);
        downgrade_to_password_derived_keys(&path, 0);
        Connection::open(&path)
            .expect("raw connection")
            .execute(
                "UPDATE notes SET access_mode = 'locked', password_hash = x'00'
                 WHERE id = 'locked'",
                [],
            )
            .expect("lock");

        let db = Db::open(path.clone()).expect("db reopens");
        let note = db.get_note("locked").expect("lookup").expect("exists");
        assert_eq!(note.access_mode, NoteAccessMode::None);
        assert_eq!(note.body, "was locked");
        assert_eq!(
            db.search_notes_content("locked", 10).expect("search").len(),
            1
        );
        let conn = Connection::open(&path).expect("raw connection");
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .expect("version");
        assert_eq!(version, 4);
        assert!(has_column(&conn, "notes", "wrapped_key").unwrap());
        assert!(!has_column(&conn, "notes", "password_hash").unwrap());

        // The new columns work on the migrated table.
        db.save_note("n1", "Plan\nsecret").expect("seed");
        db.encrypt_note("n1", "pass").expect("encrypt");
        db.note_access.clear("n1");
        let note = db.unlock_note("n1", "pass").expect("unlock");
        assert_eq!(note.body, "Plan\nsecret");

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn migration_refuses_notes_encrypted_with_a_password_derived_key() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        db.save_note("old", "Diary\nsecret").expect("seed");
        drop(db);
        downgrade_to_password_derived_keys(&path, 2);
        Connection::open(&path)
            .expect("raw connection")
            .execute(
                "UPDATE notes SET access_mode = 'encrypted', note_title = 'Diary',
                     title_pinned = 1
                 WHERE id = 'old'",
                [],
            )
            .expect("mark encrypted");

        let error = Db::open(path.clone()).err().expect("migration refuses");
        assert!(error.contains("\"Diary\""), "{error}");
        assert!(error.contains(":note decrypt"), "{error}");

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
        assert_eq!(listed_encrypted.len(), 2);
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
        assert_eq!(listed_unlocked.len(), 2);
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
                    cross_note: true,
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
                    cross_note: true,
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
    fn sqlite_default_modules_include_cross_note() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        {
            let conn = db.conn.lock().expect("lock db");
            conn.execute(
                "INSERT INTO notes (id, body, note_title, created_at, updated_at)
                 VALUES ('defaulted', 'body', 'body', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )
            .expect("insert with sqlite defaults");
        }

        let note = db
            .get_note("defaulted")
            .expect("lookup succeeds")
            .expect("note exists");
        assert_eq!(note.modules, NoteModules::default());

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
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].id, "a");
        assert_eq!(listed[1].id, "b");
        assert_eq!(listed[2].id, "welcome");

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
        assert!(!ids.contains(&"enc-a"));

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn collection_name_is_unique_after_normalization() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        let first = db
            .create_collection("Work", "primary")
            .expect("first collection");
        assert_eq!(first.name, "Work");
        let err = db
            .create_collection(" work ", "duplicate")
            .expect_err("normalized duplicate should fail");
        assert!(
            err.contains("already exists"),
            "unexpected error message: {err}"
        );

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn create_note_with_working_collection_applies_membership_and_default_tags() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        let collection = db
            .create_collection("Projects", "project notes")
            .expect("collection created");
        db.set_collection_default_tags(
            &collection.id,
            &["alpha".to_string(), "beta".to_string(), "alpha".to_string()],
        )
        .expect("default tags set");

        db.create_note_with_context(
            "n-working",
            NoteModules::default(),
            None,
            Some(&collection.id),
        )
        .expect("note created with working collection");
        assert_eq!(
            db.get_note_collection_ids("n-working")
                .expect("collection ids lookup"),
            vec![collection.id.clone()]
        );
        assert_eq!(
            db.list_note_tags("n-working").expect("tags lookup"),
            vec!["alpha".to_string(), "beta".to_string()]
        );

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn create_note_without_working_collection_applies_no_membership_or_tags() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        let collection = db
            .create_collection("Projects", "project notes")
            .expect("collection created");
        db.set_collection_default_tags(&collection.id, &["alpha".to_string()])
            .expect("default tags set");

        db.create_note_with_context("n-plain", NoteModules::default(), None, None)
            .expect("note created without working collection");
        assert!(db
            .get_note_collection_ids("n-plain")
            .expect("collection ids lookup")
            .is_empty());
        assert!(db
            .list_note_tags("n-plain")
            .expect("tags lookup")
            .is_empty());

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn collection_membership_batch_ops_and_counts() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        let work = db.create_collection("Work", "").expect("collection");
        let home = db.create_collection("Home", "").expect("collection");
        let baseline = db.collection_note_counts().expect("counts");
        for id in ["a", "b", "c"] {
            db.create_note_with_context(id, NoteModules::default(), None, None)
                .expect("note created");
        }
        let ids = |list: &[&str]| list.iter().map(|id| id.to_string()).collect::<Vec<_>>();

        assert_eq!(
            db.add_notes_to_collection(&work.id, &ids(&["a", "b", "missing"]))
                .expect("added"),
            2
        );
        assert_eq!(
            db.add_notes_to_collection(&work.id, &ids(&["a"]))
                .expect("added"),
            0
        );
        db.add_notes_to_collection(&home.id, &ids(&["a"]))
            .expect("added");

        let counts = db.collection_note_counts().expect("counts");
        assert_eq!(counts.total, baseline.total + 3);
        assert_eq!(counts.unsorted, baseline.unsorted + 1);
        assert_eq!(counts.per_collection.get(&work.id), Some(&2));
        assert_eq!(counts.per_collection.get(&home.id), Some(&1));
        let unsorted = db.list_notes_meta_unsorted().expect("unsorted");
        assert_eq!(unsorted.len(), counts.unsorted);
        assert!(unsorted.iter().any(|n| n.id == "c"));
        assert!(!unsorted.iter().any(|n| n.id == "a" || n.id == "b"));

        assert_eq!(
            db.remove_notes_from_collection(&work.id, &ids(&["a", "c"]))
                .expect("removed"),
            1
        );
        assert_eq!(
            db.get_note_collection_ids("a").expect("ids"),
            vec![home.id.clone()]
        );
        assert!(db.add_notes_to_collection("nope", &ids(&["a"])).is_err());

        drop(db);
        let _ = fs::remove_file(path);
    }

    /// Ends the current history session of a note, as if an hour passed.
    fn end_history_session(db: &Db, note_id: &str) {
        let conn = db.conn.lock().expect("conn");
        conn.execute(
            "UPDATE note_history SET session_started = session_started - 3600,
                 session_last_write = session_last_write - 3600
             WHERE note_id = ?1",
            [note_id],
        )
        .expect("age history");
    }

    fn history_texts(db: &Db, note_id: &str) -> Vec<String> {
        db.list_note_history(note_id)
            .expect("list history")
            .iter()
            .map(|version| {
                db.note_version_text(note_id, version.id)
                    .expect("version text")
            })
            .collect()
    }

    #[test]
    fn history_keeps_one_version_per_editing_session() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        db.create_note_with_context("h", NoteModules::default(), None, None)
            .expect("note created");
        db.save_note_revision("h", "one").expect("save");
        assert!(db.list_note_history("h").unwrap().is_empty(), "empty start");
        db.save_note_revision("h", "one\ntwo").expect("save");
        db.save_note_revision("h", "one\ntwo\nthree").expect("save");
        // Both saves belong to one session: the version from before it.
        assert_eq!(history_texts(&db, "h"), vec!["one"]);
        let version = &db.list_note_history("h").unwrap()[0];
        assert_eq!((version.lines_added, version.lines_removed), (2, 0));

        end_history_session(&db, "h");
        db.save_note("h", "zero\none\ntwo\nthree").expect("save");
        assert_eq!(history_texts(&db, "h"), vec!["one\ntwo\nthree", "one"]);

        // Returning to the session's starting text drops its version.
        db.save_note("h", "one\ntwo\nthree").expect("save");
        assert_eq!(history_texts(&db, "h"), vec!["one"]);

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn thinning_old_history_keeps_the_remaining_versions_rebuildable() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        let mut lines: Vec<String> = (0..30).map(|i| format!("line {i}")).collect();
        db.save_note_revision("t", &lines.join("\n")).expect("save");
        let mut versions: Vec<String> = Vec::new();
        for step in 0..40 {
            end_history_session(&db, "t");
            versions.push(lines.join("\n"));
            let at = (step * 7) % lines.len();
            if step % 3 == 0 {
                lines.insert(at, format!("new {step}"));
            } else {
                lines[at].push_str(" edited");
            }
            db.save_note_revision("t", &lines.join("\n")).expect("save");
        }
        versions.reverse(); // newest first, as listed

        // Spread the versions over past days, three a day, so pruning thins
        // them to one a day.
        let now = OffsetDateTime::now_utc();
        {
            let conn = db.conn.lock().expect("conn");
            let ids: Vec<i64> = conn
                .prepare("SELECT id FROM note_history WHERE note_id = 't' ORDER BY id DESC")
                .expect("prepare")
                .query_map([], |row| row.get(0))
                .expect("ids")
                .collect::<Result<_, _>>()
                .expect("ids");
            assert_eq!(ids.len(), versions.len());
            for (age, id) in ids.iter().enumerate() {
                let saved = now - time::Duration::days(2) - time::Duration::hours(8 * age as i64);
                conn.execute(
                    "UPDATE note_history SET saved_at = ?2 WHERE id = ?1",
                    rusqlite::params![
                        id,
                        saved
                            .format(&time::format_description::well_known::Rfc3339)
                            .unwrap()
                    ],
                )
                .expect("backdate");
            }
        }
        let saved: Vec<i64> = (0..versions.len())
            .map(|age| {
                (now - time::Duration::days(2) - time::Duration::hours(8 * age as i64))
                    .unix_timestamp()
            })
            .collect();
        let keep = crate::history::versions_to_keep(&saved, now.unix_timestamp(), 500);
        assert!(keep.iter().any(|keep| !keep), "some versions are thinned");

        // A new session's save prunes.
        end_history_session(&db, "t");
        let previous = lines.join("\n");
        db.save_note_revision("t", "final").expect("save");

        let mut expected = vec![previous];
        expected.extend(
            versions
                .into_iter()
                .zip(keep)
                .filter_map(|(text, keep)| keep.then_some(text)),
        );
        assert_eq!(history_texts(&db, "t"), expected);

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn history_rebuilds_random_sessions_across_checkpoints() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        db.create_note_with_context("r", NoteModules::default(), None, None)
            .expect("note created");
        let mut seed = 0x9e37_79b9_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut lines: Vec<String> = (0..40).map(|i| format!("line {i}")).collect();
        db.save_note_revision("r", &lines.join("\n")).expect("save");
        // Expected versions, oldest first.
        let mut expected: Vec<String> = Vec::new();
        for _ in 0..70 {
            end_history_session(&db, "r");
            let before = lines.join("\n");
            for _ in 0..1 + next() % 3 {
                let at = (next() as usize) % (lines.len() + 1);
                match next() % 3 {
                    0 => lines.insert(at, format!("new {}", next() % 1000)),
                    1 if lines.len() > 1 => {
                        lines.remove(at.min(lines.len() - 1));
                    }
                    _ => {
                        let i = at.min(lines.len() - 1);
                        lines[i].push_str(" edited");
                    }
                }
                db.save_note_revision("r", &lines.join("\n")).expect("save");
            }
            if before != lines.join("\n") {
                expected.push(before);
            }
        }
        expected.reverse();
        assert_eq!(history_texts(&db, "r"), expected);
        let fulls: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM note_history WHERE note_id = 'r' AND is_full = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(fulls >= 1, "long histories get checkpoints");

        db.delete_note("r", None).expect("delete");
        let left: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM note_history", [], |row| row.get(0))
            .unwrap();
        assert_eq!(left, 0, "history goes with its note");

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn pruning_thins_old_versions_and_keeps_the_rest_rebuildable() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        db.create_note_with_context("p", NoteModules::default(), None, None)
            .expect("note created");
        let mut body = String::from("start");
        db.save_note_revision("p", &body).expect("save");
        for i in 0..40 {
            end_history_session(&db, "p");
            body.push_str(&format!("\nline {i}"));
            db.save_note_revision("p", &body).expect("save");
        }
        // Spread the versions over two months, newest first, 36 hours apart.
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let ids: Vec<i64> = db
            .list_note_history("p")
            .unwrap()
            .iter()
            .map(|v| v.id)
            .collect();
        {
            let conn = db.conn.lock().unwrap();
            for (index, id) in ids.iter().enumerate() {
                let saved =
                    OffsetDateTime::from_unix_timestamp(now - 3600 - index as i64 * 36 * 3600)
                        .unwrap()
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap();
                conn.execute(
                    "UPDATE note_history SET saved_at = ?2 WHERE id = ?1",
                    rusqlite::params![id, saved],
                )
                .unwrap();
            }
        }
        let before: std::collections::HashMap<i64, String> = ids
            .iter()
            .map(|id| (*id, db.note_version_text("p", *id).unwrap()))
            .collect();

        end_history_session(&db, "p");
        body.push_str("\nlast");
        db.save_note_revision("p", &body)
            .expect("save triggers pruning");

        let after = db.list_note_history("p").unwrap();
        assert!(after.len() < ids.len() + 1, "old versions were thinned");
        assert!(after.len() > 20, "recent and daily versions stay");
        for version in &after[1..] {
            assert_eq!(
                db.note_version_text("p", version.id).unwrap(),
                before[&version.id],
                "version {} still rebuilds",
                version.id
            );
        }

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn history_of_encrypted_notes_is_encrypted_and_follows_protection_changes() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        db.create_note_with_context("e", NoteModules::default(), None, None)
            .expect("note created");
        db.save_note_revision("e", "plain secret v1").expect("save");
        end_history_session(&db, "e");
        db.save_note_revision("e", "plain secret v2").expect("save");

        db.encrypt_note("e", "pw").expect("encrypt");
        end_history_session(&db, "e");
        db.save_note_revision("e", "plain secret v3").expect("save");
        let raw: Vec<(Vec<u8>, Option<Vec<u8>>)> = {
            let conn = db.conn.lock().unwrap();
            let mut stmt = conn
                .prepare("SELECT payload, payload_nonce FROM note_history WHERE note_id = 'e'")
                .unwrap();
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(raw.len(), 2);
        assert!(raw.iter().all(|(_, nonce)| nonce.is_some()));
        assert_eq!(
            history_texts(&db, "e"),
            vec!["plain secret v2", "plain secret v1"]
        );

        db.decrypt_note("e", "pw").expect("decrypt");
        assert_eq!(
            history_texts(&db, "e"),
            vec!["plain secret v2", "plain secret v1"]
        );

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn changing_a_note_password_rewraps_its_key_and_keeps_history() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        db.save_note("n1", "v1").expect("save");
        db.encrypt_note("n1", "old-pass").expect("encrypt");
        end_history_session(&db, "n1");
        db.save_note("n1", "v2").expect("save");
        db.encrypt_note("n1", "new-pass").expect("change password");

        let other = Db::open(path.clone()).expect("second handle");
        assert!(other
            .unlock_note("n1", "old-pass")
            .expect_err("old password")
            .contains("invalid password"));
        assert_eq!(
            other.unlock_note("n1", "new-pass").expect("unlock").body,
            "v2"
        );
        assert_eq!(history_texts(&other, "n1"), vec!["v1"]);

        drop((db, other));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_note_rekeyed_elsewhere_refuses_writes_with_the_stale_key() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        db.save_note("n1", "secret").expect("save");
        db.encrypt_note("n1", "pass").expect("encrypt");

        let other = Db::open(path.clone()).expect("second handle");
        other.unlock_note("n1", "pass").expect("unlock");
        other.decrypt_note("n1", "pass").expect("decrypt");
        other
            .encrypt_note("n1", "pass")
            .expect("encrypt with a new key");

        assert!(db
            .save_note_revision("n1", "stale write")
            .expect_err("stale key")
            .contains("unlock first"));
        assert!(!db.note_access.is_unlocked("n1"));
        assert_eq!(other.get_note("n1").unwrap().unwrap().body, "secret");

        drop((db, other));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn encrypting_a_collection_protects_members_with_one_password() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        let work = db.create_collection("Work", "").expect("collection");
        db.save_note("a", "Alpha\nsecret a").expect("a");
        db.save_note("b", "Beta\nsecret b").expect("b");
        db.save_note("own", "Own\nsecret").expect("own");
        db.encrypt_note("own", "own-pass").expect("own password");
        db.save_note("outside", "Outside").expect("outside");
        for id in ["a", "b", "own"] {
            db.add_notes_to_collection(&work.id, &[id.to_string()])
                .expect("join");
        }

        let (protected, skipped) = db
            .encrypt_collection(&work.id, "work-pass")
            .expect("encrypt");
        assert_eq!((protected, skipped), (3, 0));
        assert!(db.get_collection(&work.id).unwrap().unwrap().encrypted);
        assert!(db
            .encrypt_collection(&work.id, "again")
            .expect_err("twice")
            .contains("already encrypted"));
        let raw_body: String = Connection::open(&path)
            .unwrap()
            .query_row("SELECT body FROM notes WHERE id = 'a'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(raw_body, "");

        // A fresh handle sees everything locked, titles still listed.
        let other = Db::open(path.clone()).expect("second handle");
        let titles: Vec<(String, bool)> = other
            .list_notes_meta()
            .unwrap()
            .into_iter()
            .filter(|n| ["a", "b", "own"].contains(&n.id.as_str()))
            .map(|n| (n.title, n.is_unlocked))
            .collect();
        assert_eq!(titles.len(), 3);
        assert!(titles
            .iter()
            .all(|(title, unlocked)| !title.is_empty() && !unlocked));
        assert!(other
            .unlock_collection(&work.id, "own-pass")
            .expect_err("wrong password")
            .contains("invalid password"));
        // Opening one note with the collection's password unlocks them all.
        assert_eq!(
            other.unlock_note("a", "work-pass").expect("unlock").body,
            "Alpha\nsecret a"
        );
        assert!(other.is_collection_unlocked(&work.id));
        assert_eq!(other.get_note("own").unwrap().unwrap().body, "Own\nsecret");
        assert_eq!(
            other.note_key_collection_name("b").unwrap().as_deref(),
            Some("Work")
        );

        // Plain notes joining a locked collection are refused, then sealed.
        let third = Db::open(path.clone()).expect("third handle");
        assert!(third
            .add_notes_to_collection(&work.id, &["outside".to_string()])
            .expect_err("locked")
            .contains("unlock it first"));
        assert!(third.get_note_collection_ids("outside").unwrap().is_empty());
        other
            .add_notes_to_collection(&work.id, &["outside".to_string()])
            .expect("join unlocked");
        let joined = other.get_note("outside").unwrap().unwrap();
        assert_eq!(joined.access_mode, NoteAccessMode::Encrypted);
        assert_eq!(joined.pinned_title.as_deref(), Some("Outside"));

        // New notes in the working collection are encrypted with its key.
        let created = other
            .create_note_with_context("new", NoteModules::default(), None, Some(&work.id))
            .expect("create");
        assert_eq!(created.access_mode, NoteAccessMode::Encrypted);
        assert!(third
            .create_note_with_context("new2", NoteModules::default(), None, Some(&work.id))
            .expect_err("locked collection")
            .contains("unlock it first"));
        assert!(third.get_note("new2").unwrap().is_none());

        drop((db, other, third));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn members_of_an_encrypted_collection_cannot_drop_its_protection() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        let vault = db.create_collection("Vault", "").expect("collection");
        db.save_note("a", "Alpha\nsecret").expect("a");
        db.add_notes_to_collection(&vault.id, &["a".to_string()])
            .expect("join");
        db.encrypt_collection(&vault.id, "vault-pass")
            .expect("encrypt");

        let error = db.decrypt_note("a", "vault-pass").expect_err("member");
        assert!(error.contains("encrypted collection \"Vault\""), "{error}");
        let error = db.encrypt_note("a", "own-pass").expect_err("member");
        assert!(error.contains("encrypted collection \"Vault\""), "{error}");
        let note = db.get_note("a").unwrap().unwrap();
        assert_eq!(note.access_mode, NoteAccessMode::Encrypted);

        // A note joining later is sealed under the collection, and held too.
        db.save_note("b", "Beta").expect("b");
        db.add_notes_to_collection(&vault.id, &["b".to_string()])
            .expect("join");
        assert!(db.decrypt_note("b", "vault-pass").is_err());

        // Out of the collection, its password still opens it, and it can be
        // decrypted or given its own password.
        db.remove_notes_from_collection(&vault.id, &["a".to_string()])
            .expect("leave");
        db.encrypt_note("a", "own-pass").expect("own password");
        let note = db.decrypt_note("a", "own-pass").expect("decrypt");
        assert_eq!(note.access_mode, NoteAccessMode::None);
        assert_eq!(note.body, "Alpha\nsecret");

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn notes_leaving_an_encrypted_collection_stay_protected_until_it_is_decrypted() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        let vault = db.create_collection("Vault", "").expect("collection");
        db.save_note("a", "Alpha").expect("a");
        db.save_note("b", "Beta").expect("b");
        db.add_notes_to_collection(&vault.id, &["a".to_string(), "b".to_string()])
            .expect("join");
        db.encrypt_collection(&vault.id, "vault-pass")
            .expect("encrypt");
        db.remove_notes_from_collection(&vault.id, &["b".to_string()])
            .expect("leave");

        let other = Db::open(path.clone()).expect("second handle");
        assert_eq!(
            other
                .unlock_note("b", "vault-pass")
                .expect("still vault's")
                .body,
            "Beta"
        );
        assert!(other
            .delete_collection(&vault.id)
            .expect_err("protects notes")
            .contains("decrypt the collection first"));
        assert!(other
            .purge_collection(&vault.id)
            .expect_err("b outlives the purge")
            .contains("decrypt the collection first"));
        assert!(other
            .delete_note("a", Some("wrong"))
            .expect_err("wrong password")
            .contains("invalid password"));

        assert!(other
            .decrypt_collection(&vault.id, "wrong")
            .expect_err("wrong password")
            .contains("invalid password"));
        assert_eq!(
            other
                .decrypt_collection(&vault.id, "vault-pass")
                .expect("decrypt"),
            2
        );
        for (id, body) in [("a", "Alpha"), ("b", "Beta")] {
            let note = db.get_note(id).unwrap().unwrap();
            assert_eq!(note.access_mode, NoteAccessMode::None);
            assert_eq!(note.body, body);
        }
        assert!(!other.get_collection(&vault.id).unwrap().unwrap().encrypted);
        assert!(other.delete_collection(&vault.id).expect("delete"));

        drop((db, other));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn parses_stored_timestamps_with_fraction_and_offset() {
        assert_eq!(timestamp_epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            timestamp_epoch("2026-09-30T12:00:00.123456Z"),
            Some(1_790_769_600)
        );
        assert_eq!(
            timestamp_epoch("2026-09-30T14:00:00+02:00"),
            Some(1_790_769_600)
        );
        assert_eq!(timestamp_epoch("garbage"), None);
        assert!(timestamp_epoch(&now_iso()).is_some());
    }

    #[test]
    fn set_note_collections_does_not_apply_collection_default_tags() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        let collection = db
            .create_collection("Projects", "project notes")
            .expect("collection created");
        db.set_collection_default_tags(&collection.id, &["alpha".to_string()])
            .expect("default tags set");

        db.create_note_with_context("n-manual", NoteModules::default(), None, None)
            .expect("note created");
        db.set_note_collections("n-manual", &[collection.id.clone()])
            .expect("manual membership set");
        assert!(db
            .list_note_tags("n-manual")
            .expect("tags lookup")
            .is_empty());

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn removing_collection_membership_does_not_remove_existing_note_tags() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        let collection = db
            .create_collection("Projects", "project notes")
            .expect("collection created");
        db.set_collection_default_tags(&collection.id, &["alpha".to_string()])
            .expect("default tags set");

        db.create_note_with_context(
            "n-keep-tags",
            NoteModules::default(),
            None,
            Some(&collection.id),
        )
        .expect("note created with context");
        db.set_note_collections("n-keep-tags", &[])
            .expect("membership removed");
        assert!(db
            .get_note_collection_ids("n-keep-tags")
            .expect("collection ids lookup")
            .is_empty());
        assert_eq!(
            db.list_note_tags("n-keep-tags").expect("tags lookup"),
            vec!["alpha".to_string()]
        );

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn filtered_list_and_search_return_only_matching_collection_notes() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        let collection_a = db
            .create_collection("Collection A", "")
            .expect("collection a created");
        let collection_b = db
            .create_collection("Collection B", "")
            .expect("collection b created");

        db.save_note("n-a", "needle appears in collection a")
            .expect("save note a");
        db.save_note("n-b", "needle appears in collection b")
            .expect("save note b");
        db.save_note("n-none", "needle appears in no collection")
            .expect("save note none");
        db.set_note_collections("n-a", &[collection_a.id.clone()])
            .expect("set note a collection");
        db.set_note_collections("n-b", &[collection_b.id.clone()])
            .expect("set note b collection");

        let list_a = db
            .list_notes_meta_filtered(Some(&collection_a.id))
            .expect("list filtered a");
        let ids_a: Vec<&str> = list_a.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids_a, vec!["n-a"]);

        let search_a = db
            .search_notes_content_filtered("needle", 20, Some(&collection_a.id))
            .expect("search filtered a");
        let search_a_ids: Vec<&str> = search_a.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(search_a_ids, vec!["n-a"]);

        let search_all = db
            .search_notes_content_filtered("needle", 20, None)
            .expect("search all");
        let search_all_ids: Vec<&str> = search_all.iter().map(|n| n.id.as_str()).collect();
        assert!(search_all_ids.contains(&"n-a"));
        assert!(search_all_ids.contains(&"n-b"));
        assert!(search_all_ids.contains(&"n-none"));

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn purge_collection_deletes_associated_notes_and_collection() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");
        let collection = db
            .create_collection("Projects", "project notes")
            .expect("collection created");
        db.save_note("n-a", "note a").expect("save note a");
        db.save_note("n-b", "note b").expect("save note b");
        db.set_note_collections("n-a", std::slice::from_ref(&collection.id))
            .expect("set note a collection");
        db.set_note_collections("n-b", std::slice::from_ref(&collection.id))
            .expect("set note b collection");

        let deleted = db
            .purge_collection(&collection.id)
            .expect("purge collection succeeds");
        assert_eq!(deleted, 2);
        assert!(db
            .get_collection(&collection.id)
            .expect("collection lookup")
            .is_none());
        assert!(db.get_note("n-a").expect("note lookup a").is_none());
        assert!(db.get_note("n-b").expect("note lookup b").is_none());

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn welcome_note_seed_migration_is_idempotent() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        let welcome_rows: i64 = db
            .conn
            .lock()
            .expect("pool lock")
            .query_row(
                "SELECT COUNT(1) FROM notes WHERE id = 'welcome'",
                [],
                |row| row.get(0),
            )
            .expect("count welcome rows");
        assert_eq!(welcome_rows, 1);

        let welcome = db
            .conn
            .lock()
            .expect("pool lock")
            .query_row(
                "SELECT body, modules_json, access_mode FROM notes WHERE id = 'welcome'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .expect("welcome row exists");
        assert!(welcome.0.contains("# Welcome to Slate"));
        assert_eq!(welcome.1, DEFAULT_NOTE_MODULES_JSON);
        assert_eq!(welcome.2, "none");

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn search_notes_content_returns_snippet_with_matched_term() {
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
            hits[0].snippet.contains("budget"),
            "snippet should include the matched term, got: {:?}",
            hits[0].snippet
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

        // Run one search against a healthy index so `ensure_search_index_checked`
        // marks the one-shot auto-heal gate as done. Without this warm-up the
        // next search would silently rebuild the index via
        // `check_and_heal_search_index` and the empty-after-clear assertion
        // below would never hold. Auto-heal coverage lives in a separate test
        // (`search_notes_content_seeds_index_for_pre_fts_databases`).
        let _ = db
            .search_notes_content("warmup", 10)
            .expect("warmup search closes the auto-heal gate");

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
    fn search_index_survives_writes_that_do_not_touch_indexed_columns() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        db.save_note("n1", "canary body text").expect("save note");
        assert_eq!(
            db.search_notes_content("canary", 10).expect("search").len(),
            1
        );

        // Writes only modules_json/updated_at, so the update trigger is scoped
        // out. The indexed row must survive untouched rather than go stale.
        db.set_note_modules(
            "n1",
            NoteModules {
                math: false,
                table: false,
                variables: false,
                style: false,
                cross_note: false,
            },
        )
        .expect("update modules");

        let after = db.search_notes_content("canary", 10).expect("search");
        assert_eq!(
            after.len(),
            1,
            "note must stay searchable after a modules write"
        );
        assert_eq!(after[0].id, "n1");

        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn search_index_follows_body_edits() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        db.save_note("n1", "original canary").expect("save note");
        db.save_note("n1", "replaced sentinel")
            .expect("resave note");

        assert!(
            db.search_notes_content("canary", 10)
                .expect("search")
                .is_empty(),
            "replaced text must leave the index"
        );
        assert_eq!(
            db.search_notes_content("sentinel", 10)
                .expect("search")
                .len(),
            1,
            "new text must be indexed"
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
    fn search_encrypted_note_stays_unsearchable_after_session_unlock() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        db.save_note("encme", "canary encrypted content")
            .expect("save note");
        db.encrypt_note("encme", "pass1").expect("encrypt note");
        db.unlock_note("encme", "pass1").expect("session unlock");

        let hits = db.search_notes_content("canary", 10).expect("search");
        assert!(
            hits.is_empty(),
            "encrypted note should remain unsearchable even after session unlock"
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
        // punctuation splits terms for better natural-language matching.
        assert_eq!(build_fts_query("hello!world"), "\"hello\"* \"world\"*");
        assert_eq!(
            build_fts_query("it's not corr"),
            "\"it\"* \"s\"* \"not\"* \"corr\"*"
        );
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
    fn reminders_are_upserted_reminded_and_deleted_with_note() {
        let path = temp_db_path();
        let db = Db::open(path.clone()).expect("db opens");

        db.save_note("n1", "hello").expect("save note");

        let first = db
            .upsert_reminder("n1", 3, 1_800_000_000_000, "14.01.2027. 10:00", "line 3")
            .expect("upsert reminder");
        assert_eq!(first.line_number, 3);
        assert_eq!(first.remind_at_ms, 1_800_000_000_000);
        assert!(first.reminded_at_ms.is_none());

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
        assert!(updated.reminded_at_ms.is_none());

        let list = db.list_reminders("n1").expect("list reminders");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].line_number, 3);

        let reminded = db
            .mark_reminder_reminded("n1", 3, 1_900_000_100_000)
            .expect("mark reminded")
            .expect("reminder exists");
        assert_eq!(reminded.reminded_at_ms, Some(1_900_000_100_000));

        assert!(db
            .move_reminder_line("n1", 3, 5, "line 5 changed")
            .expect("move reminder"));
        let after_move = db.list_reminders("n1").expect("list reminders after move");
        assert_eq!(after_move.len(), 1);
        assert_eq!(after_move[0].line_number, 5);
        assert_eq!(after_move[0].line_text, "line 5 changed");
        assert_eq!(after_move[0].reminded_at_ms, Some(1_900_000_100_000));

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
