//! The one way this server writes a file only its own user may read.
//!
//! Key material, password hashes, session tokens and the setup code all live in
//! files nobody else on the machine may read, and each of them is read back by
//! the next run — so the write has to be atomic as well as private:
//!
//! 1. write to a fresh temp file and `rename`, so a crash cannot leave half a
//!    store, and a reader never sees a partial one;
//! 2. create that temp file `create_new` with mode `0600`, so the bytes are
//!    never world-readable — not even for the instant between `write` and a
//!    `chmod` — and a temp file left by an earlier crash is never reused;
//! 3. set `0600` once more before the rename, so the mode does not depend on
//!    the umask or on how the inode came to exist;
//! 4. optionally create a missing parent directory `0700`, for the writers that
//!    own the directory they write into (the agent's key/state paths).
//!
//! The rule was written out at each of the five sites that needed it, which is
//! how one of the steps goes missing from one of them; this is the one copy.

use std::io::Write;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Write `data` to `path` as a new `0600` file, atomically.
///
/// The parent directory must already exist; a missing one is `NotFound`, which
/// is what every caller except the agent's own directories reported before the
/// rule was shared.
pub(crate) fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    write(path, data, false)
}

/// [`write_private`] for a caller whose directory may not exist yet: creates the
/// whole parent chain `0700` first. The agent's key, state and rotation paths
/// have always done this; a missing `plugin export --out` directory has not.
pub(crate) fn write_private_creating_parent(path: &Path, data: &[u8]) -> std::io::Result<()> {
    write(path, data, true)
}

fn write(path: &Path, data: &[u8], create_parent: bool) -> std::io::Result<()> {
    let dir = path.parent().filter(|dir| !dir.as_os_str().is_empty()).unwrap_or(Path::new("."));
    if create_parent {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(dir)?;
    }
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("file");
    // Unique, so two writers of the same path (or a temp left by a crash) can
    // never collide, and `create_new` can never reopen an inode with a mode
    // somebody else gave it.
    let tmp = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let written = options.open(&tmp).and_then(|mut file| {
        file.write_all(data)?;
        file.sync_all()?;
        #[cfg(unix)]
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        std::fs::rename(&tmp, path)
    });
    if let Err(error) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn the_file_is_0600_and_the_directory_it_created_is_0700() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("made/up").join("secret.json");
        write_private_creating_parent(&path, b"{\"token\":\"x\"}").expect("write");
        assert_eq!(std::fs::read(&path).expect("read back"), b"{\"token\":\"x\"}");
        assert_eq!(std::fs::metadata(&path).expect("metadata").permissions().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::metadata(path.parent().expect("parent")).expect("metadata").permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn a_missing_parent_directory_is_still_not_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("missing").join("secret.json");
        let error = write_private(&path, b"x").expect_err("no directory to write into");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(!path.parent().expect("parent").exists(), "and nothing was created");
    }

    #[test]
    fn a_write_replaces_what_was_there_and_leaves_no_temp_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.json");
        write_private(&path, b"first").expect("first write");
        write_private(&path, b"second").expect("second write");
        assert_eq!(std::fs::read(&path).expect("read back"), b"second");
        let leftovers = std::fs::read_dir(dir.path())
            .expect("read_dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0, "the temp file is renamed, never left behind");
    }
}
