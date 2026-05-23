use crate::storage::{Db, Note, NoteAccessMode, NoteModules, NoteSummary};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use ulid::Ulid;

pub const MARKDOWN_NOTE_ID_PREFIX: &str = "mdfile:";
pub const DB_IMAGE_MARKDOWN_PREFIX: &str = "slate-image://";
const NOTE_TITLE_MAX_CHARS: usize = 60;
const DEFAULT_IMAGE_STEM: &str = "image";
const DEFAULT_IMAGE_EXTENSION: &str = "png";
const MAX_IMAGE_STEM_LEN: usize = 64;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedImage {
    pub image_id: String,
    pub markdown_path: String,
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

    pub fn resolve_wiki_link(&self, short_id: &str) -> Result<Option<NoteSummary>, String> {
        self.db.resolve_wiki_link(short_id)
    }

    pub fn resolve_wiki_links(
        &self,
        short_ids: &[String],
    ) -> Result<Vec<(String, Option<NoteSummary>)>, String> {
        self.db.resolve_wiki_links(short_ids)
    }

    pub fn resolve_wiki_link_note(&self, short_id: &str) -> Result<Option<Note>, String> {
        self.db.resolve_wiki_link_note(short_id)
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
        self.list_notes_meta_filtered(active_note_id, None)
    }

    pub fn list_notes_meta_filtered(
        &self,
        active_note_id: Option<&str>,
        collection_id: Option<&str>,
    ) -> Result<Vec<NoteSummary>, String> {
        let mut notes = self.db.list_notes_meta_filtered(collection_id)?;
        notes.retain(|note| matches!(note_identity_from_id(&note.id), NoteIdentity::DbNote(_)));
        if collection_id.is_none() {
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
        }
        Ok(notes)
    }

    pub fn import_image_bytes_by_id(
        &self,
        note_id: &str,
        file_name: Option<&str>,
        mime_type: Option<&str>,
        image_bytes: &[u8],
    ) -> Result<ImportedImage, String> {
        if image_bytes.is_empty() {
            return Err("Image payload is empty".to_string());
        }
        let identity = self.parse_identity(note_id);
        if let NoteIdentity::DbNote(id) = &identity {
            let image_id = self.db.reserve_note_image(id, file_name, mime_type)?;
            self.db
                .write_note_image_bytes(id, &image_id, file_name, mime_type, image_bytes)?;
            return Ok(ImportedImage {
                image_id: image_id.clone(),
                markdown_path: markdown_path_for_db_image(&image_id),
            });
        }

        let note = self
            .open_note(&identity)?
            .ok_or_else(|| "Note not found".to_string())?;
        if note.access_mode != NoteAccessMode::None && !note.is_unlocked {
            return Err("note is locked; unlock first".to_string());
        }

        let extension = select_image_extension(file_name, mime_type);
        let stem = select_image_stem(file_name);
        let (asset_dir, markdown_prefix) = image_asset_directory(self.parse_identity(note_id))?;
        fs::create_dir_all(&asset_dir).map_err(|e| {
            format!(
                "Failed to create image assets directory '{}': {e}",
                asset_dir.display()
            )
        })?;

        let target_path = unique_asset_file_path(&asset_dir, &stem, extension.as_str());
        fs::write(&target_path, image_bytes).map_err(|e| {
            format!(
                "Failed to write image asset '{}': {e}",
                target_path.display()
            )
        })?;

        let saved_name = target_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "Failed to determine saved image file name".to_string())?;
        Ok(ImportedImage {
            image_id: saved_name.to_string(),
            markdown_path: format!("{markdown_prefix}/{saved_name}"),
        })
    }

    pub fn import_image_path_by_id(
        &self,
        note_id: &str,
        source_path: &Path,
    ) -> Result<ImportedImage, String> {
        if !source_path.exists() {
            return Err(format!(
                "Image source path does not exist: {}",
                source_path.display()
            ));
        }
        if !source_path.is_file() {
            return Err(format!(
                "Image source path is not a file: {}",
                source_path.display()
            ));
        }
        let bytes = fs::read(source_path).map_err(|e| {
            format!(
                "Failed to read image source file '{}': {e}",
                source_path.display()
            )
        })?;
        let file_name = source_path.file_name().and_then(|name| name.to_str());
        self.import_image_bytes_by_id(note_id, file_name, None, &bytes)
    }

    pub fn reserve_image_placeholder_by_id(
        &self,
        note_id: &str,
        file_name: Option<&str>,
        mime_type: Option<&str>,
    ) -> Result<ImportedImage, String> {
        let identity = self.parse_identity(note_id);
        let NoteIdentity::DbNote(id) = identity else {
            return Err("image placeholders are only supported for database notes".to_string());
        };
        let image_id = self.db.reserve_note_image(&id, file_name, mime_type)?;
        Ok(ImportedImage {
            image_id: image_id.clone(),
            markdown_path: markdown_path_for_db_image(&image_id),
        })
    }

    pub fn write_image_bytes_to_placeholder_by_id(
        &self,
        note_id: &str,
        image_id: &str,
        file_name: Option<&str>,
        mime_type: Option<&str>,
        image_bytes: &[u8],
    ) -> Result<(), String> {
        let identity = self.parse_identity(note_id);
        let NoteIdentity::DbNote(id) = identity else {
            return Err("image placeholders are only supported for database notes".to_string());
        };
        self.db
            .write_note_image_bytes(&id, image_id, file_name, mime_type, image_bytes)
    }

    pub fn write_image_path_to_placeholder_by_id(
        &self,
        note_id: &str,
        image_id: &str,
        source_path: &Path,
    ) -> Result<(), String> {
        if !source_path.exists() {
            return Err(format!(
                "Image source path does not exist: {}",
                source_path.display()
            ));
        }
        if !source_path.is_file() {
            return Err(format!(
                "Image source path is not a file: {}",
                source_path.display()
            ));
        }
        let bytes = fs::read(source_path).map_err(|e| {
            format!(
                "Failed to read image source file '{}': {e}",
                source_path.display()
            )
        })?;
        let file_name = source_path.file_name().and_then(|name| name.to_str());
        self.write_image_bytes_to_placeholder_by_id(note_id, image_id, file_name, None, &bytes)
    }

    pub fn delete_image_placeholder_by_id(
        &self,
        note_id: &str,
        image_id: &str,
    ) -> Result<bool, String> {
        let identity = self.parse_identity(note_id);
        let NoteIdentity::DbNote(id) = identity else {
            return Ok(false);
        };
        self.db.delete_note_image(&id, image_id)
    }

    pub fn resolve_image_markdown_source_by_id(
        &self,
        note_id: &str,
        src: &str,
    ) -> Result<Option<String>, String> {
        let identity = self.parse_identity(note_id);
        if let NoteIdentity::DbNote(id) = &identity {
            if let Some(image_id) = parse_db_image_markdown_source(src) {
                return self.db.resolve_note_image_data_url(id, image_id);
            }
        }
        Ok(resolve_image_markdown_path(identity, src)?
            .map(|path| path.to_string_lossy().to_string()))
    }
}

fn markdown_path_for_db_image(image_id: &str) -> String {
    format!("{DB_IMAGE_MARKDOWN_PREFIX}{image_id}")
}

fn parse_db_image_markdown_source(src: &str) -> Option<&str> {
    let trimmed = src.trim();
    let value = trimmed.strip_prefix(DB_IMAGE_MARKDOWN_PREFIX)?;
    if value.is_empty() {
        return None;
    }
    let looks_like_ulid = value.len() == 26 && value.chars().all(|ch| ch.is_ascii_alphanumeric());
    if looks_like_ulid {
        Some(value)
    } else {
        None
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
    if !path.is_absolute() {
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

    if absolute.exists() && absolute.is_dir() {
        return Err(format!(
            "Cannot open directory '{}' as a text note",
            absolute.display()
        ));
    }

    if absolute.exists() {
        std::fs::canonicalize(&absolute)
            .map_err(|e| format!("Failed to canonicalize file '{}': {e}", absolute.display()))
    } else {
        Ok(absolute)
    }
}

pub fn syntax_language_for_path(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let lang = match ext.as_str() {
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "html" | "htm" | "xhtml" => "html",
        "xml" | "svg" => "xml",
        "css" | "scss" | "less" => "css",
        "js" | "mjs" | "cjs" | "jsx" | "javascript" => "js",
        "ts" | "mts" | "cts" | "tsx" | "typescript" => "ts",
        "rs" => "rust",
        "py" => "python",
        "sh" | "bash" | "zsh" | "fish" => "sh",
        "go" => "go",
        "java" => "java",
        "c" | "h" | "hpp" | "cpp" | "cc" | "cxx" => "c",
        "ini" | "cfg" | "conf" | "properties" => "toml",
        _ => return None,
    };
    Some(lang.to_string())
}

pub fn syntax_language_for_note_id(note_id: &str) -> Option<String> {
    let path = markdown_file_path_from_note_id(note_id)?;
    syntax_language_for_path(&path)
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

fn image_asset_directory(identity: NoteIdentity) -> Result<(PathBuf, String), String> {
    match identity {
        NoteIdentity::DbNote(id) => {
            let safe_id = sanitize_note_id_for_path(&id);
            let root = crate::data_dir()?.join("assets").join(&safe_id);
            Ok((root, format!("./assets/{safe_id}")))
        }
        NoteIdentity::FileNote(path) => {
            let parent = path.parent().ok_or_else(|| {
                format!(
                    "Failed to determine markdown parent directory for '{}'",
                    path.display()
                )
            })?;
            Ok((parent.join("assets"), "./assets".to_string()))
        }
    }
}

fn resolve_image_markdown_path(
    identity: NoteIdentity,
    src: &str,
) -> Result<Option<PathBuf>, String> {
    let raw = src.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let candidate = PathBuf::from(raw);
    if candidate.is_absolute() {
        if candidate.exists() && candidate.is_file() {
            return Ok(Some(candidate));
        }
        return Ok(None);
    }

    match identity {
        NoteIdentity::DbNote(_) => {
            let root = crate::data_dir()?;
            let relative = if let Some(rest) = raw.strip_prefix("./") {
                rest
            } else {
                raw
            };
            let joined = root.join(relative);
            if !joined.exists() || !joined.is_file() {
                return Ok(None);
            }
            let canonical = joined.canonicalize().map_err(|e| {
                format!(
                    "Failed to canonicalize image path '{}': {e}",
                    joined.display()
                )
            })?;
            let canonical_root = root.canonicalize().unwrap_or(root);
            if !canonical.starts_with(&canonical_root) {
                return Ok(None);
            }
            Ok(Some(canonical))
        }
        NoteIdentity::FileNote(path) => {
            let parent = path.parent().ok_or_else(|| {
                format!(
                    "Failed to determine markdown parent directory for '{}'",
                    path.display()
                )
            })?;
            let joined = parent.join(raw);
            if !joined.exists() || !joined.is_file() {
                return Ok(None);
            }
            let canonical = joined.canonicalize().map_err(|e| {
                format!(
                    "Failed to canonicalize image path '{}': {e}",
                    joined.display()
                )
            })?;
            Ok(Some(canonical))
        }
    }
}

fn sanitize_note_id_for_path(note_id: &str) -> String {
    let mut out = String::with_capacity(note_id.len());
    for ch in note_id.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "note".to_string()
    } else {
        trimmed.to_string()
    }
}

fn select_image_stem(file_name: Option<&str>) -> String {
    let stem = file_name
        .and_then(|raw| Path::new(raw).file_stem())
        .and_then(|value| value.to_str())
        .unwrap_or(DEFAULT_IMAGE_STEM);
    sanitize_image_stem(stem)
}

fn sanitize_image_stem(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(MAX_IMAGE_STEM_LEN));
    let mut prev_dash = false;
    for ch in raw.chars() {
        if out.len() >= MAX_IMAGE_STEM_LEN {
            break;
        }
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
            continue;
        }
        if ch == '-' || ch == ' ' || ch == '.' {
            if !prev_dash && !out.is_empty() {
                out.push('-');
                prev_dash = true;
            }
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        format!("{DEFAULT_IMAGE_STEM}-{}", Ulid::new())
    } else {
        trimmed.to_string()
    }
}

fn select_image_extension(file_name: Option<&str>, mime_type: Option<&str>) -> String {
    if let Some(name) = file_name {
        if let Some(ext) = Path::new(name).extension().and_then(|value| value.to_str()) {
            if let Some(allowed) = normalize_image_extension(ext) {
                return allowed.to_string();
            }
        }
    }
    if let Some(mime) = mime_type {
        if let Some(from_mime) = image_extension_from_mime(mime) {
            return from_mime.to_string();
        }
    }
    DEFAULT_IMAGE_EXTENSION.to_string()
}

fn normalize_image_extension(ext: &str) -> Option<&'static str> {
    let normalized = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    match normalized.as_str() {
        "png" => Some("png"),
        "jpg" | "jpeg" => Some("jpg"),
        "gif" => Some("gif"),
        "webp" => Some("webp"),
        "bmp" => Some("bmp"),
        "svg" | "svgz" => Some("svg"),
        "avif" => Some("avif"),
        _ => None,
    }
}

fn image_extension_from_mime(mime_type: &str) -> Option<&'static str> {
    let normalized = mime_type.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/bmp" => Some("bmp"),
        "image/svg+xml" => Some("svg"),
        "image/avif" => Some("avif"),
        _ => None,
    }
}

fn unique_asset_file_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut suffix = 1usize;
    loop {
        let file_name = if suffix == 1 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem}-{suffix}.{ext}")
        };
        let candidate = dir.join(file_name);
        if !candidate.exists() {
            return candidate;
        }
        suffix += 1;
    }
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

/// Derive a short display title from a note body. Picks the first non-empty
/// line, trims it, and truncates to `NOTE_TITLE_MAX_CHARS` chars (appending
/// "..." when truncation occurs). Returns "Untitled" for empty bodies.
///
/// This is the single source of truth for note titles in the workspace — the
/// SQLite layer and the wasm bridge both delegate here.
pub fn derive_note_title_from_body(body: &str) -> String {
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

fn modules_for_file_path(path: &Path) -> NoteModules {
    if is_supported_markdown_path(path) {
        return NoteModules::default();
    }
    NoteModules {
        math: false,
        table: false,
        variables: false,
        style: true,
        cross_note: false,
    }
}

fn note_from_markdown_path(path: &Path) -> Result<Note, String> {
    let body = read_markdown_file(path)?;
    let (created_at, updated_at) = markdown_file_timestamps(path)?;
    Ok(Note {
        id: note_id_for_markdown_file(path),
        body,
        modules: modules_for_file_path(path),
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
    fn file_source_non_markdown_notes_disable_calc_modules_and_report_syntax() {
        let db_path = temp_db_path();
        let db = Db::open(db_path.clone()).expect("db opens");
        let service = NoteSourceService::new(db.clone());
        let json_path = std::env::temp_dir().join(format!("note-source-file-{}.json", Ulid::new()));
        fs::write(&json_path, "{\n  \"a\": 1\n}\n").expect("seed file");
        let note_id = note_id_for_markdown_file(&json_path);

        let opened = service
            .open_note_by_id(&note_id)
            .expect("open file note")
            .expect("note exists");
        assert!(!opened.modules.math);
        assert!(!opened.modules.table);
        assert!(!opened.modules.variables);
        assert!(opened.modules.style);
        assert_eq!(
            syntax_language_for_note_id(&note_id).as_deref(),
            Some("json")
        );

        let _ = fs::remove_file(json_path);
        drop(db);
        cleanup_db_files(&db_path);
    }

    #[test]
    fn resolve_markdown_file_path_rejects_directories() {
        let dir = std::env::temp_dir().join(format!("note-source-dir-{}", Ulid::new()));
        fs::create_dir_all(&dir).expect("mkdir");
        let err = resolve_markdown_file_path(
            dir.to_string_lossy().as_ref(),
            std::env::current_dir().expect("cwd").as_path(),
        )
        .expect_err("directory should not resolve");
        assert!(err.contains("Cannot open directory"));
        let _ = fs::remove_dir_all(dir);
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

    #[test]
    fn image_import_helpers_normalize_stem_and_extension() {
        assert_eq!(
            sanitize_image_stem(" Plan v1.0 @ Draft "),
            "plan-v1-0-draft"
        );
        assert_eq!(
            select_image_extension(Some("pic.JPEG"), Some("image/png")),
            "jpg"
        );
        assert_eq!(
            select_image_extension(Some("pic.bad"), Some("image/webp")),
            "webp"
        );
        assert_eq!(select_image_extension(None, Some("image/unknown")), "png");
    }

    #[test]
    fn unique_asset_file_path_adds_numeric_suffixes() {
        let dir = std::env::temp_dir().join(format!("note-sources-image-path-{}", Ulid::new()));
        fs::create_dir_all(&dir).expect("mkdir");
        let first = unique_asset_file_path(&dir, "image", "png");
        assert_eq!(
            first.file_name().and_then(|v| v.to_str()),
            Some("image.png")
        );
        fs::write(&first, b"x").expect("seed");
        let second = unique_asset_file_path(&dir, "image", "png");
        assert_eq!(
            second.file_name().and_then(|v| v.to_str()),
            Some("image-2.png")
        );
        let _ = fs::remove_file(first);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn import_image_path_by_id_writes_markdown_assets_for_file_notes() {
        let db_path = temp_db_path();
        let db = Db::open(db_path.clone()).expect("db opens");
        let service = NoteSourceService::new(db.clone());

        let note_path = std::env::temp_dir().join(format!("note-source-image-{}.md", Ulid::new()));
        fs::write(&note_path, "hello").expect("seed markdown");
        let source_path =
            std::env::temp_dir().join(format!("note-source-image-src-{}.png", Ulid::new()));
        fs::write(&source_path, b"png-bytes").expect("seed image file");
        let note_id = note_id_for_markdown_file(&note_path);

        let imported = service
            .import_image_path_by_id(&note_id, &source_path)
            .expect("import image");
        assert!(imported.markdown_path.starts_with("./assets/"));
        let asset_file_name = imported
            .markdown_path
            .rsplit('/')
            .next()
            .expect("asset file name");
        let copied = note_path
            .parent()
            .expect("note parent")
            .join("assets")
            .join(asset_file_name);
        assert!(copied.exists());
        assert_eq!(fs::read(copied).expect("copied bytes"), b"png-bytes");

        let _ = fs::remove_file(source_path);
        let _ = fs::remove_file(note_path);
        drop(db);
        cleanup_db_files(&db_path);
    }

    #[test]
    fn resolve_image_markdown_source_by_id_supports_relative_paths_for_file_notes() {
        let db_path = temp_db_path();
        let db = Db::open(db_path.clone()).expect("db opens");
        let service = NoteSourceService::new(db.clone());

        let note_dir = std::env::temp_dir().join(format!("note-source-resolve-{}", Ulid::new()));
        fs::create_dir_all(&note_dir).expect("note dir");
        let note_path = note_dir.join("note.md");
        fs::write(&note_path, "note").expect("seed note");
        let assets_dir = note_dir.join("assets");
        fs::create_dir_all(&assets_dir).expect("assets");
        let image_path = assets_dir.join("a.png");
        fs::write(&image_path, b"img").expect("seed image");

        let note_id = note_id_for_markdown_file(&note_path);
        let resolved = service
            .resolve_image_markdown_source_by_id(&note_id, "./assets/a.png")
            .expect("resolve")
            .expect("exists");
        assert_eq!(
            resolved,
            image_path
                .canonicalize()
                .expect("canonical image path")
                .to_string_lossy()
                .to_string()
        );

        let _ = fs::remove_dir_all(note_dir);
        drop(db);
        cleanup_db_files(&db_path);
    }
}
