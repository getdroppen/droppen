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
//! the direct hello is admitted by [`LinkAdmit`] under exactly the rules
//! above, and the direct path lives and dies with its channel — a removal
//! that closes the channel ([`drop_removed`]) closes the direct path too.
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
//!
//! TODO(M3): the reply should carry this agent's chain suffix for a device
//! the hub keeps behind; a removal should close that device's open channels
//! here at once (M1 has one device and no removal); and machine keys should
//! also arrive inside a channel, not only through the mailbox.

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use rh_proto::bridge::{RequestHead, ResponseHead};
use rh_proto::chain::{Action, Attestation, ChainClock, ChainError, ChainState, Entry, Signer};
use rh_proto::channel::{PathOptions, Session};
use rh_proto::direct::{Admission, Admit, AgentDirect};
use rh_proto::driver::{Channel, Incoming};
use rh_proto::frame::Frame;
use rh_proto::handshake::{self, Accepted, AgentSide, FRESHNESS_MAX_AGE_MS};
use rh_proto::identity::MachineIdentity;
use rh_proto::mux::Role;
use rh_proto::mux::StreamClass;
use rh_proto::noise::Binding;
use rh_proto::noise::{RekeyPolicy, ReplayGuard};
use rh_proto::path::{self, DirectBinding};
use rh_proto::wrap::{self, Purpose, SealContext, SealedKey, WrapError};
use serde_json::{json, Value};
use tokio::sync::{mpsc, Semaphore};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use super::bridge::{self, Loopback};
use super::hub::{self, now_ms};
use super::state::{self, AgentState};
use crate::sessions::Paths;

const BACKOFF_MIN: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// A connection that lasted this long resets the backoff.
const STABLE: Duration = Duration::from_secs(60);
/// The close of a handshake this agent took but could not save the device
/// of (review D3: a device not on its record yet is let in only once it is):
/// the page says the computer cannot save, rather than retrying blind.
pub const AGENT_UNSAVED: u16 = 4507;
/// The close of a channel nobody vouched for in time: a handshake refused
/// as stale, and an open channel whose control lapsed ([`ChannelLapse`]).
pub const CHANNEL_STALE: u16 = 4412;
/// How long an open channel waits at most before it looks at its control's
/// lapse again: its deadline is wall-clock time, and a timer may sleep
/// through a suspended computer.
const LAPSE_POLL: Duration = Duration::from_secs(10 * 60);
/// The heartbeat (architecture doc §5).
const HEARTBEAT: Duration = Duration::from_secs(20);
/// Silence from the hub after which the connection is presumed dead.
const SILENCE: Duration = Duration::from_secs(60);

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
    /// Direct paths on (`RH_AGENT_DIRECT`), and the peers they may hold at once.
    direct: bool,
    peers: Arc<Semaphore>,
    /// When a recovered device's self-activation this agent held back (too
    /// early by its own clock) may be taken: the chain is asked for again then.
    held_until: Mutex<Option<u64>>,
    /// The account changes this run alerted about, newest last (at most
    /// [`MAX_ALERT_LOG`]): what the pushes said, kept for the record.
    alerts: Mutex<Vec<String>>,
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
    /// The account's relays (`None`: `RH_AGENT_RELAYS=0`).
    pub(super) relays: Mutex<Option<super::relays::Handle>>,
    /// Whether the hub connection is up, since when, and why it last failed
    /// (the local API's `GET /api/cloud`).
    pub(super) health: Mutex<Health>,
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

/// What the link needs from the server it runs in: where its own loopback
/// API listens, the per-boot credential the bridge presents there, the
/// drive (for the streams' writer) and the server's cancellation token (the
/// link runs under a child of it, so stopping the link never stops the server).
#[derive(Clone)]
pub struct LinkEnv {
    /// The server's own listener.
    pub addr: SocketAddr,
    /// [`crate::auth::Auth::internal_credential`].
    pub credential: String,
    /// The server's drive.
    pub drive: Arc<crate::drive::Drive>,
    /// The server's token; the link takes a child of it.
    pub cancel: CancellationToken,
}

/// A link that was started: stop it with [`Running::stop`].
pub struct Running {
    pub(super) shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
    /// The streams' writer, which holds `streams.db` open.
    sync_task: tokio::task::JoinHandle<()>,
    /// This computer's device role ([`crate::aggregator::controller`]), when
    /// it may run (`RH_AGENT_CONTROL`), and what it reports.
    control: Option<tokio::task::JoinHandle<()>>,
    control_status: Arc<Mutex<crate::aggregator::controller::Status>>,
    /// Its copies of the other devices ([`crate::aggregator::copies`]), and
    /// what it reports.
    copies: Option<tokio::task::JoinHandle<()>>,
    copies_status: Arc<Mutex<crate::aggregator::copies::Status>>,
}

impl Running {
    /// Stop the link (its hub connection, relays, channels and the streams'
    /// writer) and wait for it to end.
    pub async fn stop(self) {
        self.shared.cancel.cancel();
        let _ = self.task.await;
        // The writer ends with the same token; its handle on `streams.db` is
        // dropped once it has, which unenrolling waits for before the file goes.
        if tokio::time::timeout(Duration::from_secs(30), self.sync_task).await.is_err() {
            tracing::warn!("the streams' writer did not stop within 30 s");
        }
        // Both stop at the token; one stuck in a request is ended here (they
        // write nothing after the token anyway: security review of S1-4, S1).
        for mut task in self.control.into_iter().chain(self.copies) {
            if tokio::time::timeout(CONTROL_STOP, &mut task).await.is_err() {
                tracing::warn!("the device role did not stop in time: ending it");
                task.abort();
                let _ = task.await;
            }
        }
    }

    /// What this computer's device role reports (`GET /api/cloud`): each
    /// host with what is copied of it.
    pub fn control_status(&self) -> crate::aggregator::controller::Status {
        let mut status = self.control_status.lock().expect("aggregator status").clone();
        let copies = self.copies_status.lock().expect("copies status").clone();
        for host in &mut status.hosts {
            host.copy = copies.hosts.get(&host.id).cloned();
        }
        status
    }

    /// Whether the link stopped on its own (this machine was removed, or its
    /// enrollment turned out to be with other keys).
    pub fn finished(&self) -> bool {
        self.task.is_finished()
    }

    /// The hub connection's health.
    pub fn health(&self) -> Health {
        self.shared.health.lock().expect("health").clone()
    }

    /// The account changes this run alerted about, newest last.
    pub fn alerts(&self) -> Vec<String> {
        self.shared.alerts.lock().expect("alerts").clone()
    }

    /// Read the agent state the link keeps.
    pub fn with_state<T>(&self, f: impl FnOnce(&AgentState) -> T) -> T {
        f(&self.shared.state.lock().expect("agent state"))
    }
}

/// How long [`Running::stop`] waits for the device role before ending it.
#[cfg(not(test))]
const CONTROL_STOP: Duration = Duration::from_secs(10);
#[cfg(test)]
const CONTROL_STOP: Duration = Duration::from_millis(200);

/// The most account changes [`Shared::alerts`] keeps.
const MAX_ALERT_LOG: usize = 64;

/// Whether the link may run at all (`DROPPEN_AGENT_LINK=0` turns it off).
pub fn enabled() -> bool {
    crate::env::var("DROPPEN_AGENT_LINK").as_deref() != Some("0")
}

/// Start the link for this instance's saved agent state (an enrollment in
/// progress, or complete). An error, and nothing started, when there is no
/// state, it does not load, the machine key is missing, or the state says
/// this machine was removed.
pub fn start(paths: &Paths, env: &LinkEnv, live: Arc<crate::aggregator::live::LiveCopies>) -> Result<Running, String> {
    let loaded = startable(paths)?;
    let identity = match super::load_or_create(paths)? {
        (identity, false) => identity,
        (_, true) => return Err("this instance has agent state but its identity key was missing (a new one was made); re-enroll it".into()),
    };
    let cancel = env.cancel.child_token();
    let guard = loaded.replay_guard(now_ms());
    tracing::info!(hub = %loaded.hub, account = %loaded.account, machine = %loaded.machine, enrolled = loaded.enrolled(), "starting the hub link");
    let loopback = Arc::new(Loopback::new(env.addr, env.credential.clone()));
    let (display_name, alarms, changes) = (loaded.display_name.clone(), loaded.alarms.clone(), loaded.changes.clone());
    let state = Arc::new(Mutex::new(loaded));
    let (sync, sync_task) = super::streams::spawn(
        paths,
        env.drive.clone(),
        identity.signing_key().clone(),
        loopback.clone(),
        super::streams::Options {
            display_name,
            alarms,
            changes,
            committed: committed_keys(state.clone()),
            controllers: controllers_view(state.clone()),
        },
        cancel.clone(),
    );
    let shared = Arc::new(Shared {
        paths: paths.clone(),
        deferred_mail: Mutex::new(Vec::new()),
        identity: Arc::new(identity),
        loopback,
        sync,
        state,
        guard: Mutex::new(guard),
        cancel,
        direct: super::direct::enabled(),
        peers: Arc::new(Semaphore::new(super::direct::MAX_PEERS)),
        held_until: Mutex::new(None),
        alerts: Mutex::new(Vec::new()),
        chain_version: tokio::sync::watch::channel(0).0,
        lapses: Mutex::new(Vec::new()),
        #[cfg(test)]
        after_admission: Mutex::new(None),
        hub_text: Mutex::new(None),
        relays: Mutex::new(None),
        health: Mutex::new(Health::default()),
    });
    if !shared.direct {
        tracing::info!("direct paths are off (DROPPEN_AGENT_DIRECT=0): devices reach this machine through relays only");
    }
    if super::relays::enabled() {
        *shared.relays.lock().expect("relays") = Some(super::relays::spawn(shared.clone()));
    } else {
        tracing::warn!("relays are off (DROPPEN_AGENT_RELAYS=0): devices cannot reach this machine live, only its offline copy");
    }
    let control_status = Arc::new(Mutex::new(crate::aggregator::controller::Status::default()));
    let copies_status = Arc::new(Mutex::new(crate::aggregator::copies::Status::default()));
    // This computer as a device controlling the others (S1-4) and following
    // their copies (S1-5): under the link's token, so both stop with it —
    // and with this machine's removal.
    let control = crate::aggregator::enabled().then(|| {
        let (keys_out, keys) = tokio::sync::watch::channel(None);
        let nudge = Arc::new(tokio::sync::Notify::new());
        let controller = tokio::spawn(crate::aggregator::controller::run(crate::aggregator::controller::Env {
            paths: paths.clone(),
            identity: shared.identity.clone(),
            state: shared.state.clone(),
            chain: shared.chain_version.subscribe(),
            cancel: shared.cancel.child_token(),
            status: control_status.clone(),
            timing: Default::default(),
            keys_out,
            nudge: nudge.clone(),
        }));
        let copies = tokio::spawn(crate::aggregator::copies::run(crate::aggregator::copies::Env {
            paths: paths.clone(),
            identity: shared.identity.clone(),
            state: shared.state.clone(),
            chain: shared.chain_version.subscribe(),
            keys,
            nudge,
            cancel: shared.cancel.child_token(),
            status: copies_status.clone(),
            live,
        }));
        (controller, copies)
    });
    let task = tokio::spawn(run(shared.clone()));
    let (control, copies) = control.unzip();
    Ok(Running {
        shared,
        task,
        sync_task,
        control,
        control_status,
        copies,
        copies_status,
    })
}

/// The saved agent state a link may start from — none when this instance is
/// not enrolled, or its chain removed it. A removed one also loses what it
/// held to control the other devices (`aggregator/`): its removal may have
/// been learnt by a release that did not delete it (one from before S1-4, or
/// a run that stopped first: security review of S1-4, S2).
fn startable(paths: &Paths) -> Result<AgentState, String> {
    let loaded = AgentState::load(paths)?.ok_or("this instance is not enrolled")?;
    if loaded.removed() {
        if let Err(error) = crate::aggregator::forget_all(paths) {
            tracing::error!(%error, "cannot delete the other devices' keys of a removed machine");
        }
        return Err("this device was removed from the account; it does not connect again".into());
    }
    Ok(loaded)
}

/// The writer's check of a machine key from `machine-keys.json`: exactly the
/// key this agent's chain committed for its epoch (M2 re-review #27: the file
/// is not the chain, and outlived re-enrollments).
fn committed_keys(state: Arc<Mutex<AgentState>>) -> super::streams::KeyCheck {
    Arc::new(move |epoch, key| {
        let st = state.lock().expect("agent state");
        st.chain.as_ref().is_some_and(|c| c.check_machine_key(&st.machine, epoch, key).is_ok())
    })
}

/// The status's `controllers`, read from the agent state when a body is built.
fn controllers_view(state: Arc<Mutex<AgentState>>) -> super::streams::ControllersView {
    Arc::new(move || state.lock().expect("agent state").controllers())
}

async fn run(shared: Arc<Shared>) {
    let mut backoff = BACKOFF_MIN;
    loop {
        let started = Instant::now();
        let result = tokio::select! {
            r = connection(&shared) => r,
            () = shared.cancel.cancelled() => {
                shared.set_health(false, Some("stopped".into()));
                return;
            }
        };
        shared.set_health(
            false,
            Some(result.clone().err().unwrap_or_else(|| "the hub closed the connection".into())),
        );
        if shared.state.lock().expect("agent state").removed() {
            tracing::warn!("this machine was removed from the account: the hub link stops");
            shared.cancel.cancel();
            return;
        }
        match result {
            Ok(()) => tracing::info!("hub connection closed; reconnecting"),
            Err(error) => tracing::warn!(%error, "hub connection failed; reconnecting"),
        }
        if started.elapsed() > STABLE {
            backoff = BACKOFF_MIN;
        }
        let wait = rand::thread_rng().gen_range(Duration::ZERO..=backoff);
        backoff = (backoff * 2).min(BACKOFF_MAX);
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = shared.cancel.cancelled() => return,
        }
    }
}

impl Shared {
    pub(super) fn save(&self) {
        let st = self.state.lock().expect("agent state");
        if let Err(error) = st.save(&self.paths) {
            tracing::error!(%error, "cannot save the agent state");
        }
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

/// This machine was removed from the account — its own verified chain says
/// so (a hub refusing its sign-in never gets here): mark it (saved, so the
/// link does not start again), say so, and stop the link (the hub
/// connection, its relays and channels, the streams' writer). The server and
/// its local API go on.
fn on_removed(shared: &Arc<Shared>, why: &str) {
    let news = shared.state.lock().expect("agent state").mark_removed(now_ms());
    // What it held to control the others (their keys) goes with it (§4.7).
    shared.cancel.cancel();
    if let Err(error) = crate::aggregator::forget_all(&shared.paths) {
        tracing::error!(%error, "cannot delete the other devices' keys after this machine's removal");
    }
    if news {
        shared.save();
        tracing::warn!("this machine was removed from the account ({why}): stopping the hub link");
        crate::push::notify_account_change(
            Arc::new(shared.paths.clone()),
            "Droppen: this device was removed".into(),
            format!("this device was removed from the account ({why}); it no longer connects to Droppen"),
        );
    }
    shared.cancel.cancel();
}

/// Open channels: `ch` → (the device it was authorised for, its inbound frames).
pub(super) type Routes = HashMap<u32, (String, mpsc::UnboundedSender<Frame>)>;

/// Channels opened and waiting for the device's first message: `ch` → (the
/// device, when it opened). Per connection, and bounded: a relay is not
/// trusted not to open channels without end ([`on_channel_frame`]).
pub(super) type Pending = HashMap<u32, (String, Instant)>;
/// At most this many channels wait on one connection; more are refused (4409)…
const MAX_PENDING: usize = 256;
/// …and none for longer than this (then it is refused, 4408).
const PENDING_FOR: Duration = Duration::from_secs(30);

async fn connection(shared: &Arc<Shared>) -> Result<(), String> {
    let (origin, account, machine, from) = {
        let st = shared.state.lock().expect("agent state");
        (
            st.hub.clone(),
            st.account.clone(),
            st.machine.clone(),
            st.chain.as_ref().map_or(0, |c| c.seq() + 1),
        )
    };
    let (mut sink, mut source, nonce) = hub::dial(&origin).await?;
    match hub::login(&mut sink, &mut source, &shared.identity, &account, &machine, &origin, nonce).await {
        Ok(_) => {}
        Err(hub::LoginError::Removed(message)) => {
            // The hub's word, not the chain's: a failure, retried with
            // backoff and said in the settings — never a removal. Only this
            // machine's verified chain removes it ([`chain_advanced`]).
            let why = format!("the hub refused this device: {message}");
            tracing::warn!("{why}; trying again later");
            shared.health.lock().expect("health").refused = Some(why.clone());
            return Err(why);
        }
        Err(hub::LoginError::Other(error)) => return Err(error),
    }
    let mut features: Vec<&str> = vec![super::CHAIN_V3_FEATURE, super::POLICY_FEATURE];
    if shared.relays.lock().expect("relays").is_some() {
        features.push(super::relays::FEATURE);
    }
    hub::send_json(
        &mut sink,
        &json!({"t": "agent_info", "version": crate::api::SERVER_VERSION, "features": features}),
    )
    .await?;
    hub::send_json(&mut sink, &json!({"t": "chain_sub", "from": from})).await?;
    tracing::info!(%origin, %machine, "signed in to the hub");
    shared.set_health(true, None);

    // One writer for the socket: frames from every channel, and control text.
    let (frames_tx, mut frames_rx) = mpsc::unbounded_channel::<Frame>();
    let (text_tx, mut text_rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        let mut beat = tokio::time::interval(HEARTBEAT);
        let mut nonce = 0u64;
        loop {
            let message = tokio::select! {
                f = frames_rx.recv() => match f {
                    Some(f) => match f.encode() { Ok(b) => Message::Binary(b.into()), Err(_) => continue },
                    None => break,
                },
                t = text_rx.recv() => match t { Some(t) => Message::Text(super::traffic::agent_out(t).into()), None => break },
                _ = beat.tick() => {
                    nonce += 1;
                    Message::Binary(Frame::Ping { nonce }.encode().expect("a ping encodes").into())
                }
            };
            if sink.send(message).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // The streams' writer has this connection only while this machine runs
    // AI in its account (and is told again whenever the chain changes that).
    let mut writing = false;
    sync_streams(shared, &mut writing, &frames_tx, &text_tx);
    *shared.hub_text.lock().expect("hub text") = Some(text_tx.clone());
    if let Some(relays) = shared.relays.lock().expect("relays").as_ref() {
        relays.send(super::relays::Event::HubUp(text_tx.clone()));
    }
    // Every chain advance, wherever it came from (this connection, a relay's
    // hello, a direct hello), sweeps this connection's channels too.
    let mut chain = shared.chain_version.subscribe();
    let mut routes: Routes = HashMap::new();
    let mut pending = Pending::new();
    let mut last_heard = Instant::now();
    let mut check = tokio::time::interval(HEARTBEAT);
    let result = loop {
        let message = tokio::select! {
            m = source.next() => m,
            r = chain.changed() => {
                if r.is_ok() {
                    drop_removed(shared, &mut routes, &frames_tx);
                    sync_streams(shared, &mut writing, &frames_tx, &text_tx);
                }
                continue;
            }
            _ = check.tick() => {
                if last_heard.elapsed() > SILENCE {
                    break Err("the hub went silent".to_string());
                }
                if writer.is_finished() {
                    break Err("the connection's writer stopped".to_string());
                }
                ask_again_for_held(shared, &text_tx);
                expire_pending(&mut pending, &frames_tx, Instant::now());
                drop_ended(&mut routes);
                continue;
            }
        };
        let message = match message {
            Some(Ok(m)) => m,
            Some(Err(e)) => break Err(e.to_string()),
            None => break Ok(()),
        };
        last_heard = Instant::now();
        match message {
            Message::Text(text) => {
                if let Ok(value) = serde_json::from_str::<Value>(super::traffic::agent_in(text.as_str())) {
                    on_control(shared, &value, &text_tx);
                }
            }
            Message::Binary(bytes) => {
                let Ok(frame) = Frame::decode(&bytes) else {
                    tracing::warn!(len = bytes.len(), "an undecodable frame from the hub");
                    continue;
                };
                on_frame(shared, frame, &mut routes, &mut pending, &frames_tx, &text_tx);
            }
            Message::Close(frame) => {
                let code = frame.as_ref().map(|f| u16::from(f.code));
                tracing::info!(?code, "the hub closed the connection");
                break Ok(());
            }
            _ => {}
        }
    };
    shared.sync.send(super::streams::Event::Disconnected);
    *shared.hub_text.lock().expect("hub text") = None;
    if let Some(relays) = shared.relays.lock().expect("relays").as_ref() {
        relays.send(super::relays::Event::HubDown);
    }
    // Channels end with their inbound frames.
    routes.clear();
    drop(frames_tx);
    drop(text_tx);
    writer.abort();
    result
}

/// Give the streams' writer this connection while this machine runs AI in
/// its account, and take it back once it does not (chain v3 `SetHost false`,
/// review finding 12: a member that stopped running AI writes no streams);
/// it gets it again when the chain makes it a host again. `writing` is
/// whether the writer has it now.
fn sync_streams(shared: &Shared, writing: &mut bool, frames: &mpsc::UnboundedSender<Frame>, text: &mpsc::UnboundedSender<String>) {
    let hosting = shared.state.lock().expect("agent state").hosting();
    if hosting == *writing {
        return;
    }
    *writing = hosting;
    if hosting {
        shared.sync.send(super::streams::Event::Connected(frames.clone(), text.clone()));
    } else {
        tracing::warn!("this machine does not run AI in its account now: its encrypted streams stop until it does again");
        shared.sync.send(super::streams::Event::Disconnected);
    }
}

fn on_control(shared: &Arc<Shared>, value: &Value, text: &mpsc::UnboundedSender<String>) {
    match value["t"].as_str() {
        Some("mail") => match open_mail(shared, value) {
            MailVerdict::Ack => {
                let _ = text.send(json!({"t": "mail_ack", "id": value["id"]}).to_string());
            }
            MailVerdict::Release => {
                let _ = text.send(json!({"t": "mail_release", "id": value["id"]}).to_string());
            }
            MailVerdict::Later => {
                let evicted = defer(&mut shared.deferred_mail.lock().expect("deferred mail"), value.clone());
                if let Some(old) = evicted {
                    let mk_epoch = shared.state.lock().expect("agent state").mk_epoch;
                    let _ = text.send(evicted_message(&old, mk_epoch).to_string());
                }
            }
        },
        Some("error") => tracing::warn!(code = %value["code"], message = %value["message"], "the hub reported an error"),
        // `plan`: the account's retention and storage quota (relay-and-plans
        // §3.2), which bound the encrypted copy; a hub from before plans never
        // sends it and the copy stays as it was.
        Some("stream_refused" | "stream_watched" | "stream_list" | "plan") => shared.sync.send(super::streams::Event::Control(value.clone())),
        // The account's relays, and tickets for them (relay-and-plans §2.7).
        Some("relay_set" | "relay_tickets") => {
            if let Some(relays) = shared.relays.lock().expect("relays").as_ref() {
                relays.send(super::relays::Event::Hub(value.clone()));
            }
        }
        _ => {}
    }
}

/// The most mail kept to try again as the chain catches up.
const MAX_DEFERRED: usize = 64;

/// Keep `value` (a mail not judgeable yet) to try again — once per mail id —
/// and return the oldest one it pushes out, if the list is full.
fn defer(deferred: &mut Vec<Value>, value: Value) -> Option<Value> {
    if deferred.iter().any(|v| v["id"] == value["id"]) {
        return None;
    }
    deferred.push(value);
    (deferred.len() > MAX_DEFERRED).then(|| deferred.remove(0))
}

/// What becomes of a mail pushed out of a full deferred list (M2 re-review
/// #28: it used to vanish, neither acknowledged nor ever tried again, and the
/// hub — counting it as unanswered — sent no further page of mail): one for
/// an epoch this agent is already past is settled, so acknowledged; any other
/// is released — left in the mailbox for the next sign-in, and counted as
/// answered so the next page comes.
fn evicted_message(mail: &Value, mk_epoch: Option<u32>) -> Value {
    let epoch = mail["epoch"].as_u64();
    let moot = matches!((epoch, mk_epoch), (Some(e), Some(held)) if e <= u64::from(held));
    if moot {
        tracing::info!(id = %mail["id"], "a deferred machine key for an epoch this machine is past: acknowledged");
        json!({"t": "mail_ack", "id": mail["id"]})
    } else {
        tracing::warn!(id = %mail["id"], "too much mail waiting for the chain: leaving the oldest in the mailbox for the next sign-in");
        json!({"t": "mail_release", "id": mail["id"]})
    }
}

/// What to do with a mailed machine key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MailVerdict {
    /// Taken, or refused for good: acknowledge it.
    Ack,
    /// Not the agent's to take (another purpose than 2): leave it in the
    /// mailbox — `mail_release`, never `mail_ack`. A host's mailbox is shared
    /// between its agent and its device role (chain v3), and what the agent
    /// acknowledges is gone for the device role too.
    Release,
    /// Not judgeable yet (no chain, or the chain has not caught up with the
    /// key's commitment or its sender): keep it and try again.
    Later,
}

/// Which mail the agent itself takes: only purpose 2, this host's own
/// machine key (chain v3 review, finding 6). Everything else in the mailbox
/// (a device's keys, purpose 5 host keys, purpose 4 requests…) belongs to
/// the device role of the same id, and is released.
fn agents_mail(value: &Value) -> bool {
    value["purpose"].as_u64() == Some(Purpose::MachineKey as u64)
}

/// Open a sealed machine key from the mailbox: from a current full device,
/// and exactly the key the chain committed for its epoch (M2 review — a
/// device removed since it sealed it, or the hub, can hand over nothing the
/// chain does not already name, and no epoch out of reach). Mail of any
/// other purpose is not the agent's: released, never acknowledged.
fn open_mail(shared: &Arc<Shared>, value: &Value) -> MailVerdict {
    if !agents_mail(value) {
        return MailVerdict::Release;
    }
    let mut st = shared.state.lock().expect("agent state");
    let Some(chain) = st.chain.as_ref() else { return MailVerdict::Later };
    let (Some(sender), Some(epoch), Some(sealed)) = (
        value["sender"].as_str(),
        value["epoch"].as_u64().and_then(|e| u32::try_from(e).ok()),
        value["sealed"].as_str(),
    ) else {
        return MailVerdict::Ack;
    };
    let Some(sealed) = unhex(sealed).and_then(|b| SealedKey::decode(&b).ok()) else {
        return MailVerdict::Ack;
    };
    let ctx = SealContext {
        purpose: Purpose::MachineKey,
        account: st.account.clone(),
        sender: sender.into(),
        recipient: st.machine.clone(),
        epoch,
    };
    match wrap::open_machine_key(shared.identity.dh(), &st.machine, chain, &ctx, &sealed) {
        Ok(key) => {
            if let Err(error) = state::store_machine_key(&shared.paths, epoch, &key) {
                tracing::error!(%error, "cannot store a machine key");
                return MailVerdict::Later;
            }
            let newer = st.mk_epoch.is_none_or(|e| epoch > e);
            if newer {
                st.mk_epoch = Some(epoch);
            }
            drop(st);
            shared.save();
            tracing::info!(epoch, %sender, "took a machine key from the mailbox");
            if newer {
                shared.sync.send(super::streams::Event::KeysChanged { reencrypt: false });
            }
            MailVerdict::Ack
        }
        // A commitment or a sender this view has not seen yet: the chain may
        // be behind the mail (the hub pushes mail before the chain catches up).
        Err(WrapError::Uncommitted(ChainError::UncommittedKey { .. })) => MailVerdict::Later,
        Err(WrapError::UnknownSender(_)) if chain.device_record(sender).is_none() => MailVerdict::Later,
        Err(error) => {
            tracing::warn!(%error, %sender, epoch, "refused a machine-key delivery");
            MailVerdict::Ack
        }
    }
}

/// Try the deferred mail again (the chain advanced); acknowledge what is settled.
fn retry_mail(shared: &Arc<Shared>, text: &mpsc::UnboundedSender<String>) {
    let deferred = std::mem::take(&mut *shared.deferred_mail.lock().expect("deferred mail"));
    for value in deferred {
        match open_mail(shared, &value) {
            MailVerdict::Ack => {
                let _ = text.send(json!({"t": "mail_ack", "id": value["id"]}).to_string());
            }
            MailVerdict::Release => {
                let _ = text.send(json!({"t": "mail_release", "id": value["id"]}).to_string());
            }
            MailVerdict::Later => shared.deferred_mail.lock().expect("deferred mail").push(value),
        }
    }
}

/// What an entry changes about who is in the account, in words, if anything:
/// every addition, removal, recovery join and activation, approval and change
/// of recovery words (M2 review: alerts replace the day of distrust after a
/// removal; M2 re-review: a self-activation with the recovery words is the
/// moment a recovered device starts to count, and was said nowhere) — and
/// new key epochs committed on their own, not as part of a removal's
/// rotation or a machine's enrollment (`prev`: the entry before it). The
/// review of the M2 re-review fixes, N2: a full device could commit key
/// epochs at will without anybody hearing of it.
///
/// Chain v3 (`docs/local-aggregator.md` §3.2 "告警"): every device added (its
/// flags in words), every grant of admin, every new host or host restarting,
/// every passkey slot, passkey join and activation, every change to whom a
/// host accepts control from — alerted from this agent's own chain, so a hub
/// cannot hold any of it back. A host renewing its identity key
/// (`docs/host-identity-rotation.md` §3.4 "告警") is a note with the new
/// fingerprint, as the device list shows it.
fn account_change(entry: &Entry, prev: Option<&Entry>) -> Option<String> {
    account_change_in(entry, prev, None)
}

/// [`account_change`], with what an ending renewal pause marker names
/// (`paused`, from this agent's chain: [`pause_notice_edges`]): its start's
/// removals, and for a completion only the control that comes back.
fn account_change_in(entry: &Entry, prev: Option<&Entry>, paused: Option<&[(String, Vec<String>)]>) -> Option<String> {
    let signer = match &entry.draft.signer {
        Signer::Device(id) => id.clone(),
        Signer::Recovery => "the recovery words".into(),
        Signer::Account => "the account key".into(),
        Signer::Passkey(slot) => format!("passkey slot {slot}"),
    };
    let flags = |f: rh_proto::chain::Flags| match (f.host, f.admin) {
        (true, true) => "runs AI, can manage the account",
        (true, false) => "runs AI",
        (false, true) => "can manage the account",
        (false, false) => "can control devices that accept it",
    };
    let v3 = entry.is_v3();
    Some(match &entry.draft.action {
        Action::AddDevice3(d) if matches!(entry.draft.signer, Signer::Passkey(_)) => {
            format!("device {} is joining with {signer}: approve it on an admin device, or remove it", d.id)
        }
        Action::AddDevice3(d) => format!("device {} ({}) was added by {signer}", d.id, flags(d.flags)),
        Action::RecoveryJoin3(d) => format!("device {} ({}) is coming back with the recovery words", d.id, flags(d.flags)),
        Action::RemoveDevice3(id) => format!("device {id} was removed by {signer}"),
        Action::SetAdmin { id, admin: true } => format!("device {id} can now manage the account (granted by {signer})"),
        Action::SetAdmin { id, admin: false } => format!("device {id} can no longer manage the account ({signer})"),
        Action::SetHost { id, host: true } => format!("device {id} runs AI again (set by {signer})"),
        Action::SetHost { id, host: false } => format!("device {id} no longer runs AI ({signer})"),
        Action::PasskeySlotSet(s) => format!("a passkey can now unlock new devices (slot {}, set by {signer})", s.slot),
        Action::PasskeySlotRemove(slot) => format!("passkey slot {slot} was removed by {signer}"),
        Action::PasskeyActivate(id) => format!("device {id} activated itself with its passkey"),
        Action::SetAccept { host, from } => {
            let whom = match from {
                rh_proto::chain::Accept::Admins => "admin devices only".to_string(),
                rh_proto::chain::Accept::All => "every device of the account".to_string(),
                rh_proto::chain::Accept::Devices(ids) => format!("admin devices and {}", ids.join(", ")),
            };
            format!("{host} now accepts control from {whom} (set by {signer})")
        }
        Action::RotateHostKey(r) => format!(
            "device {} renewed its identity key{} (new fingerprint {}, written by {signer})",
            r.host,
            if r.assurance == rh_proto::chain::Assurance::OnDevice {
                ", confirmed on the device"
            } else {
                ""
            },
            rh_proto::identity::device_fingerprint_text(&r.sign_key, &r.dh_key)
        ),
        Action::SetRenewalPolicy { always_assured: true } => {
            format!("identity key renewals must now be confirmed on the device (set by {signer})")
        }
        Action::SetRenewalPolicy { always_assured: false } => format!("identity key renewals may be automatic again (set by {signer})"),
        Action::RenewalPause { phase, removals, .. } => {
            // §3.4 rule 10: "control by {ids} over {host}", per host.
            let edges = |removals: &[(String, Vec<String>)]| {
                if removals.is_empty() {
                    "the paused devices".to_string()
                } else {
                    removals
                        .iter()
                        .map(|(host, ids)| format!("{} over {host}", ids.join(", ")))
                        .collect::<Vec<_>>()
                        .join("; ")
                }
            };
            match phase {
                rh_proto::chain::PausePhase::Start => {
                    format!("control by {} is paused while identity keys are renewed (by {signer})", edges(removals))
                }
                rh_proto::chain::PausePhase::Completed if paused.is_some_and(|p| p.is_empty()) => {
                    format!("a renewal pause was completed; none of the control it paused comes back (by {signer})")
                }
                rh_proto::chain::PausePhase::Completed => format!("control by {} resumes (by {signer})", edges(paused.unwrap_or(&[]))),
                rh_proto::chain::PausePhase::Cancelled => {
                    format!("the pause of control by {} was ended (by {signer})", edges(paused.unwrap_or(&[])))
                }
            }
        }
        Action::ApproveDevice(id) if v3 => format!("device {id} was approved by {signer}"),
        Action::AddDevice(d) => format!("device {} was added by {signer}", d.id),
        Action::RemoveDevice(id) => format!("device {id} was removed by {signer}"),
        Action::AddMachine(m) => format!("device {} (runs AI) was added by {signer}", m.id),
        Action::RemoveMachine(id) => format!("device {id} (runs AI) was removed by {signer}"),
        Action::RecoveryJoin(d) => format!("device {} is coming back with the recovery words", d.id),
        Action::ActivateRecovered(id) => format!("device {id} activated itself with the recovery words"),
        Action::ApproveDevice(id) => format!("device {id}, back with the recovery words, was approved by {signer}"),
        Action::SetRecoveryKey(Some(_)) => format!("the recovery words were replaced by {signer}"),
        Action::SetRecoveryKey(None) => format!("the recovery words were turned off by {signer}"),
        Action::KeyEpochs(k) if !part_of_a_batch(entry, prev) => {
            let user = k.user.map(|c| format!("user key epoch {}", c.epoch));
            let machines = (!k.machines.is_empty()).then(|| format!("{} content key epoch(s)", k.machines.len()));
            let what: Vec<String> = user.into_iter().chain(machines).collect();
            format!("new keys were committed by {signer}: {}", what.join(" and "))
        }
        _ => return None,
    })
}

/// Whether a key-epoch entry follows, from the same signer, the entry its
/// batch is for, so that entry's alert covers it — and only for what that
/// entry needs:
///
/// - after a removal, the rotation it calls for (user key and hosts' keys);
/// - after a host joins (`AddMachine`, `AddDevice3{host}`), starts running
///   AI again (`SetHost true`) or renews its identity key (`RotateHostKey`),
///   exactly that host's own next machine key epoch: one commitment, for that
///   host, and no user key.
///
/// Anything else is its own alert (chain v3 review): a user-key rotation
/// after adding a plain device, a key epoch entry that also commits other
/// hosts' keys or a user key next to a new host's, or key epochs after key
/// epochs (which used to chain the cover along).
fn part_of_a_batch(entry: &Entry, prev: Option<&Entry>) -> bool {
    let Action::KeyEpochs(keys) = &entry.draft.action else { return false };
    let Some(prev) = prev.filter(|p| p.draft.signer == entry.draft.signer) else {
        return false;
    };
    let only_the_hosts_own = |host: &str| keys.user.is_none() && matches!(keys.machines.as_slice(), [(id, _)] if id == host);
    match &prev.draft.action {
        Action::RemoveDevice(_) | Action::RemoveMachine(_) | Action::RemoveDevice3(_) => true,
        Action::AddMachine(m) => only_the_hosts_own(&m.id),
        Action::AddDevice3(d) if d.flags.host => only_the_hosts_own(&d.id),
        Action::SetHost { id, host: true } => only_the_hosts_own(id),
        Action::RotateHostKey(r) => only_the_hosts_own(&r.host),
        _ => false,
    }
}

/// What the notice of renewal pause marker `entry` names, from `chain` (the
/// marker applied): an ending marker names no removals, so its start's — and
/// "resumes" only the control that comes back, never an edge the base has
/// since dropped (a hand `SetAccept`, a removal, a host that stopped hosting)
/// or another open pause still holds. `None` for anything else.
fn pause_notice_edges(chain: &ChainState, entry: &Entry) -> Option<Vec<(String, Vec<String>)>> {
    let Action::RenewalPause { op, phase, .. } = &entry.draft.action else {
        return None;
    };
    let removals = chain.pause(op)?.removals.clone();
    if *phase != rh_proto::chain::PausePhase::Completed {
        return Some(removals);
    }
    Some(
        removals
            .into_iter()
            .filter_map(|(host, ids)| {
                let ids: Vec<String> = ids.into_iter().filter(|id| chain.may_control(id, &host).is_ok()).collect();
                (!ids.is_empty()).then_some((host, ids))
            })
            .collect(),
    )
}

/// Raise every change to who is in the account that `entries` make.
fn alert_account_changes(shared: &Arc<Shared>, entries: &[Entry]) {
    let prevs: Vec<Option<Entry>> = {
        let st = shared.state.lock().expect("agent state");
        entries
            .iter()
            .map(|e| {
                let seq = usize::try_from(e.seq()).ok()?.checked_sub(1)?;
                st.chain.as_ref()?.entries().get(seq).cloned()
            })
            .collect()
    };
    // An ending renewal pause marker names no removals: its start did.
    let paused: Vec<Option<Vec<(String, Vec<String>)>>> = {
        let st = shared.state.lock().expect("agent state");
        entries.iter().map(|e| pause_notice_edges(st.chain.as_ref()?, e)).collect()
    };
    let me = shared.state.lock().expect("agent state").machine.clone();
    let mut noted = false;
    for ((entry, prev), paused) in entries.iter().zip(&prevs).zip(&paused) {
        // This machine's own addition is its enrollment completing, not news to it.
        if matches!(&entry.draft.action, Action::AddMachine(m) if m.id == me) || matches!(&entry.draft.action, Action::AddDevice3(d) if d.id == me) {
            continue;
        }
        let Some(what) = account_change_in(entry, prev.as_ref(), paused.as_deref()) else { continue };
        tracing::warn!(seq = entry.seq(), "account change: {what}");
        {
            let mut log = shared.alerts.lock().expect("alerts");
            log.push(what.clone());
            let excess = log.len().saturating_sub(MAX_ALERT_LOG);
            log.drain(..excess);
        }
        // Kept, and published in the encrypted status stream: every device
        // reads what this machine saw change in the account.
        shared.state.lock().expect("agent state").note_change(now_ms(), &what);
        noted = true;
        crate::push::notify_account_change(Arc::new(shared.paths.clone()), "Droppen: account change".into(), what);
    }
    if noted {
        shared.save();
        let changes = shared.state.lock().expect("agent state").changes.clone();
        shared.sync.send(super::streams::Event::Changes(changes));
    }
}

/// An alarm every device must hear about: logged, kept in the state (and so
/// in the encrypted status stream and `/rh/info`, which every device of the
/// account reads), and pushed (Web Push and Bark). The same alarm is raised
/// again at most hourly. M2 re-review: a fork used to be logged only.
fn raise_alarm(shared: &Arc<Shared>, what: String) {
    tracing::error!("ALARM: {what}");
    let alarms = {
        let mut st = shared.state.lock().expect("agent state");
        if !st.note_alarm(now_ms(), &what) {
            return;
        }
        st.alarms.clone()
    };
    shared.save();
    shared.sync.send(super::streams::Event::Alarms(alarms));
    crate::push::notify_account_change(Arc::new(shared.paths.clone()), "Droppen: device chain alarm".into(), what);
}

/// The hub sends each entry once a connection: an activation held back as too
/// early is asked for again (with what follows it) once this agent's own day
/// since the join has passed.
fn ask_again_for_held(shared: &Arc<Shared>, text: &mpsc::UnboundedSender<String>) {
    let due = {
        let mut held = shared.held_until.lock().expect("held");
        match *held {
            Some(at) if at <= now_ms() => held.take().is_some(),
            _ => false,
        }
    };
    if due {
        let from = shared.state.lock().expect("agent state").seq().map_or(0, |s| s + 1);
        tracing::info!(from, "asking the hub again for the chain a held-back activation stopped");
        let _ = text.send(json!({"t": "chain_sub", "from": from}).to_string());
    }
}

/// Whether a chain error is a conflict between two views — a fork, a
/// rollback, another genesis or account key — rather than an entry refused.
fn is_conflict(error: &ChainError) -> bool {
    matches!(
        error,
        ChainError::Fork { .. } | ChainError::Rollback { .. } | ChainError::GenesisMismatch | ChainError::PinMismatch
    )
}

/// Close every open channel whose device the chain no longer lists as current
/// (removed, or never active): a removal takes effect here at once, whatever
/// the hub does, and [`accept`] refuses the device from now on.
///
/// Chain v3: also every channel whose device this machine no longer accepts
/// control from — an admin demoted while the policy is admins only, a device
/// an admin took off this host's accept list — and every channel at all once
/// this machine stops running AI (`SetHost false`): "who may control me" holds
/// for open channels, not only new ones ([`rh_proto::chain::ChainState::may_control`]).
pub(super) fn drop_removed(shared: &Arc<Shared>, routes: &mut Routes, out: &mpsc::UnboundedSender<Frame>) {
    let gone: Vec<(u32, String, bool)> = {
        let st = shared.state.lock().expect("agent state");
        let Some(chain) = st.chain.as_ref() else { return };
        routes
            .iter()
            .filter(|(_, (device, _))| chain.may_control(device, &st.machine).is_err())
            .map(|(ch, (device, _))| (*ch, device.clone(), chain.device(device).is_none()))
            .collect()
    };
    for (ch, device, removed) in gone {
        if removed {
            tracing::info!(%device, ch, "closing the channel of a device the chain no longer lists");
            routes.remove(&ch);
            refuse(out, ch, 4403, "this device was removed from the account");
        } else {
            tracing::info!(%device, ch, "closing the channel of a device this machine no longer accepts control from");
            routes.remove(&ch);
            refuse(out, ch, 4403, "the device you reach no longer accepts control from this one");
        }
    }
}

/// What one attestation says for a device's control on a channel already
/// open ([`ChannelLapse`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Word {
    /// It counts, from this time.
    Counts(u64),
    /// It is about a head beyond this view (a catch-up under way, or a hub
    /// holding entries back): it counts for nothing, and takes back nothing
    /// its admin said before.
    Ahead,
    /// It conflicts with this view.
    Fork,
    /// It does not count (see [`word_of`]).
    No,
}

/// What `att` says for `device`'s control of this machine on a channel
/// already open (local-aggregator §3.4). It counts — as
/// [`rh_proto::chain::ChainState::freshness`] counts words for admission,
/// but for a head before this view's too — when all of these hold:
/// - it is by a current admin other than `device`, or by `device` itself
///   while it is the sole admin — or was, as of the head it vouched for (a
///   sole admin pairing a second one keeps its channels until the new one
///   vouches);
/// - it is dated no further ahead than the clock skew allows;
/// - it is of this view's head, or of an earlier head and dated no later
///   (within the skew) than this agent applied the entry after that head. A
///   word about a head this agent had moved past by then only says its
///   admin is behind: a hub that keeps one admin frozen at an old head
///   cannot keep refreshing its word. The history this agent took at
///   enrollment was first seen then (enrollment verifies on a live clock),
///   so a word about a head from before it counts only if dated by then;
///   where no sighting of the entry is kept at all, only the head counts.
fn word_of(st: &AgentState, chain: &ChainState, att: &Attestation, device: &str, now_ms: u64) -> Word {
    match chain.check_attestation(att) {
        Ok(()) => {}
        Err(ChainError::Behind { .. }) => return Word::Ahead,
        Err(ChainError::Fork { .. }) => return Word::Fork,
        Err(_) => return Word::No,
    }
    let skew = rh_proto::chain::ATTESTATION_SKEW_MS;
    if att.time_ms > now_ms.saturating_add(skew) {
        return Word::No;
    }
    if att.seq < chain.seq() {
        let moved_on = st.clock.first_seen().get(&(att.seq + 1));
        if !moved_on.is_some_and(|&at| att.time_ms <= at.saturating_add(skew)) {
            return Word::No;
        }
    }
    if att.device == device && chain.admins().count() != 1 && cached_sole_admin_at(st, chain, att.seq).as_deref() != Some(device) {
        return Word::No;
    }
    Word::Counts(att.time_ms)
}

/// [`sole_admin_at`], worked out once per seq ([`AgentState::sole_admins`]).
fn cached_sole_admin_at(st: &AgentState, chain: &ChainState, seq: u64) -> Option<String> {
    let hash = chain.hash_at(seq)?;
    let mut known = st.sole_admins.lock().expect("sole admins");
    if let Some((at, sole)) = known.get(&seq) {
        if *at == hash {
            return sole.clone();
        }
    }
    let sole = sole_admin_at(chain, seq);
    known.insert(seq, (hash, sole.clone()));
    sole
}

/// The account's only admin as of entry `seq` of `chain`, if it had exactly
/// one then (this view's entries up to there, verified again).
fn sole_admin_at(chain: &ChainState, seq: u64) -> Option<String> {
    let upto = usize::try_from(seq).ok()?.checked_add(1)?;
    let view = ChainState::verify(chain.entries().get(..upto)?, None, &mut ChainClock::history()).ok()?;
    let mut admins = view.admins();
    let only = admins.next()?;
    admins.next().is_none().then(|| only.info.id.clone())
}

/// When an open channel's control lapses (local-aggregator §3.4):
/// [`rh_proto::chain::CONTROL_FRESHNESS_MS`] after the newest word that
/// counts for its device ([`word_of`]), kept per admin. A channel is
/// admitted only on a word about exactly this view's head
/// ([`handshake::accept`]); once open, the head moving on before an admin
/// vouches for the new one does not end it, but a view no admin vouched for
/// in seven days does — however long the hub keeps the connection up and a
/// removal to itself. An admin's newer attestation that does not count (of
/// a head beyond this view, or one it was left behind at) does not take
/// back its earlier word; its being an admin no longer does, and so does a
/// fork. Worked out again whenever the chain moved since, and once the
/// lapse it knew has passed — so a stream that races a chain change is
/// judged on the chain as it is.
struct ChannelLapse {
    device: String,
    /// Admin → the newest time its word counted.
    words: BTreeMap<String, u64>,
    /// The chain seq it was last worked out on.
    seq: Option<u64>,
}

impl ChannelLapse {
    fn new(device: &str) -> Self {
        Self {
            device: device.to_string(),
            words: BTreeMap::new(),
            seq: None,
        }
    }

    /// When control lapses, as last worked out (`None`: nothing counts).
    fn at(&self) -> Option<u64> {
        self.words.values().max().map(|t| t.saturating_add(rh_proto::chain::CONTROL_FRESHNESS_MS))
    }

    /// Work it out again on `st` at `now`.
    fn update(&mut self, st: &AgentState, now: u64) {
        self.seq = st.chain.as_ref().map(ChainState::seq);
        let Some(chain) = st.chain.as_ref() else {
            self.words.clear();
            return;
        };
        self.words.retain(|admin, _| chain.admin(admin).is_some());
        for att in st.attestations.values() {
            match word_of(st, chain, att, &self.device, now) {
                Word::Counts(t) => {
                    let word = self.words.entry(att.device.clone()).or_insert(t);
                    *word = (*word).max(t);
                }
                Word::Fork => {
                    self.words.clear();
                    return;
                }
                Word::Ahead | Word::No => {}
            }
        }
    }

    /// Note what one attestation that just arrived says, before the state
    /// keeps it — where it may replace a word that counted (a sole admin's
    /// renewal, then its first word on a head where it is one of two).
    fn note(&mut self, st: &AgentState, att: &Attestation, now: u64) {
        let Some(chain) = st.chain.as_ref() else { return };
        if let Word::Counts(t) = word_of(st, chain, att, &self.device, now) {
            let word = self.words.entry(att.device.clone()).or_insert(t);
            *word = (*word).max(t);
        }
    }

    /// Whether control holds at `now`: worked out again first if the chain
    /// moved since, or the lapse it knew has passed.
    fn holds(&mut self, st: &AgentState, now: u64) -> bool {
        if st.chain.as_ref().map(ChainState::seq) != self.seq || !self.at().is_some_and(|at| now <= at) {
            self.update(st, now);
        }
        self.at().is_some_and(|at| now <= at)
    }
}

/// Whether the device of `lapse` may still control this machine at `now`:
/// the chain lets it ([`rh_proto::chain::ChainState::may_control`]) and its
/// control has not lapsed.
fn still_controls(shared: &Shared, lapse: &Mutex<ChannelLapse>, now: u64) -> bool {
    let st = shared.state.lock().expect("agent state");
    let mut lapse = lapse.lock().expect("channel lapse");
    let may = st.chain.as_ref().is_some_and(|c| c.may_control(&lapse.device, &st.machine).is_ok());
    lapse.holds(&st, now) && may
}

/// How long an open channel sleeps before it looks at its lapse again: just
/// past it, never more than [`LAPSE_POLL`].
fn lapse_wait(lapses_at: Option<u64>, now: u64) -> Duration {
    match lapses_at {
        Some(at) => Duration::from_millis(at.saturating_sub(now).saturating_add(1)).min(LAPSE_POLL),
        None => Duration::ZERO,
    }
}

/// End an open channel whose control lapsed: its streams and its direct path
/// with it; the device hears [`CHANNEL_STALE`] and may come back once an admin
/// vouches for this machine's view again.
fn close_lapsed(channel: &Channel, device: &str, ch: u32) {
    tracing::warn!(%device, ch, "closing a channel: no admin device vouched for this machine's view of the account in seven days");
    channel.close(CHANNEL_STALE, "stale chain view: no admin device vouched for it in seven days; attest and reconnect");
}

/// Forget the routes of channels that ended here (a lapse, a protocol
/// error): neither the hub nor a relay echoes the `CLOSE` this agent sent.
pub(super) fn drop_ended(routes: &mut Routes) {
    routes.retain(|_, (_, route)| !route.is_closed());
}

/// The agent's own routes over a channel ([`bridge::LOCAL_PREFIX`]): what a
/// device tells the agent itself rather than its API.
///
/// - `GET /rh/info` → `{displayName, hostname, mkEpoch, machine, alarms,
///   controllers}` (`controllers` as in the status stream, [`state::Controllers`])
/// - `POST /rh/display-name {name}` — what devices call this machine
/// - `POST /rh/machine-key {epoch, key, reencrypt}` — a machine key handed
///   over inside the end-to-end channel (the mailbox is only the fallback for
///   an agent that is offline): only from a current full device, and only the
///   key the chain committed for that epoch (409 `uncommitted` /
///   `key_mismatch` otherwise). The current key again with `reencrypt` starts
///   the re-encryption a mailed key could not ask for.
struct AgentLocal {
    shared: Arc<Shared>,
    /// The channel's control lapse, shared with the channel's own task: what
    /// a request is held to again when it is dispatched.
    lapse: Arc<Mutex<ChannelLapse>>,
}

impl AgentLocal {
    fn new(shared: Arc<Shared>, lapse: Arc<Mutex<ChannelLapse>>) -> Self {
        Self { shared, lapse }
    }
}

impl bridge::Local for AgentLocal {
    fn standing(&self, device: &str) -> bridge::Standing {
        let controls = still_controls(&self.shared, &self.lapse, now_ms());
        let st = self.shared.state.lock().expect("agent state");
        let Some(chain) = st.chain.as_ref() else {
            return bridge::Standing {
                admin: false,
                view: bridge::View::Stale,
                controls: false,
            };
        };
        use rh_proto::chain::Freshness;
        let view = match chain.freshness(device, &st.attestation_list(), now_ms(), rh_proto::chain::MANAGE_FRESHNESS_MS) {
            Ok(Freshness::Fresh) => bridge::View::Fresh,
            Ok(Freshness::Stale { .. }) => bridge::View::Stale,
            Ok(Freshness::Behind { .. }) => bridge::View::Behind,
            Err(ChainError::Fork { .. }) => bridge::View::Fork,
            Err(_) => bridge::View::Stale,
        };
        bridge::Standing {
            admin: chain.admin(device).is_some(),
            view,
            controls,
        }
    }

    fn handle<'a>(&'a self, device: &'a str, method: &'a str, path: &'a str, body: &'a [u8]) -> futures_util::future::BoxFuture<'a, (u16, Value)> {
        Box::pin(async move { self.route(device, method, path, body) })
    }
}

impl AgentLocal {
    fn route(&self, device: &str, method: &str, path: &str, body: &[u8]) -> (u16, Value) {
        let shared = &self.shared;
        let full = {
            let st = shared.state.lock().expect("agent state");
            st.chain.as_ref().and_then(|c| c.admin(device)).is_some()
        };
        let bad = |message: &str| (400, json!({"error": "bad_request", "message": message}));
        match (method, path) {
            ("GET", "/rh/info") => {
                let st = shared.state.lock().expect("agent state");
                (
                    200,
                    json!({"displayName": st.display_name, "hostname": crate::api::hostname(), "mkEpoch": st.mk_epoch, "machine": st.machine, "alarms": st.alarms, "controllers": st.controllers()}),
                )
            }
            (_, "/rh/display-name" | "/rh/machine-key") if !full => (
                403,
                json!({"error": "forbidden", "message": "only a current full device of the account may do this"}),
            ),
            ("POST", "/rh/display-name") => {
                let Ok(v) = serde_json::from_slice::<Value>(body) else {
                    return bad("JSON expected");
                };
                let name = match &v["name"] {
                    Value::Null => None,
                    Value::String(s) => {
                        let s = s.trim();
                        if s.chars().count() > 64 || s.chars().any(char::is_control) {
                            return bad("a name is at most 64 characters, none of them control characters");
                        }
                        (!s.is_empty()).then(|| s.to_string())
                    }
                    _ => return bad("name must be a string or null"),
                };
                shared.state.lock().expect("agent state").display_name = name.clone();
                shared.save();
                tracing::info!(%device, "display name changed");
                shared.sync.send(super::streams::Event::DisplayName(name.clone()));
                (200, json!({"displayName": name}))
            }
            ("POST", "/rh/machine-key") => {
                let Ok(v) = serde_json::from_slice::<Value>(body) else {
                    return bad("JSON expected");
                };
                let (Some(epoch), Some(key)) = (
                    v["epoch"].as_u64().and_then(|e| u32::try_from(e).ok()),
                    v["key"]
                        .as_str()
                        .and_then(unhex)
                        .and_then(|k| <[u8; 32]>::try_from(k).ok())
                        .map(zeroize::Zeroizing::new),
                ) else {
                    return bad("{epoch, key (64 hex)} expected");
                };
                let reencrypt = v["reencrypt"].as_bool().unwrap_or(false);
                let current = {
                    let st = shared.state.lock().expect("agent state");
                    // Exactly the key the chain committed for this epoch (M2
                    // review): never one of the sender's choosing, never an
                    // epoch the chain has not reached.
                    let committed = st.chain.as_ref().map(|c| c.check_machine_key(&st.machine, epoch, key.as_ref()));
                    match committed {
                        Some(Ok(())) => {}
                        Some(Err(ChainError::UncommittedKey { .. })) => {
                            return (
                                409,
                                json!({"error": "uncommitted", "message": "the chain commits no such content key epoch (yet): sync and attest, then retry", "mkEpoch": st.mk_epoch}),
                            )
                        }
                        _ => {
                            return (
                                409,
                                json!({"error": "key_mismatch", "message": "not the key the chain committed for that epoch", "mkEpoch": st.mk_epoch}),
                            )
                        }
                    }
                    st.mk_epoch
                };
                if let Err(error) = state::store_machine_key(&shared.paths, epoch, key.as_ref()) {
                    tracing::error!(%error, "cannot store a machine key");
                    return (500, json!({"error": "unavailable", "message": "cannot store the key"}));
                }
                let newer = current.is_none_or(|c| epoch > c);
                if newer {
                    shared.state.lock().expect("agent state").mk_epoch = Some(epoch);
                    shared.save();
                    tracing::info!(epoch, %device, reencrypt, "took a machine key inside a channel");
                    shared.sync.send(super::streams::Event::KeysChanged { reencrypt });
                    return (200, json!({"mkEpoch": epoch, "changed": true}));
                }
                // An epoch it holds already (or an older one, kept for
                // reading): with `reencrypt`, the re-encryption a mailed key
                // could not ask for.
                if reencrypt && current == Some(epoch) {
                    tracing::info!(epoch, %device, "re-encryption asked for the current machine key");
                    shared.sync.send(super::streams::Event::KeysChanged { reencrypt: true });
                }
                (
                    200,
                    json!({"mkEpoch": current, "changed": false, "reencrypt": reencrypt && current == Some(epoch)}),
                )
            }
            _ => (404, json!({"error": "not_found", "message": "no such agent route"})),
        }
    }
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

fn server_info(direct: bool) -> Vec<u8> {
    json!({"agent": "remote-hand", "version": crate::api::SERVER_VERSION, "features": super::direct::features(direct)})
        .to_string()
        .into_bytes()
}

fn now_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

/// Direct hellos admitted by exactly the rules of [`accept`]: this agent's
/// live chain and clock, the freshness gate, the replay guard (persisted
/// before the reply leaves). Entries a hello teaches go to the connection,
/// which acts on them as on any chain change.
struct LinkAdmit {
    shared: Arc<Shared>,
}

impl Admit for LinkAdmit {
    fn admit(&self, device: &str, bind: &DirectBinding, hello: &[u8]) -> Admission {
        let shared = &self.shared;
        let now = now_ms();
        let (outcome, learnt, recorded) = {
            let mut st = shared.state.lock().expect("agent state");
            let st = &mut *st;
            let attestations = st.attestation_list();
            let Some(chain) = st.chain.as_mut() else {
                return Admission::Refused {
                    reply: None,
                    why: "no chain".into(),
                };
            };
            let before = chain.seq();
            let mut guard = shared.guard.lock().expect("replay guard");
            let side = AgentSide {
                chain,
                clock: &mut st.clock,
                guard: &mut guard,
                key: shared.identity.dh(),
                machine: &st.machine,
                policy: RekeyPolicy::default(),
                server_info: &server_info(shared.direct),
                attestations: &attestations,
                freshness_max_age_ms: FRESHNESS_MAX_AGE_MS,
            };
            let outcome = handshake::accept_direct(side, device, bind, hello, now);
            let mut recorded = None;
            if matches!(outcome, Ok(Accepted::Open { .. })) {
                st.save_guard(&guard);
                // The guard, any entries and the device, saved before the
                // reply leaves — in this one step under the lock.
                recorded = Some(st.record_controller(&shared.paths, device, now));
            }
            // Whatever the hello taught took effect, refused or not (M2
            // re-review: a hello whose later entry failed lost the earlier ones).
            (outcome, st.entries_after(Some(before)), recorded)
        };
        if recorded.is_none() {
            shared.save();
        }
        let unsaved = recorded.as_ref().is_some_and(|r| !let_in(shared, device, r));
        // Acted on here, whether or not the hub link is up (chain v3 review:
        // entries a direct hello taught while it was down were saved, and
        // nothing else — no alert, and no relay channel swept).
        learnt_from_direct(shared, &learnt);
        if let Err(error) = &outcome {
            if error.is_alarm() {
                raise_alarm(
                    shared,
                    format!("device {device}'s view of the device chain conflicts with this device's ({error})"),
                );
            }
        }
        match outcome {
            Ok(Accepted::Open { .. }) if unsaved => Admission::Refused {
                reply: None,
                why: "cannot save the record of who connects".into(),
            },
            Ok(Accepted::Open { reply, transport, .. }) => Admission::Open { reply, transport },
            Ok(Accepted::NeedEntries { reply, .. }) => Admission::Refused {
                reply: Some(reply),
                why: "the device has seen more of the chain".into(),
            },
            Ok(Accepted::Stale { reply, .. }) => Admission::Refused {
                reply: Some(reply),
                why: "stale chain view".into(),
            },
            Err(error) => Admission::Refused {
                reply: None,
                why: error.to_string(),
            },
        }
    }
}

/// Whether a device a handshake let in may have its channel, as its
/// [`state::Recorded`] says — and the status told, when it is new and
/// saved. A device new to the record gets in only once it is on disk; one
/// on the saved record gets in whatever the disk does: a full disk or a
/// read-only state directory must not lock every device out of a computer
/// controlled from afar.
fn let_in(shared: &Arc<Shared>, device: &str, recorded: &state::Recorded) -> bool {
    match recorded {
        state::Recorded::New => {
            shared.sync.send(super::streams::Event::Controllers);
            true
        }
        state::Recorded::Known => true,
        state::Recorded::Unsaved(error) => {
            tracing::error!(%device, %error, "cannot save the agent state; refusing a device not on its record yet");
            false
        }
        state::Recorded::KnownUnsaved(error) => {
            tracing::error!(%device, %error, "cannot save the agent state; letting in a device already on its record");
            true
        }
    }
}

/// Entries a direct hello taught (already applied to the state): acted on as
/// any chain advance, every connection's route table swept.
fn learnt_from_direct(shared: &Arc<Shared>, learnt: &[Entry]) {
    if !learnt.is_empty() {
        tracing::info!(n = learnt.len(), "caught up on the device chain from a direct hello");
        chain_moved(shared, learnt, None);
    }
}

/// Whether `incoming` is a device's signaling stream for direct paths.
fn is_signaling(incoming: &Incoming) -> bool {
    incoming.class == StreamClass::Mux && RequestHead::decode(&incoming.meta).is_ok_and(|h| h.path == path::SIGNAL_PATH)
}

/// Refuse a signaling stream (direct paths off, or a channel without paths).
async fn refuse_signaling(incoming: Incoming) {
    let head = ResponseHead {
        status: 403,
        headers: Vec::new(),
    };
    if let Ok(bytes) = head.encode() {
        let _ = incoming
            .stream
            .send(&rh_proto::bridge::message(rh_proto::bridge::Kind::Head, &bytes))
            .await;
    }
    incoming.stream.close(rh_proto::bridge::CLOSE_FORBIDDEN);
}

/// The chain advanced by `entries` on a connection (from the hub, or a
/// device's hello on the hub or a relay): close this connection's channels of
/// devices that may no longer control this machine at once, then
/// [`chain_moved`]. Whatever the source (M2 review: a removal learnt from a
/// hello used to leave the removed device's channel open).
fn chain_advanced(
    shared: &Arc<Shared>,
    entries: &[Entry],
    routes: &mut Routes,
    out: &mpsc::UnboundedSender<Frame>,
    text: &mpsc::UnboundedSender<String>,
) {
    drop_removed(shared, routes, out);
    if !chain_moved(shared, entries, Some(text)) {
        routes.clear();
    }
}

/// The chain advanced by `entries`, wherever they came from — the hub, a
/// device's hello on the hub or a relay, a direct hello, with the hub link up
/// or down: saved; every route table told to sweep
/// ([`Shared::chain_version`]: the hub connection and each relay connection
/// close the channels of devices that may no longer control this machine);
/// the account changes raised; this machine's own removal acted on; the mail
/// that waited for it tried again (on `text`, else the hub link's when it is
/// up). `false` when the chain removed this machine.
fn chain_moved(shared: &Arc<Shared>, entries: &[Entry], text: Option<&mpsc::UnboundedSender<String>>) -> bool {
    shared.save();
    shared.chain_version.send_modify(|v| *v += 1);
    // The status's `controllers` says which chain it is about: a new record.
    shared.sync.send(super::streams::Event::Controllers);
    alert_account_changes(shared, entries);
    if shared.state.lock().expect("agent state").removed_in_chain() {
        on_removed(shared, "its device chain says so");
        return false;
    }
    let hub = shared.hub_text.lock().expect("hub text").clone();
    if let Some(text) = text.or(hub.as_ref()) {
        retry_mail(shared, text);
    }
    true
}

/// An enrollment the link completes (the hub assigned the id; a device has
/// just added the machine): exactly with this machine's keys, or it is
/// refused — an alarm, and the link stops.
fn check_enrolled_keys(shared: &Arc<Shared>) -> bool {
    let (sign, dh) = (shared.identity.sign_public(), shared.identity.dh_public());
    let (machine, ok) = {
        let st = shared.state.lock().expect("agent state");
        (st.machine.clone(), st.keys_match(&sign, &dh))
    };
    if ok {
        tracing::info!(%machine, "enrolled: the device chain names this machine with its keys");
        return true;
    }
    raise_alarm(
        shared,
        format!("the device chain added {machine} with other keys than this device's; refusing it"),
    );
    shared.cancel.cancel();
    false
}

fn on_frame(
    shared: &Arc<Shared>,
    frame: Frame,
    routes: &mut Routes,
    pending: &mut Pending,
    out: &mpsc::UnboundedSender<Frame>,
    text: &mpsc::UnboundedSender<String>,
) {
    match frame {
        Frame::Chain { entries, .. } => {
            let Ok(entries) = entries.iter().map(|e| Entry::decode(e)).collect::<Result<Vec<_>, _>>() else {
                tracing::warn!("undecodable chain entries from the hub");
                return;
            };
            let (was_enrolled, absorbed) = {
                let mut st = shared.state.lock().expect("agent state");
                let was = st.enrolled();
                (was, st.absorb(&entries, now_ms()))
            };
            let now_enrolled = shared.state.lock().expect("agent state").enrolled();
            if !was_enrolled && now_enrolled && !check_enrolled_keys(shared) {
                return;
            }
            // Everything that took effect is acted on — saved, removed
            // devices' channels closed, every change alerted — even when the
            // batch stopped short (M2 re-review: a join followed by a spoiled
            // entry used to land silently). The first batch at enrollment is
            // history: `news` is empty for it.
            if absorbed.applied > 0 {
                tracing::info!(n = absorbed.applied, "device chain advanced");
                chain_advanced(shared, &absorbed.news, routes, out, text);
            }
            match absorbed.error {
                None => {}
                Some(e) if is_conflict(&e) => {
                    raise_alarm(
                        shared,
                        format!("the hub served a device chain that conflicts with this device's ({e}); refusing it"),
                    );
                }
                Some(ChainError::TooEarly { seq, ready_ms }) => {
                    *shared.held_until.lock().expect("held") = Some(ready_ms);
                    tracing::warn!(
                        seq,
                        ready_ms,
                        "a recovered device's self-activation arrived before this machine's own day since its join; holding it (and what follows) until then"
                    );
                }
                Some(error) => tracing::warn!(%error, "refused chain entries"),
            }
        }
        Frame::Attest { attestation, .. } => {
            if let Ok(att) = Attestation::decode(&attestation) {
                if take_attestation(shared, att) {
                    shared.save();
                }
            }
        }
        Frame::Ack { .. } | Frame::Conflict { .. } => shared.sync.send(super::streams::Event::Frame(frame)),
        frame => on_channel_frame(shared, frame, routes, pending, out, text),
    }
}

/// Take an attestation the hub relayed: every open channel's lapse notes
/// what it says first ([`ChannelLapse::note`]), then the state keeps it as
/// its device's newest — so a word that counted for a channel is in that
/// channel's lapse before a later word from the same admin, one that does
/// not count, replaces it in the state. Whether the state took it.
fn take_attestation(shared: &Shared, att: Attestation) -> bool {
    let mut st = shared.state.lock().expect("agent state");
    let now = now_ms();
    shared.lapses.lock().expect("lapses").retain(|weak| match weak.upgrade() {
        Some(lapse) => {
            lapse.lock().expect("channel lapse").note(&st, &att, now);
            true
        }
        None => false,
    });
    st.attest(att)
}

/// A channel's frame — from the hub (while it still relays) or from a relay
/// ([`super::relays`]): `OPEN` (the device's first message comes next),
/// `DATA`, `CREDIT`, `CLOSE`. Routes are per connection, and so are the
/// channels opening: at most [`MAX_PENDING`], none for longer than
/// [`PENDING_FOR`].
pub(super) fn on_channel_frame(
    shared: &Arc<Shared>,
    frame: Frame,
    routes: &mut Routes,
    pending: &mut Pending,
    out: &mpsc::UnboundedSender<Frame>,
    text: &mpsc::UnboundedSender<String>,
) {
    match frame {
        Frame::Open { ch, peer } => {
            let now = Instant::now();
            expire_pending(pending, out, now);
            if pending.len() >= MAX_PENDING && !pending.contains_key(&ch) {
                refuse(out, ch, 4409, "too many channels opening");
            } else {
                pending.insert(ch, (peer, now));
            }
        }
        Frame::Data { ch, payload } => {
            if let Some((_, route)) = routes.get(&ch) {
                let _ = route.send(Frame::Data { ch, payload });
            } else if let Some((device, _)) = pending.remove(&ch) {
                accept(shared, ch, device, &payload, routes, out, text);
            }
        }
        Frame::Credit { ch, bytes } => {
            if let Some((_, route)) = routes.get(&ch) {
                let _ = route.send(Frame::Credit { ch, bytes });
            }
        }
        Frame::Close { ch, code, reason } => {
            pending.remove(&ch);
            if let Some((_, route)) = routes.remove(&ch) {
                let _ = route.send(Frame::Close { ch, code, reason });
            }
        }
        _ => {}
    }
}

/// Refuse the channels that opened [`PENDING_FOR`] before `now` and sent no
/// first message since.
pub(super) fn expire_pending(pending: &mut Pending, out: &mpsc::UnboundedSender<Frame>, now: Instant) {
    pending.retain(|ch, (_, opened)| {
        let waiting = now.saturating_duration_since(*opened) < PENDING_FOR;
        if !waiting {
            refuse(out, *ch, 4408, "no first message in time");
        }
        waiting
    });
}

fn refuse(out: &mpsc::UnboundedSender<Frame>, ch: u32, code: u16, reason: &str) {
    let _ = out.send(Frame::Close {
        ch,
        code,
        reason: reason.into(),
    });
}

/// A device's first handshake message on `ch`.
fn accept(
    shared: &Arc<Shared>,
    ch: u32,
    device: String,
    message: &[u8],
    routes: &mut Routes,
    out: &mpsc::UnboundedSender<Frame>,
    text: &mpsc::UnboundedSender<String>,
) {
    let now = now_ms();
    let (outcome, learnt, recorded, lapse) = {
        let mut st = shared.state.lock().expect("agent state");
        let st = &mut *st;
        let attestations = st.attestation_list();
        let Some(chain) = st.chain.as_mut() else {
            return refuse(out, ch, 4503, "this device has not been added to the account yet");
        };
        let before = chain.seq();
        let mut guard = shared.guard.lock().expect("replay guard");
        let side = AgentSide {
            chain,
            clock: &mut st.clock,
            guard: &mut guard,
            key: shared.identity.dh(),
            machine: &st.machine,
            policy: RekeyPolicy::default(),
            server_info: &server_info(shared.direct),
            attestations: &attestations,
            freshness_max_age_ms: FRESHNESS_MAX_AGE_MS,
        };
        let outcome = handshake::accept(side, &device, message, now);
        let mut recorded = None;
        if matches!(outcome, Ok(Accepted::Open { .. })) {
            st.save_guard(&guard);
            // Who ever opened a channel (review D3), recorded as it opens and
            // saved with the guard (and any entries the hello taught) before
            // the reply leaves — in this one step under the lock.
            recorded = Some(st.record_controller(&shared.paths, &device, now));
        }
        // The channel's control lapse, seeded and registered while the state
        // is still this admission's: an attestation arriving from here on
        // reaches it ([`take_attestation`]), and none can come in between.
        let lapse = matches!(outcome, Ok(Accepted::Open { .. })).then(|| {
            let lapse = Arc::new(Mutex::new(ChannelLapse::new(&device)));
            lapse.lock().expect("channel lapse").update(st, now);
            let mut lapses = shared.lapses.lock().expect("lapses");
            lapses.retain(|weak| weak.strong_count() > 0);
            lapses.push(Arc::downgrade(&lapse));
            lapse
        });
        // Entries a hello taught count as much as the hub's — every one that
        // took effect, whether or not the handshake then failed (M2 re-review).
        (outcome, st.entries_after(Some(before)), recorded, lapse)
    };
    #[cfg(test)]
    if let Some(hook) = shared.after_admission.lock().expect("hook").take() {
        hook();
    }
    if !learnt.is_empty() {
        tracing::info!(applied = learnt.len(), "caught up on the device chain from a hello");
        chain_advanced(shared, &learnt, routes, out, text);
    }
    match outcome {
        Ok(Accepted::Open { reply, transport, hello, .. }) => {
            // Saved above (a device not on record yet gets in only once it is).
            if recorded.as_ref().is_some_and(|r| !let_in(shared, &device, r)) {
                return refuse(out, ch, AGENT_UNSAVED, "this device cannot save its record of who connects to it: check its disk");
            }
            let _ = out.send(Frame::Data { ch, payload: reply });
            let (tx, rx) = mpsc::unbounded_channel();
            routes.insert(ch, (device.clone(), tx));
            // A device that speaks sequenced frames gets them (and may ask
            // for a direct path); an older one gets the plain channel.
            let paths = path::client_wants_paths(&hello.client);
            // A device that reads streams named per key epoch: from now on
            // (remembered) the agent names them so.
            if rh_proto::stream::reads_epoch_names(&hello.client) {
                shared.sync.send(super::streams::Event::ReadsEpochNames);
            }
            let relay_hash = transport.handshake_hash();
            let session = if paths {
                Session::with_paths(ch, *transport, Role::Responder, PathOptions::default(), now_us())
            } else {
                Session::new(ch, *transport, Role::Responder)
            };
            let (channel, mut incoming): (Channel, _) = Channel::spawn(session, rx, out.clone());
            let loopback = shared.loopback.clone();
            // Taken after the chain's version is: a change landing in between
            // is one the channel's task sees (and its lapse is worked out
            // again on any chain moved since, [`ChannelLapse::holds`]).
            let mut moved = shared.chain_version.subscribe();
            let lapse = lapse.expect("an open channel's lapse, registered at its admission");
            let local: Arc<dyn bridge::Local> = Arc::new(AgentLocal::new(shared.clone(), lapse.clone()));
            let direct = (paths && shared.direct).then(|| {
                let (account, machine) = {
                    let st = shared.state.lock().expect("agent state");
                    (st.account.clone(), st.machine.clone())
                };
                Arc::new(AgentDirect {
                    admit: Arc::new(LinkAdmit { shared: shared.clone() }),
                    timeout: super::direct::ATTEMPT_TIMEOUT,
                    opts: super::direct::options(),
                    peers: shared.peers.clone(),
                    binding: Binding {
                        account,
                        device: device.clone(),
                        machine,
                    },
                    relay_hash,
                })
            });
            tracing::info!(%device, ch, paths, direct = direct.is_some(), "channel open");
            let shared = shared.clone();
            tokio::spawn(async move {
                // Control lapses on an open channel too (local-aggregator
                // §3.4, [`ChannelLapse`]): looked at when it is due, whenever
                // the chain moves (an admin whose word kept it fresh may be
                // one no longer), on every stream, and again when a request
                // is dispatched ([`bridge::Standing::controls`]). A lapse
                // ends the channel — its terminals, its direct path —
                // however long the hub keeps the connection up and a
                // removal to itself.
                let holds = || {
                    let st = shared.state.lock().expect("agent state");
                    lapse.lock().expect("channel lapse").holds(&st, now_ms())
                };
                let mut vouched = holds();
                loop {
                    if !vouched {
                        close_lapsed(&channel, &device, ch);
                        break;
                    }
                    let wait = lapse_wait(lapse.lock().expect("channel lapse").at(), now_ms());
                    let stream = tokio::select! {
                        next = incoming.recv() => match next {
                            Some(stream) => stream,
                            None => break,
                        },
                        changed = moved.changed() => {
                            if changed.is_err() {
                                break;
                            }
                            vouched = holds();
                            continue;
                        }
                        () = tokio::time::sleep(wait) => {
                            vouched = holds();
                            continue;
                        }
                    };
                    // Every stream re-checks that the device is still in the
                    // account (M2 review): a removal that lands mid-channel
                    // stops new requests at once, whatever closes the channel.
                    // Chain v3: and that this machine still accepts control
                    // from it (an accept list changed, an admin demoted,
                    // this machine stopped running AI). And that its control
                    // has not lapsed, on the chain as it is now.
                    let (current, fresh) = {
                        let st = shared.state.lock().expect("agent state");
                        let current = st.chain.as_ref().is_some_and(|c| c.may_control(&device, &st.machine).is_ok());
                        (current, lapse.lock().expect("channel lapse").holds(&st, now_ms()))
                    };
                    if !current {
                        tracing::info!(%device, ch, "a stream from a device that may no longer control this machine; closing its channel");
                        break;
                    }
                    if !fresh {
                        close_lapsed(&channel, &device, ch);
                        break;
                    }
                    if is_signaling(&stream) {
                        match &direct {
                            Some(ctx) => {
                                tracing::info!(%device, ch, "a device asks for a direct path");
                                tokio::spawn(rh_proto::direct::serve(stream.stream, channel.clone(), ctx.clone()));
                            }
                            None => {
                                tokio::spawn(refuse_signaling(stream));
                            }
                        }
                        continue;
                    }
                    tokio::spawn(bridge::serve(stream, device.clone(), loopback.clone(), local.clone()));
                }
                // The channel ended: its lapse leaves the registry (as any other
                // ended one's still listed), not only when an attestation comes.
                drop(local);
                drop(lapse);
                shared.lapses.lock().expect("lapses").retain(|weak| weak.strong_count() > 0);
            });
        }
        Ok(Accepted::NeedEntries { reply, from, .. }) => {
            tracing::info!(%device, from, "a device has seen more of the chain than this agent; asking for entries");
            let _ = out.send(Frame::Data { ch, payload: reply });
            refuse(out, ch, 4409, "send chain entries");
        }
        Ok(Accepted::Stale { reply, newest_ms, .. }) => {
            tracing::info!(%device, ?newest_ms, "refused a channel: nobody has vouched for this agent's chain head recently");
            let _ = out.send(Frame::Data { ch, payload: reply });
            refuse(out, ch, CHANNEL_STALE, "stale chain view: attest and retry");
        }
        Err(error) => {
            if error.is_alarm() {
                raise_alarm(
                    shared,
                    format!("device {device}'s view of the device chain conflicts with this device's ({error})"),
                );
            } else {
                tracing::info!(%device, %error, "refused a handshake");
            }
            refuse(out, ch, 4401, "handshake refused");
        }
    }
}

#[cfg(test)]
mod tests {
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
            loopback: Arc::new(Loopback::new("127.0.0.1:9".parse().unwrap(), "test".into())),
            sync,
            state: Arc::new(Mutex::new(st)),
            guard: Mutex::new(guard),
            cancel: CancellationToken::new(),
            direct: false,
            peers: Arc::new(Semaphore::new(1)),
            held_until: Mutex::new(None),
            alerts: Mutex::new(Vec::new()),
            chain_version: tokio::sync::watch::channel(0).0,
            lapses: Mutex::new(Vec::new()),
            after_admission: Mutex::new(None),
            hub_text: Mutex::new(None),
            relays: Mutex::new(None),
            health: Mutex::new(Health::default()),
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

    /// The batch of the PoC: d-2 removed, a thief joining with the recovery
    /// words, then an entry with a spoiled signature.
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
            *a.shared.alerts.lock().unwrap(),
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
        HashMap::from([(7, ("d-2".to_string(), tx))])
    }

    // ---- the policy's wiring (chain v3 review, finding 8) ----------------

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
        let agent = routes.get(&ch).unwrap().1.clone();
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

    /// The 412's `reason` follows the freshness check (web half of the chain
    /// v3 review): an admin vouching for a head beyond this agent's view
    /// makes it `behind`; once this agent takes a different entry at that
    /// seq, the same vouching conflicts with its view: `fork`.
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

    /// Review D3: a device whose handshake this agent accepts is recorded
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
        let path = state::state_path(&a.shared.paths);
        if broken {
            let _ = std::fs::remove_file(&path);
            std::fs::create_dir_all(path.join("in-the-way")).unwrap();
        } else {
            std::fs::remove_dir_all(&path).unwrap();
        }
    }

    /// The handshake is saved before its reply leaves (review of #117): a
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

    /// Review of #117: handshakes from a device new to the record arriving
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

    /// The race of the #117 review, made to happen every time: a hub
    /// handshake from a device new to the record is held between noting it
    /// and saving it (a test hook in [`AgentState::record_controller`])
    /// while saves fail, and a direct handshake from the same device comes
    /// in then. When the note and the save were two steps, the second saw
    /// the first's note, took the device for one on the saved record and let
    /// it in while the first was refused and took its note back — on neither
    /// disk nor memory, yet in. Now it waits for the first to finish, and is
    /// refused the same way.
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

    /// Review D3, direct paths: a device admitted on a DataChannel is
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
            (1, ("d-1".to_string(), tx.clone())),
            (2, ("d-2".to_string(), tx.clone())),
            (3, ("d-3".to_string(), tx)),
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

    // ---- control lapses on open channels too (external E2E review, item D) ----

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
    /// channels before the new one vouches (review of #177).
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

    /// Review of #177 (left over): a sole admin's renewal, then its first word
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

    /// Third look at #177: a renewal arriving right after a handshake's
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

    /// Third look at #177: a channel that ended leaves the lapse registry
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

    /// Third look at #177 (optional): who was the sole admin as of a seq is
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

    /// Review of #177: a sole admin pairing a second admin used to have
    /// every open channel of its own closed until the new admin vouched.
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

    /// Review of #177: a hub that holds one honest admin back at an old head
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

    /// Review of #177: a stream that races a chain change taking away the
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

    /// Review of #177: a management request is held to the twelve hours when
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

    /// Review of #177: routes of channels that ended here go — neither the
    /// hub nor a relay echoes this agent's `CLOSE`.
    #[test]
    fn routes_of_channels_that_ended_here_are_forgotten() {
        let (ended, rx) = mpsc::unbounded_channel();
        drop(rx);
        let (open, rx) = mpsc::unbounded_channel();
        std::mem::forget(rx);
        let mut routes: Routes = HashMap::from([(1, ("d-1".to_string(), ended)), (2, ("d-2".to_string(), open))]);
        drop_ended(&mut routes);
        assert_eq!(routes.keys().copied().collect::<Vec<_>>(), vec![2]);
    }

    // ---- a member that stops running AI (chain v3 review, finding 12) ----

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

    /// The PoC `merge_partial.rs`, at the link (M2 re-review #26.1, and the
    /// review of the fix: only `absorb`'s return value was tested). A hub
    /// batch that fails half-way is still acted on for everything it applied.
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

    /// M2 re-review: a self-activation with the recovery words — the moment a
    /// recovered device starts to count — raised no alert anywhere. Every
    /// change to who is in the account is said now, and nothing else is.
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
        // Machines too (S1-1): a machine added or removed is said everywhere.
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

    /// Chain v3 checklist item 13: every device added (with what it may do),
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
            Some("identity key renewals may be automatic again (set by d-1)")
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

    /// Security review: a relay is not trusted, and every `OPEN` it sent was
    /// held until that channel's first message or `CLOSE` — a relay could
    /// grow the agent's memory without bound. At most 256 wait per
    /// connection; the rest are refused.
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

    /// The same review: an `OPEN` with no first message waits 30 s at most,
    /// then it is refused and its room is free again.
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

    /// Review of the local sign-in: a sign-in the hub refuses as "removed"
    /// used to mark this machine removed for good — the link never started
    /// again, the settings said "removed" and offered to wipe the keys — on
    /// the hub's word alone. Only this machine's own verified chain removes
    /// it; a refusal is a failure like any other: said, and retried.
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
        // The other devices' keys it held as a device (S1-4) go with it.
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

    /// Security re-review of S1-4, 3: stopping a link — also one that already
    /// ended on its own, which is what a restart does first — ends its device
    /// role, however stuck it is.
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
        // And its copies task too (S1-5).
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

    /// Security review of S1-4, S2: a machine whose chain removed it — learnt
    /// by a release that kept `aggregator/`, or before a stop — never starts
    /// its link again, and loses the other devices' keys when it tries.
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

    /// Chain v3 review, finding 6: a host's mailbox is shared between its
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

    /// Chain v3 review: the alert for a new host covers only that host's own
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

    /// Chain v3 review: entries a direct hello taught while the hub link was
    /// down were saved and nothing else — no alert, and no relay connection
    /// told to close the channels of a device the chain had just removed.
    /// They are acted on as any chain advance now: every connection's route
    /// table is told to sweep, and the change is alerted.
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
        assert_eq!(*a.shared.alerts.lock().unwrap(), vec!["device d-2 was removed by d-1".to_string()]);
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

    /// Chain v3 re-review, M11: the hub connection sweeps its own channels on
    /// every chain advance, wherever the entries were learnt. A removal that a
    /// device's hello on a relay, or a direct hello, teaches this agent — the
    /// hub connection never sees those entries itself — still closes the
    /// removed device's channel on the hub path at once (4403), and only that
    /// channel.
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
