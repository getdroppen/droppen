//! Which account a session runs under, and the record of it being moved off
//! one: the shapes a session's summary, its detail and its live frames carry.
//!
//! They are session vocabulary, so they live here. Accounts chooses the
//! account, classifies the limit and writes the rotation log
//! (`accounts::{store, limits, rotation}`); it fills these in.

use gotcha::Schematic;
use serde::{Deserialize, Serialize};

use droppen_http::openapi_ext::Nullable;

/// The account a session runs under, as the session payloads name it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
pub struct AccountRef {
    /// The account's uuid.
    pub id: String,
    /// Its display name.
    pub label: String,
}

/// What kind of limit a failure was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "snake_case")]
pub enum LimitClass {
    /// The rolling five-hour window: "You've hit your session limit".
    Session,
    /// A weekly window, overall or for one model family.
    Weekly,
    /// A request-rate limit, or the vendor saying it is overloaded.
    Rate,
    /// Out of prepaid credit, or a billing problem.
    Credit,
    /// The credential itself was refused.
    Auth,
    /// Limit-shaped wording that nothing more specific matched.
    Other,
}

impl LimitClass {
    /// The name the tables and the API use.
    pub fn as_str(self) -> &'static str {
        match self {
            LimitClass::Session => "session",
            LimitClass::Weekly => "weekly",
            LimitClass::Rate => "rate",
            LimitClass::Credit => "credit",
            LimitClass::Auth => "auth",
            LimitClass::Other => "other",
        }
    }

    /// The inverse of [`LimitClass::as_str`]; a row nobody recognises is `other`.
    pub fn parse(raw: &str) -> Self {
        match raw {
            "session" => LimitClass::Session,
            "weekly" => LimitClass::Weekly,
            "rate" => LimitClass::Rate,
            "credit" => LimitClass::Credit,
            "auth" => LimitClass::Auth,
            _ => LimitClass::Other,
        }
    }
}

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

// Inline in `SessionRotation`, the way the contract spells `outcome`, rather
// than a schema of its own. (`///` here would be an unused doc comment: rustdoc
// does not document macro invocations.)
droppen_http::inline_string_enum!(RotationOutcome, ["switched", "no_candidate", "cap_reached"]);

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

// Inline in `SessionRotation`, like `RotationOutcome`.
droppen_http::inline_string_enum!(RotationTrigger, ["limit", "ahead"]);

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
