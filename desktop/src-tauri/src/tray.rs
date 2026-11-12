//! The menu-bar item and the app menu.
//!
//! Menu bar: the mark, the unread count beside it (the web app's own number,
//! read from its title), a dot when an update is ready, and a menu — where
//! Droppen is, open the window, open the page in this device's browser, the
//! update and its release notes, start at login (with a line to System
//! Settings → Login Items when macOS needs the user there,
//! [`crate::login_item`]), quit. Quit stops the server; closing the window
//! hides it where a tray host would bring it back (see below).
//!
//! App menu: the standard macOS menus, in the system's language, plus Reload —
//! a web view has no address bar to reload from — and the same update items
//! under *About Droppen* ([`crate::updater`]). macOS only: on Linux it would
//! be a menu bar inside every window, half of whose items GTK does not have,
//! and the web view's own editing keys work without it (Ctrl+R and F5 reload
//! the page there, `init.js`).
//!
//! The mark is macOS's black template image, which the system tints for a
//! light or dark menu bar. A Linux panel draws the image as it is, so there
//! the mark is white on the app's near-black tile: it reads on a light panel
//! as the tile and on a dark one as the mark.
//!
//! Linux desktops may have no tray host at all (stock GNOME without the
//! AppIndicator extension), and the item is a StatusNotifierItem, which an
//! XEmbed-only tray could not show either: [`host_present`] asks the session
//! bus for an `org.kde.StatusNotifierWatcher` owner. Without one nothing
//! would bring the window back, so closing it quits ([`quit_without_tray`])
//! and a launch at login shows the window instead of starting invisibly.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use droppen_desktop_core::login_item::{self, Note};
use tauri::image::Image;
#[cfg(target_os = "macos")]
use tauri::menu::{AboutMetadata, Submenu};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

use crate::i18n::{Lang, Words};
use crate::login_item::LoginItem;
use crate::shell::{Phase, Shell};

/// The menu-bar item's id.
pub const TRAY: &str = "droppen";
const OPEN: &str = "open";
const OPEN_IN_BROWSER: &str = "open-in-browser";
const AUTOSTART: &str = "autostart";
const LOGIN_ITEMS: &str = "login-items";
const QUIT: &str = "quit";
const RELOAD: &str = "reload";

/// The mark alone: macOS's black template image (the system tints it), and
/// Linux's white mark on its tile (`icons-src/tray.svg`, `tray-linux.svg`).
#[cfg(target_os = "macos")]
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray.png");
#[cfg(not(target_os = "macos"))]
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray-linux.png");

/// The mark with the update dot (`icons-src/tray-update.svg`,
/// `tray-update-linux.svg`).
#[cfg(target_os = "macos")]
const TRAY_UPDATE_ICON: &[u8] = include_bytes!("../icons/tray-update.png");
#[cfg(not(target_os = "macos"))]
const TRAY_UPDATE_ICON: &[u8] = include_bytes!("../icons/tray-update-linux.png");

/// Whether a tray host is there to show the item: macOS's menu bar, or on
/// Linux an owner of `org.kde.StatusNotifierWatcher` on the session bus.
///
/// Only the Status Notifier watcher counts: the item is a StatusNotifierItem,
/// so an XEmbed-only tray could not show it anyway, and treating that as "no
/// tray" is the honest answer. A bus that cannot be reached, or does not
/// answer within a second, is no host: the calls are cheap, but a close
/// should not wait long for one.
pub fn host_present() -> bool {
    #[cfg(target_os = "linux")]
    {
        sni_watcher_present()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

/// The tray host as the app started: [`host_present`] asked once, before the
/// first window (`lib.rs`), and kept for the words the app's pages show
/// (`init.js`, `__DROPPEN_TRAY_HOST__`). Kept so making a window never waits
/// on the bus: the probe can take up to a second, and a page's words are
/// about how the run started anyway. A close still asks afresh, so a host
/// started in the meantime brings the window back.
pub fn host_present_at_start() -> bool {
    static AT_START: OnceLock<bool> = OnceLock::new();
    *AT_START.get_or_init(host_present)
}

/// How long a close may wait for the bus: the whole probe, connect and auth
/// handshake included. `method_timeout` bounds the `NameHasOwner` call only,
/// and a bus that accepts the socket and then stalls never answers the
/// handshake, so the probe runs on a thread of its own and this is the wait
/// (`credentials/secret_service.rs`, L3, for the same reason).
#[cfg(target_os = "linux")]
const TRAY_HOST_DEADLINE: Duration = Duration::from_secs(1);

/// Ask the session bus whether anything owns the Status Notifier watcher's
/// name. Main thread safe: it waits at most [`TRAY_HOST_DEADLINE`] for the
/// thread doing the round trip; a thread still stuck after that is left to
/// finish or fail on its own, and reads as no host now.
#[cfg(target_os = "linux")]
fn sni_watcher_present() -> bool {
    sni_watcher_present_at(None)
}

/// [`sni_watcher_present`]'s wait around [`sni_watcher_at`], for the session
/// bus or `address` (a test's).
#[cfg(target_os = "linux")]
fn sni_watcher_present_at(address: Option<&str>) -> bool {
    use std::sync::mpsc;

    let address = address.map(str::to_string);
    let (done, outcome) = mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new().name("tray-host".into()).spawn(move || {
        let _ = done.send(sni_watcher_at(address.as_deref()));
    });
    if let Err(error) = spawned {
        log::info!("no tray host: {error}");
        return false;
    }
    match outcome.recv_timeout(TRAY_HOST_DEADLINE) {
        Ok(Ok(present)) => present,
        Ok(Err(error)) => {
            log::info!("no tray host: {error}");
            false
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            log::info!("no tray host: the session bus did not answer within {}s", TRAY_HOST_DEADLINE.as_secs());
            false
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            log::info!("no tray host: the bus check ended without an answer");
            false
        }
    }
}

/// The round trip itself: `address` is the session bus, or a test's bus.
#[cfg(target_os = "linux")]
fn sni_watcher_at(address: Option<&str>) -> zbus::Result<bool> {
    let builder = match address {
        Some(address) => zbus::blocking::connection::Builder::address(address)?,
        None => zbus::blocking::connection::Builder::session()?,
    };
    let bus = builder.method_timeout(TRAY_HOST_DEADLINE).build()?;
    let reply = bus.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "NameHasOwner",
        &("org.kde.StatusNotifierWatcher",),
    )?;
    reply.body().deserialize()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn a_bus_that_never_answers_is_no_tray_host() {
        // Takes the connection and says nothing, not even to the handshake.
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("silent-bus");
        let _listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");
        let address = format!("unix:path={}", path.display());
        let started = Instant::now();
        assert!(!sni_watcher_present_at(Some(address.as_str())), "a silent bus has no watcher");
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    }
}

/// The menu items that change after start-up.
struct Items<R: Runtime> {
    words: &'static Words,
    menu: Menu<R>,
    status: MenuItem<R>,
    autostart: CheckMenuItem<R>,
    /// The line under *Start at Login* that opens System Settings → Login
    /// Items, while it is in the menu.
    login_items: MenuItem<R>,
    /// Where that line goes: [`LOGIN_ITEMS_POSITION`], after the update's
    /// items above *Start at Login*.
    login_items_at: usize,
    login: Mutex<LoginShown>,
    translocated: bool,
}

/// What the menu shows of the login item, and when it was last read.
#[derive(Default)]
struct LoginShown {
    /// The line's note, while it is in the menu.
    note: Option<Note>,
    /// The user's last *Start at Login* was a turn-on that failed.
    turn_on_failed: bool,
    /// What macOS said last, to log a change once.
    status: Option<login_item::Status>,
    /// When it was last read on a hover.
    read_at: Option<Instant>,
}

/// At most one status read per this long when the pointer reaches the item:
/// the read is a round trip to macOS on the main thread.
const HOVER_READS: Duration = Duration::from_secs(2);

/// Where the line to System Settings goes: right under *Start at Login* (in
/// the menu as built, before the update's items go in above it).
const LOGIN_ITEMS_POSITION: usize = 6;

/// Where the update's items go: right above *Start at Login*.
const UPDATE_ITEMS_POSITION: usize = 5;

/// Put the item in the menu bar and the menus in the app menu. `translocated`:
/// the app runs from a temporary App Translocation copy, so a login item
/// pointing at it would break — *Start at Login* is off until it is moved.
pub fn install<R: Runtime>(app: &AppHandle<R>, login: &LoginItem, translocated: bool) -> tauri::Result<()> {
    let words = Lang::system().words();
    #[cfg(target_os = "macos")]
    app.set_menu(app_menu(app, words)?)?;

    let status = MenuItem::with_id(app, "status", words.starting, false, None::<&str>)?;
    let open = MenuItem::with_id(app, OPEN, words.open, true, None::<&str>)?;
    let open_browser = MenuItem::with_id(app, OPEN_IN_BROWSER, words.open_in_browser, true, None::<&str>)?;
    login.prepare(app, translocated);
    let login_status = login.status(app);
    log::info!("start at login: the platform says {login_status:?}");
    let shown = login_item::shown(login_status, translocated, false);
    let autostart = CheckMenuItem::with_id(app, AUTOSTART, words.start_at_login, shown.enabled, shown.checked, None::<&str>)?;
    let login_items = MenuItem::with_id(app, LOGIN_ITEMS, words.allow_at_login, true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, QUIT, words.quit, true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &open,
            &open_browser,
            &PredefinedMenuItem::separator(app)?,
            &autostart,
            &PredefinedMenuItem::separator(app)?,
            &quit_item,
        ],
    )?;
    let update_items = crate::updater::menu_items(app);
    for (offset, item) in update_items.iter().enumerate() {
        menu.insert(item, UPDATE_ITEMS_POSITION + offset)?;
    }
    TrayIconBuilder::with_id(TRAY)
        .icon(Image::from_bytes(TRAY_ICON)?)
        // A template image is the system's to tint: macOS only.
        .icon_as_template(cfg!(target_os = "macos"))
        // "Droppen", or "Droppen E2E" for an end-to-end test build beside it.
        .tooltip(droppen_desktop_core::identity::THIS.name)
        .menu(&menu)
        .show_menu_on_left_click(true)
        // The user may have allowed (or switched off) the login item in System
        // Settings since: look again when the pointer reaches the item.
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Enter { .. } = event {
                refresh_login_item(tray.app_handle());
            }
        })
        .build(app)?;
    app.manage(Items {
        words,
        menu,
        status,
        autostart,
        login_items,
        login_items_at: LOGIN_ITEMS_POSITION + update_items.len(),
        login: Mutex::new(LoginShown {
            status: Some(login_status),
            ..LoginShown::default()
        }),
        translocated,
    });
    show_note(app, shown.note);

    // One handler for both menus.
    app.on_menu_event(|app, event| match event.id().as_ref() {
        OPEN => crate::window::show(app),
        OPEN_IN_BROWSER => open_in_browser(app),
        AUTOSTART => {
            toggle_autostart(app);
        }
        LOGIN_ITEMS => crate::login_item::open_login_items(),
        QUIT => quit(app),
        RELOAD => crate::window::reload(app),
        id @ (crate::updater::MENU_ID | crate::updater::NOTES_ID) => crate::updater::menu_chosen(app, id),
        _ => {}
    });
    Ok(())
}

#[cfg(target_os = "macos")]
fn app_menu<R: Runtime>(app: &AppHandle<R>, words: &Words) -> tauri::Result<Menu<R>> {
    let about = AboutMetadata {
        name: Some("Droppen".into()),
        version: Some(app.package_info().version.to_string()),
        ..Default::default()
    };
    let separator = || PredefinedMenuItem::separator(app);
    let droppen = Submenu::with_items(
        app,
        "Droppen",
        true,
        &[
            &PredefinedMenuItem::about(app, Some(words.about), Some(about))?,
            &separator()?,
            &PredefinedMenuItem::services(app, Some(words.services))?,
            &separator()?,
            &PredefinedMenuItem::hide(app, Some(words.hide))?,
            &PredefinedMenuItem::hide_others(app, Some(words.hide_others))?,
            &PredefinedMenuItem::show_all(app, Some(words.show_all))?,
            &separator()?,
            &PredefinedMenuItem::quit(app, Some(words.quit))?,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        words.edit,
        true,
        &[
            &PredefinedMenuItem::undo(app, Some(words.undo))?,
            &PredefinedMenuItem::redo(app, Some(words.redo))?,
            &separator()?,
            &PredefinedMenuItem::cut(app, Some(words.cut))?,
            &PredefinedMenuItem::copy(app, Some(words.copy))?,
            &PredefinedMenuItem::paste(app, Some(words.paste))?,
            &PredefinedMenuItem::select_all(app, Some(words.select_all))?,
        ],
    )?;
    let view = Submenu::with_items(
        app,
        words.view,
        true,
        &[
            &MenuItem::with_id(app, RELOAD, words.reload, true, Some("CmdOrCtrl+R"))?,
            &separator()?,
            &PredefinedMenuItem::fullscreen(app, Some(words.fullscreen))?,
        ],
    )?;
    let window = Submenu::with_items(
        app,
        words.window,
        true,
        &[
            &PredefinedMenuItem::minimize(app, Some(words.minimize))?,
            &PredefinedMenuItem::maximize(app, Some(words.zoom))?,
            &separator()?,
            &PredefinedMenuItem::close_window(app, Some(words.close_window))?,
        ],
    )?;
    // Under About Droppen, as macOS apps have it.
    for (offset, item) in crate::updater::menu_items(app).iter().enumerate() {
        droppen.insert(item, 1 + offset)?;
    }
    Menu::with_items(app, &[&droppen, &edit, &view, &window])
}

/// What *Start at Login* does: turn the login item on or off — macOS then
/// opens the app in the menu bar without a window at login. A turn-on that
/// macOS takes but has to be approved opens System Settings → Login Items.
/// Returns whether it is on now (registered, approved or not yet).
///
/// On the main thread only: it updates menu items, and Tauri's menu calls
/// block when made from another thread.
pub fn toggle_autostart<R: Runtime>(app: &AppHandle<R>) -> bool {
    let Some(login) = app.try_state::<LoginItem>() else { return false };
    let on = !login.status(app).registered();
    let turned = login.set(app, on);
    if let Some(items) = app.try_state::<Items<R>>() {
        items.login.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).turn_on_failed = on && !turned.ok;
    }
    if on && turned.status == login_item::Status::RequiresApproval {
        crate::login_item::open_login_items();
    }
    show_login_item(app, turned.status);
    turned.status.registered()
}

/// Read the login item's status again and show it — at most once per
/// [`HOVER_READS`]. On the main thread only, like [`show_login_item`].
pub fn refresh_login_item<R: Runtime>(app: &AppHandle<R>) {
    let (Some(login), Some(items)) = (app.try_state::<LoginItem>(), app.try_state::<Items<R>>()) else {
        return;
    };
    {
        let mut shown = items.login.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if shown.read_at.is_some_and(|at| at.elapsed() < HOVER_READS) {
            return;
        }
        shown.read_at = Some(Instant::now());
    }
    show_login_item(app, login.status(app));
}

/// Show `status` in the menu bar's login item. On the main thread only: the
/// menu calls below block when made from another thread.
fn show_login_item<R: Runtime>(app: &AppHandle<R>, status: login_item::Status) {
    let Some(items) = app.try_state::<Items<R>>() else { return };
    let turn_on_failed = {
        let mut shown = items.login.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if shown.status != Some(status) {
            // `notFound` included: it is shown as off, and only logged.
            log::info!("start at login: the platform says {status:?} (was {:?})", shown.status);
            shown.status = Some(status);
        }
        if status.registered() {
            shown.turn_on_failed = false;
        }
        shown.turn_on_failed
    };
    let shown = login_item::shown(status, items.translocated, turn_on_failed);
    let _ = items.autostart.set_checked(shown.checked);
    show_note(app, shown.note);
}

/// Put the line to System Settings under *Start at Login*, worded for `note`,
/// or take it out. On the main thread only: the menu calls below block when
/// made from another thread, and the main thread's tray handlers take the
/// same `items.login` lock.
fn show_note<R: Runtime>(app: &AppHandle<R>, note: Option<Note>) {
    // The line's text and its action are macOS's Login Items page. Linux has
    // no such page: a turn-on that failed is simply left unchecked.
    if cfg!(not(target_os = "macos")) {
        return;
    }
    let Some(items) = app.try_state::<Items<R>>() else { return };
    let mut login = items.login.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if login.note == note {
        return;
    }
    let result = match (login.note, note) {
        (shown, Some(note)) => {
            let text = match note {
                Note::NeedsApproval => items.words.allow_at_login,
                Note::TurnOnFailed => items.words.login_item_failed,
            };
            let _ = items.login_items.set_text(text);
            if shown.is_none() {
                items.menu.insert(&items.login_items, items.login_items_at)
            } else {
                Ok(())
            }
        }
        (Some(_), None) => items.menu.remove(&items.login_items),
        (None, None) => Ok(()),
    };
    match result {
        Ok(()) => login.note = note,
        Err(error) => log::warn!("cannot update the menu: {error}"),
    }
}

/// Whether an *Open in Browser* is under way: a second click meanwhile is
/// not another browser tab.
static OPENING: AtomicBool = AtomicBool::new(false);

/// What *Open in Browser* does: the page in this device's browser, signed in
/// while the instance has no user account ([`Shell::open_in_browser`]), on a
/// thread of its own; its login page once it has one; and if it cannot, the
/// plain page instead, and the log says why.
pub fn open_in_browser<R: Runtime>(app: &AppHandle<R>) {
    let Some(shell) = app.try_state::<Arc<Shell>>().map(|shell| Arc::clone(&shell)) else {
        return;
    };
    if OPENING.swap(true, Ordering::SeqCst) {
        log::info!("open in browser: already under way");
        return;
    }
    log::info!("open in browser, from the menu bar");
    let app = app.clone();
    std::thread::spawn(move || {
        match shell.open_in_browser() {
            Ok(crate::shell::Opening::Done) => {}
            Ok(crate::shell::Opening::PlainPage(port)) => crate::window::open_page_in_browser(&app, port),
            Err((why, Some(port))) => {
                log::warn!("could not open the page signed in ({why}); opening the plain page instead");
                crate::window::open_page_in_browser(&app, port);
            }
            Err((why, None)) => log::warn!("could not open the page in the browser: {why}"),
        }
        OPENING.store(false, Ordering::SeqCst);
    });
}

/// Say where the start is, in the menu.
pub fn show_phase<R: Runtime>(app: &AppHandle<R>, phase: &Phase) {
    let Some(items) = app.try_state::<Items<R>>() else { return };
    let text = match phase {
        Phase::Starting => items.words.starting,
        Phase::Running => items.words.running,
        Phase::Restarting => items.words.restarting,
        Phase::Failed(_) => items.words.failed,
        Phase::Stopped => return,
    };
    let _ = items.status.set_text(text);
}

/// What *Quit Droppen* does: exit, which stops the server on the way out
/// (`RunEvent::Exit` in `lib.rs`).
pub fn quit<R: Runtime>(app: &AppHandle<R>) {
    log::info!("quit from the menu bar");
    app.exit(0);
}

/// What closing the window does on Linux with no tray host: the same quit,
/// stopping the server — nothing would bring the window back, so leaving the
/// app running invisibly would not be honest.
pub fn quit_without_tray<R: Runtime>(app: &AppHandle<R>) {
    log::info!("closing the window with no tray host: quitting");
    app.exit(0);
}

/// The mark with a dot when an update is ready to install, the mark alone
/// otherwise (`icons-src/tray-update.svg`, `tray.svg`). Main thread.
pub fn show_update_dot<R: Runtime>(app: &AppHandle<R>, dot: bool) {
    let Some(tray) = app.tray_by_id(TRAY) else { return };
    let bytes = if dot { TRAY_UPDATE_ICON } else { TRAY_ICON };
    if let Ok(icon) = Image::from_bytes(bytes) {
        let _ = tray.set_icon(Some(icon));
        let _ = tray.set_icon_as_template(cfg!(target_os = "macos"));
    }
}

static TITLE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The unread count beside the mark, or nothing.
pub fn show_unread<R: Runtime>(app: &AppHandle<R>, unread: Option<&str>) {
    {
        let mut title = TITLE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if title.as_deref() == unread {
            return;
        }
        *title = unread.map(str::to_string);
    }
    if let Some(tray) = app.tray_by_id(TRAY) {
        let _ = tray.set_title(unread);
    }
}

/// What the item says beside the mark now.
pub fn current_title() -> Option<String> {
    TITLE.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
}
