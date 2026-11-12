//! Droppen for macOS: runs Droppen on this device and is where you use it.
//!
//! Install it, open it, and it starts the Droppen server (bundled as a sidecar)
//! under an instance of its own, logs in for you and shows the server's web app
//! in its window. This device is then both a device that runs AI and the place
//! you operate it from. Closing the window leaves it running in the menu bar;
//! quitting stops the server.
//!
//! * [`shell`] — the server's start, supervision, login and stop;
//! * [`window`] — the main window and the rules for what it may load;
//! * [`tray`] — the menu-bar item and the app menu;
//! * [`login_item`] — *Start at Login*: `SMAppService` for a signed app, the
//!   launch agent for one that is not, an XDG autostart entry on Linux;
//! * [`sign_in`] — signing in to Droppen through the system browser;
//! * `linux_browser` — *Open in Browser*'s hand-off on Linux: the sign-in in
//!   a page only this user can read, for the default browser;
//! * [`i18n`] — their words;
//! * [`updater`] — the app's own updates, from the signed releases: checked
//!   and downloaded quietly, installed at the next quit;
//! * `testctl` — the end-to-end test hooks, only in a build with the `e2e`
//!   feature (`desktop/build.sh --e2e`), never in a release.
//!
//! What is deliberately not here yet, and where it goes (see `desktop/README.md`):
//!
//! * signing and notarisation — `desktop/SIGNING.md`;
//! * TODO(cloud sign-in, phone): the sealed rendezvous of
//!   `docs/local-aggregator.md` §4.9 through the hub, for a phone that scans
//!   the device's code. "登录 Droppen" in the window works through the system
//!   browser already ([`sign_in`]: the browser hand-off, answered through the
//!   app's own server);
//! * TODO(TauriShell): a real IPC bridge and the `KeyStore` of §7.3 in place
//!   of the URL-fragment and title contract of `droppen_desktop_core::web`.

pub mod i18n;
#[cfg(target_os = "linux")]
pub mod linux_browser;
pub mod login_item;
pub mod shell;
pub mod sign_in;
#[cfg(feature = "e2e")]
pub mod testctl;
pub mod tray;
pub mod updater;
pub mod window;

use std::path::PathBuf;
use std::sync::Arc;

use droppen_desktop_core::credentials::LayeredStore;
use droppen_desktop_core::dirs::ShellDirs;
use droppen_desktop_core::single::{self, Claim};
use droppen_desktop_core::{web, INSTANCE};
use serde::Serialize;
use tauri::{Manager, RunEvent, WindowEvent};

use shell::{Phase, Shell};

/// The argument the launch agent's launch passes (a build that is not signed,
/// or one made before `SMAppService`), and Linux's autostart entry's: start in
/// the menu bar, no window. A login item registered with `SMAppService` passes
/// none; its launch is told apart by its Apple event
/// ([`login_item::AppleEvent::is_login_launch`]).
pub const AUTOSTART_ARG: &str = "--autostart";

/// The sidecar beside the app's own executable (`Contents/MacOS/` in the
/// bundle, `target/<profile>/` in development — Tauri copies it to both).
fn sidecar_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(droppen_desktop_core::SIDECAR_NAME)))
        .unwrap_or_else(|| PathBuf::from(droppen_desktop_core::SIDECAR_NAME))
}

/// Whether the password may live in the keychain — on Linux, the Secret
/// Service. Always, in a release build; only an end-to-end build reads
/// `DROPPEN_DESKTOP_KEYCHAIN=0`, which its test runs set so they never touch a
/// real login keychain or keyring.
fn keychain_allowed() -> bool {
    #[cfg(feature = "e2e")]
    {
        droppen_desktop_core::credentials::keychain_enabled(std::env::var(droppen_desktop_core::credentials::KEYCHAIN_ENV).ok().as_deref())
    }
    #[cfg(not(feature = "e2e"))]
    {
        true
    }
}

#[derive(Serialize)]
struct Status {
    phase: &'static str,
    message: Option<String>,
}

#[tauri::command]
fn startup_status(shell: tauri::State<'_, Arc<Shell>>) -> Status {
    let phase = shell.phase();
    Status {
        phase: phase.name(),
        message: match phase {
            Phase::Failed(message) => Some(message),
            _ => None,
        },
    }
}

#[tauri::command]
fn retry_start(shell: tauri::State<'_, Arc<Shell>>) {
    shell.retry();
}

#[tauri::command]
fn open_logs(app: tauri::AppHandle, shell: tauri::State<'_, Arc<Shell>>) {
    use tauri_plugin_opener::OpenerExt;
    if let Err(error) = app.opener().open_path(shell.logs_dir().to_string_lossy(), None::<&str>) {
        log::warn!("cannot open the logs: {error}");
    }
}

/// The UI side of the shell: tray status and window navigation, always posted
/// to the main thread and never waited for — the shell calls these from its
/// own threads, including while the main thread is blocked stopping it.
struct AppUi {
    app: tauri::AppHandle,
}

impl shell::Ui for AppUi {
    fn phase(&self, phase: &Phase) {
        let app = self.app.clone();
        let phase = phase.clone();
        let _ = self.app.run_on_main_thread(move || tray::show_phase(&app, &phase));
    }

    fn hand_off(&self, login: web::Handoff) {
        let app = self.app.clone();
        let _ = self.app.run_on_main_thread(move || window::hand_off(&app, &login));
    }

    fn show_start_page(&self) {
        let app = self.app.clone();
        let _ = self.app.run_on_main_thread(move || window::show_start_page(&app));
    }
}

/// Run the app.
pub fn run() {
    let context = tauri::generate_context!();
    // The bundle identifier macOS keys the web view's data, the user defaults
    // and the running app by, and the one the app keys its own files, its
    // Keychain item and its launch agent by, must be the same: an end-to-end
    // build is `app.droppen.desktop.e2e` in both or does not start
    // (`desktop/build.sh --e2e` merges `tauri.e2e.conf.json`).
    if context.config().identifier != droppen_desktop_core::BUNDLE_ID {
        eprintln!(
            "droppen: this build is {} but its bundle identifier is {}; an end-to-end build takes src-tauri/tauri.e2e.conf.json (desktop/build.sh --e2e)",
            droppen_desktop_core::BUNDLE_ID,
            context.config().identifier
        );
        std::process::exit(2);
    }
    let dirs = match ShellDirs::from_env() {
        Some(dirs) => dirs,
        None => {
            eprintln!("droppen: HOME is not set");
            std::process::exit(1);
        }
    };
    // An end-to-end build never runs under the account's real home, where
    // the release keeps its launch agent and the server its shared files.
    #[cfg(feature = "e2e")]
    if let Err(why) = droppen_desktop_core::identity::check_scratch_home(&dirs, droppen_desktop_core::dirs::account_home().as_deref()) {
        eprintln!("droppen (end-to-end test build): {why}");
        std::process::exit(2);
    }
    let _ = dirs.create();
    droppen_desktop_core::logging::init(&dirs.app_log(), true);

    // One app per macOS user, decided before anything else starts. The app
    // Restart to Update starts waits for the old one to go instead.
    let claimed = if updater::relaunched() {
        single::claim_waiting(&dirs.support, updater::RELAUNCH_WAIT)
    } else {
        single::claim(&dirs.support)
    };
    let (_single_lock, single_listener) = match claimed {
        Ok(Claim::First { lock, listener }) => {
            #[cfg(target_os = "linux")]
            linux_browser::remove_earlier_pages();
            (Some(lock), listener)
        }
        // Opened at login while already running (both a login item and a
        // leftover launch agent, say): stay out of the way, no window.
        Ok(Claim::Running) if std::env::args().any(|arg| arg == AUTOSTART_ARG) => {
            log::info!("Droppen is already running (pid {}); a start at login leaves it as it is", std::process::id());
            std::process::exit(0);
        }
        Ok(Claim::Running) => {
            let heard = single::ask_to_show(&dirs.support);
            log::info!(
                "Droppen is already running (pid {}); asked it to show its window: {heard}",
                std::process::id()
            );
            std::process::exit(0);
        }
        Err(error) => {
            log::warn!("cannot claim the app lock, running without it: {error}");
            (None, None)
        }
    };

    let keychain = keychain_allowed();
    let translocated = std::env::current_exe().map(|exe| web::translocated(&exe)).unwrap_or(false);
    log::info!(
        "Droppen desktop starting (pid {}, {} {}{})",
        std::process::id(),
        droppen_desktop_core::credentials::KEYCHAIN,
        if keychain { "on" } else { "off" },
        if translocated { ", run from an App Translocation copy" } else { "" }
    );
    let autostart_arg = std::env::args().any(|arg| arg == AUTOSTART_ARG);
    let login = login_item::LoginItem::for_this_build(&dirs, keychain);

    let app = tauri::Builder::default()
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name(droppen_desktop_core::BUNDLE_ID)
                .args([AUTOSTART_ARG])
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(updater::plugin())
        .on_page_load(updater::page_loaded)
        .invoke_handler(tauri::generate_handler![startup_status, retry_start, open_logs])
        .setup(move |app| {
            log::info!(
                "Droppen {}{}",
                app.package_info().version,
                if updater::relaunched() { ", started by its update" } else { "" }
            );
            let handle = app.handle().clone();
            let store = LayeredStore::for_app(INSTANCE, keychain, dirs.credential_file());
            let shell = Shell::new(dirs.clone(), sidecar_path(), store, translocated, Box::new(AppUi { app: handle.clone() }));
            app.manage(Arc::clone(&shell));
            // While the app finishes launching, the Apple event is the launch's.
            let launch = login_item::AppleEvent::current();
            log::info!(
                "launch Apple event: {}",
                launch.map(|event| event.to_string()).unwrap_or_else(|| "none".into())
            );
            let opened_at_login = autostart_arg || launch.is_some_and(|event| event.is_login_launch());
            updater::install(&handle, &dirs.support)?;
            tray::install(&handle, &login, translocated)?;
            // With no tray host nothing would bring the window back: a
            // windowless launch at login would be invisible (a second launch
            // can focus it, but nothing says it is there). Show the window.
            // Asked once here and kept: the first window's init script reads
            // the same answer (`host_present_at_start`), so a slow bus delays
            // start-up once, not a window too.
            let tray_host = tray::host_present_at_start();
            let hidden = opened_at_login && tray_host;
            if opened_at_login {
                if hidden {
                    log::info!("opened at login: in the menu bar, no window");
                } else {
                    log::info!("opened at login with no tray host: showing the window");
                }
            }
            #[cfg(target_os = "linux")]
            log::info!("tray host: {}", if tray_host { "present" } else { "none" });
            app.manage(login);
            window::create_main(&handle, !hidden)?;
            if let Some(listener) = single_listener {
                let shower = handle.clone();
                single::listen(listener, move || {
                    log::info!("another launch: showing the window");
                    let app = shower.clone();
                    let _ = shower.run_on_main_thread(move || window::show(&app));
                });
            }
            watch_signals(handle.clone());
            #[cfg(feature = "e2e")]
            testctl::start(&handle);
            shell.boot();
            Ok(())
        })
        .build(context)
        .expect("build the app");

    app.run(|app, event| match event {
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::CloseRequested { api, .. },
            ..
        } if label == window::MAIN => {
            // Closing the window is not quitting while a tray host would bring
            // it back (macOS's menu bar, a Linux desktop's tray): Droppen keeps
            // running there. With no tray host on Linux nothing would, so it
            // quits as *Quit Droppen* does. Asked now, not at start, so a host
            // started in the meantime counts.
            api.prevent_close();
            if tray::host_present() {
                window::hide(app);
            } else {
                tray::quit_without_tray(app);
            }
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => match login_item::AppleEvent::current() {
            // macOS opening the login item while the app already runs (it was
            // opened at login some other way first): not a request for the window.
            Some(event) if event.is_login_reopen() => log::info!("reopen at login ({event}): no window"),
            _ => window::show(app),
        },
        RunEvent::Exit => {
            log::info!("quitting");
            if let Some(shell) = app.try_state::<Arc<Shell>>() {
                shell.shutdown();
            }
            // The server is stopped: install a downloaded update, and after
            // Restart to Update start the app again.
            updater::at_exit(app);
            log::info!("bye");
        }
        _ => {}
    });
}

/// Quit cleanly on `SIGTERM`, `SIGINT` and `SIGHUP` too (a `kill`, a `^C` in
/// the terminal that started the app, a logout), so the server is stopped
/// rather than orphaned.
fn watch_signals(app: tauri::AppHandle) {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    match signal_hook::iterator::Signals::new([SIGTERM, SIGINT, SIGHUP]) {
        Ok(mut signals) => {
            std::thread::spawn(move || {
                if let Some(signal) = signals.forever().next() {
                    log::info!("signal {signal}: quitting");
                    app.exit(0);
                }
            });
        }
        Err(error) => log::warn!("cannot watch for signals: {error}"),
    }
}
