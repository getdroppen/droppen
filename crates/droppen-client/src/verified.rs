//! A standalone device's verified view of its account's device chain, kept
//! between runs and pinned to the account it joined.
//!
//! Every trust decision the device makes rests on this view and on no word of
//! the hub's: whom it may reach, whether it was removed, what a refusal means.
//! So, as the agent keeps its own (`droppen-agent`'s `state.rs`):
//!
//! - **Pinned.** The view starts from the account key's fingerprint the device
//!   was given when it joined ([`VerifiedChain::pinned`]); the first chain it
//!   verifies against it also pins the genesis entry's hash
//!   ([`TrustRoot::pinned_to`]). A hub that serves another account's chain, or
//!   another genesis under the same key, is refused, and nothing is kept.
//! - **Verified on every load.** `chain.json` (`0600`, written atomically,
//!   [`crate::files`]) keeps the entries whole, the pins and this device's own
//!   first sightings of each entry; a load verifies them from genesis again,
//!   against the pins and with the device's own clock, so a tampered file is
//!   refused rather than trusted.
//! - **Advanced, never replaced.** New entries — from the hub
//!   (`GET /v1/chain`) or from an agent's handshake reply — go through
//!   [`ChainState::merge_partial`] ([`VerifiedChain::advance`]): what verifies
//!   is applied, and kept, even when a later entry of the same batch is
//!   refused; an entry that conflicts with what the view has is a fork.
//!
//! The history before the device joined is verified with no time gates; what
//! arrives from then on is gated by the device's own clock (a recovered
//! device's self-activation waits its day), as for an agent.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use droppen_proto::chain::{ChainClock, ChainError, ChainState, Entry, TrustRoot};
use serde::{Deserialize, Serialize};

use crate::{hex, now_ms, unhex};

/// The file under the data directory.
pub const CHAIN_FILE: &str = "chain.json";
const VERSION: u32 = 1;

/// The file's shape.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChainFile {
    version: u32,
    account: String,
    /// The account key's fingerprint (hex), from the device's joining.
    fingerprint: String,
    /// The genesis entry's hash (hex), once a chain verified.
    genesis: Option<String>,
    /// Every entry, encoded (hex), from genesis.
    entries: Vec<String>,
    /// This device's first sighting of each entry (its own clock).
    first_seen: BTreeMap<u64, u64>,
    /// The first seq this device followed live.
    live_from: u64,
}

/// A device's verified, pinned view of the chain, and where it is kept.
pub struct VerifiedChain {
    path: PathBuf,
    account: String,
    root: TrustRoot,
    chain: Option<ChainState>,
    clock: ChainClock,
    live_from: u64,
}

/// What [`VerifiedChain::advance`] did with a batch.
#[derive(Debug, PartialEq, Eq)]
pub struct Advanced {
    /// How many entries it applied (they stay applied, and are kept).
    pub applied: usize,
    /// Why it stopped before the end of the batch, if it did: a fork, an
    /// entry the rules refuse, a chain that is not this account's.
    pub error: Option<ChainError>,
}

impl VerifiedChain {
    /// A view of `account` kept in `dir`, pinned to the account key's
    /// fingerprint the device was given when it joined, before any chain was
    /// seen. Not saved until it has a chain ([`VerifiedChain::advance`]).
    pub fn pinned(dir: &Path, account: &str, fingerprint: [u8; 16]) -> Self {
        Self {
            path: dir.join(CHAIN_FILE),
            account: account.into(),
            root: TrustRoot::from_fingerprint(fingerprint),
            chain: None,
            clock: ChainClock::live(now_ms(), 0),
            live_from: 0,
        }
    }

    /// The view kept in `dir`, verified from genesis again against its pins;
    /// `None` when none was kept.
    pub fn load(dir: &Path) -> Result<Option<Self>, String> {
        let path = dir.join(CHAIN_FILE);
        let Some(bytes) = crate::files::read_private(&path)? else {
            return Ok(None);
        };
        let file: ChainFile = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        if file.version != VERSION {
            return Err(format!("{}: unsupported version {}", path.display(), file.version));
        }
        let bad = |what: &str| format!("{}: bad {what}", path.display());
        let fingerprint: [u8; 16] = unhex(&file.fingerprint)
            .ok()
            .and_then(|f| f.try_into().ok())
            .ok_or_else(|| bad("fingerprint"))?;
        let genesis = match &file.genesis {
            Some(g) => Some(
                unhex(g)
                    .ok()
                    .and_then(|g| <[u8; 32]>::try_from(g).ok())
                    .ok_or_else(|| bad("genesis pin"))?,
            ),
            None => None,
        };
        let root = TrustRoot {
            account_fingerprint: fingerprint,
            genesis_hash: genesis,
        };
        let mut clock = ChainClock::live(now_ms(), file.live_from);
        clock.restore(file.first_seen);
        let chain = if file.entries.is_empty() {
            None
        } else {
            let entries = file
                .entries
                .iter()
                .map(|e| {
                    unhex(e)
                        .map_err(|_| bad("chain entry"))
                        .and_then(|b| Entry::decode(&b).map_err(|e| format!("{}: {e}", path.display())))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let chain = ChainState::verify_rooted(&entries, &root, &mut clock)
                .map_err(|e| format!("{}: the kept chain does not verify: {e}", path.display()))?;
            if chain.account() != file.account {
                return Err(format!("{}: the kept chain is another account's", path.display()));
            }
            if root.genesis_hash != Some(chain.genesis_hash()) {
                return Err(format!("{}: the kept chain is not the one its genesis pin names", path.display()));
            }
            Some(chain)
        };
        Ok(Some(Self {
            path,
            account: file.account,
            root,
            chain,
            clock,
            live_from: file.live_from,
        }))
    }

    /// Keep the view (`0600`, atomically).
    pub fn save(&self) -> Result<(), String> {
        let file = ChainFile {
            version: VERSION,
            account: self.account.clone(),
            fingerprint: hex(&self.root.account_fingerprint),
            genesis: self.root.genesis_hash.map(|g| hex(&g)),
            entries: self.chain.iter().flat_map(|c| c.entries().iter().map(|e| hex(&e.encode()))).collect(),
            first_seen: self.clock.first_seen().clone(),
            live_from: self.live_from,
        };
        let bytes = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
        crate::files::write_private(&self.path, &bytes)
    }

    /// The verified chain, once there is one.
    pub fn view(&self) -> Option<&ChainState> {
        self.chain.as_ref()
    }

    pub fn account(&self) -> &str {
        &self.account
    }

    /// The device's own clock for the chain: its first sightings of each
    /// entry (a handshake merges an agent's entries on a copy of it).
    pub fn clock(&self) -> &ChainClock {
        &self.clock
    }

    /// What the view is pinned to.
    pub fn root(&self) -> TrustRoot {
        self.root
    }

    /// The view's seq, `None` before any chain.
    pub fn seq(&self) -> Option<u64> {
        self.chain.as_ref().map(ChainState::seq)
    }

    /// Apply `entries` on the device's clock: the first batch must start at genesis and
    /// verify against the pins (which then also pin the genesis); later ones
    /// are merged ([`ChainState::merge_partial`]). Whatever applied is kept
    /// (saved), whatever stopped the batch.
    pub fn advance(&mut self, entries: &[Entry]) -> Result<Advanced, String> {
        let advanced = self.apply(entries);
        if advanced.applied > 0 {
            self.save()?;
        }
        Ok(advanced)
    }

    fn apply(&mut self, entries: &[Entry]) -> Advanced {
        self.clock.set_now(now_ms());
        match &mut self.chain {
            Some(chain) => {
                let merged = chain.merge_partial(entries, &mut self.clock);
                Advanced {
                    applied: merged.applied,
                    error: merged.error,
                }
            }
            None if entries.is_empty() => Advanced { applied: 0, error: None },
            None => {
                let refused = |error| Advanced {
                    applied: 0,
                    error: Some(error),
                };
                // The history that let this device in has no time gates; what
                // arrives from now on does.
                let live_from = entries.last().map_or(0, |e| e.seq() + 1);
                let mut clock = ChainClock::live(now_ms(), live_from);
                let chain = match ChainState::verify_rooted(entries, &self.root, &mut clock) {
                    Ok(chain) => chain,
                    Err(error) => return refused(error),
                };
                if chain.account() != self.account {
                    return refused(ChainError::WrongAccount { seq: 0 });
                }
                self.root = self.root.pinned_to(&chain);
                self.clock = clock;
                self.live_from = live_from;
                let applied = chain.entries().len();
                self.chain = Some(chain);
                Advanced { applied, error: None }
            }
        }
    }
}
