//! *Start at Login* in the app: which way this build keeps it, the move off
//! the old launch agent at start, the default (on, once, at a new install's
//! first launch), the toggle, what the platform says about it, and whether
//! this launch is the one the platform made at login.
//!
//! The decisions are `droppen_desktop_core::login_item`'s; this is the
//! `SMAppService` and Apple event bridge around them, the launch agent
//! (tauri-plugin-autostart) every macOS build without a Developer ID keeps,
//! and Linux's XDG autostart entry (the file itself is core's).
//!
//! An end-to-end build (feature `e2e`) never calls `SMAppService` at all: its
//! [`MainApp`] is compiled without it, so a test run cannot touch the real
//! Login Items whatever it is signed with. Nor does it apply the default: its
//! runs check *Start at Login* from off. On Linux it has no backend at all,
//! so it writes no autostart entry either.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use droppen_desktop_core::credentials::LayeredStore;
use droppen_desktop_core::dirs::ShellDirs;
use droppen_desktop_core::login_item::{self as core, Backend, Before, FirstLaunch, Legacy, LegacyFile, Migration, Status};
use tauri::{AppHandle, Runtime};
use tauri_plugin_autostart::ManagerExt;

/// Whether this platform has *Start at Login*: macOS's login item, Linux's
/// XDG autostart entry.
pub const OFFERED: bool = true;

/// The app's login item: the backend chosen at start, the old launch agent,
/// `state.json`, where the default is recorded, where an earlier install
/// would have left its login, and Linux's autostart entry. Managed state.
pub struct LoginItem {
    backend: Backend,
    legacy: OldAgent,
    state_file: PathBuf,
    home: PathBuf,
    credential_file: PathBuf,
    /// Linux's entry; built on every platform (`exe` is only resolved there).
    autostart: core::AutostartFile,
    /// Whether the app's login may be in the keychain (`lib::keychain_allowed`).
    keychain: bool,
}

/// What a turn on or off came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turned {
    /// What the platform says now.
    pub status: Status,
    /// Whether it went as asked.
    pub ok: bool,
}

impl LoginItem {
    /// Choose the backend for this build ([`Backend::choose`]): `SMAppService`
    /// when the app is signed with a Developer ID, outside an end-to-end build;
    /// an XDG autostart entry on Linux; the launch agent otherwise.
    /// `keychain`: whether the app's login may be in the keychain.
    pub fn for_this_build(dirs: &ShellDirs, keychain: bool) -> Self {
        let end_to_end = cfg!(feature = "e2e");
        #[cfg(target_os = "macos")]
        let developer_id = !end_to_end
            && match core::signed_with_developer_id() {
                Ok(signed) => signed,
                Err(error) => {
                    log::warn!("cannot tell how the app is signed ({error}); start at login uses the launch agent");
                    false
                }
            };
        #[cfg(not(target_os = "macos"))]
        let developer_id = false;
        let backend = Backend::choose(core::Platform::this(), developer_id, end_to_end);
        // The AppImage is the stable thing to start, not the mount it runs
        // from; autostart_exe refuses a mount no image names.
        let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
        let appdir = std::env::var_os("APPDIR").map(PathBuf::from);
        let current_exe = std::env::current_exe().ok();
        let autostart = core::AutostartFile::new(
            dirs.autostart_file(),
            droppen_desktop_core::identity::THIS.name,
            vec![crate::AUTOSTART_ARG.to_string()],
            core::autostart_exe(appimage.as_deref(), appdir.as_deref(), current_exe.as_deref()),
        );
        log::info!(
            "start at login: {}",
            match backend {
                Backend::AppService => "SMAppService (the app is signed with a Developer ID)",
                Backend::LaunchAgent if end_to_end => "a launch agent (an end-to-end build)",
                Backend::LaunchAgent => "a launch agent (the app is not signed with a Developer ID)",
                Backend::XdgAutostart => "an XDG autostart entry",
                Backend::None => "none (an end-to-end build)",
            }
        );
        if backend == Backend::XdgAutostart {
            match autostart.exe() {
                Some(exe) => log::info!("start at login: {} starts {}", autostart.path().display(), exe.display()),
                None => log::warn!("start at login: no stable executable to name; the entry will not be written"),
            }
        }
        Self {
            backend,
            legacy: OldAgent {
                file: LegacyFile(droppen_desktop_core::dirs::login_item(&dirs.home)),
                said: Mutex::new(None),
            },
            state_file: dirs.state_file(),
            home: dirs.home.clone(),
            credential_file: dirs.credential_file(),
            autostart,
            keychain,
        }
    }

    /// At start: move an old launch agent over to `SMAppService` — or, in a
    /// build that keeps one, point it at where the app is now (Linux's entry
    /// too) — then apply the default ([`Self::apply_default`]) from the login
    /// item as it was before the move: the move removes an old launch agent
    /// the user switched off, which is still their choice. Nothing is moved
    /// while the app runs from a temporary App Translocation copy: the next
    /// launch from where it was moved does it.
    pub fn prepare<R: Runtime>(&self, app: &AppHandle<R>, translocated: bool) {
        let before = Before {
            // A login item of any kind means the user already chose: on macOS
            // the old launch agent, on Linux the entry — even one they
            // switched off in their desktop's startup settings.
            legacy_present: self.legacy.present() || (cfg!(target_os = "linux") && self.autostart.present()),
            status: self.status(app),
        };
        self.move_or_repoint(app, translocated);
        self.apply_default(app, before, translocated);
    }

    /// *Start at Login* is on by default: turned on here at a new install's
    /// first launch, once, and recorded in `state.json`; never again after
    /// that, whatever the user does with it, and never at an install that ran
    /// here before or one with a login item of any kind, as it was `before`
    /// this launch moved anything (`core::first_launch`). Called before the
    /// server starts, so nothing else has written `state.json`, the instance
    /// or the login at this launch yet. Not in an end-to-end build: its runs
    /// check *Start at Login* from off, and keep away from every real Login
    /// Item.
    fn apply_default<R: Runtime>(&self, app: &AppHandle<R>, before: Before, translocated: bool) {
        if cfg!(feature = "e2e") {
            return;
        }
        let ran_before = || self.ran_here_before();
        let decided = core::default_at_launch(&self.state_file, ran_before, before, translocated, || {
            // Never System Settings from here, even if the platform wants
            // approval: the menu says so, and the user goes there if they want.
            let turned = self.set(app, true);
            if turned.ok {
                log::info!("start at login: on by default, at this new install's first launch ({:?})", turned.status);
            } else {
                log::warn!(
                    "start at login: the platform did not take the default on ({:?}); it stays off until the user turns it on",
                    turned.status
                );
            }
        });
        match decided {
            FirstLaunch::Decided | FirstLaunch::TurnOn => {}
            FirstLaunch::LeaveAlone => log::info!(
                "start at login: left as it is ({:?}, a login item {}); its default is only for a new install",
                before.status,
                if before.legacy_present { "there" } else { "none" }
            ),
            FirstLaunch::Wait => log::info!("start at login: on by default once the app runs from where it is moved"),
        }
    }

    /// Whether an install ran here before, for the default. On Linux the
    /// app's own files answer: the Secret Service load can wait its 5 s
    /// deadline, and this runs on the main thread before the window exists.
    /// The instance directory is made by the same start that makes the
    /// keyring item, so a keyring item without it is an install that was
    /// already reset.
    fn ran_here_before(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            let _ = self.keychain;
            core::ran_here_before_files(&self.home, &self.credential_file)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let store = LayeredStore::for_app(droppen_desktop_core::INSTANCE, self.keychain, self.credential_file.clone());
            core::ran_here_before(&self.home, &self.credential_file, &store)
        }
    }

    fn move_or_repoint<R: Runtime>(&self, app: &AppHandle<R>, translocated: bool) {
        match self.backend {
            Backend::LaunchAgent => repoint_launch_agent(app, translocated),
            Backend::XdgAutostart => repoint_autostart(&self.autostart),
            Backend::None => {}
            Backend::AppService if translocated => {
                if self.legacy.present() {
                    log::info!("start at login: the old launch agent is moved once the app runs from where it was moved");
                }
            }
            Backend::AppService => match core::migrate(&MainApp, &self.legacy) {
                Migration::Nothing => {}
                Migration::Moved(status) => {
                    log::info!("start at login: the old launch agent was on; the app is now a login item ({status:?}) and the agent is removed")
                }
                Migration::Finished(status) => log::info!("start at login: removed the old launch agent left after the move ({status:?})"),
                Migration::SwitchedOff => {
                    log::info!("start at login: the old launch agent was switched off in System Settings; removed it, and start at login stays off")
                }
                Migration::Kept(error) => log::warn!("start at login: macOS would not take the login item ({error}); the old launch agent stays"),
                Migration::NotRemoved(status, error) => {
                    log::warn!("start at login: the old launch agent could not be removed ({error}); the login item is {status:?}")
                }
            },
        }
    }

    /// What the platform says now. A launch agent, and Linux's entry, are on
    /// while their file is there.
    pub fn status<R: Runtime>(&self, app: &AppHandle<R>) -> Status {
        match self.backend {
            Backend::AppService => core::effective(core::Service::status(&MainApp), &self.legacy),
            Backend::LaunchAgent if app.autolaunch().is_enabled().unwrap_or(false) => Status::Enabled,
            Backend::LaunchAgent => Status::NotRegistered,
            Backend::XdgAutostart => core::Service::status(&self.autostart),
            Backend::None => Status::NotRegistered,
        }
    }

    /// Turn it on or off.
    pub fn set<R: Runtime>(&self, app: &AppHandle<R>, on: bool) -> Turned {
        let word = if on { "on" } else { "off" };
        let ok = match self.backend {
            Backend::None => {
                log::info!("start at login: an end-to-end build keeps none");
                false
            }
            Backend::XdgAutostart => match core::set(&self.autostart, &self.legacy, on) {
                Ok(status) => {
                    log::info!("start at login: {word} ({status:?})");
                    true
                }
                Err(error) => {
                    log::warn!("cannot turn start at login {word}: {error}");
                    false
                }
            },
            Backend::AppService => match core::set(&MainApp, &self.legacy, on) {
                Ok(status) => {
                    log::info!("start at login: {word} ({status:?})");
                    true
                }
                Err(error) => {
                    log::warn!("cannot turn start at login {word}: {error}");
                    false
                }
            },
            Backend::LaunchAgent => {
                let manager = app.autolaunch();
                match if on { manager.enable() } else { manager.disable() } {
                    Ok(()) => {
                        log::info!("start at login: {word}");
                        true
                    }
                    Err(error) => {
                        log::warn!("cannot turn start at login {word}: {error}");
                        false
                    }
                }
            }
        };
        Turned { status: self.status(app), ok }
    }
}

/// A launch agent made before the app was moved still starts it from where it
/// was. Point it at where the app is now — or, if that is a temporary App
/// Translocation copy, turn it off (the app's notice says to move it).
fn repoint_launch_agent<R: Runtime>(app: &AppHandle<R>, translocated: bool) {
    let manager = app.autolaunch();
    if !manager.is_enabled().unwrap_or(false) {
        return;
    }
    let (Some(home), Ok(exe)) = (std::env::var_os("HOME"), std::env::current_exe().and_then(|exe| exe.canonicalize())) else {
        return;
    };
    let plist = std::fs::read_to_string(droppen_desktop_core::dirs::login_item(Path::new(&home))).unwrap_or_default();
    if droppen_desktop_core::dirs::login_item_starts(&plist, &exe) {
        return;
    }
    let result = if translocated { manager.disable() } else { manager.enable() };
    match (translocated, result) {
        (true, Ok(())) => log::warn!("start at login pointed elsewhere and the app runs from a temporary copy: turned it off"),
        (false, Ok(())) => log::info!("start at login pointed elsewhere: it now starts the app from {}", exe.display()),
        (_, Err(error)) => log::warn!("cannot fix start at login: {error}"),
    }
}

/// An autostart entry made before the app was moved still starts it from
/// where it was. Point it at where the app is now, as the launch agent is
/// pointed; one that cannot name a stable executable is left alone.
fn repoint_autostart(entry: &core::AutostartFile) {
    if !entry.enabled() || entry.starts() {
        return;
    }
    match entry.write() {
        Ok(()) => log::info!(
            "start at login pointed elsewhere: it now starts the app from {}",
            entry.exe().map(|exe| exe.display().to_string()).unwrap_or_default()
        ),
        Err(error) => log::warn!("cannot fix start at login: {error}"),
    }
}

/// The old launch agent: its file, and what macOS last said about it.
struct OldAgent {
    file: LegacyFile,
    /// Asked on every status read while the file is there and nothing is
    /// registered (a refused move): logged only when the answer changes.
    said: Mutex<Option<Status>>,
}

impl Legacy for OldAgent {
    fn present(&self) -> bool {
        self.file.present()
    }

    /// `SMAppService.statusForLegacyURL` of the file: `requiresApproval` is
    /// what macOS says of an item the user switched off in Login Items.
    fn switched_off(&self) -> bool {
        let status = legacy_status(&self.file.0);
        let mut said = self.said.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if *said != Some(status) {
            log::info!("start at login: macOS says the old launch agent is {status:?}");
            *said = Some(status);
        }
        status == Status::RequiresApproval
    }

    fn remove(&self) -> std::io::Result<()> {
        self.file.remove()
    }
}

/// `SMAppService.mainApp`. Not in an end-to-end build, nor off macOS (where the
/// backend is never [`Backend::AppService`]).
struct MainApp;

#[cfg(all(target_os = "macos", not(feature = "e2e")))]
impl core::Service for MainApp {
    fn status(&self) -> Status {
        Status::from_raw(unsafe { objc2_service_management::SMAppService::mainAppService().status() }.0)
    }

    fn register(&self) -> Result<(), String> {
        unsafe { objc2_service_management::SMAppService::mainAppService().registerAndReturnError() }.map_err(|error| describe(&error))
    }

    fn unregister(&self) -> Result<(), String> {
        unsafe { objc2_service_management::SMAppService::mainAppService().unregisterAndReturnError() }.map_err(|error| describe(&error))
    }
}

#[cfg(any(feature = "e2e", not(target_os = "macos")))]
impl core::Service for MainApp {
    fn status(&self) -> Status {
        Status::NotRegistered
    }

    fn register(&self) -> Result<(), String> {
        Err("not in an end-to-end build".into())
    }

    fn unregister(&self) -> Result<(), String> {
        Err("not in an end-to-end build".into())
    }
}

#[cfg(all(target_os = "macos", not(feature = "e2e")))]
fn legacy_status(plist: &Path) -> Status {
    use objc2_foundation::{NSString, NSURL};
    let url = NSURL::fileURLWithPath(&NSString::from_str(&plist.to_string_lossy()));
    Status::from_raw(unsafe { objc2_service_management::SMAppService::statusForLegacyURL(&url) }.0)
}

#[cfg(any(feature = "e2e", not(target_os = "macos")))]
fn legacy_status(_plist: &Path) -> Status {
    Status::NotRegistered
}

#[cfg(all(target_os = "macos", not(feature = "e2e")))]
fn describe(error: &objc2_foundation::NSError) -> String {
    format!("{} ({} {})", error.localizedDescription(), error.domain(), error.code())
}

/// Open System Settings → General → Login Items. Linux has no page for it:
/// there the entry is the user's own file in the autostart directory.
pub fn open_login_items() {
    #[cfg(target_os = "macos")]
    {
        log::info!("opening System Settings → Login Items");
        #[cfg(not(feature = "e2e"))]
        unsafe {
            objc2_service_management::SMAppService::openSystemSettingsLoginItems()
        };
    }
    #[cfg(not(target_os = "macos"))]
    log::info!("start at login is an autostart entry on this platform; there is no Login Items settings page");
}

const CORE_EVENTS: u32 = u32::from_be_bytes(*b"aevt");
const OPEN_APPLICATION: u32 = u32::from_be_bytes(*b"oapp");
const REOPEN_APPLICATION: u32 = u32::from_be_bytes(*b"rapp");
#[cfg(target_os = "macos")]
const PROPERTY_DATA: u32 = u32::from_be_bytes(*b"prdt");
const LAUNCHED_AS_LOGIN_ITEM: u32 = u32::from_be_bytes(*b"lgit");

/// The Apple event being handled now, as the codes it is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppleEvent {
    class: u32,
    id: u32,
    /// Its `keyAEPropData`, as an enum code, if it has one.
    property: Option<u32>,
}

impl AppleEvent {
    /// The event being handled now, if any: while the app finishes launching
    /// (Tauri's `setup` runs then), the launch's; in a reopen, the reopen's.
    /// Main thread only.
    #[cfg(target_os = "macos")]
    pub fn current() -> Option<Self> {
        objc2_foundation::NSAppleEventManager::sharedAppleEventManager()
            .currentAppleEvent()
            .map(|event| Self::of(&event))
    }

    /// None off macOS, which has no Apple events: there, only
    /// [`crate::AUTOSTART_ARG`] says a launch is at login.
    #[cfg(not(target_os = "macos"))]
    pub fn current() -> Option<Self> {
        None
    }

    #[cfg(target_os = "macos")]
    fn of(event: &objc2_foundation::NSAppleEventDescriptor) -> Self {
        use objc2::msg_send;
        use objc2::rc::Retained;
        use objc2_foundation::NSAppleEventDescriptor;

        let class: u32 = unsafe { msg_send![event, eventClass] };
        let id: u32 = unsafe { msg_send![event, eventID] };
        let property: Option<Retained<NSAppleEventDescriptor>> = unsafe { msg_send![event, paramDescriptorForKeyword: PROPERTY_DATA] };
        let property = property.map(|property| {
            let code: u32 = unsafe { msg_send![&*property, enumCodeValue] };
            code
        });
        Self { class, id, property }
    }

    /// Whether macOS opened the app as a login item: "open application" with
    /// "launched as login item" as its property. A login item registered with
    /// `SMAppService` is opened with no arguments, so this is how the app knows
    /// to start in the menu bar without a window.
    pub fn is_login_launch(&self) -> bool {
        self.class == CORE_EVENTS && self.id == OPEN_APPLICATION && self.property == Some(LAUNCHED_AS_LOGIN_ITEM)
    }

    /// Whether a reopen of the running app came from macOS opening a login
    /// item (the app was opened at login some other way first), not from the
    /// user asking for the window: a reopen or an open event that carries
    /// "launched as login item", which only macOS sets.
    pub fn is_login_reopen(&self) -> bool {
        self.class == CORE_EVENTS
            && matches!(self.id, REOPEN_APPLICATION | OPEN_APPLICATION)
            && self.property == Some(LAUNCHED_AS_LOGIN_ITEM)
    }
}

impl std::fmt::Display for AppleEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = |code: u32| String::from_utf8_lossy(&code.to_be_bytes()).into_owned();
        write!(f, "{}/{}", code(self.class), code(self.id))?;
        match self.property {
            Some(property) => write!(f, ", property {}", code(property)),
            None => write!(f, ", no property"),
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::AnyClass;
    use objc2_foundation::NSAppleEventDescriptor;

    /// An Apple event of the core suite with `id`, and `property` as its
    /// `keyAEPropData` enum when given — what macOS sends a launch.
    fn event(id: &[u8; 4], property: Option<&[u8; 4]>) -> Retained<NSAppleEventDescriptor> {
        let class = AnyClass::get(c"NSAppleEventDescriptor").expect("NSAppleEventDescriptor");
        let target: Option<&NSAppleEventDescriptor> = None;
        let event: Retained<NSAppleEventDescriptor> = unsafe {
            msg_send![
                class,
                appleEventWithEventClass: CORE_EVENTS,
                eventID: u32::from_be_bytes(*id),
                targetDescriptor: target,
                returnID: -1i16,
                transactionID: 0i32
            ]
        };
        if let Some(property) = property {
            let value: Retained<NSAppleEventDescriptor> = unsafe { msg_send![class, descriptorWithEnumCode: u32::from_be_bytes(*property)] };
            let _: () = unsafe { msg_send![&*event, setParamDescriptor: &*value, forKeyword: PROPERTY_DATA] };
        }
        event
    }

    #[test]
    fn a_login_items_launch_is_told_apart_by_its_apple_event() {
        let at_login = AppleEvent::of(&event(b"oapp", Some(b"lgit")));
        assert!(at_login.is_login_launch());
        assert!(at_login.is_login_reopen(), "while running, an open for the login item is no request for the window");
        assert_eq!(at_login.to_string(), "aevt/oapp, property lgit");

        let by_hand = AppleEvent::of(&event(b"oapp", None));
        assert!(!by_hand.is_login_launch(), "opened by the user");
        assert_eq!(by_hand.to_string(), "aevt/oapp, no property");
        assert!(!AppleEvent::of(&event(b"oapp", Some(b"xxxx"))).is_login_launch());

        let reopen = AppleEvent::of(&event(b"rapp", Some(b"lgit")));
        assert!(!reopen.is_login_launch(), "a reopen is not a launch");
        assert!(reopen.is_login_reopen());
        assert!(!AppleEvent::of(&event(b"rapp", None)).is_login_reopen(), "the user clicking the Dock icon");
        assert!(!by_hand.is_login_reopen(), "the user opening the app again");
        assert!(!AppleEvent::of(&event(b"quit", Some(b"lgit"))).is_login_reopen());

        // A test has no Apple event at all.
        assert_eq!(AppleEvent::current(), None);
    }
}
