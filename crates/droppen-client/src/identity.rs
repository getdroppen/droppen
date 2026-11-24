//! A standalone device's identity: the hub and account it belongs to, the id
//! the hub gave it, and its two keys — Ed25519 signs its sign-ins (a device
//! session, a relay's `login`), X25519 is its Noise static key. Kept in
//! `identity.json` under the device's data directory, `0600`
//! ([`crate::files`]).
//!
//! Made once, when the device joins an account — the a2a proxy's enrollment
//! (`docs/a2a.md` §7.2) is where one comes from — and read on every start.
//! Nothing here decides what the device may do: the chain does
//! ([`crate::verified`]).

use std::path::Path;

use droppen_proto::identity::fingerprint_text;
use droppen_proto::keys::DhKeypair;
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::{hex, key32};

/// The file under the data directory.
pub const IDENTITY_FILE: &str = "identity.json";
const VERSION: u32 = 1;

/// A device's keys and who it is.
pub struct DeviceIdentity {
    /// The hub, as the device was told to use it (`https://…`).
    pub hub: String,
    pub account: String,
    pub device: String,
    sign: SigningKey,
    dh: DhKeypair,
}

impl std::fmt::Debug for DeviceIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceIdentity")
            .field("hub", &self.hub)
            .field("account", &self.account)
            .field("device", &self.device)
            .finish_non_exhaustive()
    }
}

/// The file's shape. The two secrets are hex.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityFile {
    version: u32,
    hub: String,
    account: String,
    device: String,
    sign_seed: String,
    dh_secret: String,
}

impl DeviceIdentity {
    pub fn new(hub: &str, account: &str, device: &str, sign: SigningKey, dh: DhKeypair) -> Self {
        Self {
            hub: hub.into(),
            account: account.into(),
            device: device.into(),
            sign,
            dh,
        }
    }

    pub fn sign_key(&self) -> &SigningKey {
        &self.sign
    }

    pub fn dh(&self) -> &DhKeypair {
        &self.dh
    }

    pub fn sign_public(&self) -> [u8; 32] {
        self.sign.verifying_key().to_bytes()
    }

    pub fn dh_public(&self) -> [u8; 32] {
        self.dh.public()
    }

    /// The signing key's fingerprint as a person reads it (groups of four).
    pub fn fingerprint(&self) -> String {
        fingerprint_text(&self.sign_public())
    }

    /// The identity kept in `dir`, `None` when there is none yet.
    pub fn load(dir: &Path) -> Result<Option<Self>, String> {
        let path = dir.join(IDENTITY_FILE);
        let Some(bytes) = crate::files::read_private(&path)? else {
            return Ok(None);
        };
        let bytes = Zeroizing::new(bytes);
        let file: IdentityFile = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let file = Zeroizing::new(file);
        if file.version != VERSION {
            return Err(format!("{}: unsupported version {}", path.display(), file.version));
        }
        let bad = |what: &str| format!("{}: bad {what}", path.display());
        let seed = Zeroizing::new(key32(&file.sign_seed).map_err(|_| bad("signing key"))?);
        let secret = Zeroizing::new(key32(&file.dh_secret).map_err(|_| bad("key agreement key"))?);
        Ok(Some(Self {
            hub: file.hub.clone(),
            account: file.account.clone(),
            device: file.device.clone(),
            sign: SigningKey::from_bytes(&seed),
            dh: DhKeypair::from_secret(*secret),
        }))
    }

    /// Keep this identity in `dir` (`0600`, atomically).
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let file = Zeroizing::new(IdentityFile {
            version: VERSION,
            hub: self.hub.clone(),
            account: self.account.clone(),
            device: self.device.clone(),
            sign_seed: hex(&self.sign.to_bytes()),
            dh_secret: hex(self.dh.secret()),
        });
        let bytes = Zeroizing::new(serde_json::to_vec_pretty(&*file).map_err(|e| e.to_string())?);
        crate::files::write_private(&dir.join(IDENTITY_FILE), &bytes)
    }
}

impl zeroize::Zeroize for IdentityFile {
    fn zeroize(&mut self) {
        self.sign_seed.zeroize();
        self.dh_secret.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identity_reads_back_as_it_was_kept() {
        let dir = std::env::temp_dir().join(format!("droppen-client-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(DeviceIdentity::load(&dir).unwrap().is_none());
        let id = DeviceIdentity::new(
            "https://hub.example",
            "acct",
            "dev-1",
            SigningKey::from_bytes(&[3; 32]),
            DhKeypair::from_secret([4; 32]),
        );
        id.save(&dir).unwrap();
        let back = DeviceIdentity::load(&dir).unwrap().unwrap();
        assert_eq!(
            (back.hub.as_str(), back.account.as_str(), back.device.as_str()),
            ("https://hub.example", "acct", "dev-1")
        );
        assert_eq!(back.sign_public(), id.sign_public());
        assert_eq!(back.dh_public(), id.dh_public());
        assert!(!format!("{back:?}").contains(&hex(&[3; 32])), "no key in Debug");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
