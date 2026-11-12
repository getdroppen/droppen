//! Whether the network is one to spare: macOS's own view (Network.framework's
//! path monitor), so a 50 MB download waits while the Mac is on a personal
//! hotspot, a cellular link or Low Data Mode; on Linux NetworkManager's
//! (`Metered`, on the system bus), so a 100 MB AppImage waits on a phone's
//! hotspot or a mobile connection. Checks, a few hundred bytes, still run.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;

static METERED: AtomicBool = AtomicBool::new(false);
static WATCH: Once = Once::new();

/// Start watching, at launch: macOS answers the monitor a moment after it
/// starts, so a watch begun only when a download is due would let the first
/// one through on any network.
pub fn start() {
    WATCH.call_once(watch);
}

/// Whether the current network is expensive or constrained. False until
/// macOS or NetworkManager has said, and where neither says.
pub fn metered() -> bool {
    start();
    METERED.load(Ordering::Relaxed)
}

#[cfg(target_os = "macos")]
fn watch() {
    use std::ffi::c_void;

    #[link(name = "Network", kind = "framework")]
    extern "C" {
        fn nw_path_monitor_create() -> *mut c_void;
        fn nw_path_monitor_set_queue(monitor: *mut c_void, queue: *mut c_void);
        fn nw_path_monitor_set_update_handler(monitor: *mut c_void, handler: &block2::DynBlock<dyn Fn(*mut c_void)>);
        fn nw_path_monitor_start(monitor: *mut c_void);
        fn nw_path_is_expensive(path: *mut c_void) -> bool;
        fn nw_path_is_constrained(path: *mut c_void) -> bool;
    }
    extern "C" {
        fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut c_void;
    }

    let handler = block2::RcBlock::new(|path: *mut c_void| {
        if path.is_null() {
            return;
        }
        // SAFETY: Network.framework hands the handler a valid path object for
        // the duration of the call.
        let metered = unsafe { nw_path_is_expensive(path) || nw_path_is_constrained(path) };
        if METERED.swap(metered, Ordering::Relaxed) != metered {
            log::info!(
                "updates: the network is {}",
                if metered { "metered: downloads wait" } else { "not metered" }
            );
        }
    });
    // SAFETY: the monitor is created, configured and started once and lives for
    // the rest of the process (it is never released); the handler block is
    // copied by Network.framework; the global queue is never released either.
    unsafe {
        let monitor = nw_path_monitor_create();
        if monitor.is_null() {
            return;
        }
        nw_path_monitor_set_queue(monitor, dispatch_get_global_queue(0, 0));
        nw_path_monitor_set_update_handler(monitor, &handler);
        nw_path_monitor_start(monitor);
    }
}

/// How often NetworkManager is asked again, and how long it may take to answer.
#[cfg(not(target_os = "macos"))]
const ASK_EVERY: std::time::Duration = std::time::Duration::from_secs(60);
#[cfg(not(target_os = "macos"))]
const ANSWER_WITHIN: std::time::Duration = std::time::Duration::from_secs(2);

/// Ask NetworkManager now and then, on a thread of its own: a bus that does
/// not answer delays no download. Without NetworkManager (another network
/// manager, or none) the network is never metered, as on a Mac before it says.
#[cfg(not(target_os = "macos"))]
fn watch() {
    let spawned = std::thread::Builder::new().name("droppen-metered".into()).spawn(|| {
        let mut said_why = false;
        loop {
            match network_manager_metered() {
                Ok(metered) => {
                    if METERED.swap(metered, Ordering::Relaxed) != metered {
                        log::info!(
                            "updates: the network is {}",
                            if metered { "metered: downloads wait" } else { "not metered" }
                        );
                    }
                }
                Err(error) if !said_why => {
                    said_why = true;
                    log::info!("updates: NetworkManager cannot say whether the network is metered ({error}); downloads do not wait");
                }
                Err(_) => {}
            }
            std::thread::sleep(ASK_EVERY);
        }
    });
    if let Err(error) = spawned {
        log::warn!("updates: cannot watch for a metered network: {error}");
    }
}

/// NetworkManager's `Metered` for the connection the default route uses:
/// yes, or guessed yes (`NM_METERED_YES`, `NM_METERED_GUESS_YES`).
#[cfg(not(target_os = "macos"))]
fn network_manager_metered() -> zbus::Result<bool> {
    let bus = zbus::blocking::connection::Builder::system()?.method_timeout(ANSWER_WITHIN).build()?;
    let reply = bus.call_method(
        Some("org.freedesktop.NetworkManager"),
        "/org/freedesktop/NetworkManager",
        Some("org.freedesktop.DBus.Properties"),
        "Get",
        &("org.freedesktop.NetworkManager", "Metered"),
    )?;
    let value: zbus::zvariant::OwnedValue = reply.body().deserialize()?;
    Ok(matches!(u32::try_from(value)?, 1 | 3))
}
