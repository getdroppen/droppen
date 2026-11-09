//! Handing the default browser an address with a secret in it, keeping the
//! secret off every command line — where another user's `ps` could read it.
//!
//! *Open in Browser* gives the browser the server's page with a one-time
//! sign-in in its fragment (`#local-open=<token>`, [`crate::server::local_open_grant`]).
//! `open <url>` would put that in `open`'s arguments; AppleScript's
//! `open location`, which `osascript` reads from its standard input, puts it
//! nowhere but the browser — the way the server's own `open` command does it.
//!
//! Linux has no such channel: whatever opens an address there — `xdg-open`,
//! `gio open`, the OpenURI portal over D-Bus — starts the browser, or a
//! launcher of it, with the address in its arguments, and `/proc/<pid>/cmdline`
//! is every local user's to read. A browser that was not running yet keeps it
//! there for as long as it runs. So the address goes into a page that sends the
//! browser on to it, readable by this user alone ([`PageDir`]), and the browser
//! is given the page's path, which is no secret (`src-tauri/src/linux_browser.rs`).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

/// How long a redirect page is kept: the life of the one-time token in it
/// (the server's `local_open::TTL`), past which it opens nothing.
pub const PAGE_LIFETIME: Duration = Duration::from_secs(60);

/// The AppleScript that opens `url` in the default browser.
pub fn open_location_script(url: &str) -> String {
    format!("open location {}\n", applescript_string(url))
}

/// `text` as an AppleScript string literal.
fn applescript_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Open `url` in the default browser through `osascript` on its standard
/// input: the address is in no process's arguments. An error says why
/// without the address, which may go to a log.
pub fn open_secret(url: &str) -> Result<(), String> {
    let mut child = Command::new("osascript")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot run osascript: {}", error.kind()))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(open_location_script(url).as_bytes())
            .map_err(|error| format!("cannot hand osascript the address: {}", error.kind()))?;
    }
    let status = child.wait().map_err(|error| format!("osascript did not finish: {}", error.kind()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("osascript could not open the browser ({status})"))
    }
}

/// A page that sends the browser on to `url` at once, as the server's own
/// `open` writes one: `url` a script string, its `<` escaped so that nothing
/// in it can end the script.
pub fn redirect_page(url: &str) -> String {
    let quoted = serde_json::to_string(url).unwrap_or_default().replace('<', "\\u003c");
    format!(
        "<!doctype html><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\"><title>Droppen</title>\n<script>location.replace({quoted})</script>\n"
    )
}

/// Where the redirect pages go: `<bundle id>/` in the user's runtime directory
/// (`$XDG_RUNTIME_DIR`: this user's own, `0700`, in memory, emptied at
/// logout), so a token is never on a disk and nobody else can list or read it.
/// The bundle id is this build's, as for the app's other directories
/// ([`crate::identity`]): an end-to-end build's pages are in
/// `app.droppen.desktop.e2e/`.
#[derive(Debug)]
pub struct PageDir {
    dir: PathBuf,
}

impl PageDir {
    /// The pages' directory in `runtime`, made `0700` if it is not there —
    /// only if `runtime` is a directory of this user's that nobody else can
    /// reach, and so is the one made in it.
    pub fn in_runtime_dir(runtime: Option<&Path>) -> Result<Self, String> {
        use std::os::unix::fs::DirBuilderExt;
        let runtime = runtime
            .filter(|path| path.is_absolute())
            .ok_or("no runtime directory ($XDG_RUNTIME_DIR)")?;
        private_to_me(runtime).map_err(|why| format!("the runtime directory {}: {why}", runtime.display()))?;
        let dir = runtime.join(crate::BUNDLE_ID);
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(format!("cannot make {}: {error}", dir.display())),
        }
        private_to_me(&dir).map_err(|why| format!("{}: {why}", dir.display()))?;
        Ok(Self { dir })
    }

    /// The directory.
    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// A new page that sends the browser to `url`: a file of its own (never
    /// one that was there), `0600`, named `open-<16 hex>.html`.
    pub fn write(&self, url: &str) -> Result<PathBuf, String> {
        use std::os::unix::fs::OpenOptionsExt;
        let mut name = [0u8; 8];
        getrandom::fill(&mut name).map_err(|error| format!("no random name for the page: {error}"))?;
        let name: String = name.iter().map(|b| format!("{b:02x}")).collect();
        let path = self.dir.join(format!("open-{name}.html"));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| format!("cannot write the page: {error}"))?;
        if let Err(error) = file.write_all(redirect_page(url).as_bytes()) {
            let _ = std::fs::remove_file(&path);
            return Err(format!("cannot write the page: {error}"));
        }
        Ok(path)
    }

    /// Remove the pages written before `written_before`, and say how many:
    /// at each open those past their [`PAGE_LIFETIME`] (each is removed when
    /// its life ends; these an app that quit meanwhile left), and at the app's
    /// start every one an earlier run left.
    pub fn sweep(&self, written_before: SystemTime) -> usize {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return 0 };
        let mut removed = 0;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let ours = name.to_str().is_some_and(|name| name.starts_with("open-") && name.ends_with(".html"));
            let written = entry.metadata().and_then(|metadata| metadata.modified());
            if ours && written.is_ok_and(|written| written < written_before) && std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
        removed
    }
}

/// `why` with `secret` taken out wherever it is in it: what a hand-off's
/// error says goes to the log, and the token must never. None of the errors
/// here carry it; this keeps one added later from it.
pub fn without_secret(why: String, secret: &str) -> String {
    if secret.is_empty() || !why.contains(secret) {
        return why;
    }
    why.replace(secret, "<the one-time sign-in>")
}

/// Whether `dir` is a directory of this user's, not a symlink, that nobody
/// else can reach.
fn private_to_me(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let found = std::fs::symlink_metadata(dir).map_err(|error| error.to_string())?;
    if !found.is_dir() {
        return Err("not a directory".into());
    }
    // SAFETY: `geteuid` reads a process property and cannot fail.
    let me = unsafe { libc::geteuid() };
    if found.uid() != me {
        return Err(format!("it belongs to uid {}, not {me}", found.uid()));
    }
    if found.mode() & 0o077 != 0 {
        return Err(format!("its mode is {:04o}: others can reach it", found.mode() & 0o7777));
    }
    Ok(())
}

/// The id the document portal knows a sandboxed browser by, from its desktop
/// entry, as xdg-desktop-portal names it: `snap.<instance>` for a snap
/// (`X-SnapInstanceName`), the app id for a flatpak (`X-Flatpak`). `None` for
/// a browser that reads files as this user does.
pub fn sandbox_app_id(snap_instance: Option<&str>, flatpak: Option<&str>) -> Option<String> {
    match (snap_instance.filter(|name| !name.is_empty()), flatpak.filter(|id| !id.is_empty())) {
        (_, Some(flatpak)) => Some(flatpak.to_string()),
        (Some(snap), None) => Some(format!("snap.{snap}")),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &Path) -> u32 {
        std::fs::symlink_metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    #[test]
    fn the_address_goes_to_the_script_as_one_string() {
        assert_eq!(
            open_location_script("http://127.0.0.1:5555/#local-open=abc-DEF_123"),
            "open location \"http://127.0.0.1:5555/#local-open=abc-DEF_123\"\n"
        );
        // Whatever it holds, it stays one string literal.
        assert_eq!(open_location_script(r#"a"b\c"#), "open location \"a\\\"b\\\\c\"\n");
    }

    #[test]
    fn the_page_sends_the_browser_to_the_address_and_nothing_else() {
        let page = redirect_page("http://127.0.0.1:5555/#local-open=abc-DEF_123");
        assert!(
            page.contains("<script>location.replace(\"http://127.0.0.1:5555/#local-open=abc-DEF_123\")</script>"),
            "{page}"
        );
        assert!(page.contains("no-referrer"), "{page}");
        // Whatever the address holds, the script it is in goes on to the end.
        let page = redirect_page("x\"</script><script>alert(1)//");
        assert_eq!(page.matches("</script>").count(), 1, "{page}");
        assert!(page.contains(r#"location.replace("x\"\u003c/script>\u003cscript>alert(1)//")"#), "{page}");
    }

    #[test]
    fn pages_go_only_into_a_runtime_directory_nobody_else_can_reach() {
        assert!(PageDir::in_runtime_dir(None).is_err(), "no runtime directory");
        assert!(PageDir::in_runtime_dir(Some(Path::new("relative/run"))).is_err());

        let root = tempfile::tempdir().expect("tempdir");
        assert!(PageDir::in_runtime_dir(Some(&root.path().join("missing"))).is_err());

        let open = root.path().join("open");
        std::fs::create_dir(&open).expect("dir");
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let refused = PageDir::in_runtime_dir(Some(&open)).expect_err("others can reach it");
        assert!(refused.contains("0755"), "{refused}");
        assert!(!open.join(crate::BUNDLE_ID).exists(), "nothing is made in it");

        let runtime = root.path().join("run");
        std::fs::create_dir(&runtime).expect("dir");
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&runtime, &link).expect("symlink");
        assert!(PageDir::in_runtime_dir(Some(&link)).is_err(), "a symlink");

        // A directory of the app's name left open is not used either.
        let loose = runtime.join(crate::BUNDLE_ID);
        std::fs::create_dir(&loose).expect("dir");
        std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o777)).expect("chmod");
        assert!(PageDir::in_runtime_dir(Some(&runtime)).is_err(), "its own directory open to others");
        std::fs::remove_dir(&loose).expect("rmdir");

        let pages = PageDir::in_runtime_dir(Some(&runtime)).expect("a private runtime directory");
        // This build's own name, as for the app's other directories.
        let name = if cfg!(feature = "e2e") {
            "app.droppen.desktop.e2e"
        } else {
            "app.droppen.desktop"
        };
        assert_eq!(pages.path(), runtime.join(name));
        assert_eq!(mode(pages.path()), 0o700);
    }

    #[test]
    fn each_page_is_a_new_private_file() {
        let runtime = tempfile::tempdir().expect("tempdir");
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).expect("chmod");
        let pages = PageDir::in_runtime_dir(Some(runtime.path())).expect("pages");
        let url = "http://127.0.0.1:5555/#local-open=abc";
        let first = pages.write(url).expect("a page");
        let second = pages.write(url).expect("another");
        assert_ne!(first, second);
        for page in [&first, &second] {
            assert_eq!(page.parent(), Some(pages.path()));
            let name = page.file_name().and_then(|name| name.to_str()).expect("a name");
            let hex = name
                .strip_prefix("open-")
                .and_then(|rest| rest.strip_suffix(".html"))
                .expect("open-….html");
            assert!(hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit()), "{name}");
            assert_eq!(mode(page), 0o600);
            assert_eq!(std::fs::read_to_string(page).expect("read"), redirect_page(url));
        }
    }

    #[test]
    fn the_sweep_removes_only_pages_past_their_life() {
        let runtime = tempfile::tempdir().expect("tempdir");
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).expect("chmod");
        let pages = PageDir::in_runtime_dir(Some(runtime.path())).expect("pages");
        let old = pages.write("http://127.0.0.1:1/#local-open=old").expect("old");
        let fresh = pages.write("http://127.0.0.1:1/#local-open=fresh").expect("fresh");
        let other = pages.path().join("notes.txt");
        std::fs::write(&other, "x").expect("other");
        let now = SystemTime::now();
        let long_ago = now - PAGE_LIFETIME - Duration::from_secs(1);
        for path in [&old, &other] {
            std::fs::File::options()
                .write(true)
                .open(path)
                .expect("open")
                .set_modified(long_ago)
                .expect("backdate");
        }
        // At an open: the pages past their life.
        assert_eq!(pages.sweep(now - PAGE_LIFETIME), 1);
        assert!(!old.exists(), "a page past its life goes");
        assert!(fresh.exists(), "a live one stays");
        assert!(other.exists(), "and what is not a page stays");
        // At the app's start: every page an earlier run left.
        assert_eq!(pages.sweep(SystemTime::now() + Duration::from_secs(1)), 1);
        assert!(!fresh.exists());
        assert!(other.exists());
    }

    #[test]
    fn no_error_carries_the_sign_in() {
        let token = "tok-0123456789abcdefghijklmnopqrstuvwxyzABCDE";
        let url = format!("http://127.0.0.1:5555/#local-open={token}");
        let runtime = tempfile::tempdir().expect("tempdir");
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).expect("chmod");
        let pages = PageDir::in_runtime_dir(Some(runtime.path())).expect("pages");
        std::fs::remove_dir(pages.path()).expect("rmdir");
        let refused = pages.write(&url).expect_err("no directory to write in");
        assert!(!refused.contains(token), "{refused}");

        assert_eq!(
            without_secret(format!("could not open {url}"), token),
            "could not open http://127.0.0.1:5555/#local-open=<the one-time sign-in>"
        );
        assert_eq!(without_secret("no default browser".into(), token), "no default browser");
        assert_eq!(without_secret("anything".into(), ""), "anything", "an empty secret is in everything");
    }

    #[test]
    fn a_sandboxed_browser_is_known_by_its_portal_app_id() {
        assert_eq!(sandbox_app_id(Some("firefox"), None).as_deref(), Some("snap.firefox"));
        assert_eq!(sandbox_app_id(Some("chromium_beta"), None).as_deref(), Some("snap.chromium_beta"));
        assert_eq!(sandbox_app_id(None, Some("org.mozilla.firefox")).as_deref(), Some("org.mozilla.firefox"));
        assert_eq!(sandbox_app_id(None, None), None);
        assert_eq!(sandbox_app_id(Some(""), Some("")), None, "an empty key says nothing");
    }
}
