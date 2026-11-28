//! What the app remembers between launches, in `state.json`: the port it last
//! ran the server on (see [`crate::port`] for why that matters), an old login
//! account still to be removed, whether *Start at Login*'s default was
//! applied, and nothing secret. A file that is missing or unreadable is a
//! first launch, not an error.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::login_item::LoginDefault;

/// The contents of `state.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellState {
    /// The loopback port the server last ran on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// The account an earlier login of the app used, which the app removes
    /// once its `droppen-app` login has signed in
    /// ([`crate::accounts::Cli::retire`]). Written before the new login is
    /// kept, so a start cut short in between still finishes the job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_login: Option<String>,
    /// What became of *Start at Login*'s default
    /// ([`crate::login_item::default_at_launch`]); absent in an install from
    /// before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_default: Option<LoginDefault>,
}

impl ShellState {
    /// Read `path`; anything wrong with it reads as an empty state.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Write `path` atomically (a temporary file renamed over it).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let body = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        write_atomically(path, &body, 0o600)
    }

    /// Keep `port` in `path` as the one to start on next time, if it is not
    /// already; whether it wrote.
    ///
    /// Called once a window has loaded the web app on `port`, never when the
    /// port is merely chosen: until then the old port's origin still holds the
    /// login and the device keys, and a start that fails in between must go
    /// back to it.
    pub fn remember_port(path: &Path, port: u16) -> io::Result<bool> {
        let mut state = Self::load(path);
        if state.port == Some(port) {
            return Ok(false);
        }
        state.port = Some(port);
        state.save(path)?;
        Ok(true)
    }
}

/// Write `body` to a sibling temporary file with `mode`, then rename it over
/// `path`, so a crash leaves the old file or the new one and never half of one.
pub fn write_atomically(path: &Path, body: &[u8], mode: u32) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    let parent = path.parent().ok_or_else(|| io::Error::other("no parent directory"))?;
    std::fs::create_dir_all(parent)?;
    let name = path.file_name().ok_or_else(|| io::Error::other("no file name"))?.to_string_lossy();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".{name}.{}.{seq}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temporary)?;
        file.write_all(body)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_or_broken_file_is_a_first_launch() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("state.json");
        assert_eq!(ShellState::load(&path), ShellState::default());
        std::fs::write(&path, "{not json").expect("write");
        assert_eq!(ShellState::load(&path), ShellState::default());
    }

    #[test]
    fn a_port_is_remembered_once() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("state.json");
        ShellState {
            port: Some(51_234),
            replaced_login: Some("ada".into()),
            login_default: Some(LoginDefault::Applied),
        }
        .save(&path)
        .expect("save");
        assert!(ShellState::remember_port(&path, 18_721).expect("remember"));
        assert_eq!(ShellState::load(&path).port, Some(18_721));
        assert_eq!(
            ShellState::load(&path).replaced_login.as_deref(),
            Some("ada"),
            "the rest of the state is kept"
        );
        assert_eq!(ShellState::load(&path).login_default, Some(LoginDefault::Applied));
        assert!(!ShellState::remember_port(&path, 18_721).expect("again"), "nothing to write");
        // A first launch has no file yet.
        let fresh = root.path().join("new/state.json");
        assert!(ShellState::remember_port(&fresh, 18_700).expect("first"));
        assert_eq!(ShellState::load(&fresh).port, Some(18_700));
    }

    #[test]
    fn the_state_survives_a_round_trip() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("nested/state.json");
        let state = ShellState {
            port: Some(51_234),
            replaced_login: Some("ada".into()),
            login_default: Some(LoginDefault::LeftAlone),
        };
        state.save(&path).expect("save");
        assert_eq!(ShellState::load(&path), state);
        // An install from before the default has no such field, and reads.
        std::fs::write(&path, r#"{"port": 51234}"#).expect("write");
        assert_eq!(ShellState::load(&path).login_default, None);
        assert_eq!(ShellState::load(&path).port, Some(51_234));
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().expect("parent"))
            .expect("list")
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn concurrent_atomic_writes_never_collide() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("state.json");
        std::thread::scope(|scope| {
            for i in 0..8 {
                let path = path.clone();
                scope.spawn(move || {
                    for j in 0..20 {
                        let body = format!("{{\"port\": {}}}", 10_000 + i * 100 + j);
                        write_atomically(&path, body.as_bytes(), 0o600).expect("write");
                    }
                });
            }
        });
        let loaded = ShellState::load(&path);
        assert!(loaded.port.is_some());
    }
}
