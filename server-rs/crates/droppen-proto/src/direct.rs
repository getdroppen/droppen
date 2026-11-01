//! Direct-path attempts over a channel's signaling stream (feature `rtc`):
//! the agent's half ([`serve`]) and a device's half ([`Signaling`],
//! [`attempt`]). The browser's half is `web/src/lib/transport/p2p.ts`.
//!
//! One attempt, in order:
//!
//! 1. The device opens a `mux` sub-stream to [`SIGNAL_PATH`] on the channel it
//!    already has; the agent answers like a websocket (a bridge head, status
//!    101), or refuses (status 403: direct paths are off here). Each signal is
//!    one bridge binary message. The hub relays it all as ciphertext.
//! 2. The device sends [`Signal::Offer`] with its own DTLS fingerprint; the
//!    agent refuses an offer whose SDP names another certificate
//!    ([`check_fingerprint`]), answers with its own fingerprint and a fresh
//!    nonce, and both trickle candidates. The device checks the answer the
//!    same way.
//! 3. When the DataChannel opens, the device sends the direct handshake's
//!    first message ([`DcMessage::Hello`], bound by [`DirectBinding`] to the
//!    channel, the nonce and both fingerprints); the agent admits it exactly as
//!    it admits a channel ([`Admit`], `handshake::accept_direct`) and replies.
//! 4. Both attach the direct transport to the channel's driver
//!    ([`crate::driver::Channel::attach_direct`]). From then on the driver
//!    owns the DataChannel's messages; when it lets the path go the peer
//!    connection is torn down and its sockets closed.
//!
//! An attempt that is not attached within [`ATTEMPT_TIMEOUT`] fails; either
//! side says why with [`Signal::Bye`]. The device retries with backoff.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rand_core::{OsRng, RngCore};
use tokio::sync::{mpsc, Semaphore};

use crate::bridge::{self, Kind, MessageReader, RequestHead, ResponseHead};
use crate::chain::{ChainClock, ChainState};
use crate::clock::now_ms;
use crate::driver::{Channel, DirectPipe, Stream, StreamEvent, StreamWriter};
use crate::handshake::{self, HandshakeError};
use crate::keys::DhKeypair;
use crate::mux::StreamClass;
use crate::noise::{Binding, RekeyPolicy, Transport};
use crate::path::{check_fingerprint, ByeReason, DcMessage, DirectBinding, Signal, SIGNAL_PATH};
use crate::rtc::{Peer, PeerEvent, RtcOptions};

/// How long an attempt may take from offer to attached.
pub const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(15);

/// What the agent decided about a direct hello.
pub enum Admission {
    /// Send `reply` and attach `transport`.
    Open { reply: Vec<u8>, transport: Box<Transport> },
    /// Not admitted: send `reply` if there is one (a stale view, a missing
    /// entry — the device learns why), then end the attempt.
    Refused { reply: Option<Vec<u8>>, why: String },
}

/// The agent's admission of direct hellos: `handshake::accept_direct` with
/// its own live chain, clock, replay guard and attestations — the rules every
/// channel is admitted by.
pub trait Admit: Send + Sync {
    fn admit(&self, device: &str, bind: &DirectBinding, hello: &[u8]) -> Admission;
}

/// The agent's side of one channel's direct paths.
pub struct AgentDirect {
    pub admit: Arc<dyn Admit>,
    /// [`ATTEMPT_TIMEOUT`] unless a test says otherwise.
    pub timeout: Duration,
    pub opts: RtcOptions,
    /// Peers at once, across every channel (each holds sockets).
    pub peers: Arc<Semaphore>,
    /// The channel's `account | device | machine`.
    pub binding: Binding,
    /// The channel's handshake hash.
    pub relay_hash: [u8; 32],
}

async fn send_signal(writer: &StreamWriter, signal: &Signal) -> bool {
    match signal.encode() {
        Ok(bytes) => writer.send(&bridge::message(Kind::Binary, &bytes)).await.is_ok(),
        Err(_) => false,
    }
}

/// Serve one device's signaling stream (`stream`, already checked to be a
/// `mux` stream to [`SIGNAL_PATH`]) for `channel`: answer its offers, one
/// attempt at a time (a new offer replaces an attempt still in progress; a
/// direct path already attached stays until the new one replaces it).
pub async fn serve(mut stream: Stream, channel: Channel, ctx: Arc<AgentDirect>) {
    let writer = stream.writer();
    let open = ResponseHead {
        status: 101,
        headers: Vec::new(),
    };
    let Ok(head) = open.encode() else { return };
    if writer.send(&bridge::message(Kind::Head, &head)).await.is_err() {
        return;
    }
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Signal>();
    let mut reader = MessageReader::new();
    let mut current: Option<(u32, mpsc::UnboundedSender<Signal>)> = None;
    loop {
        tokio::select! {
            event = stream.recv() => match event {
                Some(StreamEvent::Data(data)) => {
                    reader.push(&data);
                    loop {
                        let signal = match reader.next_message() {
                            Ok(Some((Kind::Binary, payload))) => match Signal::decode(&payload) {
                                Ok(s) => s,
                                Err(_) => return stream.close(bridge::CLOSE_BAD_REQUEST),
                            },
                            Ok(Some(_)) => continue,
                            Ok(None) => break,
                            Err(_) => return stream.close(bridge::CLOSE_BAD_REQUEST),
                        };
                        match signal {
                            Signal::Offer { id, fp, sdp } => {
                                let (tx, rx) = mpsc::unbounded_channel();
                                current = Some((id, tx));
                                tokio::spawn(agent_attempt(id, fp, sdp, ctx.clone(), channel.clone(), out_tx.clone(), rx));
                            }
                            other => {
                                if let Some((id, tx)) = &current {
                                    if *id == other.id() {
                                        let _ = tx.send(other);
                                    }
                                }
                            }
                        }
                    }
                }
                Some(StreamEvent::Closed(_)) | None => return,
            },
            Some(signal) = out_rx.recv() => {
                if !send_signal(&writer, &signal).await {
                    return;
                }
            }
        }
    }
}

async fn agent_attempt(
    id: u32,
    device_fp: [u8; 32],
    sdp: String,
    ctx: Arc<AgentDirect>,
    channel: Channel,
    out: mpsc::UnboundedSender<Signal>,
    mut inbox: mpsc::UnboundedReceiver<Signal>,
) {
    let bye = |reason: ByeReason, detail: &str| {
        let _ = out.send(Signal::Bye {
            id,
            reason,
            detail: detail.chars().take(200).collect(),
        });
    };
    if check_fingerprint(&sdp, &device_fp).is_err() {
        tracing::warn!(device = %ctx.binding.device, "a direct offer whose SDP names another certificate than the one stated; refused");
        return bye(ByeReason::BadFingerprint, "the offer's fingerprint is not the one stated");
    }
    let Ok(_permit) = ctx.peers.clone().try_acquire_owned() else {
        return bye(ByeReason::Busy, "too many direct paths at once");
    };
    let (mut peer, answer) = match Peer::answer(&sdp, &ctx.opts).await {
        Ok(p) => p,
        Err(e) => return bye(ByeReason::Failed, &e.to_string()),
    };
    let mut nonce = [0u8; 32];
    OsRng.fill_bytes(&mut nonce);
    let _ = out.send(Signal::Answer {
        id,
        fp: peer.local_fp,
        nonce,
        sdp: answer,
    });
    let bind = DirectBinding {
        binding: ctx.binding.clone(),
        relay_hash: ctx.relay_hash,
        nonce,
        device_fp,
        agent_fp: peer.local_fp,
    };
    let started = Instant::now();
    let deadline = tokio::time::sleep(ctx.timeout);
    tokio::pin!(deadline);
    let mut pair = None;
    let hello = loop {
        tokio::select! {
            () = &mut deadline => return bye(ByeReason::Failed, "timed out"),
            signal = inbox.recv() => match signal {
                Some(Signal::Candidate { candidate: Some(c), .. }) => peer.control.remote_candidate(&c),
                Some(Signal::Bye { .. }) | None => return,
                Some(_) => {}
            },
            Some(event) = peer.events.recv() => match event {
                PeerEvent::Candidate(c) => {
                    let _ = out.send(Signal::Candidate { id, candidate: c });
                }
                PeerEvent::Open { pair: p } => pair = p,
                PeerEvent::Closed(why) => return bye(ByeReason::Failed, &why),
            },
            message = peer.messages.recv() => match message.as_deref().map(DcMessage::decode) {
                Some(Ok(DcMessage::Hello(hello))) => break hello,
                _ => return bye(ByeReason::Failed, "expected the direct handshake"),
            },
        }
    };
    match ctx.admit.admit(&ctx.binding.device, &bind, &hello) {
        Admission::Open { reply, transport } => {
            let reply = DcMessage::Reply(reply).encode();
            peer.queued.fetch_add(reply.len(), Ordering::Relaxed);
            let _ = peer.outgoing.send(reply);
            let pipe = DirectPipe {
                tx: peer.outgoing,
                rx: peer.messages,
                queued: peer.queued,
            };
            if channel.attach_direct(*transport, pipe).is_err() {
                return;
            }
            tracing::info!(device = %ctx.binding.device, ms = started.elapsed().as_millis() as u64, ?pair, "a direct path carries the channel");
            // Hold the permit for as long as the peer lives; late candidates still go out.
            let mut inbox_open = true;
            loop {
                tokio::select! {
                    event = peer.events.recv() => match event {
                        Some(PeerEvent::Candidate(c)) => {
                            let _ = out.send(Signal::Candidate { id, candidate: c });
                        }
                        Some(PeerEvent::Closed(why)) => {
                            tracing::info!(device = %ctx.binding.device, %why, "the direct path closed");
                            return;
                        }
                        None => return,
                        Some(PeerEvent::Open { .. }) => {}
                    },
                    signal = inbox.recv(), if inbox_open => match signal {
                        Some(Signal::Candidate { candidate: Some(c), .. }) => peer.control.remote_candidate(&c),
                        Some(Signal::Bye { .. }) => peer.control.close(),
                        Some(_) => {}
                        // A newer attempt took over the signaling: this one
                        // lives until the driver lets its pipe go.
                        None => inbox_open = false,
                    },
                }
            }
        }
        Admission::Refused { reply, why } => {
            tracing::info!(device = %ctx.binding.device, %why, "refused a direct path");
            if let Some(reply) = reply {
                let _ = peer.outgoing.send(DcMessage::Reply(reply).encode());
                // Let it reach the wire before the peer goes.
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            bye(ByeReason::Refused, &why);
        }
    }
}

/// A device's end of a channel's signaling stream.
pub struct Signaling {
    writer: StreamWriter,
    incoming: mpsc::UnboundedReceiver<Signal>,
}

impl Signaling {
    /// Open the signaling stream on `channel`. An error means the agent does
    /// not do direct paths on it (turned off, or too old).
    pub async fn open(channel: &Channel) -> Result<Signaling, String> {
        let head = RequestHead::get(SIGNAL_PATH).encode().map_err(|e| e.to_string())?;
        let mut stream = channel.open(StreamClass::Mux, &head).await.map_err(|e| e.to_string())?;
        let mut reader = MessageReader::new();
        // The head first.
        let status = loop {
            if let Some((kind, payload)) = reader.next_message().map_err(|e| e.to_string())? {
                if kind == Kind::Head {
                    break ResponseHead::decode(&payload).map_err(|e| e.to_string())?.status;
                }
                continue;
            }
            match tokio::time::timeout(Duration::from_secs(10), stream.recv()).await {
                Ok(Some(StreamEvent::Data(d))) => reader.push(&d),
                Ok(Some(StreamEvent::Closed(_))) | Ok(None) => return Err("the agent closed the signaling stream".into()),
                Err(_) => return Err("the agent did not answer on the signaling stream".into()),
            }
        };
        if status != 101 {
            return Err(format!("the agent refused direct paths ({status})"));
        }
        let writer = stream.writer();
        let (tx, incoming) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                while let Ok(Some((kind, payload))) = reader.next_message() {
                    if kind == Kind::Binary {
                        if let Ok(signal) = Signal::decode(&payload) {
                            if tx.send(signal).is_err() {
                                return;
                            }
                        }
                    }
                }
                match stream.recv().await {
                    Some(StreamEvent::Data(d)) => reader.push(&d),
                    _ => return,
                }
            }
        });
        Ok(Signaling { writer, incoming })
    }

    pub async fn send(&self, signal: &Signal) -> Result<(), String> {
        if send_signal(&self.writer, signal).await {
            Ok(())
        } else {
            Err("the signaling stream closed".into())
        }
    }
}

/// What a device brings to an attempt.
pub struct DeviceDirect<'a> {
    pub chain: &'a ChainState,
    pub clock: &'a mut ChainClock,
    pub key: &'a DhKeypair,
    /// The channel's `account | device | machine`.
    pub binding: Binding,
    /// The channel's handshake hash.
    pub relay_hash: [u8; 32],
    /// The hello's client info.
    pub client: &'a [u8],
    pub opts: RtcOptions,
    /// [`ATTEMPT_TIMEOUT`] unless a test says otherwise.
    pub timeout: Duration,
}

/// A direct path that carries a channel, from the device's side.
pub struct Attached {
    /// The peer's events after attaching: `Closed` when it goes.
    pub events: mpsc::UnboundedReceiver<PeerEvent>,
    pub control: crate::rtc::PeerControl,
    /// How long the attempt took, offer to attached.
    pub took: Duration,
    /// The (local, remote) addresses the DataChannel opened on, if known.
    pub pair: Option<(std::net::SocketAddr, std::net::SocketAddr)>,
}

/// Why an attempt failed.
#[derive(Debug)]
pub enum AttemptError {
    /// The agent said goodbye.
    Bye(ByeReason, String),
    /// Anything else (timeout, ICE, the handshake…).
    Failed(String),
}

impl std::fmt::Display for AttemptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttemptError::Bye(reason, detail) => write!(f, "the agent ended the attempt ({reason:?}: {detail})"),
            AttemptError::Failed(why) => f.write_str(why),
        }
    }
}

/// One attempt, attempt id `id`, over `sig` for `channel`.
pub async fn attempt(sig: &mut Signaling, id: u32, channel: &Channel, ctx: DeviceDirect<'_>) -> Result<Attached, AttemptError> {
    let started = Instant::now();
    let fail = |why: String| AttemptError::Failed(why);
    let (mut peer, offer) = Peer::offer(&ctx.opts).await.map_err(|e| fail(e.to_string()))?;
    sig.send(&Signal::Offer {
        id,
        fp: peer.local_fp,
        sdp: offer,
    })
    .await
    .map_err(fail)?;
    let result = tokio::time::timeout(ctx.timeout, async {
        let mut bind: Option<DirectBinding> = None;
        let mut initiator = None;
        let mut pair = None;
        loop {
            tokio::select! {
                signal = sig.incoming.recv() => match signal {
                    Some(Signal::Answer { id: got, fp, nonce, sdp }) if got == id => {
                        if check_fingerprint(&sdp, &fp).is_err() {
                            let _ = sig.send(&Signal::Bye { id, reason: ByeReason::BadFingerprint, detail: "the answer's fingerprint is not the one stated".into() }).await;
                            return Err(fail("the answer's SDP names another certificate than the one stated".into()));
                        }
                        peer.control.answer(&sdp);
                        bind = Some(DirectBinding {
                            binding: ctx.binding.clone(),
                            relay_hash: ctx.relay_hash,
                            nonce,
                            device_fp: peer.local_fp,
                            agent_fp: fp,
                        });
                    }
                    Some(Signal::Candidate { id: got, candidate: Some(c) }) if got == id => peer.control.remote_candidate(&c),
                    Some(Signal::Bye { id: got, reason, detail }) if got == id => return Err(AttemptError::Bye(reason, detail)),
                    Some(_) => {}
                    None => return Err(fail("the signaling stream closed".into())),
                },
                Some(event) = peer.events.recv() => match event {
                    PeerEvent::Candidate(c) => {
                        sig.send(&Signal::Candidate { id, candidate: c }).await.map_err(fail)?;
                    }
                    PeerEvent::Open { pair: p } => {
                        pair = p;
                        let bind = bind.as_ref().ok_or_else(|| fail("open before the answer".into()))?;
                        let (i, hello) = handshake::start_direct(ctx.chain, ctx.key, bind, now_ms(), ctx.client).map_err(|e| fail(e.to_string()))?;
                        initiator = Some(i);
                        let hello = DcMessage::Hello(hello).encode();
                        peer.queued.fetch_add(hello.len(), Ordering::Relaxed);
                        let _ = peer.outgoing.send(hello);
                    }
                    PeerEvent::Closed(why) => return Err(fail(why)),
                },
                message = peer.messages.recv() => {
                    let Some(Ok(DcMessage::Reply(reply))) = message.as_deref().map(DcMessage::decode) else {
                        return Err(fail("expected the direct handshake's reply".into()));
                    };
                    let i = initiator.take().ok_or_else(|| fail("a reply before the hello".into()))?;
                    let mut view = ctx.chain.clone();
                    let (transport, _) = handshake::finish(i, &mut view, ctx.clock, &reply, RekeyPolicy::default(), now_ms()).map_err(|e: HandshakeError| fail(e.to_string()))?;
                    return Ok((transport, pair));
                }
            }
        }
    })
    .await;
    let (transport, pair) = match result {
        Ok(Ok(done)) => done,
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            let _ = sig
                .send(&Signal::Bye {
                    id,
                    reason: ByeReason::Failed,
                    detail: "timed out".into(),
                })
                .await;
            return Err(fail("timed out".into()));
        }
    };
    let pipe = DirectPipe {
        tx: peer.outgoing,
        rx: peer.messages,
        queued: peer.queued,
    };
    channel.attach_direct(transport, pipe).map_err(|e| fail(e.to_string()))?;
    Ok(Attached {
        events: peer.events,
        control: peer.control,
        took: started.elapsed(),
        pair,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::tests::{fixture, h, sample_chain, Fixture};
    use crate::chain::Attestation;
    use crate::channel::{PathOptions, Session};
    use crate::driver::Incoming;
    use crate::frame::Frame;
    use crate::handshake::{Accepted, AgentSide, FRESHNESS_MAX_AGE_MS};
    use crate::mux::Role;
    use crate::noise::ReplayGuard;
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;

    /// The agent's admission, as `agent::link` does it: its chain, live
    /// clock, guard and attestations, under one lock.
    struct TestAdmit {
        state: Mutex<(ChainState, ChainClock, ReplayGuard, Vec<Attestation>)>,
        key: DhKeypair,
    }

    impl Admit for TestAdmit {
        fn admit(&self, device: &str, bind: &DirectBinding, hello: &[u8]) -> Admission {
            let mut st = self.state.lock().unwrap();
            let (chain, clock, guard, attestations) = &mut *st;
            let side = AgentSide {
                chain,
                clock,
                guard,
                key: &self.key,
                machine: "m-1",
                policy: RekeyPolicy::default(),
                server_info: b"{}",
                attestations,
                freshness_max_age_ms: FRESHNESS_MAX_AGE_MS,
            };
            match handshake::accept_direct(side, device, bind, hello, now_ms()) {
                Ok(Accepted::Open { reply, transport, .. }) => Admission::Open { reply, transport },
                Ok(Accepted::Stale { reply, .. }) => Admission::Refused {
                    reply: Some(reply),
                    why: "stale".into(),
                },
                Ok(Accepted::NeedEntries { reply, .. }) => Admission::Refused {
                    reply: Some(reply),
                    why: "behind".into(),
                },
                Err(e) => Admission::Refused {
                    reply: None,
                    why: e.to_string(),
                },
            }
        }
    }

    struct World {
        f: Fixture,
        chain: ChainState,
        device: Channel,
        agent: Channel,
        relay_hash: [u8; 32],
        admit: Arc<TestAdmit>,
        /// Drop to close the agent's side of the hub channel (what
        /// `agent::link::drop_removed` does).
        agent_inbound: Option<mpsc::UnboundedSender<Frame>>,
    }

    fn binding(chain: &ChainState) -> Binding {
        Binding {
            account: chain.account().into(),
            device: "dev-0".into(),
            machine: "m-1".into(),
        }
    }

    /// A hub channel with paths between dev-0 and the agent, through an
    /// in-process "hub" that adds 20 ms each way; the agent echoes terminal
    /// bytes and serves the signaling stream.
    async fn world(opts: RtcOptions) -> World {
        let f = fixture();
        let chain = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
        let attest = vec![Attestation::create(&chain, "dev-0", &f.devices[0].0, now_ms())];
        let admit = Arc::new(TestAdmit {
            state: Mutex::new((
                chain.clone(),
                ChainClock::live(now_ms(), chain.seq() + 1),
                ReplayGuard::new(600_000, 60_000, 64, 0),
                attest,
            )),
            key: DhKeypair::from_secret(*f.machine.1.secret()),
        });
        // The hub channel's handshake, as `link` does it.
        let (initiator, hello) = handshake::start(&chain, &f.devices[0].1, "dev-0", "m-1", now_ms(), b"rh-devkit paths/1").unwrap();
        let (reply, agent_t) = {
            let mut st = admit.state.lock().unwrap();
            let (c, clock, guard, attestations) = &mut *st;
            let side = AgentSide {
                chain: c,
                clock,
                guard,
                key: &admit.key,
                machine: "m-1",
                policy: RekeyPolicy::default(),
                server_info: br#"{"features":["paths/1","direct/1"]}"#,
                attestations,
                freshness_max_age_ms: FRESHNESS_MAX_AGE_MS,
            };
            let Accepted::Open { reply, transport, .. } = handshake::accept(side, "dev-0", &hello, now_ms()).unwrap() else {
                panic!("open")
            };
            (reply, transport)
        };
        let (device_t, _) = handshake::finish(
            initiator,
            &mut chain.clone(),
            &mut ChainClock::live(now_ms(), 0),
            &reply,
            RekeyPolicy::default(),
            now_ms(),
        )
        .unwrap();
        let relay_hash = device_t.handshake_hash();
        let now = now_ms() * 1000;
        let d_session = Session::with_paths(7, device_t, Role::Initiator, PathOptions { probe_relay: true }, now);
        let a_session = Session::with_paths(7, *agent_t, Role::Responder, PathOptions::default(), now);
        let (d_out, d_out_rx) = mpsc::unbounded_channel();
        let (a_out, a_out_rx) = mpsc::unbounded_channel();
        let (d_in, d_in_rx) = mpsc::unbounded_channel();
        let (a_in, a_in_rx) = mpsc::unbounded_channel();
        let a_in_keep = a_in.clone();
        for (mut from, to) in [(d_out_rx, a_in), (a_out_rx, d_in)] {
            tokio::spawn(async move {
                while let Some(frame) = from.recv().await {
                    let to = to.clone();
                    // 20 ms on the hub, in order (each frame's delay overlaps the next's).
                    let at = tokio::time::Instant::now() + Duration::from_millis(20);
                    tokio::time::sleep_until(at).await;
                    if to.send(frame).is_err() {
                        break;
                    }
                }
            });
        }
        let (device, _d_acc) = Channel::spawn(d_session, d_in_rx, d_out);
        let (agent, mut a_acc) = Channel::spawn(a_session, a_in_rx, a_out);
        let ctx = Arc::new(AgentDirect {
            admit: admit.clone(),
            timeout: Duration::from_secs(5),
            opts,
            peers: Arc::new(Semaphore::new(4)),
            binding: binding(&chain),
            relay_hash,
        });
        let agent_handle = agent.clone();
        tokio::spawn(async move {
            while let Some(Incoming { class, meta, mut stream }) = a_acc.recv().await {
                let head = RequestHead::decode(&meta).unwrap();
                if class == StreamClass::Mux && head.path == SIGNAL_PATH {
                    tokio::spawn(serve(stream, agent_handle.clone(), ctx.clone()));
                } else {
                    tokio::spawn(async move {
                        while let Some(StreamEvent::Data(d)) = stream.recv().await {
                            if stream.send(&d).await.is_err() {
                                break;
                            }
                        }
                    });
                }
            }
        });
        World {
            f,
            chain,
            device,
            agent,
            relay_hash,
            admit,
            agent_inbound: Some(a_in_keep),
        }
    }

    fn loopback(blackhole: Option<Arc<AtomicBool>>) -> RtcOptions {
        RtcOptions {
            stun: Vec::new(),
            default_route: false,
            loopback: true,
            blackhole,
        }
    }

    async fn try_direct(w: &World, sig: &mut Signaling, id: u32, opts: RtcOptions, timeout: Duration) -> Result<Attached, AttemptError> {
        let mut clock = ChainClock::live(now_ms(), w.chain.seq() + 1);
        let key = DhKeypair::from_secret(*w.f.devices[0].1.secret());
        attempt(
            sig,
            id,
            &w.device,
            DeviceDirect {
                chain: &w.chain,
                clock: &mut clock,
                key: &key,
                binding: binding(&w.chain),
                relay_hash: w.relay_hash,
                client: b"rh-devkit paths/1",
                opts,
                timeout,
            },
        )
        .await
    }

    async fn type_and_collect(term: &mut Stream, keys: std::ops::Range<u8>, echoed: &mut Vec<u8>) {
        for k in keys {
            term.send(&[k]).await.unwrap();
            tokio::time::sleep(Duration::from_millis(3)).await;
            while let Ok(Some(StreamEvent::Data(d))) = tokio::time::timeout(Duration::from_millis(1), term.recv()).await {
                echoed.extend(d);
            }
        }
    }

    async fn collect_until(term: &mut Stream, n: usize, echoed: &mut Vec<u8>) {
        tokio::time::timeout(Duration::from_secs(15), async {
            while echoed.len() < n {
                match term.recv().await {
                    Some(StreamEvent::Data(d)) => echoed.extend(d),
                    other => panic!("{other:?}"),
                }
            }
        })
        .await
        .expect("every echo arrived");
    }

    /// The whole thing on real WebRTC peers in this process: the device
    /// types into a terminal over the hub, a direct path comes up (signaling
    /// inside the channel, Noise on the DataChannel) and carries the
    /// keystrokes, then UDP stops mid-typing — every key is echoed once, in
    /// order, and the channel is back on the hub.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn keystrokes_survive_a_direct_path_that_goes_dark_mid_typing() {
        let dark = Arc::new(AtomicBool::new(false));
        let w = world(loopback(None)).await;
        let mut stats = w.device.path_stats();
        let mut term = w
            .device
            .open(StreamClass::Term, &RequestHead::get("/api/terminals/t/ws").encode().unwrap())
            .await
            .unwrap();
        let mut echoed = Vec::new();
        type_and_collect(&mut term, 0..40, &mut echoed).await;

        let mut sig = Signaling::open(&w.device).await.unwrap();
        let attached = try_direct(&w, &mut sig, 1, loopback(Some(dark.clone())), Duration::from_secs(5))
            .await
            .unwrap();
        assert!(attached.took < Duration::from_secs(5));
        tokio::time::timeout(Duration::from_secs(5), async {
            while !stats.borrow().direct || stats.borrow().direct_rtt_ms.is_none() {
                stats.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        let s = *stats.borrow();
        assert!(s.direct_rtt_ms.unwrap() < 15.0, "direct is faster than the 40 ms hub round trip: {s:?}");
        type_and_collect(&mut term, 40..120, &mut echoed).await;

        // UDP goes dark mid-typing.
        dark.store(true, Ordering::Relaxed);
        type_and_collect(&mut term, 120..200, &mut echoed).await;
        collect_until(&mut term, 200, &mut echoed).await;
        assert_eq!(echoed, (0..200u8).collect::<Vec<_>>(), "every key, once, in order");
        let s = *stats.borrow();
        assert!(!s.direct, "back on the hub: {s:?}");
        assert_eq!((s.ups, s.downs), (1, 1));
        drop(w.agent_inbound);
    }

    /// UDP blocked from the start: the attempt fails within its timeout and
    /// the channel goes on over the hub as if nothing happened.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn with_udp_blocked_the_attempt_fails_and_the_hub_carries_on() {
        let w = world(loopback(None)).await;
        let mut term = w
            .device
            .open(StreamClass::Term, &RequestHead::get("/api/terminals/t/ws").encode().unwrap())
            .await
            .unwrap();
        let mut sig = Signaling::open(&w.device).await.unwrap();
        let blocked = loopback(Some(Arc::new(AtomicBool::new(true))));
        let err = try_direct(&w, &mut sig, 1, blocked, Duration::from_secs(3))
            .await
            .err()
            .expect("no direct path");
        assert!(matches!(err, AttemptError::Failed(_) | AttemptError::Bye(ByeReason::Failed, _)), "{err}");
        let mut echoed = Vec::new();
        type_and_collect(&mut term, 0..30, &mut echoed).await;
        collect_until(&mut term, 30, &mut echoed).await;
        assert_eq!(echoed, (0..30u8).collect::<Vec<_>>());
        assert!(!w.device.path_stats().borrow().direct);
        // And a later attempt, with UDP back, works.
        let attached = try_direct(&w, &mut sig, 2, loopback(None), Duration::from_secs(5)).await;
        assert!(attached.is_ok(), "{:?}", attached.err().map(|e| e.to_string()));
    }

    /// An offer whose SDP names a different certificate than the one stated
    /// next to it (a swapped SDP) is refused by the agent.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_agent_refuses_an_offer_with_a_tampered_fingerprint() {
        let w = world(loopback(None)).await;
        let mut sig = Signaling::open(&w.device).await.unwrap();
        let (_peer, offer) = Peer::offer(&loopback(None)).await.unwrap();
        let honest = crate::path::sdp_fingerprint(&offer).unwrap();
        let mut other = honest;
        other[0] ^= 0xff;
        let tampered = offer.replace(&crate::path::fingerprint_hex(&honest), &crate::path::fingerprint_hex(&other));
        sig.send(&Signal::Offer {
            id: 9,
            fp: honest,
            sdp: tampered,
        })
        .await
        .unwrap();
        let answer = tokio::time::timeout(Duration::from_secs(5), sig.incoming.recv()).await.unwrap().unwrap();
        assert!(
            matches!(
                answer,
                Signal::Bye {
                    id: 9,
                    reason: ByeReason::BadFingerprint,
                    ..
                }
            ),
            "{answer:?}"
        );
    }

    /// The agent closes the channel of a device the chain no longer lists:
    /// its direct path closes with it, and the device's next attempt is
    /// refused by the same rules (here: the device is gone from the agent's
    /// chain).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_removed_devices_direct_path_closes_with_its_channel_and_cannot_return() {
        let mut w = world(loopback(None)).await;
        let mut sig = Signaling::open(&w.device).await.unwrap();
        let mut attached = try_direct(&w, &mut sig, 1, loopback(None), Duration::from_secs(5)).await.unwrap();
        // A refused re-attempt first: remove dev-0's standing at the agent by
        // dropping its attestations (stale) — refused, the path it has stays.
        w.admit.state.lock().unwrap().3.clear();
        let err = try_direct(&w, &mut sig, 2, loopback(None), Duration::from_secs(5))
            .await
            .err()
            .expect("refused");
        assert!(matches!(err, AttemptError::Failed(_) | AttemptError::Bye(ByeReason::Refused, _)), "{err}");
        // Now the agent closes the channel (the removal): the direct path goes too.
        w.agent.close(4403, "this device was removed from the account");
        drop(w.agent_inbound.take());
        let closed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match attached.events.recv().await {
                    Some(PeerEvent::Closed(_)) | None => return,
                    _ => {}
                }
            }
        })
        .await;
        assert!(closed.is_ok(), "the device's direct path closed with the channel");
    }
}
