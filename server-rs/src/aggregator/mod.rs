//! The local aggregator (local-aggregator §5, option A): this computer
//! controlling the account's other devices with its one identity — the one
//! it runs AI with (§3.3) — and its directory, `<instance>/aggregator/`
//! (§5.5).
//!
//! - [`controller`] (S1-4) runs inside the hub link: it signs in to the hub
//!   as a device, takes the other hosts' machine keys an admin sealed for it
//!   and asks the admins for the ones it lacks; they go to [`keys`]
//!   (`aggregator/keys.json`).
//! - [`copies`] (S1-5) runs beside it: with the keys it hands over, it
//!   follows the other hosts' index, status and newest sessions ([`access`]
//!   says which) over its own `/v1/client` connection, and keeps them in
//!   [`store`] (`aggregator/copies.db`). S2 adds the channels.
//! - [`gateway`] (S1-7) serves them to this computer's own pages —
//!   `GET /api/agg/ws`, only to the local accounts that may see other
//!   devices — from memory ([`live`]): the copies task publishes every copy
//!   it follows there, and follows the sessions a page opens.
//! - Before chain v3 a sign-in with aggregation on prepared a second
//!   identity here, a *limited device* (`aggregator/device-key.json`). Chain
//!   v3 made it unnecessary; one an older release left pending is deleted
//!   ([`forget`]) when an attempt ends. [`keys_from_seed`] stays for reading
//!   such a leftover key in tests.
//! - Turning the aggregator off, disconnecting the computer, or the chain
//!   removing it deletes the whole directory ([`forget_all`]).
//!
//! Whether it runs is the owner's choice ([`wanted`]): on for a computer
//! signed in from its local settings with aggregation (the default), and for
//! one enrolled from the command line that never was (§4.3, "已注册的电脑");
//! off once the owner turns it off.

pub mod access;
pub mod controller;
pub mod copies;
pub mod gateway;
pub mod hub;
pub mod keys;
pub mod live;
pub mod store;

use std::path::PathBuf;
use std::sync::Mutex;

use hkdf::Hkdf;
use rh_proto::keys::DhKeypair;
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::sessions::Paths;

/// The salt the aggregator's keys expand under.
const DEVICE_SALT: &[u8] = b"rh/v1 device";

/// Writes under `aggregator/` and its deletion, one at a time: a key the
/// controller is storing never lands after the directory was deleted.
static LOCK: Mutex<()> = Mutex::new(());

/// Run `f` holding the directory's lock.
pub fn with_lock<T>(f: impl FnOnce() -> T) -> T {
    let _guard = LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    f()
}

/// Whether the controller may run at all (`DROPPEN_AGENT_CONTROL=0` turns it off).
pub fn enabled() -> bool {
    crate::env::var("DROPPEN_AGENT_CONTROL").as_deref() != Some("0")
}

/// Whether the owner wants this computer to control other devices, by
/// `<instance>/cloud.json` (`crate::api::cloud`): not when they turned it off
/// (`aggregatorOff`, or a sign-in from the local settings that asked for no
/// aggregation); yes otherwise — also with no file at all (a computer
/// enrolled from the command line). A file that cannot be read or does not
/// parse says no (security review of S1-4, N3: it may be the owner's "off"),
/// logged once.
///
/// `{"aggregate": false}` with no `linkedAt` still says yes, on purpose:
/// `cloud.json` is written whole, so any update of it (say `PUT
/// /api/cloud/access` on a computer enrolled from the command line) writes
/// the default `aggregate: false` without anybody asking for "off". Only
/// `aggregatorOff`, or a sign-in from the local settings without
/// aggregation (`linkedAt` set and `aggregate` false), is the owner's "off".
///
/// This only says whether the controller works. Deleting what it holds is
/// the local API's turning it off (`POST /api/cloud/aggregator`), which
/// writes `aggregatorOff` and deletes `aggregator/`; editing `cloud.json` by
/// hand stops the controller but leaves `aggregator/keys.json` where it is.
pub fn wanted(paths: &Paths) -> bool {
    let path = crate::api::cloud::file_path(paths);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
        Err(error) => return unreadable(&path, &error.to_string()),
    };
    let file: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(file) => file,
        Err(error) => return unreadable(&path, &error.to_string()),
    };
    if file["aggregatorOff"] == true {
        return false;
    }
    file["aggregate"] == true || file["linkedAt"].is_null()
}

/// Whether this computer's device role may sign in to the hub now, as its
/// link says (`docs/host-identity-rotation.md` §3.5 「不拿旧钥误伤自己」):
/// not while a staged identity key is kept and the link has not signed in
/// with the key in use — a renewal committed while this computer slept would
/// make that key retired, and its sign-in evidence against this computer.
pub type SignInGate = std::sync::Arc<dyn Fn() -> bool + Send + Sync>;

/// Whether `chain` names `me` with exactly `identity`'s keys: this computer's
/// device role signs in to the hub (a device session, `/v1/client`) only
/// then. Once a renewal of its identity key is on the chain, the process
/// still runs with the old key until its link restarts with the new one —
/// and a sign-in with a retired key is kept by the hub as evidence against
/// this very computer (`docs/host-identity-rotation.md` §3.8).
pub(crate) fn holds_named_keys(chain: &rh_proto::chain::ChainState, me: &str, identity: &rh_proto::identity::MachineIdentity) -> bool {
    chain
        .device_record(me)
        .is_some_and(|d| d.info.sign_key == identity.sign_public() && d.info.dh_key == identity.dh_public())
}

/// `cloud.json` does not read: off, said once per process.
fn unreadable(path: &std::path::Path, error: &str) -> bool {
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
        tracing::warn!(path = %path.display(), %error, "cannot read the Droppen link's settings: this computer does not control other devices until they read");
    }
    false
}

/// `<instance>/aggregator/`.
pub fn dir(paths: &Paths) -> PathBuf {
    paths.instance_dir().join("aggregator")
}

/// `<instance>/aggregator/device-key.json`.
pub fn key_path(paths: &Paths) -> PathBuf {
    dir(paths).join("device-key.json")
}

/// The aggregator's public keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceKeys {
    /// Ed25519.
    pub sign_key: [u8; 32],
    /// X25519.
    pub dh_key: [u8; 32],
}

/// The public keys a device seed expands to.
pub fn keys_from_seed(seed: &[u8; 32]) -> DeviceKeys {
    let hk = Hkdf::<Sha256>::new(Some(DEVICE_SALT), seed);
    let mut ed = Zeroizing::new([0u8; 32]);
    let mut x = Zeroizing::new([0u8; 32]);
    hk.expand(b"ed25519", ed.as_mut()).expect("32 bytes");
    hk.expand(b"x25519", x.as_mut()).expect("32 bytes");
    let signing = ed25519_dalek::SigningKey::from_bytes(&ed);
    DeviceKeys {
        sign_key: signing.verifying_key().to_bytes(),
        dh_key: DhKeypair::from_secret(*x).public(),
    }
}

/// Delete the pending aggregator key a release before chain v3 may have left
/// (`aggregator/device-key.json`); nothing else in the directory.
pub fn forget(paths: &Paths) -> Result<(), String> {
    let key = key_path(paths);
    with_lock(|| match std::fs::remove_file(&key) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", key.display())),
    })
}

/// Delete the whole directory: other devices' keys and copies with it (the
/// computer leaves its account, §4.7, is removed from it, or turns the
/// aggregator off, §4.5).
///
/// Under the directory's lock it is only renamed away (`.aggregator-deleted-…`
/// in the instance directory): from that moment nothing finds it, and every
/// writer's guard refuses (the paths it checks are gone). Its files are then
/// overwritten with zeros, synced and unlinked one by one on a thread of
/// their own ([`scrub_dir`]) — a large copy never holds the lock or an async
/// worker. Best effort, at the file level (`docs/local-aggregator.md` §5.5).
/// A rename that fails falls back to doing all of it here. Whatever an
/// earlier deletion left (the process stopped mid-way) goes first.
pub fn forget_all(paths: &Paths) -> Result<(), String> {
    sweep_deleted(paths);
    let dir = dir(paths);
    let doomed = with_lock(|| -> Result<Option<PathBuf>, String> {
        if !dir.exists() {
            return Ok(None);
        }
        let away = deleted_dir(paths);
        if std::fs::rename(&dir, &away).is_ok() {
            return Ok(Some(away));
        }
        scrub_dir(&dir);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", dir.display())),
        }
    })?;
    if let Some(away) = doomed {
        delete_in_background(vec![away]);
    }
    Ok(())
}

/// The prefix of a directory [`forget_all`] renamed away.
const DELETED_PREFIX: &str = ".aggregator-deleted-";

fn deleted_dir(paths: &Paths) -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    paths
        .instance_dir()
        .join(format!("{DELETED_PREFIX}{}-{}-{n}", std::process::id(), crate::agent::now_ms()))
}

/// The deletions running on threads of their own, for [`finish_deletions`].
static DELETIONS: Mutex<Vec<std::thread::JoinHandle<()>>> = Mutex::new(Vec::new());

/// Scrub and delete `dirs` on a thread of its own (registered, so a graceful
/// shutdown waits for it a while).
fn delete_in_background(dirs: Vec<PathBuf>) {
    let handle = std::thread::spawn(move || {
        for dir in dirs {
            scrub_dir(&dir);
            let _ = std::fs::remove_dir_all(&dir);
        }
    });
    let mut running = DELETIONS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    running.retain(|h| !h.is_finished());
    running.push(handle);
}

/// Wait (at most `within`) for the deletions still running — called when the
/// server stops gracefully, so a restart right after turning the aggregator
/// off does not cut one short. Whatever is cut short anyway is swept at the
/// next start ([`sweep_deleted`]).
pub async fn finish_deletions(within: std::time::Duration) {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let left = {
            let mut running = DELETIONS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            running.retain(|h| !h.is_finished());
            running.len()
        };
        if left == 0 || tokio::time::Instant::now() >= deadline {
            if left > 0 {
                tracing::warn!(
                    left,
                    "stopping with the other devices' copies still being deleted; the next start finishes it"
                );
            }
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// Overwrite every file in `dir` with zeros, sync it and unlink it at once
/// (`copies.db` with its `-wal` and `-shm`, and `keys.json`), one file at a
/// time: whatever stops this mid-way leaves no file it already did. Best
/// effort; at the file level only.
pub fn scrub_dir(dir: &std::path::Path) {
    use std::io::Write;
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        if !meta.is_file() {
            continue;
        }
        let zeroed = std::fs::OpenOptions::new().write(true).open(&path).and_then(|mut f| {
            let zeros = vec![0u8; 64 * 1024];
            let mut left = meta.len();
            while left > 0 {
                let n = left.min(zeros.len() as u64) as usize;
                f.write_all(&zeros[..n])?;
                left -= n as u64;
            }
            f.sync_all()
        });
        if let Err(error) = zeroed {
            tracing::warn!(%error, file = %path.display(), "cannot overwrite a file of the other devices' copies before deleting it");
        }
        let _ = std::fs::remove_file(&path);
    }
}

/// Finish what a [`forget_all`] left (the process stopped before its thread
/// was done): every directory it renamed away is overwritten and deleted, on
/// a thread of its own. Called at start (whether the aggregator is on or
/// not), before every deletion, and when the copies open.
pub fn sweep_deleted(paths: &Paths) {
    let Ok(entries) = std::fs::read_dir(paths.instance_dir()) else { return };
    let left: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(DELETED_PREFIX))
        .map(|e| e.path())
        .collect();
    if !left.is_empty() {
        tracing::info!(
            left = left.len(),
            "finishing the deletion of the other devices' copies an earlier run left"
        );
        delete_in_background(left);
    }
}

/// Keep `dir` out of Time Machine (`tmutil addexclusion`'s sticky
/// exclusion, the `com_apple_backup_excludeItem` attribute): everything in
/// `aggregator/` can be had again from the hub, and a backup would keep the
/// other devices' plaintext past their removal. Best effort; macOS only.
#[cfg(target_os = "macos")]
pub fn exclude_from_backups(dir: &std::path::Path) {
    use std::os::unix::ffi::OsStrExt;
    const NAME: &[u8] = b"com.apple.metadata:com_apple_backup_excludeItem\0";
    // A binary property list of the string "com.apple.backupd", as tmutil writes it.
    const VALUE: &[u8] = b"bplist00_\x10\x11com.apple.backupd\x08\0\0\0\0\0\0\x01\x01\0\0\0\0\0\0\0\x01\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x1c";
    let Ok(path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return;
    };
    // SAFETY: both strings are NUL-terminated and outlive the call; the value is a plain byte buffer of its length.
    let done = unsafe { libc::setxattr(path.as_ptr(), NAME.as_ptr().cast(), VALUE.as_ptr().cast(), VALUE.len(), 0, 0) };
    if done != 0 {
        tracing::debug!(error = %std::io::Error::last_os_error(), "cannot exclude the other devices' copies from Time Machine");
    }
}

/// Elsewhere: nothing (no system backup that reads this attribute).
#[cfg(not(target_os = "macos"))]
pub fn exclude_from_backups(_dir: &std::path::Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leftover_pending_key_is_forgotten_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "aggtest");
        std::fs::create_dir_all(super::dir(&paths)).unwrap();
        std::fs::write(key_path(&paths), b"{}").unwrap();
        std::fs::write(super::dir(&paths).join("keys.json"), b"{}").unwrap();
        forget(&paths).unwrap();
        assert!(!key_path(&paths).exists());
        assert!(super::dir(&paths).join("keys.json").exists(), "only the legacy key goes");
        forget(&paths).unwrap();
        let machine = rh_proto::identity::MachineIdentity::from_seed(&[5; 32]);
        assert_ne!(keys_from_seed(&[5; 32]).sign_key, machine.sign_public());
        forget_all(&paths).unwrap();
        assert!(!super::dir(&paths).exists());
    }

    /// F7 and N4: turning the aggregator off, disconnecting or a removal
    /// takes the directory away at once and overwrites its files before they
    /// are unlinked; the directory is kept out of Time Machine.
    #[test]
    fn forgetting_overwrites_the_copies_before_deleting_them() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "scrubtest");
        let mut copies = store::CopyStore::open(&paths, "a-1", "m-me", Box::new(|| true)).unwrap();
        let stream = rh_proto::record::stream_id(&[1; 32], "claude", "s").unwrap();
        copies
            .put(&store::Row {
                host: "m-2".into(),
                kind: store::Kind::Session,
                name: "claude/s".into(),
                stream,
                epoch: 0,
                head: None,
                state: Some(r#"{"items":["SCRUB-PLAINTEXT-CANARY"]}"#.into()),
                too_large: false,
                head_key: None,
                updated_ms: 1,
            })
            .unwrap();
        std::fs::write(keys::path(&paths), b"{\"hosts\":{\"m-2\":{\"0\":\"KEYS-PLAINTEXT-CANARY\"}}}").unwrap();
        #[cfg(target_os = "macos")]
        {
            let out = std::process::Command::new("xattr").arg(super::dir(&paths)).output().unwrap();
            assert!(
                String::from_utf8_lossy(&out.stdout).contains("com_apple_backup_excludeItem"),
                "aggregator/ is excluded from Time Machine"
            );
        }
        let on_disk = |d: &std::path::Path| -> Vec<u8> {
            std::fs::read_dir(d)
                .map(|e| e.flatten().filter_map(|e| std::fs::read(e.path()).ok()).flatten().collect())
                .unwrap_or_default()
        };
        assert!(
            on_disk(&super::dir(&paths)).windows(22).any(|w| w == b"SCRUB-PLAINTEXT-CANARY"),
            "the canary is there first"
        );
        // What the files hold after the scrub: zeros (read through handles
        // opened before it: the scrub unlinks each file right after zeroing it).
        let mut held: Vec<std::fs::File> = std::fs::read_dir(super::dir(&paths))
            .unwrap()
            .flatten()
            .map(|e| std::fs::File::open(e.path()).unwrap())
            .collect();
        scrub_dir(&super::dir(&paths));
        assert_eq!(std::fs::read_dir(super::dir(&paths)).unwrap().count(), 0, "every file unlinked");
        let mut bytes = Vec::new();
        for f in &mut held {
            use std::io::{Read, Seek};
            f.seek(std::io::SeekFrom::Start(0)).unwrap();
            f.read_to_end(&mut bytes).unwrap();
        }
        assert!(!bytes.is_empty() && bytes.iter().all(|b| *b == 0), "every file was zeros before it went");
        drop(held);
        let _ = std::fs::remove_dir(super::dir(&paths));
        // The whole of it: gone at once, and the renamed copy goes after.
        std::fs::create_dir_all(super::dir(&paths)).unwrap();
        std::fs::write(keys::path(&paths), b"KEYS-PLAINTEXT-CANARY").unwrap();
        forget_all(&paths).unwrap();
        assert!(!super::dir(&paths).exists());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let leftovers = || {
            std::fs::read_dir(paths.instance_dir())
                .unwrap()
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with(DELETED_PREFIX))
                .count()
        };
        while leftovers() > 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(leftovers(), 0, "the renamed directory is deleted too");
        // The store a writer still holds writes nothing now.
        assert!(!copies.usable());
        drop(copies);
        // A leftover from a run that stopped mid-way is swept.
        let stale = paths.instance_dir().join(format!("{DELETED_PREFIX}1-2-3"));
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("copies.db"), b"LEFTOVER").unwrap();
        sweep_deleted(&paths);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while stale.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!stale.exists());
    }

    #[test]
    fn the_owner_decides_whether_this_computer_controls_others() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "wanttest");
        std::fs::create_dir_all(paths.instance_dir()).unwrap();
        let file = crate::api::cloud::file_path(&paths);
        // Enrolled from the command line: no file, on.
        assert!(wanted(&paths));
        let write = |v: serde_json::Value| std::fs::write(&file, v.to_string()).unwrap();
        // Signed in from the local settings, with aggregation (the default): on.
        write(serde_json::json!({"owner": "alice", "aggregate": true, "linkedAt": 5}));
        assert!(wanted(&paths));
        // Without it, or turned off by a release from before (no `aggregatorOff`): off.
        write(serde_json::json!({"owner": "alice", "aggregate": false, "linkedAt": 5}));
        assert!(!wanted(&paths));
        // Turned off: off, whatever else.
        write(serde_json::json!({"aggregate": true, "aggregatorOff": true}));
        assert!(!wanted(&paths));
        // A file with no sign-in in it (an owner set before any): on.
        write(serde_json::json!({"remoteAccess": {}}));
        assert!(wanted(&paths));
        // A file that does not parse may be the owner's "off": off (N3).
        std::fs::write(&file, b"{\"aggregatorOff\": tr").unwrap();
        assert!(!wanted(&paths));
    }
}
