use std::path::{Path, PathBuf};

use rand::RngCore;
use rh_proto::identity::MachineIdentity;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::{check_private, hub, link, rotation, write_private};
use crate::sessions::Paths;

/// The key file's contents.
#[derive(Serialize, Deserialize)]
pub(super) struct KeyFile {
    version: u32,
    uuid: String,
    /// Hex, 32 bytes.
    seed: String,
}

/// A staged seed's file (`machine-key.next-<nonce>.json`, one per staging):
/// the [`KeyFile`] fields and the renewal it was made for
/// (`docs/host-identity-rotation.md` §3.2).
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct NextKeyFile {
    version: u32,
    uuid: String,
    /// Hex, 32 bytes.
    seed: String,
    /// The admin's request nonce, hex, 16 bytes.
    nonce: String,
    /// The chain view the proof was made against.
    chain_seq: u64,
    /// Its head (hex, 32 bytes): with the rest, what the high-assurance code
    /// is computed from, also by `agent status` in another process.
    chain_head: String,
    /// 0 automatic, 1 confirmed on this device.
    assurance: u8,
    /// When it was staged (this agent's clock).
    created_ms: u64,
    /// When the hub said it staged it (`rotation_staged`): a staging the hub
    /// took may be committed until it expires, so it is kept that long.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    accepted_ms: Option<u64>,
}

/// A machine seed and the instance identity it belongs to.
pub struct StoredSeed {
    /// The instance identity the seed was created for.
    pub uuid: String,
    /// The 32 random bytes the keys expand from.
    pub seed: Zeroizing<[u8; 32]>,
}

/// A seed staged for an identity renewal: made on `POST /rh/rotate`, kept
/// beside the current one until the chain names its keys
/// ([`FileSeedStore::promote`]) or its staging can no longer be committed.
#[derive(Clone)]
pub struct StagedSeed {
    /// The instance identity the seed was created for.
    pub uuid: String,
    /// The 32 random bytes the new keys expand from.
    pub seed: Zeroizing<[u8; 32]>,
    /// The admin's request nonce.
    pub nonce: [u8; 16],
    /// The chain view the proof was made against: entry `chain_seq`…
    pub chain_seq: u64,
    /// …whose hash is this.
    pub chain_head: [u8; 32],
    /// How the renewal is confirmed.
    pub assurance: rh_proto::chain::Assurance,
    /// When it was staged (this agent's clock).
    pub created_ms: u64,
    /// When the hub said it staged it, if it did.
    pub accepted_ms: Option<u64>,
}

impl StagedSeed {
    /// The keys it expands to.
    pub fn identity(&self) -> MachineIdentity {
        MachineIdentity::from_seed(&self.seed)
    }
}

/// The seed in a 0600 file in the instance directory. Tests use the file
/// store under a temporary home; nothing here touches a real keychain.
///
/// An identity renewal (`docs/host-identity-rotation.md` §3.5) never writes
/// over the seed in use: each new one is staged beside it
/// ([`FileSeedStore::stage`]) — several may wait, every one the hub took
/// being one a renewal may still name — and the one the chain names takes
/// its place in one atomic step ([`FileSeedStore::promote`]). A crash at any
/// point leaves the old seed (alone, or with staged ones) or the new seed
/// (with, at worst, staged ones left over that are never used).
pub struct FileSeedStore {
    path: PathBuf,
}

impl FileSeedStore {
    /// The file for this instance: `<instance dir>/agent/machine-key.json`.
    pub fn for_instance(paths: &Paths) -> Self {
        Self {
            path: paths.machine_key_path(),
        }
    }

    /// A seed file at an explicit path (an archived or expired key).
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// `machine-key.next-` — what every staging's file name starts with.
    fn staging_prefix(&self) -> String {
        let stem = self.path.file_stem().and_then(|s| s.to_str()).unwrap_or("machine-key");
        format!("{stem}.next-")
    }

    /// Where the staging of `nonce` waits: `machine-key.next-<nonce hex>.json`
    /// beside `machine-key.json`.
    pub fn staging_path(&self, nonce: &[u8; 16]) -> PathBuf {
        self.path.with_file_name(format!("{}{}.json", self.staging_prefix(), hub::hex(nonce)))
    }

    /// `machine-key.expired-` — what a staging set aside as expired is
    /// renamed to start with ([`FileSeedStore::expire`]).
    fn expired_prefix(&self) -> String {
        let stem = self.path.file_stem().and_then(|s| s.to_str()).unwrap_or("machine-key");
        format!("{stem}.expired-")
    }

    /// Where the staging of `nonce` is kept once set aside as expired:
    /// `machine-key.expired-<nonce hex>.json` beside `machine-key.json`.
    pub fn expired_path(&self, nonce: &[u8; 16]) -> PathBuf {
        self.path.with_file_name(format!("{}{}.json", self.expired_prefix(), hub::hex(nonce)))
    }

    /// The files beside the seed whose names start with `prefix`.
    fn files_with(&self, prefix: &str) -> Vec<PathBuf> {
        let Some(Ok(entries)) = self.path.parent().map(std::fs::read_dir) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_str().is_some_and(|n| n.starts_with(prefix) && n.ends_with(".json")))
            .map(|e| e.path())
            .collect()
    }

    /// Every staging's file, readable or not.
    fn staging_files(&self) -> Vec<PathBuf> {
        self.files_with(&self.staging_prefix())
    }

    /// Every file of a staging set aside as expired, readable or not.
    fn expired_files(&self) -> Vec<PathBuf> {
        self.files_with(&self.expired_prefix())
    }

    /// Set the staging of `nonce` aside as expired: what this
    /// device saw says no renewal can take it any more — but a hub's word is
    /// part of that, so it is renamed, never deleted: never tried at sign-in
    /// again, and taken back ([`FileSeedStore::restore`]) if this device's own
    /// verified chain names it after all. No-op when there is none.
    pub fn expire(&self, nonce: &[u8; 16]) -> Result<(), String> {
        let (from, to) = (self.staging_path(nonce), self.expired_path(nonce));
        match std::fs::rename(&from, &to) {
            Ok(()) => {
                sync_dir(&to);
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{} → {}: {e}", from.display(), to.display())),
        }
    }

    /// Every staging set aside as expired that reads, oldest first.
    pub fn expired(&self) -> Vec<StagedSeed> {
        let mut out: Vec<StagedSeed> = self.expired_files().iter().filter_map(|p| Self::read_staging(p).ok()).collect();
        out.sort_by_key(|s| (s.created_ms, s.nonce));
        out
    }

    /// Take the staging of `nonce` set aside as expired back among the
    /// stagings: the verified chain names it.
    pub fn restore(&self, nonce: &[u8; 16]) -> Result<(), String> {
        let (from, to) = (self.expired_path(nonce), self.staging_path(nonce));
        Self::read_staging(&from)?;
        std::fs::rename(&from, &to).map_err(|e| format!("{} → {}: {e}", from.display(), to.display()))?;
        sync_dir(&to);
        Ok(())
    }

    /// The staging files — waiting, or set aside as expired — that do not
    /// read (or that others can read): never used, and never taken for
    /// proof that this device holds no staged key.
    pub fn unreadable(&self) -> Vec<PathBuf> {
        let mut files = self.staging_files();
        files.extend(self.expired_files());
        files.retain(|p| Self::read_staging(p).is_err());
        files
    }

    fn read_staging(path: &Path) -> Result<StagedSeed, String> {
        let bytes = Zeroizing::new(std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?);
        check_private(path)?;
        let file: NextKeyFile = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let bad = |what: &str| format!("{}: {what}", path.display());
        if file.version != 1 {
            return Err(bad(&format!("unsupported key file version {}", file.version)));
        }
        let seed = hub::unhex32(&file.seed).ok_or_else(|| bad("the seed is not 32 bytes of hex"))?;
        let nonce = link::unhex(&file.nonce)
            .and_then(|n| <[u8; 16]>::try_from(n).ok())
            .ok_or_else(|| bad("the nonce is not 16 bytes of hex"))?;
        let chain_head = hub::unhex32(&file.chain_head).ok_or_else(|| bad("the chain head is not 32 bytes of hex"))?;
        let assurance = rh_proto::chain::Assurance::from_byte(file.assurance).map_err(|_| bad("an unknown assurance"))?;
        Ok(StagedSeed {
            uuid: file.uuid,
            seed: Zeroizing::new(seed),
            nonce,
            chain_seq: file.chain_seq,
            chain_head,
            assurance,
            created_ms: file.created_ms,
            accepted_ms: file.accepted_ms,
        })
    }

    fn remove(path: &Path) -> Result<(), String> {
        match std::fs::remove_file(path) {
            Ok(()) => {
                sync_dir(path);
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}

impl FileSeedStore {
    /// Where, for a person reading the output.
    pub fn describe(&self) -> String {
        self.path.display().to_string()
    }

    /// The stored seed, if there is one.
    pub fn load(&self) -> Result<Option<StoredSeed>, String> {
        let path = &self.path;
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        check_private(path)?;
        let file: KeyFile = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        if file.version != 1 {
            return Err(format!("{}: unsupported key file version {}", path.display(), file.version));
        }
        let seed = hub::unhex32(&file.seed).ok_or_else(|| format!("{}: the seed is not 32 bytes of hex", path.display()))?;
        Ok(Some(StoredSeed {
            uuid: file.uuid,
            seed: Zeroizing::new(seed),
        }))
    }

    /// Store a new seed; refuses to replace one.
    pub fn create(&self, seed: &StoredSeed) -> Result<(), String> {
        let file = Zeroizing::new(
            serde_json::to_vec_pretty(&KeyFile {
                version: 1,
                uuid: seed.uuid.clone(),
                seed: seed.seed.iter().map(|b| format!("{b:02x}")).collect(),
            })
            .map_err(|e| e.to_string())?,
        );
        write_private(&self.path, &file)
    }

    /// Stage a seed for a renewal, by its nonce: the same nonce again is
    /// written anew, every other staging stays. The seed in use is not touched.
    pub fn stage(&self, staged: &StagedSeed) -> Result<(), String> {
        let file = Zeroizing::new(
            serde_json::to_vec_pretty(&NextKeyFile {
                version: 1,
                uuid: staged.uuid.clone(),
                seed: hub::hex(staged.seed.as_ref()),
                nonce: hub::hex(&staged.nonce),
                chain_seq: staged.chain_seq,
                chain_head: hub::hex(&staged.chain_head),
                assurance: staged.assurance.to_byte(),
                created_ms: staged.created_ms,
                accepted_ms: staged.accepted_ms,
            })
            .map_err(|e| e.to_string())?,
        );
        // Written whole beside the seed in use, then renamed into place: a
        // half-written staged seed is never what the next run finds, and the
        // seed in use is never touched. The rename is flushed before the hub
        // hears of it: a staging the hub takes survives a crash.
        let path = self.staging_path(&staged.nonce);
        write_private(&path, &file)?;
        sync_dir(&path);
        Ok(())
    }

    /// Every staged seed that reads, oldest first. One that does not read —
    /// or that others can read — is left out (and said in the log): never used.
    pub fn stagings(&self) -> Result<Vec<StagedSeed>, String> {
        let mut out: Vec<StagedSeed> = self
            .staging_files()
            .iter()
            .filter_map(|path| match Self::read_staging(path) {
                Ok(staged) => Some(staged),
                Err(error) => {
                    tracing::warn!(%error, "a staged identity key that does not read: not used");
                    None
                }
            })
            .collect();
        out.sort_by_key(|s| (s.created_ms, s.nonce));
        Ok(out)
    }

    /// The newest staging, if any.
    pub fn staged(&self) -> Result<Option<StagedSeed>, String> {
        Ok(self.stagings()?.into_iter().max_by_key(|s| s.created_ms))
    }

    /// Make the staging of `nonce` the seed in use, atomically: the old seed
    /// is gone once this returns, and the other stagings after it.
    pub fn promote(&self, nonce: &[u8; 16]) -> Result<(), String> {
        let next = self.staging_path(nonce);
        // Only a staged seed that reads as one (and is private) takes the place.
        Self::read_staging(&next)?;
        std::fs::rename(&next, &self.path).map_err(|e| format!("{} → {}: {e}", next.display(), self.path.display()))?;
        sync_dir(&self.path);
        // The others can never be named now (an id is renewed to one key at
        // a time, and only from the key it holds): neither the waiting ones
        // nor those set aside as expired.
        for other in self.staging_files().into_iter().chain(self.expired_files()) {
            if let Err(error) = Self::remove(&other) {
                tracing::warn!(%error, "cannot delete a staged identity key no renewal will take");
            }
        }
        Ok(())
    }

    /// Forget the staging of `nonce` (no-op when there is none).
    pub fn discard_staged(&self, nonce: &[u8; 16]) -> Result<(), String> {
        Self::remove(&self.staging_path(nonce))
    }

    /// Forget every staging, readable or not.
    pub fn discard_all_staged(&self) -> Result<(), String> {
        self.staging_files()
            .iter()
            .chain(&self.expired_files())
            .try_for_each(|p| Self::remove(p))
    }
}

#[cfg(test)]
thread_local! {
    /// How often this thread flushed a directory ([`sync_dir`]).
    static DIR_SYNCS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How often this thread flushed a directory (tests).
#[cfg(test)]
pub(crate) fn dir_syncs() -> usize {
    DIR_SYNCS.with(std::cell::Cell::get)
}

/// Flush the directory holding `path`, so a rename in it survives a crash
/// (best effort: not every platform lets a directory be opened).
pub(super) fn sync_dir(path: &Path) {
    #[cfg(test)]
    DIR_SYNCS.with(|n| n.set(n.get() + 1));
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        if let Ok(dir) = std::fs::File::open(dir) {
            let _ = dir.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

impl Paths {
    /// Where this instance's machine identity lives.
    pub fn machine_key_path(&self) -> PathBuf {
        self.instance_dir().join("agent").join("machine-key.json")
    }
}
/// This instance's machine identity, created on first use. The `bool` says
/// whether it was created just now.
pub fn load_or_create(paths: &Paths) -> Result<(MachineIdentity, bool), String> {
    let uuid = crate::identity::ensure(paths)
        .ok_or("cannot read or mint this instance's identity uuid")?
        .uuid;
    let (identity, created) = load_or_create_in(&FileSeedStore::for_instance(paths), &uuid)?;
    // A seed this device archived as retired (a crash, or a copy put back)
    // is never used again (`docs/host-identity-rotation.md` §3.8).
    if !created && rotation::archived(paths, &identity) {
        return Err(format!(
            "{} is an archived (retired) identity key and is never used again: add this device again under a new identity",
            paths.machine_key_path().display()
        ));
    }
    Ok((identity, created))
}

/// The same against `store`, for instance identity `uuid`.
pub fn load_or_create_in(store: &FileSeedStore, uuid: &str) -> Result<(MachineIdentity, bool), String> {
    if let Some(stored) = store.load()? {
        if stored.uuid != uuid {
            return Err(format!(
                "{} belongs to identity {}, but this instance is {uuid}; a machine key is not moved between instances (re-enroll instead)",
                store.describe(),
                stored.uuid
            ));
        }
        return Ok((MachineIdentity::from_seed(&stored.seed), false));
    }
    let mut seed = Zeroizing::new([0u8; 32]);
    rand::rngs::OsRng.fill_bytes(seed.as_mut());
    store.create(&StoredSeed {
        uuid: uuid.to_string(),
        seed: seed.clone(),
    })?;
    Ok((MachineIdentity::from_seed(&seed), true))
}
