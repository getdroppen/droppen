use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use rh_proto::frame::Frame;
use rh_proto::identity::MachineIdentity;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Semaphore};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use super::admit::{expire_pending, on_frame};
use super::alerts::ask_again_for_held;
use super::bridge::Loopback;
use super::hub::{self, now_ms, Backoff, HEARTBEAT, SILENCE};
use super::lapse::{drop_ended, drop_removed};
use super::mail::{defer, evicted_message, open_mail, MailVerdict};
use super::state::AgentState;
use super::{Health, Pending, Routes, Shared, ROTATED};
use crate::sessions::Paths;

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
    pub(in crate::agent) shared: Arc<Shared>,
    pub(in crate::agent) task: tokio::task::JoinHandle<()>,
    /// The streams' writer, which holds `streams.db` open.
    pub(in crate::agent) sync_task: tokio::task::JoinHandle<()>,
    /// This computer's device role ([`crate::aggregator::controller`]), when
    /// it may run (`RH_AGENT_CONTROL`), and what it reports.
    pub(in crate::agent) control: Option<tokio::task::JoinHandle<()>>,
    pub(in crate::agent) control_status: Arc<Mutex<crate::aggregator::controller::Status>>,
    /// Its copies of the other devices ([`crate::aggregator::copies`]), and
    /// what it reports.
    pub(in crate::agent) copies: Option<tokio::task::JoinHandle<()>>,
    pub(in crate::agent) copies_status: Arc<Mutex<crate::aggregator::copies::Status>>,
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

    /// Read the agent state the link keeps.
    pub fn with_state<T>(&self, f: impl FnOnce(&AgentState) -> T) -> T {
        f(&self.shared.state.lock().expect("agent state"))
    }
}

/// How long [`Running::stop`] waits for the device role before ending it.
#[cfg(not(test))]
pub(in crate::agent) const CONTROL_STOP: Duration = Duration::from_secs(10);
#[cfg(test)]
pub(in crate::agent) const CONTROL_STOP: Duration = Duration::from_millis(200);

/// Whether the link may run at all (`DROPPEN_AGENT_LINK=0` turns it off).
pub fn enabled() -> bool {
    crate::env::flag_or("DROPPEN_AGENT_LINK", true)
}

/// Start the link for this instance's saved agent state (an enrollment in
/// progress, or complete). An error, and nothing started, when there is no
/// state, it does not load, the machine key is missing, or the state says
/// this machine was removed. `restart` starts it again in this process once
/// it has taken a renewed identity key ([`super::rotation`]).
pub fn start(
    paths: &Paths,
    env: &LinkEnv,
    live: Arc<crate::aggregator::live::LiveCopies>,
    restart: super::rotation::Restart,
) -> Result<Running, String> {
    // Adding this device again under a new identity, interrupted by a crash:
    // finished first — it never starts as the old enrollment.
    super::rotation::finish_re_enroll(paths)?;
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
        chain_version: tokio::sync::watch::channel(0).0,
        lapses: Mutex::new(Vec::new()),
        #[cfg(test)]
        after_admission: Mutex::new(None),
        hub_text: Mutex::new(None),
        relays: Mutex::new(None),
        health: Mutex::new(Health::default()),
        rotation: super::rotation::Rotation::new(),
        restart,
    });
    if !shared.direct {
        tracing::info!("direct paths are off (DROPPEN_AGENT_DIRECT=0): devices reach this machine through relays only");
    }
    if super::relays::enabled() {
        *shared.relays.lock().expect("relays") = Some(super::relays::spawn(shared.clone()));
    } else {
        tracing::warn!("relays are off (DROPPEN_AGENT_RELAYS=0): devices cannot reach this machine live, only its offline copy");
    }
    // A renewed identity key was promoted: the push keys follow it.
    if shared.state.lock().expect("agent state").rotation.push_renew {
        tokio::spawn(renew_push(shared.clone()));
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
            may_sign_in: Some(sign_in_gate(&shared)),
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
            resync: Some(chain_resync(&shared)),
            may_sign_in: Some(sign_in_gate(&shared)),
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

/// How long [`renew_push`] waits before each try again: then every last one.
pub(in crate::agent) const PUSH_RENEW_AGAIN: [Duration; 4] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(30),
    Duration::from_secs(300),
];

/// R6 review: once a renewed identity key was promoted, a new VAPID
/// keypair and every stored push subscription forgotten
/// ([`crate::push::store::renew_vapid`]) — whoever copied the old VAPID key
/// and endpoints reaches no device that subscribes again. Noted done only
/// once it is (and by this link only while it runs: else the next start
/// does it again, which is harmless). Until then the push key is refused
/// (503 `push_renewing`, [`bridge::push_key_route`]), so a try that fails is
/// tried again while the link runs ([`PUSH_RENEW_AGAIN`]), not only at the
/// next start (review of the renewal fixes).
pub(in crate::agent) async fn renew_push(shared: Arc<Shared>) {
    let mut tries = 0usize;
    loop {
        match crate::push::store::renew_vapid(&shared.paths).await {
            Ok(forgotten) => {
                tracing::warn!(forgotten, "renewed the push keys with the identity key: every device subscribes again");
                if !shared.cancel.is_cancelled() {
                    shared.state.lock().expect("agent state").rotation.push_renew = false;
                    shared.save();
                }
                return;
            }
            Err(error) => {
                let wait = PUSH_RENEW_AGAIN[tries.min(PUSH_RENEW_AGAIN.len() - 1)];
                tries += 1;
                tracing::error!(%error, tries, ?wait, "cannot renew the push keys with the identity key; trying again");
                tokio::select! {
                    () = tokio::time::sleep(wait) => {}
                    () = shared.cancel.cancelled() => return,
                }
            }
        }
    }
}

/// A link's shared state for tests elsewhere in the agent: `st` for this
/// machine running with `identity`, its streams' events to `sync`, no
/// relays and no direct paths, its restarts to `restart`.
#[cfg(test)]
pub(in crate::agent) fn test_shared(
    paths: &Paths,
    st: AgentState,
    identity: MachineIdentity,
    sync: super::streams::Handle,
    restart: super::rotation::Restart,
) -> Arc<Shared> {
    let guard = st.replay_guard(now_ms());
    Arc::new(Shared {
        paths: paths.clone(),
        deferred_mail: Mutex::new(Vec::new()),
        identity: Arc::new(identity),
        loopback: Arc::new(Loopback::new("127.0.0.1:9".parse().expect("an address"), "test".into())),
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
        rotation: super::rotation::Rotation::new(),
        restart,
    })
}

/// Whether this computer's device role may sign in now
/// ([`super::rotation::device_role_may_sign_in`]), for the aggregator.
pub(in crate::agent) fn sign_in_gate(shared: &Arc<Shared>) -> crate::aggregator::SignInGate {
    let shared = Arc::downgrade(shared);
    Arc::new(move || shared.upgrade().is_some_and(|s| super::rotation::device_role_may_sign_in(&s)))
}

/// How often [`chain_resync`] asks the hub for the chain again, at most.
pub(in crate::agent) const RESYNC_EVERY: Duration = Duration::from_secs(60);

/// Ask the hub for the chain past this agent's head again, on its connection
/// when it is up — at most every [`RESYNC_EVERY`]: what a reader of the other
/// devices' copies does when one stops being readable, which a renewal of
/// that device's identity key it has not heard of yet would explain.
pub(in crate::agent) fn chain_resync(shared: &Arc<Shared>) -> Arc<dyn Fn() + Send + Sync> {
    let shared = Arc::downgrade(shared);
    let last: Mutex<Option<Instant>> = Mutex::new(None);
    Arc::new(move || {
        let Some(shared) = shared.upgrade() else { return };
        {
            let mut last = last.lock().expect("resync");
            if last.is_some_and(|at| at.elapsed() < RESYNC_EVERY) {
                return;
            }
            *last = Some(Instant::now());
        }
        let from = shared.state.lock().expect("agent state").seq().map_or(0, |s| s + 1);
        let text = shared.hub_text.lock().expect("hub text").clone();
        if let Some(text) = text {
            tracing::info!(from, "a copy stopped being readable: asking the hub for the chain again");
            let _ = text.send(json!({"t": "chain_sub", "from": from}).to_string());
        }
    })
}

/// The saved agent state a link may start from — none when this instance is
/// not enrolled, or its chain removed it. A removed one also loses what it
/// held to control the other devices (`aggregator/`): its removal may have
/// been learnt by a release that did not delete it (one from before S1-4, or
/// a run that stopped first: security review of S1-4, S2).
pub(in crate::agent) fn startable(paths: &Paths) -> Result<AgentState, String> {
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
/// key this agent's chain committed for its epoch (M2 re-review: the file
/// is not the chain, and outlived re-enrollments).
pub(in crate::agent) fn committed_keys(state: Arc<Mutex<AgentState>>) -> super::streams::KeyCheck {
    Arc::new(move |epoch, key| {
        let st = state.lock().expect("agent state");
        st.chain.as_ref().is_some_and(|c| c.check_machine_key(&st.machine, epoch, key).is_ok())
    })
}

/// The status's `controllers`, read from the agent state when a body is built.
pub(in crate::agent) fn controllers_view(state: Arc<Mutex<AgentState>>) -> super::streams::ControllersView {
    Arc::new(move || state.lock().expect("agent state").controllers())
}

pub(in crate::agent) async fn run(shared: Arc<Shared>) {
    let mut backoff = Backoff::new(hub::BACKOFF_MIN, hub::BACKOFF_MAX, hub::STABLE);
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
        if shared.cancel.is_cancelled() {
            return;
        }
        match result {
            Ok(()) => tracing::info!("hub connection closed; reconnecting"),
            Err(error) => tracing::warn!(%error, "hub connection failed; reconnecting"),
        }
        // A renewal of this device's identity key: the staged key signs in now.
        if shared.rotation.take_again() {
            continue;
        }
        let wait = backoff.after(started.elapsed());
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = shared.cancel.cancelled() => return,
        }
    }
}

pub(in crate::agent) async fn connection(shared: &Arc<Shared>) -> Result<(), String> {
    let (origin, account, machine, from) = {
        let st = shared.state.lock().expect("agent state");
        (
            st.hub.clone(),
            st.account.clone(),
            st.machine.clone(),
            st.chain.as_ref().map_or(0, |c| c.seq() + 1),
        )
    };
    // What its own chain says about its keys first: a renewal it slept
    // through is taken with the staged key below; one by somebody else
    // stops the link here ([`super::rotation`]).
    match super::rotation::on_chain(shared) {
        super::rotation::Verdict::Foreign { .. } => return Err("this device's identity key was replaced elsewhere: not connecting".into()),
        super::rotation::Verdict::Unreadable => {
            return Err("this device was renewed to a staged identity key that does not read: not connecting".into())
        }
        super::rotation::Verdict::Unchanged | super::rotation::Verdict::Renewed => {}
    }
    let stagings = super::rotation::staged_seeds(&shared.paths, &shared.identity);
    // While any staged seed is kept, EVERY sign-in tries the staged keys first
    // (`docs/host-identity-rotation.md` §3.5 「不拿旧钥误伤自己」): a renewal may
    // have been committed while this device slept, its chain still stale. The
    // one its chain names first, then the newest. The key in use signs in only
    // once every staged key was turned away as unknown — never once a renewal
    // is committed, nor after a staged key is refused as `rotated`: the hub
    // keeps a retired key's sign-in as evidence against this very device.
    let has_staged = !stagings.is_empty();
    let named = {
        let st = shared.state.lock().expect("agent state");
        super::rotation::named(&st, &stagings).map(|s| s.nonce)
    };
    let mut order: Vec<&super::StagedSeed> = stagings.iter().collect();
    order.sort_by_key(|s| (Some(s.nonce) != named, std::cmp::Reverse(s.created_ms)));
    // Popped from the end: the first to try last in the list.
    let mut candidates: Vec<MachineIdentity> = order.iter().rev().map(|s| s.identity()).collect();
    let mut staged_key = candidates.pop();
    // Whether the hub turned a staged key away on this attempt.
    let mut staged_refused = false;
    let (mut sink, mut source, nonce, welcome, since) = loop {
        let (mut sink, mut source, nonce) = hub::dial(&origin).await?;
        let identity = staged_key.as_ref().unwrap_or(&shared.identity);
        let since = now_ms();
        match hub::login(&mut sink, &mut source, identity, &account, &machine, &origin, nonce).await {
            Ok(welcome) => break (sink, source, nonce, welcome, since),
            Err(hub::LoginError::Refused { code, .. }) if staged_key.is_some() && code == "rotated" => {
                // A staged key the hub says was retired: renewed to it, then
                // away from it again. The chain it serves here says by whom;
                // the key in use (older still) is not tried.
                tracing::warn!("the hub says a staged identity key was renewed already: reading the chain it serves");
                rotated_sync(shared, &mut sink, &mut source).await;
                if !shared.cancel.is_cancelled() {
                    super::rotation::on_chain(shared);
                }
                return Err("the hub refuses a staged identity key as renewed: not signing in with the key in use".into());
            }
            Err(hub::LoginError::Refused { code, message }) if staged_key.is_some() && code != "denied" => {
                // R6 review: only the hub's `denied` — it does not know the
                // staged key — lets the next key try. Anything else
                // (`unavailable`: its own trouble) is transient: retried
                // with the back-off, the key in use never tried on it.
                let why = format!("the hub could not take a staged identity key now ({code}): {message}");
                tracing::warn!("{why}; trying again later");
                return Err(why);
            }
            Err(hub::LoginError::Removed(message)) if staged_key.is_some() => {
                // The hub's word, as below: retried, never a fallback.
                let why = format!("the hub refused this device: {message}");
                tracing::warn!("{why}; trying again later");
                shared.health.lock().expect("health").refused = Some(why.clone());
                return Err(why);
            }
            Err(error @ hub::LoginError::Refused { .. }) if staged_key.is_some() => {
                staged_refused = true;
                if let Some(next) = candidates.pop() {
                    staged_key = Some(next);
                    continue;
                }
                if shared.rotation.committed() {
                    // Committed (the chain names a staged key, or the hub
                    // closed with 4005): the key in use is retired and never
                    // signs in again. The staged key tries again later.
                    let why = format!("the hub does not take the renewed identity key yet: {error}");
                    tracing::warn!("{why}; trying again later");
                    return Err(why);
                }
                tracing::info!("the hub knows none of the staged identity keys (no renewal committed): signing in with the key in use");
                staged_key = None;
            }
            Err(hub::LoginError::Refused { code, message }) if code == "rotated" => {
                // Not evidence: the hub's word that the key in
                // use was renewed means "read the chain first". It serves the
                // chain up to the renewal on this same socket; only what
                // verifies on this device's own chain counts.
                tracing::info!("the hub says the identity key in use was renewed: reading the chain it serves");
                rotated_sync(shared, &mut sink, &mut source).await;
                if shared.cancel.is_cancelled() {
                    return Err("this device's identity key was replaced elsewhere: not connecting".into());
                }
                let renewed = super::rotation::on_chain(shared) == super::rotation::Verdict::Renewed;
                if renewed || (has_staged && !staged_refused) {
                    // Its own renewal: the staged key signs in next — at
                    // once, unless the hub just turned it away (then after
                    // the back-off).
                    if !staged_refused {
                        shared.rotation.renewed_by_hub();
                    }
                    return Err("the hub says this device's identity key was renewed: signing in with the staged key".into());
                }
                // No renewal to show for it: refused, retried with backoff.
                let why = format!("the hub refuses this device's identity key as renewed, and showed no renewal: {message}");
                tracing::warn!("{why}; trying again later");
                shared.health.lock().expect("health").refused = Some(why.clone());
                let news = {
                    let mut st = shared.state.lock().expect("agent state");
                    let news = st.rotation.refused_ms.is_none();
                    st.rotation.refused_ms = Some(now_ms());
                    news
                };
                if news {
                    shared.save();
                }
                return Err(why);
            }
            Err(hub::LoginError::Removed(message)) => {
                // The hub's word, not the chain's: a failure, retried with
                // backoff and said in the settings — never a removal. Only this
                // machine's verified chain removes it ([`chain_advanced`]).
                let why = format!("the hub refused this device: {message}");
                tracing::warn!("{why}; trying again later");
                shared.health.lock().expect("health").refused = Some(why.clone());
                return Err(why);
            }
            Err(error) => return Err(error.to_string()),
        }
    };
    if staged_key.is_none() {
        // Signed in with the key in use: a hub that refused it as `rotated`
        // before takes it again — and no staging that expired before now was
        // committed, nor should be (set aside, not deleted: R6 review).
        let named = super::rotation::named_keys(&shared.state.lock().expect("agent state"));
        super::rotation::set_aside_expired(&shared.paths, named, &shared.identity, since);
        let mut st = shared.state.lock().expect("agent state");
        if st.rotation.refused_ms.take().is_some() {
            tracing::info!("the hub takes this device's identity key again");
            drop(st);
            shared.save();
        }
    }
    let session = super::rotation::HubSession {
        offered: hub::offers(&welcome, super::ROTATE_FEATURE),
        nonce,
        staged_key: staged_key.is_some(),
        since_ms: since,
    };
    shared.rotation.connected(session);
    // However this connection ends — an error, or the link stopping it
    // mid-way — it is no longer the hub connection.
    let _session = super::rotation::SessionGuard(&shared.rotation);
    let mut features: Vec<&str> = vec![super::CHAIN_V3_FEATURE, super::POLICY_FEATURE, super::ROTATE_FEATURE];
    if shared.relays.lock().expect("relays").is_some() {
        features.push(super::relays::FEATURE);
    }
    hub::send_json(
        &mut sink,
        &json!({"t": "agent_info", "version": crate::api::SERVER_VERSION, "features": features}),
    )
    .await?;
    hub::send_json(&mut sink, &json!({"t": "chain_sub", "from": from})).await?;
    if session.staged_key {
        let signed_in = staged_key.expect("a staged key signed in");
        let result = staged_key_connection(shared, &mut source, &signed_in).await;
        shared.rotation.disconnected();
        let _ = sink.close().await;
        return result;
    }
    tracing::info!(%origin, %machine, rotate = session.offered, "signed in to the hub");
    shared.set_health(true, None);

    // One writer for the socket: frames from every channel, and control text.
    let (frames_tx, frames_rx) = mpsc::unbounded_channel::<Frame>();
    let (text_tx, text_rx) = mpsc::unbounded_channel::<String>();
    let writer = hub::spawn_writer(sink, frames_rx, text_rx, HEARTBEAT, super::traffic::agent_out);

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
                if shared.rotation.take_switch() {
                    break Ok(());
                }
                ask_again_for_held(shared, &text_tx);
                expire_pending(&mut pending, &frames_tx, Instant::now());
                drop_ended(&mut routes);
                continue;
            }
            // The chain renewed this device to its staged key: this
            // connection, signed in with the old one, makes way.
            () = shared.rotation.wake.notified() => {
                if shared.rotation.take_switch() {
                    break Ok(());
                }
                continue;
            }
        };
        let message = match message {
            Some(Ok(m)) => m,
            Some(Err(e)) => break Err(e.to_string()),
            None => break Ok(()),
        };
        last_heard = Instant::now();
        // By the wall clock too: what lets the device role sign in while a
        // staged key is kept ([`super::rotation::device_role_may_sign_in`]).
        shared.rotation.heard();
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
                if code == Some(ROTATED) {
                    // The hub committed a renewal of this device: a staged
                    // key signs in next, and promotes once the chain it then
                    // reads names it (the hub's word alone moves nothing).
                    tracing::info!("the hub says this device's identity key was renewed: signing in with the staged key, if any");
                    shared.rotation.renewed_by_hub();
                }
                break Ok(());
            }
            _ => {}
        }
        if shared.rotation.take_switch() {
            break Ok(());
        }
    };
    shared.rotation.disconnected();
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

/// How long a hub that refused the key in use as `rotated` has to serve the
/// chain on that socket (it keeps it open 10 s).
pub(in crate::agent) const ROTATED_SYNC: Duration = Duration::from_secs(10);

/// A sign-in the hub refused as `rotated`: ask for the chain
/// past this device's head on that same socket and take what verifies —
/// the hub serves it up to the renewal that retired the key and its machine
/// key epoch, then closes. What the entries say is judged by the caller
/// ([`super::rotation::on_chain`]): a hub's word alone changes nothing.
pub(in crate::agent) async fn rotated_sync(shared: &Arc<Shared>, sink: &mut hub::Sink, source: &mut hub::Source) {
    let from = shared.state.lock().expect("agent state").seq().map_or(0, |s| s + 1);
    if hub::send_json(sink, &json!({"t": "chain_sub", "from": from})).await.is_err() {
        return;
    }
    read_chain_until(shared, source, tokio::time::Instant::now() + ROTATED_SYNC, || false).await;
}

/// How a [`read_chain_until`] ended.
pub(in crate::agent) enum ChainRead {
    /// The link was cancelled.
    Cancelled,
    /// The peer closed the connection (or it failed).
    Closed,
    /// The deadline passed.
    Deadline,
    /// `on_chain` asked to stop after a chain frame.
    Stopped,
}

/// Read only `Chain` frames from `source` until `deadline`, the link's
/// cancellation, or the peer closing — acting on each as any chain frame
/// ([`on_frame`]) with frames going nowhere: nothing else happens on this
/// socket. `on_chain` runs after each one and stops the read when it says so.
pub(in crate::agent) async fn read_chain_until(
    shared: &Arc<Shared>,
    source: &mut hub::Source,
    deadline: tokio::time::Instant,
    mut on_chain: impl FnMut() -> bool,
) -> ChainRead {
    // Frames go nowhere: nothing else happens on this socket.
    let (out, _out_rx) = mpsc::unbounded_channel::<Frame>();
    let (text, _text_rx) = mpsc::unbounded_channel::<String>();
    loop {
        let message = tokio::select! {
            m = source.next() => m,
            () = tokio::time::sleep_until(deadline) => return ChainRead::Deadline,
            () = shared.cancel.cancelled() => return ChainRead::Cancelled,
        };
        match message {
            Some(Ok(Message::Binary(bytes))) => {
                if let Ok(frame @ Frame::Chain { .. }) = Frame::decode(&bytes) {
                    on_frame(shared, frame, &mut Routes::new(), &mut Pending::new(), &out, &text);
                    if on_chain() {
                        return ChainRead::Stopped;
                    }
                }
            }
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return ChainRead::Closed,
            Some(Ok(_)) => {}
        }
    }
}

/// A connection signed in with the staged identity key: the hub took it, so
/// it committed a renewal of this device (or lies). It only reads the chain:
/// once its own verified chain names exactly the staged keys, the staged
/// seed is promoted and the link restarted with it
/// ([`super::rotation::promote`]); if the chain does not within
/// [`super::rotation::NEW_KEY_GRACE`], the staged key signs in again after
/// the back-off — never the key in use, which the hub holds as retired.
/// Nothing else — no channel, no stream write, no relay — runs on it: the
/// link still holds the old key.
pub(in crate::agent) async fn staged_key_connection(shared: &Arc<Shared>, source: &mut hub::Source, signed_in: &MachineIdentity) -> Result<(), String> {
    tracing::info!("signed in to the hub with a staged identity key; reading the chain");
    let promoted = || {
        // The chain names exactly the key that signed in.
        let named = {
            let st = shared.state.lock().expect("agent state");
            st.chain
                .as_ref()
                .and_then(|c| c.device_record(&st.machine))
                .is_some_and(|d| (d.info.sign_key, d.info.dh_key) == (signed_in.sign_public(), signed_in.dh_public()))
        };
        named
            && match super::rotation::promote(shared, signed_in) {
                Ok(()) => {
                    // The link restarts with the new key; this one is done.
                    shared.cancel.cancel();
                    true
                }
                Err(error) => {
                    tracing::error!(%error, "cannot take the renewed identity key");
                    false
                }
            }
    };
    if promoted() {
        return Ok(());
    }
    let deadline = tokio::time::Instant::now() + super::rotation::NEW_KEY_GRACE;
    match read_chain_until(shared, source, deadline, || shared.cancel.is_cancelled() || promoted()).await {
        ChainRead::Cancelled | ChainRead::Stopped => return Ok(()),
        ChainRead::Closed | ChainRead::Deadline => {}
    }
    // The hub took the staged key, so it holds a renewal to it: the key in
    // use is not tried — the staged key signs in (and reads the chain) again
    // after the back-off, for as long as it is kept.
    Err("the hub took the staged identity key but the device chain does not name it yet".into())
}

/// Give the streams' writer this connection while this machine runs AI in
/// its account, and take it back once it does not (chain v3 `SetHost false`,
/// review finding 12: a member that stopped running AI writes no streams);
/// it gets it again when the chain makes it a host again. `writing` is
/// whether the writer has it now.
pub(in crate::agent) fn sync_streams(shared: &Shared, writing: &mut bool, frames: &mpsc::UnboundedSender<Frame>, text: &mpsc::UnboundedSender<String>) {
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

pub(in crate::agent) fn on_control(shared: &Arc<Shared>, value: &Value, text: &mpsc::UnboundedSender<String>) {
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
        // Renewing this device's identity key (`docs/host-identity-rotation.md`
        // §3.3): the hub's answer to a staging `POST /rh/rotate` waits for,
        // and its request to confirm a staging with the new key.
        Some("rotation_staged" | "rotation_refused") => {
            if !shared.rotation.answer(value) {
                // Its wait timed out: taken late (kept), or refused late (forgotten).
                super::rotation::late_answer(shared, value);
            }
        }
        Some("rotation_pending") => super::rotation::on_pending(shared, value, text),
        _ => {}
    }
}

