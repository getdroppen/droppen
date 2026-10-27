//! The app's own updates, without Tauri: when to check, what the menu says,
//! and the update manifest (`latest.json`) a release publishes.
//!
//! The Tauri updater plugin does the network, the signature check and the swap
//! of the bundle (`src-tauri/src/updater.rs`); what it is told to do is decided
//! here, where it can be tested without a window:
//!
//! * [`Updates`] — the state machine of a quiet updater: it checks, downloads
//!   and verifies by itself, installs at the next quit, and never asks — the
//!   menu says where it is;
//! * [`Place`] — whether the app can be updated where it runs from;
//! * [`Schedule`] — a check shortly after launch, then every six hours of
//!   wall-clock time, so a Mac that slept through the night checks on waking;
//! * [`Manifest`] — `latest.json`, as `desktop/updater-artifacts.sh` writes it
//!   (through `droppen-update-manifest`) and as the release checks it.
//!
//! # What a release publishes
//!
//! On the GitHub release `v<version>` of `getdroppen/droppen`, beside the DMG:
//! `Droppen.app.tar.gz` (the final, notarised and stapled app — universal, so
//! one archive serves both of the updater's macOS targets), its minisign
//! signature `Droppen.app.tar.gz.sig` made with the updater key, and
//! `latest.json`, which the app fetches from the *latest* release
//! ([`ENDPOINT`]). The manifest is not signed; the archive is, and its
//! signature names the version it was made for, which the app requires to be
//! the version the manifest announces (`requireSignedVersion`). `notes` is the
//! address of the release notes: the app links to it. The text the app's
//! update page shows comes from the release body instead — GitHub's API for
//! the tag [`notes_api_url`], the same notes the release page carries.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use base64::Engine;
use serde::{Deserialize, Serialize};
use url::Url;

/// The public repository the releases are published in: the source's own.
/// (Up to v0.2.126 it was `getdroppen/releases`, which v0.2.127 bridged:
/// docs/releasing.md.)
pub const RELEASES_REPO: &str = "getdroppen/droppen";

/// Where the app looks for the newest release's manifest (also in
/// `src-tauri/tauri.conf.json`, `plugins.updater.endpoints`).
pub const ENDPOINT: &str = "https://github.com/getdroppen/droppen/releases/latest/download/latest.json";

/// The update archive's file name on a release.
pub const ARCHIVE: &str = "Droppen.app.tar.gz";

/// The minisign key id of the updater key releases are signed with: the key
/// in `src-tauri/tauri.conf.json` (`plugins.updater.pubkey`), whose private
/// half is kept as `desktop/SIGNING.md` ("The key") says. A test holds the
/// two together, so the key is never replaced by accident: every installed
/// app trusts only this one.
pub const RELEASE_KEY_ID: &str = "1CB79070988604BF";

/// The updater targets a manifest names: one universal archive serves both.
pub const TARGETS: [&str; 2] = ["darwin-aarch64", "darwin-x86_64"];

/// How long after launch the first check waits, so it never slows the start.
pub const FIRST_CHECK: Duration = Duration::from_secs(15);

/// How often the app checks after that.
pub const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// How soon a check that failed (offline, say) is tried again.
pub const RETRY_AFTER: Duration = Duration::from_secs(30 * 60);

/// The release page of `version`, where its notes are.
pub fn release_page(version: &str) -> String {
    format!("https://github.com/{RELEASES_REPO}/releases/tag/v{version}")
}

/// Where the archive of `version` is published.
pub fn archive_url(version: &str) -> String {
    format!("https://github.com/{RELEASES_REPO}/releases/download/v{version}/{ARCHIVE}")
}

/// The release notes to link to for `version`: the manifest's `notes` when it
/// is an `https` page of the releases repository or of droppen.app, else the
/// release page. The manifest is unsigned, so nothing else is ever opened.
pub fn notes_link(notes: Option<&str>, version: &str) -> String {
    notes
        .and_then(|notes| allowed_notes(notes.trim()))
        .unwrap_or_else(|| release_page(version))
}

fn allowed_notes(notes: &str) -> Option<String> {
    let url = Url::parse(notes).ok()?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
        return None;
    }
    let allowed = match url.host_str()? {
        // The repository's releases only, not the rest of the source's pages.
        "github.com" => url.path().starts_with(&format!("/{RELEASES_REPO}/releases/")),
        "droppen.app" | "www.droppen.app" => true,
        _ => false,
    };
    allowed.then(|| url.to_string())
}

/// Where the release body of `version` is read (GitHub's API, the same text
/// its release page shows): `None` for anything that is not a plain version.
pub fn notes_api_url(version: &str) -> Option<String> {
    is_version(version).then(|| format!("https://api.github.com/repos/{RELEASES_REPO}/releases/tags/v{version}"))
}

/// A release's notes, as the update page reads them: the two "what's new"
/// sections of its body, one per language, without their headings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notes {
    /// The release they belong to.
    pub version: String,
    /// `## What's new in v<version>` and what follows, in markdown.
    pub english: String,
    /// `## v<version> 更新内容` and what follows, in markdown.
    pub chinese: String,
}

/// The notes in a GitHub release's JSON (`GET /repos/…/releases/tags/v<version>`).
pub fn notes_from_json(json: &str, version: &str) -> Option<Notes> {
    #[derive(Deserialize)]
    struct Release {
        body: Option<String>,
    }
    let release: Release = serde_json::from_str(json).ok()?;
    whats_new(release.body.as_deref()?, version)
}

/// The two "what's new" sections of a release body (`docs/releases/<version>.md`,
/// which the release workflow puts in the body): each runs from its `##`
/// heading to the next one. `None` when the body has neither, as a snapshot
/// release's does not.
pub fn whats_new(body: &str, version: &str) -> Option<Notes> {
    let english = section(body, |heading| heading.starts_with("## What's new")).unwrap_or_default();
    let chinese = section(body, |heading| heading.starts_with("## v") && heading.ends_with("更新内容")).unwrap_or_default();
    (!english.is_empty() || !chinese.is_empty()).then(|| Notes {
        version: version.to_string(),
        english,
        chinese,
    })
}

/// The lines under the first `##` heading `matches`, up to the next `##` one.
fn section(body: &str, matches: impl Fn(&str) -> bool) -> Option<String> {
    let mut found = false;
    let mut lines: Vec<&str> = Vec::new();
    for line in body.lines() {
        if !found {
            found = matches(line.trim());
            continue;
        }
        if line.trim_start().starts_with("## ") {
            break;
        }
        lines.push(line);
    }
    if !found {
        return None;
    }
    let text = lines.join("\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Whether `version` is a plain semantic version: `1.2.3`, or `1.2.3-pre.1`
/// (test builds), with no `v` and no build metadata.
pub fn is_version(version: &str) -> bool {
    let (core, pre) = match version.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (version, None),
    };
    let numbers: Vec<&str> = core.split('.').collect();
    numbers.len() == 3
        && numbers.iter().all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        && pre.is_none_or(|pre| {
            !pre.is_empty()
                && pre
                    .split('.')
                    .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        })
}

// ---------------------------------------------------------------------------
// Where the app runs from
// ---------------------------------------------------------------------------

/// Where the app runs from, as far as updating it goes — decided at launch,
/// so the menu can say so instead of an update failing later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Place {
    /// An app bundle the updater can replace.
    Updatable,
    /// Not in an app bundle: a development build, which never updates.
    NotABundle,
    /// A temporary, read-only App Translocation copy: a quarantined app opened
    /// where it was downloaded, or straight from its disk image.
    Translocated,
    /// A read-only volume: the disk image it came on, say.
    ReadOnly,
    /// Its path goes through a symbolic link, which the updater refuses to
    /// follow (it could be made to replace something else).
    Symlinked,
}

impl Place {
    /// Classify `exe`, the app's executable as launched: `canonical` is it with
    /// every link resolved (None if that failed), `read_only` whether its
    /// volume is mounted read-only.
    pub fn of(exe: &std::path::Path, canonical: Option<&std::path::Path>, read_only: bool) -> Self {
        let in_bundle = exe
            .parent()
            .filter(|macos| macos.file_name().is_some_and(|name| name == "MacOS"))
            .and_then(|macos| macos.parent())
            .filter(|contents| contents.file_name().is_some_and(|name| name == "Contents"))
            .and_then(|contents| contents.parent())
            .is_some_and(|bundle| bundle.extension().is_some_and(|extension| extension == "app"));
        if !in_bundle {
            Self::NotABundle
        } else if crate::web::translocated(exe) {
            Self::Translocated
        } else if read_only {
            Self::ReadOnly
        } else if canonical != Some(exe) {
            Self::Symlinked
        } else {
            Self::Updatable
        }
    }
}

// ---------------------------------------------------------------------------
// The state machine
// ---------------------------------------------------------------------------

/// A release the manifest offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Release {
    /// Its version.
    pub version: String,
    /// Its release notes' address ([`notes_link`]).
    pub notes: String,
}

/// Where the update is. Nothing here ever asks anything: the app checks,
/// downloads and verifies on its own, installs at the next quit, and only
/// says so — in its menu, with a dot on the menu-bar icon, and on the web
/// app's update page (`web/src/features/updates/updates-view.tsx`), whose
/// rail entry always opens that page and never restarts by itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum State {
    /// Nothing to do: not checked yet, or up to date.
    Idle,
    /// A check is under way.
    Checking,
    /// A newer release is downloading, in the background.
    Downloading {
        /// The release.
        release: Release,
        /// How far, when the size is known.
        percent: Option<u8>,
    },
    /// Downloaded and verified, waiting on disk: installed at the next quit,
    /// or now if the person chooses to restart.
    Ready {
        /// The release.
        release: Release,
    },
    /// The bundle is being swapped, on the way out.
    Installing {
        /// The release.
        release: Release,
    },
    /// Installed; the app is starting the new version.
    Restarting {
        /// The release.
        release: Release,
    },
}

/// Why a check, a download or an install failed — in the person's terms, so
/// the menu can say it honestly: only a network failure says "network".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Problem {
    /// The network: no connection, a timeout, a dropped download.
    Offline,
    /// The update service answered, but not with an update the app can read:
    /// no manifest on the newest release, a malformed one, no Mac archive.
    Service,
    /// The download did not verify with the app's key, or was signed for
    /// another version than announced: it was thrown away.
    Signature,
    /// Where the app is cannot be updated (see [`Place`]), or the swap was
    /// refused (no permission, an admin prompt cancelled).
    Location,
    /// The archive verified but would not install; the app is as it was.
    Install,
    /// Anything else.
    Other,
}

impl Problem {
    /// Whether the menu says so even when nobody asked: a failure the next
    /// check will not simply cure. A network or service hiccup is retried
    /// later, silently.
    pub fn worth_saying_unasked(self) -> bool {
        matches!(self, Self::Signature | Self::Location | Self::Install)
    }
}

/// What a check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checked {
    /// A release newer than this app.
    Found(Release),
    /// Nothing newer.
    UpToDate,
    /// The check failed.
    Failed(Problem),
}

/// What the caller has to do after a step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// Nothing.
    Nothing,
    /// Download this release (it may wait for a network that is not metered).
    Download(Release),
    /// Throw away the downloaded update: it is no longer the one to install.
    Discard,
}

/// The last result worth showing in the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The person asked, and this is the newest version.
    UpToDate,
    /// Something failed, and is worth saying.
    Failed(Problem),
}

/// What the update menu item says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Menu {
    /// *Check for Updates…*
    Check,
    /// *Checking for Updates…* (disabled)
    Checking,
    /// *Downloading Update… 42%* (disabled)
    Downloading(Option<u8>),
    /// *Restart to Update to 0.2.125*
    RestartToUpdate(String),
    /// *Installing Update…* (disabled)
    Installing,
    /// *Restarting…* (disabled)
    Restarting,
    /// *Up to Date (0.2.124)* — after a check the person asked for.
    UpToDate(String),
    /// A short line saying what failed; choosing it checks again.
    Failed(Problem),
    /// *Move Droppen to Applications to Update* (disabled).
    MoveToApplications,
    /// A development build: *Check for Updates…*, disabled.
    Unavailable,
}

impl Menu {
    /// Whether the item can be chosen.
    pub fn enabled(&self) -> bool {
        matches!(self, Self::Check | Self::RestartToUpdate(_) | Self::UpToDate(_) | Self::Failed(_))
    }
}

/// A snapshot for whoever shows the update besides the menu: the web app's
/// update page and rail entry, through the app's bridge (`init.js`), and
/// later the control channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// The version running.
    pub current: String,
    /// Where the update is.
    #[serde(flatten)]
    pub state: State,
    /// Where the app runs from.
    pub place: Place,
    /// When the last check finished (RFC 3339, UTC).
    pub last_checked: Option<String>,
    /// The last failure worth saying, until something succeeds.
    pub problem: Option<Problem>,
    /// The notes of the release in hand (or the running one, when there is
    /// nothing newer), for the update page. `None` until they have been read.
    pub notes: Option<Notes>,
}

/// How long the answer to an asked-for check stays in the menu.
pub const ANSWER_SHOWN: Duration = Duration::from_secs(10 * 60);

/// The update's state machine. Each step says what the caller does next; the
/// rules live here:
///
/// * checks run on their own and download what they find, silently; a check
///   the person asked for answers in the menu (*Up to Date*, or why not) for
///   [`ANSWER_SHOWN`];
/// * a failure is said unasked only when it is worth it
///   ([`Problem::worth_saying_unasked`]); otherwise it is retried later;
/// * a downloaded update waits until the app quits, or until the person
///   restarts from the menu or the web app's entry;
/// * nothing at all happens where the app cannot be updated ([`Place`]).
#[derive(Debug, Clone)]
pub struct Updates {
    current: String,
    place: Place,
    state: State,
    /// The state before the check under way, kept if the check fails.
    before: State,
    /// Whether the check (or the download it started) was asked for.
    asked: bool,
    outcome: Option<Outcome>,
    /// When an asked-for answer leaves the menu.
    outcome_until: Option<SystemTime>,
    last_checked: Option<SystemTime>,
    /// The notes read for the release in hand (or this one), for the update page.
    notes: Option<Notes>,
}

impl Updates {
    /// Nothing known yet, running `current` from `place`.
    pub fn new(current: impl Into<String>, place: Place) -> Self {
        Self {
            current: current.into(),
            place,
            state: State::Idle,
            before: State::Idle,
            asked: false,
            outcome: None,
            outcome_until: None,
            last_checked: None,
            notes: None,
        }
    }

    /// The version running.
    pub fn current(&self) -> &str {
        &self.current
    }

    /// Where the app runs from.
    pub fn place(&self) -> Place {
        self.place
    }

    /// Where the update is.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// The release downloaded and waiting, if one is.
    pub fn ready(&self) -> Option<&Release> {
        match &self.state {
            State::Ready { release } => Some(release),
            _ => None,
        }
    }

    /// The snapshot.
    pub fn status(&self) -> Status {
        Status {
            current: self.current.clone(),
            state: self.state.clone(),
            place: self.place,
            last_checked: self.last_checked.map(crate::logging::timestamp),
            problem: match self.outcome {
                Some(Outcome::Failed(problem)) => Some(problem),
                _ => None,
            },
            notes: self.notes.clone(),
        }
    }

    /// The notes already read, if any (so a check does not read the same
    /// release's body again).
    pub fn cached_notes(&self) -> Option<&Notes> {
        self.notes.as_ref()
    }

    /// Keep the notes the update page shows; `None` forgets them.
    pub fn set_notes(&mut self, notes: Option<Notes>) {
        self.notes = notes;
    }

    /// The release notes to link to: the release under way, else this version's.
    pub fn notes(&self) -> String {
        match &self.state {
            State::Downloading { release, .. } | State::Ready { release } | State::Installing { release } | State::Restarting { release } => {
                release.notes.clone()
            }
            State::Idle | State::Checking => release_page(&self.current),
        }
    }

    /// What the menu item says.
    pub fn menu(&self) -> Menu {
        match self.place {
            Place::Updatable => {}
            Place::NotABundle => return Menu::Unavailable,
            Place::Translocated | Place::ReadOnly | Place::Symlinked => return Menu::MoveToApplications,
        }
        match (&self.state, self.outcome) {
            (State::Checking, _) => Menu::Checking,
            (State::Downloading { percent, .. }, _) => Menu::Downloading(*percent),
            (State::Ready { release }, _) => Menu::RestartToUpdate(release.version.clone()),
            (State::Installing { .. }, _) => Menu::Installing,
            (State::Restarting { .. }, _) => Menu::Restarting,
            (State::Idle, Some(Outcome::UpToDate)) => Menu::UpToDate(self.current.clone()),
            (State::Idle, Some(Outcome::Failed(problem))) => Menu::Failed(problem),
            (State::Idle, None) => Menu::Check,
        }
    }

    fn say(&mut self, outcome: Option<Outcome>, asked: bool, now: SystemTime) {
        self.outcome = outcome;
        // An asked-for answer goes after a while; a failure worth saying unasked stays.
        self.outcome_until = match outcome {
            Some(Outcome::Failed(problem)) if problem.worth_saying_unasked() => None,
            Some(_) if asked => Some(now + ANSWER_SHOWN),
            _ => None,
        };
    }

    /// Time passes: whether what the menu says changed (an answer left it).
    pub fn tick(&mut self, now: SystemTime) -> bool {
        match self.outcome_until {
            Some(until) if now >= until => {
                self.outcome = None;
                self.outcome_until = None;
                true
            }
            _ => false,
        }
    }

    /// A check is wanted (`asked`: the person chose it). Whether to start
    /// one: not where the app cannot be updated, not while one runs (an
    /// asked-for one then joins it, so its answer shows), not during a
    /// download or an install. A downloaded update does not stop a check: a
    /// newer release replaces it.
    pub fn begin_check(&mut self, asked: bool) -> bool {
        if self.place != Place::Updatable {
            return false;
        }
        match self.state {
            State::Idle | State::Ready { .. } => {
                self.before = std::mem::replace(&mut self.state, State::Checking);
                self.asked = asked;
                true
            }
            State::Checking => {
                self.asked |= asked;
                false
            }
            State::Downloading { .. } | State::Installing { .. } | State::Restarting { .. } => false,
        }
    }

    /// The check finished at `at`.
    pub fn end_check(&mut self, checked: Checked, at: SystemTime) -> Next {
        if self.state != State::Checking {
            return Next::Nothing;
        }
        let asked = self.asked;
        let before = std::mem::replace(&mut self.before, State::Idle);
        self.last_checked = Some(at);
        match checked {
            Checked::Found(release) => {
                if matches!(&before, State::Ready { release: ready } if *ready == release) {
                    // Already downloaded and waiting.
                    self.state = before;
                    self.say(None, asked, at);
                    return Next::Nothing;
                }
                self.state = State::Downloading {
                    release: release.clone(),
                    percent: None,
                };
                self.say(None, asked, at);
                Next::Download(release)
            }
            Checked::UpToDate => {
                self.state = State::Idle;
                self.asked = false;
                self.say(asked.then_some(Outcome::UpToDate), asked, at);
                // A downloaded release that is no longer the newest is not installed.
                if matches!(before, State::Ready { .. }) {
                    Next::Discard
                } else {
                    Next::Nothing
                }
            }
            Checked::Failed(problem) => {
                // A downloaded update is still there to install.
                self.state = before;
                self.asked = false;
                if asked || problem.worth_saying_unasked() {
                    self.say(Some(Outcome::Failed(problem)), asked, at);
                }
                Next::Nothing
            }
        }
    }

    /// The download is put off (a metered network): back to how it was.
    pub fn postpone(&mut self) {
        if let State::Downloading { .. } = self.state {
            self.state = State::Idle;
            self.asked = false;
        }
    }

    /// `received` bytes of `total` are in. Whether what the menu says changed
    /// (whole percents only, so the menu is not redrawn for every chunk).
    pub fn progress(&mut self, received: u64, total: Option<u64>) -> bool {
        let State::Downloading { percent, .. } = &mut self.state else {
            return false;
        };
        let now = total.filter(|total| *total > 0).map(|total| (received.min(total) * 100 / total) as u8);
        let changed = *percent != now;
        *percent = now;
        changed
    }

    /// Downloaded, verified and put aside: ready.
    pub fn downloaded(&mut self, at: SystemTime) {
        if let State::Downloading { release, .. } = &self.state {
            self.state = State::Ready { release: release.clone() };
            self.asked = false;
            self.say(None, false, at);
        }
    }

    /// The download failed, or did not verify: nothing is kept.
    pub fn download_failed(&mut self, problem: Problem, at: SystemTime) {
        if let State::Downloading { .. } = self.state {
            self.state = State::Idle;
            let asked = std::mem::take(&mut self.asked);
            if asked || problem.worth_saying_unasked() {
                self.say(Some(Outcome::Failed(problem)), asked, at);
            }
        }
    }

    /// The person chose to restart into the downloaded update: whether there
    /// is one (anything asking for a restart without one is ignored).
    pub fn restart_wanted(&self) -> bool {
        self.ready().is_some()
    }

    /// On the way out: the release to install, if one is ready.
    pub fn begin_install(&mut self) -> Option<Release> {
        let release = self.ready()?.clone();
        self.state = State::Installing { release: release.clone() };
        Some(release)
    }

    /// Installed.
    pub fn installed(&mut self) {
        if let State::Installing { release } = &self.state {
            self.state = State::Restarting { release: release.clone() };
            self.outcome = None;
        }
    }

    /// The install failed; the app is as it was, and the download is thrown away.
    pub fn install_failed(&mut self, problem: Problem, at: SystemTime) {
        if let State::Installing { .. } = self.state {
            self.state = State::Idle;
            self.say(Some(Outcome::Failed(problem)), false, at);
        }
    }
}

/// When the app checks on its own: [`FIRST_CHECK`] after launch, then every
/// [`CHECK_EVERY`] of wall-clock time since the last check started (time
/// asleep counts), [`RETRY_AFTER`] after one that failed or a download put
/// off, and at once if the clock went back past the last one.
#[derive(Debug, Clone)]
pub struct Schedule {
    last: Option<SystemTime>,
    wait: Duration,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            last: None,
            wait: CHECK_EVERY,
        }
    }
}

impl Schedule {
    /// Whether a check is due at `now`, the app having run for `running_for`.
    pub fn due(&self, now: SystemTime, running_for: Duration) -> bool {
        match self.last {
            None => running_for >= FIRST_CHECK,
            Some(last) => now.duration_since(last).map_or(true, |since| since >= self.wait),
        }
    }

    /// A check (the app's or an asked-for one) started at `at`.
    pub fn started(&mut self, at: SystemTime) {
        self.last = Some(at);
        self.wait = CHECK_EVERY;
    }

    /// The check that started last failed, or its download was put off: try
    /// again sooner.
    pub fn failed(&mut self) {
        self.wait = RETRY_AFTER;
    }
}

// ---------------------------------------------------------------------------
// The manifest
// ---------------------------------------------------------------------------

/// `latest.json`: the Tauri updater's static manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The release's version (no `v`).
    pub version: String,
    /// The release notes' address.
    pub notes: String,
    /// When it was published (RFC 3339, UTC).
    pub pub_date: String,
    /// The archive per updater target ([`TARGETS`]).
    pub platforms: BTreeMap<String, Platform>,
}

/// One target's archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Platform {
    /// The contents of the archive's `.sig` file (base64, as Tauri writes it).
    pub signature: String,
    /// Where the archive is.
    pub url: String,
}

/// What [`Manifest::check`] holds a manifest to.
#[derive(Debug, Clone, Copy)]
pub struct Rules<'a> {
    /// The updater public key (base64, as in `tauri.conf.json`).
    pub pubkey: &'a str,
    /// The archive, to verify the signature against; without it only the
    /// signature's form and fields are checked.
    pub archive: Option<&'a [u8]>,
    /// Allow `http://127.0.0.1` / `http://localhost` archive addresses — for
    /// the updater's end-to-end test only, never a release.
    pub loopback_http: bool,
}

impl Manifest {
    /// The manifest of `version`'s archive at `url`, signed by `signature`
    /// (the `.sig` file's contents), published at `published`.
    pub fn new(version: &str, notes: &str, url: &str, signature: &str, published: SystemTime) -> Self {
        let platform = Platform {
            signature: signature.trim().to_string(),
            url: url.to_string(),
        };
        Self {
            version: version.to_string(),
            notes: notes.to_string(),
            pub_date: crate::logging::timestamp(published),
            platforms: TARGETS.iter().map(|target| (target.to_string(), platform.clone())).collect(),
        }
    }

    /// Parse `latest.json`.
    pub fn parse(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|error| format!("not an update manifest: {error}"))
    }

    /// The file's contents.
    pub fn to_json(&self) -> String {
        let mut json = serde_json::to_string_pretty(self).expect("a manifest serialises");
        json.push('\n');
        json
    }

    /// Everything wrong with it; empty when it is fit to publish.
    pub fn check(&self, rules: &Rules<'_>) -> Vec<String> {
        let mut problems = Vec::new();
        if !is_version(&self.version) {
            problems.push(format!("version {:?} is not a plain semantic version (no \"v\")", self.version));
        }
        if !is_rfc3339(&self.pub_date) {
            problems.push(format!("pub_date {:?} is not YYYY-MM-DDTHH:MM:SSZ", self.pub_date));
        }
        if allowed_notes(&self.notes).as_deref() != Some(self.notes.as_str()) {
            problems.push(format!(
                "notes {:?} is not an https page of github.com/{RELEASES_REPO}/releases or droppen.app (the app would link to the release page instead)",
                self.notes
            ));
        }
        let targets: Vec<&str> = self.platforms.keys().map(String::as_str).collect();
        if targets != TARGETS {
            problems.push(format!("platforms are {targets:?}, not {TARGETS:?}"));
        }
        let mut distinct: Vec<&Platform> = self.platforms.values().collect();
        distinct.dedup();
        if distinct.len() > 1 {
            problems.push("the platforms name different archives; a release has one universal archive".to_string());
        }
        if let Some(platform) = distinct.first() {
            if let Err(why) = check_url(&platform.url, &self.version, rules.loopback_http) {
                problems.push(why);
            }
            if let Err(why) = check_signature(&platform.signature, rules.pubkey, &self.version, rules.archive) {
                problems.push(why);
            }
        }
        problems
    }
}

/// Whether the app may download `version`'s archive from `url`, which the
/// unsigned manifest named: only from that version's release (or, in the
/// updater's end-to-end test, a loopback `http` server). The signature still
/// decides what is installed; this keeps the download itself where releases
/// are, so a changed manifest cannot send every app to another host.
pub fn check_download_url(url: &str, version: &str, loopback_http: bool) -> Result<(), String> {
    check_url(url, version, loopback_http)
}

fn check_url(url: &str, version: &str, loopback_http: bool) -> Result<(), String> {
    let parsed = Url::parse(url).map_err(|error| format!("url {url:?}: {error}"))?;
    if !parsed.path().ends_with(&format!("/{ARCHIVE}")) {
        return Err(format!("url {url:?} does not end in /{ARCHIVE}"));
    }
    match parsed.scheme() {
        "https" if url == archive_url(version) => Ok(()),
        "https" => Err(format!(
            "url {url:?} is not this version's archive on the release, {}",
            archive_url(version)
        )),
        "http" if loopback_http && matches!(parsed.host_str(), Some("127.0.0.1" | "localhost")) => Ok(()),
        _ => Err(format!("url {url:?} is not https")),
    }
}

/// The minisign key id of a public key (base64, as in `tauri.conf.json`), as
/// minisign and `tauri signer` print it: `1CB79070988604BF`.
pub fn key_id(pubkey: &str) -> Result<String, String> {
    let text = decode_text(pubkey, "public key")?;
    minisign_verify::PublicKey::decode(&text).map_err(|error| format!("the public key is not a minisign key: {error}"))?;
    embedded_key_id(&text, 42)
}

/// The key id in a minisign key or signature file's text: its second line is
/// base64 of two algorithm bytes, the key id (8 bytes, little-endian) and the
/// key or signature itself, `len` bytes in all.
fn embedded_key_id(text: &str, len: usize) -> Result<String, String> {
    let bytes = text
        .lines()
        .nth(1)
        .and_then(|line| base64::engine::general_purpose::STANDARD.decode(line.trim()).ok())
        .filter(|bytes| bytes.len() == len)
        .ok_or("no minisign key id in it")?;
    let id: [u8; 8] = bytes[2..10].try_into().expect("8 bytes");
    Ok(format!("{:016X}", u64::from_le_bytes(id)))
}

fn decode_text(base64: &str, what: &str) -> Result<String, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64.trim())
        .map_err(|error| format!("the {what} is not base64: {error}"))?;
    String::from_utf8(bytes).map_err(|_| format!("the {what} is not text"))
}

/// Check `signature` (a `.sig` file's contents) against `pubkey` (base64, as
/// in `tauri.conf.json`): made by that key over `archive` (when given), for
/// `version` and for a file named [`ARCHIVE`] — what the app itself checks,
/// plus the file name.
pub fn check_signature(signature: &str, pubkey: &str, version: &str, archive: Option<&[u8]>) -> Result<(), String> {
    let key_text = decode_text(pubkey, "public key")?;
    let key = minisign_verify::PublicKey::decode(&key_text).map_err(|error| format!("the public key is not a minisign key: {error}"))?;
    let signature_text = decode_text(signature, "signature")?;
    let decoded =
        minisign_verify::Signature::decode(&signature_text).map_err(|error| format!("the signature is not a minisign signature: {error}"))?;
    // Named first, so a manifest signed with a replaced key says so even
    // without the archive.
    let (signed_by, trusted) = (embedded_key_id(&signature_text, 74)?, embedded_key_id(&key_text, 42)?);
    if signed_by != trusted {
        return Err(format!(
            "the signature does not verify with this key: it is by key {signed_by}, not {trusted}"
        ));
    }
    if let Some(archive) = archive {
        key.verify(archive, &decoded, true)
            .map_err(|error| format!("the signature does not verify the archive with this key: {error}"))?;
    }
    let comment = decoded.trusted_comment();
    let field = |name: &str| comment.split('\t').find_map(|part| part.strip_prefix(name).map(str::to_string));
    match field("version:") {
        Some(signed) if signed == version => {}
        Some(signed) => return Err(format!("the signature was made for version {signed}, not {version}")),
        None => return Err("the signature names no version (sign with --app-version)".to_string()),
    }
    match field("file:") {
        Some(file) if file == ARCHIVE => Ok(()),
        other => Err(format!("the signature was made for {other:?}, not {ARCHIVE}")),
    }
}

fn is_rfc3339(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 20
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            4 | 7 => *byte == b'-',
            10 => *byte == b'T',
            13 | 16 => *byte == b':',
            19 => *byte == b'Z',
            _ => byte.is_ascii_digit(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn release(version: &str) -> Release {
        Release {
            version: version.to_string(),
            notes: release_page(version),
        }
    }

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn updatable(current: &str) -> Updates {
        Updates::new(current, Place::Updatable)
    }

    #[test]
    fn a_check_of_its_own_downloads_quietly_and_waits_for_the_quit() {
        let mut updates = updatable("0.2.124");
        assert_eq!(updates.menu(), Menu::Check);
        assert!(updates.begin_check(false));
        assert_eq!(updates.menu(), Menu::Checking);
        assert!(!updates.menu().enabled());
        assert_eq!(
            updates.end_check(Checked::Found(release("0.2.125")), at(1)),
            Next::Download(release("0.2.125"))
        );
        assert_eq!(updates.menu(), Menu::Downloading(None));
        assert!(!updates.begin_check(true), "no check during a download");
        assert!(!updates.restart_wanted(), "nothing to restart into yet");
        assert!(updates.progress(4_200, Some(10_000)));
        assert!(!updates.progress(4_250, Some(10_000)), "the same whole percent: no redraw");
        assert_eq!(updates.menu(), Menu::Downloading(Some(42)));
        assert!(!updates.menu().enabled());
        updates.downloaded(at(2));
        assert_eq!(updates.menu(), Menu::RestartToUpdate("0.2.125".into()));
        assert!(updates.menu().enabled());
        assert!(updates.restart_wanted());
        assert_eq!(updates.notes(), release_page("0.2.125"));
        // At the quit (or a restart): installed.
        assert_eq!(updates.begin_install(), Some(release("0.2.125")));
        assert_eq!(updates.menu(), Menu::Installing);
        updates.installed();
        assert_eq!(updates.menu(), Menu::Restarting);
        assert_eq!(updates.begin_install(), None);
    }

    #[test]
    fn an_asked_for_check_answers_in_the_menu_for_a_while() {
        let mut updates = updatable("0.2.124");
        assert_eq!(updates.notes(), release_page("0.2.124"), "this version's notes");
        assert!(updates.begin_check(true));
        assert_eq!(updates.end_check(Checked::UpToDate, at(100)), Next::Nothing);
        assert_eq!(updates.menu(), Menu::UpToDate("0.2.124".into()));
        assert!(updates.menu().enabled(), "choosing it checks again");
        assert!(!updates.tick(at(100) + ANSWER_SHOWN - Duration::from_secs(1)));
        assert!(updates.tick(at(100) + ANSWER_SHOWN));
        assert_eq!(updates.menu(), Menu::Check);

        assert!(updates.begin_check(true));
        updates.end_check(Checked::Failed(Problem::Offline), at(200));
        assert_eq!(updates.menu(), Menu::Failed(Problem::Offline));
        assert_eq!(updates.status().problem, Some(Problem::Offline));
        assert!(updates.tick(at(200) + ANSWER_SHOWN));
        assert_eq!(updates.menu(), Menu::Check);
    }

    #[test]
    fn a_check_of_its_own_says_nothing_of_the_network_or_the_service() {
        let mut updates = updatable("0.2.124");
        for problem in [Problem::Offline, Problem::Service, Problem::Other] {
            assert!(updates.begin_check(false));
            updates.end_check(Checked::Failed(problem), at(1));
            assert_eq!(updates.menu(), Menu::Check, "{problem:?}: retried later, silently");
        }
        assert!(updates.begin_check(false));
        updates.end_check(Checked::UpToDate, at(2));
        assert_eq!(updates.menu(), Menu::Check, "up to date, unasked: nothing to say");
    }

    #[test]
    fn a_download_that_does_not_verify_is_thrown_away_and_said() {
        let mut updates = updatable("0.2.124");
        assert!(updates.begin_check(false));
        updates.end_check(Checked::Found(release("0.2.125")), at(1));
        updates.download_failed(Problem::Signature, at(2));
        assert_eq!(updates.menu(), Menu::Failed(Problem::Signature));
        assert!(!updates.tick(at(2) + ANSWER_SHOWN * 10), "said until something succeeds");
        assert!(!updates.restart_wanted());
        // A dropped download, unasked, is just tried again.
        assert!(updates.begin_check(false));
        updates.end_check(Checked::Found(release("0.2.125")), at(3));
        assert_eq!(updates.status().problem, None, "a new attempt clears it");
        updates.download_failed(Problem::Offline, at(4));
        assert_eq!(updates.menu(), Menu::Check);
        // Put off on a metered network: back to idle.
        assert!(updates.begin_check(false));
        updates.end_check(Checked::Found(release("0.2.125")), at(5));
        updates.postpone();
        assert_eq!(updates.menu(), Menu::Check);
    }

    #[test]
    fn a_failed_install_leaves_the_app_as_it_was_and_says_so() {
        let mut updates = updatable("0.2.124");
        assert!(updates.begin_check(false));
        updates.end_check(Checked::Found(release("0.2.125")), at(1));
        updates.downloaded(at(2));
        updates.begin_install();
        updates.install_failed(Problem::Location, at(3));
        assert_eq!(updates.menu(), Menu::Failed(Problem::Location));
        assert!(!updates.restart_wanted());
    }

    #[test]
    fn a_newer_release_replaces_the_one_waiting() {
        let mut updates = updatable("0.2.124");
        assert!(updates.begin_check(false));
        updates.end_check(Checked::Found(release("0.2.125")), at(1));
        updates.downloaded(at(2));
        // Six hours on: the same release, already here.
        assert!(updates.begin_check(false));
        assert_eq!(updates.end_check(Checked::Found(release("0.2.125")), at(3)), Next::Nothing);
        assert_eq!(updates.menu(), Menu::RestartToUpdate("0.2.125".into()));
        // A failed check keeps it.
        assert!(updates.begin_check(true));
        updates.end_check(Checked::Failed(Problem::Offline), at(4));
        assert_eq!(updates.menu(), Menu::RestartToUpdate("0.2.125".into()));
        // A newer one is downloaded instead.
        assert!(updates.begin_check(false));
        assert_eq!(
            updates.end_check(Checked::Found(release("0.2.126")), at(5)),
            Next::Download(release("0.2.126"))
        );
        updates.downloaded(at(6));
        assert_eq!(updates.menu(), Menu::RestartToUpdate("0.2.126".into()));
        // Withdrawn (the newest is no newer than this app): thrown away.
        assert!(updates.begin_check(false));
        assert_eq!(updates.end_check(Checked::UpToDate, at(7)), Next::Discard);
        assert_eq!(updates.menu(), Menu::Check);
    }

    #[test]
    fn asking_during_a_check_joins_it_and_gets_the_answer() {
        let mut updates = updatable("0.2.124");
        assert!(updates.begin_check(false));
        assert!(!updates.begin_check(true), "one check at a time");
        updates.end_check(Checked::UpToDate, at(1));
        assert_eq!(updates.menu(), Menu::UpToDate("0.2.124".into()), "the person who asked hears back");
        assert_eq!(
            updates.end_check(Checked::UpToDate, at(2)),
            Next::Nothing,
            "a result for no check is ignored"
        );
    }

    #[test]
    fn where_the_app_cannot_be_updated_the_menu_says_so_and_nothing_runs() {
        let app = std::path::Path::new("/Applications/Droppen.app/Contents/MacOS/Droppen");
        assert_eq!(Place::of(app, Some(app), false), Place::Updatable);
        let home = std::path::Path::new("/Users/me/Applications/Droppen.app/Contents/MacOS/Droppen");
        assert_eq!(Place::of(home, Some(home), false), Place::Updatable, "any writable place works");
        assert_eq!(Place::of(app, Some(app), true), Place::ReadOnly);
        let dmg = std::path::Path::new("/Volumes/Droppen/Droppen.app/Contents/MacOS/Droppen");
        assert_eq!(Place::of(dmg, Some(dmg), true), Place::ReadOnly, "run straight from the disk image");
        let linked = std::path::Path::new("/tmp/x/Droppen.app/Contents/MacOS/Droppen");
        let real = std::path::Path::new("/private/tmp/x/Droppen.app/Contents/MacOS/Droppen");
        assert_eq!(Place::of(linked, Some(real), false), Place::Symlinked);
        assert_eq!(Place::of(linked, None, false), Place::Symlinked, "unresolvable: not trusted either");
        let moved = std::path::Path::new("/private/var/folders/x/T/AppTranslocation/1234/d/Droppen.app/Contents/MacOS/Droppen");
        assert_eq!(Place::of(moved, Some(moved), true), Place::Translocated);
        let dev = std::path::Path::new("/w/desktop/target/debug/droppen-desktop");
        assert_eq!(Place::of(dev, Some(dev), false), Place::NotABundle);

        for place in [Place::Translocated, Place::ReadOnly, Place::Symlinked] {
            let mut updates = Updates::new("0.2.124", place);
            assert_eq!(updates.menu(), Menu::MoveToApplications);
            assert!(!updates.menu().enabled());
            assert!(!updates.begin_check(true), "{place:?}: no check");
        }
        let mut dev = Updates::new("0.1.0", Place::NotABundle);
        assert_eq!(dev.menu(), Menu::Unavailable);
        assert!(!dev.menu().enabled());
        assert!(!dev.begin_check(false));
    }

    #[test]
    fn the_status_serialises_for_the_web_app() {
        let mut updates = updatable("0.2.124");
        assert!(updates.begin_check(false));
        updates.end_check(Checked::Found(release("0.2.125")), at(1_792_396_800));
        updates.downloaded(at(1_792_396_900));
        let json = serde_json::to_value(updates.status()).expect("json");
        assert_eq!(
            json,
            serde_json::json!({
                "current": "0.2.124",
                "state": "ready",
                "release": { "version": "0.2.125", "notes": "https://github.com/getdroppen/droppen/releases/tag/v0.2.125" },
                "place": "updatable",
                "lastChecked": "2026-10-19T08:00:00Z",
                "problem": null,
                "notes": null,
            })
        );
        let mut failed = updatable("0.2.124");
        failed.begin_check(true);
        failed.end_check(Checked::Failed(Problem::Signature), at(1));
        assert_eq!(serde_json::to_value(failed.status()).expect("json")["problem"], "signature");
        assert_eq!(
            serde_json::to_value(Updates::new("1.0.0", Place::Symlinked).status()).expect("json")["place"],
            "symlinked"
        );
    }

    #[test]
    fn the_release_body_gives_the_update_page_its_two_languages() {
        let body = "Droppen v0.2.131: the server.\n\n## What's new in v0.2.131\n\n### Your account\n\n- **One.** Two.\n\n### Coming next\n\n- Later.\n\n## v0.2.131 更新内容\n\n### 你的账户\n\n- **一。** 二。\n";
        let notes = whats_new(body, "0.2.131").expect("notes");
        assert_eq!(notes.version, "0.2.131");
        assert!(notes.english.starts_with("### Your account"), "{}", notes.english);
        assert!(notes.english.contains("### Coming next"));
        assert!(!notes.english.contains("更新内容"), "the next ## ends it");
        assert!(notes.chinese.starts_with("### 你的账户"));
        assert!(notes.chinese.contains("二。"));
        // The body as GitHub's release JSON carries it.
        let json = serde_json::json!({ "tag_name": "v0.2.131", "body": body }).to_string();
        assert_eq!(notes_from_json(&json, "0.2.131"), Some(notes));
        assert!(notes_from_json("{}", "0.2.131").is_none(), "no body");
        assert!(notes_from_json("not json", "0.2.131").is_none());
        assert!(whats_new("## Droppen App for macOS\n\nnothing here", "0.2.131").is_none(), "a snapshot release");
    }

    #[test]
    fn notes_are_read_from_the_release_of_that_version_only() {
        assert_eq!(
            notes_api_url("0.2.131").as_deref(),
            Some("https://api.github.com/repos/getdroppen/droppen/releases/tags/v0.2.131")
        );
        assert!(notes_api_url("0.3.0-rc.1").is_some());
        for wrong in ["", "v0.2.131", "0.2", "0.2.131/../../x", "0.2.131?x=1"] {
            assert_eq!(notes_api_url(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn the_status_carries_the_notes_it_has_read() {
        let mut updates = updatable("0.2.124");
        updates.set_notes(Some(Notes {
            version: "0.2.124".into(),
            english: "## a".into(),
            chinese: "## 甲".into(),
        }));
        let json = serde_json::to_value(updates.status()).expect("json");
        assert_eq!(json["notes"]["version"], "0.2.124");
        assert_eq!(json["notes"]["english"], "## a");
        assert_eq!(json["notes"]["chinese"], "## 甲");
        updates.set_notes(None);
        assert_eq!(serde_json::to_value(updates.status()).expect("json")["notes"], serde_json::Value::Null);
    }

    #[test]
    fn the_schedule_checks_after_launch_then_every_six_hours_of_wall_clock() {
        let mut schedule = Schedule::default();
        let start = at(1_000_000);
        assert!(!schedule.due(start, Duration::from_secs(5)), "not during the start");
        assert!(schedule.due(start, FIRST_CHECK));
        schedule.started(start);
        assert!(!schedule.due(start + Duration::from_secs(60), Duration::from_secs(60)));
        assert!(!schedule.due(start + CHECK_EVERY - Duration::from_secs(1), Duration::from_secs(1)));
        // Measured by the wall clock: a night asleep counts.
        assert!(schedule.due(start + CHECK_EVERY, Duration::from_secs(1)));
        // A failed check is tried again sooner.
        schedule.started(start);
        schedule.failed();
        assert!(!schedule.due(start + RETRY_AFTER - Duration::from_secs(1), Duration::ZERO));
        assert!(schedule.due(start + RETRY_AFTER, Duration::ZERO));
        // A clock set back past the last check does not stop checks.
        schedule.started(start);
        assert!(schedule.due(start - Duration::from_secs(1), Duration::ZERO));
    }

    #[test]
    fn release_notes_are_linked_only_to_the_release_or_droppen_app() {
        let page = "https://github.com/getdroppen/droppen/releases/tag/v0.2.125";
        assert_eq!(release_page("0.2.125"), page);
        assert_eq!(notes_link(Some(page), "0.2.125"), page);
        assert_eq!(
            notes_link(Some("https://droppen.app/changelog#0.2.125"), "0.2.125"),
            "https://droppen.app/changelog#0.2.125"
        );
        for hostile in [
            "http://github.com/getdroppen/droppen/releases/tag/v0.2.125",
            "https://github.com/someone-else/releases/tag/v0.2.125",
            "https://github.com.evil.example/getdroppen/droppen/releases/",
            "https://github.com/getdroppen/droppen/blob/main/README.md",
            "https://github.com/getdroppen/droppen/issues/1",
            "https://github.com/getdroppen/releases/releases/tag/v0.2.125",
            "https://user@droppen.app/",
            "https://droppen.app:8443/",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "Bug fixes and improvements.",
            "",
        ] {
            assert_eq!(notes_link(Some(hostile), "0.2.125"), page, "{hostile}");
        }
        assert_eq!(notes_link(None, "0.2.125"), page);
    }

    #[test]
    fn versions_are_plain_semver() {
        for good in ["0.2.123", "10.0.1", "0.0.1-test", "1.2.3-rc.1"] {
            assert!(is_version(good), "{good}");
        }
        for bad in [
            "v0.2.123",
            "0.2",
            "0.2.123.4",
            "0.2.x",
            "0.2.123+g1",
            "0.2.123-",
            "0.2.123-a..b",
            " 0.2.123",
            "",
        ] {
            assert!(!is_version(bad), "{bad}");
        }
    }

    #[test]
    fn dates_parse_as_rfc3339_utc() {
        // The format itself is pinned by `logging`'s own tests; here it is the
        // check a manifest's `pub_date` goes through.
        assert!(is_rfc3339("2026-10-19T09:02:03Z"));
        assert!(!is_rfc3339("2026-10-19 09:02:03Z"));
        assert!(!is_rfc3339("2026-10-19T09:02:03+00:00"));
    }

    /// The app's own configuration names the endpoint and a public key.
    #[test]
    fn the_app_config_points_at_the_latest_release_manifest() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../../src-tauri/tauri.conf.json")).expect("tauri.conf.json is json");
        let updater = &conf["plugins"]["updater"];
        assert_eq!(updater["endpoints"], serde_json::json!([ENDPOINT]));
        assert_eq!(updater["requireSignedVersion"], true);
        assert!(updater["dangerousInsecureTransportProtocol"].is_null(), "never in the release config");
        let pubkey = updater["pubkey"].as_str().expect("a pubkey");
        let key = decode_text(pubkey, "public key").expect("base64 text");
        minisign_verify::PublicKey::decode(&key).expect("a minisign public key");
        assert_eq!(
            key_id(pubkey).as_deref(),
            Ok(RELEASE_KEY_ID),
            "the updater key is the release key (desktop/SIGNING.md, \"The key\"); replacing it strands every installed app"
        );
        assert!(
            conf["bundle"]["createUpdaterArtifacts"].is_null(),
            "the archive is made from the stapled app, after the build"
        );
    }

    /// A key pair and a signature made by `tauri signer` (a throwaway key,
    /// generated for this test and not kept anywhere else).
    mod fixture {
        pub const PUBKEY: &str = include_str!("../testdata/update/test.key.pub");
        pub const ARCHIVE: &[u8] = include_bytes!("../testdata/update/Droppen.app.tar.gz");
        pub const SIGNATURE: &str = include_str!("../testdata/update/Droppen.app.tar.gz.sig");
        pub const OTHER_SIGNATURE: &str = include_str!("../testdata/update/other-key.sig");
        pub const OTHER_FILE_SIGNATURE: &str = include_str!("../testdata/update/other-file.sig");
        // The same archive, signed by desktop/update-signer (the release's
        // signer) with a test key of its own, for VERSION.
        pub const IN_REPO_SIGNATURE: &str = include_str!("../testdata/update/in-repo-signer.sig");
        pub const IN_REPO_PUBKEY: &str = include_str!("../testdata/update/in-repo-signer.key.pub");
        pub const VERSION: &str = "0.0.2-test";
    }

    fn rules(archive: Option<&'static [u8]>) -> Rules<'static> {
        Rules {
            pubkey: fixture::PUBKEY,
            archive,
            loopback_http: false,
        }
    }

    fn manifest() -> Manifest {
        Manifest::new(
            fixture::VERSION,
            &release_page(fixture::VERSION),
            &archive_url(fixture::VERSION),
            fixture::SIGNATURE,
            at(1_792_396_800),
        )
    }

    #[test]
    fn a_manifest_names_one_universal_archive_for_both_targets() {
        let manifest = manifest();
        assert_eq!(manifest.check(&rules(Some(fixture::ARCHIVE))), Vec::<String>::new());
        let json: serde_json::Value = serde_json::from_str(&manifest.to_json()).expect("json");
        let url = "https://github.com/getdroppen/droppen/releases/download/v0.0.2-test/Droppen.app.tar.gz";
        assert_eq!(json["version"], "0.0.2-test");
        assert_eq!(json["notes"], "https://github.com/getdroppen/droppen/releases/tag/v0.0.2-test");
        assert_eq!(json["pub_date"], "2026-10-19T08:00:00Z");
        for target in TARGETS {
            assert_eq!(json["platforms"][target]["url"], url);
            assert_eq!(json["platforms"][target]["signature"], fixture::SIGNATURE.trim());
        }
        assert_eq!(json["platforms"].as_object().expect("platforms").len(), 2);
        assert_eq!(Manifest::parse(&manifest.to_json()), Ok(manifest));
    }

    #[test]
    fn a_signature_by_another_key_or_over_other_bytes_is_refused() {
        let mut other_key = manifest();
        for platform in other_key.platforms.values_mut() {
            platform.signature = fixture::OTHER_SIGNATURE.trim().to_string();
        }
        let problems = other_key.check(&rules(Some(fixture::ARCHIVE)));
        assert!(problems.iter().any(|p| p.contains("does not verify")), "{problems:?}");
        // Named by key id, with or without the archive.
        let problems = other_key.check(&rules(None));
        assert!(
            problems.iter().any(|p| p.contains("is by key") && p.contains("not BC638533C21C1301")),
            "{problems:?}"
        );

        let mut tampered = fixture::ARCHIVE.to_vec();
        tampered[100] ^= 1;
        let problems = manifest().check(&Rules {
            archive: Some(Box::leak(tampered.into_boxed_slice())),
            ..rules(None)
        });
        assert!(problems.iter().any(|p| p.contains("does not verify")), "{problems:?}");

        // Signed for another file name: verifies, but is not this archive's.
        assert!(check_signature(fixture::OTHER_FILE_SIGNATURE, fixture::PUBKEY, fixture::VERSION, None)
            .unwrap_err()
            .contains("not Droppen.app.tar.gz"));
    }

    /// desktop/update-signer signs as `tauri signer sign --app-version` does:
    /// the app's checks take its signature, version and file name included.
    #[test]
    fn the_release_signer_signs_what_the_app_checks() {
        assert_eq!(
            check_signature(fixture::IN_REPO_SIGNATURE, fixture::IN_REPO_PUBKEY, fixture::VERSION, Some(fixture::ARCHIVE)),
            Ok(())
        );
        assert!(check_signature(fixture::IN_REPO_SIGNATURE, fixture::IN_REPO_PUBKEY, "0.0.3-test", Some(fixture::ARCHIVE))
            .unwrap_err()
            .contains("made for version 0.0.2-test"));
        assert!(check_signature(fixture::IN_REPO_SIGNATURE, fixture::PUBKEY, fixture::VERSION, Some(fixture::ARCHIVE)).is_err());
        let mut tampered = fixture::ARCHIVE.to_vec();
        tampered[100] ^= 1;
        assert!(check_signature(fixture::IN_REPO_SIGNATURE, fixture::IN_REPO_PUBKEY, fixture::VERSION, Some(&tampered)).is_err());
    }

    #[test]
    fn a_manifest_must_announce_the_version_the_archive_was_signed_for() {
        let mut older = manifest();
        older.version = "0.0.3-test".to_string();
        for platform in older.platforms.values_mut() {
            platform.url = archive_url("0.0.3-test");
        }
        older.notes = release_page("0.0.3-test");
        let problems = older.check(&rules(Some(fixture::ARCHIVE)));
        assert_eq!(
            problems,
            vec!["the signature was made for version 0.0.2-test, not 0.0.3-test".to_string()]
        );
    }

    #[test]
    fn a_manifest_points_at_its_own_release_over_https() {
        let mut wrong_release = manifest();
        for platform in wrong_release.platforms.values_mut() {
            platform.url = archive_url("0.0.1-test");
        }
        assert!(wrong_release.check(&rules(None))[0].contains("not this version's archive"));

        let mut local = manifest();
        for platform in local.platforms.values_mut() {
            platform.url = "http://127.0.0.1:8123/v0.0.2-test/Droppen.app.tar.gz".to_string();
        }
        assert!(local.check(&rules(None))[0].contains("is not https"));
        assert_eq!(
            local.check(&Rules {
                loopback_http: true,
                ..rules(None)
            }),
            Vec::<String>::new(),
            "the end-to-end test's local server"
        );
        let mut remote_http = local.clone();
        for platform in remote_http.platforms.values_mut() {
            platform.url = "http://example.com/v0.0.2-test/Droppen.app.tar.gz".to_string();
        }
        assert!(!remote_http
            .check(&Rules {
                loopback_http: true,
                ..rules(None)
            })
            .is_empty());
    }

    #[test]
    fn the_app_downloads_only_from_the_versions_release() {
        assert_eq!(check_download_url(&archive_url("0.2.125"), "0.2.125", false), Ok(()));
        for wrong in [
            archive_url("0.2.124"),
            "https://example.com/v0.2.125/Droppen.app.tar.gz".to_string(),
            "http://github.com/getdroppen/droppen/releases/download/v0.2.125/Droppen.app.tar.gz".to_string(),
            // The repository releases came from up to v0.2.126.
            "https://github.com/getdroppen/releases/releases/download/v0.2.125/Droppen.app.tar.gz".to_string(),
            "http://127.0.0.1:8123/v0.2.125/Droppen.app.tar.gz".to_string(),
        ] {
            assert!(check_download_url(&wrong, "0.2.125", false).is_err(), "{wrong}");
        }
        assert_eq!(
            check_download_url("http://127.0.0.1:8123/v0.2.125/Droppen.app.tar.gz", "0.2.125", true),
            Ok(()),
            "the end-to-end test's local server"
        );
    }

    #[test]
    fn a_manifest_with_missing_or_mixed_targets_is_refused() {
        let mut one = manifest();
        one.platforms.remove("darwin-x86_64");
        assert!(one.check(&rules(None)).iter().any(|p| p.contains("platforms are")));

        let mut mixed = manifest();
        mixed.platforms.get_mut("darwin-x86_64").expect("x86").url = archive_url("0.0.1-test");
        assert!(mixed.check(&rules(None)).iter().any(|p| p.contains("one universal archive")));

        let mut extra = manifest();
        extra.platforms.insert("windows-x86_64".into(), extra.platforms["darwin-x86_64"].clone());
        assert!(extra.check(&rules(None)).iter().any(|p| p.contains("platforms are")));
    }

    #[test]
    fn a_manifest_with_bad_fields_lists_every_problem() {
        let mut bad = manifest();
        bad.version = "v0.0.2-test".into();
        bad.pub_date = "yesterday".into();
        bad.notes = "Bug fixes.".into();
        let problems = bad.check(&rules(None));
        assert!(problems.iter().any(|p| p.contains("plain semantic version")), "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("pub_date")), "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("notes")), "{problems:?}");
        assert!(Manifest::parse("{\"version\": 1}").is_err());
    }
}
