//! The app's log: `~/Library/Logs/Droppen/desktop.log`, one line per event,
//! also echoed to stderr for a run from a terminal.
//!
//! Deliberately tiny — a `log` backend that appends to one file and rolls it
//! once over a size — because the interesting log is the server's, and the
//! server writes its own ([`crate::sidecar`] sends it to `server.log`).
//! **Never log a secret here**: not the password, not a token, not a URL that
//! carries one. The call sites that handle them say so. The logs are this
//! user's alone all the same — `0600` files in a `0700` directory, like the
//! installer's (`scripts/install.sh`, `private_logs`): they say where the
//! server's setup code is and what it did, which is nobody else's business on
//! a shared Mac.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Roll a log over at this size, keeping one previous file (`.1`).
pub const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

struct FileLogger {
    file: Mutex<Option<File>>,
    echo: bool,
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info || metadata.target().starts_with("droppen")
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!("{} {:<5} {}\n", timestamp(SystemTime::now()), record.level(), record.args());
        if self.echo {
            eprint!("{line}");
        }
        if let Ok(mut guard) = self.file.lock() {
            if let Some(file) = guard.as_mut() {
                let _ = file.write_all(line.as_bytes());
            }
        }
    }

    fn flush(&self) {
        if let Ok(mut guard) = self.file.lock() {
            if let Some(file) = guard.as_mut() {
                let _ = file.flush();
            }
        }
    }
}

/// Install the logger, writing to `path` (rolled first if it is too big).
/// A second call, or a log file that cannot be opened, leaves stderr only.
pub fn init(path: &Path, echo: bool) {
    let file = open_append(path).ok();
    let logger = FileLogger {
        file: Mutex::new(file),
        echo,
    };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(log::LevelFilter::Debug);
    }
}

/// Open `path` for appending, rolling it to `<path>.1` first when it has grown
/// past [`MAX_LOG_BYTES`]. Used for the server's output too.
///
/// The file is this user's alone (`0600`, made so if an earlier build left it
/// wider), in a directory that is too (`0700`).
pub fn open_append(path: &Path) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        crate::dirs::create_private_dir(parent)?;
    }
    if std::fs::metadata(path).map(|meta| meta.len() > MAX_LOG_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(path, rolled(path));
    }
    let file = OpenOptions::new().create(true).append(true).mode(0o600).open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

fn rolled(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".1");
    PathBuf::from(name)
}

/// `2026-10-18T12:34:56Z`, without a date crate.
pub fn timestamp(at: SystemTime) -> String {
    let secs = at.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to a date.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamps_are_utc_iso8601() {
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        // 2026-10-18T12:34:56Z
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(1_792_326_896)), "2026-10-18T12:34:56Z");
        // A leap day.
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(1_709_164_800)), "2024-02-29T00:00:00Z");
    }

    #[test]
    fn a_big_log_is_rolled_before_appending() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("logs/server.log");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(&path, vec![b'x'; (MAX_LOG_BYTES + 1) as usize]).expect("write");
        let mut file = open_append(&path).expect("open");
        file.write_all(b"fresh\n").expect("write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "fresh\n");
        assert_eq!(std::fs::metadata(rolled(&path)).expect("rolled").len(), MAX_LOG_BYTES + 1);
    }

    #[test]
    fn rolling_the_desktop_log_leaves_the_installed_servers_subfolder_alone() {
        let root = tempfile::tempdir().expect("tempdir");
        let logs = root.path().join("Logs/Droppen");
        let server = logs.join("server");
        std::fs::create_dir_all(&server).expect("dir");
        std::fs::write(server.join("default.log"), "keep\n").expect("write");
        let path = logs.join("desktop.log");
        std::fs::write(&path, vec![b'x'; (MAX_LOG_BYTES + 1) as usize]).expect("write");
        open_append(&path).expect("open");
        assert_eq!(std::fs::read_to_string(server.join("default.log")).expect("read"), "keep\n", "the server's folder is not a desktop log");
    }

    #[test]
    fn a_log_is_this_users_alone() {
        let mode = |path: &Path| std::fs::metadata(path).expect("metadata").permissions().mode() & 0o777;
        let root = tempfile::tempdir().expect("tempdir");

        // Made fresh: a 0600 file in a 0700 directory.
        let path = root.path().join("Logs/Droppen/desktop.log");
        open_append(&path).expect("open").write_all(b"line\n").expect("write");
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().expect("parent")), 0o700);

        // Left wider by an earlier build: tightened when opened.
        let older = root.path().join("older/server.log");
        std::fs::create_dir_all(older.parent().expect("parent")).expect("dir");
        std::fs::set_permissions(older.parent().expect("parent"), std::fs::Permissions::from_mode(0o755)).expect("chmod dir");
        std::fs::write(&older, "before\n").expect("write");
        std::fs::set_permissions(&older, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        open_append(&older).expect("open").write_all(b"after\n").expect("write");
        assert_eq!(mode(&older), 0o600);
        assert_eq!(mode(older.parent().expect("parent")), 0o700);
        assert_eq!(std::fs::read_to_string(&older).expect("read"), "before\nafter\n", "appended, not replaced");
    }
}
