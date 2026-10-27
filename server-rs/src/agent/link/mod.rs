//! The enrolled agent's one outbound connection to its hub, kept up for as
//! long as the server runs.
//!
//! On every (re)connection it signs in as the machine, follows the account's
//! device chain from where it left off (refusing rollbacks and forks), keeps
//! the attestations the hub relays, opens machine keys left in its mailbox,
//! and answers `OPEN`s from devices as the Noise responder
//! ([`rh_proto::handshake::accept`] with its own live clock, the freshness
//! gate and the replay guard). Each accepted channel runs on
//! [`rh_proto::driver`], and every stream a device opens on it is bridged to
//! this server's loopback API ([`super::bridge`]).
//!
//! A device that offers sequenced frames ([`rh_proto::path::CLIENT_FEATURE`]
//! in its hello) gets a two-path channel, and may ask on its signaling
//! stream for a direct path ([`super::direct`], [`rh_proto::direct::serve`]):
//! the direct hello is admitted by [`admit::LinkAdmit`] under exactly the
//! rules above, and the direct path lives and dies with its channel — a
//! removal that closes the channel ([`drop_removed`]) closes the direct path
//! too.
//!
//! An open channel is held to control's seven days as a new one is: once no
//! admin has vouched for this machine's view for that long, it is closed
//! ([`ChannelLapse`]), whatever the hub keeps up or to itself.
//!
//! Reconnects back off exponentially with full jitter (0.5 s doubling to 30 s,
//! reset after a connection that lasted a minute). A hub restart — every
//! deploy — closes the socket with 1012; channels die with it and devices
//! re-handshake, which costs one round trip.
//!
//! State is saved after anything that changes it, and the replay guard is
//! saved *before* a handshake reply leaves, so a crash cannot make a replayed
//! first message acceptable.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rh_proto::frame::Frame;
use rh_proto::identity::MachineIdentity;
use rh_proto::noise::ReplayGuard;
use serde_json::Value;
use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;

use lapse::ChannelLapse;
use super::bridge::Loopback;
use super::hub::now_ms;
use super::state::{self, AgentState};
use super::{bridge, direct, hub, load_or_create, relays, rotation, streams, traffic, StagedSeed, CHAIN_V3_FEATURE, POLICY_FEATURE, ROTATE_FEATURE};
use crate::sessions::Paths;

mod admit;
mod alerts;
mod connection;
mod lapse;
mod local;
mod mail;

pub use connection::{enabled, start, LinkEnv, Running};
#[cfg(test)]
pub(super) use connection::run;
#[cfg(test)]
pub(super) use connection::test_shared;
pub(super) use admit::{expire_pending, on_channel_frame};
pub(super) use alerts::raise_alarm;
pub(super) use lapse::{drop_ended, drop_removed};
pub(super) use local::standing_of;
pub(crate) use local::unhex;

/// The close of a handshake this agent took but could not save the device
/// of (a device not on its record yet is let in only once it is): the page
/// says the computer cannot save, rather than retrying blind.
pub const AGENT_UNSAVED: u16 = 4507;
/// The close of a channel nobody vouched for in time: a handshake refused
/// as stale, and an open channel whose control lapsed ([`ChannelLapse`]).
pub const CHANNEL_STALE: u16 = 4412;
/// How long an open channel waits at most before it looks at its control's
/// lapse again: its deadline is wall-clock time, and a timer may sleep
/// through a suspended computer.
const LAPSE_POLL: Duration = Duration::from_secs(10 * 60);
/// The hub's close once it committed a renewal of this device's identity key
/// (`docs/host-identity-rotation.md` §3.5): sign in with the new key.
pub const ROTATED: u16 = 4005;
pub(super) struct Shared {
    pub(super) paths: Paths,
    /// Mailed machine keys this agent could not judge yet (the chain had not
    /// caught up with their commitment or sender): tried again as it does.
    deferred_mail: Mutex<Vec<Value>>,
    pub(super) identity: Arc<MachineIdentity>,
    loopback: Arc<Loopback>,
    /// The encrypted streams' writer ([`super::streams`]).
    sync: super::streams::Handle,
    /// Shared with the streams' writer, which seals only under machine keys
    /// this chain committed ([`super::streams::KeyCheck`]).
    pub(super) state: Arc<Mutex<AgentState>>,
    guard: Mutex<ReplayGuard>,
    pub(super) cancel: CancellationToken,
    /// Direct paths on (`DROPPEN_AGENT_DIRECT`), and the peers they may hold at once.
    direct: bool,
    peers: Arc<Semaphore>,
    /// When a recovered device's self-activation this agent held back (too
    /// early by its own clock) may be taken: the chain is asked for again then.
    held_until: Mutex<Option<u64>>,
    /// Bumped whenever the chain advanced, wherever from ([`chain_moved`]):
    /// the hub connection and every relay connection ([`super::relays`])
    /// close the channels of devices that may no longer control this machine.
    pub(super) chain_version: tokio::sync::watch::Sender<u64>,
    /// Every open channel's control lapse ([`ChannelLapse`]): told of each
    /// attestation as it arrives ([`take_attestation`]), before a newer one
    /// from the same admin can replace it in the state.
    pub(super) lapses: Mutex<Vec<std::sync::Weak<Mutex<ChannelLapse>>>>,
    /// A test's hook right after a handshake's admission, its state lock let
    /// go: where an attestation arriving then would have slipped past a
    /// channel whose lapse was registered only later.
    #[cfg(test)]
    pub(super) after_admission: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// The hub connection's control sender, while it is up.
    pub(super) hub_text: Mutex<Option<mpsc::UnboundedSender<String>>>,
    /// The account's relays (`None`: `DROPPEN_AGENT_RELAYS=0`).
    pub(super) relays: Mutex<Option<super::relays::Handle>>,
    /// Whether the hub connection is up, since when, and why it last failed
    /// (the local API's `GET /api/cloud`).
    pub(super) health: Mutex<Health>,
    /// Renewing this device's identity key ([`super::rotation`]).
    pub(super) rotation: super::rotation::Rotation,
    /// Restarts the link in this process once a renewed key is in place.
    pub(super) restart: super::rotation::Restart,
}

/// The hub connection as the local API reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Health {
    /// Signed in to the hub right now.
    pub connected: bool,
    /// Since when (ms) it has been connected, or not; `0` before the first try.
    pub since_ms: u64,
    /// Why the last connection failed or ended, in words.
    pub error: Option<String>,
    /// The hub refused this machine's sign-in (it says "removed", or will not
    /// have it otherwise): retried with backoff, never taken as a removal —
    /// only this machine's verified chain removes it.
    pub refused: Option<String>,
}

impl Shared {
    pub(super) fn save(&self) {
        if let Err(error) = self.try_save() {
            tracing::error!(%error, "cannot save the agent state");
        }
    }

    /// [`Shared::save`], its failure returned: for a step that must not go
    /// on unless the state it relies on is on disk.
    pub(super) fn try_save(&self) -> Result<(), String> {
        let st = self.state.lock().expect("agent state");
        st.save(&self.paths)
    }

    fn set_health(&self, connected: bool, error: Option<String>) {
        let mut health = self.health.lock().expect("health");
        if health.connected != connected || health.since_ms == 0 {
            health.since_ms = now_ms();
        }
        health.connected = connected;
        if connected {
            health.error = None;
            health.refused = None;
        } else if error.is_some() {
            health.error = error;
        }
    }
}

/// Open channels: `ch` → (the device it was authorised for, the static key
/// it opened the channel with, its inbound frames).
pub(super) type Routes = HashMap<u32, (String, [u8; 32], mpsc::UnboundedSender<Frame>)>;

/// Channels opened and waiting for the device's first message: `ch` → (the
/// device, when it opened). Per connection, and bounded: a relay is not
/// trusted not to open channels without end ([`on_channel_frame`]).
pub(super) type Pending = HashMap<u32, (String, Instant)>;
/// At most this many channels wait on one connection; more are refused (4409)…
const MAX_PENDING: usize = 256;
/// …and none for longer than this (then it is refused, 4408).
const PENDING_FOR: Duration = Duration::from_secs(30);

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use futures_util::{SinkExt, StreamExt};
    use rh_proto::bridge::{RequestHead, ResponseHead};
    use rh_proto::chain::{Action, Attestation, ChainError, Entry, Signer};
    use rh_proto::channel::Session;
    use rh_proto::direct::{Admission, Admit};
    use rh_proto::driver::Channel;
    use rh_proto::frame::Frame;
    use rh_proto::handshake;
    use rh_proto::mux::{Role, StreamClass};
    use rh_proto::noise::{Binding, RekeyPolicy};
    use rh_proto::path::DirectBinding;
    use serde_json::{json, Value};
    use tokio::sync::mpsc;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_util::sync::CancellationToken;

    use super::admit::*;
    use super::alerts::*;
    use super::connection::*;
    use super::lapse::*;
    use super::local::*;
    use super::mail::*;
    use super::*;
    use rh_proto::chain::{genesis_draft, recovery_keys, ChainClock, ChainState, DeviceInfo, RecoveryKey, RecoveryKeys};
    use rh_proto::identity::{fingerprint, MachineIdentity};

    /// An account — genesis (d-1, recovery words) → add d-2 → add m-1 — the
    /// hub's view of it, and this machine enrolled in it.
    struct Account {
        d1: MachineIdentity,
        rk: RecoveryKeys,
        hub: ChainState,
        shared: Arc<Shared>,
        /// Every `CLOSE` the agent sent a device that [`connect`]ed: `(ch, code)`.
        heard_closes: Arc<Mutex<Vec<(u32, u16)>>>,
        _dir: tempfile::TempDir,
    }

    fn info(id: &str, k: &MachineIdentity) -> DeviceInfo {
        DeviceInfo {
            id: id.into(),
            sign_key: k.sign_public(),
            dh_key: k.dh_public(),
            full: true,
        }
    }

    fn account() -> Account {
        account_with(super::super::streams::Handle::detached())
    }

    /// [`account`], its streams' writer events going to `sync`.
    fn account_with(sync: super::super::streams::Handle) -> Account {
        account_at(sync, "127.0.0.1:9".parse().unwrap())
    }

    /// [`account_with`], its loopback server at `loopback`.
    fn account_at(sync: super::super::streams::Handle, loopback: std::net::SocketAddr) -> Account {
        let ak = MachineIdentity::from_seed(&[1; 32]);
        let d1 = MachineIdentity::from_seed(&[2; 32]);
        let d2 = MachineIdentity::from_seed(&[3; 32]);
        let machine = MachineIdentity::from_seed(&[4; 32]);
        let rk = recovery_keys(&[7; 16]);
        let genesis = genesis_draft(
            "a-1",
            ak.sign_public(),
            info("d-1", &d1),
            Some(RecoveryKey::new(&rk.signing, "a-1", &ak.sign_public())),
            1_000,
        )
        .sign(ak.signing_key());
        let mut hub = ChainState::verify(std::slice::from_ref(&genesis), None, &mut ChainClock::history()).unwrap();
        let mut entries = vec![genesis];
        for action in [Action::AddDevice(info("d-2", &d2)), Action::AddMachine(machine.info("m-1"))] {
            let e = hub.draft(Signer::Device("d-1".into()), action, 2_000).sign(d1.signing_key());
            hub.apply(&e, &mut ChainClock::history()).unwrap();
            entries.push(e);
        }
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agentlink");
        let mut st = AgentState::enrolling("wss://hub.example", "a-1", "m-1", fingerprint(&ak.sign_public()), now_ms());
        assert!(st.absorb(&entries, now_ms()).error.is_none());
        let guard = st.replay_guard(now_ms());
        let shared = Arc::new(Shared {
            paths,
            deferred_mail: Mutex::new(Vec::new()),
            identity: Arc::new(machine),
            loopback: Arc::new(Loopback::new(loopback, "test".into())),
            sync,
            state: Arc::new(Mutex::new(st)),
            guard: Mutex::new(guard),
            cancel: CancellationToken::new(),
            direct: false,
            peers: Arc::new(Semaphore::new(1)),
            held_until: Mutex::new(None),
            chain_version: tokio::sync::watch::channel(0).0,
            lapses: Mutex::new(Vec::new()),
            after_admission: Mutex::new(None),
            hub_text: Mutex::new(None),
            relays: Mutex::new(None),
            health: Mutex::new(Health::default()),
            rotation: super::super::rotation::Rotation::new(),
            restart: super::super::rotation::no_restart(),
        });
        Account {
            d1,
            rk,
            hub,
            shared,
            heard_closes: Arc::default(),
            _dir: dir,
        }
    }

    /// A batch that fails half-way: d-2 removed, a thief joining with the
    /// recovery words, then an entry with a spoiled signature.
    fn half_bad_batch(a: &mut Account) -> Vec<Entry> {
        let thief = MachineIdentity::from_seed(&[9; 32]);
        let rm = a
            .hub
            .draft(Signer::Device("d-1".into()), Action::RemoveDevice("d-2".into()), 3_000)
            .sign(a.d1.signing_key());
        a.hub.apply(&rm, &mut ChainClock::history()).unwrap();
        let join = a
            .hub
            .draft(Signer::Recovery, Action::RecoveryJoin(info("d-thief", &thief)), 4_000)
            .sign(&a.rk.signing);
        a.hub.apply(&join, &mut ChainClock::history()).unwrap();
        let mut bogus = a
            .hub
            .draft(Signer::Device("d-1".into()), Action::RemoveMachine("m-1".into()), 5_000)
            .sign(a.d1.signing_key());
        bogus.signature[0] ^= 1;
        vec![rm, join, bogus]
    }

    /// What acting on every entry that took effect means, checked after a
    /// batch that stopped half-way: d-2's open channel closed (4403), both
    /// changes alerted, and the state saved with them.
    fn assert_acted_on(a: &Account, routes: &Routes, out: &mut mpsc::UnboundedReceiver<Frame>) {
        assert!(routes.is_empty(), "the removed device's channel is gone");
        let mut closed = Vec::new();
        while let Ok(frame) = out.try_recv() {
            if let Frame::Close { ch, code, .. } = frame {
                closed.push((ch, code));
            }
        }
        assert!(closed.contains(&(7, 4403)), "{closed:?}");
        assert_eq!(
            a.shared.state.lock().unwrap().changes.iter().map(|a| a.what.clone()).collect::<Vec<_>>(),
            vec![
                "device d-2 was removed by d-1".to_string(),
                "device d-thief is coming back with the recovery words".to_string()
            ]
        );
        let saved = AgentState::load(&a.shared.paths).unwrap().unwrap();
        let chain = saved.chain.as_ref().unwrap();
        assert_eq!(chain.seq(), 4, "saved through the join");
        assert!(chain.device_record("d-thief").is_some() && chain.device("d-2").is_none());
    }

    fn routes_with_d2() -> Routes {
        let (tx, rx) = mpsc::unbounded_channel();
        std::mem::forget(rx);
        HashMap::from([(7, ("d-2".to_string(), d2().dh_public(), tx))])
    }

    // ---- the policy's wiring --------------------------------------------

    /// d-2's keys in [`account`].
    fn d2() -> MachineIdentity {
        MachineIdentity::from_seed(&[3; 32])
    }

    /// A device of [`policy_account`] that neither runs AI nor manages the account.
    fn d3() -> MachineIdentity {
        MachineIdentity::from_seed(&[5; 32])
    }

    /// Entries d-1 signs for `actions`, applied to the hub's view and taken
    /// by this agent's state — and nothing else acted on (no channel closed),
    /// as when a stream races the chain change.
    fn take(a: &mut Account, actions: Vec<Action>) -> Vec<Entry> {
        let entries: Vec<Entry> = actions
            .into_iter()
            .map(|action| {
                let e = a.hub.draft(Signer::Device("d-1".into()), action, 3_000).sign(a.d1.signing_key());
                a.hub.apply(&e, &mut ChainClock::history()).unwrap();
                e
            })
            .collect();
        let absorbed = a.shared.state.lock().unwrap().absorb(&entries, now_ms());
        assert!(absorbed.error.is_none(), "{:?}", absorbed.error);
        entries
    }

    /// `device` (keys `key`) vouches for this agent's chain head, `age_ms` ago.
    fn attest(a: &Account, device: &str, key: &MachineIdentity, age_ms: u64) {
        let att = Attestation::create(&a.hub, device, key.signing_key(), now_ms() - age_ms);
        assert!(a.shared.state.lock().unwrap().attest(att));
    }

    /// [`account`] plus d-3, a device that neither runs AI nor manages the
    /// account (v2 `AddDevice{full: false}`).
    fn policy_account() -> Account {
        let mut a = account();
        let k = d3();
        let plain = DeviceInfo {
            id: "d-3".into(),
            sign_key: k.sign_public(),
            dh_key: k.dh_public(),
            full: false,
        };
        take(&mut a, vec![Action::AddDevice(plain)]);
        a
    }

    /// Open a channel to this agent as `device` (keys `key`) on `ch`, carried
    /// the way the hub carries it: the device's end, or the codes the agent
    /// refused it with.
    async fn connect(a: &Account, key: &MachineIdentity, device: &str, ch: u32, routes: &mut Routes) -> Result<Channel, Vec<(u32, u16)>> {
        let binding = Binding {
            account: "a-1".into(),
            device: device.into(),
            machine: "m-1".into(),
        };
        let mut initiator = rh_proto::noise::Initiator::new(key.dh(), &a.shared.identity.dh_public(), &binding).unwrap();
        let (chain_seq, chain_head) = {
            let st = a.shared.state.lock().unwrap();
            let chain = st.chain.as_ref().unwrap();
            (chain.seq(), chain.head())
        };
        let body = handshake::HelloBody {
            chain_seq,
            chain_head,
            entries: Vec::new(),
            client: Vec::new(),
        };
        let hello = initiator.write_hello(now_ms(), &body.encode().unwrap()).unwrap();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let (text_tx, _text_rx) = mpsc::unbounded_channel();
        accept(&a.shared, ch, device.into(), &hello, routes, &out_tx, &text_tx);
        let reply = match out_rx.try_recv() {
            Ok(Frame::Data { payload, .. }) if routes.contains_key(&ch) => payload,
            Ok(first) => {
                let mut closed = closes(&mut out_rx);
                if let Frame::Close { ch, code, .. } = first {
                    closed.insert(0, (ch, code));
                }
                return Err(closed);
            }
            Err(_) => return Err(Vec::new()),
        };
        let (transport, _) = initiator.read_reply(&reply, RekeyPolicy::default(), now_ms()).unwrap();
        let session = Session::new(ch, transport, Role::Initiator);
        let (to_device, from_agent) = mpsc::unbounded_channel();
        let (device_out, mut device_frames) = mpsc::unbounded_channel();
        let (channel, _incoming) = Channel::spawn(session, from_agent, device_out);
        let heard = a.heard_closes.clone();
        tokio::spawn(async move {
            while let Some(frame) = out_rx.recv().await {
                if let Frame::Close { ch, code, .. } = &frame {
                    heard.lock().unwrap().push((*ch, *code));
                }
                if to_device.send(frame).is_err() {
                    break;
                }
            }
        });
        let agent = routes.get(&ch).unwrap().2.clone();
        tokio::spawn(async move {
            while let Some(frame) = device_frames.recv().await {
                if agent.send(frame).is_err() {
                    break;
                }
            }
        });
        Ok(channel)
    }

    /// One rpc over `channel`: the answer's status and JSON body, or `None`
    /// when nothing answered (the stream closed without a head, or two
    /// seconds passed).
    async fn rpc(channel: &Channel, method: &str, path: &str, body: &[u8]) -> Option<(u16, Value)> {
        rpc_held(channel, method, path, body, Duration::ZERO).await
    }

    /// [`rpc`], its body and `End` held back for `hold` after the stream opened.
    async fn rpc_held(channel: &Channel, method: &str, path: &str, body: &[u8], hold: Duration) -> Option<(u16, Value)> {
        use rh_proto::bridge::{message, Kind, MessageReader};
        use rh_proto::driver::StreamEvent;
        let head = RequestHead {
            method: method.into(),
            path: path.into(),
            headers: Vec::new(),
        };
        let mut stream = channel.open(StreamClass::Rpc, &head.encode().unwrap()).await.ok()?;
        tokio::time::sleep(hold).await;
        if !body.is_empty() {
            stream.send(&message(Kind::Binary, body)).await.ok()?;
        }
        stream.send(&message(Kind::End, &[])).await.ok()?;
        let mut reader = MessageReader::new();
        let (mut status, mut out) = (None, Vec::new());
        loop {
            match tokio::time::timeout(Duration::from_secs(2), stream.recv()).await {
                Ok(Some(StreamEvent::Data(data))) => {
                    reader.push(&data);
                    while let Ok(Some((kind, payload))) = reader.next_message() {
                        match kind {
                            Kind::Head => status = Some(ResponseHead::decode(&payload).unwrap().status),
                            _ => out.extend_from_slice(&payload),
                        }
                    }
                }
                Ok(Some(StreamEvent::Closed(_)) | None) => break,
                Err(_) => return None,
            }
        }
        Some((status?, serde_json::from_slice(&out).unwrap_or(Value::Null)))
    }

    /// Where each device stands for the admin-only routes, as this agent's
    /// own chain says now: an admin with a twelve-hour-fresh view, an admin
    /// whose view nobody vouched for in twelve hours, a device that is no
    /// admin. (`standing()` answering admin and fresh for everybody would
    /// hand every device the posture routes.)
    #[test]
    fn standing_is_read_from_this_agents_chain_and_attestations() {
        use bridge::Local as _;
        let a = policy_account();
        let local = AgentLocal::new(a.shared.clone(), Arc::new(Mutex::new(ChannelLapse::new("d-1"))));
        assert_eq!(
            (local.standing("d-1").admin, local.standing("d-1").view),
            (true, bridge::View::Stale),
            "nobody vouched yet"
        );
        attest(&a, "d-2", &d2(), 13 * 60 * 60 * 1000);
        assert_eq!(
            local.standing("d-1").view,
            bridge::View::Stale,
            "a view vouched for thirteen hours ago is stale for management"
        );
        attest(&a, "d-2", &d2(), 0);
        assert_eq!(
            (local.standing("d-1").admin, local.standing("d-1").view),
            (true, bridge::View::Fresh)
        );
        assert!(!local.standing("d-3").admin, "a device that does not manage the account");
        assert!(!local.standing("d-9").admin, "a device the chain does not name");
        a.shared.state.lock().unwrap().chain = None;
        assert_eq!(
            local.standing("d-1"),
            bridge::Standing {
                admin: false,
                view: bridge::View::Stale,
                controls: false,
            }
        );
    }

    /// The 412's `reason` follows the freshness check: an admin vouching for a
    /// head beyond this agent's view makes it `behind`; once this agent takes
    /// a different entry at that seq, the same vouching conflicts with its
    /// view: `fork`.
    #[test]
    fn standing_tells_a_view_behind_from_a_fork() {
        use bridge::Local as _;
        let mut a = policy_account();
        let local = AgentLocal::new(a.shared.clone(), Arc::new(Mutex::new(ChannelLapse::new("d-1"))));
        let mut ahead = a.hub.clone();
        let theirs = ahead
            .draft(Signer::Device("d-1".into()), Action::RemoveDevice("d-3".into()), 3_001)
            .sign(a.d1.signing_key());
        ahead.apply(&theirs, &mut ChainClock::history()).unwrap();
        let vouched = Attestation::create(&ahead, "d-2", d2().signing_key(), now_ms());
        assert!(a.shared.state.lock().unwrap().attest(vouched), "kept: catching up will tell");
        assert_eq!(local.standing("d-1").view, bridge::View::Behind);
        // The same removal, drafted a moment apart: another entry at that seq.
        take(&mut a, vec![Action::RemoveDevice("d-3".into())]);
        assert_ne!(a.hub.head(), ahead.head());
        assert_eq!(local.standing("d-1").view, bridge::View::Fork, "this view took another entry there");
    }

    /// Checklist item 12 end to end, through a channel this agent accepted:
    /// the bridge refuses a posture route to a device that is not an admin
    /// (403 `limited_device`) and to an admin whose view is older than twelve
    /// hours (412 `stale`), before anything reaches loopback or the agent's
    /// own routes; what uses the computer goes on.
    #[tokio::test]
    async fn the_bridge_refuses_posture_routes_by_this_agents_own_view() {
        let mut a = policy_account();
        take(
            &mut a,
            vec![Action::SetAccept {
                host: "m-1".into(),
                from: rh_proto::chain::Accept::All,
            }],
        );
        attest(&a, "d-2", &d2(), 13 * 60 * 60 * 1000);
        let mut routes = Routes::new();
        let admin = connect(&a, &a.d1, "d-1", 1, &mut routes)
            .await
            .expect("an admin, vouched for within seven days");
        let plain = connect(&a, &d3(), "d-3", 2, &mut routes).await.expect("a device this host accepts");
        let name = br#"{"name":"study"}"#;
        let (status, body) = rpc(&plain, "POST", "/rh/display-name", name).await.expect("an answer");
        assert_eq!((status, body["error"].as_str()), (403, Some("limited_device")));
        let (status, body) = rpc(&plain, "POST", "/api/server/self-upgrade", b"")
            .await
            .expect("an answer, not loopback's");
        assert_eq!((status, body["error"].as_str()), (403, Some("limited_device")));
        let (status, body) = rpc(&admin, "POST", "/rh/display-name", name).await.expect("an answer");
        assert_eq!(
            (status, body["error"].as_str(), body["reason"].as_str()),
            (412, Some("stale"), Some("stale")),
            "an admin whose view is 13 hours old"
        );
        let (status, _) = rpc(&plain, "GET", "/rh/info", b"").await.expect("an answer");
        assert_eq!(status, 200, "what uses the computer goes on");
        attest(&a, "d-2", &d2(), 0);
        let (status, body) = rpc(&admin, "POST", "/rh/display-name", name).await.expect("an answer");
        assert_eq!((status, body["displayName"].as_str()), (200, Some("study")), "a fresh admin");
        assert_eq!(a.shared.state.lock().unwrap().display_name.as_deref(), Some("study"));
    }

    /// While this computer renews its push keys with its identity key
    /// (`rotation.push_renew`, until the push keys are renewed), a device
    /// asking over a channel for the push key, or subscribing against it, is
    /// answered 503 `push_renewing` — not handed the key about to be
    /// replaced, which its page would keep as current — and nothing reaches
    /// the server; once they are renewed, the request reaches it again.
    #[tokio::test]
    async fn the_push_key_is_not_handed_out_while_it_is_renewed() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // The local server: hands out the push key it holds, and counts the requests.
        let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        let reached = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = reached.clone();
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = server.accept().await {
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buf = [0u8; 4096];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match tcp.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => request.extend_from_slice(&buf[..n]),
                        }
                    }
                    let line = String::from_utf8_lossy(&request).lines().next().unwrap_or_default().to_string();
                    seen.lock().unwrap().push(line);
                    let body = r#"{"publicKey":"KEY-IN-USE"}"#;
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = tcp.write_all(reply.as_bytes()).await;
                });
            }
        });
        let a = account_at(super::super::streams::Handle::detached(), addr);
        attest(&a, "d-2", &d2(), 0);
        let mut routes = Routes::new();
        let admin = connect(&a, &a.d1, "d-1", 1, &mut routes).await.expect("an admin");
        // A renewed identity key was taken: the push keys follow it.
        a.shared.state.lock().unwrap().rotation.push_renew = true;
        let (status, body) = rpc(&admin, "GET", "/api/push/vapid", b"").await.expect("an answer");
        assert_eq!(
            (status, body["error"].as_str()),
            (503, Some("push_renewing")),
            "the key about to be replaced is not handed out: {body}"
        );
        let subscription = br#"{"endpoint":"https://push.example/x","keys":{"p256dh":"p","auth":"a"}}"#;
        let (status, body) = rpc(&admin, "POST", "/api/push/subscriptions", subscription).await.expect("an answer");
        assert_eq!((status, body["error"].as_str()), (503, Some("push_renewing")), "{body}");
        assert!(reached.lock().unwrap().is_empty(), "nothing reached the server: {:?}", reached.lock().unwrap());
        // Renewed: the page gets the key.
        a.shared.state.lock().unwrap().rotation.push_renew = false;
        let (status, body) = rpc(&admin, "GET", "/api/push/vapid", b"").await.expect("an answer");
        assert_eq!((status, body["publicKey"].as_str()), (200, Some("KEY-IN-USE")));
        assert_eq!(*reached.lock().unwrap(), vec!["GET /api/push/vapid HTTP/1.1".to_string()]);
    }

    /// A renewal of the push keys that fails (the state database refusing the
    /// write, here) is tried again while the link runs, after a back-off — not
    /// only at the next
    /// start, the push key refused until then — and `push_renew` clears once
    /// it is done.
    #[tokio::test(start_paused = true)]
    async fn the_push_keys_are_renewed_again_after_a_failure_without_a_restart() {
        let a = account();
        let paths = a.shared.paths.clone();
        let old = crate::push::store::vapid_public(&paths).await.unwrap();
        let db = crate::db::at(paths.instance_db());
        db.write(|tx| tx.execute_batch("CREATE TRIGGER refuse_push BEFORE UPDATE ON kv WHEN NEW.scope = 'push' BEGIN SELECT RAISE(ABORT, 'refused (test)'); END;"))
            .unwrap();
        a.shared.state.lock().unwrap().rotation.push_renew = true;
        let task = tokio::spawn(renew_push(a.shared.clone()));
        // The first try fails.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(a.shared.state.lock().unwrap().rotation.push_renew);
        assert_eq!(crate::push::store::vapid_public(&paths).await.unwrap(), old);
        db.write(|tx| tx.execute_batch("DROP TRIGGER refuse_push")).unwrap();
        // Without a restart: tried again, renewed, noted done.
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(
            !a.shared.state.lock().unwrap().rotation.push_renew,
            "not tried again: the push key stays refused until the next start"
        );
        assert_ne!(crate::push::store::vapid_public(&paths).await.unwrap(), old, "a new push key");
        assert!(!AgentState::load(&paths).unwrap().unwrap().rotation.push_renew, "saved");
        tokio::time::timeout(Duration::from_secs(1), task).await.expect("done").unwrap();
    }

    /// A device whose handshake this agent accepts is recorded
    /// among those that ever opened a channel — in the state saved before the
    /// reply leaves, and the status stream told — and one it refuses is not.
    #[tokio::test]
    async fn an_accepted_handshake_records_the_device_and_a_refused_one_does_not() {
        use super::super::streams::Event;
        let (sync, mut events) = super::super::streams::Handle::captured();
        let mut a = account_with(sync);
        let k = d3();
        let plain = DeviceInfo {
            id: "d-3".into(),
            sign_key: k.sign_public(),
            dh_key: k.dh_public(),
            full: false,
        };
        take(&mut a, vec![Action::AddDevice(plain)]);
        attest(&a, "d-2", &d2(), 0);
        let ids = |a: &Account| controller_ids(&a.shared.state.lock().unwrap());
        let told = |events: &mut mpsc::UnboundedReceiver<Event>| {
            std::iter::from_fn(|| events.try_recv().ok())
                .filter(|e| matches!(e, Event::Controllers))
                .count()
        };
        let mut routes = Routes::new();
        let refused = connect(&a, &k, "d-3", 1, &mut routes).await.err().expect("this host accepts admins only");
        assert_eq!(refused, vec![(1, 4401)]);
        assert!(ids(&a).is_empty(), "a refused handshake opened nothing");
        assert_eq!(told(&mut events), 0);

        let _open = connect(&a, &a.d1, "d-1", 2, &mut routes).await.expect("an admin");
        assert_eq!(ids(&a), vec!["d-1".to_string()]);
        let saved = AgentState::load(&a.shared.paths).unwrap().unwrap();
        assert_eq!(controller_ids(&saved), vec!["d-1".to_string()], "saved with the replay guard");
        let record = saved.controllers().unwrap();
        assert_eq!((record.from, record.seq), (0, 3), "whole since enrollment, at this agent's chain");
        assert_eq!(told(&mut events), 1, "the status is told");
        let _again = connect(&a, &a.d1, "d-1", 3, &mut routes).await.expect("again");
        assert_eq!(told(&mut events), 0, "a device seen before is nothing new");
    }

    /// The ids of the devices `st` recorded as having opened a channel.
    fn controller_ids(st: &AgentState) -> Vec<String> {
        st.controllers().unwrap().devices.into_iter().map(|d| d.id).collect()
    }

    /// Make the next save of `a`'s state fail (its file's place taken by a
    /// directory), or let it succeed again.
    fn break_saves(a: &Account, broken: bool) {
        let path = a.shared.paths.agent_state_path();
        if broken {
            let _ = std::fs::remove_file(&path);
            std::fs::create_dir_all(path.join("in-the-way")).unwrap();
        } else {
            std::fs::remove_dir_all(&path).unwrap();
        }
    }

    /// The handshake is saved before its reply leaves: a
    /// device new to the record whose save fails is refused — no reply, no
    /// route, its note taken back so the next try saves again — and never
    /// gets in without being on disk. One already on record gets in whatever
    /// the disk does: a full disk locks nobody out.
    #[tokio::test]
    async fn a_new_device_whose_state_cannot_be_saved_is_refused_and_one_on_record_is_not() {
        use super::super::streams::Event;
        let (sync, mut events) = super::super::streams::Handle::captured();
        let a = account_with(sync);
        attest(&a, "d-2", &d2(), 0);
        let told = |events: &mut mpsc::UnboundedReceiver<Event>| {
            std::iter::from_fn(|| events.try_recv().ok())
                .filter(|e| matches!(e, Event::Controllers))
                .count()
        };
        let mut routes = Routes::new();
        break_saves(&a, true);
        let refused = connect(&a, &a.d1, "d-1", 1, &mut routes).await.err().expect("not saved, not let in");
        assert_eq!(refused, vec![(1, AGENT_UNSAVED)]);
        assert!(routes.is_empty());
        assert!(controller_ids(&a.shared.state.lock().unwrap()).is_empty(), "taken back: the next try saves again");
        assert_eq!(told(&mut events), 0, "nothing new to say");
        break_saves(&a, false);
        let _open = connect(&a, &a.d1, "d-1", 2, &mut routes).await.expect("saved, let in");
        assert_eq!(controller_ids(&AgentState::load(&a.shared.paths).unwrap().unwrap()), vec!["d-1".to_string()]);
        assert_eq!(told(&mut events), 1, "the status is told once it is saved");
        break_saves(&a, true);
        let _again = connect(&a, &a.d1, "d-1", 3, &mut routes)
            .await
            .expect("on record: let in, the disk notwithstanding");
        break_saves(&a, false);
    }

    /// A first handshake message from `device` (keys `key`) as the device's
    /// side writes it for the hub or a relay, at this agent's chain head.
    fn hub_hello(a: &Account, key: &MachineIdentity, device: &str) -> Vec<u8> {
        let binding = Binding {
            account: "a-1".into(),
            device: device.into(),
            machine: "m-1".into(),
        };
        let mut initiator = rh_proto::noise::Initiator::new(key.dh(), &a.shared.identity.dh_public(), &binding).unwrap();
        let (chain_seq, chain_head) = {
            let st = a.shared.state.lock().unwrap();
            let chain = st.chain.as_ref().unwrap();
            (chain.seq(), chain.head())
        };
        let body = handshake::HelloBody {
            chain_seq,
            chain_head,
            entries: Vec::new(),
            client: Vec::new(),
        };
        initiator.write_hello(now_ms(), &body.encode().unwrap()).unwrap()
    }

    /// Handshakes from a device new to the record arriving
    /// at once — on the hub link and two direct paths, which run side by
    /// side — while saves fail: none gets in, and none leaves it noted. The
    /// note, the save and the taking back are one step under the state's
    /// lock, so "on the record already" is never another handshake's note
    /// that did not reach the disk.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_handshakes_of_a_new_device_while_saves_fail_let_it_in_on_none() {
        let a = account();
        attest(&a, "d-1", &a.d1, 0);
        break_saves(&a, true);
        let start = Arc::new(std::sync::Barrier::new(3));
        let mut direct = Vec::new();
        for _ in 0..2 {
            let (shared, start) = (a.shared.clone(), start.clone());
            let (bind, hello) = direct_hello(&a, &d2(), "d-2");
            direct.push(tokio::task::spawn_blocking(move || {
                start.wait();
                matches!(LinkAdmit { shared }.admit("d-2", &bind, &hello), Admission::Open { .. })
            }));
        }
        let hub = {
            let (shared, start) = (a.shared.clone(), start.clone());
            let hello = hub_hello(&a, &d2(), "d-2");
            tokio::task::spawn_blocking(move || {
                start.wait();
                hub_attempt(&shared, 9, "d-2", &hello)
            })
        };
        for task in direct {
            assert!(!task.await.unwrap(), "no direct path for it");
        }
        assert_eq!(hub.await.unwrap(), (false, vec![(9, AGENT_UNSAVED)]), "no channel on the hub either: 4507");
        assert!(controller_ids(&a.shared.state.lock().unwrap()).is_empty(), "noted nowhere");
        break_saves(&a, false);
        let (bind, hello) = direct_hello(&a, &d2(), "d-2");
        assert!(matches!(LinkAdmit { shared: a.shared.clone() }.admit("d-2", &bind, &hello), Admission::Open { .. }), "saved: in");
    }

    /// One hub (or relay) handshake on `ch`, as [`on_channel_frame`] runs
    /// it: whether a channel opened, and the closes the agent sent.
    fn hub_attempt(shared: &Arc<Shared>, ch: u32, device: &str, hello: &[u8]) -> (bool, Vec<(u32, u16)>) {
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let (text_tx, _text_rx) = mpsc::unbounded_channel();
        let mut routes = Routes::new();
        accept(shared, ch, device.into(), hello, &mut routes, &out_tx, &text_tx);
        (!routes.is_empty(), closes(&mut out_rx))
    }

    /// The race, made to happen every time: a hub handshake from a device new
    /// to the record is held between noting it and saving it (a test hook in
    /// [`AgentState::record_controller`]) while saves fail, and a direct
    /// handshake from the same device comes in then. Were the note and the
    /// save two steps, the second would see the first's note, take the device
    /// for one on the saved record and let it in while the first is refused
    /// and takes its note back — on neither disk nor memory, yet in. It waits
    /// for the first to finish, and is refused the same way.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_handshake_held_between_note_and_save_hides_its_note_from_a_concurrent_one() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let a = account();
        attest(&a, "d-1", &a.d1, 0);
        // Both hellos first: writing one reads the state, which the held handshake keeps.
        let hello = hub_hello(&a, &d2(), "d-2");
        let (bind, direct) = direct_hello(&a, &d2(), "d-2");
        let (noted_tx, noted_rx) = std::sync::mpsc::channel::<()>();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        let (first, go_rx) = (AtomicBool::new(true), Mutex::new(go_rx));
        a.shared.state.lock().unwrap().between_note_and_save = Some(Arc::new(move || {
            if first.swap(false, Ordering::SeqCst) {
                noted_tx.send(()).unwrap();
                go_rx.lock().unwrap().recv().unwrap();
            }
        }));
        break_saves(&a, true);
        let hub = {
            let shared = a.shared.clone();
            tokio::task::spawn_blocking(move || hub_attempt(&shared, 9, "d-2", &hello))
        };
        noted_rx.recv_timeout(Duration::from_secs(5)).expect("the hub handshake noted d-2 and is held before its save");
        let (done_tx, done_rx) = std::sync::mpsc::channel::<bool>();
        let second = {
            let shared = a.shared.clone();
            tokio::task::spawn_blocking(move || {
                let open = matches!(LinkAdmit { shared }.admit("d-2", &bind, &direct), Admission::Open { .. });
                done_tx.send(open).unwrap();
            })
        };
        assert!(
            done_rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "the direct handshake cannot see (or act on) the held one's unsaved note: it waits"
        );
        go_tx.send(()).unwrap();
        assert_eq!(hub.await.unwrap(), (false, vec![(9, AGENT_UNSAVED)]), "the hub handshake: refused, 4507");
        assert!(!done_rx.recv_timeout(Duration::from_secs(5)).unwrap(), "the direct one: refused too");
        second.await.unwrap();
        assert!(controller_ids(&a.shared.state.lock().unwrap()).is_empty(), "noted nowhere");
        a.shared.state.lock().unwrap().between_note_and_save = None;
        break_saves(&a, false);
    }

    /// A direct hello from `device` (keys `key`) as the device's side writes
    /// it, on a made-up DataChannel binding, at this agent's chain head.
    fn direct_hello(a: &Account, key: &MachineIdentity, device: &str) -> (DirectBinding, Vec<u8>) {
        let bind = DirectBinding {
            binding: Binding {
                account: "a-1".into(),
                device: device.into(),
                machine: "m-1".into(),
            },
            relay_hash: [1; 32],
            nonce: [2; 32],
            device_fp: [3; 32],
            agent_fp: [4; 32],
        };
        let machine_dh = a.shared.identity.dh_public();
        let mut initiator = rh_proto::noise::Initiator::with_prologue(key.dh(), &machine_dh, &bind.prologue().unwrap()).unwrap();
        let (chain_seq, chain_head) = {
            let st = a.shared.state.lock().unwrap();
            let chain = st.chain.as_ref().unwrap();
            (chain.seq(), chain.head())
        };
        let body = handshake::HelloBody {
            chain_seq,
            chain_head,
            entries: Vec::new(),
            client: Vec::new(),
        };
        (bind, initiator.write_hello(now_ms(), &body.encode().unwrap()).unwrap())
    }

    /// A device admitted on a DataChannel is
    /// recorded and saved before the reply leaves, like on the hub or a
    /// relay; a device new to the record whose save fails is refused (and
    /// the status told nothing), one already on record is not.
    #[tokio::test]
    async fn a_direct_admission_records_the_device_and_a_new_one_that_cannot_be_saved_is_refused() {
        use super::super::streams::Event;
        let (sync, mut events) = super::super::streams::Handle::captured();
        let a = account_with(sync);
        attest(&a, "d-1", &a.d1, 0);
        let told = |events: &mut mpsc::UnboundedReceiver<Event>| {
            std::iter::from_fn(|| events.try_recv().ok())
                .filter(|e| matches!(e, Event::Controllers))
                .count()
        };
        let admit = LinkAdmit { shared: a.shared.clone() };
        break_saves(&a, true);
        let (bind, hello) = direct_hello(&a, &d2(), "d-2");
        assert!(
            matches!(admit.admit("d-2", &bind, &hello), Admission::Refused { reply: None, .. }),
            "not saved, not let in"
        );
        assert!(controller_ids(&a.shared.state.lock().unwrap()).is_empty(), "taken back");
        assert_eq!(told(&mut events), 0, "nothing saved, nothing to say");
        break_saves(&a, false);
        let (bind, hello) = direct_hello(&a, &d2(), "d-2");
        assert!(matches!(admit.admit("d-2", &bind, &hello), Admission::Open { .. }), "an admin, saved");
        assert_eq!(
            controller_ids(&AgentState::load(&a.shared.paths).unwrap().unwrap()),
            vec!["d-2".to_string()],
            "on disk by the time the reply is handed back"
        );
        assert_eq!(told(&mut events), 1, "the status is told, once saved");
        break_saves(&a, true);
        let (bind, hello) = direct_hello(&a, &d2(), "d-2");
        assert!(
            matches!(admit.admit("d-2", &bind, &hello), Admission::Open { .. }),
            "on record: let in, the disk notwithstanding"
        );
        break_saves(&a, false);
    }

    /// "Who may control me" holds for open channels: a chain change closes
    /// the channel of every device this machine no longer accepts control
    /// from — a device the policy does not name, an admin demoted — not only
    /// of devices the chain removed. An admin's channel stays.
    #[tokio::test]
    async fn a_chain_change_closes_the_channels_of_devices_that_may_no_longer_control_this_machine() {
        let mut a = policy_account();
        take(
            &mut a,
            vec![Action::SetAdmin {
                id: "d-2".into(),
                admin: false,
            }],
        );
        let (tx, rx) = mpsc::unbounded_channel();
        std::mem::forget(rx);
        let mut routes: Routes = HashMap::from([
            (1, ("d-1".to_string(), a.d1.dh_public(), tx.clone())),
            (2, ("d-2".to_string(), d2().dh_public(), tx.clone())),
            (3, ("d-3".to_string(), d3().dh_public(), tx)),
        ]);
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        drop_removed(&a.shared, &mut routes, &out_tx);
        let mut closed = closes(&mut out_rx);
        closed.sort();
        assert_eq!(
            closed,
            vec![(2, 4403), (3, 4403)],
            "the demoted admin and the device the policy does not name"
        );
        assert_eq!(routes.keys().copied().collect::<Vec<_>>(), vec![1], "the admin's channel stays");
    }

    /// Every stream re-checks, on this agent's chain now, that the device may
    /// still control this machine — not merely that it is still a device: a
    /// stream that races the chain change (its channel not swept yet) from an
    /// admin demoted under an admins-only policy is not served.
    #[tokio::test]
    async fn each_stream_rechecks_that_the_device_may_still_control_this_machine() {
        let mut a = policy_account();
        attest(&a, "d-1", &a.d1, 0);
        let mut routes = Routes::new();
        let channel = connect(&a, &d2(), "d-2", 4, &mut routes).await.expect("an admin");
        let (status, _) = rpc(&channel, "GET", "/rh/info", b"").await.expect("served while it may control");
        assert_eq!(status, 200);
        take(
            &mut a,
            vec![Action::SetAdmin {
                id: "d-2".into(),
                admin: false,
            }],
        );
        assert!(routes.contains_key(&4), "the channel is not swept yet");
        assert!(
            a.shared.state.lock().unwrap().chain.as_ref().unwrap().device("d-2").is_some(),
            "still a device"
        );
        assert_eq!(
            rpc(&channel, "GET", "/rh/info", b"").await,
            None,
            "not served once it may no longer control"
        );
    }

    /// Open channel `ch` as `device` the way the hub's connection or a
    /// relay's hands it on: whether it was let in, and the codes it was
    /// refused with.
    fn open_via(a: &Account, key: &MachineIdentity, device: &str, ch: u32, relay: bool) -> (bool, Vec<(u32, u16)>) {
        let (mut routes, mut pending) = (Routes::new(), Pending::new());
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let (text_tx, _text_rx) = mpsc::unbounded_channel();
        let frames = [
            Frame::Open { ch, peer: device.into() },
            Frame::Data {
                ch,
                payload: hello(a, key, device, Vec::new()),
            },
        ];
        for frame in frames {
            if relay {
                on_channel_frame(&a.shared, frame, &mut routes, &mut pending, &out_tx, &text_tx);
            } else {
                on_frame(&a.shared, frame, &mut routes, &mut pending, &out_tx, &text_tx);
            }
        }
        (routes.contains_key(&ch), closes(&mut out_rx))
    }

    /// `docs/host-identity-rotation.md` §3.4 rule 10: a
    /// renewal pause is an overlay on who may control a host. While one that
    /// removes m-1←d-3 is open, d-3's open channel is closed (and its next
    /// stream not served), and a new one is refused on the hub's path and on
    /// a relay's alike; once it is completed — or another one cancelled — d-3
    /// is let in again. Every check is rh-proto's `may_control` (and the
    /// handshake's), so the overlay holds everywhere at once.
    #[tokio::test]
    async fn a_renewal_pause_closes_and_refuses_a_paused_controller_until_it_ends() {
        use rh_proto::chain::{Accept, PausePhase};
        let mut a = policy_account();
        take(
            &mut a,
            vec![Action::SetAccept {
                host: "m-1".into(),
                from: Accept::Devices(vec!["d-3".into()]),
            }],
        );
        attest(&a, "d-1", &a.d1, 0);
        let mut routes = Routes::new();
        let channel = connect(&a, &d3(), "d-3", 4, &mut routes).await.expect("m-1 accepts d-3");
        assert_eq!(served(&channel).await, Some(200));
        assert!(open_via(&a, &d3(), "d-3", 5, false).0, "the hub's path lets it in");
        assert!(open_via(&a, &d3(), "d-3", 6, true).0, "a relay's path lets it in");
        let pause = |op: u8, phase: PausePhase, removals: Vec<(String, Vec<String>)>| Action::RenewalPause {
            op: [op; 16],
            phase,
            removals,
        };
        let removing_d3 = || vec![("m-1".to_string(), vec!["d-3".to_string()])];
        // The pause starts.
        take(&mut a, vec![pause(1, PausePhase::Start, removing_d3())]);
        attest(&a, "d-1", &a.d1, 0);
        {
            let st = a.shared.state.lock().unwrap();
            let chain = st.chain.as_ref().unwrap();
            assert_eq!(chain.paused_edges("m-1"), vec!["d-3".to_string()]);
            assert_eq!(chain.base_accept("m-1"), Accept::Devices(vec!["d-3".into()]), "the base is untouched");
        }
        // The open channel: its next stream is not served, and the chain's
        // change closes it.
        assert_eq!(served(&channel).await, None, "not served while paused");
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        drop_removed(&a.shared, &mut routes, &out_tx);
        match out_rx.try_recv() {
            Ok(Frame::Close { ch, code, reason }) => {
                assert_eq!((ch, code), (4, 4403));
                assert_eq!(reason, PAUSED_REASON, "said as a pause, not a narrowing");
            }
            other => panic!("{other:?}"),
        }
        assert!(routes.is_empty());
        // New ones: refused on both paths.
        for (ch, relay) in [(7, false), (8, true)] {
            let (let_in, closed) = open_via(&a, &d3(), "d-3", ch, relay);
            assert!(!let_in && closed.contains(&(ch, 4401)), "relay {relay}: {closed:?}");
        }
        // Completed: let in again, on both paths, and served.
        take(&mut a, vec![pause(1, PausePhase::Completed, Vec::new())]);
        attest(&a, "d-1", &a.d1, 0);
        assert!(a.shared.state.lock().unwrap().chain.as_ref().unwrap().paused_edges("m-1").is_empty());
        assert!(open_via(&a, &d3(), "d-3", 9, false).0 && open_via(&a, &d3(), "d-3", 10, true).0);
        let channel = connect(&a, &d3(), "d-3", 11, &mut routes).await.expect("in again");
        assert_eq!(served(&channel).await, Some(200));
        // Cancelled: the same.
        take(&mut a, vec![pause(2, PausePhase::Start, removing_d3())]);
        attest(&a, "d-1", &a.d1, 0);
        assert!(!open_via(&a, &d3(), "d-3", 12, true).0, "paused again");
        take(&mut a, vec![pause(2, PausePhase::Cancelled, Vec::new())]);
        attest(&a, "d-1", &a.d1, 0);
        assert!(open_via(&a, &d3(), "d-3", 13, false).0 && open_via(&a, &d3(), "d-3", 14, true).0);
    }

    // ---- control lapses on open channels too ----------------------------

    const SEVEN_DAYS: u64 = rh_proto::chain::CONTROL_FRESHNESS_MS;

    async fn served(channel: &Channel) -> Option<u16> {
        rpc(channel, "GET", "/rh/info", b"").await.map(|(status, _)| status)
    }

    /// An open channel is held to control's seven days as a new one is: once
    /// no admin has vouched for this machine's view for that long, the agent
    /// closes it (4412) — however long the hub keeps the connection up and
    /// a removal to itself — and a new one is refused.
    #[tokio::test]
    async fn an_open_channel_closes_once_no_admin_vouched_for_this_view_in_seven_days() {
        let a = account();
        attest(&a, "d-1", &a.d1, SEVEN_DAYS - 1_500);
        let mut routes = Routes::new();
        let channel = connect(&a, &d2(), "d-2", 4, &mut routes).await.expect("fresh enough to open");
        assert_eq!(served(&channel).await, Some(200));
        tokio::time::sleep(Duration::from_millis(2_500)).await;
        assert_eq!(*a.heard_closes.lock().unwrap(), vec![(4, CHANNEL_STALE)], "closed once its seven days ran out");
        assert_eq!(served(&channel).await, None, "nothing is served on it");
        drop_ended(&mut routes);
        assert!(!routes.contains_key(&4), "its route goes with it");
        assert_eq!(
            connect(&a, &d2(), "d-2", 5, &mut routes).await.err(),
            Some(vec![(5, CHANNEL_STALE)]),
            "nor on a new one"
        );
    }

    /// An admin vouching again before the seven days are out keeps an open
    /// channel going.
    #[tokio::test]
    async fn a_newer_attestation_keeps_an_open_channel_going() {
        let a = account();
        attest(&a, "d-1", &a.d1, SEVEN_DAYS - 1_500);
        let mut routes = Routes::new();
        let channel = connect(&a, &d2(), "d-2", 4, &mut routes).await.expect("fresh enough to open");
        attest(&a, "d-1", &a.d1, 0);
        tokio::time::sleep(Duration::from_millis(2_500)).await;
        assert!(a.heard_closes.lock().unwrap().is_empty());
        assert_eq!(served(&channel).await, Some(200));
    }

    /// The head moving on before an admin vouches for the new one does not
    /// end an open channel (the last word was about a view this one
    /// extends); a new channel still needs exactly this head vouched for.
    #[tokio::test]
    async fn the_head_moving_on_before_an_admin_vouches_for_it_does_not_close_an_open_channel() {
        let mut a = account();
        attest(&a, "d-1", &a.d1, 0);
        let mut routes = Routes::new();
        let channel = connect(&a, &d2(), "d-2", 4, &mut routes).await.expect("fresh");
        take(&mut a, vec![Action::SetRecoveryKey(None)]);
        a.shared.chain_version.send_modify(|v| *v += 1);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(served(&channel).await, Some(200), "still served");
        assert!(a.heard_closes.lock().unwrap().is_empty());
        assert_eq!(
            connect(&a, &d2(), "d-2", 5, &mut routes).await.err(),
            Some(vec![(5, CHANNEL_STALE)]),
            "a new channel needs this very head vouched for"
        );
    }

    /// A chain change that takes away the word an open channel's control
    /// stood on — the admin who vouched is one no longer — closes it then,
    /// not when that word would have run out.
    #[tokio::test]
    async fn a_chain_change_that_takes_away_the_word_an_open_channel_stood_on_closes_it() {
        let mut a = account();
        attest(&a, "d-1", &a.d1, 0);
        let mut routes = Routes::new();
        let channel = connect(&a, &d2(), "d-2", 4, &mut routes).await.expect("fresh");
        // d-2, the sole admin now, may still control this machine; nobody whose word counts has vouched.
        take(
            &mut a,
            vec![Action::SetAdmin {
                id: "d-1".into(),
                admin: false,
            }],
        );
        a.shared.chain_version.send_modify(|v| *v += 1);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(*a.heard_closes.lock().unwrap(), vec![(4, CHANNEL_STALE)]);
        assert_eq!(served(&channel).await, None);
    }

    /// `device`'s lapse on the agent's state now, worked out afresh or on top
    /// of what `lapse` knew.
    fn lapse_at(a: &Account, lapse: &mut ChannelLapse) -> Option<u64> {
        let st = a.shared.state.lock().unwrap();
        lapse.update(&st, now_ms());
        lapse.at()
    }

    /// Whose word counts for an open channel, and until when: a current
    /// admin's other than the device's own, dated no further ahead than the
    /// skew; of this head, or of an earlier one if it was given before this
    /// agent moved past it — and an admin's later word that does not count
    /// does not take back the one before.
    #[test]
    fn an_open_channel_counts_the_words_that_vouch_for_this_view() {
        let mut a = account();
        let mut d2_lapse = ChannelLapse::new("d-2");
        assert_eq!(lapse_at(&a, &mut d2_lapse), None, "nobody vouched");
        attest(&a, "d-1", &a.d1, 1_000);
        let d1_at = a.shared.state.lock().unwrap().attestations["d-1"].time_ms;
        assert_eq!(lapse_at(&a, &mut d2_lapse), Some(d1_at + SEVEN_DAYS));
        assert_eq!(
            lapse_at(&a, &mut ChannelLapse::new("d-1")),
            None,
            "its own word does not count while another admin's could"
        );
        let future = Attestation::create(&a.hub, "d-2", d2().signing_key(), now_ms() + 10 * 60 * 1000);
        assert!(a.shared.state.lock().unwrap().attest(future));
        assert_eq!(lapse_at(&a, &mut ChannelLapse::new("d-1")), None, "nor a word dated too far ahead");
        let at_n = a.hub.clone();
        take(&mut a, vec![Action::SetRecoveryKey(None)]);
        assert_eq!(
            lapse_at(&a, &mut ChannelLapse::new("d-2")),
            Some(d1_at + SEVEN_DAYS),
            "a word on the head before, given before this agent moved past it"
        );
        // This agent took the next entry an hour ago; d-1, held back at the old head, vouches for it afresh.
        {
            let mut st = a.shared.state.lock().unwrap();
            let next = st.chain.as_ref().unwrap().seq();
            let mut seen = st.clock.first_seen().clone();
            seen.insert(next, now_ms() - 60 * 60 * 1000);
            st.clock.restore(seen);
            assert!(st.attest(Attestation::create(&at_n, "d-1", a.d1.signing_key(), now_ms())));
        }
        assert_eq!(
            lapse_at(&a, &mut ChannelLapse::new("d-2")),
            None,
            "a fresh word about a head this agent had moved past counts for nothing"
        );
        assert_eq!(lapse_at(&a, &mut d2_lapse), Some(d1_at + SEVEN_DAYS), "and does not take back d-1's word before");
        // d-1 vouches for a head the hub has not served this machine.
        let ahead = a
            .hub
            .draft(Signer::Device("d-1".into()), Action::SetRecoveryKey(None), 4_000)
            .sign(a.d1.signing_key());
        a.hub.apply(&ahead, &mut ChainClock::history()).unwrap();
        attest(&a, "d-1", &a.d1, 0);
        assert_eq!(lapse_at(&a, &mut ChannelLapse::new("d-2")), None, "a word from beyond this view extends nothing");
        assert_eq!(lapse_at(&a, &mut d2_lapse), Some(d1_at + SEVEN_DAYS), "nor takes anything back");
    }

    /// A sole admin's own word counts for the head it was the sole admin at,
    /// even once a second admin joined: pairing one does not end its
    /// channels before the new one vouches.
    #[test]
    fn a_sole_admins_own_word_counts_for_the_head_it_was_sole_at() {
        let mut a = account();
        take(
            &mut a,
            vec![Action::SetAdmin {
                id: "d-2".into(),
                admin: false,
            }],
        );
        attest(&a, "d-1", &a.d1, 1_000);
        let d1_at = a.shared.state.lock().unwrap().attestations["d-1"].time_ms;
        let mut d1_lapse = ChannelLapse::new("d-1");
        assert_eq!(lapse_at(&a, &mut d1_lapse), Some(d1_at + SEVEN_DAYS), "the sole admin's own word");
        take(
            &mut a,
            vec![Action::SetAdmin {
                id: "d-2".into(),
                admin: true,
            }],
        );
        assert_eq!(
            lapse_at(&a, &mut ChannelLapse::new("d-1")),
            Some(d1_at + SEVEN_DAYS),
            "still: it was the sole admin at the head it vouched for"
        );
        attest(&a, "d-1", &a.d1, 0);
        assert_eq!(
            lapse_at(&a, &mut ChannelLapse::new("d-1")),
            None,
            "its word on the head where it is one of two does not count"
        );
        assert_eq!(lapse_at(&a, &mut d1_lapse), Some(d1_at + SEVEN_DAYS), "nor takes back the one before");
    }

    #[test]
    fn a_channel_looks_at_its_lapse_again_just_after_it_or_every_ten_minutes() {
        assert_eq!(lapse_wait(Some(105), 100), Duration::from_millis(6));
        assert_eq!(lapse_wait(Some(100), 200), Duration::from_millis(1));
        assert_eq!(lapse_wait(Some(u64::MAX), 0), LAPSE_POLL);
        assert_eq!(lapse_wait(None, 0), Duration::ZERO);
    }

    /// A sole admin's renewal, then its first word
    /// on the head where a second admin joined (which does not count, and
    /// replaces the renewal in the state), reaching this agent before the
    /// channel heard of the chain change: the renewal is the channel's all
    /// the same — kept as it arrived — and the channel stays open past its
    /// first word's seven days.
    #[tokio::test]
    async fn a_renewal_replaced_before_the_channel_looked_still_keeps_it_open() {
        let mut a = account();
        take(
            &mut a,
            vec![Action::SetAdmin {
                id: "d-2".into(),
                admin: false,
            }],
        );
        attest(&a, "d-1", &a.d1, SEVEN_DAYS - 1_500);
        let mut routes = Routes::new();
        let channel = connect(&a, &a.d1, "d-1", 4, &mut routes).await.expect("the sole admin's own word");
        // Frames as the hub relays them.
        let relayed = |a: &Account, att: Attestation| {
            let (out, _) = mpsc::unbounded_channel();
            let (text, _) = mpsc::unbounded_channel();
            let frame = Frame::Attest {
                ch: 0,
                attestation: att.encode(),
            };
            on_frame(&a.shared, frame, &mut Routes::new(), &mut Pending::new(), &out, &text);
        };
        // It renews (still the sole admin)…
        relayed(&a, Attestation::create(&a.hub, "d-1", a.d1.signing_key(), now_ms()));
        // …pairs a second admin, and vouches for that head, before the channel heard of the change.
        take(
            &mut a,
            vec![Action::SetAdmin {
                id: "d-2".into(),
                admin: true,
            }],
        );
        relayed(&a, Attestation::create(&a.hub, "d-1", a.d1.signing_key(), now_ms()));
        a.shared.chain_version.send_modify(|v| *v += 1);
        tokio::time::sleep(Duration::from_millis(2_500)).await;
        assert!(a.heard_closes.lock().unwrap().is_empty(), "{:?}", a.heard_closes.lock().unwrap());
        assert_eq!(served(&channel).await, Some(200), "open past its first word's seven days");
    }

    /// Third look: a renewal arriving right after a handshake's
    /// admission — before the channel's own task ran, and replaced at once by
    /// a word that does not count — still reaches the channel: its lapse was
    /// seeded and registered under the admission's state lock.
    #[tokio::test]
    async fn a_renewal_arriving_just_after_admission_reaches_the_channel() {
        let mut a = account();
        take(
            &mut a,
            vec![Action::SetAdmin {
                id: "d-2".into(),
                admin: false,
            }],
        );
        attest(&a, "d-1", &a.d1, SEVEN_DAYS - 1_500);
        let renewal = Attestation::create(&a.hub, "d-1", a.d1.signing_key(), now_ms());
        let pairing = a
            .hub
            .draft(
                Signer::Device("d-1".into()),
                Action::SetAdmin {
                    id: "d-2".into(),
                    admin: true,
                },
                3_000,
            )
            .sign(a.d1.signing_key());
        a.hub.apply(&pairing, &mut ChainClock::history()).unwrap();
        let replacing = Attestation::create(&a.hub, "d-1", a.d1.signing_key(), now_ms());
        let shared = a.shared.clone();
        *a.shared.after_admission.lock().unwrap() = Some(Box::new(move || {
            assert!(take_attestation(&shared, renewal));
            assert!(shared.state.lock().unwrap().absorb(std::slice::from_ref(&pairing), now_ms()).error.is_none());
            assert!(take_attestation(&shared, replacing));
        }));
        let mut routes = Routes::new();
        let channel = connect(&a, &a.d1, "d-1", 4, &mut routes).await.expect("the sole admin's own word");
        a.shared.chain_version.send_modify(|v| *v += 1);
        tokio::time::sleep(Duration::from_millis(2_500)).await;
        assert!(a.heard_closes.lock().unwrap().is_empty(), "{:?}", a.heard_closes.lock().unwrap());
        assert_eq!(served(&channel).await, Some(200), "open past its first word's seven days");
    }

    /// Third look: a channel that ended leaves the lapse registry
    /// then, not only once an attestation arrives.
    #[tokio::test]
    async fn an_ended_channel_leaves_the_lapse_registry() {
        let a = account();
        attest(&a, "d-1", &a.d1, SEVEN_DAYS - 1_500);
        let mut routes = Routes::new();
        let channel = connect(&a, &d2(), "d-2", 4, &mut routes).await.expect("fresh enough to open");
        assert_eq!(served(&channel).await, Some(200));
        assert_eq!(a.shared.lapses.lock().unwrap().len(), 1);
        tokio::time::sleep(Duration::from_millis(2_500)).await;
        assert_eq!(*a.heard_closes.lock().unwrap(), vec![(4, CHANNEL_STALE)]);
        assert!(a.shared.lapses.lock().unwrap().is_empty(), "gone with its channel");
    }

    /// Third look (optional): who was the sole admin as of a seq is
    /// worked out once, and kept with the entry's hash.
    #[test]
    fn the_sole_admin_as_of_a_seq_is_worked_out_once() {
        let mut a = account();
        take(
            &mut a,
            vec![Action::SetAdmin {
                id: "d-2".into(),
                admin: false,
            }],
        );
        attest(&a, "d-1", &a.d1, 1_000);
        let vouched = a.shared.state.lock().unwrap().attestations["d-1"].seq;
        take(
            &mut a,
            vec![Action::SetAdmin {
                id: "d-2".into(),
                admin: true,
            }],
        );
        assert!(lapse_at(&a, &mut ChannelLapse::new("d-1")).is_some());
        let st = a.shared.state.lock().unwrap();
        let known = st.sole_admins.lock().unwrap();
        let (hash, sole) = known.get(&vouched).expect("worked out for the seq it vouched for");
        assert_eq!(sole.as_deref(), Some("d-1"));
        assert_eq!(Some(*hash), st.chain.as_ref().unwrap().hash_at(vouched));
    }

    /// A sole admin pairing a second admin keeps every open channel of its
    /// own: pairing one must not end its channels before the new admin
    /// vouches.
    #[tokio::test]
    async fn a_sole_admin_pairing_a_second_admin_keeps_its_open_channels() {
        let mut a = account();
        take(
            &mut a,
            vec![Action::SetAdmin {
                id: "d-2".into(),
                admin: false,
            }],
        );
        attest(&a, "d-1", &a.d1, 0);
        let mut routes = Routes::new();
        let channel = connect(&a, &a.d1, "d-1", 4, &mut routes).await.expect("the sole admin's own word");
        take(
            &mut a,
            vec![Action::SetAdmin {
                id: "d-2".into(),
                admin: true,
            }],
        );
        a.shared.chain_version.send_modify(|v| *v += 1);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(served(&channel).await, Some(200));
        // It vouches for the new head (where its word no longer counts), and the chain moves on again.
        attest(&a, "d-1", &a.d1, 0);
        take(&mut a, vec![Action::SetRecoveryKey(None)]);
        a.shared.chain_version.send_modify(|v| *v += 1);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(served(&channel).await, Some(200));
        assert!(a.heard_closes.lock().unwrap().is_empty());
    }

    /// A hub that holds one honest admin back at an old head
    /// and forwards its fresh words about it cannot keep an open channel
    /// going past seven days from the last word given before this agent
    /// moved on.
    #[tokio::test]
    async fn fresh_words_about_a_head_this_agent_moved_past_do_not_keep_a_channel_open() {
        let mut a = account();
        attest(&a, "d-1", &a.d1, SEVEN_DAYS - 1_500);
        let at_n = a.hub.clone();
        let mut routes = Routes::new();
        let channel = connect(&a, &d2(), "d-2", 4, &mut routes).await.expect("fresh enough to open");
        take(&mut a, vec![Action::SetRecoveryKey(None)]);
        {
            let mut st = a.shared.state.lock().unwrap();
            let next = st.chain.as_ref().unwrap().seq();
            let mut seen = st.clock.first_seen().clone();
            seen.insert(next, now_ms() - 60 * 60 * 1000);
            st.clock.restore(seen);
        }
        a.shared.chain_version.send_modify(|v| *v += 1);
        let fresh = Attestation::create(&at_n, "d-1", a.d1.signing_key(), now_ms());
        assert!(a.shared.state.lock().unwrap().attest(fresh));
        tokio::time::sleep(Duration::from_millis(2_500)).await;
        assert_eq!(*a.heard_closes.lock().unwrap(), vec![(4, CHANNEL_STALE)]);
        assert_eq!(served(&channel).await, None);
    }

    /// A stream that races a chain change taking away the
    /// word the channel stood on (its chain version not seen yet) is judged
    /// on the chain as it is, not on the lapse worked out before.
    #[tokio::test]
    async fn a_stream_racing_a_chain_change_is_judged_on_the_chain_as_it_is() {
        let mut a = account();
        attest(&a, "d-1", &a.d1, 0);
        let mut routes = Routes::new();
        let channel = connect(&a, &d2(), "d-2", 4, &mut routes).await.expect("fresh");
        assert_eq!(served(&channel).await, Some(200));
        take(
            &mut a,
            vec![Action::SetAdmin {
                id: "d-1".into(),
                admin: false,
            }],
        );
        assert_eq!(served(&channel).await, None, "not served");
        assert_eq!(*a.heard_closes.lock().unwrap(), vec![(4, CHANNEL_STALE)]);
    }

    /// A management request is held to the twelve hours when
    /// it is dispatched, not only when its stream opened — a device could
    /// hold back its `End` past them.
    #[tokio::test]
    async fn a_management_request_is_judged_again_when_it_is_dispatched() {
        let a = account();
        attest(&a, "d-1", &a.d1, rh_proto::chain::MANAGE_FRESHNESS_MS - 1_500);
        let mut routes = Routes::new();
        let channel = connect(&a, &d2(), "d-2", 4, &mut routes).await.expect("fresh");
        let held = rpc_held(&channel, "POST", "/rh/display-name", br#"{"name":"held"}"#, Duration::from_millis(2_500)).await;
        let (status, body) = held.expect("an answer");
        assert_eq!((status, body["error"].as_str(), body["reason"].as_str()), (412, Some("stale"), Some("stale")));
        assert_eq!(a.shared.state.lock().unwrap().display_name, None, "not renamed");
    }

    /// Routes of channels that ended here go — neither the
    /// hub nor a relay echoes this agent's `CLOSE`.
    #[test]
    fn routes_of_channels_that_ended_here_are_forgotten() {
        let (ended, rx) = mpsc::unbounded_channel();
        drop(rx);
        let (open, rx) = mpsc::unbounded_channel();
        std::mem::forget(rx);
        let mut routes: Routes = HashMap::from([(1, ("d-1".to_string(), [1; 32], ended)), (2, ("d-2".to_string(), [2; 32], open))]);
        drop_ended(&mut routes);
        assert_eq!(routes.keys().copied().collect::<Vec<_>>(), vec![2]);
    }

    // ---- a member that stops running AI ---------------------------------

    /// `SetHost false` for this machine: it stays in the account and keeps
    /// its state, closes every channel and accepts none, and its streams'
    /// writer loses the connection; the chain making it a host again (with
    /// its next machine key epoch) brings the writer back and lets devices in.
    #[tokio::test]
    async fn a_member_that_stops_running_ai_writes_no_streams_accepts_no_channels_and_resumes() {
        let (sync, mut events) = super::super::streams::Handle::captured();
        let mut a = account_with(sync);
        attest(&a, "d-2", &d2(), 0);
        let (frames, _frames_rx) = mpsc::unbounded_channel();
        let (text, _text_rx) = mpsc::unbounded_channel();
        // Whether the writer was given the connection, or had it taken back.
        let mut connection = move || {
            std::iter::from_fn(|| events.try_recv().ok())
                .filter_map(|e| match e {
                    super::super::streams::Event::Connected(..) => Some("connected"),
                    super::super::streams::Event::Disconnected => Some("disconnected"),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let mut writing = false;
        sync_streams(&a.shared, &mut writing, &frames, &text);
        assert!(writing);
        assert_eq!(connection(), vec!["connected"]);
        let mut routes = Routes::new();
        let _open = connect(&a, &a.d1, "d-1", 1, &mut routes).await.expect("a host lets an admin in");

        let stop = a
            .hub
            .draft(
                Signer::Device("d-1".into()),
                Action::SetHost {
                    id: "m-1".into(),
                    host: false,
                },
                3_000,
            )
            .sign(a.d1.signing_key());
        a.hub.apply(&stop, &mut ChainClock::history()).unwrap();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let frame = Frame::Chain {
            ch: 0,
            entries: vec![stop.encode()],
        };
        on_frame(&a.shared, frame, &mut routes, &mut Pending::new(), &out_tx, &text);
        assert!(routes.is_empty() && closes(&mut out_rx) == vec![(1, 4403)], "every channel closes");
        {
            let st = a.shared.state.lock().unwrap();
            assert!(st.enrolled() && st.not_hosting() && !st.hosting() && !st.removed());
            assert_eq!(
                super::super::control::AgentStatus::of(&st).state,
                super::super::control::Standing::NotHosting
            );
        }
        assert!(!a.shared.cancel.is_cancelled(), "the link keeps running, to hear when it runs AI again");
        sync_streams(&a.shared, &mut writing, &frames, &text);
        assert!(!writing);
        assert_eq!(connection(), vec!["disconnected"]);
        attest(&a, "d-2", &d2(), 0);
        let refused = connect(&a, &a.d1, "d-1", 2, &mut routes).await.err().expect("no channel");
        assert_eq!(refused, vec![(2, 4401)]);
        let saved = AgentState::load(&a.shared.paths).unwrap().expect("its state stays");
        assert!(saved.not_hosting());

        let key = [7u8; 32];
        let restart = vec![
            Action::SetHost {
                id: "m-1".into(),
                host: true,
            },
            Action::KeyEpochs(rh_proto::chain::KeyCommits {
                user: None,
                machines: vec![(
                    "m-1".into(),
                    rh_proto::chain::KeyCommit {
                        epoch: 0,
                        hash: rh_proto::chain::machine_key_commit("a-1", "m-1", 0, &key),
                    },
                )],
            }),
        ];
        let entries: Vec<Entry> = restart
            .into_iter()
            .map(|action| {
                let e = a.hub.draft(Signer::Device("d-1".into()), action, 4_000).sign(a.d1.signing_key());
                a.hub.apply(&e, &mut ChainClock::history()).unwrap();
                e
            })
            .collect();
        let frame = Frame::Chain {
            ch: 0,
            entries: entries.iter().map(Entry::encode).collect(),
        };
        on_frame(&a.shared, frame, &mut routes, &mut Pending::new(), &out_tx, &text);
        assert!(a.shared.state.lock().unwrap().hosting(), "it runs AI again");
        sync_streams(&a.shared, &mut writing, &frames, &text);
        assert!(writing);
        assert_eq!(connection(), vec!["connected"]);
        attest(&a, "d-2", &d2(), 0);
        assert!(connect(&a, &a.d1, "d-1", 3, &mut routes).await.is_ok(), "and lets devices in again");
    }

    /// A hub batch that fails half-way is acted on, at the link, for
    /// everything it applied: the removed device's channel closed, both
    /// changes alerted, the state saved ([`assert_acted_on`]).
    #[tokio::test]
    async fn a_hub_batch_that_fails_half_way_is_acted_on_for_what_took_effect() {
        let mut a = account();
        let batch = half_bad_batch(&mut a);
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let (text_tx, _text_rx) = mpsc::unbounded_channel();
        let mut routes = routes_with_d2();
        let frame = Frame::Chain {
            ch: 0,
            entries: batch.iter().map(Entry::encode).collect(),
        };
        on_frame(&a.shared, frame, &mut routes, &mut Pending::new(), &out_tx, &text_tx);
        assert_acted_on(&a, &routes, &mut out_rx);
    }

    /// The same through a device's hello: its entries apply up to the
    /// spoiled one, the handshake is refused — and what applied is acted on.
    #[tokio::test]
    async fn a_hello_that_fails_half_way_is_acted_on_for_what_took_effect() {
        let mut a = account();
        let batch = half_bad_batch(&mut a);
        let machine_dh = a.shared.identity.dh_public();
        let binding = Binding {
            account: "a-1".into(),
            device: "d-1".into(),
            machine: "m-1".into(),
        };
        let mut initiator = rh_proto::noise::Initiator::new(a.d1.dh(), &machine_dh, &binding).unwrap();
        let body = handshake::HelloBody {
            chain_seq: a.hub.seq(),
            chain_head: a.hub.head(),
            entries: batch,
            client: Vec::new(),
        };
        let hello = initiator.write_hello(now_ms(), &body.encode().unwrap()).unwrap();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let (text_tx, _text_rx) = mpsc::unbounded_channel();
        let mut routes = routes_with_d2();
        accept(&a.shared, 9, "d-1".into(), &hello, &mut routes, &out_tx, &text_tx);
        assert_acted_on(&a, &routes, &mut out_rx);
    }

    /// A self-activation with the recovery words — the moment a recovered
    /// device starts to count — is an alert like every other change to who is
    /// in the account, and nothing else is.
    #[test]
    fn every_change_to_who_is_in_the_account_is_an_alert() {
        let ak = MachineIdentity::from_seed(&[1; 32]);
        let dev = MachineIdentity::from_seed(&[2; 32]);
        let info = DeviceInfo {
            id: "d-1".into(),
            sign_key: dev.sign_public(),
            dh_key: dev.dh_public(),
            full: true,
        };
        let genesis = genesis_draft("a-1", ak.sign_public(), info, None, 1_000).sign(ak.signing_key());
        let state = ChainState::verify(std::slice::from_ref(&genesis), None, &mut ChainClock::history()).unwrap();
        let entry = |signer: Signer, action: Action| state.draft(signer, action, 2_000).sign(dev.signing_key());
        let said = |e: &Entry| account_change(e, None);
        assert_eq!(
            said(&entry(Signer::Recovery, Action::ActivateRecovered("d-r".into()))).as_deref(),
            Some("device d-r activated itself with the recovery words")
        );
        assert!(said(&entry(Signer::Device("d-1".into()), Action::ApproveDevice("d-r".into()))).is_some_and(|s| s.contains("approved by d-1")));
        assert!(said(&entry(Signer::Device("d-1".into()), Action::RemoveDevice("d-2".into()))).is_some());
        // Machines too: a machine added or removed is said everywhere.
        assert_eq!(
            said(&entry(Signer::Device("d-1".into()), Action::RemoveMachine("m-1".into()))).as_deref(),
            Some("device m-1 (runs AI) was removed by d-1")
        );
        let other = MachineIdentity::from_seed(&[5; 32]);
        assert_eq!(
            said(&entry(Signer::Device("d-1".into()), Action::AddMachine(other.info("m-2")))).as_deref(),
            Some("device m-2 (runs AI) was added by d-1")
        );
        // Key epochs on their own are said (N2); as part of a removal's rotation, the removal says it.
        let keys = entry(
            Signer::Device("d-1".into()),
            Action::KeyEpochs(rh_proto::chain::KeyCommits {
                user: None,
                machines: vec![("m-1".into(), rh_proto::chain::KeyCommit { epoch: 7, hash: [0; 32] })],
            }),
        );
        assert_eq!(
            account_change(&keys, None).as_deref(),
            Some("new keys were committed by d-1: 1 content key epoch(s)")
        );
        let removal = entry(Signer::Device("d-1".into()), Action::RemoveDevice("d-2".into()));
        assert!(account_change(&keys, Some(&removal)).is_none());
        let by_other = entry(Signer::Device("d-9".into()), Action::RemoveDevice("d-2".into()));
        assert!(
            account_change(&keys, Some(&by_other)).is_some(),
            "another signer's removal covers nothing of it"
        );
    }

    /// Every device added (with what it may do),
    /// every grant of admin, every host joining or restarting, every passkey
    /// slot and passkey join, every change to whom a host accepts control
    /// from is an alert this agent raises from its own chain — a hub cannot
    /// hold it back; the key epochs that come with a new host are covered by
    /// its addition.
    #[test]
    fn every_v3_change_to_who_is_in_the_account_or_who_controls_it_is_an_alert() {
        use rh_proto::chain::{Accept, DeviceV3, Flags, KeyCommit, KeyCommits};
        let ak = MachineIdentity::from_seed(&[1; 32]);
        let dev = MachineIdentity::from_seed(&[2; 32]);
        let genesis = genesis_draft("a-1", ak.sign_public(), info("d-1", &dev), None, 1_000).sign(ak.signing_key());
        let mut state = ChainState::verify(std::slice::from_ref(&genesis), None, &mut ChainClock::history()).unwrap();
        // Any v3 entry makes what follows v3; drafts here are only described, not applied.
        let host = MachineIdentity::from_seed(&[6; 32]);
        let add_host = state
            .draft(
                Signer::Device("d-1".into()),
                Action::AddDevice3(DeviceV3 {
                    id: "h-2".into(),
                    sign_key: host.sign_public(),
                    dh_key: host.dh_public(),
                    flags: Flags { host: true, admin: false },
                }),
                2_000,
            )
            .sign(dev.signing_key());
        state.apply(&add_host, &mut ChainClock::history()).unwrap();
        let said = |e: &Entry| account_change(e, None);
        assert_eq!(said(&add_host).as_deref(), Some("device h-2 (runs AI) was added by d-1"));
        let entry = |action: Action| state.draft(Signer::Device("d-1".into()), action, 3_000).sign(dev.signing_key());
        let keys = entry(Action::KeyEpochs(KeyCommits {
            user: None,
            machines: vec![("h-2".into(), KeyCommit { epoch: 0, hash: [0; 32] })],
        }));
        assert!(
            account_change(&keys, Some(&add_host)).is_none(),
            "the new host's own key is part of its addition"
        );
        for (action, needle) in [
            (
                Action::SetAdmin {
                    id: "h-2".into(),
                    admin: true,
                },
                "h-2 can now manage the account",
            ),
            (
                Action::SetAdmin {
                    id: "h-2".into(),
                    admin: false,
                },
                "h-2 can no longer manage the account",
            ),
            (
                Action::SetHost {
                    id: "h-2".into(),
                    host: false,
                },
                "h-2 no longer runs AI",
            ),
            (
                Action::SetHost {
                    id: "h-2".into(),
                    host: true,
                },
                "h-2 runs AI again",
            ),
            (Action::RemoveDevice3("h-2".into()), "device h-2 was removed by d-1"),
            (Action::PasskeySlotRemove("p-1".into()), "passkey slot p-1 was removed"),
            (
                Action::SetAccept {
                    host: "m-1".into(),
                    from: Accept::Devices(vec!["h-2".into()]),
                },
                "m-1 now accepts control from admin devices and h-2",
            ),
            (
                Action::SetAccept {
                    host: "m-1".into(),
                    from: Accept::All,
                },
                "m-1 now accepts control from every device of the account",
            ),
        ] {
            let what = said(&entry(action)).unwrap_or_default();
            assert!(what.contains(needle), "{what:?} lacks {needle:?}");
        }
        let joined = state
            .draft(
                Signer::Passkey("p-1".into()),
                Action::AddDevice3(DeviceV3 {
                    id: "d-p".into(),
                    sign_key: host.sign_public(),
                    dh_key: host.dh_public(),
                    flags: Flags::NONE,
                }),
                4_000,
            )
            .sign(dev.signing_key());
        assert!(said(&joined).is_some_and(|s| s.contains("joining with passkey slot p-1") && s.contains("approve it")));
    }

    /// Host identity rotation (`docs/host-identity-rotation.md` §3.4 "告警"):
    /// a host renewing its identity key is a note with the new fingerprint as
    /// the device list shows it, said once — the machine key epoch that must
    /// follow is part of it — and this agent applies the entry like any other.
    #[test]
    fn a_host_renewing_its_identity_key_is_a_note_with_the_new_fingerprint() {
        use rh_proto::chain::{Assurance, DeviceV3, Flags, HostRotation, KeyCommit, KeyCommits};
        let ak = MachineIdentity::from_seed(&[1; 32]);
        let dev = MachineIdentity::from_seed(&[2; 32]);
        let genesis = genesis_draft("a-1", ak.sign_public(), info("d-1", &dev), None, 1_000).sign(ak.signing_key());
        let mut state = ChainState::verify(std::slice::from_ref(&genesis), None, &mut ChainClock::history()).unwrap();
        let (old, new) = (MachineIdentity::from_seed(&[6; 32]), MachineIdentity::from_seed(&[7; 32]));
        let key = |epoch| {
            Action::KeyEpochs(KeyCommits {
                user: None,
                machines: vec![("h-2".into(), KeyCommit { epoch, hash: [epoch as u8; 32] })],
            })
        };
        let mut push = |state: &mut ChainState, action: Action| {
            let e = state.draft(Signer::Device("d-1".into()), action, 2_000 + state.seq()).sign(dev.signing_key());
            state.apply(&e, &mut ChainClock::history()).unwrap();
            e
        };
        let add = DeviceV3 {
            id: "h-2".into(),
            sign_key: old.sign_public(),
            dh_key: old.dh_public(),
            flags: Flags { host: true, admin: false },
        };
        push(&mut state, Action::AddDevice3(add));
        push(&mut state, key(0));
        let mut r = HostRotation {
            host: "h-2".into(),
            sign_key: new.sign_public(),
            dh_key: new.dh_public(),
            staged_seq: state.seq(),
            staged_head: state.head(),
            nonce: [1; 16],
            assurance: Assurance::Auto,
            old_sig: [0; 64],
            new_sig: [0; 64],
        };
        let body = r.body("a-1", &old.sign_public(), &old.dh_public());
        r.old_sig = rh_proto::sig::sign(old.signing_key(), rh_proto::sig::HOST_ROTATE, &body);
        r.new_sig = rh_proto::sig::sign(new.signing_key(), rh_proto::sig::HOST_ROTATE, &body);
        let rotate = push(&mut state, Action::RotateHostKey(r.clone()));
        let keys = push(&mut state, key(1));
        let fp = rh_proto::identity::device_fingerprint_text(&new.sign_public(), &new.dh_public());
        assert_eq!(
            account_change(&rotate, None),
            Some(format!("device h-2 renewed its identity key (new fingerprint {fp}, written by d-1)"))
        );
        assert!(account_change(&keys, Some(&rotate)).is_none(), "its machine key is part of the renewal");
        let assured = state
            .draft(
                Signer::Device("d-1".into()),
                Action::RotateHostKey(HostRotation {
                    assurance: Assurance::OnDevice,
                    ..r
                }),
                9_000,
            )
            .sign(dev.signing_key());
        assert!(account_change(&assured, None).is_some_and(|s| s.contains("confirmed on the device")));
        // The account's renewal policy (kind 21) is a note too, each way.
        let policy = |on: bool| {
            state
                .draft(Signer::Device("d-1".into()), Action::SetRenewalPolicy { always_assured: on }, 9_001)
                .sign(dev.signing_key())
        };
        assert_eq!(
            account_change(&policy(true), None).as_deref(),
            Some("identity key renewals must now be confirmed on the device (set by d-1)")
        );
        assert_eq!(
            account_change(&policy(false), None).as_deref(),
            Some("identity key renewals may now be automatic (set by d-1)")
        );
        // A renewal pause (kind 22): its start names the control it pauses;
        // an end names the start's, from this agent's chain.
        let marker = |phase: rh_proto::chain::PausePhase, removals: &[(&str, &[&str])]| {
            state
                .draft(
                    Signer::Device("d-1".into()),
                    Action::RenewalPause {
                        op: [7; 16],
                        phase,
                        removals: removals.iter().map(|(h, ids)| (h.to_string(), ids.iter().map(|i| i.to_string()).collect())).collect(),
                    },
                    9_002,
                )
                .sign(dev.signing_key())
        };
        use rh_proto::chain::PausePhase::{Cancelled, Completed, Start};
        assert_eq!(
            account_change(&marker(Start, &[("h-2", &["m-1", "d-2"][..]), ("m-1", &["h-2"][..])]), None).as_deref(),
            Some("control by m-1, d-2 over h-2; h-2 over m-1 is paused while identity keys are renewed (by d-1)")
        );
        let started = vec![("h-2".to_string(), vec!["m-1".to_string()])];
        assert_eq!(
            account_change_in(&marker(Completed, &[]), None, Some(&started)).as_deref(),
            Some("control by m-1 over h-2 resumes (by d-1)")
        );
        assert_eq!(
            account_change_in(&marker(Cancelled, &[]), None, Some(&started)).as_deref(),
            Some("the pause of control by m-1 over h-2 was ended (by d-1)")
        );
        assert_eq!(
            account_change(&marker(Completed, &[]), None).as_deref(),
            Some("control by the paused devices resumes (by d-1)")
        );
        // From this agent's chain: "resumes" names only what comes back — not an
        // edge a hand SetAccept dropped from the base during the pause.
        let plain = |id: &str, seed: u8| {
            let k = MachineIdentity::from_seed(&[seed; 32]);
            Action::AddDevice3(DeviceV3 {
                id: id.into(),
                sign_key: k.sign_public(),
                dh_key: k.dh_public(),
                flags: Flags::NONE,
            })
        };
        let accept = |ids: &[&str]| Action::SetAccept {
            host: "h-2".into(),
            from: rh_proto::chain::Accept::Devices(ids.iter().map(|i| i.to_string()).collect()),
        };
        let pause = |op: u8, phase: rh_proto::chain::PausePhase, ids: &[&str]| Action::RenewalPause {
            op: [op; 16],
            phase,
            removals: if ids.is_empty() { vec![] } else { vec![("h-2".into(), ids.iter().map(|i| i.to_string()).collect())] },
        };
        push(&mut state, plain("d-2", 8));
        push(&mut state, plain("d-3", 9));
        push(&mut state, accept(&["d-2", "d-3"]));
        let start = push(&mut state, pause(1, Start, &["d-2", "d-3"]));
        assert_eq!(
            account_change_in(&start, None, pause_notice_edges(&state, &start).as_deref()).as_deref(),
            Some("control by d-2, d-3 over h-2 is paused while identity keys are renewed (by d-1)")
        );
        push(&mut state, accept(&["d-3"]));
        push(&mut state, key(2));
        let done = push(&mut state, pause(1, Completed, &[]));
        assert_eq!(
            account_change_in(&done, None, pause_notice_edges(&state, &done).as_deref()).as_deref(),
            Some("control by d-3 over h-2 resumes (by d-1)"),
            "d-2 left the base during the pause"
        );
        push(&mut state, pause(2, Start, &["d-3"]));
        push(&mut state, Action::RemoveDevice3("d-3".into()));
        let done = push(&mut state, pause(2, Completed, &[]));
        assert_eq!(
            account_change_in(&done, None, pause_notice_edges(&state, &done).as_deref()).as_deref(),
            Some("a renewal pause was completed; none of the control it paused comes back (by d-1)")
        );
        // The renewed host is h-2 with its new keys, here as everywhere.
        assert_eq!(state.machine("h-2").map(|d| d.info.sign_key), Some(new.sign_public()));
        assert_eq!(state.keys_at("h-2", rotate.seq() - 1).map(|k| k.sign_key), Some(old.sign_public()));
    }

    #[test]
    fn deferred_mail_is_bounded_without_dropping_any_silently() {
        let mail = |id: i64, epoch: u32| json!({"t": "mail", "id": id, "epoch": epoch});
        let mut deferred = Vec::new();
        for id in 0..MAX_DEFERRED as i64 {
            assert!(defer(&mut deferred, mail(id, 5)).is_none());
        }
        assert!(defer(&mut deferred, mail(3, 5)).is_none(), "the same mail again is held once");
        assert_eq!(deferred.len(), MAX_DEFERRED);
        let out = defer(&mut deferred, mail(1000, 5)).expect("the oldest is pushed out");
        assert_eq!(out["id"], 0);
        assert_eq!(deferred.len(), MAX_DEFERRED);
        // Pushed out: acknowledged when moot, else released to the mailbox — never dropped.
        assert_eq!(evicted_message(&mail(0, 2), Some(3))["t"], "mail_ack");
        assert_eq!(evicted_message(&mail(0, 5), Some(3))["t"], "mail_release");
        assert_eq!(evicted_message(&mail(0, 5), None)["t"], "mail_release");
    }

    fn closes(out: &mut mpsc::UnboundedReceiver<Frame>) -> Vec<(u32, u16)> {
        let mut closed = Vec::new();
        while let Ok(frame) = out.try_recv() {
            if let Frame::Close { ch, code, .. } = frame {
                closed.push((ch, code));
            }
        }
        closed
    }

    /// A relay is not trusted, and every `OPEN` it sends waits until that
    /// channel's first message or `CLOSE`: unbounded, a relay could grow the
    /// agent's memory without bound. At most 256 wait per connection; the
    /// rest are refused.
    #[tokio::test]
    async fn channels_opening_are_bounded_per_connection() {
        let a = account();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let (text_tx, _text_rx) = mpsc::unbounded_channel();
        let mut routes = Routes::new();
        let mut pending = Pending::new();
        for ch in 1..=300u32 {
            let open = Frame::Open { ch, peer: "d-1".into() };
            on_channel_frame(&a.shared, open, &mut routes, &mut pending, &out_tx, &text_tx);
        }
        assert_eq!(pending.len(), 256);
        assert_eq!(closes(&mut out_rx), (257..=300).map(|ch| (ch, 4409)).collect::<Vec<_>>());
        let again = Frame::Open { ch: 1, peer: "d-1".into() };
        on_channel_frame(&a.shared, again, &mut routes, &mut pending, &out_tx, &text_tx);
        assert!(closes(&mut out_rx).is_empty(), "an OPEN already waiting takes no more room");
    }

    /// An `OPEN` with no first message waits 30 s at most, then it is refused
    /// and its room is free again.
    #[tokio::test]
    async fn channels_opening_wait_thirty_seconds_at_most() {
        let a = account();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let (text_tx, _text_rx) = mpsc::unbounded_channel();
        let mut routes = Routes::new();
        let mut pending = Pending::new();
        for ch in 1..=MAX_PENDING as u32 {
            let open = Frame::Open { ch, peer: "d-1".into() };
            on_channel_frame(&a.shared, open, &mut routes, &mut pending, &out_tx, &text_tx);
        }
        let long_ago = Instant::now() - PENDING_FOR - Duration::from_secs(1);
        for ch in 1..=10 {
            pending.get_mut(&ch).unwrap().1 = long_ago;
        }
        let open = Frame::Open {
            ch: 1000,
            peer: "d-1".into(),
        };
        on_channel_frame(&a.shared, open, &mut routes, &mut pending, &out_tx, &text_tx);
        let mut closed = closes(&mut out_rx);
        closed.sort();
        assert_eq!(closed, (1..=10).map(|ch| (ch, 4408)).collect::<Vec<_>>(), "refused, not left waiting");
        assert!(pending.contains_key(&1000), "and the new one has room");
        assert_eq!(pending.len(), MAX_PENDING - 10 + 1);
        // Also with no new OPEN: the connection's periodic check.
        expire_pending(&mut pending, &out_tx, Instant::now() + PENDING_FOR);
        assert!(pending.is_empty());
        assert_eq!(closes(&mut out_rx).len(), MAX_PENDING - 10 + 1);
    }

    /// A hub on loopback that answers every sign-in with `error removed` and
    /// a 4403 close: what a lying (or wrong) hub can say. Its port, and how
    /// many sign-ins it refused.
    async fn refusing_hub() -> (u16, Arc<std::sync::atomic::AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let refused = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = refused.clone();
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let count = count.clone();
                tokio::spawn(async move {
                    let Ok(mut ws) = tokio_tungstenite::accept_async(tcp).await else {
                        return;
                    };
                    let challenge = json!({"t": "challenge", "nonce": "00".repeat(32), "hub": "ws://127.0.0.1", "version": 1});
                    let _ = ws.send(Message::Text(challenge.to_string().into())).await;
                    let _ = ws.next().await;
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let error = json!({"t": "error", "code": "removed", "message": "this device was removed from the account"});
                    let _ = ws.send(Message::Text(error.to_string().into())).await;
                    let _ = ws
                        .send(Message::Close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                            code: 4403.into(),
                            reason: "this device was removed from the account".into(),
                        })))
                        .await;
                });
            }
        });
        (port, refused)
    }

    /// A sign-in the hub refuses as "removed" must not mark this machine
    /// removed — that would stop the link for good, say "removed" in the
    /// settings and offer to wipe the keys — on the hub's word alone. Only
    /// this machine's own verified chain removes it; a refusal is a failure
    /// like any other: said, and retried.
    #[tokio::test]
    async fn the_hub_saying_removed_is_not_a_removal() {
        let a = account();
        let (port, refused) = refusing_hub().await;
        a.shared.state.lock().unwrap().hub = format!("ws://127.0.0.1:{port}");
        let task = tokio::spawn(run(a.shared.clone()));
        let deadline = Instant::now() + Duration::from_secs(10);
        while refused.load(std::sync::atomic::Ordering::SeqCst) < 2 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(refused.load(std::sync::atomic::Ordering::SeqCst) >= 2, "it tries again");
        assert!(!a.shared.cancel.is_cancelled() && !task.is_finished(), "the link keeps running");
        assert!(!a.shared.state.lock().unwrap().removed(), "not removed on the hub's word");
        let saved = AgentState::load(&a.shared.paths).unwrap();
        assert!(saved.is_none_or(|st| !st.removed()), "nor saved as removed");
        let health = a.shared.health.lock().unwrap().clone();
        assert!(health.refused.as_deref().is_some_and(|r| r.contains("removed")), "{health:?}");
        a.shared.cancel.cancel();
        let _ = task.await;
    }

    /// The same machine's own chain removing it does stop the link.
    #[tokio::test]
    async fn the_verified_chain_removing_this_machine_stops_the_link() {
        let mut a = account();
        // The other devices' keys it held as a device go with it.
        let mut keys = crate::aggregator::keys::HostKeys::load(&a.shared.paths, "a-1", "m-1").unwrap();
        keys.put("m-2", 0, &[2; 32]).unwrap();
        assert!(crate::aggregator::keys::path(&a.shared.paths).exists());
        let rm = a
            .hub
            .draft(Signer::Device("d-1".into()), Action::RemoveMachine("m-1".into()), 3_000)
            .sign(a.d1.signing_key());
        a.hub.apply(&rm, &mut ChainClock::history()).unwrap();
        let (out_tx, _out_rx) = mpsc::unbounded_channel();
        let (text_tx, _text_rx) = mpsc::unbounded_channel();
        let frame = Frame::Chain {
            ch: 0,
            entries: vec![rm.encode()],
        };
        on_frame(&a.shared, frame, &mut Routes::new(), &mut Pending::new(), &out_tx, &text_tx);
        assert!(a.shared.state.lock().unwrap().removed());
        assert!(a.shared.cancel.is_cancelled(), "the link stops");
        assert!(AgentState::load(&a.shared.paths).unwrap().unwrap().removed(), "and stays stopped");
        assert!(!crate::aggregator::dir(&a.shared.paths).exists(), "the other devices' keys are gone");
    }

    /// Stopping a link — also one that already ended on its own, which is
    /// what a restart does first — ends its device role, however stuck it is:
    /// a restart must not leave the retired role running.
    #[tokio::test]
    async fn stopping_a_link_that_ended_ends_its_device_role_too() {
        struct Dropped(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let a = account();
        let ended = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Dropped(ended.clone());
        let copies_ended = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let copies_flag = Dropped(copies_ended.clone());
        // A device role stuck for good: it never looks at the token.
        let control = tokio::spawn(async move {
            let _flag = flag;
            std::future::pending::<()>().await;
        });
        // And its copies task too.
        let copies = tokio::spawn(async move {
            let _flag = copies_flag;
            std::future::pending::<()>().await;
        });
        let running = Running {
            shared: a.shared.clone(),
            task: tokio::spawn(async {}),
            sync_task: tokio::spawn(async {}),
            control: Some(control),
            control_status: Arc::new(Mutex::new(crate::aggregator::controller::Status::default())),
            copies: Some(copies),
            copies_status: Arc::default(),
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(running.finished(), "the link itself ended");
        tokio::time::timeout(Duration::from_secs(5), running.stop()).await.expect("stop returns");
        assert!(ended.load(std::sync::atomic::Ordering::SeqCst), "the stuck device role was ended");
        assert!(copies_ended.load(std::sync::atomic::Ordering::SeqCst), "the stuck copies task was ended");
    }

    /// A machine whose chain removed it — learnt by a build that kept
    /// `aggregator/`, or before a stop — must never start its link again, and
    /// loses the other devices' keys when it tries.
    #[tokio::test]
    async fn a_removed_machine_does_not_start_and_its_other_devices_keys_go() {
        let mut a = account();
        let rm = a
            .hub
            .draft(Signer::Device("d-1".into()), Action::RemoveMachine("m-1".into()), 3_000)
            .sign(a.d1.signing_key());
        a.hub.apply(&rm, &mut ChainClock::history()).unwrap();
        {
            let mut st = a.shared.state.lock().unwrap();
            assert!(st.absorb(&[rm], now_ms()).error.is_none());
            assert!(st.removed());
        }
        a.shared.save();
        let mut keys = crate::aggregator::keys::HostKeys::load(&a.shared.paths, "a-1", "m-1").unwrap();
        keys.put("m-2", 0, &[2; 32]).unwrap();
        let refused = startable(&a.shared.paths).err().expect("a removed machine does not start");
        assert!(refused.contains("removed"), "{refused}");
        assert!(!crate::aggregator::dir(&a.shared.paths).exists(), "the other devices' keys are gone");
    }

    /// A host's mailbox is shared between its
    /// agent and its device role. The agent acknowledges only what it
    /// handles (purpose 2, its own machine key — taken, or refused for good);
    /// every other purpose is released, so the device role can still take it.
    #[tokio::test]
    async fn the_agent_acknowledges_only_its_own_mail_and_releases_the_rest() {
        let a = account();
        let (text_tx, mut text_rx) = mpsc::unbounded_channel();
        let said = |rx: &mut mpsc::UnboundedReceiver<String>| -> Vec<Value> {
            std::iter::from_fn(|| rx.try_recv().ok())
                .map(|t| serde_json::from_str(&t).unwrap())
                .collect()
        };
        for (id, purpose) in [(1, json!(5)), (2, json!(1)), (3, json!(4)), (4, Value::Null), (5, json!("2"))] {
            let mail = json!({"t": "mail", "id": id, "purpose": purpose, "sender": "d-1", "epoch": 0, "sealed": "00"});
            on_control(&a.shared, &mail, &text_tx);
            assert_eq!(said(&mut text_rx), vec![json!({"t": "mail_release", "id": id})], "purpose {purpose}");
        }
        // Its own purpose: a delivery it refuses for good is acknowledged.
        let junk = json!({"t": "mail", "id": 9, "purpose": 2, "sender": "d-1", "epoch": 0, "sealed": "zz"});
        on_control(&a.shared, &junk, &text_tx);
        assert_eq!(said(&mut text_rx), vec![json!({"t": "mail_ack", "id": 9})]);
        // Even with no chain yet, another purpose is released at once, not held.
        a.shared.state.lock().unwrap().chain = None;
        on_control(&a.shared, &json!({"t": "mail", "id": 10, "purpose": 5}), &text_tx);
        assert_eq!(said(&mut text_rx), vec![json!({"t": "mail_release", "id": 10})]);
        assert!(a.shared.deferred_mail.lock().unwrap().is_empty());
    }

    /// The alert for a new host covers only that host's own
    /// first machine key epoch, and a removal's alert the rotation after it.
    /// A user-key rotation after adding a plain device, a key epoch entry
    /// that also commits a user key or other hosts' keys, and key epochs
    /// after key epochs are each their own alert.
    #[test]
    fn only_a_new_hosts_own_key_or_a_removals_rotation_rides_on_its_alert() {
        use rh_proto::chain::{DeviceV3, Flags, KeyCommit, KeyCommits};
        let ak = MachineIdentity::from_seed(&[1; 32]);
        let dev = MachineIdentity::from_seed(&[2; 32]);
        let genesis = genesis_draft("a-1", ak.sign_public(), info("d-1", &dev), None, 1_000).sign(ak.signing_key());
        let state = ChainState::verify(std::slice::from_ref(&genesis), None, &mut ChainClock::history()).unwrap();
        let by = |signer: &str, action: Action| state.draft(Signer::Device(signer.into()), action, 2_000).sign(dev.signing_key());
        let device = |id: &str, host: bool| {
            Action::AddDevice3(DeviceV3 {
                id: id.into(),
                sign_key: dev.sign_public(),
                dh_key: dev.dh_public(),
                flags: Flags { host, admin: false },
            })
        };
        let commit = KeyCommit { epoch: 0, hash: [0; 32] };
        let keys = |user: bool, hosts: &[&str]| {
            by(
                "d-1",
                Action::KeyEpochs(KeyCommits {
                    user: user.then_some(commit),
                    machines: hosts.iter().map(|h| (h.to_string(), commit)).collect(),
                }),
            )
        };
        let covered = |entry: &Entry, prev: &Entry| account_change(entry, Some(prev)).is_none();
        let add_host = by("d-1", device("h-2", true));
        let add_plain = by("d-1", device("d-3", false));
        let restart = by(
            "d-1",
            Action::SetHost {
                id: "h-2".into(),
                host: true,
            },
        );
        let add_machine = by("d-1", Action::AddMachine(dev.info("m-2")));
        let removal = by("d-1", Action::RemoveDevice3("d-9".into()));
        assert!(covered(&keys(false, &["h-2"]), &add_host), "the new host's own first key");
        assert!(covered(&keys(false, &["h-2"]), &restart), "a host running AI again: its new key");
        assert!(covered(&keys(false, &["m-2"]), &add_machine), "v2: a machine's own key");
        assert!(covered(&keys(true, &["h-2", "m-1"]), &removal), "a removal's rotation");
        assert!(!covered(&keys(true, &[]), &add_plain), "a user-key rotation after adding a plain device");
        assert!(!covered(&keys(false, &["m-1"]), &add_plain), "a plain device has no key of its own");
        assert!(!covered(&keys(true, &["h-2"]), &add_host), "a user key next to the new host's");
        assert!(
            !covered(&keys(false, &["h-2", "m-1"]), &add_host),
            "other hosts' keys next to the new host's"
        );
        assert!(!covered(&keys(false, &["m-1"]), &add_host), "another host's key after a new host");
        assert!(!covered(&keys(true, &[]), &keys(false, &["h-2"])), "key epochs after key epochs");
        let other_signer = state
            .draft(Signer::Device("d-2".into()), device("h-2", true), 2_000)
            .sign(dev.signing_key());
        assert!(
            !covered(&keys(false, &["h-2"]), &other_signer),
            "another signer's addition covers nothing"
        );
        let said = account_change(&keys(true, &[]), Some(&add_plain)).unwrap();
        assert_eq!(said, "new keys were committed by d-1: user key epoch 0");
    }

    /// Entries a direct hello teaches while the hub link is down are saved
    /// and acted on as any chain advance: every connection's route table is
    /// told to sweep, and the change is alerted — not only saved.
    #[tokio::test]
    async fn a_direct_hello_with_the_hub_link_down_still_sweeps_every_connection() {
        let mut a = account();
        assert!(a.shared.hub_text.lock().unwrap().is_none(), "the hub link is down");
        let mut sweeps = a.shared.chain_version.subscribe();
        let rm = a
            .hub
            .draft(Signer::Device("d-1".into()), Action::RemoveDevice("d-2".into()), 3_000)
            .sign(a.d1.signing_key());
        a.hub.apply(&rm, &mut ChainClock::history()).unwrap();
        let bind = DirectBinding {
            binding: Binding {
                account: "a-1".into(),
                device: "d-1".into(),
                machine: "m-1".into(),
            },
            relay_hash: [1; 32],
            nonce: [2; 32],
            device_fp: [3; 32],
            agent_fp: [4; 32],
        };
        let machine_dh = a.shared.identity.dh_public();
        let mut initiator = rh_proto::noise::Initiator::with_prologue(a.d1.dh(), &machine_dh, &bind.prologue().unwrap()).unwrap();
        let body = handshake::HelloBody {
            chain_seq: a.hub.seq(),
            chain_head: a.hub.head(),
            entries: vec![rm],
            client: Vec::new(),
        };
        let hello = initiator.write_hello(now_ms(), &body.encode().unwrap()).unwrap();
        let _ = LinkAdmit { shared: a.shared.clone() }.admit("d-1", &bind, &hello);
        assert!(a.shared.state.lock().unwrap().chain.as_ref().unwrap().device("d-2").is_none());
        assert!(sweeps.has_changed().unwrap(), "every relay connection is told to sweep");
        // What a relay connection does on that word: d-2's channel closes.
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        sweeps.mark_unchanged();
        let mut routes = routes_with_d2();
        drop_removed(&a.shared, &mut routes, &out_tx);
        assert!(routes.is_empty());
        assert_eq!(closes(&mut out_rx), vec![(7, 4403)]);
        assert_eq!(
            a.shared.state.lock().unwrap().changes.iter().map(|a| a.what.clone()).collect::<Vec<_>>(),
            vec!["device d-2 was removed by d-1".to_string()]
        );
    }

    /// A hub on loopback that signs the agent in and then only carries
    /// frames: what the test hands it goes to the agent, and every frame the
    /// agent sends (heartbeats aside) comes back to the test.
    async fn carrying_hub() -> (u16, mpsc::UnboundedSender<Frame>, mpsc::UnboundedReceiver<Frame>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (to_agent, mut for_agent) = mpsc::unbounded_channel::<Frame>();
        let (from_agent, heard) = mpsc::unbounded_channel::<Frame>();
        tokio::spawn(async move {
            let Ok((tcp, _)) = listener.accept().await else { return };
            let Ok(ws) = tokio_tungstenite::accept_async(tcp).await else {
                return;
            };
            let (mut sink, mut source) = ws.split();
            let challenge = json!({"t": "challenge", "nonce": "00".repeat(32), "hub": "ws://127.0.0.1", "version": 1});
            if sink.send(Message::Text(challenge.to_string().into())).await.is_err() {
                return;
            }
            let _login = source.next().await;
            if sink.send(Message::Text(json!({"t": "welcome"}).to_string().into())).await.is_err() {
                return;
            }
            loop {
                tokio::select! {
                    frame = for_agent.recv() => {
                        let Some(frame) = frame else { break };
                        if sink.send(Message::Binary(frame.encode().unwrap().into())).await.is_err() {
                            break;
                        }
                    }
                    message = source.next() => match message {
                        Some(Ok(Message::Binary(bytes))) => match Frame::decode(&bytes) {
                            Ok(Frame::Ping { .. } | Frame::Pong { .. }) | Err(_) => {}
                            Ok(frame) => {
                                let _ = from_agent.send(frame);
                            }
                        },
                        Some(Ok(_)) => {}
                        _ => break,
                    },
                }
            }
        });
        (port, to_agent, heard)
    }

    /// `device`'s (keys `key`) first handshake message over the hub or a
    /// relay, at this agent's chain head, teaching `entries`.
    fn hello(a: &Account, key: &MachineIdentity, device: &str, entries: Vec<Entry>) -> Vec<u8> {
        let binding = Binding {
            account: "a-1".into(),
            device: device.into(),
            machine: "m-1".into(),
        };
        let mut initiator = rh_proto::noise::Initiator::new(key.dh(), &a.shared.identity.dh_public(), &binding).unwrap();
        let body = handshake::HelloBody {
            chain_seq: a.hub.seq(),
            chain_head: a.hub.head(),
            entries,
            client: Vec::new(),
        };
        initiator.write_hello(now_ms(), &body.encode().unwrap()).unwrap()
    }

    /// The next frame the hub hears from the agent on a channel in `chs`,
    /// within five seconds.
    async fn heard_on(heard: &mut mpsc::UnboundedReceiver<Frame>, chs: &[u32]) -> Option<Frame> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let frame = tokio::time::timeout_at(deadline, heard.recv()).await.ok()??;
            let ch = match &frame {
                Frame::Data { ch, .. } | Frame::Close { ch, .. } | Frame::Credit { ch, .. } => *ch,
                _ => continue,
            };
            if chs.contains(&ch) {
                return Some(frame);
            }
        }
    }

    /// The hub connection sweeps its own channels on every chain advance,
    /// wherever the entries were learnt. A removal that a device's hello on a
    /// relay, or a direct hello, teaches this agent — entries the hub
    /// connection never sees itself — still closes the removed device's
    /// channel on the hub path at once (4403), and only that channel.
    #[tokio::test]
    async fn a_removal_learnt_from_a_relay_or_direct_hello_closes_the_hub_paths_channels() {
        for via_relay in [true, false] {
            let via = if via_relay { "a relay hello" } else { "a direct hello" };
            let mut a = account();
            attest(&a, "d-1", &a.d1, 0);
            let (port, hub, mut heard) = carrying_hub().await;
            a.shared.state.lock().unwrap().hub = format!("ws://127.0.0.1:{port}");
            let link = tokio::spawn(run(a.shared.clone()));

            // d-1 on channel 6 and d-2 on channel 7, carried by the hub.
            for (ch, device, key) in [(6, "d-1", &a.d1), (7, "d-2", &d2())] {
                hub.send(Frame::Open { ch, peer: device.into() }).unwrap();
                hub.send(Frame::Data {
                    ch,
                    payload: hello(&a, key, device, Vec::new()),
                })
                .unwrap();
                match heard_on(&mut heard, &[ch]).await {
                    Some(Frame::Data { .. }) => {}
                    other => panic!("{via}: channel {ch} did not open: {other:?}"),
                }
            }

            // d-1 removes d-2; this agent learns it off the hub connection.
            let rm = a
                .hub
                .draft(Signer::Device("d-1".into()), Action::RemoveDevice("d-2".into()), 3_000)
                .sign(a.d1.signing_key());
            a.hub.apply(&rm, &mut ChainClock::history()).unwrap();
            if via_relay {
                // What a relay connection does with a device's first message.
                let (relay_out, _relay_frames) = mpsc::unbounded_channel();
                let (relay_text, _relay_said) = mpsc::unbounded_channel();
                let mut relay_routes = Routes::new();
                let taught = hello(&a, &a.d1, "d-1", vec![rm]);
                accept(&a.shared, 100, "d-1".into(), &taught, &mut relay_routes, &relay_out, &relay_text);
            } else {
                let bind = DirectBinding {
                    binding: Binding {
                        account: "a-1".into(),
                        device: "d-1".into(),
                        machine: "m-1".into(),
                    },
                    relay_hash: [1; 32],
                    nonce: [2; 32],
                    device_fp: [3; 32],
                    agent_fp: [4; 32],
                };
                let machine_dh = a.shared.identity.dh_public();
                let mut initiator = rh_proto::noise::Initiator::with_prologue(a.d1.dh(), &machine_dh, &bind.prologue().unwrap()).unwrap();
                let body = handshake::HelloBody {
                    chain_seq: a.hub.seq(),
                    chain_head: a.hub.head(),
                    entries: vec![rm],
                    client: Vec::new(),
                };
                let taught = initiator.write_hello(now_ms(), &body.encode().unwrap()).unwrap();
                let _ = LinkAdmit { shared: a.shared.clone() }.admit("d-1", &bind, &taught);
            }
            assert!(
                a.shared.state.lock().unwrap().chain.as_ref().unwrap().device("d-2").is_none(),
                "{via}: the removal took effect"
            );

            match heard_on(&mut heard, &[6, 7]).await {
                Some(Frame::Close { ch: 7, code: 4403, .. }) => {}
                other => panic!("{via}: d-2's channel on the hub path was not closed: {other:?}"),
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
            while let Ok(frame) = heard.try_recv() {
                assert!(!matches!(frame, Frame::Close { ch: 6, .. }), "{via}: d-1's channel stays open");
            }
            a.shared.cancel.cancel();
            let _ = link.await;
        }
    }

    #[test]
    fn conflicts_are_told_apart_from_refused_entries() {
        assert!(is_conflict(&ChainError::Fork { seq: 3 }));
        assert!(is_conflict(&ChainError::GenesisMismatch));
        assert!(!is_conflict(&ChainError::TooEarly { seq: 3, ready_ms: 1 }));
        assert!(!is_conflict(&ChainError::BadSignature { seq: 3 }));
    }
}
