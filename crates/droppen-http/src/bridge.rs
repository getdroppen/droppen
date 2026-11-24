//! The headers a bridged request carries from this machine's agent to its own
//! loopback listener: the per-boot internal credential, the device the
//! end-to-end channel was authorised for, and — for an A2A proxy's channel
//! (`docs/a2a.md` §7.3) — what kind of device that is.
//!
//! The agent's bridge sets them; the auth guard honours them only from a
//! loopback peer with this boot's credential, and strips them from every
//! request either way. They sit here, below both, so the guard does not depend
//! on the agent that writes them.
//!
//! The header names are protocol identifiers, not product names: the bridge
//! writes them and the auth guard keys on them, so they stay as they are.

/// The header carrying the internal credential.
pub const CREDENTIAL_HEADER: &str = "x-rh-agent";
/// The header naming the device a bridged request is from.
pub const DEVICE_HEADER: &str = "x-rh-agent-device";
/// The header marking a bridged request as an A2A proxy's
/// ([`A2A_KIND`]): absent for every other device. The guard confines such a
/// request to `/a2a/*` and hands it on as a
/// [`crate::identity::BridgedDevice`] with `a2a` set, never as a login.
pub const KIND_HEADER: &str = "x-rh-agent-kind";
/// [`KIND_HEADER`]'s one value: the device is an A2A proxy. Any other value
/// makes the request nobody's.
pub const A2A_KIND: &str = "a2a";
