//! What became of the messages a client sent a session and of the questions its
//! agent asked, in the shape a session's state and the API report them.
//!
//! These are session vocabulary: [`super::SessionDetail`] and the live feed's
//! frames carry them, and the routes answer with them. The stores that keep
//! them and the drivers that move them (`inject::receipts`,
//! `inject::questions`) build on sessions, so the shapes live here and
//! the stores fill them in.

use std::collections::HashMap;

use gotcha::Schematic;
use serde::{Deserialize, Serialize};

use super::util::QuestionEntry;
use droppen_http::openapi_ext::{Nullable, Optional};

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
    pub fn from_stored(stored: &str) -> ReceiptStatus {
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

/// Where a question is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum QuestionState {
    /// claude is blocked on it, and it can be answered.
    Pending,
    /// The user answered it.
    Answered,
    /// The user declined to answer; the agent was told to use its judgement.
    Dismissed,
    /// The turn it belonged to was stopped, or claude withdrew it.
    Cancelled,
    /// The process that asked it is gone. Still answerable: the answer goes to
    /// the session as an ordinary message instead.
    Expired,
}

impl QuestionState {
    /// The word the table stores.
    pub fn as_str(self) -> &'static str {
        match self {
            QuestionState::Pending => "pending",
            QuestionState::Answered => "answered",
            QuestionState::Dismissed => "dismissed",
            QuestionState::Cancelled => "cancelled",
            QuestionState::Expired => "expired",
        }
    }

    /// A stored word, read back. Anything unrecognised is a question nobody can
    /// do anything with any more, which is what `cancelled` says.
    pub fn from_stored(stored: &str) -> QuestionState {
        match stored {
            "pending" => QuestionState::Pending,
            "answered" => QuestionState::Answered,
            "dismissed" => QuestionState::Dismissed,
            "expired" => QuestionState::Expired,
            _ => QuestionState::Cancelled,
        }
    }
}

/// One question an agent asked, as a session's state carries it.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SessionQuestion {
    /// The CLI's id for the request, which the answer routes are addressed by.
    pub request_id: String,
    /// The `tool_use` the question is, as the transcript's item names it.
    pub tool_use_id: Nullable<String>,
    /// What was asked.
    pub questions: Vec<QuestionEntry>,
    /// Where it is in its life.
    pub state: QuestionState,
    /// What was chosen, keyed by the exact question text; multi-select answers
    /// are joined with `", "`. `null` until answered.
    pub answers: Nullable<HashMap<String, String>>,
    /// Free text sent with the answer, or instead of one.
    pub response: Nullable<String>,
    /// Who answered or dismissed it, when the server knows; `null` with
    /// authentication off.
    pub answered_by: Nullable<String>,
    /// When it was answered or dismissed, ISO-8601.
    pub answered_at: Nullable<String>,
    /// When claude asked it, ISO-8601.
    pub created_at: String,
}
