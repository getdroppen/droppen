//! The two headers a bridged request carries from this machine's agent to its
//! own loopback listener: the per-boot internal credential, and the device the
//! end-to-end channel was authorised for.
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
/// The header marking a bridged request initiated on behalf of a plugin running
/// inside a cloud page's sandboxed frame.
pub const PLUGIN_HEADER: &str = "x-rh-plugin";

/// Request extension inserted by the auth guard when a bridged request carries
/// `X-Rh-Plugin: <id>`. Present only for authenticated cloud requests whose
/// bridge forwarded the header; stripped from the HTTP headers on every request
/// so handlers inspect this extension rather than raw headers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginCaller(pub String);
