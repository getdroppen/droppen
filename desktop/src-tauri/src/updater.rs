//! The app updates itself, quietly: Tauri's updater plugin, driven by
//! [`droppen_desktop_core::update`]. It never shows a dialog, never takes
//! focus and never interrupts.
//!
//! * **Checks.** 15 seconds after launch, then every six hours (wall clock);
//!   a failed check is tried again in half an hour, silently. *Check for
//!   Updates…* in the menu-bar item and the app menu checks at once and
//!   answers in the item itself (*Up to Date (0.2.124)*, or one honest line
//!   saying what failed).
//! * **Download.** A newer release is downloaded in the background — not on a
//!   metered network ([`network`]) — and its minisign signature checked against
//!   the key in `tauri.conf.json` and against the version the manifest
//!   announced (`requireSignedVersion`). A download that does not verify is
//!   thrown away, and the menu says so. A verified one waits on disk, in the
//!   app's support directory (`update/`).
//! * **Ready.** Said passively: the menu item becomes *Restart to Update to
//!   0.2.125*, the menu-bar icon gets a dot, and the web app's rail entry gets
//!   a mark opening the update page, which shows the release's notes and the
//!   restart. *Release Notes* opens the release page.
//! * **Install.** At the next quit — the ordinary one: the login revoked and
//!   the server stopped with `SIGTERM` (`Shell::shutdown`), so terminals and
//!   running sessions carry on under their shims — the archive is verified
//!   again and swapped in for the bundle in one step (the plugin's
//!   `renamex_np(RENAME_SWAP)`), so the next launch is the new version. Never
//!   while the app runs. If the swap would take an admin password
//!   ([`writable_without_asking`]), a quit leaves the update for *Restart to
//!   Update*, which may ask for one; a refusal the app cannot foresee (a macOS
//!   privacy rule) still gets the plugin's password prompt at the quit. A
//!   failed install leaves the bundle as it was. *Restart to
//!   Update* (the menu, or the web app's entry) is the same quit, then the app
//!   starts the version now in place, with [`UPDATED_ARG`]; the new launch
//!   waits up to [`RELAUNCH_WAIT`] for the old one's single-instance lock.
//! * **Not here.** A development build (not in an app bundle) never updates;
//!   a copy run from App Translocation, a disk image or a path through a
//!   symbolic link says *Move Droppen to Applications to Update* instead of
//!   failing later ([`droppen_desktop_core::update::Place`]). The bundled
//!   server is only ever updated with the app — never by its own self-upgrade,
//!   which refuses under the app.
//!
//! The web app learns the update's status through `init.js`'s small bridge:
//! after every change and every page load the app calls
//! `__droppenDesktop.update(bridge, status, restart)` — in a window showing
//! the app's server only ([`web::is_server`], checked again in the page) —
//! and the page asks for a check or a restart with a
//! `droppen-desktop://update/check` or `…/restart` navigation carrying that
//! token, which the window cancels ([`check_asked_by_page`],
//! [`restart_asked_by_page`]). `bridge` and `restart` are this launch's own
//! random values, never the login nonce that guards the login `init.js`
//! keeps: a page that learnt either can at most ask for a check or the
//! restart, which the menu offers anyway.
//!
//! Every check that succeeds also reads the notes of the release it names
//! (the release body from GitHub's API, `update::notes_from_json`), so the
//! update page can show what changed instead of only linking to it. The
//! read is best effort and never blocks the update: it is a second request,
//! and a failure only leaves the page with the link.
//!
//! TODO(control channel): once the app has a channel to the server, carry
//! [`status`] / [`watch`] and [`check`] / [`restart_now`] over it instead of
//! the `init.js` bridge.
//!
//! An end-to-end test build (feature `e2e`) checks only when
//! `DROPPEN_DESKTOP_UPDATE_ENDPOINT` names a manifest, verified with
//! `DROPPEN_DESKTOP_UPDATE_PUBKEY` (its build allows `http://`, see
//! `desktop/build.sh`); `desktop/e2e-updater-macos.sh` serves them.

mod network;
mod words;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use droppen_desktop_core::update::{self, Checked, Next, Place, Problem, Release, Schedule, Status, Updates};
use tauri::menu::MenuItem;
use tauri::webview::{PageLoadEvent, PageLoadPayload};
use tauri::{AppHandle, Manager, Runtime, Webview};

use droppen_desktop_core::web;

use crate::i18n::Lang;
use crate::shell::Shell;
use words::Words;

/// The update menu item's id, in the menu-bar item and the app menu.
pub const MENU_ID: &str = "updates";
/// The release-notes item's id.
pub const NOTES_ID: &str = "update-notes";

/// The argument the app starts its new version with after *Restart to Update*.
pub const UPDATED_ARG: &str = "--droppen-updated";

/// How long the new version waits for the old one to go.
pub const RELAUNCH_WAIT: Duration = Duration::from_secs(30);

/// How long a check may take.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a connection may take, and a download may stall.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the release notes may take to read (the update itself never waits).
const NOTES_TIMEOUT: Duration = Duration::from_secs(15);
/// How often the schedule looks at the clock.
const TICK: Duration = Duration::from_secs(5);
/// The downloaded archive, in the support directory's `update/`.
const STAGED: &str = "Droppen.app.tar.gz";

/// The test build's manifest address (feature `e2e` only).
#[cfg(feature = "e2e")]
pub const ENDPOINT_ENV: &str = "DROPPEN_DESKTOP_UPDATE_ENDPOINT";
/// The test build's updater public key (feature `e2e` only).
#[cfg(feature = "e2e")]
pub const PUBKEY_ENV: &str = "DROPPEN_DESKTOP_UPDATE_PUBKEY";

type Watcher = Box<dyn Fn(&Status) + Send + Sync>;

struct Updater<R: Runtime> {
    updates: Mutex<Updates>,
    schedule: Mutex<Schedule>,
    item: MenuItem<R>,
    notes: MenuItem<R>,
    words: &'static Words,
    /// The app's executable, when it runs from an app bundle.
    executable: Option<PathBuf>,
    /// Where the verified download waits for the quit.
    staging: PathBuf,
    /// What the last check found: downloaded, then installed from.
    found: Mutex<Option<tauri_plugin_updater::Update>>,
    /// *Restart to Update* was chosen: start the app again after the quit,
    /// hidden if its window was.
    restart: AtomicBool,
    restart_hidden: AtomicBool,
    /// Whether the menu-bar icon shows the dot.
    dot: AtomicBool,
    watchers: Mutex<Vec<Watcher>>,
    /// What `init.js` requires of a status call (baked into it as the window
    /// is made), and what it hands back to ask for the restart: see the
    /// module notes.
    bridge: String,
    restart_token: String,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The updater plugin, configured from `tauri.conf.json` (`plugins.updater`).
pub fn plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R, tauri_plugin_updater::Config> {
    tauri_plugin_updater::Builder::new().build()
}

/// Whether this launch is the version *Restart to Update* started.
pub fn relaunched() -> bool {
    std::env::args().any(|arg| arg == UPDATED_ARG)
}

/// Where the app runs from: its executable, if in a bundle, and whether it
/// can be updated there.
fn place() -> (Option<PathBuf>, Place) {
    let Ok(exe) = std::env::current_exe() else {
        return (None, Place::NotABundle);
    };
    let canonical = exe.canonicalize().ok();
    let place = Place::of(&exe, canonical.as_deref(), read_only(&exe));
    #[cfg(feature = "e2e")]
    if place == Place::Updatable && std::env::var_os(ENDPOINT_ENV).is_none() {
        // A test build without a test endpoint never checks the real one.
        return (Some(exe), Place::NotABundle);
    }
    ((place != Place::NotABundle).then_some(exe), place)
}

/// Whether `path` is on a volume mounted read-only (a disk image).
#[cfg(target_os = "macos")]
fn read_only(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: a NUL-terminated path and a buffer statfs fills.
    if unsafe { libc::statfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return false;
    }
    // SAFETY: statfs succeeded, so the buffer is filled.
    let flags = unsafe { stat.assume_init() }.f_flags;
    flags & libc::MNT_RDONLY as u32 != 0
}

#[cfg(not(target_os = "macos"))]
fn read_only(_path: &Path) -> bool {
    false
}

/// Set up the updates: the two menu items (which `tray` puts in both menus)
/// and the schedule. Before `tray::install`.
pub fn install<R: Runtime>(app: &AppHandle<R>, support: &Path) -> tauri::Result<()> {
    let words = Words::of(Lang::system());
    let (executable, place) = place();
    let current = app.package_info().version.to_string();
    let updates = Updates::new(current.clone(), place);
    let menu = updates.menu();
    let item = MenuItem::with_id(app, MENU_ID, words.menu(&menu), menu.enabled(), None::<&str>)?;
    let notes = MenuItem::with_id(app, NOTES_ID, words.release_notes, place != Place::NotABundle, None::<&str>)?;
    let staging = support.join("update");
    // A download from an earlier run is not trusted to be the newest: start over.
    let _ = std::fs::remove_dir_all(&staging);
    app.manage(Updater {
        updates: Mutex::new(updates),
        schedule: Mutex::new(Schedule::default()),
        item,
        notes,
        words,
        executable,
        staging,
        found: Mutex::new(None),
        restart: AtomicBool::new(false),
        restart_hidden: AtomicBool::new(false),
        dot: AtomicBool::new(false),
        watchers: Mutex::new(Vec::new()),
        bridge: web::new_nonce(),
        restart_token: web::new_nonce(),
    });
    match place {
        Place::Updatable => {
            log::info!("updates: on for Droppen {current}; first check in {}s", update::FIRST_CHECK.as_secs());
            // Known well before the first download is due.
            network::start();
            schedule(app.clone());
        }
        other => log::info!("updates: off ({other:?})"),
    }
    Ok(())
}

/// What `init.js` requires of the app's status calls (see the module notes);
/// a fresh value nothing will match when the updater is not set up.
pub fn bridge_token<R: Runtime>(app: &AppHandle<R>) -> String {
    app.try_state::<Updater<R>>()
        .map(|me| me.bridge.clone())
        .unwrap_or_else(web::new_nonce)
}

/// The update item and the release-notes item, for the menus.
pub fn menu_items<R: Runtime>(app: &AppHandle<R>) -> Vec<MenuItem<R>> {
    app.try_state::<Updater<R>>()
        .map(|me| vec![me.item.clone(), me.notes.clone()])
        .unwrap_or_default()
}

/// The update now (see the module's TODO(control channel)).
pub fn status<R: Runtime>(app: &AppHandle<R>) -> Option<Status> {
    app.try_state::<Updater<R>>().map(|me| lock(&me.updates).status())
}

/// Call `watcher` with every change — from any thread, so it must not block.
pub fn watch<R: Runtime>(app: &AppHandle<R>, watcher: impl Fn(&Status) + Send + Sync + 'static) {
    if let Some(me) = app.try_state::<Updater<R>>() {
        lock(&me.watchers).push(Box::new(watcher));
    }
}

/// A menu item of the update's was chosen: the update item checks, or
/// restarts into the update that is ready; the notes item opens the notes.
pub fn menu_chosen<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let Some(me) = app.try_state::<Updater<R>>() else { return };
    if id == NOTES_ID {
        let notes = lock(&me.updates).notes();
        open_notes(app, &notes);
        return;
    }
    let restart = lock(&me.updates).restart_wanted();
    if restart {
        restart_now(app);
    } else {
        check(app, true);
    }
}

fn schedule<R: Runtime>(app: AppHandle<R>) {
    let started = Instant::now();
    let spawned = std::thread::Builder::new().name("droppen-updates".into()).spawn(move || loop {
        std::thread::sleep(TICK);
        let Some(me) = app.try_state::<Updater<R>>() else { return };
        let now = SystemTime::now();
        if lock(&me.updates).tick(now) {
            changed(&app);
        }
        let due = lock(&me.schedule).due(now, started.elapsed());
        if due {
            check(&app, false);
        }
    });
    if let Err(error) = spawned {
        log::warn!("updates: cannot schedule checks: {error}");
    }
}

/// The updater, with its time limits (and a test build's endpoint and key).
fn updater<R: Runtime>(app: &AppHandle<R>) -> Result<tauri_plugin_updater::Updater, tauri_plugin_updater::Error> {
    use tauri_plugin_updater::UpdaterExt;
    #[allow(unused_mut)]
    let mut builder = app
        .updater_builder()
        .timeout(CHECK_TIMEOUT)
        .configure_client(|client| client.connect_timeout(CONNECT_TIMEOUT).read_timeout(READ_TIMEOUT));
    #[cfg(feature = "e2e")]
    if let Some(endpoint) = std::env::var(ENDPOINT_ENV).ok().and_then(|endpoint| url::Url::parse(&endpoint).ok()) {
        builder = builder.endpoints(vec![endpoint])?;
        if let Ok(pubkey) = std::env::var(PUBKEY_ENV) {
            builder = builder.pubkey(pubkey);
        }
    }
    builder.build()
}

/// The public key the app verifies updates with.
fn pubkey<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    #[cfg(feature = "e2e")]
    if let Ok(pubkey) = std::env::var(PUBKEY_ENV) {
        return Some(pubkey);
    }
    app.config().plugins.0.get("updater")?.get("pubkey")?.as_str().map(str::to_string)
}

/// What a failure of the plugin's means to the person (see [`Problem`]):
/// only a network failure is the network.
pub fn problem(error: &tauri_plugin_updater::Error) -> Problem {
    use std::io::ErrorKind;
    use tauri_plugin_updater::Error as E;
    match error {
        E::Reqwest(error) if error.is_status() || error.is_decode() => Problem::Service,
        E::Reqwest(_) => Problem::Offline,
        // The plugin's own words for a download answered with an error status.
        E::Network(message) if message.contains("status") => Problem::Service,
        E::Network(_) => Problem::Offline,
        E::ReleaseNotFound
        | E::Serialization(_)
        | E::Semver(_)
        | E::TargetNotFound(_)
        | E::TargetsNotFound(_)
        | E::UrlParse(_)
        | E::EmptyEndpoints
        | E::InsecureTransportProtocol => Problem::Service,
        E::Minisign(_) | E::Base64(_) | E::SignatureUtf8(_) | E::SignedVersionMismatch { .. } | E::MissingSignedVersion => Problem::Signature,
        E::TempDirNotOnSameMountPoint | E::FailedToDetermineExtractPath | E::AuthenticationFailed => Problem::Location,
        E::Io(error) if matches!(error.kind(), ErrorKind::PermissionDenied | ErrorKind::ReadOnlyFilesystem) => Problem::Location,
        // `current_exe()` through a symbolic link, which the plugin refuses.
        E::Io(error) if error.to_string().contains("symlink") => Problem::Location,
        E::Io(_) | E::BinaryNotFoundInArchive | E::InvalidUpdaterFormat | E::TempDirNotFound | E::PreviousAppNotRestored(_) => Problem::Install,
        _ => Problem::Other,
    }
}

/// Check for an update (`asked`: the person chose to, so the menu answers).
pub fn check<R: Runtime>(app: &AppHandle<R>, asked: bool) {
    let Some(me) = app.try_state::<Updater<R>>() else { return };
    if !lock(&me.updates).begin_check(asked) {
        return;
    }
    lock(&me.schedule).started(SystemTime::now());
    log::info!("updates: checking{}", if asked { " (asked)" } else { "" });
    changed(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let found = match updater(&app) {
            Ok(updater) => updater.check().await,
            Err(error) => Err(error),
        };
        let checked = match found {
            // The manifest is not signed: its archive address must be the
            // release's own (or the end-to-end test's loopback server).
            Ok(Some(found)) if update::check_download_url(found.download_url.as_str(), &found.version, cfg!(feature = "e2e")).is_err() => {
                log::warn!(
                    "updates: the manifest names {} for Droppen {}, not its release's archive; not downloaded",
                    found.download_url,
                    found.version
                );
                Checked::Failed(Problem::Service)
            }
            Ok(Some(found)) => {
                let release = Release {
                    version: found.version.clone(),
                    notes: update::notes_link(found.body.as_deref(), &found.version),
                };
                if let Some(me) = app.try_state::<Updater<R>>() {
                    *lock(&me.found) = Some(found);
                }
                Checked::Found(release)
            }
            Ok(None) => Checked::UpToDate,
            Err(error) => {
                log::warn!("updates: the check failed: {error}");
                Checked::Failed(problem(&error))
            }
        };
        let Some(me) = app.try_state::<Updater<R>>() else { return };
        // The update page's notes: the release in hand, or this version's when
        // there is nothing newer. Started here so the page has them by the
        // time anyone opens it; the read never delays the download.
        match &checked {
            Checked::Found(release) => fetch_notes(app.clone(), release.version.clone()),
            Checked::UpToDate => fetch_notes(app.clone(), lock(&me.updates).current().to_string()),
            Checked::Failed(_) => {}
        }
        if matches!(checked, Checked::Failed(_)) {
            lock(&me.schedule).failed();
        }
        let next = {
            let mut updates = lock(&me.updates);
            match &checked {
                Checked::Found(release) => log::info!("updates: Droppen {} is available (this is {})", release.version, updates.current()),
                Checked::UpToDate => log::info!("updates: up to date ({})", updates.current()),
                Checked::Failed(_) => {}
            }
            updates.end_check(checked, SystemTime::now())
        };
        match next {
            Next::Download(release) => download(&app, release),
            Next::Discard => {
                log::info!("updates: the downloaded update is no longer the newest; thrown away");
                let _ = std::fs::remove_dir_all(&me.staging);
            }
            Next::Nothing => {}
        }
        changed(&app);
    });
}

/// Read the notes of Droppen `version` (its GitHub release body) and keep them
/// for the update page. Best effort: a failure leaves the page its link.
fn fetch_notes<R: Runtime>(app: AppHandle<R>, version: String) {
    #[cfg(feature = "e2e")]
    {
        // A test build never asks GitHub: its manifest comes from the loopback
        // server `desktop/e2e-updater-macos.sh` serves.
        let _ = (app, version);
        return;
    }
    #[cfg(not(feature = "e2e"))]
    {
        if app
            .try_state::<Updater<R>>()
            .is_some_and(|me| lock(&me.updates).cached_notes().is_some_and(|notes| notes.version == version))
        {
            return;
        }
        let Some(url) = update::notes_api_url(&version) else {
            return;
        };
        tauri::async_runtime::spawn(async move {
            let client = match reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(NOTES_TIMEOUT)
                .user_agent(droppen_desktop_core::server::USER_AGENT)
                .build()
            {
                Ok(client) => client,
                Err(error) => {
                    log::info!("updates: cannot read the notes of Droppen {version}: {error}");
                    return;
                }
            };
            let response = match client.get(&url).send().await {
                Ok(response) => response,
                Err(error) => {
                    log::info!("updates: cannot read the notes of Droppen {version}: {error}");
                    return;
                }
            };
            if !response.status().is_success() {
                log::info!("updates: no notes for Droppen {version}: HTTP {}", response.status());
                return;
            }
            let Ok(json) = response.text().await else {
                return;
            };
            if json.len() > 256 * 1024 {
                log::info!("updates: Droppen {version}'s release body is too large to read");
                return;
            }
            let Some(notes) = update::notes_from_json(&json, &version) else {
                log::info!("updates: Droppen {version}'s release body names no notes");
                return;
            };
            let Some(me) = app.try_state::<Updater<R>>() else { return };
            log::info!("updates: read the notes of Droppen {version}");
            lock(&me.updates).set_notes(Some(notes));
            changed(&app);
        });
    }
}

/// Download `release` in the background, verify it, and put it aside.
fn download<R: Runtime>(app: &AppHandle<R>, release: Release) {
    let Some(me) = app.try_state::<Updater<R>>() else { return };
    let _ = std::fs::remove_dir_all(&me.staging);
    if network::metered() {
        log::info!("updates: Droppen {} waits for a network that is not metered", release.version);
        lock(&me.updates).postpone();
        lock(&me.schedule).failed();
        return;
    }
    let Some(found) = lock(&me.found).clone().filter(|found| found.version == release.version) else {
        lock(&me.updates).download_failed(Problem::Other, SystemTime::now());
        return;
    };
    log::info!("updates: downloading Droppen {} from {}", release.version, found.download_url);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let progress = app.clone();
        let mut received = 0u64;
        let downloaded = found
            .download(
                move |chunk, total| {
                    received += chunk as u64;
                    if let Some(me) = progress.try_state::<Updater<R>>() {
                        if lock(&me.updates).progress(received, total) {
                            changed(&progress);
                        }
                    }
                },
                || {},
            )
            .await;
        let Some(me) = app.try_state::<Updater<R>>() else { return };
        let staged = downloaded.map_err(|error| (problem(&error), error.to_string())).and_then(|bytes| {
            stage(&me.staging, &bytes)
                .map(|()| bytes.len())
                .map_err(|error| (Problem::Install, format!("cannot keep the download: {error}")))
        });
        match staged {
            Ok(size) => {
                log::info!(
                    "updates: Droppen {} downloaded and verified ({size} bytes); it is installed at the next quit",
                    release.version
                );
                lock(&me.updates).downloaded(SystemTime::now());
            }
            Err((problem, error)) => {
                log::warn!("updates: the download of Droppen {} was not kept ({problem:?}): {error}", release.version);
                let _ = std::fs::remove_dir_all(&me.staging);
                if !problem.worth_saying_unasked() {
                    lock(&me.schedule).failed();
                }
                lock(&me.updates).download_failed(problem, SystemTime::now());
            }
        }
        changed(&app);
    });
}

/// Keep the verified archive, private to this user, until the quit.
fn stage(staging: &Path, bytes: &[u8]) -> std::io::Result<()> {
    droppen_desktop_core::dirs::create_private_dir(staging)?;
    droppen_desktop_core::state::write_atomically(&staging.join(STAGED), bytes, 0o600)
}

/// *Restart to Update*: quit (which installs) and start the app again — only
/// if an update is downloaded and verified.
pub fn restart_now<R: Runtime>(app: &AppHandle<R>) {
    let Some(me) = app.try_state::<Updater<R>>() else { return };
    if !lock(&me.updates).restart_wanted() {
        log::info!("updates: a restart was asked for, but no update is ready");
        return;
    }
    let hidden = app
        .get_webview_window(crate::window::MAIN)
        .and_then(|window| window.is_visible().ok())
        .is_some_and(|visible| !visible);
    me.restart_hidden.store(hidden, Ordering::SeqCst);
    me.restart.store(true, Ordering::SeqCst);
    log::info!("updates: restarting to update");
    app.exit(0);
}

/// The web app's update page asked for a check (`init.js`): honoured only
/// with the token this launch handed the app's pages.
pub fn check_asked_by_page<R: Runtime>(app: &AppHandle<R>, token: &str) {
    let ours = app.try_state::<Updater<R>>().is_some_and(|me| !token.is_empty() && me.restart_token == token);
    if ours {
        check(app, true);
    } else {
        log::warn!("updates: a check request without this launch's token was ignored");
    }
}

/// The web app's entry asked for the restart (`init.js`): honoured only with
/// the restart token this launch handed the app's pages.
pub fn restart_asked_by_page<R: Runtime>(app: &AppHandle<R>, token: &str) {
    let ours = app.try_state::<Updater<R>>().is_some_and(|me| !token.is_empty() && me.restart_token == token);
    if ours {
        restart_now(app);
    } else {
        log::warn!("updates: a restart request without this launch's restart token was ignored");
    }
}

/// On the way out (`RunEvent::Exit`, once the server is stopped): install
/// the downloaded update, if there is one and it can be done without asking
/// (or *Restart to Update* was chosen); then, after a restart, start the app
/// again — the new version, or the old one if the install failed.
pub fn at_exit<R: Runtime>(app: &AppHandle<R>) {
    let Some(me) = app.try_state::<Updater<R>>() else { return };
    let restart = me.restart.load(Ordering::SeqCst);
    let ready = lock(&me.updates).ready().cloned();
    if let (Some(release), Some(executable)) = (ready, me.executable.as_deref()) {
        if restart || writable_without_asking(executable) {
            lock(&me.updates).begin_install();
            let started = Instant::now();
            match install_staged(app, &me, &release) {
                Ok(()) => {
                    lock(&me.updates).installed();
                    log::info!(
                        "updates: installed Droppen {} in {:.1}s",
                        release.version,
                        started.elapsed().as_secs_f32()
                    );
                }
                Err((problem, error)) => {
                    lock(&me.updates).install_failed(problem, SystemTime::now());
                    log::warn!("updates: not installed ({problem:?}), Droppen is as it was: {error}");
                }
            }
        } else {
            log::info!(
                "updates: installing Droppen {} needs an admin password; it waits for Restart to Update",
                release.version
            );
        }
    }
    let _ = std::fs::remove_dir_all(&me.staging);
    if restart {
        relaunch(me.executable.as_deref(), me.restart_hidden.load(Ordering::SeqCst));
    }
}

/// Whether the bundle can be swapped without an admin password, so that an
/// ordinary quit may install: the bundle and its folder writable by this user
/// (`access`), neither locked (`uchg`/`schg`) nor append-only
/// (`uappnd`/`sappnd`, which lets nothing be renamed out), and, in a folder
/// with the sticky bit, the bundle this user's own — the cases where
/// the swap would fail with a permission error. The plugin answers that error
/// with the admin password prompt (its privileged install), which a quit must
/// never show; that prompt is for *Restart to Update* only. (What this cannot
/// see — a macOS privacy rule refusing the swap — still ends in the prompt.)
fn writable_without_asking(executable: &Path) -> bool {
    let Some(bundle) = executable.ancestors().nth(3) else { return false };
    let Some(folder) = bundle.parent() else { return false };
    swappable(bundle, folder)
}

fn swappable(bundle: &Path, folder: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    let writable = [bundle, folder].iter().all(|path| {
        std::ffi::CString::new(path.as_os_str().as_bytes())
            // SAFETY: a NUL-terminated path.
            .is_ok_and(|path| unsafe { libc::access(path.as_ptr(), libc::W_OK) } == 0)
    });
    let (Ok(bundle_meta), Ok(folder_meta)) = (std::fs::symlink_metadata(bundle), std::fs::metadata(folder)) else {
        return false;
    };
    let locked_flags = (libc::UF_IMMUTABLE | libc::SF_IMMUTABLE | libc::UF_APPEND | libc::SF_APPEND) as u32;
    let locked = [bundle, folder].iter().any(|path| flags(path).is_some_and(|flags| flags & locked_flags != 0));
    // SAFETY: getuid cannot fail.
    let sticky_not_ours = folder_meta.mode() & libc::S_ISVTX as u32 != 0 && bundle_meta.uid() != unsafe { libc::getuid() };
    writable && !locked && !sticky_not_ours
}

/// A path's BSD file flags (`chflags`), without following a symbolic link.
fn flags(path: &Path) -> Option<u32> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: a NUL-terminated path and a buffer lstat fills.
    if unsafe { libc::lstat(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: lstat succeeded, so the buffer is filled.
    Some(unsafe { stat.assume_init() }.st_flags)
}

/// Verify the kept archive again, then swap it in.
fn install_staged<R: Runtime>(app: &AppHandle<R>, me: &Updater<R>, release: &Release) -> Result<(), (Problem, String)> {
    let found = lock(&me.found)
        .clone()
        .filter(|found| found.version == release.version)
        .ok_or((Problem::Other, "the update is not the one downloaded".to_string()))?;
    let bytes = std::fs::read(me.staging.join(STAGED)).map_err(|error| (Problem::Install, format!("the download is gone: {error}")))?;
    let key = pubkey(app).ok_or((Problem::Other, "no updater key".to_string()))?;
    update::check_signature(&found.signature, &key, &release.version, Some(&bytes)).map_err(|error| (Problem::Signature, error))?;
    found.install(&bytes).map_err(|error| (problem(&error), error.to_string()))
}

/// Start the app again: the version now in place.
fn relaunch(executable: Option<&Path>, hidden: bool) {
    let Some(executable) = executable else { return };
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let mut command = Command::new(executable);
    command.arg(UPDATED_ARG);
    if hidden {
        command.arg(crate::AUTOSTART_ARG);
    }
    match command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
    {
        Ok(child) => log::info!("updates: started Droppen again (pid {})", child.id()),
        Err(error) => log::error!("updates: cannot start Droppen again, open it by hand: {error}"),
    }
}

/// The menu items, the menu-bar dot, the web app and the watchers follow the
/// state. Drawn on the main thread, posted and never waited for.
fn changed<R: Runtime>(app: &AppHandle<R>) {
    let Some(me) = app.try_state::<Updater<R>>() else { return };
    let (menu, status, ready) = {
        let updates = lock(&me.updates);
        (updates.menu(), updates.status(), updates.ready().is_some())
    };
    let text = me.words.menu(&menu);
    let enabled = menu.enabled();
    let item = me.item.clone();
    let dot = (me.dot.swap(ready, Ordering::SeqCst) != ready).then_some(ready);
    let handle = app.clone();
    let script = bridge_script(app, &status);
    let _ = app.run_on_main_thread(move || {
        let _ = item.set_text(text);
        let _ = item.set_enabled(enabled);
        if let Some(dot) = dot {
            crate::tray::show_update_dot(&handle, dot);
        }
        if let Some((port, script)) = script {
            // Only into the app's own pages: a window that shows anything else
            // gets nothing (the script checks the page's origin again).
            for window in handle.webview_windows().into_values() {
                if window.url().is_ok_and(|url| web::is_server(&url, port)) {
                    let _ = window.eval(&script);
                }
            }
        }
    });
    for watcher in lock(&me.watchers).iter() {
        watcher(&status);
    }
}

/// The call that hands `status` to `init.js`'s bridge, with the server's port
/// it is for: it runs only in a page of that origin, and carries the bridge
/// and restart tokens, never the login nonce.
fn bridge_script<R: Runtime>(app: &AppHandle<R>, status: &Status) -> Option<(u16, String)> {
    let me = app.try_state::<Updater<R>>()?;
    let port = app.try_state::<Arc<Shell>>()?.port()?;
    Some((port, call_script(port, &me.bridge, &me.restart_token, status)?))
}

fn call_script(port: u16, bridge: &str, restart: &str, status: &Status) -> Option<String> {
    let origin = serde_json::to_string(&format!("http://127.0.0.1:{port}")).ok()?;
    let bridge = serde_json::to_string(bridge).ok()?;
    let restart = serde_json::to_string(restart).ok()?;
    let status = serde_json::to_string(status).ok()?;
    Some(format!(
        "(function(){{if(location.origin!=={origin})return;var d=window.__droppenDesktop;if(d)d.update({bridge},{status},{restart});}})()"
    ))
}

/// A page of the app's server finished loading: tell it the update's status.
pub fn page_loaded<R: Runtime>(webview: &Webview<R>, payload: &PageLoadPayload<'_>) {
    if payload.event() != PageLoadEvent::Finished {
        return;
    }
    let app = webview.app_handle();
    let Some((port, script)) = status(app).and_then(|status| bridge_script(app, &status)) else {
        return;
    };
    if web::is_server(payload.url(), port) {
        let _ = webview.eval(&script);
    }
}

fn open_notes<R: Runtime>(app: &AppHandle<R>, notes: &str) {
    #[cfg(feature = "e2e")]
    {
        let _ = app;
        log::info!("updates: would open the release notes {notes} (e2e build: not opened)");
    }
    #[cfg(not(feature = "e2e"))]
    {
        use tauri_plugin_opener::OpenerExt;
        log::info!("updates: opening the release notes {notes}");
        if let Err(error) = app.opener().open_url(notes, None::<&str>) {
            log::warn!("updates: cannot open the browser: {error}");
        }
    }
}

/// The end-to-end test hooks' view of the update (feature `e2e`).
#[cfg(feature = "e2e")]
pub mod testing {
    use super::*;

    /// The update item's text and whether it can be chosen, the notes item's
    /// text, whether the menu-bar dot shows and whether a download is kept.
    pub fn menu<R: Runtime>(app: &AppHandle<R>) -> Option<serde_json::Value> {
        let me = app.try_state::<Updater<R>>()?;
        Some(serde_json::json!({
            "text": me.item.text().ok()?,
            "enabled": me.item.is_enabled().ok()?,
            "notes": me.notes.text().ok()?,
            "dot": me.dot.load(Ordering::SeqCst),
            "staged": me.staging.join(STAGED).exists(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri_plugin_updater::Error as E;

    #[test]
    fn the_bridge_call_runs_only_on_the_servers_origin_and_carries_no_login_nonce() {
        let status = Updates::new("0.2.125".to_string(), Place::Updatable).status();
        let script = call_script(51_234, "bridge-1", "restart-2", &status).expect("a script");
        assert!(script.starts_with(r#"(function(){if(location.origin!=="http://127.0.0.1:51234")return;"#), "{script}");
        assert!(script.contains(r#"d.update("bridge-1","#), "{script}");
        assert!(script.contains(r#","restart-2");"#), "{script}");
    }

    #[test]
    fn a_quit_installs_only_where_the_swap_needs_no_admin_password() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let bundle = dir.path().join("Droppen.app");
        std::fs::create_dir(&bundle).expect("a bundle");
        let executable = bundle.join("Contents/MacOS/Droppen");
        std::fs::create_dir_all(executable.parent().expect("a parent")).expect("Contents/MacOS");
        std::fs::write(&executable, b"").expect("an executable");
        assert!(writable_without_asking(&executable));

        // Locked (Finder's "Locked", `chflags uchg`) or append-only (`chflags
        // uappnd`), the bundle or its folder: the swap would be refused.
        let set = |path: &Path, flags: u32| {
            let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).expect("a path");
            // SAFETY: a NUL-terminated path.
            assert_eq!(unsafe { libc::chflags(path.as_ptr(), flags as _) }, 0);
        };
        for path in [bundle.as_path(), dir.path()] {
            for flag in [libc::UF_IMMUTABLE, libc::UF_APPEND] {
                set(path, flag as u32);
                let swappable = writable_without_asking(&executable);
                set(path, 0);
                assert!(!swappable, "{} with flag {flag:#x}", path.display());
            }
        }
        assert!(writable_without_asking(&executable));

        // Not writable: an admin's folder.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).expect("chmod");
        assert!(!writable_without_asking(&executable));
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).expect("chmod");

        // Not in a bundle at all.
        assert!(!writable_without_asking(Path::new("Droppen")));
    }

    #[test]
    fn failures_are_named_honestly() {
        use std::io::{Error as Io, ErrorKind};
        let cases = [
            (E::Network("connection reset".into()), Problem::Offline),
            (E::Network("Download request failed with status: 404 Not Found".into()), Problem::Service),
            (E::ReleaseNotFound, Problem::Service),
            (E::TargetsNotFound(vec!["darwin-aarch64".into()]), Problem::Service),
            (E::Minisign(minisign_verify::Error::InvalidSignature), Problem::Signature),
            (E::Minisign(minisign_verify::Error::UnexpectedKeyId), Problem::Signature),
            (
                E::SignedVersionMismatch {
                    signed: "0.2.124".into(),
                    announced: "0.2.125".into(),
                },
                Problem::Signature,
            ),
            (E::MissingSignedVersion, Problem::Signature),
            (
                E::Io(Io::other(
                    "StartingBinary found current_exe() that contains a symlink on a non-allowed platform: /tmp",
                )),
                Problem::Location,
            ),
            (E::Io(Io::from(ErrorKind::PermissionDenied)), Problem::Location),
            (E::Io(Io::from(ErrorKind::ReadOnlyFilesystem)), Problem::Location),
            (E::AuthenticationFailed, Problem::Location),
            (E::TempDirNotOnSameMountPoint, Problem::Location),
            (E::Io(Io::from(ErrorKind::UnexpectedEof)), Problem::Install),
            (E::InvalidUpdaterFormat, Problem::Install),
            (
                E::PreviousAppNotRestored("/Applications/Droppen (previous version).app".into()),
                Problem::Install,
            ),
            (E::FormatDate, Problem::Other),
        ];
        for (error, expected) in cases {
            assert_eq!(problem(&error), expected, "{error}");
        }
    }
}
