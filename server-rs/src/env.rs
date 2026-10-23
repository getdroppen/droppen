//! The server's environment variables, under the names they have had since the
//! rename to Droppen.
//!
//! Before the rename they were `REMOTE_HAND_*` and `RH_*`; each is `DROPPEN_*`
//! now ([`RENAMED`] lists every one with its old name). The server reads them
//! through [`var`], which takes the new name and, when that is unset, the old
//! one — so a launchd plist, a unit or a shell profile written before the
//! rename keeps working:
//!
//! * a value is trimmed, and an empty one is unset, as it always was;
//! * only the old name is set: its value is used, and the server says once that
//!   the old name is deprecated — add the new name with the same value, and
//!   keep the old one while a rollback to a version from before the rename is
//!   possible (such a version reads only the old name);
//! * both are set, to different values: the new one is used, and that is said
//!   once; to the same value (the switch-over), nothing is said.
//!
//! Each old name is mentioned at most once per process, and by name only —
//! never a value, which may be a secret or a path with the account's name in
//! it. The line goes to the log once there is one (`serve`), else to stderr;
//! never to stdout, where `user list`, `setup-code` and `--version` print the
//! answer a script reads.
//!
//! The variables one Droppen program sets for another ([`Kind::Handoff`]: `up`
//! for the server it runs, the desktop app for its server, a server for a
//! session's `droppen-notify`) are written under both names ([`with_old_name`]),
//! because either side may be the older program — an `up` from before a
//! self-upgrade runs every server after it — and read under both without a
//! word: an old name there was set by an older Droppen, not by anybody's
//! configuration, so there is nothing for a person to rename. When the two
//! differ, the old name wins: only a program from before the rename writes
//! one without the other, over a new name it inherited from further out.
//!
//! A program that may start an *older* server — `up`, which runs `.prev`
//! after a rollback — also hands it every setting it was given under a new
//! name under the old one ([`settings_under_old_names`]): a server from before
//! the rename would otherwise miss `DROPPEN_DATA_DIR` and run on the default
//! install's files.

use std::collections::BTreeSet;
use std::sync::{Mutex, PoisonError};

/// Who sets a renamed variable, which decides whether its old name is worth a
/// word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A person or a deployment (a plist, a unit, a shell profile, a script):
    /// using the old name says that it is deprecated.
    Setting,
    /// One Droppen program, for another it starts: written under both names,
    /// and the old one read silently.
    Handoff,
}

/// Every variable the server reads that had another name before the rename:
/// `(new, old, kind)`. `REMOTE_HAND_X` became `DROPPEN_X`, `RH_HUB_X`
/// `DROPPEN_HUB_X` and any other `RH_X` `DROPPEN_X`.
///
/// Not here, because they never had another name: `APP_*` (the framework's),
/// `DROPPEN_NOTIFY_AS`, `DROPPEN_ABSORB_POLL_MS`, `DROPPEN_DESKTOP_SHELL_READ`,
/// `DROPPEN_ALLOWED_HOSTS`.
/// Not here either, because their old names are gone: `DROPPEN_CODEX_API_KEY`
/// (the server sets it and names it in the same spawn of `codex`), and the
/// names only a test reads.
pub const RENAMED: &[(&str, &str, Kind)] = &[
    // Where this server keeps its files, and the Claude roots it reads
    // (`crate::sessions::Paths`).
    ("DROPPEN_DATA_DIR", "REMOTE_HAND_DATA_DIR", Kind::Setting),
    ("DROPPEN_CONFIG_DIR", "REMOTE_HAND_CONFIG_DIR", Kind::Setting),
    ("DROPPEN_CLAUDE_LOCAL_ROOT", "REMOTE_HAND_CLAUDE_LOCAL_ROOT", Kind::Setting),
    ("DROPPEN_CLAUDE_MANAGED_ROOT", "REMOTE_HAND_CLAUDE_MANAGED_ROOT", Kind::Setting),
    ("DROPPEN_SHIM_BIN", "REMOTE_HAND_SHIM_BIN", Kind::Setting),
    // Push and notifications.
    ("DROPPEN_PUSH_CONTACT", "REMOTE_HAND_PUSH_CONTACT", Kind::Setting),
    ("DROPPEN_PUSH_ALLOW_INSECURE_ENDPOINT", "RH_PUSH_ALLOW_INSECURE_ENDPOINT", Kind::Setting),
    ("DROPPEN_NOTIFY_ALLOW_INSECURE_ENDPOINT", "RH_NOTIFY_ALLOW_INSECURE_ENDPOINT", Kind::Setting),
    // Requests.
    ("DROPPEN_ALLOWED_ORIGINS", "RH_ALLOWED_ORIGINS", Kind::Setting),
    // Session recordings (`crate::record`).
    ("DROPPEN_SESSION_LOG", "RH_SESSION_LOG", Kind::Setting),
    ("DROPPEN_SESSION_LOG_MAX_MB", "RH_SESSION_LOG_MAX_MB", Kind::Setting),
    // Driving the CLIs (`crate::inject`), and accounts.
    ("DROPPEN_IDLE_TIMEOUT_MS", "RH_IDLE_TIMEOUT_MS", Kind::Setting),
    ("DROPPEN_STALL_MS", "RH_STALL_MS", Kind::Setting),
    ("DROPPEN_INTERRUPT_STEP_MS", "RH_INTERRUPT_STEP_MS", Kind::Setting),
    ("DROPPEN_WATCHDOG_INTERVAL_MS", "RH_WATCHDOG_INTERVAL_MS", Kind::Setting),
    ("DROPPEN_AWAIT_BACKGROUND_MS", "RH_AWAIT_BACKGROUND_MS", Kind::Setting),
    ("DROPPEN_RESIDENT_IDLE_MS", "RH_RESIDENT_IDLE_MS", Kind::Setting),
    ("DROPPEN_STEER_SETTLE_MS", "RH_STEER_SETTLE_MS", Kind::Setting),
    ("DROPPEN_TASK_ROOT", "RH_TASK_ROOT", Kind::Setting),
    ("DROPPEN_ROTATE_AHEAD_PERCENT", "RH_ROTATE_AHEAD_PERCENT", Kind::Setting),
    ("DROPPEN_USAGE_FALLBACK_MS", "RH_USAGE_FALLBACK_MS", Kind::Setting),
    // The cloud agent and the hub it signs in to.
    ("DROPPEN_HUB_DEFAULT", "RH_HUB_DEFAULT", Kind::Setting),
    ("DROPPEN_HUB_API_TRAFFIC", "RH_HUB_API_TRAFFIC", Kind::Setting),
    ("DROPPEN_AGENT_CONTROL", "RH_AGENT_CONTROL", Kind::Setting),
    ("DROPPEN_AGENT_LINK", "RH_AGENT_LINK", Kind::Setting),
    ("DROPPEN_AGENT_DIRECT", "RH_AGENT_DIRECT", Kind::Setting),
    ("DROPPEN_AGENT_DIRECT_STUN", "RH_AGENT_DIRECT_STUN", Kind::Setting),
    ("DROPPEN_AGENT_RELAYS", "RH_AGENT_RELAYS", Kind::Setting),
    ("DROPPEN_AGENT_RELAY_INSECURE", "RH_AGENT_RELAY_INSECURE", Kind::Setting),
    ("DROPPEN_AGENT_MIGRATE_WITHIN_SECS", "RH_AGENT_MIGRATE_WITHIN_SECS", Kind::Setting),
    ("DROPPEN_AGENT_STATUS_CLI", "RH_AGENT_STATUS_CLI", Kind::Setting),
    ("DROPPEN_AGENT_SYNC_DAYS", "RH_AGENT_SYNC_DAYS", Kind::Setting),
    ("DROPPEN_AGENT_SYNC_EXTERNAL", "RH_AGENT_SYNC_EXTERNAL", Kind::Setting),
    ("DROPPEN_AGENT_COMPACT_RECORDS", "RH_AGENT_COMPACT_RECORDS", Kind::Setting),
    ("DROPPEN_AGENT_COMPACT_EVERY_SECS", "RH_AGENT_COMPACT_EVERY_SECS", Kind::Setting),
    ("DROPPEN_AGENT_COMPACT_IDLE_SECS", "RH_AGENT_COMPACT_IDLE_SECS", Kind::Setting),
    ("DROPPEN_AGENT_COMPACT_OLDEST_SECS", "RH_AGENT_COMPACT_OLDEST_SECS", Kind::Setting),
    // One program to another.
    ("DROPPEN_SUPERVISED", "RH_SUPERVISED", Kind::Handoff),
    ("DROPPEN_DESKTOP_APP", "RH_DESKTOP_APP", Kind::Handoff),
    ("DROPPEN_NOTIFY_URL", "RH_NOTIFY_URL", Kind::Handoff),
    ("DROPPEN_NOTIFY_TOKEN", "RH_NOTIFY_TOKEN", Kind::Handoff),
];

/// The name `name` had before the rename, and who sets it; `None` for a name
/// that never had another.
fn renamed(name: &str) -> Option<(&'static str, Kind)> {
    RENAMED.iter().find(|(new, _, _)| *new == name).map(|(_, old, kind)| (*old, *kind))
}

/// The name `name` had before the rename to Droppen, if it had one.
pub fn old_name(name: &str) -> Option<&'static str> {
    renamed(name).map(|(old, _)| old)
}

/// `name` and its old name, for a program that sets the variable for another
/// it starts ([`Kind::Handoff`]): an older one reads only the old name.
pub fn with_old_name(name: &'static str) -> impl Iterator<Item = &'static str> {
    std::iter::once(name).chain(old_name(name))
}

/// The renamed settings whose empty value is a value ([`var_or_empty`]).
const EMPTY_IS_A_VALUE: &[&str] = &["DROPPEN_AGENT_DIRECT_STUN"];

/// Every setting ([`Kind::Setting`]) this process has under its new name,
/// under its old name with the same value: what `up` adds to the environment
/// of a server it starts, which may be one from before the rename (`.prev`,
/// after a rollback) that reads only the old names. Without them such a server
/// would run on the default install's data and configuration — another
/// instance's state, identity and binary store — and not on the ones this
/// process was pointed at. Nothing for a setting only the old name has (the
/// child inherits it as it is) or none has.
pub fn settings_under_old_names() -> Vec<(&'static str, String)> {
    settings_under_old_names_in(|n| std::env::var(n).ok())
}

/// [`settings_under_old_names`], reading through `read`.
pub fn settings_under_old_names_in(read: impl Fn(&str) -> Option<String>) -> Vec<(&'static str, String)> {
    RENAMED
        .iter()
        .filter(|(_, _, kind)| *kind == Kind::Setting)
        .filter_map(|(new, old, _)| {
            let value = read(new).map(|v| v.trim().to_string())?;
            (!value.is_empty() || EMPTY_IS_A_VALUE.contains(new)).then_some((*old, value))
        })
        .collect()
}

/// Whether `key` is `name` under either of its names.
pub fn is(key: &str, name: &str) -> bool {
    key == name || old_name(name) == Some(key)
}

/// The variable `name` (a `DROPPEN_*` name), trimmed, with an empty value
/// unset; its old name when it is unset. See the module documentation for
/// what is said about an old name.
pub fn var(name: &str) -> Option<String> {
    read(name, Empty::Unset)
}

/// [`var`] for the one variable whose empty value means something
/// (`DROPPEN_AGENT_DIRECT_STUN`: no STUN servers at all): set to anything,
/// the empty string included, is set.
pub fn var_or_empty(name: &str) -> Option<String> {
    debug_assert!(EMPTY_IS_A_VALUE.contains(&name), "{name}: an empty value is unset");
    read(name, Empty::Set)
}

/// What an empty value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Empty {
    /// Unset, like a variable that is not there ([`var`]).
    Unset,
    /// A value ([`var_or_empty`]).
    Set,
}

fn read(name: &str, empty: Empty) -> Option<String> {
    debug_assert!(name.starts_with("DROPPEN_"), "{name}: read a variable by its new name");
    let (value, old) = lookup(name, empty, |n| std::env::var(n).ok());
    if let Some(line) = old.and_then(|old| notice(&NOTICED, name, &old)) {
        say(&line);
    }
    value
}

/// The old names already mentioned in this process.
static NOTICED: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());

/// What a renamed variable's old name did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OldName {
    /// Only the old name is set: its value is used.
    Used(&'static str),
    /// Both are set, to different values: the new name's is used.
    Overridden(&'static str),
}

/// `name`'s value as [`var`] (or, with [`Empty::Set`], [`var_or_empty`])
/// reads it through `read`, and what its old name did. A new name set to the
/// old one's value says nothing: that is the switch-over, both set until the
/// old one is removed.
///
/// A handoff ([`Kind::Handoff`]) set to two different values takes the old
/// name's, and says nothing: the old name is the newer of the two, written by
/// a program from before the rename over a new name it inherited (a server of
/// that age started inside a newer one's session, say).
pub fn lookup(name: &str, empty: Empty, read: impl Fn(&str) -> Option<String>) -> (Option<String>, Option<OldName>) {
    let get = |n: &str| {
        read(n)
            .map(|v| v.trim().to_string())
            .filter(|v| empty == Empty::Set || !v.is_empty())
    };
    let value = get(name);
    let Some((old, kind)) = renamed(name) else { return (value, None) };
    match (value, get(old)) {
        (None, Some(was)) => (Some(was), Some(OldName::Used(old))),
        (Some(_), Some(was)) if kind == Kind::Handoff => (Some(was), None),
        (Some(value), Some(was)) if was != value => (Some(value), Some(OldName::Overridden(old))),
        (value, _) => (value, None),
    }
}

/// The line to say about `name`'s old name, the first time it comes up
/// (`noticed` remembers); `None` after, and for an old name a Droppen program
/// set ([`Kind::Handoff`]) that merely stood in for the new one.
fn notice(noticed: &Mutex<BTreeSet<&'static str>>, name: &str, old: &OldName) -> Option<String> {
    let (OldName::Used(was) | OldName::Overridden(was)) = *old;
    if matches!(old, OldName::Used(_)) && renamed(name).is_some_and(|(_, kind)| kind == Kind::Handoff) {
        return None;
    }
    if !noticed.lock().unwrap_or_else(PoisonError::into_inner).insert(was) {
        return None;
    }
    Some(match old {
        OldName::Used(_) => format!(
            "{was} is deprecated: add {name} with the same value (and keep {was} while a rollback to a version from before the rename is possible)"
        ),
        OldName::Overridden(_) => format!("{name} and the deprecated {was} are both set, to different values: {name} is used; remove {was}"),
    })
}

/// Say `line`: as a warning in the log once there is one (`serve` sets it up),
/// else on stderr, prefixed like this binary's errors. Never on stdout.
fn say(line: &str) {
    if tracing::dispatcher::has_been_set() {
        tracing::warn!("{line}");
    } else {
        eprintln!("{}: warning: {line}", crate::program::Name::invoked().error_prefix());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`lookup`] over just `vars`.
    fn lookup_in(vars: &[(&str, &str)], name: &str) -> (Option<String>, Option<OldName>) {
        lookup_with(vars, name, Empty::Unset)
    }

    fn lookup_with(vars: &[(&str, &str)], name: &str, empty: Empty) -> (Option<String>, Option<OldName>) {
        let vars: std::collections::HashMap<&str, &str> = vars.iter().copied().collect();
        lookup(name, empty, |n| vars.get(n).map(|v| v.to_string()))
    }

    #[test]
    fn every_new_name_is_its_old_name_under_droppen() {
        let mut seen = BTreeSet::new();
        for (new, old, _) in RENAMED {
            let rest = new.strip_prefix("DROPPEN_").unwrap_or_else(|| panic!("{new} is a DROPPEN_ name"));
            let was = old.strip_prefix("REMOTE_HAND_").or_else(|| old.strip_prefix("RH_"));
            assert_eq!(was, Some(rest), "{old} → {new}");
            assert!(seen.insert(*new) && seen.insert(*old), "{new} / {old} listed once");
        }
        assert_eq!(old_name("DROPPEN_DATA_DIR"), Some("REMOTE_HAND_DATA_DIR"));
        assert_eq!(old_name("DROPPEN_HUB_DEFAULT"), Some("RH_HUB_DEFAULT"));
        assert_eq!(old_name("DROPPEN_AGENT_LINK"), Some("RH_AGENT_LINK"));
        // Born under the new name: nothing to fall back to.
        assert_eq!(old_name("DROPPEN_NOTIFY_AS"), None);
        assert_eq!(old_name("DROPPEN_ABSORB_POLL_MS"), None);
    }

    /// The names `install.sh`, the release scripts and the desktop app already
    /// use for something else are not taken: `install.sh` runs the server with
    /// its own environment.
    #[test]
    fn no_new_name_is_one_already_in_use() {
        let taken = [
            "DROPPEN_CODE",
            "DROPPEN_HUB",
            "DROPPEN_PORT",
            "DROPPEN_USER",
            "DROPPEN_VERSION",
            "DROPPEN_LABEL",
            "DROPPEN_SERVICE_PATH",
            "DROPPEN_RELEASE_BASE",
            "DROPPEN_NO_OPEN",
            "DROPPEN_NOTIFY_AS",
            "DROPPEN_ABSORB_POLL_MS",
            "DROPPEN_DESKTOP_SHELL_READ",
            "DROPPEN_DESKTOP_KEYCHAIN",
            "DROPPEN_DESKTOP_TEST_CONTROL",
        ];
        for (new, _, _) in RENAMED {
            assert!(!taken.contains(new), "{new} is already used for something else");
        }
    }

    #[test]
    fn the_new_name_wins_over_the_old_one() {
        let both = [("DROPPEN_DATA_DIR", "/new"), ("REMOTE_HAND_DATA_DIR", "/old")];
        assert_eq!(
            lookup_in(&both, "DROPPEN_DATA_DIR"),
            (Some("/new".into()), Some(OldName::Overridden("REMOTE_HAND_DATA_DIR")))
        );
        // The switch-over: both set to one value, nothing to say.
        let same = [("DROPPEN_DATA_DIR", "/d"), ("REMOTE_HAND_DATA_DIR", " /d ")];
        assert_eq!(lookup_in(&same, "DROPPEN_DATA_DIR"), (Some("/d".into()), None));
        assert_eq!(lookup_in(&[("DROPPEN_AGENT_LINK", "0")], "DROPPEN_AGENT_LINK"), (Some("0".into()), None));
    }

    #[test]
    fn the_old_name_still_works() {
        assert_eq!(
            lookup_in(&[("RH_SESSION_LOG", " 0\n")], "DROPPEN_SESSION_LOG"),
            (Some("0".into()), Some(OldName::Used("RH_SESSION_LOG")))
        );
        // An empty new name is unset, as an empty variable always was.
        assert_eq!(
            lookup_in(&[("DROPPEN_HUB_DEFAULT", " "), ("RH_HUB_DEFAULT", "http://h")], "DROPPEN_HUB_DEFAULT"),
            (Some("http://h".into()), Some(OldName::Used("RH_HUB_DEFAULT")))
        );
    }

    #[test]
    fn with_neither_name_set_it_is_unset() {
        assert_eq!(lookup_in(&[], "DROPPEN_STALL_MS"), (None, None));
        assert_eq!(lookup_in(&[("RH_STALL_MS", ""), ("DROPPEN_STALL_MS", "")], "DROPPEN_STALL_MS"), (None, None));
        // Names that were never renamed have nothing to fall back to.
        assert_eq!(lookup_in(&[("RH_ABSORB_POLL_MS", "5")], "DROPPEN_ABSORB_POLL_MS"), (None, None));
        assert_eq!(lookup_in(&[("DROPPEN_ABSORB_POLL_MS", "5")], "DROPPEN_ABSORB_POLL_MS"), (Some("5".into()), None));
    }

    #[test]
    fn an_empty_value_is_a_value_where_it_means_something() {
        let stun = "DROPPEN_AGENT_DIRECT_STUN";
        assert_eq!(lookup_with(&[(stun, "")], stun, Empty::Set), (Some(String::new()), None));
        assert_eq!(
            lookup_with(&[("RH_AGENT_DIRECT_STUN", "")], stun, Empty::Set),
            (Some(String::new()), Some(OldName::Used("RH_AGENT_DIRECT_STUN")))
        );
        // The new name, even empty, wins.
        assert_eq!(
            lookup_with(&[(stun, ""), ("RH_AGENT_DIRECT_STUN", "stun:a:1")], stun, Empty::Set),
            (Some(String::new()), Some(OldName::Overridden("RH_AGENT_DIRECT_STUN")))
        );
        assert_eq!(lookup_with(&[], stun, Empty::Set), (None, None));
    }

    #[test]
    fn each_old_name_is_said_once_and_never_its_value() {
        let noticed = Mutex::new(BTreeSet::new());
        let used = OldName::Used("REMOTE_HAND_DATA_DIR");
        let line = notice(&noticed, "DROPPEN_DATA_DIR", &used).expect("the first time");
        assert_eq!(
            line,
            "REMOTE_HAND_DATA_DIR is deprecated: add DROPPEN_DATA_DIR with the same value (and keep REMOTE_HAND_DATA_DIR while a rollback to a version from before the rename is possible)"
        );
        assert_eq!(notice(&noticed, "DROPPEN_DATA_DIR", &used), None, "once");
        let line = notice(&noticed, "DROPPEN_STALL_MS", &OldName::Overridden("RH_STALL_MS")).expect("another name");
        assert_eq!(
            line,
            "DROPPEN_STALL_MS and the deprecated RH_STALL_MS are both set, to different values: DROPPEN_STALL_MS is used; remove RH_STALL_MS"
        );
        assert_eq!(notice(&noticed, "DROPPEN_STALL_MS", &OldName::Overridden("RH_STALL_MS")), None);
    }

    /// An old name an older Droppen set for this process (an `up` from before
    /// a self-upgrade, a session started by an older server) is nobody's to
    /// rename, so nothing is said about it.
    #[test]
    fn a_handoff_under_its_old_name_says_nothing() {
        let noticed = Mutex::new(BTreeSet::new());
        assert_eq!(notice(&noticed, "DROPPEN_SUPERVISED", &OldName::Used("RH_SUPERVISED")), None);
        assert_eq!(notice(&noticed, "DROPPEN_NOTIFY_TOKEN", &OldName::Used("RH_NOTIFY_TOKEN")), None);
        assert_eq!(
            lookup_in(&[("RH_NOTIFY_URL", "http://127.0.0.1:1/api/notify")], "DROPPEN_NOTIFY_URL"),
            (Some("http://127.0.0.1:1/api/notify".into()), Some(OldName::Used("RH_NOTIFY_URL")))
        );
    }

    /// Two different values of a handoff: the old name was written last, by a
    /// program from before the rename (a server that old, started in a newer
    /// one's session, over the session's own channel), so it wins, quietly.
    #[test]
    fn a_handoff_set_twice_takes_the_old_name_which_is_the_newer() {
        let inner = [("DROPPEN_NOTIFY_URL", "http://outer/api/notify"), ("RH_NOTIFY_URL", "http://inner/api/notify")];
        assert_eq!(lookup_in(&inner, "DROPPEN_NOTIFY_URL"), (Some("http://inner/api/notify".into()), None));
        let same = [("DROPPEN_SUPERVISED", "1"), ("RH_SUPERVISED", "1")];
        assert_eq!(lookup_in(&same, "DROPPEN_SUPERVISED"), (Some("1".into()), None));
        // A setting is the other way round: the new name is the person's latest word.
        let setting = [("DROPPEN_STALL_MS", "5"), ("RH_STALL_MS", "9")];
        assert_eq!(lookup_in(&setting, "DROPPEN_STALL_MS"), (Some("5".into()), Some(OldName::Overridden("RH_STALL_MS"))));
    }

    /// What `up` adds for a server that may be from before the rename: each
    /// setting it has under a new name, under the old one, with its value.
    #[test]
    fn an_older_child_gets_each_setting_under_its_old_name() {
        let vars: std::collections::HashMap<&str, &str> = [
            ("DROPPEN_DATA_DIR", " /bob/data "),
            ("DROPPEN_CONFIG_DIR", "/bob/config"),
            // Both set to different values: the child reads what this one does.
            ("DROPPEN_SESSION_LOG", "0"),
            ("RH_SESSION_LOG", "1"),
            // Only the old name: inherited as it is, nothing to add.
            ("RH_STALL_MS", "5"),
            // Empty: unset, except where empty is a value.
            ("DROPPEN_HUB_DEFAULT", ""),
            ("DROPPEN_AGENT_DIRECT_STUN", ""),
            // Handoffs are the supervisor's own business.
            ("DROPPEN_NOTIFY_URL", "http://x"),
        ]
        .into_iter()
        .collect();
        let added = settings_under_old_names_in(|n| vars.get(n).map(|v| v.to_string()));
        assert_eq!(
            added,
            vec![
                ("REMOTE_HAND_DATA_DIR", "/bob/data".to_string()),
                ("REMOTE_HAND_CONFIG_DIR", "/bob/config".to_string()),
                ("RH_SESSION_LOG", "0".to_string()),
                ("RH_AGENT_DIRECT_STUN", String::new()),
            ]
        );
    }

    #[test]
    fn a_handoff_is_written_and_recognised_under_both_names() {
        assert_eq!(with_old_name("DROPPEN_SUPERVISED").collect::<Vec<_>>(), ["DROPPEN_SUPERVISED", "RH_SUPERVISED"]);
        assert_eq!(with_old_name("DROPPEN_NOTIFY_AS").collect::<Vec<_>>(), ["DROPPEN_NOTIFY_AS"]);
        assert!(is("RH_DESKTOP_APP", "DROPPEN_DESKTOP_APP") && is("DROPPEN_DESKTOP_APP", "DROPPEN_DESKTOP_APP"));
        assert!(!is("RH_DESKTOP", "DROPPEN_DESKTOP_APP"));
        for (new, _, kind) in RENAMED {
            let handoff = ["DROPPEN_SUPERVISED", "DROPPEN_DESKTOP_APP", "DROPPEN_NOTIFY_URL", "DROPPEN_NOTIFY_TOKEN"].contains(new);
            assert_eq!(*kind == Kind::Handoff, handoff, "{new}");
        }
    }

    /// Every read of a renamed variable goes through this module: no source
    /// file but this one names an old `RH_*` or `REMOTE_HAND_*` variable in a
    /// string of its own.
    #[test]
    fn nothing_else_reads_an_old_name() {
        // A name no code reads: a terminal test's stand-in for "any variable".
        const ALLOWED: &[&str] = &["RH_TERMINAL_KEEP"];
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        let mut dirs = vec![src.clone()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).expect("read src").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "rs") || path == src.join("env.rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("read a source file");
                for (number, line) in text.lines().enumerate() {
                    for prefix in ["\"RH_", "\"REMOTE_HAND_"] {
                        for (at, _) in line.match_indices(prefix) {
                            let word: String = line[at + 1..].chars().take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_').collect();
                            if !ALLOWED.contains(&word.as_str()) {
                                found.push(format!("{}:{}: {word}", path.display(), number + 1));
                            }
                        }
                    }
                }
            }
        }
        assert!(found.is_empty(), "read these through crate::env::var under their DROPPEN_ names:\n{}", found.join("\n"));
    }
}
