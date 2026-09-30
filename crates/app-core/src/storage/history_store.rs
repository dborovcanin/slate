//! Persistence of note history (see `crate::history`). Versions are written
//! inside the transaction that replaces the body they version, so the newest
//! row always matches the stored body.
//!
//! A version covers one editing session: saves less than
//! [`SESSION_GAP_SECS`] apart (and within [`SESSION_MAX_SECS`] of the first)
//! extend the newest row instead of adding one. Payloads of encrypted notes
//! are encrypted with the note's key.

use super::{decrypt_bytes_with_key, encrypt_bytes_with_key};
use crate::history::{self, Payload};
use crate::storage::NoteVersion;
use rusqlite::{Connection, OptionalExtension};

/// A pause longer than this ends an editing session.
const SESSION_GAP_SECS: i64 = 5 * 60;
/// A session longer than this starts a new version even without a pause.
const SESSION_MAX_SECS: i64 = 30 * 60;

fn epoch_now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

fn seal(bytes: Vec<u8>, key: Option<&[u8; 32]>) -> Result<(Vec<u8>, Option<Vec<u8>>), String> {
    match key {
        Some(key) => {
            let sealed = encrypt_bytes_with_key(&bytes, key)?;
            Ok((sealed.ciphertext, Some(sealed.nonce)))
        }
        None => Ok((bytes, None)),
    }
}

fn open(bytes: Vec<u8>, nonce: Option<Vec<u8>>, key: Option<&[u8; 32]>) -> Result<Vec<u8>, String> {
    match (nonce, key) {
        (Some(nonce), Some(key)) => decrypt_bytes_with_key(&bytes, &nonce, key),
        (None, _) => Ok(bytes),
        (Some(_), None) => Err("note history is encrypted; unlock the note first".to_string()),
    }
}

fn store_payload(
    payload: &Payload,
    key: Option<&[u8; 32]>,
) -> Result<(Vec<u8>, Option<Vec<u8>>), String> {
    seal(payload.encode(), key)
}

fn load_payload(
    bytes: Vec<u8>,
    nonce: Option<Vec<u8>>,
    key: Option<&[u8; 32]>,
) -> Result<Payload, String> {
    Payload::decode(&open(bytes, nonce, key)?)
}

/// Lines the session after a version added and removed, from the delta that
/// turns the newer text back into that version.
fn session_line_counts(back_to_version: &history::Delta) -> (usize, usize) {
    let (inserted, deleted) = back_to_version.line_counts();
    (deleted, inserted)
}

struct NewestRow {
    id: i64,
    session_started: i64,
    session_last_write: i64,
    payload: Vec<u8>,
    payload_nonce: Option<Vec<u8>>,
}

/// Records that the body of `note_id`, saved at `saved_at` as `old_body`, is
/// about to become `new_body`. Call inside the transaction that writes
/// `new_body`.
pub(super) fn record(
    conn: &Connection,
    note_id: &str,
    old_body: &str,
    saved_at: &str,
    new_body: &str,
    key: Option<&[u8; 32]>,
) -> Result<(), String> {
    if old_body == new_body {
        return Ok(());
    }
    let now = epoch_now();
    let newest = conn
        .query_row(
            "SELECT id, session_started, session_last_write, payload, payload_nonce
             FROM note_history WHERE note_id = ?1 ORDER BY id DESC LIMIT 1",
            [note_id],
            |row| {
                Ok(NewestRow {
                    id: row.get(0)?,
                    session_started: row.get(1)?,
                    session_last_write: row.get(2)?,
                    payload: row.get(3)?,
                    payload_nonce: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;

    if let Some(row) = newest.filter(|row| {
        now - row.session_last_write < SESSION_GAP_SECS
            && now - row.session_started < SESSION_MAX_SECS
    }) {
        return extend_session(conn, row, old_body, new_body, now, key);
    }
    if old_body.is_empty() {
        // A note's empty starting text is not worth a version.
        return Ok(());
    }

    let back = history::diff(new_body, old_body);
    let (lines_added, lines_removed) = session_line_counts(&back);
    let delta = Payload::Delta(back);
    let delta_len = delta.encode().len();
    let (deltas_since_full, bytes_since_full): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(length(payload)), 0) FROM note_history
             WHERE note_id = ?1 AND id > COALESCE(
                 (SELECT MAX(id) FROM note_history WHERE note_id = ?1 AND is_full = 1), 0)",
            [note_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let payload = if history::should_checkpoint(
        deltas_since_full as usize,
        bytes_since_full as usize + delta_len,
        old_body.len(),
    ) {
        Payload::Full(old_body.to_string())
    } else {
        delta
    };
    let (bytes, nonce) = store_payload(&payload, key)?;
    conn.execute(
        "INSERT INTO note_history (
            note_id, saved_at, session_started, session_last_write,
            is_full, payload, payload_nonce, lines_added, lines_removed
        ) VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            note_id,
            saved_at,
            now,
            payload.is_full(),
            bytes,
            nonce,
            lines_added as i64,
            lines_removed as i64,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// A save within the current session: the newest row keeps the version from
/// before the session, re-expressed against the new body.
fn extend_session(
    conn: &Connection,
    row: NewestRow,
    old_body: &str,
    new_body: &str,
    now: i64,
    key: Option<&[u8; 32]>,
) -> Result<(), String> {
    let payload = load_payload(row.payload, row.payload_nonce, key)?;
    let version = payload.resolve(old_body)?;
    let back = history::diff(new_body, &version);
    if back.is_identity() {
        // The session returned to where it started: nothing to keep.
        conn.execute("DELETE FROM note_history WHERE id = ?1", [row.id])
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    let (lines_added, lines_removed) = session_line_counts(&back);
    if payload.is_full() {
        conn.execute(
            "UPDATE note_history
             SET session_last_write = ?2, lines_added = ?3, lines_removed = ?4
             WHERE id = ?1",
            rusqlite::params![row.id, now, lines_added as i64, lines_removed as i64],
        )
        .map_err(|e| e.to_string())?;
        return Ok(());
    }
    let (bytes, nonce) = store_payload(&Payload::Delta(back), key)?;
    conn.execute(
        "UPDATE note_history
         SET session_last_write = ?2, payload = ?3, payload_nonce = ?4,
             lines_added = ?5, lines_removed = ?6
         WHERE id = ?1",
        rusqlite::params![
            row.id,
            now,
            bytes,
            nonce,
            lines_added as i64,
            lines_removed as i64
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Ends the note's current editing session, so the next save starts a new
/// version even right away.
pub(super) fn end_session(conn: &Connection, note_id: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE note_history SET session_started = 0
         WHERE id = (SELECT MAX(id) FROM note_history WHERE note_id = ?1)",
        [note_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Stored versions of a note, newest first.
pub(super) fn list(conn: &Connection, note_id: &str) -> Result<Vec<NoteVersion>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, saved_at, lines_added, lines_removed FROM note_history
             WHERE note_id = ?1 ORDER BY id DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([note_id], |row| {
            Ok(NoteVersion {
                id: row.get(0)?,
                saved_at: row.get(1)?,
                lines_added: row.get::<_, i64>(2)? as usize,
                lines_removed: row.get::<_, i64>(3)? as usize,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

/// The text of version `version_id`, rebuilt from `current_body`. Reads only
/// the rows from the nearest full checkpoint at or after the version.
pub(super) fn version_text(
    conn: &Connection,
    note_id: &str,
    version_id: i64,
    current_body: &str,
    key: Option<&[u8; 32]>,
) -> Result<String, String> {
    let start_id: Option<i64> = conn
        .query_row(
            "SELECT MIN(id) FROM note_history WHERE note_id = ?1 AND id >= ?2 AND is_full = 1",
            rusqlite::params![note_id, version_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, payload, payload_nonce FROM note_history
             WHERE note_id = ?1 AND id >= ?2 AND id <= ?3 ORDER BY id DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(
            rusqlite::params![note_id, version_id, start_id.unwrap_or(i64::MAX)],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                ))
            },
        )
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if rows.last().map(|(id, _, _)| *id) != Some(version_id) {
        return Err("no such note version".to_string());
    }
    let payloads = rows
        .into_iter()
        .map(|(_, bytes, nonce)| load_payload(bytes, nonce, key))
        .collect::<Result<Vec<_>, _>>()?;
    history::rebuild(current_body, &payloads, payloads.len() - 1)
}

/// Re-encrypts every history row of a note from `from` to `to` (either may be
/// `None` for plaintext), when the note's protection changes.
pub(super) fn rekey(
    conn: &Connection,
    note_id: &str,
    from: Option<&[u8; 32]>,
    to: Option<&[u8; 32]>,
) -> Result<(), String> {
    let rows = {
        let mut stmt = conn
            .prepare("SELECT id, payload, payload_nonce FROM note_history WHERE note_id = ?1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([note_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };
    for (id, bytes, nonce) in rows {
        let (bytes, nonce) = seal(open(bytes, nonce, from)?, to)?;
        conn.execute(
            "UPDATE note_history SET payload = ?2, payload_nonce = ?3 WHERE id = ?1",
            rusqlite::params![id, bytes, nonce],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
