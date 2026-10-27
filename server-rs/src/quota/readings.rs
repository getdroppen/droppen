//! A Claude login's quota reading: where it comes from, where it lives, and who
//! may write it.
//!
//! ## Two sources
//!
//! * **Sessions, first.** claude writes a `rate_limit_event` before nearly every
//!   turn, and every one of them — `allowed` as much as `rejected` — carries
//!   `rate_limit_info.unifiedWindows`: the utilization and reset instant of each
//!   window the login has. That is a reading, made by the very credential being
//!   billed, and it costs nothing. [`windows_from_rate_limit_info`] turns it into
//!   [`QuotaWindow`]s and a session's driver hands them to
//!   [`super::QuotaCache::ingest_session`].
//! * **The `/usage` probe, as a fallback.** Only when a login's newest window is
//!   older than the fallback threshold — nothing has run under it lately — and no
//!   probe of it has failed recently. See [`SharedReading::fallback`].
//!
//! ## Why the probe is rationed this hard
//!
//! An account's probe runs against its *usage login*, a real `claude auth login`
//! whose refresh token rotates: every refresh hands out a new one and retires the
//! old. Two instances on one machine share those directories, and if each probes
//! on its own schedule, both can refresh one expired access token with the same
//! refresh token. The second use reads as token reuse and the whole login is
//! revoked. So:
//!
//! * only one process machine-wide probes a given login at a time: a
//!   non-blocking `flock` on `<usage dir>/.rh-probe.lock` ([`ProbeLock`]). The
//!   loser does not wait and does not retry; the winner's result lands in the
//!   shared file;
//! * the reading lives in a file every instance reads ([`UsageLogin::reading_file`]),
//!   so the instance that did not probe sees the result too, and neither probes a
//!   login the other has just read.
//!
//! ## The shared file
//!
//! `<data>/accounts/<id>/usage.json` for an account, `<data>/local-usage.json`
//! for the user's own `~/.claude`. Every window carries the instant it was read
//! and where it came from; merging a new reading replaces the windows it names and
//! keeps the others with their own, older, instants. Every write is a
//! read-modify-write under a blocking `flock` on `<file>.lock` and lands by
//! rename, so a reader — which takes no lock — never sees half a file.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gotcha::Schematic;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{window, QuotaProbeError, QuotaWindow, UsageProbe, RATE_LIMIT_BACKOFF};
use crate::accounts::rest::iso_ms;
use crate::accounts::store::{account_dir_in, usage_config_dir_in};
use crate::claude_roots::ClaudeRoot;
use crate::openapi_ext::{Nullable, Optional};
use crate::sessions::Paths;

/// How long after a failed probe — `not_logged_in` or `probe_failed` — the
/// fallback leaves that login alone.
///
/// A dead login does not come back by being asked again, and every ask is a CLI
/// startup against it; half an hour matches [`RATE_LIMIT_BACKOFF`], so every
/// failure costs the same two probes an hour.
pub const PROBE_FAILURE_BACKOFF: Duration = Duration::from_secs(30 * 60);
/// How old a login's newest window may get before the fallback probe runs, unless
/// `DROPPEN_USAGE_FALLBACK_MS` says otherwise.
pub const DEFAULT_FALLBACK_AFTER: Duration = Duration::from_secs(15 * 60);
/// How often one login's session readings may be written to its shared file.
///
/// claude sends a `rate_limit_event` per turn, per session; a few busy sessions on
/// one account would otherwise rewrite the file several times a second. The
/// newest reading is held in memory in between — [`super::QuotaCache::cached`]
/// shows it at once — and flushed by the next write or when the interval is up.
pub const SESSION_FLUSH_EVERY: Duration = Duration::from_secs(15);
/// The probe lock's file name inside an account's usage login.
pub const PROBE_LOCK_NAME: &str = ".rh-probe.lock";

/// Where an account's quota reading came from: the rate-limit events of a live session, or the `/usage` probe the server falls back to when no session has run for a while.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "snake_case")]
pub enum QuotaReadingSource {
    /// A `rate_limit_event` a running session wrote.
    Session,
    /// The fallback `claude -p /usage` probe.
    Probe,
}

/// Which login a reading belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UsageLogin {
    /// The user's own `~/.claude`.
    Local,
    /// A stored account, by uuid.
    Account(String),
}

/// Everything a probe of one login needs to know about the disk.
#[derive(Debug, Clone)]
pub struct ProbeTarget {
    /// `CLAUDE_CONFIG_DIR` for the probe; `None` for the user's own login.
    pub config_dir: Option<PathBuf>,
    /// The machine-wide probe lock.
    pub lock: PathBuf,
    /// The shared reading file the result is merged into.
    pub reading_file: PathBuf,
}

impl UsageLogin {
    /// The shared reading file; `None` for an account id that is not a uuid,
    /// which can therefore never name a directory of its own.
    pub fn reading_file(&self, paths: &Paths) -> Option<PathBuf> {
        match self {
            UsageLogin::Local => Some(paths.data_dir().join("local-usage.json")),
            UsageLogin::Account(id) => account_dir_in(paths, id).ok().map(|dir| dir.join("usage.json")),
        }
    }

    /// Where a probe of this login runs and what it locks.
    ///
    /// An account's lock sits in its usage login, `<data>/accounts/<id>/claude`,
    /// which is exactly the directory whose refresh token it protects. The local
    /// login's lock sits next to its reading file rather than in `~/.claude`:
    /// the user's own root is never written to by this server.
    pub fn probe_target(&self, paths: &Paths) -> Option<ProbeTarget> {
        let reading_file = self.reading_file(paths)?;
        Some(match self {
            UsageLogin::Local => ProbeTarget {
                config_dir: None,
                lock: paths.data_dir().join("local-usage.probe.lock"),
                reading_file,
            },
            UsageLogin::Account(id) => {
                let dir = usage_config_dir_in(paths, id).ok()?;
                ProbeTarget {
                    lock: probe_lock_path(&dir),
                    config_dir: Some(dir),
                    reading_file,
                }
            }
        })
    }
}

/// The probe lock of a usage login directory.
///
/// Public because the model-list probe spawns the same CLI against the same
/// login, and is exactly as able to refresh its token.
pub fn probe_lock_path(config_dir: &Path) -> PathBuf {
    config_dir.join(PROBE_LOCK_NAME)
}

/// Which login a session's reading belongs to.
///
/// * a session bound to an account bills that account, whichever root it is in;
/// * a session with no account in the user's own `~/.claude` bills the local
///   login;
/// * a session with no account in this instance's managed root runs on whatever
///   login that root itself holds — some other login, of nobody's choosing here —
///   and its reading is dropped rather than filed under the wrong name. So is one
///   whose root cannot be told.
pub fn session_login(account: Option<&str>, root: Option<ClaudeRoot>) -> Option<UsageLogin> {
    match (account, root) {
        (Some(id), _) => Some(UsageLogin::Account(id.to_string())),
        (None, Some(ClaudeRoot::Local)) => Some(UsageLogin::Local),
        (None, _) => None,
    }
}

// ---------------------------------------------------------- session readings

/// The label and length of one `unifiedWindows` key, named the way
/// [`super::parse_claude_usage`] names the probe's windows, so the two sources
/// merge by label.
///
/// * `five_hour` → `5h`, `seven_day` → `weekly`;
/// * `seven_day_<family>` → `weekly (<Family>)`, spelled as the CLI's own
///   `/usage` report spells it — which for Sonnet is `Current week (Sonnet
///   only)`, read out of the formatter in claude 2.1.281;
/// * `seven_day_overage_included` → `weekly (Fable)`. **Inferred** from one
///   sample, whose CLI text was "You've reached your Fable limit". The CLI
///   itself scopes the window to a list of models that need usage credits past
///   it, so should that list ever hold more than Fable, this label will be too
///   narrow;
/// * any other `seven_day_<x>` → `weekly (<x>)`, as it comes;
/// * anything else is not a window this knows the length of, and is skipped.
fn unified_window_label(key: &str) -> Option<(String, i64)> {
    const WEEK: i64 = 10080;
    match key {
        "five_hour" => Some(("5h".to_string(), 300)),
        "seven_day" => Some(("weekly".to_string(), WEEK)),
        "seven_day_overage_included" => Some(("weekly (Fable)".to_string(), WEEK)),
        _ => {
            let scope = key.strip_prefix("seven_day_").filter(|scope| !scope.is_empty())?;
            let scope = match scope {
                "opus" => "Opus",
                "sonnet" => "Sonnet only",
                "haiku" => "Haiku",
                "fable" => "Fable",
                other => other,
            };
            Some((format!("weekly ({scope})"), WEEK))
        }
    }
}

/// The windows in a `rate_limit_event`'s `rate_limit_info`, whatever its status.
///
/// `utilization` is a fraction; the panel speaks percent to one decimal, as the
/// probe's report does. `resetsAt` is epoch seconds. A window with no
/// utilization is skipped rather than read as zero.
pub fn windows_from_rate_limit_info(info: &Value) -> Vec<QuotaWindow> {
    let Some(Value::Object(unified)) = info.get("unifiedWindows") else {
        return vec![];
    };
    let mut windows: Vec<(bool, QuotaWindow)> = Vec::new();
    for (key, raw) in unified {
        let Some((label, minutes)) = unified_window_label(key) else {
            tracing::debug!(window = %key, "quota: skipping a unifiedWindows entry of unknown length");
            continue;
        };
        let Some(utilization) = raw.get("utilization").and_then(Value::as_f64).filter(|u| u.is_finite()) else {
            continue;
        };
        let used = (utilization * 1000.0).round() / 10.0;
        let resets_at = raw
            .get("resetsAt")
            .and_then(Value::as_f64)
            .filter(|secs| secs.is_finite() && *secs > 0.0)
            .map(|secs| iso_ms((secs * 1000.0) as i64));
        // A family's own key outranks the inferred one if both ever arrive in
        // one event: `seven_day_fable` would say what `overage_included` guesses.
        let inferred = key == "seven_day_overage_included";
        let candidate = window(label, minutes, used, resets_at);
        match windows.iter_mut().find(|(_, existing)| existing.label == candidate.label) {
            Some(slot) if slot.0 && !inferred => *slot = (inferred, candidate),
            Some(_) => {}
            None => windows.push((inferred, candidate)),
        }
    }
    windows.into_iter().map(|(_, window)| window).collect()
}

// ---------------------------------------------------------- the shared file

/// One window as the shared file keeps it: the reading and when it was made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredWindow {
    /// See [`QuotaWindow::label`].
    pub label: String,
    /// See [`QuotaWindow::used_percent`].
    pub used_percent: f64,
    /// See [`QuotaWindow::remaining_percent`].
    pub remaining_percent: f64,
    /// See [`QuotaWindow::window_minutes`].
    pub window_minutes: i64,
    /// See [`QuotaWindow::resets_at`].
    #[serde(default)]
    pub resets_at: Option<String>,
    /// Epoch milliseconds of the reading this window came from.
    pub read_at_ms: i64,
    /// Where that reading came from.
    pub source: QuotaReadingSource,
}

impl StoredWindow {
    /// A window read at `at_ms` from `source`.
    pub fn new(window: QuotaWindow, at_ms: i64, source: QuotaReadingSource) -> Self {
        StoredWindow {
            label: window.label,
            used_percent: window.used_percent,
            remaining_percent: window.remaining_percent,
            window_minutes: window.window_minutes,
            resets_at: window.resets_at.0,
            read_at_ms: at_ms,
            source,
        }
    }

    /// The window as the API shows it, stamped with when it was read.
    pub fn to_window(&self) -> QuotaWindow {
        QuotaWindow {
            label: self.label.clone(),
            used_percent: self.used_percent,
            remaining_percent: self.remaining_percent,
            window_minutes: self.window_minutes,
            resets_at: Nullable(self.resets_at.clone()),
            updated_at: Optional(Some(iso_ms(self.read_at_ms))),
        }
    }
}

/// Merge `incoming` into `windows` by label.
///
/// A window the new reading names is replaced; one it does not name keeps its
/// own, older, instant. The one exception is a reading *older* than the window
/// already held — two instances, or a probe that took a while beside a session
/// that did not — which loses, so the order writes happen to land in cannot
/// wind a window back.
fn merge_into(windows: &mut Vec<StoredWindow>, incoming: impl IntoIterator<Item = StoredWindow>) {
    for new in incoming {
        match windows.iter_mut().find(|held| held.label == new.label) {
            Some(held) if held.read_at_ms > new.read_at_ms => {}
            Some(held) => *held = new,
            None => windows.push(new),
        }
    }
    // Shortest window first, windows of unknown length last: `5h`, `weekly`,
    // `weekly (Fable)`, whichever source put them there first.
    windows.sort_by(|a, b| (a.window_minutes == 0, a.window_minutes, &a.label).cmp(&(b.window_minutes == 0, b.window_minutes, &b.label)));
}

/// What the shared file holds for one login.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SharedReading {
    /// Every window ever read, each with its own instant.
    pub windows: Vec<StoredWindow>,
    /// The last fallback probe's failure; `None` once a probe succeeds.
    pub probe_error: Option<QuotaProbeError>,
    /// When that failure happened, epoch milliseconds.
    pub probe_error_at_ms: Option<i64>,
    /// Whether that failure was a 429, which backs off for
    /// [`RATE_LIMIT_BACKOFF`] rather than [`PROBE_FAILURE_BACKOFF`].
    pub probe_rate_limited: bool,
    /// When the last probe of any outcome finished, epoch milliseconds.
    pub probed_at_ms: Option<i64>,
}

/// Whether the fallback probe should run for a login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    /// The newest window is recent enough.
    Fresh,
    /// A probe failed recently; leave the login alone until this instant.
    BackingOff {
        /// Epoch milliseconds.
        until_ms: i64,
    },
    /// Probe it.
    Due,
}

impl SharedReading {
    /// Merge one reading's windows, all read at `at_ms`.
    pub fn merge(&mut self, windows: Vec<QuotaWindow>, at_ms: i64, source: QuotaReadingSource) {
        merge_into(&mut self.windows, windows.into_iter().map(|w| StoredWindow::new(w, at_ms, source)));
    }

    /// Merge windows that already carry their instants.
    pub fn merge_stored(&mut self, windows: &[StoredWindow]) {
        merge_into(&mut self.windows, windows.iter().cloned());
    }

    /// Record a finished probe: its windows when it read some, its verdict when
    /// it did not — and in that case the windows already held stay, so a failure
    /// never blanks the panel.
    pub fn record_probe(&mut self, probe: UsageProbe, at_ms: i64) {
        self.probed_at_ms = Some(at_ms);
        match probe.error {
            None => {
                self.merge(probe.windows, at_ms, QuotaReadingSource::Probe);
                self.probe_error = None;
                self.probe_error_at_ms = None;
                self.probe_rate_limited = false;
            }
            Some(error) => {
                self.probe_error = Some(error);
                self.probe_error_at_ms = Some(at_ms);
                self.probe_rate_limited = probe.rate_limited;
            }
        }
    }

    /// The most recently read window.
    pub fn newest(&self) -> Option<&StoredWindow> {
        self.windows.iter().max_by_key(|w| w.read_at_ms)
    }

    /// Whether the fallback probe should run now.
    ///
    /// A recent failure wins over staleness: a login whose probe just failed is
    /// by definition stale, and asking again is exactly the loop to avoid.
    pub fn fallback(&self, now_ms: i64, fallback_after: Duration) -> Fallback {
        if let (Some(_), Some(at)) = (self.probe_error, self.probe_error_at_ms) {
            let backoff = if self.probe_rate_limited {
                RATE_LIMIT_BACKOFF
            } else {
                PROBE_FAILURE_BACKOFF
            };
            let until_ms = at.saturating_add(backoff.as_millis() as i64);
            if now_ms < until_ms {
                return Fallback::BackingOff { until_ms };
            }
        }
        match self.newest() {
            Some(newest) if now_ms.saturating_sub(newest.read_at_ms) < fallback_after.as_millis() as i64 => Fallback::Fresh,
            _ => Fallback::Due,
        }
    }
}

/// Read a login's shared file. Takes no lock: writes land by rename.
///
/// A missing file is an empty reading; so is an unreadable one, with a warning,
/// because the next write replaces it whole.
pub fn load(path: &Path) -> SharedReading {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            tracing::warn!(path = %path.display(), %error, "quota: the shared reading file is not readable; starting it afresh");
            SharedReading::default()
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => SharedReading::default(),
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "quota: cannot read the shared reading file");
            SharedReading::default()
        }
    }
}

/// Read, change and write a login's shared file, under its lock.
///
/// The lock is `<file>.lock` and blocking: every holder is one of these short
/// read-modify-writes, never a probe. The new contents go to a temporary file
/// beside the real one and are renamed over it.
pub fn update(path: &Path, change: impl FnOnce(&mut SharedReading)) -> io::Result<SharedReading> {
    let parent = path.parent().ok_or_else(|| io::Error::other("a reading file has no directory"))?;
    std::fs::create_dir_all(parent)?;
    let _lock = Flock::exclusive(&with_suffix(path, ".lock"))?;
    let mut reading = load(path);
    change(&mut reading);
    let bytes = serde_json::to_vec_pretty(&reading).map_err(io::Error::other)?;
    write_atomic(path, &bytes)?;
    Ok(reading)
}

/// `path` with `suffix` appended to its file name.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().map(|name| name.to_os_string()).unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

/// Write `bytes` to `path` so a reader sees the old file or the new one, never
/// a mixture.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = with_suffix(path, &format!(".tmp.{}", std::process::id()));
    let written = (|| {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    let renamed = written.and_then(|()| std::fs::rename(&tmp, path));
    if renamed.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    renamed
}

/// An exclusive `flock`, released when dropped.
///
/// `flock` locks belong to the open file description, so two separate opens of
/// one file contend even inside a single process — which is what lets one
/// instance's probe lock hold off a second attempt of its own, and what the
/// tests use to stand in for a second instance.
struct Flock {
    file: File,
}

impl Flock {
    fn open(path: &Path) -> io::Result<File> {
        OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)
    }

    /// Wait for the lock.
    fn exclusive(path: &Path) -> io::Result<Flock> {
        let file = Self::open(path)?;
        loop {
            // SAFETY: `flock` on a descriptor this function owns; it touches no
            // memory of ours.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                return Ok(Flock { file });
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    /// Take the lock if nobody holds it; `None` if somebody does.
    fn try_exclusive(path: &Path) -> io::Result<Option<Flock>> {
        let file = Self::open(path)?;
        // SAFETY: as above.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(Some(Flock { file }));
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::WouldBlock {
            Ok(None)
        } else {
            Err(error)
        }
    }
}

impl Drop for Flock {
    fn drop(&mut self) {
        // Closing the descriptor would release it anyway; saying so is cheaper
        // than relying on the order fields are dropped in.
        // SAFETY: as above.
        unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// The machine-wide right to probe one usage login, held for the whole probe.
pub struct ProbeLock {
    _lock: Flock,
}

impl ProbeLock {
    /// Take it if nobody on this machine holds it; `None` if somebody does.
    ///
    /// Never waits: a probe that finds another one running has nothing to add,
    /// because that one's result is about to land in the shared file.
    pub fn try_acquire(path: &Path) -> io::Result<Option<ProbeLock>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(Flock::try_exclusive(path)?.map(|lock| ProbeLock { _lock: lock }))
    }
}

// ------------------------------------------------------------- the throttle

/// Session readings for one login, held between writes of its shared file.
#[derive(Debug, Default)]
pub struct Throttle {
    /// When the file was last written from here.
    last_write: Option<Instant>,
    /// Readings not yet written, merged by label.
    pending: Vec<StoredWindow>,
    /// Whether a flush of `pending` is already scheduled.
    scheduled: bool,
}

/// What to do with a session reading that was just offered.
#[derive(Debug, PartialEq)]
pub enum Offer {
    /// Write these windows now.
    Write(Vec<StoredWindow>),
    /// Held. `Some` asks for a flush this far from now; `None` means one is
    /// already on its way.
    Hold(Option<Duration>),
}

impl Throttle {
    /// Offer a reading; it is written at once unless the file was written less
    /// than [`SESSION_FLUSH_EVERY`] ago.
    pub fn offer(&mut self, now: Instant, windows: Vec<StoredWindow>) -> Offer {
        merge_into(&mut self.pending, windows);
        match self.last_write {
            Some(at) if now.saturating_duration_since(at) < SESSION_FLUSH_EVERY => {
                if self.scheduled {
                    Offer::Hold(None)
                } else {
                    self.scheduled = true;
                    Offer::Hold(Some(SESSION_FLUSH_EVERY - now.saturating_duration_since(at)))
                }
            }
            _ => {
                self.last_write = Some(now);
                self.scheduled = false;
                Offer::Write(std::mem::take(&mut self.pending))
            }
        }
    }

    /// Everything held, to be written now, whatever the clock says; `None` when
    /// nothing is held.
    pub fn take(&mut self, now: Instant) -> Option<Vec<StoredWindow>> {
        self.scheduled = false;
        if self.pending.is_empty() {
            return None;
        }
        self.last_write = Some(now);
        Some(std::mem::take(&mut self.pending))
    }

    /// Everything held, when the interval since the last write is up.
    pub fn due(&mut self, now: Instant) -> Option<Vec<StoredWindow>> {
        if self.last_write.is_some_and(|at| now.saturating_duration_since(at) < SESSION_FLUSH_EVERY) {
            return None;
        }
        self.take(now)
    }

    /// What is held right now.
    pub fn pending(&self) -> &[StoredWindow] {
        &self.pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `allowed` event, the kind claude writes before nearly every turn.
    const ALLOWED: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"isUsingOverage":false,"overageDisabledReason":"org_level_disabled","overageStatus":"rejected","rateLimitType":"five_hour","resetsAt":1790436000,"status":"allowed","unifiedWindows":{"five_hour":{"resetsAt":1790436000,"utilization":0.19},"seven_day":{"resetsAt":1790460000,"utilization":0.46}}}}"#;
    /// The `rate_limit_info` of a Fable weekly hit.
    const REJECTED_FABLE: &str = r#"{"isUsingOverage":false,"overageDisabledReason":"org_level_disabled","overageStatus":"rejected","rateLimitType":"seven_day_overage_included","resetsAt":1790575200,"status":"rejected","unifiedWindows":{"five_hour":{"resetsAt":1790436000,"utilization":0.02},"seven_day":{"resetsAt":1790575200,"utilization":0.82},"seven_day_overage_included":{"resetsAt":1790575200,"utilization":1}}}"#;

    const MINUTE: i64 = 60_000;

    fn stored(label: &str, minutes: i64, used: f64, at_ms: i64, source: QuotaReadingSource) -> StoredWindow {
        StoredWindow::new(window(label.to_string(), minutes, used, None), at_ms, source)
    }

    fn labels(windows: &[StoredWindow]) -> Vec<&str> {
        windows.iter().map(|w| w.label.as_str()).collect()
    }

    #[test]
    fn an_allowed_event_is_a_reading_of_every_window_it_names() {
        let line: Value = serde_json::from_str(ALLOWED).expect("json");
        let windows = windows_from_rate_limit_info(&line["rate_limit_info"]);
        assert_eq!(windows.len(), 2);

        assert_eq!(windows[0].label, "5h");
        assert_eq!(windows[0].window_minutes, 300);
        assert_eq!(windows[0].used_percent, 19.0, "0.19 is 19 %, rounded to one decimal");
        assert_eq!(windows[0].remaining_percent, 81.0);
        assert_eq!(windows[0].resets_at.0.as_deref(), Some("2026-09-26T15:20:00.000Z"));

        assert_eq!(windows[1].label, "weekly");
        assert_eq!(windows[1].window_minutes, 10080);
        assert_eq!(windows[1].used_percent, 46.0);
        assert_eq!(windows[1].resets_at.0.as_deref(), Some("2026-09-26T22:00:00.000Z"));
    }

    #[test]
    fn the_fable_weekly_hit_maps_its_overage_window_to_weekly_fable() {
        let info: Value = serde_json::from_str(REJECTED_FABLE).expect("json");
        let windows = windows_from_rate_limit_info(&info);
        let named: Vec<(&str, f64)> = windows.iter().map(|w| (w.label.as_str(), w.used_percent)).collect();
        assert_eq!(named, [("5h", 2.0), ("weekly", 82.0), ("weekly (Fable)", 100.0)]);
        assert_eq!(windows[2].remaining_percent, 0.0);
        assert_eq!(windows[2].window_minutes, 10080);
        assert_eq!(windows[2].resets_at.0.as_deref(), Some("2026-09-28T06:00:00.000Z"));
    }

    #[test]
    fn window_keys_are_named_the_way_the_probe_names_them() {
        let label = |key: &str| unified_window_label(key).map(|(label, _)| label);
        assert_eq!(label("seven_day_opus").as_deref(), Some("weekly (Opus)"));
        // `/usage` prints `Current week (Sonnet only)` for this window.
        assert_eq!(label("seven_day_sonnet").as_deref(), Some("weekly (Sonnet only)"));
        assert_eq!(label("seven_day_haiku").as_deref(), Some("weekly (Haiku)"));
        assert_eq!(label("seven_day_fable").as_deref(), Some("weekly (Fable)"));
        assert_eq!(
            label("seven_day_cowork").as_deref(),
            Some("weekly (cowork)"),
            "an unknown scope comes as it is"
        );
        assert_eq!(label("overage"), None, "no length to claim, so no window");
        assert_eq!(label("seven_day_"), None);

        // Both Fable keys in one event: the family's own name wins.
        let info = serde_json::json!({"unifiedWindows": {
            "seven_day_fable": {"utilization": 0.5},
            "seven_day_overage_included": {"utilization": 0.9},
        }});
        let windows = windows_from_rate_limit_info(&info);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].used_percent, 50.0);

        // No `unifiedWindows`, or a window with no utilization: nothing to read.
        assert!(windows_from_rate_limit_info(&serde_json::json!({"status": "allowed"})).is_empty());
        let partial = serde_json::json!({"unifiedWindows": {"five_hour": {"resetsAt": 1790436000}}});
        assert!(windows_from_rate_limit_info(&partial).is_empty());
    }

    #[test]
    fn a_merge_replaces_the_windows_it_names_and_keeps_the_rest_with_their_own_instants() {
        let t0 = 1_790_000_000_000;
        let mut reading = SharedReading::default();
        reading.merge(
            vec![
                window("5h".into(), 300, 10.0, None),
                window("weekly".into(), 10080, 40.0, None),
                window("weekly (Fable)".into(), 10080, 80.0, None),
            ],
            t0,
            QuotaReadingSource::Probe,
        );
        // A session event names two of the three.
        let info: Value = serde_json::from_str::<Value>(ALLOWED).expect("json")["rate_limit_info"].clone();
        reading.merge(windows_from_rate_limit_info(&info), t0 + 20 * MINUTE, QuotaReadingSource::Session);

        assert_eq!(labels(&reading.windows), ["5h", "weekly", "weekly (Fable)"]);
        let by = |label: &str| reading.windows.iter().find(|w| w.label == label).expect(label).clone();
        assert_eq!(by("5h").used_percent, 19.0);
        assert_eq!(by("5h").read_at_ms, t0 + 20 * MINUTE);
        assert_eq!(by("5h").source, QuotaReadingSource::Session);
        assert_eq!(by("weekly").read_at_ms, t0 + 20 * MINUTE);
        assert_eq!(by("weekly (Fable)").used_percent, 80.0, "a window the event does not name is kept");
        assert_eq!(by("weekly (Fable)").read_at_ms, t0, "with its own, older, instant");
        assert_eq!(by("weekly (Fable)").source, QuotaReadingSource::Probe);

        let newest = reading.newest().expect("a newest window");
        assert_eq!(newest.read_at_ms, t0 + 20 * MINUTE, "the login's instant is its newest window's");
        assert_eq!(newest.source, QuotaReadingSource::Session);

        // A reading older than what is held cannot wind a window back.
        reading.merge(vec![window("5h".into(), 300, 99.0, None)], t0 + MINUTE, QuotaReadingSource::Probe);
        assert_eq!(by_label(&reading, "5h").used_percent, 19.0);

        // Stamped on the way out.
        let shown = by_label(&reading, "weekly (Fable)").to_window();
        assert_eq!(shown.updated_at.0, Some(iso_ms(t0)));
    }

    fn by_label(reading: &SharedReading, label: &str) -> StoredWindow {
        reading.windows.iter().find(|w| w.label == label).expect(label).clone()
    }

    #[test]
    fn a_failed_probe_keeps_the_windows_and_records_its_verdict() {
        let t0 = 1_790_000_000_000;
        let mut reading = SharedReading::default();
        reading.record_probe(
            UsageProbe {
                windows: vec![window("5h".into(), 300, 10.0, None)],
                error: None,
                rate_limited: false,
            },
            t0,
        );
        assert_eq!(reading.windows.len(), 1);
        assert_eq!(reading.probe_error, None);

        reading.record_probe(
            UsageProbe {
                windows: vec![],
                error: Some(QuotaProbeError::NotLoggedIn),
                rate_limited: false,
            },
            t0 + MINUTE,
        );
        assert_eq!(reading.windows.len(), 1, "a failure must not blank the panel");
        assert_eq!(reading.windows[0].read_at_ms, t0, "and must not claim to be fresh");
        assert_eq!(reading.probe_error, Some(QuotaProbeError::NotLoggedIn));
        assert_eq!(reading.probe_error_at_ms, Some(t0 + MINUTE));
        assert_eq!(reading.probed_at_ms, Some(t0 + MINUTE));

        // A clean probe clears the verdict.
        reading.record_probe(
            UsageProbe {
                windows: vec![window("5h".into(), 300, 12.0, None)],
                error: None,
                rate_limited: false,
            },
            t0 + 40 * MINUTE,
        );
        assert_eq!(reading.probe_error, None);
        assert_eq!(reading.probe_error_at_ms, None);
    }

    #[test]
    fn the_fallback_probes_only_a_stale_login_with_no_recent_failure() {
        let after = DEFAULT_FALLBACK_AFTER;
        let t0 = 1_790_000_000_000;

        // Never read at all: probe it.
        assert_eq!(SharedReading::default().fallback(t0, after), Fallback::Due);

        let mut reading = SharedReading::default();
        reading.merge(vec![window("5h".into(), 300, 10.0, None)], t0, QuotaReadingSource::Session);
        assert_eq!(reading.fallback(t0 + 14 * MINUTE, after), Fallback::Fresh, "a session read it lately");
        assert_eq!(reading.fallback(t0 + 15 * MINUTE, after), Fallback::Due, "nothing has run for 15 minutes");

        // A probe that failed in the last half hour holds it off, whichever way.
        for error in [QuotaProbeError::NotLoggedIn, QuotaProbeError::ProbeFailed] {
            let mut failed = reading.clone();
            failed.record_probe(
                UsageProbe {
                    windows: vec![],
                    error: Some(error),
                    rate_limited: false,
                },
                t0 + 20 * MINUTE,
            );
            assert_eq!(
                failed.fallback(t0 + 49 * MINUTE, after),
                Fallback::BackingOff { until_ms: t0 + 50 * MINUTE },
                "{error:?}"
            );
            assert_eq!(
                failed.fallback(t0 + 50 * MINUTE, after),
                Fallback::Due,
                "{error:?} is forgiven after 30 minutes"
            );
        }

        // A 429 keeps its own backoff.
        let mut limited = reading.clone();
        limited.record_probe(
            UsageProbe {
                windows: vec![],
                error: Some(QuotaProbeError::ProbeFailed),
                rate_limited: true,
            },
            t0 + 20 * MINUTE,
        );
        assert!(limited.probe_rate_limited);
        let until_ms = t0 + 20 * MINUTE + RATE_LIMIT_BACKOFF.as_millis() as i64;
        assert_eq!(limited.fallback(t0 + 21 * MINUTE, after), Fallback::BackingOff { until_ms });

        // A fresh session reading beats a stale failure: nothing to probe for.
        let mut revived = limited.clone();
        revived.merge(vec![window("5h".into(), 300, 30.0, None)], t0 + 60 * MINUTE, QuotaReadingSource::Session);
        assert_eq!(revived.fallback(t0 + 61 * MINUTE, after), Fallback::Fresh);
    }

    #[test]
    fn a_held_probe_lock_turns_the_second_prober_away() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("claude").join(PROBE_LOCK_NAME);

        // Two handles, opened separately, stand in for two instances.
        let first = ProbeLock::try_acquire(&path).expect("lockable").expect("the first prober gets it");
        assert!(path.exists(), "the lock creates its own directory");
        assert!(
            ProbeLock::try_acquire(&path).expect("lockable").is_none(),
            "a second prober skips rather than waits"
        );
        drop(first);
        assert!(
            ProbeLock::try_acquire(&path).expect("lockable").is_some(),
            "and gets it once the first is done"
        );
    }

    #[test]
    fn the_shared_file_round_trips_and_is_replaced_whole() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("accounts").join("x").join("usage.json");
        assert_eq!(load(&path), SharedReading::default(), "no file is an empty reading");

        let t0 = 1_790_000_000_000;
        let written = update(&path, |r| {
            r.merge(
                vec![window("5h".into(), 300, 19.0, Some("2026-09-26T15:20:00.000Z".into()))],
                t0,
                QuotaReadingSource::Session,
            );
            r.record_probe(
                UsageProbe {
                    windows: vec![],
                    error: Some(QuotaProbeError::NotLoggedIn),
                    rate_limited: false,
                },
                t0 + MINUTE,
            );
        })
        .expect("written");
        assert_eq!(load(&path), written, "what is read back is what was written");

        let raw: Value = serde_json::from_slice(&std::fs::read(&path).expect("file")).expect("json");
        assert_eq!(raw["windows"][0]["source"], "session");
        assert_eq!(raw["windows"][0]["readAtMs"], t0);
        assert_eq!(raw["probeError"], "not_logged_in");
        assert_eq!(raw["probeErrorAtMs"], t0 + MINUTE);

        // Nothing is left beside it but the lock.
        let mut names: Vec<String> = std::fs::read_dir(path.parent().expect("dir"))
            .expect("dir")
            .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["usage.json", "usage.json.lock"]);

        // Garbage on disk is a fresh start, not a failure.
        std::fs::write(&path, b"{ not json").expect("garbage");
        assert_eq!(load(&path), SharedReading::default());
        update(&path, |r| {
            r.merge(vec![window("weekly".into(), 10080, 1.0, None)], t0, QuotaReadingSource::Probe)
        })
        .expect("rewritten");
        assert_eq!(labels(&load(&path).windows), ["weekly"]);
    }

    /// Writers that race lose nothing: each one's read-modify-write happens
    /// under the lock, so every label lands.
    #[test]
    fn concurrent_writers_serialize_on_the_file_lock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("usage.json");
        let threads: Vec<_> = (0..8)
            .map(|n| {
                let path = path.clone();
                std::thread::spawn(move || {
                    update(&path, |r| {
                        r.merge(vec![window(format!("w{n}"), 60, n as f64, None)], 1, QuotaReadingSource::Session)
                    })
                    .expect("written");
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("writer");
        }
        assert_eq!(load(&path).windows.len(), 8);
    }

    #[test]
    fn session_readings_are_written_at_most_once_per_interval() {
        let start = Instant::now();
        let reading = |used: f64, at: i64| vec![stored("5h", 300, used, at, QuotaReadingSource::Session)];
        let mut throttle = Throttle::default();

        // The first goes straight through.
        assert_eq!(throttle.offer(start, reading(1.0, 1)), Offer::Write(reading(1.0, 1)));
        assert!(throttle.pending().is_empty());

        // Inside the interval: held, and a flush is asked for when it is up.
        let five = Duration::from_secs(5);
        assert_eq!(
            throttle.offer(start + five, reading(2.0, 2)),
            Offer::Hold(Some(SESSION_FLUSH_EVERY - five))
        );
        // Another: held too, newest kept, no second flush asked for.
        assert_eq!(throttle.offer(start + five * 2, reading(3.0, 3)), Offer::Hold(None));
        assert_eq!(throttle.pending(), reading(3.0, 3).as_slice(), "the newest reading is what is held");

        // The interval is not up yet, so a periodic sweep leaves it...
        assert_eq!(throttle.due(start + five * 2), None);
        // ...and once it is, the held reading goes out.
        assert_eq!(throttle.due(start + SESSION_FLUSH_EVERY), Some(reading(3.0, 3)));
        assert_eq!(throttle.due(start + SESSION_FLUSH_EVERY * 3), None, "nothing held, nothing to write");

        // The next offer after a flush is measured from that flush.
        assert_eq!(
            throttle.offer(start + SESSION_FLUSH_EVERY + five, reading(4.0, 4)),
            Offer::Hold(Some(SESSION_FLUSH_EVERY - five))
        );
        assert_eq!(
            throttle.take(start + SESSION_FLUSH_EVERY + five),
            Some(reading(4.0, 4)),
            "a scheduled flush takes it whatever the clock says"
        );
        assert_eq!(throttle.take(start + SESSION_FLUSH_EVERY + five), None);
    }

    #[test]
    fn a_session_reading_goes_to_the_login_the_session_bills() {
        let id = "0b6f7a7e-2f7a-4d8e-9d0c-1a2b3c4d5e6f";
        assert_eq!(session_login(Some(id), Some(ClaudeRoot::Managed)), Some(UsageLogin::Account(id.into())));
        assert_eq!(
            session_login(Some(id), None),
            Some(UsageLogin::Account(id.into())),
            "the account decides, not the root"
        );
        assert_eq!(
            session_login(None, Some(ClaudeRoot::Local)),
            Some(UsageLogin::Local),
            "the user's own ~/.claude"
        );
        assert_eq!(
            session_login(None, Some(ClaudeRoot::Managed)),
            None,
            "the managed root's own login is nobody's here"
        );
        assert_eq!(session_login(None, None), None, "a root that cannot be told is skipped");
    }

    #[test]
    fn the_files_sit_where_both_instances_look() {
        let paths = Paths::for_instance(PathBuf::from("/home/me"), "bob");
        let id = "0b6f7a7e-2f7a-4d8e-9d0c-1a2b3c4d5e6f";
        let account = UsageLogin::Account(id.into());
        assert_eq!(
            account.reading_file(&paths),
            Some(PathBuf::from(format!("/home/me/.local/share/remote-hand/accounts/{id}/usage.json"))),
            "machine-level, beside the usage login rather than in an instance"
        );
        let target = account.probe_target(&paths).expect("a target");
        assert_eq!(
            target.config_dir,
            Some(PathBuf::from(format!("/home/me/.local/share/remote-hand/accounts/{id}/claude")))
        );
        assert_eq!(
            target.lock,
            PathBuf::from(format!("/home/me/.local/share/remote-hand/accounts/{id}/claude/.rh-probe.lock"))
        );

        let local = UsageLogin::Local.probe_target(&paths).expect("a target");
        assert_eq!(local.reading_file, PathBuf::from("/home/me/.local/share/remote-hand/local-usage.json"));
        assert_eq!(local.config_dir, None);
        assert!(!local.lock.starts_with("/home/me/.claude"), "nothing is written into the user's own root");

        assert_eq!(
            UsageLogin::Account("../../etc".into()).reading_file(&paths),
            None,
            "only a uuid names a directory"
        );
    }
}
