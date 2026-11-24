//! Joining an account (`docs/a2a.md` §7.2) — the seam for what this version
//! does not do yet.
//!
//! What it will do, once chain v3's a2a flag (bit 2, `SetA2aAccept`,
//! `may_call_a2a`: another lane) and the hub's `kind: "a2a"` registration
//! codes exist: read `DROPPEN_ENROLL_CODE`, make the proxy's identity, prove
//! the code under the `enroll-proof a2a` label (`RH1`), log the device's
//! fingerprint for the page to check, wait on `/v1/client` until the chain
//! names this device with the a2a flag and exactly these keys, and keep
//! `identity.json` ([`droppen_client::identity::DeviceIdentity::save`]) and
//! the chain pinned to the code's account fingerprint
//! ([`droppen_client::verified::VerifiedChain::pinned`], then `advance`).
//! [`crate::run`] then opens the device from the data directory as it does
//! on every later start.
//!
//! Until then the proxy starts only from a data directory a joining made.

use crate::config::Config;

/// Join the account with `config`'s enrollment code, keeping the result in
/// its data directory.
pub async fn enroll(config: &Config) -> Result<(), String> {
    Err(match config.enroll_code {
        Some(_) => format!(
            "{} holds no proxy identity, and joining an account with DROPPEN_ENROLL_CODE is not in this version of droppen yet",
            config.data_dir.display()
        ),
        None => format!(
            "{} holds no proxy identity: this proxy has not joined an account (set DROPPEN_ENROLL_CODE to a code from my.droppen.app, \"Add an A2A proxy\")",
            config.data_dir.display()
        ),
    })
}
