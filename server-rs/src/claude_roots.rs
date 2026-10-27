//! The two Claude config roots remote-hand knows about.
//!
//! Port of the former TypeScript server's `claude-roots.ts`. Claude Code keeps
//! everything (projects, live-session registrations, credentials) under one
//! config root, chosen by `CLAUDE_CONFIG_DIR`. remote-hand knows two of them:
//!
//! * **local** — `~/.claude`, the user's own login. Never touched by the account
//!   machinery: sessions there keep spawning exactly as before. Everything in it
//!   is **external** to this server — browsable, but not its own.
//! * **managed** — the root *this instance* owns, at
//!   `<data>/instances/<name>/claude`, used for every session that runs under one
//!   of the stored accounts. The token is injected per spawn, so a single managed
//!   root serves all accounts.
//!
//! The managed root is per-instance, and that is what makes the session list mean
//! something: a server's own sessions are the ones in its own root, so two
//! remote-hands on one machine cannot list — let alone resume — each other's.
//! Only these two roots are ever read. A sibling instance's directory is not
//! scanned, not watched and not searched by id: [`ClaudeRoot::ALL`] is exactly
//! "self and local", and nothing else enumerates roots.
//!
//! The TypeScript reads `homedir()` at module scope; here the roots hang off a
//! [`Paths`], which resolves both of them once at construction, so a test can
//! point a whole server at a synthetic tree without touching process-global
//! state. The `DROPPEN_CLAUDE_*_ROOT` environment seams also answer to their
//! pre-rename names, `REMOTE_HAND_CLAUDE_*_ROOT`, through [`crate::env`];
//! [`env_dir`] is the only place they — or any other directory override — are
//! read.

use std::path::PathBuf;

use gotcha::Schematic;
use serde::{Deserialize, Serialize};

use crate::sessions::Paths;

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

/// Test seam: point the user's own root at a scratch directory (before the
/// rename, `REMOTE_HAND_CLAUDE_LOCAL_ROOT`, still read).
pub const LOCAL_ROOT_ENV: &str = "DROPPEN_CLAUDE_LOCAL_ROOT";
/// Test seam: point the managed root at a scratch directory (before the
/// rename, `REMOTE_HAND_CLAUDE_MANAGED_ROOT`, still read).
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
    // `process.env.X || fallback` in JavaScript treats "" as absent.
    value.filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The directory a root lives in, as `paths` resolved it when it was built.
pub fn root_dir(paths: &Paths, root: ClaudeRoot) -> PathBuf {
    paths.claude_root(root)
}

/// `<root>/projects` — one directory per project, each holding `<uuid>.jsonl`.
pub fn projects_dir(paths: &Paths, root: ClaudeRoot) -> PathBuf {
    root_dir(paths, root).join("projects")
}

/// `<root>/sessions` — where a running CLI registers its pid.
pub fn live_dir(paths: &Paths, root: ClaudeRoot) -> PathBuf {
    root_dir(paths, root).join("sessions")
}

/// Create the managed root if it is missing, and return it.
pub async fn ensure_managed_root(paths: &Paths) -> std::io::Result<PathBuf> {
    let dir = root_dir(paths, ClaudeRoot::Managed);
    tokio::fs::create_dir_all(&dir).await?;
    Ok(dir)
}

/// What the boot-time claim of the pre-instance managed root did.
///
/// Returned rather than logged-and-forgotten so a test can assert the decision
/// instead of a directory listing that three different outcomes could produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyMigration {
    /// There was no `<data>/claude` to claim.
    Nothing,
    /// `<data>/claude` became this instance's own root.
    Claimed,
    /// A non-default instance found one and deliberately left it alone.
    NotOurs,
    /// Both directories exist; neither is touched.
    BothPresent,
    /// The rename failed. The server carries on with an empty root.
    Failed,
}

/// Claim the pre-instance managed root for the default instance, once.
///
/// Before ownership, every account-backed session of every instance lived in one
/// shared `<data>/claude`. That directory is now one instance's own root, and
/// the only defensible answer to "whose?" is the instance a single-server
/// machine has always been running: `default`. So `default` renames it into
/// place, and any other instance leaves it exactly where it is — a name it never
/// owned is not a name it may take.
///
/// Idempotent by construction: the rename only happens when the destination does
/// not exist, so the second boot finds `LegacyMigration::BothPresent` at worst
/// and `Nothing` in the ordinary case. A rename rather than a copy, because two
/// roots holding the same session ids is precisely the duplicate-id confusion
/// the ownership model exists to remove.
///
/// **Must only be called while holding the instance lock.** It moves a directory
/// two servers could otherwise both be reading.
pub fn migrate_legacy_managed_root(paths: &Paths) -> LegacyMigration {
    let legacy = paths.legacy_claude_managed();
    if !legacy.is_dir() {
        return LegacyMigration::Nothing;
    }
    let owned = root_dir(paths, ClaudeRoot::Managed);
    // An override points the managed root somewhere of the operator's choosing;
    // moving the legacy tree into it would be a surprise, and `legacy == owned`
    // (an override that names `<data>/claude`) would be a rename onto itself.
    if owned == legacy {
        return LegacyMigration::Nothing;
    }
    if paths.instance() != crate::instance::DEFAULT_INSTANCE {
        tracing::warn!(
            legacy = %legacy.display(),
            instance = paths.instance(),
            "found the pre-instance Claude root; leaving it for the `default` instance to claim"
        );
        return LegacyMigration::NotOurs;
    }
    if owned.exists() {
        tracing::warn!(
            legacy = %legacy.display(),
            owned = %owned.display(),
            "both the pre-instance Claude root and this instance's own exist; leaving both alone. \
             Move what you want to keep by hand — sessions under the old one are no longer listed."
        );
        return LegacyMigration::BothPresent;
    }
    if let Some(parent) = owned.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            tracing::error!(path = %parent.display(), %error, "cannot create the instance directory for the Claude root");
            return LegacyMigration::Failed;
        }
    }
    match std::fs::rename(&legacy, &owned) {
        Ok(()) => {
            tracing::warn!(
                from = %legacy.display(),
                to = %owned.display(),
                "the Claude root Droppen owns is now per-instance; moved the existing one into the `default` instance"
            );
            LegacyMigration::Claimed
        }
        Err(error) => {
            tracing::error!(
                from = %legacy.display(),
                to = %owned.display(),
                %error,
                "cannot move the pre-instance Claude root; account-backed sessions under it will not be listed"
            );
            LegacyMigration::Failed
        }
    }
}

/// The managed root's `.credentials.json`, when there is one.
///
/// This used to be a refusal: older CLIs let file credentials outrank the
/// environment, so a token injected next to a file would have billed whichever
/// login was on disk. claude 2.1.266 (checked 2026-09-23 with a bogus file next
/// to a valid `CLAUDE_CODE_OAUTH_TOKEN`) uses the environment, so the file is
/// worth a log line and nothing more — refusing only made the root's own
/// login and account sessions mutually exclusive.
pub async fn managed_root_file_credentials(paths: &Paths) -> Option<PathBuf> {
    let path = root_dir(paths, ClaudeRoot::Managed).join(".credentials.json");
    tokio::fs::metadata(&path).await.ok().map(|_| path)
}

/// The local root's `.credentials.json`, read only for the plan label.
pub fn local_credentials_path(paths: &Paths) -> PathBuf {
    root_dir(paths, ClaudeRoot::Local).join(".credentials.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> Paths {
        Paths::new(PathBuf::from("/home/me"))
    }

    #[test]
    fn the_roots_default_under_the_home_directory() {
        let paths = paths();
        assert_eq!(root_dir(&paths, ClaudeRoot::Local), PathBuf::from("/home/me/.claude"));
        assert_eq!(
            root_dir(&paths, ClaudeRoot::Managed),
            PathBuf::from("/home/me/.local/share/remote-hand/instances/default/claude"),
            "the managed root lives inside the directory this instance owns"
        );
        assert_eq!(projects_dir(&paths, ClaudeRoot::Local), PathBuf::from("/home/me/.claude/projects"));
        assert_eq!(live_dir(&paths, ClaudeRoot::Local), PathBuf::from("/home/me/.claude/sessions"));
    }

    /// The point of the whole change: two servers on one machine have disjoint
    /// managed roots, so neither can list, resume or watch the other's sessions.
    #[test]
    fn two_instances_have_disjoint_managed_roots() {
        let a = Paths::for_instance(PathBuf::from("/home/me"), "a");
        let b = Paths::for_instance(PathBuf::from("/home/me"), "b");
        assert_ne!(root_dir(&a, ClaudeRoot::Managed), root_dir(&b, ClaudeRoot::Managed));
        assert_eq!(
            root_dir(&a, ClaudeRoot::Local),
            root_dir(&b, ClaudeRoot::Local),
            "the user's own root is shared: it is external to both"
        );
    }

    /// The layout is spelled once, in `crate::instance`; `Paths` must agree with
    /// the directory the lock and the shims actually live in.
    #[test]
    fn the_paths_and_the_instance_dirs_agree_on_the_root() {
        let paths = Paths::for_instance(PathBuf::from("/home/me"), "ci-1");
        let dirs = crate::instance::InstanceDirs::new(&paths.data_dir(), "ci-1").expect("valid name");
        assert_eq!(paths.instance_dir(), dirs.root());
    }

    #[test]
    fn the_default_instance_claims_the_pre_instance_root_exactly_once() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        let legacy = paths.legacy_claude_managed();
        std::fs::create_dir_all(legacy.join("projects").join("-w")).expect("legacy tree");
        std::fs::write(legacy.join("projects").join("-w").join("s.jsonl"), "{}\n").expect("legacy session");

        assert_eq!(migrate_legacy_managed_root(&paths), LegacyMigration::Claimed);
        let owned = root_dir(&paths, ClaudeRoot::Managed);
        assert!(owned.join("projects").join("-w").join("s.jsonl").is_file(), "the sessions came with it");
        assert!(!legacy.exists(), "and the old name is gone, so no id lives in two roots");

        // Every later boot finds nothing to do.
        assert_eq!(migrate_legacy_managed_root(&paths), LegacyMigration::Nothing);
    }

    #[test]
    fn a_named_instance_never_takes_the_pre_instance_root() {
        let home = tempfile::tempdir().expect("tempdir");
        let named = Paths::for_instance(home.path().to_path_buf(), "b");
        let legacy = named.legacy_claude_managed();
        std::fs::create_dir_all(&legacy).expect("legacy tree");

        assert_eq!(migrate_legacy_managed_root(&named), LegacyMigration::NotOurs);
        assert!(legacy.is_dir(), "it is left for `default`");
        assert!(!root_dir(&named, ClaudeRoot::Managed).exists());

        // And `default` can still claim it afterwards.
        let default = Paths::new(home.path().to_path_buf());
        assert_eq!(migrate_legacy_managed_root(&default), LegacyMigration::Claimed);
    }

    #[test]
    fn a_root_that_already_exists_is_never_overwritten() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        std::fs::create_dir_all(paths.legacy_claude_managed()).expect("legacy tree");
        std::fs::create_dir_all(root_dir(&paths, ClaudeRoot::Managed)).expect("own tree");

        assert_eq!(migrate_legacy_managed_root(&paths), LegacyMigration::BothPresent);
        assert!(paths.legacy_claude_managed().is_dir(), "nothing is destroyed on this path");
    }

    #[test]
    fn an_empty_override_is_no_override() {
        assert_eq!(override_dir(None), None);
        assert_eq!(override_dir(Some("")), None, "`process.env.X || fallback` treats \"\" as absent");
        assert_eq!(override_dir(Some("/scratch")), Some(PathBuf::from("/scratch")));
    }
}
