//! The server's environment variables, each read by its `DROPPEN_*` name:
//! trimmed, and an empty value unset ([`var`]), except where an empty value
//! means something ([`var_or_empty`]).
//!
//! The names from before the rename to Droppen (`REMOTE_HAND_*`, `RH_*`) are
//! neither read nor set for the programs the server starts: every supported
//! Droppen program reads the `DROPPEN_*` names. Two lists here still spell
//! some of them, and this is the one file that may: what an older Droppen
//! hands its children, which this server does not pass on
//! ([`OLDER_HANDOFFS`]), and the two directory names `serve` warns about
//! ([`unread_dirs`]).

/// The settings whose empty value is a value ([`var_or_empty`]).
const EMPTY_IS_A_VALUE: &[&str] = &["DROPPEN_AGENT_DIRECT_STUN"];

/// What an older Droppen (0.2.125 to 0.2.134) sets, besides the `DROPPEN_*`
/// name, under the name from before the rename for a program it starts: a
/// session's notify channel and token, the desktop app's and `up`'s word to
/// their server. This server reads none of them, and no child of it inherits
/// them ([`crate::process_env::clean_env`]): a server started inside such a
/// session would otherwise hand that session's token, a capability with no
/// expiry, to every session of its own, and an older `droppen-notify` there
/// would take that channel for its own.
pub const OLDER_HANDOFFS: [&str; 4] = ["RH_NOTIFY_URL", "RH_NOTIFY_TOKEN", "RH_DESKTOP_APP", "RH_SUPERVISED"];

/// The two old names that silently move a server's files when unread: a
/// hand-written launchd plist or unit that sets only these, under an older
/// `up` that self-upgrades its server to this one, would run that server on
/// the default install's files. `(old, new)`.
const UNREAD_DIRS: [(&str, &str); 2] = [("REMOTE_HAND_DATA_DIR", "DROPPEN_DATA_DIR"), ("REMOTE_HAND_CONFIG_DIR", "DROPPEN_CONFIG_DIR")];

/// What `serve` says at start about [`UNREAD_DIRS`]: a line, by name and never
/// by value, for each old name that is set while its new name is not. Nothing
/// when both are set, as v0.2.134 asked: the new one is then read.
pub fn unread_dirs() -> Vec<String> {
    unread_dirs_in(|name| std::env::var(name).ok())
}

/// [`unread_dirs`], reading through `read`.
fn unread_dirs_in(read: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let set = |name: &str| read(name).is_some_and(|value| !value.trim().is_empty());
    UNREAD_DIRS
        .iter()
        .filter(|(old, new)| set(old) && !set(new))
        .map(|(old, new)| format!("{old} is set but no longer read; set {new}"))
        .collect()
}

/// The variable `name` (a `DROPPEN_*` name), trimmed, with an empty value
/// unset.
pub fn var(name: &str) -> Option<String> {
    read(name).filter(|value| !value.is_empty())
}

/// [`var`] for the one variable whose empty value means something
/// (`DROPPEN_AGENT_DIRECT_STUN`: no STUN servers at all): set to anything,
/// the empty string included, is set.
pub fn var_or_empty(name: &str) -> Option<String> {
    debug_assert!(EMPTY_IS_A_VALUE.contains(&name), "{name}: an empty value is unset");
    read(name)
}

/// A switch read through [`var`]: on only for an explicit `1`, `true`, `yes`
/// or `on` (any case), and off for everything else — `0`, `false`, any other
/// word, an empty value and an unset one alike. Being set is not being on.
pub fn flag(name: &str) -> bool {
    var(name).is_some_and(|value| truthy(&value))
}

/// [`flag`] for a switch that is on unless it is explicitly turned off.
///
/// `default` answers only when the variable is unset (or empty); a value that
/// is set but is not one of the on spellings is off, whichever way the default
/// leans. Being set is not being on.
pub fn flag_or(name: &str, default: bool) -> bool {
    var(name).map_or(default, |value| truthy(&value))
}

/// The on spellings, shared by [`flag`] and [`flag_or`].
fn truthy(value: &str) -> bool {
    matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

/// `name`'s value, trimmed.
fn read(name: &str) -> Option<String> {
    debug_assert!(name.starts_with("DROPPEN_"), "{name}: read a variable by its DROPPEN_ name");
    std::env::var(name).ok().map(|value| value.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spellings both switch readers use, through [`truthy`].
    #[test]
    fn a_switch_is_on_only_for_an_explicit_yes() {
        const NAME: &str = "DROPPEN_TEST_FLAG";
        std::env::remove_var(NAME);
        assert!(!flag(NAME), "unset is off");
        assert!(flag_or(NAME, true), "unset takes the caller's default");
        assert!(!flag_or(NAME, false));
        for value in ["1", "true", "TRUE", " yes ", "on", "On"] {
            std::env::set_var(NAME, value);
            assert!(flag(NAME), "{value:?} is on");
            assert!(flag_or(NAME, false), "{value:?} is on whichever way the default leans");
        }
        for value in ["0", "false", "FALSE", "off", "no", "enabled"] {
            std::env::set_var(NAME, value);
            assert!(!flag(NAME), "{value:?} is off");
            assert!(!flag_or(NAME, true), "{value:?} is off whichever way the default leans");
        }
        // An empty value is unset, so it takes each reader's default.
        std::env::set_var(NAME, "");
        assert!(!flag(NAME));
        assert!(flag_or(NAME, true));
        assert!(!flag_or(NAME, false));
        std::env::remove_var(NAME);
        assert!(!flag(NAME), "unset again is off");
        assert!(flag_or(NAME, true), "unset again takes the default");
    }

    /// A value is trimmed; an empty one is unset, except to [`read`], which
    /// [`var_or_empty`] answers through.
    #[test]
    fn a_value_is_trimmed_and_an_empty_one_is_unset() {
        const NAME: &str = "DROPPEN_TEST_VAR";
        std::env::remove_var(NAME);
        assert_eq!(var(NAME), None);
        std::env::set_var(NAME, " /a b \n");
        assert_eq!(var(NAME).as_deref(), Some("/a b"));
        std::env::set_var(NAME, " ");
        assert_eq!(var(NAME), None, "empty is unset");
        assert_eq!(read(NAME).as_deref(), Some(""), "where empty is a value, it is one");
        std::env::remove_var(NAME);
    }

    /// Each unread directory name is said by name, only while its new name is
    /// unset.
    #[test]
    fn an_unread_directory_name_is_said_by_name() {
        let said = |vars: &[(&str, &str)]| {
            let vars: std::collections::HashMap<&str, &str> = vars.iter().copied().collect();
            unread_dirs_in(|name| vars.get(name).map(|value| value.to_string()))
        };
        assert_eq!(
            said(&[("REMOTE_HAND_DATA_DIR", "/secret/a"), ("REMOTE_HAND_CONFIG_DIR", "/secret/b")]),
            [
                "REMOTE_HAND_DATA_DIR is set but no longer read; set DROPPEN_DATA_DIR",
                "REMOTE_HAND_CONFIG_DIR is set but no longer read; set DROPPEN_CONFIG_DIR",
            ]
        );
        assert!(said(&[("REMOTE_HAND_DATA_DIR", "/d"), ("DROPPEN_DATA_DIR", "/d")]).is_empty(), "moved over already");
        assert!(said(&[("REMOTE_HAND_DATA_DIR", " ")]).is_empty(), "empty is unset");
        assert!(said(&[("RH_SESSION_LOG", "0")]).is_empty(), "only the two directory names");
    }

    /// No source names an `RH_*` or `REMOTE_HAND_*` variable in a string of
    /// its own: the server reads its environment by the `DROPPEN_*` names,
    /// through this module.
    #[test]
    fn nothing_reads_an_old_name() {
        // A name no code reads: a terminal test's stand-in for "any variable".
        // And rh-proto's switch that rewrites its test vectors: the protocol
        // crate the hub and the relays build too keeps its own name for it.
        const ALLOWED: &[&str] = &["RH_TERMINAL_KEEP", "RH_PROTO_WRITE_VECTORS"];
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize().expect("this crate");
        let src = manifest.join("src");
        let mut found = Vec::new();
        // The server's sources too, and those of every crate of its workspace,
        // this one's among them: they read the environment through this
        // module, and a crate that leaves the server is read wherever it goes.
        let crates = std::fs::read_dir(manifest.parent().expect("the workspace's crates")).expect("the workspace's crates").flatten();
        let mut dirs: Vec<_> = std::iter::once(manifest.join("../../src").canonicalize().expect("the server's sources")).chain(crates.map(|entry| entry.path().join("src"))).filter(|dir| dir.is_dir()).collect();
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).expect("read src").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                // This file spells out the prefixes it looks for.
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
        assert!(found.is_empty(), "read these by their DROPPEN_ names, through droppen_base::env::var:\n{}", found.join("\n"));
    }
}
