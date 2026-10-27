//! The two names this binary answers to: `droppen`, and `remote-hand-server`.
//!
//! One binary, one behaviour. A release ships it under both names (the
//! tarball's `droppen` and `remote-hand-server` are one file), and the
//! installer adds `<data>/bin/droppen` beside the `<data>/bin/remote-hand-server`
//! it manages. Nothing here changes what a command *does*: `argv[0]` only picks
//! the words a person reads — the usage, the commands an error suggests, and
//! the prefix on stderr.
//!
//! * Run as `droppen` (any path whose last part is `droppen`): the usage and the
//!   suggested commands say `droppen`, and errors start with `droppen:`.
//! * Run under any other name — `remote-hand-server`, the supervisor's child,
//!   a test binary: the usage says `remote-hand-server`, and errors start with
//!   `remote-hand:`, which scripts that grep for it keep relying on.
//!
//! The managed binary is not renamed: the `up` supervisor, self-upgrade and the
//! `.prev`/`.bad` rollback ladder all stay on `<data>/bin/remote-hand-server`
//! ([`crate::selfupdate::store`]); `droppen` there is only a link to it.
//!
//! [`droppen_path`] is the other half: the path a person on this device types
//! to run this server, which the web app shows in the commands it asks people
//! to type (`/api/auth/status`'s `droppenPath`).

use std::borrow::Cow;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The new name, and the one file name that selects it.
pub const DROPPEN: &str = "droppen";
/// The old name, which the managed binary keeps.
pub const LEGACY_COMMAND: &str = "remote-hand-server";

/// Which name this process was started under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Name {
    /// `droppen`.
    Droppen,
    /// `remote-hand-server`, or anything else.
    Legacy,
}

impl Name {
    /// The name `argv[0]` selects: `droppen` when its last path component is
    /// `droppen` (in any case — macOS file names are case-insensitive), the old
    /// name otherwise.
    pub fn from_argv0(argv0: Option<&OsStr>) -> Self {
        let base = argv0.and_then(|arg| Path::new(arg).file_name()).and_then(OsStr::to_str);
        match base {
            Some(base) if base.eq_ignore_ascii_case(DROPPEN) => Self::Droppen,
            _ => Self::Legacy,
        }
    }

    /// This process's name, read from `argv[0]` once.
    pub fn invoked() -> Self {
        static NAME: OnceLock<Name> = OnceLock::new();
        *NAME.get_or_init(|| Self::from_argv0(std::env::args_os().next().as_deref()))
    }

    /// The command a person types: `droppen` or `remote-hand-server`.
    pub fn command(self) -> &'static str {
        match self {
            Self::Droppen => DROPPEN,
            Self::Legacy => LEGACY_COMMAND,
        }
    }

    /// What starts an error on stderr: `droppen`, or `remote-hand` under the
    /// old name, which scripts grep for.
    pub fn error_prefix(self) -> &'static str {
        match self {
            Self::Droppen => DROPPEN,
            Self::Legacy => "remote-hand",
        }
    }

    /// `text` with every command `remote-hand-server` in it said as this name.
    ///
    /// Only the command word is rewritten, not a path or a file that contains
    /// it: `remote-hand-server user add` and `` `remote-hand-server up` ``
    /// become `droppen …`, while `<data>/bin/remote-hand-server`,
    /// `remote-hand-server.prev` and `remote-hand-server-x` stay as they are —
    /// those name files on disk, which keep the old name. Under the old name
    /// `text` comes back untouched.
    pub fn rewrite(self, text: &str) -> Cow<'_, str> {
        if self == Self::Legacy || !text.contains(LEGACY_COMMAND) {
            return Cow::Borrowed(text);
        }
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find(LEGACY_COMMAND) {
            let (before, from) = rest.split_at(at);
            let after = &from[LEGACY_COMMAND.len()..];
            let previous = before.chars().last().or_else(|| out.chars().last());
            let starts_a_word = !previous.is_some_and(|c| c == '/' || is_name_char(c));
            let mut next = after.chars();
            let ends_a_word = match next.next() {
                None => true,
                // `remote-hand-server.prev` is a file; `… remote-hand-server.` ends a sentence.
                Some('.') => !next.next().is_some_and(|c| c.is_ascii_alphanumeric()),
                Some(c) => !is_name_char(c),
            };
            out.push_str(before);
            out.push_str(if starts_a_word && ends_a_word { DROPPEN } else { LEGACY_COMMAND });
            rest = after;
        }
        out.push_str(rest);
        Cow::Owned(out)
    }
}

/// Characters that continue a command or file name.
fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'
}

/// The path a person on this device types to run this server's binary, with
/// the home directory as `~`.
///
/// The `droppen` link beside the running binary when there is one (the
/// installer's `<data>/bin/droppen`, a release tarball's `droppen`), else the
/// running binary itself. Shown before anyone signs in (the login page's
/// "create an account" hint), hence `~` rather than a path with the account's
/// name in it.
pub fn droppen_path() -> String {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from(LEGACY_COMMAND));
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty()).map(PathBuf::from);
    droppen_path_for(&exe, home.as_deref())
}

/// [`droppen_path`], with the executable and the home directory supplied.
pub fn droppen_path_for(exe: &Path, home: Option<&Path>) -> String {
    // Linux names a binary replaced while it runs (an upgrade before the
    // restart) `<path> (deleted)`; the path is still the one to type.
    let exe = match exe.to_str().and_then(|text| text.strip_suffix(" (deleted)")) {
        Some(stripped) => PathBuf::from(stripped),
        None => exe.to_path_buf(),
    };
    let chosen = if Name::from_argv0(exe.file_name()) == Name::Droppen {
        exe
    } else {
        match exe.parent().map(|dir| dir.join(DROPPEN)) {
            Some(link) if same_file(&link, &exe) => link,
            _ => exe,
        }
    };
    abbreviate_home(&chosen, home)
}

/// Whether two paths are one file once links are followed.
fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
    }
}

/// What a command typed on this device needs in its environment to act on
/// the instance `paths` names, as `(name, value)` pairs to put before
/// [`droppen_path`]: `APP_INSTANCE` for any instance but `default`, and
/// `DROPPEN_DATA_DIR` / `DROPPEN_CONFIG_DIR` when this server keeps its files
/// elsewhere than `~/.local/share/remote-hand` / `~/.config/remote-hand` (a
/// second install beside the first). Nothing for the usual install. Paths
/// have the home directory as `~`, for the reason [`droppen_path`] does.
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
pub fn droppen_env(paths: &crate::sessions::Paths) -> Vec<(String, String)> {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty()).map(PathBuf::from);
    droppen_env_for(paths, home.as_deref())
}

/// [`droppen_env`], with the home directory supplied.
pub fn droppen_env_for(paths: &crate::sessions::Paths, home: Option<&Path>) -> Vec<(String, String)> {
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

/// [`droppen_env`] as a person types it before a command: `NAME=value ` for
/// each, the values made shell words ([`shell_word`]); empty for the usual
/// install.
pub fn command_prefix(paths: &crate::sessions::Paths) -> String {
    droppen_env(paths).into_iter().map(|(name, value)| format!("{name}={} ", shell_word(&value))).collect()
}

/// `text` as one word of a POSIX shell: as it is when nothing in it is
/// special, else in single quotes — after a leading `~/`, which expands only
/// unquoted. The web app's `shellWord` (`web/src/lib/droppen-command.ts`).
pub fn shell_word(text: &str) -> String {
    let plain = |text: &str| {
        !text.is_empty()
            && text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '%' | '+' | '=' | ':' | ',' | '.' | '/' | '~' | '-'))
    };
    if plain(text) {
        return text.to_string();
    }
    let quote = |text: &str| format!("'{}'", text.replace('\'', r"'\''"));
    match text.strip_prefix("~/") {
        Some(rest) => format!("~/{}", quote(rest)),
        None => quote(text),
    }
}

/// `path` with a leading home directory written `~`.
fn abbreviate_home(path: &Path, home: Option<&Path>) -> String {
    if let Some(rest) = home
        .filter(|home| home.is_absolute() && home != &Path::new("/"))
        .and_then(|home| path.strip_prefix(home).ok())
    {
        if !rest.as_os_str().is_empty() {
            return format!("~/{}", rest.to_string_lossy());
        }
    }
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv0_droppen_selects_the_new_name_and_anything_else_the_old() {
        for argv0 in ["droppen", "/Users/a/.local/share/remote-hand/bin/droppen", "./droppen", "Droppen"] {
            assert_eq!(Name::from_argv0(Some(OsStr::new(argv0))), Name::Droppen, "{argv0}");
        }
        for argv0 in [
            "remote-hand-server",
            "/Users/a/.local/share/remote-hand/bin/remote-hand-server",
            "/w/target/debug/deps/remote_hand_server-1a2b",
            "droppen-relay",
            "/opt/droppen/bin/server",
            "",
        ] {
            assert_eq!(Name::from_argv0(Some(OsStr::new(argv0))), Name::Legacy, "{argv0}");
        }
        assert_eq!(Name::from_argv0(None), Name::Legacy);
    }

    #[test]
    fn each_name_has_its_command_and_error_prefix() {
        assert_eq!(Name::Droppen.command(), "droppen");
        assert_eq!(Name::Droppen.error_prefix(), "droppen");
        assert_eq!(Name::Legacy.command(), "remote-hand-server");
        assert_eq!(Name::Legacy.error_prefix(), "remote-hand", "scripts grep for `remote-hand:`");
    }

    #[test]
    fn the_new_name_rewrites_commands_and_leaves_files_alone() {
        let d = Name::Droppen;
        assert_eq!(d.rewrite("    remote-hand-server user add <name>"), "    droppen user add <name>");
        assert_eq!(
            d.rewrite("run `remote-hand-server agent unenroll` first"),
            "run `droppen agent unenroll` first"
        );
        assert_eq!(d.rewrite("(remote-hand-server plugin disable x)"), "(droppen plugin disable x)");
        assert_eq!(d.rewrite("APP_HOME_DIR=/x remote-hand-server serve"), "APP_HOME_DIR=/x droppen serve");
        assert_eq!(d.rewrite("start it with remote-hand-server."), "start it with droppen.");
        assert_eq!(d.rewrite("remote-hand-server"), "droppen");
        // Files and paths keep their names.
        for kept in [
            "~/.local/share/remote-hand/bin/remote-hand-server",
            "<data>/bin/remote-hand-server.prev",
            "remote-hand-server.bad",
            "remote-hand-server-aarch64",
            "my-remote-hand-server",
            "~/.local/share/remote-hand/instances",
        ] {
            assert_eq!(d.rewrite(kept), kept);
        }
        assert_eq!(
            d.rewrite("remote-hand-server up runs <data>/bin/remote-hand-server"),
            "droppen up runs <data>/bin/remote-hand-server"
        );
        assert!(matches!(d.rewrite("nothing to change"), Cow::Borrowed(_)));
    }

    #[test]
    fn a_command_names_the_instance_and_the_files_of_a_server_that_is_not_the_usual_one() {
        let home = Path::new("/home/ada");
        let pair = |name: &str, value: &str| (name.to_string(), value.to_string());
        let usual = crate::sessions::Paths::new(home.to_path_buf());
        assert_eq!(droppen_env_for(&usual, Some(home)), Vec::<(String, String)>::new(), "the usual install needs nothing");

        let other_instance = crate::sessions::Paths::for_instance(home.to_path_buf(), "bob");
        assert_eq!(droppen_env_for(&other_instance, Some(home)), vec![pair("APP_INSTANCE", "bob")]);

        // A second install, its files under another root.
        // Under both names: the `droppen` a shell finds may be a pre-rename
        // binary, which reads only the old ones.
        let second = crate::sessions::Paths::for_instance(home.join("bob"), "default");
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

    #[test]
    fn a_value_is_quoted_only_where_a_shell_needs_it() {
        assert_eq!(shell_word("bob"), "bob");
        assert_eq!(shell_word("~/.local/share/bob-remote-hand"), "~/.local/share/bob-remote-hand");
        assert_eq!(shell_word("~/Library/Application Support/x"), "~/'Library/Application Support/x'", "`~` stays outside");
        assert_eq!(shell_word("/opt/o'brien"), r"'/opt/o'\''brien'");
        assert_eq!(shell_word("/opt/$x"), "'/opt/$x'");
        assert_eq!(shell_word(""), "''");
    }

    #[test]
    fn the_old_name_rewrites_nothing() {
        let text = "    remote-hand-server user add <name>";
        assert!(matches!(Name::Legacy.rewrite(text), Cow::Borrowed(t) if t == text));
    }

    #[cfg(unix)]
    #[test]
    fn the_command_path_is_the_droppen_link_beside_the_binary_or_else_the_binary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        let bin = home.join(".local/share/remote-hand/bin");
        std::fs::create_dir_all(&bin).expect("bin");
        let server = bin.join("remote-hand-server");
        std::fs::write(&server, b"#!/bin/sh\n").expect("server");

        // No link: the binary itself, with the home directory as `~`.
        assert_eq!(droppen_path_for(&server, Some(home)), "~/.local/share/remote-hand/bin/remote-hand-server");
        assert_eq!(
            droppen_path_for(&server, None),
            server.to_string_lossy(),
            "without a home directory, the whole path"
        );

        // A `droppen` that is something else is not this server.
        std::fs::write(bin.join("droppen"), b"other").expect("other");
        assert_eq!(droppen_path_for(&server, Some(home)), "~/.local/share/remote-hand/bin/remote-hand-server");
        std::fs::remove_file(bin.join("droppen")).expect("rm");

        // The installer's link (relative, as it writes it).
        std::os::unix::fs::symlink("remote-hand-server", bin.join("droppen")).expect("link");
        assert_eq!(droppen_path_for(&server, Some(home)), "~/.local/share/remote-hand/bin/droppen");
        // A replaced binary, as Linux names it, still finds it.
        let deleted = PathBuf::from(format!("{} (deleted)", server.display()));
        assert_eq!(droppen_path_for(&deleted, Some(home)), "~/.local/share/remote-hand/bin/droppen");

        // A hard link, as in a release tarball.
        let unpacked = home.join("unpacked");
        std::fs::create_dir_all(&unpacked).expect("unpacked");
        std::fs::write(unpacked.join("droppen"), b"bin").expect("droppen");
        std::fs::hard_link(unpacked.join("droppen"), unpacked.join("remote-hand-server")).expect("hard link");
        assert_eq!(droppen_path_for(&unpacked.join("remote-hand-server"), Some(home)), "~/unpacked/droppen");

        // Run as `droppen`: that path.
        assert_eq!(droppen_path_for(&unpacked.join("droppen"), Some(home)), "~/unpacked/droppen");

        // Outside the home directory: the whole path.
        assert_eq!(
            droppen_path_for(Path::new("/usr/local/bin/remote-hand-server"), Some(home)),
            "/usr/local/bin/remote-hand-server"
        );
        // A home of `/` abbreviates nothing.
        assert_eq!(
            droppen_path_for(Path::new("/usr/local/bin/remote-hand-server"), Some(Path::new("/"))),
            "/usr/local/bin/remote-hand-server"
        );
    }
}
