//! Idempotent sends, and what became of every message a client named.
//!
//! # The bug this exists for
//!
//! A phone on a bad network POSTs a message, the server takes it, and the
//! answer never makes it back. The client used to read that as "not sent": it
//! put the message back at the head of its queue, restored the draft, and the
//! user's retry ran it a second time — two copies of the same instruction to an
//! agent that acts on what it is told.
//!
//! So a client now names each message once, when the message is created
//! (`clientMessageId`), and sends the same name with every attempt. This store
//! remembers the name: the first attempt *claims* it — a row inserted under the
//! primary key **before** anything is enqueued, so two attempts racing each other
//! cannot both get through — and records what it answered; every later attempt
//! answers the same thing without enqueuing anything.
//!
//! # Receipts
//!
//! The same row then follows the message. It travels through a driver's queue
//! under a `qid` this store chose for it, and the drivers report by that `qid`
//! what happened next:
//!
//! * `queued` — accepted, and waiting in the server's queue;
//! * `sent` — taken into a turn: written into claude's stdin, steered, or
//!   acknowledged by codex's `turn/start`;
//! * `failed` — dropped without being delivered: withdrawn by the user, failed
//!   by the pump, or left behind by a process that died for good.
//!
//! The client reads these back two ways: the recent ones for a session ride on
//! its `state` frames, and `GET …/receipts?ids=` answers for any it asks about —
//! including the ones it has never heard back about at all, which is the only
//! way to tell "the server never saw it" (safe to send again) from "the answer
//! was lost" (it is in there).
//!
//! Every write here is synchronous and small, like every other state-database
//! write in this server; a driver reports a message at most twice in its life.

use std::sync::Arc;

use gotcha::Schematic;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::openapi_ext::Optional;
use crate::sessions::Source;

/// How long a claim is remembered: a retry later than this is a new message.
pub const KEEP_FOR_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// How far back the receipts a session's state carries reach.
pub const RECENT_MS: i64 = 2 * 60 * 60 * 1000;
/// The most receipts a session's state carries.
pub const RECENT_CAP: usize = 100;
/// The most ids one lookup may ask about.
pub const LOOKUP_CAP: usize = 100;
/// An attempt this process started that is still unfinished after this long
/// belongs to nobody.
///
/// A claim made before this process booted is stale at once: the server that
/// owned it died. One made by this process is not, because the first attempt
/// runs on a task of its own, detached from the request that started it, and
/// always settles unless the process dies. Taking such a claim over while it is
/// merely slow would deliver the message twice, which is the one thing this
/// store exists to prevent — a codex send that has to retire one app-server and
/// start another legitimately takes minutes. So the age limit is only a safety
/// valve for a send that is truly stuck.
pub const STALE_AFTER_MS: i64 = 30 * 60 * 1000;
/// How often the seven-day prune runs, at most.
const PRUNE_EVERY_MS: i64 = 10 * 60 * 1000;

/// What became of a message, as far as this server knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum ReceiptStatus {
    /// Accepted, and waiting in the server's queue.
    Queued,
    /// Taken into a turn: handed to the CLI.
    Sent,
    /// Dropped by the server without being delivered.
    Failed,
}

impl ReceiptStatus {
    /// The word the table stores.
    pub fn as_str(self) -> &'static str {
        match self {
            ReceiptStatus::Queued => "queued",
            ReceiptStatus::Sent => "sent",
            ReceiptStatus::Failed => "failed",
        }
    }

    /// The receipt a stored status reads as. `pending` only ever reaches a
    /// reader for a row whose answer was written, which accepted it.
    fn from_stored(stored: &str) -> ReceiptStatus {
        match stored {
            "sent" => ReceiptStatus::Sent,
            "failed" => ReceiptStatus::Failed,
            _ => ReceiptStatus::Queued,
        }
    }
}

/// One message's receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct MessageReceipt {
    /// The id the client sent the message under.
    pub client_message_id: String,
    /// What became of it.
    pub status: ReceiptStatus,
    /// The queue id it travels under, as `resident.queuedMessages[].id` names it.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub qid: Optional<String>,
    /// When the status last changed, ISO-8601.
    pub at: String,
}

/// The answer to `GET /api/sessions/{source}/{id}/receipts`.
///
/// An id in neither list is one whose first attempt is still being accepted;
/// ask again.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ReceiptLookup {
    /// The ids this server accepted for this session.
    pub receipts: Vec<MessageReceipt>,
    /// The ids this server never saw: nothing was enqueued under them, so the
    /// message is safe to send again — under the same id.
    pub unknown: Vec<String>,
}

/// Which route a claim was made by. A retry must come back the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `POST /api/sessions/{source}/{id}/message`.
    Message,
    /// `POST /api/sessions/new`.
    NewSession,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Message => "message",
            Kind::NewSession => "new",
        }
    }
}

/// What a claim on an id found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claim {
    /// Nobody had it: this attempt owns it, and the message goes in under `qid`.
    Owned {
        /// The queue id to deliver the message under.
        qid: String,
    },
    /// An earlier attempt finished: answer exactly what it answered.
    Replay(String),
    /// An earlier attempt is still running, or died running.
    InFlight(InFlight),
    /// The id was claimed by another route or for another session.
    Mismatch,
}

/// An attempt that has claimed an id and not yet answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InFlight {
    /// The queue id it is delivering under.
    pub qid: String,
    /// The stored status: `pending` unless a driver has already moved it.
    pub status: String,
    /// The session, where it is known yet.
    pub session_id: Option<String>,
    /// When it claimed the id, for [`Receipts::take_over`].
    pub created_ms: i64,
    /// Whether nobody can still be running it: it was claimed before this
    /// process started, or longer ago than [`STALE_AFTER_MS`].
    pub stale: bool,
}

/// The idempotency and receipt store, over the instance database.
#[derive(Debug)]
pub struct Receipts {
    db: Arc<Db>,
    /// When this store was opened. A claim older than this was made by a
    /// process that is gone, and is finished by nothing.
    booted_ms: i64,
    /// When the last prune ran.
    prune_gate: crate::db::PruneGate,
}

impl Receipts {
    /// The store over an open instance database.
    pub fn new(db: Arc<Db>) -> Self {
        Self {
            db,
            booted_ms: now_ms(),
            prune_gate: crate::db::PruneGate::new(PRUNE_EVERY_MS),
        }
    }

    /// Claim `id` for one attempt, or find out who has it.
    ///
    /// One transaction on the one connection, so the read and the insert
    /// cannot interleave with another claim: of two attempts racing on an id,
    /// exactly one is [`Claim::Owned`].
    pub fn claim(&self, id: &str, kind: Kind, source: Source, session: Option<&str>, now: i64) -> rusqlite::Result<Claim> {
        self.prune_now_and_then(now);
        let booted = self.booted_ms;
        self.db.write(|tx| {
            let row = tx
                .query_row(
                    "SELECT kind, session_source, session_id, qid, status, response_json, created_ms
                       FROM client_messages WHERE client_message_id = ?1",
                    [id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, Option<String>>(5)?,
                            row.get::<_, i64>(6)?,
                        ))
                    },
                )
                .optional()?;
            let Some((row_kind, row_source, row_session, qid, status, response, created_ms)) = row else {
                let qid = uuid::Uuid::new_v4().to_string();
                tx.execute(
                    "INSERT INTO client_messages
                         (client_message_id, kind, session_source, session_id, qid, status, response_json, created_ms, updated_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'pending', NULL, ?6, ?6)",
                    rusqlite::params![id, kind.as_str(), source.as_str(), session, qid, now],
                )?;
                return Ok(Claim::Owned { qid });
            };
            let same_session = match kind {
                Kind::Message => row_session.as_deref() == session,
                // A new session's id is the server's to choose, so a retry
                // cannot name it; the route and the CLI are the whole key.
                Kind::NewSession => true,
            };
            if row_kind != kind.as_str() || row_source != source.as_str() || !same_session {
                return Ok(Claim::Mismatch);
            }
            if let Some(response) = response {
                return Ok(Claim::Replay(response));
            }
            Ok(Claim::InFlight(InFlight {
                qid: qid.unwrap_or_default(),
                status,
                session_id: row_session,
                created_ms,
                stale: created_ms < booted || now.saturating_sub(created_ms) > STALE_AFTER_MS,
            }))
        })
    }

    /// Take over a stale claim, if nobody else has since. `true` means this
    /// attempt owns it now, under the claim's own `qid`.
    pub fn take_over(&self, id: &str, seen_created_ms: i64, now: i64) -> rusqlite::Result<bool> {
        self.db.write(|tx| {
            let changed = tx.execute(
                "UPDATE client_messages SET created_ms = ?3, updated_ms = ?3, status = 'pending'
                  WHERE client_message_id = ?1 AND response_json IS NULL AND created_ms = ?2",
                rusqlite::params![id, seen_created_ms, now],
            )?;
            Ok(changed == 1)
        })
    }

    /// Record what the attempt that owns `id` answered.
    ///
    /// A status a driver has already moved is kept: a message that went
    /// straight into a turn can be `sent` before the route that enqueued it
    /// gets here.
    pub fn complete(&self, id: &str, response: &str, session: Option<&str>, now: i64) {
        self.db.commit("a client message's answer", |tx| {
            tx.execute(
                "UPDATE client_messages
                    SET response_json = ?2,
                        session_id = COALESCE(?3, session_id),
                        status = CASE status WHEN 'pending' THEN 'queued' ELSE status END,
                        updated_ms = ?4
                  WHERE client_message_id = ?1",
                rusqlite::params![id, response, session, now],
            )?;
            Ok(())
        });
    }

    /// Forget a claim whose attempt was refused: nothing was enqueued, so a
    /// retry is a fresh attempt rather than a replay of the refusal.
    pub fn abandon(&self, id: &str) {
        self.db.commit("an abandoned client message", |tx| {
            tx.execute("DELETE FROM client_messages WHERE client_message_id = ?1 AND response_json IS NULL", [id])?;
            Ok(())
        });
    }

    /// A driver's report on the message travelling as `qid`. `true` when a
    /// receipt changed, which is when the session's feed is worth a poke.
    ///
    /// Only forward: a message that has been sent is not queued again, and
    /// one that failed is not resurrected. `sent → failed` is allowed for the
    /// one case that has it — a steered message whose process died before
    /// reading it, put back in the queue, and then never delivered.
    ///
    /// `session` fills in a `new` row's session the moment its driver knows
    /// it, so a claim stranded by a crash can still be answered.
    pub fn mark(&self, qid: &str, status: ReceiptStatus, session: &str, now: i64) -> bool {
        let sql = match status {
            ReceiptStatus::Queued => {
                "UPDATE client_messages SET status = 'queued', updated_ms = ?2, session_id = COALESCE(session_id, ?3)
                  WHERE qid = ?1 AND status = 'pending'"
            }
            ReceiptStatus::Sent => {
                "UPDATE client_messages SET status = 'sent', updated_ms = ?2, session_id = COALESCE(session_id, ?3)
                  WHERE qid = ?1 AND status IN ('pending', 'queued')"
            }
            ReceiptStatus::Failed => {
                "UPDATE client_messages SET status = 'failed', updated_ms = ?2, session_id = COALESCE(session_id, ?3)
                  WHERE qid = ?1 AND status IN ('pending', 'queued', 'sent')"
            }
        };
        match self.db.write(|tx| tx.execute(sql, rusqlite::params![qid, now, session])) {
            Ok(changed) => changed > 0,
            Err(error) => {
                tracing::warn!(path = %self.db.path().display(), qid, %error, "cannot record a message receipt");
                false
            }
        }
    }

    /// The receipts a session's state carries: the last [`RECENT_MS`] of
    /// them, at most [`RECENT_CAP`], oldest first.
    pub fn for_session(&self, source: Source, session: &str, now: i64) -> Vec<MessageReceipt> {
        let read = self.db.read(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT client_message_id, status, qid, updated_ms FROM client_messages
                  WHERE session_source = ?1 AND session_id = ?2 AND response_json IS NOT NULL AND updated_ms >= ?3
                  ORDER BY created_ms DESC, client_message_id DESC
                  LIMIT ?4",
            )?;
            let rows = stmt.query_map(
                rusqlite::params![source.as_str(), session, now - RECENT_MS, RECENT_CAP as i64],
                receipt_row,
            )?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        });
        match read {
            Ok(mut receipts) => {
                receipts.reverse();
                receipts
            }
            Err(error) => {
                tracing::warn!(path = %self.db.path().display(), %error, "cannot read message receipts");
                Vec::new()
            }
        }
    }

    /// What became of each of `ids`, for one session.
    ///
    /// An id still being accepted, or claimed for another session, is in
    /// neither list: neither "it is in there" nor "send it again" is true of it.
    /// A claim stranded by a server that died mid-send is listed as unknown
    /// when nothing shows it got as far as a driver: sending it again is safe,
    /// because the retry finds the claim and settles it before enqueuing — see
    /// [`crate::drive::idempotent`].
    pub fn lookup(&self, source: Source, session: &str, ids: &[String], now: i64) -> rusqlite::Result<ReceiptLookup> {
        let booted = self.booted_ms;
        self.db.read(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT client_message_id, status, qid, updated_ms, session_source, session_id, response_json IS NOT NULL, created_ms
                   FROM client_messages WHERE client_message_id = ?1",
            )?;
            let mut lookup = ReceiptLookup::default();
            for id in ids {
                let found = stmt
                    .query_row([id], |row| {
                        Ok((
                            receipt_row(row)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, Option<String>>(5)?,
                            row.get::<_, bool>(6)?,
                            row.get::<_, i64>(7)?,
                        ))
                    })
                    .optional()?;
                match found {
                    None => lookup.unknown.push(id.clone()),
                    Some((receipt, stored, row_source, row_session, answered, created_ms)) => {
                        if row_source != source.as_str() || row_session.as_deref() != Some(session) {
                            continue;
                        }
                        let stale = created_ms < booted || now.saturating_sub(created_ms) > STALE_AFTER_MS;
                        // A stranded claim a driver did touch got as far as a
                        // queue, and its receipt says what became of it.
                        if answered || (stale && stored != "pending") {
                            lookup.receipts.push(receipt);
                        } else if stale {
                            lookup.unknown.push(id.clone());
                        }
                    }
                }
            }
            Ok(lookup)
        })
    }

    /// Forget every claim older than [`KEEP_FOR_MS`].
    pub fn prune(&self, now: i64) {
        self.db.commit("the client message prune", |tx| {
            tx.execute("DELETE FROM client_messages WHERE created_ms < ?1", [now - KEEP_FOR_MS])?;
            Ok(())
        });
    }

    /// [`Receipts::prune`], at most once every [`PRUNE_EVERY_MS`].
    fn prune_now_and_then(&self, now: i64) {
        if self.prune_gate.due(now) {
            self.prune(now);
        }
    }
}

/// The first four columns of a receipt query, as a receipt.
fn receipt_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessageReceipt> {
    Ok(MessageReceipt {
        client_message_id: row.get(0)?,
        status: ReceiptStatus::from_stored(&row.get::<_, String>(1)?),
        qid: Optional(row.get::<_, Option<String>>(2)?),
        at: crate::accounts::rest::iso_ms(row.get(3)?),
    })
}

/// Epoch milliseconds, the clock every column here is kept in.
pub use crate::time::now_ms;

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "11111111-1111-4111-8111-111111111111";

    fn store() -> (tempfile::TempDir, Receipts) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::at(dir.path().join("state.db"));
        (dir, Receipts::new(db))
    }

    fn owned(claim: Claim) -> String {
        match claim {
            Claim::Owned { qid } => qid,
            other => panic!("expected to own the claim, got {other:?}"),
        }
    }

    #[test]
    fn the_first_claim_owns_an_id_and_every_later_one_waits_then_replays() {
        let (_dir, receipts) = store();
        let now = now_ms();
        let qid = owned(receipts.claim("a", Kind::Message, Source::Claude, Some(SESSION), now).expect("claim"));

        match receipts.claim("a", Kind::Message, Source::Claude, Some(SESSION), now).expect("claim") {
            Claim::InFlight(flight) => {
                assert_eq!(flight.qid, qid, "the second attempt is told which message the first is delivering");
                assert!(!flight.stale, "a claim made by this process a moment ago is live");
            }
            other => panic!("expected the first attempt to still be running, got {other:?}"),
        }

        receipts.complete("a", r#"{"accepted":true,"queued":true}"#, None, now);
        assert_eq!(
            receipts.claim("a", Kind::Message, Source::Claude, Some(SESSION), now).expect("claim"),
            Claim::Replay(r#"{"accepted":true,"queued":true}"#.to_string())
        );
    }

    #[test]
    fn an_id_claimed_for_one_session_is_not_another_sessions() {
        let (_dir, receipts) = store();
        let now = now_ms();
        owned(receipts.claim("a", Kind::Message, Source::Claude, Some(SESSION), now).expect("claim"));
        assert_eq!(
            receipts.claim("a", Kind::Message, Source::Claude, Some("other"), now).expect("claim"),
            Claim::Mismatch
        );
        assert_eq!(
            receipts.claim("a", Kind::Message, Source::Codex, Some(SESSION), now).expect("claim"),
            Claim::Mismatch
        );
        assert_eq!(
            receipts.claim("a", Kind::NewSession, Source::Claude, None, now).expect("claim"),
            Claim::Mismatch
        );
    }

    /// A refusal records nothing: the retry is a fresh attempt.
    #[test]
    fn an_abandoned_claim_is_forgotten() {
        let (_dir, receipts) = store();
        let now = now_ms();
        let first = owned(receipts.claim("a", Kind::Message, Source::Claude, Some(SESSION), now).expect("claim"));
        receipts.abandon("a");
        let second = owned(receipts.claim("a", Kind::Message, Source::Claude, Some(SESSION), now).expect("claim"));
        assert_ne!(first, second, "a fresh attempt gets a fresh queue id");
        let lookup = receipts.lookup(Source::Claude, SESSION, &["a".into()], now).expect("lookup");
        assert!(lookup.receipts.is_empty() && lookup.unknown.is_empty(), "in flight again: {lookup:?}");
    }

    #[test]
    fn receipts_move_forward_by_queue_id_and_never_back() {
        let (_dir, receipts) = store();
        let now = now_ms();
        let qid = owned(receipts.claim("a", Kind::Message, Source::Claude, Some(SESSION), now).expect("claim"));
        receipts.complete("a", "{}", None, now);
        let status = |receipts: &Receipts| receipts.for_session(Source::Claude, SESSION, now)[0].status;
        assert_eq!(status(&receipts), ReceiptStatus::Queued);

        assert!(receipts.mark(&qid, ReceiptStatus::Sent, SESSION, now + 1));
        assert_eq!(status(&receipts), ReceiptStatus::Sent);
        assert!(
            !receipts.mark(&qid, ReceiptStatus::Queued, SESSION, now + 2),
            "sent is never queued again"
        );
        assert!(
            !receipts.mark(&qid, ReceiptStatus::Sent, SESSION, now + 2),
            "and saying it twice changes nothing"
        );
        assert!(receipts.mark(&qid, ReceiptStatus::Failed, SESSION, now + 3));
        assert!(!receipts.mark(&qid, ReceiptStatus::Sent, SESSION, now + 4), "failed is final");
        assert_eq!(status(&receipts), ReceiptStatus::Failed);
        assert!(
            !receipts.mark("no-such-qid", ReceiptStatus::Sent, SESSION, now),
            "an unnamed message is nobody's receipt"
        );
    }

    /// The race the route cannot avoid: the driver takes the message into a
    /// turn before the route that enqueued it has written its answer down.
    #[test]
    fn a_driver_that_gets_there_first_keeps_its_word() {
        let (_dir, receipts) = store();
        let now = now_ms();
        let qid = owned(receipts.claim("a", Kind::Message, Source::Claude, Some(SESSION), now).expect("claim"));
        assert!(receipts.mark(&qid, ReceiptStatus::Sent, SESSION, now));
        assert!(
            receipts.for_session(Source::Claude, SESSION, now).is_empty(),
            "an attempt still answering is not reported yet"
        );
        receipts.complete("a", "{}", None, now);
        assert_eq!(receipts.for_session(Source::Claude, SESSION, now)[0].status, ReceiptStatus::Sent);
    }

    #[test]
    fn a_new_sessions_claim_learns_its_session_from_the_answer() {
        let (_dir, receipts) = store();
        let now = now_ms();
        owned(receipts.claim("n", Kind::NewSession, Source::Codex, None, now).expect("claim"));
        receipts.complete("n", r#"{"id":"thread-1"}"#, Some("thread-1"), now);
        assert_eq!(
            receipts.claim("n", Kind::NewSession, Source::Codex, None, now).expect("claim"),
            Claim::Replay(r#"{"id":"thread-1"}"#.to_string())
        );
        let receipts_for = receipts.for_session(Source::Codex, "thread-1", now);
        assert_eq!(receipts_for.len(), 1);
        assert_eq!(receipts_for[0].client_message_id, "n");
    }

    #[test]
    fn a_lookup_tells_never_seen_from_accepted_and_from_still_answering() {
        let (_dir, receipts) = store();
        let now = now_ms();
        let qid = owned(receipts.claim("done", Kind::Message, Source::Claude, Some(SESSION), now).expect("claim"));
        receipts.complete("done", "{}", None, now);
        owned(receipts.claim("busy", Kind::Message, Source::Claude, Some(SESSION), now).expect("claim"));
        owned(
            receipts
                .claim("elsewhere", Kind::Message, Source::Claude, Some("other"), now)
                .expect("claim"),
        );
        receipts.complete("elsewhere", "{}", None, now);

        let ids: Vec<String> = ["done", "busy", "never", "elsewhere"].iter().map(|id| id.to_string()).collect();
        let lookup = receipts.lookup(Source::Claude, SESSION, &ids, now).expect("lookup");
        assert_eq!(lookup.unknown, vec!["never".to_string()]);
        assert_eq!(lookup.receipts.len(), 1);
        assert_eq!(lookup.receipts[0].client_message_id, "done");
        assert_eq!(lookup.receipts[0].qid.0.as_deref(), Some(qid.as_str()));
        assert_eq!(lookup.receipts[0].status, ReceiptStatus::Queued);
    }

    #[test]
    fn a_sessions_state_carries_only_recent_receipts_and_only_so_many() {
        let (_dir, receipts) = store();
        let now = now_ms();
        for n in 0..(RECENT_CAP + 5) {
            let id = format!("m{n:03}");
            receipts
                .claim(&id, Kind::Message, Source::Claude, Some(SESSION), now + n as i64)
                .expect("claim");
            receipts.complete(&id, "{}", None, now + n as i64);
        }
        let old = "old";
        receipts
            .claim(old, Kind::Message, Source::Claude, Some(SESSION), now - RECENT_MS - 1)
            .expect("claim");
        receipts.complete(old, "{}", None, now - RECENT_MS - 1);

        let recent = receipts.for_session(Source::Claude, SESSION, now + 200);
        assert_eq!(recent.len(), RECENT_CAP);
        assert_eq!(
            recent.first().map(|r| r.client_message_id.as_str()),
            Some("m005"),
            "the newest hundred, oldest first"
        );
        assert_eq!(recent.last().map(|r| r.client_message_id.as_str()), Some("m104"));
        assert!(recent.iter().all(|r| r.client_message_id != old));
    }

    #[test]
    fn claims_older_than_a_week_are_pruned() {
        let (_dir, receipts) = store();
        let now = now_ms();
        let then = now - KEEP_FOR_MS - 1;
        receipts
            .claim("ancient", Kind::Message, Source::Claude, Some(SESSION), then)
            .expect("claim");
        receipts.complete("ancient", "{}", None, then);
        receipts.claim("fresh", Kind::Message, Source::Claude, Some(SESSION), now).expect("claim");
        receipts.complete("fresh", "{}", None, now);

        receipts.prune(now);
        let lookup = receipts
            .lookup(Source::Claude, SESSION, &["ancient".into(), "fresh".into()], now)
            .expect("lookup");
        assert_eq!(lookup.unknown, vec!["ancient".to_string()], "a week-old claim is forgotten");
        assert_eq!(lookup.receipts.len(), 1);
    }

    /// A claim made by a server that is gone is finished by nothing, and must
    /// not answer `in_progress` for ever.
    #[test]
    fn a_claim_from_before_this_process_is_stale_and_can_be_taken_over_once() {
        let (_dir, receipts) = store();
        let then = receipts.booted_ms - 1;
        let qid = owned(receipts.claim("a", Kind::Message, Source::Claude, Some(SESSION), then).expect("claim"));
        let now = now_ms();
        let flight = match receipts.claim("a", Kind::Message, Source::Claude, Some(SESSION), now).expect("claim") {
            Claim::InFlight(flight) => flight,
            other => panic!("expected a stale claim, got {other:?}"),
        };
        assert!(flight.stale);
        assert_eq!(
            receipts.lookup(Source::Claude, SESSION, &["a".into()], now).expect("lookup").unknown,
            vec!["a".to_string()],
            "a stranded claim no driver touched is safe to send again"
        );
        assert!(receipts.take_over("a", flight.created_ms, now).expect("take over"));
        assert!(
            !receipts.take_over("a", flight.created_ms, now).expect("take over"),
            "a second taker loses: the claim moved under it"
        );
        match receipts.claim("a", Kind::Message, Source::Claude, Some(SESSION), now).expect("claim") {
            Claim::InFlight(flight) => {
                assert!(!flight.stale, "taken over, it is live again");
                assert_eq!(flight.qid, qid, "and still delivers under the same queue id");
            }
            other => panic!("expected the taken-over claim, got {other:?}"),
        }
    }
}
