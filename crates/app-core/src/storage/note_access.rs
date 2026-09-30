use rustc_hash::FxHashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const DEFAULT_UNLOCK_TTL: Duration = Duration::from_secs(15 * 60);

/// The key an unlocked encrypted note is read and written with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteAccessGrant {
    pub(crate) key: [u8; 32],
    pub(crate) encryption_salt: Vec<u8>,
}

#[derive(Debug, Clone)]
struct NoteAccessSession {
    grant: NoteAccessGrant,
    expires_at: Instant,
}

pub(crate) struct NoteAccessService {
    ttl: Duration,
    sessions: Mutex<FxHashMap<String, NoteAccessSession>>,
}

impl NoteAccessService {
    /// Take the session map, recovering from a poisoned lock.
    ///
    /// The guarded value is a plain map and every critical section below is a
    /// get/insert/remove plus an `Instant` comparison — nothing that can panic
    /// and leave the map logically inconsistent. Recovering keeps a panic
    /// elsewhere from permanently refusing every unlock for the rest of the
    /// process, which is what unwrapping here would do.
    fn sessions(&self) -> std::sync::MutexGuard<'_, FxHashMap<String, NoteAccessSession>> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn new() -> Self {
        Self::with_ttl(DEFAULT_UNLOCK_TTL)
    }

    fn with_ttl(ttl: Duration) -> Self {
        Self {
            ttl,
            sessions: Mutex::new(FxHashMap::default()),
        }
    }

    pub(crate) fn is_unlocked(&self, note_id: &str) -> bool {
        self.session(note_id).is_some()
    }

    pub(crate) fn session(&self, note_id: &str) -> Option<NoteAccessGrant> {
        let now = Instant::now();
        let mut sessions = self.sessions();
        let entry = sessions.get_mut(note_id)?;
        if entry.expires_at <= now {
            sessions.remove(note_id);
            return None;
        }

        // Sliding TTL: active sessions stay alive while the note is in use.
        entry.expires_at = now + self.ttl;
        Some(entry.grant.clone())
    }

    pub(crate) fn unlock_encrypted(&self, note_id: &str, key: [u8; 32], encryption_salt: &[u8]) {
        self.set_session(
            note_id,
            NoteAccessGrant {
                key,
                encryption_salt: encryption_salt.to_vec(),
            },
        );
    }

    pub(crate) fn clear(&self, note_id: &str) {
        self.sessions().remove(note_id);
    }

    fn set_session(&self, note_id: &str, grant: NoteAccessGrant) {
        let mut sessions = self.sessions();
        sessions.insert(
            note_id.to_string(),
            NoteAccessSession {
                grant,
                expires_at: Instant::now() + self.ttl,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn expired_sessions_are_relocked() {
        let service = NoteAccessService::with_ttl(Duration::from_millis(5));
        service.unlock_encrypted("n1", [1u8; 32], &[2u8; 16]);
        assert!(service.is_unlocked("n1"));
        thread::sleep(Duration::from_millis(8));
        assert!(!service.is_unlocked("n1"));
    }

    #[test]
    fn encrypted_sessions_keep_derived_key_material() {
        let service = NoteAccessService::new();
        let key = [9u8; 32];
        let salt = [4u8; 16];
        service.unlock_encrypted("n1", key, &salt);
        let Some(NoteAccessGrant {
            key: returned_key,
            encryption_salt,
        }) = service.session("n1")
        else {
            panic!("expected encrypted session");
        };
        assert_eq!(returned_key, key);
        assert_eq!(encryption_salt, salt.to_vec());
    }
}
