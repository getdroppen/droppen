//! *Start at Login*: a login item macOS knows by the app's identity, not by
//! where it is.
//!
//! A release build is signed with a Developer ID and notarised, so it registers
//! itself with `SMAppService.mainApp` (macOS 13, the app's minimum): the item
//! follows the app wherever it is moved or renamed, and System Settings →
//! General → Login Items lists it as Droppen. macOS opens it at login without
//! arguments; the app tells that launch apart by its Apple event (the app crate).
//!
//! Earlier versions wrote a launch agent instead,
//! `~/Library/LaunchAgents/app.droppen.desktop.plist` ([`crate::dirs::login_item`]),
//! which names the app's path and so broke when the app moved. [`migrate`]
//! carries an install that has one over, once, keeping its choice: the file is
//! there only while *Start at Login* is on, unless the user switched it off in
//! System Settings → Login Items, which macOS tells ([`Legacy::switched_off`]).
//!
//! Only a build signed with a Developer ID Application certificate
//! ([`DEVELOPER_ID`]) uses `SMAppService` — the release. Every other build —
//! development, ad hoc (`desktop/build.sh` without an identity), Apple
//! Development–signed, end-to-end — keeps the launch agent, as before
//! ([`Backend`]): `SMAppService.mainApp` is keyed by the app's identity, so a
//! local build registering it would take over the release's Login Item, and an
//! end-to-end run must never register anything with the real Login Items.
//!
//! *Start at Login* is on by default: a new install's first launch turns it
//! on, once ([`default_at_launch`]), and `state.json` records that it did. From
//! then on it is the user's — the menu, System Settings — and the app never
//! turns it on again. An install that ran here before ([`first_launch`] lists
//! the signs), or one with a login item of any kind, is left as it is.
//!
//! The decisions are here, over [`Service`] and [`Legacy`], so they are tested
//! with fakes; the app crate supplies the real `SMAppService` and file.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::state::ShellState;

/// What macOS says about the app's login item (`SMAppServiceStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// Not registered: off.
    NotRegistered,
    /// Registered and allowed to run: on.
    Enabled,
    /// Registered, but not allowed to run until the user switches it on in
    /// System Settings → Login Items (or after they switched it off there).
    RequiresApproval,
    /// macOS cannot find the item: an error on its side.
    NotFound,
}

impl Status {
    /// From `SMAppServiceStatus`'s raw value; an unknown one reads as
    /// [`Status::NotFound`].
    pub fn from_raw(raw: isize) -> Self {
        match raw {
            0 => Self::NotRegistered,
            1 => Self::Enabled,
            2 => Self::RequiresApproval,
            _ => Self::NotFound,
        }
    }

    /// Whether the item is registered: on, approved or not yet.
    pub fn registered(self) -> bool {
        matches!(self, Self::Enabled | Self::RequiresApproval)
    }
}

/// `SMAppService.mainApp`, or a fake.
pub trait Service {
    /// The item's status now.
    fn status(&self) -> Status;
    /// Register the app as a login item.
    fn register(&self) -> Result<(), String>;
    /// Unregister it.
    fn unregister(&self) -> Result<(), String>;
}

/// The old launch agent, or a fake.
pub trait Legacy {
    /// Whether its file is there, which is what *on* meant in the app.
    fn present(&self) -> bool;
    /// Whether the user switched it off in System Settings → Login Items
    /// (`SMAppService.statusForLegacyURL` says `requiresApproval`): the file
    /// is there, but it does not run.
    fn switched_off(&self) -> bool;
    /// Remove its file; one already gone is no error.
    fn remove(&self) -> io::Result<()>;
}

/// The old launch agent's file. Whether it was switched off is macOS's to
/// say, so the app wraps this in a [`Legacy`] that asks.
#[derive(Debug, Clone)]
pub struct LegacyFile(pub PathBuf);

impl LegacyFile {
    /// Whether the file is there.
    pub fn present(&self) -> bool {
        self.0.exists()
    }

    /// Remove it; one already gone is no error.
    pub fn remove(&self) -> io::Result<()> {
        match std::fs::remove_file(&self.0) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}

/// How *Start at Login* is kept in this build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// `SMAppService.mainApp`.
    AppService,
    /// The launch agent naming the app's path (tauri-plugin-autostart).
    LaunchAgent,
}

impl Backend {
    /// `SMAppService` for an app signed with a Developer ID, outside an
    /// end-to-end build; the launch agent otherwise.
    pub fn choose(developer_id: bool, end_to_end: bool) -> Self {
        if developer_id && !end_to_end {
            Self::AppService
        } else {
            Self::LaunchAgent
        }
    }
}

/// What [`migrate`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Migration {
    /// There is no old launch agent: nothing to do.
    Nothing,
    /// The old one was on: the app is registered (with this status now) and
    /// the old file removed.
    Moved(Status),
    /// The app was already registered — an earlier move stopped before the
    /// file went: the file removed.
    Finished(Status),
    /// The user had switched the old one off in System Settings: it was off,
    /// so nothing is registered, and the file removed.
    SwitchedOff,
    /// macOS would not register the app: the old launch agent stays, so the
    /// app still opens at login, and the next launch tries again.
    Kept(String),
    /// Registered, but the old file could not be removed; the next launch
    /// tries again (the app opens once either way: it is single-instance).
    NotRemoved(Status, String),
}

/// Carry an install that has the old launch agent over to `service`, once.
///
/// The file is there only while *Start at Login* was on, so its presence is
/// the choice to keep — unless the user switched the old item off in System
/// Settings, which is off: remove the file and register nothing. Otherwise
/// register, then remove the file. Registered and still waiting for approval
/// counts as moved — the choice is recorded with macOS, and the menu says it
/// needs approval. If registering fails the file stays, so a failure never
/// turns *Start at Login* off.
pub fn migrate(service: &dyn Service, legacy: &dyn Legacy) -> Migration {
    if !legacy.present() {
        return Migration::Nothing;
    }
    let before = service.status();
    if !before.registered() && legacy.switched_off() {
        return match legacy.remove() {
            Ok(()) => Migration::SwitchedOff,
            Err(error) => Migration::NotRemoved(before, error.to_string()),
        };
    }
    let (status, moved_now) = if before.registered() {
        (before, false)
    } else {
        let result = service.register();
        let after = service.status();
        match result {
            // Asking for approval comes back as an error with the item
            // registered: that is a move all the same.
            _ if after.registered() => (after, true),
            Ok(()) => return Migration::Kept(format!("registered, but macOS says {after:?}")),
            Err(error) => return Migration::Kept(error),
        }
    };
    match legacy.remove() {
        Ok(()) if moved_now => Migration::Moved(status),
        Ok(()) => Migration::Finished(status),
        Err(error) => Migration::NotRemoved(status, error.to_string()),
    }
}

/// Turn *Start at Login* on or off; what macOS says afterwards.
///
/// Off also removes an old launch agent still there, so nothing opens the
/// app at login once the user said not to. An error is returned only if the
/// item did not end up as asked: registering into *needs approval* is on (the
/// menu says what is missing), and unregistering what was not registered is
/// off.
pub fn set(service: &dyn Service, legacy: &dyn Legacy, on: bool) -> Result<Status, String> {
    let result = if on { service.register() } else { service.unregister() };
    let removed = if on { Ok(()) } else { legacy.remove() };
    let status = service.status();
    match result {
        Ok(()) | Err(_) if status.registered() == on => {}
        Ok(()) => return Err(format!("macOS says {status:?}")),
        Err(error) => return Err(error),
    }
    removed.map_err(|error| format!("cannot remove the old login item: {error}"))?;
    Ok(status)
}

/// What became of *Start at Login*'s default, kept in `state.json`
/// ([`ShellState::login_default`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LoginDefault {
    /// A new install whose launches so far ran from a temporary App
    /// Translocation copy, where a login item would break: decided at the
    /// first launch from where the app was moved.
    Pending,
    /// Turned on at the first launch — asked of macOS, at least: if macOS
    /// would not take it, the menu shows it off and the user can turn it on.
    Applied,
    /// Left as it was: an install that ran here before, or one that already
    /// had a login item.
    LeftAlone,
}

/// What a launch does about the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstLaunch {
    /// Decided at an earlier launch: nothing, now or ever.
    Decided,
    /// Not a new install, or one that has a login item already: left alone.
    LeaveAlone,
    /// A new install running from a temporary copy: the launch from where it
    /// is moved decides.
    Wait,
    /// A new install: turn *Start at Login* on.
    TurnOn,
}

/// The login item as a launch found it, before the launch moved, repointed
/// or removed anything ([`migrate`] removes an old launch agent the user
/// switched off, which must still read as the user's own choice).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Before {
    /// The old launch agent's file was there.
    pub legacy_present: bool,
    /// What it counted as ([`effective`] for `SMAppService`).
    pub status: Status,
}

/// What this launch does about the default. `found` is `state.json` as the
/// launch found it — `None` when there was none — `ran_before` whether
/// anything else an earlier install leaves is here ([`ran_here_before`],
/// asked only when `state.json` cannot say), `before` the login item before
/// this launch changed anything, and `translocated` whether the app runs from
/// a temporary copy.
///
/// Not a new install, and so left alone:
///
/// * a `state.json` without the record — an install from before the default
///   (every launch that shows the window writes the file, `remember_port`);
/// * no `state.json`, but the app's stored login or its server's instance
///   directory is here — an earlier install whose Application Support folder
///   went (cleared, or a reinstall);
/// * a login item there already, on or off: an old launch agent (switched off
///   in System Settings, too), or one registered. Whatever it is, it is the
///   user's.
///
/// A `pending` record is a new install whose first launches ran from a
/// temporary copy: by now those launches made the instance and the login, so
/// only the login item is asked about.
pub fn first_launch(found: Option<&ShellState>, ran_before: impl FnOnce() -> bool, before: Before, translocated: bool) -> FirstLaunch {
    match found.map(|state| state.login_default) {
        Some(Some(LoginDefault::Applied | LoginDefault::LeftAlone)) => return FirstLaunch::Decided,
        Some(None) => return FirstLaunch::LeaveAlone,
        Some(Some(LoginDefault::Pending)) => {}
        None if ran_before() => return FirstLaunch::LeaveAlone,
        None => {}
    }
    if before.legacy_present || before.status.registered() {
        FirstLaunch::LeaveAlone
    } else if translocated {
        FirstLaunch::Wait
    } else {
        FirstLaunch::TurnOn
    }
}

/// Apply the default at this launch: decide ([`first_launch`]) from
/// `state_file`, `ran_before` and the login item as it was `before` this
/// launch moved anything, call `turn_on` if it is this launch's to turn on,
/// and keep what was decided in `state_file`, so no later launch decides
/// again. Called before anything else at the launch writes `state_file`.
/// What it did is returned; a record that cannot be saved is logged, and the
/// next launch decides again from what it finds (a login item it turned on is
/// then there, so it is left alone).
pub fn default_at_launch(
    state_file: &Path,
    ran_before: impl FnOnce() -> bool,
    before: Before,
    translocated: bool,
    turn_on: impl FnOnce(),
) -> FirstLaunch {
    let found = state_file.exists().then(|| ShellState::load(state_file));
    let decision = first_launch(found.as_ref(), ran_before, before, translocated);
    let record = match decision {
        FirstLaunch::Decided => return decision,
        FirstLaunch::LeaveAlone => LoginDefault::LeftAlone,
        FirstLaunch::Wait => LoginDefault::Pending,
        FirstLaunch::TurnOn => {
            turn_on();
            LoginDefault::Applied
        }
    };
    let mut state = found.unwrap_or_default();
    if state.login_default != Some(record) {
        state.login_default = Some(record);
        if let Err(error) = state.save(state_file) {
            log::warn!("cannot record start at login's default ({record:?}): {error}");
        }
    }
    decision
}

/// Where an earlier install's server kept the app's instance: under today's
/// data directory, and under the one it takes once that is named after
/// Droppen (`~/.local/share/droppen`).
pub fn instance_dirs(home: &Path) -> [PathBuf; 2] {
    [
        crate::instance::dir_under(home, crate::INSTANCE),
        home.join(".local").join("share").join("droppen").join("instances").join(crate::INSTANCE),
    ]
}

/// Whether the app ran here before, whatever `state.json` says: its server's
/// instance directory ([`instance_dirs`]) or its credential file is there, or
/// `store` (the keychain item, then the file) has its login. The keychain is
/// asked last, only when nothing on disk answers; one that cannot be read is
/// logged and reads as nothing there.
pub fn ran_here_before(home: &Path, credential_file: &Path, store: &crate::credentials::LayeredStore) -> bool {
    instance_dirs(home).iter().any(|dir| dir.exists()) || credential_file.exists() || store.load().0.is_some()
}

/// The status to show: `service`'s, except that an old launch agent still
/// there and not switched off (a move macOS refused, to be tried again at the
/// next launch) opens the app at login all the same, so it is on.
pub fn effective(service: Status, legacy: &dyn Legacy) -> Status {
    if !service.registered() && legacy.present() && !legacy.switched_off() {
        Status::Enabled
    } else {
        service
    }
}

/// What the menu shows for *Start at Login*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    /// The check mark: the item is registered.
    pub checked: bool,
    /// Whether *Start at Login* can be changed: not from a temporary App
    /// Translocation copy.
    pub enabled: bool,
    /// A line under it that opens System Settings → Login Items, when macOS
    /// needs the user there.
    pub note: Option<Note>,
}

/// Why the menu points at System Settings → Login Items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    /// The item is registered but not allowed to run.
    NeedsApproval,
    /// The user just turned it on and macOS did not take it.
    TurnOnFailed,
}

/// The menu for `status`; `turn_on_failed`: the user's last *Start at Login*
/// was a turn-on macOS did not take.
///
/// `notFound` is plain off: it is also what macOS says after the user removed
/// the item from Login Items with −, and a lasting "not found" line would be
/// wrong then. A line is shown only for an item waiting for approval, or right
/// after a turn-on that failed.
pub fn shown(status: Status, translocated: bool, turn_on_failed: bool) -> Shown {
    Shown {
        checked: status.registered(),
        enabled: !translocated,
        note: match status {
            Status::RequiresApproval => Some(Note::NeedsApproval),
            Status::Enabled => None,
            Status::NotRegistered | Status::NotFound if turn_on_failed => Some(Note::TurnOnFailed),
            Status::NotRegistered | Status::NotFound => None,
        },
    }
}

/// The code requirement for [`Backend::AppService`]: a certificate chain to
/// Apple's root whose leaf is a Developer ID Application certificate (the
/// extension `1.2.840.113635.100.6.1.13`) — any team's, so a fork's release
/// qualifies too, but not an Apple Development or App Store signature.
pub const DEVELOPER_ID: &str = "anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13]";

/// Whether the running app is signed with a Developer ID ([`DEVELOPER_ID`]).
/// Ad hoc, unsigned and otherwise signed code is `Ok(false)`; anything else
/// that goes wrong asking is an error, and the caller treats it as `false`.
#[cfg(target_os = "macos")]
pub fn signed_with_developer_id() -> Result<bool, String> {
    use security_framework::os::macos::code_signing::{Flags, SecCode};

    let code = SecCode::for_self(Flags::NONE).map_err(|error| format!("{error}"))?;
    answer(code.check_validity(Flags::NONE, &developer_id()?))
}

#[cfg(target_os = "macos")]
fn developer_id() -> Result<security_framework::os::macos::code_signing::SecRequirement, String> {
    DEVELOPER_ID.parse().map_err(|error| format!("{error}"))
}

/// A validity check's result as an answer: met, not met (unsigned, or the
/// requirement failed), or an error.
#[cfg(target_os = "macos")]
fn answer(result: Result<(), security_framework::base::Error>) -> Result<bool, String> {
    /// `errSecCSUnsigned` and `errSecCSReqFailed`.
    const UNSIGNED: i32 = -67062;
    const REQUIREMENT_FAILED: i32 = -67050;
    match result {
        Ok(()) => Ok(true),
        Err(error) if matches!(error.code(), UNSIGNED | REQUIREMENT_FAILED) => Ok(false),
        Err(error) => Err(format!("{error} ({})", error.code())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    /// `SMAppService.mainApp` as a fake: what each call answers, and a record
    /// of the calls.
    struct FakeService {
        status: Cell<Status>,
        /// The status a successful register leaves.
        after_register: Status,
        register_error: Option<String>,
        unregister_error: Option<String>,
        calls: RefCell<Vec<&'static str>>,
    }

    impl FakeService {
        fn new(status: Status) -> Self {
            Self {
                status: Cell::new(status),
                after_register: Status::Enabled,
                register_error: None,
                unregister_error: None,
                calls: RefCell::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls.borrow().clone()
        }
    }

    impl Service for FakeService {
        fn status(&self) -> Status {
            self.status.get()
        }

        fn register(&self) -> Result<(), String> {
            self.calls.borrow_mut().push("register");
            match &self.register_error {
                // macOS registers the item and asks for approval, or refuses.
                Some(error) => {
                    if self.after_register == Status::RequiresApproval {
                        self.status.set(Status::RequiresApproval);
                    }
                    Err(error.clone())
                }
                None => {
                    self.status.set(self.after_register);
                    Ok(())
                }
            }
        }

        fn unregister(&self) -> Result<(), String> {
            self.calls.borrow_mut().push("unregister");
            if !self.status.get().registered() {
                return Err("Operation not permitted (job not found)".into());
            }
            match &self.unregister_error {
                Some(error) => Err(error.clone()),
                None => {
                    self.status.set(Status::NotRegistered);
                    Ok(())
                }
            }
        }
    }

    /// The old launch agent: a real file in a scratch home, and what macOS
    /// would say about it.
    struct OldAgent {
        _root: tempfile::TempDir,
        file: LegacyFile,
        switched_off: bool,
    }

    impl Legacy for OldAgent {
        fn present(&self) -> bool {
            self.file.present()
        }
        fn switched_off(&self) -> bool {
            self.switched_off
        }
        fn remove(&self) -> io::Result<()> {
            self.file.remove()
        }
    }

    fn old_agent(present: bool, switched_off: bool) -> OldAgent {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("Library/LaunchAgents/app.droppen.desktop.plist");
        if present {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(&path, "<plist/>").expect("write");
        }
        OldAgent {
            _root: root,
            file: LegacyFile(path),
            switched_off,
        }
    }

    #[test]
    fn no_old_launch_agent_means_nothing_to_move() {
        let service = FakeService::new(Status::NotRegistered);
        let legacy = old_agent(false, false);
        assert_eq!(migrate(&service, &legacy), Migration::Nothing);
        assert!(service.calls().is_empty(), "off stays off: nothing registered");
        assert_eq!(service.status(), Status::NotRegistered);
    }

    #[test]
    fn an_old_launch_agent_that_was_on_is_registered_then_removed() {
        let service = FakeService::new(Status::NotRegistered);
        let legacy = old_agent(true, false);
        assert_eq!(migrate(&service, &legacy), Migration::Moved(Status::Enabled));
        assert_eq!(service.calls(), ["register"]);
        assert!(!legacy.present(), "the old file is gone");
        // The next launch has nothing left to do.
        assert_eq!(migrate(&service, &legacy), Migration::Nothing);
        assert_eq!(service.calls(), ["register"]);
    }

    #[test]
    fn an_old_launch_agent_switched_off_in_system_settings_stays_off() {
        let service = FakeService::new(Status::NotRegistered);
        let legacy = old_agent(true, true);
        assert_eq!(migrate(&service, &legacy), Migration::SwitchedOff);
        assert!(service.calls().is_empty(), "nothing registered");
        assert!(!legacy.present(), "the old file is gone");
        assert_eq!(service.status(), Status::NotRegistered);
        // Before the move it did not count as on either.
        let legacy = old_agent(true, true);
        assert_eq!(effective(Status::NotRegistered, &legacy), Status::NotRegistered);
    }

    #[test]
    fn a_move_that_needs_approval_is_a_move() {
        let mut service = FakeService::new(Status::NotRegistered);
        service.after_register = Status::RequiresApproval;
        service.register_error = Some("Operation not permitted".into());
        let legacy = old_agent(true, false);
        assert_eq!(migrate(&service, &legacy), Migration::Moved(Status::RequiresApproval));
        assert!(!legacy.present());
        assert_eq!(shown(service.status(), false, false).note, Some(Note::NeedsApproval));
    }

    #[test]
    fn a_refused_registration_keeps_the_old_launch_agent() {
        let mut service = FakeService::new(Status::NotRegistered);
        service.register_error = Some("Invalid code signature".into());
        let legacy = old_agent(true, false);
        assert_eq!(migrate(&service, &legacy), Migration::Kept("Invalid code signature".into()));
        assert!(legacy.present(), "the app still opens at login the old way");

        // Registered "successfully" into a status that is not registered: kept too.
        let mut service = FakeService::new(Status::NotRegistered);
        service.after_register = Status::NotFound;
        assert!(matches!(migrate(&service, &legacy), Migration::Kept(_)));
        assert!(legacy.present());
    }

    #[test]
    fn a_move_cut_short_before_the_file_went_is_finished() {
        let service = FakeService::new(Status::Enabled);
        let legacy = old_agent(true, false);
        assert_eq!(migrate(&service, &legacy), Migration::Finished(Status::Enabled));
        assert!(service.calls().is_empty(), "not registered twice");
        assert!(!legacy.present());

        // Even if the old one was switched off since: the new one is the
        // user's choice now.
        let legacy = old_agent(true, true);
        assert_eq!(migrate(&service, &legacy), Migration::Finished(Status::Enabled));
        assert!(service.calls().is_empty());
    }

    #[test]
    fn a_file_that_cannot_be_removed_is_reported_and_left_for_next_time() {
        struct Stuck(bool);
        impl Legacy for Stuck {
            fn present(&self) -> bool {
                true
            }
            fn switched_off(&self) -> bool {
                self.0
            }
            fn remove(&self) -> io::Result<()> {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "read-only"))
            }
        }
        let service = FakeService::new(Status::NotRegistered);
        assert!(matches!(migrate(&service, &Stuck(false)), Migration::NotRemoved(Status::Enabled, _)));
        // The next launch only removes it.
        assert!(matches!(migrate(&service, &Stuck(false)), Migration::NotRemoved(Status::Enabled, _)));
        assert_eq!(service.calls(), ["register"]);

        let service = FakeService::new(Status::NotRegistered);
        assert!(matches!(migrate(&service, &Stuck(true)), Migration::NotRemoved(Status::NotRegistered, _)));
        assert!(service.calls().is_empty());
    }

    #[test]
    fn turning_it_on_and_off_says_what_macos_says() {
        let service = FakeService::new(Status::NotRegistered);
        let legacy = old_agent(false, false);
        assert_eq!(set(&service, &legacy, true), Ok(Status::Enabled));
        assert_eq!(set(&service, &legacy, false), Ok(Status::NotRegistered));
        // Off when it was not registered is off, not an error.
        assert_eq!(set(&service, &legacy, false), Ok(Status::NotRegistered));
        assert_eq!(service.calls(), ["register", "unregister", "unregister"]);
    }

    #[test]
    fn on_into_needs_approval_is_on_and_a_refusal_is_an_error() {
        let mut service = FakeService::new(Status::NotRegistered);
        service.after_register = Status::RequiresApproval;
        service.register_error = Some("Operation not permitted".into());
        let legacy = old_agent(false, false);
        assert_eq!(set(&service, &legacy, true), Ok(Status::RequiresApproval));

        let mut service = FakeService::new(Status::NotRegistered);
        service.register_error = Some("Invalid code signature".into());
        assert_eq!(set(&service, &legacy, true), Err("Invalid code signature".into()));

        let mut service = FakeService::new(Status::Enabled);
        service.unregister_error = Some("Internal failure".into());
        assert_eq!(set(&service, &legacy, false), Err("Internal failure".into()));
    }

    #[test]
    fn off_also_removes_an_old_launch_agent_still_there() {
        let service = FakeService::new(Status::Enabled);
        let legacy = old_agent(true, false);
        assert_eq!(set(&service, &legacy, false), Ok(Status::NotRegistered));
        assert!(!legacy.present());
    }

    #[test]
    fn an_old_launch_agent_left_by_a_refused_move_still_counts_as_on() {
        let on = old_agent(true, false);
        let none = old_agent(false, false);
        assert_eq!(effective(Status::NotRegistered, &on), Status::Enabled);
        assert_eq!(effective(Status::NotFound, &on), Status::Enabled);
        assert_eq!(effective(Status::NotRegistered, &none), Status::NotRegistered);
        assert_eq!(effective(Status::RequiresApproval, &on), Status::RequiresApproval);
        assert_eq!(effective(Status::NotFound, &none), Status::NotFound);

        // And turning it off removes it, so it reads as off.
        let service = FakeService::new(Status::NotRegistered);
        assert_eq!(set(&service, &on, false), Ok(Status::NotRegistered));
        assert_eq!(effective(service.status(), &on), Status::NotRegistered);
    }

    /// One launch of a Developer ID build, in the app's own order
    /// (`LoginItem::prepare`): note the login item as found, move an old
    /// launch agent over (not from a temporary copy), then decide the default
    /// from what was found, turning on through `set` as the app does.
    /// `ran_before`: whether anything else of an earlier install is here.
    fn launch(state_file: &Path, service: &FakeService, legacy: &OldAgent, translocated: bool, ran_before: bool) -> FirstLaunch {
        let before = Before {
            legacy_present: legacy.present(),
            status: effective(service.status(), legacy),
        };
        if !translocated {
            migrate(service, legacy);
        }
        default_at_launch(state_file, || ran_before, before, translocated, || {
            set(service, legacy, true).expect("on");
        })
    }

    #[test]
    fn a_new_install_turns_start_at_login_on_once_and_never_again() {
        let root = tempfile::tempdir().expect("tempdir");
        let state_file = root.path().join("state.json");
        let service = FakeService::new(Status::NotRegistered);
        let legacy = old_agent(false, false);

        assert_eq!(launch(&state_file, &service, &legacy, false, false), FirstLaunch::TurnOn);
        assert_eq!(service.status(), Status::Enabled);
        assert_eq!(service.calls(), ["register"]);
        assert_eq!(ShellState::load(&state_file).login_default, Some(LoginDefault::Applied));

        // The user turns it off: no later launch turns it on again, though by
        // now the instance and the login are there too.
        assert_eq!(set(&service, &legacy, false), Ok(Status::NotRegistered));
        for _ in 0..2 {
            assert_eq!(launch(&state_file, &service, &legacy, false, true), FirstLaunch::Decided);
        }
        assert_eq!(service.status(), Status::NotRegistered);
        assert_eq!(service.calls(), ["register", "unregister"]);

        // Nor after the rest of the state is written, as later launches do.
        ShellState::remember_port(&state_file, 18_700).expect("port");
        assert_eq!(launch(&state_file, &service, &legacy, false, true), FirstLaunch::Decided);
        assert_eq!(ShellState::load(&state_file).login_default, Some(LoginDefault::Applied));
    }

    #[test]
    fn an_install_from_before_the_default_is_left_as_it_is() {
        let root = tempfile::tempdir().expect("tempdir");
        let state_file = root.path().join("state.json");
        // Earlier launches wrote state.json (the port), with no record.
        ShellState::remember_port(&state_file, 51_234).expect("port");
        let service = FakeService::new(Status::NotRegistered);
        let legacy = old_agent(false, false);
        assert_eq!(launch(&state_file, &service, &legacy, false, false), FirstLaunch::LeaveAlone);
        assert!(service.calls().is_empty(), "off stays off");
        let state = ShellState::load(&state_file);
        assert_eq!(state.login_default, Some(LoginDefault::LeftAlone));
        assert_eq!(state.port, Some(51_234), "the rest is kept");
        assert_eq!(launch(&state_file, &service, &legacy, false, false), FirstLaunch::Decided);
        assert!(service.calls().is_empty());
    }

    #[test]
    fn an_old_launch_agent_switched_off_stays_off_even_with_no_state_json() {
        // The Application Support folder went (cleared, a reinstall), but the
        // old launch agent the user switched off in System Settings is still
        // there. The move removes its file first; the default is decided
        // from what was there before it, so off stays off.
        let root = tempfile::tempdir().expect("tempdir");
        let state_file = root.path().join("state.json");
        let service = FakeService::new(Status::NotRegistered);
        let legacy = old_agent(true, true);
        assert_eq!(launch(&state_file, &service, &legacy, false, false), FirstLaunch::LeaveAlone);
        assert!(!legacy.present(), "the move removed the old file");
        assert!(service.calls().is_empty(), "and nothing is registered");
        assert_eq!(service.status(), Status::NotRegistered);
        assert_eq!(ShellState::load(&state_file).login_default, Some(LoginDefault::LeftAlone));
        // The next launch finds no file, nothing registered: still decided.
        assert_eq!(launch(&state_file, &service, &legacy, false, false), FirstLaunch::Decided);
        assert!(service.calls().is_empty());
    }

    #[test]
    fn a_new_install_that_has_a_login_item_already_keeps_it_as_it_is() {
        // An old launch agent that was on: the move registers the app, and the
        // default has nothing to add.
        let root = tempfile::tempdir().expect("tempdir");
        let state_file = root.path().join("state.json");
        let service = FakeService::new(Status::NotRegistered);
        let legacy = old_agent(true, false);
        assert_eq!(launch(&state_file, &service, &legacy, false, false), FirstLaunch::LeaveAlone);
        assert_eq!(service.calls(), ["register"], "registered once, by the move");
        assert_eq!(ShellState::load(&state_file).login_default, Some(LoginDefault::LeftAlone));

        // Registered already, waiting for approval: not registered again.
        let root = tempfile::tempdir().expect("tempdir");
        let state_file = root.path().join("state.json");
        let service = FakeService::new(Status::RequiresApproval);
        let legacy = old_agent(false, false);
        assert_eq!(launch(&state_file, &service, &legacy, false, false), FirstLaunch::LeaveAlone);
        assert!(service.calls().is_empty());
    }

    #[test]
    fn an_earlier_install_without_its_state_json_is_left_as_it_is() {
        // No state.json and no login item, but the app's login or its
        // server's instance is here: it ran before, and the user may have
        // turned start at login off since.
        let root = tempfile::tempdir().expect("tempdir");
        let state_file = root.path().join("state.json");
        let service = FakeService::new(Status::NotRegistered);
        let legacy = old_agent(false, false);
        assert_eq!(launch(&state_file, &service, &legacy, false, true), FirstLaunch::LeaveAlone);
        assert!(service.calls().is_empty());
        assert_eq!(ShellState::load(&state_file).login_default, Some(LoginDefault::LeftAlone));
    }

    #[test]
    fn a_new_install_run_from_a_temporary_copy_waits_for_the_launch_from_where_it_is_moved() {
        let root = tempfile::tempdir().expect("tempdir");
        let state_file = root.path().join("state.json");
        let service = FakeService::new(Status::NotRegistered);
        let legacy = old_agent(false, false);
        assert_eq!(launch(&state_file, &service, &legacy, true, false), FirstLaunch::Wait);
        assert!(service.calls().is_empty(), "a login item would point at the copy");
        assert_eq!(ShellState::load(&state_file).login_default, Some(LoginDefault::Pending));
        // That launch showed its window and started the server, so state.json
        // has a port by now, and the instance and the login are there.
        ShellState::remember_port(&state_file, 18_700).expect("port");
        assert_eq!(launch(&state_file, &service, &legacy, true, true), FirstLaunch::Wait);

        // Moved to Applications: still the new install it was, turned on now.
        assert_eq!(launch(&state_file, &service, &legacy, false, true), FirstLaunch::TurnOn);
        assert_eq!(service.status(), Status::Enabled);
        assert_eq!(ShellState::load(&state_file).login_default, Some(LoginDefault::Applied));
        assert_eq!(ShellState::load(&state_file).port, Some(18_700));
    }

    #[test]
    fn a_turn_on_macos_will_not_take_is_still_the_one_turn_on() {
        let root = tempfile::tempdir().expect("tempdir");
        let state_file = root.path().join("state.json");
        let mut service = FakeService::new(Status::NotRegistered);
        service.register_error = Some("Invalid code signature".into());
        let legacy = old_agent(false, false);
        let before = Before {
            legacy_present: false,
            status: service.status(),
        };
        let decided = default_at_launch(&state_file, || false, before, false, || {
            assert!(set(&service, &legacy, true).is_err());
        });
        assert_eq!(decided, FirstLaunch::TurnOn);
        assert_eq!(ShellState::load(&state_file).login_default, Some(LoginDefault::Applied));
        // The menu shows it off; the next launch does not try again.
        assert_eq!(launch(&state_file, &service, &legacy, false, true), FirstLaunch::Decided);
        assert_eq!(service.calls(), ["register"]);
    }

    #[test]
    fn the_decision_by_itself() {
        let found = |legacy_present, status| Before { legacy_present, status };
        let off = found(false, Status::NotRegistered);
        let none = ShellState::default();
        let with = |record| ShellState {
            login_default: Some(record),
            ..ShellState::default()
        };
        let not_asked = || -> bool { panic!("state.json answers: nothing else is asked") };
        assert_eq!(first_launch(None, || false, off, false), FirstLaunch::TurnOn);
        assert_eq!(
            first_launch(None, || false, found(false, Status::NotFound), false),
            FirstLaunch::TurnOn,
            "removed with −: not registered"
        );
        assert_eq!(first_launch(None, || false, found(false, Status::Enabled), false), FirstLaunch::LeaveAlone);
        assert_eq!(first_launch(None, || false, found(true, Status::NotRegistered), false), FirstLaunch::LeaveAlone);
        assert_eq!(first_launch(None, || true, off, false), FirstLaunch::LeaveAlone, "it ran here before");
        assert_eq!(first_launch(None, || false, off, true), FirstLaunch::Wait);
        assert_eq!(first_launch(Some(&none), not_asked, off, false), FirstLaunch::LeaveAlone);
        assert_eq!(first_launch(Some(&with(LoginDefault::Pending)), not_asked, off, false), FirstLaunch::TurnOn);
        assert_eq!(
            first_launch(Some(&with(LoginDefault::Pending)), not_asked, found(false, Status::Enabled), false),
            FirstLaunch::LeaveAlone
        );
        assert_eq!(first_launch(Some(&with(LoginDefault::Applied)), not_asked, off, false), FirstLaunch::Decided);
        assert_eq!(first_launch(Some(&with(LoginDefault::LeftAlone)), not_asked, off, false), FirstLaunch::Decided);
        assert_eq!(serde_json::to_string(&LoginDefault::LeftAlone).expect("json"), r#""leftAlone""#);
    }

    #[test]
    fn an_earlier_install_is_told_by_its_instance_or_its_login() {
        use crate::credentials::{Credential, FileStore, KeyStore, LayeredStore, StoreError};

        /// The keychain item, or none.
        struct Keychain(Option<Credential>);
        impl KeyStore for Keychain {
            fn kind(&self) -> &'static str {
                "keychain"
            }
            fn load(&self) -> Result<Option<Credential>, StoreError> {
                Ok(self.0.clone())
            }
            fn save(&self, _credential: &Credential) -> Result<(), StoreError> {
                Ok(())
            }
            fn delete(&self) -> Result<(), StoreError> {
                Ok(())
            }
        }
        let root = tempfile::tempdir().expect("tempdir");
        let home = root.path();
        let file = home.join("Library/Application Support/app.droppen.desktop/credentials.json");
        let store = |item: Option<Credential>| LayeredStore::new(Some(Box::new(Keychain(item))), Box::new(FileStore::new(file.clone())));
        assert!(!ran_here_before(home, &file, &store(None)), "nothing here: new");

        let login = Credential {
            username: "droppen-app".into(),
            password: "p".into(),
        };
        assert!(ran_here_before(home, &file, &store(Some(login))), "the login in the keychain");

        std::fs::create_dir_all(file.parent().expect("parent")).expect("dir");
        std::fs::write(&file, "{}").expect("write");
        assert!(ran_here_before(home, &file, &store(None)), "a credential file, readable or not");
        std::fs::remove_file(&file).expect("remove");

        for dir in instance_dirs(home) {
            std::fs::create_dir_all(&dir).expect("instance");
            assert!(ran_here_before(home, &file, &store(None)), "{}", dir.display());
            std::fs::remove_dir(&dir).expect("remove");
        }
        assert_eq!(instance_dirs(home)[0], home.join(".local/share/remote-hand/instances").join(crate::INSTANCE));
        assert_eq!(instance_dirs(home)[1], home.join(".local/share/droppen/instances").join(crate::INSTANCE));
        assert!(!ran_here_before(home, &file, &store(None)));
    }

    #[test]
    fn the_menu_follows_the_status() {
        assert_eq!(
            shown(Status::Enabled, false, false),
            Shown { checked: true, enabled: true, note: None }
        );
        assert_eq!(
            shown(Status::NotRegistered, false, false),
            Shown { checked: false, enabled: true, note: None }
        );
        assert_eq!(
            shown(Status::RequiresApproval, false, false),
            Shown { checked: true, enabled: true, note: Some(Note::NeedsApproval) }
        );
        assert!(!shown(Status::Enabled, true, false).enabled, "not from a translocated copy");
    }

    #[test]
    fn not_found_is_plain_off_unless_a_turn_on_just_failed() {
        // What macOS says after the user removed the item with −.
        assert_eq!(
            shown(Status::NotFound, false, false),
            Shown { checked: false, enabled: true, note: None }
        );
        // Right after a turn-on macOS did not take.
        assert_eq!(shown(Status::NotFound, false, true).note, Some(Note::TurnOnFailed));
        assert_eq!(shown(Status::NotRegistered, false, true).note, Some(Note::TurnOnFailed));
        // A turn-on that did take, after all, says nothing more.
        assert_eq!(shown(Status::Enabled, false, true).note, None);
        assert_eq!(shown(Status::RequiresApproval, false, true).note, Some(Note::NeedsApproval));
    }

    #[test]
    fn raw_statuses_map_like_sm_app_service_status() {
        assert_eq!(Status::from_raw(0), Status::NotRegistered);
        assert_eq!(Status::from_raw(1), Status::Enabled);
        assert_eq!(Status::from_raw(2), Status::RequiresApproval);
        assert_eq!(Status::from_raw(3), Status::NotFound);
        assert_eq!(Status::from_raw(42), Status::NotFound);
        assert_eq!(serde_json::to_string(&Status::RequiresApproval).expect("json"), r#""requiresApproval""#);
    }

    #[test]
    fn only_a_developer_id_build_outside_end_to_end_uses_sm_app_service() {
        assert_eq!(Backend::choose(true, false), Backend::AppService);
        assert_eq!(Backend::choose(false, false), Backend::LaunchAgent);
        assert_eq!(Backend::choose(true, true), Backend::LaunchAgent);
        assert_eq!(Backend::choose(false, true), Backend::LaunchAgent);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_test_binary_is_not_signed_with_a_developer_id() {
        // Linker-signed (ad hoc) by cargo: answered, and answered "no".
        assert_eq!(signed_with_developer_id(), Ok(false));
    }

    /// The requirement checked against code on disk, without hashing it all.
    #[cfg(target_os = "macos")]
    fn on_disk_meets_developer_id(path: &std::path::Path) -> Result<bool, String> {
        use core_foundation::url::CFURL;
        use security_framework::os::macos::code_signing::{Flags, SecStaticCode};

        let url = CFURL::from_path(path, path.is_dir()).ok_or("not a path")?;
        let code = SecStaticCode::from_path(&url, Flags::NONE).map_err(|error| format!("{error}"))?;
        answer(code.check_validity(Flags::BASIC_VALIDATE_ONLY, &developer_id()?))
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_requirement_takes_a_developer_id_and_nothing_else() {
        // Apple's own signature (a platform binary) is not a Developer ID.
        assert_eq!(on_disk_meets_developer_id(std::path::Path::new("/usr/bin/true")), Ok(false));
        // An ad hoc signature is not either.
        let exe = std::env::current_exe().expect("exe");
        assert_eq!(on_disk_meets_developer_id(&exe), Ok(false));

        // Where this Mac has apps signed otherwise, `codesign` is the referee:
        // a Developer ID Application authority must meet the requirement, an
        // App Store or Apple one must not. Droppen itself is left alone.
        let Ok(apps) = std::fs::read_dir("/Applications") else { return };
        let (mut developer_id, mut other) = (None, None);
        for app in apps.flatten().map(|entry| entry.path()) {
            if app.extension().is_none_or(|ext| ext != "app") || app.to_string_lossy().contains("Droppen") {
                continue;
            }
            let Ok(output) = std::process::Command::new("/usr/bin/codesign").arg("-dvv").arg(&app).output() else { continue };
            let text = String::from_utf8_lossy(&output.stderr);
            let Some(authority) = text.lines().find_map(|line| line.strip_prefix("Authority=")) else { continue };
            if authority.starts_with("Developer ID Application:") {
                developer_id.get_or_insert(app);
            } else if authority == "Apple Mac OS Application Signing" || authority == "Software Signing" {
                other.get_or_insert(app);
            }
            if developer_id.is_some() && other.is_some() {
                break;
            }
        }
        // What this Mac has installed is not the test's to vouch for: an app
        // that cannot be read (mid-update, damaged) is skipped, not a failure.
        let checked = |app: &std::path::Path| match on_disk_meets_developer_id(app) {
            Ok(meets) => Some(meets),
            Err(error) => {
                eprintln!("skipped {}: {error}", app.display());
                None
            }
        };
        if let Some(meets) = developer_id.as_deref().and_then(checked) {
            assert!(meets, "{:?} is signed with a Developer ID", developer_id);
        }
        if let Some(meets) = other.as_deref().and_then(checked) {
            assert!(!meets, "{:?} is not signed with a Developer ID", other);
        }
    }
}
