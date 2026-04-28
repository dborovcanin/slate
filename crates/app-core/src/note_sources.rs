use crate::storage::{Db, Note, NoteAccessMode, NoteModules, NoteSummary};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use ulid::Ulid;

pub const MARKDOWN_NOTE_ID_PREFIX: &str = "mdfile:";
const NOTE_TITLE_MAX_CHARS: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteIdentity {
    DbNote(String),
    FileNote(PathBuf),
}

impl NoteIdentity {
    pub fn to_note_id(&self) -> String {
        match self {
            Self::DbNote(id) => id.clone(),
            Self::FileNote(path) => note_id_for_markdown_file(path),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteSourceCapabilities {
    pub can_save: bool,
    pub can_delete: bool,
    pub can_lock: bool,
    pub can_encrypt: bool,
    pub can_module_persist: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SaveOptions {
    pub expected_revision: Option<String>,
    pub force: bool,
}

#[derive(Clone)]
pub struct NoteSourceService {
    db: Db,
}

impl NoteSourceService {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub fn parse_identity(&self, note_id: &str) -> NoteIdentity {
        note_identity_from_id(note_id)
    }

    pub fn capabilities_for_note_id(&self, note_id: &str) -> NoteSourceCapabilities {
        self.capabilities_for_identity(&self.parse_identity(note_id))
    }

    pub fn capabilities_for_identity(&self, identity: &NoteIdentity) -> NoteSourceCapabilities {
        match identity {
            NoteIdentity::DbNote(_) => NoteSourceCapabilities {
                can_save: true,
                can_delete: true,
                can_lock: true,
                can_encrypt: true,
                can_module_persist: true,
            },
            NoteIdentity::FileNote(_) => NoteSourceCapabilities {
                can_save: true,
                can_delete: false,
                can_lock: false,
                can_encrypt: false,
                can_module_persist: false,
            },
        }
    }

    pub fn open_note_by_id(&self, note_id: &str) -> Result<Option<Note>, String> {
        self.open_note(&self.parse_identity(note_id))
    }

    pub fn open_note(&self, identity: &NoteIdentity) -> Result<Option<Note>, String> {
        match identity {
            NoteIdentity::DbNote(id) => self.db.get_note(id),
            NoteIdentity::FileNote(path) => Ok(Some(note_from_markdown_path(path)?)),
        }
    }

    pub fn save_note_by_id(
        &self,
        note_id: &str,
        body: &str,
        options: SaveOptions,
    ) -> Result<Note, String> {
        self.save_note(&self.parse_identity(note_id), body, options)
    }

    pub fn save_note(
        &self,
        identity: &NoteIdentity,
        body: &str,
        options: SaveOptions,
    ) -> Result<Note, String> {
        match identity {
            NoteIdentity::DbNote(id) => {
                let current_revision = self.db.get_note_updated_at(id)?;
                ensure_revision_matches(
                    current_revision.as_deref(),
                    options.expected_revision.as_deref(),
                    options.force,
                    "note changed since last load",
                )?;
                self.db.save_note(id, body)
            }
            NoteIdentity::FileNote(path) => {
                let current_revision = revision_from_markdown_path(path)?;
                ensure_revision_matches(
                    current_revision.as_deref(),
                    options.expected_revision.as_deref(),
                    options.force,
                    "file changed on disk",
                )?;
                write_markdown_file_atomically(path, body)?;
                Ok(note_from_markdown_path(path)?)
            }
        }
    }

    pub fn get_note_meta_by_id(&self, note_id: &str) -> Result<Option<NoteSummary>, String> {
        self.get_note_meta(&self.parse_identity(note_id))
    }

    pub fn get_note_meta(&self, identity: &NoteIdentity) -> Result<Option<NoteSummary>, String> {
        match identity {
            NoteIdentity::DbNote(id) => self.db.get_note_meta(id),
            NoteIdentity::FileNote(path) => Ok(Some(note_summary_from_markdown_path(path)?)),
        }
    }

    pub fn get_note_revision_by_id(&self, note_id: &str) -> Result<Option<String>, String> {
        self.get_note_revision(&self.parse_identity(note_id))
    }

    pub fn get_note_revision(&self, identity: &NoteIdentity) -> Result<Option<String>, String> {
        match identity {
            NoteIdentity::DbNote(id) => self.db.get_note_updated_at(id),
            NoteIdentity::FileNote(path) => revision_from_markdown_path(path),
        }
    }

    pub fn list_notes_meta(
        &self,
        active_note_id: Option<&str>,
    ) -> Result<Vec<NoteSummary>, String> {
        let mut notes = self.db.list_notes_meta()?;
        notes.retain(|note| matches!(note_identity_from_id(&note.id), NoteIdentity::DbNote(_)));
        if let Some(active_id) = active_note_id {
            let active_identity = note_identity_from_id(active_id);
            if let NoteIdentity::FileNote(_) = active_identity {
                if let Some(summary) = self.get_note_meta(&active_identity)? {
                    if !notes.iter().any(|note| note.id == summary.id) {
                        notes.insert(0, summary);
                    }
                }
            }
        }
        Ok(notes)
    }
}

pub fn note_id_for_markdown_file(path: &Path) -> String {
    let encoded = URL_SAFE_NO_PAD.encode(path.to_string_lossy().as_bytes());
    format!("{MARKDOWN_NOTE_ID_PREFIX}{encoded}")
}

pub fn markdown_file_path_from_note_id(note_id: &str) -> Option<PathBuf> {
    let encoded = note_id.strip_prefix(MARKDOWN_NOTE_ID_PREFIX)?;
    let decoded = URL_SAFE_NO_PAD.decode(encoded).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let path = PathBuf::from(decoded);
    if !path.is_absolute() || !is_supported_markdown_path(&path) {
        return None;
    }
    Some(path)
}

pub fn is_markdown_file_note_id(note_id: &str) -> bool {
    markdown_file_path_from_note_id(note_id).is_some()
}

pub fn note_identity_from_id(note_id: &str) -> NoteIdentity {
    if let Some(path) = markdown_file_path_from_note_id(note_id) {
        NoteIdentity::FileNote(path)
    } else {
        NoteIdentity::DbNote(note_id.to_string())
    }
}

pub fn is_supported_markdown_path(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "md" | "markdown" | "mdown" | "mkd"
    )
}

pub fn resolve_markdown_file_path(raw: &str, cwd: &Path) -> Result<PathBuf, String> {
    let candidate = PathBuf::from(raw);
    let absolute = if candidate.is_absolute() {
        candidate
    } else {
        cwd.join(candidate)
    };

    if !is_supported_markdown_path(&absolute) {
        return Err(format!(
            "Unsupported file extension for '{}'; expected markdown (.md/.markdown/.mdown/.mkd)",
            absolute.display()
        ));
    }

    if absolute.exists() {
        std::fs::canonicalize(&absolute).map_err(|e| {
            format!(
                "Failed to canonicalize markdown file '{}': {e}",
                absolute.display()
            )
        })
    } else {
        Ok(absolute)
    }
}

fn ensure_revision_matches(
    current_revision: Option<&str>,
    expected_revision: Option<&str>,
    force: bool,
    message: &str,
) -> Result<(), String> {
    if force || expected_revision.is_none() {
        return Ok(());
    }
    if current_revision == expected_revision {
        return Ok(());
    }
    Err(format!("{message}; use :w! to force save"))
}

fn now_iso() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}

fn iso_from_system_time(ts: SystemTime) -> Option<String> {
    let ts: time::OffsetDateTime = ts.into();
    ts.format(&time::format_description::well_known::Rfc3339)
        .ok()
}

fn revision_from_markdown_path(path: &Path) -> Result<Option<String>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let metadata = fs::metadata(path).map_err(|e| {
        format!(
            "Failed to read metadata for markdown file '{}': {e}",
            path.display()
        )
    })?;
    Ok(metadata
        .modified()
        .ok()
        .and_then(iso_from_system_time)
        .or_else(|| metadata.created().ok().and_then(iso_from_system_time)))
}

fn read_markdown_file(path: &Path) -> Result<String, String> {
    if path.exists() {
        fs::read_to_string(path)
            .map_err(|e| format!("Failed to read markdown file '{}': {e}", path.display()))
    } else {
        Ok(String::new())
    }
}

fn write_markdown_file_atomically(path: &Path, body: &str) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| {
        format!(
            "Failed to determine parent directory for markdown file '{}'",
            path.display()
        )
    })?;
    fs::create_dir_all(parent).map_err(|e| {
        format!(
            "Failed to create parent directory for markdown file '{}': {e}",
            path.display()
        )
    })?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("note.md");
    let tmp_path = parent.join(format!(".{file_name}.{}.tmp", Ulid::new()));
    let write_result = (|| -> Result<(), String> {
        let mut tmp = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp_path)
            .map_err(|e| {
                format!(
                    "Failed to create temporary markdown file '{}': {e}",
                    tmp_path.display()
                )
            })?;
        tmp.write_all(body.as_bytes()).map_err(|e| {
            format!(
                "Failed writing temporary markdown file '{}': {e}",
                tmp_path.display()
            )
        })?;
        tmp.sync_all().map_err(|e| {
            format!(
                "Failed to sync temporary markdown file '{}': {e}",
                tmp_path.display()
            )
        })?;
        fs::rename(&tmp_path, path).map_err(|e| {
            format!(
                "Failed to replace markdown file '{}' with '{}': {e}",
                path.display(),
                tmp_path.display()
            )
        })?;
        if let Ok(dir) = OpenOptions::new().read(true).open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    write_result
}

fn markdown_file_timestamps(path: &Path) -> Result<(String, String), String> {
    let now = now_iso();
    if !path.exists() {
        return Ok((now.clone(), now));
    }

    let metadata = fs::metadata(path).map_err(|e| {
        format!(
            "Failed to read metadata for markdown file '{}': {e}",
            path.display()
        )
    })?;
    let created = metadata.created().ok().and_then(iso_from_system_time);
    let modified = metadata.modified().ok().and_then(iso_from_system_time);
    let created_at = created
        .clone()
        .or_else(|| modified.clone())
        .unwrap_or_else(|| now.clone());
    let updated_at = modified.or(created).unwrap_or(now);
    Ok((created_at, updated_at))
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

fn note_from_markdown_path(path: &Path) -> Result<Note, String> {
    let body = read_markdown_file(path)?;
    let (created_at, updated_at) = markdown_file_timestamps(path)?;
    Ok(Note {
        id: note_id_for_markdown_file(path),
        body,
        modules: NoteModules::default(),
        access_mode: NoteAccessMode::None,
        is_unlocked: true,
        created_at,
        updated_at,
    })
}

fn note_summary_from_markdown_path(path: &Path) -> Result<NoteSummary, String> {
    let note = note_from_markdown_path(path)?;
    let title = derive_note_title_from_body(&note.body);
    let body_prefix = note.body.chars().take(200).collect();
    Ok(NoteSummary {
        id: note.id,
        title,
        body_prefix,
        access_mode: NoteAccessMode::None,
        is_unlocked: true,
        updated_at: note.updated_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db_path() -> PathBuf {
        std::env::temp_dir().join(format!("note-sources-{}.db", Ulid::new()))
    }

    fn cleanup_db_files(path: &Path) {
        let _ = fs::remove_file(path);
        let journal = path.with_extension("db-journal");
        let _ = fs::remove_file(journal);
        let wal = format!("{}-wal", path.display());
        let shm = format!("{}-shm", path.display());
        let _ = fs::remove_file(wal);
        let _ = fs::remove_file(shm);
    }

    #[test]
    fn contract_db_source_open_save_meta_revision() {
        let db_path = temp_db_path();
        let db = Db::open(db_path.clone()).expect("db opens");
        db.save_note("n1", "first body").expect("seed note");
        let service = NoteSourceService::new(db.clone());

        let opened = service
            .open_note_by_id("n1")
            .expect("open db note")
            .expect("note exists");
        assert_eq!(opened.body, "first body");

        let rev = service
            .get_note_revision_by_id("n1")
            .expect("revision lookup")
            .expect("revision exists");
        let saved = service
            .save_note_by_id(
                "n1",
                "second body",
                SaveOptions {
                    expected_revision: Some(rev),
                    force: false,
                },
            )
            .expect("save succeeds");
        assert_eq!(saved.body, "second body");

        let meta = service
            .get_note_meta_by_id("n1")
            .expect("meta lookup")
            .expect("meta exists");
        assert_eq!(meta.id, "n1");
        assert_eq!(meta.title, "second body");

        let listed = service.list_notes_meta(None).expect("list meta");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, "n1");

        let caps = service.capabilities_for_note_id("n1");
        assert!(caps.can_delete);
        assert!(caps.can_lock);
        assert!(caps.can_encrypt);
        assert!(caps.can_module_persist);

        drop(db);
        cleanup_db_files(&db_path);
    }

    #[test]
    fn contract_file_source_open_save_meta_revision() {
        let db_path = temp_db_path();
        let db = Db::open(db_path.clone()).expect("db opens");
        let service = NoteSourceService::new(db.clone());
        let markdown_path =
            std::env::temp_dir().join(format!("note-source-file-{}.md", Ulid::new()));
        fs::write(&markdown_path, "file body").expect("seed file");
        let note_id = note_id_for_markdown_file(&markdown_path);

        let opened = service
            .open_note_by_id(&note_id)
            .expect("open file note")
            .expect("note exists");
        assert_eq!(opened.body, "file body");

        let rev = service
            .get_note_revision_by_id(&note_id)
            .expect("revision lookup")
            .expect("revision exists");
        let saved = service
            .save_note_by_id(
                &note_id,
                "updated file body",
                SaveOptions {
                    expected_revision: Some(rev),
                    force: false,
                },
            )
            .expect("save file succeeds");
        assert_eq!(saved.body, "updated file body");
        assert_eq!(
            fs::read_to_string(&markdown_path).expect("file read"),
            "updated file body"
        );

        let meta = service
            .get_note_meta_by_id(&note_id)
            .expect("meta lookup")
            .expect("meta exists");
        assert_eq!(meta.id, note_id);
        assert_eq!(meta.title, "updated file body");

        let listed_without_active = service.list_notes_meta(None).expect("list without active");
        assert_eq!(listed_without_active.len(), 1);
        assert_eq!(listed_without_active[0].id, "welcome");

        let listed_with_active = service
            .list_notes_meta(Some(&note_id))
            .expect("list with active");
        assert_eq!(listed_with_active.len(), 2);
        assert_eq!(listed_with_active[0].id, note_id);

        let caps = service.capabilities_for_note_id(&note_id);
        assert!(caps.can_save);
        assert!(!caps.can_delete);
        assert!(!caps.can_lock);
        assert!(!caps.can_encrypt);
        assert!(!caps.can_module_persist);

        let _ = fs::remove_file(markdown_path);
        drop(db);
        cleanup_db_files(&db_path);
    }

    #[test]
    fn contract_conflict_requires_force_for_file_and_db_sources() {
        let db_path = temp_db_path();
        let db = Db::open(db_path.clone()).expect("db opens");
        let service = NoteSourceService::new(db.clone());

        db.save_note("n1", "alpha").expect("seed db note");
        let db_rev = service
            .get_note_revision_by_id("n1")
            .expect("db revision lookup")
            .expect("db revision exists");
        db.save_note("n1", "beta").expect("external db write");
        let conflict = service
            .save_note_by_id(
                "n1",
                "gamma",
                SaveOptions {
                    expected_revision: Some(db_rev),
                    force: false,
                },
            )
            .expect_err("db save should conflict");
        assert!(conflict.contains(":w!"));
        service
            .save_note_by_id(
                "n1",
                "gamma",
                SaveOptions {
                    expected_revision: None,
                    force: true,
                },
            )
            .expect("forced db save should pass");

        let markdown_path =
            std::env::temp_dir().join(format!("note-source-conflict-{}.md", Ulid::new()));
        fs::write(&markdown_path, "first").expect("seed file");
        let note_id = note_id_for_markdown_file(&markdown_path);
        let file_rev = service
            .get_note_revision_by_id(&note_id)
            .expect("file revision lookup")
            .expect("file revision exists");
        fs::write(&markdown_path, "external").expect("external file write");
        let file_conflict = service
            .save_note_by_id(
                &note_id,
                "local",
                SaveOptions {
                    expected_revision: Some(file_rev),
                    force: false,
                },
            )
            .expect_err("file save should conflict");
        assert!(file_conflict.contains(":w!"));
        service
            .save_note_by_id(
                &note_id,
                "forced local",
                SaveOptions {
                    expected_revision: None,
                    force: true,
                },
            )
            .expect("forced file save should pass");

        let _ = fs::remove_file(markdown_path);
        drop(db);
        cleanup_db_files(&db_path);
    }

    #[test]
    fn contract_list_composition_keeps_db_notes_and_active_file_note() {
        let db_path = temp_db_path();
        let db = Db::open(db_path.clone()).expect("db opens");
        let service = NoteSourceService::new(db.clone());

        db.save_note("n1", "alpha").expect("seed db note");
        db.save_note("n2", "beta").expect("seed db note");

        let markdown_path =
            std::env::temp_dir().join(format!("note-source-list-{}.md", Ulid::new()));
        fs::write(&markdown_path, "file note").expect("seed file");
        let file_id = note_id_for_markdown_file(&markdown_path);

        let listed = service
            .list_notes_meta(Some(&file_id))
            .expect("list should succeed");
        assert!(listed.iter().any(|entry| entry.id == "n1"));
        assert!(listed.iter().any(|entry| entry.id == "n2"));
        assert_eq!(
            listed.first().map(|entry| entry.id.as_str()),
            Some(file_id.as_str())
        );

        let _ = fs::remove_file(markdown_path);
        drop(db);
        cleanup_db_files(&db_path);
    }
}
