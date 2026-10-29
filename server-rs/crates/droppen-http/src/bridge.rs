//! The two headers a bridged request carries from this machine's agent to its
//! own loopback listener: the per-boot internal credential, and the device the
//! end-to-end channel was authorised for.
//!
//! The agent's bridge sets them; the auth guard honours them only from a
//! loopback peer with this boot's credential, and strips them from every
//! request either way. They sit here, below both, so the guard does not depend
//! on the agent that writes them.

/// The header carrying the internal credential.
pub const CREDENTIAL_HEADER: &str = "x-rh-agent";
/// The header naming the device a bridged request is from.
pub const DEVICE_HEADER: &str = "x-rh-agent-device";
