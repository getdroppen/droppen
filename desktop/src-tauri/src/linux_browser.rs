//! *Open in Browser*'s hand-off on Linux: the page with its one-time sign-in
//! in the default browser, the token on no command line.
//!
//! Whatever opens an address here starts the browser with it in its arguments,
//! which every local user can read (`droppen_desktop_core::browser`). So the
//! address goes into a redirect page only this user can read
//! ([`PageDir`], in `$XDG_RUNTIME_DIR`), and the default browser — the one
//! `https` links open in — is started with the page's path. A browser in a
//! sandbox (a snap, a flatpak) cannot read that directory: the page is added
//! to the document portal for that browser alone, read-only, and the browser
//! gets the document's path instead, as xdg-desktop-portal does for the files
//! it opens. The page is removed when the token in it dies
//! ([`PAGE_LIFETIME`]).
//!
//! In the app's own AppImage launch, the browser — and everything else the
//! app opens ([`crate::opening`]) — is started in the environment from before
//! the AppImage's AppRun, and at home ([`launch_outside`]): one that was not
//! running would otherwise load the app's bundled GTK and libraries
//! (`LD_LIBRARY_PATH`, `GTK_PATH`, `GIO_MODULE_DIR`…), and carry on in the
//! app's mount, with paths into it, after the app quits.

use std::ffi::{CString, OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use droppen_desktop_core::browser::{self, PageDir, PAGE_LIFETIME};
use gio::glib;
use gio::prelude::*;

const DOCUMENTS: &str = "org.freedesktop.portal.Documents";
const DOCUMENTS_PATH: &str = "/org/freedesktop/portal/documents";
/// How long the document portal may take to answer: D-Bus starts it on
/// demand.
const PORTAL_TIMEOUT_MS: i32 = 10_000;

/// Open `url` in the default browser through a redirect page. Blocking. An
/// error says why without the address, which may go to a log; the page is
/// gone by then.
pub fn open_secret(url: &str) -> Result<(), String> {
    let pages = page_dir()?;
    pages.sweep(SystemTime::now() - PAGE_LIFETIME);
    let browser = gio::AppInfo::default_for_uri_scheme("https").ok_or("there is no default browser")?;
    let page = pages.write(url)?;
    match hand_over(&browser, &page) {
        Ok(()) => {
            std::thread::spawn(move || {
                std::thread::sleep(PAGE_LIFETIME);
                let _ = std::fs::remove_file(page);
            });
            Ok(())
        }
        Err(why) => {
            let _ = std::fs::remove_file(&page);
            Err(why)
        }
    }
}

/// At the app's start, once it holds the app's lock: remove every page an
/// earlier run left (it quit or crashed within a minute of an *Open in
/// Browser*). Their tokens die with that run's server, which does not
/// outlive this start.
pub fn remove_earlier_pages() {
    let Ok(pages) = page_dir() else { return };
    let removed = pages.sweep(SystemTime::now());
    if removed > 0 {
        log::info!("removed {removed} sign-in page(s) an earlier run left for the browser");
    }
}

/// The pages' directory, in this session's `$XDG_RUNTIME_DIR`.
fn page_dir() -> Result<PageDir, String> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    PageDir::in_runtime_dir(runtime.as_deref())
}

/// Start `browser` with `page` — through the document portal, if it is in a
/// sandbox.
fn hand_over(browser: &gio::AppInfo, page: &Path) -> Result<(), String> {
    let id = browser.id().map(|id| id.to_string()).unwrap_or_else(|| browser.name().to_string());
    let entry = browser.downcast_ref::<gio::DesktopAppInfo>();
    let key = |name: &str| entry.and_then(|entry| entry.string(name));
    let target = match browser::sandbox_app_id(key("X-SnapInstanceName").as_deref(), key("X-Flatpak").as_deref()) {
        Some(app_id) => {
            let document = document_for(page, &app_id).map_err(|why| format!("cannot hand {app_id} the page through the document portal: {why}"))?;
            log::info!("the default browser, {id}, is sandboxed ({app_id}): the page goes through the document portal");
            document
        }
        None => page.to_path_buf(),
    };
    let uri = gio::File::for_path(&target).uri();
    let launched = match droppen_desktop_core::appimage::outside_this_appimage() {
        Some(env) => launch_outside(browser, &[uri.as_str()], &env),
        None => browser.launch_uris(&[uri.as_str()], gio::AppLaunchContext::NONE),
    };
    launched.map_err(|error| format!("cannot start the default browser, {id}: {error}"))
}

/// Open `uri` (an address, or a file's `file://` one) in the application the
/// desktop opens it with — the one `gio::AppInfo::launch_default_for_uri`
/// picks, for the URI's scheme or else for the file's type — started outside
/// the AppImage, in `env` ([`launch_outside`]).
pub fn open_outside(uri: &str, env: &[(OsString, OsString)]) -> Result<(), String> {
    let handler = match glib::uri_parse_scheme(uri).and_then(|scheme| gio::AppInfo::default_for_uri_scheme(&scheme)) {
        Some(handler) => handler,
        None => gio::File::for_uri(uri)
            .query_default_handler(gio::Cancellable::NONE)
            .map_err(|error| error.to_string())?,
    };
    launch_outside(&handler, &[uri], env).map_err(|error| error.to_string())
}

/// Start `app` with `uris` as a desktop would, from the app's own AppImage
/// launch: in `env` alone (the one from before AppRun,
/// `droppen_desktop_core::appimage::outside_this_appimage`), and at its home
/// (`droppen_desktop_core::appimage::start_folder`) unless the app's entry
/// names a folder (`Path`). It would otherwise start in the app's own folder,
/// the mount's `usr`, and with it as its `$PWD`: the AppImage's GIO starts it
/// through `sh`, which sets that from the folder it runs in.
fn launch_outside(app: &gio::AppInfo, uris: &[&str], env: &[(OsString, OsString)]) -> Result<(), glib::Error> {
    let context = gio::AppLaunchContext::new();
    set_environment(&context, env);
    let entry = app
        .downcast_ref::<gio::DesktopAppInfo>()
        .filter(|entry| entry.string("Path").is_none_or(|path| path.is_empty()));
    let folder = CString::new(droppen_desktop_core::appimage::start_folder(env).into_os_string().into_vec());
    match (entry, folder) {
        (Some(entry), Ok(folder)) => {
            // Run in the child, between its fork and its exec: a chdir alone,
            // to a path made before the fork.
            let start_there = move || {
                // SAFETY: `folder` is a NUL-terminated path the closure owns.
                unsafe { libc::chdir(folder.as_ptr()) };
            };
            // GIO's own flags for `launch_uris`.
            entry.launch_uris_as_manager(uris, Some(&context), glib::SpawnFlags::SEARCH_PATH, Some(Box::new(start_there)), None)
        }
        // An entry that names its folder starts there, GIO's chdir; an
        // `AppInfo` that is no entry (none on Linux) takes no setup.
        _ => app.launch_uris(uris, Some(&context)),
    }
}

/// Make `context` start what it launches in `env` alone: each variable of the
/// context's (the app's, at first) that `env` lacks unset, and each of `env`'s
/// set.
fn set_environment(context: &gio::AppLaunchContext, env: &[(OsString, OsString)]) {
    for entry in context.environment() {
        let bytes = entry.as_bytes();
        let name = OsStr::from_bytes(bytes.split(|byte| *byte == b'=').next().unwrap_or(bytes));
        if !env.iter().any(|(kept, _)| kept == name) {
            context.unsetenv(name);
        }
    }
    for (name, value) in env {
        context.setenv(name, value);
    }
}

/// `page`, added to the document portal for `app_id` to read and nobody else
/// (not kept past the portal's run), at the path that app sees it under.
fn document_for(page: &Path, app_id: &str) -> Result<PathBuf, String> {
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).map_err(|error| error.to_string())?;
    let fds = gio::UnixFDList::new();
    let file = std::fs::File::open(page).map_err(|error| error.to_string())?;
    let handle = fds.append(file).map_err(|error| error.to_string())?;
    // AddFull(ah fds, u flags, s app_id, as permissions): no flags — a new
    // document, forgotten when the portal exits.
    let message = gio::DBusMessage::new_method_call(Some(DOCUMENTS), DOCUMENTS_PATH, Some(DOCUMENTS), "AddFull");
    message.set_body(&(vec![glib::variant::Handle(handle)], 0u32, app_id, vec!["read"]).to_variant());
    message.set_unix_fd_list(Some(&fds));
    // By message rather than `call_with_unix_fd_list_sync`, whose binding
    // (gio 0.18) panics on a reply that carries no descriptors.
    let (reply, _) = bus
        .send_message_with_reply_sync(&message, gio::DBusSendMessageFlags::NONE, PORTAL_TIMEOUT_MS, gio::Cancellable::NONE)
        .map_err(|error| error.to_string())?;
    if reply.message_type() == gio::DBusMessageType::Error {
        return Err(reply.error_name().map(|name| name.to_string()).unwrap_or_else(|| "refused".into()));
    }
    let (ids, _): (Vec<String>, glib::VariantDict) = reply.body().and_then(|body| body.get()).ok_or("an unexpected answer")?;
    let id = ids.into_iter().next().filter(|id| !id.is_empty()).ok_or("no document")?;
    let mount = bus
        .call_sync(
            Some(DOCUMENTS),
            DOCUMENTS_PATH,
            DOCUMENTS,
            "GetMountPoint",
            None,
            Some(glib::VariantTy::new("(ay)").map_err(|error| error.to_string())?),
            gio::DBusCallFlags::NONE,
            PORTAL_TIMEOUT_MS,
            gio::Cancellable::NONE,
        )
        .map_err(|error| error.to_string())?;
    let (mut mount,): (Vec<u8>,) = mount.get().ok_or("an unexpected mount point")?;
    // A byte string with its NUL.
    if let Some(end) = mount.iter().position(|b| *b == 0) {
        mount.truncate(end);
    }
    let name = page.file_name().ok_or("the page has no name")?;
    Ok(PathBuf::from(std::ffi::OsString::from_vec(mount)).join(id).join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(context: &gio::AppLaunchContext) -> Vec<OsString> {
        let mut entries = context.environment();
        entries.sort();
        entries
    }

    /// The context starts what it launches in the environment it is given,
    /// and nothing of the app's own: what AppRun set is gone, what it added
    /// to is as given, whatever this test process has besides.
    #[test]
    fn a_launch_gets_exactly_the_environment_given() {
        let context = gio::AppLaunchContext::new();
        // The app's own: AppRun's, and whatever else this process has.
        context.setenv("LD_LIBRARY_PATH", "/tmp/.mount_DroppeAb12Cd/usr/lib/:");
        context.setenv("GTK_THEME", "Adwaita:light");
        context.setenv(
            "XDG_DATA_DIRS",
            "/tmp/.mount_DroppeAb12Cd/usr/share:/usr/share:/usr/local/share:/usr/share",
        );
        let given = [
            (OsString::from("HOME"), OsString::from("/home/me")),
            (OsString::from("PATH"), OsString::from("/usr/local/bin:/usr/bin:/bin")),
            (OsString::from("XDG_DATA_DIRS"), OsString::from("/usr/local/share:/usr/share")),
            (OsString::from("WITH_EQUALS"), OsString::from("a=b")),
        ];
        set_environment(&context, &given);
        assert_eq!(
            environment(&context),
            [
                "HOME=/home/me",
                "PATH=/usr/local/bin:/usr/bin:/bin",
                "WITH_EQUALS=a=b",
                "XDG_DATA_DIRS=/usr/local/share:/usr/share"
            ]
            .map(OsString::from)
            .to_vec()
        );
    }

    /// Start a probe entry — `Path` its folder, if any — in `env` through
    /// [`launch_outside`]; the folder it ran in and its `$GIVEN`.
    fn probe(dir: &Path, path: Option<&Path>, env: &[(OsString, OsString)]) -> String {
        let script = dir.join("probe.sh");
        let seen = dir.join("seen");
        let _ = std::fs::remove_file(&seen);
        std::fs::write(
            &script,
            format!("{{ pwd -P; echo \"$GIVEN\"; }} >'{0}.part' && mv '{0}.part' '{0}'\n", seen.display()),
        )
        .expect("probe");
        let keys = glib::KeyFile::new();
        keys.set_string("Desktop Entry", "Type", "Application");
        keys.set_string("Desktop Entry", "Name", "Probe");
        keys.set_string("Desktop Entry", "Exec", &format!("/bin/sh {} %u", script.display()));
        if let Some(path) = path {
            keys.set_string("Desktop Entry", "Path", &path.to_string_lossy());
        }
        let entry = gio::DesktopAppInfo::from_keyfile(&keys).expect("an entry");
        launch_outside(entry.upcast_ref(), &["https://example.com/"], env).expect("launched");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !seen.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        std::fs::read_to_string(&seen).expect("what the probe saw")
    }

    /// What the app starts starts at the home it is given, in the environment
    /// given, rather than in the app's own folder; an entry that names its
    /// folder starts there.
    #[test]
    fn a_launch_starts_at_home() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().canonicalize().expect("canonical");
        let home = dir.join("home");
        std::fs::create_dir(&home).expect("home");
        let env = [
            (OsString::from("HOME"), home.clone().into_os_string()),
            (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
            (OsString::from("GIVEN"), OsString::from("given")),
        ];
        assert_ne!(std::env::current_dir().expect("this folder"), home);
        assert_eq!(probe(&dir, None, &env), format!("{}\ngiven\n", home.display()));
        let own = dir.join("own");
        std::fs::create_dir(&own).expect("own");
        assert_eq!(probe(&dir, Some(&own), &env), format!("{}\ngiven\n", own.display()));
    }
}
