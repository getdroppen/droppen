//! The signer against the Tauri updater: keys in Tauri's format (as
//! `tauri signer generate` makes them), signatures checked the way
//! tauri-plugin-updater 2.13.2 checks them (`updater.rs`: `verify_signature`,
//! `verify_signed_version`, `signed_version`, `base64_to_string`), with the
//! same crate (minisign-verify). desktop/core's `update.rs` checks a
//! signature this signer made too (its `testdata/update/in-repo-signer.*`).

use super::*;
use base64::Engine;
use minisign::KeyPair;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// A key pair as tauri-cli's `generate_key` makes it: base64 of each box.
fn tauri_key(password: &str) -> (String, String) {
    let KeyPair { pk, sk } = KeyPair::generate_encrypted_keypair(Some(password.to_string())).expect("key pair");
    let pk = B64.encode(pk.to_box().expect("pk box").to_string());
    let sk = B64.encode(sk.to_box(None).expect("sk box").to_string());
    (pk, sk)
}

/// tauri-plugin-updater 2.13.2's check, with `requireSignedVersion` on: the
/// signature over `data` by `pub_key`, then the version it was signed for.
fn updater_verifies(data: &[u8], release_signature: &str, pub_key: &str, announced_version: &str) -> Result<(), String> {
    let to_string = |b64: &str| String::from_utf8(B64.decode(b64).map_err(|e| e.to_string())?).map_err(|e| e.to_string());
    let public_key = minisign_verify::PublicKey::decode(&to_string(pub_key)?).map_err(|e| e.to_string())?;
    let signature = minisign_verify::Signature::decode(&to_string(release_signature)?).map_err(|e| e.to_string())?;
    public_key.verify(data, &signature, true).map_err(|e| e.to_string())?;
    let signed = signature.trusted_comment().split('\t').find_map(|field| field.strip_prefix("version:"));
    match signed {
        None => Err("the signature specifies no version (requireSignedVersion)".to_string()),
        Some(v) if v.trim_start_matches('v') == announced_version.trim_start_matches('v') => Ok(()),
        Some(v) => Err(format!("signed for {v}, announced {announced_version}")),
    }
}

fn archive(dir: &Path, bytes: &[u8]) -> PathBuf {
    let path = dir.join("Droppen.app.tar.gz");
    std::fs::write(&path, bytes).expect("archive");
    path
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("droppen-update-signer-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[test]
fn a_signature_the_tauri_updater_accepts_for_its_version_only() {
    let dir = scratch("accept");
    let (pk, sk) = tauri_key("a password");
    let path = archive(&dir, b"an update archive");
    let key = secret_key(&sk, "a password").expect("the Tauri-format key");
    let (signature, encoded) = sign(&key, &path, "1.2.3", 1_700_000_000).expect("signed");
    assert_eq!(
        signature.trusted_comment().expect("trusted comment"),
        "timestamp:1700000000\tfile:Droppen.app.tar.gz\tversion:1.2.3",
        "tauri-cli's trusted comment, field for field"
    );
    assert_eq!(updater_verifies(b"an update archive", &encoded, &pk, "1.2.3"), Ok(()));
    assert_eq!(updater_verifies(b"an update archive", &encoded, &pk, "v1.2.3"), Ok(()));
    assert!(updater_verifies(b"an update archive", &encoded, &pk, "1.2.4").is_err(), "another version");
    assert!(updater_verifies(b"another archive", &encoded, &pk, "1.2.3").is_err(), "other bytes");
    let (other_pk, _) = tauri_key("another password");
    assert!(updater_verifies(b"an update archive", &encoded, &other_pk, "1.2.3").is_err(), "another key");
    // The .sig file is the base64 of the signature box, as tauri-cli writes it.
    assert_eq!(signature_path(&path), dir.join("Droppen.app.tar.gz.sig"));
    assert_eq!(B64.decode(&encoded).expect("base64"), signature.to_string().into_bytes());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_key_needs_its_password() {
    let (_, sk) = tauri_key("right");
    assert!(secret_key(&sk, "wrong").is_err());
    assert!(secret_key("not base64!", "right").is_err());
    assert!(secret_key(&B64.encode("not a key"), "right").is_err());
}

/// The key of tauri-cli 2.12.3's own tests (`helpers/updater_signature.rs`,
/// "encrypted with an empty string"): a key Tauri made, read as Tauri reads it.
#[test]
fn a_key_tauri_made_signs() {
    const TAURI_TEST_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHJzaWduIGVuY3J5cHRlZCBzZWNyZXQga2V5ClJXUlRZMEl5dkpDN09RZm5GeVAzc2RuYlNzWVVJelJRQnNIV2JUcGVXZUplWXZXYXpqUUFBQkFBQUFBQUFBQUFBQUlBQUFBQTZrN2RnWGh5dURxSzZiL1ZQSDdNcktiaHRxczQwMXdQelRHbjRNcGVlY1BLMTBxR2dpa3I3dDE1UTVDRDE4MXR4WlQwa1BQaXdxKy9UU2J2QmVSNXhOQWFDeG1GSVllbUNpTGJQRkhhTnROR3I5RmdUZi90OGtvaGhJS1ZTcjdZU0NyYzhQWlQ5cGM9Cg==";
    let dir = scratch("tauri-key");
    let key = secret_key(TAURI_TEST_KEY, "").expect("Tauri's test key, empty password");
    let pk = B64.encode(minisign::PublicKey::from_secret_key(&key).expect("pk").to_box().expect("pk box").to_string());
    let path = archive(&dir, b"bytes");
    let (_, encoded) = sign(&key, &path, "0.0.1", 1).expect("signed");
    assert_eq!(updater_verifies(b"bytes", &encoded, &pk, "0.0.1"), Ok(()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_tab_or_a_newline_is_refused() {
    assert!(trusted_comment("Droppen.app.tar.gz", "1.2.3\tversion:9", 1).is_err());
    assert!(trusted_comment("a\nb", "1.2.3", 1).is_err());
    assert!(trusted_comment("Droppen.app.tar.gz", "", 1).is_err());
}
