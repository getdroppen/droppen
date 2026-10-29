//! The server's environment variables, each read by its `DROPPEN_*` name:
//! trimmed, and an empty value unset ([`var`]), except where an empty value
//! means something ([`var_or_empty`]).
//!
//! The names from before the rename to Droppen (`REMOTE_HAND_*`, `RH_*`) are
//! neither read nor set for the programs the server starts: every supported
//! Droppen program reads the `DROPPEN_*` names.

/// The settings whose empty value is a value ([`var_or_empty`]).
const EMPTY_IS_A_VALUE: &[&str] = &["DROPPEN_AGENT_DIRECT_STUN"];

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

    /// No source names an `RH_*` or `REMOTE_HAND_*` variable in a string of
    /// its own: the server reads its environment by the `DROPPEN_*` names,
    /// through this module.
    #[test]
    fn nothing_reads_an_old_name() {
        // A name no code reads: a terminal test's stand-in for "any variable".
        const ALLOWED: &[&str] = &["RH_TERMINAL_KEEP"];
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let src = manifest.join("src");
        let mut found = Vec::new();
        // The server's sources too, and those of the crates it is made of: they
        // read the environment through this module.
        let mut dirs = vec![
            src.clone(),
            manifest.join("../../src"),
            manifest.join("../droppen-http/src"),
            manifest.join("../droppen-instance/src"),
            manifest.join("../droppen-pty/src"),
            manifest.join("../droppen-supervisor/src"),
        ];
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
