//! Where a CLI is, how it got there, and the `PATH` that finds it.
//!
//! Three questions with filesystem answers and no processes:
//!
//! * **The `PATH`.** [`base_dirs`] and [`build`] come from `droppen-path`, the
//!   one rule the desktop app (`desktop/core/src/env_path.rs`) uses too; the
//!   command-line installer's launchd agent gets the same (`scripts/install.sh`
//!   `service_path`): the login shell's `PATH` first, then the base
//!   directories. The app builds its server's `PATH` this way at launch; the
//!   guide rebuilds it the same way when it re-reads the login shell, so a
//!   re-read can only ever add what the app would have found itself.
//!   [`with_overlay`] appends the directories the user pointed the guide at,
//!   after all of it.
//! * **Where the CLI is.** [`locate`]: on the child `PATH` first; failing that,
//!   the places the installers and version managers put things
//!   ([`fallback_candidates`]) — which is what finds a `claude` under nvm when
//!   the login shell was too slow to say where nvm is.
//! * **How it was installed.** [`install_method`], off the resolved path, so
//!   the guide can offer the matching update command.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::{FoundVia, InstallMethod, ToolFound, ToolId};
use crate::process_env::{resolve_on_path, Env};

// The PATH rule itself lives in `droppen-path`, shared with the desktop app.
pub use droppen_path::{base_dirs, build};

/// `path` followed by each directory of `more` it does not have yet, whether
/// or not it exists.
///
/// How a re-read keeps what the process was started with: a new `PATH` only
/// ever *adds* to the inherited one, so a server started from a terminal with a
/// `PATH` of its own cannot lose a directory its sessions relied on.
pub fn merge(path: &str, more: &str) -> String {
    let mut dirs: Vec<&str> = path.split(':').filter(|dir| !dir.is_empty()).collect();
    let mut seen: HashSet<&str> = dirs.iter().copied().collect();
    for dir in more.split(':') {
        if !dir.is_empty() && !dir.contains("/remote-hand/instances/") && seen.insert(dir) {
            dirs.push(dir);
        }
    }
    dirs.join(":")
}

/// `path` with every overlay directory that exists and is not on it yet
/// appended, in the order the user adopted them.
///
/// Last, not first: an adopted directory is where a CLI was *found*, and it
/// must not start shadowing the tools the user's own `PATH` puts first. One
/// with a `:` in its path is never appended: it would split into two
/// entries, the second relative.
pub fn with_overlay(path: &str, overlay: &[PathBuf]) -> String {
    let mut dirs: Vec<String> = path.split(':').filter(|dir| !dir.is_empty()).map(str::to_string).collect();
    let mut seen: HashSet<String> = dirs.iter().cloned().collect();
    for dir in overlay {
        let dir = dir.to_string_lossy().into_owned();
        if dir.starts_with('/') && !dir.contains(':') && Path::new(&dir).is_dir() && seen.insert(dir.clone()) {
            dirs.push(dir);
        }
    }
    dirs.join(":")
}

/// Every place a CLI's installer or a version manager may have put it, most
/// likely first, as candidate executable paths.
///
/// `system_dirs` is Homebrew's two prefixes (`/opt/homebrew/bin`,
/// `/usr/local/bin`) in production and a scratch directory in the tests, so a
/// test never finds the real machine's CLI.
pub fn fallback_candidates(tool: ToolId, home: &Path, system_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let name = tool.binary();
    let mut out = vec![home.join(".local/bin").join(name)];
    if tool == ToolId::Claude {
        // The old per-user "local" install `claude migrate-installer` made.
        out.push(home.join(".claude/local/claude"));
    }
    out.extend(system_dirs.iter().map(|dir| dir.join(name)));
    for dir in [".npm-global/bin", ".volta/bin", ".bun/bin"] {
        out.push(home.join(dir).join(name));
    }
    // Every installed Node, newest first: a CLI installed under an older one is
    // still the CLI.
    for version in newest_first(&home.join(".nvm/versions/node")) {
        out.push(version.join("bin").join(name));
    }
    for version in newest_first(&home.join(".local/share/fnm/node-versions")) {
        out.push(version.join("installation/bin").join(name));
    }
    out.push(home.join(".asdf/shims").join(name));
    out.push(home.join(".local/share/mise/shims").join(name));
    out
}

/// The subdirectories of `dir`, highest version number first; names that are
/// not versions sort last, by name.
fn newest_first(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut versions: Vec<(Option<Vec<u64>>, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .map(|path| {
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            (version_key(&name), path)
        })
        .collect();
    versions.sort_by(|(a_key, a_path), (b_key, b_path)| match (a_key, b_key) {
        (Some(a), Some(b)) => b.cmp(a),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a_path.cmp(b_path),
    });
    versions.into_iter().map(|(_, path)| path).collect()
}

/// `v22.3.0` → `[22, 3, 0]`.
fn version_key(name: &str) -> Option<Vec<u64>> {
    let parts: Option<Vec<u64>> = name.trim_start_matches('v').split('.').map(|part| part.parse().ok()).collect();
    parts.filter(|parts| !parts.is_empty())
}

/// What [`locate`] looks with.
#[derive(Debug, Clone)]
pub struct Search<'a> {
    /// The home directory the fallbacks are under.
    pub home: &'a Path,
    /// The environment children get; its `PATH` is looked at first.
    pub env: &'a Env,
    /// The `PATH` this process was started with, to tell an adopted directory
    /// from one that was always there.
    pub inherited_path: &'a str,
    /// The directories the user adopted.
    pub overlay: &'a [PathBuf],
    /// Homebrew's prefixes (see [`fallback_candidates`]).
    pub system_dirs: &'a [PathBuf],
}

/// Where `tool` is, and how it was found.
pub fn locate(tool: ToolId, search: &Search<'_>) -> Option<ToolFound> {
    if let Some(path) = resolve_on_path(tool.binary(), search.env) {
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let inherited = search.inherited_path.split(':').any(|entry| Path::new(entry) == dir);
        let adopted = search.overlay.contains(&dir);
        let via = if adopted && !inherited { FoundVia::Picked } else { FoundVia::Path };
        return Some(found(tool, path, via));
    }
    fallback_candidates(tool, search.home, search.system_dirs)
        .into_iter()
        .find(|candidate| resolve_on_path(&candidate.to_string_lossy(), search.env).is_some())
        .map(|path| found(tool, path, FoundVia::Fallback))
}

fn found(tool: ToolId, path: PathBuf, via: FoundVia) -> ToolFound {
    ToolFound {
        method: install_method(tool, &path),
        path: path.to_string_lossy().into_owned(),
        via,
    }
}

/// How the binary at `path` was installed, read off where it really lives.
///
/// The official installers leave a symlink in `~/.local/bin` pointing into a
/// versions directory of their own, Homebrew links into its Cellar or
/// Caskroom, and npm into a `node_modules`; the link target says which. Matched
/// as path fragments rather than against `home`, because the resolved path has
/// been through every symlink on the way — on macOS `/var` itself is one.
pub fn install_method(tool: ToolId, path: &Path) -> InstallMethod {
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let real = real.to_string_lossy();
    let native = match tool {
        ToolId::Claude => "/.local/share/claude/versions/",
        ToolId::Codex => "/.codex/packages/standalone/",
    };
    if real.contains(native) {
        InstallMethod::Native
    } else if real.contains("/Caskroom/") || real.contains("/Cellar/") {
        InstallMethod::Brew
    } else if real.contains("/node_modules/") {
        InstallMethod::Npm
    } else {
        InstallMethod::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A file with the executable bit — created, never run.
    fn marker(path: &Path) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, "not a program; never executed\n").expect("write");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    fn env_with_path(path: &str) -> Env {
        [("PATH".to_string(), path.to_string())].into_iter().collect()
    }

    // The PATH rule's contract, pinned where the server uses it: the fixed
    // directories and their order decide which `node` and `claude` a session
    // runs, and the login shell's entries and the session-bin filter decide the
    // rest. `env_path.rs` pins the same contract from the app's side, so a
    // change to the rule or to an expectation is deliberate on both.

    #[test]
    fn without_the_login_shell_the_base_is_the_path() {
        let path = build(Path::new("/Users/ada"), None);
        let dirs: Vec<&str> = path.split(':').collect();
        let base: Vec<String> = base_dirs(Path::new("/Users/ada"))
            .iter()
            .map(|dir| dir.to_string_lossy().into_owned())
            .collect();
        assert_eq!(dirs, base);
        assert_eq!(dirs[0], "/Users/ada/.local/bin");
        let homebrew = dirs.iter().position(|dir| *dir == "/opt/homebrew/bin").expect("homebrew");
        let usr_local = dirs.iter().position(|dir| *dir == "/usr/local/bin").expect("usr/local");
        let usr_bin = dirs.iter().position(|dir| *dir == "/usr/bin").expect("usr/bin");
        assert!(homebrew < usr_local && usr_local < usr_bin);
        assert_eq!(*dirs.last().expect("last"), "/sbin");
    }

    #[test]
    fn the_login_shell_comes_first_and_the_base_fills_in_after_it() {
        let root = tempfile::tempdir().expect("tempdir");
        let nvm = root.path().join("nvm/versions/node/v22/bin");
        std::fs::create_dir_all(&nvm).expect("nvm");
        let nvm = nvm.to_string_lossy().into_owned();
        // nvm's node before Homebrew's, as the user's terminal has it.
        let shell = format!("{nvm}:/usr/bin:/bin");
        let path = build(Path::new("/Users/ada"), Some(&shell));
        let dirs: Vec<&str> = path.split(':').collect();
        assert_eq!(&dirs[..3], [nvm.as_str(), "/usr/bin", "/bin"], "the shell's own order, first");
        let homebrew = dirs.iter().position(|dir| *dir == "/opt/homebrew/bin").expect("homebrew");
        assert!(homebrew > 2, "Homebrew is a fallback after the shell: {path}");
        // The rest of the base, in its own order, once each.
        let rest: Vec<&str> = dirs[3..].to_vec();
        let base: Vec<String> = base_dirs(Path::new("/Users/ada"))
            .iter()
            .map(|dir| dir.to_string_lossy().into_owned())
            .filter(|dir| dir != "/usr/bin" && dir != "/bin")
            .collect();
        assert_eq!(rest, base);
    }

    #[test]
    fn the_login_shell_adds_existing_absolute_directories_once() {
        let root = tempfile::tempdir().expect("tempdir");
        let nvm = root.path().join("nvm/versions/node/v22/bin");
        std::fs::create_dir_all(&nvm).expect("nvm");
        let extra = format!(
            "{}:/usr/bin:relative/bin::{}:/does/not/exist:/opt/homebrew/bin:/usr/bin",
            nvm.display(),
            nvm.display()
        );
        let path = build(Path::new("/Users/ada"), Some(&extra));
        let dirs: Vec<&str> = path.split(':').collect();
        let nvm = nvm.to_string_lossy();
        assert_eq!(dirs.iter().filter(|dir| **dir == nvm).count(), 1);
        assert_eq!(dirs.iter().filter(|dir| **dir == "/usr/bin").count(), 1);
        assert_eq!(dirs.iter().filter(|dir| **dir == "/bin").count(), 1);
        assert_eq!(dirs.iter().filter(|dir| **dir == "/opt/homebrew/bin").count(), 1);
        assert!(!dirs.contains(&"relative/bin"));
        assert!(!dirs.contains(&"/does/not/exist"));
        assert!(!dirs.contains(&""));
        assert_eq!(dirs[0], nvm, "the shell's own directories come first");
        // Each where it first appears: /usr/bin where the shell put it, not
        // again where the base would have.
        assert_eq!(dirs[1], "/usr/bin");
        assert_eq!(*dirs.last().expect("last"), "/sbin", "the base comes after the shell's");
    }

    #[test]
    fn the_command_line_install_builds_the_same_path() {
        // `scripts/install.sh` `service_path`, run on its own, against the same
        // HOME and shell PATH: the launchd agent (or systemd unit) of a
        // command-line install gives the server what the desktop app does, in
        // the same order.
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/install.sh");
        let script = std::fs::read_to_string(&script).expect("install.sh");
        let start = script.find("\nservice_path() {").expect("service_path in install.sh");
        let end = start + script[start..].find("\n}\n").expect("end of service_path") + 3;
        let function = &script[start..end];

        let root = tempfile::tempdir().expect("tempdir");
        let home = root.path().join("home");
        let nvm = home.join(".nvm/versions/node/v22/bin");
        let session_bin = home.join(".local/share/remote-hand/instances/other/session-bin");
        std::fs::create_dir_all(&nvm).expect("nvm");
        std::fs::create_dir_all(&session_bin).expect("session-bin");
        let shell = format!(
            "{}:{}:/usr/bin:relative:{}/.local/bin:/does/not/exist::{}:/bin",
            nvm.display(),
            session_bin.display(),
            home.display(),
            nvm.display()
        );
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &format!("{function}\nPATH=\"$1\" service_path"), "sh", &shell])
            .env_clear()
            .env("HOME", &home)
            .output()
            .expect("sh");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let installed = String::from_utf8(output.stdout).expect("utf-8");
        assert_eq!(installed.trim_end(), build(&home, Some(&shell)));
    }

    #[test]
    fn the_adopted_folders_come_after_the_shell_and_the_base() {
        let root = tempfile::tempdir().expect("tempdir");
        let nvm = root.path().join("nvm/bin");
        let adopted = root.path().join("adopted/bin");
        std::fs::create_dir_all(&nvm).expect("nvm");
        std::fs::create_dir_all(&adopted).expect("adopted");
        let shell = format!("{}:/usr/bin", nvm.display());
        let built = build(Path::new("/Users/ada"), Some(&shell));
        let path = with_overlay(&built, &[adopted.clone(), PathBuf::from("/usr/bin")]);
        let dirs: Vec<&str> = path.split(':').collect();
        assert_eq!(dirs[0], nvm.to_string_lossy());
        assert_eq!(dirs[dirs.len() - 2], "/sbin", "the base before the adopted folder");
        assert_eq!(*dirs.last().expect("last"), adopted.to_string_lossy());
        assert_eq!(dirs.iter().filter(|dir| **dir == "/usr/bin").count(), 1);
    }

    #[test]
    fn without_a_home_the_path_has_no_relative_directory() {
        for home in ["", ".", "relative/home"] {
            let path = build(Path::new(home), Some("relative/bin:/usr/bin"));
            assert!(path.split(':').all(|dir| dir.starts_with('/')), "{home:?}: {path}");
            assert!(path.starts_with("/usr/bin:/opt/homebrew/bin:/usr/local/bin:"), "{home:?}: {path}");
            assert!(base_dirs(Path::new(home)).iter().all(|dir| dir.is_absolute()));
        }
        assert_eq!(base_dirs(Path::new("/Users/ada"))[0], Path::new("/Users/ada/.local/bin"));
    }

    #[test]
    fn a_session_bin_directory_never_leaks_in() {
        let root = tempfile::tempdir().expect("tempdir");
        let session_bin = root.path().join(".local/share/remote-hand/instances/another/session-bin");
        std::fs::create_dir_all(&session_bin).expect("dir");
        let path = build(Path::new("/Users/ada"), Some(&session_bin.to_string_lossy()));
        assert!(!path.contains("session-bin"), "{path}");
    }

    #[test]
    fn a_merge_keeps_everything_it_had_and_adds_the_rest_once() {
        assert_eq!(merge("/a:/b", "/b:/c::/a:/d"), "/a:/b:/c:/d");
        assert_eq!(merge("", "/x"), "/x");
        assert_eq!(
            merge("/a", "/Users/ada/.local/share/remote-hand/instances/default/session-bin:/b"),
            "/a:/b",
            "a session's own bin directory is never inherited into every child"
        );
        // A Linux server started with nvm's node ahead of the system's: what a
        // re-read builds goes after, so `#!/usr/bin/env node` still finds nvm's.
        let inherited = "/home/ada/.nvm/versions/node/v22.3.0/bin:/usr/bin:/bin";
        let built = build(Path::new("/home/ada"), Some("/usr/bin:/bin"));
        let merged = merge(inherited, &built);
        assert!(merged.starts_with(inherited), "{merged}");
        assert!(merged.contains(":/home/ada/.local/bin"), "the base is added after: {merged}");
    }

    #[test]
    fn the_overlay_goes_last_once_and_only_if_it_exists() {
        let root = tempfile::tempdir().expect("tempdir");
        let picked = root.path().join("tools/bin");
        std::fs::create_dir_all(&picked).expect("dir");
        let gone = root.path().join("gone/bin");
        // macOS shows this folder as "Tools 24/25"; on disk it is "Tools 24:25".
        let colon = root.path().join("Tools 24:25/bin");
        std::fs::create_dir_all(&colon).expect("dir");
        let path = with_overlay("/usr/bin:/bin", &[picked.clone(), gone, picked.clone(), PathBuf::from("relative"), colon]);
        assert_eq!(path, format!("/usr/bin:/bin:{}", picked.display()), "never half a folder and a relative `25/bin`");
        assert_eq!(with_overlay(&path, &[picked]), path, "nothing is added twice");
    }

    #[test]
    fn the_newest_nvm_node_is_searched_first_and_older_ones_after() {
        let home = tempfile::tempdir().expect("tempdir");
        let system = home.path().join("system-bin");
        for version in ["v18.20.1", "v22.3.0", "v20.11.0", "not-a-version"] {
            std::fs::create_dir_all(home.path().join(".nvm/versions/node").join(version).join("bin")).expect("dir");
        }
        let candidates = fallback_candidates(ToolId::Claude, home.path(), std::slice::from_ref(&system));
        let nvm: Vec<String> = candidates
            .iter()
            .filter(|path| path.to_string_lossy().contains("/.nvm/"))
            .map(|path| {
                path.parent()
                    .and_then(Path::parent)
                    .and_then(Path::file_name)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(nvm, ["v22.3.0", "v20.11.0", "v18.20.1", "not-a-version"]);
        assert_eq!(candidates[0], home.path().join(".local/bin/claude"));
        assert_eq!(candidates[1], home.path().join(".claude/local/claude"), "the old local install");
        assert_eq!(candidates[2], system.join("claude"));
        let codex = fallback_candidates(ToolId::Codex, home.path(), &[]);
        assert!(!codex.iter().any(|path| path.to_string_lossy().contains(".claude/local")));
    }

    /// The case the fallbacks exist for: a login shell too slow to say where
    /// nvm is, and a `claude` installed under it.
    #[test]
    fn a_cli_under_nvm_is_found_off_the_path_as_a_fallback() {
        let home = tempfile::tempdir().expect("tempdir");
        let empty = home.path().join("empty-bin");
        std::fs::create_dir_all(&empty).expect("dir");
        let package = home
            .path()
            .join(".nvm/versions/node/v22.3.0/lib/node_modules/@anthropic-ai/claude-code/cli.js");
        marker(&package);
        let bin = home.path().join(".nvm/versions/node/v22.3.0/bin");
        std::fs::create_dir_all(&bin).expect("bin");
        std::os::unix::fs::symlink(&package, bin.join("claude")).expect("link");
        std::fs::create_dir_all(home.path().join(".nvm/versions/node/v20.0.0/bin")).expect("older node");

        let env = env_with_path(&empty.to_string_lossy());
        let search = Search {
            home: home.path(),
            env: &env,
            inherited_path: &empty.to_string_lossy(),
            overlay: &[],
            system_dirs: &[],
        };
        let found = locate(ToolId::Claude, &search).expect("found under nvm");
        assert_eq!(found.path, bin.join("claude").to_string_lossy());
        assert_eq!(found.via, FoundVia::Fallback);
        assert_eq!(found.method, InstallMethod::Npm, "the link leads into node_modules");
        assert!(locate(ToolId::Codex, &search).is_none(), "nothing else is invented");
    }

    #[test]
    fn a_cli_on_the_path_is_found_there_and_an_adopted_directory_says_so() {
        let home = tempfile::tempdir().expect("tempdir");
        let inherited = home.path().join("inherited");
        let adopted = home.path().join("adopted");
        marker(&inherited.join("codex"));
        marker(&adopted.join("claude"));
        let path = format!("{}:{}", inherited.display(), adopted.display());
        let env = env_with_path(&path);
        let overlay = [adopted.clone()];
        let search = Search {
            home: home.path(),
            env: &env,
            inherited_path: &inherited.to_string_lossy(),
            overlay: &overlay,
            system_dirs: &[],
        };
        assert_eq!(locate(ToolId::Codex, &search).expect("codex").via, FoundVia::Path);
        let claude = locate(ToolId::Claude, &search).expect("claude");
        assert_eq!(claude.via, FoundVia::Picked);
        assert_eq!(claude.path, adopted.join("claude").to_string_lossy());
    }

    #[test]
    fn the_install_method_is_read_off_where_the_binary_really_lives() {
        let home = tempfile::tempdir().expect("tempdir");
        let root = home.path();
        let cases = [
            (ToolId::Claude, ".local/share/claude/versions/2.1.300", InstallMethod::Native),
            (ToolId::Codex, ".codex/packages/standalone/0.163.0/codex", InstallMethod::Native),
            (ToolId::Claude, "homebrew/Caskroom/claude-code/2.1.300/claude", InstallMethod::Brew),
            (ToolId::Codex, "homebrew/Cellar/codex/0.163.0/bin/codex", InstallMethod::Brew),
            (ToolId::Codex, "lib/node_modules/@openai/codex/bin/codex.js", InstallMethod::Npm),
            (ToolId::Claude, "somewhere/else/claude", InstallMethod::Unknown),
            (ToolId::Claude, ".codex/packages/standalone/claude", InstallMethod::Unknown),
        ];
        for (index, (tool, target, expected)) in cases.into_iter().enumerate() {
            let target = root.join(target);
            marker(&target);
            let link = root.join(format!("bin-{index}")).join(tool.binary());
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert_eq!(install_method(tool, &link), expected, "{}", target.display());
        }
    }
}
