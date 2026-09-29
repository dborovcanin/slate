//! Hands files and URLs to the desktop's default application.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Opens `target` (a path or URL) with the platform launcher. The launcher's
/// output is discarded so it cannot paint over the TUI, and it is reaped on a
/// background thread so it does not linger as a zombie.
pub(crate) fn open_with_default_app(target: &OsStr) -> Result<(), String> {
    let mut command = launcher_command()?;
    let mut child = command
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("{} failed: {err}", command.get_program().to_string_lossy()))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn launcher_command() -> Result<Command, String> {
    #[cfg(target_os = "linux")]
    return Ok(Command::new("xdg-open"));
    #[cfg(target_os = "macos")]
    return Ok(Command::new("open"));
    #[cfg(target_os = "windows")]
    {
        let mut command = Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler");
        return Ok(command);
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    Err("opening files is unsupported on this platform".to_string())
}

/// Writes `bytes` to a new private file (0600 in a 0700 directory) so an
/// external viewer can open a database-stored image. Prefers the per-user
/// runtime directory, which is not shared and is cleared on logout.
pub(crate) fn write_private_temp_file(bytes: &[u8], extension: &str) -> Result<PathBuf, String> {
    let root = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    let directory = root.join(format!("slate-open-image-{}", ulid::Ulid::new()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|error| error.to_string())?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir(&directory).map_err(|error| error.to_string())?;

    let path = directory.join(format!("image.{extension}"));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&path).map_err(|error| error.to_string())?;
        std::io::Write::write_all(&mut file, bytes).map_err(|error| error.to_string())?;
        Ok(path.clone())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&directory);
    }
    result
}

/// Removes a file created by [`write_private_temp_file`] and its directory.
pub(crate) fn remove_private_temp_file(path: &std::path::Path) {
    if let Some(directory) = path.parent() {
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_temp_file_is_private_and_removable() {
        let path = write_private_temp_file(b"pixels", "png").expect("create private image");
        assert_eq!(std::fs::read(&path).expect("read image"), b"pixels");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = |p: &std::path::Path| {
                std::fs::metadata(p).expect("metadata").permissions().mode() & 0o777
            };
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(path.parent().expect("private dir")), 0o700);
        }
        remove_private_temp_file(&path);
        assert!(!path.parent().expect("private dir").exists());
    }
}
