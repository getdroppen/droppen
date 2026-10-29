//! What this machine answers a device over the end-to-end channel: its
//! refusals, the bodies of its own routes (`/rh/*`) and the handshake's
//! `server_info`. Device pages of any version read them. The hub never
//! sees them (they travel inside the channel), so they are this crate's
//! types, not `rh_proto::wire`'s. A route still returns `(status, Value)`:
//! [`AnswerBody::body`] makes the `Value`.

use serde::Serialize;
use serde_json::Value;

use super::state::{Alarm, Controllers};

/// An answer as the JSON body a route returns.
pub(in crate::agent) trait AnswerBody: Serialize {
    fn body(&self) -> Value {
        serde_json::to_value(self).expect("answers encode")
    }
}

/// A refusal: `error` is the code a page tells apart, `message` what it
/// shows.
#[derive(Serialize)]
pub(in crate::agent) struct Refused<'a> {
    pub error: &'a str,
    pub message: &'a str,
}

/// A refusal with a reason under its code: the view a stale answer is about
/// (`stale`), or what the hub said (`hub_refused`).
#[derive(Serialize)]
pub(in crate::agent) struct RefusedWithReason<'a> {
    pub error: &'a str,
    pub reason: &'a str,
    pub message: &'a str,
}

/// A machine key refused, with the epoch this machine holds (`null` when it
/// holds none).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::agent) struct KeyRefused<'a> {
    pub error: &'a str,
    pub message: &'a str,
    pub mk_epoch: Option<u32>,
}

/// A chain view that is not on this machine's chain, with how far that
/// chain goes.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::agent) struct NotOnChain<'a> {
    pub error: &'a str,
    pub message: &'a str,
    pub chain_seq: u64,
}

/// `GET /rh/info`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::agent) struct Info<'a> {
    pub display_name: &'a Option<String>,
    pub hostname: String,
    pub mk_epoch: Option<u32>,
    pub machine: &'a str,
    pub alarms: &'a [Alarm],
    pub controllers: Option<Controllers>,
}

/// `POST /rh/display-name`: the name now (`null`: none).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::agent) struct DisplayName {
    pub display_name: Option<String>,
}

/// `POST /rh/machine-key`: the epoch held now, whether the key changed it,
/// and, for a key held already, whether re-encryption was asked for.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::agent) struct MachineKeyTaken {
    pub mk_epoch: Option<u32>,
    pub changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reencrypt: Option<bool>,
}

/// `POST /rh/rotate`: the new keys, both keys' signatures over the renewal,
/// the DH confirmation MAC, the chain head it was staged at, and when the
/// hub said it is ready (`null` until it did).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::agent) struct Rotated {
    pub sign_key: String,
    pub dh_key: String,
    pub old_sig: String,
    pub new_sig: String,
    pub mac: String,
    pub staged_seq: u64,
    pub staged_head: String,
    pub ready_ms: Option<u64>,
}

/// The handshake's `server_info`: what this agent is and the features it
/// offers devices.
#[derive(Serialize)]
pub(in crate::agent) struct ServerInfo {
    pub agent: &'static str,
    pub version: &'static str,
    pub features: Vec<&'static str>,
}

impl AnswerBody for Refused<'_> {}
impl AnswerBody for RefusedWithReason<'_> {}
impl AnswerBody for KeyRefused<'_> {}
impl AnswerBody for NotOnChain<'_> {}
impl AnswerBody for Info<'_> {}
impl AnswerBody for DisplayName {}
impl AnswerBody for MachineKeyTaken {}
impl AnswerBody for Rotated {}
