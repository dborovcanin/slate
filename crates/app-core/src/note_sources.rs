use crate::storage::{Db, Note, NoteAccessMode, NoteModules, NoteRevision, NoteSummary};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use image::{ImageFormat, ImageReader};
use std::fs::{self, OpenOptions};
use std::io::Cursor;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use ulid::Ulid;

pub const MARKDOWN_NOTE_ID_PREFIX: &str = "mdfile:";
pub const DB_IMAGE_MARKDOWN_PREFIX: &str = "slate-image://";
pub const MAX_NOTE_IMAGE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_NOTE_IMAGE_DIMENSION: u32 = 16_384;
pub const MAX_NOTE_IMAGE_PIXELS: u64 = 100_000_000;
const NOTE_TITLE_MAX_CHARS: usize = 60;
const DEFAULT_IMAGE_STEM: &str = "image";
const MAX_IMAGE_STEM_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteIdentity {
    DbNote(String),
    FileNote(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteSourceCapabilities {
    pub can_save: bool,
    pub can_delete: bool,
    pub can_encrypt: bool,
    pub can_module_persist: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedImage {
    pub image_id: String,
    pub markdown_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ValidatedImageMetadata {
    extension: &'static str,
    mime_type: &'static str,
}

#[derive(Debug, Clone, Default)]
pub struct SaveOptions {
    pub expected_revision: Option<String>,
    pub force: bool,
    /// For a database note, the reminders to store with the text in the same
    /// transaction (`None` leaves them as they are). File notes have none.
    pub reminders: Option<Vec<crate::storage::ReminderLine>>,
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
                can_encrypt: true,
                can_module_persist: true,
            },
            NoteIdentity::FileNote(_) => NoteSourceCapabilities {
                can_save: true,
                can_delete: false,
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

    #[cfg(test)]
    pub fn save_note_by_id(
        &self,
        note_id: &str,
        body: &str,
        options: SaveOptions,
    ) -> Result<Note, String> {
        self.save_note(&self.parse_identity(note_id), body, options)
    }

    pub fn save_note_revision_by_id(
        &self,
        note_id: &str,
        body: &str,
        options: SaveOptions,
    ) -> Result<NoteRevision, String> {
        self.save_note_revision(&self.parse_identity(note_id), body, options)
    }

    /// Same write and same conflict check as [`Self::save_note`], returning only
    /// the new revision. Neither backing store re-reads what it just wrote.
    pub fn save_note_revision(
        &self,
        identity: &NoteIdentity,
        body: &str,
        options: SaveOptions,
    ) -> Result<NoteRevision, String> {
        match identity {
            NoteIdentity::DbNote(id) => self.db.save_note_revision_if(
                id,
                body,
                db_expected_revision(&options),
                options.reminders.as_deref(),
            ),
            NoteIdentity::FileNote(path) => {
                save_markdown_file(path, body, &options)?;
                Ok(NoteRevision {
                    id: note_id_for_markdown_file(path),
                    updated_at: revision_from_markdown_path(path)?.unwrap_or_default(),
                })
            }
        }
    }

    pub fn save_note(
        &self,
        identity: &NoteIdentity,
        body: &str,
        options: SaveOptions,
    ) -> Result<Note, String> {
        match identity {
            NoteIdentity::DbNote(id) => {
                self.db
                    .save_note_if(id, body, db_expected_revision(&options))
            }
            NoteIdentity::FileNote(path) => {
                save_markdown_file(path, body, &options)?;
                Ok(note_from_markdown_path(path)?)
            }
        }
    }

    /// Pins a stored note's title, or unpins it with an empty title. The
    /// text is left alone; file-backed notes are titled by their first line.
    pub fn rename_note(&self, note_id: &str, title: &str) -> Result<(), String> {
        match self.parse_identity(note_id) {
            NoteIdentity::DbNote(id) => self.db.set_note_title(&id, title),
            NoteIdentity::FileNote(_) => {
                Err("file-backed notes are renamed in the editor".to_string())
            }
        }
    }

    #[cfg(test)]
    pub fn get_note_meta_by_id(&self, note_id: &str) -> Result<Option<NoteSummary>, String> {
        self.get_note_meta(&self.parse_identity(note_id))
    }

    pub fn get_note_meta(&self, identity: &NoteIdentity) -> Result<Option<NoteSummary>, String> {
        match identity {
            NoteIdentity::DbNote(id) => self.db.get_note_meta(id),
            NoteIdentity::FileNote(path) => Ok(Some(note_summary_from_markdown_path(path)?)),
        }
    }

    #[cfg(test)]
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
        let metadata = validate_note_image(file_name, mime_type, image_bytes)?;
        let identity = self.parse_identity(note_id);
        if let NoteIdentity::DbNote(id) = &identity {
            let image_id = self
                .db
                .reserve_note_image(id, file_name, Some(metadata.mime_type))?;
            self.db.write_note_image_bytes(
                id,
                &image_id,
                file_name,
                Some(metadata.mime_type),
                image_bytes,
            )?;
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

        let stem = select_image_stem(file_name);
        let (asset_dir, markdown_prefix) = image_asset_directory(self.parse_identity(note_id))?;
        fs::create_dir_all(&asset_dir).map_err(|e| {
            format!(
                "Failed to create image assets directory '{}': {e}",
                asset_dir.display()
            )
        })?;

        let target_path = unique_asset_file_path(&asset_dir, &stem, metadata.extension);
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
        let bytes = read_image_file_bounded(source_path)?;
        let file_name = source_path.file_name().and_then(|name| name.to_str());
        self.import_image_bytes_by_id(note_id, file_name, None, &bytes)
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

    /// Locates the image a markdown `src` refers to without reading it.
    /// Missing images and paths outside the note's allowed roots are `None`.
    pub fn locate_image_by_id(
        &self,
        note_id: &str,
        src: &str,
    ) -> Result<Option<NoteImageSource>, String> {
        let identity = self.parse_identity(note_id);
        if let NoteIdentity::DbNote(id) = &identity {
            if let Some(image_id) = parse_db_image_markdown_source(src) {
                return Ok(Some(NoteImageSource::Stored {
                    note_id: id.clone(),
                    image_id: image_id.to_string(),
                }));
            }
        }
        Ok(resolve_image_markdown_path(identity, src)?.map(NoteImageSource::File))
    }

    /// A value that changes when the image content is replaced. Reads only
    /// file metadata or the stored row's timestamp, never the image bytes.
    pub fn image_stamp(&self, source: &NoteImageSource) -> Result<Option<u64>, String> {
        match source {
            NoteImageSource::File(path) => {
                let Ok(metadata) = fs::metadata(path) else {
                    return Ok(None);
                };
                if !metadata.is_file() {
                    return Ok(None);
                }
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .unwrap_or_default();
                Ok(Some(hash_stamp((modified.as_nanos(), metadata.len()))))
            }
            NoteImageSource::Stored { note_id, image_id } => {
                Ok(self.db.note_image_stamp(note_id, image_id)?.map(hash_stamp))
            }
        }
    }

    /// Reads the image bytes, bounded by `MAX_NOTE_IMAGE_BYTES`.
    pub fn read_image(&self, source: &NoteImageSource) -> Result<Option<NoteImageBytes>, String> {
        match source {
            NoteImageSource::File(path) => {
                let bytes = read_image_file_bounded(path)?;
                let extension = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(str::to_ascii_lowercase)
                    .and_then(|ext| {
                        image_metadata_for_extension(if ext == "jpeg" { "jpg" } else { &ext })
                    })
                    .map_or("png", |metadata| metadata.extension);
                Ok(Some(NoteImageBytes { bytes, extension }))
            }
            NoteImageSource::Stored { note_id, image_id } => Ok(self
                .db
                .read_note_image_bytes(note_id, image_id)?
                .map(|(mime, bytes)| NoteImageBytes {
                    bytes,
                    extension: image_extension_for_mime(&mime),
                })),
        }
    }
}

/// Where a note image referenced from markdown lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteImageSource {
    /// A local file inside the note's allowed image root.
    File(PathBuf),
    /// An image stored in the database for a DB note.
    Stored { note_id: String, image_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteImageBytes {
    pub bytes: Vec<u8>,
    /// File extension matching the image type, e.g. `png`.
    pub extension: &'static str,
}

fn hash_stamp(value: impl std::hash::Hash) -> u64 {
    use std::hash::Hasher as _;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn image_extension_for_mime(mime: &str) -> &'static str {
    ["png", "jpg", "gif", "webp", "bmp"]
        .into_iter()
        .filter_map(image_metadata_for_extension)
        .find(|metadata| metadata.mime_type == mime)
        .map_or("png", |metadata| metadata.extension)
}

pub fn validate_note_image_payload_len(byte_len: usize) -> Result<(), String> {
    if byte_len == 0 {
        return Err("Image payload is empty".to_string());
    }
    if byte_len > MAX_NOTE_IMAGE_BYTES {
        return Err(format!(
            "Image payload exceeds the {} MiB limit",
            MAX_NOTE_IMAGE_BYTES / (1024 * 1024)
        ));
    }
    Ok(())
}

#[cfg(test)]
pub fn validate_declared_image_type(
    file_name: Option<&str>,
    mime_type: Option<&str>,
) -> Result<(), String> {
    declared_image_type(file_name, mime_type).map(|_| ())
}

fn validate_note_image(
    file_name: Option<&str>,
    mime_type: Option<&str>,
    image_bytes: &[u8],
) -> Result<ValidatedImageMetadata, String> {
    validate_note_image_payload_len(image_bytes.len())?;
    let (declared_extension, declared_mime) = declared_image_type(file_name, mime_type)?;
    if looks_like_svg(image_bytes) {
        return Err(
            "SVG images are not supported because active SVG content is unsafe".to_string(),
        );
    }

    let reader = ImageReader::new(Cursor::new(image_bytes))
        .with_guessed_format()
        .map_err(|error| format!("Failed to inspect image format: {error}"))?;
    let format = reader
        .format()
        .ok_or_else(|| "Unsupported or unrecognized image format".to_string())?;
    let actual = image_metadata_for_format(format)
        .ok_or_else(|| format!("Unsupported image format: {format:?}"))?;

    if let Some(extension) = declared_extension {
        if extension != actual.extension {
            return Err(format!(
                "Image extension does not match detected {} content",
                actual.extension
            ));
        }
    }
    if let Some(mime) = declared_mime {
        if mime != actual.mime_type {
            return Err(format!(
                "Image MIME type does not match detected {} content",
                actual.mime_type
            ));
        }
    }

    let (width, height) = reader
        .into_dimensions()
        .map_err(|error| format!("Failed to read image dimensions: {error}"))?;
    validate_image_dimensions(width, height)?;
    Ok(actual)
}

fn declared_image_type(
    file_name: Option<&str>,
    mime_type: Option<&str>,
) -> Result<(Option<&'static str>, Option<&'static str>), String> {
    let extension = file_name
        .and_then(|name| Path::new(name).extension())
        .and_then(|value| value.to_str())
        .map(declared_image_extension)
        .transpose()?
        .flatten();
    let mime = mime_type
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(declared_image_mime)
        .transpose()?
        .flatten();

    if let (Some(extension), Some(mime)) = (extension, mime) {
        let expected = image_metadata_for_extension(extension)
            .expect("validated extension must have metadata");
        if expected.mime_type != mime {
            return Err("Image extension and MIME type do not match".to_string());
        }
    }
    Ok((extension, mime))
}

fn declared_image_extension(raw: &str) -> Result<Option<&'static str>, String> {
    let normalized = raw.trim().trim_start_matches('.').to_ascii_lowercase();
    match normalized.as_str() {
        "png" => Ok(Some("png")),
        "jpg" | "jpeg" => Ok(Some("jpg")),
        "gif" => Ok(Some("gif")),
        "webp" => Ok(Some("webp")),
        "bmp" => Ok(Some("bmp")),
        "avif" => Err("AVIF image imports are not supported".to_string()),
        "svg" | "svgz" => {
            Err("SVG images are not supported because active SVG content is unsafe".to_string())
        }
        "" => Ok(None),
        _ => Err(format!("Unsupported image extension: .{normalized}")),
    }
}

fn declared_image_mime(raw: &str) -> Result<Option<&'static str>, String> {
    let normalized = raw
        .split(';')
        .next()
        .unwrap_or(raw)
        .trim()
        .to_ascii_lowercase();
    match normalized.as_str() {
        "image/png" | "image/x-png" => Ok(Some("image/png")),
        "image/jpeg" | "image/jpg" | "image/pjpeg" => Ok(Some("image/jpeg")),
        "image/gif" => Ok(Some("image/gif")),
        "image/webp" => Ok(Some("image/webp")),
        "image/bmp" | "image/x-ms-bmp" => Ok(Some("image/bmp")),
        "image/avif" => Err("AVIF image imports are not supported".to_string()),
        "application/octet-stream" | "binary/octet-stream" => Ok(None),
        "image/svg+xml" => {
            Err("SVG images are not supported because active SVG content is unsafe".to_string())
        }
        "" => Ok(None),
        _ => Err(format!("Unsupported image MIME type: {normalized}")),
    }
}

fn image_metadata_for_extension(extension: &str) -> Option<ValidatedImageMetadata> {
    match extension {
        "png" => Some(ValidatedImageMetadata {
            extension: "png",
            mime_type: "image/png",
        }),
        "jpg" => Some(ValidatedImageMetadata {
            extension: "jpg",
            mime_type: "image/jpeg",
        }),
        "gif" => Some(ValidatedImageMetadata {
            extension: "gif",
            mime_type: "image/gif",
        }),
        "webp" => Some(ValidatedImageMetadata {
            extension: "webp",
            mime_type: "image/webp",
        }),
        "bmp" => Some(ValidatedImageMetadata {
            extension: "bmp",
            mime_type: "image/bmp",
        }),
        _ => None,
    }
}

fn image_metadata_for_format(format: ImageFormat) -> Option<ValidatedImageMetadata> {
    let extension = match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Gif => "gif",
        ImageFormat::WebP => "webp",
        ImageFormat::Bmp => "bmp",
        _ => return None,
    };
    image_metadata_for_extension(extension)
}

fn validate_image_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("Image dimensions must be greater than zero".to_string());
    }
    if width > MAX_NOTE_IMAGE_DIMENSION || height > MAX_NOTE_IMAGE_DIMENSION {
        return Err(format!(
            "Image dimensions exceed the {} pixel side limit",
            MAX_NOTE_IMAGE_DIMENSION
        ));
    }
    if u64::from(width).saturating_mul(u64::from(height)) > MAX_NOTE_IMAGE_PIXELS {
        return Err(format!(
            "Image dimensions exceed the {} megapixel limit",
            MAX_NOTE_IMAGE_PIXELS / 1_000_000
        ));
    }
    Ok(())
}

fn looks_like_svg(image_bytes: &[u8]) -> bool {
    let prefix_len = image_bytes.len().min(4096);
    let prefix = String::from_utf8_lossy(&image_bytes[..prefix_len]).to_ascii_lowercase();
    let trimmed = prefix.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
    trimmed.starts_with("<svg")
        || ((trimmed.starts_with("<?xml") || trimmed.starts_with("<!--"))
            && trimmed.contains("<svg"))
}

fn read_image_file_bounded(source_path: &Path) -> Result<Vec<u8>, String> {
    let metadata = fs::metadata(source_path).map_err(|e| {
        format!(
            "Failed to inspect image source file '{}': {e}",
            source_path.display()
        )
    })?;
    validate_note_image_payload_len(usize::try_from(metadata.len()).unwrap_or(usize::MAX))?;

    let file = fs::File::open(source_path).map_err(|e| {
        format!(
            "Failed to read image source file '{}': {e}",
            source_path.display()
        )
    })?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_NOTE_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| {
            format!(
                "Failed to read image source file '{}': {e}",
                source_path.display()
            )
        })?;
    validate_note_image_payload_len(bytes.len())?;
    Ok(bytes)
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

#[cfg(test)]
pub fn syntax_language_for_note_id(note_id: &str) -> Option<String> {
    let path = markdown_file_path_from_note_id(note_id)?;
    syntax_language_for_path(&path)
}

/// The revision a database save must still find, or `None` to write anyway.
fn db_expected_revision(options: &SaveOptions) -> Option<&str> {
    if options.force {
        None
    } else {
        options.expected_revision.as_deref()
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

fn image_asset_directory(identity: NoteIdentity) -> Result<(PathBuf, String), String> {
    match identity {
        NoteIdentity::DbNote(id) => {
            let safe_id = sanitize_note_id_for_path(&id);
            let root = crate::data_dir()?.join("assets").join(&safe_id);
            Ok((root, format!("./assets/{safe_id}")))
        }
        NoteIdentity::FileNote(path) => Ok((file_note_asset_root(&path)?, "./assets".to_string())),
    }
}

pub fn file_note_asset_root(path: &Path) -> Result<PathBuf, String> {
    let parent = path.parent().ok_or_else(|| {
        format!(
            "Failed to determine markdown parent directory for '{}'",
            path.display()
        )
    })?;
    Ok(parent.join("assets"))
}

fn resolve_image_markdown_path(
    identity: NoteIdentity,
    src: &str,
) -> Result<Option<PathBuf>, String> {
    let raw = src.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    match identity {
        NoteIdentity::DbNote(_) => {
            let root = crate::data_dir()?;
            let candidate = PathBuf::from(raw);
            let joined = if candidate.is_absolute() {
                candidate
            } else {
                let relative = if let Some(rest) = raw.strip_prefix("./") {
                    rest
                } else {
                    raw
                };
                root.join(relative)
            };
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
            let candidate = PathBuf::from(raw);
            let joined = if candidate.is_absolute() {
                candidate
            } else {
                parent.join(candidate)
            };
            if !joined.exists() || !joined.is_file() {
                return Ok(None);
            }
            let canonical = joined.canonicalize().map_err(|e| {
                format!(
                    "Failed to canonicalize image path '{}': {e}",
                    joined.display()
                )
            })?;
            let asset_root = file_note_asset_root(&path)?;
            let canonical_asset_root = asset_root.canonicalize().unwrap_or(asset_root);
            if !canonical.starts_with(&canonical_asset_root) {
                return Ok(None);
            }
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

/// Writes a file note unless it changed on disk since `options`' revision.
/// The revision is checked again just before the new file replaces the old
/// one, leaving a concurrent writer only the rename itself to slip into;
/// plain files offer no transaction to close that fully.
fn save_markdown_file(path: &Path, body: &str, options: &SaveOptions) -> Result<(), String> {
    let check = || {
        ensure_revision_matches(
            revision_from_markdown_path(path)?.as_deref(),
            options.expected_revision.as_deref(),
            options.force,
            "file changed on disk",
        )
    };
    check()?;
    write_markdown_file_atomically(path, body, &check)
}

/// Writes `body` to a temporary file and renames it over `path`, after
/// `before_replace` agrees.
fn write_markdown_file_atomically(
    path: &Path,
    body: &str,
    before_replace: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    // Replace a symlink's target rather than the link itself.
    let resolved;
    let path = if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        resolved.as_path()
    } else {
        path
    };
    // The replacement keeps the original's mode (private notes, executable scripts).
    let permissions = fs::metadata(path).ok().map(|meta| meta.permissions());
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
        if let Some(permissions) = permissions {
            tmp.set_permissions(permissions).map_err(|e| {
                format!(
                    "Failed to set permissions on temporary markdown file '{}': {e}",
                    tmp_path.display()
                )
            })?;
        }
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
        before_replace()?;
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
/// This is the single source of truth for note titles in the workspace.
/// Title text of a line: trimmed, with a Markdown ATX heading marker
/// (`# `..`###### ` and optional closing `#`s) removed. `#tag` and
/// `#include` are left alone because no space follows the `#`.
pub fn title_text_for_line(line: &str) -> &str {
    let trimmed = line.trim();
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&hashes) {
        return trimmed;
    }
    let rest = &trimmed[hashes..];
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return trimmed;
    }
    let content = rest.trim();
    let without_closing = content.trim_end_matches('#');
    if without_closing.len() != content.len() && without_closing.ends_with(char::is_whitespace) {
        without_closing.trim_end()
    } else {
        content
    }
}

pub fn derive_note_title_from_body(body: &str) -> String {
    for line in body.lines() {
        let trimmed = title_text_for_line(line);
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
        pinned_title: None,
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

    #[test]
    fn title_text_strips_atx_heading_markers_only() {
        assert_eq!(title_text_for_line("# 2026-09-26"), "2026-09-26");
        assert_eq!(title_text_for_line("  ### Plan ###  "), "Plan");
        assert_eq!(title_text_for_line("## C#"), "C#");
        assert_eq!(title_text_for_line("#tag and text"), "#tag and text");
        assert_eq!(title_text_for_line("####### seven"), "####### seven");
        assert_eq!(title_text_for_line("#"), "");
        assert_eq!(title_text_for_line("plain"), "plain");
    }

    #[test]
    fn derived_title_skips_empty_headings_and_strips_markers() {
        assert_eq!(
            derive_note_title_from_body("#\n\n# Weekly review\nbody"),
            "Weekly review"
        );
        assert_eq!(derive_note_title_from_body("First line"), "First line");
        assert_eq!(derive_note_title_from_body("\n  \n"), "Untitled");
    }
    use image::codecs::png::PngEncoder;
    use image::{ColorType, ImageEncoder};

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

    fn tiny_png_bytes() -> Vec<u8> {
        let mut out = Vec::new();
        PngEncoder::new(&mut out)
            .write_image(&[255, 0, 0], 1, 1, ColorType::Rgb8.into())
            .expect("png encode");
        out
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
                    reminders: None,
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
                    reminders: None,
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
        assert!(!caps.can_encrypt);
        assert!(!caps.can_module_persist);

        let _ = fs::remove_file(markdown_path);
        drop(db);
        cleanup_db_files(&db_path);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_file_write_keeps_mode_and_symlink() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let dir = std::env::temp_dir().join(format!("note-source-mode-{}", Ulid::new()));
        fs::create_dir_all(&dir).expect("temp dir");
        for mode in [0o600, 0o755] {
            let path = dir.join(format!("note-{mode:o}.md"));
            fs::write(&path, "old").expect("seed file");
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("chmod");
            write_markdown_file_atomically(&path, "new", &|| Ok(())).expect("write");
            let meta = fs::metadata(&path).expect("metadata");
            assert_eq!(meta.permissions().mode() & 0o777, mode);
            assert_eq!(fs::read_to_string(&path).expect("read"), "new");
        }

        let target = dir.join("target.md");
        let link = dir.join("link.md");
        fs::write(&target, "old").expect("seed target");
        symlink(&target, &link).expect("symlink");
        write_markdown_file_atomically(&link, "through link", &|| Ok(())).expect("write");
        assert!(fs::symlink_metadata(&link)
            .expect("link metadata")
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(&target).expect("read"), "through link");

        let _ = fs::remove_dir_all(&dir);
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
    fn concurrent_saves_with_the_same_revision_cannot_both_win() {
        let db_path = temp_db_path();
        let db = Db::open(db_path.clone()).expect("db opens");
        let service = NoteSourceService::new(db.clone());
        service
            .save_note_by_id("n1", "start", SaveOptions::default())
            .expect("seed");
        for round in 0..40 {
            let expected = service
                .get_note_revision_by_id("n1")
                .expect("revision")
                .expect("exists");
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let writers: Vec<_> = (0..2)
                .map(|writer| {
                    let service = NoteSourceService::new(db.clone());
                    let expected = expected.clone();
                    let barrier = std::sync::Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        barrier.wait();
                        service.save_note_revision_by_id(
                            "n1",
                            &format!("round {round} writer {writer}"),
                            SaveOptions {
                                expected_revision: Some(expected),
                                force: false,
                                reminders: None,
                            },
                        )
                    })
                })
                .collect();
            let wins = writers
                .into_iter()
                .map(|writer| writer.join().expect("writer thread"))
                .filter(Result::is_ok)
                .count();
            assert_eq!(wins, 1, "round {round}");
        }
        drop(service);
        drop(db);
        cleanup_db_files(&db_path);
    }

    #[test]
    fn file_write_is_abandoned_when_the_last_check_fails() {
        let dir = std::env::temp_dir().join(format!("note-source-check-{}", Ulid::new()));
        fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("note.md");
        fs::write(&path, "theirs").expect("seed");
        let error = write_markdown_file_atomically(&path, "mine", &|| {
            Err("file changed on disk".to_string())
        })
        .expect_err("refused");
        assert!(error.contains("changed on disk"));
        assert_eq!(fs::read_to_string(&path).expect("read"), "theirs");
        let leftovers = fs::read_dir(&dir).expect("dir").count();
        assert_eq!(leftovers, 1, "the temporary file is removed");
        let _ = fs::remove_dir_all(&dir);
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
                    reminders: None,
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
                    reminders: None,
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
                    reminders: None,
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
                    reminders: None,
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
    fn image_import_helpers_normalize_stem_and_validate_claims() {
        assert_eq!(
            sanitize_image_stem(" Plan v1.0 @ Draft "),
            "plan-v1-0-draft"
        );
        assert!(validate_declared_image_type(Some("pic.JPEG"), Some("image/jpeg")).is_ok());
        assert!(validate_declared_image_type(Some("pic.png"), Some("image/jpeg")).is_err());
        assert!(validate_declared_image_type(Some("pic.bad"), Some("image/png")).is_err());
        assert!(validate_declared_image_type(Some("pic.svg"), Some("image/svg+xml")).is_err());
        assert!(validate_declared_image_type(Some("pic.avif"), Some("image/avif")).is_err());
    }

    #[test]
    fn image_validation_accepts_matching_raster_metadata() {
        let png = tiny_png_bytes();
        let metadata =
            validate_note_image(Some("pic.png"), Some("image/png; charset=binary"), &png)
                .expect("valid png");
        assert_eq!(metadata.extension, "png");
        assert_eq!(metadata.mime_type, "image/png");
    }

    #[test]
    fn image_validation_rejects_mismatched_or_unrecognized_content() {
        let png = tiny_png_bytes();
        let extension_error = validate_note_image(Some("pic.jpg"), None, &png)
            .expect_err("extension mismatch should fail");
        assert!(extension_error.contains("extension does not match"));

        let mime_error = validate_note_image(Some("pic.png"), Some("image/jpeg"), &png)
            .expect_err("mime mismatch should fail");
        assert!(mime_error.contains("extension and MIME type do not match"));

        let unknown = validate_note_image(None, None, b"not-an-image")
            .expect_err("unrecognized bytes should fail");
        assert!(unknown.contains("Unsupported or unrecognized"));
    }

    #[test]
    fn image_validation_rejects_svg_content_and_oversized_dimensions() {
        let svg = b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"></svg>";
        let svg_error = validate_note_image(None, None, svg).expect_err("svg should fail");
        assert!(svg_error.contains("SVG images are not supported"));

        assert!(validate_image_dimensions(MAX_NOTE_IMAGE_DIMENSION, 1).is_ok());
        assert!(validate_image_dimensions(MAX_NOTE_IMAGE_DIMENSION + 1, 1).is_err());
        assert!(validate_image_dimensions(10_001, 10_000).is_err());
    }

    #[test]
    fn image_payload_size_limit_is_enforced_without_allocating_payload() {
        assert!(validate_note_image_payload_len(1).is_ok());
        assert!(validate_note_image_payload_len(MAX_NOTE_IMAGE_BYTES).is_ok());
        let error = validate_note_image_payload_len(MAX_NOTE_IMAGE_BYTES + 1)
            .expect_err("oversized image should fail");
        assert!(error.contains("16 MiB"));
    }

    #[test]
    fn bounded_image_file_read_rejects_oversized_sparse_file() {
        let path = std::env::temp_dir().join(format!("note-source-oversized-{}.png", Ulid::new()));
        let file = fs::File::create(&path).expect("create sparse image");
        file.set_len((MAX_NOTE_IMAGE_BYTES + 1) as u64)
            .expect("size sparse image");
        drop(file);

        let error = read_image_file_bounded(&path).expect_err("oversized file should fail");
        assert!(error.contains("16 MiB"));

        let _ = fs::remove_file(path);
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
        let png = tiny_png_bytes();
        fs::write(&source_path, &png).expect("seed image file");
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
        assert_eq!(fs::read(copied).expect("copied bytes"), png);

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

    #[test]
    fn resolve_image_markdown_source_rejects_paths_outside_file_note_assets() {
        let db_path = temp_db_path();
        let db = Db::open(db_path.clone()).expect("db opens");
        let service = NoteSourceService::new(db.clone());

        let note_dir = std::env::temp_dir().join(format!("note-source-scope-{}", Ulid::new()));
        fs::create_dir_all(&note_dir).expect("note dir");
        let note_path = note_dir.join("note.md");
        let outside_image = note_dir.join("outside.png");
        fs::write(&note_path, "note").expect("seed note");
        fs::write(&outside_image, b"img").expect("seed outside image");

        let note_id = note_id_for_markdown_file(&note_path);
        let resolved = service
            .resolve_image_markdown_source_by_id(&note_id, "./outside.png")
            .expect("resolve should not error");
        assert!(resolved.is_none());

        // Directory traversal using parent segments '../'
        let traversal_resolved = service
            .resolve_image_markdown_source_by_id(&note_id, "./assets/../outside.png")
            .expect("resolve traversal should not error");
        assert!(traversal_resolved.is_none());

        // Deep traversal
        let deep_traversal = service
            .resolve_image_markdown_source_by_id(&note_id, "../../../../etc/passwd")
            .expect("resolve deep traversal should not error");
        assert!(deep_traversal.is_none());

        // Absolute path outside assets
        let abs_resolved = service
            .resolve_image_markdown_source_by_id(&note_id, outside_image.to_str().unwrap())
            .expect("resolve absolute outside path should not error");
        assert!(abs_resolved.is_none());

        let _ = fs::remove_dir_all(note_dir);
        drop(db);
        cleanup_db_files(&db_path);
    }

    #[test]
    fn stored_image_is_located_stamped_and_read_without_data_urls() {
        let db_path = temp_db_path();
        let db = Db::open(db_path.clone()).expect("db opens");
        db.save_note("n1", "note").expect("seed note");
        let service = NoteSourceService::new(db.clone());
        let imported = service
            .import_image_bytes_by_id("n1", Some("pic.png"), Some("image/png"), &tiny_png_bytes())
            .expect("import image");

        let source = service
            .locate_image_by_id("n1", &imported.markdown_path)
            .expect("locate")
            .expect("stored image");
        assert!(matches!(source, NoteImageSource::Stored { .. }));
        let first_stamp = service.image_stamp(&source).expect("stamp").expect("ready");
        assert_eq!(
            service.image_stamp(&source).expect("stamp"),
            Some(first_stamp)
        );
        let read = service.read_image(&source).expect("read").expect("bytes");
        assert_eq!(read.bytes, tiny_png_bytes());
        assert_eq!(read.extension, "png");

        let mut replacement = Vec::new();
        PngEncoder::new(&mut replacement)
            .write_image(&[0, 0, 255, 0, 0, 255], 2, 1, ColorType::Rgb8.into())
            .expect("png encode");
        db.write_note_image_bytes(
            "n1",
            &imported.image_id,
            Some("pic.png"),
            Some("image/png"),
            &replacement,
        )
        .expect("replace image");
        assert_ne!(
            service.image_stamp(&source).expect("stamp"),
            Some(first_stamp)
        );

        drop(db);
        cleanup_db_files(&db_path);
    }
}
