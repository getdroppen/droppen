//! The M0 acceptance check, in-process: a device and an agent complete the
//! end-to-end handshake and talk through a relay that only forwards opaque
//! frames — and a canary the device sends never appears in the relayed bytes.

use std::collections::VecDeque;

use crate::chain::tests::{fixture, sample_chain};
use crate::chain::{ChainClock, ChainState};
use crate::channel::Session;
use crate::frame::{Frame, FrameDecoder, RawFrame};
use crate::handshake::{self, Accepted, AgentSide};
use crate::mux::{MuxEvent, Role, StreamClass};
use crate::noise::{Binding, Initiator, RekeyPolicy, ReplayGuard, Responder};

const CANARY: &[u8] = b"RH-CANARY-6f1d2c9e-do-not-relay";

/// Pairs two connections and copies whole frames between them, keeping a copy
/// of every byte it handled. It parses envelopes and nothing else, like the hub.
#[derive(Default)]
struct DumbRelay {
    seen: Vec<u8>,
    from_device: FrameDecoder,
    from_agent: FrameDecoder,
    to_agent: VecDeque<Vec<u8>>,
    to_device: VecDeque<Vec<u8>>,
}

impl DumbRelay {
    fn device_sends(&mut self, frames: &[Frame]) {
        for frame in frames {
            let bytes = frame.encode().unwrap();
            self.seen.extend_from_slice(&bytes);
            self.from_device.push(&bytes);
        }
        while let Some(raw) = self.from_device.next_frame().unwrap() {
            // The one thing a hub rewrites: an OPEN names the peer it came from.
            let raw = if raw.ty == crate::frame::FrameType::Open as u8 {
                Frame::Open {
                    ch: raw.ch,
                    peer: "dev-0".into(),
                }
                .to_raw()
                .unwrap()
            } else {
                raw
            };
            self.to_agent.push_back(raw.encode().unwrap());
        }
    }

    fn agent_sends(&mut self, frames: &[Frame]) {
        for frame in frames {
            let bytes = frame.encode().unwrap();
            self.seen.extend_from_slice(&bytes);
            self.from_agent.push(&bytes);
        }
        while let Some(raw) = self.from_agent.next_frame().unwrap() {
            self.to_device.push_back(raw.encode().unwrap());
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn binding_for(chain: &ChainState, device: &str) -> Binding {
    Binding {
        account: chain.account().into(),
        device: device.into(),
        machine: "m-1".into(),
    }
}

struct Agent {
    chain: ChainState,
    clock: ChainClock,
    attestations: Vec<crate::chain::Attestation>,
    guard: ReplayGuard,
    session: Option<Session>,
    received: Vec<u8>,
}

impl Agent {
    fn on_frame(&mut self, bytes: &[u8], machine: &crate::keys::DhKeypair, pending_open: &mut Option<String>) -> Vec<Frame> {
        match Frame::from_raw(RawFrame::decode(bytes).unwrap()).unwrap() {
            Frame::Open { peer, .. } => {
                *pending_open = Some(peer);
                vec![]
            }
            Frame::Data { ch, payload } if self.session.is_none() => {
                let device = pending_open.take().expect("OPEN first");
                let side = AgentSide {
                    chain: &mut self.chain,
                    clock: &mut self.clock,
                    guard: &mut self.guard,
                    key: machine,
                    machine: "m-1",
                    policy: RekeyPolicy::default(),
                    server_info: b"features:term,rpc",
                    attestations: &self.attestations,
                    freshness_max_age_ms: crate::handshake::FRESHNESS_MAX_AGE_MS,
                };
                let Accepted::Open { reply, transport, hello, .. } = handshake::accept(side, &device, &payload, 1_000).unwrap() else {
                    panic!("accepted")
                };
                assert_eq!(hello.client, b"hello agent");
                self.session = Some(Session::new(ch, *transport, Role::Responder));
                vec![Frame::Data { ch, payload: reply }]
            }
            Frame::Data { payload, .. } => {
                let session = self.session.as_mut().unwrap();
                match session.receive(&payload, 1_000).unwrap().into_iter().next() {
                    Some(MuxEvent::Data { sid, data }) => {
                        self.received.extend_from_slice(&data);
                        session.mux().consumed(sid, data.len());
                        let mut echo = b"echo:".to_vec();
                        echo.extend_from_slice(&data);
                        session.mux().send(sid, &echo).unwrap();
                    }
                    Some(MuxEvent::Opened { .. }) | Some(MuxEvent::Closed { .. }) | Some(MuxEvent::Pong { .. }) | None => {}
                }
                session.flush(1_000).unwrap()
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

#[test]
fn a_device_and_an_agent_talk_through_a_relay_that_never_sees_plaintext() {
    let f = fixture();
    let entries = sample_chain(&f);
    // Each side verifies the chain itself; the agent against its pinned AK.
    let pin = crate::identity::fingerprint(&f.ak.verifying_key().to_bytes());
    let device_view = ChainState::verify(&entries, None, &mut ChainClock::history()).unwrap();
    let mut agent = Agent {
        chain: ChainState::verify(&entries, Some(&pin), &mut ChainClock::history()).unwrap(),
        clock: ChainClock::live(0, entries.len() as u64),
        attestations: vec![crate::chain::Attestation::create(&device_view, "dev-0", &f.devices[0].0, 900)],
        guard: ReplayGuard::new(60_000, 60_000, 8, 0),
        session: None,
        received: vec![],
    };
    let mut relay = DumbRelay::default();
    let mut pending_open = None;

    // Device: OPEN, and the first handshake message (its chain view and client
    // metadata — never a request).
    let (initiator, hello) = handshake::start(&device_view, &f.devices[0].1, "dev-0", "m-1", 1_000, b"hello agent").unwrap();
    relay.device_sends(&[Frame::Open { ch: 1, peer: "m-1".into() }, Frame::Data { ch: 1, payload: hello }]);
    while let Some(bytes) = relay.to_agent.pop_front() {
        let out = agent.on_frame(&bytes, &f.machine.1, &mut pending_open);
        relay.agent_sends(&out);
    }
    let Frame::Data { payload: reply, .. } = Frame::decode(&relay.to_device.pop_front().unwrap()).unwrap() else {
        panic!("a reply")
    };
    let (transport, reply) = handshake::finish(
        initiator,
        &mut device_view.clone(),
        &mut ChainClock::live(0, 5),
        &reply,
        RekeyPolicy::default(),
        1_000,
    )
    .unwrap();
    assert_eq!(reply.server, b"features:term,rpc");
    let mut device = Session::new(1, transport, Role::Initiator);

    // Device: a terminal stream carrying the canary, and an rpc stream.
    let term = device.mux().open(StreamClass::Term, b"t-1").unwrap();
    device.mux().send(term, CANARY).unwrap();
    let rpc = device.mux().open(StreamClass::Rpc, b"GET /api/sessions").unwrap();
    device.mux().send(rpc, b"{}").unwrap();
    let frames = device.flush(1_000).unwrap();
    relay.device_sends(&frames);
    while let Some(bytes) = relay.to_agent.pop_front() {
        let out = agent.on_frame(&bytes, &f.machine.1, &mut pending_open);
        relay.agent_sends(&out);
    }
    let mut echoed = Vec::new();
    while let Some(bytes) = relay.to_device.pop_front() {
        let Frame::Data { payload, .. } = Frame::decode(&bytes).unwrap() else {
            panic!("data")
        };
        if let Some(MuxEvent::Data { data, .. }) = device.receive(&payload, 1_000).unwrap().into_iter().next() {
            echoed.extend_from_slice(&data);
        }
    }

    assert!(contains(&agent.received, CANARY), "the agent read the canary");
    assert!(contains(&echoed, CANARY), "and echoed it back");
    assert!(relay.seen.len() > CANARY.len() * 2, "the relay carried the traffic");
    assert!(!contains(&relay.seen, CANARY), "the relay never saw the canary");
    assert!(!contains(&relay.seen, b"GET /api/sessions"), "nor the request line");
    assert!(!contains(&relay.seen, b"hello agent"), "nor the hello's metadata");
}

#[test]
fn a_relay_that_tampers_or_replays_breaks_the_channel_instead_of_fooling_it() {
    let f = fixture();
    let chain = ChainState::verify(&sample_chain(&f), None, &mut ChainClock::history()).unwrap();
    let binding = binding_for(&chain, "dev-0");
    let machine_key = chain.machine("m-1").unwrap().info.dh_key;
    let handshake = || {
        let mut i = Initiator::new(&f.devices[0].1, &machine_key, &binding).unwrap();
        let mut r = Responder::new(&f.machine.1, &binding).unwrap();
        r.read_hello(&i.write_hello(0, b"").unwrap()).unwrap();
        let (m2, rt) = r.write_reply(b"", RekeyPolicy::default(), 0).unwrap();
        let (it, _) = i.read_reply(&m2, RekeyPolicy::default(), 0).unwrap();
        (Session::new(1, it, Role::Initiator), Session::new(1, rt, Role::Responder))
    };

    let (mut device, mut agent) = handshake();
    let sid = device.mux().open(StreamClass::Term, b"").unwrap();
    device.mux().send(sid, b"rm -rf /tmp/x\r").unwrap();
    let frames = device.flush(0).unwrap();
    let Frame::Data { payload: open, .. } = &frames[0] else { panic!() };
    let Frame::Data { payload: data, .. } = &frames[1] else { panic!() };
    agent.receive(open, 0).unwrap();
    let mut tampered = data.clone();
    tampered[10] ^= 0x20;
    assert!(agent.receive(&tampered, 0).is_err());
    assert!(agent.receive(data, 0).is_err(), "the channel stays dead after a forgery");

    let (mut device, mut agent) = handshake();
    let sid = device.mux().open(StreamClass::Term, b"").unwrap();
    device.mux().send(sid, b"\x03").unwrap();
    let frames = device.flush(0).unwrap();
    for frame in &frames {
        let Frame::Data { payload, .. } = frame else { panic!() };
        agent.receive(payload, 0).unwrap();
    }
    let Frame::Data { payload: replayed, .. } = &frames[1] else { panic!() };
    assert!(agent.receive(replayed, 0).is_err(), "a replayed keystroke is refused");
}
