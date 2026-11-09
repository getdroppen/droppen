//! Who the app is to macOS — and the end-to-end test build's own identity.
//!
//! Much of what the app keeps is keyed by its bundle identifier rather than by
//! `$HOME`, and so is shared by every copy with the same identifier whatever
//! home it runs under: the web view's data (`~/Library/WebKit/<id>`,
//! `~/Library/Caches/<id>`), its user defaults and saved window state,
//! LaunchServices' idea of which app is running, the Keychain item's service,
//! the launch agent's name, the privacy permissions macOS grants, and the
//! check that a leftover server is the app's own ([`crate::sidecar`]).
//!
//! An end-to-end test build (`desktop/build.sh --e2e`, feature `e2e`) runs on
//! a Mac where the real Droppen may be running, so it is somebody else
//! throughout ([`E2E`]): its own bundle identifier (and so all of the above),
//! its own name, server instance and port range — and it refuses to run under
//! the account's real home at all, or on Linux with an XDG data or state home
//! outside the one it has ([`check_scratch_home`]), which keeps everything
//! that does hang off `$HOME` (the support directory, the logs, the server's
//! data, the launch agent, `~/Downloads`) in the run's own folder.
//! `desktop/e2e-identity.sh` holds the same names for the test scripts, and a
//! test here keeps the two in step.

use std::ops::RangeInclusive;
use std::path::{Component, Path};

use crate::dirs::ShellDirs;

/// Everything that tells one Droppen app apart from another on the same Mac.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// `CFBundleIdentifier` (`identifier` in `tauri.conf.json`); also the
    /// Keychain service, the support directory's and the launch agent's name.
    pub bundle_id: &'static str,
    /// What the app is called: the bundle's name (`productName`), the logs
    /// folder `~/Library/Logs/<name>`, the menu-bar item's tooltip.
    pub name: &'static str,
    /// The server instance the app runs ([`crate::INSTANCE`]).
    pub instance: &'static str,
    /// Where a new port is chosen from ([`crate::port::RANGE`]).
    pub ports: RangeInclusive<u16>,
}

/// The released app.
pub const RELEASE: Identity = Identity {
    bundle_id: "app.droppen.desktop",
    name: "Droppen",
    instance: "droppen-app",
    ports: 18700..=18759,
};

/// An end-to-end test build: nothing in common with [`RELEASE`].
pub const E2E: Identity = Identity {
    bundle_id: "app.droppen.desktop.e2e",
    name: "Droppen E2E",
    instance: "droppen-app-e2e",
    ports: 18800..=18859,
};

/// This build's identity.
pub const THIS: Identity = if cfg!(feature = "e2e") { E2E } else { RELEASE };

const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

// Nothing of an end-to-end build's may be the release's.
const _: () = assert!(!same(E2E.bundle_id, RELEASE.bundle_id));
const _: () = assert!(!same(E2E.name, RELEASE.name));
const _: () = assert!(!same(E2E.instance, RELEASE.instance));
const _: () = assert!(*E2E.ports.start() > *RELEASE.ports.end() || *E2E.ports.end() < *RELEASE.ports.start());

/// An end-to-end build refuses to run unless `$HOME` (`dirs.home`) is a folder
/// of its own: not the account's home (`account`, the user database's,
/// [`crate::dirs::account_home`]), nor a folder above it. Under the real home
/// it would share the release's `~/Library/LaunchAgents`, `~/Downloads` and
/// the machine-wide files of the server's data directory. Nor unless its
/// support folder and logs are inside that folder: on Linux an
/// `$XDG_DATA_HOME` or `$XDG_STATE_HOME` elsewhere — a shell may export the
/// account's — would put them there, and Tauri the web view's data too.
pub fn check_scratch_home(dirs: &ShellDirs, account: Option<&Path>) -> Result<(), String> {
    let home = &dirs.home;
    if !home.is_absolute() {
        return Err(format!("HOME must be an absolute path, not {}", home.display()));
    }
    let Some(account) = account else {
        return Err("cannot read this account's home from the user database".into());
    };
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let (scratch, account) = (canonical(home), canonical(account));
    if account.starts_with(&scratch) {
        return Err(format!(
            "HOME is {}, the account's own home or a folder above it: an end-to-end test build runs only under a throwaway HOME",
            scratch.display()
        ));
    }
    for (variable, dir) in [("XDG_DATA_HOME", &dirs.support), ("XDG_STATE_HOME", &dirs.logs)] {
        let inside = dir
            .strip_prefix(home)
            .is_ok_and(|rest| rest.components().all(|part| matches!(part, Component::Normal(_))));
        if !inside {
            return Err(format!(
                "{variable} puts {} outside HOME ({}): an end-to-end test build keeps everything in its throwaway HOME",
                dir.display(),
                home.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dirs::Xdg;

    #[test]
    fn this_build_is_the_release_unless_it_is_an_end_to_end_one() {
        let expected = if cfg!(feature = "e2e") { &E2E } else { &RELEASE };
        assert_eq!(&THIS, expected);
        assert_eq!(crate::BUNDLE_ID, THIS.bundle_id);
        assert_eq!(crate::INSTANCE, THIS.instance);
        assert_eq!(crate::port::RANGE, THIS.ports);
    }

    #[test]
    fn the_release_identity_is_the_one_tauri_conf_names() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../src-tauri/tauri.conf.json")).expect("tauri.conf.json");
        assert_eq!(conf["identifier"], RELEASE.bundle_id);
        assert_eq!(conf["productName"], RELEASE.name);
    }

    #[test]
    fn the_end_to_end_identity_is_the_one_its_build_and_scripts_use() {
        // What `desktop/build.sh --e2e` merges into tauri.conf.json.
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../src-tauri/tauri.e2e.conf.json")).expect("tauri.e2e.conf.json");
        assert_eq!(conf["identifier"], E2E.bundle_id);
        assert_eq!(conf["productName"], E2E.name);
        // What the test scripts guard and clean up with.
        let script = include_str!("../../e2e-identity.sh");
        let value = |name: &str| {
            let prefix = format!("{name}=");
            script
                .lines()
                .find_map(|line| line.strip_prefix(&prefix))
                .map(|value| value.trim_matches(|c| c == '"' || c == '\''))
                .unwrap_or_else(|| panic!("{name} in e2e-identity.sh"))
                .to_string()
        };
        assert_eq!(value("E2E_BUNDLE_ID"), E2E.bundle_id);
        assert_eq!(value("E2E_NAME"), E2E.name);
        assert_eq!(value("E2E_INSTANCE"), E2E.instance);
        assert_eq!(value("RELEASE_BUNDLE_ID"), RELEASE.bundle_id);
        assert_eq!(value("RELEASE_NAME"), RELEASE.name);
        assert_eq!(value("RELEASE_INSTANCE"), RELEASE.instance);
    }

    /// Run `script` in bash with `desktop/e2e-identity.sh` sourced; its
    /// status and what it printed.
    #[cfg(target_os = "macos")]
    fn guard(script: &str, args: &[&std::ffi::OsStr]) -> (bool, String) {
        let library = Path::new(env!("CARGO_MANIFEST_DIR")).join("../e2e-identity.sh");
        let output = std::process::Command::new("/bin/bash")
            .arg("-c")
            .arg(format!("set -euo pipefail; . \"$0\"; {script}"))
            .arg(&library)
            .args(args)
            .output()
            .expect("bash");
        let said = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        (output.status.success(), said)
    }

    /// A stand-in app bundle: `id` in its Info.plist, `binary` as its executable.
    #[cfg(target_os = "macos")]
    fn bundle(root: &Path, name: &str, id: &str, binary: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let app = root.join(format!("{name}.app"));
        std::fs::create_dir_all(app.join("Contents/MacOS")).expect("bundle");
        std::fs::write(
            app.join("Contents/Info.plist"),
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>{id}</string></dict></plist>\n"),
        )
        .expect("plist");
        let exe = app.join("Contents/MacOS/Droppen");
        std::fs::write(&exe, binary).expect("binary");
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        app
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn the_test_scripts_start_only_an_end_to_end_build() {
        let root = tempfile::tempdir().expect("tempdir");
        let hooks = format!("DROPPEN_DESKTOP_TEST_CONTROL {}", E2E.bundle_id);
        let e2e = bundle(root.path(), "Droppen E2E", E2E.bundle_id, &hooks);
        let (ok, said) = guard("e2e_check_app \"$1\"", &[e2e.as_os_str()]);
        assert!(ok, "{said}");

        let release = bundle(root.path(), "Droppen", RELEASE.bundle_id, &hooks);
        let (ok, said) = guard("e2e_check_app \"$1\"", &[release.as_os_str()]);
        assert!(!ok && said.contains("the released app"), "{said}");
        // An end-to-end build from before it had an identity of its own.
        let old = bundle(root.path(), "Droppen old e2e", RELEASE.bundle_id, "DROPPEN_DESKTOP_TEST_CONTROL");
        assert!(!guard("e2e_check_app \"$1\"", &[old.as_os_str()]).0);
        let without = bundle(root.path(), "Droppen no identity", E2E.bundle_id, "DROPPEN_DESKTOP_TEST_CONTROL");
        let (ok, said) = guard("e2e_check_app \"$1\"", &[without.as_os_str()]);
        assert!(!ok && said.contains("without the end-to-end identity"), "{said}");
        let no_hooks = bundle(root.path(), "Droppen no hooks", E2E.bundle_id, E2E.bundle_id);
        assert!(!guard("e2e_check_app \"$1\"", &[no_hooks.as_os_str()]).0);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn the_test_scripts_write_only_where_the_real_app_keeps_nothing() {
        let root = tempfile::tempdir().expect("tempdir");
        let (ok, said) = guard("e2e_check_scratch \"$1\" run", &[root.path().join("run/home").as_os_str()]);
        assert!(ok, "{said}");
        let home = crate::dirs::account_home().expect("account home");
        let refused = [
            home.clone(),
            home.join("."),
            home.parent().expect("above the home").to_path_buf(),
            "/".into(),
            "/Applications/Droppen.app".into(),
            home.join("Library/Application Support/app.droppen.desktop"),
            home.join("Library/Application Support/app.droppen.desktop/update"),
            home.join("Library/WebKit/app.droppen.desktop"),
            home.join("Library/Logs/Droppen"),
            home.join("Library/LaunchAgents/app.droppen.desktop.plist"),
            home.join(".local/share/remote-hand/instances/droppen-app"),
            home.join(".config/remote-hand"),
            home.join(".local/share/cm-remote-hand"),
        ];
        for path in refused {
            let (ok, said) = guard("e2e_check_scratch \"$1\" run", &[path.as_os_str()]);
            assert!(!ok, "{} was let through: {said}", path.display());
        }
        // A run's support folder leaves room for the single-instance socket.
        let support = |root: &str| format!("{root}/home/Library/Application Support/{}", E2E.bundle_id);
        assert!(guard("e2e_check_socket_room \"$1\"", &[support("/private/tmp/droppen-upd.AbC123").as_ref()]).0);
        assert!(!guard("e2e_check_socket_room \"$1\"", &[support("/private/tmp/droppen-updater-test.AbC123").as_ref()]).0);
        // What it cleans up under the account's ~/Library is the e2e bundle id's only.
        let (ok, said) = guard("e2e_bundle_dirs", &[]);
        assert!(ok, "{said}");
        let dirs: Vec<&str> = said.lines().collect();
        assert!(!dirs.is_empty());
        for dir in dirs {
            assert!(dir.starts_with(&*home.to_string_lossy()), "{dir}");
            assert!(dir.ends_with(E2E.bundle_id) || dir.ends_with(&format!("{}.savedState", E2E.bundle_id)), "{dir}");
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn the_test_scripts_signal_only_their_own_processes() {
        let root = tempfile::tempdir().expect("tempdir");
        let folder = root.path().canonicalize().expect("canonical");
        // Not from the run's folder: left alone.
        let mut elsewhere = std::process::Command::new("/bin/sleep").arg("30").spawn().expect("sleep");
        let pid = elsewhere.id().to_string();
        let (ok, said) = guard("e2e_signal TERM \"$1\" \"$2\"", &[pid.as_ref(), folder.as_os_str()]);
        assert!(ok && said.contains("not this run's"), "{said}");
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(elsewhere.try_wait().expect("wait").is_none(), "a process from elsewhere was stopped");
        let _ = elsewhere.kill();
        let _ = elsewhere.wait();
        // From the run's folder: stopped. (A link, not a copy: macOS kills a
        // copy of a system binary run from elsewhere; `ps` names the path the
        // process was started by.)
        let link = folder.join("sleep");
        std::os::unix::fs::symlink("/bin/sleep", &link).expect("link");
        let mut ours = std::process::Command::new(&link).arg("30").spawn().expect("sleep from the folder");
        let pid = ours.id().to_string();
        let (listed, said) = guard("e2e_pids_in \"$1\"", &[folder.as_os_str()]);
        assert!(listed && said.lines().any(|line| line == pid), "{said}");
        let (ok, said) = guard("e2e_signal TERM \"$1\" \"$2\"", &[pid.as_ref(), folder.as_os_str()]);
        assert!(ok, "{said}");
        let status = ours.wait().expect("wait");
        assert!(!status.success(), "stopped by the signal");
        // A folder of `/` or nothing matches nothing.
        let (_, said) = guard("e2e_pids_in /; e2e_pids_in ''", &[]);
        assert!(said.trim().is_empty(), "{said}");
    }

    #[test]
    fn build_sh_reads_the_bundle_ids_from_here() {
        // desktop/build.sh checks the config and the built bundle against
        // these, read with its `identity_bundle_id`; src-tauri/build.rs reads
        // them the same way.
        let desktop = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let build_sh = std::fs::read_to_string(desktop.join("build.sh")).expect("build.sh");
        let function = build_sh
            .lines()
            .find(|line| line.starts_with("identity_bundle_id() {"))
            .expect("identity_bundle_id in build.sh");
        for (name, expected) in [("RELEASE", RELEASE.bundle_id), ("E2E", E2E.bundle_id)] {
            let output = std::process::Command::new("/bin/bash")
                .arg("-c")
                .arg(format!("{function}\nidentity_bundle_id {name}"))
                .env("SRC", desktop.join("..").canonicalize().expect("repo"))
                .output()
                .expect("bash");
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected, "{name}");
        }
    }

    #[test]
    fn an_end_to_end_build_runs_only_under_a_home_of_its_own() {
        let root = tempfile::tempdir().expect("tempdir");
        let account = root.path().join("Users/someone");
        let scratch = root.path().join("tmp/run/home");
        std::fs::create_dir_all(&account).expect("account");
        std::fs::create_dir_all(&scratch).expect("scratch");
        let check = |home: &Path, account: Option<&Path>| check_scratch_home(&ShellDirs::under(home), account);
        assert_eq!(check(&scratch, Some(&account)), Ok(()));
        // Inside the account's home is a folder of its own too.
        let inside = account.join("tmp/run");
        std::fs::create_dir_all(&inside).expect("inside");
        assert_eq!(check(&inside, Some(&account)), Ok(()));

        for refused in [account.clone(), root.path().join("Users"), root.path().to_path_buf()] {
            let error = check(&refused, Some(&account)).expect_err("refused");
            assert!(error.contains("throwaway HOME"), "{error}");
        }
        // Reached through a symbolic link, it is still the account's home.
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&account, &link).expect("link");
        assert!(check(&link, Some(&account)).is_err());
        assert!(check(&account.join("."), Some(&account)).is_err());

        assert!(check(Path::new("relative/home"), Some(&account)).is_err());
        assert!(check(&scratch, None).is_err(), "an account without a known home: refused");
    }

    #[test]
    fn an_end_to_end_build_keeps_its_xdg_directories_in_its_home() {
        let root = tempfile::tempdir().expect("tempdir");
        let account = root.path().join("home/someone");
        let scratch = root.path().join("tmp/run/home");
        let xdg = |data: &Path, state: &Path| Xdg::from_values(Some(data.into()), Some(state.into()));
        let check = |xdg: &Xdg| check_scratch_home(&ShellDirs::with_xdg(&scratch, xdg), Some(&account));
        assert_eq!(check(&xdg(&scratch.join("share"), &scratch.join("state"))), Ok(()));
        // The account's own, or one that climbs out.
        for outside in [
            xdg(&account.join(".local/share"), &scratch.join("state")),
            xdg(&scratch.join("share"), &account.join(".local/state")),
            xdg(&scratch.join("../share"), &scratch.join("state")),
        ] {
            if cfg!(target_os = "macos") {
                assert_eq!(check(&outside), Ok(()), "macOS reads no XDG variable");
            } else {
                let error = check(&outside).expect_err("refused");
                assert!(error.contains("XDG_") && error.contains("throwaway HOME"), "{error}");
            }
        }
    }
}
