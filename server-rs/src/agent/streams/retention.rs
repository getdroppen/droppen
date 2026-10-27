//! Retention and the storage quota, the agent's half (`docs/relay-and-plans.md`
//! §3.4, §3.5). Pure decisions; the sync task ([`super`]) acts on them.
//!
//! - **The plan.** The hub tells the agent its account's plan after it signs
//!   in, on every change, after a write it refused for the quota, and hourly:
//!   `{"t": "plan", "plan", "retention_days", "storage_bytes" (null: no
//!   limit), "usage_bytes" (the whole account), "machine_usage_bytes" (this
//!   machine's pages), "grace_until_ms"?}` ([`HubPlan`]). The agent keeps the
//!   last one in `streams.db`, so a restart starts with it; before any has
//!   arrived (a hub from before plans) the copy is bounded by the 30-day
//!   default, and nothing is trimmed or evicted.
//! - **The window.** Sessions are copied while updated within
//!   min(`DROPPEN_AGENT_SYNC_DAYS`, `retention_days`) days ([`window_days`]); the
//!   agent deletes the copies of those outside it itself, a day before the
//!   hub's sweep would. A downgrade narrows it only when the hub's plan says
//!   so (the hub applies the grace); an upgrade widens it and the backfill
//!   brings the sessions back in, newest first.
//! - **Daily writes.** The hub expires a stream by its last *content* write,
//!   so the index and the status streams get one at least every
//!   [`REFRESH_EVERY_MS`] even when nothing changed ([`refresh_due`]): only a
//!   machine offline for longer than the retention loses them.
//! - **Space.** Above [`EVICT_ABOVE`] of the quota the agent deletes its own
//!   session copies, least recently updated first, never one updated in the
//!   last [`EVICT_SPARES_MS`], until the estimate is down to [`EVICT_TO`]
//!   ([`evictions`]); each machine does this for itself. An evicted session
//!   stays out of the copy until it changes again (and space allows).
//! - **Long live sessions.** A checkpoint drops the leading pages all of
//!   whose entries are older than `retention_days` ([`window_start`],
//!   [`first_kept`]) and starts the copy with a window-start page
//!   ([`super::content::window_body`]).

use rh_proto::record::StreamId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::store::{Kind, PageRef, Row};

pub const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// The index and status streams get a content write at least this often.
pub const REFRESH_EVERY_MS: u64 = DAY_MS;
/// Copies are deleted for space once the account's usage is above this share of its quota…
pub const EVICT_ABOVE: f64 = 0.90;
/// …down to this share.
pub const EVICT_TO: f64 = 0.85;
/// A session updated within this is never deleted for space.
pub const EVICT_SPARES_MS: u64 = DAY_MS;
/// A checkpoint only to trim a copy's old pages, at most this often per stream.
pub const TRIM_EVERY_MS: u64 = 60 * 60 * 1000;
/// The most session copies deleted (out of the window) in one round.
pub const DELETES_PER_ROUND: usize = 16;

/// The account's plan as the hub last told it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubPlan {
    pub plan: String,
    /// How many days the hub keeps what was written (≥ 1).
    pub retention_days: u64,
    /// The account's storage quota (`None`: no limit).
    pub storage_bytes: Option<u64>,
    /// What the whole account stores now.
    pub usage_bytes: u64,
    /// This machine's share of it (its streams' pages).
    pub machine_usage_bytes: Option<u64>,
    /// While a downgrade's grace lasts (the hub already applies it).
    pub grace_until_ms: Option<u64>,
    /// When this agent heard it.
    pub received_ms: u64,
}

impl HubPlan {
    /// A `plan` control message, if it is one this agent can act on.
    pub fn parse(v: &Value, now_ms: u64) -> Option<Self> {
        if v["t"].as_str() != Some("plan") {
            return None;
        }
        let days = v["retention_days"].as_i64()?;
        let non_negative = |key: &str| v[key].as_i64().map(|n| n.max(0) as u64);
        Some(Self {
            plan: v["plan"].as_str().unwrap_or("").chars().take(64).collect(),
            retention_days: days.max(1) as u64,
            storage_bytes: non_negative("storage_bytes"),
            usage_bytes: non_negative("usage_bytes").unwrap_or(0),
            machine_usage_bytes: non_negative("machine_usage_bytes"),
            grace_until_ms: v["grace_until_ms"].as_u64(),
            received_ms: now_ms,
        })
    }
}

/// How many days of sessions are copied: the agent's own setting, or less if
/// the plan keeps less.
pub fn window_days(env_days: u64, plan: Option<&HubPlan>) -> u64 {
    plan.map_or(env_days, |p| env_days.min(p.retention_days))
}

/// Entries older than this are trimmed from a copy (only under a plan).
pub fn trim_cutoff(plan: Option<&HubPlan>, now_ms: u64) -> Option<u64> {
    plan.map(|p| now_ms.saturating_sub(p.retention_days.saturating_mul(DAY_MS)))
}

/// Whether a stream last written (content) at `last_write_ms` needs a write now
/// to stay on the hub.
pub fn refresh_due(last_write_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(last_write_ms) >= REFRESH_EVERY_MS
}

/// What the account is estimated to store: the hub's figure less what this
/// agent deleted since it was told.
pub fn usage_estimate(plan: &HubPlan, freed: u64) -> u64 {
    plan.usage_bytes.saturating_sub(freed)
}

/// Whether there is room to copy a session again: no quota, or not above
/// [`EVICT_ABOVE`] of it.
pub fn space_allows(plan: Option<&HubPlan>, freed: u64) -> bool {
    match plan.and_then(|p| p.storage_bytes.map(|limit| (p, limit))) {
        Some((p, limit)) => (usage_estimate(p, freed) as f64) <= limit as f64 * EVICT_ABOVE,
        None => true,
    }
}

/// One of this machine's session copies that space could take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub stream: StreamId,
    /// When the session was last updated (ms).
    pub updated_ms: u64,
    /// What its copy is estimated to hold on the hub.
    pub bytes: u64,
}

/// The copies to delete to bring the account from `usage` down to
/// [`EVICT_TO`] of `limit`, least recently updated first and never one
/// updated within [`EVICT_SPARES_MS`]; none unless `usage` is above
/// [`EVICT_ABOVE`] of it. It may not get there (what is left is too recent):
/// writes are then refused until the account has room.
pub fn evictions(mut candidates: Vec<Candidate>, usage: u64, limit: u64, now_ms: u64) -> Vec<Candidate> {
    if (usage as f64) <= limit as f64 * EVICT_ABOVE {
        return Vec::new();
    }
    let target = (limit as f64 * EVICT_TO) as u64;
    candidates.retain(|c| now_ms.saturating_sub(c.updated_ms) >= EVICT_SPARES_MS);
    candidates.sort_by_key(|c| (c.updated_ms, c.stream.0));
    let mut left = usage;
    let mut out = Vec::new();
    for c in candidates {
        if left <= target {
            break;
        }
        left = left.saturating_sub(c.bytes);
        out.push(c);
    }
    out
}

impl Row {
    /// What a stream's copy is estimated to hold on the hub: its pages (as
    /// sealed, when known; else the state they hold) and its tail.
    pub fn stream_bytes(&self) -> u64 {
        let pages: u64 = self.pages.iter().map(|p| p.size).sum();
        pages.max(self.state_bytes.max(0) as u64) + self.tail_bytes.max(0) as u64
    }
}

/// Where a copy kept to entries from `cutoff` on starts, given each item's
/// time (`None`: undated): just past the last dated entry older than the
/// cutoff that comes before the first dated entry within it — undated items
/// among the old ones go with them. `0` when nothing is provably old, and
/// when no entry is dated within the window (a copy is never trimmed empty).
pub fn window_start(times: &[Option<u64>], cutoff: u64) -> usize {
    let mut last_old = None;
    for (i, t) in times.iter().enumerate() {
        match t {
            Some(t) if *t >= cutoff => return last_old.map_or(0, |j| j + 1),
            Some(_) => last_old = Some(i),
            None => {}
        }
    }
    0
}

/// The newest time among `times` (`None` when none is dated).
pub fn newest(times: &[Option<u64>]) -> Option<u64> {
    times.iter().flatten().copied().max()
}

/// Of the last manifest's item pages (the window page left out), the first a
/// copy holding entries from item `k` on keeps: the leading pages wholly
/// before `k` go, every later one stays as it is.
pub fn first_kept(pages: &[&PageRef], k: usize) -> usize {
    pages.iter().take_while(|p| p.to <= k && p.to > p.from).count()
}

/// Whether a session copy holds a leading page all of whose entries are older
/// than `cutoff`, worth a checkpoint of its own to drop it: at most every
/// [`TRIM_EVERY_MS`] (a busy session is trimmed at its next compaction
/// anyway), or at once if its last checkpoint is from before `since` (when the
/// retention last changed).
pub fn trim_due(row: &Row, cutoff: Option<u64>, since: u64, now_ms: u64) -> bool {
    let Some(cutoff) = cutoff else { return false };
    if row.kind != Kind::Session || row.checkpoint_ms.is_some_and(|t| now_ms.saturating_sub(t) < TRIM_EVERY_MS && t >= since) {
        return false;
    }
    let pages: Vec<&PageRef> = row.pages.iter().filter(|p| !p.window && p.to > p.from).collect();
    pages.len() >= 2 && pages[0].last_ms.is_some_and(|t| t < cutoff)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(days: u64, quota: Option<u64>, usage: u64) -> HubPlan {
        HubPlan {
            plan: "free".into(),
            retention_days: days,
            storage_bytes: quota,
            usage_bytes: usage,
            machine_usage_bytes: Some(usage / 2),
            grace_until_ms: None,
            received_ms: 1,
        }
    }

    #[test]
    fn the_plan_message_is_read_leniently() {
        let v = serde_json::json!({"t": "plan", "plan": "free", "retention_days": 7, "storage_bytes": 5_368_709_120u64, "usage_bytes": 123, "machine_usage_bytes": 45});
        let p = HubPlan::parse(&v, 9).unwrap();
        assert_eq!(
            (p.retention_days, p.storage_bytes, p.usage_bytes, p.machine_usage_bytes, p.received_ms),
            (7, Some(5_368_709_120), 123, Some(45), 9)
        );
        let unlimited = serde_json::json!({"t": "plan", "plan": "internal", "retention_days": 3650, "storage_bytes": null, "usage_bytes": 0, "grace_until_ms": 77});
        let p = HubPlan::parse(&unlimited, 1).unwrap();
        assert_eq!((p.storage_bytes, p.grace_until_ms, p.machine_usage_bytes), (None, Some(77), None));
        assert_eq!(
            HubPlan::parse(&serde_json::json!({"t": "plan", "retention_days": 0}), 1)
                .unwrap()
                .retention_days,
            1
        );
        assert!(HubPlan::parse(&serde_json::json!({"t": "plan"}), 1).is_none(), "no retention, no plan");
        assert!(HubPlan::parse(&serde_json::json!({"t": "mail", "retention_days": 7}), 1).is_none());
        // It round-trips through streams.db.
        let p = HubPlan::parse(&v, 9).unwrap();
        assert_eq!(serde_json::from_str::<HubPlan>(&serde_json::to_string(&p).unwrap()).unwrap(), p);
    }

    #[test]
    fn the_window_is_the_smaller_of_the_setting_and_the_plan_and_unchanged_without_one() {
        assert_eq!(window_days(30, None), 30, "a hub without plans: as before");
        assert_eq!(window_days(30, Some(&plan(7, None, 0))), 7, "free");
        assert_eq!(window_days(30, Some(&plan(30, None, 0))), 30, "pro");
        assert_eq!(window_days(30, Some(&plan(3650, None, 0))), 30, "internal: the setting");
        assert_eq!(window_days(3, Some(&plan(7, None, 0))), 3);
        let now = 100 * DAY_MS;
        assert_eq!(trim_cutoff(None, now), None, "no trimming without a plan");
        assert_eq!(trim_cutoff(Some(&plan(7, None, 0)), now), Some(93 * DAY_MS));
    }

    #[test]
    fn index_and_status_are_written_at_least_daily() {
        let now = 10 * DAY_MS;
        assert!(!refresh_due(now - 1000, now));
        assert!(!refresh_due(now - DAY_MS + 1, now));
        assert!(refresh_due(now - DAY_MS, now));
        assert!(refresh_due(0, now));
    }

    fn cand(n: u8, updated_ms: u64, bytes: u64) -> Candidate {
        Candidate {
            stream: StreamId([n; 32]),
            updated_ms,
            bytes,
        }
    }

    #[test]
    fn space_is_made_from_the_least_recently_updated_copies_down_to_85_percent() {
        let now = 30 * DAY_MS;
        let cands = vec![
            cand(1, now - 2 * DAY_MS, 30),
            cand(2, now - 10 * DAY_MS, 40),
            cand(3, now - 3600 * 1000, 500), // updated an hour ago: spared
            cand(4, now - 5 * DAY_MS, 25),
        ];
        // At 90% exactly: nothing.
        assert!(evictions(cands.clone(), 900, 1000, now).is_empty());
        // 95% → 85%: the oldest (2, 40 B) → 910, the next (4, 25 B) → 885, then 1 (30 B) → 855; what is
        // left is 3, updated within the day: spared, so it stops short.
        let chosen: Vec<u8> = evictions(cands.clone(), 950, 1000, now).iter().map(|c| c.stream.0[0]).collect();
        assert_eq!(chosen, vec![2, 4, 1]);
        // Only as many as it takes.
        let chosen: Vec<u8> = evictions(cands.clone(), 910, 1000, now).iter().map(|c| c.stream.0[0]).collect();
        assert_eq!(chosen, vec![2, 4]);
        assert!(space_allows(None, 0));
        assert!(space_allows(Some(&plan(7, None, u64::MAX)), 0), "no quota");
        assert!(!space_allows(Some(&plan(7, Some(1000), 950)), 0));
        assert!(space_allows(Some(&plan(7, Some(1000), 950)), 100), "what was freed since counts");
    }

    #[test]
    fn a_copy_starts_past_the_old_entries_and_never_ends_up_empty() {
        let cutoff = 100;
        let t = |v: u64| Some(v);
        // The undated entry between two old ones goes with them; the one just
        // before the first new one is not provably old, and stays.
        assert_eq!(window_start(&[t(1), None, t(2), None, t(150), t(160)], cutoff), 3);
        // An undated start before any old entry is not provably old: kept.
        assert_eq!(window_start(&[None, None, t(150)], cutoff), 0);
        assert_eq!(window_start(&[t(1), t(2)], cutoff), 0, "nothing within the window: not trimmed");
        assert_eq!(window_start(&[None, None], cutoff), 0, "undated");
        assert_eq!(window_start(&[], cutoff), 0);
        assert_eq!(window_start(&[t(150), t(1), t(160)], cutoff), 0, "the first entry is new");
        assert_eq!(newest(&[t(3), None, t(9), t(4)]), Some(9));
        assert_eq!(newest(&[None]), None);
    }

    fn page(from: usize, to: usize, last_ms: Option<u64>, window: bool) -> PageRef {
        PageRef {
            hash: format!("{from}-{to}"),
            from,
            to,
            fp: String::new(),
            epoch: 0,
            window,
            last_ms,
            size: 100,
        }
    }

    #[test]
    fn whole_old_pages_go_from_the_head_and_the_rest_stay() {
        let pages = [page(0, 10, Some(5), false), page(10, 20, Some(50), false), page(20, 25, Some(150), false)];
        let refs: Vec<&PageRef> = pages.iter().collect();
        assert_eq!(first_kept(&refs, 0), 0);
        assert_eq!(first_kept(&refs, 9), 0, "a page with one entry inside the window stays whole");
        assert_eq!(first_kept(&refs, 10), 1);
        assert_eq!(first_kept(&refs, 22), 2);
        assert_eq!(first_kept(&refs, 25), 3);
    }

    fn session_row(pages: Vec<PageRef>, checkpoint_ms: Option<u64>) -> Row {
        Row {
            stream: StreamId([1; 32]),
            kind: Kind::Session,
            source: "claude".into(),
            id: "s".into(),
            epoch: 0,
            name_epoch: None,
            next_seq: 3,
            link: [0; 32],
            cursor: String::new(),
            pending: None,
            manifest_upto: Some(3),
            pages,
            tail_records: 0,
            tail_bytes: 10,
            state_bytes: 150,
            checkpoint_ms,
            tail_since_ms: None,
            updated_ms: 1,
        }
    }

    #[test]
    fn a_copy_with_an_old_leading_page_is_due_a_trim_at_most_hourly() {
        let now = 100 * DAY_MS;
        let cutoff = Some(now - 7 * DAY_MS);
        let old = Some(now - 8 * DAY_MS);
        let new = Some(now - DAY_MS);
        let row = session_row(vec![page(0, 10, old, false), page(10, 20, new, false)], None);
        assert!(trim_due(&row, cutoff, 0, now));
        assert!(!trim_due(&row, None, 0, now), "no plan, no trimming");
        assert!(
            !trim_due(&session_row(row.pages.clone(), Some(now - 60_000)), cutoff, 0, now),
            "not again within the hour"
        );
        assert!(
            trim_due(&session_row(row.pages.clone(), Some(now - 60_000)), cutoff, now - 1000, now),
            "unless the retention changed since"
        );
        assert!(!trim_due(
            &session_row(vec![page(0, 10, new, false), page(10, 20, new, false)], None),
            cutoff,
            0,
            now
        ));
        assert!(
            !trim_due(&session_row(vec![page(5, 5, None, true), page(5, 10, old, false)], None), cutoff, 0, now),
            "the last page is never trimmed on its own"
        );
        assert!(
            !trim_due(
                &session_row(vec![page(0, 10, old, false), page(10, 10, None, false)], None),
                cutoff,
                0,
                now
            ),
            "nor beside an empty one"
        );
        assert!(
            !trim_due(
                &session_row(vec![page(0, 10, None, false), page(10, 20, new, false)], None),
                cutoff,
                0,
                now
            ),
            "an undated page (from before this agent dated them) waits for a compaction"
        );
        let mut index = row.clone();
        index.kind = Kind::Index;
        assert!(!trim_due(&index, cutoff, 0, now));
        assert_eq!(row.stream_bytes(), 200 + 10, "pages as sealed, and the tail");
        assert_eq!(
            session_row(Vec::new(), None).stream_bytes(),
            150 + 10,
            "or the state, when page sizes are unknown"
        );
    }
}
