//! Joining an account.
//!
//! On my.droppen.app the user adds a device that runs AI and makes an
//! `RH1-<account fingerprint>-<secret>` code, registered with the hub as
//! `kind: "machine"`. The proxy, started with the code in
//! `DROPPEN_ENROLL_CODE` and an empty `DROPPEN_DATA_DIR`:
//!
//! 1. makes its identity (fresh keys) and claims the code at the hub
//!    ([`droppen_client::enroll::claim_machine`]): the hub assigns its machine
//!    id, and the page gets the proof;
//! 2. keeps its identity and its chain view pinned to the code's account
//!    fingerprint (`0600`) — before anything else, so a restart goes on from
//!    here without the code;
//! 3. logs its device fingerprint, for the user to paste into the page,
//!    which adds it (`AddDevice3 {host}`) only if it is the one that proved
//!    the code;
//! 4. follows the chain (`/v1/client`, else `GET /v1/chain`) until its own
//!    verified view names it with exactly its keys and the host flag — and
//!    refuses to run when it names it otherwise.
//!
//! A proxy that has joined ignores the code (it says to remove it): the code
//! is for one service's first start only, and should never sit in a shared
//! variable.

use std::time::Duration;

use droppen_client::device::{DeviceOptions, StandaloneDevice};
use droppen_client::enroll::{claim_machine, ClaimError};
use droppen_client::identity::DeviceIdentity;
use droppen_client::verified::VerifiedChain;
use droppen_client::HubUrl;
use droppen_proto::chain::ChainState;
use droppen_proto::identity::EnrollCode;

use crate::config::Config;

/// How often a proxy waiting to be added asks again when the hub's socket
/// cannot be had.
const WAIT_POLL: Duration = Duration::from_secs(5);

/// The proxy's device from its data directory — joining the account first
/// when it never did — once its own chain names it as a host with its keys.
pub async fn open_or_join(config: &Config, options: DeviceOptions) -> Result<StandaloneDevice, String> {
    let device = match StandaloneDevice::open_dir(&config.data_dir, options.clone())? {
        Some(device) => {
            if config.enroll_code.is_some() {
                tracing::warn!(
                    "DROPPEN_ENROLL_CODE is set but this proxy has joined its account already: the code is ignored; remove it from the environment"
                );
            }
            device
        }
        None => join(config, options).await?,
    };
    let joined = HubUrl::parse(&device.identity().hub)?.http;
    if joined != config.hub {
        return Err(format!(
            "the proxy joined its account at {joined}, not at DROPPEN_HUB ({}): set DROPPEN_HUB to {joined}, or start from an empty DROPPEN_DATA_DIR",
            config.hub
        ));
    }
    wait_until_added(&device).await?;
    Ok(device)
}

/// Claim the code in `config`, and keep what joining made.
async fn join(config: &Config, options: DeviceOptions) -> Result<StandaloneDevice, String> {
    let Some(text) = &config.enroll_code else {
        return Err(format!(
            "{} holds no proxy identity: this proxy has not joined an account; set DROPPEN_ENROLL_CODE to the code from my.droppen.app (\"Add a device\") for its first start",
            config.data_dir.display()
        ));
    };
    let code: EnrollCode = text.parse().map_err(|e| format!("DROPPEN_ENROLL_CODE: {e}"))?;
    let hub = HubUrl::parse(&config.hub)?;
    let (sign, dh) = DeviceIdentity::fresh_keys();
    let claimed = claim_machine(&hub, &code, &sign, &dh).await.map_err(|e| match e {
        ClaimError::Refused { code, .. } if code == "unknown_enrollment" => {
            "the hub does not know this DROPPEN_ENROLL_CODE (it expired after 10 minutes, was used, or is another account's): make a new one on my.droppen.app".to_string()
        }
        other => format!("joining the account with DROPPEN_ENROLL_CODE: {other}"),
    })?;
    // The pin first: a restart that finds the identity finds the pin too.
    let chain = VerifiedChain::pinned(&config.data_dir, &claimed.account, code.account_fingerprint);
    chain.save()?;
    let identity = DeviceIdentity::new(&config.hub, &claimed.account, &claimed.device, sign, dh);
    identity.save(&config.data_dir)?;
    tracing::info!(account = %claimed.account, device = %claimed.device, "the hub took the enrollment code");
    StandaloneDevice::new(identity, chain, options)
}

/// Wait until the device's own verified chain names it as a host with its
/// keys.
async fn wait_until_added(device: &StandaloneDevice) -> Result<(), String> {
    let identity = device.identity();
    let (me, sign, dh) = (identity.device.clone(), identity.sign_public(), identity.dh_public());
    if let Some(outcome) = device.with_chain(|chain| added(chain, &me, &sign, &dh)) {
        return outcome;
    }
    tracing::info!(
        device = %me,
        fingerprint = %identity.fingerprint(),
        "waiting to be added to the account: on my.droppen.app, paste this fingerprint where the page asks for it"
    );
    let outcome = device
        .watch_chain(WAIT_POLL, |chain| added(chain, &me, &sign, &dh))
        .await
        .map_err(|e| format!("waiting to be added to the account: {e}"))?;
    if outcome.is_ok() {
        tracing::info!(device = %me, "added to the account");
    }
    outcome
}

/// Whether `chain` names device `me` with exactly these keys and as a host:
/// `None` while it does not name it (keep waiting), an error when it names it
/// otherwise.
pub fn added(chain: Option<&ChainState>, me: &str, sign: &[u8; 32], dh: &[u8; 32]) -> Option<Result<(), String>> {
    let record = chain?.device_record(me)?;
    if &record.info.sign_key != sign || &record.info.dh_key != dh {
        return Some(Err(format!(
            "the account added {me} with other keys than this proxy's: refusing to run as it (an enrollment someone else completed?)"
        )));
    }
    if !record.is_active() {
        return Some(Err(format!(
            "{me} is no current device of the account (removed): join again with a new code"
        )));
    }
    if !record.host {
        return Some(Err(format!(
            "the account added {me} without the host flag: this program runs only as a device that runs AI; remove it, and add it again as a device"
        )));
    }
    Some(Ok(()))
}
