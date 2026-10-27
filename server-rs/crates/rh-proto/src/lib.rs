//! remote-hand's cloud protocol, shared by the agent (today's server, plus one
//! outbound connection) and, from M1, the hub.
//!
//! The design is `docs/cross-machine-architecture.md`. Its one hard rule: the
//! hub can route, store and meter, and can never read anything that has to do
//! with an AI conversation. So the layers are:
//!
//! - [`frame`] — what the hub sees: `[type][ch][len][payload]` envelopes, and
//!   the bodies of the stream frames it stores.
//! - [`noise`] — the end-to-end channel inside `DATA` payloads: Noise IK
//!   (`25519_AESGCM_SHA256`), bound to the account, device, machine and device
//!   chain by its prologue, with replay protection on the first message and
//!   rekeying by count and age.
//! - [`bridge`] — what a sub-stream carries when the agent bridges it to its
//!   loopback API (request and response heads, framed messages).
//! - [`mux`] — sub-streams inside the channel (`rpc`, `mux`, `term`, `blob`),
//!   with credit windows and priorities. [`channel`] glues the two together.
//! - [`handshake`] — what the handshake payloads carry (chain views, never a
//!   request) and the order an agent checks a device in.
//! - `driver` (feature `runtime`) — a channel running on tokio, shared by the
//!   agent and headless devices.
//! - [`flow`] — hub-level credit on each channel, which the hub enforces.
//! - [`path`] — the direct path next to the hub: signaling inside the channel,
//!   DTLS fingerprints bound into a Noise handshake on the DataChannel, and
//!   sequenced frames that move between the two paths with nothing lost.
//! - [`chain`] — the signed device chain every trust decision rests on.
//! - [`record`] — encrypted log records the hub stores for offline reading.
//! - [`stream`] — what each stream holds, snapshot pages and signed manifests,
//!   and drafts that are relayed but never stored.
//! - [`wrap`] — sealing `AK`/`UK`/`MK` to a device or an agent.
//! - [`identity`] — a machine's keys, fingerprints and enrollment codes.
//! - [`link_return`] — the sealed reply droppen.app hands a device's local
//!   page back through the browser when there is no popup opener.
//! - [`pairing`], [`pake`] — adding a second device by QR code or numeric code
//!   (CPace over X25519), and the MAC'd key delivery.
//! - [`mnemonic`] — the recovery key as BIP-39 words.
//! - [`sig`] — every signature's label and structure, in one place.
//! - [`ticket`] — the Hub's signed relay tickets, and a relay's proof that it
//!   answers at its origin; [`netpolicy`] — where a relay's origin may point.
//!
//! The browser half is `web/src/lib/e2e/`, written against WebCrypto. The two
//! are kept honest by test vectors in `vectors/`: `rust.json` is produced here
//! and read by the TypeScript tests, `ts.json` the other way round.

pub mod bridge;
pub mod chain;
pub mod channel;
pub mod codec;
#[cfg(feature = "rtc")]
pub mod direct;
#[cfg(feature = "runtime")]
pub mod driver;
pub mod flow;
pub mod frame;
pub mod handshake;
pub mod identity;
pub mod keys;
pub mod link_return;
pub mod mnemonic;
pub mod mux;
pub mod netpolicy;
pub mod noise;
pub mod pairing;
pub mod pake;
pub mod path;
pub mod record;
#[cfg(feature = "rtc")]
pub mod rtc;
pub mod sig;
pub mod stream;
pub mod ticket;
pub mod wrap;

#[cfg(test)]
mod attack_regressions;
#[cfg(test)]
mod relay_test;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod v3_regressions;
#[cfg(test)]
mod vectors;
