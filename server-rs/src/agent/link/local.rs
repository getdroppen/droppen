use std::sync::{Arc, Mutex};

use rh_proto::chain::ChainError;
use serde_json::{json, Value};

use super::bridge;
use super::hub::now_ms;
use super::lapse::{still_controls, ChannelLapse};
use super::state;
use super::Shared;

/// The agent's own routes over a channel ([`bridge::LOCAL_PREFIX`]): what a
/// device tells the agent itself rather than its API.
///
/// - `GET /rh/info` → `{displayName, hostname, mkEpoch, machine, alarms,
///   controllers}` (`controllers` as in the status stream, [`state::Controllers`])
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
    let view = match chain.freshness(device, &st.attestation_list(), now_ms(), rh_proto::chain::MANAGE_FRESHNESS_MS) {
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
        let controls = still_controls(&self.shared, &self.lapse, now_ms());
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
        // Set before a renewed identity key is taken, cleared once
        // [`renew_push`] made the new push keys.
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
        let bad = |message: &str| (400, json!({"error": "bad_request", "message": message}));
        match (method, path) {
            ("GET", "/rh/info") => {
                let st = shared.state.lock().expect("agent state");
                (
                    200,
                    json!({"displayName": st.display_name, "hostname": crate::api::hostname(), "mkEpoch": st.mk_epoch, "machine": st.machine, "alarms": st.alarms, "controllers": st.controllers()}),
                )
            }
            (_, "/rh/display-name" | "/rh/machine-key") if !full => (
                403,
                json!({"error": "forbidden", "message": "only a current full device of the account may do this"}),
            ),
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
                (200, json!({"displayName": name}))
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
                    // Exactly the key the chain committed for this epoch (M2
                    // review): never one of the sender's choosing, never an
                    // epoch the chain has not reached.
                    let committed = st.chain.as_ref().map(|c| c.check_machine_key(&st.machine, epoch, key.as_ref()));
                    match committed {
                        Some(Ok(())) => {}
                        Some(Err(ChainError::UncommittedKey { .. })) => {
                            return (
                                409,
                                json!({"error": "uncommitted", "message": "the chain commits no such content key epoch (yet): sync and attest, then retry", "mkEpoch": st.mk_epoch}),
                            )
                        }
                        _ => {
                            return (
                                409,
                                json!({"error": "key_mismatch", "message": "not the key the chain committed for that epoch", "mkEpoch": st.mk_epoch}),
                            )
                        }
                    }
                    st.mk_epoch
                };
                if let Err(error) = state::store_machine_key(&shared.paths, epoch, key.as_ref()) {
                    tracing::error!(%error, "cannot store a machine key");
                    return (500, json!({"error": "unavailable", "message": "cannot store the key"}));
                }
                let newer = current.is_none_or(|c| epoch > c);
                if newer {
                    shared.state.lock().expect("agent state").mk_epoch = Some(epoch);
                    shared.save();
                    tracing::info!(epoch, %device, reencrypt, "took a machine key inside a channel");
                    shared.sync.send(super::streams::Event::KeysChanged { reencrypt });
                    return (200, json!({"mkEpoch": epoch, "changed": true}));
                }
                // An epoch it holds already (or an older one, kept for
                // reading): with `reencrypt`, the re-encryption a mailed key
                // could not ask for.
                if reencrypt && current == Some(epoch) {
                    tracing::info!(epoch, %device, "re-encryption asked for the current machine key");
                    shared.sync.send(super::streams::Event::KeysChanged { reencrypt: true });
                }
                (
                    200,
                    json!({"mkEpoch": current, "changed": false, "reencrypt": reencrypt && current == Some(epoch)}),
                )
            }
            _ => (404, json!({"error": "not_found", "message": "no such agent route"})),
        }
    }
}

pub(crate) fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}
