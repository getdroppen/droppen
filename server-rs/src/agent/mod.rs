//! `remote-hand-server agent enroll <code> --hub <url> | run` — the cloud agent.
//!
//! In the cloud design (`docs/cross-machine-architecture.md`) the agent *is*
//! this server plus one outbound connection to a hub: it keeps the
//! transcripts, credentials, terminals and files, and the hub only ever
//! forwards ciphertext ([`rh_proto`]).
//!
//! - [`enroll`] — `agent enroll`: join an account with a code a device issued.
//! - [`link`] — the connection an enrolled server keeps to its hub, answering
//!   devices' end-to-end handshakes; started by `serve` (and `agent run`) when,
//!   and only when, this instance is enrolled.
//! - [`direct`] — direct paths: a WebRTC DataChannel next to the hub that a
//!   channel moves onto when the device can reach this machine, and back off
//!   when it fails (`RH_AGENT_DIRECT=0` turns them off).
//! - [`relays`] — the account's relays: tickets from the hub, a connection
//!   to each relay of the account's set, round trips measured and reported
//!   (`RH_AGENT_RELAYS=0` turns them off).
//! - [`bridge`] — a channel's sub-streams bridged to this server's own
//!   loopback API, with a per-boot credential `auth::guard` accepts only from
//!   loopback.
//! - [`state`] — what an enrolled agent remembers (pinned root, verified
//!   chain, first sightings, replay guard, attestations, machine keys).
//! - [`streams`] — the encrypted copy of this machine's session list, status
//!   and transcripts it keeps on the hub for offline reading (M2).
//!
//! A server that has never been enrolled is exactly today's single-machine
//! server: without `<instance>/agent/state.json` nothing here runs.
//!
//! # The identity seed
//!
//! One random 32-byte seed; the Ed25519 and X25519 keys are expanded from it
//! ([`rh_proto::identity::MachineIdentity`]). It is kept by [`FileSeedStore`],
//! `<instance dir>/agent/machine-key.json`, mode 0600
//! in a 0700 directory, next to the instance identity uuid ([`crate::identity`])
//! it belongs to. A key file that names another instance, or that anyone but
//! the owner can read, is refused rather than used.
//!
//! **What 0600 does not do.** Every process running as this user can read the
//! file — including the AI CLIs this server runs. Whoever reads it can sign in
//! to the hub as this machine and accept handshakes as it from elsewhere, for
//! as long as the machine stays enrolled (removing the machine from the chain,
//! or re-enrolling, ends that). It cannot impersonate a *device* to this agent
//! in any way that changes state: a first message carries no request, and a
//! transport message needs the device's own key. The macOS Keychain would
//! narrow who can read the seed to this binary's code identity — at the cost
//! of a prompt after every self-upgrade unless releases are signed with a
//! stable identity — which is why the store is a seam and not done yet.

/// What this agent declares (`agent_info.features`, and its handshake
/// reply's `features`) once it reads chain v3 (local-aggregator §3.5): the
/// hub sends it v3 entries, and lets the account take them once every host
/// declares this and [`POLICY_FEATURE`].
pub const CHAIN_V3_FEATURE: &str = "chain/3";
/// What this agent declares once it refuses the admin-only routes to devices
/// that are not admins (local-aggregator §5.4, [`bridge::refusal`]) and
/// accepts control only as its chain's accept policy says.
pub const POLICY_FEATURE: &str = "policy/1";
/// What this agent declares once it renews its identity key on request
/// (`docs/host-identity-rotation.md` §3.2, §5.6): `POST /rh/rotate`, the
/// staging on its hub connection, the sign-in confirmation and the
/// promotion ([`rotation`]). The hub takes a renewal entry only once every
/// host declares it; the hub's own `welcome` offering it is what lets this
/// agent send the rotation messages at all.
pub const ROTATE_FEATURE: &str = "rotate/1";

pub mod bridge;
pub mod control;
pub mod direct;
pub mod dispute;
pub mod enroll;
pub mod hub;
pub mod link;
pub mod relays;
pub mod rotation;
#[cfg(test)]
mod rotation_tests;
pub mod state;
pub mod streams;
pub mod system;
pub mod traffic;

use std::path::{Path, PathBuf};

use rand::RngCore;
use rh_proto::identity::{display_fingerprint, fingerprint_text, EnrollCode, MachineIdentity};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::sessions::Paths;

/// What to print when the subcommand does not parse.
pub const USAGE: &str = "\
USAGE:
    remote-hand-server agent enroll <code> --hub <wss-url> [--instance <instance>]
    remote-hand-server agent enroll - --hub <wss-url> [--instance <instance>]
    remote-hand-server agent run [--instance <instance>]
    remote-hand-server agent status [--instance <instance>]
    remote-hand-server agent unenroll [--instance <instance>]

The cloud agent. `enroll` joins this instance to an account. The instance must
have a local account first (`remote-hand-server user add <name>`, or the setup
page): a linked device reaches other devices from its page, so that page must
ask who is there. It creates the instance's machine identity key on first use
(<data>/instances/<instance>/agent/machine-key.json, mode 0600), presents the
code's token to the hub, proves it holds the code, and waits until your device
adds this machine to the account's device chain. With `-` for the code, the
code is read from standard input instead (asked for without echo on a
terminal, else the first line piped in), so it never shows in the process list
other users of this device can read. It exits 0 once the device is added, 76
when the hub took the code but the page has not added the device yet (`serve`
then finishes it), and 1 otherwise. (The local settings' \"Sign in
to Droppen\" does the same without a command line or a restart.) `run` serves
the instance with its hub connection (an enrolled instance's plain `serve`
connects too). `status` says where the instance stands. `unenroll` deletes
everything under <instance>/agent/ — the instance is a server on its own
again; remove the machine on droppen.app too — and needs the instance's server
stopped (or use the local settings, which do it live). An instance that is
never enrolled serves exactly as before.";

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Enroll(EnrollCode),
    /// `enroll -`: the code is on the first line of stdin.
    EnrollFromStdin,
    Run,
    Status,
    Unenroll,
}

#[derive(Debug, PartialEq, Eq)]
struct Invocation {
    action: Action,
    /// The hub's origin (`enroll` only).
    hub: Option<String>,
}

fn parse(args: &[String]) -> Result<Invocation, String> {
    let mut rest = Vec::new();
    let mut hub = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--hub=") {
            hub = Some(value.to_string());
        } else if arg == "--hub" {
            hub = Some(iter.next().ok_or("--hub needs a url")?.clone());
        } else {
            rest.push(arg.as_str());
        }
    }
    let hub = hub.map(|h| hub::origin(&h)).transpose()?;
    let action = match rest.as_slice() {
        ["enroll", "-"] => Action::EnrollFromStdin,
        ["enroll", code] => Action::Enroll(code.parse().map_err(|e: &str| format!("{e}\n\n{USAGE}"))?),
        ["run"] => Action::Run,
        ["status"] => Action::Status,
        ["unenroll"] => Action::Unenroll,
        _ => return Err(USAGE.to_string()),
    };
    let enrolling = matches!(action, Action::Enroll(_) | Action::EnrollFromStdin);
    if enrolling && hub.is_none() {
        return Err(format!("enroll needs the hub: --hub wss://<hub>\n\n{USAGE}"));
    }
    if !enrolling && hub.is_some() {
        return Err("--hub is only for enroll: the other subcommands use the hub this instance enrolled with".into());
    }
    Ok(Invocation { action, hub })
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The key file's contents.
#[derive(Serialize, Deserialize)]
struct KeyFile {
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
struct NextKeyFile {
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
        Self { path: key_path(paths) }
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

    /// Set the staging of `nonce` aside as expired (R6 review): what this
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
    /// proof that this device holds no staged key (R6 review).
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
        // hears of it (R6 review): a staging the hub takes survives a crash.
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

/// How often this thread flushed a directory (tests: R6 review).
#[cfg(test)]
pub(crate) fn dir_syncs() -> usize {
    DIR_SYNCS.with(std::cell::Cell::get)
}

/// Flush the directory holding `path`, so a rename in it survives a crash
/// (best effort: not every platform lets a directory be opened).
fn sync_dir(path: &Path) {
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

/// Where this instance's machine identity lives.
pub fn key_path(paths: &Paths) -> PathBuf {
    paths.instance_dir().join("agent").join("machine-key.json")
}

/// The hub origin this deployment used before the `api.` split, and the one
/// it uses now.
const LEGACY_HUB: &str = "wss://droppen.app";
const CANONICAL_HUB: &str = "wss://api.droppen.app";
/// How long the canonical hub has to answer the pre-flight sign-in.
const MIGRATE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Move an install enrolled before the split from the apex to the canonical
/// `api.` origin — once, and only if the canonical hub takes its sign-in.
/// The machine keeps its identity, account and chain; only the origin its
/// link dials changes. A hub that cannot be reached, or refuses the machine,
/// leaves the enrolled origin alone (the apex still serves it).
pub async fn migrate_hub(paths: &Paths) -> Result<(), String> {
    let Some(mut state) = state::AgentState::load(paths)? else {
        return Ok(());
    };
    if state.hub != LEGACY_HUB {
        return Ok(());
    }
    let (identity, created) = load_or_create(paths)?;
    if created {
        return Ok(());
    }
    let origin = hub::origin(CANONICAL_HUB)?;
    let attempt = async {
        let (mut sink, mut source, nonce) = hub::dial(&origin).await?;
        hub::login(&mut sink, &mut source, &identity, &state.account, &state.machine, &origin, nonce)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    };
    match tokio::time::timeout(MIGRATE_TIMEOUT, attempt).await {
        Ok(Ok(())) => {
            let from = std::mem::replace(&mut state.hub, origin);
            state.save(paths)?;
            tracing::info!(from = %from, to = %state.hub, machine = %state.machine, "moved this machine to the canonical hub");
        }
        Ok(Err(error)) => tracing::warn!(%error, "the canonical hub did not take this machine; keeping the enrolled hub"),
        Err(_) => tracing::warn!("the canonical hub did not answer in time; keeping the enrolled hub"),
    }
    Ok(())
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
            key_path(paths).display()
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

#[cfg(unix)]
pub(crate) fn check_private(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(format!(
            "{} is readable by others (mode {:o}); chmod 600 it, or delete it to make a new identity",
            path.display(),
            mode & 0o777
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn check_private(_: &Path) -> Result<(), String> {
    Ok(())
}

/// Write `data` to `path` as a new 0600 file in a 0700 directory, atomically:
/// a half-written key file must never be what the next run finds.
///
/// The rule itself lives in [`crate::fsutil::write_private_creating_parent`];
/// this adds the path to the error, which is the shape every caller here
/// reports.
pub(crate) fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    crate::fsutil::write_private_creating_parent(path, data).map_err(|error| format!("{}: {error}", path.display()))
}

fn describe(paths: &Paths, identity: &MachineIdentity, created: bool) {
    println!("Droppen agent — instance {}", paths.instance());
    println!(
        "device identity {} {}",
        if created { "created at" } else { "loaded from" },
        key_path(paths).display()
    );
    println!(
        "  signing key    {}  (Ed25519: hub sign-in, stream manifests, push payloads)",
        fingerprint_text(&identity.sign_public())
    );
    println!(
        "  channel key    {}  (X25519: Noise static key; MK is sealed to it)",
        fingerprint_text(&identity.dh_public())
    );
}

/// `agent unenroll` (the server stopped): everything this instance kept for
/// Droppen goes — `agent/`, the aggregator's key (`aggregator/`) and the
/// local link's settings (`cloud.json`), as the local settings' "Disconnect"
/// does. The state it had, if any.
fn unenroll_instance(paths: &Paths) -> Result<Option<state::AgentState>, String> {
    let before = state::AgentState::load(paths).ok().flatten();
    control::unenroll_files(paths)?;
    crate::aggregator::forget_all(paths)?;
    let cloud = crate::api::cloud::file_path(paths);
    match std::fs::remove_file(&cloud) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", cloud.display())),
    }
    Ok(before)
}

/// `agent status`: where this instance stands, from its files.
fn print_status(paths: &Paths) {
    let status = match state::AgentState::load(paths) {
        Ok(Some(st)) => control::AgentStatus::of_instance(paths, &st),
        Ok(None) => {
            println!("instance {}: not enrolled (a server on its own)", paths.instance());
            return;
        }
        Err(error) => {
            println!("instance {}: the agent state does not load: {error}", paths.instance());
            return;
        }
    };
    let state = match status.state {
        control::Standing::Linked => "enrolled",
        control::Standing::NotHosting => {
            "enrolled, not running AI in this account (it writes no streams and accepts no channels until a device makes it run AI again)"
        }
        control::Standing::Enrolling => "enrolling (waiting for the page that showed the code to add this device)",
        control::Standing::Removed => "removed from the account (the link does not run)",
        control::Standing::None | control::Standing::Broken => "unknown",
    };
    println!("instance {}: {state}", paths.instance());
    let show = |label: &str, value: Option<String>| {
        if let Some(value) = value {
            println!("  {label:<12} {value}");
        }
    };
    show("hub", status.hub);
    show("account", status.account);
    show("account key", status.account_fingerprint);
    show("device id", status.machine);
    show("chain seq", status.chain_seq.map(|s| s.to_string()));
    show("devices", status.devices.map(|n| n.to_string()));
    show("run AI", status.machines.map(|n| n.to_string()));
    show("machine key", status.mk_epoch.map(|e| format!("epoch {e}")));
    if let Ok((identity, _)) = load_or_create_existing(paths) {
        show("fingerprint", Some(control::machine_fingerprint(&identity)));
    }
    for line in rotation_lines(&status.rotation, now_ms()) {
        println!("  {line}");
    }
    for alarm in &status.alarms {
        println!("  ALARM        {}", alarm.what);
    }
    for change in &status.changes {
        println!("  change       {}", change.what);
    }
}

/// What `agent status` says about renewing this device's identity key: on a
/// server without a screen, the code of a high-assurance renewal is read
/// here (over SSH) and typed on the admin device.
fn rotation_lines(rotation: &rotation::Status, now: u64) -> Vec<String> {
    let minutes = |until: u64| until.saturating_sub(now).div_ceil(60_000);
    match rotation.state.as_str() {
        "staged" => match (&rotation.code, rotation.code_expires_ms) {
            (Some(code), Some(until)) => vec![
                "renewal      a new identity key is staged; the admin device asks you to confirm it".to_string(),
                format!("CODE         {code}  (type it on the admin device; shown for {} more minute(s))", minutes(until)),
            ],
            _ => vec!["renewal      a new identity key is staged, waiting for the renewal to be written".to_string()],
        },
        "renewed" => vec![format!(
            "renewal      identity key renewed{}",
            rotation
                .previous_fingerprint
                .as_ref()
                .map(|fp| format!(" (it was {fp})"))
                .unwrap_or_default()
        )],
        "foreign" => vec![
            format!("renewal      {}", rotation::REPLACED_ELSEWHERE),
            "             It does not connect meanwhile. On an admin device remove this device's old id; then here: \
             the local settings' Droppen section, \"add this device again with a new identity\""
                .to_string(),
        ],
        "refused" => vec![
            "renewal      the hub refuses this device's identity key as renewed but showed no renewal; it tries again".to_string(),
        ],
        "unreadable" => vec![format!("renewal      {}", rotation::STAGED_KEY_UNREADABLE)],
        _ => Vec::new(),
    }
}

/// The machine identity if it exists (never made here).
fn load_or_create_existing(paths: &Paths) -> Result<(MachineIdentity, bool), String> {
    if !key_path(paths).exists() {
        return Err("no machine key".into());
    }
    load_or_create(paths)
}

/// The longest line `agent enroll -` reads: a code is about 60 characters.
const CODE_LINE_LIMIT: u64 = 1024;

/// `agent enroll -`: the code from standard input — asked for without echo on
/// a terminal, else the first line of what is piped in, read no further than
/// [`CODE_LINE_LIMIT`].
fn read_code_from_stdin() -> Result<EnrollCode, String> {
    let line = if crate::auth::cli::stdin_is_a_terminal() {
        crate::auth::cli::read_secret("One-time code from droppen.app: ").map_err(|e| format!("could not read the code: {e}"))?
    } else {
        read_capped_line(std::io::stdin().lock())?
    };
    code_from_line(&line)
}

/// The code on the line `agent enroll -` read.
fn code_from_line(line: &str) -> Result<EnrollCode, String> {
    line.trim().parse().map_err(|e: &str| format!("the code on standard input: {e}"))
}

fn read_capped_line(input: impl std::io::Read) -> Result<String, String> {
    use std::io::BufRead;
    let mut line = String::new();
    std::io::BufReader::new(input.take(CODE_LINE_LIMIT))
        .read_line(&mut line)
        .map_err(|e| format!("could not read the code from standard input: {e}"))?;
    Ok(line)
}

/// The exit status of `agent enroll` when the hub took the code but the
/// droppen.app page has not added this device yet: the enrolling state is
/// saved and `serve` finishes it once the page does. Anything else that is
/// not success exits 1.
pub const ENROLL_PENDING_EXIT: u8 = 76;

/// `agent enroll`, the code given either way.
async fn enroll_cli(paths: &Paths, code: &EnrollCode, hub: &str) -> Result<(), String> {
    let before = FileSeedStore::for_instance(paths)
        .load()
        .ok()
        .flatten()
        .map(|s| MachineIdentity::from_seed(&s.seed).sign_public());
    // A seed a renewal retired (or one archived) never signs up again.
    let identity = rotation::identity_for_enrolling(paths)?;
    let created = before != Some(identity.sign_public());
    describe(paths, &identity, created);
    println!(
        "enrolling with account key {} (the code's secret never leaves this device)",
        display_fingerprint(&code.account_fingerprint)
    );
    match enroll::enroll(paths, &identity, code, hub).await {
        Ok(()) => Ok(()),
        Err(enroll::EnrollFailure::Failed(message)) => Err(message),
        Err(enroll::EnrollFailure::Pending(message)) => {
            // Not a failure and not done: its own exit status, so a script
            // (the installer) can start the server, which finishes it.
            let name = crate::program::Name::invoked();
            eprintln!("{}: {}", name.error_prefix(), name.rewrite(&message));
            std::process::exit(i32::from(ENROLL_PENDING_EXIT));
        }
    }
}

/// Why `agent enroll` will not link this instance, if it will not.
///
/// A device linked to Droppen reaches other devices from its page, so that
/// page must ask who is there: the instance needs a local account, and
/// authentication must not be switched off — the same two rules the local
/// settings' "Sign in to Droppen" has (`crate::api::cloud`). Checked before
/// the code is read, so nobody types a code into a command that will refuse it.
async fn refuse_enroll(paths: &Paths, auth_enabled: Option<bool>) -> Option<String> {
    let name = paths.instance();
    if auth_enabled == Some(false) {
        return Some(format!(
            "instance {name} is configured with local authentication off (auth.enabled = false): its page would answer anyone who can reach it, and a device linked to Droppen reaches other devices from its page.\n\
             Remove auth.enabled / APP_AUTH__ENABLED=false first."
        ));
    }
    // A user account: the desktop app's own login does not count, for the
    // reason it does not end the setup state.
    if crate::auth::users::UsersStore::new(paths).has_users().await {
        return None;
    }
    let code_file = crate::auth::setup::code_file(paths);
    Some(format!(
        "instance {name} has no local account, and a device linked to Droppen must have one: other devices are reachable from its page.\n\
         Create one first, either here:\n    \
             remote-hand-server user add <name> --instance {name}\n\
         or on its page: open it with remote-hand-server open --instance {name}, or with the\n\
         one-time setup code its running server keeps (remote-hand-server setup-code --instance {name};\n\
         in {} while it runs).\n\
         Then run this again.",
        code_file.display()
    ))
}

/// Run an `agent` subcommand. `args` is everything after `agent`.
pub async fn run(args: &[String]) -> Result<(), String> {
    let (args, instance) = crate::auth::cli::take_instance(args)?;
    let invocation = parse(&args)?;
    let paths = crate::auth::cli::home_paths(instance.as_deref())?;
    if matches!(invocation.action, Action::Enroll(_) | Action::EnrollFromStdin) {
        let config = crate::config::load(std::env::var("GOTCHA_ACTIVE_PROFILE").ok()).map_err(|error| error.to_string())?;
        if let Some(refusal) = refuse_enroll(&paths, config.app.auth.enabled()).await {
            return Err(refusal);
        }
    }
    match invocation.action {
        Action::EnrollFromStdin => {
            let code = read_code_from_stdin()?;
            enroll_cli(&paths, &code, invocation.hub.as_deref().expect("parse requires it")).await
        }
        Action::Enroll(code) => enroll_cli(&paths, &code, invocation.hub.as_deref().expect("parse requires it")).await,
        Action::Status => {
            print_status(&paths);
            Ok(())
        }
        Action::Unenroll => {
            // A running server holds the link and its files open, and would
            // write them back: it is stopped first, or the local settings do it.
            let dirs = crate::instance::InstanceDirs::new(&paths.data_dir(), paths.instance()).map_err(|e| e.to_string())?;
            let _lock = crate::instance::InstanceLock::acquire(&dirs).map_err(|e| {
                format!("{e}\nStop the instance's server first, or disconnect it from its settings (Droppen → Disconnect this device).")
            })?;
            let before = unenroll_instance(&paths)?;
            match before {
                Some(st) => println!(
                    "unenrolled instance {}: it was machine {} in account {} at {}; everything under {}, its aggregator \
                     key and cloud.json are gone.\n\
                     Remove the machine on droppen.app too, if it is still listed there.",
                    paths.instance(),
                    st.machine,
                    st.account,
                    st.hub,
                    paths.instance_dir().join("agent").display()
                ),
                None => println!("instance {} was not enrolled; nothing under agent/ now", paths.instance()),
            }
            Ok(())
        }
        Action::Run => {
            let enrolled = state::AgentState::load(&paths)?;
            let Some(state) = enrolled else {
                return Err(format!(
                    "instance {} is not enrolled; run `remote-hand-server agent enroll <code> --hub <url>` first",
                    paths.instance()
                ));
            };
            println!(
                "serving instance {} with its link to {} (device {}, account {})",
                paths.instance(),
                state.hub,
                state.machine,
                state.account
            );
            crate::app::RemoteHandApp {
                instance_override: Some(paths.instance().to_string()),
                ..Default::default()
            }
            .run_server()
            .await
            .map_err(|e| e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    fn code() -> String {
        EnrollCode {
            account_fingerprint: [9; 16],
            secret: [7; 16],
        }
        .to_string()
    }

    #[test]
    fn the_subcommands_parse_and_junk_is_the_usage() {
        assert_eq!(
            parse(&args(&["run"])).unwrap(),
            Invocation {
                action: Action::Run,
                hub: None
            }
        );
        let enroll = parse(&args(&["enroll", &code(), "--hub", "wss://hub.example/v1/agent"])).unwrap();
        assert_eq!(enroll.hub.as_deref(), Some("wss://hub.example"));
        assert!(matches!(enroll.action, Action::Enroll(ref code) if code.secret == [7; 16]));
        assert_eq!(
            parse(&args(&["enroll", "-", "--hub", "wss://hub.example"])).unwrap().action,
            Action::EnrollFromStdin,
            "`-`: the code comes on stdin, out of the process list"
        );
        assert!(parse(&args(&["enroll", "-"])).is_err(), "still needs the hub");
        // Piped in: one line, and never more than the cap however much follows.
        assert_eq!(read_capped_line(format!("{}\nmore", code()).as_bytes()).unwrap().trim(), code());
        // The same code with the newer `DR1-` prefix, in any case, as an argument or piped in.
        let dr = code().replacen("RH1-", "DR1-", 1);
        let want = EnrollCode {
            account_fingerprint: [9; 16],
            secret: [7; 16],
        };
        for text in [dr.clone(), dr.to_lowercase()] {
            let parsed = parse(&args(&["enroll", &text, "--hub", "wss://hub.example"])).unwrap();
            assert!(matches!(parsed.action, Action::Enroll(ref c) if *c == want), "{text}");
            let line = read_capped_line(format!("{text}\r\nmore").as_bytes()).unwrap();
            assert_eq!(code_from_line(&line).unwrap(), want, "{text} on stdin");
        }
        assert_eq!(code_from_line(&format!("{}\n", code())).unwrap(), want);
        assert!(code_from_line(&code().replacen("RH1-", "DRP1-", 1)).is_err());
        assert!(code_from_line("").is_err());
        let flood = "x".repeat(10 * CODE_LINE_LIMIT as usize);
        assert_eq!(read_capped_line(flood.as_bytes()).unwrap().len(), CODE_LINE_LIMIT as usize);
        assert_eq!(parse(&args(&["status"])).unwrap().action, Action::Status);
        assert_eq!(parse(&args(&["unenroll"])).unwrap().action, Action::Unenroll);
        assert!(parse(&args(&["unenroll", "--hub", "wss://hub.example"])).is_err());
        for bad in [
            &["enroll"][..],
            &["enroll", "RH1-nope", "--hub", "wss://hub.example"],
            &["enroll", "DR1-nope", "--hub", "wss://hub.example"],
            &["enroll", &code().replacen("RH1-", "DR2-", 1), "--hub", "wss://hub.example"],
            &["enroll", &code()],
            &["enroll", &code(), "--hub", "ws://hub.example"],
            &["run", "extra"],
            &["serve"],
            &["run", "--hub", "wss://hub.example"],
            &[],
        ] {
            assert!(parse(&args(bad)).is_err(), "{bad:?}");
        }
        assert!(
            parse(&args(&["enroll", &code(), "--hub=ws://127.0.0.1:9000/v1/agent"])).is_ok(),
            "plain ws on loopback, for local testing"
        );
    }

    #[tokio::test]
    async fn enrolling_needs_a_local_account_and_authentication_on() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(dir.path().to_path_buf(), "enrolltest");
        let refusal = refuse_enroll(&paths, None).await.expect("no account: refused");
        assert!(refusal.contains("has no local account"), "{refusal}");
        assert!(refusal.contains("user add <name> --instance enrolltest"), "says how: {refusal}");
        assert!(refusal.contains("setup-code --instance enrolltest"), "and where the code is: {refusal}");
        assert!(refusal.contains(&crate::auth::setup::code_file(&paths).display().to_string()));
        assert!(!key_path(&paths).exists(), "refused before a machine key is made");

        let users = crate::auth::users::UsersStore::new(&paths);
        users.add_app("app", "correct horse battery").await.expect("the desktop app's account");
        assert!(
            refuse_enroll(&paths, None).await.is_some(),
            "the desktop app's own account is not a user account"
        );
        users.add("ada", "correct horse battery").await.expect("account");
        assert_eq!(refuse_enroll(&paths, None).await, None, "with an account it goes ahead");
        assert_eq!(refuse_enroll(&paths, Some(true)).await, None);
        let off = refuse_enroll(&paths, Some(false)).await.expect("authentication off: refused");
        assert!(off.contains("auth.enabled = false"), "{off}");
    }

    #[test]
    fn the_identity_is_created_once_private_and_bound_to_the_instance() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agenttest");
        let (first, created) = load_or_create(&paths).unwrap();
        assert!(created);
        let (again, created) = load_or_create(&paths).unwrap();
        assert!(!created, "a second run loads rather than mints");
        assert_eq!(first.sign_public(), again.sign_public());
        assert_eq!(first.dh_public(), again.dh_public());
        let stored = FileSeedStore::for_instance(&paths).load().unwrap().unwrap();
        assert_eq!(
            stored.uuid,
            crate::identity::ensure(&paths).unwrap().uuid,
            "recorded against this instance's identity"
        );
        assert_eq!(MachineIdentity::from_seed(&stored.seed).sign_public(), first.sign_public());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = key_path(&paths);
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(load_or_create(&paths).unwrap_err().contains("readable by others"));
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        // The same file copied into another instance is refused, not silently re-derived.
        let other = Paths::for_instance(dir.path().to_path_buf(), "agentother");
        let copied = key_path(&other);
        std::fs::create_dir_all(copied.parent().unwrap()).unwrap();
        std::fs::copy(key_path(&paths), &copied).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&copied, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(load_or_create(&other).unwrap_err().contains("belongs to identity"));
    }

    /// Review of the local sign-in: the command line's unenroll left
    /// `cloud.json` (the owner) and the aggregator's key behind.
    #[test]
    fn unenroll_on_the_command_line_leaves_nothing_of_droppen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agenttest");
        load_or_create(&paths).unwrap();
        state::AgentState::enrolling("wss://hub.example", "a-1", "m-1", [9; 16], 5)
            .save(&paths)
            .unwrap();
        // A pending key an older release left behind.
        std::fs::create_dir_all(crate::aggregator::dir(&paths)).unwrap();
        std::fs::write(crate::aggregator::key_path(&paths), b"{}").unwrap();
        let cloud = crate::api::cloud::file_path(&paths);
        std::fs::write(&cloud, br#"{"owner":"alice"}"#).unwrap();
        let before = unenroll_instance(&paths).unwrap();
        assert_eq!(before.map(|st| st.machine).as_deref(), Some("m-1"));
        assert!(!paths.instance_dir().join("agent").exists());
        assert!(!crate::aggregator::dir(&paths).exists(), "the aggregator's key is gone");
        assert!(!cloud.exists(), "cloud.json is gone");
    }

    /// Host identity renewal (`docs/host-identity-rotation.md` §3.2, §3.5):
    /// staged seeds wait beside the one in use — one 0600 file each, by
    /// nonce, never over it, several at once — and the one a renewal names
    /// takes its place in one rename, the others going after it. At every
    /// point in between the store holds the old seed (alone, or with staged
    /// ones) or the new one (at worst with staged ones never used): what a
    /// crash leaves.
    /// R6 review: every write or rename of a staging — staging it, setting
    /// it aside, taking it back, promoting it — flushes the directory, so a
    /// crash right after never loses it (nor brings an old one back). A2:
    /// one set aside is never among the stagings; a promotion forgets it.
    #[test]
    fn every_staging_rename_is_flushed_and_one_set_aside_is_kept_apart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agentsync");
        let (old, _) = load_or_create(&paths).unwrap();
        let store = FileSeedStore::for_instance(&paths);
        let uuid = crate::identity::ensure(&paths).unwrap().uuid;
        let staged = |seed: u8, nonce: u8| StagedSeed {
            uuid: uuid.clone(),
            seed: Zeroizing::new([seed; 32]),
            nonce: [nonce; 16],
            chain_seq: 4,
            chain_head: [5; 32],
            assurance: rh_proto::chain::Assurance::Auto,
            created_ms: u64::from(nonce),
            accepted_ms: Some(1),
        };
        let flushed = |what: &str, f: &dyn Fn()| {
            let before = dir_syncs();
            f();
            assert!(dir_syncs() > before, "{what} flushes the directory");
        };
        flushed("staging", &|| store.stage(&staged(7, 1)).unwrap());
        flushed("staging another", &|| store.stage(&staged(8, 2)).unwrap());
        flushed("setting one aside", &|| store.expire(&[1; 16]).unwrap());
        assert!(store.expired_path(&[1; 16]).ends_with(format!("agent/machine-key.expired-{}.json", hub::hex(&[1; 16]))));
        assert_eq!(store.stagings().unwrap().iter().map(|s| s.nonce).collect::<Vec<_>>(), vec![[2; 16]]);
        assert_eq!(store.expired().iter().map(|s| s.nonce).collect::<Vec<_>>(), vec![[1; 16]]);
        assert!(store.expire(&[1; 16]).is_ok(), "none left to set aside: nothing to do");
        flushed("taking it back", &|| store.restore(&[1; 16]).unwrap());
        assert_eq!(store.stagings().unwrap().len(), 2);
        store.expire(&[1; 16]).unwrap();
        flushed("promoting", &|| store.promote(&[2; 16]).unwrap());
        assert_eq!(load_or_create(&paths).unwrap().0.sign_public(), MachineIdentity::from_seed(&[8; 32]).sign_public());
        assert_ne!(old.sign_public(), MachineIdentity::from_seed(&[8; 32]).sign_public());
        assert!(store.stagings().unwrap().is_empty() && store.expired().is_empty(), "a promotion forgets those set aside too");
        assert!(store.unreadable().is_empty());
    }

    #[test]
    fn staged_seeds_never_touch_the_one_in_use_until_one_is_promoted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agenttest");
        let (old, _) = load_or_create(&paths).unwrap();
        let store = FileSeedStore::for_instance(&paths);
        assert_eq!(
            store.staging_path(&[1; 16]),
            key_path(&paths).with_file_name(format!("machine-key.next-{}.json", hub::hex(&[1; 16])))
        );
        assert!(store.stagings().unwrap().is_empty() && store.staged().unwrap().is_none());
        assert!(store.promote(&[1; 16]).is_err(), "nothing staged, nothing promoted");
        let uuid = crate::identity::ensure(&paths).unwrap().uuid;
        let staged = |seed: u8, nonce: u8, created_ms: u64| StagedSeed {
            uuid: uuid.clone(),
            seed: Zeroizing::new([seed; 32]),
            nonce: [nonce; 16],
            chain_seq: 4,
            chain_head: [5; 32],
            assurance: rh_proto::chain::Assurance::OnDevice,
            created_ms,
            accepted_ms: None,
        };
        store.stage(&staged(7, 1, 6)).unwrap();
        // Crash here: the old seed is the one in use.
        assert_eq!(load_or_create(&paths).unwrap().0.sign_public(), old.sign_public());
        let read = store.staged().unwrap().unwrap();
        assert_eq!((read.nonce, read.chain_seq, read.chain_head, read.created_ms, read.accepted_ms), ([1; 16], 4, [5; 32], 6, None));
        assert_eq!(read.assurance, rh_proto::chain::Assurance::OnDevice);
        assert_eq!(read.identity().sign_public(), MachineIdentity::from_seed(&[7; 32]).sign_public());
        let file: serde_json::Value = serde_json::from_slice(&std::fs::read(store.staging_path(&[1; 16])).unwrap()).unwrap();
        for field in ["version", "uuid", "seed", "nonce", "chainSeq", "chainHead", "assurance", "createdMs"] {
            assert!(file.get(field).is_some(), "{field}: {file}");
        }
        // Taken by the hub: noted in the same file.
        store.stage(&StagedSeed { accepted_ms: Some(9), ..staged(7, 1, 6) }).unwrap();
        assert_eq!(store.stagings().unwrap().len(), 1, "the same nonce again: the same staging");
        assert_eq!(store.staged().unwrap().unwrap().accepted_ms, Some(9));
        // A second staging waits beside the first: neither replaces the other.
        store.stage(&staged(8, 2, 7)).unwrap();
        let both: Vec<[u8; 16]> = store.stagings().unwrap().iter().map(|s| s.nonce).collect();
        assert_eq!(both, vec![[1; 16], [2; 16]], "oldest first");
        assert_eq!(store.staged().unwrap().unwrap().nonce, [2; 16], "the newest");
        assert_eq!(load_or_create(&paths).unwrap().0.sign_public(), old.sign_public());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let first = store.staging_path(&[1; 16]);
            assert_eq!(std::fs::metadata(&first).unwrap().permissions().mode() & 0o777, 0o600);
            std::fs::set_permissions(&first, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(store.stagings().unwrap().len(), 1, "one others can read is not used");
            assert!(store.promote(&[1; 16]).is_err(), "nor taken");
            std::fs::set_permissions(&first, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        // The FIRST is the one a renewal names: promoted; the other goes.
        store.promote(&[1; 16]).unwrap();
        assert!(store.stagings().unwrap().is_empty());
        let (now, created) = load_or_create(&paths).unwrap();
        assert!(!created);
        assert_eq!(now.sign_public(), MachineIdentity::from_seed(&[7; 32]).sign_public());
        assert_eq!(store.load().unwrap().unwrap().uuid, uuid, "the same instance");
        // Forgetting staged seeds leaves the one in use.
        store.stage(&staged(9, 3, 8)).unwrap();
        store.stage(&staged(10, 4, 9)).unwrap();
        store.discard_staged(&[3; 16]).unwrap();
        store.discard_staged(&[3; 16]).unwrap();
        assert_eq!(store.stagings().unwrap().len(), 1);
        store.discard_all_staged().unwrap();
        assert!(store.stagings().unwrap().is_empty());
        assert_eq!(load_or_create(&paths).unwrap().0.sign_public(), now.sign_public());
    }

    #[test]
    fn the_status_shows_a_renewal_and_its_code() {
        let code = rotation::Status {
            state: "staged".into(),
            code: Some("ABCD-EFGH".into()),
            code_expires_ms: Some(10 * 60_000),
            ..Default::default()
        };
        let lines = rotation_lines(&code, 0);
        assert!(lines.iter().any(|l| l.contains("ABCD-EFGH") && l.contains("10 more minute")), "{lines:?}");
        let foreign = rotation::Status {
            state: "foreign".into(),
            reason: Some(rotation::FOREIGN_RENEWED_ELSEWHERE.into()),
            ..Default::default()
        };
        let said = rotation_lines(&foreign, 0).join("\n");
        assert!(said.contains("replaced elsewhere") && said.contains("backup") && said.contains("new identity"), "{said}");
        assert!(!said.to_lowercase().contains("impersonat") && !said.to_lowercase().contains("hijack"));
        assert!(rotation_lines(&rotation::Status::default(), 0).is_empty());
    }

    #[test]
    fn a_corrupt_key_file_is_an_error_not_a_new_identity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agenttest");
        load_or_create(&paths).unwrap();
        let path = key_path(&paths);
        let uuid = crate::identity::ensure(&paths).unwrap().uuid;
        std::fs::write(&path, format!(r#"{{"version":1,"uuid":"{uuid}","seed":"abcd"}}"#)).unwrap();
        assert!(load_or_create(&paths).unwrap_err().contains("not 32 bytes"));
    }
}
