//! Encryption of notes and collections.
//!
//! Every encrypted note's text and history are sealed (AES-256-GCM) with the
//! note's own random key. That key is stored wrapped, sealed in turn by what
//! protects the note:
//!
//! - its own password: the wrapping key is derived from the password and the
//!   note's `encryption_salt`, or
//! - an encrypted collection: the wrapping key is the collection's random
//!   key, itself stored wrapped with a key derived from the collection's
//!   password.
//!
//! Deriving a key from a password is slow on purpose (PBKDF2), so it runs
//! once per unlock; everything after it only unwraps keys. One password thus
//! unlocks a whole collection, and moving a note to another protector
//! re-wraps its key without touching its text or history.

use super::{
    derive_note_title_from_body, history_store, now_iso, Db, Note, NoteAccessGrant, NoteAccessMode,
    NoteSecurityRow,
};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use pbkdf2::pbkdf2_hmac;
use rusqlite::{Connection, OptionalExtension};
use sha2::Sha256;

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const PBKDF2_ITERATIONS: u32 = 200_000;
pub(super) const NOTE_LOCKED: &str = "note is locked; unlock first";
const COLLECTION_LOCKED: &str = "collection is locked; unlock it first";

pub(super) fn normalize_password(password: &str) -> Result<String, String> {
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

fn new_salt() -> Result<Vec<u8>, String> {
    let mut salt = [0u8; SALT_LEN];
    fill_random_bytes(&mut salt)?;
    Ok(salt.to_vec())
}

fn new_key() -> Result<[u8; 32], String> {
    let mut key = [0u8; 32];
    fill_random_bytes(&mut key)?;
    Ok(key)
}

fn derive_key(password: &str, salt: &[u8]) -> [u8; 32] {
    let mut key = [0u8; 32];
    pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, PBKDF2_ITERATIONS, &mut key);
    key
}

#[derive(Debug, Clone)]
pub(super) struct EncryptedBody {
    pub(super) nonce: Vec<u8>,
    pub(super) ciphertext: Vec<u8>,
}

pub(super) fn encrypt_bytes_with_key(
    bytes: &[u8],
    key: &[u8; 32],
) -> Result<EncryptedBody, String> {
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|e| format!("Failed to init cipher: {e}"))?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    fill_random_bytes(&mut nonce_bytes)?;
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), bytes)
        .map_err(|_| "Failed to encrypt note body".to_string())?;
    Ok(EncryptedBody {
        nonce: nonce_bytes.to_vec(),
        ciphertext,
    })
}

pub(super) fn decrypt_bytes_with_key(
    ciphertext: &[u8],
    nonce: &[u8],
    key: &[u8; 32],
) -> Result<Vec<u8>, String> {
    if nonce.len() != NONCE_LEN {
        return Err("encrypted note nonce invalid".to_string());
    }
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|e| format!("Failed to init cipher: {e}"))?;
    cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| "invalid password".to_string())
}

pub(super) fn encrypt_note_body_with_key(
    body: &str,
    key: &[u8; 32],
) -> Result<EncryptedBody, String> {
    encrypt_bytes_with_key(body.as_bytes(), key)
}

pub(super) fn decrypt_note_body_with_key(
    ciphertext: &[u8],
    nonce: &[u8],
    key: &[u8; 32],
) -> Result<String, String> {
    let plain = decrypt_bytes_with_key(ciphertext, nonce, key)?;
    String::from_utf8(plain).map_err(|_| "encrypted note content invalid UTF-8".to_string())
}

/// `key` sealed with `wrapping`, stored as the nonce followed by the
/// ciphertext.
fn wrap_key(wrapping: &[u8; 32], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    let sealed = encrypt_bytes_with_key(key, wrapping)?;
    let mut wrapped = sealed.nonce;
    wrapped.extend(sealed.ciphertext);
    Ok(wrapped)
}

/// The key sealed by [`wrap_key`]. A wrong wrapping key means the password
/// it came from was wrong.
fn unwrap_key(wrapping: &[u8; 32], wrapped: &[u8]) -> Result<[u8; 32], String> {
    if wrapped.len() <= NONCE_LEN {
        return Err("stored key invalid".to_string());
    }
    let (nonce, ciphertext) = wrapped.split_at(NONCE_LEN);
    let key = decrypt_bytes_with_key(ciphertext, nonce, wrapping)
        .map_err(|_| "invalid password".to_string())?;
    key.try_into().map_err(|_| "stored key invalid".to_string())
}

/// What wraps a note's key.
enum Protector<'a> {
    /// The note's own password: a key derived from it and `salt`.
    Password { salt: Vec<u8>, key: [u8; 32] },
    /// An encrypted collection's key.
    Collection { id: &'a str, key: [u8; 32] },
}

impl Protector<'_> {
    fn password(password: &str) -> Result<Self, String> {
        let salt = new_salt()?;
        let key = derive_key(password, &salt);
        Ok(Self::Password { salt, key })
    }

    fn wrapping_key(&self) -> &[u8; 32] {
        match self {
            Self::Password { key, .. } | Self::Collection { key, .. } => key,
        }
    }

    fn salt(&self) -> Option<&[u8]> {
        match self {
            Self::Password { salt, .. } => Some(salt),
            Self::Collection { .. } => None,
        }
    }

    fn collection_id(&self) -> Option<&str> {
        match self {
            Self::Password { .. } => None,
            Self::Collection { id, .. } => Some(id),
        }
    }
}

/// The key of an encrypted collection, unwrapped with its password.
fn collection_key_with_password(
    conn: &Connection,
    collection_id: &str,
    password: &str,
) -> Result<[u8; 32], String> {
    let (salt, wrapped): (Vec<u8>, Vec<u8>) = conn
        .query_row(
            "SELECT key_salt, wrapped_key FROM collection_keys WHERE collection_id = ?1",
            [collection_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "collection is not encrypted".to_string())?;
    unwrap_key(&derive_key(password, &salt), &wrapped)
}

fn is_collection_encrypted(conn: &Connection, collection_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM collection_keys WHERE collection_id = ?1)",
        [collection_id],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}

/// Name of an encrypted collection `note_id` belongs to, preferring
/// `collection_id` when given. Members of an encrypted collection stay
/// encrypted, under its password, while they belong to it.
fn encrypted_collection_of(
    conn: &Connection,
    note_id: &str,
    only: Option<&str>,
) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT c.name FROM note_collections nc
         JOIN collection_keys k ON k.collection_id = nc.collection_id
         JOIN collections c ON c.id = nc.collection_id
         WHERE nc.note_id = ?1 AND (?2 IS NULL OR nc.collection_id = ?2)
         ORDER BY c.name COLLATE NOCASE
         LIMIT 1",
        rusqlite::params![note_id, only],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// Notes whose keys the collection's key wraps, with their wrapped keys.
fn protected_notes(
    conn: &Connection,
    collection_id: &str,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut stmt = conn
        .prepare("SELECT id, wrapped_key FROM notes WHERE key_collection_id = ?1")
        .map_err(|e| e.to_string())?;
    let notes = stmt
        .query_map([collection_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(notes)
}

/// Refuses to drop a collection whose key still protects notes, since they
/// could not be opened again.
pub(super) fn ensure_collection_protects_no_notes(
    conn: &Connection,
    collection_id: &str,
) -> Result<(), String> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(1) FROM notes WHERE key_collection_id = ?1",
            [collection_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if count > 0 {
        return Err(format!(
            "decrypt the collection first: its password protects {count} note{}",
            if count == 1 { "" } else { "s" }
        ));
    }
    Ok(())
}

/// Moves a note's key under `protector`. The note must still hold the key
/// `unlocked` came from; otherwise its unlock is dropped.
fn rewrap_note_key(
    conn: &Connection,
    db: &Db,
    id: &str,
    unlocked: &NoteAccessGrant,
    protector: &Protector,
) -> Result<NoteAccessGrant, String> {
    let wrapped = wrap_key(protector.wrapping_key(), &unlocked.key)?;
    let changed = conn
        .execute(
            "UPDATE notes
             SET encryption_salt = ?2, wrapped_key = ?3, key_collection_id = ?4
             WHERE id = ?1 AND wrapped_key = ?5",
            rusqlite::params![
                id,
                protector.salt(),
                wrapped,
                protector.collection_id(),
                unlocked.wrapped_key
            ],
        )
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        db.note_access.clear(id);
        return Err(NOTE_LOCKED.to_string());
    }
    Ok(NoteAccessGrant {
        key: unlocked.key,
        wrapped_key: wrapped,
    })
}

impl Db {
    /// Writes `body` over an unlocked encrypted note, sealed with its key.
    /// Refused, and the unlock dropped, when the note's key changed since.
    pub(super) fn write_encrypted_body(
        &self,
        conn: &Connection,
        id: &str,
        unlocked: &NoteAccessGrant,
        body: &str,
        now: &str,
    ) -> Result<(), String> {
        let encrypted = encrypt_note_body_with_key(body, &unlocked.key)?;
        let changed = conn
            .execute(
                "UPDATE notes
                 SET body = '',
                     note_title = CASE WHEN title_pinned = 1 THEN note_title ELSE '' END,
                     encrypted_body = ?2,
                     encryption_nonce = ?3,
                     updated_at = ?4
                 WHERE id = ?1 AND wrapped_key = ?5",
                rusqlite::params![
                    id,
                    encrypted.ciphertext,
                    encrypted.nonce,
                    now,
                    unlocked.wrapped_key
                ],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            self.note_access.clear(id);
            return Err(NOTE_LOCKED.to_string());
        }
        Ok(())
    }

    /// The key of an encrypted note, unwrapped with `password`: the note's
    /// own, or its collection's for a note an encrypted collection protects.
    pub(super) fn note_key_with_password(
        &self,
        conn: &Connection,
        security: &NoteSecurityRow,
        password: &str,
    ) -> Result<[u8; 32], String> {
        let wrapped = security
            .wrapped_key
            .as_deref()
            .ok_or_else(|| "encrypted note key missing".to_string())?;
        let wrapping = match security.key_collection_id.as_deref() {
            Some(collection_id) => collection_key_with_password(conn, collection_id, password)?,
            None => derive_key(
                password,
                security
                    .encryption_salt
                    .as_deref()
                    .ok_or_else(|| "encrypted note salt missing".to_string())?,
            ),
        };
        unwrap_key(&wrapping, wrapped)
    }

    /// Encrypts a plain note's text and history with a new random key that
    /// `protector` wraps. With `pin_title` its title is pinned so it stays
    /// visible; an unpinned title is not stored while encrypted. The text is
    /// unchanged, so the note's revision is too.
    fn seal_plain_note(
        &self,
        conn: &Connection,
        id: &str,
        protector: &Protector,
        pin_title: bool,
    ) -> Result<NoteAccessGrant, String> {
        let body: String = conn
            .query_row("SELECT body FROM notes WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Note not found".to_string())?;
        let key = new_key()?;
        let wrapped = wrap_key(protector.wrapping_key(), &key)?;
        let encrypted = encrypt_note_body_with_key(&body, &key)?;
        history_store::rekey(conn, id, None, Some(&key))?;
        conn.execute(
            "UPDATE notes
             SET body = '',
                 note_title = CASE WHEN title_pinned = 1 OR ?7 THEN note_title ELSE '' END,
                 title_pinned = CASE WHEN ?7 THEN 1 ELSE title_pinned END,
                 access_mode = 'encrypted',
                 encryption_salt = ?2,
                 wrapped_key = ?3,
                 key_collection_id = ?4,
                 encryption_nonce = ?5,
                 encrypted_body = ?6
             WHERE id = ?1",
            rusqlite::params![
                id,
                protector.salt(),
                wrapped,
                protector.collection_id(),
                encrypted.nonce,
                encrypted.ciphertext,
                pin_title
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(NoteAccessGrant {
            key,
            wrapped_key: wrapped,
        })
    }

    /// Stores an encrypted note's text and history in the clear again,
    /// keeping its revision.
    fn unseal_note(
        &self,
        conn: &Connection,
        id: &str,
        security: &NoteSecurityRow,
        key: &[u8; 32],
    ) -> Result<(), String> {
        let body = decrypt_note_body_with_key(
            security
                .encrypted_body
                .as_deref()
                .ok_or_else(|| "encrypted note payload missing".to_string())?,
            security
                .encryption_nonce
                .as_deref()
                .ok_or_else(|| "encrypted note nonce missing".to_string())?,
            key,
        )?;
        history_store::rekey(conn, id, Some(key), None)?;
        conn.execute(
            "UPDATE notes
             SET body = ?2,
                 note_title = CASE WHEN title_pinned = 1 THEN note_title ELSE ?3 END,
                 access_mode = 'none',
                 encryption_salt = NULL,
                 wrapped_key = NULL,
                 key_collection_id = NULL,
                 encryption_nonce = NULL,
                 encrypted_body = NULL
             WHERE id = ?1",
            rusqlite::params![id, body, derive_note_title_from_body(&body)],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Encrypts the plain notes among `note_ids`, which just joined
    /// `collection_id`, when that collection is encrypted. Call inside the
    /// transaction that adds the memberships and apply the returned unlocks
    /// once it commits. A locked collection refuses plain notes.
    pub(super) fn seal_for_collection(
        &self,
        conn: &Connection,
        collection_id: &str,
        note_ids: &[String],
        pin_title: bool,
    ) -> Result<Vec<(String, NoteAccessGrant)>, String> {
        if !is_collection_encrypted(conn, collection_id)? {
            return Ok(Vec::new());
        }
        let mut plain = Vec::new();
        for id in note_ids {
            let mode: Option<String> = conn
                .query_row("SELECT access_mode FROM notes WHERE id = ?1", [id], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(|e| e.to_string())?;
            if mode.as_deref() == Some("none") {
                plain.push(id);
            }
        }
        if plain.is_empty() {
            return Ok(Vec::new());
        }
        let key = self
            .note_access
            .collection_key(collection_id)
            .ok_or_else(|| COLLECTION_LOCKED.to_string())?;
        let protector = Protector::Collection {
            id: collection_id,
            key,
        };
        plain
            .into_iter()
            .map(|id| {
                Ok((
                    id.clone(),
                    self.seal_plain_note(conn, id, &protector, pin_title)?,
                ))
            })
            .collect()
    }

    pub(super) fn apply_unlocks(&self, unlocks: Vec<(String, NoteAccessGrant)>) {
        for (id, grant) in unlocks {
            self.note_access
                .unlock_encrypted(&id, grant.key, &grant.wrapped_key);
        }
    }

    /// Encrypts a new, empty note with its own `password`.
    pub(super) fn seal_new_note_with_password(
        &self,
        conn: &Connection,
        id: &str,
        password: &str,
    ) -> Result<NoteAccessGrant, String> {
        let password = normalize_password(password)?;
        self.seal_plain_note(conn, id, &Protector::password(&password)?, false)
    }

    /// Forgets the key this handle holds for note `id`, so it is locked
    /// again until unlocked with its password.
    pub fn lock_note(&self, id: &str) {
        self.note_access.clear(id);
    }

    pub fn unlock_note(&self, id: &str, password: &str) -> Result<Note, String> {
        let password = normalize_password(password)?;
        let conn = self.conn.lock()?;
        let security = self
            .load_note_security(&conn, id)?
            .ok_or_else(|| "Note not found".to_string())?;
        if security.access_mode == NoteAccessMode::Encrypted {
            match security.key_collection_id.as_deref() {
                Some(collection_id) => {
                    self.unlock_collection_in(&conn, collection_id, &password)?
                }
                None => {
                    let key = self.note_key_with_password(&conn, &security, &password)?;
                    let wrapped = security.wrapped_key.as_deref().unwrap_or_default();
                    self.note_access.unlock_encrypted(id, key, wrapped);
                }
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

    /// Encrypts a note with its own password. An encrypted note must be
    /// unlocked: its key is kept and re-wrapped, which changes its password
    /// or takes it out of its collection's protection.
    pub fn encrypt_note(&self, id: &str, password: &str) -> Result<Note, String> {
        let password = normalize_password(password)?;
        let conn = self.conn.lock()?;
        let security = self
            .load_note_security(&conn, id)?
            .ok_or_else(|| "Note not found".to_string())?;
        if let Some(collection_id) = security.key_collection_id.as_deref() {
            if let Some(name) = encrypted_collection_of(&conn, id, Some(collection_id))? {
                return Err(format!(
                    "the note is protected by the encrypted collection \"{name}\"; \
                     remove it from the collection to give it its own password"
                ));
            }
        }
        let protector = Protector::password(&password)?;
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let grant = match security.access_mode {
            // The title is pinned so it stays visible while encrypted.
            NoteAccessMode::None => self.seal_plain_note(&tx, id, &protector, true)?,
            NoteAccessMode::Encrypted => {
                let unlocked = self
                    .note_access
                    .session(id)
                    .ok_or_else(|| NOTE_LOCKED.to_string())?;
                rewrap_note_key(&tx, self, id, &unlocked, &protector)?
            }
        };
        tx.commit().map_err(|e| e.to_string())?;
        self.note_access
            .unlock_encrypted(id, grant.key, &grant.wrapped_key);
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
        let conn = self.conn.lock()?;
        let security = self
            .load_note_security(&conn, id)?
            .ok_or_else(|| "Note not found".to_string())?;
        if security.access_mode == NoteAccessMode::Encrypted {
            if let Some(name) = encrypted_collection_of(&conn, id, None)? {
                return Err(format!(
                    "the note is in the encrypted collection \"{name}\"; \
                     remove it from the collection or decrypt the collection first"
                ));
            }
            let key = self.note_key_with_password(&conn, &security, &password)?;
            let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
            self.unseal_note(&tx, id, &security, &key)?;
            tx.commit().map_err(|e| e.to_string())?;
            self.note_access.clear(id);
        }
        self.load_note_with_access(&conn, id)?
            .ok_or_else(|| "Note not found after decrypt".to_string())
    }

    /// Unlocks an encrypted collection and every note its key protects.
    pub fn unlock_collection(&self, collection_id: &str, password: &str) -> Result<(), String> {
        let password = normalize_password(password)?;
        let conn = self.conn.lock()?;
        self.unlock_collection_in(&conn, collection_id, &password)
    }

    fn unlock_collection_in(
        &self,
        conn: &Connection,
        collection_id: &str,
        password: &str,
    ) -> Result<(), String> {
        let collection_key = collection_key_with_password(conn, collection_id, password)?;
        for (note_id, wrapped) in protected_notes(conn, collection_id)? {
            let key = unwrap_key(&collection_key, &wrapped)?;
            self.note_access.unlock_encrypted(&note_id, key, &wrapped);
        }
        self.note_access
            .unlock_collection(collection_id, collection_key);
        Ok(())
    }

    pub fn is_collection_unlocked(&self, collection_id: &str) -> bool {
        self.note_access.collection_key(collection_id).is_some()
    }

    /// Name of the encrypted collection whose password unlocks the note;
    /// `None` for a note with its own password or no encryption.
    pub fn note_key_collection_name(&self, id: &str) -> Result<Option<String>, String> {
        let conn = self.conn.lock()?;
        conn.query_row(
            "SELECT c.name FROM notes n JOIN collections c ON c.id = n.key_collection_id
             WHERE n.id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    /// Encrypts a collection with a new key that `password` protects; notes
    /// added to it later are encrypted with it too. Plain members are
    /// encrypted with their titles pinned, and unlocked notes with their own
    /// password move under the collection's. Members that are locked or
    /// protected by another collection keep their protection. Returns how
    /// many notes the collection now protects and how many were skipped.
    pub fn encrypt_collection(
        &self,
        collection_id: &str,
        password: &str,
    ) -> Result<(usize, usize), String> {
        let password = normalize_password(password)?;
        let mut conn = self.conn.lock()?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let encrypted: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM collection_keys WHERE collection_id = c.id)
                 FROM collections c WHERE c.id = ?1",
                [collection_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "collection not found".to_string())?;
        if encrypted {
            return Err("collection is already encrypted".to_string());
        }
        let collection_key = new_key()?;
        let salt = new_salt()?;
        let wrapped = wrap_key(&derive_key(&password, &salt), &collection_key)?;
        tx.execute(
            "INSERT INTO collection_keys (collection_id, key_salt, wrapped_key, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![collection_id, salt, wrapped, now_iso()],
        )
        .map_err(|e| e.to_string())?;

        let members = {
            let mut stmt = tx
                .prepare("SELECT note_id FROM note_collections WHERE collection_id = ?1")
                .map_err(|e| e.to_string())?;
            let ids = stmt
                .query_map([collection_id], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            ids
        };
        let protector = Protector::Collection {
            id: collection_id,
            key: collection_key,
        };
        let mut unlocks = Vec::new();
        let mut skipped = 0;
        for note_id in members {
            let Some(security) = self.load_note_security(&tx, &note_id)? else {
                continue;
            };
            let unlocked = self.note_access.session(&note_id);
            let grant = match (security.access_mode, unlocked) {
                (NoteAccessMode::None, _) => {
                    Some(self.seal_plain_note(&tx, &note_id, &protector, true)?)
                }
                (NoteAccessMode::Encrypted, Some(unlocked))
                    if security.key_collection_id.is_none() =>
                {
                    rewrap_note_key(&tx, self, &note_id, &unlocked, &protector).ok()
                }
                _ => None,
            };
            match grant {
                Some(grant) => unlocks.push((note_id, grant)),
                None => skipped += 1,
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        let protected = unlocks.len();
        self.apply_unlocks(unlocks);
        self.note_access
            .unlock_collection(collection_id, collection_key);
        Ok((protected, skipped))
    }

    /// Decrypts every note the collection's key protects, members or not,
    /// and removes the key. Returns how many notes were decrypted.
    pub fn decrypt_collection(&self, collection_id: &str, password: &str) -> Result<usize, String> {
        let password = normalize_password(password)?;
        let mut conn = self.conn.lock()?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let collection_key = collection_key_with_password(&tx, collection_id, &password)?;
        let notes = protected_notes(&tx, collection_id)?;
        for (note_id, wrapped) in &notes {
            let key = unwrap_key(&collection_key, wrapped)?;
            let security = self
                .load_note_security(&tx, note_id)?
                .ok_or_else(|| "Note not found".to_string())?;
            self.unseal_note(&tx, note_id, &security, &key)?;
        }
        tx.execute(
            "DELETE FROM collection_keys WHERE collection_id = ?1",
            [collection_id],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        for (note_id, _) in &notes {
            self.note_access.clear(note_id);
        }
        self.note_access.clear_collection(collection_id);
        Ok(notes.len())
    }
}
