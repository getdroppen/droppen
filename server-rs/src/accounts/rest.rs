//! The limit log, and how long each account is resting.
//!
//! Phase 1 of automatic account rotation: **record.** Every limit a CLI reports
//! is written to `account_limit_events` with the raw words it used, and the
//! account it happened under is marked as resting in `account_rest` until the
//! limit lifts. Nothing here acts on it; phase 2, [`super::rotation`], reads
//! the rests to choose which account a limited session moves to, and a person
//! can see who was limited when, and until when.
//!
//! # Where it lives
//!
//! In this *instance's* `state.db`, not the machine-wide one the accounts are
//! in. That is a decision rather than an accident: two instances on one machine
//! share the accounts but not their view of them, and a rest one instance
//! recorded is not something the other one has seen happen.
//!
//! # Who the rows are about
//!
//! An account's uuid, or [`LOCAL_CLAUDE`] / [`LOCAL_CODEX`] for a session on the
//! machine's own login — which has no row in `accounts` to hang a rest on, but
//! runs out of quota all the same.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use gotcha::Schematic;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use super::limits::{self, LimitClass, Rest, Verdict};
use super::store::AccountProvider;
use crate::db::Db;
use crate::openapi_ext::Nullable;
use crate::sessions::iso8601;

/// The rows for a claude session on the machine's own login.
pub const LOCAL_CLAUDE: &str = "local:claude";
/// The rows for a codex session on the machine's own login.
pub const LOCAL_CODEX: &str = "local:codex";
/// How many events the log keeps; the oldest go first.
pub const EVENTS_CAP: i64 = 500;
/// How much of what the CLI said an event keeps, in characters.
pub const RAW_CAP: usize = 500;
/// How much of it a rest's `reason` keeps.
pub const REASON_CAP: usize = 200;
/// How far back a run of `other` events still counts as one run.
///
/// An odd failure yesterday and another today are not a pattern; the backoff
/// only climbs for failures that follow one another.
const OTHER_STREAK_WINDOW_MS: i64 = 60 * 60 * 1000;
/// How close together two sightings of one limit in one session are one hit.
///
/// claude reports a refused turn twice — a `rate_limit_event` saying
/// `rejected`, then the `result` saying "You've hit your session limit" — a few
/// milliseconds apart. Both are the same hit, and the log keeps one of them.
const DUPLICATE_WINDOW_MS: i64 = 10_000;

/// The name these tables give the login a session runs under.
pub fn account_key(account: Option<&str>, provider: AccountProvider) -> String {
    match account {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => match provider {
            AccountProvider::Claude => LOCAL_CLAUDE.to_string(),
            AccountProvider::Codex => LOCAL_CODEX.to_string(),
        },
    }
}

/// Whom a claude session's limit is about, as these tables name it — or
/// nobody.
///
/// [`account_key`] for a session with an account, or for one with none in the
/// user's own `~/.claude`: that one runs on the local login. A session with no
/// account in this instance's managed root does not: it runs on whatever login
/// the root itself holds, which is nobody's choice here, and filing its refusal
/// under [`LOCAL_CLAUDE`] would mark the user's own login signed out for a
/// failure that was never its. Such a limit — and one whose root cannot be told
/// — is not filed at all, which is what [`crate::quota::readings::session_login`]
/// already does with its readings.
pub fn claude_limit_account(account: Option<&str>, root: Option<crate::claude_roots::ClaudeRoot>) -> Option<String> {
    match (account.filter(|id| !id.is_empty()), root) {
        (Some(id), _) => Some(id.to_string()),
        (None, Some(crate::claude_roots::ClaudeRoot::Local)) => Some(LOCAL_CLAUDE.to_string()),
        (None, _) => None,
    }
}

/// Whether `id` names the machine's own login rather than a stored account.
pub fn is_local(id: &str) -> bool {
    id == LOCAL_CLAUDE || id == LOCAL_CODEX
}

/// One account resting, as `Account.rest` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AccountRest {
    /// `all`, or the model family a weekly window belongs to.
    pub scope: String,
    /// Which kind of limit put it here.
    pub class: LimitClass,
    /// When the rest ends, ISO-8601; `null` until it is cleared or the
    /// credential changes.
    pub until_at: Nullable<String>,
    /// The line of the CLI's output that said why, redacted.
    pub reason: String,
    /// When the limit was seen, ISO-8601.
    pub observed_at: String,
    /// The session it was seen in.
    pub session_id: Nullable<String>,
    /// Which hook saw it: `claude:result`, `claude:rate_limit_event`,
    /// `claude:exit`, `codex:turn` or `codex:rate_limits`. The part before the
    /// colon is the CLI.
    pub source: String,
}

/// One entry in the limit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AccountLimitEvent {
    /// Increasing; newer events have larger ids.
    pub id: i64,
    /// When it was seen, ISO-8601.
    pub observed_at: String,
    /// The account it happened under; `null` for the machine's own login.
    pub account_id: Nullable<String>,
    /// The session it happened in.
    pub session_id: Nullable<String>,
    /// Which hook saw it: `claude:result`, `claude:rate_limit_event`,
    /// `claude:exit`, `codex:turn` or `codex:rate_limits`. The part before the
    /// colon is the CLI.
    pub source: String,
    /// The model the session was running, when known.
    pub model: Nullable<String>,
    /// Which kind of limit.
    pub class: LimitClass,
    /// `all`, or the model family a weekly window belongs to.
    pub scope: String,
    /// When the limit lifts, ISO-8601, if anything said or a default applied.
    pub until_at: Nullable<String>,
    /// What the CLI said, redacted and cut to 500 characters.
    pub raw: String,
}

/// `GET /api/accounts/limit-events` and the per-account variant.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AccountLimitEventList {
    /// Newest first.
    pub events: Vec<AccountLimitEvent>,
}

/// A limit one hook saw, ready to be written down.
#[derive(Debug, Clone)]
pub struct Observation {
    /// An account uuid, [`LOCAL_CLAUDE`] or [`LOCAL_CODEX`].
    pub account_id: String,
    /// The session it happened in, when there is one.
    pub session_id: Option<String>,
    /// Which hook; see [`AccountRest::source`].
    pub source: &'static str,
    /// The model the session was running, when known.
    pub model: Option<String>,
    /// What the classifier made of it.
    pub verdict: Verdict,
    /// What the CLI said. **Must already be redacted**; clipped here.
    pub raw: String,
    /// The line that says why. **Must already be redacted**; clipped here.
    pub reason: String,
    /// Whether this observation may rest the account at all.
    ///
    /// `false` for the one source that can report a limit without saying when
    /// it lifts and without that being the class's usual shape: a codex rate
    /// limit update with a full window and no reset.
    pub may_rest: bool,
}

/// What [`LimitLog::record`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recorded {
    /// The event's id — the earlier one's, for a duplicate.
    pub id: i64,
    /// Whether this was a second sighting of a hit already in the log, and so
    /// was not logged again.
    pub duplicate: bool,
    /// Whether the account's rest was written (or rewritten) by it.
    pub rested: bool,
    /// The rest's end this event implied: `None` with `rested` is indefinite.
    pub until_ms: Option<i64>,
}

/// The two tables, over one instance database.
pub struct LimitLog {
    db: Arc<Db>,
}

/// Milliseconds since the epoch, now.
pub use crate::time::now_ms;

/// Epoch milliseconds as the ISO-8601 the rest of the API speaks.
pub fn iso_ms(ms: i64) -> String {
    iso8601(UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64))
}

/// Whether a new rest replaces the one already on file for the same scope.
///
/// A new reading usually wins — it is the newer fact. The exception is a
/// *default* (nothing said when the limit lifts) that would cut short a rest
/// already known to run longer: a codex usage limit fails its turn a moment
/// after the rate-limit update that carried the real reset time, and the
/// thirty-minute default must not overwrite four days with thirty minutes.
///
/// An explicit time always wins, and so does a refused credential — except for
/// a second sighting of the same hit, which only ever lengthens (see
/// [`keeps_longer`]). A limit of
/// any other kind landing on an indefinite `auth` rest wins too — getting as
/// far as a quota means the credential is working again.
fn supersedes(existing: Option<Option<i64>>, until: Option<i64>, explicit: bool, class: LimitClass, now: i64) -> bool {
    let Some(old_until) = existing else { return true };
    let active = old_until.is_none_or(|at| at > now);
    if !active || explicit || class == LimitClass::Auth {
        return true;
    }
    match (old_until, until) {
        (None, _) => true,
        (Some(old), Some(new)) => new >= old,
        (Some(_), None) => true,
    }
}

/// Whether a second sighting of a hit rewrites the rest the first one wrote.
///
/// Only to make it longer: two readings of one reset — an epoch and a wall
/// clock rounded to the minute — are the same fact, and the later one must not
/// shave a minute off. An indefinite rest stays indefinite.
fn keeps_longer(existing: Option<Option<i64>>, until: Option<i64>, now: i64) -> bool {
    match (existing, until) {
        (None, _) => true,
        (Some(Some(old)), _) if old <= now => true,
        (Some(None), new) => new.is_none(),
        (Some(Some(_)), None) => true,
        (Some(Some(old)), Some(new)) => new > old,
    }
}

impl LimitLog {
    /// The log over an open instance database.
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }

    /// Write one observation down: an event, and a rest when it earns one.
    ///
    /// The event is skipped when the same session logged the same class and
    /// scope within [`DUPLICATE_WINDOW_MS`] — one hit, reported twice — but the
    /// rest is still written if this sighting makes it longer.
    ///
    /// One transaction, so the log and the rest cannot disagree about what
    /// happened; the log is trimmed to [`EVENTS_CAP`] in the same breath.
    pub fn record(&self, observation: &Observation, now: i64) -> rusqlite::Result<Recorded> {
        let verdict = &observation.verdict;
        let raw = limits::clip(&observation.raw, RAW_CAP);
        let reason = limits::clip(&observation.reason, REASON_CAP);
        self.db.write(|tx| {
            let duplicate_of: Option<i64> = match &observation.session_id {
                Some(session) => tx
                    .query_row(
                        "SELECT id FROM account_limit_events
                          WHERE session_id = ?1 AND account_id = ?2 AND class = ?3 AND scope = ?4 AND observed_ms >= ?5
                          ORDER BY id DESC LIMIT 1",
                        rusqlite::params![
                            session,
                            observation.account_id,
                            verdict.class.as_str(),
                            verdict.scope,
                            now - DUPLICATE_WINDOW_MS,
                        ],
                        |row| row.get(0),
                    )
                    .optional()?,
                None => None,
            };
            // How many `other` events for this account came just before this
            // one; only `other` asks, because only its rest depends on it. A
            // duplicate is the same hit as the event it duplicates, not a
            // repeat of it.
            let streak = if verdict.class == LimitClass::Other {
                let mut stmt = tx.prepare("SELECT class, observed_ms FROM account_limit_events WHERE account_id = ?1 ORDER BY id DESC LIMIT 32")?;
                let rows = stmt.query_map((&observation.account_id,), |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
                let mut streak = 0u32;
                for row in rows {
                    let (class, observed) = row?;
                    if class != LimitClass::Other.as_str() || observed < now - OTHER_STREAK_WINDOW_MS {
                        break;
                    }
                    streak += 1;
                }
                if duplicate_of.is_some() {
                    streak.saturating_sub(1)
                } else {
                    streak
                }
            } else {
                0
            };

            // `Some(until)` is a rest to write, and `until` is `None` for an
            // indefinite one; `None` is no rest at all.
            let (rest, explicit): (Option<Option<i64>>, bool) = if !observation.may_rest {
                (None, false)
            } else if let Some(until) = verdict.until_ms {
                (Some(Some(until)), true)
            } else {
                match limits::default_rest(verdict.class, streak) {
                    Rest::Skip => (None, false),
                    Rest::For(duration) => (Some(Some(now + duration.as_millis() as i64)), false),
                    Rest::Indefinite => (Some(None), false),
                }
            };
            let until_ms = match rest {
                Some(until) => until,
                None => verdict.until_ms,
            };

            let id = match duplicate_of {
                Some(id) => id,
                None => {
                    tx.execute(
                        "INSERT INTO account_limit_events
                             (observed_ms, account_id, session_id, source, model, class, scope, until_ms, raw)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        rusqlite::params![
                            now,
                            observation.account_id,
                            observation.session_id,
                            observation.source,
                            observation.model,
                            verdict.class.as_str(),
                            verdict.scope,
                            until_ms,
                            raw,
                        ],
                    )?;
                    let id = tx.last_insert_rowid();
                    tx.execute(
                        "DELETE FROM account_limit_events WHERE id <= (
                             SELECT id FROM account_limit_events ORDER BY id DESC LIMIT 1 OFFSET ?1
                         )",
                        (EVENTS_CAP,),
                    )?;
                    id
                }
            };

            let mut rested = false;
            if let Some(until) = rest {
                let existing: Option<Option<i64>> = tx
                    .query_row(
                        "SELECT until_ms FROM account_rest WHERE account_id = ?1 AND scope = ?2",
                        (&observation.account_id, &verdict.scope),
                        |row| row.get::<_, Option<i64>>(0),
                    )
                    .optional()?;
                let replace = if duplicate_of.is_some() {
                    keeps_longer(existing, until, now)
                } else {
                    supersedes(existing, until, explicit, verdict.class, now)
                };
                if replace {
                    tx.execute(
                        "INSERT INTO account_rest
                             (account_id, scope, class, until_ms, reason, session_id, source, observed_ms)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                         ON CONFLICT (account_id, scope) DO UPDATE SET
                             class = excluded.class,
                             until_ms = excluded.until_ms,
                             reason = excluded.reason,
                             session_id = excluded.session_id,
                             source = excluded.source,
                             observed_ms = excluded.observed_ms",
                        rusqlite::params![
                            observation.account_id,
                            verdict.scope,
                            verdict.class.as_str(),
                            until,
                            reason,
                            observation.session_id,
                            observation.source,
                            now,
                        ],
                    )?;
                    rested = true;
                }
            }
            Ok(Recorded {
                id,
                duplicate: duplicate_of.is_some(),
                rested,
                until_ms,
            })
        })
    }

    /// Every active rest, by account. A rest is active while it has no end or
    /// its end is still ahead.
    pub fn active_rests(&self, now: i64) -> BTreeMap<String, Vec<AccountRest>> {
        let mut by_account: BTreeMap<String, Vec<AccountRest>> = BTreeMap::new();
        let rows = self.db.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT account_id, scope, class, until_ms, reason, session_id, source, observed_ms
                   FROM account_rest
                  WHERE until_ms IS NULL OR until_ms > ?1
                  ORDER BY account_id, scope",
            )?;
            let rows = stmt.query_map((now,), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    AccountRest {
                        scope: row.get(1)?,
                        class: LimitClass::parse(&row.get::<_, String>(2)?),
                        until_at: Nullable(row.get::<_, Option<i64>>(3)?.map(iso_ms)),
                        reason: row.get(4)?,
                        session_id: Nullable(row.get(5)?),
                        source: row.get(6)?,
                        observed_at: iso_ms(row.get(7)?),
                    },
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        });
        match rows {
            Ok(rows) => {
                for (account, rest) in rows {
                    by_account.entry(account).or_default().push(rest);
                }
            }
            Err(error) => {
                tracing::warn!(database = %self.db.path().display(), %error, "cannot read the account rests");
            }
        }
        by_account
    }

    /// When one account's rest under one scope ends: `Some(None)` for a rest
    /// with no end, `None` when it is not resting under that scope at `now`.
    pub fn rest_until(&self, account_id: &str, scope: &str, now: i64) -> Option<Option<i64>> {
        self.db
            .read(|conn| {
                conn.query_row(
                    "SELECT until_ms FROM account_rest
                      WHERE account_id = ?1 AND scope = ?2 AND (until_ms IS NULL OR until_ms > ?3)",
                    (account_id, scope, now),
                    |row| row.get::<_, Option<i64>>(0),
                )
                .optional()
            })
            .unwrap_or(None)
    }

    /// When the newest active rest of one class on one account was seen, epoch
    /// milliseconds: the instant a login was last caught failing, for a reader
    /// that has to weigh it against later evidence that it works again.
    pub fn active_observed_ms(&self, account_id: &str, class: LimitClass, now: i64) -> Option<i64> {
        self.db
            .read(|conn| {
                conn.query_row(
                    "SELECT MAX(observed_ms) FROM account_rest
                      WHERE account_id = ?1 AND class = ?2 AND (until_ms IS NULL OR until_ms > ?3)",
                    (account_id, class.as_str(), now),
                    |row| row.get::<_, Option<i64>>(0),
                )
            })
            .unwrap_or(None)
    }

    /// One account's active rests.
    pub fn active_for(&self, account_id: &str, now: i64) -> Vec<AccountRest> {
        self.active_rests(now).remove(account_id).unwrap_or_default()
    }

    /// End an account's rests: one scope, or all of them. How many ended.
    pub fn clear(&self, account_id: &str, scope: Option<&str>) -> rusqlite::Result<usize> {
        self.db.write(|tx| match scope {
            Some(scope) => tx.execute("DELETE FROM account_rest WHERE account_id = ?1 AND scope = ?2", (account_id, scope)),
            None => tx.execute("DELETE FROM account_rest WHERE account_id = ?1", (account_id,)),
        })
    }

    /// End an account's rests of one class — `auth`, when its credential changes.
    pub fn clear_class(&self, account_id: &str, class: LimitClass) -> rusqlite::Result<usize> {
        self.db.write(|tx| {
            tx.execute(
                "DELETE FROM account_rest WHERE account_id = ?1 AND class = ?2",
                (account_id, class.as_str()),
            )
        })
    }

    /// The newest `limit` events, for one account or for all of them.
    pub fn events(&self, account_id: Option<&str>, limit: usize) -> Vec<AccountLimitEvent> {
        let limit = limit.clamp(1, EVENTS_CAP as usize) as i64;
        let read = self.db.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, observed_ms, account_id, session_id, source, model, class, scope, until_ms, raw
                   FROM account_limit_events
                  WHERE ?1 IS NULL OR account_id = ?1
                  ORDER BY id DESC
                  LIMIT ?2",
            )?;
            let rows = stmt.query_map((account_id, limit), |row| {
                let account: Option<String> = row.get(2)?;
                Ok(AccountLimitEvent {
                    id: row.get(0)?,
                    observed_at: iso_ms(row.get(1)?),
                    // The machine's own login is not an account a client can
                    // look up, so it goes out as `null`; `source` still says
                    // which CLI it was.
                    account_id: Nullable(account.filter(|id| !is_local(id))),
                    session_id: Nullable(row.get(3)?),
                    source: row.get(4)?,
                    model: Nullable(row.get(5)?),
                    class: LimitClass::parse(&row.get::<_, String>(6)?),
                    scope: row.get(7)?,
                    until_at: Nullable(row.get::<_, Option<i64>>(8)?.map(iso_ms)),
                    raw: row.get(9)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        });
        read.unwrap_or_else(|error| {
            tracing::warn!(database = %self.db.path().display(), %error, "cannot read the limit log");
            Vec::new()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCOUNT: &str = "11111111-2222-3333-4444-555555555555";
    const MINUTE: i64 = 60_000;

    fn log() -> (tempfile::TempDir, LimitLog) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Arc::new(Db::open(dir.path().join("state.db")));
        (dir, LimitLog::new(db))
    }

    fn seen(account: &str, class: LimitClass, scope: &str, until_ms: Option<i64>) -> Observation {
        Observation {
            account_id: account.to_string(),
            session_id: Some("session-1".into()),
            source: "claude:result",
            model: Some("claude-fable-5-1".into()),
            verdict: Verdict {
                class,
                scope: scope.to_string(),
                until_ms,
            },
            raw: "You've hit your session limit · resets 2:30am (Asia/Tokyo)".into(),
            reason: "You've hit your session limit".into(),
            may_rest: true,
        }
    }

    #[test]
    fn a_limit_rests_the_account_until_it_lifts_and_then_not() {
        let (_dir, log) = log();
        let now = 1_000 * MINUTE;
        let recorded = log.record(&seen(ACCOUNT, LimitClass::Session, "all", None), now).expect("record");
        assert!(recorded.rested);
        assert_eq!(recorded.until_ms, Some(now + 30 * MINUTE), "the session default");

        let rests = log.active_for(ACCOUNT, now + MINUTE);
        assert_eq!(rests.len(), 1);
        assert_eq!(rests[0].class, LimitClass::Session);
        assert_eq!(rests[0].until_at.0.as_deref(), Some(iso_ms(now + 30 * MINUTE).as_str()));
        assert_eq!(rests[0].session_id.0.as_deref(), Some("session-1"));
        assert_eq!(rests[0].source, "claude:result");

        assert!(log.active_for(ACCOUNT, now + 30 * MINUTE).is_empty(), "expired at its end, not after");
        assert!(log.active_rests(now + 31 * MINUTE).is_empty());

        // The event outlives the rest.
        let events = log.events(Some(ACCOUNT), 50);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].account_id.0.as_deref(), Some(ACCOUNT));
        assert_eq!(events[0].class, LimitClass::Session);
    }

    #[test]
    fn an_explicit_reset_wins_and_a_default_never_shortens_a_known_one() {
        let (_dir, log) = log();
        let now = 1_000 * MINUTE;
        let four_days = now + 4 * 24 * 60 * MINUTE;
        log.record(&seen(ACCOUNT, LimitClass::Rate, "all", Some(four_days)), now).unwrap();
        let later = log.record(&seen(ACCOUNT, LimitClass::Session, "all", None), now + MINUTE).unwrap();
        assert!(!later.rested, "thirty minutes of default must not cut four known days short");
        assert_eq!(log.active_for(ACCOUNT, now + 2 * MINUTE)[0].class, LimitClass::Rate);

        let explicit = log
            .record(&seen(ACCOUNT, LimitClass::Session, "all", Some(now + 10 * MINUTE)), now + 2 * MINUTE)
            .unwrap();
        assert!(explicit.rested, "an explicit reset is the newest fact and wins");
        assert_eq!(log.active_for(ACCOUNT, now + 3 * MINUTE)[0].class, LimitClass::Session);

        // Different scopes are different rests.
        log.record(&seen(ACCOUNT, LimitClass::Weekly, "opus", None), now + 3 * MINUTE).unwrap();
        assert_eq!(log.active_for(ACCOUNT, now + 4 * MINUTE).len(), 2);
    }

    #[test]
    fn auth_rests_until_cleared_and_a_credential_change_clears_it() {
        let (_dir, log) = log();
        let now = 1_000 * MINUTE;
        let recorded = log.record(&seen(ACCOUNT, LimitClass::Auth, "all", None), now).unwrap();
        assert_eq!((recorded.rested, recorded.until_ms), (true, None), "indefinite");
        let far = now + 365 * 24 * 60 * MINUTE;
        assert_eq!(log.active_for(ACCOUNT, far).len(), 1, "still resting a year on");
        assert_eq!(log.active_for(ACCOUNT, far)[0].until_at.0, None);

        log.record(&seen(ACCOUNT, LimitClass::Weekly, "opus", None), now).unwrap();
        assert_eq!(log.clear_class(ACCOUNT, LimitClass::Auth).unwrap(), 1);
        let left = log.active_for(ACCOUNT, now + MINUTE);
        assert_eq!(left.len(), 1, "only the auth rest went");
        assert_eq!(left[0].class, LimitClass::Weekly);

        log.record(&seen(ACCOUNT, LimitClass::Auth, "all", None), now).unwrap();
        assert_eq!(
            log.active_observed_ms(ACCOUNT, LimitClass::Auth, far),
            Some(now),
            "when the login was caught failing"
        );
        assert_eq!(
            log.active_observed_ms(ACCOUNT, LimitClass::Session, far),
            None,
            "a class it is not resting under"
        );
        assert_eq!(log.active_observed_ms(LOCAL_CLAUDE, LimitClass::Auth, far), None, "nor another login");
        assert_eq!(log.clear(ACCOUNT, Some("opus")).unwrap(), 1);
        assert_eq!(log.active_for(ACCOUNT, now + MINUTE).len(), 1);
        assert_eq!(log.clear(ACCOUNT, None).unwrap(), 1);
        assert!(log.active_rests(now + MINUTE).is_empty());
    }

    #[test]
    fn other_is_rested_only_once_it_repeats_and_backs_off() {
        let (_dir, log) = log();
        let mut now = 1_000 * MINUTE;
        let other = seen(ACCOUNT, LimitClass::Other, "all", None);

        let first = log.record(&other, now).unwrap();
        assert!(!first.rested, "a one-off is recorded, not rested");
        assert!(log.active_rests(now).is_empty());

        let mut rests = Vec::new();
        for _ in 0..6 {
            now += MINUTE / 2;
            let recorded = log.record(&other, now).unwrap();
            assert!(recorded.rested);
            rests.push((recorded.until_ms.unwrap() - now) / MINUTE);
        }
        assert_eq!(rests, vec![1, 2, 4, 8, 10, 10]);

        // Anything else breaks the run, and so does an hour of quiet.
        log.record(&seen(ACCOUNT, LimitClass::Rate, "all", None), now).unwrap();
        assert!(!log.record(&other, now + MINUTE).unwrap().rested, "the run starts again");
        assert!(
            !log.record(&other, now + 3 * 60 * MINUTE).unwrap().rested,
            "an hour later it is a new one-off"
        );
    }

    /// claude reports one refused turn twice — the `rate_limit_event` and then
    /// the `result` — and the log keeps one event, while the rest keeps the
    /// longer of the two ends.
    #[test]
    fn one_hit_reported_twice_by_a_session_is_one_event_with_the_longer_rest() {
        let (_dir, log) = log();
        let now = 1_000 * MINUTE;
        let hour = now + 60 * MINUTE;

        let first = log.record(&seen(ACCOUNT, LimitClass::Session, "all", Some(hour)), now).unwrap();
        assert!(first.rested && !first.duplicate);
        // The `result` a moment later, its wall-clock reset a minute short.
        let second = log
            .record(&seen(ACCOUNT, LimitClass::Session, "all", Some(hour - MINUTE)), now + 5)
            .unwrap();
        assert!(second.duplicate, "{second:?}");
        assert_eq!(second.id, first.id, "it answers with the event it duplicates");
        assert!(!second.rested, "a shorter reading of the same reset does not shorten the rest");
        assert_eq!(log.events(None, 10).len(), 1);
        assert_eq!(log.active_for(ACCOUNT, now)[0].until_at.0, Some(iso_ms(hour)));

        // A longer one, still inside the window, lengthens it and logs nothing.
        let third = log
            .record(&seen(ACCOUNT, LimitClass::Session, "all", Some(hour + MINUTE)), now + 9_000)
            .unwrap();
        assert!(third.duplicate && third.rested);
        assert_eq!(log.events(None, 10).len(), 1);
        assert_eq!(log.active_for(ACCOUNT, now)[0].until_at.0, Some(iso_ms(hour + MINUTE)));

        // Not duplicates: another session, another class, another scope, and
        // the same hit again once the window has passed.
        let mut elsewhere = seen(ACCOUNT, LimitClass::Session, "all", Some(hour));
        elsewhere.session_id = Some("session-2".into());
        assert!(!log.record(&elsewhere, now + 9_500).unwrap().duplicate);
        assert!(!log.record(&seen(ACCOUNT, LimitClass::Rate, "all", None), now + 9_600).unwrap().duplicate);
        assert!(
            !log.record(&seen(ACCOUNT, LimitClass::Weekly, "opus", None), now + 9_700)
                .unwrap()
                .duplicate
        );
        assert!(
            !log.record(&seen(ACCOUNT, LimitClass::Session, "all", Some(hour)), now + 10_001)
                .unwrap()
                .duplicate
        );
        assert_eq!(log.events(None, 10).len(), 5);

        // A duplicate `other` is the same one-off, not a repeat that earns a rest.
        let other = seen(ACCOUNT, LimitClass::Other, "all", None);
        let later = now + 5 * MINUTE;
        assert!(!log.record(&other, later).unwrap().rested);
        let again = log.record(&other, later + 5).unwrap();
        assert!(again.duplicate && !again.rested);
        assert!(log.active_for(ACCOUNT, later).iter().all(|rest| rest.class != LimitClass::Other));
    }

    /// The first real Fable weekly hit, replayed through the log the way the
    /// claude driver records it: a `rate_limit_event` for a window of unknown
    /// scope, then the `result` naming the family a moment later.
    #[test]
    fn an_unscoped_weekly_event_lends_its_time_to_the_result_that_names_the_family() {
        use std::time::Instant;

        use super::super::limits::{read_rate_limit_event, read_text, with_hint, HintSlot, SCOPE_UNKNOWN};

        let info = serde_json::json!({
            "isUsingOverage": false, "overageDisabledReason": "org_level_disabled", "overageStatus": "rejected",
            "rateLimitType": "seven_day_overage_included", "resetsAt": 1790575200, "status": "rejected",
            "unifiedWindows": {
                "five_hour": {"resetsAt": 1790436000, "utilization": 0.02},
                "seven_day": {"resetsAt": 1790575200, "utilization": 0.82},
                "seven_day_overage_included": {"resetsAt": 1790575200, "utilization": 1},
            },
        });
        let text = "You've reached your Fable limit. Switch to another model, or manage usage credits at claude.ai/settings/usage?from=cc_cli_limit_message, to continue.";
        let now = 1_790_300_000_000; // 2026-09-25, well before the reset
        let observe = |verdict: Verdict, source: &'static str, raw: String, may_rest: bool| Observation {
            source,
            verdict,
            raw,
            reason: String::new(),
            may_rest,
            ..seen(ACCOUNT, LimitClass::Weekly, "all", None)
        };
        // What `note_rate_limit` does with the event.
        let event = |log: &LimitLog, slot: &mut HintSlot, at: Instant| {
            let reading = read_rate_limit_event(&info).expect("a refusal");
            let may_rest = reading.hint.is_none();
            if let Some(hint) = reading.hint {
                slot.hold(hint, at);
            }
            log.record(&observe(reading.verdict, "claude:rate_limit_event", info.to_string(), may_rest), now)
                .unwrap()
        };
        // What `note_result_limit` does with the result.
        let result = |log: &LimitLog, slot: &mut HintSlot, at: Instant| {
            let verdict = with_hint(read_text(text, Some(429)).expect("a limit"), slot.take(at).as_ref());
            log.record(&observe(verdict, "claude:result", text.to_string(), true), now + 400).unwrap()
        };
        let start = Instant::now();

        // Event, then the result within the window: one `fable` rest with the
        // event's time, and both rows in the log.
        let (_dir, log) = self::log();
        let mut slot = HintSlot::default();
        assert!(!event(&log, &mut slot, start).rested, "a window of unknown scope rests nothing");
        let merged = result(&log, &mut slot, start + std::time::Duration::from_millis(400));
        assert!(merged.rested);
        let rests = log.active_for(ACCOUNT, now + 1_000);
        assert_eq!(rests.len(), 1, "{rests:?}");
        assert_eq!((rests[0].class, rests[0].scope.as_str()), (LimitClass::Weekly, "fable"));
        assert_eq!(
            rests[0].until_at.0,
            Some(iso_ms(1_790_575_200_000)),
            "the event's reset, 2026-09-28T06:00Z"
        );
        let events = log.events(None, 10);
        assert_eq!(
            events.iter().map(|e| (e.source.as_str(), e.scope.as_str())).collect::<Vec<_>>(),
            vec![("claude:result", "fable"), ("claude:rate_limit_event", SCOPE_UNKNOWN)]
        );
        assert!(events[1].raw.contains("unifiedWindows"), "the whole payload is kept");

        // The event alone rests nothing.
        let (_dir, log) = self::log();
        let mut slot = HintSlot::default();
        event(&log, &mut slot, start);
        assert!(log.active_rests(now + 1_000).is_empty());
        assert_eq!(log.events(None, 10).len(), 1);

        // The result alone — or too late — rests `fable` for the weekly default.
        let (_dir, log) = self::log();
        let mut slot = HintSlot::default();
        result(&log, &mut slot, start);
        let alone = log.active_for(ACCOUNT, now + 1_000);
        assert_eq!(alone[0].scope, "fable");
        assert_eq!(alone[0].until_at.0, Some(iso_ms(now + 400 + 60 * MINUTE)));
        let (_dir, log) = self::log();
        let mut slot = HintSlot::default();
        event(&log, &mut slot, start);
        result(&log, &mut slot, start + std::time::Duration::from_secs(11));
        assert_eq!(log.active_for(ACCOUNT, now + 1_000)[0].until_at.0, Some(iso_ms(now + 400 + 60 * MINUTE)));
    }

    #[test]
    fn an_event_that_may_not_rest_is_only_an_event() {
        let (_dir, log) = log();
        let mut observation = seen(ACCOUNT, LimitClass::Rate, "all", None);
        observation.may_rest = false;
        observation.source = "codex:rate_limits";
        let recorded = log.record(&observation, 1_000 * MINUTE).unwrap();
        assert!(!recorded.rested);
        assert!(log.active_rests(1_000 * MINUTE).is_empty());
        assert_eq!(log.events(None, 10).len(), 1);
    }

    #[test]
    fn the_log_keeps_the_newest_five_hundred_and_the_local_login_goes_out_as_null() {
        let (_dir, log) = log();
        let now = 1_000 * MINUTE;
        for n in 0..(EVENTS_CAP + 20) {
            let account = if n % 2 == 0 { LOCAL_CLAUDE } else { ACCOUNT };
            // A session each, or they would be one hit reported many times.
            let mut observation = seen(account, LimitClass::Rate, "all", None);
            observation.session_id = Some(format!("session-{n}"));
            log.record(&observation, now + n).unwrap();
        }
        let all = log.events(None, 10_000);
        assert_eq!(all.len(), EVENTS_CAP as usize, "capped, and a limit past the cap is clamped to it");
        assert!(all.windows(2).all(|pair| pair[0].id > pair[1].id), "newest first");
        let total: i64 = log
            .db
            .read(|conn| conn.query_row("SELECT count(*) FROM account_limit_events", [], |row| row.get(0)))
            .unwrap();
        assert_eq!(total, EVENTS_CAP, "the table itself is trimmed, not just the answer");
        let newest_id = all[0].id;
        assert_eq!(all.last().unwrap().id, newest_id - EVENTS_CAP + 1, "the oldest went first");

        let local = log.events(Some(LOCAL_CLAUDE), 5);
        assert_eq!(local.len(), 5);
        assert!(
            local.iter().all(|event| event.account_id.0.is_none()),
            "the local login is null on the wire"
        );
        assert!(log
            .events(Some(ACCOUNT), 5)
            .iter()
            .all(|event| event.account_id.0.as_deref() == Some(ACCOUNT)));

        // The local login rests like anything else.
        assert_eq!(log.active_for(LOCAL_CLAUDE, now + 100).len(), 1);
    }

    #[test]
    fn a_session_on_the_local_login_is_filed_under_its_cli() {
        assert_eq!(account_key(None, AccountProvider::Claude), LOCAL_CLAUDE);
        assert_eq!(account_key(Some(""), AccountProvider::Codex), LOCAL_CODEX);
        assert_eq!(account_key(Some(ACCOUNT), AccountProvider::Claude), ACCOUNT);

        use crate::claude_roots::ClaudeRoot;
        assert_eq!(claude_limit_account(Some(ACCOUNT), Some(ClaudeRoot::Managed)).as_deref(), Some(ACCOUNT));
        assert_eq!(claude_limit_account(Some(ACCOUNT), None).as_deref(), Some(ACCOUNT));
        assert_eq!(
            claude_limit_account(None, Some(ClaudeRoot::Local)).as_deref(),
            Some(LOCAL_CLAUDE),
            "no account in ~/.claude is the local login"
        );
        assert_eq!(
            claude_limit_account(None, Some(ClaudeRoot::Managed)),
            None,
            "the managed root's own login is not the user's: its refusal must not sign the local login out"
        );
        assert_eq!(claude_limit_account(Some(""), Some(ClaudeRoot::Managed)), None);
        assert_eq!(claude_limit_account(None, None), None, "a root nobody can tell files nothing");
        assert!(is_local(LOCAL_CODEX) && !is_local(ACCOUNT));
    }

    #[test]
    fn raw_and_reason_are_clipped() {
        let (_dir, log) = log();
        let mut observation = seen(ACCOUNT, LimitClass::Rate, "all", None);
        observation.raw = "x".repeat(2_000);
        observation.reason = "y".repeat(2_000);
        log.record(&observation, 1_000 * MINUTE).unwrap();
        assert_eq!(log.events(None, 1)[0].raw.chars().count(), RAW_CAP);
        assert_eq!(log.active_for(ACCOUNT, 1_000 * MINUTE)[0].reason.chars().count(), REASON_CAP);
    }
}
