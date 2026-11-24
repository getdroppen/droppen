//! A2A proxies as callers (`docs/a2a.md` §7): a device of the account that
//! calls this device's A2A through an end-to-end channel, never with a token.
//!
//! The agent's bridge lets such a request reach `/a2a/*` only, and the login
//! guard hands it on as a [`droppen_http::identity::BridgedDevice`] with
//! `a2a` set — never as a login, never with a bearer token. Its caller is
//! `a2a:device:<id>` ([`caller_of`]). Who it is counts only while the
//! account's device chain, as this device verified it, still names it an A2A
//! proxy this device takes A2A calls from: the [`DeviceGate`] port, which the
//! server answers from its device link, so this crate names no link, chain or
//! agent type. The gate is asked on every request, before every stream event
//! and push, and whenever it says the chain moved ([`DeviceGate::changes`]),
//! what a proxy it no longer admits has here is taken back
//! ([`crate::A2a::devices_changed`]).

use gotcha::Schematic;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use droppen_http::openapi_ext::Nullable;

/// Every A2A proxy's caller starts so.
pub const PREFIX: &str = "a2a:device:";

/// The caller an A2A proxy speaks as: `a2a:device:<id>`.
pub fn caller_of(device: &str) -> String {
    format!("{PREFIX}{device}")
}

/// The device id of a caller that is an A2A proxy's.
pub fn device_of(caller: &str) -> Option<&str> {
    caller.strip_prefix(PREFIX)
}

/// Whether `id` is shaped like a device id at all: what an entry's
/// `allowedDevices` may hold ([`crate::entry::clean_devices`]).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// An A2A proxy this device takes A2A calls from, as the entry form's
/// `allowedDevices` picker lists it (`GET /api/a2a/devices`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct A2aDevice {
    /// Its device id: what `allowedDevices` holds.
    pub id: String,
    /// Its fingerprint, as "My devices" shows it.
    pub fingerprint: String,
    /// What the account calls it, when this device knows.
    pub name: Nullable<String>,
}

/// The current A2A proxies this device takes A2A calls from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
pub struct A2aDeviceList {
    /// Every one, by id; none on a device not linked to Droppen.
    pub devices: Vec<A2aDevice>,
}

/// The device chain's word on A2A proxies, the port the server answers from
/// its device link, as [`crate::SessionBackend`] is the sessions'.
pub trait DeviceGate: Send + Sync + 'static {
    /// Whether `device` is, on this device's own verified chain now, a
    /// current A2A proxy this device takes A2A calls from. No chain, no link,
    /// no answer: `false`.
    fn admits(&self, device: &str) -> bool;

    /// Every A2A proxy [`DeviceGate::admits`], by id.
    fn admitted(&self) -> Vec<A2aDevice>;

    /// Moves on whenever an answer of [`DeviceGate::admits`] may have
    /// changed: the chain moved, the link started or stopped.
    fn changes(&self) -> watch::Receiver<u64>;
}

/// A gate that admits no device and never changes: a server with no device
/// chain to ask.
pub struct NoDevices(watch::Sender<u64>);

impl NoDevices {
    /// The gate.
    pub fn new() -> Self {
        Self(watch::channel(0).0)
    }
}

impl Default for NoDevices {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceGate for NoDevices {
    fn admits(&self, _device: &str) -> bool {
        false
    }

    fn admitted(&self) -> Vec<A2aDevice> {
        Vec::new()
    }

    fn changes(&self) -> watch::Receiver<u64> {
        self.0.subscribe()
    }
}
