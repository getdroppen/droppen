//! A stand-in agent for testing a hub without a remote-hand server: it
//! enrolls with a code, follows the chain from its pinned root, takes
//! attestations, accepts handshakes exactly as the real agent does
//! ([`droppen_proto::handshake::accept`] with a live clock) and echoes every
//! stream back to its opener.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use rand::rngs::OsRng;
use rand::RngCore;
use droppen_proto::chain::{Attestation, ChainClock, ChainState, Entry, TrustRoot};
use droppen_proto::channel::Session;
use droppen_proto::driver::{Channel, StreamEvent};
use droppen_proto::frame::{Frame, AGENT_PATH};
use droppen_proto::handshake::{self, Accepted, AgentSide, FRESHNESS_MAX_AGE_MS};
use droppen_proto::identity::{EnrollCode, MachineIdentity};
use droppen_proto::mux::Role;
use droppen_proto::noise::{RekeyPolicy, ReplayGuard};
use droppen_proto::sig::{HubLogin, LoginRole};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::{err, hex, key32, now_ms, traffic, HubUrl, Result, CONNECT_PATH};

struct State {
    root: TrustRoot,
    chain: Option<ChainState>,
    clock: ChainClock,
    guard: ReplayGuard,
    attestations: HashMap<String, Attestation>,
    channels: HashMap<u32, mpsc::UnboundedSender<Frame>>,
    pending: HashMap<u32, String>,
}

/// A running echo agent.
pub struct EchoAgent {
    pub account: String,
    pub machine: String,
    /// Its machine identity (tests sign stream heads and manifests with it).
    pub identity: Arc<MachineIdentity>,
    state: Arc<Mutex<State>>,
    /// Handshake outcomes, for tests: `"open"`, `"stale"`, `"need"` or an error.
    pub outcomes: tokio::sync::Mutex<mpsc::UnboundedReceiver<String>>,
    out: mpsc::UnboundedSender<Frame>,
    text: mpsc::UnboundedSender<String>,
    /// What the hub answered stream writes with: `ACK`/`CONFLICT` frames.
    pub stream_replies: tokio::sync::Mutex<mpsc::UnboundedReceiver<Frame>>,
    /// Text control messages (`stream_refused`, `stream_watched`, …).
    pub controls: tokio::sync::Mutex<mpsc::UnboundedReceiver<serde_json::Value>>,
    outcome_tx: mpsc::UnboundedSender<String>,
}

impl EchoAgent {
    /// Enroll with `code` at `hub` and start serving.
    pub async fn enroll(hub: &HubUrl, code: &EnrollCode) -> Result<EchoAgent> {
        Self::enroll_declaring(hub, code, &[]).await
    }

    /// [`EchoAgent::enroll`], declaring `features` in an `agent_info` before
    /// it subscribes to the chain (as a real agent does): `chain/3` and
    /// `policy/1` for an agent that reads chain v3.
    pub async fn enroll_declaring(hub: &HubUrl, code: &EnrollCode, features: &[&str]) -> Result<EchoAgent> {
        let mut seed = [0u8; 32];
        OsRng.fill_bytes(&mut seed);
        let identity = MachineIdentity::from_seed(&seed);
        let (ws, _) = tokio_tungstenite::connect_async(format!("{}{AGENT_PATH}", hub.ws))
            .await
            .map_err(err("connect"))?;
        let (mut sink, mut stream) = ws.split();
        let challenge = crate::next_json(&mut stream, AGENT_PATH).await?;
        let nonce = key32(challenge["nonce"].as_str().ok_or("no nonce")?)?;
        sink.send(Message::Text(
            traffic::sent(AGENT_PATH, json!({"t": "enroll", "token": hex(&code.token())}).to_string()).into(),
        ))
        .await
        .map_err(err("send"))?;
        let assigned = crate::next_json(&mut stream, AGENT_PATH).await?;
        if assigned["t"] != "enroll_assigned" {
            return Err(format!("enrollment refused: {assigned}"));
        }
        let account = assigned["account"].as_str().ok_or("no account")?.to_string();
        let machine = assigned["machine"].as_str().ok_or("no machine")?.to_string();
        let time_ms = now_ms();
        let sig = HubLogin {
            role: LoginRole::Agent,
            account: account.clone(),
            id: machine.clone(),
            hub: hub.origin.clone(),
            nonce,
            time_ms,
        }
        .sign(identity.signing_key())
        .map_err(err("sign"))?;
        let proof = code.proof(&account, &machine, &identity.sign_public(), &identity.dh_public());
        let message = json!({"t": "enroll_proof", "sign_key": hex(&identity.sign_public()), "dh_key": hex(&identity.dh_public()), "time_ms": time_ms, "sig": hex(&sig), "proof": hex(&proof)});
        sink.send(Message::Text(traffic::sent(AGENT_PATH, message.to_string()).into()))
            .await
            .map_err(err("send"))?;
        let welcome = crate::next_json(&mut stream, AGENT_PATH).await?;
        if welcome["t"] != "welcome" {
            return Err(format!("enrollment proof refused: {welcome}"));
        }
        if !features.is_empty() {
            sink.send(Message::Text(
                traffic::sent(
                    AGENT_PATH,
                    json!({"t": "agent_info", "version": "devkit", "features": features}).to_string(),
                )
                .into(),
            ))
            .await
            .map_err(err("send"))?;
        }
        sink.send(Message::Text(
            traffic::sent(AGENT_PATH, json!({"t": "chain_sub", "from": 0}).to_string()).into(),
        ))
        .await
        .map_err(err("send"))?;
        let state = Arc::new(Mutex::new(State {
            root: TrustRoot::from_fingerprint(code.account_fingerprint),
            chain: None,
            clock: ChainClock::live(now_ms(), 0),
            guard: ReplayGuard::after_restart(now_ms().saturating_sub(60_000)),
            attestations: HashMap::new(),
            channels: HashMap::new(),
            pending: HashMap::new(),
        }));
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Frame>();
        let (text_tx, mut text_rx) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            loop {
                let message = tokio::select! {
                    frame = out_rx.recv() => match frame {
                        Some(frame) => match frame.encode() {
                            Ok(bytes) => Message::Binary(bytes.into()),
                            Err(_) => continue,
                        },
                        None => break,
                    },
                    text = text_rx.recv() => match text {
                        Some(text) => Message::Text(traffic::sent(AGENT_PATH, text).into()),
                        None => break,
                    },
                };
                if sink.send(message).await.is_err() {
                    break;
                }
            }
        });
        let (outcome_tx, outcome_rx) = mpsc::unbounded_channel();
        let outcomes_for_relays = outcome_tx.clone();
        let (reply_tx, reply_rx) = mpsc::unbounded_channel();
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let identity = Arc::new(identity);
        let out = out_tx.clone();
        {
            let state = state.clone();
            let machine = machine.clone();
            let identity = identity.clone();
            tokio::spawn(async move {
                while let Some(Ok(message)) = stream.next().await {
                    let bytes = match message {
                        Message::Binary(bytes) => bytes,
                        Message::Text(text) => {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(traffic::got(AGENT_PATH, text.as_str())) {
                                let _ = control_tx.send(v);
                            }
                            continue;
                        }
                        _ => continue,
                    };
                    let Ok(frame) = Frame::decode(&bytes) else { continue };
                    if matches!(frame, Frame::Ack { .. } | Frame::Conflict { .. }) {
                        let _ = reply_tx.send(frame);
                        continue;
                    }
                    on_frame(&state, &identity, &machine, frame, &out_tx, &outcome_tx);
                }
            });
        }
        Ok(EchoAgent {
            account,
            machine,
            identity,
            state,
            outcomes: tokio::sync::Mutex::new(outcome_rx),
            out,
            text: text_tx,
            stream_replies: tokio::sync::Mutex::new(reply_rx),
            controls: tokio::sync::Mutex::new(control_rx),
            outcome_tx: outcomes_for_relays,
        })
    }

    /// This agent's one identity as a device's keys (chain v3: a host
    /// controls other hosts as a device, with the same keys).
    pub fn device_keys(&self) -> (ed25519_dalek::SigningKey, droppen_proto::keys::DhKeypair) {
        (
            self.identity.signing_key().clone(),
            droppen_proto::keys::DhKeypair::from_secret(*self.identity.dh().secret()),
        )
    }

    /// The seq of the chain this agent follows (`None` before any).
    pub fn chain_seq(&self) -> Option<u64> {
        self.state.lock().expect("state").chain.as_ref().map(|c| c.seq())
    }

    /// Wait until the chain this agent follows names it.
    pub async fn wait_in_chain(&self, timeout: Duration) -> Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self
                .state
                .lock()
                .expect("state")
                .chain
                .as_ref()
                .is_some_and(|c| c.machine(&self.machine).is_some())
            {
                return Ok(());
            }
            if tokio::time::Instant::now() > deadline {
                return Err("the machine never appeared in the chain".into());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Send a frame as this machine (stream writes, in tests).
    pub fn send(&self, frame: Frame) -> Result<()> {
        self.out.send(frame).map_err(|_| "the agent's connection is closed".to_string())
    }

    /// Send a control message as this machine (`stream_delete`, in tests).
    pub fn send_control(&self, value: &serde_json::Value) -> Result<()> {
        self.text
            .send(value.to_string())
            .map_err(|_| "the agent's connection is closed".to_string())
    }

    /// The hub's next `ACK`/`CONFLICT`.
    pub async fn next_reply(&self, timeout: Duration) -> Option<Frame> {
        tokio::time::timeout(timeout, self.stream_replies.lock().await.recv())
            .await
            .ok()
            .flatten()
    }

    /// The next control message matching `pred` (others are dropped).
    pub async fn wait_control(&self, timeout: Duration, pred: impl Fn(&serde_json::Value) -> bool) -> Option<serde_json::Value> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut rx = self.controls.lock().await;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(left, rx.recv()).await {
                Ok(Some(v)) if pred(&v) => return Some(v),
                Ok(Some(_)) => continue,
                _ => return None,
            }
        }
    }

    /// Ask the hub for a ticket for `relay` (a member of the account's set)
    /// and sign in there as this machine: channels from devices then arrive
    /// over the relay and are answered exactly as before.
    pub async fn attach_relay(&self, relay: &str, timeout: Duration) -> Result<()> {
        self.send_control(&json!({"t": "relay_tickets", "relays": [relay]}))?;
        let answer = self
            .wait_control(timeout, |v| v["t"] == "relay_tickets")
            .await
            .ok_or("the hub did not answer relay_tickets")?;
        let t = answer["tickets"]
            .as_array()
            .and_then(|a| a.iter().find(|t| t["relay"] == relay))
            .ok_or_else(|| format!("no ticket for {relay}: {answer}"))?;
        let (origin, ticket) = (
            t["origin"].as_str().ok_or("no origin")?.to_string(),
            t["ticket"].as_str().ok_or("no ticket")?.to_string(),
        );
        let (ws, _) = tokio_tungstenite::connect_async(format!("{origin}{CONNECT_PATH}"))
            .await
            .map_err(err("connect"))?;
        let (mut sink, mut stream) = ws.split();
        let challenge = crate::next_json(&mut stream, CONNECT_PATH).await?;
        let nonce = key32(challenge["nonce"].as_str().ok_or("no nonce")?)?;
        let time_ms = now_ms();
        let sig = HubLogin {
            role: LoginRole::RelayAgent,
            account: self.account.clone(),
            id: self.machine.clone(),
            hub: origin.clone(),
            nonce,
            time_ms,
        }
        .sign(self.identity.signing_key())
        .map_err(err("sign"))?;
        sink.send(Message::Text(
            traffic::sent(
                CONNECT_PATH,
                json!({"t": "login", "ticket": ticket, "time_ms": time_ms, "sig": hex(&sig)}).to_string(),
            )
            .into(),
        ))
        .await
        .map_err(err("send"))?;
        let welcome = crate::next_json(&mut stream, CONNECT_PATH).await?;
        if welcome["t"] != "welcome" {
            return Err(format!("the relay refused the agent: {welcome}"));
        }
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Frame>();
        tokio::spawn(async move {
            while let Some(frame) = out_rx.recv().await {
                let Ok(bytes) = frame.encode() else { continue };
                if sink.send(Message::Binary(bytes.into())).await.is_err() {
                    break;
                }
            }
        });
        let state = self.state.clone();
        let identity = self.identity.clone();
        let machine = self.machine.clone();
        let outcomes = self.outcome_tx.clone();
        tokio::spawn(async move {
            while let Some(Ok(message)) = stream.next().await {
                let Message::Binary(bytes) = message else { continue };
                let Ok(frame) = Frame::decode(&bytes) else { continue };
                on_frame(&state, &identity, &machine, frame, &out_tx, &outcomes);
            }
        });
        Ok(())
    }

    pub fn attestations(&self) -> usize {
        self.state.lock().expect("state").attestations.len()
    }

    pub async fn next_outcome(&self, timeout: Duration) -> Option<String> {
        tokio::time::timeout(timeout, self.outcomes.lock().await.recv()).await.ok().flatten()
    }
}

fn on_frame(
    state: &Arc<Mutex<State>>,
    identity: &MachineIdentity,
    machine: &str,
    frame: Frame,
    out: &mpsc::UnboundedSender<Frame>,
    outcomes: &mpsc::UnboundedSender<String>,
) {
    let mut st = state.lock().expect("state");
    match frame {
        Frame::Chain { entries, .. } => {
            let Ok(entries) = entries.iter().map(|e| Entry::decode(e)).collect::<std::result::Result<Vec<_>, _>>() else {
                return;
            };
            let st = &mut *st;
            st.clock.set_now(now_ms());
            match &mut st.chain {
                None => {
                    if let Ok(chain) = ChainState::verify_rooted(&entries, &st.root, &mut st.clock) {
                        st.root = st.root.pinned_to(&chain);
                        st.chain = Some(chain);
                    }
                }
                Some(chain) => {
                    let _ = chain.merge(&entries, &mut st.clock);
                }
            }
        }
        Frame::Attest { attestation, .. } => {
            if let Ok(att) = Attestation::decode(&attestation) {
                st.attestations.insert(att.device.clone(), att);
            }
        }
        Frame::Open { ch, peer } => {
            st.pending.insert(ch, peer);
        }
        Frame::Data { ch, payload } => {
            if let Some(route) = st.channels.get(&ch) {
                let _ = route.send(Frame::Data { ch, payload });
                return;
            }
            let Some(device) = st.pending.remove(&ch) else { return };
            let st = &mut *st;
            let Some(chain) = st.chain.as_mut() else { return };
            let attestations: Vec<Attestation> = st.attestations.values().cloned().collect();
            let side = AgentSide {
                chain,
                clock: &mut st.clock,
                guard: &mut st.guard,
                key: identity.dh(),
                machine,
                policy: RekeyPolicy::default(),
                server_info: b"droppen-devkit echo agent",
                attestations: &attestations,
                freshness_max_age_ms: FRESHNESS_MAX_AGE_MS,
            };
            match handshake::accept(side, &device, &payload, now_ms()) {
                Ok(Accepted::Open { reply, transport, .. }) => {
                    let _ = out.send(Frame::Data { ch, payload: reply });
                    let (tx, rx) = mpsc::unbounded_channel();
                    st.channels.insert(ch, tx);
                    let (_channel, mut incoming) = Channel::spawn(Session::new(ch, *transport, Role::Responder), rx, out.clone());
                    tokio::spawn(async move {
                        while let Some(inc) = incoming.recv().await {
                            let mut stream = inc.stream;
                            tokio::spawn(async move {
                                while let Some(StreamEvent::Data(d)) = stream.recv().await {
                                    if stream.send(&d).await.is_err() {
                                        break;
                                    }
                                }
                            });
                        }
                    });
                    let _ = outcomes.send("open".into());
                }
                Ok(Accepted::Stale { reply, .. }) => {
                    let _ = out.send(Frame::Data { ch, payload: reply });
                    let _ = out.send(Frame::Close {
                        ch,
                        code: 4403,
                        reason: "stale".into(),
                    });
                    let _ = outcomes.send("stale".into());
                }
                Ok(Accepted::NeedEntries { reply, .. }) => {
                    let _ = out.send(Frame::Data { ch, payload: reply });
                    let _ = out.send(Frame::Close {
                        ch,
                        code: 4409,
                        reason: "need entries".into(),
                    });
                    let _ = outcomes.send("need".into());
                }
                Err(e) => {
                    let _ = out.send(Frame::Close {
                        ch,
                        code: 4401,
                        reason: "refused".into(),
                    });
                    let _ = outcomes.send(format!("error: {e}"));
                }
            }
        }
        Frame::Credit { ch, bytes } => {
            if let Some(route) = st.channels.get(&ch) {
                let _ = route.send(Frame::Credit { ch, bytes });
            }
        }
        Frame::Close { ch, code, reason } => {
            if let Some(route) = st.channels.remove(&ch) {
                let _ = route.send(Frame::Close { ch, code, reason });
            }
            st.pending.remove(&ch);
        }
        _ => {}
    }
}
