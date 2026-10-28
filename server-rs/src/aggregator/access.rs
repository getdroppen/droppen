//! Which of the other devices' streams this computer follows (local-aggregator
//! §6.3, S1-5). Pure, for tests.
//!
//! Only hosts that accept this computer's control (review D2:
//! [`rh_proto::chain::ChainState::hosts_admitting`]) and whose keys it holds
//! — never itself. Of each:
//!
//! - **the index and the status**, each under the newest few machine-key
//!   epochs held ([`NEWEST_EPOCHS`]): a stream moves to its newest epoch's
//!   name after a rotation, and a host that was off since has not moved yet.
//!   Once a name is known to exist, the older ones are let go; newer ones
//!   stay followed, for the move;
//! - **the newest sessions** by the index's `updatedAt`
//!   ([`SESSIONS_PER_HOST`]), each named under its index entry's epoch
//!   (`se`), or else the first epoch held.
//!
//! - **the sessions a page opened through the local gateway** (S1-7, §6.3's
//!   "打开过的最近 20 个"): ahead of the newest, most recently opened first,
//!   at most [`OPENED_MAX`] — named as the newest are.
//!
//! At most [`MAX_SUBS`] streams on the one connection (three quarters of the
//! hub's 256): every index and status first, then the opened sessions, then
//! the newest, while there is room.

use std::collections::{BTreeMap, HashMap};

use rh_client::streams::IndexCopy;
use rh_proto::record::StreamId;

use super::store::Kind;

/// The most streams followed on the one connection.
pub const MAX_SUBS: usize = 192;
/// The newest epochs whose index and status names are followed.
pub const NEWEST_EPOCHS: usize = 3;
/// The newest sessions of a host followed.
pub const SESSIONS_PER_HOST: usize = 5;
/// The sessions opened through the local gateway followed at once (S1-7).
pub const OPENED_MAX: usize = 20;

/// One stream to follow.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Want {
    /// The host it is of.
    pub host: String,
    /// What it is.
    pub kind: Kind,
    /// `source/id` for a session; empty otherwise.
    pub name: String,
    /// The epoch its name is under.
    pub epoch: u32,
    /// Its name.
    pub stream: StreamId,
}

/// A host followed: its machine-key epochs held (any order) and the keys.
pub struct Host<'a> {
    /// Its id.
    pub id: &'a str,
    /// `(epoch, MK)` held.
    pub keys: &'a [(u32, [u8; 32])],
}

/// The index or status names of `host` to follow: the newest
/// [`NEWEST_EPOCHS`] epochs held, less those older than the newest one known
/// to exist (`exists`).
pub fn epoch_names(host: &Host<'_>, kind: Kind, exists: &HashMap<StreamId, bool>) -> Vec<Want> {
    let name = |mk: &[u8; 32]| match kind {
        Kind::Status => rh_proto::stream::status_stream(mk),
        _ => rh_proto::stream::index_stream(mk),
    };
    let mut newest: Vec<&(u32, [u8; 32])> = host.keys.iter().collect();
    newest.sort_by_key(|(e, _)| std::cmp::Reverse(*e));
    newest.truncate(NEWEST_EPOCHS);
    let wants: Vec<Want> = newest
        .into_iter()
        .map(|(epoch, mk)| Want {
            host: host.id.to_string(),
            kind,
            name: String::new(),
            epoch: *epoch,
            stream: name(mk),
        })
        .collect();
    match wants.iter().find(|w| exists.get(&w.stream) == Some(&true)).map(|w| w.epoch) {
        Some(found) => wants.into_iter().filter(|w| w.epoch >= found).collect(),
        None => wants,
    }
}

/// When (ms) an index row was last updated; rows that do not say sort last.
fn updated_ms(row: &serde_json::Value) -> u64 {
    row["updatedAt"]
        .as_str()
        .and_then(droppen_base::time::parse_iso)
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

/// The want of `host`'s session `key` (`source/id`) as its `index` names it:
/// under its entry's epoch (`se`), or else the first held. `None`: not in
/// the index, or no key for that epoch.
pub fn session(host: &Host<'_>, index: &IndexCopy, key: &str) -> Option<Want> {
    let row = index.rows.get(key)?;
    let (source, id) = (row["source"].as_str()?, row["id"].as_str()?);
    let stream = index.session_stream(host.keys, source, id)?;
    let epoch = match row.get("se") {
        Some(se) => u32::try_from(se.as_u64()?).ok()?,
        None => host.keys.iter().map(|(e, _)| *e).min()?,
    };
    Some(Want {
        host: host.id.to_string(),
        kind: Kind::Session,
        name: key.to_string(),
        epoch,
        stream,
    })
}

/// The newest [`SESSIONS_PER_HOST`] sessions of `host` in its `index`, with
/// when each was updated — those whose name it has a key for.
pub fn sessions(host: &Host<'_>, index: &IndexCopy) -> Vec<(u64, Want)> {
    let mut rows: Vec<(u64, &String, &serde_json::Value)> = index.rows.iter().map(|(k, v)| (updated_ms(v), k, v)).collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    rows.into_iter()
        .filter_map(|(at, key, _)| Some((at, session(host, index, key)?)))
        .take(SESSIONS_PER_HOST)
        .collect()
}

/// Everything to follow: every host's index and status names, then the
/// sessions opened through the local gateway (`opened`: `(host, source/id)`,
/// most recently opened first; at most [`OPENED_MAX`]), then the sessions of
/// the indexes known (`indexes`: host → its index as copied), newest first —
/// each stream once, within [`MAX_SUBS`].
pub fn plan(hosts: &[Host<'_>], indexes: &BTreeMap<String, IndexCopy>, exists: &HashMap<StreamId, bool>, opened: &[(String, String)]) -> Vec<Want> {
    let mut out: Vec<Want> = Vec::new();
    for host in hosts {
        out.extend(epoch_names(host, Kind::Index, exists));
        out.extend(epoch_names(host, Kind::Status, exists));
    }
    out.truncate(MAX_SUBS);
    let mut newest: Vec<(u64, Want)> = hosts
        .iter()
        .filter_map(|h| indexes.get(h.id).map(|index| sessions(h, index)))
        .flatten()
        .collect();
    newest.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let mut seen: std::collections::HashSet<(String, StreamId)> = out.iter().map(|w| (w.host.clone(), w.stream)).collect();
    // An opened session that is among the newest anyway takes none of the
    // opened ones' room.
    let among_newest: std::collections::HashSet<(String, StreamId)> = newest.iter().map(|(_, w)| (w.host.clone(), w.stream)).collect();
    let opened: Vec<Want> = opened
        .iter()
        .filter_map(|(id, key)| {
            let host = hosts.iter().find(|h| h.id == id)?;
            session(host, indexes.get(id)?, key)
        })
        .filter(|w| !among_newest.contains(&(w.host.clone(), w.stream)) && !seen.contains(&(w.host.clone(), w.stream)))
        .take(OPENED_MAX)
        .collect();
    for want in opened.into_iter().chain(newest.into_iter().map(|(_, w)| w)) {
        if out.len() >= MAX_SUBS {
            break;
        }
        if seen.insert((want.host.clone(), want.stream)) {
            out.push(want);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn index(rows: &[(&str, &str, Option<u32>)]) -> IndexCopy {
        let mut copy = IndexCopy::default();
        for (id, at, se) in rows {
            let mut row = json!({"source": "claude", "id": id, "updatedAt": at});
            if let Some(se) = se {
                row["se"] = json!(se);
            }
            copy.rows.insert(format!("claude/{id}"), row);
        }
        copy
    }

    #[test]
    fn the_index_follows_the_newest_epoch_names_until_one_is_known_to_exist() {
        let keys = [(0, [1; 32]), (1, [2; 32]), (2, [3; 32]), (3, [4; 32])];
        let host = Host { id: "m-2", keys: &keys };
        let mut exists = HashMap::new();
        let epochs = |w: Vec<Want>| w.iter().map(|w| w.epoch).collect::<Vec<_>>();
        // Nothing known: the newest three.
        assert_eq!(epochs(epoch_names(&host, Kind::Index, &exists)), vec![3, 2, 1]);
        // Epoch 2's exists, 3's not yet: 3 stays (the move), 1 goes.
        exists.insert(rh_proto::stream::index_stream(&[3; 32]), true);
        exists.insert(rh_proto::stream::index_stream(&[4; 32]), false);
        assert_eq!(epochs(epoch_names(&host, Kind::Index, &exists)), vec![3, 2]);
        // The status has names of its own.
        assert_eq!(epochs(epoch_names(&host, Kind::Status, &exists)), vec![3, 2, 1]);
        let w = &epoch_names(&host, Kind::Status, &exists)[0];
        assert_eq!(w.stream, rh_proto::stream::status_stream(&[4; 32]));
    }

    #[test]
    fn the_newest_five_sessions_are_followed_under_their_own_epoch() {
        let keys = [(0, [1; 32]), (1, [2; 32])];
        let host = Host { id: "m-2", keys: &keys };
        let idx = index(&[
            ("a", "2026-10-19T01:00:00.000Z", None),
            ("b", "2026-10-19T02:00:00.000Z", Some(1)),
            ("c", "2026-10-19T03:00:00.000Z", None),
            ("d", "2026-10-19T04:00:00.000Z", None),
            ("e", "2026-10-19T05:00:00.000Z", None),
            ("f", "2026-10-19T06:00:00.000Z", Some(9)),
            ("g", "2026-10-19T00:00:00.000Z", None),
        ]);
        let got = sessions(&host, &idx);
        let names: Vec<&str> = got.iter().map(|(_, w)| w.name.as_str()).collect();
        // f has no key (epoch 9): skipped; the newest five of the rest.
        assert_eq!(names, vec!["claude/e", "claude/d", "claude/c", "claude/b", "claude/a"]);
        let b = &got.iter().find(|(_, w)| w.name == "claude/b").unwrap().1;
        assert_eq!((b.epoch, b.stream), (1, rh_proto::record::stream_id(&[2; 32], "claude", "b").unwrap()));
        let a = &got.iter().find(|(_, w)| w.name == "claude/a").unwrap().1;
        assert_eq!(a.epoch, 0, "no se: the first epoch");
    }

    #[test]
    fn sessions_get_the_room_left_after_every_index_and_status() {
        let keys: Vec<(u32, [u8; 32])> = (0..3).map(|e| (e, [e as u8 + 1; 32])).collect();
        let ids: Vec<String> = (0..30).map(|i| format!("m-{i}")).collect();
        let hosts: Vec<Host<'_>> = ids.iter().map(|id| Host { id, keys: &keys }).collect();
        let mut indexes = BTreeMap::new();
        for (i, id) in ids.iter().enumerate() {
            let at = format!("2026-10-19T{:02}:00:00.000Z", i % 24);
            indexes.insert(id.clone(), index(&[("s1", &at, None), ("s2", &at, None)]));
        }
        let plan = plan(&hosts, &indexes, &HashMap::new(), &[]);
        assert_eq!(plan.len(), MAX_SUBS);
        let firsts = plan.iter().filter(|w| w.kind != Kind::Session).count();
        assert_eq!(firsts, 30 * 2 * 3, "every index and status name first");
        // The room left goes to the newest sessions (hosts 23, 22, …).
        let newest: Vec<&str> = plan[firsts..].iter().map(|w| w.host.as_str()).collect();
        assert_eq!(newest.len(), MAX_SUBS - 180);
        assert!(newest.starts_with(&["m-23", "m-23", "m-22", "m-22"]), "{newest:?}");
        // Fewer hosts: everything fits.
        let small = plan_for(&hosts[..2], &indexes);
        assert_eq!(small.iter().filter(|w| w.kind == Kind::Session).count(), 4);
        assert!(small.iter().all(|w| w.host != "m-me"));
    }

    fn plan_for(hosts: &[Host<'_>], indexes: &BTreeMap<String, IndexCopy>) -> Vec<Want> {
        plan(hosts, indexes, &HashMap::new(), &[])
    }

    /// S1-7: a session a page opened is followed ahead of the newest — once,
    /// even when it is one of them — and only while its index names it.
    #[test]
    fn opened_sessions_come_before_the_newest_and_only_once() {
        let keys = [(0, [1; 32])];
        let hosts = [Host { id: "m-2", keys: &keys }, Host { id: "m-3", keys: &keys }];
        let mut indexes = BTreeMap::new();
        let rows: Vec<(String, String)> = (0..8).map(|i| (format!("s{i}"), format!("2026-10-19T0{i}:00:00.000Z"))).collect();
        let rows: Vec<(&str, &str, Option<u32>)> = rows.iter().map(|(id, at)| (id.as_str(), at.as_str(), None)).collect();
        indexes.insert("m-2".to_string(), index(&rows));
        let opened = vec![
            ("m-2".to_string(), "claude/s0".to_string()),
            ("m-2".to_string(), "claude/s7".to_string()),
            ("m-2".to_string(), "claude/gone".to_string()),
            ("m-3".to_string(), "claude/s1".to_string()),
            ("m-9".to_string(), "claude/s1".to_string()),
        ];
        let plan = plan(&hosts, &indexes, &HashMap::new(), &opened);
        let sessions: Vec<&str> = plan.iter().filter(|w| w.kind == Kind::Session).map(|w| w.name.as_str()).collect();
        // s0 (oldest, opened) first, s7 (opened and newest) once; not in the
        // index, an unknown host, a host whose index is not known: nothing.
        assert_eq!(
            sessions,
            vec!["claude/s0", "claude/s7", "claude/s6", "claude/s5", "claude/s4", "claude/s3"]
        );
        assert!(plan.iter().all(|w| w.kind != Kind::Session || w.host == "m-2"));
        // At most OPENED_MAX of them.
        let many: Vec<(String, String)> = (0..8).cycle().take(40).map(|i| ("m-2".to_string(), format!("claude/s{i}"))).collect();
        let plan = super::plan(&hosts, &indexes, &HashMap::new(), &many);
        assert_eq!(plan.iter().filter(|w| w.kind == Kind::Session).count(), 8, "each once");
    }

    /// Review of S1-7: opened sessions that are among the newest anyway take
    /// none of the [`OPENED_MAX`] room for the others.
    #[test]
    fn the_newest_opened_do_not_use_up_the_room_for_the_others() {
        let keys = [(0, [1; 32])];
        let hosts = [Host { id: "m-2", keys: &keys }];
        let rows: Vec<(String, String)> = (0..40)
            .map(|i| (format!("s{i:02}"), format!("2026-10-{:02}T00:00:00.000Z", 1 + i % 28)))
            .collect();
        let rows: Vec<(&str, &str, Option<u32>)> = rows.iter().map(|(id, at)| (id.as_str(), at.as_str(), None)).collect();
        let index = index(&rows);
        let mut indexes = BTreeMap::new();
        indexes.insert("m-2".to_string(), index.clone());
        let newest: Vec<String> = sessions(&hosts[0], &index).into_iter().map(|(_, w)| w.name).collect();
        assert_eq!(newest.len(), SESSIONS_PER_HOST);
        // Opened most recently: the newest five, then thirty others.
        let mut opened: Vec<(String, String)> = newest.iter().map(|n| ("m-2".to_string(), n.clone())).collect();
        opened.extend(
            (0..40)
                .map(|i| format!("claude/s{i:02}"))
                .filter(|n| !newest.contains(n))
                .take(30)
                .map(|n| ("m-2".to_string(), n)),
        );
        let plan = plan(&hosts, &indexes, &HashMap::new(), &opened);
        assert_eq!(plan.iter().filter(|w| w.kind == Kind::Session).count(), OPENED_MAX + SESSIONS_PER_HOST);
    }
}
