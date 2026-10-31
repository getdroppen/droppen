// The direct path's owner on the device (`lib/transport/p2p.ts`), against a
// fake WebRTC and a TypeScript stand-in for the agent's half
// (`droppen-proto/src/direct.rs`): two channel drivers with paths, wired through a
// "hub", the agent answering the signaling stream the way the real one does.
//
// What these pin: signaling rides the channel's own `GET /rh/direct` stream;
// an offer states this device's fingerprint and goes without candidates
// (they trickle, then an end); the agent's answer is refused — with a
// `badFingerprint` goodbye — unless its SDP names the certificate stated next
// to it; on the DataChannel the direct handshake runs under the binding of
// this channel, the agent's nonce and both fingerprints, and then the driver
// carries the channel over it; "hub only" means no signaling and no peer
// connection at all, and turning it on lets a direct path go at once; an
// attempt that does not connect times out and the next waits longer, unless
// the network changes; an agent that refuses the stream or says `disabled`
// gets no more attempts; a refused handshake catches up and tries later; a
// DataChannel that closes, or a path the driver gives up, falls back to the
// hub and is retried; paths that keep dying young are retried only on the
// backoff — not on a network change, and not afresh on the next channel —
// until one lives; closing the channel closes everything.
import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import {
  bridgeMessage,
  type Bytes,
  type ByeReason,
  ChannelDriver,
  type ChannelStream,
  checkFingerprint,
  DEFAULT_REKEY,
  decodeDcMessage,
  decodeRequestHead,
  decodeSignal,
  type DirectBinding,
  type DirectLink,
  directPrologue,
  encodeDcMessage,
  encodeResponseHead,
  encodeSignal,
  fingerprintHex,
  type Frame,
  generateDeviceKeys,
  HandshakeError,
  Initiator,
  MessageReader,
  type NoiseKeyPair,
  Responder,
  type Signal,
  SIGNAL_PATH,
  utf8,
} from "../src/lib/e2e/index.js";
import {
  type DataChannelLike,
  type DirectEnv,
  type DirectHandshake,
  DirectHistory,
  DirectPaths,
  type DirectTiming,
  hubOnly,
  onHubOnly,
  type PeerLike,
  setHubOnly,
  SHORT_LIVED_LIMIT,
  STUN_SERVERS,
} from "../src/lib/transport/p2p.js";

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const binding = { account: "a-1", device: "d-1", machine: "m-1" };

/** Wait for `what`; `timeoutMs` only keeps a broken test from hanging (it is no claim about how fast). */
async function until(what: () => boolean, label: string, timeoutMs = 10_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!what()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`);
    await sleep(2);
  }
}

const randomFp = () => crypto.getRandomValues(new Uint8Array(32)) as Bytes;
const sdpFor = (fp: Uint8Array) =>
  `v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\nc=IN IP4 0.0.0.0\r\na=fingerprint:sha-256 ${fingerprintHex(fp)}\r\na=setup:actpass\r\na=mid:0\r\n`;
const HOST = "candidate:1 1 udp 2122260223 192.0.2.1 50000 typ host";

/** A DataChannel: messages go to `other` a tick later; closing either end closes both. */
class FakeDc implements DataChannelLike {
  binaryType = "blob";
  bufferedAmount = 0;
  bufferedAmountLowThreshold = 0;
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onbufferedamountlow: (() => void) | null = null;
  state: "connecting" | "open" | "closed" = "connecting";
  other: FakeDc | null = null;
  blackhole = false;
  carried = 0;

  send(data: Bytes): void {
    if (this.state !== "open") throw new Error("InvalidStateError: the data channel is not open");
    this.carried++;
    if (this.blackhole) return;
    const copy = Uint8Array.from(data);
    const other = this.other;
    setTimeout(() => {
      if (other?.state === "open" && !other.blackhole) other.onmessage?.({ data: copy.buffer });
    }, 0);
  }

  open(): void {
    if (this.state !== "connecting") return;
    this.state = "open";
    this.onopen?.();
  }

  close(): void {
    if (this.state === "closed") return;
    this.state = "closed";
    setTimeout(() => this.onclose?.(), 0);
    const other = this.other;
    if (other && other.state !== "closed") setTimeout(() => other.close(), 0);
  }
}

class FakePeer implements PeerLike {
  readonly fp = randomFp();
  localDescription: { sdp: string } | null = null;
  remoteSdp: string | null = null;
  added: string[] = [];
  connectionState = "new";
  onicecandidate: ((ev: { candidate: { candidate: string } | null }) => void) | null = null;
  onconnectionstatechange: (() => void) | null = null;
  dc!: FakeDc;
  label = "";
  ordered = false;
  closed = false;
  /** The harness connects the DataChannels when the answer is applied. */
  onremote: (() => void) | null = null;

  createDataChannel(label: string, init: { ordered: boolean }): FakeDc {
    this.label = label;
    this.ordered = init.ordered;
    this.dc = new FakeDc();
    return this.dc;
  }

  async createOffer() {
    // A browser may put a host candidate in the SDP itself.
    return { type: "offer", sdp: `${sdpFor(this.fp)}a=candidate:0 1 udp 1 192.0.2.9 9 typ host\r\n` };
  }

  async setLocalDescription(d: { type: "offer"; sdp?: string }): Promise<void> {
    this.localDescription = { sdp: d.sdp ?? "" };
    setTimeout(() => {
      this.onicecandidate?.({ candidate: { candidate: HOST } });
      this.onicecandidate?.({ candidate: { candidate: "" } });
      this.onicecandidate?.({ candidate: null });
    }, 0);
  }

  async setRemoteDescription(d: { type: "answer"; sdp: string }): Promise<void> {
    this.remoteSdp = d.sdp;
    queueMicrotask(() => this.onremote?.());
  }

  async addIceCandidate(c: { candidate: string; sdpMLineIndex: number }): Promise<void> {
    this.added.push(c.candidate);
  }

  close(): void {
    this.closed = true;
    this.connectionState = "closed";
    this.dc.close();
  }
}

interface AgentBehaviour {
  /** The signaling stream's head (101 opens it; the real agent says 403 when direct paths are off). */
  status?: number;
  /** State a fingerprint other than the one the answer's SDP names. */
  tamper?: boolean;
  /** Answer every offer with this goodbye instead. */
  bye?: ByeReason;
  /** Open the DataChannel once the answer is applied (otherwise ICE never connects). */
  connect?: boolean;
}

/** The agent's half: serves `/rh/direct` like `direct.rs` `serve`, and echoes every other stream. */
class AgentSide {
  signals: Signal[] = [];
  signalStreams = 0;
  dcs: FakeDc[] = [];
  attached = 0;
  behaviour: AgentBehaviour;
  private signal: ((s: Signal) => void) | null = null;
  private lastOffer = 0;

  constructor(
    readonly driver: ChannelDriver,
    readonly keys: NoiseKeyPair,
    readonly peers: FakePeer[],
    behaviour: AgentBehaviour,
  ) {
    this.behaviour = { status: 101, connect: true, ...behaviour };
    driver.onincoming = (stream) => this.serve(stream);
  }

  byes(): Signal[] {
    return this.signals.filter((s) => s.type === "bye");
  }

  private serve(stream: ChannelStream): void {
    let path = "";
    try {
      path = decodeRequestHead(stream.meta).path;
    } catch {
      // not a bridged request: a test terminal
    }
    if (path !== SIGNAL_PATH) {
      stream.ondata = (data) => stream.send(data);
      return;
    }
    this.signalStreams++;
    stream.send(bridgeMessage("head", encodeResponseHead({ status: this.behaviour.status!, headers: [] })));
    if (this.behaviour.status !== 101) {
      stream.close(4403);
      return;
    }
    const reader = new MessageReader();
    const send = (s: Signal) => stream.send(bridgeMessage("binary", encodeSignal(s)));
    this.signal = send;
    stream.ondata = (data) => {
      reader.push(data);
      for (let m = reader.next(); m; m = reader.next()) {
        if (m.kind !== "binary") continue;
        const s = decodeSignal(m.payload);
        this.signals.push(s);
        if (s.type === "offer") this.offer(s, send);
      }
    };
  }

  /** End the latest attempt from the agent's side, with a goodbye, as `direct.rs` does. */
  bye(reason: ByeReason): void {
    this.signal?.({ type: "bye", id: this.lastOffer, reason, detail: `the agent says ${reason}` });
  }

  private offer(s: Extract<Signal, { type: "offer" }>, send: (s: Signal) => void): void {
    this.lastOffer = s.id;
    if (this.behaviour.bye) return send({ type: "bye", id: s.id, reason: this.behaviour.bye, detail: "no" });
    checkFingerprint(s.sdp, s.fp);
    const agentFp = randomFp();
    const nonce = randomFp();
    const dc = new FakeDc();
    this.dcs.push(dc);
    const peer = this.peers.at(-1)!;
    peer.onremote = () => {
      if (!this.behaviour.connect) return;
      dc.other = peer.dc;
      peer.dc.other = dc;
      setTimeout(() => {
        dc.open();
        peer.dc.open();
      }, 1);
    };
    const bind: DirectBinding = { binding, relayHash: this.driver.handshakeHash, nonce, deviceFp: s.fp, agentFp };
    let link: DirectLink | null = null;
    let chain = Promise.resolve();
    dc.onmessage = (ev) => {
      const bytes = new Uint8Array(ev.data as ArrayBuffer);
      chain = chain.then(async () => {
        if (link) return this.driver.receiveDirect(bytes, link);
        const m = decodeDcMessage(bytes);
        assert.equal(m.type, "hello", "the device speaks first, with the direct handshake");
        const r = await Responder.withPrologue(this.keys, directPrologue(bind));
        const hello = await r.readHello(m.body);
        assert.equal(new TextDecoder().decode(hello.payload), "web/1 paths/1");
        const { message, transport } = await r.writeReply(utf8("accepted"), DEFAULT_REKEY, Date.now());
        dc.send(encodeDcMessage({ type: "reply", body: message }));
        link = { send: (msg) => dc.send(msg), bufferedAmount: () => 0, released: () => dc.close() };
        this.attached++;
        this.driver.attachDirect(transport, link);
      });
    };
    send({ type: "answer", id: s.id, fp: this.behaviour.tamper ? randomFp() : agentFp, nonce, sdp: sdpFor(agentFp) });
    send({ type: "candidate", id: s.id, candidate: "candidate:2 1 udp 2122260223 192.0.2.2 40000 typ host" });
    send({ type: "candidate", id: s.id, candidate: null });
  }
}

async function noiseKeys(): Promise<NoiseKeyPair> {
  const k = await generateDeviceKeys();
  return { privateKey: k.dh.privateKey, publicKey: k.dhPublic };
}

interface Setup {
  device: ChannelDriver;
  agent: AgentSide;
  paths: DirectPaths;
  peers: FakePeer[];
  configs: unknown[];
  peerTimes: number[];
  setAllowed(on: boolean): void;
  networkChanged(): void;
  refusals: number;
}

async function setUp(
  t: TestContext,
  options: { allowed?: boolean; behaviour?: AgentBehaviour; timing?: Partial<DirectTiming>; refuse?: () => boolean; history?: DirectHistory } = {},
): Promise<Setup> {
  const deviceKeys = await noiseKeys();
  const agentKeys = await noiseKeys();
  const i = await Initiator.create(deviceKeys, agentKeys.publicKey, binding);
  const r = await Responder.create(agentKeys, binding);
  await r.readHello(await i.writeHello(1, utf8("")));
  const { message, transport: rt } = await r.writeReply(utf8(""), DEFAULT_REKEY, 1);
  const { transport: it } = await i.readReply(message, DEFAULT_REKEY, 1);
  let device!: ChannelDriver;
  let agentDriver!: ChannelDriver;
  const hub = (to: () => ChannelDriver) => (f: Frame) => setTimeout(() => to().receive(f), 1);
  device = new ChannelDriver(1, it, "initiator", hub(() => agentDriver), Date.now, { paths: { probeRelay: true } });
  agentDriver = new ChannelDriver(1, rt, "responder", hub(() => device), Date.now, { paths: { probeRelay: false } });

  const peers: FakePeer[] = [];
  const configs: unknown[] = [];
  const peerTimes: number[] = [];
  let allowed = options.allowed ?? true;
  const allowedFns = new Set<() => void>();
  const netFns = new Set<() => void>();
  const env: Partial<DirectEnv> = {
    createPeer: (config) => {
      configs.push(config);
      peerTimes.push(performance.now());
      const p = new FakePeer();
      peers.push(p);
      return p;
    },
    allowed: () => allowed,
    onAllowedChange: (fn) => {
      allowedFns.add(fn);
      return () => allowedFns.delete(fn);
    },
    onNetworkChange: (fn) => {
      netFns.add(fn);
      return () => netFns.delete(fn);
    },
    random: () => 0.5,
    log: () => undefined,
  };
  const s: Setup = {
    device,
    agent: new AgentSide(agentDriver, agentKeys, peers, options.behaviour ?? {}),
    paths: null as unknown as DirectPaths,
    peers,
    configs,
    peerTimes,
    setAllowed(on) {
      allowed = on;
      for (const fn of [...allowedFns]) fn();
    },
    networkChanged() {
      for (const fn of [...netFns]) fn();
    },
    refusals: 0,
  };
  const handshake: DirectHandshake = {
    start: async (b) => {
      const initiator = await Initiator.withPrologue(deviceKeys, agentKeys.publicKey, directPrologue(b));
      return { initiator, message: await initiator.writeHello(Date.now(), utf8("web/1 paths/1")) };
    },
    finish: async (initiator, reply) => {
      const { transport } = await initiator.readReply(reply, DEFAULT_REKEY, Date.now());
      if (options.refuse?.()) throw new HandshakeError("refused", "the agent needs a recent attestation of its chain head");
      return transport;
    },
    refused: async () => {
      s.refusals++;
    },
  };
  s.paths = new DirectPaths({ driver: device, binding, handshake, env, timing: { retryFirstMs: 30, ...options.timing }, history: options.history });
  t.after(() => {
    s.paths.close();
    device.close();
    agentDriver.close();
  });
  return s;
}

/** An echoing terminal over the channel; what came back. */
function terminal(s: Setup): { term: ChannelStream; echoed: number[] } {
  const term = s.device.open("term", utf8("t-1"));
  const echoed: number[] = [];
  term.ondata = (d) => echoed.push(...d);
  return { term, echoed };
}

test("a direct path comes up over the channel's own signaling, bound to it, and carries the channel", async (t) => {
  const s = await setUp(t);
  s.paths.start();
  await until(() => s.device.paths?.direct === true, "the direct path");
  assert.equal(s.paths.state, "direct");
  assert.equal(s.agent.signalStreams, 1, "one signaling stream");
  const peer = s.peers[0]!;
  assert.deepEqual(s.configs[0], { iceServers: STUN_SERVERS }, "STUN only, no TURN");
  assert.equal(peer.label, "rh");
  assert.ok(peer.ordered);
  assert.equal(peer.dc.binaryType, "arraybuffer");
  const [offer, ...rest] = s.agent.signals;
  assert.ok(offer?.type === "offer" && offer.id === 1);
  assert.deepEqual(offer.fp, peer.fp, "the offer states this device's own fingerprint");
  assert.ok(!offer.sdp.includes("a=candidate"), "candidates trickle instead");
  assert.deepEqual(
    rest.map((x) => (x.type === "candidate" ? x.candidate : x.type)),
    [HOST, null],
    "one candidate, then the end, once",
  );
  assert.ok(peer.remoteSdp?.includes("a=fingerprint:sha-256"));
  assert.deepEqual(peer.added, ["candidate:2 1 udp 2122260223 192.0.2.2 40000 typ host", ""], "the agent's candidate, then its end");

  const { term, echoed } = terminal(s);
  const before = peer.dc.carried;
  for (let k = 0; k < 20; k++) term.send(Uint8Array.of(k));
  await until(() => echoed.length === 20, "the echoes");
  assert.deepEqual(echoed, Array.from({ length: 20 }, (_, k) => k));
  assert.ok(peer.dc.carried > before, "over the DataChannel");
  assert.equal(s.paths.attempts, 1);
});

test("an answer whose SDP names another certificate is refused with badFingerprint, and tried again later", async (t) => {
  const s = await setUp(t, { behaviour: { tamper: true } });
  s.paths.start();
  await until(() => s.agent.byes().length >= 1, "the goodbye");
  const bye = s.agent.byes()[0]!;
  assert.ok(bye.type === "bye" && bye.id === 1 && bye.reason === "badFingerprint");
  assert.equal(s.peers[0]!.remoteSdp, null, "the tampered answer was never applied");
  assert.ok(s.peers[0]!.closed);
  assert.equal(s.device.paths!.direct, false);
  await until(() => s.agent.signals.some((x) => x.type === "offer" && x.id === 2), "the next attempt");
});

test("hub only: no signaling and no peer connection; on, a direct path; hub only again lets it go at once", async (t) => {
  const s = await setUp(t, { allowed: false });
  s.paths.start();
  await sleep(60);
  assert.equal(s.paths.state, "off");
  assert.equal(s.peers.length, 0, "no peer connection, so no STUN, so no address shown");
  assert.equal(s.agent.signalStreams, 0, "and nothing asked of the agent");

  s.setAllowed(true);
  await until(() => s.device.paths?.direct === true, "the direct path");
  const peer = s.peers[0]!;
  s.setAllowed(false);
  // At once: the setting itself tears the peer connection down, with no turn
  // of the event loop in between (the path was healthy: nothing else would).
  assert.ok(peer.closed, "the peer connection is torn down");
  await until(() => s.device.paths?.direct === false, "the fallback");
  await until(() => s.agent.byes().length > 0, "the goodbye");
  const bye = s.agent.byes()[0];
  assert.ok(bye?.type === "bye" && bye.reason === "disabled");
  assert.equal(s.paths.state, "off");
  await sleep(120);
  assert.equal(s.peers.length, 1, "and no further attempts");
});

test("the hub-only setting is this device's, remembered, and heard by whoever listens", () => {
  let heard = 0;
  const off = onHubOnly(() => heard++);
  const was = hubOnly();
  setHubOnly(true);
  assert.equal(hubOnly(), true);
  setHubOnly(false);
  assert.equal(hubOnly(), false);
  assert.equal(heard, 2);
  off();
  setHubOnly(was);
});

test("an attempt that never connects times out and says so; the next waits longer; a network change retries at once", async (t) => {
  // The gaps are measured on the real clock, so the waits are long enough
  // that each upper bound below leaves a loaded machine 140 ms or more while
  // still telling the steps apart: a first wait already doubled would make a
  // gap of 340, a second one not doubled 190, an uncapped third 640.
  const s = await setUp(t, { behaviour: { connect: false }, timing: { attemptTimeoutMs: 40, retryFirstMs: 150, retryMaxMs: 300 } });
  s.paths.start();
  await until(() => s.peers.length >= 4, "four attempts");
  const byes = s.agent.byes();
  assert.ok(byes.length >= 3 && byes.every((b) => b.type === "bye" && b.reason === "failed"), "each attempt says it failed");
  assert.ok(s.peers.slice(0, 3).every((p) => p.closed), "and is torn down");
  const gaps = s.peerTimes.slice(1).map((at, i) => at - s.peerTimes[i]!);
  // timeout 40 + wait 150, then 40 + 300, then 40 + 300 (the cap).
  assert.ok(gaps[0]! >= 180 && gaps[0]! < 330, `first gap ${gaps[0]}`);
  assert.ok(gaps[1]! >= 330 && gaps[1]! < 600, `second gap, doubled: ${gaps[1]}`);
  assert.ok(gaps[2]! >= 330 && gaps[2]! < 600, `third gap, capped: ${gaps[2]}`);
  // While it waits, the network changes: at once, in the same turn.
  await until(() => s.paths.state === "waiting", "waiting");
  const count = s.peers.length;
  s.networkChanged();
  assert.equal(s.peers.length, count + 1, "a new attempt at once");
});

test("an agent that refuses the signaling stream gets no attempt on this channel", async (t) => {
  const s = await setUp(t, { behaviour: { status: 403 } });
  s.paths.start();
  await until(() => s.paths.state === "unsupported", "unsupported");
  await sleep(80);
  assert.equal(s.peers.length, 0);
  s.networkChanged();
  s.setAllowed(true);
  assert.equal(s.peers.length, 0);
});

test("an agent that says disabled stops the attempts for this channel", async (t) => {
  const s = await setUp(t, { behaviour: { bye: "disabled" } });
  s.paths.start();
  await until(() => s.paths.state === "unsupported", "unsupported");
  assert.ok(s.peers[0]!.closed);
  await sleep(100);
  assert.equal(s.peers.length, 1, "no more attempts");
});

test("a refused direct handshake says so, catches up, and tries again later", async (t) => {
  let refuse = true;
  const s = await setUp(t, { refuse: () => refuse });
  s.paths.start();
  await until(() => s.agent.byes().length >= 1, "the goodbye");
  const bye = s.agent.byes()[0]!;
  assert.ok(bye.type === "bye" && bye.reason === "refused");
  assert.equal(s.refusals, 1, "synced and attested, as the hub path does");
  refuse = false;
  await until(() => s.device.paths?.direct === true, "the next attempt carries the channel");
  assert.equal(s.paths.attempts, 2);
});

test("a DataChannel that closes falls back at once, and a new attempt follows", async (t) => {
  const s = await setUp(t);
  s.paths.start();
  await until(() => s.device.paths?.direct === true, "the direct path");
  const { term, echoed } = terminal(s);
  for (let k = 0; k < 10; k++) term.send(Uint8Array.of(k));
  await until(() => echoed.length === 10, "the echoes");
  s.agent.dcs[0]!.close();
  await until(() => s.device.paths?.direct === false, "the fallback");
  for (let k = 10; k < 20; k++) term.send(Uint8Array.of(k));
  await until(() => s.device.paths?.direct === true && s.peers.length === 2, "a second direct path");
  await until(() => echoed.length === 20, "the echoes");
  assert.deepEqual(echoed, Array.from({ length: 20 }, (_, k) => k), "nothing lost, nothing twice");
  assert.equal(s.device.paths!.ups, 2);
  assert.ok(s.peers[0]!.closed);
});

test("a path the driver gives up (it went dark) is torn down and tried again", async (t) => {
  const s = await setUp(t);
  s.paths.start();
  await until(() => s.device.paths?.direct === true, "the direct path");
  const peer = s.peers[0]!;
  peer.dc.blackhole = true;
  s.agent.dcs[0]!.blackhole = true;
  const { term, echoed } = terminal(s);
  for (let k = 0; k < 5; k++) term.send(Uint8Array.of(k));
  await until(() => s.device.paths?.direct === false, "given up", 15_000);
  await until(() => peer.closed, "the peer connection torn down");
  await until(() => s.agent.byes().some((b) => b.type === "bye" && b.id === 1 && b.reason === "failed"), "the goodbye");
  await until(() => echoed.length === 5, "the echoes, over the hub");
  await until(() => s.device.paths?.direct === true && s.peers.length === 2, "a new direct path");
});

/** Let `n` direct paths in a row carry the channel and die young (their DataChannel closes at once); resolves when the last has been let go. */
async function dieYoung(s: Setup, n: number): Promise<number> {
  let endedAt = 0;
  for (let k = 0; k < n; k++) {
    await until(() => s.paths.state === "direct" && s.agent.dcs.at(-1)?.state === "open", "a direct path carrying the channel");
    s.agent.dcs.at(-1)!.close();
    await until(() => s.paths.state === "waiting", "let go");
    endedAt = performance.now();
  }
  return endedAt;
}

test("direct paths that keep dying young are not retried at once on every network change, and wait ever longer", async (t) => {
  // retryFirstMs 40: the waits after the three young deaths are 40, 80 and
  // 160, then 320 — each long enough to tell from "at once" on a loaded machine.
  const s = await setUp(t, { timing: { retryFirstMs: 40, retryMaxMs: 5_000, stableMs: 60_000 } });
  s.paths.start();
  const endedAt = await dieYoung(s, SHORT_LIVED_LIMIT);
  const count = s.peers.length;
  assert.equal(count, SHORT_LIVED_LIMIT);
  // The page comes back to the foreground, or the network says it changed:
  // before, that cut the wait short and started from 2 s again, every time.
  s.networkChanged();
  assert.equal(s.peers.length, count, "no attempt at once");
  assert.equal(s.paths.state, "waiting");
  await until(() => s.peers.length === count + 1, "the next attempt, when the wait is over");
  const waited = s.peerTimes.at(-1)! - endedAt;
  assert.ok(waited >= 120, `the wait after the third young death is the doubled one (160 ms), not cut short: ${waited}`);
  // A fourth young death: the wait doubles again, and a network change still leaves it alone.
  const fourthEnded = await dieYoung(s, 1);
  s.networkChanged();
  assert.equal(s.peers.length, count + 1, "still no attempt at once");
  await until(() => s.peers.length === count + 2, "the attempt after the fourth");
  const waited2 = s.peerTimes.at(-1)! - fourthEnded;
  assert.ok(waited2 >= 260, `and the wait doubled (320 ms): ${waited2}`);
  // Turning direct paths off and on again is the user asking: at once.
  await until(() => s.paths.state === "direct", "carrying the channel");
  s.setAllowed(false);
  s.setAllowed(true);
  assert.equal(s.peers.length, count + 3, "at once, when the user turns them back on");
});

test("a direct path that lives past STABLE_MS ends the streak: the next network change retries at once again", async (t) => {
  const s = await setUp(t, { timing: { retryFirstMs: 40, retryMaxMs: 5_000, stableMs: 150 } });
  s.paths.start();
  await dieYoung(s, SHORT_LIVED_LIMIT);
  const count = s.peers.length;
  await until(() => s.paths.state === "direct" && s.peers.length === count + 1, "the next path");
  await sleep(200);
  s.agent.dcs.at(-1)!.close();
  await until(() => s.paths.state === "waiting", "let go");
  s.networkChanged();
  assert.equal(s.peers.length, count + 2, "a new attempt at once");
});

test("two young deaths are not yet a streak: a network change still retries at once", async (t) => {
  const s = await setUp(t, { timing: { retryFirstMs: 40, retryMaxMs: 5_000, stableMs: 60_000 } });
  s.paths.start();
  await dieYoung(s, 2);
  assert.equal(s.paths.history.shortLived, 2);
  const count = s.peers.length;
  s.networkChanged();
  assert.equal(s.peers.length, count + 1, "a new attempt at once: it takes three in a row");
});

test("paths the agent ends on purpose (done, disabled) or the channel's end do not count as dying young", async (t) => {
  const s = await setUp(t, { timing: { retryFirstMs: 40, retryMaxMs: 5_000, stableMs: 60_000 } });
  s.paths.start();
  for (let k = 0; k < SHORT_LIVED_LIMIT; k++) {
    await until(() => s.paths.state === "direct" && s.agent.dcs.at(-1)?.state === "open", "a direct path carrying the channel");
    s.agent.bye("done");
    await until(() => s.paths.state === "waiting", "let go");
  }
  assert.equal(s.paths.history.shortLived, 0, "three goodbyes, no streak");
  const count = s.peers.length;
  s.networkChanged();
  assert.equal(s.peers.length, count + 1, "so a network change still retries at once");
  // A goodbye saying the path failed does count.
  await until(() => s.paths.state === "direct" && s.agent.dcs.at(-1)?.state === "open", "a direct path carrying the channel");
  s.agent.bye("failed");
  await until(() => s.paths.state === "waiting", "let go");
  assert.equal(s.paths.history.shortLived, 1);
  // And the channel ending under a young path (the driver lets it go as "closed") is no failure of the path.
  await until(() => s.paths.state === "direct" && s.agent.dcs.at(-1)?.state === "open", "a direct path carrying the channel");
  s.device.close();
  await until(() => s.paths.state === "closed", "closed with its channel");
  assert.equal(s.paths.history.shortLived, 1, "still one");
});

test("the streak outlives the channel: the next channel's direct paths wait it out", async (t) => {
  const history = new DirectHistory();
  const timing = { retryFirstMs: 40, retryMaxMs: 5_000, stableMs: 60_000 };
  const first = await setUp(t, { timing, history });
  first.paths.start();
  await dieYoung(first, SHORT_LIVED_LIMIT);
  first.paths.close();
  first.device.close();
  // The hub connection came back (or the page moved): a new channel, with its own direct paths.
  const next = await setUp(t, { timing, history });
  const startedAt = performance.now();
  next.paths.start();
  assert.equal(next.peers.length, 0, "no attempt at once on the new channel");
  assert.equal(next.paths.state, "waiting");
  await until(() => next.peers.length === 1, "an attempt once the wait is over");
  const waited = next.peerTimes[0]! - startedAt;
  assert.ok(waited >= 120, `after the inherited wait (160 ms): ${waited}`);
  await until(() => next.device.paths?.direct === true, "which carries the channel");
});

test("closing the channel closes the direct path and stops the attempts", async (t) => {
  const s = await setUp(t);
  s.paths.start();
  await until(() => s.device.paths?.direct === true, "the direct path");
  s.device.close();
  await until(() => s.paths.state === "closed", "closed");
  assert.ok(s.peers[0]!.closed);
  await sleep(80);
  assert.equal(s.peers.length, 1);
});
