//! The Claude model list, asked of the CLI over its control protocol.
//!
//! The CLI does publish what *this account* may run, but not as a file: a
//! `claude` started in stream-json mode answers a `control_request` of subtype
//! `initialize` with, among other things, the models the picker in the CLI's own
//! UI offers. That is the only list on this machine that is account-scoped and
//! current — the static aliases in the parent module are a guess about tiers,
//! and the usage history remote-hand used to read for concrete ids is a file the
//! CLI no longer writes.
//!
//! Asking costs a CLI startup, so it is asked rarely: one probe per
//! [`MODELS_TTL`] **per config root**, concurrent callers share the one in
//! flight, and the last good answer outlives a failure. A request never pays for
//! more than one probe, and never waits at all once anything has been read.
//!
//! Per config root, because the answer is per *login*: a session bound to a
//! stored account runs under that account's root, and the models it is entitled
//! to are not the ones the user's own `~/.claude` is entitled to. Probing one
//! root and showing the answer to every session is how a picker offers a model
//! the session cannot run and hides one it can.
//!
//! # Security
//!
//! The same `initialize` response also carries an `account` object — the signed
//! in email, organisation and plan. None of it is read past the one `models`
//! lookup below: it is not stored, not logged, and not quoted in any error. The
//! probe's failure path says "the probe answered nothing", never *what* it
//! answered, for exactly that reason.
//!
//! # Stability
//!
//! The control protocol is the CLI's own, undocumented and unversioned, so this
//! is written to degrade rather than to trust: anything unexpected — a missing
//! key, an error subtype, a CLI that is not installed, one that never answers —
//! ends as `None` and the caller falls back to the alias list and reports
//! [`ModelListSource::Fallback`], which the picker already shows as a warning.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::future::{BoxFuture, FutureExt, Shared};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};

use super::{claude_fallback, ModelInfo, ModelListSource, CLAUDE_DEFAULT_EFFORT};
use crate::openapi_ext::Nullable;
use crate::process_env::{apply_env, claude_env};
use crate::sessions::Paths;

/// A CLI that speaks stream-json on both sides, which is the only mode that
/// takes a control request — and the invocation the Agent SDK itself uses.
///
/// `--print` is **not** redundant here, however obviously a handshake that never
/// sends a prompt is not printing anything. It is what makes
/// `--no-session-persistence` apply at all: the CLI's own help says that flag
/// "only works with --print", and without it every probe leaves an empty
/// directory behind in `~/.claude/session-env/`. At one spawn per
/// [`MODELS_TTL`] per server that is ~100 directories a day, for ever. Measured,
/// not reasoned: without `--print` the count went up by one per probe, with it
/// by none, and the control response is identical either way.
const PROBE_ARGS: [&str; 7] = [
    "--print",
    "--no-session-persistence",
    "--output-format",
    "stream-json",
    "--verbose",
    "--input-format",
    "stream-json",
];
/// The one line written to the child. The id is arbitrary and only ever matched
/// by the CLI echoing it back, which this does not check: one request is in
/// flight, so the first control response is the answer to it.
const INITIALIZE: &str = r#"{"type":"control_request","request_id":"remote-hand-models","request":{"subtype":"initialize"}}"#;
/// How long the child gets to answer before it is killed.
///
/// The real CLI answers in ~300 ms, so this is generous by an order of
/// magnitude; it is a ceiling on the one request that finds the cache cold, not
/// a budget anything is expected to spend.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long an answer — or a failure — is reused for.
///
/// The list changes when Anthropic ships a model or the account's plan changes,
/// neither of which is a thing a picker has to notice within the minute. Half an
/// hour is one CLI startup per browser session rather than one per page load.
pub const MODELS_TTL: Duration = Duration::from_secs(30 * 60);
/// A ceiling on what is read from the child before giving up, so a CLI that
/// streams something unexpected cannot grow this process's memory.
const MAX_PROBE_OUTPUT: u64 = 4 * 1024 * 1024;
/// The alias the CLI offers for "whatever this account should use by default".
const DEFAULT_ALIAS: &str = "default";

// ---------------------------------------------------------------- the protocol

/// The models named by one `control_response` line, or `None` when the line does
/// not carry any.
///
/// Both nestings are accepted: the live CLI wraps the payload as
/// `response.response`, and a flatter `response` is what the same message would
/// look like if that ever changed.
///
/// **Only `models` is read.** See the module's security note: the sibling
/// `account` object never leaves this function.
fn control_response_models(msg: &Value) -> Option<Vec<ModelInfo>> {
    let response = msg.get("response")?;
    // The CLI marks a refused request with a subtype; only a success carries a
    // payload, and an error's `error` string is not ours to quote.
    if response.get("subtype").and_then(Value::as_str).is_some_and(|kind| kind != "success") {
        return None;
    }
    let payload = response.get("response").unwrap_or(response);
    let rows: Vec<ModelInfo> = payload.get("models")?.as_array()?.iter().filter_map(model_row).collect();
    if rows.is_empty() {
        return None;
    }
    Some(drop_redundant_default(rows))
}

/// One published model as the picker takes it.
///
/// The **alias** is kept as the id, not the concrete model it currently resolves
/// to: `opus[1m]` tracks whatever the newest 1M-context opus is, and pinning the
/// resolved id here would freeze the picker to today's model until this cache
/// expired. The concrete id becomes the *label* instead, because reading which
/// model an alias currently means is the whole reason to ask the CLI at all.
fn model_row(entry: &Value) -> Option<ModelInfo> {
    let entry = entry.as_object()?;
    let id = entry.get("value").and_then(Value::as_str).filter(|value| !value.is_empty())?;
    let resolved = entry.get("resolvedModel").and_then(Value::as_str).filter(|resolved| !resolved.is_empty());
    let efforts = published_efforts(entry);
    // The protocol publishes no default rung, so this is the same one the static
    // list uses — and only where there is a ladder to pick it from.
    let default_effort = efforts
        .as_ref()
        .is_some_and(|ladder| !ladder.is_empty())
        .then(|| CLAUDE_DEFAULT_EFFORT.to_string());
    Some(ModelInfo {
        id: id.to_string(),
        label: Nullable(resolved.map(str::to_owned)),
        efforts: Nullable(efforts),
        default_effort: Nullable(default_effort),
    })
}

/// The effort ladder a published entry describes.
///
/// `[]` and `None` are different answers and the picker reads them differently:
/// `[]` hides the effort control outright, `None` means "this list says nothing"
/// and leaves the CLI's whole ladder on offer. So the empty list is only
/// returned where the payload *states* it — `supportsEffort: false`, or the
/// shape the haiku row actually has, which is neither key at all — and anything
/// ambiguous (an entry that claims efforts but names none) stays unknown.
fn published_efforts(entry: &serde_json::Map<String, Value>) -> Option<Vec<String>> {
    let listed = entry.get("supportedEffortLevels");
    if let Some(levels) = listed.and_then(Value::as_array) {
        let ladder: Vec<String> = levels.iter().filter_map(Value::as_str).map(str::to_owned).collect();
        if !ladder.is_empty() {
            return Some(ladder);
        }
    }
    match entry.get("supportsEffort").and_then(Value::as_bool) {
        Some(false) => Some(Vec::new()),
        None if listed.is_none() => Some(Vec::new()),
        _ => None,
    }
}

/// Drop the `default` row when another row already names the same model.
///
/// The CLI lists `default` beside the alias it currently resolves to, which
/// under labels that *are* the resolved id is two rows reading
/// `claude-opus-5[1m]`. The named alias wins: it says which model it is, while
/// `default` only says the account has not chosen. A `default` that resolves to
/// something no other row names is kept — then it is the only way to reach that
/// model.
fn drop_redundant_default(rows: Vec<ModelInfo>) -> Vec<ModelInfo> {
    let named: HashSet<&str> = rows
        .iter()
        .filter(|row| row.id != DEFAULT_ALIAS)
        .filter_map(|row| row.label.0.as_deref())
        .collect();
    let redundant: Vec<bool> = rows
        .iter()
        .map(|row| row.id == DEFAULT_ALIAS && row.label.0.as_deref().is_some_and(|label| named.contains(label)))
        .collect();
    rows.into_iter().zip(redundant).filter(|(_, drop)| !drop).map(|(row, _)| row).collect()
}

// ------------------------------------------------------------------- the probe

/// Start a `claude`, ask it once, and take the model list out of its answer.
///
/// `config_dir` `None` means the user's own login (whatever the CLI picks up by
/// itself); anything else is an account's probe directory, asked for exactly the
/// way [`crate::sessions::Paths::probe_claude_usage`] asks it — `CLAUDE_CONFIG_DIR` set
/// *and* the directory as the cwd, so the child adopts no project of the user's.
/// Without the variable the child falls back to `$HOME/.claude` and answers for
/// the wrong login, which is the whole bug this argument exists for.
///
/// `None` for every failure there is: no CLI, a CLI that will not start, one
/// that never answers, an answer that is not JSON, an answer with no models in
/// it. The caller's fallback is the same list a machine with no CLI at all gets,
/// so none of these need telling apart.
///
/// An account's directory is a usage login with a rotating refresh token, and
/// this CLI is as able to refresh it as the quota probe is — two processes
/// doing so at once is how such a login gets revoked. So it takes the same
/// machine-wide lock ([`crate::quota::readings::ProbeLock`]) for as long as the
/// child lives, and a login somebody else is probing answers `None` rather
/// than racing them for the token.
async fn probe_claude_models(paths: &Paths, config_dir: Option<&Path>) -> Option<Vec<ModelInfo>> {
    use crate::quota::readings::{probe_lock_path, ProbeLock};
    let _lock = match config_dir {
        Some(dir) => match ProbeLock::try_acquire(&probe_lock_path(dir)) {
            Ok(Some(lock)) => Some(lock),
            Ok(None) => {
                tracing::debug!("another process is probing this usage login; the picker keeps what it had");
                return None;
            }
            Err(error) => {
                tracing::debug!(%error, "cannot take this usage login's probe lock; not probing it");
                return None;
            }
        },
        None => None,
    };
    let mut command = tokio::process::Command::new("claude");
    command
        .args(PROBE_ARGS)
        .current_dir(config_dir.map(Path::to_path_buf).unwrap_or_else(|| paths.home.clone()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Nothing here reads stderr, and an unread pipe that fills would wedge
        // the child against a timeout instead of an answer.
        .stderr(Stdio::null())
        // The belt to the explicit kill's braces: a future that is dropped
        // rather than awaited must not leave a CLI running.
        .kill_on_drop(true);
    // `claude_env` is what strips `CLAUDECODE` and the rest of the harness
    // variables. Without that the CLI refuses to start at all inside a Claude
    // session, which is exactly where a remote-hand server often runs. It also
    // strips the inherited `CLAUDE_CONFIG_DIR`, so an account's root has to be
    // put back explicitly — the same two-armed match the usage probe uses.
    let env = match config_dir {
        Some(dir) => claude_env([
            ("CLAUDE_CONFIG_DIR".to_string(), dir.to_string_lossy().into_owned()),
            ("TZ".to_string(), "UTC".to_string()),
        ]),
        None => claude_env([("TZ", "UTC")]),
    };
    apply_env(&mut command, &env);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            tracing::debug!(%error, "no claude to ask for a model list");
            return None;
        }
    };
    let asked = match (child.stdin.take(), child.stdout.take()) {
        (Some(stdin), Some(stdout)) => tokio::time::timeout(PROBE_TIMEOUT, ask(stdin, stdout)).await.ok().flatten(),
        _ => None,
    };
    // The CLI has no reason to exit on its own: it is waiting for a message
    // nobody is going to send it.
    let _ = child.kill().await;
    if asked.is_none() {
        tracing::debug!("the claude model probe answered nothing; the picker falls back to the aliases");
    }
    asked
}

/// Write the one request and read until something answers it.
///
/// `stdin` is held for the whole read rather than dropped after the write: a CLI
/// whose input closes may decide the conversation is over before it has replied.
async fn ask(mut stdin: ChildStdin, stdout: ChildStdout) -> Option<Vec<ModelInfo>> {
    stdin.write_all(INITIALIZE.as_bytes()).await.ok()?;
    stdin.write_all(b"\n").await.ok()?;
    stdin.flush().await.ok()?;

    let mut lines = BufReader::new(stdout.take(MAX_PROBE_OUTPUT)).lines();
    // The handshake is chatty and not all of it is JSON, so everything that is
    // not the answer is skipped rather than treated as a failure.
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        if msg.get("type").and_then(Value::as_str) != Some("control_response") {
            continue;
        }
        return control_response_models(&msg);
    }
    None
}

// ------------------------------------------------------------------- the cache

/// A probe that is already running; awaiting it is free.
type Pending = Shared<BoxFuture<'static, ()>>;

/// One reading, successful or not.
#[derive(Debug, Clone)]
struct CacheEntry {
    /// When the last attempt finished: this is what [`MODELS_TTL`] governs, so a
    /// CLI that fails costs one probe per TTL rather than one per request.
    at: Instant,
    /// The last list actually published, kept across a later failure.
    models: Option<Vec<ModelInfo>>,
}

/// The Claude model lists, cached for the process — one per config root.
///
/// Keyed by the config **directory**, exactly as [`crate::quota::QuotaCache`] is
/// keyed, so each login gets its own list, its own TTL and its own in-flight
/// dedup. The key is a path and never a credential: see the module's security
/// note, and keep it that way.
///
/// The cost is one CLI startup per root per [`MODELS_TTL`] rather than one per
/// server, and only for roots something actually asks about — a root nobody
/// opens a picker for is never probed.
pub struct ClaudeModelCache {
    paths: Arc<Paths>,
    entries: Mutex<HashMap<String, CacheEntry>>,
    in_flight: Mutex<HashMap<String, Pending>>,
}

/// `None` is the user's own login; the same key [`crate::quota`] spells `"local"`.
fn cache_key(config_dir: Option<&Path>) -> String {
    config_dir
        .map(|dir| dir.to_string_lossy().into_owned())
        .unwrap_or_else(|| "local".to_string())
}

impl ClaudeModelCache {
    /// An empty cache for one server instance.
    pub fn new(paths: Arc<Paths>) -> Self {
        Self {
            paths,
            entries: Mutex::new(HashMap::new()),
            in_flight: Mutex::new(HashMap::new()),
        }
    }

    /// The list to show for one config root, and where it came from.
    ///
    /// Only the very first call *for that root* can wait, and only for as long
    /// as [`PROBE_TIMEOUT`] allows: a cold cache that answered with the aliases
    /// would show `(latest)` guesses on the one page load the user is most
    /// likely to be looking at. Every call after that reads the cached list and
    /// lets an expired one refresh behind it.
    pub async fn models(self: &Arc<Self>, config_dir: Option<&Path>) -> (Vec<ModelInfo>, ModelListSource) {
        let key = cache_key(config_dir);
        let pending = self.refresh(config_dir);
        if !self.entries.lock().expect("claude model cache poisoned").contains_key(&key) {
            if let Some(pending) = pending {
                pending.await;
            }
        }
        let cached = self
            .entries
            .lock()
            .expect("claude model cache poisoned")
            .get(&key)
            .and_then(|entry| entry.models.clone());
        match cached {
            Some(models) if !models.is_empty() => (models, ModelListSource::Cache),
            _ => (claude_fallback(), ModelListSource::Fallback),
        }
    }

    /// Start a probe when this root's reading has aged out, and hand back the
    /// run in flight so a caller can wait for it — or not.
    ///
    /// Concurrent callers collapse onto one child *per root*: N tabs opening the
    /// composer of one account at once is one CLI startup, not N.
    fn refresh(self: &Arc<Self>, config_dir: Option<&Path>) -> Option<Pending> {
        let key = cache_key(config_dir);
        if let Some(entry) = self.entries.lock().expect("claude model cache poisoned").get(&key) {
            if entry.at.elapsed() < MODELS_TTL {
                return None;
            }
        }
        let mut in_flight = self.in_flight.lock().expect("claude model probe poisoned");
        if let Some(running) = in_flight.get(&key) {
            return Some(running.clone());
        }

        let cache = self.clone();
        let dir: Option<PathBuf> = config_dir.map(Path::to_path_buf);
        let probe_key = key.clone();
        let pending: Pending = async move {
            let models = probe_claude_models(&cache.paths, dir.as_deref()).await;
            cache.store(&probe_key, models);
            cache.in_flight.lock().expect("claude model probe poisoned").remove(&probe_key);
        }
        .boxed()
        .shared();

        // Driven even if nobody awaits it: a caller that already had a list
        // still wants the fresh one ready for the next request.
        tokio::spawn(pending.clone());
        in_flight.insert(key, pending.clone());
        Some(pending)
    }

    /// Record a reading for one root, keeping that root's last good list behind
    /// a failure. One key is never written from another's probe.
    fn store(&self, key: &str, models: Option<Vec<ModelInfo>>) {
        let mut entries = self.entries.lock().expect("claude model cache poisoned");
        let kept = models.or_else(|| entries.get(key).and_then(|previous| previous.models.clone()));
        entries.insert(
            key.to_string(),
            CacheEntry {
                at: Instant::now(),
                models: kept,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The `initialize` response of the live CLI, verbatim but for the `account`
    /// object — which is present in the real payload and is deliberately *not*
    /// reproduced here, because nothing in this module may read it.
    fn live_response() -> Value {
        json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": "remote-hand-models",
                "response": {
                    "models": [
                        {
                            "value": "default",
                            "resolvedModel": "claude-opus-5[1m]",
                            "displayName": "Default (recommended)",
                            "description": "Opus 5 with 1M context · Best for everyday, complex tasks · $5/$25 per Mtok",
                            "supportsEffort": true,
                            "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"],
                            "supportsAdaptiveThinking": true,
                            "supportsFastMode": true,
                            "supportsAutoMode": true
                        },
                        {
                            "value": "opus[1m]",
                            "resolvedModel": "claude-opus-5[1m]",
                            "displayName": "Opus (1M context)",
                            "description": "Opus 5 with 1M context · Best for everyday, complex tasks · $5/$25 per Mtok",
                            "supportsEffort": true,
                            "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"],
                            "supportsAdaptiveThinking": true,
                            "supportsFastMode": true,
                            "supportsAutoMode": true
                        },
                        {
                            "value": "sonnet",
                            "resolvedModel": "claude-sonnet-5",
                            "displayName": "Sonnet",
                            "description": "Sonnet 5 · Best for daily use · $3/$15 per Mtok",
                            "supportsEffort": true,
                            "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"],
                            "supportsAdaptiveThinking": true,
                            "supportsFastMode": true,
                            "supportsAutoMode": true
                        },
                        {
                            "value": "haiku",
                            "resolvedModel": "claude-haiku-4-5-20251001",
                            "displayName": "Haiku",
                            "description": "Haiku 4.5 · Fastest for quick answers · $1/$5 per Mtok"
                        }
                    ],
                    "account": { "email": "nobody@example.com", "organization": "an org nothing may read" },
                    "pid": 1234
                }
            }
        })
    }

    #[test]
    fn the_live_response_lists_the_concrete_model_each_alias_resolves_to() {
        let models = control_response_models(&live_response()).expect("a model list");

        let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, vec!["opus[1m]", "sonnet", "haiku"], "the alias is the id, and default collapses");
        assert_eq!(
            models[0].label,
            Nullable(Some("claude-opus-5[1m]".to_string())),
            "the label is the concrete id, which is the point of asking"
        );
        assert_eq!(models[1].label, Nullable(Some("claude-sonnet-5".to_string())));
        assert_eq!(
            models[0].efforts,
            Nullable(Some(vec![
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
                "xhigh".to_string(),
                "max".to_string()
            ]))
        );
        assert_eq!(models[0].default_effort, Nullable(Some("medium".to_string())));
    }

    /// The haiku row carries neither effort key, which is the payload stating
    /// that the model takes no `--effort` — not that it failed to mention one.
    #[test]
    fn a_row_with_no_effort_keys_at_all_takes_no_effort() {
        let models = control_response_models(&live_response()).expect("a model list");
        let haiku = models.iter().find(|model| model.id == "haiku").expect("the haiku row");
        assert_eq!(haiku.efforts, Nullable(Some(Vec::new())), "an empty ladder, not an unknown one");
        assert_eq!(haiku.default_effort, Nullable(None), "and so no rung to default to");
    }

    #[test]
    fn a_row_that_refuses_efforts_outright_is_read_the_same_way() {
        let models = control_response_models(&json!({
            "response": { "response": { "models": [
                { "value": "haiku", "resolvedModel": "claude-haiku-4-5", "supportsEffort": false },
                { "value": "mystery", "resolvedModel": "claude-mystery-9", "supportsEffort": true }
            ] } }
        }))
        .expect("a model list");
        assert_eq!(models[0].efforts, Nullable(Some(Vec::new())), "supportsEffort: false is an empty ladder");
        assert_eq!(
            models[1].efforts,
            Nullable(None),
            "efforts claimed but never named is unknown, which leaves the whole ladder on offer"
        );
        assert_eq!(models[1].default_effort, Nullable(None), "and nothing to default to either");
    }

    /// Dropping `default` is about the label collision, not about the alias: one
    /// that names a model nothing else names is the only way to reach it.
    #[test]
    fn a_default_that_names_a_model_of_its_own_is_kept() {
        let models = control_response_models(&json!({
            "response": { "response": { "models": [
                { "value": "default", "resolvedModel": "claude-opus-5" },
                { "value": "sonnet", "resolvedModel": "claude-sonnet-5" }
            ] } }
        }))
        .expect("a model list");
        let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, vec!["default", "sonnet"], "and the CLI's own order is kept");
    }

    #[test]
    fn a_response_with_nothing_usable_in_it_is_no_answer() {
        for msg in [
            json!({ "type": "control_response" }),
            json!({ "response": {} }),
            json!({ "response": { "response": { "models": [] } } }),
            json!({ "response": { "response": { "models": "not a list" } } }),
            json!({ "response": { "subtype": "error", "error": "no", "response": { "models": [{ "value": "opus" }] } } }),
            json!({ "response": { "response": { "models": [{ "displayName": "no value" }, "not an object"] } } }),
        ] {
            assert!(control_response_models(&msg).is_none(), "{msg} must not answer");
        }
    }

    /// The flatter shape, in case the wrapping the live CLI uses ever goes.
    #[test]
    fn the_payload_is_found_whether_or_not_it_is_wrapped_twice() {
        let models = control_response_models(&json!({
            "response": { "subtype": "success", "models": [{ "value": "opus", "resolvedModel": "claude-opus-5" }] }
        }))
        .expect("a model list");
        assert_eq!(models[0].id, "opus");
        assert_eq!(models[0].label, Nullable(Some("claude-opus-5".to_string())));
    }

    fn cache() -> (tempfile::TempDir, Arc<ClaudeModelCache>) {
        let home = tempfile::tempdir().expect("tempdir");
        let cache = Arc::new(ClaudeModelCache::new(Arc::new(Paths::new(home.path().to_path_buf()))));
        (home, cache)
    }

    /// A probe that cannot run at all leaves the picker on the static list, and
    /// says so — the one thing that must never happen is an empty picker.
    #[tokio::test]
    async fn a_failed_probe_falls_back_to_the_aliases() {
        let (_home, cache) = cache();
        // Nothing is spawned: the entry is planted, so `refresh` is inside its
        // TTL and the failure path is what answers.
        cache.store(&cache_key(None), None);

        let (models, source) = cache.models(None).await;
        assert_eq!(models, claude_fallback());
        assert_eq!(source, ModelListSource::Fallback, "and the picker is told it is a guess");
    }

    /// A CLI that is briefly unaskable must not empty a picker that was answered
    /// half an hour ago.
    #[tokio::test]
    async fn a_failure_keeps_the_last_good_list() {
        let (_home, cache) = cache();
        let good = control_response_models(&live_response()).expect("a model list");
        cache.store(&cache_key(None), Some(good.clone()));
        cache.store(&cache_key(None), None);

        let (models, source) = cache.models(None).await;
        assert_eq!(models, good);
        assert_eq!(source, ModelListSource::Cache);
    }

    /// The whole point of the key: two logins, two lists, and neither one is
    /// ever answered with the other's.
    ///
    /// The lists here are the shape the defect has in the wild — a root entitled
    /// to a model the other is not — so a cache that shared one entry would fail
    /// this exactly as the picker failed the user.
    #[tokio::test]
    async fn two_config_dirs_keep_two_lists() {
        let (_home, cache) = cache();
        let local = control_response_models(&live_response()).expect("a model list");
        let managed = control_response_models(&json!({
            "response": { "response": { "models": [{ "value": "sonnet", "resolvedModel": "claude-sonnet-5" }] } }
        }))
        .expect("a model list");
        let account = Path::new("/tmp/does-not-need-to-exist/accounts/an-account/claude");

        cache.store(&cache_key(None), Some(local.clone()));
        cache.store(&cache_key(Some(account)), Some(managed.clone()));

        assert_eq!(cache.models(None).await, (local.clone(), ModelListSource::Cache));
        assert_eq!(cache.models(Some(account)).await, (managed.clone(), ModelListSource::Cache));

        // And a failure on one root does not reach into the other's entry.
        cache.store(&cache_key(Some(account)), None);
        assert_eq!(
            cache.models(Some(account)).await,
            (managed, ModelListSource::Cache),
            "its own last good list"
        );
        assert_eq!(
            cache.models(None).await,
            (local, ModelListSource::Cache),
            "and the local one is untouched"
        );
    }

    /// A root nobody has asked about is not answered out of another root's
    /// entry: it falls back, which is what tells the picker the list is a guess.
    #[tokio::test]
    async fn an_unprobed_dir_does_not_borrow_another_dirs_list() {
        let (_home, cache) = cache();
        let good = control_response_models(&live_response()).expect("a model list");
        cache.store(&cache_key(None), Some(good));

        // Planted rather than probed: an entry that exists and holds nothing is
        // what a root whose CLI failed looks like, and no spawn happens.
        let account = Path::new("/tmp/does-not-need-to-exist/accounts/another-account/claude");
        cache.store(&cache_key(Some(account)), None);

        let (models, source) = cache.models(Some(account)).await;
        assert_eq!(models, claude_fallback());
        assert_eq!(source, ModelListSource::Fallback);
    }

    /// The key is the directory and nothing else — no token, no email, no
    /// account object from the probe's own answer ever goes near it.
    #[test]
    fn the_key_is_the_directory() {
        assert_eq!(cache_key(None), "local");
        assert_eq!(cache_key(Some(Path::new("/data/accounts/abc/claude"))), "/data/accounts/abc/claude");
        assert_ne!(
            cache_key(Some(Path::new("/data/accounts/abc/claude"))),
            cache_key(Some(Path::new("/data/accounts/def/claude")))
        );
    }

    /// The end-to-end check, against the `claude` actually installed here.
    ///
    /// Ignored by default: it spawns a real CLI and depends on a real login, so
    /// it is a thing to run by hand — `cargo test --lib models -- --ignored
    /// --nocapture` — not a thing CI can hold anybody to.
    #[tokio::test]
    #[ignore = "spawns the real claude CLI"]
    async fn the_installed_cli_publishes_concrete_ids() {
        let (_home, cache) = cache();
        let (models, source) = cache.models(None).await;
        for model in &models {
            println!("{} -> {:?} efforts={:?}", model.id, model.label.0, model.efforts.0);
        }
        assert_eq!(source, ModelListSource::Cache, "the CLI answered");
        assert!(
            models
                .iter()
                .any(|model| model.label.0.as_deref().is_some_and(|label| label.starts_with("claude-"))),
            "at least one row is labelled with the concrete model it resolves to"
        );
    }
}
