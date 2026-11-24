//! The files a standalone device keeps ([`crate::identity`], [`crate::verified`]):
//! written so only its own user may read them, and atomically, because the
//! next run reads them back — the rule `droppen-base`'s `fsutil` keeps for the
//! server, kept here for a crate that may not depend on the server:
//!
//! 1. a fresh temp file beside the target, created `create_new` with mode
//!    `0600`, so the bytes are never readable by others and a temp file an
//!    earlier crash left is never reused;
//! 2. written, synced, set `0600` once more (whatever the umask did), then
//!    renamed over the target, so a reader never sees half a file;
//! 3. a missing directory made `0700` first.
//!
//! [`read_private`] is the reading side: a file others can read is refused,
//! as the agent refuses its own.

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Write `data` to `path` as a new `0600` file in a `0700` directory,
/// atomically. The error names the path.
pub fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    write(path, data).map_err(|error| format!("{}: {error}", path.display()))
}

fn write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().filter(|dir| !dir.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)?;
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("file");
    // Unique within the process and across processes; `create_new` refuses
    // anything that is there already, so two writers never share one.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(
        ".{name}.{}.{nanos}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let written = options.open(&tmp).and_then(|mut file| {
        file.write_all(data)?;
        file.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, path)
    });
    if let Err(error) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(())
}

/// The bytes of `path`, `None` when there is no such file. A file that
/// others can read is refused, naming it and its mode.
pub fn read_private(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)
            .map_err(|error| format!("{}: {error}", path.display()))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(format!(
                "{} is readable by others (mode {:o}); chmod 600 it",
                path.display(),
                mode & 0o777
            ));
        }
    }
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("droppen-client-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[cfg(unix)]
    #[test]
    fn a_file_is_0600_in_a_0700_directory_and_one_others_can_read_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("modes");
        let path = dir.join("made").join("state.json");
        write_private(&path, b"{}").unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(read_private(&path).unwrap().as_deref(), Some(&b"{}"[..]));
        write_private(&path, b"[1]").unwrap();
        assert_eq!(read_private(&path).unwrap().as_deref(), Some(&b"[1]"[..]), "replaced whole");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_private(&path).unwrap_err().contains("readable by others"));
        assert_eq!(read_private(&dir.join("none")).unwrap(), None);
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "no temp file left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
