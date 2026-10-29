//! Subscription quota: the Claude plan and live window usage, plus Codex's own
//! rolling windows.
//!
//! ## Claude: sessions first, the probe as a fallback
//!
//! A Claude login's windows come from two places, described in full in
//! [`readings`]:
//!
//! * **the sessions themselves** — every `rate_limit_event` claude writes carries
//!   `unifiedWindows`, and the session's driver files it under the login it runs
//!   on ([`QuotaCache::ingest_session`]);
//! * **`claude -p /usage`**, only when a login's newest window is older than
//!   [`readings::DEFAULT_FALLBACK_AFTER`] (`DROPPEN_USAGE_FALLBACK_MS`) and no probe of
//!   it failed in the last half hour — from a background ticker
//!   ([`spawn_fallback_ticker`]) and from `GET /api/stats/quota`.
//!
//! Readings live in one file per login that every instance on the machine reads,
//! and only one process machine-wide may probe a login at a time. Both rules
//! exist because an account's usage login has a rotating refresh token: two
//! instances probing on their own schedules could refresh it twice, and the
//! second refresh revokes the login.
//!
//! Three rules hold:
//!
//! * a failed probe is remembered, **failures included** — a broken or hanging
//!   CLI costs one probe per backoff, not one per request;
//! * probes are serialized, so N accounts never fire N CLI startups at once;
//! * a request waits at most [`COLLECT_BUDGET`] for cold probes and then answers
//!   with whatever is on file. The probes carry on; the next poll picks them up.
//!
//! ## The poll budget
//!
//! Rate limiting is keyed by *account*, not by client, so a panel that polls
//! eagerly spends the same 429 the user's real work needs. A login is probed at
//! most once per fallback threshold — four times an hour at the default, machine
//! wide — and not at all while a session is reading it for free. A 429 takes it
//! down to two probes an hour ([`RATE_LIMIT_BACKOFF`]), as does any other failure
//! ([`readings::PROBE_FAILURE_BACKOFF`]).
//!
//! Codex's windows are read of a different CLI over a different protocol, so
//! they live in [`codex`], with a reading reused for [`USAGE_TTL`], probes
//! serialized and requests bounded by [`COLLECT_BUDGET`]. Scraping a session
//! file on every request instead would report a three-day-old percentage as the
//! current one.

pub mod claude;
pub mod codex;
pub mod readings;
pub mod routes;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::future::{BoxFuture, FutureExt, Shared};
use gotcha::Schematic;
use serde::{Deserialize, Serialize};

use crate::accounts::login::{Login, Logins};
use crate::accounts::store::{Account, AccountKind};
use droppen_http::openapi_ext::{Nullable, Optional};
use droppen_instance::paths::Paths;
use droppen_base::time::now_ms;

pub use readings::{QuotaReadingSource, UsageLogin};
pub use routes::{describe, QuotaState};

/// How long a Codex reading — successful or not — is reused. Claude readings
/// follow [`readings`] instead.
pub const USAGE_TTL: Duration = Duration::from_secs(180);
/// How long a single `/api/stats/quota` call may wait for cold probes.
pub const COLLECT_BUDGET: Duration = Duration::from_secs(3);
/// How long an account that answered a probe with a 429 is left alone.
///
/// The floor of a deliberately crude AIMD: back off hard on the one signal that
/// says the account is over its limit, and return to the ordinary cadence the
/// moment a probe comes back clean. There is no ramp in between because there is nothing
/// to ramp — the panel has exactly one request to make, and making it less often
/// is the only lever. Half an hour is long enough to outlast a typical window
/// reset and short enough that a user who fixes the problem is not left staring
/// at a stale panel.
pub const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(30 * 60);

/// One rolling window of a subscription.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    /// Human label for the window: `5h`, `weekly`, `daily`…
    pub label: String,
    /// How much of it is gone, 0–100.
    pub used_percent: f64,
    /// How much is left, 0–100.
    pub remaining_percent: f64,
    /// The window's length in minutes; 0 when unknown.
    pub window_minutes: i64,
    /// When it resets, ISO-8601.
    pub resets_at: Nullable<String>,
    /// ISO-8601 of when this window was read. Windows of one reading can differ in age.
    // A session event names only some of a login's windows, so each carries its
    // own instant. Absent on Codex windows.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub updated_at: Optional<String>,
}

/// Why an account has no windows.
///
/// `not_logged_in` is a verdict — the CLI ran and reported nothing — while
/// `probe_failed` only says the reading did not happen, so the UI must not send
/// the user back through `claude auth login` for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "snake_case")]
pub enum QuotaProbeError {
    /// The CLI ran and reported no windows.
    NotLoggedIn,
    /// The reading did not happen.
    ProbeFailed,
}

/// The user's own Claude login.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeQuota {
    /// Subscription plan label, e.g. `Max 20x`.
    pub plan: Nullable<String>,
    /// Raw `rateLimitTier`.
    pub tier: Nullable<String>,
    /// ISO-8601 of the reading.
    // The newest window's instant.
    pub updated_at: Nullable<String>,
    /// Where the newest window was read; absent when there are no windows.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub source: Optional<QuotaReadingSource>,
    /// The rolling windows.
    pub windows: Vec<QuotaWindow>,
}

/// One stored account's live window usage, read off its sessions or probed with its own usage login.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AccountQuota {
    /// The account's uuid.
    pub id: String,
    /// Its display name.
    pub label: String,
    /// How it authenticates.
    pub kind: AccountKind,
    /// Address of the login, when one is known.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub email: Optional<String>,
    /// Whether the quota panel probes it.
    // The fallback probe, that is: session readings are shown either way.
    pub usage_enabled: bool,
    /// The rolling windows.
    // Whatever read them — present for any account a session has run under,
    // probed or not.
    pub windows: Vec<QuotaWindow>,
    /// ISO-8601 of the last reading that produced windows.
    // The newest window's instant.
    pub updated_at: Nullable<String>,
    /// Where the newest window was read; absent when there are no windows.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub source: Optional<QuotaReadingSource>,
    /// Absent when fine or still probing.
    // The fallback probe's last verdict, and only for an account it probes.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub error: Optional<QuotaProbeError>,
}

/// Where a Codex reading came from.
///
/// On the wire, for the same reason `models::ModelListSource` is: the
/// app server answers with the account's live numbers, while the session logs
/// only say what some past turn was told. One shown as the other is how a panel
/// tells a user they have 30% left of a week they have already spent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "snake_case")]
pub enum CodexQuotaSource {
    /// `codex app-server` answered `account/rateLimits/read` just now.
    AppServer,
    /// The app server could not be asked; these numbers come out of a rollout
    /// file and are only as current as `updatedAt` says.
    SessionLog,
}

/// Codex's rolling-window usage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct CodexQuota {
    /// Plan name, e.g. `pro`.
    pub plan: Nullable<String>,
    /// ISO-8601 of the reading. For [`CodexQuotaSource::AppServer`] this is the
    /// moment the app server answered; for the log fallback it is the timestamp
    /// the rollout entry carried, which may be hours old.
    pub updated_at: Nullable<String>,
    /// The rolling windows.
    pub windows: Vec<QuotaWindow>,
    /// Which of the two readings above this is.
    pub source: CodexQuotaSource,
    /// Whether ordinary usage is blocked right now — the account is out, and
    /// the next message will be refused rather than merely expensive.
    ///
    /// `null` is *unknown*, not "fine": only the app server is asked this, so
    /// the log fallback always leaves it null rather than implying an account
    /// with 0% left is still able to run.
    pub blocked: Nullable<bool>,
}

/// `GET /api/stats/quota`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct Quota {
    /// The user's own login, or `null` when there is nothing to report.
    pub claude: Nullable<ClaudeQuota>,
    /// Codex, or `null`.
    pub codex: Nullable<CodexQuota>,
    /// One entry per stored account.
    pub accounts: Vec<AccountQuota>,
}

/// What one `claude -p /usage` run produced.
#[derive(Debug, Clone, Default)]
pub struct UsageProbe {
    /// The windows it reported.
    pub windows: Vec<QuotaWindow>,
    /// Why there are none.
    pub error: Option<QuotaProbeError>,
    /// The account is over its request limit; back it off rather than retry.
    pub rate_limited: bool,
}

// ------------------------------------------------------------ claude: parsing

/// `Max 20x` from `subscriptionType: "max"` plus `rateLimitTier: "…20x…"`.
///
/// Crate-visible because the first-run guide ([`crate::setup`]) labels the
/// plan `claude auth status` reports the same way the quota panel does.
pub(crate) fn claude_plan_label(subscription_type: Option<&str>, tier: Option<&str>) -> String {
    let base = match subscription_type {
        Some(value) if !value.is_empty() => {
            let mut chars = value.chars();
            let first = chars.next().expect("non-empty").to_uppercase().to_string();
            format!("{first}{}", chars.as_str())
        }
        _ => "Claude".to_string(),
    };
    static MULT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = MULT.get_or_init(|| regex::Regex::new(r"(\d+)x").expect("valid multiplier pattern"));
    match tier.and_then(|tier| re.captures(tier)) {
        Some(caps) => format!("{base} {}x", &caps[1]),
        None => base,
    }
}

fn window(label: String, minutes: i64, used: f64, resets_at: Option<String>) -> QuotaWindow {
    let used = used.clamp(0.0, 100.0);
    QuotaWindow {
        label,
        used_percent: used,
        remaining_percent: (100.0 - used).max(0.0),
        window_minutes: minutes,
        resets_at: Nullable(resets_at),
        updated_at: Optional(None),
    }
}

// ------------------------------------------------------------------ probing

/// What the quota probes run against: the user's home directory, which the
/// `claude` and `codex` CLIs are started in, and Codex's session logs, read
/// when its app server cannot be reached.
///
/// Its own type rather than methods on [`Paths`], so that starting a probe
/// goes through this module.
#[derive(Debug, Clone)]
pub struct ProbeRoots {
    home: PathBuf,
    codex_sessions: PathBuf,
}

impl ProbeRoots {
    /// The probes of the instance `paths` names.
    pub fn of(paths: &Paths) -> Self {
        Self {
            home: paths.home.clone(),
            codex_sessions: paths.codex_sessions(),
        }
    }
}

/// Markers that make a bare `429` an HTTP status code rather than an incidental
/// run of digits (a pid, a path segment, a token count). Kept narrow: the risk
/// this guards against is a *healthy* `/usage` reading being read as a 429.
const STATUS_MARKERS: [&str; 2] = ["http", "rate limit"];
/// How far on either side of a `429` a status marker still counts as its context.
const STATUS_WINDOW: usize = 32;

/// Whether a probe's output says the account is over its request limit.
///
/// Deliberately narrow. `/usage` legitimately talks about rate limits — the
/// credentials it reads carry a `rateLimitTier`, and the reply is about nothing
/// but windows — so matching the bare digits `429` anywhere would back an account
/// off on a path, a pid or a token count that happened to contain them. A `429`
/// counts only as a bounded token sitting beside a status marker; the RFC 6585
/// reason phrase "too many requests" counts on its own, since `/usage` never
/// prints it.
///
/// Public because the credential check in [`crate::accounts::verify`] asks the
/// same question of its own probe's output, and for the same reason: an account
/// that is over its limit is a *working* account.
pub fn is_rate_limited(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("too many requests") || mentions_status(&lower, "429", Some((&STATUS_MARKERS, STATUS_WINDOW)))
}

/// Whether `lower` contains `code` as a standalone HTTP status code: bounded by
/// non-digits, so a longer number cannot match. With `near`, the code also has
/// to sit within that many bytes of one of the given markers — the narrow rule
/// for `429`, where a coincidental number in prose must not back an account
/// off, and the plain rule for `401`/`403`, where the status is evidence by
/// itself.
pub(crate) fn mentions_status(lower: &str, code: &str, near: Option<(&[&str], usize)>) -> bool {
    let bytes = lower.as_bytes();
    let mut from = 0;
    while let Some(rel) = lower[from..].find(code) {
        let start = from + rel;
        let end = start + code.len();
        let bounded = (start == 0 || !bytes[start - 1].is_ascii_digit()) && (end == bytes.len() || !bytes[end].is_ascii_digit());
        if bounded {
            match near {
                None => return true,
                Some((markers, window)) => {
                    let ctx_start = start.saturating_sub(window);
                    let ctx_end = (end + window).min(bytes.len());
                    // Slicing on bytes may cut a multi-byte char at the window edge;
                    // `from_utf8_lossy` tolerates it and the markers are all ASCII.
                    let context = String::from_utf8_lossy(&bytes[ctx_start..ctx_end]);
                    if markers.iter().any(|marker| context.contains(marker)) {
                        return true;
                    }
                }
            }
        }
        from = end;
    }
    false
}


// --------------------------------------------------------------------- caching

/// What is known of one login right now.
#[derive(Debug, Clone, Default)]
pub struct UsageReading {
    /// The windows, each stamped with when it was read.
    pub windows: Vec<QuotaWindow>,
    /// When the newest of them was read.
    pub updated_at: Option<String>,
    /// Where the newest of them came from.
    pub source: Option<QuotaReadingSource>,
    /// The fallback probe's last verdict, when it failed.
    pub error: Option<QuotaProbeError>,
}

impl UsageReading {
    fn from_shared(shared: &readings::SharedReading) -> Self {
        let newest = shared.newest();
        UsageReading {
            windows: shared.windows.iter().map(readings::StoredWindow::to_window).collect(),
            updated_at: newest.map(|w| crate::accounts::rest::iso_ms(w.read_at_ms)),
            source: newest.map(|w| w.source),
            error: shared.probe_error,
        }
    }
}

/// A probe that is already running; awaiting it is free.
type Pending = Shared<BoxFuture<'static, ()>>;

impl Account {
    /// Whether the fallback probe may run for this account at all: an OAuth
    /// account that opted in. A pasted key or an env block has no usage login
    /// to probe.
    fn probeable(&self) -> bool {
        self.usage_enabled && self.kind == AccountKind::OauthToken
    }
}

/// Every login the fallback probe looks after: the user's own, then each
/// probeable account.
fn probe_logins(accounts: &[Account]) -> Vec<UsageLogin> {
    std::iter::once(UsageLogin::Local)
        .chain(accounts.iter().filter(|a| a.probeable()).map(|a| UsageLogin::Account(a.id.clone())))
        .collect()
}

/// `DROPPEN_USAGE_FALLBACK_MS`, or [`readings::DEFAULT_FALLBACK_AFTER`].
fn fallback_after_from_env() -> Duration {
    droppen_base::env::var("DROPPEN_USAGE_FALLBACK_MS")
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
        .unwrap_or(readings::DEFAULT_FALLBACK_AFTER)
}

/// Claude readings per login — a view over the shared files, plus the session
/// readings not yet written to them — and the fallback probe.
pub struct QuotaCache {
    paths: Arc<Paths>,
    /// The CLIs whose probes fill this cache: the readings per login are
    /// claude's, the local reading beside them is codex's.
    logins: Logins,
    /// The fallback probe running for each login, if any.
    in_flight: Mutex<HashMap<UsageLogin, Pending>>,
    /// Codex's own reading, which is asked of a different CLI over a different
    /// protocol but obeys the same three rules and shares this module's budget.
    codex: Arc<CodexQuotaCache>,
    /// Each probe costs seconds of CLI startup, so N accounts must not fire N of
    /// them at once: every probe queues behind the previous one.
    queue: Arc<tokio::sync::Mutex<()>>,
    /// Session readings held between writes of each login's file.
    ///
    /// Also the lock every write of a session reading is made under, so a view
    /// never sees a reading that has left memory and not yet reached the file.
    throttles: Mutex<HashMap<UsageLogin, readings::Throttle>>,
    /// How old a login's newest window may get before it is probed.
    fallback_after: Duration,
}

impl QuotaCache {
    /// An empty cache for one server instance, probing through each CLI's
    /// own login.
    pub fn new(paths: Arc<Paths>, logins: Logins) -> Self {
        Self {
            codex: Arc::new(CodexQuotaCache::new(ProbeRoots::of(&paths), logins.codex.clone())),
            paths,
            logins,
            in_flight: Mutex::new(HashMap::new()),
            queue: Arc::new(tokio::sync::Mutex::new(())),
            throttles: Mutex::new(HashMap::new()),
            fallback_after: fallback_after_from_env(),
        }
    }

    /// The shared file with this instance's unwritten session readings on top.
    fn shared(&self, login: &UsageLogin) -> readings::SharedReading {
        let Some(path) = login.reading_file(&self.paths) else {
            return readings::SharedReading::default();
        };
        let throttles = self.throttles.lock().expect("quota throttles poisoned");
        let mut shared = readings::load(&path);
        if let Some(throttle) = throttles.get(login) {
            shared.merge_stored(throttle.pending());
        }
        shared
    }

    /// What is known of a login right now; never spawns anything, never waits.
    pub fn cached(&self, login: &UsageLogin) -> UsageReading {
        UsageReading::from_shared(&self.shared(login))
    }

    // ------------------------------------------------------ session readings

    /// File the windows of one session's `rate_limit_event` under the login the
    /// session runs on.
    ///
    /// Visible to [`QuotaCache::cached`] at once; written to the shared file at
    /// most once per [`readings::SESSION_FLUSH_EVERY`] per login, the newest
    /// reading winning. Called from a session's driver, so it never waits on
    /// anything but a short file write.
    pub fn ingest_session(self: &Arc<Self>, login: UsageLogin, windows: Vec<QuotaWindow>) {
        self.ingest_session_at(login, windows, now_ms(), Instant::now());
    }

    fn ingest_session_at(self: &Arc<Self>, login: UsageLogin, windows: Vec<QuotaWindow>, at_ms: i64, now: Instant) {
        if windows.is_empty() {
            return;
        }
        let Some(path) = login.reading_file(&self.paths) else { return };
        let stored = windows
            .into_iter()
            .map(|w| readings::StoredWindow::new(w, at_ms, QuotaReadingSource::Session))
            .collect();
        let mut throttles = self.throttles.lock().expect("quota throttles poisoned");
        match throttles.entry(login.clone()).or_default().offer(now, stored) {
            readings::Offer::Write(windows) => Self::write_session(&path, &login, &windows),
            readings::Offer::Hold(Some(after)) => {
                drop(throttles);
                self.schedule_flush(login, after);
            }
            readings::Offer::Hold(None) => {}
        }
    }

    /// Merge session windows into a login's file. A failure is logged and the
    /// windows are gone: the next event, seconds away, carries newer ones.
    fn write_session(path: &Path, login: &UsageLogin, windows: &[readings::StoredWindow]) {
        if let Err(error) = readings::update(path, |shared| shared.merge_stored(windows)) {
            tracing::warn!(login = ?login, %error, "quota: cannot write a session reading");
        }
    }

    /// Write what is held for a login once its interval is up.
    ///
    /// Without a runtime — a synchronous test — nothing is scheduled, and the
    /// reading waits for the next write or the ticker's sweep.
    fn schedule_flush(self: &Arc<Self>, login: UsageLogin, after: Duration) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        let cache = self.clone();
        runtime.spawn(async move {
            tokio::time::sleep(after).await;
            cache.flush(&login, true);
        });
    }

    /// Write what is held for one login: always when `force`, otherwise only
    /// once its interval is up.
    fn flush(&self, login: &UsageLogin, force: bool) {
        let Some(path) = login.reading_file(&self.paths) else { return };
        let mut throttles = self.throttles.lock().expect("quota throttles poisoned");
        let Some(throttle) = throttles.get_mut(login) else { return };
        let now = Instant::now();
        let held = if force { throttle.take(now) } else { throttle.due(now) };
        if let Some(windows) = held {
            Self::write_session(&path, login, &windows);
        }
    }

    /// Write every login's held readings whose interval is up — the ticker's
    /// safety net under the scheduled flushes.
    fn flush_due(&self) {
        let logins: Vec<UsageLogin> = self.throttles.lock().expect("quota throttles poisoned").keys().cloned().collect();
        for login in logins {
            self.flush(&login, false);
        }
    }

    // --------------------------------------------------------- the fallback

    /// Whether this login's newest reading is old enough, and its last probe
    /// long enough ago, for the fallback probe to run.
    fn fallback_due(&self, login: &UsageLogin) -> bool {
        self.shared(login).fallback(now_ms(), self.fallback_after) == readings::Fallback::Due
    }

    /// Start the fallback probe for one login when it is due, and return the run
    /// in flight so a caller can wait for it — or not.
    pub fn fallback(self: &Arc<Self>, login: &UsageLogin) -> Option<Pending> {
        let target = login.probe_target(&self.paths)?;
        let mut in_flight = self.in_flight.lock().expect("quota in-flight poisoned");
        if let Some(running) = in_flight.get(login) {
            return Some(running.clone());
        }
        if !self.fallback_due(login) {
            return None;
        }

        let cache = self.clone();
        let key = login.clone();
        let pending: Pending = async move {
            {
                // Serialized: one CLI startup at a time across every login.
                let _turn = cache.queue.clone().lock_owned().await;
                cache.probe(&key, &target).await;
            }
            cache.in_flight.lock().expect("quota in-flight poisoned").remove(&key);
        }
        .boxed()
        .shared();

        // Drive it even if nobody awaits: a caller that gave up on the budget
        // still wants the reading ready for the next poll.
        tokio::spawn(pending.clone());
        in_flight.insert(login.clone(), pending.clone());
        Some(pending)
    }

    /// Probe one login, if no other process on this machine is probing it.
    async fn probe(&self, login: &UsageLogin, target: &readings::ProbeTarget) {
        // Held for the whole probe: this is what stops two processes refreshing
        // one usage login's token.
        let _lock = match readings::ProbeLock::try_acquire(&target.lock) {
            Ok(Some(lock)) => lock,
            Ok(None) => {
                tracing::debug!(login = ?login, "quota: another process is probing this login; its reading lands in the shared file");
                return;
            }
            Err(error) => {
                tracing::warn!(login = ?login, lock = %target.lock.display(), %error, "quota: cannot take the probe lock; not probing");
                return;
            }
        };
        // Asked again under the lock: another instance may have finished a probe
        // between this one being decided and the lock being free.
        if !self.fallback_due(login) {
            return;
        }
        // The readings per login are claude's: a usage login is a `claude auth
        // login`, and the local one is the user's own `~/.claude`.
        let roots = ProbeRoots::of(&self.paths);
        let Some(run) = self.logins.claude.probe(&roots, target.config_dir.as_deref()) else {
            return;
        };
        let probe = run.await;
        if probe.rate_limited {
            tracing::warn!(
                login = ?login,
                minutes = RATE_LIMIT_BACKOFF.as_secs() / 60,
                "a /usage probe came back rate-limited; backing this login off"
            );
        }
        let at = now_ms();
        if let Err(error) = readings::update(&target.reading_file, |shared| shared.record_probe(probe, at)) {
            tracing::warn!(login = ?login, %error, "quota: cannot record a probe");
        }
    }

    /// Give every login the fallback probe is due for its probe, without
    /// waiting for any of them.
    pub async fn sweep(self: &Arc<Self>, store: &crate::accounts::store::AccountsStore) {
        self.flush_due();
        for login in probe_logins(&store.list().await) {
            let _ = self.fallback(&login);
        }
    }

    /// Wait for cold probes, but never longer than [`COLLECT_BUDGET`].
    ///
    /// A cold panel must not hold the request open for as long as the probes
    /// take: wait briefly, then answer with whatever is cached.
    async fn within_budget(pending: Vec<Pending>) {
        if pending.is_empty() {
            return;
        }
        let all = futures_util::future::join_all(pending);
        let _ = tokio::time::timeout(COLLECT_BUDGET, all).await;
    }

    // ------------------------------------------------------------ the panel

    /// Every account's reading, probed or not; reads only.
    ///
    /// Windows are shown for any account a session has run under — a reading a
    /// session made is as true of an account whose probe is off as of one whose
    /// probe is on. Only the probe's verdict is confined to the accounts it runs
    /// for: an account it does not look after has no verdict to report, even one
    /// left on file from before its probe was switched off.
    fn account_quotas(&self, accounts: &[Account]) -> Vec<AccountQuota> {
        accounts
            .iter()
            .map(|account| {
                let reading = self.cached(&UsageLogin::Account(account.id.clone()));
                AccountQuota {
                    id: account.id.clone(),
                    label: account.label.clone(),
                    kind: account.kind,
                    email: account.email.clone(),
                    usage_enabled: account.usage_enabled,
                    windows: reading.windows,
                    updated_at: Nullable(reading.updated_at),
                    source: Optional(reading.source),
                    error: Optional(reading.error.filter(|_| account.probeable())),
                }
            })
            .collect()
    }

    /// Reads only; call after the probes have had their budget.
    async fn claude_quota(&self) -> Option<ClaudeQuota> {
        let (plan, tier) = match self.logins.claude.local_plan(&self.paths) {
            Some(read) => read.await,
            None => (None, None),
        };
        let reading = self.cached(&UsageLogin::Local);
        if plan.is_none() && reading.windows.is_empty() {
            return None;
        }
        Some(ClaudeQuota {
            plan: Nullable(plan),
            tier: Nullable(tier),
            updated_at: Nullable(reading.updated_at),
            source: Optional(reading.source),
            windows: reading.windows,
        })
    }

    /// The whole panel: the local login, Codex, and every stored account.
    pub async fn collect(self: &Arc<Self>, store: &crate::accounts::store::AccountsStore) -> Quota {
        let accounts = store.list().await;
        // One budget for every probe: the fallback probes that are due and the
        // Codex refresh run together. Codex is in here rather than read inline
        // because it is a CLI spawn too — see [`codex`] — and a spawn on the
        // request path is exactly what this budget exists to bound.
        let mut pending: Vec<Pending> = probe_logins(&accounts).iter().filter_map(|login| self.fallback(login)).collect();
        pending.extend(self.codex.refresh());
        Self::within_budget(pending).await;

        Quota {
            claude: Nullable(self.claude_quota().await),
            codex: Nullable(self.codex.cached()),
            accounts: self.account_quotas(&accounts),
        }
    }
}

// ------------------------------------------------------------- codex's reading

/// One reading, successful or not.
#[derive(Debug, Clone)]
struct CacheEntry {
    /// When the last attempt finished: this is what [`USAGE_TTL`] governs, so
    /// an app server that hangs costs one probe per TTL, not one per request.
    at: Instant,
    /// The last reading actually produced, kept across a later failure. It
    /// carries its own `updatedAt`, so keeping it is honest: the panel shows
    /// the number it was given and the age it was given at.
    quota: Option<CodexQuota>,
}

/// The Codex reading, cached for the process.
///
/// One entry, because Codex has one config root on this machine — unlike the
/// Claude cache next door, which is keyed per login. Concurrent callers
/// collapse onto the one probe in flight.
pub struct CodexQuotaCache {
    probe: ProbeRoots,
    /// Codex's login, which reads the reading.
    login: Arc<dyn Login>,
    entry: Mutex<Option<CacheEntry>>,
    in_flight: Mutex<Option<Pending>>,
}

impl CodexQuotaCache {
    /// An empty cache for one server instance, filled by `login`'s reading
    /// of its own login ([`Login::local_quota`]).
    pub fn new(probe: ProbeRoots, login: Arc<dyn Login>) -> Self {
        Self {
            probe,
            login,
            entry: Mutex::new(None),
            in_flight: Mutex::new(None),
        }
    }

    /// What the cache holds right now; never spawns anything, never waits.
    pub fn cached(&self) -> Option<CodexQuota> {
        self.entry
            .lock()
            .expect("codex quota cache poisoned")
            .as_ref()
            .and_then(|entry| entry.quota.clone())
    }

    /// Start a probe when the reading has aged out, and hand back the run in
    /// flight so a caller can wait for it — or not.
    pub fn refresh(self: &Arc<Self>) -> Option<Pending> {
        if let Some(entry) = self.entry.lock().expect("codex quota cache poisoned").as_ref() {
            if entry.at.elapsed() < USAGE_TTL {
                return None;
            }
        }
        let mut in_flight = self.in_flight.lock().expect("codex quota probe poisoned");
        if let Some(running) = in_flight.as_ref() {
            return Some(running.clone());
        }

        let cache = self.clone();
        let pending: Pending = async move {
            let quota = match cache.login.local_quota(&cache.probe) {
                Some(read) => read.await,
                None => None,
            };
            cache.store(quota);
            *cache.in_flight.lock().expect("codex quota probe poisoned") = None;
        }
        .boxed()
        .shared();

        // Driven even if nobody awaits it: a caller that gave up on the collect
        // budget still wants the reading ready for the next poll.
        tokio::spawn(pending.clone());
        *in_flight = Some(pending.clone());
        Some(pending)
    }

    /// Record a reading, keeping the last one behind a failure.
    fn store(&self, quota: Option<CodexQuota>) {
        let mut entry = self.entry.lock().expect("codex quota cache poisoned");
        let kept = quota.or_else(|| entry.as_ref().and_then(|previous| previous.quota.clone()));
        *entry = Some(CacheEntry {
            at: Instant::now(),
            quota: kept,
        });
    }
}

/// How often the fallback ticker looks for logins nothing has read lately.
pub const FALLBACK_TICK: Duration = Duration::from_secs(60);

/// Probe, every [`FALLBACK_TICK`], the logins no session has read lately.
///
/// Only the instance that owns its lock may start this — it spawns CLIs — which
/// is why [`crate::app`] calls it from inside the lock-gated block. The first
/// sweep waits one tick, so a boot is not also a burst of probes.
pub fn spawn_fallback_ticker(quota: Arc<QuotaCache>, store: Arc<crate::accounts::store::AccountsStore>, cancel: tokio_util::sync::CancellationToken) {
    tokio::spawn(async move {
        let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + FALLBACK_TICK, FALLBACK_TICK);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = ticks.tick() => quota.sweep(&store).await,
            }
        }
        tracing::debug!("quota fallback ticker stopped");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::quota::claude::RateLimitWindows;
    use serde_json::Value;

    #[test]
    fn the_plan_label_joins_the_subscription_and_the_multiplier() {
        assert_eq!(claude_plan_label(Some("max"), Some("default_max_20x")), "Max 20x");
        assert_eq!(claude_plan_label(Some("pro"), None), "Pro");
        assert_eq!(claude_plan_label(None, Some("tier_5x")), "Claude 5x");
        assert_eq!(claude_plan_label(None, None), "Claude");
    }

    const ACCOUNT: &str = "0b6f7a7e-2f7a-4d8e-9d0c-1a2b3c4d5e6f";

    /// A cache over a home of its own, so its shared files are the test's.
    fn cache_in(home: &Path) -> Arc<QuotaCache> {
        Arc::new(QuotaCache::new(Arc::new(Paths::new(home.to_path_buf())), crate::app::logins()))
    }

    fn account(id: &str, kind: AccountKind, usage_enabled: bool) -> Account {
        Account {
            id: id.into(),
            label: "Account".into(),
            kind,
            email: Optional(None),
            usage_enabled,
            created_at: String::new(),
            updated_at: String::new(),
            unverified: Optional(None),
            provider: crate::accounts::store::AccountProvider::Claude,
            env_keys: Optional(None),
            rest: Vec::new(),
            rotation: kind == AccountKind::OauthToken,
        }
    }

    /// The windows of the `allowed` event fixture.
    fn allowed_windows() -> Vec<QuotaWindow> {
        let line: Value = serde_json::from_str(
            r#"{"type":"rate_limit_event","rate_limit_info":{"isUsingOverage":false,"overageDisabledReason":"org_level_disabled","overageStatus":"rejected","rateLimitType":"five_hour","resetsAt":1790436000,"status":"allowed","unifiedWindows":{"five_hour":{"resetsAt":1790436000,"utilization":0.19},"seven_day":{"resetsAt":1790460000,"utilization":0.46}}}}"#,
        )
        .expect("json");
        line["rate_limit_info"].windows_from_rate_limit_info()
    }

    /// The poll budget, stated as the arithmetic that produces it.
    ///
    /// 429 is keyed by account, so the panel is spending the same allowance the
    /// user's real work needs. Codex still reuses a reading for [`USAGE_TTL`],
    /// which caps it at 20 an hour. A Claude login is probed only once its
    /// newest window is older than the fallback threshold — at most four an hour
    /// at the default, machine wide, and none while a session is reading it — and
    /// any failure takes it down to two.
    #[test]
    fn the_poll_budget_is_the_fallback_threshold_and_the_backoffs() {
        assert_eq!(3600 / USAGE_TTL.as_secs(), 20, "Codex: the TTL is the poll budget");
        assert_eq!(
            3600 / readings::DEFAULT_FALLBACK_AFTER.as_secs(),
            4,
            "a Claude login nothing has read lately"
        );
        assert!(
            readings::DEFAULT_FALLBACK_AFTER >= USAGE_TTL * 5,
            "the fallback is far rarer than the old 3-minute cycle"
        );

        // The backoff has to be a real reduction, not a rounding of the same
        // number, or a rate-limited account keeps spending its allowance.
        assert!(
            RATE_LIMIT_BACKOFF > readings::DEFAULT_FALLBACK_AFTER,
            "a 429 must cost more than one skipped poll"
        );
        assert_eq!(3600 / RATE_LIMIT_BACKOFF.as_secs(), 2, "a backed-off login is probed twice an hour");
        assert_eq!(3600 / readings::PROBE_FAILURE_BACKOFF.as_secs(), 2, "and so is one whose probe failed");
    }

    /// A reading is on the panel the moment the event arrives; the file — which
    /// the other instance reads — is written at most once per interval.
    #[test]
    fn a_session_reading_is_seen_at_once_and_written_at_most_once_per_interval() {
        let home = tempfile::tempdir().expect("tempdir");
        let ours = cache_in(home.path());
        let theirs = cache_in(home.path());
        let login = UsageLogin::Account(ACCOUNT.into());
        let start = Instant::now();
        let t0 = 1_790_000_000_000;

        ours.ingest_session_at(login.clone(), allowed_windows(), t0, start);
        let file = login.reading_file(&ours.paths).expect("a file");
        assert!(file.is_file(), "the first reading is written straight away");
        for cache in [&ours, &theirs] {
            let reading = cache.cached(&login);
            assert_eq!(reading.windows.len(), 2);
            assert_eq!(reading.source, Some(QuotaReadingSource::Session));
            assert_eq!(reading.updated_at.as_deref(), Some(crate::accounts::rest::iso_ms(t0).as_str()));
            assert_eq!(reading.windows[0].updated_at.0, reading.updated_at, "each window says when it was read");
        }

        // Five seconds on: held here, visible here, not yet on file.
        let mut newer = allowed_windows();
        newer[0] = window("5h".into(), 300, 25.0, None);
        ours.ingest_session_at(login.clone(), newer, t0 + 5_000, start + Duration::from_secs(5));
        assert_eq!(ours.cached(&login).windows[0].used_percent, 25.0, "this instance shows it at once");
        assert_eq!(theirs.cached(&login).windows[0].used_percent, 19.0, "the file still has the last write");

        // The flush the throttle asked for.
        ours.flush(&login, true);
        assert_eq!(
            theirs.cached(&login).windows[0].used_percent,
            25.0,
            "and now the other instance sees it too"
        );
        assert_eq!(
            theirs.cached(&login).updated_at.as_deref(),
            Some(crate::accounts::rest::iso_ms(t0 + 5_000).as_str())
        );
    }

    /// The user's own login is filed beside the accounts, never in `~/.claude`.
    #[test]
    fn a_local_session_reading_lands_in_the_local_file() {
        let home = tempfile::tempdir().expect("tempdir");
        let cache = cache_in(home.path());
        cache.ingest_session(UsageLogin::Local, allowed_windows());
        assert!(cache.paths.data_dir().join("local-usage.json").is_file());
        assert!(!home.path().join(".claude").exists(), "nothing is written into the user's own root");
        assert_eq!(cache.cached(&UsageLogin::Local).windows.len(), 2);
    }

    /// Every account shows what its sessions read — an OAuth account with its
    /// probe off and an env account included — while only the probeable ones
    /// are ever probed or carry a probe's verdict.
    #[test]
    fn an_account_with_its_probe_off_still_shows_what_its_sessions_read() {
        let home = tempfile::tempdir().expect("tempdir");
        let cache = cache_in(home.path());
        let off = account(ACCOUNT, AccountKind::OauthToken, false);
        let env = account("1b6f7a7e-2f7a-4d8e-9d0c-1a2b3c4d5e6f", AccountKind::Env, false);
        let on = account("2b6f7a7e-2f7a-4d8e-9d0c-1a2b3c4d5e6f", AccountKind::OauthToken, true);
        let accounts = vec![off.clone(), env.clone(), on.clone()];

        cache.ingest_session(UsageLogin::Account(off.id.clone()), allowed_windows());
        cache.ingest_session(UsageLogin::Account(env.id.clone()), allowed_windows());
        // A verdict left on file from when the probe was on is not reported.
        let file = UsageLogin::Account(off.id.clone()).reading_file(&cache.paths).expect("a file");
        readings::update(&file, |shared| {
            shared.record_probe(
                UsageProbe {
                    windows: vec![],
                    error: Some(QuotaProbeError::NotLoggedIn),
                    rate_limited: false,
                },
                now_ms(),
            )
        })
        .expect("written");

        let quotas = cache.account_quotas(&accounts);
        for quota in &quotas[..2] {
            assert_eq!(
                quota.windows.len(),
                2,
                "{}: a session reading is shown whatever the probe setting",
                quota.id
            );
            assert_eq!(quota.source.0, Some(QuotaReadingSource::Session));
            assert!(quota.updated_at.0.is_some());
            assert_eq!(quota.error.0, None, "no probe runs for it, so there is no verdict to show");
        }
        assert!(quotas[2].windows.is_empty(), "nothing has read the third yet");
        assert_eq!(quotas[2].source.0, None);

        // The fallback looks after the local login and the opted-in account only.
        assert_eq!(probe_logins(&accounts), [UsageLogin::Local, UsageLogin::Account(on.id.clone())]);
        assert!(cache.in_flight.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn a_fresh_reading_is_not_re_probed() {
        let home = tempfile::tempdir().expect("tempdir");
        let cache = cache_in(home.path());
        let login = UsageLogin::Account(ACCOUNT.into());
        cache.ingest_session(login.clone(), allowed_windows());
        assert!(cache.fallback(&login).is_none(), "a session read it moments ago");
        assert!(cache.in_flight.lock().expect("lock").is_empty());
    }

    /// The other instance holds the probe lock: this one skips, spawns nothing
    /// and writes nothing, because the other's result will land in the file.
    #[tokio::test]
    async fn a_probe_another_process_is_running_is_skipped() {
        let home = tempfile::tempdir().expect("tempdir");
        let cache = cache_in(home.path());
        let login = UsageLogin::Account(ACCOUNT.into());
        let target = login.probe_target(&cache.paths).expect("a target");
        // Stale, so the probe would otherwise be due.
        readings::update(&target.reading_file, |shared| {
            shared.merge(allowed_windows(), now_ms() - 60 * 60 * 1000, QuotaReadingSource::Session)
        })
        .expect("written");
        assert!(cache.fallback_due(&login));
        let before = std::fs::read(&target.reading_file).expect("file");

        let held = readings::ProbeLock::try_acquire(&target.lock).expect("lockable").expect("free");
        cache.probe(&login, &target).await;
        assert_eq!(
            std::fs::read(&target.reading_file).expect("file"),
            before,
            "no probe ran, so nothing was recorded"
        );
        drop(held);
    }

    /// Under the lock the decision is asked again: a reading that arrived while
    /// this probe queued means there is nothing left to probe for.
    #[tokio::test]
    async fn a_probe_that_finds_a_fresh_reading_under_the_lock_does_not_run() {
        let home = tempfile::tempdir().expect("tempdir");
        let cache = cache_in(home.path());
        let login = UsageLogin::Account(ACCOUNT.into());
        let target = login.probe_target(&cache.paths).expect("a target");
        cache.ingest_session(login.clone(), allowed_windows());
        let before = std::fs::read(&target.reading_file).expect("file");
        cache.probe(&login, &target).await;
        assert_eq!(std::fs::read(&target.reading_file).expect("file"), before);
        assert!(target.lock.exists(), "the lock was taken, and the decision asked again under it");
    }

    #[test]
    fn only_an_unambiguous_429_counts_as_rate_limited() {
        // A 429 as a status code, beside "rate limit" / "http" / the reason phrase.
        assert!(is_rate_limited("API Error: 429 rate limit exceeded"));
        assert!(is_rate_limited("HTTP/1.1 429 Too Many Requests"));
        assert!(is_rate_limited("too many requests, retry after 600"));

        // The false positive that matters: `/usage` is *about* rate limits, and
        // the credentials beside it carry a tier by that name. Neither is a 429.
        assert!(!is_rate_limited(r#"{"rateLimitTier": "default_max_20x"}"#));
        assert!(!is_rate_limited("Current session: 12% used · resets Aug 11, 7:30am (UTC)"));
        assert!(!is_rate_limited("Current week (all models): 40% used"));

        // Incidental 429s that must NOT trip: a standalone token count and a path
        // segment, neither near a status marker...
        assert!(!is_rate_limited("context window: 429 tokens used, resets in 5h"));
        assert!(!is_rate_limited("wrote /home/me/.cache/claude-429/usage.json"));
        // ...and a longer number that merely contains the digits.
        assert!(!is_rate_limited("processed 4290000 tokens across the week"));
    }
}
