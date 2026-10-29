//! A device session: the hub's challenge signed with the device's own key
//! (`HubLogin` of role device-session, `docs/hub-api/http.md`). No password
//! and no bearer: the signature is what signs the device in — a browser's
//! device key, the test device's, or the local install's machine key, which
//! is its device key too (`docs/local-aggregator.md` §4.2).

use ed25519_dalek::SigningKey;
use rh_proto::sig::{HubLogin, LoginRole};
use rh_proto::wire::client::{SessionChallengeRequest, SessionRequest};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::http::{post, HttpError};
use crate::{hex, key32, HubUrl};

/// A device session as the hub minted it.
pub struct Minted {
    /// The bearer token (wiped when dropped).
    pub token: Zeroizing<String>,
    /// When (ms) the hub lets it lapse, if it said.
    pub expires_ms: Option<u64>,
}

impl std::fmt::Debug for Minted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Minted").field("expires_ms", &self.expires_ms).finish_non_exhaustive()
    }
}

/// Mint a device session for `device` of `account`, signing at `time_ms`.
pub async fn mint(http: &reqwest::Client, hub: &HubUrl, account: &str, device: &str, sign: &SigningKey, time_ms: u64) -> Result<Minted, HttpError> {
    let v: Value = post(
        http,
        &format!("{}/v1/session/challenge", hub.http),
        None,
        &serde_json::to_value(SessionChallengeRequest {
            account: account.into(),
            device: device.into(),
        })
        .expect("a session challenge request encodes"),
    )
    .await?;
    let nonce = v["nonce"]
        .as_str()
        .and_then(|n| key32(n).ok())
        .ok_or_else(|| HttpError::Answer("no nonce".into()))?;
    let signature = HubLogin {
        role: LoginRole::DeviceSession,
        account: account.into(),
        id: device.into(),
        hub: hub.origin.clone(),
        nonce,
        time_ms,
    }
    .sign(sign)
    .map_err(|e| HttpError::Answer(format!("sign-in: {e}")))?;
    let v: Value = post(
        http,
        &format!("{}/v1/session", hub.http),
        None,
        &serde_json::to_value(SessionRequest {
            account: account.into(),
            device: device.into(),
            time_ms,
            nonce: hex(&nonce),
            signature: hex(&signature),
        })
        .expect("a session request encodes"),
    )
    .await?;
    let token = v["token"].as_str().ok_or_else(|| HttpError::Answer("no device token".into()))?;
    Ok(Minted {
        token: Zeroizing::new(token.to_string()),
        expires_ms: v["expires_ms"].as_u64(),
    })
}
