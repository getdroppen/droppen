//! Where an instance keeps its files: the home directory, remote-hand's own
//! config and data roots, this instance's directory and databases, and the two
//! Claude config roots.
//!
//! Nearly every subsystem takes a [`Paths`], so it sits below all of them and
//! depends only on [`crate::env`] and [`crate::instance`]: naming a path must not
//! pull in the code of the subsystem that happens to define it. A subsystem adds
//! the paths of its own files in an `impl Paths` block next to the code that
//! reads them. [`Paths::droppen_env`] is how a command typed on this device
//! finds the same instance and files.

use std::path::{Path, PathBuf};

use gotcha::Schematic;
use serde::{Deserialize, Serialize};

use crate::program::{abbreviate_home, shell_word};

/// Which Claude config root a session file lives in.
///
/// The name is the OpenAPI component name in `openapi/openapi.json`
/// (`ClaudeRoot`), so `Schematic::name()` and the golden contract agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum ClaudeRoot {
    /// `~/.claude` — the user's own login.
    Local,
    /// The root *this instance* owns for account-backed sessions.
    Managed,
}

impl ClaudeRoot {
    /// Both roots, local first — the order duplicate ids are resolved against.
    ///
    /// Two, and never more: the only managed root a server may look at is its
    /// own. Widening this array is how one instance would start listing another
    /// one's sessions, so every scan in the crate goes through it.
    pub const ALL: [ClaudeRoot; 2] = [ClaudeRoot::Local, ClaudeRoot::Managed];

    /// The name this root has on the wire, matching the `serde` representation.
    ///
    /// Spelled out rather than round-tripped through `serde_json` because
    /// [`crate::record`] writes it into every line of a recording and reads it
    /// back in the replay tool: one function both directions go through is what
    /// stops the two from drifting.
    pub fn as_str(self) -> &'static str {
        match self {
            ClaudeRoot::Local => "local",
            ClaudeRoot::Managed => "managed",
        }
    }

    /// The inverse of [`ClaudeRoot::as_str`]; `None` for anything else.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|root| root.as_str() == name)
    }
}

/// Test seam: point the user's own root at a scratch directory. Its old name,
/// `REMOTE_HAND_CLAUDE_LOCAL_ROOT`, is still read until v0.2.135
/// ([`crate::env`]).
pub const LOCAL_ROOT_ENV: &str = "DROPPEN_CLAUDE_LOCAL_ROOT";
/// Test seam: point the managed root at a scratch directory. Its old name,
/// `REMOTE_HAND_CLAUDE_MANAGED_ROOT`, is still read until v0.2.135
/// ([`crate::env`]).
pub const MANAGED_ROOT_ENV: &str = "DROPPEN_CLAUDE_MANAGED_ROOT";

/// An environment override, honoured only when it is a non-empty value; under
/// its old name too ([`crate::env::var`]).
///
/// Kept as its own function (rather than inlined `std::env::var`) so the
/// precedence rule is unit-testable without mutating the process environment.
pub fn env_dir(name: &str) -> Option<PathBuf> {
    override_dir(crate::env::var(name).as_deref())
}

/// The override-or-nothing decision, with the value supplied explicitly.
pub fn override_dir(value: Option<&str>) -> Option<PathBuf> {
    // An empty value is absent: it means the same as unset.
    value.filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Every directory session discovery reads, rooted at one home directory.
///
/// Threading this through instead of reading `homedir()` at module scope (as the
/// TypeScript does) is what lets the integration test point the server at a
/// synthetic `~/.claude` tree.
///
/// Every directory the environment may move is resolved once, at construction,
/// through [`env_dir`] — the one reader of those overrides.
/// `DROPPEN_CONFIG_DIR` and `DROPPEN_DATA_DIR` name remote-hand's own roots;
/// `DROPPEN_CLAUDE_LOCAL_ROOT` and `DROPPEN_CLAUDE_MANAGED_ROOT` name the two
/// Claude ones (each still read under its old `REMOTE_HAND_*` name,
/// [`crate::env`]). Resolving them
/// here rather than on each access is what makes a `Paths` a value: two of them
/// can disagree, and neither changes under a handler because something else
/// touched the process environment.
#[derive(Debug, Clone)]
pub struct Paths {
    /// The home directory everything hangs off.
    pub home: PathBuf,
    config_dir: PathBuf,
    data_dir: PathBuf,
    claude_local: PathBuf,
    /// Which instance's directory the owned half of the world lives in.
    instance: String,
    claude_managed: PathBuf,
}

impl Paths {
    /// Root a path set at `home`, for the default instance.
    ///
    /// A server always calls [`Paths::for_instance`] instead; this is the shape
    /// every unit test and every path-only caller wants, and it names the same
    /// instance a server started with no `APP_INSTANCE` would.
    pub fn new(home: PathBuf) -> Self {
        Self::for_instance(home, crate::instance::DEFAULT_INSTANCE)
    }

    /// Root a path set at `home`, for the instance this server owns.
    ///
    /// The instance name is part of a `Paths` rather than looked up per access
    /// for the same reason the environment overrides are resolved once: two of
    /// these coexist in one test process, and neither may change under a handler
    /// because something else moved.
    pub fn for_instance(home: PathBuf, instance: &str) -> Self {
        let config_dir = env_dir("DROPPEN_CONFIG_DIR").unwrap_or_else(|| home.join(".config").join("remote-hand"));
        let data_dir = env_dir("DROPPEN_DATA_DIR").unwrap_or_else(|| home.join(".local").join("share").join("remote-hand"));
        let claude_local = env_dir(LOCAL_ROOT_ENV).unwrap_or_else(|| home.join(".claude"));
        // The managed root defaults *inside this instance's own directory*, so
        // it is resolved after the data directory and inherits any
        // `DROPPEN_DATA_DIR` override. Two servers on one machine therefore
        // never see each other's sessions — which is the whole of the ownership
        // model this file's `Managed` root now encodes.
        let claude_managed = env_dir(MANAGED_ROOT_ENV)
            .unwrap_or_else(|| crate::instance::instance_root(&data_dir, instance).join("claude"));
        Self {
            home,
            config_dir,
            data_dir,
            claude_local,
            instance: instance.to_string(),
            claude_managed,
        }
    }

    /// The instance whose directory the owned half of this path set lives in.
    pub fn instance(&self) -> &str {
        &self.instance
    }

    /// `<data>/instances/<name>` — this instance's private directory.
    pub fn instance_dir(&self) -> PathBuf {
        crate::instance::instance_root(&self.data_dir, &self.instance)
    }

    /// The machine-wide state database: everything in the config directory that
    /// is not a file a person edits.
    ///
    /// Machine-wide because the things in it are: a Claude account is a login
    /// somebody registered, a title is a name someone gave a session, and a
    /// session marked read is read on every server on this machine. It is also
    /// the scope where the current files are least defensible — two servers
    /// share this directory right now and guard it with mutexes private to each
    /// process, so the loser of a race simply loses their edit. See
    /// [`crate::db`].
    ///
    /// One table in it is no longer written: `users`, the *local* accounts,
    /// which moved to [`Paths::instance_db`] when it turned out that a machine
    /// runs one instance per person as readily as one per purpose. What is
    /// there is frozen rather than dropped, so a rolled-back binary still finds
    /// accounts and stays shut; [`crate::auth::users`] has the argument.
    pub fn config_db(&self) -> PathBuf {
        self.config_dir.join("state.db")
    }

    /// This instance's own state database: the runs, the apps, the layouts, the
    /// accounts and the logins they mint, the schedule.
    ///
    /// Separate from [`Paths::config_db`] because the instance boundary is a
    /// real one — stopping the staging server must not log the phone out of the
    /// default one — and separate files are the cheapest way to keep a boundary
    /// that has already been got wrong twice (`screens.json` and `push.json`
    /// were machine-wide and clobbered each other).
    pub fn instance_db(&self) -> PathBuf {
        self.instance_dir().join("state.db")
    }

    /// The index over the transcripts, which belongs to nobody.
    ///
    /// A third file rather than a table in [`Paths::instance_db`], because it is
    /// a *cache* and the difference has consequences: it can be deleted at any
    /// time and rebuilt from the corpus, it is the one store here that will
    /// reach hundreds of megabytes, and it does not need the `synchronous =
    /// FULL` the authoritative state does — losing the last few commits to a
    /// power cut costs a re-scan, not a fact. Keeping it out of `state.db` is
    /// what lets all three of those be true at once.
    pub fn index_db(&self) -> PathBuf {
        self.instance_dir().join("index.db")
    }

    /// `<data>/claude` — the managed root as it was before it became per-instance.
    ///
    /// Only [`crate::claude_roots::migrate_legacy_root`] reads this: it is where
    /// every account-backed session of every earlier build lives, and the default
    /// instance claims it once at boot.
    pub fn legacy_claude_managed(&self) -> PathBuf {
        self.data_dir.join("claude")
    }

    /// The directory of one Claude config root.
    pub fn claude_root(&self, root: ClaudeRoot) -> PathBuf {
        match root {
            ClaudeRoot::Local => self.claude_local.clone(),
            ClaudeRoot::Managed => self.claude_managed.clone(),
        }
    }

    /// `<data>/codex-claims` — which instance made which codex thread.
    ///
    /// Shared, like `inject-locks` and unlike everything under
    /// [`Paths::instance_dir`]: a claim exists precisely to be read by the
    /// instances that did *not* write it, which is the only way one of them can
    /// tell another's thread from the user's own in the `~/.codex` they share.
    /// See [`codex_claims`](crate::sessions::codex_claims).
    pub fn codex_claims_dir(&self) -> PathBuf {
        self.data_dir.join("codex-claims")
    }

    /// remote-hand's own sidecar state, overridable with `DROPPEN_CONFIG_DIR`.
    pub fn config_dir(&self) -> PathBuf {
        self.config_dir.clone()
    }

    /// remote-hand's own data (accounts, uploads, chats, the managed Claude
    /// root), overridable with `DROPPEN_DATA_DIR`.
    pub fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    /// Where composer attachments land, one uuid directory per upload; the CLIs
    /// are pointed at it with `--add-dir`.
    pub fn uploads_dir(&self) -> PathBuf {
        self.data_dir.join("uploads")
    }
}

impl Paths {
    /// What a command typed on this device needs in its environment to act on
    /// the instance this `Paths` names, as `(name, value)` pairs to put before
    /// [`droppen_path`](crate::program::droppen_path): `APP_INSTANCE` for any instance but `default`, and
    /// `DROPPEN_DATA_DIR` / `DROPPEN_CONFIG_DIR` when this server keeps its files
    /// elsewhere than `~/.local/share/remote-hand` / `~/.config/remote-hand` (a
    /// second install beside the first). Nothing for the usual install. Paths
    /// have the home directory as `~`, for the reason [`droppen_path`](crate::program::droppen_path) does.
    ///
    /// Each directory under its old name too, `REMOTE_HAND_DATA_DIR` /
    /// `REMOTE_HAND_CONFIG_DIR`, with the same value: the `droppen` a person's
    /// shell finds may be another install's pre-rename binary, which reads only
    /// the old names and would act on its own files instead
    /// ([`crate::env`]).
    ///
    /// Without these, `open`, `setup-code` and `user add` typed as the page says
    /// would act on the default instance of the default install: another
    /// server's setup code, typed here, is a wrong one that counts against the
    /// limit.
    pub fn droppen_env(&self) -> Vec<(String, String)> {
        let home = std::env::var_os("HOME").filter(|home| !home.is_empty()).map(PathBuf::from);
        droppen_env_for(self, home.as_deref())
    }
}

/// [`Paths::droppen_env`] as a person types it before a command: `NAME=value `
/// for each, the values made shell words ([`shell_word`]); empty for the usual
/// install.
pub fn command_prefix(paths: &Paths) -> String {
    paths.droppen_env()
        .into_iter()
        .map(|(name, value)| format!("{name}={} ", shell_word(&value)))
        .collect()
}

/// [`Paths::droppen_env`], with the home directory supplied.
pub fn droppen_env_for(paths: &Paths, home: Option<&Path>) -> Vec<(String, String)> {
    let mut env = Vec::new();
    if paths.instance() != crate::instance::DEFAULT_INSTANCE {
        env.push(("APP_INSTANCE".to_string(), paths.instance().to_string()));
    }
    let usual = |rest: &[&str]| home.map(|home| rest.iter().fold(home.to_path_buf(), |path, part| path.join(part)));
    let (data, config) = (paths.data_dir(), paths.config_dir());
    let mut both = |name: &'static str, path: &Path| {
        let value = abbreviate_home(path, home);
        env.extend(crate::env::with_old_name(name).map(|name| (name.to_string(), value.clone())));
    };
    if usual(&[".local", "share", "remote-hand"]).as_deref() != Some(data.as_path()) {
        both("DROPPEN_DATA_DIR", &data);
    }
    if usual(&[".config", "remote-hand"]).as_deref() != Some(config.as_path()) {
        both("DROPPEN_CONFIG_DIR", &config);
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_override_is_no_override() {
        assert_eq!(override_dir(None), None);
        assert_eq!(override_dir(Some("")), None, "`process.env.X || fallback` treats \"\" as absent");
        assert_eq!(override_dir(Some("/scratch")), Some(PathBuf::from("/scratch")));
    }

    #[test]
    fn a_command_names_the_instance_and_the_files_of_a_server_that_is_not_the_usual_one() {
        let home = Path::new("/home/ada");
        let pair = |name: &str, value: &str| (name.to_string(), value.to_string());
        let usual = crate::paths::Paths::new(home.to_path_buf());
        assert_eq!(droppen_env_for(&usual, Some(home)), Vec::<(String, String)>::new(), "the usual install needs nothing");

        let other_instance = crate::paths::Paths::for_instance(home.to_path_buf(), "bob");
        assert_eq!(droppen_env_for(&other_instance, Some(home)), vec![pair("APP_INSTANCE", "bob")]);

        // A second install, its files under another root.
        // Under both names: the `droppen` a shell finds may be a pre-rename
        // binary, which reads only the old ones.
        let second = crate::paths::Paths::for_instance(home.join("bob"), "default");
        let (data, config) = (crate::env::old_name("DROPPEN_DATA_DIR").unwrap(), crate::env::old_name("DROPPEN_CONFIG_DIR").unwrap());
        assert_eq!(
            droppen_env_for(&second, Some(home)),
            vec![
                pair("DROPPEN_DATA_DIR", "~/bob/.local/share/remote-hand"),
                pair(data, "~/bob/.local/share/remote-hand"),
                pair("DROPPEN_CONFIG_DIR", "~/bob/.config/remote-hand"),
                pair(config, "~/bob/.config/remote-hand"),
            ]
        );
        // With no home directory to compare with, the files are named outright.
        assert_eq!(
            droppen_env_for(&usual, None),
            vec![
                pair("DROPPEN_DATA_DIR", "/home/ada/.local/share/remote-hand"),
                pair(data, "/home/ada/.local/share/remote-hand"),
                pair("DROPPEN_CONFIG_DIR", "/home/ada/.config/remote-hand"),
                pair(config, "/home/ada/.config/remote-hand"),
            ]
        );
    }
}
