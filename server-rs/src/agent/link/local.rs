use std::sync::{Arc, Mutex};

use rh_proto::chain::ChainError;
use serde_json::Value;

use super::bridge;
use crate::agent::answers::{AnswerBody, DisplayName, Info, KeyRefused, MachineKeyTaken, Refused};
use super::lapse::{still_controls, ChannelLapse};
use super::Shared;
use droppen_base::text::unhex;
use droppen_base::time::now_ms_u64;

/// The agent's own routes over a channel ([`bridge::LOCAL_PREFIX`]): what a
/// device tells the agent itself rather than its API.
///
/// - `GET /rh/info` → `{displayName, hostname, mkEpoch, machine, alarms,
///   controllers}` (`controllers` as in the status stream, [`super::state::Controllers`])
/// - `POST /rh/display-name {name}` — what devices call this machine
/// - `POST /rh/machine-key {epoch, key, reencrypt}` — a machine key handed
///   over inside the end-to-end channel (the mailbox is only the fallback for
///   an agent that is offline): only from a current full device, and only the
///   key the chain committed for that epoch (409 `uncommitted` /
///   `key_mismatch` otherwise). The current key again with `reencrypt` starts
///   the re-encryption a mailed key could not ask for.
/// - `POST /rh/rotate {nonce, eph, chainSeq, chainHead, assurance}` — renew
///   this device's identity key ([`super::rotation::rotate`]).
pub(in crate::agent) struct AgentLocal {
    shared: Arc<Shared>,
    /// The channel's control lapse, shared with the channel's own task: what
    /// a request is held to again when it is dispatched.
    lapse: Arc<Mutex<ChannelLapse>>,
}

impl AgentLocal {
    pub(in crate::agent) fn new(shared: Arc<Shared>, lapse: Arc<Mutex<ChannelLapse>>) -> Self {
        Self { shared, lapse }
    }
}

/// Where `device` stands for the admin-only routes, as this agent's own
/// chain and the attestations it holds say now — what a route answered in
/// the agent itself checks again after waiting (`/rh/rotate`). `controls`:
/// the chain lets it control this machine; the channel's control lapse
/// (seven days without an admin's word) was checked by the bridge moments
/// before, and an admin-only route needs a view vouched for within twelve
/// hours anyway.
pub(in crate::agent) fn standing_of(shared: &Shared, device: &str) -> bridge::Standing {
    let controls = {
        let st = shared.state.lock().expect("agent state");
        st.chain.as_ref().is_some_and(|c| c.may_control(device, &st.machine).is_ok())
    };
    standing_with(shared, device, controls)
}

/// [`standing_of`], with `controls` as judged by the caller.
pub(in crate::agent) fn standing_with(shared: &Shared, device: &str, controls: bool) -> bridge::Standing {
    let st = shared.state.lock().expect("agent state");
    let Some(chain) = st.chain.as_ref() else {
        return bridge::Standing {
            admin: false,
            view: bridge::View::Stale,
            controls: false,
        };
    };
    use rh_proto::chain::Freshness;
    let view = match chain.freshness(device, &st.attestation_list(), now_ms_u64(), rh_proto::chain::MANAGE_FRESHNESS_MS) {
        Ok(Freshness::Fresh) => bridge::View::Fresh,
        Ok(Freshness::Stale { .. }) => bridge::View::Stale,
        Ok(Freshness::Behind { .. }) => bridge::View::Behind,
        Err(ChainError::Fork { .. }) => bridge::View::Fork,
        Err(_) => bridge::View::Stale,
    };
    bridge::Standing {
        admin: chain.admin(device).is_some(),
        view,
        controls,
    }
}

impl bridge::Local for AgentLocal {
    fn standing(&self, device: &str) -> bridge::Standing {
        // The channel's own control lapse (local-aggregator §3.4).
        let controls = still_controls(&self.shared, &self.lapse, now_ms_u64());
        standing_with(&self.shared, device, controls)
    }

    fn handle<'a>(&'a self, device: &'a str, method: &'a str, path: &'a str, body: &'a [u8]) -> futures_util::future::BoxFuture<'a, (u16, Value)> {
        Box::pin(async move {
            if (method, path) == ("POST", "/rh/rotate") {
                return super::rotation::rotate(&self.shared, device, body).await;
            }
            self.route(device, method, path, body)
        })
    }

    fn push_renewing(&self) -> bool {
        // Set before a renewed identity key is taken, cleared once the push
        // keys are renewed.
        self.shared.state.lock().expect("agent state").rotation.push_renew
    }
}

impl AgentLocal {
    fn route(&self, device: &str, method: &str, path: &str, body: &[u8]) -> (u16, Value) {
        let shared = &self.shared;
        let full = {
            let st = shared.state.lock().expect("agent state");
            st.chain.as_ref().and_then(|c| c.admin(device)).is_some()
        };
        let bad = |message: &str| (400, Refused { error: "bad_request", message }.body());
        match (method, path) {
            ("GET", "/rh/info") => {
                let st = shared.state.lock().expect("agent state");
                let info = Info {
                    display_name: &st.display_name,
                    hostname: droppen_http::hostname(),
                    mk_epoch: st.mk_epoch,
                    machine: &st.machine,
                    alarms: &st.alarms,
                    controllers: st.controllers(),
                };
                (200, info.body())
            }
            (_, "/rh/display-name" | "/rh/machine-key") if !full => {
                let refused = Refused {
                    error: "forbidden",
                    message: "only a current full device of the account may do this",
                };
                (403, refused.body())
            }
            ("POST", "/rh/display-name") => {
                let Ok(v) = serde_json::from_slice::<Value>(body) else {
                    return bad("JSON expected");
                };
                let name = match &v["name"] {
                    Value::Null => None,
                    Value::String(s) => {
                        let s = s.trim();
                        if s.chars().count() > 64 || s.chars().any(char::is_control) {
                            return bad("a name is at most 64 characters, none of them control characters");
                        }
                        (!s.is_empty()).then(|| s.to_string())
                    }
                    _ => return bad("name must be a string or null"),
                };
                shared.state.lock().expect("agent state").display_name = name.clone();
                shared.save();
                tracing::info!(%device, "display name changed");
                shared.sync.send(super::streams::Event::DisplayName(name.clone()));
                (200, DisplayName { display_name: name }.body())
            }
            ("POST", "/rh/machine-key") => {
                let Ok(v) = serde_json::from_slice::<Value>(body) else {
                    return bad("JSON expected");
                };
                let (Some(epoch), Some(key)) = (
                    v["epoch"].as_u64().and_then(|e| u32::try_from(e).ok()),
                    v["key"]
                        .as_str()
                        .and_then(unhex)
                        .and_then(|k| <[u8; 32]>::try_from(k).ok())
                        .map(zeroize::Zeroizing::new),
                ) else {
                    return bad("{epoch, key (64 hex)} expected");
                };
                let reencrypt = v["reencrypt"].as_bool().unwrap_or(false);
                let current = {
                    let st = shared.state.lock().expect("agent state");
                    // Exactly the key the chain committed for this epoch:
                    // never one of the sender's choosing, never an epoch the
                    // chain has not reached.
                    let committed = st.chain.as_ref().map(|c| c.check_machine_key(&st.machine, epoch, key.as_ref()));
                    match committed {
                        Some(Ok(())) => {}
                        Some(Err(ChainError::UncommittedKey { .. })) => {
                            let refused = KeyRefused {
                                error: "uncommitted",
                                message: "the chain commits no such content key epoch (yet): sync and attest, then retry",
                                mk_epoch: st.mk_epoch,
                            };
                            return (409, refused.body());
                        }
                        _ => {
                            let refused = KeyRefused {
                                error: "key_mismatch",
                                message: "not the key the chain committed for that epoch",
                                mk_epoch: st.mk_epoch,
                            };
                            return (409, refused.body());
                        }
                    }
                    st.mk_epoch
                };
                if let Err(error) = shared.files.store_machine_key(epoch, key.as_ref()) {
                    tracing::error!(%error, "cannot store a machine key");
                    return (500, Refused { error: "unavailable", message: "cannot store the key" }.body());
                }
                let newer = current.is_none_or(|c| epoch > c);
                if newer {
                    shared.state.lock().expect("agent state").mk_epoch = Some(epoch);
                    shared.save();
                    tracing::info!(epoch, %device, reencrypt, "took a machine key inside a channel");
                    shared.sync.send(super::streams::Event::KeysChanged { reencrypt });
                    let taken = MachineKeyTaken {
                        mk_epoch: Some(epoch),
                        changed: true,
                        reencrypt: None,
                    };
                    return (200, taken.body());
                }
                // An epoch it holds already (or an older one, kept for
                // reading): with `reencrypt`, the re-encryption a mailed key
                // could not ask for.
                if reencrypt && current == Some(epoch) {
                    tracing::info!(epoch, %device, "re-encryption asked for the current machine key");
                    shared.sync.send(super::streams::Event::KeysChanged { reencrypt: true });
                }
                let held = MachineKeyTaken {
                    mk_epoch: current,
                    changed: false,
                    reencrypt: Some(reencrypt && current == Some(epoch)),
                };
                (200, held.body())
            }
            _ => (404, Refused { error: "not_found", message: "no such agent route" }.body()),
        }
    }
}
