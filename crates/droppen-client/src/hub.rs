//! A standalone device at the hub: a device session it mints with its own
//! key ([`crate::session`]) and mints again before it lapses or when the hub
//! refuses it, and the few routes it reads (`docs/hub-api/http.md`) — the
//! chain (`GET /v1/chain`), the relays (`GET /v1/relays`) and relay tickets
//! (`POST /v1/relays/tickets`).
//!
//! What the hub says is used for routing only — where a host is, which relay
//! to try. Every trust decision is the verified chain's ([`crate::verified`]):
//! the entries fetched here are verified there, and a ticket only lets this
//! device sign in on a relay.

use std::collections::BTreeMap;
use std::time::Duration;

use droppen_proto::chain::Entry;
use ed25519_dalek::SigningKey;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

use crate::failure::{Failure, Refuser};
use crate::http::{send, HttpError};
use crate::identity::DeviceIdentity;
use crate::session::{mint, Minted};
use crate::{now_ms, unhex, HubUrl};

/// A session with less than this left is minted again first.
const SESSION_MARGIN_MS: u64 = 60_000;

/// A relay ticket (`POST /v1/relays/tickets`).
#[derive(Clone)]
pub struct Ticket {
    pub relay: String,
    /// Where the relay is dialled (`wss://…`).
    pub origin: String,
    /// The ticket, opaque (base64url).
    pub ticket: String,
    /// When (ms) it runs out.
    pub expires_ms: u64,
}

impl std::fmt::Debug for Ticket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ticket")
            .field("relay", &self.relay)
            .field("origin", &self.origin)
            .field("expires_ms", &self.expires_ms)
            .finish_non_exhaustive()
    }
}

/// One relay of `GET /v1/relays`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayEntry {
    pub id: String,
    pub origin: String,
    /// `up` or `draining`.
    pub state: String,
}

/// `GET /v1/relays`: the relays this account may use, the ones it uses now,
/// and each host's last reported round trips (`agents`: host → relay → ms).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Directory {
    pub relays: Vec<RelayEntry>,
    pub account_set: Vec<String>,
    pub agents: BTreeMap<String, BTreeMap<String, u64>>,
}

impl Directory {
    /// The relays to try for `host`, best first: those its agent reported a
    /// round trip to (it is attached there), nearest first; then the
    /// account's set. Only relays that are `up`.
    pub fn candidates(&self, host: &str) -> Vec<RelayEntry> {
        let up = |id: &str| self.relays.iter().find(|r| r.id == id && r.state == "up").cloned();
        let mut attached: Vec<(u64, &String)> = self
            .agents
            .get(host)
            .map(|m| m.iter().map(|(id, rtt)| (*rtt, id)).collect())
            .unwrap_or_default();
        attached.sort();
        let mut out: Vec<RelayEntry> = Vec::new();
        for id in attached.into_iter().map(|(_, id)| id).chain(self.account_set.iter()) {
            if out.iter().any(|r| &r.id == id) {
                continue;
            }
            if let Some(relay) = up(id) {
                out.push(relay);
            }
        }
        out
    }

    fn parse(v: &Value) -> Directory {
        let relays = v["relays"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|r| {
                        Some(RelayEntry {
                            id: r["id"].as_str()?.to_string(),
                            origin: r["origin"].as_str()?.to_string(),
                            state: r["state"].as_str().unwrap_or_default().to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let account_set = v["account_set"]
            .as_array()
            .map(|a| a.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
            .unwrap_or_default();
        let agents = v["agents"]
            .as_object()
            .map(|hosts| {
                hosts
                    .iter()
                    .map(|(host, rtts)| {
                        let rtts = rtts
                            .as_object()
                            .map(|m| m.iter().filter_map(|(relay, ms)| Some((relay.clone(), ms.as_u64()?))).collect())
                            .unwrap_or_default();
                        (host.clone(), rtts)
                    })
                    .collect()
            })
            .unwrap_or_default();
        Directory { relays, account_set, agents }
    }
}

/// A device signed in at the hub with its own key.
pub struct HubSession {
    http: reqwest::Client,
    hub: HubUrl,
    account: String,
    device: String,
    sign: SigningKey,
    session: tokio::sync::Mutex<Option<Minted>>,
}

impl HubSession {
    /// For `identity` at its hub. Nothing is asked of the hub until a call needs it.
    pub fn new(identity: &DeviceIdentity) -> Result<Self, String> {
        let hub = HubUrl::parse(&identity.hub)?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        Ok(Self {
            http,
            hub,
            account: identity.account.clone(),
            device: identity.device.clone(),
            sign: identity.sign_key().clone(),
            session: tokio::sync::Mutex::new(None),
        })
    }

    pub fn hub(&self) -> &HubUrl {
        &self.hub
    }

    /// The session's token, minted first when there is none or it is about
    /// to lapse.
    async fn token(&self, fresh: bool) -> Result<zeroize::Zeroizing<String>, Failure> {
        let mut session = self.session.lock().await;
        let usable = session
            .as_ref()
            .is_some_and(|s| !fresh && s.expires_ms.is_none_or(|at| at > now_ms() + SESSION_MARGIN_MS));
        if !usable {
            let minted = mint(&self.http, &self.hub, &self.account, &self.device, &self.sign, now_ms())
                .await
                .map_err(|e| failure("a device session", e))?;
            *session = Some(minted);
        }
        Ok(session.as_ref().expect("minted").token.clone())
    }

    /// A device session's token for the hub's sockets (`/v1/client`).
    pub(crate) async fn session_token(&self) -> Result<zeroize::Zeroizing<String>, Failure> {
        self.token(false).await
    }

    /// One call, signed in; minted again once when the hub no longer takes
    /// the session (`401`).
    async fn call(&self, method: Method, path: &str, body: Option<&Value>) -> Result<Value, Failure> {
        let url = format!("{}{path}", self.hub.http);
        let token = self.token(false).await?;
        match send(&self.http, method.clone(), &url, Some(&token), body).await {
            Err(HttpError::Status { status, .. }) if status == StatusCode::UNAUTHORIZED => {
                let token = self.token(true).await?;
                send(&self.http, method, &url, Some(&token), body).await.map_err(|e| failure(path, e))
            }
            other => other.map_err(|e| failure(path, e)),
        }
    }

    /// The chain's entries from index `from`, decoded (not verified: that is
    /// [`crate::verified::VerifiedChain::advance`]'s), and the hub's head.
    pub async fn chain_from(&self, from: u64) -> Result<(Vec<Entry>, Option<u64>), Failure> {
        let v = self.call(Method::GET, &format!("/v1/chain?from={from}"), None).await?;
        let entries = v["entries"]
            .as_array()
            .ok_or_else(|| Failure::Other("GET /v1/chain: no entries".into()))?
            .iter()
            .map(|e| {
                let bytes = unhex(e.as_str().unwrap_or_default()).map_err(|_| Failure::Other("GET /v1/chain: an entry is not hex".into()))?;
                Entry::decode(&bytes).map_err(|e| Failure::Chain(format!("the hub served an entry that does not decode: {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((entries, v["seq"].as_u64()))
    }

    /// `GET /v1/relays`.
    pub async fn relays(&self) -> Result<Directory, Failure> {
        Ok(Directory::parse(&self.call(Method::GET, "/v1/relays", None).await?))
    }

    /// A ticket for `relay` (`POST /v1/relays/tickets`).
    pub async fn ticket(&self, relay: &str) -> Result<Ticket, Failure> {
        let v = self.call(Method::POST, "/v1/relays/tickets", Some(&json!({"relays": [relay]}))).await?;
        if let Some(t) = v["tickets"].as_array().and_then(|a| a.iter().find(|t| t["relay"] == relay)) {
            let field = |name: &str| {
                t[name]
                    .as_str()
                    .map(String::from)
                    .ok_or_else(|| Failure::Other(format!("a ticket with no {name}")))
            };
            return Ok(Ticket {
                relay: relay.into(),
                origin: field("origin")?,
                ticket: field("ticket")?,
                expires_ms: t["expires_ms"].as_u64().ok_or_else(|| Failure::Other("a ticket with no expiry".into()))?,
            });
        }
        let refusal = v["errors"].as_array().and_then(|a| a.iter().find(|e| e["relay"] == relay));
        let code = refusal.and_then(|e| e["error"].as_str()).unwrap_or("none");
        let why = format!("no ticket for relay {relay}: {code}");
        Err(match code {
            "forbidden" | "isolated" => Failure::Refused {
                by: Refuser::Hub,
                code: 403,
                why,
            },
            // `unknown`, `unavailable`, `limit`: not this relay, now.
            _ => Failure::Offline(why),
        })
    }
}

/// A hub call's failure, for `what`.
fn failure(what: &str, error: HttpError) -> Failure {
    match &error {
        HttpError::Network(e) => Failure::Offline(format!("the hub is unreachable ({what}): {e}")),
        HttpError::Status { status, .. } => {
            let code = error.code().unwrap_or_default().to_string();
            let why = format!("{what}: {} {code}", status.as_u16());
            match (status.as_u16(), code.as_str()) {
                (403, "device_removed") | (410, _) => Failure::Removed(why),
                (401 | 403, _) => Failure::Refused {
                    by: Refuser::Hub,
                    code: status.as_u16(),
                    why,
                },
                (429 | 500..=599, _) => Failure::Offline(why),
                _ => Failure::Other(why),
            }
        }
        HttpError::Answer(why) => Failure::Other(format!("{what}: {why}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hosts_relays_come_nearest_first_then_the_accounts_set_and_only_up_ones() {
        let d = Directory::parse(&json!({
            "relays": [
                {"id": "sin-1", "origin": "wss://sin", "state": "up"},
                {"id": "tyo-1", "origin": "wss://tyo", "state": "up"},
                {"id": "fra-1", "origin": "wss://fra", "state": "draining"},
                {"id": "iad-1", "origin": "wss://iad", "state": "up"},
            ],
            "account_set": [{"id": "iad-1", "last_used_ms": 0}, {"id": "sin-1", "last_used_ms": 5}],
            "agents": {"host": {"sin-1": 80, "tyo-1": 12, "fra-1": 3}},
        }));
        let ids: Vec<String> = d.candidates("host").into_iter().map(|r| r.id).collect();
        assert_eq!(ids, ["tyo-1", "sin-1", "iad-1"]);
        let ids: Vec<String> = d.candidates("other").into_iter().map(|r| r.id).collect();
        assert_eq!(ids, ["iad-1", "sin-1"]);
    }

    #[test]
    fn the_hubs_refusals_say_removed_only_for_a_removed_device() {
        let status = |code: u16, error: &str| HttpError::Status {
            method: Method::POST,
            url: "x".into(),
            status: StatusCode::from_u16(code).unwrap(),
            body: json!({"error": error}),
        };
        assert!(matches!(failure("s", status(403, "device_removed")), Failure::Removed(_)));
        assert!(matches!(failure("s", status(410, "account_deleted")), Failure::Removed(_)));
        assert!(matches!(
            failure("s", status(403, "forbidden")),
            Failure::Refused {
                by: Refuser::Hub,
                code: 403,
                ..
            }
        ));
        assert!(matches!(failure("s", status(503, "unavailable")), Failure::Offline(_)));
        assert!(matches!(failure("s", status(400, "bad_request")), Failure::Other(_)));
    }
}
