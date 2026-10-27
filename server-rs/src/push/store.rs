//! `<data>/instances/<name>/push.json`: the VAPID keypair and every subscription.
//!
//! Instance-scoped rather than shared, unlike most of remote-hand's sidecars: a
//! subscription is a permission one browser granted to one *origin*, and the
//! VAPID key is that origin's identity to the push service. Two servers sharing
//! the file would each sign with the other's key and deliver to the other's
//! devices. See [`crate::instance`], which also copies a pre-instance file in
//! once so an upgrade keeps its devices.
//!
//! Stored `0600`, because it holds a private key — the same rule (and the same
//! reasoning) as [`crate::accounts::store`]. The mode is now the database's; see
//! "Where the file actually is" below.
//!
//! What the private key is worth is worth being precise about. It is not a
//! secret about the *user*: it signs `Authorization` headers to push services,
//! and losing it means someone else can send notifications that appear to come
//! from this server to subscriptions they already hold. It never leaves the
//! file — [`vapid_public`] hands out the public half and nothing else — and it
//! is generated on first use rather than at startup, so an install that never
//! turns notifications on never has one.
//!
//! Subscriptions are keyed by `endpoint`, which is what makes a re-subscribe
//! idempotent: browsers hand back the same endpoint until they rotate it, and a
//! rotated one leaves the old row to be reaped by the first `404`/`410` the send
//! path sees.
//!
//! # Where the file actually is
//!
//! In [`crate::db`], and split the way the two halves of `push.json` deserve:
//! the subscriptions are rows in `push_subscriptions` keyed by that same
//! endpoint, so a re-subscribe is an `UPSERT` rather than a rewrite of every
//! device; the VAPID private scalar is one string, so it is a `kv` row under
//! [`scope::PUSH`].
//!
//! `push.json` was written for a while after the move — a [`crate::supervisor`]
//! demotion to a `.prev` binary that had never heard of the database would
//! otherwise have come up with no devices and no key, and a fresh key
//! unsubscribes every device there is. No binary that old is left to demote to,
//! so nothing reads the file any more.
//!
//! The `0600` rule moves with the key rather than lapsing: the database holds
//! the same scalar and is kept at `0600` for exactly this class of value (see
//! `Db::keep_to_ourselves`), so there is no longer a plaintext copy of it beside
//! the database at all.

use p256::ecdsa::SigningKey;
use serde::{Deserialize, Serialize};

use super::ece;
use crate::db::{scope, Db};
use crate::sessions::{iso8601, Paths};

/// Serializes the one check-then-write left in this module: [`vapid_key`] looks
/// for a key, finds none, and mints one, and the look and the mint cannot be a
/// single transaction because generating the scalar happens between them. Two
/// callers racing there would mint two keys, one of which wins — and every
/// subscription minted against the loser is a device that can never be delivered
/// to again.
///
/// [`upsert`] and [`remove`] do not take it and no longer need to: each is one
/// transaction, and the read-modify-write in `upsert` happens *inside* it. They
/// held it back when every change was followed by a whole-file rewrite of
/// `push.json`, which two tasks could interleave.
static WRITE_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// One browser's push subscription, exactly as `PushManager.subscribe` produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    /// The push service URL to `POST` to. Unique; the upsert key.
    pub endpoint: String,
    /// The subscription's public key, base64url of an uncompressed P-256 point.
    pub p256dh: String,
    /// The subscription's auth secret, base64url of 16 bytes.
    pub auth: String,
    /// When it was first stored, ISO-8601.
    pub created_at: String,
    /// Whatever the client called this device, for a future management UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl Subscription {
    /// Whether the two keys are the shapes RFC 8291 requires.
    ///
    /// Checked when a subscription arrives rather than when one is sent to: a
    /// malformed row would otherwise sit in the file failing every notification
    /// forever, and the browser that sent it would never hear about it.
    ///
    /// The point is validated all the way onto the curve, not just its length
    /// and `0x04` tag: an off-curve point is the exact shape [`ece::encrypt`]
    /// rejects at send time (`p256::PublicKey::from_sec1_bytes`), so accepting one
    /// here would store a row that fails every notification — the case this check
    /// exists to prevent.
    pub fn keys_are_well_formed(&self) -> bool {
        let point_ok = ece::b64url_decode(&self.p256dh)
            .is_some_and(|k| k.len() == ece::POINT_LEN && k[0] == 0x04 && p256::PublicKey::from_sec1_bytes(&k).is_ok());
        let auth_ok = ece::b64url_decode(&self.auth).is_some_and(|a| a.len() == ece::AUTH_SECRET_LEN);
        point_ok && auth_ok
    }
}

/// The whole file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushFile {
    /// The VAPID private scalar, base64url. Absent until the first caller needs one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vapid_private: Option<String>,
    /// Every subscription, in the order they were first seen.
    #[serde(default)]
    pub subscriptions: Vec<Subscription>,
}

/// The `kv` key the VAPID private scalar lives under, inside [`scope::PUSH`].
///
/// Spelled as the JSON field was, so a person holding `sqlite3` and a memory of
/// `push.json` finds the same word in both places.
const VAPID_PRIVATE: &str = "vapidPrivate";

/// A subscription as the `payload` column holds it: the JSON it has always been.
fn encode(subscription: &Subscription) -> String {
    serde_json::to_string(subscription).unwrap_or_else(|_| "{}".to_string())
}

/// Everything the database holds about push, in the shape the file had.
///
/// `ORDER BY rowid` is what keeps "the order they were first seen": the table
/// has no ordinal of its own, and an `UPSERT` leaves a row's rowid alone, so a
/// browser re-subscribing under the same endpoint does not jump to the end of
/// the list. A row that will not parse is dropped rather than fatal.
fn current(db: &Db) -> PushFile {
    PushFile {
        vapid_private: db.kv_get(scope::PUSH, VAPID_PRIVATE),
        subscriptions: db
            .read(|conn| {
                let mut statement = conn.prepare("SELECT payload FROM push_subscriptions ORDER BY rowid")?;
                let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
                Ok(rows
                    .filter_map(Result::ok)
                    .filter_map(|payload| serde_json::from_str::<Subscription>(&payload).ok())
                    .collect())
            })
            .unwrap_or_default(),
    }
}

/// Everything stored, as the file used to hold it.
pub async fn read(paths: &Paths) -> PushFile {
    current(&crate::db::at(paths.instance_db()))
}

/// The VAPID signing key, generating and persisting one on first use.
///
/// A file whose `vapidPrivate` will not parse is an error rather than a silent
/// regeneration: rotating the key would invalidate every subscription that was
/// minted against the old one, and doing that because of a stray edit would be a
/// surprising way to lose every device at once.
pub async fn vapid_key(paths: &Paths) -> std::io::Result<SigningKey> {
    let _guard = WRITE_LOCK.lock().await;
    let db = crate::db::at(paths.instance_db());
    if let Some(encoded) = db.kv_get(scope::PUSH, VAPID_PRIVATE) {
        return ece::b64url_decode(&encoded)
            .and_then(|bytes| SigningKey::from_slice(&bytes).ok())
            .ok_or_else(|| std::io::Error::other(format!("{} holds a vapidPrivate that is not a P-256 scalar", db.path().display())));
    }
    let key = SigningKey::random(&mut rand::rngs::OsRng);
    // Written through `Db::write` rather than `Db::kv_set`, which logs and
    // carries on: a key that was minted but not stored is a *different* key on
    // the next call, and the subscriptions minted against the first one would
    // never be delivered to again. The caller has to hear about that.
    db.write(|tx| {
        tx.execute(
            "INSERT INTO kv (scope, key, value) VALUES (?1, ?2, ?3)
             ON CONFLICT (scope, key) DO UPDATE SET value = excluded.value, updated_at = unixepoch()",
            (scope::PUSH, VAPID_PRIVATE, ece::b64url(&key.to_bytes())),
        )?;
        Ok(())
    })
    .map_err(std::io::Error::other)?;
    Ok(key)
}

/// The `applicationServerKey` a browser subscribes with, minting the keypair if
/// this is the first anyone has asked.
pub async fn vapid_public(paths: &Paths) -> std::io::Result<String> {
    Ok(ece::vapid_public_b64(&vapid_key(paths).await?))
}

/// Store a subscription, replacing any earlier one with the same endpoint.
pub async fn upsert(paths: &Paths, endpoint: String, p256dh: String, auth: String, label: Option<String>) -> std::io::Result<()> {
    let db = crate::db::at(paths.instance_db());
    let now = iso8601(std::time::SystemTime::now());
    db.write(|tx| {
        let stored: Option<Subscription> = tx
            .query_row("SELECT payload FROM push_subscriptions WHERE endpoint = ?1", (&endpoint,), |row| {
                row.get::<_, String>(0)
            })
            .ok()
            .and_then(|payload| serde_json::from_str(&payload).ok());
        let row = match stored {
            // The keys may have been re-minted under the same endpoint;
            // `createdAt` is when this device first appeared and stays put. A
            // row that will not parse is not one of those: it is treated as a
            // device arriving for the first time, which is the tolerant read
            // this store has always had.
            Some(mut existing) => {
                // The key is the row's, never the payload's. A payload whose
                // `endpoint` had drifted from the key it is stored under would
                // otherwise be handed to the send path naming a device the table
                // cannot find, and its `410` reaper would never match it.
                existing.endpoint = endpoint.clone();
                existing.p256dh = p256dh;
                existing.auth = auth;
                if label.is_some() {
                    existing.label = label;
                }
                existing
            }
            None => Subscription {
                endpoint: endpoint.clone(),
                p256dh,
                auth,
                created_at: now,
                label,
            },
        };
        // `DO UPDATE` rather than `INSERT OR REPLACE`, because replacing would
        // give the row a new rowid and move a re-subscribing device to the end
        // of a list whose order is documented as "first seen".
        tx.execute(
            "INSERT INTO push_subscriptions (endpoint, payload) VALUES (?1, ?2)
             ON CONFLICT (endpoint) DO UPDATE SET payload = excluded.payload",
            (&endpoint, encode(&row)),
        )?;
        Ok(())
    })
    .map_err(std::io::Error::other)?;
    Ok(())
}

/// Forget a subscription. `Ok(false)` when there was nothing to forget, which is
/// not an error — an unsubscribe is allowed to be idempotent.
pub async fn remove(paths: &Paths, endpoint: &str) -> std::io::Result<bool> {
    let db = crate::db::at(paths.instance_db());
    let removed = db
        .write(|tx| tx.execute("DELETE FROM push_subscriptions WHERE endpoint = ?1", (endpoint,)))
        .map_err(std::io::Error::other)?;
    Ok(removed > 0)
}

/// Every stored subscription.
pub async fn subscriptions(paths: &Paths) -> Vec<Subscription> {
    read(paths).await.subscriptions
}

/// A new VAPID keypair, and every stored subscription forgotten — in one
/// transaction — once this computer renewed its identity key
/// (`docs/host-identity-rotation.md` §2.2, R6 review): whoever copied the
/// old VAPID key and the endpoints can no longer reach a device that
/// subscribes again, against the new key (`GET /api/push/vapid`). The one
/// rotation [`vapid_key`] refuses to do by accident. An install that never
/// had a key still has none (it is made on first use). How many
/// subscriptions were forgotten.
pub async fn renew_vapid(paths: &Paths) -> std::io::Result<usize> {
    let _guard = WRITE_LOCK.lock().await;
    let db = crate::db::at(paths.instance_db());
    let key = SigningKey::random(&mut rand::rngs::OsRng);
    db.write(|tx| {
        let forgotten = tx.execute("DELETE FROM push_subscriptions", [])?;
        tx.execute(
            "UPDATE kv SET value = ?3, updated_at = unixepoch() WHERE scope = ?1 AND key = ?2",
            (scope::PUSH, VAPID_PRIVATE, ece::b64url(&key.to_bytes())),
        )?;
        Ok(forgotten)
    })
    .map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        (dir, paths)
    }

    fn valid_keys() -> (String, String) {
        let key = p256::SecretKey::random(&mut rand::rngs::OsRng);
        use p256::elliptic_curve::sec1::ToEncodedPoint;
        (
            ece::b64url(key.public_key().to_encoded_point(false).as_bytes()),
            ece::b64url(&[9u8; ece::AUTH_SECRET_LEN]),
        )
    }

    #[tokio::test]
    async fn a_subscription_round_trips_and_upserts_by_endpoint() {
        let (_dir, paths) = paths();
        let (p256dh, auth) = valid_keys();
        assert!(subscriptions(&paths).await.is_empty(), "a missing file has no subscriptions");

        upsert(
            &paths,
            "https://push.example/a".into(),
            p256dh.clone(),
            auth.clone(),
            Some("phone".into()),
        )
        .await
        .expect("upsert");
        let stored = subscriptions(&paths).await;
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].endpoint, "https://push.example/a");
        assert_eq!(stored[0].label.as_deref(), Some("phone"));
        assert!(stored[0].keys_are_well_formed());
        let first_seen = stored[0].created_at.clone();

        // The browser re-subscribed: same endpoint, fresh keys.
        let (new_p256dh, new_auth) = valid_keys();
        upsert(&paths, "https://push.example/a".into(), new_p256dh.clone(), new_auth, None)
            .await
            .expect("re-upsert");
        let stored = subscriptions(&paths).await;
        assert_eq!(stored.len(), 1, "keyed by endpoint, so this replaced rather than added");
        assert_eq!(stored[0].p256dh, new_p256dh, "the newer keys win");
        assert_eq!(stored[0].created_at, first_seen, "but the device is still the same device");
        assert_eq!(stored[0].label.as_deref(), Some("phone"), "and an absent label does not erase one");

        upsert(&paths, "https://push.example/b".into(), p256dh, auth, None).await.expect("second");
        assert_eq!(subscriptions(&paths).await.len(), 2, "a different endpoint is a different device");
    }

    #[tokio::test]
    async fn removing_is_idempotent() {
        let (_dir, paths) = paths();
        let (p256dh, auth) = valid_keys();
        upsert(&paths, "https://push.example/a".into(), p256dh, auth, None).await.expect("upsert");

        assert!(remove(&paths, "https://push.example/a").await.expect("remove"));
        assert!(subscriptions(&paths).await.is_empty());
        assert!(
            !remove(&paths, "https://push.example/a").await.expect("remove again"),
            "nothing to forget is not a failure"
        );
    }

    #[tokio::test]
    async fn the_vapid_key_is_minted_once_and_then_reused() {
        let (_dir, paths) = paths();
        assert!(read(&paths).await.vapid_private.is_none(), "no key until someone asks for one");

        let first = vapid_public(&paths).await.expect("mint");
        let again = vapid_public(&paths).await.expect("reuse");
        assert_eq!(first, again, "a rotated key would invalidate every subscription");
        assert_eq!(ece::b64url_decode(&first).expect("decode").len(), ece::POINT_LEN);

        // The private half stays in the store and never appears in what is handed out.
        let stored = read(&paths).await.vapid_private.expect("stored");
        assert_ne!(stored, first);
    }

    /// R6 review: once the identity key is renewed, the VAPID key is new
    /// and every subscription made against the old one is gone — a copy of
    /// the old key and endpoints is useless once the pages subscribe again.
    #[tokio::test]
    async fn renewing_the_vapid_key_forgets_every_subscription() {
        let (_dir, paths) = paths();
        // An install that never turned notifications on still has no key.
        assert_eq!(renew_vapid(&paths).await.expect("renew"), 0);
        assert!(read(&paths).await.vapid_private.is_none());
        let old = vapid_public(&paths).await.expect("mint");
        for endpoint in ["https://push.example/a", "https://push.example/b"] {
            let (p256dh, auth) = valid_keys();
            upsert(&paths, endpoint.into(), p256dh, auth, None).await.expect("upsert");
        }
        assert_eq!(renew_vapid(&paths).await.expect("renew"), 2);
        let new = vapid_public(&paths).await.expect("the new key");
        assert_ne!(new, old, "a new keypair");
        assert_eq!(vapid_public(&paths).await.expect("reuse"), new, "kept from then on");
        assert!(subscriptions(&paths).await.is_empty(), "every subscription forgotten");
        // A page subscribing again against the new key is kept as before.
        let (p256dh, auth) = valid_keys();
        upsert(&paths, "https://push.example/a".into(), p256dh, auth, None).await.expect("again");
        assert_eq!(subscriptions(&paths).await.len(), 1);
    }

    #[tokio::test]
    async fn a_corrupt_private_key_is_an_error_not_a_silent_rotation() {
        let (_dir, paths) = paths();
        crate::db::at(paths.instance_db()).kv_set(scope::PUSH, VAPID_PRIVATE, "not-a-scalar");
        assert!(vapid_key(&paths).await.is_err(), "regenerating here would drop every device");
    }

    /// Minting a key writes the database and nothing else.
    ///
    /// Two halves, and the second is why the first is safe. Nothing creates
    /// `push.json` any more — so there is no second, plaintext copy of the VAPID
    /// scalar on disk to keep at `0600` — and the store that did absorb it is
    /// itself unreadable to anyone else, WAL sidecar included, which is where
    /// the rule this module has always had now lives.
    #[tokio::test]
    #[cfg(unix)]
    async fn a_mint_writes_no_file_and_leaves_the_store_unreadable() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, paths) = paths();
        // Pin the umask so this asserts on what the code did rather than on
        // whatever the machine running the tests happens to default to.
        // SAFETY: `umask` is always safe to call; the previous value is restored.
        let previous = unsafe { libc::umask(0o022) };
        let result = vapid_public(&paths).await;
        // SAFETY: as above.
        unsafe { libc::umask(previous) };
        result.expect("mint");

        assert!(!paths.push_file().exists(), "no key is written out beside the database");
        assert!(!paths.push_file().with_extension("json.tmp").exists());
        let mode = std::fs::metadata(paths.instance_db()).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    /// Nothing writes a `push.json` back: the database is the only copy of the
    /// key and the devices, and a file left over from an older build is inert.
    #[tokio::test]
    async fn no_change_writes_the_file_back() {
        let (_dir, paths) = paths();
        let (p256dh, auth) = valid_keys();
        std::fs::create_dir_all(paths.push_file().parent().expect("parent")).expect("instance dir");
        let before = r#"{"vapidPrivate":null,"subscriptions":[]}"#;
        std::fs::write(paths.push_file(), before).expect("seed");

        vapid_public(&paths).await.expect("mint");
        upsert(&paths, "https://push.example/a".into(), p256dh, auth, Some("phone".into()))
            .await
            .expect("upsert");
        assert!(remove(&paths, "https://push.example/a").await.expect("remove"));

        assert_eq!(
            std::fs::read_to_string(paths.push_file()).expect("read"),
            before,
            "a mint, an upsert and a removal leave the file byte for byte as found"
        );
        assert!(!paths.push_file().with_extension("json.tmp").exists());
    }

    /// What a restart is, from the store's point of view. The key matters most:
    /// a VAPID scalar that did not survive one would unsubscribe every device.
    #[tokio::test]
    async fn a_key_and_a_device_survive_the_database_being_reopened() {
        let (_dir, paths) = paths();
        let (p256dh, auth) = valid_keys();
        let minted = vapid_public(&paths).await.expect("mint");
        upsert(&paths, "https://push.example/a".into(), p256dh, auth, None).await.expect("upsert");

        let reopened = Db::open(paths.instance_db());
        assert_eq!(
            reopened.kv_get(scope::PUSH, VAPID_PRIVATE),
            read(&paths).await.vapid_private,
            "the private half is in the store anyone reopening finds"
        );
        let endpoint: String = reopened
            .read(|conn| conn.query_row("SELECT endpoint FROM push_subscriptions", [], |row| row.get(0)))
            .expect("the row is there too");
        assert_eq!(endpoint, "https://push.example/a");
        assert_eq!(vapid_public(&paths).await.expect("reuse"), minted, "and it is still the same key");
    }

    #[test]
    fn a_malformed_or_off_curve_key_is_caught_before_it_is_stored() {
        use p256::elliptic_curve::sec1::ToEncodedPoint;
        let real = p256::SecretKey::random(&mut rand::rngs::OsRng);
        let on_curve = || ece::b64url(real.public_key().to_encoded_point(false).as_bytes());

        let mut sub = Subscription {
            endpoint: "https://push.example/a".into(),
            p256dh: on_curve(),
            auth: ece::b64url(&[9u8; ece::AUTH_SECRET_LEN]),
            created_at: "2026-01-01T00:00:00.000Z".into(),
            label: None,
        };
        assert!(sub.keys_are_well_formed(), "a real uncompressed point on the curve is accepted");

        // The shape is right — 65 bytes with the `0x04` tag — but the coordinates
        // are not a curve point. This is the row that would fail every send.
        sub.p256dh = ece::b64url(&[0x04; ece::POINT_LEN]);
        assert!(
            !sub.keys_are_well_formed(),
            "an off-curve point is refused, not stored to fail every send"
        );

        sub.p256dh = ece::b64url(&[0x02; ece::POINT_LEN]);
        assert!(!sub.keys_are_well_formed(), "a compressed point is not what RFC 8291 asks for");

        sub.p256dh = on_curve();
        sub.auth = ece::b64url(&[9u8; 8]);
        assert!(!sub.keys_are_well_formed(), "the auth secret is exactly 16 bytes");

        sub.auth = "!!!".into();
        assert!(!sub.keys_are_well_formed(), "and it has to be base64url at all");
    }
}
