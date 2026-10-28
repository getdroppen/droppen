//! The two Claude config roots remote-hand knows about.
//!
//! Claude Code keeps everything (projects, live-session registrations,
//! credentials) under one config root, chosen by `CLAUDE_CONFIG_DIR`.
//! remote-hand knows two of them:
//!
//! * **local** — `~/.claude`, the user's own login. Never touched by the account
//!   machinery: sessions there spawn with the user's own login. Everything in
//!   it is **external** to this server — browsable, but not its own.
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
//! The roots hang off a [`Paths`], which resolves both of them once at
//! construction, so a test can point a whole server at a synthetic tree without
//! touching process-global state. The `DROPPEN_CLAUDE_*_ROOT` environment seams
//! also answer to their pre-rename names, `REMOTE_HAND_CLAUDE_*_ROOT`, through
//! [`crate::env`]; [`crate::paths::env_dir`] is the only place they — or any
//! other directory override — are read.

use std::path::PathBuf;

use crate::paths::{ClaudeRoot, Paths};

/// The two roots of one instance, as its [`Paths`] resolved them when it was
/// built, and where Claude Code keeps things under them.
///
/// Its own type rather than functions of [`Paths`], so that the layout under a
/// root is this module's to say; [`Paths::claude_root`] stays where a root
/// itself is resolved.
#[derive(Debug, Clone)]
pub struct ClaudeRoots {
    local: PathBuf,
    managed: PathBuf,
}

impl ClaudeRoots {
    /// The roots of the instance `paths` names.
    pub fn of(paths: &Paths) -> Self {
        Self {
            local: paths.claude_root(ClaudeRoot::Local),
            managed: paths.claude_root(ClaudeRoot::Managed),
        }
    }

    /// The directory a root lives in.
    pub fn dir(&self, root: ClaudeRoot) -> PathBuf {
        match root {
            ClaudeRoot::Local => self.local.clone(),
            ClaudeRoot::Managed => self.managed.clone(),
        }
    }

    /// `<root>/projects` — one directory per project, each holding `<uuid>.jsonl`.
    pub fn projects_dir(&self, root: ClaudeRoot) -> PathBuf {
        self.dir(root).join("projects")
    }

    /// `~/.claude/projects` — one directory per project, each holding `<uuid>.jsonl`.
    ///
    /// The *local* root only; [`ClaudeRoots::managed_projects`] is the managed one and
    /// [`ClaudeRoots::projects_dirs`] the fan-out over both.
    pub fn local_projects(&self) -> PathBuf {
        self.projects_dir(ClaudeRoot::Local)
    }

    /// The projects directory of the root remote-hand owns for account sessions.
    pub fn managed_projects(&self) -> PathBuf {
        self.projects_dir(ClaudeRoot::Managed)
    }

    /// Every projects directory a Claude session can live in, local first.
    pub fn projects_dirs(&self) -> [(ClaudeRoot, PathBuf); 2] {
        ClaudeRoot::ALL.map(|root| (root, self.projects_dir(root)))
    }

    /// `<root>/sessions` — where a running CLI registers its pid.
    pub fn live_dir(&self, root: ClaudeRoot) -> PathBuf {
        self.dir(root).join("sessions")
    }

    /// Create the managed root if it is missing, and return it.
    pub async fn ensure_managed(&self) -> std::io::Result<PathBuf> {
        let dir = self.dir(ClaudeRoot::Managed);
        tokio::fs::create_dir_all(&dir).await?;
        Ok(dir)
    }
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
/// A shared `<data>/claude` may still hold every instance's account-backed
/// sessions from builds before roots were per-instance. It is now one
/// instance's own root, and the only defensible answer to "whose?" is the
/// instance a single-server machine has always been running: `default`. So
/// `default` renames it into place, and any other instance leaves it
/// exactly where it is — a name it never owned is not a name it may take.
///
/// Idempotent by construction: the rename only happens when the destination
/// does not exist, so the second boot finds `LegacyMigration::BothPresent`
/// at worst and `Nothing` in the ordinary case. A rename rather than a
/// copy, because two roots holding the same session ids is precisely the
/// duplicate-id confusion the ownership model exists to remove.
///
/// **Must only be called while holding the instance lock.** It moves a
/// directory two servers could otherwise both be reading.
pub fn migrate_legacy_root(paths: &Paths) -> LegacyMigration {
    let legacy = paths.legacy_claude_managed();
    if !legacy.is_dir() {
        return LegacyMigration::Nothing;
    }
    let owned = ClaudeRoots::of(paths).dir(ClaudeRoot::Managed);
    // An override points the managed root somewhere of the operator's
    // choosing; moving the legacy tree into it would be a surprise, and
    // `legacy == owned` (an override that names `<data>/claude`) would be a
    // rename onto itself.
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

impl ClaudeRoots {
    /// The managed root's `.credentials.json`, when there is one.
    ///
    /// Not a refusal: claude 2.1.266 reads the environment rather than the file
    /// (verified with a bogus file next to a valid `CLAUDE_CODE_OAUTH_TOKEN`), so
    /// the file is worth a log line and nothing more. An older claude lets the file
    /// outrank the injected token, so the log line names it. Refusing would only
    /// make the root's own login and account sessions mutually exclusive.
    pub async fn managed_file_credentials(&self) -> Option<PathBuf> {
        let path = self.dir(ClaudeRoot::Managed).join(".credentials.json");
        tokio::fs::metadata(&path).await.ok().map(|_| path)
    }

    /// The local root's `.credentials.json`, read only for the plan label.
    pub fn local_credentials(&self) -> PathBuf {
        self.dir(ClaudeRoot::Local).join(".credentials.json")
    }
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
        assert_eq!(ClaudeRoots::of(&paths).dir(ClaudeRoot::Local), PathBuf::from("/home/me/.claude"));
        assert_eq!(
            ClaudeRoots::of(&paths).dir(ClaudeRoot::Managed),
            PathBuf::from("/home/me/.local/share/remote-hand/instances/default/claude"),
            "the managed root lives inside the directory this instance owns"
        );
        assert_eq!(ClaudeRoots::of(&paths).projects_dir(ClaudeRoot::Local), PathBuf::from("/home/me/.claude/projects"));
        assert_eq!(ClaudeRoots::of(&paths).live_dir(ClaudeRoot::Local), PathBuf::from("/home/me/.claude/sessions"));
    }

    /// The point of the whole change: two servers on one machine have disjoint
    /// managed roots, so neither can list, resume or watch the other's sessions.
    #[test]
    fn two_instances_have_disjoint_managed_roots() {
        let a = Paths::for_instance(PathBuf::from("/home/me"), "a");
        let b = Paths::for_instance(PathBuf::from("/home/me"), "b");
        assert_ne!(ClaudeRoots::of(&a).dir(ClaudeRoot::Managed), ClaudeRoots::of(&b).dir(ClaudeRoot::Managed));
        assert_eq!(
            ClaudeRoots::of(&a).dir(ClaudeRoot::Local),
            ClaudeRoots::of(&b).dir(ClaudeRoot::Local),
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

        assert_eq!(migrate_legacy_root(&paths), LegacyMigration::Claimed);
        let owned = ClaudeRoots::of(&paths).dir(ClaudeRoot::Managed);
        assert!(owned.join("projects").join("-w").join("s.jsonl").is_file(), "the sessions came with it");
        assert!(!legacy.exists(), "and the old name is gone, so no id lives in two roots");

        // Every later boot finds nothing to do.
        assert_eq!(migrate_legacy_root(&paths), LegacyMigration::Nothing);
    }

    #[test]
    fn a_named_instance_never_takes_the_pre_instance_root() {
        let home = tempfile::tempdir().expect("tempdir");
        let named = Paths::for_instance(home.path().to_path_buf(), "b");
        let legacy = named.legacy_claude_managed();
        std::fs::create_dir_all(&legacy).expect("legacy tree");

        assert_eq!(migrate_legacy_root(&named), LegacyMigration::NotOurs);
        assert!(legacy.is_dir(), "it is left for `default`");
        assert!(!ClaudeRoots::of(&named).dir(ClaudeRoot::Managed).exists());

        // And `default` can still claim it afterwards.
        let default = Paths::new(home.path().to_path_buf());
        assert_eq!(migrate_legacy_root(&default), LegacyMigration::Claimed);
    }

    #[test]
    fn a_root_that_already_exists_is_never_overwritten() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        std::fs::create_dir_all(paths.legacy_claude_managed()).expect("legacy tree");
        std::fs::create_dir_all(ClaudeRoots::of(&paths).dir(ClaudeRoot::Managed)).expect("own tree");

        assert_eq!(migrate_legacy_root(&paths), LegacyMigration::BothPresent);
        assert!(paths.legacy_claude_managed().is_dir(), "nothing is destroyed on this path");
    }
}
