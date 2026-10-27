//! Moving a claude session off an account that hit a limit.
//!
//! Phase 2 of automatic account rotation. Phase 1 ([`super::limits`],
//! [`super::rest`]) reads what the CLI said and writes down who is resting
//! until when. This module is what acts on it: when a session's own turn ends
//! in a limit, which account it goes to next, what it is told when it gets
//! there, and what the person who owns it is told.
//!
//! Everything that decides is pure — accounts, rests and readings in, an order
//! or a sentence out — so the rules can be pinned without a CLI running out of
//! quota. [`RotationLog`] is the one piece with a database behind it: the latest
//! decisions per session, which `SessionDetail.rotation` reports.
//!
//! # The pool
//!
//! [`super::store::Account::rotation`]. A Claude OAuth account is in it unless
//! somebody takes it out; an env account and every codex account are out unless
//! somebody puts them in. A session only ever moves *from* an account in the
//! pool *to* another one in it: an env block is usually a different vendor
//! behind a gateway, with model names of its own, and moving a conversation
//! across that line is not something to do on a person's behalf. Codex
//! sessions and sessions on the machine's own login never move in this phase at
//! all.
//!
//! # The switch itself
//!
//! Not here: [`crate::inject`] owns the process. What this module hands it is a
//! [`Plan`] and the words — the continuation prompt the new account's process is
//! given ([`continuation_prompt`]) and the notification ([`notice`]).
//!
//! # Ahead of the limit
//!
//! Waiting for the refusal costs the turn it lands in, and a whole-account limit
//! takes claude's background subagents with it. The live readings say when an
//! account is about to run out, so a session can also be moved *before* that,
//! at a quiet point — a turn just ended, nothing running in the background —
//! where a `--resume` under another account loses nothing. [`plan_ahead`] is
//! that decision: a relevant window of the session's own account at or past
//! `DROPPEN_ROTATE_AHEAD_PERCENT` ([`ahead_threshold`]) on a fresh reading, and an
//! account to go to whose every relevant window is known to be well under it.
//! No continuation prompt: nothing failed, so there is nothing to carry on.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Datelike, TimeZone};
use gotcha::oas::Schema;
use gotcha::Schematic;
use gotcha_core::EnhancedSchema;
use serde::{Deserialize, Serialize};

use super::limits::{LimitClass, SCOPE_ALL};
use super::rest::iso_ms;
use super::store::{Account, AccountProvider, AccountRef, AccountsStore};
use crate::db::Db;
use crate::openapi_ext::Nullable;
use crate::quota::readings::UsageLogin;
use crate::quota::{QuotaCache, QuotaWindow};

/// At most this many automatic switches for one message and the continuations
/// that follow it. Three accounts refusing in a row is not bad luck.
pub const ROTATION_CAP: u8 = 3;
/// How recent a quota reading must be to rank an account by it.
pub const FRESH_READING_MS: i64 = 30 * 60 * 1000;
/// A window this full is spent, whatever the rounding says.
pub const EXHAUSTED_PERCENT: f64 = 98.0;
/// How many decisions the log keeps per session; the oldest go first.
pub const LOG_PER_SESSION: i64 = 20;
/// `DROPPEN_ROTATE_AHEAD_PERCENT` when nobody set it.
pub const AHEAD_DEFAULT_PERCENT: f64 = 95.0;
/// How far under the threshold every relevant window of an account must be for
/// a session to be moved onto it ahead of a limit. Without the gap, an account
/// a hair under the line would take the session and hand it straight on.
pub const AHEAD_HYSTERESIS: f64 = 10.0;
/// How recent the reading of a session's own window must be to act ahead of it.
pub const AHEAD_FRESH_MS: i64 = 15 * 60 * 1000;
/// At most one move ahead of a limit per session in this long.
pub const AHEAD_EVERY_MS: i64 = 30 * 60 * 1000;
/// A binding somebody changed by hand this recently is theirs: nothing moves
/// the session ahead of a limit until it has stood this long.
pub const HAND_BINDING_GRACE_MS: i64 = 30 * 60 * 1000;

/// The model families a limit can be scoped to, as [`super::limits`] names them.
const FAMILIES: [&str; 4] = ["fable", "opus", "sonnet", "haiku"];

// ------------------------------------------------------------ the rules

/// The family a model id or alias belongs to: `claude-fable-5-1`, `fable`,
/// `opus[1m]` and `claude-opus-4-1` all say which. `None` when the model is
/// unknown or names no family — a custom gateway model, the CLI's default.
pub fn model_family(model: Option<&str>) -> Option<&'static str> {
    let lower = model?.to_ascii_lowercase();
    FAMILIES.into_iter().find(|family| lower.contains(family))
}

/// Whether a limit of this class moves a session at all.
///
/// The five-hour window, a weekly one, credit and a refused credential all
/// last long enough that waiting on the same account is waiting for nothing.
/// A request-rate limit lifts in about a minute, and an unclassified one may
/// not be a limit at all, so neither is worth a respawn.
pub fn rotates_on(class: LimitClass) -> bool {
    matches!(class, LimitClass::Session | LimitClass::Weekly | LimitClass::Credit | LimitClass::Auth)
}

/// Whether a limit that ended a session's own turn moves the session.
///
/// `from` is the account the process was billing, when it names one this store
/// still holds: a session on the machine's own login is `None`, and never
/// moves. It has to be in the pool — unless `rebound`: somebody has already
/// bound the session to another account by hand, and the move is theirs.
pub fn triggers(class: LimitClass, from: Option<&Account>, rebound: bool) -> bool {
    rotates_on(class) && from.is_some_and(|account| account.provider == AccountProvider::Claude && (account.rotation || rebound))
}

/// Whether a rest of `scope` stops an account taking a session whose model is
/// of `family`: a whole-account rest always does, a family's only its own.
///
/// A session whose family is unknown — a model this module cannot place, or
/// none named at all — is kept off every family's rest as well: the model it
/// ends up running may well be the one that is resting, and a wasted respawn
/// onto a spent account costs more than passing over one that was fine.
pub fn rest_covers(scope: &str, family: Option<&str>) -> bool {
    scope == SCOPE_ALL || family.is_none_or(|family| family == scope)
}

/// When a switch may happen, relative to claude's background work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// At once. The limit covers every model, so the background work is
    /// billing the same exhausted credential and has nothing to finish.
    Now,
    /// Once claude's background tasks are done, within the ceiling every
    /// parked restart has. The limit is one model family's, and a subagent on
    /// another family can still finish what it is doing.
    ForBackground,
}

/// The wait a limit of `scope` allows.
pub fn wait_policy(scope: &str) -> Wait {
    if scope == SCOPE_ALL {
        Wait::Now
    } else {
        Wait::ForBackground
    }
}

/// What to do about a limit that triggers a rotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Move to this account.
    Switch(String),
    /// Nothing in the pool can take the session.
    NoCandidate,
    /// This message has already been moved [`ROTATION_CAP`] times.
    CapReached,
}

/// Where a session on `from` goes, all rules at once.
///
/// The cap first. Then a binding somebody set by hand while the process was
/// still running on `from`: that account, unless it is resting under a scope
/// that covers `family` too — then the ordinary choice, as if nobody had. Then
/// [`candidates`].
pub fn choose(pool: &[PoolAccount], from: &str, bound: Option<&str>, family: Option<&str>, switches_so_far: u8, now_ms: i64) -> Plan {
    if switches_so_far >= ROTATION_CAP {
        return Plan::CapReached;
    }
    if let Some(bound) = bound.filter(|bound| *bound != from) {
        let free = pool
            .iter()
            .find(|account| account.id == bound)
            .is_some_and(|account| !account.resting.iter().any(|scope| rest_covers(scope, family)));
        if free {
            return Plan::Switch(bound.to_string());
        }
    }
    plan(switches_so_far, &candidates(pool, from, family, now_ms))
}

/// The plan for a limit, given how many switches the failed message's chain has
/// already had and the candidates in order.
pub fn plan(switches_so_far: u8, candidates: &[String]) -> Plan {
    if switches_so_far >= ROTATION_CAP {
        return Plan::CapReached;
    }
    match candidates.first() {
        Some(id) => Plan::Switch(id.clone()),
        None => Plan::NoCandidate,
    }
}

// ------------------------------------------------------------ choosing

/// One account as the choice sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct PoolAccount {
    /// The account's uuid.
    pub id: String,
    /// Its display name.
    pub label: String,
    /// Whether it is in the rotation pool.
    pub in_pool: bool,
    /// The scopes of its active rests.
    pub resting: Vec<String>,
    /// Its latest quota reading, window by window.
    pub windows: Vec<Window>,
}

/// One quota window, with the two instants the choice needs as numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    /// `5h`, `weekly`, `weekly (Fable)`…
    pub label: String,
    /// 0–100.
    pub used_percent: f64,
    /// When the window resets, epoch milliseconds.
    pub resets_at_ms: Option<i64>,
    /// When it was read, epoch milliseconds.
    pub read_at_ms: Option<i64>,
}

impl Window {
    /// A panel window, its ISO-8601 instants read back into numbers.
    pub fn from_quota(window: &QuotaWindow) -> Window {
        let millis = |iso: Option<&String>| {
            iso.and_then(|text| DateTime::parse_from_rfc3339(text).ok())
                .map(|at| at.timestamp_millis())
        };
        Window {
            label: window.label.clone(),
            used_percent: window.used_percent,
            resets_at_ms: millis(window.resets_at.0.as_ref()),
            read_at_ms: millis(window.updated_at.0.as_ref()),
        }
    }
}

/// The weekly window of one family, as [`crate::quota::readings`] labels it.
fn family_window(family: &str) -> Option<&'static str> {
    match family {
        "fable" => Some("weekly (Fable)"),
        "opus" => Some("weekly (Opus)"),
        "sonnet" => Some("weekly (Sonnet only)"),
        "haiku" => Some("weekly (Haiku)"),
        _ => None,
    }
}

/// Whether a window of this label speaks for a session of `family`: `5h` and
/// `weekly` always, a family's own weekly window only for that family's
/// sessions. A session whose family is unknown has only the first two.
pub fn relevant(label: &str, family: Option<&str>) -> bool {
    label == "5h" || label == "weekly" || family.and_then(family_window) == Some(label)
}

/// Where an account stands, by its reading.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Standing {
    /// Read recently, every relevant window below [`EXHAUSTED_PERCENT`]; the
    /// number is the tightest relevant window's remaining percent.
    Ready(f64),
    /// No reading, or one too old to rank by.
    Unknown,
    /// A relevant window at or past [`EXHAUSTED_PERCENT`].
    Spent,
}

/// How one account stands for a session of `family`, at `now_ms`.
///
/// The relevant windows are `5h`, `weekly`, and the family's own weekly window
/// when the reading has one. A window whose reset has passed counts as empty,
/// however full it was when it was read — the reading is of a window that no
/// longer exists.
fn standing(account: &PoolAccount, family: Option<&str>, now_ms: i64) -> Standing {
    let relevant: Vec<&Window> = account.windows.iter().filter(|window| relevant(&window.label, family)).collect();
    let used = |window: &Window| {
        if window.resets_at_ms.is_some_and(|at| at <= now_ms) {
            0.0
        } else {
            window.used_percent
        }
    };
    if relevant.iter().any(|window| used(window) >= EXHAUSTED_PERCENT) {
        return Standing::Spent;
    }
    let has = |label: &str| relevant.iter().any(|window| window.label == label);
    let fresh = has("5h")
        && has("weekly")
        && relevant
            .iter()
            .all(|window| window.read_at_ms.is_some_and(|at| now_ms.saturating_sub(at) < FRESH_READING_MS));
    if !fresh {
        return Standing::Unknown;
    }
    Standing::Ready(relevant.iter().map(|window| 100.0 - used(window)).fold(f64::INFINITY, f64::min))
}

/// The accounts a session on `current` may move to, best first.
///
/// Only pool accounts other than `current`, and none resting under a scope that
/// covers `family`. Then three groups, in this order:
///
/// 1. read in the last [`FRESH_READING_MS`] with every relevant window under
///    [`EXHAUSTED_PERCENT`], the most headroom in its tightest window first;
/// 2. no reading, or a stale one, in the order the accounts are listed;
/// 3. anything with a relevant window at [`EXHAUSTED_PERCENT`] or more, last —
///    a reading can be wrong, and a nearly-spent account beats none.
pub fn candidates(pool: &[PoolAccount], current: &str, family: Option<&str>, now_ms: i64) -> Vec<String> {
    let mut ready: Vec<(f64, usize, &PoolAccount)> = Vec::new();
    let mut unknown: Vec<&PoolAccount> = Vec::new();
    let mut spent: Vec<&PoolAccount> = Vec::new();
    for (index, account) in pool.iter().enumerate() {
        if !account.in_pool || account.id == current {
            continue;
        }
        if account.resting.iter().any(|scope| rest_covers(scope, family)) {
            continue;
        }
        match standing(account, family, now_ms) {
            Standing::Ready(remaining) => ready.push((remaining, index, account)),
            Standing::Unknown => unknown.push(account),
            Standing::Spent => spent.push(account),
        }
    }
    ready.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(Ordering::Equal).then(a.1.cmp(&b.1)));
    ready
        .into_iter()
        .map(|(_, _, account)| account)
        .chain(unknown)
        .chain(spent)
        .map(|account| account.id.clone())
        .collect()
}

// ------------------------------------------------------------ ahead of a limit

/// `DROPPEN_ROTATE_AHEAD_PERCENT`, read: the used percent at which a session is
/// moved off its account before the limit, or `None` when that is switched off.
///
/// Unset, empty or unreadable is the default, [`AHEAD_DEFAULT_PERCENT`]; `0`
/// (or anything at or below it) switches the whole behaviour off.
pub fn ahead_threshold(raw: Option<&str>) -> Option<f64> {
    match raw
        .map(str::trim)
        .and_then(|raw| raw.parse::<f64>().ok())
        .filter(|percent| percent.is_finite())
    {
        Some(percent) if percent <= 0.0 => None,
        Some(percent) => Some(percent),
        None => Some(AHEAD_DEFAULT_PERCENT),
    }
}

/// [`ahead_threshold`] of this process' environment.
pub fn ahead_threshold_from_env() -> Option<f64> {
    ahead_threshold(crate::env::var("DROPPEN_ROTATE_AHEAD_PERCENT").as_deref())
}

/// A window that is about to run out, as the decision and its record name it.
#[derive(Debug, Clone, PartialEq)]
pub struct Near {
    /// `5h`, `weekly`, `weekly (Fable)`…
    pub label: String,
    /// [`LimitClass::Session`] for the five-hour window, [`LimitClass::Weekly`]
    /// for a weekly one.
    pub class: LimitClass,
    /// `all`, or the family whose own weekly window it is.
    pub scope: String,
    /// How much of it is gone, 0–100.
    pub used_percent: f64,
    /// When it resets, epoch milliseconds.
    pub resets_at_ms: Option<i64>,
}

/// The class and scope a window of this label would be refused under.
fn window_limit(label: &str) -> (LimitClass, String) {
    if label == "5h" {
        return (LimitClass::Session, SCOPE_ALL.to_string());
    }
    let scope = FAMILIES
        .into_iter()
        .find(|family| family_window(family) == Some(label))
        .unwrap_or(SCOPE_ALL);
    (LimitClass::Weekly, scope.to_string())
}

/// The window of an account's reading a session of `family` is about to run
/// out of, if there is one: relevant ([`relevant`]), at or past `threshold`,
/// read in the last [`AHEAD_FRESH_MS`], and with its reset still to come — a
/// window that has reset is not the one that was read. The fullest when more
/// than one is; the shorter on a tie.
pub fn near_window(windows: &[Window], family: Option<&str>, threshold: f64, now_ms: i64) -> Option<Near> {
    windows
        .iter()
        .filter(|window| relevant(&window.label, family))
        .filter(|window| window.used_percent >= threshold)
        .filter(|window| window.read_at_ms.is_some_and(|at| now_ms.saturating_sub(at) < AHEAD_FRESH_MS))
        .filter(|window| window.resets_at_ms.is_some_and(|at| at > now_ms))
        .fold(None::<&Window>, |best, window| match best {
            Some(best) if best.used_percent >= window.used_percent => Some(best),
            _ => Some(window),
        })
        .map(|window| {
            let (class, scope) = window_limit(&window.label);
            Near {
                label: window.label.clone(),
                class,
                scope,
                used_percent: window.used_percent,
                resets_at_ms: window.resets_at_ms,
            }
        })
}

/// Whether a session of `family` may be moved onto `account` ahead of a limit:
/// a reading recent enough to rank by ([`FRESH_READING_MS`]) in which every
/// relevant window is under `threshold − `[`AHEAD_HYSTERESIS`]. No reading, or
/// a stale one, is not known to be good, however good it once was.
pub fn known_good(account: &PoolAccount, family: Option<&str>, threshold: f64, now_ms: i64) -> bool {
    match standing(account, family, now_ms) {
        // The tightest window's remaining percent, so every window is under the
        // line exactly when that one is.
        Standing::Ready(remaining) => 100.0 - remaining < threshold - AHEAD_HYSTERESIS,
        Standing::Unknown | Standing::Spent => false,
    }
}

/// The best account a session on `from` can be moved to ahead of a limit:
/// [`candidates`]' first that is [`known_good`]. The ordering puts the most
/// headroom first, so when that one is not good enough none of the read ones
/// is, and an unread one is never enough for a move nothing forced.
pub fn ahead_candidate(pool: &[PoolAccount], from: &str, family: Option<&str>, threshold: f64, now_ms: i64) -> Option<String> {
    candidates(pool, from, family, now_ms).into_iter().find(|id| {
        pool.iter()
            .find(|account| &account.id == id)
            .is_some_and(|account| known_good(account, family, threshold, now_ms))
    })
}

/// What the session's own circumstances say about a move ahead of a limit,
/// gathered by its driver.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AheadGuards {
    /// Claude is minding background work: a respawn now would cut it off.
    pub background: bool,
    /// When this session last moved ahead of a limit, epoch milliseconds.
    pub last_ahead_ms: Option<i64>,
    /// When somebody last bound this session to an account by hand.
    pub bound_by_hand_ms: Option<i64>,
}

/// Why a move ahead of a limit that is due does not happen now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deferral {
    /// Background work is running; the next quiet point asks again.
    Background,
    /// The session already moved ahead of a limit in the last [`AHEAD_EVERY_MS`].
    RecentlyMoved,
    /// Somebody bound it by hand in the last [`HAND_BINDING_GRACE_MS`].
    BoundByHand,
}

/// A move ahead of a limit, decided.
#[derive(Debug, Clone, PartialEq)]
pub enum AheadPlan {
    /// Nothing to do: switched off, outside the pool, or nothing near.
    Stay,
    /// A window is near, and something says not now.
    Defer(Near, Deferral),
    /// A window is near and no account is known to be good enough.
    NoCandidate(Near),
    /// Move to this account.
    Switch(String, Near),
}

/// Whether a session of `family` on `from` moves ahead of a limit, and where.
///
/// In order: the account must be in the pool, a relevant window near
/// ([`near_window`]); then background work, a move in the last
/// [`AHEAD_EVERY_MS`] and a binding set by hand in the last
/// [`HAND_BINDING_GRACE_MS`] each put it off; then [`ahead_candidate`].
pub fn plan_ahead(pool: &[PoolAccount], from: &str, family: Option<&str>, threshold: f64, guards: AheadGuards, now_ms: i64) -> AheadPlan {
    let Some(account) = pool.iter().find(|account| account.id == from && account.in_pool) else {
        return AheadPlan::Stay;
    };
    let Some(near) = near_window(&account.windows, family, threshold, now_ms) else {
        return AheadPlan::Stay;
    };
    let within = |at: Option<i64>, span: i64| at.is_some_and(|at| now_ms.saturating_sub(at) < span);
    if guards.background {
        return AheadPlan::Defer(near, Deferral::Background);
    }
    if within(guards.last_ahead_ms, AHEAD_EVERY_MS) {
        return AheadPlan::Defer(near, Deferral::RecentlyMoved);
    }
    if within(guards.bound_by_hand_ms, HAND_BINDING_GRACE_MS) {
        return AheadPlan::Defer(near, Deferral::BoundByHand);
    }
    match ahead_candidate(pool, from, family, threshold, now_ms) {
        Some(to) => AheadPlan::Switch(to, near),
        None => AheadPlan::NoCandidate(near),
    }
}

/// The account a new session should start on, given the default.
///
/// The default, unless it is in the pool and resting under a scope that covers
/// the model asked for — then the first candidate [`candidates`] names, or the
/// default after all when there is none. Or, with `ahead` set, unless one of its
/// relevant windows is about to run out ([`near_window`]) — then the first
/// [`known_good`] candidate, or the default when nothing is known to be better.
/// A default outside the pool is never replaced, for the reason the module note
/// gives.
pub fn default_for_new_session(default_id: &str, pool: &[PoolAccount], family: Option<&str>, ahead: Option<f64>, now_ms: i64) -> String {
    let Some(default) = pool.iter().find(|account| account.id == default_id) else {
        return default_id.to_string();
    };
    if !default.in_pool {
        return default_id.to_string();
    }
    if default.resting.iter().any(|scope| rest_covers(scope, family)) {
        return candidates(pool, default_id, family, now_ms)
            .into_iter()
            .next()
            .unwrap_or_else(|| default_id.to_string());
    }
    ahead
        .filter(|threshold| near_window(&default.windows, family, *threshold, now_ms).is_some())
        .and_then(|threshold| ahead_candidate(pool, default_id, family, threshold, now_ms))
        .unwrap_or_else(|| default_id.to_string())
}

// ------------------------------------------------------------ a start nobody pointed

/// Why a start that names no account runs where it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "snake_case")]
pub enum StartReason {
    /// The configured default.
    Default,
    /// Another account in the pool: the configured default is resting, or
    /// about to run out, on what the start asks for.
    Rotated,
    /// The machine's own login: no default, and nothing says it is signed out.
    Local,
    /// The machine's own login, because somebody made it the default.
    LocalChosen,
    /// An account in the pool, standing in for the machine's own login: no
    /// default, and the local login is signed out.
    Fallback,
    /// The machine's own login although it is signed out: no default, and no
    /// account in the pool free to stand in for it. The start fails on its
    /// credential and says how to sign in again.
    LocalSignedOut,
}

/// `GET /api/accounts/start`: the login a start that names no account runs on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AccountStart {
    /// The stored account; `null` is the machine's own login.
    pub account_id: Nullable<String>,
    /// Why that one.
    pub reason: StartReason,
    /// The model to send, when the start lets this answer choose it
    /// (`chooseModel`) and the choice decides where it runs: one of this
    /// login's own models that the rule places on this login. `null` when the
    /// start names its model, or when any model lands on the same login — a
    /// composer then takes the head of the login's list, as it always has.
    pub model: Nullable<String>,
    /// Whether a start that names no account, sent with `model` (or with the
    /// model the query named), lands on `accountId` by this same rule. `false`
    /// only when no login's own models land on it: a start that wants this pair
    /// then has to name the account.
    pub placed: bool,
}

/// The families a start's model can name, and none, as [`StartInputs::model_matters`] asks about them.
const START_FAMILIES: [Option<&str>; 5] = [None, Some("fable"), Some("opus"), Some("sonnet"), Some("haiku")];

/// What [`start_choice`] reads, gathered once — everything but the model — so
/// one answer can ask it about several models.
#[derive(Debug, Clone)]
pub struct StartInputs {
    /// The CLI.
    pub provider: AccountProvider,
    /// Its configured default account.
    pub default: Option<String>,
    /// Its local login.
    pub local: super::local_login::LocalLogin,
    /// Its accounts, as the choice sees them.
    pub pool: Vec<PoolAccount>,
    /// `DROPPEN_ROTATE_AHEAD_PERCENT`.
    pub ahead: Option<f64>,
    /// Now, epoch milliseconds.
    pub now_ms: i64,
}

impl StartInputs {
    /// Everything the rule reads for `provider`, from the store and the readings.
    pub async fn gather(accounts: &AccountsStore, quota: &QuotaCache, provider: AccountProvider) -> StartInputs {
        let logins = accounts.local_logins();
        StartInputs {
            provider,
            default: accounts.default_account_id(provider).await,
            local: match provider {
                AccountProvider::Claude => logins.claude,
                AccountProvider::Codex => logins.codex,
            },
            pool: pool_of(accounts, quota, provider).await,
            ahead: ahead_threshold_from_env(),
            now_ms: super::rest::now_ms(),
        }
    }

    /// Where a start asking for `model` (none: the CLI's own default) runs.
    pub fn place(&self, model: Option<&str>) -> AccountStart {
        let family = match self.provider {
            AccountProvider::Claude => model_family(model),
            // Codex models name no claude family; every rest stands in the way.
            AccountProvider::Codex => None,
        };
        self.place_family(family)
    }

    fn place_family(&self, family: Option<&str>) -> AccountStart {
        start_choice(self.provider, self.default.as_deref(), &self.local, &self.pool, family, self.ahead, self.now_ms)
    }

    /// Whether the model a start asks for can change the login it runs on:
    /// a default resting (or about to run out) on one family and not another, a
    /// signed-out local login whose stand-in depends on the family. Never for
    /// codex, whose models name no family.
    pub fn model_matters(&self) -> bool {
        let first = self.place_family(None).account_id;
        START_FAMILIES[1..]
            .iter()
            .any(|family| self.place_family(*family).account_id != first)
    }

    /// The logins to try, in the order a start reaches them, for a start that
    /// will ask for a model of the login it lands on: the configured default,
    /// or a local login nothing condemns (or somebody chose); then the pool,
    /// ranked as a start naming no model would rank it, then the rest of the
    /// pool; then the local login.
    pub fn start_order(&self) -> Vec<Option<String>> {
        let mut order: Vec<Option<String>> = Vec::new();
        let mut push = |login: Option<String>| {
            if !order.contains(&login) {
                order.push(login);
            }
        };
        match &self.default {
            Some(default) => push(Some(default.clone())),
            None if self.local.is_default || self.local.state != super::local_login::LocalLoginState::SignedOut => push(None),
            None => {}
        }
        for id in candidates(&self.pool, self.default.as_deref().unwrap_or(""), None, self.now_ms) {
            push(Some(id));
        }
        for account in self.pool.iter().filter(|account| account.in_pool) {
            push(Some(account.id.clone()));
        }
        push(None);
        order
    }

    /// The first of `login`'s own models, in its list's order, that this rule
    /// places on `login` itself — the list's head unless that head is resting
    /// there (or about to run out), then the next model it can still run.
    pub fn own_model<'m>(&self, login: Option<&str>, models: &'m [String]) -> Option<&'m str> {
        models
            .iter()
            .map(String::as_str)
            .find(|model| self.place(Some(model)).account_id.0.as_deref() == login)
    }

    /// The answer for a start that lets the answer choose its model, given
    /// each login's own list in [`Self::start_order`] (`lists`): the first
    /// login with a model of its own the rule places on it, and that model.
    /// When the model does not matter, no model ([`Self::model_matters`]).
    /// When no login has one, the login a start naming no model gets, with the
    /// head of its list and `placed: false` — a pair only naming the account
    /// reaches, which a start then sends with `accountFrom: "start"`.
    pub fn choose(&self, lists: &[(Option<String>, Vec<String>)]) -> AccountStart {
        if !self.model_matters() {
            return self.place(None);
        }
        for (login, models) in lists {
            if let Some(model) = self.own_model(login.as_deref(), models) {
                let mut start = self.place(Some(model));
                start.model = Nullable(Some(model.to_string()));
                return start;
            }
        }
        let mut start = self.place(None);
        start.model = Nullable(
            lists
                .iter()
                .find(|(login, _)| *login == start.account_id.0)
                .and_then(|(_, models)| models.first().cloned()),
        );
        start.placed = false;
        start
    }

    /// Whether `account`, named by a start with `accountFrom: "start"`, is
    /// still the rule's choice for a pair no model-less start reaches: the
    /// login a start naming no model gets ([`Self::choose`]'s last answer).
    pub fn still_names(&self, account: &str) -> bool {
        self.place(None).account_id.0.as_deref() == Some(account)
    }
}

/// The login a new session that names no account starts on, all rules at once.
///
/// 1. A configured default — moved, for claude, off a rest or a window about to
///    run out by [`default_for_new_session`] (codex sessions never move).
/// 2. The machine's own login when somebody chose it as the default.
/// 3. The machine's own login when nothing says it is signed out.
/// 4. Signed out, and nobody chose it: the first of [`candidates`] — an account
///    *in the pool*, not resting under a scope that covers `family`. Never an
///    account out of the pool: an env block or an API key behind a gateway is
///    out of it by default for the module note's reason, and a start billed to
///    one because the user's own login lapsed is not something to do on their
///    behalf.
/// 5. Nothing free in the pool: the local login after all, whose refusal says
///    what to do, rather than an account that would only refuse as well.
pub fn start_choice(
    provider: AccountProvider,
    default: Option<&str>,
    local: &super::local_login::LocalLogin,
    pool: &[PoolAccount],
    family: Option<&str>,
    ahead: Option<f64>,
    now_ms: i64,
) -> AccountStart {
    let on = |account_id: Option<String>, reason: StartReason| AccountStart {
        account_id: Nullable(account_id),
        reason,
        model: Nullable(None),
        placed: true,
    };
    if let Some(default) = default {
        let chosen = match provider {
            AccountProvider::Claude => default_for_new_session(default, pool, family, ahead, now_ms),
            AccountProvider::Codex => default.to_string(),
        };
        let reason = if chosen == default { StartReason::Default } else { StartReason::Rotated };
        return on(Some(chosen), reason);
    }
    if local.is_default {
        return on(None, StartReason::LocalChosen);
    }
    if local.state != super::local_login::LocalLoginState::SignedOut {
        return on(None, StartReason::Local);
    }
    match candidates(pool, "", family, now_ms).into_iter().next() {
        Some(stand_in) => on(Some(stand_in), StartReason::Fallback),
        None => on(None, StartReason::LocalSignedOut),
    }
}

/// [`start_choice`] against the store: where a `provider` session asking for
/// `model` starts when nobody named an account — what a chat, a scheduled task
/// and `POST /api/sessions/new` without `accountId` do, and what
/// `GET /api/accounts/start` tells a picker beforehand.
pub async fn start_for(accounts: &AccountsStore, quota: &QuotaCache, provider: AccountProvider, model: Option<&str>) -> AccountStart {
    StartInputs::gather(accounts, quota, provider).await.place(model)
}

/// [`default_for_new_session`] against the store: the account a new claude
/// session asking for `model` should start on, given the configured default
/// and `DROPPEN_ROTATE_AHEAD_PERCENT`.
pub async fn default_for(accounts: &AccountsStore, quota: &QuotaCache, default_id: &str, model: Option<&str>) -> String {
    let pool = pool(accounts, quota).await;
    default_for_new_session(default_id, &pool, model_family(model), ahead_threshold_from_env(), super::rest::now_ms())
}

/// Every claude account, as the choice sees it: pool membership, active rests,
/// and the latest reading of each.
pub async fn pool(accounts: &AccountsStore, quota: &QuotaCache) -> Vec<PoolAccount> {
    pool_of(accounts, quota, AccountProvider::Claude).await
}

/// [`pool`] for either CLI. A codex account carries no windows: its readings
/// are another protocol's, so it ranks by the order the accounts are listed.
pub async fn pool_of(accounts: &AccountsStore, quota: &QuotaCache, provider: AccountProvider) -> Vec<PoolAccount> {
    accounts
        .list()
        .await
        .into_iter()
        .filter(|account| account.provider == provider)
        .map(|account| PoolAccount {
            windows: match provider {
                AccountProvider::Claude => quota
                    .cached(&UsageLogin::Account(account.id.clone()))
                    .windows
                    .iter()
                    .map(Window::from_quota)
                    .collect(),
                AccountProvider::Codex => Vec::new(),
            },
            resting: account.rest.iter().map(|rest| rest.scope.clone()).collect(),
            in_pool: account.rotation,
            label: account.label,
            id: account.id,
        })
        .collect()
}

// ------------------------------------------------------------ the words

/// What a limit of this class and scope means, in the words the prompt and the
/// notification use.
pub fn reason(class: LimitClass, scope: &str) -> String {
    match class {
        LimitClass::Session => "5 小时额度用完".to_string(),
        LimitClass::Weekly if scope == SCOPE_ALL => "每周额度用完".to_string(),
        LimitClass::Weekly => format!("{} 每周额度用完", family_title(scope)),
        LimitClass::Credit => "余额不足".to_string(),
        LimitClass::Auth => "登录凭据失效".to_string(),
        LimitClass::Rate => "请求过于频繁".to_string(),
        LimitClass::Other => "触发了用量限制".to_string(),
    }
}

/// `fable` → `Fable`.
fn family_title(family: &str) -> String {
    let mut chars = family.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// When a limit lifts, as the person reading about it on their phone would say
/// it in `now`'s zone: `23:10` today, `明天 07:00`, `9月30日 07:00` further out.
pub fn until_text<Tz: TimeZone>(until_ms: Option<i64>, now: &DateTime<Tz>) -> Option<String>
where
    Tz::Offset: std::fmt::Display,
{
    let at = now.timezone().timestamp_millis_opt(until_ms?).single()?;
    let clock = at.format("%H:%M").to_string();
    let today = now.date_naive();
    let day = at.date_naive();
    Some(if day == today {
        clock
    } else if today.succ_opt() == Some(day) {
        format!("明天 {clock}")
    } else {
        format!("{}月{}日 {clock}", day.month(), day.day())
    })
}

/// [`until_text`] in this server's own zone.
pub fn until_text_local(until_ms: Option<i64>) -> Option<String> {
    until_text(until_ms, &chrono::Local::now())
}

/// `a` followed by `b`, with the space Chinese typesetting puts between a CJK
/// character and the Latin letter or digit after it: `的 5 小时`, `的 Fable`.
fn spaced(a: &str, b: &str) -> String {
    let latin = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    if !a.ends_with(' ') && !latin(a.chars().last()) && latin(b.chars().next()) {
        format!("{a} {b}")
    } else {
        format!("{a}{b}")
    }
}

/// A background task a switch cut short, as the continuation lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interrupted {
    /// What the task was asked to do, or its id when claude gave no description.
    pub description: String,
    /// Where its output went, when there is a file to read.
    pub output: Option<String>,
}

/// The message the new account's process is handed instead of the one that
/// failed.
///
/// Not the original prompt: that is already in the transcript, possibly with
/// half of its answer after it, and sending it again would run the half that
/// happened twice. This tells the model what changed and asks it to carry on.
pub fn continuation_prompt(from: &str, to: &str, reason: &str, until: Option<&str>, interrupted: &[Interrupted]) -> String {
    let mut text = format!(
        "[Droppen] 这个会话已从账号 {from} 自动切换到 {to}：{}",
        spaced(&format!("{from} 的"), reason)
    );
    if let Some(until) = until {
        text.push_str(&format!("，{until} 恢复"));
    }
    text.push_str("。上一条消息因此没有完成，请从中断处继续。");
    if !interrupted.is_empty() {
        text.push_str("\n\n以下后台子任务被中断，请检查它们的输出后按需重新派发：");
        for task in interrupted {
            text.push_str("\n- ");
            text.push_str(&task.description);
            if let Some(output) = &task.output {
                text.push_str(&format!("（输出：{output}）"));
            }
        }
    }
    text
}

/// What a near window is called in the notification: `5 小时额度`,
/// `每周额度`, `Fable 每周额度`.
fn window_words(class: LimitClass, scope: &str) -> String {
    match class {
        LimitClass::Session => "5 小时额度".to_string(),
        _ if scope == SCOPE_ALL => "每周额度".to_string(),
        _ => format!("{} 每周额度", family_title(scope)),
    }
}

/// A used percent as the notification says it: whole, and never past 100.
pub fn percent_text(used: f64) -> String {
    format!("{:.0}", used.clamp(0.0, 100.0))
}

/// The one line a phone is sent about a move ahead of a limit:
/// `已提前从 A 切换到 B（A 的 5 小时额度已用 96%，23:10 重置）`.
pub fn ahead_notice(from: &str, to: &str, near: &Near, until: Option<&str>) -> String {
    let used = format!(
        "{}已用 {}%",
        spaced(&format!("{from} 的"), &window_words(near.class, &near.scope)),
        percent_text(near.used_percent)
    );
    match until {
        Some(until) => format!("已提前从 {from} 切换到 {to}（{used}，{until} 重置）"),
        None => format!("已提前从 {from} 切换到 {to}（{used}）"),
    }
}

/// The message a held session is handed when its own account's limit lifts
/// and nothing else took it over: the same account, carrying on.
pub fn resumed_prompt(account: &str) -> String {
    format!("[Droppen] 账号 {account} 的额度已恢复。上一条消息因额度用完没有完成，请从中断处继续。")
}

/// The one line a phone is sent about a decision.
pub fn notice(outcome: RotationOutcome, from: &str, to: &str, reason: &str, until: Option<&str>) -> String {
    let lifts = |text: &str| match until {
        Some(until) => format!("{text}（{until} 恢复）"),
        None => text.to_string(),
    };
    match outcome {
        RotationOutcome::Switched => match until {
            Some(until) => format!("已从 {from} 切到 {to}（{reason}，{until} 恢复）"),
            None => format!("已从 {from} 切到 {to}（{reason}）"),
        },
        RotationOutcome::NoCandidate => lifts(&format!("{}，没有可切换的账号", spaced(&format!("{from} 的"), reason))),
        RotationOutcome::CapReached => lifts(&format!(
            "{}，已连续自动切换 {ROTATION_CAP} 次，不再切换",
            spaced(&format!("{from} 的"), reason)
        )),
    }
}

// ------------------------------------------------------------ the record

/// How a rotation decision came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationOutcome {
    /// The session now runs under the new account.
    Switched,
    /// No account in the pool was free.
    NoCandidate,
    /// The attempt limit ran out first.
    CapReached,
}

impl RotationOutcome {
    /// The name the table and the API use.
    pub fn as_str(self) -> &'static str {
        match self {
            RotationOutcome::Switched => "switched",
            RotationOutcome::NoCandidate => "no_candidate",
            RotationOutcome::CapReached => "cap_reached",
        }
    }

    /// The inverse of [`RotationOutcome::as_str`]; an unknown row reads as no
    /// candidate, the one outcome that claims nothing moved.
    pub fn parse(raw: &str) -> Self {
        match raw {
            "switched" => RotationOutcome::Switched,
            "cap_reached" => RotationOutcome::CapReached,
            _ => RotationOutcome::NoCandidate,
        }
    }
}

/// Hand-written so the enum stays *inline* in `SessionRotation`, the way the
/// contract spells `outcome`, rather than becoming a schema of its own. See
/// `crate::files::FsKind` for the same arrangement.
impl Schematic for RotationOutcome {
    fn name() -> &'static str {
        "RotationOutcome"
    }

    fn required() -> bool {
        true
    }

    fn type_() -> &'static str {
        "string"
    }

    fn generate_schema() -> EnhancedSchema {
        let variants = serde_json::json!(["switched", "no_candidate", "cap_reached"]);
        EnhancedSchema {
            schema: Schema {
                _type: Some("string".to_string()),
                format: None,
                nullable: None,
                description: None,
                extras: BTreeMap::from([("enum".to_string(), variants)]),
            },
            required: true,
        }
    }
}

/// What set a rotation off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationTrigger {
    /// A turn of the session's own ended on the account's limit. Every row
    /// written before the trigger was recorded is one of these.
    Limit,
    /// A window of the account's was about to run out, and the session was
    /// moved at a quiet point before it did.
    Ahead,
}

impl RotationTrigger {
    /// The name the table and the API use.
    pub fn as_str(self) -> &'static str {
        match self {
            RotationTrigger::Limit => "limit",
            RotationTrigger::Ahead => "ahead",
        }
    }

    /// The inverse of [`RotationTrigger::as_str`]; a row with none, or with one
    /// this build does not know, is a limit — the only kind there used to be.
    pub fn parse(raw: Option<&str>) -> Self {
        match raw {
            Some("ahead") => RotationTrigger::Ahead,
            _ => RotationTrigger::Limit,
        }
    }
}

/// Inline in `SessionRotation`, like [`RotationOutcome`].
impl Schematic for RotationTrigger {
    fn name() -> &'static str {
        "RotationTrigger"
    }

    fn required() -> bool {
        true
    }

    fn type_() -> &'static str {
        "string"
    }

    fn generate_schema() -> EnhancedSchema {
        let variants = serde_json::json!(["limit", "ahead"]);
        EnhancedSchema {
            schema: Schema {
                _type: Some("string".to_string()),
                format: None,
                nullable: None,
                description: None,
                extras: BTreeMap::from([("enum".to_string(), variants)]),
            },
            required: true,
        }
    }
}

/// One automatic account rotation of a session, as `SessionDetail.rotation`
/// reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SessionRotation {
    /// The account that hit the limit, or was about to.
    pub from: AccountRef,
    /// The account it was moved to; only a move when `outcome` is `switched`.
    pub to: AccountRef,
    /// When it happened, ISO-8601.
    pub at: String,
    /// Which kind of limit the old account hit, or was about to hit.
    pub class: LimitClass,
    /// `all`, or the model family the limit belongs to.
    pub scope: String,
    /// When the old account's limit lifts, ISO-8601; `null` when unknown. For
    /// a move `ahead` of a limit, when the window that was near resets.
    pub until_at: Nullable<String>,
    /// Background subagents the move cut short.
    pub interrupted_tasks: u32,
    /// `switched` when the session now runs under `to`; `no_candidate` when no
    /// account in the pool was free; `cap_reached` when the attempt limit ran
    /// out first.
    pub outcome: RotationOutcome,
    /// `limit` when a turn ended on the old account's limit; `ahead` when the
    /// session was moved at a quiet point because a window of the old account
    /// was about to run out.
    // Deliberately not `#[serde(default)]`: gotcha reads that as optional,
    // and the contract promises the field on every response.
    pub trigger: RotationTrigger,
    /// For a move `ahead` of a limit, how much of the near window was gone,
    /// 0–100; `null` otherwise.
    pub used_percent: Option<f64>,
}

/// A decision, ready to be written down.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    /// The session it was about.
    pub session_id: String,
    /// When, epoch milliseconds.
    pub at_ms: i64,
    /// The account that hit the limit.
    pub from: AccountRef,
    /// Where the session went; `from` again when it went nowhere.
    pub to: AccountRef,
    /// Which kind of limit.
    pub class: LimitClass,
    /// `all`, or a model family.
    pub scope: String,
    /// When the limit lifts, epoch milliseconds, when known.
    pub until_ms: Option<i64>,
    /// How many background tasks the switch cut short.
    pub interrupted_tasks: u32,
    /// How it came out.
    pub outcome: RotationOutcome,
    /// What set it off.
    pub trigger: RotationTrigger,
    /// For a move ahead of a limit, the near window's used percent.
    pub used_percent: Option<f64>,
}

/// Where [`RotationLog::note_hand_binding`] keeps its marks, in `kv`.
const HAND_BINDING_SCOPE: &str = "rotation.hand_binding";

/// The rotation decisions, in this instance's database.
///
/// Per instance for the reason the rests are: the process that was moved is
/// this instance's, and so is the session's view of it.
pub struct RotationLog {
    db: Arc<Db>,
}

impl RotationLog {
    /// The log over an open instance database.
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }

    /// Write one decision down, keeping the newest [`LOG_PER_SESSION`] for its
    /// session. Its id, for [`RotationLog::set_interrupted`].
    pub fn record(&self, decision: &Decision) -> rusqlite::Result<i64> {
        self.db.write(|tx| {
            tx.execute(
                "INSERT INTO session_rotations
                     (session_id, at_ms, from_account, from_label, to_account, to_label,
                      class, scope, until_ms, interrupted_tasks, outcome, trigger_kind, used_percent)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                rusqlite::params![
                    decision.session_id,
                    decision.at_ms,
                    decision.from.id,
                    decision.from.label,
                    decision.to.id,
                    decision.to.label,
                    decision.class.as_str(),
                    decision.scope,
                    decision.until_ms,
                    decision.interrupted_tasks,
                    decision.outcome.as_str(),
                    decision.trigger.as_str(),
                    decision.used_percent,
                ],
            )?;
            let id = tx.last_insert_rowid();
            tx.execute(
                "DELETE FROM session_rotations WHERE session_id = ?1 AND id <= (
                     SELECT id FROM session_rotations WHERE session_id = ?1 ORDER BY id DESC LIMIT 1 OFFSET ?2
                 )",
                rusqlite::params![decision.session_id, LOG_PER_SESSION],
            )?;
            Ok(id)
        })
    }

    /// Correct how many tasks a switch cut short, once it has actually happened:
    /// a switch that waits for background work only knows at the respawn.
    pub fn set_interrupted(&self, id: i64, interrupted: u32) -> rusqlite::Result<()> {
        self.db.write(|tx| {
            tx.execute("UPDATE session_rotations SET interrupted_tasks = ?2 WHERE id = ?1", (id, interrupted))?;
            Ok(())
        })
    }

    /// A session's newest decision, as the API reports it.
    pub fn latest(&self, session_id: &str) -> Option<SessionRotation> {
        self.history(session_id, 1).pop()
    }

    /// A session's newest `limit` decisions, newest first.
    pub fn history(&self, session_id: &str, limit: usize) -> Vec<SessionRotation> {
        let read = self.db.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT at_ms, from_account, from_label, to_account, to_label, class, scope, until_ms, interrupted_tasks, outcome,
                        trigger_kind, used_percent
                   FROM session_rotations
                  WHERE session_id = ?1
                  ORDER BY id DESC
                  LIMIT ?2",
            )?;
            let rows = stmt.query_map((session_id, limit as i64), |row| {
                Ok(SessionRotation {
                    at: iso_ms(row.get(0)?),
                    from: AccountRef {
                        id: row.get(1)?,
                        label: row.get(2)?,
                    },
                    to: AccountRef {
                        id: row.get(3)?,
                        label: row.get(4)?,
                    },
                    class: LimitClass::parse(&row.get::<_, String>(5)?),
                    scope: row.get(6)?,
                    until_at: Nullable(row.get::<_, Option<i64>>(7)?.map(iso_ms)),
                    interrupted_tasks: row.get::<_, i64>(8)?.clamp(0, u32::MAX as i64) as u32,
                    outcome: RotationOutcome::parse(&row.get::<_, String>(9)?),
                    trigger: RotationTrigger::parse(row.get::<_, Option<String>>(10)?.as_deref()),
                    used_percent: row.get::<_, Option<f64>>(11)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        });
        read.unwrap_or_else(|error| {
            tracing::warn!(database = %self.db.path().display(), %error, "cannot read the rotation log");
            Vec::new()
        })
    }

    /// When this session last moved ahead of a limit, epoch milliseconds: the
    /// newest `ahead` decision that switched it, while the log still holds it.
    pub fn last_ahead_ms(&self, session_id: &str) -> Option<i64> {
        let read = self.db.read(|conn| {
            conn.query_row(
                "SELECT max(at_ms) FROM session_rotations
                  WHERE session_id = ?1 AND trigger_kind = 'ahead' AND outcome = 'switched'",
                (session_id,),
                |row| row.get::<_, Option<i64>>(0),
            )
        });
        read.unwrap_or_else(|error| {
            tracing::warn!(database = %self.db.path().display(), %error, "cannot read the rotation log");
            None
        })
    }

    /// Write down that somebody bound `session_id` to an account by hand at
    /// `at_ms` — the binding route, or a message naming an account other than
    /// the bound one. A rotation's own rebinding is not one of these.
    ///
    /// A `kv` row in this instance's database rather than a column on the
    /// binding: the binding table lives in the configuration database, and
    /// this is only ever asked by this instance's drivers, beside its log.
    pub fn note_hand_binding(&self, session_id: &str, at_ms: i64) {
        let written = self.db.write(|tx| crate::db::kv_set_in(tx, HAND_BINDING_SCOPE, session_id, &at_ms.to_string()));
        if let Err(error) = written {
            tracing::warn!(session = %session_id, %error, "cannot record a binding made by hand");
        }
    }

    /// When somebody last bound `session_id` to an account by hand, epoch
    /// milliseconds; see [`RotationLog::note_hand_binding`].
    pub fn bound_by_hand_ms(&self, session_id: &str) -> Option<i64> {
        self.db.kv_get(HAND_BINDING_SCOPE, session_id).and_then(|raw| raw.parse::<i64>().ok())
    }

    /// Whether a decision with this id is still in the log. Tests only.
    #[cfg(test)]
    fn contains(&self, id: i64) -> bool {
        use rusqlite::OptionalExtension;
        self.db
            .read(|conn| {
                conn.query_row("SELECT id FROM session_rotations WHERE id = ?1", (id,), |row| row.get::<_, i64>(0))
                    .optional()
            })
            .ok()
            .flatten()
            .is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::store::AccountKind;
    use crate::openapi_ext::Optional;
    use chrono::FixedOffset;

    const NOW: i64 = 1_790_000_000_000;
    const MINUTE: i64 = 60_000;

    fn window(label: &str, used: f64, read_ago_min: i64) -> Window {
        Window {
            label: label.to_string(),
            used_percent: used,
            resets_at_ms: Some(NOW + 60 * MINUTE),
            read_at_ms: Some(NOW - read_ago_min * MINUTE),
        }
    }

    fn account(id: &str, windows: Vec<Window>) -> PoolAccount {
        PoolAccount {
            id: id.to_string(),
            label: format!("{id}@example"),
            in_pool: true,
            resting: Vec::new(),
            windows,
        }
    }

    fn fresh(id: &str, five_hour: f64, weekly: f64) -> PoolAccount {
        account(id, vec![window("5h", five_hour, 2), window("weekly", weekly, 2)])
    }

    fn public(provider: AccountProvider, kind: AccountKind, rotation: bool) -> Account {
        Account {
            id: "11111111-2222-3333-4444-555555555555".into(),
            label: "a".into(),
            kind,
            email: Optional(None),
            usage_enabled: false,
            created_at: String::new(),
            updated_at: String::new(),
            unverified: Optional(None),
            provider,
            env_keys: Optional(None),
            rest: Vec::new(),
            rotation,
        }
    }

    #[test]
    fn model_ids_and_aliases_name_their_family() {
        assert_eq!(model_family(Some("claude-fable-5-1")), Some("fable"));
        assert_eq!(model_family(Some("fable")), Some("fable"));
        assert_eq!(model_family(Some("opus[1m]")), Some("opus"));
        assert_eq!(model_family(Some("claude-sonnet-4-5-20250929")), Some("sonnet"));
        assert_eq!(model_family(Some("Claude-Haiku-4-5")), Some("haiku"));
        assert_eq!(model_family(Some("glm-4.6")), None, "a gateway's model is nobody's family");
        assert_eq!(model_family(None), None);
    }

    /// The trigger, class by class: the long windows and the dead credential
    /// move a session, a request-rate blip and an unclassified failure do not.
    #[test]
    fn only_limits_that_outlast_a_respawn_trigger_a_rotation() {
        let pooled = public(AccountProvider::Claude, AccountKind::OauthToken, true);
        for class in [LimitClass::Session, LimitClass::Weekly, LimitClass::Credit, LimitClass::Auth] {
            assert!(triggers(class, Some(&pooled), false), "{class:?} moves the session");
        }
        for class in [LimitClass::Rate, LimitClass::Other] {
            assert!(!triggers(class, Some(&pooled), false), "{class:?} does not");
            assert!(!triggers(class, Some(&pooled), true), "{class:?} does not, even rebound by hand");
        }
        assert!(!triggers(LimitClass::Session, None, true), "a session on no account never moves");
        let outside = public(AccountProvider::Claude, AccountKind::Env, false);
        assert!(
            !triggers(LimitClass::Session, Some(&outside), false),
            "nor one on an account outside the pool"
        );
        assert!(
            triggers(LimitClass::Session, Some(&outside), true),
            "…unless somebody already bound the session elsewhere, and the move is theirs"
        );
        let codex = public(AccountProvider::Codex, AccountKind::Env, true);
        assert!(!triggers(LimitClass::Session, Some(&codex), true), "nor a codex one, ever");
    }

    #[test]
    fn a_whole_account_limit_switches_now_and_a_family_limit_waits() {
        assert_eq!(wait_policy("all"), Wait::Now);
        assert_eq!(wait_policy("fable"), Wait::ForBackground);
        assert_eq!(wait_policy("opus"), Wait::ForBackground);
    }

    #[test]
    fn the_cap_stops_a_chain_after_three_switches() {
        let some = vec!["b".to_string(), "c".to_string()];
        assert_eq!(plan(0, &some), Plan::Switch("b".into()));
        assert_eq!(plan(2, &some), Plan::Switch("b".into()), "the third switch still happens");
        assert_eq!(plan(3, &some), Plan::CapReached, "a fourth does not, however many accounts are free");
        assert_eq!(plan(0, &[]), Plan::NoCandidate);
        assert_eq!(plan(3, &[]), Plan::CapReached, "the cap is the better explanation");
    }

    /// The ordering, all five groups at once.
    #[test]
    fn candidates_come_fresh_by_headroom_then_unread_then_spent() {
        let mut resting_all = fresh("resting-all", 1.0, 1.0);
        resting_all.resting = vec!["all".into()];
        let mut resting_fable = fresh("resting-fable", 1.0, 1.0);
        resting_fable.resting = vec!["fable".into()];
        let mut resting_opus = fresh("resting-opus", 50.0, 50.0);
        resting_opus.resting = vec!["opus".into()];
        let mut outside = fresh("outside", 0.0, 0.0);
        outside.in_pool = false;
        let pool = vec![
            fresh("current", 0.0, 0.0),
            account("stale", vec![window("5h", 10.0, 45), window("weekly", 10.0, 45)]),
            fresh("tight", 20.0, 90.0),
            resting_all,
            fresh("spent", 98.0, 10.0),
            account("unread", Vec::new()),
            resting_fable,
            fresh("roomy", 30.0, 40.0),
            outside,
            resting_opus,
        ];
        assert_eq!(
            candidates(&pool, "current", Some("fable"), NOW),
            vec!["roomy", "resting-opus", "tight", "stale", "unread", "spent"],
            "roomy has 60 % left in its tightest window, resting-opus 50 %, tight 10 %; \
             an opus rest does not stop a fable session; stale and unread keep list order; spent goes last"
        );
        assert_eq!(
            candidates(&pool, "current", Some("opus"), NOW),
            vec!["resting-fable", "roomy", "tight", "stale", "unread", "spent"],
            "the same pool for an opus session: now the fable rest is the one that does not count, \
             and with 99 % left that account leads"
        );
    }

    /// The family's own weekly window counts when there is one, and a window
    /// whose reset has passed is empty however full it was read.
    #[test]
    fn the_familys_window_counts_and_a_reset_window_is_empty() {
        let mut fable_spent = fresh("fable-spent", 10.0, 10.0);
        fable_spent.windows.push(window("weekly (Fable)", 99.0, 2));
        let mut reset = fresh("reset", 10.0, 10.0);
        reset.windows[0] = Window {
            resets_at_ms: Some(NOW - MINUTE),
            ..window("5h", 100.0, 2)
        };
        let pool = vec![fable_spent, reset];
        assert_eq!(candidates(&pool, "x", Some("fable"), NOW), vec!["reset", "fable-spent"]);
        assert_eq!(
            candidates(&pool, "x", Some("sonnet"), NOW),
            vec!["fable-spent", "reset"],
            "the fable window is nothing to a sonnet session"
        );
    }

    /// A family nobody can name is kept off every family's rest, and a
    /// whole-account rest stays what it was.
    #[test]
    fn an_unknown_family_is_kept_off_any_family_rest() {
        assert!(rest_covers("fable", None));
        assert!(rest_covers("all", None));
        assert!(rest_covers("fable", Some("fable")));
        assert!(!rest_covers("fable", Some("opus")));

        let mut fable = fresh("fable-resting", 1.0, 1.0);
        fable.resting = vec!["fable".into()];
        let pool = vec![fable, fresh("free", 50.0, 50.0)];
        assert_eq!(
            candidates(&pool, "x", None, NOW),
            vec!["free"],
            "an unknown model might be the resting one"
        );
        assert_eq!(candidates(&pool, "x", Some("opus"), NOW), vec!["fable-resting", "free"]);
    }

    /// A binding set by hand while the process was still on the old account
    /// is where the session goes — unless that account is resting too.
    #[test]
    fn a_manual_binding_wins_unless_it_is_resting_too() {
        let mut spent = fresh("bound-resting", 1.0, 1.0);
        spent.resting = vec!["all".into()];
        let pool = vec![fresh("first", 1.0, 1.0), fresh("bound", 80.0, 80.0), spent];
        assert_eq!(
            choose(&pool, "from", Some("bound"), None, 0, NOW),
            Plan::Switch("bound".into()),
            "no search: the person already chose"
        );
        assert_eq!(choose(&pool, "from", Some("bound-resting"), None, 0, NOW), Plan::Switch("first".into()));
        assert_eq!(
            choose(&pool, "from", Some("from"), None, 0, NOW),
            Plan::Switch("first".into()),
            "not rebound"
        );
        assert_eq!(
            choose(&pool, "from", Some("bound"), None, 3, NOW),
            Plan::CapReached,
            "and the cap still holds"
        );
    }

    #[test]
    fn a_resting_default_is_swapped_for_the_first_free_account() {
        let mut default = fresh("default", 100.0, 10.0);
        default.resting = vec!["all".into()];
        let pool = vec![default.clone(), fresh("b", 10.0, 10.0)];
        assert_eq!(default_for_new_session("default", &pool, Some("fable"), None, NOW), "b");

        let mut fable_only = default.clone();
        fable_only.resting = vec!["fable".into()];
        let pool = vec![fable_only, fresh("b", 10.0, 10.0)];
        assert_eq!(default_for_new_session("default", &pool, Some("fable"), None, NOW), "b");
        assert_eq!(
            default_for_new_session("default", &pool, Some("opus"), None, NOW),
            "default",
            "a fable rest is no reason to move an opus session"
        );

        let alone = vec![default.clone()];
        assert_eq!(
            default_for_new_session("default", &alone, None, None, NOW),
            "default",
            "nowhere to go: stay"
        );

        let mut outside = default;
        outside.in_pool = false;
        let pool = vec![outside, fresh("b", 10.0, 10.0)];
        assert_eq!(
            default_for_new_session("default", &pool, None, None, NOW),
            "default",
            "a default outside the pool is the user's to change"
        );
    }

    /// The same rule against a real store: the default resting under the
    /// five-hour window sends a new session to the next pool account, a
    /// family rest only a session of that family, and nothing else moves.
    #[tokio::test]
    async fn a_new_session_passes_over_a_resting_default() {
        use crate::accounts::limits::Verdict;
        use crate::accounts::rest::{now_ms, Observation};
        use crate::accounts::store::Credential;
        use crate::sessions::Paths;

        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Arc::new(Paths::new(dir.path().to_path_buf()));
        let store = AccountsStore::new(paths.clone());
        let quota = QuotaCache::new(paths);
        let create = |label: &'static str| {
            let store = &store;
            async move {
                store
                    .create(label, AccountProvider::Claude, Credential::Oauth(format!("sk-ant-oat01-test-{label}")))
                    .await
                    .expect("account")
            }
        };
        let default = create("default").await;
        let other = create("other").await;
        assert_eq!(
            default_for(&store, &quota, &default.id, Some("fable")).await,
            default.id,
            "not resting: stays"
        );

        let rest = |scope: &str| Observation {
            account_id: default.id.clone(),
            session_id: Some("s".into()),
            source: "claude:result",
            model: None,
            verdict: Verdict {
                class: if scope == "all" { LimitClass::Session } else { LimitClass::Weekly },
                scope: scope.to_string(),
                until_ms: Some(now_ms() + 60 * MINUTE),
            },
            raw: "You have hit your limit".into(),
            reason: "You have hit your limit".into(),
            may_rest: true,
        };
        store.limit_log().record(&rest("fable"), now_ms()).expect("rest");
        assert_eq!(default_for(&store, &quota, &default.id, Some("claude-fable-5-1")).await, other.id);
        assert_eq!(default_for(&store, &quota, &default.id, Some("opus")).await, default.id);
        assert_eq!(
            default_for(&store, &quota, &default.id, None).await,
            other.id,
            "a session that names no model might be a fable one, so it is kept off the fable rest"
        );

        store.limit_log().record(&rest("all"), now_ms()).expect("rest");
        assert_eq!(default_for(&store, &quota, &default.id, None).await, other.id);
    }

    fn local(state: super::super::local_login::LocalLoginState, is_default: bool) -> super::super::local_login::LocalLogin {
        super::super::local_login::LocalLogin {
            state,
            rest: Vec::new(),
            is_default,
        }
    }

    /// The order of the rules, pinned without a store: a configured default
    /// first (moved off a rest, for claude only), then a chosen local login,
    /// then a local login nothing condemns; signed out, the first free account
    /// *in the pool*, and with none the local login after all.
    #[test]
    fn a_start_nobody_pointed_takes_a_pool_account_only_for_a_signed_out_local_login() {
        use super::super::local_login::LocalLoginState::{SignedIn, SignedOut, Unknown};
        let start = |provider, default: Option<&str>, login, pool: &[PoolAccount], family| {
            let picked = start_choice(provider, default, &login, pool, family, None, NOW);
            (picked.account_id.0, picked.reason)
        };
        let gateway = PoolAccount {
            in_pool: false,
            ..account("gateway", Vec::new())
        };
        let resting = PoolAccount {
            resting: vec!["all".into()],
            ..account("resting", Vec::new())
        };
        let opus_spent = PoolAccount {
            resting: vec!["opus".into()],
            ..account("opus-spent", Vec::new())
        };
        let spare = account("spare", Vec::new());
        let pool = [gateway.clone(), resting.clone(), opus_spent.clone(), spare.clone()];
        let claude = AccountProvider::Claude;

        assert_eq!(start(claude, None, local(Unknown, false), &pool, None), (None, StartReason::Local));
        assert_eq!(start(claude, None, local(SignedIn, false), &pool, None), (None, StartReason::Local));
        assert_eq!(
            start(claude, None, local(SignedOut, false), &pool, Some("sonnet")),
            (Some("opus-spent".into()), StartReason::Fallback),
            "the gateway first in the list is out of the pool, and an account resting on everything is passed over"
        );
        assert_eq!(
            start(claude, None, local(SignedOut, false), &pool, Some("opus")),
            (Some("spare".into()), StartReason::Fallback),
            "a family rest keeps a session of that family off it"
        );
        assert_eq!(
            start(claude, None, local(SignedOut, true), &pool, None),
            (None, StartReason::LocalChosen),
            "a local login somebody chose stays chosen, signed out or not"
        );
        assert_eq!(
            start(claude, None, local(SignedOut, false), &[gateway.clone(), resting.clone()], None),
            (None, StartReason::LocalSignedOut),
            "nothing free in the pool: the local login and its sign-in error, never the gateway"
        );
        assert_eq!(
            start(claude, None, local(SignedOut, false), &[], None),
            (None, StartReason::LocalSignedOut)
        );

        let mut resting_default = resting.clone();
        resting_default.id = "default".into();
        let with_default = [resting_default, spare.clone()];
        assert_eq!(
            start(claude, Some("default"), local(SignedOut, false), &with_default, None),
            (Some("spare".into()), StartReason::Rotated),
            "a resting default is moved as it always was"
        );
        assert_eq!(
            start(AccountProvider::Codex, Some("default"), local(SignedOut, false), &with_default, None),
            (Some("default".into()), StartReason::Default),
            "a codex default never moves"
        );
        assert_eq!(
            start(claude, Some("gateway"), local(SignedOut, false), &pool, None),
            (Some("gateway".into()), StartReason::Default),
            "a default is the user's choice, in the pool or not"
        );
    }

    /// A start that lets the answer choose its model: the login decides the
    /// model (the head of its own list) and the model's family can decide the
    /// login. Asked one at a time they chase each other; together they settle
    /// on a login and one of its own models that the rule places there.
    #[test]
    fn a_start_choosing_its_model_settles_on_a_login_and_a_model_placed_on_it() {
        use super::super::local_login::LocalLoginState::{SignedIn, SignedOut};
        let ids = |models: &[&str]| models.iter().map(|model| model.to_string()).collect::<Vec<_>>();
        // D, the default, was probed and its list leads with opus — on which
        // D is resting. E, in the pool, was not probed: the static list, fable first.
        let d_models = ids(&["opus[1m]", "fable", "sonnet"]);
        let e_models = ids(&["fable", "opus", "sonnet", "haiku"]);
        let inputs = StartInputs {
            provider: AccountProvider::Claude,
            default: Some("d".into()),
            local: local(SignedIn, false),
            pool: vec![
                PoolAccount {
                    resting: vec!["opus".into()],
                    ..account("d", Vec::new())
                },
                account("e", Vec::new()),
            ],
            ahead: None,
            now_ms: NOW,
        };
        let on = |start: AccountStart| start.account_id.0;
        assert_eq!(on(inputs.place(Some("opus[1m]"))).as_deref(), Some("e"), "one at a time: D's head goes to E");
        assert_eq!(on(inputs.place(Some("fable"))).as_deref(), Some("d"), "and E's head comes back to D");
        assert!(inputs.model_matters());
        assert_eq!(inputs.start_order(), vec![Some("d".to_string()), Some("e".to_string()), None]);
        assert_eq!(
            inputs.own_model(Some("d"), &d_models),
            Some("fable"),
            "together: D, with the first of its own models D can still run"
        );
        assert_eq!(inputs.own_model(Some("e"), &e_models), Some("opus"), "E only takes what D cannot");

        // No default, the local login signed out: E rests on opus, F is free.
        // A model-less start goes to F; a fable start to E (listed first).
        let signed_out = StartInputs {
            default: None,
            local: local(SignedOut, false),
            pool: vec![
                PoolAccount {
                    resting: vec!["opus".into()],
                    ..account("e", Vec::new())
                },
                account("f", Vec::new()),
            ],
            ..inputs.clone()
        };
        assert!(signed_out.model_matters());
        assert_eq!(signed_out.start_order(), vec![Some("f".to_string()), Some("e".to_string()), None]);
        assert_eq!(
            signed_out.own_model(Some("f"), &e_models),
            Some("opus"),
            "F's head, fable, would land on E: F with opus, which E cannot take"
        );
        assert_eq!(signed_out.own_model(None, &e_models), None, "a stand-in is free: nothing lands on the local login");

        // The same, as the endpoint asks it: every login's list in start order.
        let lists = vec![(Some("d".to_string()), d_models.clone()), (Some("e".to_string()), e_models.clone()), (None, e_models.clone())];
        let chosen = inputs.choose(&lists);
        assert_eq!(
            (chosen.account_id.0.as_deref(), chosen.model.0.as_deref(), chosen.placed),
            (Some("d"), Some("fable"), true)
        );
        // Lists that leave no login a model of its own it lands on: D's (a
        // restricted plan, probed) holds only opus, on which D rests; E's only
        // fable, which lands back on D. The login a model-less start gets (E),
        // with the head of its list — `placed: false`, so a start names it.
        let lists = vec![
            (Some("d".to_string()), ids(&["opus[1m]", "opus"])),
            (Some("e".to_string()), ids(&["fable"])),
            (None, ids(&["fable"])),
        ];
        let stranded = inputs.choose(&lists);
        assert_eq!(
            (stranded.account_id.0.as_deref(), stranded.model.0.as_deref(), stranded.placed),
            (Some("e"), Some("fable"), false)
        );
        assert!(inputs.still_names("e"), "and E is the rule's choice for it while nothing changes");
        assert!(!inputs.still_names("d"));

        // Nothing family-dependent: any model lands on the same login.
        let plain = StartInputs {
            pool: vec![account("d", Vec::new()), account("e", Vec::new())],
            ..inputs.clone()
        };
        assert!(!plain.model_matters());
        let codex = StartInputs {
            provider: AccountProvider::Codex,
            ..inputs.clone()
        };
        assert!(!codex.model_matters(), "codex models name no family");
    }

    /// The same against a real store, both CLIs: a local login signed out by a
    /// refused credential, accounts in and out of the pool, a rest, an
    /// explicitly chosen local login and a configured default.
    #[tokio::test]
    async fn a_start_against_the_store_skips_a_signed_out_local_login_only_for_the_pool() {
        use crate::accounts::limits::Verdict;
        use crate::accounts::rest::{now_ms, Observation, LOCAL_CLAUDE, LOCAL_CODEX};
        use crate::accounts::store::{parse_env_block, AccountPatch, Credential};
        use crate::sessions::Paths;

        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Arc::new(Paths::new(dir.path().to_path_buf()));
        let store = AccountsStore::new(paths.clone());
        let quota = QuotaCache::new(paths);
        let start = |provider: AccountProvider, model: Option<&'static str>| {
            let (store, quota) = (&store, &quota);
            async move {
                let picked = start_for(store, quota, provider, model).await;
                (picked.account_id.0, picked.reason)
            }
        };
        let refused = |account: &str, class: LimitClass, until_ms: Option<i64>| Observation {
            account_id: account.to_string(),
            session_id: Some("s1".into()),
            source: "claude:result",
            model: None,
            verdict: Verdict {
                class,
                scope: "all".into(),
                until_ms,
            },
            raw: "Failed to authenticate: OAuth session expired and could not be refreshed".into(),
            reason: String::new(),
            may_rest: true,
        };

        assert_eq!(start(AccountProvider::Claude, None).await, (None, StartReason::Local), "nothing stored");

        // First in the list: an env block behind a gateway, out of the pool as
        // every env account is unless somebody puts it in.
        let gateway = store
            .create(
                "Gateway",
                AccountProvider::Claude,
                Credential::Env(parse_env_block("ANTHROPIC_AUTH_TOKEN=t\nANTHROPIC_BASE_URL=https://relay.example").expect("block")),
            )
            .await
            .expect("gateway");
        assert!(!gateway.rotation, "an env account is out of the pool by default");
        assert_eq!(
            start(AccountProvider::Claude, None).await,
            (None, StartReason::Local),
            "a local login nobody knows anything about is still where a start goes"
        );

        store
            .limit_log()
            .record(&refused(LOCAL_CLAUDE, LimitClass::Auth, None), now_ms())
            .expect("local refused");
        assert_eq!(
            start(AccountProvider::Claude, None).await,
            (None, StartReason::LocalSignedOut),
            "signed out with only the gateway stored: the local login and its error, never the gateway"
        );

        let work = store
            .create("Work", AccountProvider::Claude, Credential::Oauth("sk-ant-token-work".into()))
            .await
            .expect("work");
        let spare = store
            .create("Spare", AccountProvider::Claude, Credential::Oauth("sk-ant-token-spare".into()))
            .await
            .expect("spare");
        assert_eq!(
            start(AccountProvider::Claude, None).await,
            (Some(work.id.clone()), StartReason::Fallback),
            "the first account in the pool, past the gateway listed before it"
        );

        store
            .limit_log()
            .record(&refused(&work.id, LimitClass::Session, Some(now_ms() + 3_600_000)), now_ms())
            .expect("work resting");
        assert_eq!(
            start(AccountProvider::Claude, None).await,
            (Some(spare.id.clone()), StartReason::Fallback),
            "an account resting on everything is passed over"
        );
        store
            .update(
                &spare.id,
                AccountPatch {
                    rotation: Some(false),
                    ..AccountPatch::default()
                },
            )
            .await
            .expect("spare out of the pool");
        assert_eq!(
            start(AccountProvider::Claude, None).await,
            (None, StartReason::LocalSignedOut),
            "every pool account resting, the rest out of it: nothing stands in"
        );

        // Codex: its own local login and its own accounts, out of the pool by default.
        assert_eq!(start(AccountProvider::Codex, None).await, (None, StartReason::Local));
        let codex = store
            .create(
                "Codex",
                AccountProvider::Codex,
                Credential::Env(parse_env_block("OPENAI_API_KEY=sk-codex-one").expect("block")),
            )
            .await
            .expect("codex");
        store
            .limit_log()
            .record(&refused(LOCAL_CODEX, LimitClass::Auth, None), now_ms())
            .expect("codex local refused");
        assert_eq!(
            start(AccountProvider::Codex, None).await,
            (None, StartReason::LocalSignedOut),
            "a codex account is out of the pool unless somebody puts it in"
        );
        store
            .update(
                &codex.id,
                AccountPatch {
                    rotation: Some(true),
                    ..AccountPatch::default()
                },
            )
            .await
            .expect("codex into the pool");
        assert_eq!(start(AccountProvider::Codex, None).await, (Some(codex.id.clone()), StartReason::Fallback));
        assert_eq!(
            start(AccountProvider::Claude, None).await,
            (None, StartReason::LocalSignedOut),
            "each CLI stands in only with its own accounts"
        );

        // The local login chosen in Settings stays chosen, signed out or not.
        store.set_default(None, AccountProvider::Codex).await.expect("local codex");
        assert_eq!(start(AccountProvider::Codex, None).await, (None, StartReason::LocalChosen));

        // A configured default is the user's and always wins — even one out of the pool.
        store.set_default(Some(&gateway.id), AccountProvider::Claude).await.expect("default");
        assert_eq!(start(AccountProvider::Claude, None).await, (Some(gateway.id.clone()), StartReason::Default));
    }

    fn tokyo(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(9 * 3600)
            .unwrap()
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn a_reset_time_reads_as_a_clock_a_tomorrow_or_a_date() {
        let now = tokyo(2026, 9, 29, 18, 0);
        let at = |d, h, mi| Some(tokyo(2026, 9, d, h, mi).timestamp_millis());
        assert_eq!(until_text(at(29, 23, 10), &now).as_deref(), Some("23:10"));
        assert_eq!(until_text(at(30, 7, 0), &now).as_deref(), Some("明天 07:00"));
        assert_eq!(
            until_text(Some(tokyo(2026, 10, 3, 7, 0).timestamp_millis()), &now).as_deref(),
            Some("10月3日 07:00")
        );
        assert_eq!(until_text(None, &now), None);
    }

    #[test]
    fn the_continuation_says_what_happened_and_asks_to_carry_on() {
        let text = continuation_prompt("ada@work", "ada@home", &reason(LimitClass::Session, "all"), Some("23:10"), &[]);
        assert_eq!(
            text,
            "[Droppen] 这个会话已从账号 ada@work 自动切换到 ada@home：ada@work 的 5 小时额度用完，23:10 恢复。上一条消息因此没有完成，请从中断处继续。"
        );

        let text = continuation_prompt("a", "b", &reason(LimitClass::Weekly, "fable"), None, &[]);
        assert_eq!(
            text, "[Droppen] 这个会话已从账号 a 自动切换到 b：a 的 Fable 每周额度用完。上一条消息因此没有完成，请从中断处继续。",
            "no reset time, no clause"
        );
        let text = continuation_prompt("a", "b", &reason(LimitClass::Auth, "all"), None, &[]);
        assert!(text.contains("a 的登录凭据失效。"), "{text}");

        // And when nothing moved, but the account's own limit lifted.
        assert_eq!(
            resumed_prompt("ada@work"),
            "[Droppen] 账号 ada@work 的额度已恢复。上一条消息因额度用完没有完成，请从中断处继续。"
        );
    }

    #[test]
    fn interrupted_tasks_are_listed_one_per_line_with_their_output() {
        let tasks = [
            Interrupted {
                description: "跑完整测试".into(),
                output: Some("/tmp/claude-501/p/s/tasks/b1.output".into()),
            },
            Interrupted {
                description: "Review the diff".into(),
                output: None,
            },
        ];
        let text = continuation_prompt("a", "b", &reason(LimitClass::Session, "all"), Some("23:10"), &tasks);
        let (head, tail) = text.split_once("\n\n").expect("the task list is its own paragraph");
        assert!(head.ends_with("请从中断处继续。"), "{head}");
        assert_eq!(
            tail,
            "以下后台子任务被中断，请检查它们的输出后按需重新派发：\n- 跑完整测试（输出：/tmp/claude-501/p/s/tasks/b1.output）\n- Review the diff"
        );
    }

    #[test]
    fn the_notification_names_both_accounts_or_says_there_was_nowhere_to_go() {
        let why = reason(LimitClass::Session, "all");
        assert_eq!(
            notice(RotationOutcome::Switched, "ada@work", "ada@home", &why, Some("23:10")),
            "已从 ada@work 切到 ada@home（5 小时额度用完，23:10 恢复）"
        );
        assert_eq!(
            notice(RotationOutcome::NoCandidate, "ada@work", "ada@work", &why, Some("23:10")),
            "ada@work 的 5 小时额度用完，没有可切换的账号（23:10 恢复）"
        );
        assert_eq!(
            notice(RotationOutcome::CapReached, "c", "c", &reason(LimitClass::Credit, "all"), None),
            "c 的余额不足，已连续自动切换 3 次，不再切换"
        );
    }

    fn decision(session: &str, n: i64) -> Decision {
        Decision {
            session_id: session.to_string(),
            at_ms: NOW + n,
            from: AccountRef {
                id: "a".into(),
                label: "A".into(),
            },
            to: AccountRef {
                id: "b".into(),
                label: "B".into(),
            },
            class: LimitClass::Session,
            scope: "all".into(),
            until_ms: Some(NOW + 60 * MINUTE),
            interrupted_tasks: 0,
            outcome: RotationOutcome::Switched,
            trigger: RotationTrigger::Limit,
            used_percent: None,
        }
    }

    #[test]
    fn the_log_reports_the_newest_decision_and_keeps_twenty_per_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = RotationLog::new(Arc::new(Db::open(dir.path().join("state.db"))));
        assert_eq!(log.latest("s1"), None);

        let first = log.record(&decision("s1", 0)).expect("record");
        for n in 1..25 {
            log.record(&decision("s1", n)).expect("record");
        }
        let other = log.record(&decision("s2", 0)).expect("record");
        assert_eq!(log.history("s1", 100).len(), LOG_PER_SESSION as usize);
        assert!(!log.contains(first), "the oldest went first");
        assert!(log.contains(other), "and another session's are its own");

        let newest = log.record(&Decision {
            outcome: RotationOutcome::NoCandidate,
            to: AccountRef {
                id: "a".into(),
                label: "A".into(),
            },
            ..decision("s1", 99)
        });
        let newest = newest.expect("record");
        log.set_interrupted(newest, 2).expect("update");
        let latest = log.latest("s1").expect("a decision");
        assert_eq!(latest.outcome, RotationOutcome::NoCandidate);
        assert_eq!(latest.interrupted_tasks, 2);
        assert_eq!(latest.until_at.0.as_deref(), Some(iso_ms(NOW + 60 * MINUTE).as_str()));
        assert_eq!(latest.to.id, "a", "a decision that moved nothing names the account it stayed on");
    }

    // ------------------------------------------------------ ahead of a limit

    const AHEAD: f64 = AHEAD_DEFAULT_PERCENT;

    /// A window read `read_ago_min` minutes ago that resets `resets_in_min`
    /// minutes from now.
    fn timed(label: &str, used: f64, read_ago_min: i64, resets_in_min: i64) -> Window {
        Window {
            resets_at_ms: Some(NOW + resets_in_min * MINUTE),
            ..window(label, used, read_ago_min)
        }
    }

    #[test]
    fn the_ahead_threshold_defaults_to_ninety_five_and_zero_switches_it_off() {
        assert_eq!(ahead_threshold(None), Some(95.0));
        assert_eq!(ahead_threshold(Some("")), Some(95.0), "empty is unset");
        assert_eq!(ahead_threshold(Some("ninety")), Some(95.0), "and so is nonsense");
        assert_eq!(ahead_threshold(Some(" 90 ")), Some(90.0));
        assert_eq!(ahead_threshold(Some("97.5")), Some(97.5));
        assert_eq!(ahead_threshold(Some("0")), None, "zero is off");
        assert_eq!(ahead_threshold(Some("-5")), None);
    }

    /// Near is a fresh reading of a relevant window, at or past the threshold,
    /// whose reset is still to come — every one of those, or nothing.
    #[test]
    fn a_window_is_near_only_when_fresh_relevant_full_and_not_yet_reset() {
        let five = |used: f64, read_ago: i64, resets_in: i64| vec![timed("5h", used, read_ago, resets_in)];
        let near = near_window(&five(96.0, 2, 60), None, AHEAD, NOW).expect("near");
        assert_eq!(
            near,
            Near {
                label: "5h".into(),
                class: LimitClass::Session,
                scope: "all".into(),
                used_percent: 96.0,
                resets_at_ms: Some(NOW + 60 * MINUTE),
            }
        );
        assert!(near_window(&five(95.0, 2, 60), None, AHEAD, NOW).is_some(), "at the threshold counts");
        assert!(near_window(&five(94.9, 2, 60), None, AHEAD, NOW).is_none(), "under it does not");
        assert!(
            near_window(&five(96.0, 14, 60), None, AHEAD, NOW).is_some(),
            "read fourteen minutes ago is fresh"
        );
        assert!(near_window(&five(96.0, 16, 60), None, AHEAD, NOW).is_none(), "sixteen is stale");
        assert!(
            near_window(&five(96.0, 2, -1), None, AHEAD, NOW).is_none(),
            "a window that has reset is not the one read"
        );
        let unknown_reset = vec![Window {
            resets_at_ms: None,
            ..window("5h", 99.0, 2)
        }];
        assert!(
            near_window(&unknown_reset, None, AHEAD, NOW).is_none(),
            "nor one whose reset nobody knows"
        );
        assert!(
            near_window(&five(96.0, 2, 60), None, 97.0, NOW).is_none(),
            "the threshold is the setting's"
        );

        // A family's own window speaks for that family only.
        let fable = vec![timed("5h", 10.0, 2, 60), timed("weekly (Fable)", 97.0, 2, 3000)];
        let near = near_window(&fable, Some("fable"), AHEAD, NOW).expect("a fable session is near");
        assert_eq!(
            (near.class, near.scope.as_str(), near.label.as_str()),
            (LimitClass::Weekly, "fable", "weekly (Fable)")
        );
        assert!(near_window(&fable, Some("opus"), AHEAD, NOW).is_none(), "an opus session is not");
        assert!(near_window(&fable, None, AHEAD, NOW).is_none(), "nor one whose family is unknown");
        let opus = vec![timed("weekly (Opus)", 99.0, 2, 3000)];
        assert_eq!(near_window(&opus, Some("opus"), AHEAD, NOW).map(|near| near.scope), Some("opus".into()));

        // Two near windows: the fuller one is the one that runs out.
        let both = vec![timed("5h", 96.0, 2, 60), timed("weekly", 99.0, 2, 3000)];
        let near = near_window(&both, None, AHEAD, NOW).expect("near");
        assert_eq!((near.class, near.scope.as_str(), near.used_percent), (LimitClass::Weekly, "all", 99.0));
    }

    fn running_out() -> PoolAccount {
        account("from", vec![timed("5h", 96.0, 2, 60), timed("weekly", 40.0, 2, 3000)])
    }

    /// A move nothing forced needs somewhere known to be well under the line:
    /// `threshold − 10` in every relevant window, on a reading recent enough
    /// to rank by. Close to the line, unread and stale are all "not known".
    #[test]
    fn a_move_ahead_needs_a_known_good_account_under_the_hysteresis() {
        let pool = vec![
            running_out(),
            fresh("close", 85.0, 10.0),
            account("stale", vec![window("5h", 1.0, 45), window("weekly", 1.0, 45)]),
            account("unread", Vec::new()),
        ];
        let guards = AheadGuards::default();
        let near = near_window(&pool[0].windows, None, AHEAD, NOW).expect("near");
        assert_eq!(
            plan_ahead(&pool, "from", None, AHEAD, guards, NOW),
            AheadPlan::NoCandidate(near.clone()),
            "85 % is not under 85, and nothing else is known"
        );

        let mut pool = pool;
        pool.push(fresh("just-under", 84.9, 84.9));
        pool.push(fresh("roomy", 30.0, 50.0));
        assert_eq!(
            plan_ahead(&pool, "from", None, AHEAD, guards, NOW),
            AheadPlan::Switch("roomy".into(), near.clone()),
            "the most headroom first"
        );
        pool.retain(|account| account.id != "roomy");
        assert_eq!(
            plan_ahead(&pool, "from", None, AHEAD, guards, NOW),
            AheadPlan::Switch("just-under".into(), near.clone())
        );

        // A family's own window counts against a candidate too.
        let mut fable_full = fresh("fable-full", 10.0, 10.0);
        fable_full.windows.push(window("weekly (Fable)", 90.0, 2));
        let pool = vec![running_out(), fable_full];
        assert!(matches!(
            plan_ahead(&pool, "from", Some("fable"), AHEAD, guards, NOW),
            AheadPlan::NoCandidate(_)
        ));
        assert!(matches!(plan_ahead(&pool, "from", Some("opus"), AHEAD, guards, NOW), AheadPlan::Switch(ref to, _) if to == "fable-full"));

        // Never off a pool account's own reading of nothing near, nor off an
        // account outside the pool.
        let calm = vec![fresh("from", 50.0, 50.0), fresh("roomy", 1.0, 1.0)];
        assert_eq!(plan_ahead(&calm, "from", None, AHEAD, guards, NOW), AheadPlan::Stay);
        let mut outside = running_out();
        outside.in_pool = false;
        let pool = vec![outside, fresh("roomy", 1.0, 1.0)];
        assert_eq!(plan_ahead(&pool, "from", None, AHEAD, guards, NOW), AheadPlan::Stay);
        // A resting candidate is no candidate, however empty its reading.
        let mut resting = fresh("resting", 1.0, 1.0);
        resting.resting = vec!["all".into()];
        let pool = vec![running_out(), resting];
        assert!(matches!(plan_ahead(&pool, "from", None, AHEAD, guards, NOW), AheadPlan::NoCandidate(_)));
    }

    /// Background work, a move in the last half hour and a binding somebody
    /// made by hand in the last half hour each put a due move off — and each
    /// stops counting once it is over.
    #[test]
    fn background_work_a_recent_move_and_a_hand_binding_each_defer_a_move_ahead() {
        let pool = vec![running_out(), fresh("roomy", 10.0, 10.0)];
        let plan = |guards: AheadGuards| plan_ahead(&pool, "from", None, AHEAD, guards, NOW);
        let deferred = |plan: AheadPlan| match plan {
            AheadPlan::Defer(_, why) => Some(why),
            _ => None,
        };
        assert!(matches!(plan(AheadGuards::default()), AheadPlan::Switch(ref to, _) if to == "roomy"));

        let busy = AheadGuards {
            background: true,
            ..AheadGuards::default()
        };
        assert_eq!(
            deferred(plan(busy)),
            Some(Deferral::Background),
            "never cut a subagent off for a move nothing forced"
        );

        let moved = |ago: i64| AheadGuards {
            last_ahead_ms: Some(NOW - ago * MINUTE),
            ..AheadGuards::default()
        };
        assert_eq!(deferred(plan(moved(10))), Some(Deferral::RecentlyMoved));
        assert_eq!(deferred(plan(moved(29))), Some(Deferral::RecentlyMoved));
        assert!(matches!(plan(moved(31)), AheadPlan::Switch(..)), "once in thirty minutes, not never");

        let chosen = |ago: i64| AheadGuards {
            bound_by_hand_ms: Some(NOW - ago * MINUTE),
            ..AheadGuards::default()
        };
        assert_eq!(deferred(plan(chosen(5))), Some(Deferral::BoundByHand), "somebody just chose this account");
        assert!(matches!(plan(chosen(31)), AheadPlan::Switch(..)));
    }

    /// A new session is not started on a default that is about to run out
    /// when a known-good account is there to take it — and only then.
    #[test]
    fn a_default_about_to_run_out_is_swapped_for_a_known_good_account() {
        let default = account("default", vec![timed("5h", 97.0, 2, 60), timed("weekly", 40.0, 2, 3000)]);
        let pool = vec![default.clone(), fresh("b", 10.0, 10.0)];
        assert_eq!(default_for_new_session("default", &pool, None, Some(AHEAD), NOW), "b");
        assert_eq!(default_for_new_session("default", &pool, None, None, NOW), "default", "switched off");
        assert_eq!(
            default_for_new_session("default", &pool, None, Some(98.0), NOW),
            "default",
            "under that line"
        );

        let unknown = vec![default.clone(), account("unread", Vec::new()), fresh("close", 90.0, 10.0)];
        assert_eq!(
            default_for_new_session("default", &unknown, None, Some(AHEAD), NOW),
            "default",
            "nothing known to be better: the default after all"
        );

        let fable = account(
            "default",
            vec![
                timed("5h", 10.0, 2, 60),
                timed("weekly", 10.0, 2, 3000),
                timed("weekly (Fable)", 99.0, 2, 3000),
            ],
        );
        let pool = vec![fable, fresh("b", 10.0, 10.0)];
        assert_eq!(default_for_new_session("default", &pool, Some("fable"), Some(AHEAD), NOW), "b");
        assert_eq!(default_for_new_session("default", &pool, Some("sonnet"), Some(AHEAD), NOW), "default");
    }

    #[test]
    fn the_ahead_notice_says_how_full_the_window_is_and_when_it_resets() {
        let near = |class, scope: &str, used| Near {
            label: String::new(),
            class,
            scope: scope.to_string(),
            used_percent: used,
            resets_at_ms: None,
        };
        assert_eq!(
            ahead_notice("A", "B", &near(LimitClass::Session, "all", 96.0), Some("23:10")),
            "已提前从 A 切换到 B（A 的 5 小时额度已用 96%，23:10 重置）"
        );
        assert_eq!(
            ahead_notice("ada@work", "ada@home", &near(LimitClass::Weekly, "all", 97.4), Some("明天 07:00")),
            "已提前从 ada@work 切换到 ada@home（ada@work 的每周额度已用 97%，明天 07:00 重置）"
        );
        assert_eq!(
            ahead_notice("A", "B", &near(LimitClass::Weekly, "fable", 95.6), None),
            "已提前从 A 切换到 B（A 的 Fable 每周额度已用 96%）",
            "no reset time, no clause"
        );
        assert_eq!(percent_text(100.4), "100");
    }

    /// The trigger and the used percent go into the log and come back out,
    /// on the wire as `trigger` and `usedPercent`; only an `ahead` switch
    /// counts towards the half-hour limit.
    #[test]
    fn the_trigger_is_recorded_and_only_an_ahead_switch_counts_towards_the_rate_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = RotationLog::new(Arc::new(Db::open(dir.path().join("state.db"))));
        log.record(&decision("s", 0)).expect("a limit switch");
        let latest = log.latest("s").expect("a decision");
        assert_eq!((latest.trigger, latest.used_percent), (RotationTrigger::Limit, None));
        let wire = serde_json::to_value(&latest).expect("json");
        assert_eq!(wire["trigger"], "limit");
        assert!(wire["usedPercent"].is_null(), "present and null: {wire}");
        assert_eq!(log.last_ahead_ms("s"), None, "a limit switch is not a move ahead");

        log.record(&Decision {
            trigger: RotationTrigger::Ahead,
            used_percent: Some(96.4),
            ..decision("s", 5)
        })
        .expect("an ahead switch");
        log.record(&Decision {
            trigger: RotationTrigger::Ahead,
            outcome: RotationOutcome::NoCandidate,
            ..decision("s", 9)
        })
        .expect("a row that moved nothing");
        let latest = log.history("s", 2).pop().expect("the switch");
        assert_eq!((latest.trigger, latest.used_percent), (RotationTrigger::Ahead, Some(96.4)));
        let wire = serde_json::to_value(&latest).expect("json");
        assert_eq!((wire["trigger"].as_str(), wire["usedPercent"].as_f64()), (Some("ahead"), Some(96.4)));
        assert_eq!(log.last_ahead_ms("s"), Some(NOW + 5), "the switch, not the row that went nowhere");
        assert_eq!(log.last_ahead_ms("other"), None);
        // A row with no trigger at all — every row an older release wrote —
        // is a limit; see the migration's own test for one read off disk.
        assert_eq!(RotationTrigger::parse(None), RotationTrigger::Limit);
        assert_eq!(RotationTrigger::parse(Some("something-new")), RotationTrigger::Limit);
    }

    #[test]
    fn a_binding_made_by_hand_is_remembered_per_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = RotationLog::new(Arc::new(Db::open(dir.path().join("state.db"))));
        assert_eq!(log.bound_by_hand_ms("s"), None);
        log.note_hand_binding("s", NOW);
        assert_eq!(log.bound_by_hand_ms("s"), Some(NOW));
        log.note_hand_binding("s", NOW + 5);
        assert_eq!(log.bound_by_hand_ms("s"), Some(NOW + 5), "the latest choice");
        assert_eq!(log.bound_by_hand_ms("other"), None);
    }
}
