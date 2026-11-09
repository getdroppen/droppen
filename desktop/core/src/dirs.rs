//! Where the app keeps things.
//!
//! Everything hangs off `$HOME`, read once, so a test (or a manual run with a
//! throwaway `HOME`) moves all of it at once — and on Linux off the XDG base
//! directories `$XDG_DATA_HOME` and `$XDG_STATE_HOME` too ([`Xdg`]), read at
//! the same time, where they are set:
//!
//! | what | macOS | Linux |
//! |---|---|---|
//! | the app's state, the credential file fallback, the server's pid | `~/Library/Application Support/app.droppen.desktop/` (0700) | `$XDG_DATA_HOME/app.droppen.desktop/` (0700), by default `~/.local/share/…` |
//! | the app's log and the server's output | `~/Library/Logs/Droppen/` | `$XDG_STATE_HOME/app.droppen.desktop/logs/`, by default `~/.local/state/…` |
//! | the server's own data | `~/.local/share/remote-hand/instances/droppen-app/`, chosen by the server itself | the same: the server reads no XDG variable |
//!
//! (An end-to-end test build's: `app.droppen.desktop.e2e/`, `Droppen E2E/` —
//! on Linux `app.droppen.desktop.e2e/logs/` — and `droppen-app-e2e/`,
//! [`crate::identity`], under a throwaway `HOME`.)
//!
//! On Linux the support directory is the web view's too: Tauri gives WebKitGTK
//! the same `$XDG_DATA_HOME/<bundle id>/` for its local storage, IndexedDB,
//! caches and cookies. The app's files stay at its top, beside WebKit's
//! folders, rather than in one of their own: WebKit makes its folders 0755,
//! so the app's 0700 on this one is what keeps the web view's data — the
//! window's login token is in its local storage — the user's alone.
//!
//! The server's directories are deliberately *not* overridden: the app runs
//! the binary exactly as a command-line install would, under an instance name
//! of its own ([`crate::INSTANCE`]), so the machine-wide files every instance
//! shares (uploads, notification settings, AI accounts) are the same ones.

use std::ffi::OsString;
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// The app's directories, all derived from one home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellDirs {
    /// `$HOME`.
    pub home: PathBuf,
    /// `~/Library/Application Support/app.droppen.desktop`; on Linux
    /// `$XDG_DATA_HOME/app.droppen.desktop`.
    pub support: PathBuf,
    /// `~/Library/Logs/Droppen` (`Droppen E2E` in an end-to-end build); on
    /// Linux `$XDG_STATE_HOME/app.droppen.desktop/logs`.
    pub logs: PathBuf,
}

/// The XDG base directories a Linux session may set
/// (<https://specifications.freedesktop.org/basedir-spec/latest/>), read
/// together with `$HOME` ([`ShellDirs::from_env`]); `None` is the spec's
/// default. macOS ignores them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Xdg {
    /// `$XDG_DATA_HOME`, else `~/.local/share`: the support directory's parent.
    pub data_home: Option<PathBuf>,
    /// `$XDG_STATE_HOME`, else `~/.local/state`: the logs'.
    pub state_home: Option<PathBuf>,
}

impl Xdg {
    /// From the variables' values. An unset, empty or relative one is none:
    /// the spec has a relative path ignored — and so does Tauri, which puts
    /// the web view's data under the same `$XDG_DATA_HOME`.
    pub fn from_values(data_home: Option<OsString>, state_home: Option<OsString>) -> Self {
        let absolute = |value: Option<OsString>| value.map(PathBuf::from).filter(|path| path.is_absolute());
        Self {
            data_home: absolute(data_home),
            state_home: absolute(state_home),
        }
    }
}

impl ShellDirs {
    /// The directories under `home`, where no XDG variable is set.
    pub fn under(home: impl Into<PathBuf>) -> Self {
        Self::with_xdg(home, &Xdg::default())
    }

    /// The directories under `home`, or where `xdg` says on Linux.
    pub fn with_xdg(home: impl Into<PathBuf>, xdg: &Xdg) -> Self {
        let home = home.into();
        #[cfg(target_os = "macos")]
        let (support, logs) = {
            let _ = xdg;
            (
                home.join("Library").join("Application Support").join(crate::BUNDLE_ID),
                home.join("Library").join("Logs").join(crate::identity::THIS.name),
            )
        };
        // Both by the bundle id, as Tauri names the web view's data directory,
        // rather than by the app's name: no folder in a Linux home is called
        // `Droppen E2E`.
        #[cfg(not(target_os = "macos"))]
        let (support, logs) = {
            let data = xdg.data_home.clone().unwrap_or_else(|| home.join(".local").join("share"));
            let state = xdg.state_home.clone().unwrap_or_else(|| home.join(".local").join("state"));
            (data.join(crate::BUNDLE_ID), state.join(crate::BUNDLE_ID).join("logs"))
        };
        Self { home, support, logs }
    }

    /// The directories under `$HOME` (and `$XDG_DATA_HOME`, `$XDG_STATE_HOME`
    /// on Linux), or `None` when it is unset or relative.
    pub fn from_env() -> Option<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        let xdg = Xdg::from_values(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("XDG_STATE_HOME"));
        home.is_absolute().then(|| Self::with_xdg(home, &xdg))
    }

    /// Create both directories, private (0700): the support one may hold the
    /// credential file (and on Linux holds the web view's data), and the logs
    /// say where the server's setup code is.
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
            assert_eq!(dirs.logs, PathBuf::from("/tmp/home/.local/state").join(support).join("logs"));
        }
        assert!(dirs.credential_file().starts_with(&dirs.support));
        assert!(dirs.pid_file().starts_with(&dirs.support));
        assert!(dirs.server_log().starts_with(&dirs.logs));
    }

    #[test]
    fn linux_follows_the_xdg_base_directories_that_are_absolute() {
        let xdg = Xdg::from_values(Some("/data".into()), Some("/state".into()));
        let dirs = ShellDirs::with_xdg("/tmp/home", &xdg);
        assert_eq!(dirs.home, PathBuf::from("/tmp/home"));
        if cfg!(target_os = "macos") {
            assert_eq!(dirs, ShellDirs::under("/tmp/home"), "macOS reads no XDG variable");
        } else {
            assert_eq!(dirs.support, Path::new("/data").join(crate::BUNDLE_ID));
            assert_eq!(dirs.logs, Path::new("/state").join(crate::BUNDLE_ID).join("logs"));
        }
        // Unset, empty or relative: the default.
        for (data, state) in [(None, None), (Some(""), Some("")), (Some("share"), Some("./state"))] {
            assert_eq!(Xdg::from_values(data.map(OsString::from), state.map(OsString::from)), Xdg::default());
        }
        // Each on its own.
        let state_only = Xdg::from_values(Some("share".into()), Some("/state".into()));
        assert_eq!(state_only.data_home, None);
        assert_eq!(state_only.state_home, Some(PathBuf::from("/state")));
        assert_eq!(
            ShellDirs::with_xdg("/tmp/home", &state_only).support,
            ShellDirs::under("/tmp/home").support
        );
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
