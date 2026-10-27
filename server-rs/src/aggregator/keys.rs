//! `<instance>/aggregator/keys.json`: the other hosts' machine keys this
//! computer holds as a device (local-aggregator §5.5, S1-4).
//!
//! Chain v3 hands a device that does not manage the account no user key:
//! an admin seals it each committed machine-key epoch of every host that
//! accepts its control instead (mailbox purpose 5, review D2). This computer
//! is such a device — with the one identity it runs AI with — and keeps what
//! it opened here, by host and epoch, for reading those hosts' encrypted
//! copies (S1-5). Every key in it was checked against the chain's commitment
//! before it was stored ([`rh_proto::wrap::open_host_key`]).
//!
//! The file is secret: 0600 in a 0700 directory, written whole and
//! atomically ([`crate::agent::write_private`]), refused when others can read
//! it. It is bound to the account and the device id it was written for: a
//! file another enrollment left is not read, it is started again. Turning
//! the aggregator off, disconnecting, or being removed deletes the whole
//! directory ([`super::forget_all`]).
//!
//! **A deleted file stays deleted** (security review of S1-4, S1): every
//! write — a new key, a request noted, hosts dropped — happens under the
//! directory's lock ([`super::with_lock`], which [`super::forget_all`] takes
//! too) and only while the holder's guard says it may (the aggregator on,
//! the link not stopped). And a file this handle wrote, or found, that is
//! gone now was deleted under it: the handle refuses to write it back
//! ([`OFF`]) — what it holds in memory belongs to a directory that no longer
//! exists, so the holder drops it and starts again from the disk.
//!
//! Key bytes are zeroized when dropped: the hex strings in memory, and the
//! file's bytes as read.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::sessions::Paths;

/// `<instance>/aggregator/keys.json`.
pub fn path(paths: &Paths) -> PathBuf {
    super::dir(paths).join("keys.json")
}

/// The error of a write that may not happen: the aggregator is off, its link
/// stopped, or the file was deleted under this handle. The holder drops the
/// handle.
pub const OFF: &str = "the aggregator is off, or its keys were deleted";

/// The start of the error of a `put` for an epoch already held with another
/// key: definitive — the chain commits one key per epoch, so it is never
/// tried again (security re-review of S1-4, 4).
pub const CONFLICT: &str = "another key is held for that epoch already";

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeysFile {
    /// The account the keys are of.
    #[serde(default)]
    account: String,
    /// The device (this computer's id) they were sealed to.
    #[serde(default)]
    device: String,
    /// Host → machine-key epoch → the key, in hex.
    #[serde(default)]
    hosts: BTreeMap<String, BTreeMap<u32, String>>,
    /// When (ms) this device last asked the admins for keys it lacks
    /// (mailbox purpose 4), across restarts.
    #[serde(default)]
    asked_ms: Option<u64>,
    /// The gap it asked about then (`host/epoch,…`) and how many times in a
    /// row it asked about that very gap: the requests back off per gap
    /// ([`super::controller::ASK_BACKOFF`]; security re-review of S1-4, 2).
    #[serde(default)]
    asked_gap: String,
    #[serde(default)]
    asked_tries: u32,
}

impl Drop for KeysFile {
    fn drop(&mut self) {
        for epochs in self.hosts.values_mut() {
            for key in epochs.values_mut() {
                key.zeroize();
            }
        }
    }
}

/// Whether a write may happen now (checked under the directory's lock).
pub type Guard = Box<dyn Fn() -> bool + Send + Sync>;

/// The machine keys this device holds of other hosts.
pub struct HostKeys {
    path: PathBuf,
    file: KeysFile,
    /// The file is on disk: found at load, or written since.
    on_disk: bool,
    guard: Guard,
}

/// [`crate::agent::hub::hex`], zeroized: this is key material.
fn hex(data: &[u8]) -> Zeroizing<String> {
    Zeroizing::new(crate::agent::hub::hex(data))
}

fn unhex32(text: &str) -> Option<[u8; 32]> {
    crate::agent::hub::unhex32(text)
}

impl HostKeys {
    /// The keys of `account` held by `device`: what the file says, or none
    /// when there is no file or it was written for another account or device
    /// (an earlier enrollment). A file others can read, or that does not
    /// parse, is an error: it is not quietly replaced. Writes are always
    /// allowed until [`HostKeys::guarded`] says otherwise.
    pub fn load(paths: &Paths, account: &str, device: &str) -> Result<Self, String> {
        let path = path(paths);
        let fresh = || {
            let mut file = KeysFile::default();
            file.account = account.to_string();
            file.device = device.to_string();
            file
        };
        let (file, on_disk) = match std::fs::read(&path) {
            Ok(bytes) => {
                let bytes = Zeroizing::new(bytes);
                crate::agent::check_private(&path)?;
                let file: KeysFile = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
                if file.account == account && file.device == device {
                    (file, true)
                } else {
                    (fresh(), true)
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (fresh(), false),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        Ok(Self {
            path,
            file,
            on_disk,
            guard: Box::new(|| true),
        })
    }

    /// This handle, writing only while `guard` says so (the aggregator on and
    /// its link running: checked under the directory's lock, at every write).
    pub fn guarded(mut self, guard: Guard) -> Self {
        self.guard = guard;
        self
    }

    /// Write the file — under the directory's lock, only while the guard says
    /// so, and never back after it was deleted under this handle ([`OFF`]).
    fn save(&mut self) -> Result<(), String> {
        super::with_lock(|| {
            if !(self.guard)() || (self.on_disk && !self.path.exists()) {
                return Err(OFF.to_string());
            }
            let bytes = Zeroizing::new(serde_json::to_vec_pretty(&self.file).map_err(|e| e.to_string())?);
            crate::agent::write_private(&self.path, &bytes)?;
            if let Some(dir) = self.path.parent() {
                super::exclude_from_backups(dir);
            }
            self.on_disk = true;
            Ok(())
        })
    }

    /// Whether it holds `host`'s key epoch `epoch`.
    pub fn has(&self, host: &str, epoch: u32) -> bool {
        self.file.hosts.get(host).is_some_and(|e| e.contains_key(&epoch))
    }

    /// The epochs of `host` it holds, oldest first.
    pub fn epochs(&self, host: &str) -> Vec<u32> {
        self.file.hosts.get(host).map(|e| e.keys().copied().collect()).unwrap_or_default()
    }

    /// The hosts it holds any key of.
    pub fn hosts(&self) -> Vec<String> {
        self.file.hosts.keys().cloned().collect()
    }

    /// `host`'s key epoch `epoch`, if held.
    pub fn key(&self, host: &str, epoch: u32) -> Option<Zeroizing<[u8; 32]>> {
        self.file
            .hosts
            .get(host)
            .and_then(|e| e.get(&epoch))
            .and_then(|k| unhex32(k))
            .map(Zeroizing::new)
    }

    /// Keep `key` as `host`'s epoch `epoch` (it was checked against the
    /// chain's commitment): `true` when it is new. The same key again is
    /// nothing; another key for an epoch held is refused (the chain commits
    /// one key per epoch, so the one held is the committed one). A write
    /// refused leaves nothing behind in memory either.
    pub fn put(&mut self, host: &str, epoch: u32, key: &[u8]) -> Result<bool, String> {
        let text = hex(key);
        let new_host = !self.file.hosts.contains_key(host);
        match self.file.hosts.get(host).and_then(|e| e.get(&epoch)) {
            Some(held) if held.as_str() == text.as_str() => return Ok(false),
            Some(_) => return Err(format!("{CONFLICT}: {host}'s epoch {epoch}")),
            None => {}
        }
        self.file.hosts.entry(host.to_string()).or_default().insert(epoch, text.to_string());
        if let Err(error) = self.save() {
            if let Some(epochs) = self.file.hosts.get_mut(host) {
                if let Some(mut taken) = epochs.remove(&epoch) {
                    taken.zeroize();
                }
            }
            if new_host {
                self.file.hosts.remove(host);
            }
            return Err(error);
        }
        Ok(true)
    }

    /// Drop every host `keep` says no to (removed, no longer running AI, or
    /// no longer accepting this device's control: review D2): the hosts
    /// dropped. They are gone from memory even when the write is refused.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) -> Result<Vec<String>, String> {
        let gone: Vec<String> = self.file.hosts.keys().filter(|h| !keep(h)).cloned().collect();
        if gone.is_empty() {
            return Ok(gone);
        }
        for host in &gone {
            if let Some(mut epochs) = self.file.hosts.remove(host) {
                for key in epochs.values_mut() {
                    key.zeroize();
                }
            }
        }
        self.save()?;
        Ok(gone)
    }

    /// When this device last asked the admins for keys.
    pub fn asked_ms(&self) -> Option<u64> {
        self.file.asked_ms
    }

    /// The last request: when (ms), about which gap, and how many times in a
    /// row about that gap.
    pub fn asked(&self) -> (Option<u64>, &str, u32) {
        (self.file.asked_ms, &self.file.asked_gap, self.file.asked_tries)
    }

    /// It asked the admins about `gap` at `at_ms`, the `tries`-th time in a row.
    pub fn set_asked(&mut self, at_ms: u64, gap: &str, tries: u32) -> Result<(), String> {
        self.file.asked_ms = Some(at_ms);
        self.file.asked_gap = gap.to_string();
        self.file.asked_tries = tries;
        self.save()
    }

    /// The gap it asked about closed: the next one starts its backoff again.
    /// Nothing is written when there is nothing to forget.
    pub fn gap_closed(&mut self) -> Result<(), String> {
        if self.file.asked_gap.is_empty() && self.file.asked_tries == 0 {
            return Ok(());
        }
        self.file.asked_gap.clear();
        self.file.asked_tries = 0;
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "keystest");
        (dir, paths)
    }

    #[test]
    fn keys_are_kept_by_host_and_epoch_privately_and_bound_to_their_account_and_device() {
        let (_dir, paths) = paths();
        let mut keys = HostKeys::load(&paths, "a-1", "m-me").unwrap();
        assert!(keys.put("m-2", 0, &[0x22; 32]).unwrap());
        assert!(!keys.put("m-2", 0, &[0x22; 32]).unwrap(), "the same key again is nothing");
        assert!(keys.put("m-2", 1, &[0x23; 32]).unwrap());
        assert!(keys.put("m-2", 0, &[0x99; 32]).is_err(), "never another key for an epoch held");
        assert_eq!(*keys.key("m-2", 0).unwrap(), [0x22; 32]);
        keys.set_asked(1234, "m-2/1", 1).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path(&paths)), 0o600);
            assert_eq!(mode(&super::super::dir(&paths)), 0o700);
        }
        let again = HostKeys::load(&paths, "a-1", "m-me").unwrap();
        assert_eq!(again.epochs("m-2"), vec![0, 1]);
        assert_eq!(again.asked(), (Some(1234), "m-2/1", 1));
        // Another enrollment's file is not read.
        assert!(HostKeys::load(&paths, "a-1", "m-other").unwrap().hosts().is_empty());
        assert!(HostKeys::load(&paths, "a-2", "m-me").unwrap().hosts().is_empty());
        // A file others can read is refused, not replaced.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path(&paths), std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(HostKeys::load(&paths, "a-1", "m-me").is_err());
        }
    }

    #[test]
    fn hosts_that_no_longer_admit_this_device_are_dropped() {
        let (_dir, paths) = paths();
        let mut keys = HostKeys::load(&paths, "a-1", "m-me").unwrap();
        keys.put("m-2", 0, &[2; 32]).unwrap();
        keys.put("m-3", 0, &[3; 32]).unwrap();
        assert_eq!(keys.retain(|h| h == "m-2").unwrap(), vec!["m-3".to_string()]);
        let again = HostKeys::load(&paths, "a-1", "m-me").unwrap();
        assert_eq!(again.hosts(), vec!["m-2".to_string()]);
        assert!(!again.has("m-3", 0));
    }

    /// Security review of S1-4, S1: once the directory is deleted (off,
    /// disconnected, removed), nothing this handle still holds writes it back —
    /// not a request noted, not hosts dropped, not a new key.
    #[test]
    fn a_deleted_file_is_never_written_back() {
        let (_dir, paths) = paths();
        let mut keys = HostKeys::load(&paths, "a-1", "m-me").unwrap();
        keys.put("m-2", 0, &[2; 32]).unwrap();
        keys.put("m-3", 0, &[3; 32]).unwrap();
        super::super::forget_all(&paths).unwrap();
        assert_eq!(keys.set_asked(99, "", 1).unwrap_err(), OFF);
        assert_eq!(keys.retain(|h| h == "m-2").unwrap_err(), OFF);
        assert_eq!(keys.put("m-4", 0, &[4; 32]).unwrap_err(), OFF);
        assert!(!keys.hosts().contains(&"m-4".to_string()), "a refused key leaves no empty host behind");
        assert!(!super::super::dir(&paths).exists(), "the directory stays deleted");
        // A new handle (aggregation on again) starts from the disk: nothing.
        let mut fresh = HostKeys::load(&paths, "a-1", "m-me").unwrap();
        assert!(fresh.hosts().is_empty());
        fresh.put("m-2", 1, &[5; 32]).unwrap();
        assert_eq!(HostKeys::load(&paths, "a-1", "m-me").unwrap().epochs("m-2"), vec![1]);
    }

    /// Security re-review of S1-4, 2: the request record says which gap, and
    /// how many times; a closed gap clears it (and writes only when there was
    /// something to clear).
    #[test]
    fn a_closed_gap_starts_the_request_backoff_again() {
        let (_dir, paths) = paths();
        let mut keys = HostKeys::load(&paths, "a-1", "m-me").unwrap();
        keys.gap_closed().unwrap();
        assert!(!path(&paths).exists(), "nothing to forget: nothing written");
        keys.set_asked(5, "m-2/1", 3).unwrap();
        keys.gap_closed().unwrap();
        let disk = HostKeys::load(&paths, "a-1", "m-me").unwrap();
        assert_eq!(disk.asked(), (Some(5), "", 0));
    }

    /// S1: the guard (the aggregator on, the link running) is asked under the
    /// lock at every write.
    #[test]
    fn nothing_is_written_while_the_guard_says_no() {
        let (_dir, paths) = paths();
        let on = Arc::new(AtomicBool::new(true));
        let gate = on.clone();
        let mut keys = HostKeys::load(&paths, "a-1", "m-me")
            .unwrap()
            .guarded(Box::new(move || gate.load(Ordering::SeqCst)));
        keys.put("m-2", 0, &[2; 32]).unwrap();
        on.store(false, Ordering::SeqCst);
        assert_eq!(keys.put("m-2", 1, &[3; 32]).unwrap_err(), OFF);
        assert_eq!(keys.set_asked(7, "", 1).unwrap_err(), OFF);
        let disk = HostKeys::load(&paths, "a-1", "m-me").unwrap();
        assert_eq!(disk.epochs("m-2"), vec![0], "only what was written while on");
        assert_eq!(disk.asked_ms(), None);
    }
}
