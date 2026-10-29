//! What the server's tests reach of the agent beyond what the server itself
//! calls: where the agent keeps its files, a staged seed, the streams store's
//! key-value rows, a running link's identity and renewal flag, and the bridge's
//! plugin routes. Only with `test-support`, which only the server's
//! `[dev-dependencies]` turn on; none of it is in a normal or release build.
//!
//! Each method is the agent's own, under the same name where it has one.

use std::path::PathBuf;

use rh_proto::identity::MachineIdentity;
use zeroize::Zeroizing;

use crate::link::Running;
use crate::seed::FileSeedStore;
use crate::streams::store::Store;
use crate::{AgentFiles, StagedSeed};

/// A seed to stage as a renewal, as an admin's request makes one.
pub struct TestSeed {
    /// The instance identity the seed was created for.
    pub uuid: String,
    /// The 32 bytes the new keys expand from.
    pub seed: [u8; 32],
    /// The admin's request nonce.
    pub nonce: [u8; 16],
    /// The chain entry the proof was made against…
    pub chain_seq: u64,
    /// …and its hash.
    pub chain_head: [u8; 32],
    /// How the renewal is confirmed.
    pub assurance: rh_proto::chain::Assurance,
    /// When it was made.
    pub created_ms: u64,
    /// When the hub took it.
    pub accepted_ms: Option<u64>,
}

/// The agent's files, as the server's tests read and set them.
pub trait AgentFilesTesting {
    /// The machine key's file.
    fn machine_key(&self) -> PathBuf;
    /// Where the machine key is archived at `now`.
    fn archived_key(&self, now: u64) -> PathBuf;
    /// The agent state's file.
    fn state(&self) -> PathBuf;
    /// The streams' database file.
    fn streams_db(&self) -> PathBuf;
    /// Keep machine-key epoch `epoch` as `key`.
    fn store_machine_key(&self, epoch: u32, key: &[u8]) -> Result<(), String>;
    /// Forget what an enrollment that never completed left behind.
    fn forget_enrollment_leftovers(&self) -> Result<(), String>;
    /// Whether `identity`'s seed is archived.
    fn seed_archived(&self, identity: &MachineIdentity) -> bool;
    /// Whether `identity`'s seed was retired by a renewal of this machine.
    fn seed_retired(&self, identity: &MachineIdentity) -> bool;
    /// Row `key` of the streams store's key-value table.
    fn streams_kv(&self, key: &str) -> Option<String>;
    /// Set row `key` of the streams store's key-value table.
    fn set_streams_kv(&self, key: &str, value: &str);
    /// Stage `seed` beside the one in use.
    fn stage_seed(&self, seed: TestSeed) -> Result<(), String>;
    /// Put the staged seed of `nonce` in use.
    fn promote_seed(&self, nonce: &[u8; 16]) -> Result<(), String>;
    /// Forget every staged seed.
    fn discard_staged_seeds(&self) -> Result<(), String>;
    /// Set the staged seed of `nonce` aside as expired.
    fn expire_staged_seed(&self, nonce: &[u8; 16]) -> Result<(), String>;
    /// How many seeds are staged, waiting or set aside as expired.
    fn kept_staged_seeds(&self) -> usize;
}

impl AgentFilesTesting for AgentFiles {
    fn machine_key(&self) -> PathBuf {
        AgentFiles::machine_key(self)
    }
    fn archived_key(&self, now: u64) -> PathBuf {
        AgentFiles::archived_key(self, now)
    }
    fn state(&self) -> PathBuf {
        AgentFiles::state(self)
    }
    fn streams_db(&self) -> PathBuf {
        AgentFiles::streams_db(self)
    }
    fn store_machine_key(&self, epoch: u32, key: &[u8]) -> Result<(), String> {
        AgentFiles::store_machine_key(self, epoch, key)
    }
    fn forget_enrollment_leftovers(&self) -> Result<(), String> {
        AgentFiles::forget_enrollment_leftovers(self)
    }
    fn seed_archived(&self, identity: &MachineIdentity) -> bool {
        AgentFiles::seed_archived(self, identity)
    }
    fn seed_retired(&self, identity: &MachineIdentity) -> bool {
        AgentFiles::seed_retired(self, identity)
    }
    fn streams_kv(&self, key: &str) -> Option<String> {
        Store::open(self).get_kv(key)
    }
    fn set_streams_kv(&self, key: &str, value: &str) {
        Store::open(self).set_kv(key, value)
    }
    fn stage_seed(&self, seed: TestSeed) -> Result<(), String> {
        FileSeedStore::for_instance(self).stage(&StagedSeed {
            uuid: seed.uuid,
            seed: Zeroizing::new(seed.seed),
            nonce: seed.nonce,
            chain_seq: seed.chain_seq,
            chain_head: seed.chain_head,
            assurance: seed.assurance,
            created_ms: seed.created_ms,
            accepted_ms: seed.accepted_ms,
        })
    }
    fn promote_seed(&self, nonce: &[u8; 16]) -> Result<(), String> {
        FileSeedStore::for_instance(self).promote(nonce)
    }
    fn discard_staged_seeds(&self) -> Result<(), String> {
        FileSeedStore::for_instance(self).discard_all_staged()
    }
    fn expire_staged_seed(&self, nonce: &[u8; 16]) -> Result<(), String> {
        FileSeedStore::for_instance(self).expire(nonce)
    }
    fn kept_staged_seeds(&self) -> usize {
        let store = FileSeedStore::for_instance(self);
        store.stagings().unwrap_or_default().len() + store.expired().len()
    }
}

/// A running link, as the server's tests read and nudge it.
pub trait RunningTesting {
    /// The identity it signs in with.
    fn identity(&self) -> &MachineIdentity;
    /// Note that the push keys are to be renewed, as a promotion notes it,
    /// and save the state.
    fn ask_push_renew(&self);
}

impl RunningTesting for Running {
    fn identity(&self) -> &MachineIdentity {
        &self.shared.identity
    }
    fn ask_push_renew(&self) {
        self.shared.state.lock().expect("agent state").rotation.push_renew = true;
        self.shared.save();
    }
}

/// Whether `route` is one of the plugin routes, which the bridge refuses to
/// every device rather than classifying.
pub fn plugin_route(route: &str) -> bool {
    crate::bridge::plugin_route(route)
}
