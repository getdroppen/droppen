//! The main window, and what any of the app's windows may load.
//!
//! The window opens on the bundled start-up page, then loads the server's web
//! app from `http://127.0.0.1:<port>/` — the SPA the server binary embeds, so
//! the window shows exactly what that build serves. (The alternative, loading
//! the SPA from the app bundle and reaching the server over an IPC bridge, is
//! the `TauriShell` of `docs/local-aggregator.md` §6.2/§7.3: it would keep the
//! page's code inside the signed bundle rather than behind a port, at the cost
//! of a second copy of the web build and a bridge for every request. Later.)
//!
//! What a window may load ([`droppen_desktop_core::web`]):
//!
//! * any navigation, in any frame, is asked about ([`allow_navigation`]):
//!   pages and frames may load the web — WebKit does not say which frame is
//!   asking, and a page's iframe is the page's business — except that the
//!   server's login page is answered with a fresh login instead (in whichever
//!   window asked: that window goes where it wanted, and every other one is
//!   handed the same login at its page, [`hand_off`]), `mailto:` goes to the
//!   system, and odd schemes are refused silently;
//! * a plain link off the app opens in the default browser (`init.js` turns
//!   it into a new-window request); a *page* load that takes the window off
//!   the app anyway (a script setting `location`, a form, a redirect; WebKit
//!   reports page loads for the main frame alone, and only at commit) is
//!   handed to the browser — without its fragment — and the window is taken
//!   back to the app's last page from Rust ([`page_loaded`]);
//! * downloads are taken only from the app's own server, and marked as
//!   downloaded like a browser marks them; anything else goes to the browser
//!   ([`download`]);
//! * a new window the web app opens is a second app window when it is the
//!   app's own server (it shares the login), and the browser otherwise — a
//!   Droppen sign-in hand-off with its fragment, as [`crate::sign_in`] does.
//!
//! What WKWebView gives the web app (checked with the `e2e` build's probe):
//! `localStorage`, IndexedDB, websockets, the clipboard and the service worker
//! work; there is no Push API and no Badging API, and the web app asks for
//! `Notification` only together with Web Push, so it never asks here. The
//! unread count reaches the Dock badge and the menu bar through the title.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use droppen_desktop_core::web::{self, Route};
use tauri::webview::{DownloadEvent, NewWindowResponse, PageLoadEvent, PageLoadPayload};
use tauri::{AppHandle, Manager, Runtime, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::i18n::Lang;
use crate::shell::Shell;

/// The main window's label.
pub const MAIN: &str = "main";

/// The bundled start-up page.
const START_PAGE: &str = "tauri://localhost/index.html";

/// Runs at the start of every document (see `init.js`); `__DROPPEN_NONCE__` is
/// replaced with this launch's nonce, `__DROPPEN_BRIDGE__` with the update
/// bridge's own token (`updater::bridge_token`), `__DROPPEN_LANGUAGES__` with
/// the system's preferred languages (`i18n::Lang::script_list`).
const INIT_JS: &str = include_str!("init.js");

static WINDOWS: AtomicU32 = AtomicU32::new(1);

fn init_script<R: Runtime>(app: &AppHandle<R>) -> String {
    let nonce = app.state::<Arc<Shell>>().nonce().to_string();
    INIT_JS
        .replace("__DROPPEN_NONCE__", &nonce)
        .replace("__DROPPEN_BRIDGE__", &crate::updater::bridge_token(app))
        .replace("__DROPPEN_LANGUAGES__", &Lang::script_list(Lang::system_tags()))
}

/// Create the main window on the start-up page.
pub fn create_main<R: Runtime>(app: &AppHandle<R>, visible: bool) -> tauri::Result<()> {
    let builder = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("index.html".into()))
        .title("Droppen")
        .inner_size(1280.0, 820.0)
        .min_inner_size(720.0, 480.0)
        .center()
        .visible(visible);
    configure(app, builder, MAIN).build()?;
    Ok(())
}

/// The rules every app window loads by; `label` is the window's own.
fn configure<'a, R: Runtime>(
    app: &AppHandle<R>,
    builder: WebviewWindowBuilder<'a, R, AppHandle<R>>,
    label: &str,
) -> WebviewWindowBuilder<'a, R, AppHandle<R>> {
    let on_navigation = app.clone();
    let on_new_window = app.clone();
    let label = label.to_string();
    builder
        .initialization_script(init_script(app))
        .on_navigation(move |url| allow_navigation(&on_navigation, &label, url))
        .on_new_window(move |url, features| {
            let port = on_new_window.state::<Arc<Shell>>().port();
            if port.is_some_and(|port| web::is_server(&url, port)) {
                if web::login_redirect(&url, port.unwrap_or_default()).is_some() {
                    return NewWindowResponse::Deny;
                }
                let label = format!("view-{}", WINDOWS.fetch_add(1, Ordering::SeqCst));
                let builder = WebviewWindowBuilder::new(&on_new_window, &label, WebviewUrl::External("about:blank".parse().expect("about:blank")))
                    .title("Droppen")
                    .inner_size(1100.0, 760.0)
                    .window_features(features);
                return match configure(&on_new_window, builder, &label).build() {
                    Ok(window) => NewWindowResponse::Create { window },
                    Err(error) => {
                        log::warn!("cannot open a second window: {error}");
                        NewWindowResponse::Deny
                    }
                };
            }
            if droppen_desktop_core::sign_in::is_sign_in(&url) {
                crate::sign_in::opened(&on_new_window, &url);
            } else if matches!(url.scheme(), "http" | "https" | "mailto") {
                open_in_browser(&on_new_window, &url);
            }
            NewWindowResponse::Deny
        })
        .on_document_title_changed(|window, title| title_changed(&window, &title))
        .on_download(|webview, event| download(webview.app_handle(), event))
        .on_page_load(|window, payload| page_loaded(&window, &payload))
}

/// A navigation in any frame of the window labelled `label` (see the module
/// notes).
fn allow_navigation<R: Runtime>(app: &AppHandle<R>, label: &str, url: &Url) -> bool {
    let shell = app.state::<Arc<Shell>>();
    match web::route(url, shell.port()) {
        Route::Here => true,
        Route::Relogin(next) => {
            log::info!("the web app in window {label} asked for its login page; logging in again instead");
            shell.relogin(next, label.to_string());
            false
        }
        Route::System => {
            open_in_browser(app, url);
            false
        }
        Route::Refuse => {
            log::warn!("refused to load a {}: URL", url.scheme());
            false
        }
        Route::RestartToUpdate(token) => {
            crate::updater::restart_asked_by_page(app, &token);
            false
        }
    }
}

/// The last page of the app each window showed, as it may go back to it.
static LAST_APP_PAGE: Mutex<BTreeMap<String, Url>> = Mutex::new(BTreeMap::new());

/// A page (main-frame) load: one that left the app is opened in the default
/// browser instead, and the window goes back to the app.
fn page_loaded<R: Runtime>(window: &WebviewWindow<R>, payload: &PageLoadPayload<'_>) {
    let shell = window.app_handle().state::<Arc<Shell>>();
    let port = shell.port();
    let url = payload.url();
    if port.is_some_and(|port| web::is_server(url, port)) {
        LAST_APP_PAGE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(window.label().to_string(), web::for_return(url));
    }
    if let (Some(port), PageLoadEvent::Finished) = (port, payload.event()) {
        if web::is_server(url, port) {
            shell.loaded_on(port);
        }
    }
    match (web::leaves_the_app(url, port), payload.event()) {
        // `Started` is WebKit's commit — wry has no earlier, main-frame-only
        // hook — so the page outside the app has replaced the app's by now (a
        // plain link was turned into a new-window request by init.js before
        // getting here; this catches the rest: a script's `location.href = …`,
        // a form, a redirect). Hand it to the browser and take the window back
        // from Rust, never through the page, which could refuse.
        (true, PageLoadEvent::Started) => {
            open_in_browser(window.app_handle(), url);
            let back = LAST_APP_PAGE
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(window.label())
                .cloned()
                .or_else(|| port.and_then(|port| format!("http://127.0.0.1:{port}/").parse().ok()));
            match back {
                Some(back) => {
                    if let Err(error) = window.navigate(back) {
                        log::warn!("cannot take the window back to the app: {error}");
                    }
                }
                None => show_start_page(window.app_handle()),
            }
        }
        #[cfg(feature = "e2e")]
        (false, PageLoadEvent::Finished) if url.scheme() == "http" => crate::testctl::probe(window),
        _ => {}
    }
}

/// Where each download the app takes is being written, for marking it when
/// it is done.
static DOWNLOADS: Mutex<BTreeMap<String, Vec<PathBuf>>> = Mutex::new(BTreeMap::new());

/// A download: taken only from the app's own server (or a `blob:` its pages
/// made), into `~/Downloads`, and marked as downloaded (quarantined) like a
/// browser marks it; anything else goes to the browser.
fn download<R: Runtime>(app: &AppHandle<R>, event: DownloadEvent<'_>) -> bool {
    let port = app.state::<Arc<Shell>>().port();
    match event {
        DownloadEvent::Requested { url, destination } => {
            if !web::download_allowed(&url, port) {
                log::warn!(
                    "a download from {}://{} goes to the browser",
                    url.scheme(),
                    url.host_str().unwrap_or_default()
                );
                if matches!(url.scheme(), "http" | "https") {
                    open_in_browser(app, &url);
                }
                return false;
            }
            log::info!("download to {}", destination.display());
            DOWNLOADS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .entry(url.to_string())
                .or_default()
                .push(destination.clone());
            true
        }
        DownloadEvent::Finished { url, success, .. } => {
            // WebKit does not say where it wrote; the destination was chosen
            // (and recorded) when the download was asked for.
            let path = {
                let mut downloads = DOWNLOADS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let path = downloads
                    .get_mut(url.as_str())
                    .and_then(|paths| (!paths.is_empty()).then(|| paths.remove(0)));
                if downloads.get(url.as_str()).is_some_and(Vec::is_empty) {
                    downloads.remove(url.as_str());
                }
                path
            };
            match (success, path) {
                (true, Some(path)) => match quarantine(&path) {
                    Ok(()) => log::info!("download finished: {} (marked as downloaded)", path.display()),
                    Err(error) => log::warn!("download finished: {} (could not mark it: {error})", path.display()),
                },
                (true, None) => log::info!("download finished"),
                (false, _) => log::warn!("download failed"),
            }
            true
        }
        _ => true,
    }
}

/// Mark `path` with `com.apple.quarantine`, as a browser marks what it
/// downloads, so the system checks it before it is opened.
fn quarantine(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default();
        let value = format!("0081;{secs:08x};Droppen;");
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
        // SAFETY: both strings are NUL-terminated and the value's length is passed.
        let done = unsafe { libc::setxattr(path.as_ptr(), c"com.apple.quarantine".as_ptr(), value.as_ptr().cast(), value.len(), 0, 0) };
        if done != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Ok(())
    }
}

fn open_in_browser<R: Runtime>(app: &AppHandle<R>, url: &Url) {
    // Never with the fragment: on this window's URLs it can be the login.
    let url = web::for_browser(url);
    // The host only in the log: a URL can carry anything.
    let host = url.host_str().unwrap_or_default();
    #[cfg(feature = "e2e")]
    {
        let _ = app;
        log::info!("would open {}://{host} in the browser (e2e build: not opened)", url.scheme());
    }
    #[cfg(not(feature = "e2e"))]
    {
        use tauri_plugin_opener::OpenerExt;
        log::info!("opening {}://{host} in the browser", url.scheme());
        if let Err(error) = app.opener().open_url(url.as_str(), None::<&str>) {
            log::warn!("cannot open the browser: {error}");
        }
    }
}

/// The server's page, plain — no sign-in with it — in the default browser:
/// what *Open in Browser* opens once the instance has a user account (its
/// login page), and falls back to when it cannot open it signed in (that
/// browser then meets the setup or the login page).
pub fn open_page_in_browser<R: Runtime>(app: &AppHandle<R>, port: u16) {
    match Url::parse(&format!("http://127.0.0.1:{port}/")) {
        Ok(url) => open_in_browser(app, &url),
        Err(error) => log::warn!("cannot make the page's address for port {port}: {error}"),
    }
}

fn title_changed<R: Runtime>(window: &WebviewWindow<R>, title: &str) {
    let _ = window.set_title(if title.trim().is_empty() { "Droppen" } else { title });
    if window.label() != MAIN {
        return;
    }
    let unread = web::unread_from_title(title);
    crate::tray::show_unread(window.app_handle(), unread.as_deref());
    let _ = window.set_badge_count(unread.as_deref().and_then(web::badge_number));
}

/// Hand a fresh login to every app window ([`web::Handoff::url_for`]): the
/// one that asked goes where it wanted, the others back to the page they show
/// (the token the URLs carry is never logged).
pub fn hand_off<R: Runtime>(app: &AppHandle<R>, login: &web::Handoff) {
    if app.get_webview_window(MAIN).is_none() {
        log::error!("no main window to load the web app in");
    }
    for (label, window) in app.webview_windows() {
        let shown = window.url().ok();
        let Some(url) = login.url_for(&label, label == MAIN, shown.as_ref()) else {
            continue;
        };
        if label != MAIN && label != login.asking() {
            log::info!("window {label} is handed the new login too");
        }
        if let Err(error) = window.navigate(url) {
            log::error!("cannot load the web app in window {label}: {error}");
        }
    }
}

/// Put the main window back on the start-up page (which shows a failure with
/// *Try again*), unless it is already there.
pub fn show_start_page<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window(MAIN) else { return };
    if window.url().is_ok_and(|url| url.scheme() == "tauri") {
        return;
    }
    if let Ok(url) = START_PAGE.parse() {
        let _ = window.navigate(url);
    }
}

/// Bring the main window back (menu bar, Dock, a second launch).
pub fn show<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Hide the main window; the app and its server keep running.
pub fn hide<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.hide();
    }
}

/// Reload whichever window is focused, else the main one.
pub fn reload<R: Runtime>(app: &AppHandle<R>) {
    let focused = app.webview_windows().into_values().find(|window| window.is_focused().unwrap_or(false));
    if let Some(window) = focused.or_else(|| app.get_webview_window(MAIN)) {
        let _ = window.reload();
    }
}
