//! Where the app keeps things.
//!
//! Everything hangs off `$HOME`, read once, so a test (or a manual run with a
//! throwaway `HOME`) moves all of it at once:
//!
//! | what | where |
//! |---|---|
//! | the app's state, the credential file fallback, the server's pid | `~/Library/Application Support/app.droppen.desktop/` (0700) |
//! | the app's log and the server's output | `~/Library/Logs/Droppen/` |
//! | the server's own data | `~/.local/share/remote-hand/instances/droppen-app/`, chosen by the server itself |
//!
//! (An end-to-end test build's: `app.droppen.desktop.e2e/`, `Droppen E2E/` and
//! `droppen-app-e2e/` — [`crate::identity`] — under a throwaway `HOME`.)
//!
//! On Linux, for now, the support directory is `~/.local/share/<bundle id>/`
//! and the logs its `logs/` ([`ShellDirs::under`]).
//!
//! The server's directories are deliberately *not* overridden: the app runs
//! the binary exactly as a command-line install would, under an instance name
//! of its own ([`crate::INSTANCE`]), so the machine-wide files every instance
//! shares (uploads, notification settings, AI accounts) are the same ones.

use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// The app's directories, all derived from one home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellDirs {
    /// `$HOME`.
    pub home: PathBuf,
    /// `~/Library/Application Support/app.droppen.desktop`.
    pub support: PathBuf,
    /// `~/Library/Logs/Droppen` (`Droppen E2E` in an end-to-end build).
    pub logs: PathBuf,
}

impl ShellDirs {
    /// The directories under `home`.
    pub fn under(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        #[cfg(target_os = "macos")]
        let support = home.join("Library").join("Application Support").join(crate::BUNDLE_ID);
        #[cfg(target_os = "macos")]
        let logs = home.join("Library").join("Logs").join(crate::identity::THIS.name);
        // No `~/Library` in a Linux home: where Tauri keeps an app's data and
        // logs there by default. TODO(linux: L2): `$XDG_DATA_HOME` and
        // `$XDG_STATE_HOME`.
        #[cfg(not(target_os = "macos"))]
        let support = home.join(".local").join("share").join(crate::BUNDLE_ID);
        #[cfg(not(target_os = "macos"))]
        let logs = support.join("logs");
        Self { home, support, logs }
    }

    /// The directories under `$HOME`, or `None` when it is unset or relative.
    pub fn from_env() -> Option<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        home.is_absolute().then(|| Self::under(home))
    }

    /// Create both directories, private (0700): the support one may hold the
    /// credential file, and the logs say where the server's setup code is.
    pub fn create(&self) -> io::Result<()> {
        create_private_dir(&self.support)?;
        create_private_dir(&self.logs)
    }

    /// `state.json`: the port and account name the app keeps across launches.
    pub fn state_file(&self) -> PathBuf {
        self.support.join("state.json")
    }

    /// `credentials.json`: the password, when the Keychain cannot hold it.
    pub fn credential_file(&self) -> PathBuf {
        self.support.join("credentials.json")
    }

    /// `server.pid`: the server this app started, for the next launch to stop
    /// if this one died without doing so.
    pub fn pid_file(&self) -> PathBuf {
        self.support.join("server.pid")
    }

    /// The app's own log.
    pub fn app_log(&self) -> PathBuf {
        self.logs.join("desktop.log")
    }

    /// The server's standard output and error.
    pub fn server_log(&self) -> PathBuf {
        self.logs.join("server.log")
    }
}

/// The user's home as the user database has it, whatever `$HOME` says (an
/// end-to-end run, say, points `$HOME` elsewhere).
pub fn account_home() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;

    let mut buffer = vec![0u8; 16 * 1024];
    // SAFETY: getpwuid_r fills `entry` with pointers into `buffer`, both ours
    // and alive until the name is copied out below.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    let status = unsafe { libc::getpwuid_r(libc::getuid(), &mut entry, buffer.as_mut_ptr().cast(), buffer.len(), &mut found) };
    if status != 0 || found.is_null() || entry.pw_dir.is_null() {
        return None;
    }
    let dir = unsafe { std::ffi::CStr::from_ptr(entry.pw_dir) };
    let dir = PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes()));
    dir.is_absolute().then_some(dir)
}

/// The login item *Start at Login* writes: `~/Library/LaunchAgents/<bundle id>.plist`
/// under `home` (tauri-plugin-autostart, launch-agent mode).
pub fn login_item(home: &Path) -> PathBuf {
    home.join("Library").join("LaunchAgents").join(format!("{}.plist", crate::BUNDLE_ID))
}

/// Whether a login item's property list starts `exe` — a login item made
/// before the app was moved still names where it was.
pub fn login_item_starts(plist: &str, exe: &Path) -> bool {
    plist.contains(&format!("<string>{}</string>", exe.display()))
}

/// `mkdir -p` with the last component 0700, tightened if it already existed.
pub fn create_private_dir(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_hangs_off_the_home_it_was_given() {
        let dirs = ShellDirs::under("/tmp/home");
        let (support, logs) = if cfg!(feature = "e2e") {
            ("app.droppen.desktop.e2e", "Droppen E2E")
        } else {
            ("app.droppen.desktop", "Droppen")
        };
        if cfg!(target_os = "macos") {
            assert_eq!(dirs.support, PathBuf::from("/tmp/home/Library/Application Support").join(support));
            assert_eq!(dirs.logs, PathBuf::from("/tmp/home/Library/Logs").join(logs));
        } else {
            assert_eq!(dirs.support, PathBuf::from("/tmp/home/.local/share").join(support));
            assert_eq!(dirs.logs, dirs.support.join("logs"));
        }
        assert!(dirs.credential_file().starts_with(&dirs.support));
        assert!(dirs.pid_file().starts_with(&dirs.support));
        assert!(dirs.server_log().starts_with(&dirs.logs));
    }

    #[test]
    fn the_account_home_is_absolute_and_ignores_home() {
        let home = account_home().expect("the user database knows this user");
        assert!(home.is_absolute());
        // The same answer whatever $HOME says (not changed here: tests share it).
        assert_eq!(account_home(), Some(home));
    }

    #[test]
    fn a_login_item_is_checked_against_where_the_app_is_now() {
        assert_eq!(
            login_item(Path::new("/Users/someone")),
            PathBuf::from(format!("/Users/someone/Library/LaunchAgents/{}.plist", crate::BUNDLE_ID))
        );
        let plist = "<plist><dict><key>ProgramArguments</key><array><string>/Users/someone/Downloads/Droppen.app/Contents/MacOS/Droppen</string><string>--autostart</string></array></dict></plist>";
        assert!(login_item_starts(
            plist,
            Path::new("/Users/someone/Downloads/Droppen.app/Contents/MacOS/Droppen")
        ));
        assert!(!login_item_starts(plist, Path::new("/Applications/Droppen.app/Contents/MacOS/Droppen")));
    }

    #[test]
    fn the_support_directory_is_private_even_if_it_existed() {
        let root = tempfile::tempdir().expect("tempdir");
        let dirs = ShellDirs::under(root.path());
        std::fs::create_dir_all(&dirs.support).expect("pre-create");
        std::fs::set_permissions(&dirs.support, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        dirs.create().expect("create");
        let mode = std::fs::metadata(&dirs.support).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        assert!(dirs.logs.is_dir());
    }
}
