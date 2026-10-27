//! End-to-end test hooks — compiled only with the `e2e` feature
//! (`desktop/build.sh --e2e`); a release build has none of this.
//!
//! With `DROPPEN_DESKTOP_TEST_CONTROL=<dir>` set, the app watches `<dir>/command`
//! and runs what it says, appending one JSON line per command to
//! `<dir>/results.jsonl`. The directory must already exist, belong to the user
//! running the app and be private (0700), or the hook stays off. It only does
//! what a person can do from the window, the menu bar or the keyboard, plus
//! reporting state and snapshotting the web view (screen capture needs a
//! permission an automated run does not have). Write `command` whole and
//! rename it into place: the file is read and removed as soon as it is seen,
//! so one still being written can be read empty and dropped.
//!
//! | command | does |
//! |---|---|
//! | `close` | what the window's close button does (a close request) |
//! | `show` | what *Open Droppen* in the menu bar does |
//! | `quit` | what *Quit Droppen* in the menu bar does |
//! | `autostart` | what *Start at Login* does (toggles it) |
//! | `open-in-browser` | what *Open in Browser* does (this build only logs what it would open) |
//! | `retry` | what *Try again* on the app's page does |
//! | `state` | reports window visibility and URL kind, the menu-bar count, the phase, port and server pid |
//! | `snapshot <file.png>` | writes the main window's web content to a PNG |
//! | `js <code>` | runs `code` in the main window's page (for setting up a case) |
//! | `probe` | reports the [`probe`]'s reading of the page now, without a page load (the first-run guide's steps are one page) |
//! | `update-check` | what *Check for Updates…* does: an asked-for check |
//! | `update-notes` | chooses *Release Notes* |
//! | `menu-snapshot <file.png>` | opens the menu-bar menu, writes it to a PNG, closes it |
//!
//! A command may start with `@<id> ` (`@7 probe`): its result line then
//! carries `"id": "<id>"`. `snapshot`, `menu-snapshot` and `probe` answer
//! later than the next command may be read, so a script that gave up waiting
//! for one tells its late answer apart from the next command's by the id.
//!
//! `state` also reports the version and the update: its status, what its menu
//! items say, the menu-bar dot and whether a download is kept.
//!
//! The same build logs a probe of what the web view supports after each page
//! load ([`probe`]) — feature flags only, never the page's text.

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tauri::{AppHandle, Manager, Runtime, WebviewWindow};

use crate::shell::Shell;

/// The environment variable naming the control directory.
pub const ENV: &str = "DROPPEN_DESKTOP_TEST_CONTROL";

/// Whether `dir` may be a control directory: a real directory (not a link),
/// owned by this user, readable and writable by nobody else.
fn private_dir(dir: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    if !meta.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    if meta.uid() != uid {
        return Err(format!("{} belongs to uid {}, not {uid}", dir.display(), meta.uid()));
    }
    let mode = meta.permissions().mode() & 0o777;
    if mode != 0o700 {
        return Err(format!("{} is {mode:o}, not 700", dir.display()));
    }
    Ok(())
}

/// Start watching, if the variable names an acceptable directory.
pub fn start<R: Runtime>(app: &AppHandle<R>) {
    let Some(dir) = std::env::var_os(ENV).map(PathBuf::from) else { return };
    if let Err(why) = private_dir(&dir) {
        log::error!("test control refused: {why}");
        return;
    }
    log::warn!("test control is on: {}", dir.display());
    let app = app.clone();
    std::thread::spawn(move || loop {
        let command_file = dir.join("command");
        if let Ok(command) = std::fs::read_to_string(&command_file) {
            let _ = std::fs::remove_file(&command_file);
            let (id, command) = split_id(command.trim());
            if !command.is_empty() {
                let reply = Reply {
                    dir: dir.clone(),
                    id: id.map(str::to_string),
                };
                run(&app, &reply, command);
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    });
}

/// A command line's id (`@<id> `), if it has one, and the command.
fn split_id(line: &str) -> (Option<&str>, &str) {
    match line.strip_prefix('@') {
        Some(rest) => {
            let (id, command) = rest.split_once(' ').unwrap_or((rest, ""));
            (Some(id), command.trim_start())
        }
        None => (None, line),
    }
}

/// Where one command's result goes: a line of `results.jsonl`, with the
/// command's id when it came with one.
#[derive(Clone)]
struct Reply {
    dir: PathBuf,
    id: Option<String>,
}

impl Reply {
    fn send(&self, mut value: serde_json::Value) {
        use std::io::Write;
        if let (Some(id), Some(fields)) = (&self.id, value.as_object_mut()) {
            fields.insert("id".to_string(), json!(id));
        }
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join("results.jsonl")) {
            let _ = writeln!(file, "{value}");
        }
    }
}

fn run<R: Runtime>(app: &AppHandle<R>, reply: &Reply, command: &str) {
    let (verb, argument) = command.split_once(' ').unwrap_or((command, ""));
    log::info!("test control: {verb}");
    match verb {
        "close" => {
            if let Some(window) = app.get_webview_window(crate::window::MAIN) {
                // Emits CloseRequested exactly as the close button does.
                let _ = window.close();
            }
            reply.send(json!({ "command": "close", "ok": true }));
        }
        "show" => {
            crate::window::show(app);
            reply.send(json!({ "command": "show", "ok": true }));
        }
        "quit" => {
            reply.send(json!({ "command": "quit", "ok": true }));
            crate::tray::quit(app);
        }
        "autostart" => {
            let on = crate::tray::toggle_autostart(app);
            reply.send(json!({ "command": "autostart", "ok": true, "on": on }));
        }
        "open-in-browser" => {
            crate::tray::open_in_browser(app);
            reply.send(json!({ "command": "open-in-browser", "ok": true }));
        }
        "retry" => {
            app.state::<Arc<Shell>>().retry();
            reply.send(json!({ "command": "retry", "ok": true }));
        }
        "state" => {
            let window = app.get_webview_window(crate::window::MAIN);
            let shell = app.state::<Arc<Shell>>();
            let (message, phase) = match shell.phase() {
                crate::shell::Phase::Failed(message) => (Some(message), "failed"),
                other => (None, other.name()),
            };
            reply.send(
                json!({
                    "command": "state",
                    "windowVisible": window.as_ref().and_then(|w| w.is_visible().ok()),
                    "windowTitle": window.as_ref().and_then(|w| w.title().ok()),
                    // The scheme and port only: the URL can carry the token.
                    "windowScheme": window.as_ref().and_then(|w| w.url().ok()).map(|url| url.scheme().to_string()),
                    "windowPort": window.as_ref().and_then(|w| w.url().ok()).and_then(|url| url.port()),
                    "trayPresent": app.tray_by_id(crate::tray::TRAY).is_some(),
                    "trayTitle": crate::tray::current_title(),
                    "phase": phase,
                    "message": message,
                    "port": shell.port(),
                    "serverPid": shell.server_pid(),
                    "version": app.package_info().version.to_string(),
                    "update": crate::updater::status(app),
                    "updateMenu": crate::updater::testing::menu(app),
                }),
            );
        }
        "update-check" => {
            crate::updater::check(app, true);
            reply.send(json!({ "command": "update-check", "ok": true }));
        }
        "update-notes" => {
            crate::updater::menu_chosen(app, crate::updater::NOTES_ID);
            reply.send(json!({ "command": verb, "ok": true }));
        }
        "menu-snapshot" => menu_snapshot(app, reply, PathBuf::from(argument.trim())),
        "snapshot" => snapshot(app, reply, PathBuf::from(argument)),
        "js" => {
            let ok = app
                .get_webview_window(crate::window::MAIN)
                .map(|window| window.eval(argument).is_ok())
                .unwrap_or(false);
            reply.send(json!({ "command": "js", "ok": ok }));
        }
        "probe" => probe_now(app, reply),
        other => reply.send(json!({ "command": other, "ok": false, "error": "unknown command" })),
    }
}

/// Open the menu-bar menu, write it to a PNG and close it. The menu is opened
/// from a main-queue block rather than from the event loop's own callback, so
/// the loop keeps running this hook's next step while the menu tracks the
/// mouse; that step finds the app's own menu window and captures it (the app
/// may capture its own windows; a screen capture would need a permission a
/// test run does not have).
#[cfg(target_os = "macos")]
fn menu_snapshot<R: Runtime>(app: &AppHandle<R>, reply: &Reply, path: PathBuf) {
    use std::ffi::c_void;
    extern "C" {
        static _dispatch_main_q: c_void;
        fn dispatch_async_f(queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
    }
    type Job = Box<dyn FnOnce() + Send>;
    extern "C" fn run(context: *mut c_void) {
        // SAFETY: the box made below, handed over exactly once.
        let job = unsafe { Box::from_raw(context as *mut Job) };
        job();
    }
    let opener = app.clone();
    let job: Job = Box::new(move || {
        if let Some(tray) = opener.tray_by_id(crate::tray::TRAY) {
            let _ = tray.with_inner_tray_icon(|inner| inner.show_menu());
        }
    });
    // SAFETY: the main queue lives as long as the process; `run` frees the box.
    unsafe { dispatch_async_f(&raw const _dispatch_main_q, Box::into_raw(Box::new(job)) as *mut c_void, run) };
    std::thread::sleep(Duration::from_millis(800));
    let (reply, closer) = (reply.clone(), app.clone());
    let posted = app.run_on_main_thread(move || {
        let written = capture_menu_window(&path);
        if let Some(tray) = closer.tray_by_id(crate::tray::TRAY) {
            let _ = tray.with_inner_tray_icon(|inner| {
                if let (Some(item), Some(mtm)) = (inner.ns_status_item(), objc2::MainThreadMarker::new()) {
                    if let Some(menu) = item.menu(mtm) {
                        menu.cancelTracking();
                    }
                }
            });
        }
        reply.send(json!({ "command": "menu-snapshot", "path": path.display().to_string(), "ok": written.is_ok(), "error": written.err() }));
    });
    if let Err(error) = posted {
        log::warn!("test control: {error}");
    }
}

/// The open menu's window, captured by the window server (as drawn, vibrancy
/// included), else drawn by its own views.
#[cfg(target_os = "macos")]
fn capture_menu_window(path: &Path) -> Result<(), String> {
    use objc2::rc::Retained;
    use objc2::AllocAnyThread;
    use objc2_app_kit::{NSApplication, NSBitmapImageFileType, NSBitmapImageRep};
    use objc2_foundation::NSDictionary;
    use std::ffi::c_void;

    #[repr(C)]
    struct CGRect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGWindowListCreateImage(bounds: CGRect, options: u32, window: u32, image_options: u32) -> *mut c_void;
        fn CGImageRelease(image: *mut c_void);
    }
    const INCLUDING_WINDOW: u32 = 1 << 3;
    const BOUNDS_IGNORE_FRAMING: u32 = 1 << 0;
    const BEST_RESOLUTION: u32 = 1 << 3;

    let mtm = objc2::MainThreadMarker::new().ok_or("not the main thread")?;
    let windows = NSApplication::sharedApplication(mtm).windows();
    let menu = windows
        .iter()
        .filter(|window| window.isVisible() && window.class().name().to_string_lossy().contains("Menu"))
        .max_by(|a, b| a.frame().size.height.total_cmp(&b.frame().size.height))
        .ok_or_else(|| {
            let names: Vec<String> = windows
                .iter()
                .map(|window| window.class().name().to_string_lossy().into_owned())
                .collect();
            format!("no menu window among {names:?}")
        })?;
    let null = CGRect {
        x: f64::INFINITY,
        y: f64::INFINITY,
        width: 0.0,
        height: 0.0,
    };
    // SAFETY: a window number of this process's; the image is released below.
    let image = unsafe {
        CGWindowListCreateImage(
            null,
            INCLUDING_WINDOW,
            menu.windowNumber() as u32,
            BOUNDS_IGNORE_FRAMING | BEST_RESOLUTION,
        )
    };
    let bitmap: Option<Retained<NSBitmapImageRep>> = if image.is_null() {
        None
    } else {
        // SAFETY: `initWithCGImage:` takes a CGImageRef, which `image` is; the
        // bitmap retains it.
        let bitmap = unsafe { objc2::msg_send![NSBitmapImageRep::alloc(), initWithCGImage: image] };
        // SAFETY: made by CGWindowListCreateImage above, released once.
        unsafe { CGImageRelease(image) };
        bitmap
    };
    let bitmap = match bitmap {
        Some(bitmap) => bitmap,
        None => {
            let view = menu.contentView().ok_or("the menu window has no view")?;
            let bounds = view.bounds();
            let bitmap = view.bitmapImageRepForCachingDisplayInRect(bounds).ok_or("no bitmap")?;
            view.cacheDisplayInRect_toBitmapImageRep(bounds, &bitmap);
            bitmap
        }
    };
    // SAFETY: an empty properties dictionary is a valid argument.
    let png = unsafe { bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }.ok_or("no PNG")?;
    std::fs::write(path, png.to_vec()).map_err(|error| error.to_string())
}

#[cfg(not(target_os = "macos"))]
fn menu_snapshot<R: Runtime>(_app: &AppHandle<R>, reply: &Reply, _path: PathBuf) {
    reply.send(json!({ "command": "menu-snapshot", "ok": false, "error": "macOS only" }));
}

/// Log what the web view offers the web app, a few seconds after a load.
pub fn probe<R: Runtime>(window: &WebviewWindow<R>) {
    let window = window.clone();
    std::thread::spawn(move || {
        let _ = window.eval(PROBE_START);
        std::thread::sleep(Duration::from_secs(4));
        let _ = window.eval_with_callback(PROBE_READ, |json| log::info!("probe: {json}"));
    });
}

/// The probe's reading of the page as it is now, reported rather than logged:
/// the log's `probe:` lines stay one per page load, which the end-to-end
/// script counts to know a load has finished.
fn probe_now<R: Runtime>(app: &AppHandle<R>, reply: &Reply) {
    let Some(window) = app.get_webview_window(crate::window::MAIN) else {
        reply.send(json!({ "command": "probe", "ok": false, "error": "no window" }));
        return;
    };
    let answer = reply.clone();
    // The callback gets the script's value JSON-encoded: a string holding the probe's JSON.
    let evaluated = window.eval_with_callback(PROBE_READ, move |encoded| {
        let probe = serde_json::from_str::<String>(&encoded)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
        let ok = probe.is_some();
        answer.send(json!({ "command": "probe", "ok": ok, "probe": probe }));
    });
    if let Err(error) = evaluated {
        reply.send(json!({ "command": "probe", "ok": false, "error": error.to_string() }));
    }
}

const PROBE_START: &str = r#"(function () {
  window.__droppenProbe = { indexedDBOpen: "pending", serviceWorkers: "pending" };
  try {
    navigator.serviceWorker.getRegistrations().then(
      function (list) { window.__droppenProbe.serviceWorkers = list.length; },
      function (e) { window.__droppenProbe.serviceWorkers = "error"; }
    );
  } catch (e) { window.__droppenProbe.serviceWorkers = "throws"; }
  try {
    var request = indexedDB.open("droppen-desktop-probe", 1);
    request.onsuccess = function () { window.__droppenProbe.indexedDBOpen = "ok"; request.result.close(); indexedDB.deleteDatabase("droppen-desktop-probe"); };
    request.onerror = function () { window.__droppenProbe.indexedDBOpen = "error"; };
  } catch (e) { window.__droppenProbe.indexedDBOpen = "throws"; }
})();"#;

/// Feature flags, a few counts and which screen of the first-run guide is on
/// show (`guide`: its `data-screen`) — never the page's text, which is the
/// user's sessions.
const PROBE_READ: &str = r#"(function () {
  try {
    return JSON.stringify({
      path: location.pathname,
      hasToken: !!localStorage.getItem("rh.auth.token"),
      loginForm: !!document.querySelector('input[type="password"]'),
      title: document.title.replace(/^\(\d+\+?\) /, "(n) "),
      serviceWorker: "serviceWorker" in navigator,
      serviceWorkers: (window.__droppenProbe || {}).serviceWorkers,
      indexedDB: typeof indexedDB !== "undefined",
      indexedDBOpen: (window.__droppenProbe || {}).indexedDBOpen,
      webSocket: typeof WebSocket === "function",
      notification: typeof Notification === "function" ? Notification.permission : "none",
      pushManager: "PushManager" in window,
      setAppBadge: "setAppBadge" in navigator,
      secureContext: window.isSecureContext,
      clipboard: !!(navigator.clipboard && navigator.clipboard.writeText),
      sessionLinks: document.querySelectorAll('a[href*="/sessions/"]').length,
      movedNotice: !!document.getElementById("droppen-desktop-moved-notice"),
      iframes: document.querySelectorAll("iframe").length,
      securityDesktopApp: !!document.querySelector('[data-testid="security-desktop-app"]'),
      droppenSection: !!document.querySelector('[data-testid="droppen-section"]'),
      firstRunCard: !!document.querySelector('[data-testid="droppen-first-run"]'),
      guide: (function (g) { return g && g.getAttribute("data-screen"); })(document.querySelector('[data-testid="guide"]')),
      setupBar: document.querySelector('[data-testid="setup-resume-bar"]') ? "resume" : document.querySelector('[data-testid="setup-no-tool-bar"]') ? "noTool" : null,
      desktopUpdate: !!document.querySelector('[data-testid="desktop-update"]')
    });
  } catch (e) {
    return "probe failed";
  }
})()"#;

#[cfg(target_os = "macos")]
fn snapshot<R: Runtime>(app: &AppHandle<R>, reply: &Reply, path: PathBuf) {
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
    use objc2_foundation::{NSDictionary, NSError};
    use objc2_web_kit::WKWebView;

    let Some(window) = app.get_webview_window(crate::window::MAIN) else {
        reply.send(json!({ "command": "snapshot", "ok": false, "error": "no window" }));
        return;
    };
    let reply = reply.clone();
    let result = window.with_webview(move |platform| {
        let block = block2::RcBlock::new(move |image: *mut NSImage, _error: *mut NSError| {
            // SAFETY: WebKit hands the completion handler a valid NSImage or null.
            let written = unsafe { image.as_ref() }
                .and_then(|image| image.TIFFRepresentation())
                .and_then(|tiff| NSBitmapImageRep::imageRepWithData(&tiff))
                // SAFETY: an empty properties dictionary is a valid argument.
                .and_then(|bitmap| unsafe { bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) })
                .ok_or_else(|| "no image".to_string())
                .and_then(|png| std::fs::write(&path, png.to_vec()).map_err(|error| error.to_string()));
            reply.send(json!({ "command": "snapshot", "path": path.display().to_string(), "ok": written.is_ok(), "error": written.err() }));
        });
        // SAFETY: `inner` is the WKWebView this window owns, and this closure
        // runs on the main thread, where WebKit must be called.
        unsafe {
            let webview = &*(platform.inner() as *const WKWebView);
            webview.takeSnapshotWithConfiguration_completionHandler(None, &block);
        }
    });
    if let Err(error) = result {
        log::warn!("snapshot: {error}");
    }
}

#[cfg(not(target_os = "macos"))]
fn snapshot<R: Runtime>(_app: &AppHandle<R>, reply: &Reply, _path: PathBuf) {
    reply.send(json!({ "command": "snapshot", "ok": false, "error": "macOS only" }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_may_carry_an_id_for_its_result() {
        assert_eq!(split_id("@7 probe"), (Some("7"), "probe"));
        assert_eq!(split_id("@a-1 js document.title = \"x y\""), (Some("a-1"), "js document.title = \"x y\""));
        assert_eq!(split_id("snapshot /tmp/a b.png"), (None, "snapshot /tmp/a b.png"));
        assert_eq!(split_id("@lonely"), (Some("lonely"), ""));

        let root = tempfile::tempdir().expect("tempdir");
        let tagged = Reply {
            dir: root.path().to_path_buf(),
            id: Some("7".to_string()),
        };
        tagged.send(json!({ "command": "probe", "ok": true }));
        Reply { id: None, ..tagged }.send(json!({ "command": "state" }));
        let lines: Vec<serde_json::Value> = std::fs::read_to_string(root.path().join("results.jsonl"))
            .expect("results")
            .lines()
            .map(|line| serde_json::from_str(line).expect("json"))
            .collect();
        assert_eq!(lines, [json!({ "command": "probe", "ok": true, "id": "7" }), json!({ "command": "state" })]);
    }

    #[test]
    fn only_a_private_directory_of_ones_own_is_a_control_directory() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("ctl");
        assert!(private_dir(&dir).is_err(), "missing");
        std::fs::create_dir(&dir).expect("dir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert!(private_dir(&dir).is_err(), "readable by others");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        assert!(private_dir(&dir).is_ok());
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&dir, &link).expect("symlink");
        assert!(private_dir(&link).is_err(), "a link is not the directory");
        let file = root.path().join("file");
        std::fs::write(&file, "").expect("file");
        assert!(private_dir(&file).is_err(), "a file is not a directory");
    }
}
