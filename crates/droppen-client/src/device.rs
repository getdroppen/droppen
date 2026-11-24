//! A standalone device: one that lives on its own, with no page and no
//! agent — its identity and its verified chain under a data directory, its
//! session at the hub, and channels to the account's hosts through relays.
//! The a2a proxy (`droppen a2a-proxy`, `docs/a2a.md` §7.4) is one.
//!
//! A request to a host ([`StandaloneDevice::rpc`]) goes on the host's channel,
//! opened when first needed and again after it ended. Opening one
//! ([`StandaloneDevice::channel`]) is up to [`ATTEMPTS`] tries, as the web
//! does (`web/src/lib/hub/machine.ts`):
//!
//! - **Where.** A relay connection on which the host is signed in already;
//!   else the relays the hub says the host is attached to, nearest first,
//!   then the account's set (`GET /v1/relays`), each signed in on when first
//!   needed ([`crate::relays`]). A relay that does not carry the host
//!   (presence offline, `CLOSE 4404`/`4410`, no answer to the hello) is
//!   skipped for the next try.
//! - **Asked for entries** (the agent is behind this device's view): the
//!   chain is synced and the hello tried again, a little later each time —
//!   the hub catches the agent up meanwhile, and the hello carries the last
//!   entries this device has.
//! - **Stale** (no admin vouched for the agent's view within seven days): the
//!   chain is synced and the hello tried once more — an attestation may have
//!   been on its way — then [`Failure::Stale`]. A device that is no admin
//!   cannot vouch itself.
//! - **Refused** (`CLOSE 4401`): once, the chain is synced and the hello
//!   tried again (the host may have renewed its key before this device
//!   heard); then [`Failure::Refused`], for the caller's chain to judge.
//!
//! Entries an agent's reply teaches the view are kept, whatever the outcome
//! ([`VerifiedChain::advance`]). A fork is [`Failure::Chain`] and stops the
//! try.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use droppen_proto::chain::{ChainClock, ChainError, ChainState};
use droppen_proto::mux::StreamClass;

use crate::channel::{attempt, Attempt, Hello, HostChannel};
use crate::failure::{Failure, Refuser};
use crate::hub::{Directory, HubSession};
use crate::identity::DeviceIdentity;
use crate::relays::{Presence, RelayConn, RelayOptions, RelayPool};
use crate::rpc::{self, RpcError, RpcRequest, RpcResponse};
use crate::verified::VerifiedChain;

/// Tries at a channel before giving up.
pub const ATTEMPTS: usize = 4;
/// How many `Stale` replies end the tries.
const STALE_TRIES: usize = 2;

/// How a device runs.
#[derive(Debug, Clone)]
pub struct DeviceOptions {
    /// What its hello says it is (`droppen-a2a-proxy/0.2.141`, say): no
    /// feature token, so an agent gives it a plain channel.
    pub client: String,
    /// How long an agent has to answer a hello.
    pub reply_timeout: Duration,
    /// The wait before a retry is this, times the try's number.
    pub retry_step: Duration,
    pub relays: RelayOptions,
}

impl Default for DeviceOptions {
    fn default() -> Self {
        Self {
            client: "droppen-client".into(),
            reply_timeout: Duration::from_secs(15),
            retry_step: Duration::from_millis(200),
            relays: RelayOptions::default(),
        }
    }
}

type Slot = Arc<tokio::sync::Mutex<Option<Arc<HostChannel>>>>;

/// A device on its own.
pub struct StandaloneDevice {
    identity: Arc<DeviceIdentity>,
    hub: Arc<HubSession>,
    chain: Mutex<VerifiedChain>,
    relays: RelayPool,
    channels: tokio::sync::Mutex<HashMap<String, Slot>>,
    options: DeviceOptions,
}

impl StandaloneDevice {
    /// The device kept in `dir` (its identity and its chain), `None` when it
    /// has not joined an account yet.
    pub fn open_dir(dir: &Path, options: DeviceOptions) -> Result<Option<Self>, String> {
        let Some(identity) = DeviceIdentity::load(dir)? else {
            return Ok(None);
        };
        let Some(chain) = VerifiedChain::load(dir)? else {
            return Err(format!(
                "{} has this device's identity but no device chain: it never finished joining its account",
                dir.display()
            ));
        };
        if chain.account() != identity.account {
            return Err(format!("{}: the identity and the chain are different accounts'", dir.display()));
        }
        Self::new(identity, chain, options).map(Some)
    }

    pub fn new(identity: DeviceIdentity, chain: VerifiedChain, options: DeviceOptions) -> Result<Self, String> {
        let hub = Arc::new(HubSession::new(&identity)?);
        let relays = RelayPool::new(hub.clone(), &identity, options.relays.clone());
        Ok(Self {
            identity: Arc::new(identity),
            hub,
            chain: Mutex::new(chain),
            relays,
            channels: tokio::sync::Mutex::new(HashMap::new()),
            options,
        })
    }

    pub fn identity(&self) -> &DeviceIdentity {
        &self.identity
    }

    pub fn hub(&self) -> &HubSession {
        &self.hub
    }

    pub fn relays(&self) -> &RelayPool {
        &self.relays
    }

    /// `f` of the verified chain as it is now (`None` before any).
    pub fn with_chain<R>(&self, f: impl FnOnce(Option<&ChainState>) -> R) -> R {
        f(self.chain.lock().expect("chain").view())
    }

    /// The chain's seq now.
    pub fn chain_seq(&self) -> Option<u64> {
        self.chain.lock().expect("chain").seq()
    }

    /// Fetch what the hub has past this device's view and verify it into the
    /// view (kept). `true` when the view moved. A fork is [`Failure::Chain`];
    /// an entry the rules refuse leaves the view where it got to.
    pub async fn sync_chain(&self) -> Result<bool, Failure> {
        let mut moved = false;
        loop {
            let from = self.chain_seq().map_or(0, |s| s + 1);
            let (entries, head) = self.hub.chain_from(from).await?;
            if entries.is_empty() {
                return Ok(moved);
            }
            let advanced = self.chain.lock().expect("chain").advance(&entries).map_err(Failure::Other)?;
            moved |= advanced.applied > 0;
            match advanced.error {
                // Another branch: an entry where this view has another, the
                // next one linked to an entry this view does not have, or
                // another account or genesis than the pins.
                Some(
                    e @ (ChainError::Fork { .. }
                    | ChainError::BrokenLink { .. }
                    | ChainError::Rollback { .. }
                    | ChainError::GenesisMismatch
                    | ChainError::PinMismatch
                    | ChainError::WrongAccount { .. }),
                ) => {
                    tracing::error!(error = %e, "the hub served a device chain that conflicts with this device's view");
                    return Err(Failure::Chain(format!("the hub served a chain that conflicts with this device's: {e}")));
                }
                Some(e) => {
                    tracing::warn!(error = %e, "refused chain entries from the hub");
                    return Ok(moved);
                }
                None => {}
            }
            if advanced.applied == 0 || head.is_none_or(|h| self.chain_seq().is_some_and(|s| s >= h)) {
                return Ok(moved);
            }
        }
    }

    /// Sync, reporting a failure rather than returning it (a retry goes on
    /// with what the view has).
    async fn resync(&self) {
        if let Err(error) = self.sync_chain().await {
            tracing::warn!(%error, "could not sync the device chain");
        }
    }

    /// The channel to `host`: the open one, or a new one.
    pub async fn channel(&self, host: &str) -> Result<Arc<HostChannel>, Failure> {
        let slot = self.channels.lock().await.entry(host.to_string()).or_default().clone();
        let mut slot = slot.lock().await;
        if let Some(channel) = slot.as_ref() {
            if channel.is_open() {
                return Ok(channel.clone());
            }
        }
        *slot = None;
        let channel = Arc::new(self.open(host).await?);
        tracing::info!(host, relay = %channel.relay, "channel open");
        *slot = Some(channel.clone());
        Ok(channel)
    }

    /// `request` to `host`'s loopback server over an `rpc` pipe: the answer's
    /// head as soon as it arrives, its body as it comes. A channel found
    /// closed as the pipe would open is opened again, once (nothing of the
    /// request had left).
    pub async fn rpc(&self, host: &str, request: RpcRequest) -> Result<RpcResponse, Failure> {
        let channel = self.channel(host).await?;
        match rpc::send(channel, StreamClass::Rpc, request.clone()).await {
            Ok(response) => Ok(response),
            Err(RpcError::NotSent(_)) => {
                let channel = self.channel(host).await?;
                rpc::rpc(channel, StreamClass::Rpc, request).await
            }
            Err(RpcError::Failed(failure)) => Err(failure),
        }
    }

    /// The view and its clock, copied for a handshake.
    fn snapshot(&self) -> Option<(ChainState, ChainClock)> {
        let chain = self.chain.lock().expect("chain");
        chain.view().map(|v| (v.clone(), chain.clock().clone()))
    }

    /// Keep what a handshake reply taught the view.
    fn learn(&self, entries: &[droppen_proto::chain::Entry]) {
        if entries.is_empty() {
            return;
        }
        let advanced = self.chain.lock().expect("chain").advance(entries);
        match advanced {
            Ok(advanced) if advanced.applied > 0 => tracing::info!(applied = advanced.applied, "caught up on the device chain from a host"),
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "could not keep the device chain"),
        }
    }

    async fn open(&self, host: &str) -> Result<HostChannel, Failure> {
        let mut skip: HashSet<String> = HashSet::new();
        let mut directory: Option<Directory> = None;
        let mut resynced = false;
        let mut stale = 0;
        let mut last: Option<Failure> = None;
        for attempt_no in 0..ATTEMPTS {
            if self.with_chain(|c| c.is_none_or(|c| c.machine(host).is_none())) {
                self.resync().await;
            }
            let Some((chain, clock)) = self.snapshot() else {
                return Err(Failure::Chain("this device has no verified chain yet".into()));
            };
            let conn = match self.carrier(host, &skip, &mut directory).await {
                Ok(conn) => conn,
                Err(failure) => return Err(last.filter(|_| matches!(failure, Failure::Offline(_))).unwrap_or(failure)),
            };
            let relay = conn.relay.clone();
            let hello = Hello {
                identity: &self.identity,
                chain,
                clock,
                client: self.options.client.as_bytes(),
                reply_timeout: self.options.reply_timeout,
            };
            let (outcome, learnt) = attempt(conn, host, hello).await;
            self.learn(&learnt);
            let wait = self.options.retry_step * (attempt_no as u32 + 1);
            match outcome {
                Attempt::Open(channel) => return Ok(channel),
                Attempt::NeedEntries => {
                    tracing::info!(host, "the device is behind this one's view of the chain: syncing and trying again");
                    last = Some(Failure::Offline("the device has not caught up with the account's chain".into()));
                    self.resync().await;
                    tokio::time::sleep(wait).await;
                }
                Attempt::Stale => {
                    stale += 1;
                    last = Some(Failure::Stale(
                        "no admin device vouched for the device's view of the account in the last 7 days".into(),
                    ));
                    if stale >= STALE_TRIES {
                        break;
                    }
                    self.resync().await;
                    tokio::time::sleep(wait).await;
                }
                Attempt::Failed(failure @ Failure::Offline(_)) => {
                    tracing::info!(host, relay, %failure, "the relay does not carry the device now");
                    skip.insert(relay);
                    last = Some(failure);
                }
                Attempt::Failed(
                    failure @ (Failure::Refused {
                        by: Refuser::Agent,
                        code: 4401,
                        ..
                    }
                    | Failure::Other(_)),
                ) if !resynced => {
                    // The host may have renewed its key before this device
                    // heard: the chain says, then one more try.
                    tracing::info!(host, %failure, "the hello was refused: syncing the chain and trying once more");
                    resynced = true;
                    self.resync().await;
                    last = Some(failure);
                }
                Attempt::Failed(failure) => return Err(failure),
            }
        }
        Err(last.unwrap_or_else(|| Failure::Offline("the device could not be reached".into())))
    }

    /// A relay connection to open a channel to `host` on: one it is signed
    /// in on already, else the hub's candidates (`skip` aside).
    async fn carrier(&self, host: &str, skip: &HashSet<String>, directory: &mut Option<Directory>) -> Result<Arc<RelayConn>, Failure> {
        for conn in self.relays.connected().await {
            if !skip.contains(&conn.relay) && conn.presence_now(host) == Some(Presence::Online) {
                return Ok(conn);
            }
        }
        if directory.is_none() {
            *directory = Some(self.hub.relays().await?);
        }
        let candidates = directory.as_ref().map(|d| d.candidates(host)).unwrap_or_default();
        let mut offline = false;
        let mut unnamed = None;
        let mut failure = None;
        for relay in candidates.iter().filter(|r| !skip.contains(&r.id)) {
            match self.relays.connection(&relay.id).await {
                Ok(conn) => match conn.presence(host).await {
                    Presence::Online => return Ok(conn),
                    Presence::Offline => offline = true,
                    // The ticket it signed in with did not name the host; a
                    // renewal since may have (the relay sends no presence
                    // for it). The relay judges the `OPEN` by the ticket it
                    // holds now: 4403 if it still does not.
                    Presence::NotNamed => unnamed = unnamed.or(Some(conn)),
                },
                Err(f @ (Failure::Refused { .. } | Failure::Removed(_))) => return Err(f),
                Err(f) => failure = Some(f),
            }
        }
        if let Some(conn) = unnamed {
            return Ok(conn);
        }
        if offline {
            return Err(Failure::Offline("the device is not signed in on any relay this device can reach".into()));
        }
        Err(failure.unwrap_or_else(|| Failure::Offline("no relay to reach the device through".into())))
    }
}
