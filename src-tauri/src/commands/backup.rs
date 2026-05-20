use app_core::storage::Db;
#[cfg(feature = "gui")]
use app_core::AppCore;
use serde::Serialize;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
#[cfg(feature = "gui")]
use tauri::State;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use ulid::Ulid;
use url::Url;

const STAGED_RESTORE_FILENAME: &str = "notes.db.staged-restore";

#[derive(Debug, Clone, Serialize)]
pub struct BackupResult {
    pub path: String,
    pub bytes: u64,
}

enum ZipEntrySource {
    Bytes(Vec<u8>),
    File(PathBuf),
}

struct ZipEntry {
    name: &'static str,
    source: ZipEntrySource,
}

#[cfg(feature = "gui")]
#[tauri::command]
pub async fn backup_notes_database(
    core: State<'_, AppCore>,
    path: String,
) -> Result<BackupResult, String> {
    let db = core.db().clone();
    tauri::async_runtime::spawn_blocking(move || backup_notes_database_blocking(&db, &path))
        .await
        .map_err(|e| format!("backup task join failed: {e}"))?
}

pub fn backup_notes_database_blocking(db: &Db, path: &str) -> Result<BackupResult, String> {
    let resolved = resolve_backup_path(path)?;
    let parent = resolved
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let temp_db = parent.join(format!(".slate-backup-{}.db", Ulid::new()));
    let temp_zip = parent.join(format!(".slate-backup-{}.zip.tmp", Ulid::new()));
    let temp_old = parent.join(format!(".slate-backup-old-{}.zip.tmp", Ulid::new()));

    let result = (|| {
        db.backup_to_sqlite_file(&temp_db)?;
        let created_at = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|e| format!("Failed to format backup timestamp: {e}"))?;
        let manifest = serde_json::json!({
            "format": "slate-notes-sqlite-backup",
            "version": 1,
            "created_at": created_at,
            "database": "notes.db",
        });
        let manifest = serde_json::to_vec_pretty(&manifest)
            .map_err(|e| format!("Failed to build backup manifest: {e}"))?;
        let readme = format!(
            "Slate notes database backup\n\nCreated: {created_at}\n\nThis zip contains notes.db, a complete SQLite snapshot of Slate's notes database.\nTo restore manually, close Slate, unzip this file, and replace the active Slate notes.db with the extracted notes.db.\n"
        );

        write_zip_file(
            &temp_zip,
            &[
                ZipEntry {
                    name: "notes.db",
                    source: ZipEntrySource::File(temp_db.clone()),
                },
                ZipEntry {
                    name: "manifest.json",
                    source: ZipEntrySource::Bytes(manifest),
                },
                ZipEntry {
                    name: "README.txt",
                    source: ZipEntrySource::Bytes(readme.into_bytes()),
                },
            ],
        )?;

        // Atomic overwrite: stage the existing backup aside before replacing it so a
        // failed rename cannot leave the user with no backup at all.
        if resolved.exists() {
            fs::rename(&resolved, &temp_old).map_err(|e| {
                format!(
                    "Failed to stage existing backup '{}': {e}",
                    resolved.display()
                )
            })?;
        }
        if let Err(e) = fs::rename(&temp_zip, &resolved) {
            let _ = fs::rename(&temp_old, &resolved);
            return Err(format!(
                "Failed to move backup '{}' to '{}': {e}",
                temp_zip.display(),
                resolved.display()
            ));
        }
        let bytes = fs::metadata(&resolved)
            .map_err(|e| format!("Failed to inspect backup '{}': {e}", resolved.display()))?
            .len();
        Ok(BackupResult {
            path: resolved.display().to_string(),
            bytes,
        })
    })();

    let _ = fs::remove_file(&temp_db);
    let _ = fs::remove_file(&temp_zip);
    let _ = fs::remove_file(&temp_old);
    result
}

fn resolve_backup_path(path: &str) -> Result<PathBuf, String> {
    let raw = path.trim().trim_matches(|c| c == '"' || c == '\'');
    if raw.is_empty() {
        return Err("backup path is empty".to_string());
    }

    let resolved = if raw == "~" || raw.starts_with("~/") || raw.starts_with("~\\") {
        let home = resolve_home_dir().ok_or_else(|| {
            "home directory is not set; cannot expand '~' in backup path".to_string()
        })?;
        if raw == "~" {
            home
        } else {
            home.join(&raw[2..])
        }
    } else if raw.len() >= "file://".len() && raw[.."file://".len()].eq_ignore_ascii_case("file://")
    {
        let uri =
            Url::parse(raw).map_err(|e| format!("invalid file URL backup path '{raw}': {e}"))?;
        uri.to_file_path()
            .map_err(|_| format!("invalid file URL backup path '{raw}'"))?
    } else {
        PathBuf::from(raw)
    };

    if resolved.is_dir() {
        return Err(format!(
            "backup path points to a directory: {}",
            resolved.display()
        ));
    }
    if !resolved
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("zip"))
    {
        return Err(format!(
            "backup path must include a .zip filename: {}",
            resolved.display()
        ));
    }

    let parent = resolved
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        return Err(format!(
            "backup directory does not exist: {}",
            parent.display()
        ));
    }
    if !parent.is_dir() {
        return Err(format!(
            "backup parent is not a directory: {}",
            parent.display()
        ));
    }

    Ok(resolved)
}

fn resolve_home_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|v| !v.is_empty());
    if home.is_some() {
        return home.map(PathBuf::from);
    }
    let profile = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty());
    if profile.is_some() {
        return profile.map(PathBuf::from);
    }
    let drive = std::env::var_os("HOMEDRIVE").filter(|v| !v.is_empty());
    let path = std::env::var_os("HOMEPATH").filter(|v| !v.is_empty());
    match (drive, path) {
        (Some(drive), Some(path)) => {
            let mut full = PathBuf::from(drive);
            full.push(path);
            Some(full)
        }
        _ => None,
    }
}

fn write_zip_file(path: &Path, entries: &[ZipEntry]) -> Result<(), String> {
    let file = File::create(path)
        .map_err(|e| format!("Failed to create backup zip '{}': {e}", path.display()))?;
    let mut writer = BufWriter::new(file);
    let mut central_directory = Vec::new();
    let mut offset = 0u32;

    for entry in entries {
        let name = entry.name.as_bytes();
        if name.len() > u16::MAX as usize {
            return Err(format!("zip entry name is too long: {}", entry.name));
        }
        let (crc, size) = entry_crc32_and_size(entry)?;
        let local_offset = offset;

        write_u32(&mut writer, 0x0403_4b50)?;
        write_u16(&mut writer, 20)?;
        write_u16(&mut writer, 0)?;
        write_u16(&mut writer, 0)?;
        write_u16(&mut writer, 0)?;
        write_u16(&mut writer, 0)?;
        write_u32(&mut writer, crc)?;
        write_u32(&mut writer, size)?;
        write_u32(&mut writer, size)?;
        write_u16(&mut writer, name.len() as u16)?;
        write_u16(&mut writer, 0)?;
        writer
            .write_all(name)
            .map_err(|e| format!("Failed to write zip entry '{}': {e}", entry.name))?;
        write_entry_data(&mut writer, entry)?;

        offset = offset
            .checked_add(30)
            .and_then(|v| v.checked_add(name.len() as u32))
            .and_then(|v| v.checked_add(size))
            .ok_or_else(|| "backup zip is too large for zip32".to_string())?;

        write_u32(&mut central_directory, 0x0201_4b50)?;
        write_u16(&mut central_directory, 20)?;
        write_u16(&mut central_directory, 20)?;
        write_u16(&mut central_directory, 0)?;
        write_u16(&mut central_directory, 0)?;
        write_u16(&mut central_directory, 0)?;
        write_u16(&mut central_directory, 0)?;
        write_u32(&mut central_directory, crc)?;
        write_u32(&mut central_directory, size)?;
        write_u32(&mut central_directory, size)?;
        write_u16(&mut central_directory, name.len() as u16)?;
        write_u16(&mut central_directory, 0)?;
        write_u16(&mut central_directory, 0)?;
        write_u16(&mut central_directory, 0)?;
        write_u16(&mut central_directory, 0)?;
        write_u32(&mut central_directory, 0)?;
        write_u32(&mut central_directory, local_offset)?;
        central_directory.extend_from_slice(name);
    }

    let central_offset = offset;
    let central_size = u32::try_from(central_directory.len())
        .map_err(|_| "backup zip central directory is too large".to_string())?;
    writer
        .write_all(&central_directory)
        .map_err(|e| format!("Failed to write backup zip central directory: {e}"))?;
    write_u32(&mut writer, 0x0605_4b50)?;
    write_u16(&mut writer, 0)?;
    write_u16(&mut writer, 0)?;
    write_u16(&mut writer, entries.len() as u16)?;
    write_u16(&mut writer, entries.len() as u16)?;
    write_u32(&mut writer, central_size)?;
    write_u32(&mut writer, central_offset)?;
    write_u16(&mut writer, 0)?;
    writer
        .flush()
        .map_err(|e| format!("Failed to flush backup zip '{}': {e}", path.display()))?;
    Ok(())
}

// ZIP32 local headers require CRC and uncompressed size before the data, so file
// entries need a first pass to compute them before the second (write) pass.
fn entry_crc32_and_size(entry: &ZipEntry) -> Result<(u32, u32), String> {
    match &entry.source {
        ZipEntrySource::Bytes(b) => {
            let size = u32::try_from(b.len())
                .map_err(|_| format!("zip entry is too large for zip32: {}", entry.name))?;
            Ok((crc32(b), size))
        }
        ZipEntrySource::File(p) => stream_crc32_and_size(p, entry.name),
    }
}

fn stream_crc32_and_size(path: &Path, entry_name: &str) -> Result<(u32, u32), String> {
    let file = File::open(path).map_err(|e| format!("Failed to read '{}': {e}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut crc = 0xffff_ffffu32;
    let mut size = 0u64;
    let mut buf = [0u8; 65536];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("Failed to read '{}': {e}", path.display()))?;
        if n == 0 {
            break;
        }
        for &byte in &buf[..n] {
            crc ^= byte as u32;
            for _ in 0..8 {
                let mask = 0u32.wrapping_sub(crc & 1);
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        size += n as u64;
    }
    let size = u32::try_from(size)
        .map_err(|_| format!("zip entry is too large for zip32: {entry_name}"))?;
    Ok((!crc, size))
}

fn write_entry_data<W: Write>(writer: &mut W, entry: &ZipEntry) -> Result<(), String> {
    match &entry.source {
        ZipEntrySource::Bytes(b) => writer
            .write_all(b)
            .map_err(|e| format!("Failed to write zip entry '{}': {e}", entry.name)),
        ZipEntrySource::File(p) => {
            let file =
                File::open(p).map_err(|e| format!("Failed to read '{}': {e}", p.display()))?;
            let mut reader = BufReader::new(file);
            io::copy(&mut reader, writer)
                .map(|_| ())
                .map_err(|e| format!("Failed to write zip entry '{}': {e}", entry.name))
        }
    }
}

fn write_u16<W: Write>(writer: &mut W, value: u16) -> Result<(), String> {
    writer
        .write_all(&value.to_le_bytes())
        .map_err(|e| format!("Failed to write zip data: {e}"))
}

fn write_u32<W: Write>(writer: &mut W, value: u32) -> Result<(), String> {
    writer
        .write_all(&value.to_le_bytes())
        .map_err(|e| format!("Failed to write zip data: {e}"))
}

/// Extract `notes.db` from a Slate backup ZIP and stage it for restore.
/// The staged file is written to `<data_dir>/notes.db.staged-restore`.
/// The actual swap is deferred to `apply_staged_restore_if_pending`, which must
/// be called after all Db connections are closed (i.e. after the TUI session exits).
pub fn stage_restore_from_zip(zip_path: &str) -> Result<String, String> {
    let resolved = resolve_backup_path(zip_path)?;
    let data_dir = app_core::data_dir()?;
    let staged = data_dir.join(STAGED_RESTORE_FILENAME);

    let db_bytes = extract_notes_db_from_zip(&resolved)?;

    let tmp = data_dir.join(format!(".slate-staged-restore-{}.tmp", Ulid::new()));
    (|| {
        let mut f =
            File::create(&tmp).map_err(|e| format!("failed to write staged restore: {e}"))?;
        f.write_all(&db_bytes)
            .map_err(|e| format!("failed to write staged restore: {e}"))?;
        f.flush()
            .map_err(|e| format!("failed to flush staged restore: {e}"))?;
        fs::rename(&tmp, &staged).map_err(|e| format!("failed to stage restore file: {e}"))?;
        Ok(())
    })()
    .inspect_err(|_: &String| {
        let _ = fs::remove_file(&tmp);
    })?;

    Ok(resolved.display().to_string())
}

/// Apply a staged restore in-session without restarting the process.
///
/// If a staged restore file is present, calls `db.restore_from_sqlite_file` to
/// close all pool connections, swap the file, and reopen — all while the TUI
/// session is still running.  Returns `true` if a restore was applied.
pub fn apply_restore_in_session(db: &Db) -> Result<bool, String> {
    let data_dir = app_core::data_dir()?;
    let staged = data_dir.join(STAGED_RESTORE_FILENAME);
    if !staged.exists() {
        return Ok(false);
    }
    db.restore_from_sqlite_file(&staged)?;
    Ok(true)
}

/// If a staged restore file exists, atomically swap it in as the live `notes.db`.
/// Returns `true` if a restore was applied, `false` if nothing was pending.
/// Must be called after all Db connections are closed.
pub fn apply_staged_restore_if_pending() -> Result<bool, String> {
    let data_dir = app_core::data_dir()?;
    let staged = data_dir.join(STAGED_RESTORE_FILENAME);
    if !staged.exists() {
        return Ok(false);
    }
    let live = data_dir.join("notes.db");
    let old = data_dir.join(format!(".slate-notes-prerestore-{}.db", Ulid::new()));
    if live.exists() {
        fs::rename(&live, &old)
            .map_err(|e| format!("failed to move existing notes.db aside: {e}"))?;
    }
    if let Err(e) = fs::rename(&staged, &live) {
        let _ = fs::rename(&old, &live);
        return Err(format!("failed to apply staged restore: {e}"));
    }
    let _ = fs::remove_file(&old);
    Ok(true)
}

/// Read a Slate backup ZIP (stored, method=0) and extract the raw bytes of `notes.db`.
fn extract_notes_db_from_zip(zip_path: &Path) -> Result<Vec<u8>, String> {
    let mut file = File::open(zip_path)
        .map_err(|e| format!("cannot open backup '{}': {e}", zip_path.display()))?;

    // Scan local file headers sequentially; all entries are stored (method=0).
    // Local header layout (ZIP32):
    //   4  signature  0x04034b50
    //   2  version needed
    //   2  general flags
    //   2  compression method
    //   2  last mod time
    //   2  last mod date
    //   4  crc-32
    //   4  compressed size
    //   4  uncompressed size
    //   2  file name length
    //   2  extra field length
    //   n  file name
    //   m  extra field
    //  ... data (compressed size bytes)
    loop {
        let mut sig = [0u8; 4];
        if file.read_exact(&mut sig).is_err() {
            break;
        }
        if u32::from_le_bytes(sig) != 0x0403_4b50 {
            break;
        }
        let mut header = [0u8; 26];
        file.read_exact(&mut header)
            .map_err(|e| format!("truncated ZIP header in '{}': {e}", zip_path.display()))?;
        // header[0..2]   = version needed
        // header[2..4]   = general flags
        // header[4..6]   = compression method
        // header[6..8]   = last mod time
        // header[8..10]  = last mod date
        // header[10..14] = crc-32
        // header[14..18] = compressed size
        // header[18..22] = uncompressed size
        // header[22..24] = file name length
        // header[24..26] = extra field length
        let method = u16::from_le_bytes([header[4], header[5]]);
        let compressed_size = u32::from_le_bytes([header[14], header[15], header[16], header[17]]);
        let name_len = u16::from_le_bytes([header[22], header[23]]) as usize;
        let extra_len = u16::from_le_bytes([header[24], header[25]]) as usize;

        let mut name_bytes = vec![0u8; name_len];
        file.read_exact(&mut name_bytes)
            .map_err(|e| format!("truncated ZIP entry name in '{}': {e}", zip_path.display()))?;
        file.seek(SeekFrom::Current(extra_len as i64))
            .map_err(|e| format!("seek failed in '{}': {e}", zip_path.display()))?;

        let entry_name = String::from_utf8_lossy(&name_bytes);
        if entry_name == "notes.db" {
            if method != 0 {
                return Err(format!(
                    "notes.db in backup is compressed (method={method}); only stored backups are supported"
                ));
            }
            let mut data = vec![0u8; compressed_size as usize];
            file.read_exact(&mut data)
                .map_err(|e| format!("failed to read notes.db from backup: {e}"))?;
            return Ok(data);
        }

        file.seek(SeekFrom::Current(compressed_size as i64))
            .map_err(|e| format!("seek failed in '{}': {e}", zip_path.display()))?;
    }

    Err(format!(
        "no notes.db entry found in backup '{}'",
        zip_path.display()
    ))
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_core::storage::Db;
    use std::io::Read;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("slate-backup-test-{name}-{}", Ulid::new()))
    }

    #[test]
    fn backup_notes_database_writes_portable_zip() {
        let db_path = temp_path("source.db");
        let zip_path = std::env::temp_dir().join(format!("slate-backup-test-{}.zip", Ulid::new()));
        let db = Db::open(db_path.clone()).expect("db opens");
        db.save_note("n1", "hello backup").expect("note saved");

        let result = backup_notes_database_blocking(&db, zip_path.to_str().unwrap())
            .expect("backup succeeds");
        assert_eq!(PathBuf::from(result.path), zip_path);
        let mut bytes = Vec::new();
        File::open(&zip_path)
            .expect("zip opens")
            .read_to_end(&mut bytes)
            .expect("zip reads");
        assert!(bytes.starts_with(&0x0403_4b50u32.to_le_bytes()));
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("notes.db"));
        assert!(text.contains("manifest.json"));
        assert!(text.contains("README.txt"));

        let _ = fs::remove_file(db_path);
        let _ = fs::remove_file(zip_path);
    }

    #[test]
    fn backup_overwrites_existing_zip_safely() {
        let db_path = temp_path("source.db");
        let zip_path = std::env::temp_dir().join(format!("slate-backup-test-{}.zip", Ulid::new()));
        let db = Db::open(db_path.clone()).expect("db opens");
        db.save_note("n1", "first backup").expect("note saved");

        backup_notes_database_blocking(&db, zip_path.to_str().unwrap())
            .expect("first backup succeeds");
        assert!(zip_path.exists());

        db.save_note("n2", "second note").expect("note saved");
        backup_notes_database_blocking(&db, zip_path.to_str().unwrap())
            .expect("second backup overwrites");
        assert!(zip_path.exists());

        let _ = fs::remove_file(db_path);
        let _ = fs::remove_file(zip_path);
    }

    #[test]
    fn resolve_backup_path_rejects_empty() {
        assert!(resolve_backup_path("").is_err());
        assert!(resolve_backup_path("   ").is_err());
    }

    #[test]
    fn resolve_backup_path_rejects_non_zip_extension() {
        let path = std::env::temp_dir().join("backup.tar");
        assert!(resolve_backup_path(path.to_str().unwrap()).is_err());
    }

    #[test]
    fn resolve_backup_path_rejects_directory() {
        let dir = std::env::temp_dir();
        assert!(resolve_backup_path(dir.to_str().unwrap()).is_err());
    }

    #[test]
    fn resolve_backup_path_rejects_missing_parent() {
        let path = std::env::temp_dir()
            .join(format!("nonexistent-slate-{}", Ulid::new()))
            .join("backup.zip");
        assert!(resolve_backup_path(path.to_str().unwrap()).is_err());
    }

    #[test]
    fn resolve_backup_path_accepts_valid_zip_in_temp() {
        let path = std::env::temp_dir().join("test.zip");
        assert!(resolve_backup_path(path.to_str().unwrap()).is_ok());
    }

    #[test]
    fn crc32_matches_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn extract_notes_db_from_zip_round_trips_backup() {
        let db_path = temp_path("source.db");
        let zip_path = std::env::temp_dir().join(format!("slate-backup-test-{}.zip", Ulid::new()));
        let db = Db::open(db_path.clone()).expect("db opens");
        db.save_note("n1", "round trip content")
            .expect("note saved");

        backup_notes_database_blocking(&db, zip_path.to_str().unwrap()).expect("backup succeeds");

        let extracted = extract_notes_db_from_zip(&zip_path).expect("extract succeeds");
        // SQLite files start with the header string
        assert!(extracted.starts_with(b"SQLite format 3\0"));

        let _ = fs::remove_file(db_path);
        let _ = fs::remove_file(zip_path);
    }
}
