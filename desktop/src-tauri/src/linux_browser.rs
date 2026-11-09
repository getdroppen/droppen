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

use std::os::unix::ffi::OsStringExt;
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
    browser
        .launch_uris(&[uri.as_str()], gio::AppLaunchContext::NONE)
        .map_err(|error| format!("cannot start the default browser, {id}: {error}"))
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
