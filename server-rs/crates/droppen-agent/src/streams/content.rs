//! What the agent's streams say: the bodies of records and pages, cut from the
//! same things the live API serves — the session list's rows, the feed's
//! normalised transcript items — so a browser reading them renders exactly
//! what it would have been sent live. Pure functions; the sync task feeds
//! them and the writer seals what they return.
//!
//! | stream | body |
//! |---|---|
//! | index | `{"reset"?: true, "upserts": [SessionSummary…], "removed": ["source/id"…], "asOf"}`, or just `{"asOf"}` ([`as_of_body`]) |
//! | status | `{"asOf", "displayName", "mkEpoch", "settings", "accounts", "quota", "cli", "os", "agentVersion", "alarms"?, "changes"?, "controllers"}` ([`StatusBody`]), whole each time |
//! | session | `{"fromIndex", "items", "endIndex", "fp", "prefixFp"}`, replace-from; a trimmed copy's first page adds `"base"`, `"before"` ([`window_body`]) |
//!
//! Every body fits one record ([`BODY_BUDGET`]); a transcript item too big for
//! one on its own is cut down (its longest strings halved until it fits) and
//! marked `"truncated": true, "rhOfflineTruncated": true` — the offline copy is
//! for reading back, and the machine keeps the original.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::source::{Fingerprints, Items, SessionRow};
use crate::state::{Alarm, Controllers};
use crate::system::Os;

/// The most a body may be: under a record's and a page's largest body
/// (`rh_proto::stream::MAX_PAGE_BODY`, a few hundred bytes more), with room for
/// the envelope around the items.
pub(crate) const BODY_BUDGET: usize = 60_000;
/// How many fingerprints behind its end a session cursor keeps: how far back
/// an in-place rewrite (a tool result attaching to its call) can reach and
/// still be sent as a delta rather than a rebuild.
pub(crate) const CURSOR_FPS: usize = 8;
/// The most items an offline copy of a session holds: the newest ones, past
/// this. About the same as 1024 full pages would allow, far more than anything
/// reads at once.
pub(crate) const MAX_ITEMS: usize = 20_000;

/// Which sessions are copied to the hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Scope {
    /// Only sessions updated in this many days.
    pub days: u64,
    /// Also sessions this server only found (the user's own `~/.claude`).
    pub external: bool,
}

impl Default for Scope {
    fn default() -> Self {
        Self { days: 30, external: false }
    }
}

impl Scope {
    /// From the environment: `DROPPEN_AGENT_SYNC_DAYS`, `DROPPEN_AGENT_SYNC_EXTERNAL=1`.
    pub(crate) fn from_env() -> Self {
        let mut scope = Self::default();
        if let Some(days) = droppen_base::env::var("DROPPEN_AGENT_SYNC_DAYS").and_then(|d| d.parse().ok()) {
            scope.days = days;
        }
        scope.external = droppen_base::env::var("DROPPEN_AGENT_SYNC_EXTERNAL").as_deref() == Some("1");
        scope
    }

    /// Whether `row` is copied: its own (unless external ones are asked for)
    /// and updated recently enough. A row whose time does not parse is not.
    pub(crate) fn admits<R: SessionRow + ?Sized>(&self, row: &R, now_ms: u64) -> bool {
        if !row.owned() && !self.external {
            return false;
        }
        let Some(updated) = droppen_base::time::parse_iso(row.updated_at()) else {
            return false;
        };
        let updated_ms = updated.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        now_ms.saturating_sub(updated_ms) <= self.days * 24 * 60 * 60 * 1000
    }
}

// ------------------------------------------------------------------ sessions

/// Where a session stream stands in its transcript.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SessionCursor {
    /// The first item the copy holds (0 unless a long session was capped).
    #[serde(default)]
    pub start: usize,
    /// How many items, from 0, the copy holds through.
    pub end: usize,
    /// The fingerprints of `[0, i)` for the last few `i` up to `end`.
    pub fps: Vec<(usize, String)>,
    /// The transcript file's signature when the cursor was taken.
    #[serde(default)]
    pub sig: String,
}

impl SessionCursor {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }

    pub(crate) fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// The cursor of a copy holding `[start, end)` of a transcript with `chain`.
    pub(crate) fn at(start: usize, end: usize, chain: &dyn Fingerprints, sig: &str) -> Self {
        let from = end.saturating_sub(CURSOR_FPS).max(start);
        Self {
            start,
            end,
            fps: (from..=end).filter_map(|i| chain.at(i).map(|fp| (i, fp))).collect(),
            sig: sig.to_string(),
        }
    }

    /// The fingerprint at `end`.
    pub(crate) fn end_fp(&self) -> Option<&str> {
        self.fps.iter().find(|(i, _)| *i == self.end).map(|(_, fp)| fp.as_str())
    }
}

/// What to send for a session, given what the copy holds and the transcript now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delta {
    /// Nothing changed.
    None,
    /// Replace from this item to the end.
    From(usize),
    /// What the copy holds is no longer a prefix of the transcript (rewritten,
    /// replaced, adopted): rebuild it.
    Rebuild,
}

impl SessionCursor {
    /// The newest position whose fingerprint the copy and the transcript agree
    /// on decides: from there to the end is the delta.
    pub(crate) fn delta(&self, chain: &dyn Fingerprints) -> Delta {
        let len = chain.len().saturating_sub(1);
        for (at, fp) in self.fps.iter().rev() {
            if chain.at(*at).is_some_and(|ours| ours == *fp) {
                if *at == self.end && len == self.end {
                    return Delta::None;
                }
                return Delta::From(*at);
            }
        }
        Delta::Rebuild
    }
}

/// Cut `item` (an item's JSON) down to `max` bytes: halve its longest strings
/// until it fits, and say so on it.
pub(crate) fn fit_item(item: Value, max: usize) -> Value {
    let len = |v: &Value| serde_json::to_vec(v).map_or(usize::MAX, |b| b.len());
    if len(&item) <= max {
        return item;
    }
    let mut item = item;
    if let Value::Object(map) = &mut item {
        map.insert("truncated".into(), Value::Bool(true));
        map.insert("rhOfflineTruncated".into(), Value::Bool(true));
    }
    for _ in 0..64 {
        if len(&item) <= max {
            return item;
        }
        let Some(longest) = longest_string(&mut item) else { break };
        let keep = longest.len() / 2;
        let cut = (0..=keep).rev().find(|i| longest.is_char_boundary(*i)).unwrap_or(0);
        longest.truncate(cut);
        longest.push_str("\n… [cut short in the offline copy]");
    }
    if len(&item) <= max {
        return item;
    }
    let stand_in = StandIn {
        role: item.get("role").cloned().unwrap_or(Value::String("system".into())),
        kind: item.get("kind").cloned().unwrap_or(Value::String("text".into())),
        text: "[too large for the offline copy]",
        detail: (),
        tool_name: (),
        meta: (),
        is_error: false,
        question: (),
        timestamp: item.get("timestamp").cloned().unwrap_or(Value::Null),
        truncated: true,
        rh_offline_truncated: true,
    };
    serde_json::to_value(stand_in).unwrap_or(Value::Null)
}

/// What an offline copy holds in place of an item no halving brings under its
/// limit: an item of the same role, kind and time that says so. The unit
/// fields are written as `null`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StandIn {
    role: Value,
    kind: Value,
    text: &'static str,
    detail: (),
    tool_name: (),
    meta: (),
    is_error: bool,
    question: (),
    timestamp: Value,
    truncated: bool,
    rh_offline_truncated: bool,
}

fn longest_string(v: &mut Value) -> Option<&mut String> {
    fn walk<'a>(v: &'a mut Value, best: &mut Option<&'a mut String>) {
        match v {
            Value::String(s) => {
                if best.as_ref().is_none_or(|b| s.len() > b.len()) {
                    *best = Some(s);
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| walk(x, best)),
            Value::Object(o) => o.values_mut().for_each(|x| walk(x, best)),
            _ => {}
        }
    }
    let mut best = None;
    walk(v, &mut best);
    best.filter(|s| s.len() > 64)
}

/// One session body: items `[from, to)` (already serialised, in order).
fn session_body(from: usize, items: &[String], chain: &dyn Fingerprints) -> Vec<u8> {
    let to = from + items.len();
    let fp = chain.at(to).unwrap_or_default();
    let prefix = chain.resume(to).unwrap_or_default();
    let mut out = format!("{{\"fromIndex\":{from},\"items\":[");
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(item);
    }
    out.push_str(&format!("],\"endIndex\":{to},\"fp\":\"{fp}\",\"prefixFp\":\"{prefix}\"}}"));
    out.into_bytes()
}

/// Items `[from, to)` of `items` as bodies, each under [`BODY_BUDGET`], with
/// the position each one ends at.
pub(crate) fn session_bodies(items: &dyn Items, from: usize, to: usize, chain: &dyn Fingerprints) -> Vec<(usize, Vec<u8>)> {
    const ENVELOPE: usize = 200;
    let item_max = BODY_BUDGET - ENVELOPE;
    let mut out = Vec::new();
    let mut batch: Vec<String> = Vec::new();
    let mut batch_from = from;
    let mut size = ENVELOPE;
    for i in (0..items.len()).take(to).skip(from) {
        let value = items.json(i);
        let text = serde_json::to_string(&fit_item(value, item_max)).unwrap_or_else(|_| "null".into());
        if !batch.is_empty() && size + text.len() + 1 > BODY_BUDGET {
            let end = batch_from + batch.len();
            out.push((end, session_body(batch_from, &batch, chain)));
            batch.clear();
            batch_from = i;
            size = ENVELOPE;
        }
        size += text.len() + 1;
        batch.push(text);
    }
    if !batch.is_empty() || out.is_empty() {
        let end = batch_from + batch.len();
        out.push((end, session_body(batch_from, &batch, chain)));
    }
    out
}

/// Where a long session's copy starts: the newest [`MAX_ITEMS`].
pub(crate) fn session_start(len: usize) -> usize {
    len.saturating_sub(MAX_ITEMS)
}

/// Each item's time (ms since the epoch), `None` where it has none that parses.
pub(crate) fn item_times(items: &dyn Items) -> Vec<Option<u64>> {
    (0..items.len())
        .map(|i| {
            items
                .timestamp(i)
                .and_then(droppen_base::time::parse_iso)
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
        })
        .collect()
}

/// The window-start page of a session copy the retention window trimmed
/// (relay-and-plans §3.4): the copy holds items from `base` on, and the
/// newest entry it leaves out is from `before` (ms; RFC 3339 in the body).
///
/// It is a session body like any other — an empty replace-from at `base`,
/// `{"fromIndex": base, "items": [], "endIndex": base, "fp", "prefixFp"}` —
/// with `"base": base, "before": "<RFC 3339>"` added (`before` is `null`
/// when no entry left out is dated). Placed first in the manifest, before the
/// first page kept: a reader from before it (`fmt 1` as it was) takes it for
/// a copy that starts at `base` and the next page replaces from there, so it
/// reads the trimmed copy as one that simply starts later; a reader that knows
/// it also tells its user that the `base` earlier entries are only on the
/// machine. Records never carry it.
pub(crate) fn window_body(base: usize, before: Option<u64>, chain: &dyn Fingerprints) -> Vec<u8> {
    let mut body: Value = serde_json::from_slice(&session_body(base, &[], chain)).unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
    body["base"] = Value::from(base);
    body["before"] = before.map_or(Value::Null, |ms| {
        Value::String(droppen_base::time::iso8601(std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms)))
    });
    serde_json::to_vec(&body).unwrap_or_default()
}

// ------------------------------------------------------------------ index

/// The rows to send: in-scope rows as `key → JSON`.
pub(crate) fn index_rows<R: SessionRow>(rows: &[R], scope: &Scope, now_ms: u64) -> BTreeMap<String, String> {
    rows.iter()
        .filter(|r| scope.admits(r, now_ms))
        .filter_map(|r| r.json().map(|json| (r.row_key(), json)))
        .collect()
}

/// What changed between the rows the hub holds and the rows now.
pub(crate) fn index_diff(held: &BTreeMap<String, String>, now: &BTreeMap<String, String>) -> (Vec<(String, String)>, Vec<String>) {
    let upserts = now
        .iter()
        .filter(|(k, v)| held.get(*k) != Some(*v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let removed = held.keys().filter(|k| !now.contains_key(*k)).cloned().collect();
    (upserts, removed)
}

/// One index body, with the rows it upserts and the keys it removes.
pub(crate) type IndexBody = (Vec<u8>, Vec<(String, String)>, Vec<String>);

/// Index bodies for `upserts` and `removed`, each under [`BODY_BUDGET`]; the
/// first says `reset` when `reset` is set (a page set, or a rebuild). Each
/// comes with the rows it carries, for the cursor.
pub(crate) fn index_bodies(upserts: &[(String, String)], removed: &[String], reset: bool, as_of: u64) -> Vec<IndexBody> {
    let mut out = Vec::new();
    let mut ups: Vec<(String, String)> = Vec::new();
    let mut gone: Vec<String> = Vec::new();
    let mut size = 100;
    let flush = |ups: &mut Vec<(String, String)>, gone: &mut Vec<String>, first: bool, out: &mut Vec<_>| {
        let mut body = String::from("{");
        if first && reset {
            body.push_str("\"reset\":true,");
        }
        body.push_str("\"upserts\":[");
        body.push_str(&ups.iter().map(|(_, v)| v.as_str()).collect::<Vec<_>>().join(","));
        body.push_str("],\"removed\":");
        body.push_str(&serde_json::to_string(&gone).unwrap_or_else(|_| "[]".into()));
        body.push_str(&format!(",\"asOf\":{as_of}}}"));
        out.push((body.into_bytes(), std::mem::take(ups), std::mem::take(gone)));
    };
    for key in removed {
        if size + key.len() + 3 > BODY_BUDGET {
            let first = out.is_empty();
            flush(&mut ups, &mut gone, first, &mut out);
            size = 100;
        }
        size += key.len() + 3;
        gone.push(key.clone());
    }
    for (key, json) in upserts {
        // A row on its own past the budget cannot be sent; rows are a few KiB.
        if json.len() + 100 > BODY_BUDGET {
            continue;
        }
        if size + json.len() + 1 > BODY_BUDGET {
            let first = out.is_empty();
            flush(&mut ups, &mut gone, first, &mut out);
            size = 100;
        }
        size += json.len() + 1;
        ups.push((key.clone(), json.clone()));
    }
    if !ups.is_empty() || !gone.is_empty() || out.is_empty() {
        let first = out.is_empty();
        flush(&mut ups, &mut gone, first, &mut out);
    }
    out
}

/// An index record that changes nothing: `{"asOf": <ms>}`, the daily content
/// write that keeps a quiet machine's index on the hub (relay-and-plans §3.4).
/// Every reader folds it as a no-op but for the time it moves.
pub(crate) fn as_of_body(as_of: u64) -> Vec<u8> {
    format!("{{\"asOf\":{as_of}}}").into_bytes()
}

// ------------------------------------------------------------------ status

/// The machine status body: what the UI shows about the machine, never a
/// credential. [`StatusBody::new`] makes it, the `with_*` steps add what the
/// machine knows besides, and [`StatusBody::encode`] writes it.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusBody<'a> {
    #[serde(flatten)]
    head: StatusHead<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    os: Option<&'a Os>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_version: Option<&'a str>,
    #[serde(skip_serializing_if = "<[Alarm]>::is_empty")]
    alarms: &'a [Alarm],
    #[serde(skip_serializing_if = "<[Alarm]>::is_empty")]
    changes: &'a [Alarm],
    #[serde(skip_serializing_if = "Option::is_none")]
    controllers: Option<Option<&'a Controllers>>,
}

/// What [`BODY_BUDGET`] is measured on: the body but for what the `with_*`
/// steps add.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusHead<'a> {
    as_of: u64,
    display_name: Option<String>,
    mk_epoch: Option<u32>,
    settings: Option<Value>,
    accounts: Option<AccountsDigest<'a>>,
    quota: Option<Value>,
    cli: Option<CliVersions>,
}

/// The status's `cli`: the first line each CLI prints for `--version`, `null`
/// for one that is not there or prints nothing usable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CliVersions {
    /// `claude --version`.
    pub claude: Option<String>,
    /// `codex --version`.
    pub codex: Option<String>,
}

/// What the status keeps of `/api/accounts`: what names each account, the
/// defaults, and the local logins. Every field is the answer's own value, or
/// `null` where the answer has none.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountsDigest<'a> {
    accounts: Vec<AccountDigest<'a>>,
    default_account_id: &'a Value,
    default_codex_account_id: &'a Value,
    local: Option<LocalLogins<'a>>,
}

impl<'a> AccountsDigest<'a> {
    fn of(answer: &'a Value) -> Self {
        let local = &answer["local"];
        Self {
            accounts: answer["accounts"].as_array().into_iter().flatten().map(AccountDigest::of).collect(),
            default_account_id: &answer["defaultAccountId"],
            default_codex_account_id: &answer["defaultCodexAccountId"],
            local: local.is_object().then(|| LocalLogins {
                claude: LocalLogin::of(&local["claude"]),
                codex: LocalLogin::of(&local["codex"]),
            }),
        }
    }
}

/// One account as the status names it: no secret, env key or rest.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountDigest<'a> {
    id: &'a Value,
    label: &'a Value,
    kind: &'a Value,
    provider: &'a Value,
    email: &'a Value,
    usage_enabled: &'a Value,
    /// Whether it may stand in for a signed-out local login: an offline page
    /// that cannot tell says so wrongly.
    rotation: &'a Value,
}

impl<'a> AccountDigest<'a> {
    fn of(account: &'a Value) -> Self {
        Self {
            id: &account["id"],
            label: &account["label"],
            kind: &account["kind"],
            provider: &account["provider"],
            email: &account["email"],
            usage_enabled: &account["usageEnabled"],
            rotation: &account["rotation"],
        }
    }
}

/// Each local login's state and whether somebody chose it as the default —
/// what an offline Settings needs to show which default is set — but not its
/// rests: their reasons are a CLI's own words, and the body has a budget.
#[derive(Clone, Serialize)]
struct LocalLogins<'a> {
    claude: Option<LocalLogin<'a>>,
    codex: Option<LocalLogin<'a>>,
}

/// One local login in [`LocalLogins`].
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalLogin<'a> {
    state: &'a Value,
    is_default: &'a Value,
}

impl<'a> LocalLogin<'a> {
    fn of(login: &'a Value) -> Option<Self> {
        login.is_object().then(|| Self {
            state: &login["state"],
            is_default: &login["isDefault"],
        })
    }
}

impl<'a> StatusBody<'a> {
    /// The status body as far as the server's own answers go. `accounts` is
    /// `/api/accounts`, of which only what names an account is kept. When the
    /// head is over [`BODY_BUDGET`] the quota goes, then the accounts.
    pub(crate) fn new(
        as_of: u64,
        display_name: Option<String>,
        mk_epoch: Option<u32>,
        settings: Option<Value>,
        accounts: Option<&'a Value>,
        quota: Option<Value>,
        cli: Option<CliVersions>,
    ) -> Self {
        let mut head = StatusHead {
            as_of,
            display_name,
            mk_epoch,
            settings,
            accounts: accounts.map(AccountsDigest::of),
            quota,
            cli,
        };
        let len = |head: &StatusHead| serde_json::to_vec(head).map_or(0, |b| b.len());
        if len(&head) > BODY_BUDGET {
            head.quota = None;
        }
        if len(&head) > BODY_BUDGET {
            head.accounts = None;
        }
        StatusBody {
            head,
            os: None,
            agent_version: None,
            alarms: &[],
            changes: &[],
            controllers: None,
        }
    }

    /// With what this computer runs: `os` (`{name, version, arch}`,
    /// [`crate::system::os`]) and `agentVersion` (this server's
    /// version). Older readers ignore both; the web app falls back to
    /// `settings.platform` and `settings.serverVersion` without them.
    pub(crate) fn with_system(self, os: &'a Os, agent_version: &'a str) -> Self {
        Self {
            os: Some(os),
            agent_version: Some(agent_version),
            ..self
        }
    }

    /// With the machine's chain alarms (`alarms`: `[{atMs, what}]`, oldest
    /// first; left out when there are none). Every device of the account reads
    /// the status stream, so this is how a fork this machine saw reaches all of
    /// them — the hub can withhold the record, but not forge or alter it, and
    /// the machine-signed head says when it is behind.
    pub(crate) fn with_alarms(self, alarms: &'a [Alarm]) -> Self {
        Self { alarms, ..self }
    }

    /// With the account changes this machine saw (`changes`: `[{atMs, what}]`,
    /// oldest first, at most [`crate::state::MAX_CHANGES`]; left out when
    /// there are none): machines and devices added or removed, recovery joins —
    /// every device of the account reads them, whoever made the change.
    pub(crate) fn with_changes(self, changes: &'a [Alarm]) -> Self {
        Self { changes, ..self }
    }

    /// With which devices ever opened a channel to this machine (`controllers`:
    /// `{from, fromMs, seq, devices: [{id, firstMs}]}`, [`Controllers`]),
    /// always — an empty list included, `null` before the agent has a chain; a
    /// body without the field is from an agent that does not keep the record.
    /// When a device that could control this machine no longer can, the page
    /// says whether it ever connected. A new device, or the chain moving on, is
    /// a new record.
    pub(crate) fn with_controllers(self, controllers: Option<&'a Controllers>) -> Self {
        Self {
            controllers: Some(controllers),
            ..self
        }
    }

    /// The body as it is written.
    pub(crate) fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }
}

/// The part of a status body that changing means a new record (not `asOf`).
pub(crate) fn status_fingerprint(body: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut v {
        map.remove("asOf");
    }
    let digest = Sha256::digest(serde_json::to_vec(&v).unwrap_or_default());
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::test_transcripts::{encode_fp, fingerprint_chain, TestItems, TestRow, RESUME_OVERLAP};

    fn items(n: usize, text_len: usize) -> TestItems {
        TestItems(
            (0..n)
                .map(|i| {
                    serde_json::json!({
                        "role": "assistant", "kind": "text", "text": format!("{i}:{}", "x".repeat(text_len)),
                        "detail": null, "toolName": null, "meta": null, "isError": false, "question": null,
                        "timestamp": null, "truncated": false,
                    })
                })
                .collect(),
        )
    }

    #[test]
    fn a_delta_starts_at_the_newest_position_both_sides_agree_on() {
        let path = std::path::Path::new("/tmp/t.jsonl");
        let mut list = items(10, 5);
        let chain = fingerprint_chain(path, &list);
        let cursor = SessionCursor::at(0, 10, &chain, "s");
        assert_eq!(cursor.delta(&chain), Delta::None);
        list.0.extend(items(2, 7).0);
        let grown = fingerprint_chain(path, &list);
        assert_eq!(cursor.delta(&grown), Delta::From(10), "appended: from the old end");
        // The last item rewritten in place (a tool result attaching).
        let mut rewritten = items(10, 5);
        rewritten.0[9]["text"] = "changed".into();
        let chain2 = fingerprint_chain(path, &rewritten);
        assert_eq!(cursor.delta(&chain2), Delta::From(9));
        // Rewritten further back than the cursor remembers: rebuild.
        let mut early = items(10, 5);
        early.0[0]["text"] = "changed".into();
        assert_eq!(cursor.delta(&fingerprint_chain(path, &early)), Delta::Rebuild);
        // Another file entirely.
        assert_eq!(
            cursor.delta(&fingerprint_chain(std::path::Path::new("/tmp/other"), &list)),
            Delta::Rebuild
        );
    }

    #[test]
    fn session_bodies_fit_the_budget_cover_the_range_and_carry_the_fingerprints() {
        let path = std::path::Path::new("/tmp/t.jsonl");
        let list = items(40, 4000);
        let chain = fingerprint_chain(path, &list);
        let bodies = session_bodies(&list, 5, 40, &chain);
        assert!(bodies.len() > 1);
        let mut at = 5;
        for (end, body) in &bodies {
            assert!(body.len() <= BODY_BUDGET);
            let v: Value = serde_json::from_slice(body).unwrap();
            assert_eq!(v["fromIndex"], at);
            assert_eq!(v["endIndex"], *end);
            assert_eq!(v["items"].as_array().unwrap().len(), end - at);
            assert_eq!(v["fp"], encode_fp(&chain.0[*end]));
            assert_eq!(v["prefixFp"], encode_fp(&chain.0[end - RESUME_OVERLAP]));
            at = *end;
        }
        assert_eq!(at, 40);
        // An empty range is one body that says so.
        let empty = session_bodies(&list, 40, 40, &chain);
        assert_eq!(empty.len(), 1);
        assert_eq!(serde_json::from_slice::<Value>(&empty[0].1).unwrap()["items"], serde_json::json!([]));
    }

    #[test]
    fn an_item_too_big_for_a_record_is_cut_down_and_marked() {
        let path = std::path::Path::new("/tmp/t.jsonl");
        let mut list = items(3, 10);
        list.0[1]["detail"] = "é".repeat(200_000).into();
        let chain = fingerprint_chain(path, &list);
        let bodies = session_bodies(&list, 0, 3, &chain);
        let all: Vec<Value> = bodies
            .iter()
            .flat_map(|(_, b)| serde_json::from_slice::<Value>(b).unwrap()["items"].as_array().unwrap().clone())
            .collect();
        assert_eq!(all.len(), 3);
        assert_eq!(all[1]["rhOfflineTruncated"], true);
        assert_eq!(all[1]["truncated"], true);
        assert!(all[1]["detail"].as_str().unwrap().ends_with("[cut short in the offline copy]"));
        assert!(all[0].get("rhOfflineTruncated").is_none());
        for (_, b) in &bodies {
            assert!(b.len() <= BODY_BUDGET);
        }
    }

    /// A session-list row; `scope` as the list spells it (`owned`, `external`).
    fn summary(id: &str, scope: &str, updated: &str) -> TestRow {
        TestRow::of(serde_json::json!({
            "pinned": false, "id": id, "source": "claude", "scope": scope, "server": "s", "title": "t",
            "cwd": null, "repoKey": null, "gitBranch": null, "updatedAt": updated, "createdAt": updated,
            "sizeBytes": 1, "filePath": "/x", "status": "idle", "activity": "idle", "resident": null,
            "claudeRoot": null, "accountId": null, "attentionAt": null, "unread": false, "unreadCount": 0,
        }))
    }

    #[test]
    fn the_sync_scope_is_own_sessions_of_the_last_thirty_days() {
        let now = droppen_base::time::parse_iso("2026-10-15T12:00:00Z")
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let scope = Scope::default();
        assert!(scope.admits(&summary("a", "owned", "2026-10-14T00:00:00Z"), now));
        assert!(scope.admits(&summary("a", "owned", "2026-09-16T00:00:00Z"), now));
        assert!(
            !scope.admits(&summary("a", "owned", "2026-09-01T00:00:00Z"), now),
            "older than 30 days"
        );
        assert!(
            !scope.admits(&summary("a", "external", "2026-10-14T00:00:00Z"), now),
            "~/.claude is not synced by default"
        );
        assert!(Scope { external: true, ..scope }.admits(&summary("a", "external", "2026-10-14T00:00:00Z"), now));
        assert!(!scope.admits(&summary("a", "owned", "not a date"), now));
        let rows = vec![
            summary("a", "owned", "2026-10-14T00:00:00Z"),
            summary("b", "external", "2026-10-14T00:00:00Z"),
        ];
        let kept = index_rows(&rows, &scope, now);
        assert_eq!(kept.keys().collect::<Vec<_>>(), vec!["claude/a"]);
    }

    #[test]
    fn index_diffs_and_bodies() {
        let held: BTreeMap<String, String> = [("claude/a".into(), "{\"v\":1}".into()), ("claude/b".into(), "{\"v\":1}".into())].into();
        let now: BTreeMap<String, String> = [("claude/a".into(), "{\"v\":2}".into()), ("claude/c".into(), "{\"v\":1}".into())].into();
        let (ups, gone) = index_diff(&held, &now);
        assert_eq!(ups.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), vec!["claude/a", "claude/c"]);
        assert_eq!(gone, vec!["claude/b".to_string()]);
        let bodies = index_bodies(&ups, &gone, false, 7);
        assert_eq!(bodies.len(), 1);
        let v: Value = serde_json::from_slice(&bodies[0].0).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"upserts": [{"v": 2}, {"v": 1}], "removed": ["claude/b"], "asOf": 7})
        );
        // Many rows split across bodies; only the first carries `reset`.
        let many: Vec<(String, String)> = (0..100)
            .map(|i| (format!("claude/{i}"), format!("{{\"pad\":\"{}\"}}", "y".repeat(2000))))
            .collect();
        let split = index_bodies(&many, &[], true, 1);
        assert!(split.len() > 1);
        let firsts: Vec<bool> = split
            .iter()
            .map(|(b, ..)| serde_json::from_slice::<Value>(b).unwrap().get("reset").is_some())
            .collect();
        assert_eq!(firsts.iter().filter(|r| **r).count(), 1);
        assert!(firsts[0]);
        assert_eq!(split.iter().map(|(_, ups, _)| ups.len()).sum::<usize>(), 100);
        for (b, ..) in &split {
            assert!(b.len() <= BODY_BUDGET);
        }
    }

    #[test]
    fn the_daily_index_record_and_the_window_page_are_session_and_index_bodies_old_readers_fold() {
        assert_eq!(serde_json::from_slice::<Value>(&as_of_body(42)).unwrap(), serde_json::json!({"asOf": 42}));
        let path = std::path::Path::new("/tmp/t.jsonl");
        let list = items(10, 5);
        let chain = fingerprint_chain(path, &list);
        let w: Value = serde_json::from_slice(&window_body(4, Some(1_760_000_000_000), &chain)).unwrap();
        assert_eq!(
            w,
            serde_json::json!({
                "fromIndex": 4, "items": [], "endIndex": 4,
                "fp": encode_fp(&chain.0[4]), "prefixFp": encode_fp(&chain.0[1]),
                "base": 4, "before": "2025-10-09T08:53:20.000Z",
            })
        );
        let undated: Value = serde_json::from_slice(&window_body(4, None, &chain)).unwrap();
        assert_eq!(undated["before"], Value::Null);
        // Item times: what parses, and nothing for what does not.
        let mut dated = items(3, 1);
        dated.0[0]["timestamp"] = "2026-10-15T08:00:01.000Z".into();
        dated.0[2]["timestamp"] = "not a time".into();
        assert_eq!(item_times(&dated), vec![Some(1_792_051_201_000), None, None]);
    }

    #[test]
    fn the_status_keeps_no_credentials_and_its_fingerprint_ignores_the_time() {
        let accounts = serde_json::json!({"accounts": [{"id": "a1", "label": "work", "kind": "oauth", "provider": "anthropic", "email": "e@x", "usageEnabled": true, "token": "SECRET", "envKeys": ["K"]}], "defaultAccountId": "a1"});
        let a = StatusBody::new(
            1,
            Some("mac".into()),
            Some(0),
            Some(serde_json::json!({"hostname": "mac"})),
            Some(&accounts),
            None,
            Some(CliVersions {
                claude: Some("2.0".into()),
                codex: None,
            }),
        )
        .encode();
        let b = StatusBody::new(
            2,
            Some("mac".into()),
            Some(0),
            Some(serde_json::json!({"hostname": "mac"})),
            Some(&accounts),
            None,
            Some(CliVersions {
                claude: Some("2.0".into()),
                codex: None,
            }),
        )
        .encode();
        let text = String::from_utf8(a.clone()).unwrap();
        assert!(!text.contains("SECRET") && !text.contains("envKeys"));
        assert_eq!(serde_json::from_slice::<Value>(&a).unwrap()["accounts"]["accounts"][0]["rotation"], Value::Null, "absent stays absent");
        assert_eq!(serde_json::from_slice::<Value>(&a).unwrap()["accounts"]["local"], Value::Null, "a list without local logins");
        assert_eq!(status_fingerprint(&a), status_fingerprint(&b));
        let c = StatusBody::new(
            3,
            Some("mac".into()),
            Some(0),
            Some(serde_json::json!({"hostname": "other"})),
            None,
            None,
            None,
        )
        .encode();
        assert_ne!(status_fingerprint(&a), status_fingerprint(&c));
        let renamed = StatusBody::new(
            4,
            Some("studio".into()),
            Some(0),
            Some(serde_json::json!({"hostname": "mac"})),
            None,
            None,
            None,
        )
        .encode();
        assert_eq!(serde_json::from_slice::<Value>(&renamed).unwrap()["displayName"], "studio");
    }

    /// The local logins travel with the accounts — their state and whether
    /// one was chosen as the default, so an offline Settings shows which
    /// default is set — but not their rests, whose reasons are a CLI's words.
    #[test]
    fn the_status_carries_each_local_logins_state_and_choice_but_not_its_rests() {
        let accounts = serde_json::json!({
            "accounts": [{"id": "a1", "label": "work", "rotation": true, "rest": [{"scope": "all", "reason": "weekly limit"}]}],
            "defaultAccountId": null, "defaultCodexAccountId": null,
            "local": {
                "claude": {"state": "signed_out", "isDefault": true, "rest": [{"scope": "all", "class": "auth", "reason": "OAuth session expired"}]},
                "codex": {"state": "unknown", "isDefault": false, "rest": []},
            },
        });
        let body = StatusBody::new(1, Some("mac".into()), Some(0), None, Some(&accounts), None, None).encode();
        let parsed = serde_json::from_slice::<Value>(&body).unwrap();
        assert_eq!(parsed["accounts"]["accounts"][0]["rotation"], true, "whether each account may stand in");
        let local = &parsed["accounts"]["local"];
        assert_eq!(
            local,
            &serde_json::json!({"claude": {"state": "signed_out", "isDefault": true}, "codex": {"state": "unknown", "isDefault": false}})
        );
        let text = String::from_utf8(body).unwrap();
        assert!(!text.contains("OAuth session expired") && !text.contains("weekly limit"), "no rests, the accounts' nor the logins'");
    }

    #[test]
    fn the_status_says_what_the_computer_runs() {
        let body = StatusBody::new(1, Some("box".into()), Some(0), None, None, None, None);
        let os = Os {
            name: "macos".into(),
            version: Some("15.3".into()),
            distro: None,
            arch: "aarch64".into(),
        };
        let with = body.clone().with_system(&os, "0.2.122").encode();
        let body = body.encode();
        let v: Value = serde_json::from_slice(&with).unwrap();
        assert_eq!((v["os"]["name"].as_str(), v["agentVersion"].as_str()), (Some("macos"), Some("0.2.122")));
        assert_eq!(v["displayName"], "box", "the rest is as it was");
        assert_ne!(
            status_fingerprint(&body),
            status_fingerprint(&with),
            "an upgrade is a change worth writing"
        );
    }

    #[test]
    fn account_changes_go_into_the_status_and_change_it() {
        use crate::state::Alarm;
        let body = StatusBody::new(1, Some("box".into()), Some(0), None, None, None, None);
        assert_eq!(body.clone().with_changes(&[]).encode(), body.encode(), "no changes, no field");
        let changes = [Alarm {
            at_ms: 7,
            what: "device m-2 (runs AI) was added by d-1".into(),
        }];
        let with = body.clone().with_changes(&changes).encode();
        let body = body.encode();
        let v: Value = serde_json::from_slice(&with).unwrap();
        assert_eq!(v["changes"][0]["atMs"], 7);
        assert_eq!(v["changes"][0]["what"], "device m-2 (runs AI) was added by d-1");
        assert_ne!(status_fingerprint(&with), status_fingerprint(&body), "a new change is a new record");
    }

    #[test]
    fn the_devices_that_opened_a_channel_are_always_in_the_status_and_change_it() {
        use crate::state::{Controller, Controllers};
        let body = StatusBody::new(1, Some("box".into()), Some(0), None, None, None, None);
        let nobody = Controllers {
            from: 0,
            from_ms: 1_000,
            seq: 4,
            devices: Vec::new(),
        };
        let empty = body.clone().with_controllers(Some(&nobody)).encode();
        let v: Value = serde_json::from_slice(&empty).unwrap();
        assert_eq!(
            v["controllers"],
            serde_json::json!({"from": 0, "fromMs": 1_000, "seq": 4, "devices": []}),
            "there even when nobody opened a channel: its absence means an agent that does not say"
        );
        assert_eq!(v["displayName"], "box", "the rest is as it was");
        assert_ne!(status_fingerprint(&empty), status_fingerprint(&body.encode()));
        let one = Controllers {
            devices: vec![Controller {
                id: "d-1".into(),
                first_ms: 2_000,
            }],
            ..nobody.clone()
        };
        let named: Value = serde_json::from_slice(&body.clone().with_controllers(Some(&one)).encode()).unwrap();
        assert_eq!(named["controllers"]["devices"], serde_json::json!([{"id": "d-1", "firstMs": 2_000}]));
        let with_one = body.clone().with_controllers(Some(&one)).encode();
        assert_ne!(status_fingerprint(&with_one), status_fingerprint(&empty), "a device that opened a channel is a new record");
        let moved = Controllers { seq: 5, ..nobody };
        assert_ne!(
            status_fingerprint(&body.clone().with_controllers(Some(&moved)).encode()),
            status_fingerprint(&empty),
            "and so is the chain moving on"
        );
        let unknown: Value = serde_json::from_slice(&body.clone().with_controllers(None).encode()).unwrap();
        assert_eq!(unknown["controllers"], Value::Null, "no chain yet: nothing to say");
    }

    #[test]
    fn alarms_go_into_the_status_and_change_it() {
        use crate::state::Alarm;
        let body = StatusBody::new(5, Some("studio".into()), Some(0), None, None, None, None);
        assert_eq!(body.clone().with_alarms(&[]).encode(), body.encode(), "no alarms, no field");
        let alarms = [Alarm {
            at_ms: 4,
            what: "the hub served a device chain that conflicts with this device's".into(),
        }];
        let with = body.clone().with_alarms(&alarms).encode();
        let body = body.encode();
        let v: Value = serde_json::from_slice(&with).unwrap();
        assert_eq!(v["alarms"][0]["atMs"], 4);
        assert!(v["alarms"][0]["what"].as_str().unwrap().contains("conflicts"));
        assert_ne!(status_fingerprint(&with), status_fingerprint(&body), "a new alarm is a new record");
    }
}

/// Today's status bodies, byte for byte, with their fingerprints: the status
/// stream writes a new record whenever the fingerprint moves, so a change to
/// how the body is built must leave both alone.
#[cfg(test)]
mod status_pins {
    use super::*;
    use crate::state::{Alarm, Controller, Controllers};
    use serde_json::json;
    use sha2::{Digest, Sha256};

    const AGENT: &str = "0.2.134";
    const OS_RELEASE: &str = "NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\n";

    fn cli() -> Option<CliVersions> {
        Some(CliVersions {
            claude: Some("2.1.3 (Claude Code)".into()),
            codex: None,
        })
    }

    fn sha(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    fn text(bytes: &[u8]) -> &str {
        std::str::from_utf8(bytes).unwrap()
    }

    /// Nothing known: no name, no epoch, no CLI versions, no chain yet.
    fn minimal() -> Vec<u8> {
        let os = crate::system::detect("macos", "aarch64", |_| None);
        StatusBody::new(1, None, None, None, None, None, None)
            .with_system(&os, AGENT)
            .with_alarms(&[])
            .with_changes(&[])
            .with_controllers(None)
            .encode()
    }

    /// The `/api/accounts` answer the full fixture digests: credentials,
    /// rests and a malformed entry the digest must not carry or choke on.
    fn accounts() -> Value {
        json!({
            "accounts": [
                {"id": "a1", "label": "work", "kind": "oauth", "provider": "anthropic", "email": "w@example.com",
                 "usageEnabled": true, "rotation": true, "token": "SECRET-TOKEN", "secretSha256": "SECRET-HASH",
                 "envKeys": ["ANTHROPIC_API_KEY"], "rest": [{"scope": "all", "reason": "weekly limit"}],
                 "createdAt": "2026-10-01T00:00:00.000Z"},
                {"id": "a2", "kind": "env", "provider": "openai"},
                "not an account"
            ],
            "defaultAccountId": "a1",
            "local": {
                "claude": {"state": "signed_in", "isDefault": false, "email": "me@example.com",
                           "rest": [{"scope": "all", "reason": "OAuth session expired"}]},
                "codex": null
            }
        })
    }

    fn alarms() -> [Alarm; 1] {
        [Alarm {
            at_ms: 4,
            what: "the hub served a device chain that conflicts with this device's".into(),
        }]
    }

    fn changes() -> [Alarm; 2] {
        [
            Alarm {
                at_ms: 7,
                what: "device m-2 (runs AI) was added by d-1".into(),
            },
            Alarm {
                at_ms: 9,
                what: "device d-3 was removed by d-1".into(),
            },
        ]
    }

    fn controllers() -> Controllers {
        Controllers {
            from: 2,
            from_ms: 1_000,
            seq: 9,
            devices: vec![Controller {
                id: "d-1".into(),
                first_ms: 2_000,
            }],
        }
    }

    /// Every field set, as a linked machine with a chain writes it.
    fn full(settings: Value, quota: Value) -> Vec<u8> {
        let os = crate::system::detect("linux", "x86_64", |_| Some(OS_RELEASE.into()));
        let accounts = accounts();
        StatusBody::new(1_761_700_000_000, Some("studio".into()), Some(3), Some(settings), Some(&accounts), Some(quota), cli())
            .with_system(&os, AGENT)
            .with_alarms(&alarms())
            .with_changes(&changes())
            .with_controllers(Some(&controllers()))
            .encode()
    }

    fn settings() -> Value {
        json!({"hostname": "studio", "platform": "linux", "serverVersion": "0.2.134",
               "plugins": [{"id": "tasks", "switchedOn": true}], "load": 0.1, "note": "é \"quoted\"\n"})
    }

    fn quota() -> Value {
        json!({"claude": {"windows": [{"label": "5h", "usedPercent": 12.5, "resetsAt": "2026-10-29T10:00:00.000Z"}], "plan": "max"},
               "codex": null})
    }

    /// A body whose head (what the budget is measured on) is `under` bytes
    /// short of [`BODY_BUDGET`], with `alarms` added after.
    fn near_budget(under: usize, alarms: &[Alarm]) -> (usize, Vec<u8>) {
        let os = crate::system::detect("linux", "x86_64", |_| Some(OS_RELEASE.into()));
        let accounts = accounts();
        let head = |note: usize| {
            let mut settings = settings();
            settings["note"] = json!("s".repeat(note));
            StatusBody::new(9, Some("studio".into()), Some(3), Some(settings), Some(&accounts), Some(quota()), cli())
        };
        let note = BODY_BUDGET - under - head(0).encode().len();
        let len = head(note).encode().len();
        let controllers = controllers();
        (len, head(note).with_system(&os, AGENT).with_alarms(alarms).with_controllers(Some(&controllers)).encode())
    }

    #[test]
    fn a_body_with_nothing_known() {
        let body = minimal();
        assert_eq!(
            text(&body),
            r#"{"asOf":1,"displayName":null,"mkEpoch":null,"settings":null,"accounts":null,"quota":null,"cli":null,"os":{"name":"macos","version":null,"arch":"aarch64"},"agentVersion":"0.2.134","controllers":null}"#
        );
        assert_eq!(status_fingerprint(&body), "9cdc04650e2700c0d1df7a0c0ea2f9be");
    }

    #[test]
    fn a_body_with_every_field_and_no_credential() {
        let body = full(settings(), quota());
        assert_eq!(
            text(&body),
            concat!(
                r#"{"asOf":1761700000000,"displayName":"studio","mkEpoch":3,"#,
                r#""settings":{"hostname":"studio","platform":"linux","serverVersion":"0.2.134","plugins":[{"id":"tasks","switchedOn":true}],"load":0.1,"note":"é \"quoted\"\n"},"#,
                r#""accounts":{"accounts":[{"id":"a1","label":"work","kind":"oauth","provider":"anthropic","email":"w@example.com","usageEnabled":true,"rotation":true},"#,
                r#"{"id":"a2","label":null,"kind":"env","provider":"openai","email":null,"usageEnabled":null,"rotation":null},"#,
                r#"{"id":null,"label":null,"kind":null,"provider":null,"email":null,"usageEnabled":null,"rotation":null}],"#,
                r#""defaultAccountId":"a1","defaultCodexAccountId":null,"local":{"claude":{"state":"signed_in","isDefault":false},"codex":null}},"#,
                r#""quota":{"claude":{"windows":[{"label":"5h","usedPercent":12.5,"resetsAt":"2026-10-29T10:00:00.000Z"}],"plan":"max"},"codex":null},"#,
                r#""cli":{"claude":"2.1.3 (Claude Code)","codex":null},"#,
                r#""os":{"name":"linux","version":"24.04","distro":"ubuntu","arch":"x86_64"},"agentVersion":"0.2.134","#,
                r#""alarms":[{"atMs":4,"what":"the hub served a device chain that conflicts with this device's"}],"#,
                r#""changes":[{"atMs":7,"what":"device m-2 (runs AI) was added by d-1"},{"atMs":9,"what":"device d-3 was removed by d-1"}],"#,
                r#""controllers":{"from":2,"fromMs":1000,"seq":9,"devices":[{"id":"d-1","firstMs":2000}]}}"#,
            )
        );
        assert_eq!(status_fingerprint(&body), "8a6cbd4466e55af3e5a60c556785087c");
        for secret in ["SECRET", "ANTHROPIC_API_KEY", "envKeys", "weekly limit", "OAuth session expired", "me@example.com", "createdAt"] {
            assert!(!text(&body).contains(secret), "{secret}");
        }
    }

    #[test]
    fn a_quota_over_the_budget_is_left_out() {
        let body = full(settings(), json!({"claude": {"note": "q".repeat(BODY_BUDGET)}}));
        assert_eq!(
            text(&body),
            concat!(
                r#"{"asOf":1761700000000,"displayName":"studio","mkEpoch":3,"#,
                r#""settings":{"hostname":"studio","platform":"linux","serverVersion":"0.2.134","plugins":[{"id":"tasks","switchedOn":true}],"load":0.1,"note":"é \"quoted\"\n"},"#,
                r#""accounts":{"accounts":[{"id":"a1","label":"work","kind":"oauth","provider":"anthropic","email":"w@example.com","usageEnabled":true,"rotation":true},"#,
                r#"{"id":"a2","label":null,"kind":"env","provider":"openai","email":null,"usageEnabled":null,"rotation":null},"#,
                r#"{"id":null,"label":null,"kind":null,"provider":null,"email":null,"usageEnabled":null,"rotation":null}],"#,
                r#""defaultAccountId":"a1","defaultCodexAccountId":null,"local":{"claude":{"state":"signed_in","isDefault":false},"codex":null}},"#,
                r#""quota":null,"#,
                r#""cli":{"claude":"2.1.3 (Claude Code)","codex":null},"#,
                r#""os":{"name":"linux","version":"24.04","distro":"ubuntu","arch":"x86_64"},"agentVersion":"0.2.134","#,
                r#""alarms":[{"atMs":4,"what":"the hub served a device chain that conflicts with this device's"}],"#,
                r#""changes":[{"atMs":7,"what":"device m-2 (runs AI) was added by d-1"},{"atMs":9,"what":"device d-3 was removed by d-1"}],"#,
                r#""controllers":{"from":2,"fromMs":1000,"seq":9,"devices":[{"id":"d-1","firstMs":2000}]}}"#,
            )
        );
        assert_eq!(status_fingerprint(&body), "b907b9b11e3a05b0724d34374544b764");
    }

    /// Over the budget without the quota too: the accounts go as well.
    #[test]
    fn accounts_over_the_budget_go_after_the_quota() {
        let mut accounts = accounts();
        accounts["accounts"][1]["label"] = json!("l".repeat(2_000));
        let mut settings = settings();
        settings["note"] = json!("s".repeat(BODY_BUDGET - 1_000));
        let os = crate::system::detect("linux", "x86_64", |_| Some(OS_RELEASE.into()));
        let controllers = controllers();
        let body = StatusBody::new(9, Some("studio".into()), Some(3), Some(settings), Some(&accounts), Some(quota()), cli())
            .with_system(&os, AGENT)
            .with_controllers(Some(&controllers))
            .encode();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!((v["quota"].is_null(), v["accounts"].is_null()), (true, true));
        assert_eq!(
            (body.len(), sha(&body).as_str(), status_fingerprint(&body).as_str()),
            (59_456, "4388154ae7ac05a6ee1fbf23103d9b94343cdfa6c5fbb3e08233d9059bbedc25", "88d900500dde7d30cb2d330a1e48ed26")
        );
    }

    /// The budget is measured on the head alone: what the system, the alarms,
    /// the changes and the controllers add after it may take the body past
    /// [`BODY_BUDGET`] without costing the quota or the accounts.
    #[test]
    fn the_budget_is_measured_before_the_system_and_the_alarms() {
        let alarms: Vec<Alarm> = (0..5).map(|i| Alarm { at_ms: i, what: "a".repeat(200) }).collect();
        let (head, body) = near_budget(40, &alarms);
        assert!(head <= BODY_BUDGET && body.len() > BODY_BUDGET, "{head} {}", body.len());
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!((v["quota"]["claude"]["plan"].as_str(), v["accounts"]["defaultAccountId"].as_str()), (Some("max"), Some("a1")));
        assert_eq!(
            (body.len(), sha(&body).as_str(), status_fingerprint(&body).as_str()),
            (61_262, "569b1357a399cd6f2812bbabef0776241e3d457952b2b0685eab60060a45d9d9", "baf957ef260af6950017edc65111d451")
        );
    }

    /// An item no halving brings under the limit is replaced by a stand-in
    /// that keeps its role, kind and time.
    #[test]
    fn an_item_too_big_to_cut_becomes_a_stand_in() {
        let numbers: Vec<u32> = (0..400).collect();
        let item = json!({"role": "assistant", "kind": "tool_result", "text": "short", "detail": numbers, "timestamp": "2026-10-29T05:00:00.000Z"});
        assert_eq!(
            serde_json::to_string(&fit_item(item, 200)).unwrap(),
            r#"{"role":"assistant","kind":"tool_result","text":"[too large for the offline copy]","detail":null,"toolName":null,"meta":null,"isError":false,"question":null,"timestamp":"2026-10-29T05:00:00.000Z","truncated":true,"rhOfflineTruncated":true}"#
        );
        assert_eq!(
            serde_json::to_string(&fit_item(json!(numbers), 200)).unwrap(),
            r#"{"role":"system","kind":"text","text":"[too large for the offline copy]","detail":null,"toolName":null,"meta":null,"isError":false,"question":null,"timestamp":null,"truncated":true,"rhOfflineTruncated":true}"#
        );
    }
}
