use rustc_hash::FxHashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const DEFAULT_UNLOCK_TTL: Duration = Duration::from_secs(15 * 60);

/// The key an unlocked encrypted note is read and written with, and the
/// wrapped form it was unwrapped from. A write only lands while the stored
/// wrapped key still matches, so a note re-keyed elsewhere is never written
/// with a stale key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteAccessGrant {
    pub(crate) key: [u8; 32],
    pub(crate) wrapped_key: Vec<u8>,
}

#[derive(Debug, Clone)]
struct Session<T> {
    grant: T,
    expires_at: Instant,
}

type Sessions<T> = Mutex<FxHashMap<String, Session<T>>>;

/// Keys of unlocked notes and collections, forgotten after a sliding TTL.
pub(crate) struct NoteAccessService {
    ttl: Duration,
    notes: Sessions<NoteAccessGrant>,
    collections: Sessions<[u8; 32]>,
}

/// Takes a session map, recovering from a poisoned lock.
///
/// The guarded value is a plain map and every critical section below is a
/// get/insert/remove plus an `Instant` comparison — nothing that can panic
/// and leave the map logically inconsistent. Recovering keeps a panic
/// elsewhere from permanently refusing every unlock for the rest of the
/// process, which is what unwrapping here would do.
fn locked<T>(sessions: &Sessions<T>) -> std::sync::MutexGuard<'_, FxHashMap<String, Session<T>>> {
    sessions
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl NoteAccessService {
    pub(crate) fn new() -> Self {
        Self::with_ttl(DEFAULT_UNLOCK_TTL)
    }

    fn with_ttl(ttl: Duration) -> Self {
        Self {
            ttl,
            notes: Mutex::new(FxHashMap::default()),
            collections: Mutex::new(FxHashMap::default()),
        }
    }

    pub(crate) fn is_unlocked(&self, note_id: &str) -> bool {
        self.session(note_id).is_some()
    }

    pub(crate) fn session(&self, note_id: &str) -> Option<NoteAccessGrant> {
        self.get(&self.notes, note_id)
    }

    pub(crate) fn unlock_encrypted(&self, note_id: &str, key: [u8; 32], wrapped_key: &[u8]) {
        self.set(
            &self.notes,
            note_id,
            NoteAccessGrant {
                key,
                wrapped_key: wrapped_key.to_vec(),
            },
        );
    }

    pub(crate) fn clear(&self, note_id: &str) {
        locked(&self.notes).remove(note_id);
    }

    pub(crate) fn collection_key(&self, collection_id: &str) -> Option<[u8; 32]> {
        self.get(&self.collections, collection_id)
    }

    pub(crate) fn unlock_collection(&self, collection_id: &str, key: [u8; 32]) {
        self.set(&self.collections, collection_id, key);
    }

    /// Forgets every unlocked note and collection.
    pub(crate) fn clear_all(&self) {
        locked(&self.notes).clear();
        locked(&self.collections).clear();
    }

    pub(crate) fn clear_collection(&self, collection_id: &str) {
        locked(&self.collections).remove(collection_id);
    }

    fn get<T: Clone>(&self, sessions: &Sessions<T>, id: &str) -> Option<T> {
        let now = Instant::now();
        let mut sessions = locked(sessions);
        let entry = sessions.get_mut(id)?;
        if entry.expires_at <= now {
            sessions.remove(id);
            return None;
        }

        // Sliding TTL: active sessions stay alive while in use.
        entry.expires_at = now + self.ttl;
        Some(entry.grant.clone())
    }

    fn set<T>(&self, sessions: &Sessions<T>, id: &str, grant: T) {
        locked(sessions).insert(
            id.to_string(),
            Session {
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
        service.unlock_collection("c1", [3u8; 32]);
        assert!(service.is_unlocked("n1"));
        assert_eq!(service.collection_key("c1"), Some([3u8; 32]));
        thread::sleep(Duration::from_millis(8));
        assert!(!service.is_unlocked("n1"));
        assert_eq!(service.collection_key("c1"), None);
    }

    #[test]
    fn encrypted_sessions_keep_the_key_and_its_wrapped_form() {
        let service = NoteAccessService::new();
        let key = [9u8; 32];
        let wrapped = [4u8; 60];
        service.unlock_encrypted("n1", key, &wrapped);
        let Some(NoteAccessGrant {
            key: returned_key,
            wrapped_key,
        }) = service.session("n1")
        else {
            panic!("expected encrypted session");
        };
        assert_eq!(returned_key, key);
        assert_eq!(wrapped_key, wrapped.to_vec());
    }
}
