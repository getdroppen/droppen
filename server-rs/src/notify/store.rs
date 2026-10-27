//! Where an agent's nudge is sent, and the secret the injected script proves
//! itself with.
//!
//! Four rows in the machine-wide state database ([`crate::db`]) under
//! [`scope::NOTIFY`], keyed by the names `notify.json` gave them.
//!
//! Machine-global rather than instance-scoped, and that is the one decision in
//! this file worth arguing about. [`crate::push::store`] is per-instance because
//! a Web Push subscription is a permission one browser granted to one *origin*,
//! so two servers sharing it would sign with each other's keys. Nothing here is
//! origin-bound: the endpoint names the user's phone, which is the same phone
//! whichever instance an agent happens to be running under. Configuring it once
//! and having every instance find it is the behaviour the user expects, so it
//! sits in the machine-wide database beside [`crate::chats::provider`]'s rows.
//! That also ends the race the two servers had over the file, which no mutex in
//! either process could have arbitrated.
//!
//! `notify.json` was a mirror for one version, `0600` and byte for byte as
//! before, so that a `.prev` binary which had never heard of this database would
//! still find the endpoint and the secret — a regenerated secret refuses every
//! session token already in flight. The rollback floor has since risen to a
//! build that reads the database, and a second copy of a signing secret is not a
//! thing to keep on disk for a reason that no longer exists. Nothing reads the
//! file any more.
//!
//! Two of the three configured fields are secrets, which is why the one copy
//! that is left — the database — is `0600` (see [`crate::db::Db`]), the same
//! rule and the same reasoning as [`crate::push::store`] and
//! [`crate::accounts::store`]. They are secrets for different reasons:
//!
//! * **`endpoint`** ends in a Bark device key. Whoever holds it can put a
//!   notification on the user's lock screen, and that is the whole of the
//!   authorization Bark has. It is the reason this module exists at all: the key
//!   must never reach a skill file, a session's environment or a transcript,
//!   because every one of those is readable by the agent it would be handed to.
//! * **`secret`** is what [`super::token`] mints session tokens against. Anyone
//!   who reads it can forge a nudge from any session on the machine.
//!
//! Neither ever leaves the server: [`StoredNotify::info`] is the only view a
//! route may return, and it carries an *origin* and two booleans.

use std::sync::Arc;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::NotifyConfigInfo;
use crate::db::{scope, Db};
use crate::sessions::Paths;

/// Serializes read-modify-write across every caller in the process, for the same
/// reason [`crate::push::store`]'s does: minting the secret is a read, a change
/// and a write, and two of those interleaved would leave one of them minting a
/// secret the other has already handed out tokens against. It outlived the JSON
/// mirror it was also ordering, because that read-modify-write is still here.
///
/// Still a process lock, and still not enough on its own — the other server on
/// this machine has its own copy of it. What makes the pair safe is that the
/// change itself is now one transaction rather than a read of one file and a
/// rename over another.
static WRITE_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// How many bytes of randomness the token secret has.
const SECRET_BYTES: usize = 32;

/// Everything configured, secrets included. Never serialized to a client.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredNotify {
    /// Whether an agent may be told this channel exists at all.
    ///
    /// Separate from "is there an endpoint" so a user can silence their phone
    /// for an afternoon without pasting the device key back in afterwards.
    #[serde(default)]
    pub enabled: bool,
    /// Where a nudge is POSTed: `https://api.day.app/<device key>`, or the same
    /// shape against a self-hosted bark-server. Empty until configured.
    #[serde(default)]
    pub endpoint: String,
    /// How a *phone* reaches this server, for the link a notification carries.
    ///
    /// Not derivable here. The server knows the address it bound, which behind a
    /// tunnel, a reverse proxy or a Tailscale name is not the address the phone
    /// would open. Empty means notifications carry no link, which is a smaller
    /// failure than a link that goes nowhere.
    #[serde(default)]
    pub base_url: String,
    /// base64url of 32 random bytes; minted on first use.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secret: String,
}

impl StoredNotify {
    /// Whether a nudge sent right now would go anywhere.
    ///
    /// Both halves matter, and they are asked as one question everywhere: the
    /// skill is only injected when this is true, so an agent is never handed a
    /// tool whose only possible outcome is a refusal.
    pub fn is_live(&self) -> bool {
        self.enabled && !self.endpoint.is_empty()
    }

    /// The redacted view, and the only thing a route may return.
    pub fn info(&self) -> NotifyConfigInfo {
        NotifyConfigInfo {
            enabled: self.enabled,
            endpoint_mask: super::bark::endpoint_mask(&self.endpoint),
            has_endpoint: !self.endpoint.is_empty(),
            base_url: self.base_url.clone(),
        }
    }
}

/// The one row whose value is not the string it stands for. `STRICT` makes
/// every column here TEXT, so the document's one boolean is spelled the way it
/// is on the wire.
const ENABLED: &str = "enabled";

/// The machine-wide database the channel lives in.
fn store(paths: &Paths) -> Arc<Db> {
    crate::db::at(paths.config_db())
}

/// Map a parsed document onto the store's shape.
///
/// Anything unusable reads as "nothing configured" rather than as an error — the
/// settings panel has to stay usable so the user can fix whatever went wrong,
/// exactly as [`crate::chats::provider::read_provider`] argues. Per-field
/// coercion for the same reason: a number where a string belongs degrades to an
/// empty field rather than throwing the other three away.
fn from_value(raw: &Value) -> StoredNotify {
    let string = |key: &str| raw.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    StoredNotify {
        enabled: raw.get(ENABLED).and_then(Value::as_bool).unwrap_or_default(),
        endpoint: string("endpoint"),
        base_url: string("baseUrl"),
        secret: string("secret"),
    }
}

/// Everything configured; an empty store reads as nothing configured.
///
/// The rows are reassembled into the document [`from_value`] already knows how
/// to read, so a hand-edited row cannot come to mean something else.
pub async fn read(paths: &Paths) -> StoredNotify {
    let rows = store(paths).kv_scope(scope::NOTIFY);
    let raw = Value::Object(
        rows.into_iter()
            .map(|(key, value)| {
                let value = if key == ENABLED {
                    Value::Bool(value == "true")
                } else {
                    Value::String(value)
                };
                (key, value)
            })
            .collect(),
    );
    from_value(&raw)
}

/// Commit `data`.
///
/// The four rows land together, so there is no moment at which the endpoint has
/// been changed and the secret it is signed with has not. Still `io::Result`
/// because [`update`], [`secret`] and the routes above them are built on it;
/// there is no I/O left in here that can fail, so the result is unconditionally
/// `Ok` — which is the right answer anyway, since a caller told this failed
/// would go on to mint a second secret over the first.
pub async fn write(paths: &Paths, data: &StoredNotify) -> std::io::Result<()> {
    let enabled = data.enabled.to_string();
    store(paths).kv_replace_scope(
        scope::NOTIFY,
        [
            (ENABLED, enabled.as_str()),
            ("endpoint", data.endpoint.as_str()),
            ("baseUrl", data.base_url.as_str()),
            ("secret", data.secret.as_str()),
        ],
    );
    Ok(())
}

/// Apply a patch under the write lock, and hand back what the store now says.
///
/// Read-modify-write rather than a whole-document `PUT` because the endpoint is
/// write-only from the client's side: a settings page that has never been shown
/// the device key cannot send it back, so a `PUT` of everything it *can* see
/// would erase the one field that matters.
pub async fn update(paths: &Paths, patch: impl FnOnce(&mut StoredNotify)) -> std::io::Result<StoredNotify> {
    let _guard = WRITE_LOCK.lock().await;
    let mut data = read(paths).await;
    patch(&mut data);
    write(paths, &data).await?;
    Ok(data)
}

/// The secret session tokens are minted against, generating one on first use.
///
/// Minted lazily, like [`crate::push::store::vapid_key`]: an install that never
/// turns notifications on never has one. Never regenerated on a value that will
/// not decode, for the counterpart of that function's reason — every token handed to
/// a running session was minted against the stored bytes, and rotating them
/// because of a stray edit would silently break every session on the machine.
pub async fn secret(paths: &Paths) -> std::io::Result<Vec<u8>> {
    let _guard = WRITE_LOCK.lock().await;
    let mut data = read(paths).await;
    if !data.secret.is_empty() {
        return base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&data.secret)
            .map_err(|_| std::io::Error::other(format!("{} holds a secret that is not base64url", paths.config_db().display())));
    }
    let mut bytes = [0u8; SECRET_BYTES];
    rand::Rng::fill(&mut rand::rngs::OsRng, &mut bytes[..]);
    data.secret = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    write(paths, &data).await?;
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        (dir, paths)
    }

    #[tokio::test]
    async fn the_configuration_round_trips_and_patches_field_by_field() {
        let (_dir, paths) = paths();
        assert_eq!(read(&paths).await, StoredNotify::default(), "a missing file is nothing configured");

        let stored = update(&paths, |data| {
            data.endpoint = "https://api.day.app/FAKEBARKDEVICEKEY0Wc31".into();
            data.enabled = true;
        })
        .await
        .expect("update");
        assert!(stored.is_live());

        // The client can see there is an endpoint and where it points, and
        // nothing else: the device key is the whole of Bark's authorization.
        let info = stored.info();
        assert_eq!(info.endpoint_mask, "https://api.day.app/••••Wc31");
        assert!(info.has_endpoint);
        assert!(!serde_json::to_string(&info).expect("json").contains("FAKEBARKDEVICEKEY0Wc31"));

        let stored = update(&paths, |data| data.enabled = false).await.expect("disable");
        assert_eq!(
            stored.endpoint, "https://api.day.app/FAKEBARKDEVICEKEY0Wc31",
            "silencing the phone does not forget it"
        );
        assert!(!stored.is_live());
    }

    #[tokio::test]
    async fn the_secret_is_minted_once_and_then_reused() {
        let (_dir, paths) = paths();
        assert!(read(&paths).await.secret.is_empty(), "no secret until someone asks for one");

        let first = secret(&paths).await.expect("mint");
        assert_eq!(first.len(), SECRET_BYTES);
        assert_eq!(secret(&paths).await.expect("reuse"), first, "rotating it would break every live session");

        // Minting must not disturb what the user configured.
        update(&paths, |data| data.endpoint = "https://api.day.app/FAKEBARKDEVICEKEY0Wc31".into())
            .await
            .expect("update");
        assert_eq!(secret(&paths).await.expect("still there"), first);
        assert_eq!(read(&paths).await.endpoint, "https://api.day.app/FAKEBARKDEVICEKEY0Wc31");
    }

    /// A secret that will not decode is carried, not replaced: it is what every
    /// token already handed to a running session was minted against.
    #[tokio::test]
    async fn a_corrupt_secret_is_an_error_not_a_silent_rotation() {
        let (_dir, paths) = paths();
        crate::db::at(paths.config_db()).kv_set(scope::NOTIFY, "secret", "not base64url!!");
        assert!(secret(&paths).await.is_err(), "re-minting here would silently refuse every live session");
    }

    #[tokio::test]
    async fn a_malformed_document_reads_as_nothing_configured() {
        assert_eq!(from_value(&Value::Null), StoredNotify::default());
        assert_eq!(from_value(&serde_json::json!([1, 2])), StoredNotify::default());

        // And a field of the wrong type costs only itself.
        let data = from_value(&serde_json::json!({ "enabled": true, "endpoint": 7, "baseUrl": "https://rh.example" }));
        assert!(data.enabled);
        assert_eq!(data.endpoint, "");
        assert_eq!(data.base_url, "https://rh.example");
    }

    #[tokio::test]
    async fn the_configuration_is_still_there_through_a_fresh_connection() {
        let (_dir, paths) = paths();
        update(&paths, |data| {
            data.endpoint = "https://api.day.app/FAKEBARKDEVICEKEY0Wc31".into();
            data.enabled = true;
        })
        .await
        .expect("update");
        let minted = secret(&paths).await.expect("mint");

        let reopened = crate::db::Db::open(paths.config_db());
        let rows = reopened.kv_scope(scope::NOTIFY);
        assert_eq!(rows.get(ENABLED).map(String::as_str), Some("true"), "the one boolean is spelled out");
        assert_eq!(
            rows.get("endpoint").map(String::as_str),
            Some("https://api.day.app/FAKEBARKDEVICEKEY0Wc31")
        );
        assert_eq!(
            rows.get("secret").map(String::as_str),
            Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&minted).as_str()),
            "the secret every live token was minted against survives the connection that minted it"
        );
    }

    /// The inverse of the test that used to pin the mirror's `0600` mode:
    /// there is no second copy of the device key and the signing secret on disk
    /// to get the mode of wrong. The one copy left is the database, which is
    /// `0600` for the reason the file was.
    #[tokio::test]
    async fn configuring_a_channel_writes_no_file() {
        let (_dir, paths) = paths();
        update(&paths, |data| data.endpoint = "https://api.day.app/FAKEBARKDEVICEKEY0Wc31".into())
            .await
            .expect("update");
        secret(&paths).await.expect("mint");

        let file = paths.config_dir().join("notify.json");
        assert!(!file.exists(), "no file is created where there was none");
        assert!(!file.with_extension("json.tmp").exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(paths.config_db()).expect("stat").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the database holds the secrets now, so it is as private as the file was");
        }
    }

}
