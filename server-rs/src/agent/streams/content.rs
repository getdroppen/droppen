//! What the agent's streams say: the bodies of records and pages, cut from the
//! same things the live API serves — the session list's rows, the feed's
//! normalised transcript items — so a browser reading them renders exactly
//! what it would have been sent live. Pure functions; the sync task feeds
//! them and the writer seals what they return.
//!
//! | stream | body |
//! |---|---|
//! | index | `{"reset"?: true, "upserts": [SessionSummary…], "removed": ["source/id"…], "asOf"}`, or just `{"asOf"}` ([`as_of_body`]) |
//! | status | `{"asOf", "displayName", "mkEpoch", "settings", "accounts", "quota", "cli", "alarms"?, "changes"?}`, whole each time |
//! | session | `{"fromIndex", "items", "endIndex", "fp", "prefixFp"}`, replace-from; a trimmed copy's first page adds `"base"`, `"before"` ([`window_body`]) |
//!
//! Every body fits one record ([`BODY_BUDGET`]); a transcript item too big for
//! one on its own is cut down (its longest strings halved until it fits) and
//! marked `"truncated": true, "rhOfflineTruncated": true` — the offline copy is
//! for reading back, and the machine keeps the original.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::feed::{encode_fp, Fp, RESUME_OVERLAP};
use crate::sessions::{SessionScope, SessionSummary};

/// The most a body may be: under a record's and a page's largest body
/// (`rh_proto::stream::MAX_PAGE_BODY`, a few hundred bytes more), with room for
/// the envelope around the items.
pub const BODY_BUDGET: usize = 60_000;
/// How many fingerprints behind its end a session cursor keeps: how far back
/// an in-place rewrite (a tool result attaching to its call) can reach and
/// still be sent as a delta rather than a rebuild.
pub const CURSOR_FPS: usize = 8;
/// The most items an offline copy of a session holds: the newest ones, past
/// this. About the same as 1024 full pages would allow, far more than anything
/// reads at once.
pub const MAX_ITEMS: usize = 20_000;

/// Which sessions are copied to the hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scope {
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
    pub fn from_env() -> Self {
        let mut scope = Self::default();
        if let Some(days) = crate::env::var("DROPPEN_AGENT_SYNC_DAYS").and_then(|d| d.parse().ok()) {
            scope.days = days;
        }
        scope.external = crate::env::var("DROPPEN_AGENT_SYNC_EXTERNAL").as_deref() == Some("1");
        scope
    }

    /// Whether `row` is copied: its own (unless external ones are asked for)
    /// and updated recently enough. A row whose time does not parse is not.
    pub fn admits(&self, row: &SessionSummary, now_ms: u64) -> bool {
        if row.scope != SessionScope::Owned && !self.external {
            return false;
        }
        let Some(updated) = crate::schedule::parse_iso(&row.updated_at) else {
            return false;
        };
        let updated_ms = updated.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        now_ms.saturating_sub(updated_ms) <= self.days * 24 * 60 * 60 * 1000
    }
}

/// The key a row is filed under in the index: `source/id`.
pub fn row_key(row: &SessionSummary) -> String {
    format!("{}/{}", row.source.as_str(), row.id)
}

// ------------------------------------------------------------------ sessions

/// Where a session stream stands in its transcript.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCursor {
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
    pub fn parse(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }

    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// The cursor of a copy holding `[start, end)` of a transcript with `chain`.
    pub fn at(start: usize, end: usize, chain: &[Fp], sig: &str) -> Self {
        let from = end.saturating_sub(CURSOR_FPS).max(start);
        Self {
            start,
            end,
            fps: (from..=end).filter_map(|i| chain.get(i).map(|fp| (i, encode_fp(fp)))).collect(),
            sig: sig.to_string(),
        }
    }

    /// The fingerprint at `end`.
    pub fn end_fp(&self) -> Option<&str> {
        self.fps.iter().find(|(i, _)| *i == self.end).map(|(_, fp)| fp.as_str())
    }
}

/// What to send for a session, given what the copy holds and the transcript now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delta {
    /// Nothing changed.
    None,
    /// Replace from this item to the end.
    From(usize),
    /// What the copy holds is no longer a prefix of the transcript (rewritten,
    /// replaced, adopted): rebuild it.
    Rebuild,
}

/// The newest position whose fingerprint the copy and the transcript agree
/// on decides: from there to the end is the delta.
pub fn session_delta(cursor: &SessionCursor, chain: &[Fp]) -> Delta {
    let len = chain.len().saturating_sub(1);
    for (at, fp) in cursor.fps.iter().rev() {
        if chain.get(*at).is_some_and(|ours| encode_fp(ours) == *fp) {
            if *at == cursor.end && len == cursor.end {
                return Delta::None;
            }
            return Delta::From(*at);
        }
    }
    Delta::Rebuild
}

/// Cut `item` (an item's JSON) down to `max` bytes: halve its longest strings
/// until it fits, and say so on it.
pub fn fit_item(item: Value, max: usize) -> Value {
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
    serde_json::json!({
        "role": item.get("role").cloned().unwrap_or(Value::String("system".into())),
        "kind": item.get("kind").cloned().unwrap_or(Value::String("text".into())),
        "text": "[too large for the offline copy]",
        "detail": null, "toolName": null, "meta": null, "isError": false, "question": null,
        "timestamp": item.get("timestamp").cloned().unwrap_or(Value::Null),
        "truncated": true, "rhOfflineTruncated": true,
    })
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
fn session_body(from: usize, items: &[String], chain: &[Fp]) -> Vec<u8> {
    let to = from + items.len();
    let fp = chain.get(to).map(encode_fp).unwrap_or_default();
    let prefix = chain.get(to.saturating_sub(RESUME_OVERLAP)).map(encode_fp).unwrap_or_default();
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
pub fn session_bodies<T: Serialize>(items: &[T], from: usize, to: usize, chain: &[Fp]) -> Vec<(usize, Vec<u8>)> {
    const ENVELOPE: usize = 200;
    let item_max = BODY_BUDGET - ENVELOPE;
    let mut out = Vec::new();
    let mut batch: Vec<String> = Vec::new();
    let mut batch_from = from;
    let mut size = ENVELOPE;
    for (i, item) in items.iter().enumerate().take(to).skip(from) {
        let value = serde_json::to_value(item).unwrap_or(Value::Null);
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
pub fn session_start(len: usize) -> usize {
    len.saturating_sub(MAX_ITEMS)
}

/// Each item's time (ms since the epoch), `None` where it has none that parses.
pub fn item_times(items: &[crate::sessions::util::TranscriptItem]) -> Vec<Option<u64>> {
    items
        .iter()
        .map(|item| {
            item.timestamp
                .0
                .as_deref()
                .and_then(crate::schedule::parse_iso)
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
pub fn window_body(base: usize, before: Option<u64>, chain: &[Fp]) -> Vec<u8> {
    let mut body: Value = serde_json::from_slice(&session_body(base, &[], chain)).unwrap_or_else(|_| serde_json::json!({}));
    body["base"] = Value::from(base);
    body["before"] = before.map_or(Value::Null, |ms| {
        Value::String(crate::sessions::iso8601(std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms)))
    });
    serde_json::to_vec(&body).unwrap_or_default()
}

// ------------------------------------------------------------------ index

/// The rows to send: in-scope rows as `key → JSON`.
pub fn index_rows(rows: &[SessionSummary], scope: &Scope, now_ms: u64) -> BTreeMap<String, String> {
    rows.iter()
        .filter(|r| scope.admits(r, now_ms))
        .filter_map(|r| serde_json::to_string(r).ok().map(|json| (row_key(r), json)))
        .collect()
}

/// What changed between the rows the hub holds and the rows now.
pub fn index_diff(held: &BTreeMap<String, String>, now: &BTreeMap<String, String>) -> (Vec<(String, String)>, Vec<String>) {
    let upserts = now
        .iter()
        .filter(|(k, v)| held.get(*k) != Some(*v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let removed = held.keys().filter(|k| !now.contains_key(*k)).cloned().collect();
    (upserts, removed)
}

/// One index body, with the rows it upserts and the keys it removes.
pub type IndexBody = (Vec<u8>, Vec<(String, String)>, Vec<String>);

/// Index bodies for `upserts` and `removed`, each under [`BODY_BUDGET`]; the
/// first says `reset` when `reset` is set (a page set, or a rebuild). Each
/// comes with the rows it carries, for the cursor.
pub fn index_bodies(upserts: &[(String, String)], removed: &[String], reset: bool, as_of: u64) -> Vec<IndexBody> {
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
pub fn as_of_body(as_of: u64) -> Vec<u8> {
    format!("{{\"asOf\":{as_of}}}").into_bytes()
}

// ------------------------------------------------------------------ status

/// The machine status body: what the UI shows about the machine, never a
/// credential. `accounts` keeps only what names an account.
pub fn status_body(
    as_of: u64,
    display_name: Option<String>,
    mk_epoch: Option<u32>,
    settings: Option<Value>,
    accounts: Option<Value>,
    quota: Option<Value>,
    cli: Value,
) -> Vec<u8> {
    let accounts = accounts.map(|a| {
        let list: Vec<Value> = a["accounts"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|acc| {
                serde_json::json!({
                    "id": acc["id"], "label": acc["label"], "kind": acc["kind"], "provider": acc["provider"],
                    "email": acc["email"], "usageEnabled": acc["usageEnabled"],
                    // Whether it may stand in for a signed-out local login: an
                    // offline page that cannot tell says so wrongly.
                    "rotation": acc["rotation"],
                })
            })
            .collect();
        // Each local login's state and whether somebody chose it as the
        // default — what an offline Settings needs to show which default is set
        // — but not its rests: their reasons are a CLI's own words, and the
        // body has a budget.
        let login = |cli: &str| {
            let login = &a["local"][cli];
            if login.is_object() {
                serde_json::json!({"state": login["state"], "isDefault": login["isDefault"]})
            } else {
                Value::Null
            }
        };
        let local = if a["local"].is_object() {
            serde_json::json!({"claude": login("claude"), "codex": login("codex")})
        } else {
            Value::Null
        };
        serde_json::json!({
            "accounts": list,
            "defaultAccountId": a["defaultAccountId"],
            "defaultCodexAccountId": a["defaultCodexAccountId"],
            "local": local,
        })
    });
    let mut body = serde_json::json!({"asOf": as_of, "displayName": display_name, "mkEpoch": mk_epoch, "settings": settings, "accounts": accounts, "quota": quota, "cli": cli});
    if serde_json::to_vec(&body).map_or(0, |b| b.len()) > BODY_BUDGET {
        body["quota"] = Value::Null;
    }
    if serde_json::to_vec(&body).map_or(0, |b| b.len()) > BODY_BUDGET {
        body["accounts"] = Value::Null;
    }
    serde_json::to_vec(&body).unwrap_or_default()
}

/// A status body as JSON bytes, so adding a field is a step in the builder
/// chain and the parse/set/re-encode lives in one place.
trait StatusBodyExt {
    /// `self` with `key` set to `value`; a body that does not parse, or a
    /// `None`, is left untouched.
    fn with_field(self, key: &str, value: Option<Value>) -> Self;
}

impl StatusBodyExt for Vec<u8> {
    fn with_field(self, key: &str, value: Option<Value>) -> Self {
        let Some(value) = value else {
            return self;
        };
        let Ok(mut v) = serde_json::from_slice::<Value>(&self) else {
            return self;
        };
        v[key] = value;
        serde_json::to_vec(&v).unwrap_or(self)
    }
}

/// A status body with what this computer runs in it: `os` (`{name,
/// version, arch}`, [`crate::agent::system::os`]) and `agentVersion` (this
/// server's version). Older readers ignore both; the web app falls back to
/// `settings.platform` and `settings.serverVersion` without them.
pub fn with_system(body: Vec<u8>, os: &Value, agent_version: &str) -> Vec<u8> {
    body.with_field("os", Some(os.clone()))
        .with_field("agentVersion", Some(Value::String(agent_version.to_string())))
}

/// A status body with the machine's chain alarms in it (`alarms`: `[{atMs,
/// what}]`, oldest first; left out when there are none). Every device of the
/// account reads the status stream, so this is how a fork this machine saw
/// reaches all of them — the hub can withhold the record, but not forge or
/// alter it, and the machine-signed head says when it is behind.
pub fn with_alarms(body: Vec<u8>, alarms: &[crate::agent::state::Alarm]) -> Vec<u8> {
    body.with_field(
        "alarms",
        (!alarms.is_empty()).then(|| serde_json::to_value(alarms).unwrap_or(Value::Null)),
    )
}

/// A status body with the account changes this machine saw in it
/// (`changes`: `[{atMs, what}]`, oldest first, at most
/// [`crate::agent::state::MAX_CHANGES`]; left out when there are none):
/// machines and devices added or removed, recovery joins — every device of
/// the account reads them, whoever made the change.
pub fn with_changes(body: Vec<u8>, changes: &[crate::agent::state::Alarm]) -> Vec<u8> {
    body.with_field(
        "changes",
        (!changes.is_empty()).then(|| serde_json::to_value(changes).unwrap_or(Value::Null)),
    )
}

/// A status body with which devices ever opened a channel to this machine in
/// it (`controllers`: `{from, fromMs, seq, devices: [{id, firstMs}]}`,
/// [`crate::agent::state::Controllers`]), always — an empty list included,
/// `null` before the agent has a chain; a body without the field is from an
/// agent that does not keep the record. When a device that could control
/// this machine no longer can, the page says whether it ever connected. A
/// new device, or the chain moving on, is a new record.
pub fn with_controllers(body: Vec<u8>, controllers: Option<&crate::agent::state::Controllers>) -> Vec<u8> {
    let value = controllers.and_then(|c| serde_json::to_value(c).ok()).unwrap_or(Value::Null);
    body.with_field("controllers", Some(value))
}

/// The part of a status body that changing means a new record (not `asOf`).
pub fn status_fingerprint(body: &[u8]) -> String {
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
    use crate::feed::fingerprint_chain;
    use crate::sessions::util::TranscriptItem;

    fn items(n: usize, text_len: usize) -> Vec<TranscriptItem> {
        (0..n)
            .map(|i| {
                serde_json::from_value(serde_json::json!({
                    "role": "assistant", "kind": "text", "text": format!("{i}:{}", "x".repeat(text_len)),
                    "detail": null, "toolName": null, "meta": null, "isError": false, "question": null,
                    "timestamp": null, "truncated": false,
                }))
                .unwrap()
            })
            .collect()
    }

    #[test]
    fn a_delta_starts_at_the_newest_position_both_sides_agree_on() {
        let path = std::path::Path::new("/tmp/t.jsonl");
        let mut list = items(10, 5);
        let chain = fingerprint_chain(path, &list);
        let cursor = SessionCursor::at(0, 10, &chain, "s");
        assert_eq!(session_delta(&cursor, &chain), Delta::None);
        list.extend(items(2, 7));
        let grown = fingerprint_chain(path, &list);
        assert_eq!(session_delta(&cursor, &grown), Delta::From(10), "appended: from the old end");
        // The last item rewritten in place (a tool result attaching).
        let mut rewritten = items(10, 5);
        rewritten[9].text = "changed".into();
        let chain2 = fingerprint_chain(path, &rewritten);
        assert_eq!(session_delta(&cursor, &chain2), Delta::From(9));
        // Rewritten further back than the cursor remembers: rebuild.
        let mut early = items(10, 5);
        early[0].text = "changed".into();
        assert_eq!(session_delta(&cursor, &fingerprint_chain(path, &early)), Delta::Rebuild);
        // Another file entirely.
        assert_eq!(
            session_delta(&cursor, &fingerprint_chain(std::path::Path::new("/tmp/other"), &list)),
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
            assert_eq!(v["fp"], encode_fp(&chain[*end]));
            assert_eq!(v["prefixFp"], encode_fp(&chain[end - RESUME_OVERLAP]));
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
        list[1].detail = crate::openapi_ext::Nullable(Some("é".repeat(200_000)));
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

    fn summary(id: &str, scope: SessionScope, updated: &str) -> SessionSummary {
        let mut v: Value = serde_json::json!({
            "pinned": false, "id": id, "source": "claude", "scope": "owned", "server": "s", "title": "t",
            "cwd": null, "repoKey": null, "gitBranch": null, "updatedAt": updated, "createdAt": updated,
            "sizeBytes": 1, "filePath": "/x", "status": "idle", "activity": "idle", "resident": null,
            "claudeRoot": null, "accountId": null, "attentionAt": null, "unread": false, "unreadCount": 0,
        });
        v["scope"] = serde_json::to_value(scope).unwrap();
        match serde_json::from_value(v.clone()) {
            Ok(s) => s,
            Err(e) => panic!("a SessionSummary fixture: {e}: {v}"),
        }
    }

    #[test]
    fn the_sync_scope_is_own_sessions_of_the_last_thirty_days() {
        let now = crate::schedule::parse_iso("2026-10-15T12:00:00Z")
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let scope = Scope::default();
        assert!(scope.admits(&summary("a", SessionScope::Owned, "2026-10-14T00:00:00Z"), now));
        assert!(scope.admits(&summary("a", SessionScope::Owned, "2026-09-16T00:00:00Z"), now));
        assert!(
            !scope.admits(&summary("a", SessionScope::Owned, "2026-09-01T00:00:00Z"), now),
            "older than 30 days"
        );
        assert!(
            !scope.admits(&summary("a", SessionScope::External, "2026-10-14T00:00:00Z"), now),
            "~/.claude is not synced by default"
        );
        assert!(Scope { external: true, ..scope }.admits(&summary("a", SessionScope::External, "2026-10-14T00:00:00Z"), now));
        assert!(!scope.admits(&summary("a", SessionScope::Owned, "not a date"), now));
        let rows = vec![
            summary("a", SessionScope::Owned, "2026-10-14T00:00:00Z"),
            summary("b", SessionScope::External, "2026-10-14T00:00:00Z"),
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
                "fp": encode_fp(&chain[4]), "prefixFp": encode_fp(&chain[1]),
                "base": 4, "before": "2025-10-09T08:53:20.000Z",
            })
        );
        let undated: Value = serde_json::from_slice(&window_body(4, None, &chain)).unwrap();
        assert_eq!(undated["before"], Value::Null);
        // Item times: what parses, and nothing for what does not.
        let mut dated = items(3, 1);
        dated[0].timestamp = crate::openapi_ext::Nullable(Some("2026-10-15T08:00:01.000Z".into()));
        dated[2].timestamp = crate::openapi_ext::Nullable(Some("not a time".into()));
        assert_eq!(item_times(&dated), vec![Some(1_792_051_201_000), None, None]);
    }

    #[test]
    fn the_status_keeps_no_credentials_and_its_fingerprint_ignores_the_time() {
        let accounts = serde_json::json!({"accounts": [{"id": "a1", "label": "work", "kind": "oauth", "provider": "anthropic", "email": "e@x", "usageEnabled": true, "token": "SECRET", "envKeys": ["K"]}], "defaultAccountId": "a1"});
        let a = status_body(
            1,
            Some("mac".into()),
            Some(0),
            Some(serde_json::json!({"hostname": "mac"})),
            Some(accounts.clone()),
            None,
            serde_json::json!({"claude": "2.0"}),
        );
        let b = status_body(
            2,
            Some("mac".into()),
            Some(0),
            Some(serde_json::json!({"hostname": "mac"})),
            Some(accounts),
            None,
            serde_json::json!({"claude": "2.0"}),
        );
        let text = String::from_utf8(a.clone()).unwrap();
        assert!(!text.contains("SECRET") && !text.contains("envKeys"));
        assert_eq!(serde_json::from_slice::<Value>(&a).unwrap()["accounts"]["accounts"][0]["rotation"], Value::Null, "absent stays absent");
        assert_eq!(serde_json::from_slice::<Value>(&a).unwrap()["accounts"]["local"], Value::Null, "a list without local logins");
        assert_eq!(status_fingerprint(&a), status_fingerprint(&b));
        let c = status_body(
            3,
            Some("mac".into()),
            Some(0),
            Some(serde_json::json!({"hostname": "other"})),
            None,
            None,
            Value::Null,
        );
        assert_ne!(status_fingerprint(&a), status_fingerprint(&c));
        let renamed = status_body(
            4,
            Some("studio".into()),
            Some(0),
            Some(serde_json::json!({"hostname": "mac"})),
            None,
            None,
            Value::Null,
        );
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
        let body = status_body(1, Some("mac".into()), Some(0), None, Some(accounts), None, Value::Null);
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
        let body = status_body(1, Some("box".into()), Some(0), None, None, None, Value::Null);
        let os = serde_json::json!({"name": "macos", "version": "15.3", "arch": "aarch64"});
        let with = with_system(body.clone(), &os, "0.2.122");
        let v: Value = serde_json::from_slice(&with).unwrap();
        assert_eq!((v["os"]["name"].as_str(), v["agentVersion"].as_str()), (Some("macos"), Some("0.2.122")));
        assert_eq!(v["displayName"], "box", "the rest is as it was");
        assert_ne!(
            status_fingerprint(&body),
            status_fingerprint(&with),
            "an upgrade is a change worth writing"
        );
        assert_eq!(with_system(b"not json".to_vec(), &os, "x"), b"not json".to_vec());
    }

    #[test]
    fn account_changes_go_into_the_status_and_change_it() {
        use crate::agent::state::Alarm;
        let body = status_body(1, Some("box".into()), Some(0), None, None, None, Value::Null);
        assert_eq!(with_changes(body.clone(), &[]), body, "no changes, no field");
        let changes = [Alarm {
            at_ms: 7,
            what: "device m-2 (runs AI) was added by d-1".into(),
        }];
        let with = with_changes(body.clone(), &changes);
        let v: Value = serde_json::from_slice(&with).unwrap();
        assert_eq!(v["changes"][0]["atMs"], 7);
        assert_eq!(v["changes"][0]["what"], "device m-2 (runs AI) was added by d-1");
        assert_ne!(status_fingerprint(&with), status_fingerprint(&body), "a new change is a new record");
    }

    #[test]
    fn the_devices_that_opened_a_channel_are_always_in_the_status_and_change_it() {
        use crate::agent::state::{Controller, Controllers};
        let body = status_body(1, Some("box".into()), Some(0), None, None, None, Value::Null);
        let nobody = Controllers {
            from: 0,
            from_ms: 1_000,
            seq: 4,
            devices: Vec::new(),
        };
        let empty = with_controllers(body.clone(), Some(&nobody));
        let v: Value = serde_json::from_slice(&empty).unwrap();
        assert_eq!(
            v["controllers"],
            serde_json::json!({"from": 0, "fromMs": 1_000, "seq": 4, "devices": []}),
            "there even when nobody opened a channel: its absence means an agent that does not say"
        );
        assert_eq!(v["displayName"], "box", "the rest is as it was");
        assert_ne!(status_fingerprint(&empty), status_fingerprint(&body));
        let one = Controllers {
            devices: vec![Controller {
                id: "d-1".into(),
                first_ms: 2_000,
            }],
            ..nobody.clone()
        };
        let named: Value = serde_json::from_slice(&with_controllers(body.clone(), Some(&one))).unwrap();
        assert_eq!(named["controllers"]["devices"], serde_json::json!([{"id": "d-1", "firstMs": 2_000}]));
        let with_one = with_controllers(body.clone(), Some(&one));
        assert_ne!(status_fingerprint(&with_one), status_fingerprint(&empty), "a device that opened a channel is a new record");
        let moved = Controllers { seq: 5, ..nobody };
        assert_ne!(
            status_fingerprint(&with_controllers(body.clone(), Some(&moved))),
            status_fingerprint(&empty),
            "and so is the chain moving on"
        );
        let unknown: Value = serde_json::from_slice(&with_controllers(body.clone(), None)).unwrap();
        assert_eq!(unknown["controllers"], Value::Null, "no chain yet: nothing to say");
        assert_eq!(with_controllers(b"not json".to_vec(), Some(&one)), b"not json".to_vec());
    }

    #[test]
    fn alarms_go_into_the_status_and_change_it() {
        use crate::agent::state::Alarm;
        let body = status_body(5, Some("studio".into()), Some(0), None, None, None, Value::Null);
        assert_eq!(with_alarms(body.clone(), &[]), body, "no alarms, no field");
        let alarms = [Alarm {
            at_ms: 4,
            what: "the hub served a device chain that conflicts with this device's".into(),
        }];
        let with = with_alarms(body.clone(), &alarms);
        let v: Value = serde_json::from_slice(&with).unwrap();
        assert_eq!(v["alarms"][0]["atMs"], 4);
        assert!(v["alarms"][0]["what"].as_str().unwrap().contains("conflicts"));
        assert_ne!(status_fingerprint(&with), status_fingerprint(&body), "a new alarm is a new record");
    }
}
