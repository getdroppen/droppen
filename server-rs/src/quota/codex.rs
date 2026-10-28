//! Codex's rolling windows, asked of the CLI's own app server.
//!
//! Scraping whichever rollout file has the newest mtime is not the same thing as
//! the newest *reading*: a session opened a fortnight ago and merely touched
//! today wins that comparison, and its last few `token_count` events carry
//! `rate_limits: {primary: null, secondary: null}` — the shape Codex writes when
//! the turn did not come back with fresh numbers. Walking further back until
//! something has numbers, without a floor, then serves whatever it finds as the
//! current reading: a user whose account is fully spent sees a three-day-old
//! "30% left".
//!
//! `codex app-server` answers `account/rateLimits/read` with the account's
//! live windows, which is the only reading on this machine that is *current* by
//! construction. So that is the primary path, and [`CodexQuotaSource`] goes on
//! the wire beside it the way [`crate::models::ModelListSource`] does, because
//! the log scrape is still the fallback and a guess shown as a fact is the bug
//! this module exists to prevent.
//!
//! # Cost
//!
//! An app-server spawn is ~0.8 s, where a file read is microseconds, so it
//! obeys the three rules the parent module's header sets out: one probe per
//! [`crate::quota::USAGE_TTL`], **failures cached too**, one probe in flight at
//! a time, and a request waits only inside the collect budget it already had.
//!
//! # Security
//!
//! The reply carries account state — a credit balance, reset grants, a plan.
//! Only the two windows, the plan label and the one "ordinary usage is blocked"
//! bit are read; nothing else is stored, logged or quoted, not even in an error
//! path. The failure path says the probe answered nothing, never *what* it
//! answered.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use futures_util::future::{BoxFuture, FutureExt, Shared};
use gotcha::Schematic;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};

use super::{window, QuotaWindow, USAGE_TTL};
use crate::openapi_ext::Nullable;
use crate::process_env::{apply_env, clean_env_bare};
use crate::record::replay::parse_iso8601;
use crate::sessions::util::{read_tail_lines, try_parse};
use crate::sessions::{iso8601, Paths};

/// The one question this module asks. It needs no thread, no turn and no
/// workspace: the handshake and this call are the whole conversation.
const RATE_LIMITS: &str = "account/rateLimits/read";
/// How long the app server gets to finish that conversation before it is killed.
///
/// The live CLI answers in ~0.8 s including its own startup, so this is a
/// ceiling on the one probe that finds the cache cold rather than a budget
/// anything is expected to spend. It is deliberately longer than
/// [`crate::quota::COLLECT_BUDGET`]: a request never waits for the whole of it,
/// but a probe that needs six seconds should still land in the cache for the
/// next poll instead of being thrown away.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the child gets to exit on its own after its stdin closes.
///
/// Worth waiting for rather than killing outright: an app server that ends
/// normally removes its own scratch directory under `~/.codex/tmp/arg0/`, and a
/// SIGKILL leaves it behind — one per probe, for ever. Measured: a clean exit
/// takes ~25 ms, and the directory count is unchanged across probes.
const EXIT_GRACE: Duration = Duration::from_secs(2);
/// A ceiling on what is read from the child, so an app server that streams
/// something unexpected cannot grow this process's memory.
const MAX_PROBE_OUTPUT: u64 = 1024 * 1024;

/// How much of a rollout file's tail the fallback reads. The `token_count`
/// events are at the end; the giant `session_meta` line is at the head.
const LOG_TAIL_BYTES: u64 = 128 * 1024;
/// How many of the newest rollout files the fallback looks at.
///
/// More than one because mtime does not order *readings*: the file touched most
/// recently may be an old session resumed for one turn, while a younger session
/// holds the newer numbers. Five is enough to cover the handful of sessions a
/// person has open at once and still bounded at five tail reads.
const LOG_FILES: usize = 5;
/// How old a log-derived reading may be before it is discarded outright.
///
/// A rolling-window percentage only describes the window it was read in. The
/// shortest window Codex reports is five hours, so a reading from more than a
/// day ago is certainly about a *different* five-hour window, and the weekly
/// one has moved on by a seventh of its length — far past the point where the
/// number answers "how much have I got left". Twenty-four hours rather than
/// five is a deliberately generous floor: this is a guard against serving
/// nonsense, not a freshness policy (the UI's own staleness hint fires after
/// ten minutes). Past it the honest answer is no reading at all.
const LOG_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// A window named by its own length: `5h`, `weekly`, `daily`…
///
/// Both readings describe their windows in minutes and nothing else, so the
/// label is derived rather than reported — which is also why the same weekly
/// window reads the same whether it arrived as the RPC's `primary` or the log's
/// `secondary`.
fn window_label(minutes: i64) -> String {
    match minutes {
        m if m <= 0 => "window".to_string(),
        m if m < 60 => format!("{m}m"),
        1440 => "daily".to_string(),
        10080 => "weekly".to_string(),
        m if m < 1440 => format!("{}h", (m as f64 / 60.0).round() as i64),
        m => format!("{}d", (m as f64 / 1440.0).round() as i64),
    }
}

// ----------------------------------------------------------------- the shapes

/// Where a Codex reading came from.
///
/// On the wire, for the same reason [`crate::models::ModelListSource`] is: the
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

// ------------------------------------------------------- the app-server reply

/// Reading the app server's `account/rateLimits/read` answer.
///
/// The field names are the RPC's own — `usedPercent`, `windowDurationMins`,
/// `resetsAt` — which are *not* the rollout file's `used_percent`,
/// `window_minutes`, `resets_at`. Both spellings are parsed in this module and
/// neither parser accepts the other's, deliberately: a silent rename is how a
/// reading turns into an empty window list.
trait CodexReply {
    /// The windows, plan and blocked bit of one result, or `None` when it names
    /// no window at all: an app server that answered something unreadable is
    /// not a reading, and the caller falls back.
    fn rate_limits_reading(&self, now: SystemTime) -> Option<CodexQuota>;

    /// Whether ordinary usage is blocked, from the two fields that say so.
    ///
    /// `ordinaryUsageAllowed` is the verdict — it is the flag the CLI itself
    /// gates on — and `rateLimitReachedType` is the cross-check, present beside
    /// it in the live reply as `"rate_limit_reached"`. Either one alone is
    /// taken at its word; together they are or'd, because both spellings mean
    /// "the account is out" and a disagreement is not a reason to tell the user
    /// they are fine.
    ///
    /// `None` is unknown: a reply carrying neither field says nothing about
    /// this, and inventing `false` for it is the same class of lie as a stale
    /// percentage.
    fn blocked(&self, limits: &Value) -> Option<bool>;
}

impl CodexReply for Value {
    fn rate_limits_reading(&self, now: SystemTime) -> Option<CodexQuota> {
        let limits = self.get("rateLimits")?;
        let windows: Vec<QuotaWindow> = [limits.get("primary"), limits.get("secondary")]
            .into_iter()
            .filter_map(|raw| raw.rpc_window())
            .collect();
        if windows.is_empty() {
            return None;
        }
        Some(CodexQuota {
            plan: Nullable(
                limits
                    .get("planType")
                    .and_then(Value::as_str)
                    .filter(|plan| !plan.is_empty())
                    .map(str::to_owned),
            ),
            // The reading is live by construction, so it is stamped now.
            // Anything else here would be the stale reading this module exists
            // to prevent.
            updated_at: Nullable(Some(iso8601(now))),
            windows,
            source: CodexQuotaSource::AppServer,
            blocked: Nullable(self.blocked(limits)),
        })
    }

    fn blocked(&self, limits: &Value) -> Option<bool> {
        let denied = self.get("ordinaryUsageAllowed").and_then(Value::as_bool).map(|allowed| !allowed);
        let reached = limits
            .get("rateLimitReachedType")
            .and_then(Value::as_str)
            .map(|kind| !matches!(kind.trim(), "" | "none" | "not_reached"));
        match (denied, reached) {
            (Some(denied), Some(reached)) => Some(denied || reached),
            (denied, reached) => denied.or(reached),
        }
    }
}

/// Reading one window object, in either of the two spellings.
trait WindowValue {
    /// One window of the RPC reply, or `None` for the `null` a plan with a
    /// single window puts in `secondary`.
    fn rpc_window(&self) -> Option<QuotaWindow>;

    /// One `token_count` line's window, in the rollout file's own spelling.
    fn log_window(&self) -> Option<QuotaWindow>;

    /// A unix-seconds reset instant as ISO-8601.
    ///
    /// `Duration::from_secs_f64` panics on a negative, NaN or overflowing
    /// value, and neither an RPC reply nor a rollout file is trusted input.
    fn resets_at(&self) -> Option<String>;
}

impl WindowValue for Option<&Value> {
    fn rpc_window(&self) -> Option<QuotaWindow> {
        let object = (*self)?.as_object()?;
        let used = object.get("usedPercent")?.as_f64()?;
        let minutes = object.get("windowDurationMins").and_then(Value::as_i64).unwrap_or(0);
        Some(window(window_label(minutes), minutes, used, object.get("resetsAt").resets_at()))
    }

    fn log_window(&self) -> Option<QuotaWindow> {
        let object = (*self)?.as_object()?;
        let used = object.get("used_percent")?.as_f64()?;
        let minutes = object.get("window_minutes").and_then(Value::as_i64).unwrap_or(0);
        Some(window(window_label(minutes), minutes, used, object.get("resets_at").resets_at()))
    }

    fn resets_at(&self) -> Option<String> {
        self.and_then(Value::as_f64)
            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0 && *seconds < 1e12)
            .map(|seconds| iso8601(std::time::UNIX_EPOCH + Duration::from_secs_f64(seconds)))
    }
}

// ------------------------------------------------------------------ the probe

impl Paths {
    /// Start a `codex app-server`, ask it the one question, and take the windows
    /// out of its answer.
    ///
    /// A short-lived child of this module's own rather than the session client in
    /// [`crate::inject::app_server`]: that one is one app server *per session* over
    /// the restart-safe shim, and it exists to keep a turn alive across a server
    /// restart — it claims the session lock, writes a run journal record and leaves
    /// shim sidecars on disk. None of that belongs to a read that needs no thread
    /// at all. This is the [`crate::models::probe`] shape instead: timeout,
    /// `kill_on_drop`, an explicit close, cached and TTL'd, fallback on any failure.
    ///
    /// `None` for every failure there is — no CLI, one that will not start, one
    /// that never answers, an answer that is not JSON, an error reply, a reply with
    /// no windows in it. The caller falls back to the session logs, so none of them
    /// need telling apart.
    async fn probe_app_server(&self) -> Option<CodexQuota> {
    // The user's own home, exactly as the Claude probes are spawned: the app
    // server reads the login this machine's `codex` would use, which is the
    // login whose windows the panel is about.
    let reading = app_server_call(Path::new("codex"), &self.home, &clean_env_bare(), RATE_LIMITS, PROBE_TIMEOUT)
        .await
        .and_then(|result| result.rate_limits_reading(SystemTime::now()));
    if reading.is_none() {
        tracing::debug!("the codex app server answered no rate limits; falling back to the session logs");
    }
        reading
    }
}

/// One question put to a short-lived `codex app-server`: the handshake, one
/// request with no parameters, and the `result` of its answer.
///
/// Shared by the quota probe above and by the first-run guide's account check
/// ([`crate::setup`], `account/read`), so there is one copy of the handshake
/// and of the shutdown that keeps `~/.codex/tmp/arg0/` clean. `program` is
/// `codex` for the probe and the binary the guide found for the guide;
/// `timeout` bounds the conversation, not the exit grace after it.
///
/// `None` for every failure, as [`Paths::probe_app_server`] describes, and an `error`
/// reply is one of them: its message describes the account and is not ours to
/// quote (see the module's security note).
pub(crate) async fn app_server_call(program: &Path, cwd: &Path, env: &crate::process_env::Env, method: &str, timeout: Duration) -> Option<Value> {
    let mut command = tokio::process::Command::new(program);
    command
        .arg("app-server")
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Nothing here reads stderr, and an unread pipe that fills would wedge
        // the child against the timeout instead of an answer.
        .stderr(Stdio::null())
        // The belt to the explicit close's braces: a future that is dropped
        // rather than awaited must not leave an app server running.
        .kill_on_drop(true);
    apply_env(&mut command, env);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            tracing::debug!(%error, method, "no codex app server to ask");
            return None;
        }
    };
    let asked = match (child.stdin.take(), child.stdout.take()) {
        (Some(stdin), Some(stdout)) => tokio::time::timeout(timeout, ask(stdin, stdout, method)).await.ok().flatten(),
        _ => None,
    };
    // `ask` owns the stdin and drops it on the way out — on the timeout path
    // too, where the whole future is dropped — and that EOF is the app server's
    // shutdown signal. Waiting for it is what keeps `~/.codex/tmp/arg0/` from
    // filling up; the kill is only for a child that ignores the EOF.
    if tokio::time::timeout(EXIT_GRACE, child.wait()).await.is_err() {
        let _ = child.kill().await;
    }
    asked
}

/// The whole conversation: handshake, one request, one answer.
///
/// `initialized` is sent only after `initialize` has been answered, which is
/// the order [`crate::inject::app_server`] uses against the same protocol.
async fn ask(mut stdin: ChildStdin, stdout: ChildStdout, method: &str) -> Option<Value> {
    let mut lines = BufReader::new(stdout.take(MAX_PROBE_OUTPUT)).lines();
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "clientInfo": {"name": "remote_hand", "title": "remote-hand", "version": env!("CARGO_PKG_VERSION")},
                "capabilities": {"experimentalApi": true},
            },
        }),
    )
    .await?;
    reply(&mut lines, 1).await?;

    send(&mut stdin, json!({"jsonrpc": "2.0", "method": "initialized", "params": {}})).await?;
    send(&mut stdin, json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": {}})).await?;
    let mut answer = reply(&mut lines, 2).await?;
    // An `error` member is a refusal; its message describes this account and is
    // not ours to quote. See the module's security note.
    if answer.get("error").is_some() {
        tracing::debug!(method, "the codex app server refused the request");
        return None;
    }
    answer.get_mut("result").map(Value::take)
}

/// One NDJSON line to the child.
async fn send(stdin: &mut ChildStdin, message: Value) -> Option<()> {
    stdin.write_all(message.to_string().as_bytes()).await.ok()?;
    stdin.write_all(b"\n").await.ok()?;
    stdin.flush().await.ok()
}

/// Read until the answer to `id` arrives. The app server is chatty and not all
/// of it is JSON, so everything that is not this answer is skipped rather than
/// treated as a failure.
async fn reply<R: tokio::io::AsyncBufRead + Unpin>(lines: &mut tokio::io::Lines<R>, id: u64) -> Option<Value> {
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
        if message.get("id").and_then(Value::as_u64) == Some(id) {
            return Some(message);
        }
    }
    None
}

// --------------------------------------------------------------- the fallback

/// The newest usable reading in one rollout file, with the epoch millis it was
/// written at, or `None` when the file has none inside [`LOG_MAX_AGE`].
///
/// The walk is backwards from the newest line and **stops** at the first entry
/// older than `floor_ms`. That bound is what keeps a reading from another week
/// out of today's panel: without it the loop walks past every
/// `rate_limits: {primary: null}` event until it finds numbers, however old.
/// Lines are chronological, so one entry below the floor means every remaining
/// entry is too.
async fn file_reading(path: &Path, floor_ms: i64) -> Option<(i64, CodexQuota)> {
    let lines = read_tail_lines(path, LOG_TAIL_BYTES).await.ok()?;
    for line in lines.iter().rev() {
        let Some(entry) = try_parse(line) else { continue };
        // An entry with no readable timestamp cannot be dated, so it cannot be
        // vouched for either — it is skipped rather than trusted.
        let Some(stamp) = entry.get("timestamp").and_then(Value::as_str) else {
            continue;
        };
        let Some(at) = parse_iso8601(stamp) else { continue };
        if at < floor_ms {
            break;
        }
        if entry.get("type").and_then(Value::as_str) != Some("event_msg") {
            continue;
        }
        let Some(payload) = entry.get("payload").and_then(Value::as_object) else {
            continue;
        };
        if payload.get("type").and_then(Value::as_str) != Some("token_count") {
            continue;
        }
        let Some(limits) = payload.get("rate_limits") else { continue };
        let windows: Vec<QuotaWindow> = [limits.get("primary"), limits.get("secondary")]
            .into_iter()
            .filter_map(|raw| raw.log_window())
            .collect();
        if windows.is_empty() {
            continue;
        }
        return Some((
            at,
            CodexQuota {
                plan: Nullable(limits.get("plan_type").and_then(Value::as_str).map(str::to_owned)),
                updated_at: Nullable(Some(stamp.to_string())),
                windows,
                source: CodexQuotaSource::SessionLog,
                blocked: Nullable(None),
            },
        ));
    }
    None
}

impl Paths {
    /// The newest rollout files under the date-partitioned sessions tree, freshest
    /// mtime first.
    async fn newest_codex_files(&self, keep: usize) -> Vec<PathBuf> {
    let mut found: Vec<(PathBuf, f64)> = Vec::new();
    let mut stack = vec![self.codex_sessions()];
    while let Some(dir) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else { continue };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let Ok(file_type) = entry.file_type().await else { continue };
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            // Unreadable file: skip.
            let Ok(metadata) = entry.metadata().await else { continue };
            found.push((path, crate::sessions::mtime_ms(&metadata)));
        }
    }
    found.sort_by(|(_, left), (_, right)| right.partial_cmp(left).unwrap_or(std::cmp::Ordering::Equal));
    found.truncate(keep);
        found.into_iter().map(|(path, _)| path).collect()
    }
}

impl Paths {
    /// The newest reading the session logs can still vouch for.
    ///
    /// Newest by *timestamp* across the [`LOG_FILES`] newest files, not the newest
    /// file's own answer: mtime orders when a file was last touched, which is not
    /// when its numbers were read. Resuming a fortnight-old session for one turn
    /// makes it the newest file on disk while a younger session holds the newer
    /// windows, and picking by mtime alone is half of the bug this module prevents.
    async fn log_reading(&self, now: SystemTime) -> Option<CodexQuota> {
    let floor_ms = now
        .checked_sub(LOG_MAX_AGE)
        .and_then(|floor| floor.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_millis() as i64)
        .unwrap_or(0);
    let mut best: Option<(i64, CodexQuota)> = None;
    for path in self.newest_codex_files(LOG_FILES).await {
        let Some((at, quota)) = file_reading(&path, floor_ms).await else {
            continue;
        };
        if best.as_ref().is_none_or(|(best, _)| at > *best) {
            best = Some((at, quota));
        }
    }
        best.map(|(_, quota)| quota)
    }
}

// ------------------------------------------------------------------ the cache

/// A probe that is already running; awaiting it is free.
type Pending = Shared<BoxFuture<'static, ()>>;

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
    paths: Arc<Paths>,
    entry: Mutex<Option<CacheEntry>>,
    in_flight: Mutex<Option<Pending>>,
}

impl CodexQuotaCache {
    /// An empty cache for one server instance.
    pub fn new(paths: Arc<Paths>) -> Self {
        Self {
            paths,
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
            let quota = cache.paths.probe_codex().await;
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

impl Paths {
    /// The live reading if the app server can be reached, the session logs if
    /// not.
    async fn probe_codex(&self) -> Option<CodexQuota> {
        if let Some(live) = self.probe_app_server().await {
            return Some(live);
        }
        self.log_reading(SystemTime::now()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `2026-09-20T12:00:00Z` — a fixed "now" for the readings below.
    const NOW_MS: u64 = 1_790_078_400_000;

    fn now() -> SystemTime {
        std::time::UNIX_EPOCH + Duration::from_millis(NOW_MS)
    }

    /// A real `codex app-server` reply from `2026-09-20`: fully spent, blocked,
    /// one window.
    fn live_reply() -> Value {
        json!({
            "ordinaryUsageAllowed": false,
            "rateLimits": {
                "limitId": "codex",
                "limitName": null,
                "normalModelSlug": null,
                "primary": {"usedPercent": 100, "windowDurationMins": 10080, "resetsAt": 1_790_128_010i64},
                "secondary": null,
                "credits": {"hasCredits": false, "unlimited": false, "balance": "0E-10"},
                "individualLimit": null,
                "spendControlReached": false,
                "planType": "pro",
                "rateLimitReachedType": "rate_limit_reached"
            },
            "rateLimitsByLimitId": {"codex": {"planType": "pro"}},
            "rateLimitResetCredits": {"availableCount": 2, "credits": []}
        })
    }

    #[test]
    fn codex_windows_are_named_by_their_length() {
        assert_eq!(window_label(0), "window");
        assert_eq!(window_label(30), "30m");
        assert_eq!(window_label(300), "5h");
        assert_eq!(window_label(1440), "daily");
        assert_eq!(window_label(10080), "weekly");
        assert_eq!(window_label(4320), "3d");
    }

    #[test]
    fn the_live_reply_reads_as_a_spent_weekly_window() {
        let quota = live_reply().rate_limits_reading(now()).expect("a reading");

        assert_eq!(quota.source, CodexQuotaSource::AppServer);
        assert_eq!(quota.plan, Nullable(Some("pro".to_string())));
        assert_eq!(quota.windows.len(), 1, "secondary is null, so there is one window: {:?}", quota.windows);
        assert_eq!(quota.windows[0].label, "weekly");
        assert_eq!(quota.windows[0].window_minutes, 10080);
        assert_eq!(quota.windows[0].used_percent, 100.0);
        assert_eq!(quota.windows[0].remaining_percent, 0.0, "the whole point: nothing left");
        assert_eq!(quota.windows[0].resets_at.as_ref().map(String::as_str), Some("2026-09-23T01:46:50.000Z"));
        assert_eq!(quota.blocked, Nullable(Some(true)), "and the panel can say the account is out");
        // Live by construction, so it is stamped now rather than with anything
        // the reply carried.
        assert_eq!(quota.updated_at, Nullable(Some(iso8601(now()))));
    }

    /// The RPC spells its fields differently from the rollout file, and reading
    /// one with the other's parser is how a reply becomes "no windows".
    #[test]
    fn the_rpc_spelling_and_the_log_spelling_are_not_interchangeable() {
        let rpc = json!({"usedPercent": 40.0, "windowDurationMins": 300, "resetsAt": 1_722_470_400i64});
        let log = json!({"used_percent": 40.0, "window_minutes": 300, "resets_at": 1_722_470_400i64});

        let from_rpc = Some(&rpc).rpc_window().expect("an rpc window");
        assert_eq!(from_rpc.label, "5h");
        assert_eq!(from_rpc.remaining_percent, 60.0);
        assert_eq!(from_rpc.resets_at.as_ref().map(String::as_str), Some("2024-08-01T00:00:00.000Z"));
        assert_eq!(Some(&log).log_window(), Some(from_rpc), "the same window, in the log's spelling");

        assert!(Some(&log).rpc_window().is_none(), "the log spelling is not an rpc window");
        assert!(Some(&rpc).log_window().is_none(), "and the rpc spelling is not a log one");
        assert!(None.rpc_window().is_none(), "a null secondary is not a window");
        assert!(
            Some(&json!({"windowDurationMins": 300})).rpc_window().is_none(),
            "no percentage, no window"
        );
    }

    #[test]
    fn a_plan_with_two_windows_keeps_both() {
        let quota = json!({
            "ordinaryUsageAllowed": true,
            "rateLimits": {
                "primary": {"usedPercent": 12.0, "windowDurationMins": 300},
                "secondary": {"usedPercent": 83.0, "windowDurationMins": 10080, "resetsAt": 1_722_470_400i64},
                "planType": "plus",
            }
        })
        .rate_limits_reading(now())
        .expect("a reading");
        let labels: Vec<&str> = quota.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["5h", "weekly"]);
        assert_eq!(quota.blocked, Nullable(Some(false)), "allowed, and the panel may say so");
    }

    #[test]
    fn a_reply_with_nothing_usable_in_it_is_no_reading() {
        for reply in [
            json!({}),
            json!({"rateLimits": {"primary": null, "secondary": null}}),
            json!({"rateLimits": {"primary": {"windowDurationMins": 300}}}),
            json!({"rateLimits": {"primary": {"used_percent": 50.0, "window_minutes": 300}}}),
        ] {
            assert!(reply.rate_limits_reading(now()).is_none(), "{reply} must not answer");
        }
    }

    /// `blocked` is a verdict with three values, and the third is "nobody said".
    #[test]
    fn blocked_is_unknown_until_something_says_otherwise() {
        let of = |reply: Value| {
            let limits = reply.get("rateLimits").cloned().unwrap_or(json!({}));
            reply.blocked(&limits)
        };
        assert_eq!(of(json!({"ordinaryUsageAllowed": false})), Some(true));
        assert_eq!(of(json!({"ordinaryUsageAllowed": true})), Some(false));
        assert_eq!(of(json!({"rateLimits": {"rateLimitReachedType": "rate_limit_reached"}})), Some(true));
        assert_eq!(of(json!({"rateLimits": {"rateLimitReachedType": "none"}})), Some(false));
        assert_eq!(of(json!({"rateLimits": {"limitId": "codex"}})), None, "neither field: unknown, not fine");
        // Disagreement resolves towards "you are out": both spellings mean the
        // same thing, and the costly mistake is telling a spent account it is
        // fine.
        assert_eq!(
            of(json!({"ordinaryUsageAllowed": true, "rateLimits": {"rateLimitReachedType": "rate_limit_reached"}})),
            Some(true)
        );
    }

    // ------------------------------------------------------------ the fallback

    fn log_line(stamp: &str, limits: Value) -> String {
        json!({"type": "event_msg", "timestamp": stamp, "payload": {"type": "token_count", "rate_limits": limits}}).to_string()
    }

    fn stamp(ms_ago: u64) -> String {
        iso8601(now() - Duration::from_millis(ms_ago))
    }

    async fn sessions(lines: &[(&str, String)]) -> (tempfile::TempDir, Paths) {
        let home = tempfile::tempdir().expect("tempdir");
        let dir = home.path().join(".codex/sessions/2026/09/20");
        tokio::fs::create_dir_all(&dir).await.expect("sessions");
        for (name, body) in lines {
            tokio::fs::write(dir.join(name), body).await.expect("rollout");
            // mtime order is the order they are written in, and the fallback
            // must not depend on it.
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let paths = Paths::new(home.path().to_path_buf());
        (home, paths)
    }

    /// The file with the newest mtime is an old session whose recent entries
    /// carry no numbers, and a younger file holds the reading that is actually
    /// current.
    #[tokio::test]
    async fn the_newest_reading_wins_over_the_newest_file() {
        let (_home, paths) = sessions(&[
            (
                "younger-reading.jsonl",
                format!(
                    "{}\n",
                    log_line(
                        &stamp(60 * 60 * 1000),
                        json!({"plan_type": "pro", "primary": {"used_percent": 100.0, "window_minutes": 10080}})
                    )
                ),
            ),
            (
                "touched-today.jsonl",
                format!(
                    "{}\n{}\n",
                    log_line(
                        &stamp(6 * 60 * 60 * 1000),
                        json!({"plan_type": "pro", "primary": {"used_percent": 70.0, "window_minutes": 10080}})
                    ),
                    // The shape that would make a reader walk backwards for ever.
                    log_line(&stamp(30 * 60 * 1000), json!({"plan_type": "pro", "primary": null, "secondary": null})),
                ),
            ),
        ])
        .await;

        let quota = paths.log_reading(now()).await.expect("a reading");
        assert_eq!(quota.source, CodexQuotaSource::SessionLog);
        assert_eq!(quota.windows[0].used_percent, 100.0, "the newest numbers, not the newest file's");
        assert_eq!(quota.updated_at, Nullable(Some(stamp(60 * 60 * 1000))));
    }

    /// The bound: a reading nothing has refreshed inside a day is not a reading
    /// of *this* window, and the honest answer is none at all.
    #[tokio::test]
    async fn a_reading_older_than_the_cutoff_is_not_served() {
        let just_inside = 23 * 60 * 60 * 1000;
        let just_outside = 25 * 60 * 60 * 1000;
        let limits = json!({"plan_type": "pro", "primary": {"used_percent": 30.0, "window_minutes": 10080}});

        let (_home, paths) = sessions(&[("old.jsonl", format!("{}\n", log_line(&stamp(just_outside), limits.clone())))]).await;
        assert!(paths.log_reading(now()).await.is_none(), "three days of drift must not read as current");

        let (_home, paths) = sessions(&[("recent.jsonl", format!("{}\n", log_line(&stamp(just_inside), limits)))]).await;
        assert!(
            paths.log_reading(now()).await.is_some(),
            "and the cutoff is a floor, not a freshness rule"
        );
    }

    /// The log form still parses, windows and all: it is the fallback path.
    #[tokio::test]
    async fn the_log_form_still_parses_both_windows() {
        let (_home, paths) = sessions(&[(
            "rollout.jsonl",
            format!(
                "{}\n{}\n",
                json!({"type": "session_meta", "timestamp": stamp(60_000), "payload": {}}),
                log_line(
                    &stamp(60_000),
                    json!({
                        "plan_type": "pro",
                        "primary": {"used_percent": 25.0, "window_minutes": 300, "resets_at": 1_722_470_400i64},
                        "secondary": {"used_percent": 60.0, "window_minutes": 10080},
                    })
                ),
            ),
        )])
        .await;

        let quota = paths.log_reading(now()).await.expect("a reading");
        assert_eq!(quota.plan, Nullable(Some("pro".to_string())));
        assert_eq!(quota.source, CodexQuotaSource::SessionLog);
        assert_eq!(quota.blocked, Nullable(None), "the logs never say whether usage is blocked");
        assert_eq!(quota.windows.len(), 2);
        assert_eq!(quota.windows[0].label, "5h");
        assert_eq!(quota.windows[0].remaining_percent, 75.0);
        assert_eq!(quota.windows[0].resets_at.as_ref().map(String::as_str), Some("2024-08-01T00:00:00.000Z"));
        assert_eq!(quota.windows[1].label, "weekly");
        assert_eq!(quota.windows[1].resets_at, Nullable(None));
        assert_eq!(quota.updated_at, Nullable(Some(stamp(60_000))), "the log's own stamp, not now");
    }

    #[tokio::test]
    async fn no_sessions_at_all_is_no_reading() {
        let home = tempfile::tempdir().expect("tempdir");
        assert!(Paths::new(home.path().to_path_buf()).log_reading(now()).await.is_none());
    }

    // --------------------------------------------------------------- the cache

    fn cache() -> (tempfile::TempDir, Arc<CodexQuotaCache>) {
        let home = tempfile::tempdir().expect("tempdir");
        let cache = Arc::new(CodexQuotaCache::new(Arc::new(Paths::new(home.path().to_path_buf()))));
        (home, cache)
    }

    #[tokio::test]
    async fn a_fresh_reading_is_not_re_probed_and_a_failure_keeps_the_last_one() {
        let (_home, cache) = cache();
        let good = live_reply().rate_limits_reading(now()).expect("a reading");

        // Planted rather than probed: nothing is spawned in a unit test.
        cache.store(Some(good.clone()));
        assert_eq!(cache.cached(), Some(good.clone()));
        assert!(cache.refresh().is_none(), "inside the TTL there is nothing to do");
        assert!(cache.in_flight.lock().expect("lock").is_none());

        cache.store(None);
        assert_eq!(cache.cached(), Some(good), "a failed probe must not blank the panel");
        assert!(cache.refresh().is_none(), "and a failure is cached too, so it costs one probe per TTL");
    }

    #[tokio::test]
    async fn an_unprobed_cache_holds_nothing() {
        let (_home, cache) = cache();
        assert_eq!(cache.cached(), None);
    }

    /// The end-to-end check, against the `codex` actually installed here.
    ///
    /// Ignored by default: it spawns a real CLI and depends on a real login, so
    /// it is a thing to run by hand — `cargo test --lib quota -- --ignored
    /// --nocapture` — not a thing CI can hold anybody to.
    #[tokio::test]
    #[ignore = "spawns the real codex CLI"]
    async fn the_installed_app_server_answers_with_live_windows() {
        let paths = Paths::new(dirs_home());
        let quota = paths.probe_app_server().await.expect("the app server answered");
        println!("{quota:?}");
        assert_eq!(quota.source, CodexQuotaSource::AppServer);
        assert!(!quota.windows.is_empty());
    }

    #[cfg(test)]
    fn dirs_home() -> PathBuf {
        PathBuf::from(std::env::var("HOME").expect("HOME"))
    }
}
