//! Stream names per key epoch (`docs/relay-and-plans.md` §5).
//!
//! A stream's id is `HMAC(HKDF(MK_e, "rh/v1 stream-id"), source ‖ 0 ‖ id)`
//! with `e` the machine-key epoch it was created or migrated in (its
//! `name_epoch` in `streams.db`; none — a stream from before — is the first
//! epoch's, as every stream was named until now). So a rotation moves the
//! streams to names derived from a key a removed device never had: it cannot
//! compute the ids written after it, even with the hub's help.
//!
//! - **Who reads it.** Only once a device said it reads such names
//!   ([`rh_proto::stream::STREAMS_FEATURE`] in its channel hello, remembered in
//!   `streams.db`) does the agent name streams this way; until then it is
//!   exactly as before — every name the first epoch's, a rotation's
//!   re-encryption a same-name rebase.
//! - **Index and status** are always under the newest epoch's name: a reader
//!   tries the newest epoch it holds first, then older ones (a machine that
//!   is off and has not moved yet).
//! - **Sessions** keep their name until migrated; each index entry says
//!   which epoch names its session's stream (`"se"`; absent: the first).
//! - **A rotation** names the index and status anew at once, and migrates the
//!   sessions at one random moment within [`MIGRATE_WITHIN`] (at once when the
//!   rotation re-encrypts: a device was removed), in shuffled order: each is
//!   rebuilt under its new name from the local transcript (manifest and
//!   pages from seq 0), and the old stream is deleted only once the new one's
//!   checkpoint is acknowledged *and* the acknowledged index points at it.
//!   Re-encryption is that migration; the hub sees a batch of streams stop
//!   and a batch start — the random moment, the order and the padding make
//!   that an anonymity set, no more.
//! - **Nothing goes for being missing from a list.** The session list
//!   a round is made from can come back short — a transcript unreadable for
//!   a moment, an archived session, a session index still loading at boot, a
//!   listing that failed — and the hub's copy may be the only one left to
//!   read offline. So a migration ([`migration_fate`]) or a re-encryption
//!   ([`reencrypt_fate`]) deletes a stream only once it is positively
//!   superseded ([`retire`]); a session the list does not name is kept, for
//!   the window round to delete once the list says it is out of the window or
//!   its copy has not been written for the whole window. A stream the hub
//!   holds that this agent does not know goes only when it is an older name
//!   of one it does know ([`strays`]); anything else is left for the hub's
//!   retention sweep. An unreadable `streams.db` deletes nothing.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use rh_proto::record::StreamId;
use serde_json::Value;

use super::store::{Kind, Row};

/// A rotation's session migration starts at a random moment within this
/// (`RH_AGENT_MIGRATE_WITHIN_SECS`).
pub const MIGRATE_WITHIN: Duration = Duration::from_secs(6 * 60 * 60);
/// Where `streams.db` remembers that a device reads epoch names.
pub const STREAMS2_KEY: &str = "streams2";
/// Where `streams.db` keeps when the pending session migration starts (ms).
pub const MIGRATE_AT_KEY: &str = "migrate_at";

/// The epoch `row`'s name derives from (`first`: the first epoch held).
pub fn name_epoch(row: &Row, first: u32) -> u32 {
    row.name_epoch.unwrap_or(first)
}

/// Whether the hub holds anything of the stream (an append or checkpoint was acknowledged).
pub fn acked(row: &Row) -> bool {
    row.next_seq > 0 || row.manifest_upto.is_some()
}

/// Whether `row` is a name from before epoch `current` the hub never held:
/// nothing acknowledged, nothing written ahead. A newer epoch overtaking a
/// migration skips such a name (host identity renewal right after a
/// removal: two epochs in a row) instead of filling it only to move it again.
pub fn never_written(row: &Row, first: u32, current: u32) -> bool {
    name_epoch(row, first) < current && !acked(row) && row.pending.is_none()
}

/// The session streams by `source/id`: one, or an old name and its successor while one migrates.
pub fn by_session(rows: &[Row]) -> HashMap<String, Vec<&Row>> {
    let mut out: HashMap<String, Vec<&Row>> = HashMap::new();
    for row in rows.iter().filter(|r| r.kind == Kind::Session) {
        out.entry(format!("{}/{}", row.source, row.id)).or_default().push(row);
    }
    out
}

/// Of one session's streams, the one its changes go to: the newest name.
pub fn current<'a>(rows: &[&'a Row], first: u32) -> Option<&'a Row> {
    rows.iter().copied().max_by_key(|r| name_epoch(r, first))
}

/// The epoch an index entry gives for a session's stream (`se`): the newest
/// name the hub holds something under — a reader must find it there — else
/// the newest name, else `target` (where it will be created).
pub fn entry_epoch(rows: &[&Row], first: u32, target: u32) -> u32 {
    rows.iter()
        .filter(|r| acked(r))
        .map(|r| name_epoch(r, first))
        .max()
        .or_else(|| rows.iter().map(|r| name_epoch(r, first)).max())
        .unwrap_or(target)
}

/// An index entry (a session row's JSON object) with `"se": se` added.
pub fn with_se(json: &str, se: u32) -> String {
    let Ok(Value::Object(mut map)) = serde_json::from_str::<Value>(json) else {
        return json.to_string();
    };
    map.insert("se".into(), Value::from(se));
    serde_json::to_string(&Value::Object(map)).unwrap_or_else(|_| json.to_string())
}

/// The `se` an index entry carries, if any.
pub fn se_of(json: &str) -> Option<u32> {
    serde_json::from_str::<Value>(json)
        .ok()?
        .get("se")?
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
}

/// Whether `old`, named before `target`, may be deleted: a stream of the same
/// session (or the index, or the status) named at `target` or later is on the
/// hub, and — for a session — the acknowledged index entry (`entry`) already
/// points readers at it (or the session is not in the index at all).
pub fn retire(old: &Row, all: &[Row], first: u32, target: u32, entry: Option<&str>) -> bool {
    let successor = all.iter().any(|r| {
        r.stream != old.stream && r.kind == old.kind && r.source == old.source && r.id == old.id && name_epoch(r, first) >= target && acked(r)
    });
    successor && (old.kind != Kind::Session || entry.is_none_or(|e| se_of(e).is_some_and(|se| se >= target)))
}

/// What a round does with a stream marked to move to a newer name (or,
/// before epoch names, to be re-encrypted in place).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fate {
    /// Nothing left to move (it is named there already): drop the mark.
    Unmark,
    /// Positively superseded ([`retire`]): delete it on the hub.
    Retire,
    /// Rebuild it now from the listed session's transcript: under its new
    /// name, or — before epoch names — in place, past its head.
    Rebuild,
    /// Leave it on the hub as it is, still marked, for a later round.
    Keep,
}

/// What one migration round knows.
pub struct Round<'a> {
    /// Every stream in `streams.db` (read in full: an unreadable store makes no round at all).
    pub all: &'a [Row],
    /// The first epoch held.
    pub first: u32,
    /// The acknowledged index rows; `None` when they could not be read — no
    /// session is retired on them then, as "not in the index" is not known.
    pub entries: Option<&'a BTreeMap<String, String>>,
    /// The sessions the copy wants this round — made from the local session
    /// list, which may have come back short.
    pub wanted: &'a BTreeMap<String, String>,
    /// The migration's random moment has come.
    pub due: bool,
}

/// What a migration does with `old`, marked to move to a name of epoch `to`
/// (`idle`: no request of its own in flight or written ahead). It goes only
/// once positively superseded ([`retire`]); a session missing from this
/// round's list is kept — the list may be short — and left to the window
/// round, whose rule for an unlisted session is that its copy has not been
/// written for the whole window.
pub fn migration_fate(round: &Round, old: &Row, to: u32, idle: bool) -> Fate {
    if name_epoch(old, round.first) >= to {
        return Fate::Unmark;
    }
    let key = format!("{}/{}", old.source, old.id);
    let entry = match (old.kind, round.entries) {
        (Kind::Session, None) => None,
        (_, entries) => Some(entries.and_then(|e| e.get(&key)).map(String::as_str)),
    };
    if let (true, Some(entry)) = (idle, entry) {
        if retire(old, round.all, round.first, to, entry) {
            return Fate::Retire;
        }
    }
    if old.kind != Kind::Session || !round.wanted.contains_key(&key) {
        return Fate::Keep;
    }
    let moving = round
        .all
        .iter()
        .any(|r| r.kind == Kind::Session && r.source == old.source && r.id == old.id && name_epoch(r, round.first) >= to);
    if moving || !round.due {
        Fate::Keep
    } else {
        Fate::Rebuild
    }
}

/// Before epoch names, re-encryption rebuilds a stream in place from this
/// machine's truth. A session this round's list (`listed`: `source/id`) does
/// not name has nothing to rebuild it from *now*: it is kept, still marked —
/// never deleted for it — and the window round deletes it once its copy has
/// not been written for the whole window.
pub fn reencrypt_fate(row: &Row, listed: &HashSet<String>) -> Fate {
    if row.kind == Kind::Session && !listed.contains(&format!("{}/{}", row.source, row.id)) {
        Fate::Keep
    } else {
        Fate::Rebuild
    }
}

/// Of the streams the hub lists for this machine (`held`), those to delete:
/// none this agent knows, and of the rest only one that is positively an
/// older name — under a key it holds (`keys`) — of a stream it knows whose
/// newer name is on the hub and, for a session, that the acknowledged index
/// does not point readers at: a migration's delete that never reached the
/// hub. One it cannot place (a store reset, a session it no longer lists) is
/// kept for the hub's retention sweep. `known` and `entries` are `None` when
/// `streams.db` could not be read: nothing goes then. The hub's list
/// may be capped; only what it names is ever considered.
pub fn strays(
    held: &[StreamId],
    known: Option<&[Row]>,
    entries: Option<&BTreeMap<String, String>>,
    keys: &[(u32, &[u8; 32])],
    first: u32,
) -> Vec<StreamId> {
    let (Some(known), Some(entries)) = (known, entries) else {
        return Vec::new();
    };
    let mine: HashSet<StreamId> = known.iter().map(|r| r.stream).collect();
    let mut older: HashSet<StreamId> = HashSet::new();
    for row in known.iter().filter(|r| acked(r)) {
        let newer = name_epoch(row, first);
        let entry = if row.kind == Kind::Session {
            entries.get(&format!("{}/{}", row.source, row.id))
        } else {
            None
        };
        for (epoch, mk) in keys.iter().filter(|(e, _)| *e < newer) {
            let readers_elsewhere = entry.is_none_or(|e| se_of(e).is_some_and(|se| se > *epoch));
            if readers_elsewhere {
                if let Ok(stream) = rh_proto::record::stream_id(mk, &row.source, &row.id) {
                    older.insert(stream);
                }
            }
        }
    }
    held.iter().filter(|s| !mine.contains(s) && older.contains(s)).copied().collect()
}

/// When a rotation's session migration starts: at once, or at a random moment
/// within `within` — never later than one already set.
pub fn migrate_at(now_ms: u64, immediate: bool, prior: Option<u64>, within: Duration, rng: &mut impl rand::Rng) -> u64 {
    if immediate {
        return now_ms;
    }
    let at = now_ms + rng.gen_range(0..=within.as_millis() as u64);
    prior.map_or(at, |p| p.min(at))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(n: u8, kind: Kind, id: &str, name_epoch: Option<u32>, next_seq: u64) -> Row {
        Row {
            stream: StreamId([n; 32]),
            kind,
            source: if kind == Kind::Session { "claude".into() } else { "rh.index".into() },
            id: id.into(),
            epoch: name_epoch.unwrap_or(0),
            name_epoch,
            next_seq,
            link: [0; 32],
            cursor: String::new(),
            pending: None,
            manifest_upto: None,
            pages: Vec::new(),
            tail_records: 0,
            tail_bytes: 0,
            state_bytes: 0,
            checkpoint_ms: None,
            tail_since_ms: None,
            updated_ms: 1,
        }
    }

    #[test]
    fn a_session_follows_its_newest_name_and_its_entry_the_newest_one_on_the_hub() {
        let old = row(1, Kind::Session, "a", None, 5);
        let new = row(2, Kind::Session, "a", Some(3), 0);
        let rows = vec![
            old.clone(),
            new.clone(),
            row(3, Kind::Session, "b", None, 1),
            row(4, Kind::Index, "", Some(3), 1),
        ];
        let groups = by_session(&rows);
        assert_eq!(groups.len(), 2, "sessions only");
        let a = &groups["claude/a"];
        assert_eq!(current(a, 0).unwrap().stream, new.stream, "changes go to the new name");
        assert_eq!(entry_epoch(a, 0, 3), 0, "readers stay on the old name until the new one is on the hub");
        let acked_new = row(2, Kind::Session, "a", Some(3), 1);
        assert_eq!(entry_epoch(&[&old, &acked_new], 0, 3), 3);
        assert_eq!(entry_epoch(&[], 0, 3), 3, "a new session: where it will be created");
        assert_eq!(name_epoch(&old, 2), 2, "no name epoch: the first one held");
    }

    /// Host identity renewal (`docs/host-identity-rotation.md` §5.4): a
    /// removal's epoch, then the renewal's, back to back. A migration to the
    /// first that did not get to write a name is redirected to the newest:
    /// the name the hub never held is skipped; one it holds, or one with a
    /// write in flight, is moved as any other.
    #[test]
    fn a_name_the_hub_never_held_is_skipped_for_the_newest_epoch() {
        let old = row(1, Kind::Session, "a", None, 5);
        let unwritten = row(2, Kind::Session, "a", Some(1), 0);
        let written = row(3, Kind::Session, "b", Some(1), 2);
        let newest = row(4, Kind::Session, "c", Some(2), 0);
        assert!(!never_written(&old, 0, 2), "on the hub: it moves");
        assert!(never_written(&unwritten, 0, 2), "never written: skipped");
        assert!(!never_written(&written, 0, 2));
        assert!(!never_written(&newest, 0, 2), "the newest epoch's own new name stays");
        let mut checkpointed = unwritten.clone();
        checkpointed.manifest_upto = Some(0);
        assert!(!never_written(&checkpointed, 0, 2), "a checkpoint the hub took");
        let mut in_flight = unwritten.clone();
        in_flight.pending = Some(super::super::store::Pending {
            expect_seq: 0,
            records: Vec::new(),
            next_seq: 1,
            link: [0; 32],
            cursor: String::new(),
            diff: None,
            bytes: 0,
        });
        assert!(!never_written(&in_flight, 0, 2), "a write ahead may have landed");
        assert!(!never_written(&unwritten, 0, 1), "not before the epoch it was named in");
    }

    #[test]
    fn index_entries_say_their_epoch_and_an_entry_without_one_is_the_first() {
        let json = r#"{"source":"claude","id":"a","title":"t"}"#;
        let with = with_se(json, 4);
        assert_eq!(se_of(&with), Some(4));
        assert_eq!(serde_json::from_str::<Value>(&with).unwrap()["title"], "t");
        assert_eq!(se_of(&with_se(&with, 5)), Some(5), "replaced, not doubled");
        assert_eq!(se_of(json), None);
        assert_eq!(with_se("not json", 1), "not json");
    }

    #[test]
    fn an_old_name_goes_only_once_its_successor_is_on_the_hub_and_the_index_points_at_it() {
        let old = row(1, Kind::Session, "a", None, 5);
        let pending = row(2, Kind::Session, "a", Some(3), 0);
        let landed = row(2, Kind::Session, "a", Some(3), 1);
        let entry_old = r#"{"id":"a","se":0}"#;
        let entry_new = r#"{"id":"a","se":3}"#;
        assert!(
            !retire(&old, &[old.clone(), pending.clone()], 0, 3, Some(entry_new)),
            "the new stream is not acknowledged yet"
        );
        assert!(
            !retire(&old, &[old.clone(), landed.clone()], 0, 3, Some(entry_old)),
            "the index still points at the old one"
        );
        assert!(!retire(&old, &[old.clone(), landed.clone()], 0, 3, Some(r#"{"id":"a"}"#)));
        assert!(retire(&old, &[old.clone(), landed.clone()], 0, 3, Some(entry_new)));
        assert!(retire(&old, &[old.clone(), landed.clone()], 0, 3, None), "not in the index at all");
        assert!(!retire(&old, std::slice::from_ref(&old), 0, 3, None), "no successor");
        // The index (and the status) only need their successor on the hub.
        let index_old = row(5, Kind::Index, "", None, 9);
        let index_new = row(6, Kind::Index, "", Some(3), 0);
        assert!(!retire(&index_old, &[index_old.clone(), index_new.clone()], 0, 3, None));
        let mut index_new = index_new;
        index_new.manifest_upto = Some(0);
        assert!(retire(&index_old, &[index_old.clone(), index_new], 0, 3, None));
    }

    #[test]
    fn a_migration_starts_within_six_hours_at_once_when_it_re_encrypts_and_never_later_than_one_set() {
        let mut rng = rand::rngs::mock::StepRng::new(u64::MAX / 2, 0);
        let within = MIGRATE_WITHIN;
        assert_eq!(migrate_at(1000, true, Some(5000), within, &mut rng), 1000);
        let at = migrate_at(1000, false, None, within, &mut rng);
        assert!((1000..=1000 + within.as_millis() as u64).contains(&at));
        assert_eq!(migrate_at(1000, false, Some(1500), within, &mut rng), 1500, "an earlier one stands");
        let mut real = rand::thread_rng();
        for _ in 0..100 {
            let at = migrate_at(0, false, None, within, &mut real);
            assert!(at <= within.as_millis() as u64);
        }
    }

    // ---------------------------------------------------------------- nothing goes for being missing from a list

    /// The first epoch's key and the one a rotation moved to (epoch 3).
    const K0: [u8; 32] = [7; 32];
    const K3: [u8; 32] = [9; 32];

    /// A row under its real name: `id` of `kind` named under `mk`.
    fn named(mk: &[u8; 32], kind: Kind, id: &str, name_epoch: Option<u32>, next_seq: u64) -> Row {
        let mut r = row(0, kind, id, name_epoch, next_seq);
        r.stream = rh_proto::record::stream_id(mk, &r.source, id).unwrap();
        r
    }

    /// Acknowledged index rows, each pointing readers at epoch `se`.
    fn index(entries: &[(&str, u32)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(id, se)| (format!("claude/{id}"), with_se(&format!(r#"{{"id":"{id}"}}"#), *se)))
            .collect()
    }

    /// The sessions a round's list names (and so the copy wants).
    fn wanted(ids: &[&str]) -> BTreeMap<String, String> {
        ids.iter().map(|id| (format!("claude/{id}"), format!(r#"{{"id":"{id}"}}"#))).collect()
    }

    fn listed(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|id| format!("claude/{id}")).collect()
    }

    fn keys() -> [(u32, &'static [u8; 32]); 2] {
        [(0, &K0), (3, &K3)]
    }

    #[test]
    fn a_list_that_came_back_short_moves_what_it_names_and_deletes_nothing_it_left_out() {
        // Two sessions on the first epoch's names, marked to move to epoch 3;
        // this round's list names only `a` (a partial listing).
        let a = row(1, Kind::Session, "a", None, 5);
        let b = row(2, Kind::Session, "b", None, 7);
        let all = vec![a.clone(), b.clone()];
        let entries = index(&[("a", 0), ("b", 0)]);
        let wanted = wanted(&["a"]);
        let round = Round {
            all: &all,
            first: 0,
            entries: Some(&entries),
            wanted: &wanted,
            due: true,
        };
        assert_eq!(migration_fate(&round, &a, 3, true), Fate::Rebuild, "the listed session moves");
        assert_eq!(
            migration_fate(&round, &b, 3, true),
            Fate::Keep,
            "the session the short list left out keeps its copy on the hub"
        );
        // Before epoch names, a re-encryption's short list likewise.
        assert_eq!(reencrypt_fate(&a, &listed(&["a"])), Fate::Rebuild);
        assert_eq!(reencrypt_fate(&b, &listed(&["a"])), Fate::Keep);
    }

    #[test]
    fn an_errored_listing_or_an_unreadable_store_deletes_nothing() {
        let a = named(&K0, Kind::Session, "a", None, 5);
        let a3 = named(&K3, Kind::Session, "a", Some(3), 2);
        let b = named(&K0, Kind::Session, "b", None, 7);
        let all = vec![a.clone(), a3.clone(), b.clone()];
        let entries = index(&[("a", 0), ("b", 0)]);
        // The session listing failed: it comes back empty, with no error to tell.
        let nothing = BTreeMap::new();
        let round = Round {
            all: &all,
            first: 0,
            entries: Some(&entries),
            wanted: &nothing,
            due: true,
        };
        assert_eq!(migration_fate(&round, &b, 3, true), Fate::Keep, "an empty listing is no reason to delete");
        assert_eq!(reencrypt_fate(&b, &HashSet::new()), Fate::Keep);
        let status = row(9, Kind::Status, "", None, 4);
        assert_eq!(reencrypt_fate(&status, &HashSet::new()), Fate::Rebuild, "the status needs no list");
        // The acknowledged index could not be read: whether it still points
        // readers at `a`'s old name is not known, so the old name stays.
        let both = wanted(&["a", "b"]);
        let unreadable = Round {
            all: &all,
            first: 0,
            entries: None,
            wanted: &both,
            due: true,
        };
        assert_eq!(migration_fate(&unreadable, &a, 3, true), Fate::Keep);
        // `streams.db` could not be read: no stream the hub holds is taken for unknown.
        let held = [a.stream, a3.stream, b.stream];
        assert!(strays(&held, None, Some(&entries), &keys(), 0).is_empty());
        assert!(strays(&held, Some(&all), None, &keys(), 0).is_empty());
    }

    #[test]
    fn at_boot_a_session_list_still_loading_or_a_fresh_store_deletes_nothing_it_cannot_place() {
        // The first rounds after a restart, the session index still loading:
        // the list is empty, before the migration's moment and after it.
        let a = named(&K0, Kind::Session, "a", None, 5);
        let all = vec![a.clone()];
        let entries = index(&[("a", 0)]);
        let loading = BTreeMap::new();
        let loaded = wanted(&["a"]);
        for due in [false, true] {
            let round = Round {
                all: &all,
                first: 0,
                entries: Some(&entries),
                wanted: &loading,
                due,
            };
            assert_eq!(migration_fate(&round, &a, 3, true), Fate::Keep, "due: {due}");
        }
        // Once it has loaded, the session moves at the migration's moment.
        for (due, fate) in [(false, Fate::Keep), (true, Fate::Rebuild)] {
            let round = Round {
                all: &all,
                first: 0,
                entries: Some(&entries),
                wanted: &loaded,
                due,
            };
            assert_eq!(migration_fate(&round, &a, 3, true), fate, "due: {due}");
        }
        // `streams.db` was set aside at boot and started afresh: so far it has
        // re-made only the index under epoch 3. The old index is an older name
        // of it, but `a`'s copy is nothing this agent can place: it stays.
        let mut index3 = named(&K3, Kind::Index, "", Some(3), 0);
        index3.manifest_upto = Some(0);
        let index0 = named(&K0, Kind::Index, "", None, 9);
        let fresh = vec![index3.clone()];
        let held = [index0.stream, a.stream, index3.stream];
        assert_eq!(strays(&held, Some(&fresh), Some(&BTreeMap::new()), &keys(), 0), vec![index0.stream]);
    }

    #[test]
    fn an_archived_or_missing_session_keeps_its_only_copy() {
        // `c` was archived, or its transcript is gone: no list names it, the
        // index has dropped it, and the hub's copy is the only one left to
        // read offline. A device was removed, so the move is due at once.
        let c = row(4, Kind::Session, "c", None, 9);
        let all = vec![c.clone()];
        let entries = BTreeMap::new();
        let others = wanted(&["a"]);
        let round = Round {
            all: &all,
            first: 0,
            entries: Some(&entries),
            wanted: &others,
            due: true,
        };
        assert_eq!(migration_fate(&round, &c, 3, true), Fate::Keep, "no successor: not superseded");
        assert_eq!(reencrypt_fate(&c, &listed(&["a"])), Fate::Keep);
    }

    #[test]
    fn a_rotation_deletes_an_old_name_once_it_is_superseded_and_nothing_else() {
        // `a` moved: its stream under epoch 3 is on the hub and the index
        // points at it. `b` was archived before it moved.
        let a = named(&K0, Kind::Session, "a", None, 5);
        let mut a3 = named(&K3, Kind::Session, "a", Some(3), 0);
        a3.manifest_upto = Some(0);
        let b = named(&K0, Kind::Session, "b", None, 7);
        let index0 = named(&K0, Kind::Index, "", None, 9);
        let mut index3 = named(&K3, Kind::Index, "", Some(3), 0);
        index3.manifest_upto = Some(0);
        let all = vec![a.clone(), a3.clone(), b.clone(), index0.clone(), index3.clone()];
        let entries = index(&[("a", 3)]);
        let wanted = wanted(&["a"]);
        let round = Round {
            all: &all,
            first: 0,
            entries: Some(&entries),
            wanted: &wanted,
            due: true,
        };
        assert_eq!(migration_fate(&round, &a, 3, true), Fate::Retire, "superseded: it goes");
        assert_eq!(
            migration_fate(&round, &a, 3, false),
            Fate::Keep,
            "not while a request of its own is in flight"
        );
        assert_eq!(migration_fate(&round, &index0, 3, true), Fate::Retire);
        assert_eq!(migration_fate(&round, &a3, 3, true), Fate::Unmark);
        assert_eq!(migration_fate(&round, &b, 3, true), Fate::Keep, "the archived one is not superseded");
        let pointing_back = index(&[("a", 0)]);
        let early = Round {
            entries: Some(&pointing_back),
            ..round
        };
        assert_eq!(migration_fate(&early, &a, 3, true), Fate::Keep, "the index still sends readers to it");

        // The deletes of `a`'s and the index's old names never reached the
        // hub (the connection went): this agent has forgotten both, and the
        // hub lists them with `b`'s copy and one this agent cannot place.
        let moved = vec![a3.clone(), b.clone(), index3.clone()];
        let unknown = named(&K0, Kind::Session, "z", None, 3).stream;
        let held = [a.stream, a3.stream, b.stream, index0.stream, index3.stream, unknown];
        let mut gone = strays(&held, Some(&moved), Some(&entries), &keys(), 0);
        gone.sort_by_key(|s| s.0);
        let mut want = vec![a.stream, index0.stream];
        want.sort_by_key(|s| s.0);
        assert_eq!(gone, want, "only older names of streams that moved");
        // Not while the index still points readers at the old name, nor
        // before the new name is on the hub.
        assert_eq!(strays(&held, Some(&moved), Some(&pointing_back), &keys(), 0), vec![index0.stream]);
        let landing = vec![named(&K3, Kind::Session, "a", Some(3), 0), b.clone()];
        assert!(strays(&held, Some(&landing), Some(&entries), &keys(), 0).is_empty());
    }
}
