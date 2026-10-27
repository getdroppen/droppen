//! This computer's device role at its hub (local-aggregator §4.2, S1-4): the
//! HTTP half of what a device does there, under the one identity it runs AI
//! with.
//!
//! - **Sign-in**: no password, no login session — the machine key signs the
//!   hub's challenge as `HubLogin {role: DeviceSession}` (a role of its own,
//!   so the signature is worth nothing as an agent's or a relay's) and gets a
//!   device session (`POST /v1/session`, `docs/hub-api/http.md`). The hub
//!   gives a host one only while the chain lists it as a current device.
//! - **Mailbox**: `GET /v1/mailbox` (the hub never hands the device role its
//!   agent's own machine key, purpose 2), `DELETE /v1/mailbox/{id}`, and a
//!   key request (purpose 4, no secret) to an admin.
//!
//! The wire is `rh-client`'s, shared with the test device; this module says
//! what its answers mean to the controller ([`HubError`]).
//!
//! Nothing secret is logged: not the session token, not a sealed key.

use std::time::Duration;

use rh_client::http::HttpError;
use rh_client::HubUrl;
use rh_proto::identity::MachineIdentity;
use serde_json::{json, Value};

/// How long one request may take.
const TIMEOUT: Duration = Duration::from_secs(15);
/// The most mail one page of `GET /v1/mailbox` carries.
pub const MAIL_PAGE: usize = 64;
/// Mint a new session this long before the one held expires.
const RENEW_BEFORE_MS: u64 = 5 * 60 * 1000;

/// Why a request to the hub failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubError {
    /// The session is not (or no longer) good: mint a new one.
    Unauthorized,
    /// `403 device_removed`: the hub says this device was removed (only the
    /// chain is believed; the link acts on that).
    Removed,
    /// `403 host_devices_off`: the hub lets no host act as a device now
    /// (`RH_HUB_CHAIN_V3=off`).
    Off,
    /// `403 not_in_chain`: the hub's chain does not list this computer yet.
    NotInChain,
    /// `403 rotation_pending`: this computer's identity renewal is pending
    /// and the hub suspends its device role until it is done (R6 review:
    /// parked until its chain moves, not retried on the back-off).
    Suspended,
    /// The network, a timeout, a hub that is down.
    Unreachable(String),
    /// Any other answer: its status and error code.
    Refused(u16, String),
}

impl std::fmt::Display for HubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HubError::Unauthorized => f.write_str("the hub did not take this device's session"),
            HubError::Removed => f.write_str("the hub says this device was removed"),
            HubError::Off => f.write_str("the hub lets no device that runs AI act as a device now"),
            HubError::NotInChain => f.write_str("the hub does not list this device in the account yet"),
            HubError::Suspended => f.write_str("the hub suspends this device's device role while its identity renewal is pending (rotation_pending)"),
            HubError::Unreachable(why) => write!(f, "the hub is unreachable: {why}"),
            HubError::Refused(status, code) => write!(f, "the hub refused: {status} {code}"),
        }
    }
}

/// A mail for this device.
#[derive(Debug, Clone)]
pub struct Mail {
    /// Its id in the mailbox.
    pub id: i64,
    /// The device that sent it.
    pub sender: String,
    /// What it is (`rh_proto::wrap::MAIL_*`).
    pub purpose: u8,
    /// The key epoch it names.
    pub epoch: u32,
    /// What it carries.
    pub sealed: Vec<u8>,
}

/// A hub's websocket origin as the HTTP base the same hub answers on:
/// `wss://host` → `https://host`, and `ws://host` → `http://host` (the
/// loopback hub [`crate::agent::hub::origin`] allows). Anything else passes
/// through, so a value already in HTTP form is returned as it is.
pub trait HubOriginExt {
    /// This origin as an HTTP base.
    fn http_base(&self) -> String;
}

impl HubOriginExt for str {
    fn http_base(&self) -> String {
        if let Some(rest) = self.strip_prefix("wss://") {
            format!("https://{rest}")
        } else if let Some(rest) = self.strip_prefix("ws://") {
            format!("http://{rest}")
        } else {
            self.to_string()
        }
    }
}

/// A request that got no answer, or one this side could not use.
fn unreachable(e: HttpError) -> HubError {
    match e {
        HttpError::Network(e) => HubError::Unreachable(e.without_url().to_string()),
        HttpError::Status { status, body, .. } => HubError::Refused(status.as_u16(), body["error"].as_str().unwrap_or_default().to_string()),
        HttpError::Answer(what) => HubError::Refused(0, what),
    }
}

/// This computer as a device of `account` at the hub `origin` (`wss://…`).
pub struct DeviceClient {
    http: reqwest::Client,
    hub: HubUrl,
    /// The account.
    pub account: String,
    /// This computer's id: its device id, its machine id.
    pub device: String,
    /// The device session and when (ms) it expires.
    session: Option<(zeroize::Zeroizing<String>, u64)>,
}

impl DeviceClient {
    /// A client for `device` of `account` at `origin` (no session yet).
    pub fn new(origin: &str, account: &str, device: &str) -> Result<Self, String> {
        // No proxy, as the agent's websocket to the same hub takes none.
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .no_proxy()
            .build()
            .map_err(|e| e.to_string())?;
        crate::agent::traffic::install_client_tap();
        Ok(Self {
            http,
            hub: HubUrl {
                http: origin.http_base(),
                ws: origin.to_string(),
                origin: origin.to_string(),
            },
            account: account.to_string(),
            device: device.to_string(),
            session: None,
        })
    }

    /// The hub this client talks to.
    pub fn origin(&self) -> &str {
        &self.hub.origin
    }

    /// Whether a session is held that does not expire soon.
    pub fn signed_in(&self, now_ms: u64) -> bool {
        self.session.as_ref().is_some_and(|(_, expires)| *expires > now_ms + RENEW_BEFORE_MS)
    }

    async fn send(&self, method: reqwest::Method, path: &str, token: Option<&str>, body: Option<&Value>) -> Result<(u16, Value), HubError> {
        let url = format!("{}{path}", self.hub.http);
        let (status, value) = rh_client::http::request(&self.http, method, &url, token, body)
            .await
            .map_err(unreachable)?;
        Ok((status.as_u16(), value))
    }

    fn judge(status: u16, body: &Value) -> Result<(), HubError> {
        if (200..300).contains(&status) {
            return Ok(());
        }
        let code = body["error"].as_str().unwrap_or_default().to_string();
        Err(match (status, code.as_str()) {
            (401, _) => HubError::Unauthorized,
            (403, "device_removed") => HubError::Removed,
            (403, "host_devices_off") => HubError::Off,
            (403, "not_in_chain") => HubError::NotInChain,
            (403, "rotation_pending") => HubError::Suspended,
            (500..=599, _) => HubError::Unreachable(format!("{status} {code}")),
            _ => HubError::Refused(status, code),
        })
    }

    /// Mint a device session: the hub's challenge, signed by the machine key
    /// as a device session (`HubLogin {role: DeviceSession}`).
    pub async fn sign_in(&mut self, identity: &MachineIdentity, now_ms: u64) -> Result<(), HubError> {
        let minted = rh_client::session::mint(&self.http, &self.hub, &self.account, &self.device, identity.signing_key(), now_ms)
            .await
            .map_err(|e| match e {
                HttpError::Status { status, body, .. } => Self::judge(status.as_u16(), &body)
                    .err()
                    .unwrap_or(HubError::Refused(status.as_u16(), String::new())),
                other => unreachable(other),
            })?;
        let expires = minted.expires_ms.unwrap_or(now_ms + 60 * 60 * 1000);
        self.session = Some((minted.token, expires));
        Ok(())
    }

    fn token(&self) -> Result<String, HubError> {
        self.session.as_ref().map(|(t, _)| t.to_string()).ok_or(HubError::Unauthorized)
    }

    /// A signed-in request; a 401 forgets the session.
    async fn authed(&mut self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value, HubError> {
        let token = zeroize::Zeroizing::new(self.token()?);
        let (status, value) = self.send(method, path, Some(&token), body).await?;
        let judged = Self::judge(status, &value);
        if judged == Err(HubError::Unauthorized) {
            self.session = None;
        }
        judged.map(|()| value)
    }

    /// This device's mail with an id past `after`, oldest first (one page).
    pub async fn mailbox(&mut self, after: i64) -> Result<Vec<Mail>, HubError> {
        let v = self.authed(reqwest::Method::GET, &format!("/v1/mailbox?after={after}"), None).await?;
        let page = v["mail"].as_array().cloned().unwrap_or_default();
        Ok(page
            .iter()
            .filter_map(|m| {
                Some(Mail {
                    id: m["id"].as_i64()?,
                    sender: m["sender"].as_str()?.to_string(),
                    purpose: u8::try_from(m["purpose"].as_u64()?).ok()?,
                    epoch: u32::try_from(m["epoch"].as_u64()?).ok()?,
                    sealed: rh_client::unhex(m["sealed"].as_str()?).ok()?,
                })
            })
            .collect())
    }

    /// Acknowledge (delete) one of this device's mails.
    pub async fn ack(&mut self, id: i64) -> Result<(), HubError> {
        self.authed(reqwest::Method::DELETE, &format!("/v1/mailbox/{id}"), None).await.map(|_| ())
    }

    /// Ask admin `admin` for the hosts' machine keys this device lacks
    /// (purpose 4: no secret, epoch 0). The hub takes it from a device that
    /// is not an admin only to an active admin; it is answered with host
    /// keys (purpose 5), never a user key.
    pub async fn ask(&mut self, admin: &str) -> Result<(), HubError> {
        let body = json!({
            "recipient": admin, "sender": self.device,
            "purpose": rh_proto::wrap::MAIL_KEY_REQUEST, "epoch": 0, "sealed": "",
        });
        self.authed(reqwest::Method::POST, "/v1/mailbox", Some(&body)).await.map(|_| ())
    }

    /// `GET /v1/me`: here, which hosts' agents declared `chain/3` and
    /// `policy/1` (`machines[].reads_v3`).
    pub async fn me(&mut self) -> Result<Value, HubError> {
        self.authed(reqwest::Method::GET, "/v1/me", None).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hub_is_reached_over_https_where_the_agent_dials_wss() {
        assert_eq!("wss://droppen.app".http_base(), "https://droppen.app");
        assert_eq!("ws://127.0.0.1:9000".http_base(), "http://127.0.0.1:9000");
    }

    #[test]
    fn refusals_are_told_apart() {
        let body = |code: &str| json!({"error": code});
        assert_eq!(DeviceClient::judge(204, &Value::Null), Ok(()));
        assert_eq!(DeviceClient::judge(401, &body("denied")), Err(HubError::Unauthorized));
        assert_eq!(DeviceClient::judge(403, &body("device_removed")), Err(HubError::Removed));
        assert_eq!(DeviceClient::judge(403, &body("host_devices_off")), Err(HubError::Off));
        assert_eq!(DeviceClient::judge(403, &body("not_in_chain")), Err(HubError::NotInChain));
        // Host identity renewal: while this computer's renewal is pending
        // its device role is suspended — its own refusal, parked until the
        // chain moves (R6 review), never a removal; a retired key's
        // session is only unauthorized (sync the chain first).
        assert_eq!(DeviceClient::judge(403, &body("rotation_pending")), Err(HubError::Suspended));
        assert_eq!(DeviceClient::judge(401, &body("rotated")), Err(HubError::Unauthorized));
        assert!(matches!(DeviceClient::judge(503, &body("unavailable")), Err(HubError::Unreachable(_))));
        assert_eq!(
            DeviceClient::judge(403, &body("not_an_admin")),
            Err(HubError::Refused(403, "not_an_admin".into()))
        );
    }
}
