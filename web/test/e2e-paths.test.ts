// Two paths under one channel (`lib/e2e/path.ts` `Lanes`, `channel.ts`
// `Session.withPaths`, `driver.ts` in paths mode): the ports of
// `droppen-proto/src/path.rs`, `channel.rs` and `driver.rs`'s tests.
//
// What these pin: sequencing hands every frame up exactly once and in order
// across switches and a direct path that dies with frames in flight (a
// property run over many interleavings); acknowledgements fall due after a
// delay or a count, and ride along with every probe answer; a session moves
// new frames to the direct path, holds
// what overtook the hub, resends over the hub what the direct path did not
// deliver, and drops the duplicates; probes measure each path and keep the
// direct one alive; frames that go unacknowledged give the direct path up
// before the silence timeout, probes notwithstanding, while a path slowly
// draining a backlog is kept; a garbled direct message lets the direct path go,
// not the channel; and two running drivers, typing into an echoing terminal,
// survive a switch to direct and its silent death, fall back at once when the
// DataChannel closes, and let the direct path go with the channel; a request in
// flight when the direct path dies (closed, or dark either way) is answered over
// the hub within a bound, and one behind a slow backlog is answered there.
import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import {
  ACK_DELAY_MS,
  ACK_EVERY,
  type Bytes,
  ChannelDriver,
  type ChannelStream,
  DEFAULT_REKEY,
  DIRECT_BUDGET,
  DIRECT_PROBE_MS,
  DIRECT_TIMEOUT_MS,
  type DirectLink,
  type DownReason,
  type Envelope,
  type Frame,
  generateDeviceKeys,
  Initiator,
  Lanes,
  MAX_AHEAD,
  MAX_HELD_BYTES,
  type MuxEvent,
  type NoiseKeyPair,
  type PathStats,
  RELAY_PROBE_MS,
  Responder,
  Session,
  type Transport,
  UNACKED_TIMEOUT_MS,
  utf8,
  type Wire,
} from "../src/lib/e2e/index.js";

const binding = { account: "a-1", device: "d-1", machine: "m-1" };

async function keys(): Promise<NoiseKeyPair> {
  const k = await generateDeviceKeys();
  return { privateKey: k.dh.privateKey, publicKey: k.dhPublic };
}

async function transports(): Promise<[Transport, Transport]> {
  const device = await keys();
  const agent = await keys();
  const i = await Initiator.create(device, agent.publicKey, binding);
  const r = await Responder.create(agent, binding);
  await r.readHello(await i.writeHello(1, utf8("")));
  const { message, transport: rt } = await r.writeReply(utf8(""), DEFAULT_REKEY, 1);
  const { transport: it } = await i.readReply(message, DEFAULT_REKEY, 1);
  return [it, rt];
}

/** A small seeded generator (xorshift32), so every interleaving can be replayed. */
function rng(seed: number): () => number {
  let x = (seed * 2654435761) >>> 0 || 1;
  return () => {
    x ^= x << 13;
    x >>>= 0;
    x ^= x >>> 17;
    x ^= x << 5;
    x >>>= 0;
    return x;
  };
}

const seqOf = (e: Envelope) => (e.type === "data" ? e.seq : -1);
const frameOf = (e: Envelope) => (e.type === "data" ? e.frame : new Uint8Array(0));
const one = (n: number) => Uint8Array.of(n) as Bytes;

test("lanes deliver in order and drop duplicates; acks free what they cover", () => {
  const tx = new Lanes();
  const rx = new Lanes();
  const frames = [0, 1, 2, 3, 4].map((i) => tx.stamp(one(i), true));
  const out: number[] = [];
  for (const i of [0, 2, 3, 1, 4, 0, 1, 2, 3, 4]) {
    for (const f of rx.receive(seqOf(frames[i]!), frameOf(frames[i]!), true, 0)) out.push(f[0]!);
  }
  assert.deepEqual(out, [0, 1, 2, 3, 4]);
  const s = rx.stats();
  assert.deepEqual([s.delivered, s.duplicates, s.held], [5, 5, 2]);
  assert.equal(rx.owed, 10);
  assert.equal(rx.ackDueMs(), ACK_DELAY_MS, "ten owed: due after the delay");
  assert.equal(rx.takeAck(), 5);
  assert.equal(rx.owed, 0);
  assert.equal(tx.unackedBytes, 5);
  tx.onAck(3);
  assert.equal(tx.unackedBytes, 2);
  tx.onAck(1);
  assert.equal(tx.unackedBytes, 2, "an old ack is moot");
  assert.throws(() => tx.onAck(6), "an ack for the future");
  assert.deepEqual(tx.takeUnacked().map(seqOf), [4, 5]);
  assert.equal(tx.unackedBytes, 0);
  assert.equal(tx.stats().resent, 2);
  const r = new Lanes();
  assert.throws(() => r.receive(0, one(0), false, 0));
  assert.throws(() => r.receive(2 + MAX_AHEAD, one(0), false, 0));
  const full = new Lanes();
  assert.deepEqual(full.receive(3, new Uint8Array(MAX_HELD_BYTES) as Bytes, false, 0), []);
  assert.throws(() => full.receive(4, one(0), false, 0), "too much held");
});

test("acks fall due after a delay or a count; frames over the hub owe nothing", () => {
  const rx = new Lanes();
  assert.equal(rx.ackDueMs(), null);
  rx.receive(1, one(0), true, 100);
  assert.equal(rx.ackDueMs(), 100 + ACK_DELAY_MS);
  for (let seq = 2; seq <= ACK_EVERY; seq++) rx.receive(seq, one(0), true, 105);
  assert.equal(rx.ackDueMs(), 100, "at once once enough are owed");
  const hub = new Lanes();
  hub.receive(1, one(0), false, 0);
  assert.equal(hub.ackDueMs(), null);
});

test("sequencing survives switches and a dying direct path (200 interleavings)", () => {
  for (let seed = 0; seed < 200; seed++) {
    const r = rng(seed);
    const tx = new Lanes();
    const rx = new Lanes();
    let relay: Envelope[] = [];
    let direct: Envelope[] = [];
    let onDirect = false;
    const delivered: number[] = [];
    const total = 300;
    let sent = 0;
    let steps = 0;
    const take = (frames: Bytes[]) => {
      for (const f of frames) delivered.push(new DataView(f.buffer, f.byteOffset, 4).getUint32(0));
    };
    while ((sent < total || relay.length > 0 || direct.length > 0 || tx.unackedBytes > 0) && steps < 100_000) {
      steps++;
      const roll = r() % 10;
      if (roll <= 2 && sent < total) {
        const frame = new Uint8Array(4) as Bytes;
        new DataView(frame.buffer).setUint32(0, sent);
        (onDirect ? direct : relay).push(tx.stamp(frame, onDirect));
        sent++;
      } else if (roll === 3 && !onDirect && r() % 4 === 0) {
        onDirect = true;
      } else if (roll === 4 && onDirect && r() % 6 === 0) {
        // The direct path dies: what is in flight on it is lost, and the
        // sender resends what was not acknowledged over the hub. What was
        // not lost still arrives; the hub copies are duplicates.
        const lost = r() % (direct.length + 1);
        direct = direct.slice(0, direct.length - lost);
        onDirect = false;
        relay = [...relay, ...tx.takeUnacked()];
      } else if (roll >= 5 && roll <= 7) {
        const e = direct.shift();
        if (e) take(rx.receive(seqOf(e), frameOf(e), true, 0));
      } else if (roll === 8) {
        const e = relay.shift();
        if (e) take(rx.receive(seqOf(e), frameOf(e), false, 0));
      } else if (rx.owed > 0 || (sent === total && relay.length === 0 && direct.length === 0)) {
        tx.onAck(rx.takeAck());
      }
    }
    assert.deepEqual(
      delivered,
      Array.from({ length: total }, (_, i) => i),
      `seed ${seed}`,
    );
  }
});

// ---------------------------------------------------------------------------
// Two sessions, sans-IO (channel.rs's tests)
// ---------------------------------------------------------------------------

class Pair {
  relay: [Frame[], Frame[]] = [[], []];
  direct: [Uint8Array[], Uint8Array[]] = [[], []];
  got: [number[], number[]] = [[], []];
  now = 0;
  constructor(
    readonly a: Session,
    readonly b: Session,
  ) {}

  static async create(): Promise<Pair> {
    const [it, rt] = await transports();
    return new Pair(Session.withPaths(1, it, "initiator", { probeRelay: true }, 0), Session.withPaths(1, rt, "responder", { probeRelay: false }, 0));
  }

  async attach(): Promise<void> {
    const [it, rt] = await transports();
    this.a.attachDirect(it, this.now);
    this.b.attachDirect(rt, this.now);
  }

  side(i: number): Session {
    return i === 0 ? this.a : this.b;
  }

  async sendAll(): Promise<void> {
    for (const i of [0, 1]) {
      for (let p = await this.side(i).poll({ relay: true, direct: true }, this.now, 1); p; p = await this.side(i).poll({ relay: true, direct: true }, this.now, 1)) {
        const w: Wire = p.wire;
        if (w.type === "relay") this.relay[i]!.push(w.frame);
        else this.direct[i]!.push(w.message);
      }
    }
  }

  take(to: number, events: MuxEvent[]): void {
    for (const e of events) if (e.type === "data") this.got[to]!.push(...e.data);
  }

  async deliver(from: number, via: "relay" | "direct"): Promise<boolean> {
    const to = 1 - from;
    if (via === "relay") {
      const f = this.relay[from]!.shift();
      if (!f || f.type !== "data") return false;
      this.take(to, await this.side(to).receiveRelay(f.payload, this.now));
    } else {
      const m = this.direct[from]!.shift();
      if (!m) return false;
      this.take(to, await this.side(to).receiveDirect(m, this.now));
    }
    return true;
  }

  async drain(from: number, via: "relay" | "direct"): Promise<void> {
    while (await this.deliver(from, via)) {
      // until empty
    }
  }
}

async function openStream(p: Pair): Promise<number> {
  const sid = p.a.mux.open("term", utf8("t"));
  await p.sendAll();
  await p.drain(0, "relay");
  return sid;
}

async function typeKeys(p: Pair, sid: number, typed: number[], n: number): Promise<void> {
  for (let k = 0; k < n; k++) {
    const key = typed.length & 0xff;
    p.a.mux.send(sid, Uint8Array.of(key));
    typed.push(key);
    await p.sendAll();
  }
}

test("keystrokes cross a switch to direct and a fallback, in order", async () => {
  const p = await Pair.create();
  const sid = await openStream(p);
  const typed: number[] = [];
  await typeKeys(p, sid, typed, 20);
  assert.equal(p.direct[0].length, 0);
  // The direct path comes up while those 20 are still on the hub.
  await p.attach();
  await typeKeys(p, sid, typed, 30);
  assert.deepEqual([p.relay[0].length, p.direct[0].length], [20, 30]);
  // The direct path is faster: everything on it arrives first, and is held.
  await p.drain(0, "direct");
  assert.equal(p.got[1].length, 0, "nothing is handed up ahead of the gap");
  await p.drain(0, "relay");
  assert.deepEqual(p.got[1], typed, "in order once the hub's catch up");
  // Answers flow back over the direct path, acknowledgements with them.
  p.b.mux.send(sid, utf8("echo"));
  await p.sendAll();
  await p.drain(1, "direct");
  assert.equal(new TextDecoder().decode(Uint8Array.from(p.got[0])), "echo");

  // Mid-typing the direct path dies: what is in flight on it is lost.
  await typeKeys(p, sid, typed, 15);
  assert.equal(p.direct[0].length, 15);
  p.direct[0] = [];
  p.now += DIRECT_TIMEOUT_MS + 1;
  p.a.tick(p.now);
  assert.ok(!p.a.directActive, "a noticed by the timeout");
  await typeKeys(p, sid, typed, 5);
  // b hears a `down` over the hub first, then the resends, then the new keys.
  await p.drain(0, "relay");
  assert.ok(!p.b.directActive, "b stopped using it when told");
  assert.deepEqual(p.got[1], typed, "nothing lost, nothing twice, in order");
  assert.equal(p.b.stats().lanes.duplicates, 0);
  assert.equal(p.a.stats().lanes.resent, 15);
  assert.deepEqual([p.a.stats().ups, p.a.stats().downs], [1, 1]);
  const events = p.b.takeEvents();
  assert.ok(events.some((e) => e.type === "directUp"));
  assert.ok(events.some((e) => e.type === "directDown" && e.reason === "timeout" && e.remote));
});

test("frames the direct path did deliver are not handed up twice", async () => {
  const p = await Pair.create();
  const sid = await openStream(p);
  await p.attach();
  for (let k = 0; k < 10; k++) p.a.mux.send(sid, Uint8Array.of(k));
  await p.sendAll();
  // All of them arrive directly, but no acknowledgement makes it back.
  await p.drain(0, "direct");
  await p.sendAll();
  p.direct[1] = [];
  // b's DataChannel closes; a hears it over the hub and resends all ten.
  p.b.detachDirect("failed", true);
  await p.sendAll();
  await p.drain(1, "relay");
  assert.ok(!p.a.directActive);
  await p.sendAll();
  await p.drain(0, "relay");
  assert.deepEqual(p.got[1], [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
  assert.equal(p.b.stats().lanes.duplicates, 10, "the resends were recognised and dropped");
});

test("probes measure each path and keep the direct one alive", async () => {
  const p = await Pair.create();
  await openStream(p);
  await p.attach();
  p.now = 1;
  p.a.tick(p.now);
  await p.sendAll();
  p.now = 3.5;
  await p.drain(0, "direct");
  await p.drain(0, "relay");
  await p.sendAll();
  p.now = 4;
  await p.drain(1, "direct");
  await p.drain(1, "relay");
  const s = p.a.stats();
  assert.ok(s.direct);
  assert.equal(s.directRttMs, 3);
  assert.equal(s.relayRttMs, 3);
  assert.equal(p.b.stats().relayRttMs, null, "b probes only the direct path");
  for (let step = 1; step <= 10; step++) {
    p.now = 4 + step * DIRECT_PROBE_MS;
    p.a.tick(p.now);
    p.b.tick(p.now);
    for (let round = 0; round < 2; round++) {
      await p.sendAll();
      while ((await p.deliver(0, "direct")) || (await p.deliver(1, "direct"))) {
        // both ways
      }
    }
  }
  assert.ok(p.a.directActive && p.b.directActive, "probes kept it alive past the timeout");
  assert.notEqual(p.a.nextDeadlineMs(), null);
});

test("a page that slept through its timers does not read its old probes as a silent path", async () => {
  const p = await Pair.create();
  await openStream(p);
  // A probe answered in 3 ms before the page went away.
  p.a.tick(p.now);
  await p.sendAll();
  p.now = 3;
  await p.drain(0, "relay");
  await p.sendAll();
  await p.drain(1, "relay");
  assert.equal(p.a.stats().relayRttMs, 3);
  // Ten minutes asleep: the probe sent before the pause was never read.
  p.now = 10 * 60_000;
  p.a.tick(p.now);
  assert.equal(p.a.stats().relayRttMs, 3, "the sleep is not the path's silence");
  // The fresh probe is measured on its own.
  await p.sendAll();
  p.now += 3;
  await p.drain(0, "relay");
  await p.sendAll();
  await p.drain(1, "relay");
  assert.equal(p.a.stats().relayRttMs, 3, "measured afresh after the pause");
});

test("a hub path that stops answering its probes shows ever slower, never its last good round trip", async () => {
  const p = await Pair.create();
  await openStream(p);
  // The first probe goes at once, and comes back in 3 ms.
  p.a.tick(p.now);
  await p.sendAll();
  p.now = 3;
  await p.drain(0, "relay");
  await p.sendAll();
  await p.drain(1, "relay");
  assert.equal(p.a.stats().relayRttMs, 3);
  // Then the hub stops carrying anything (a connection dead without a word):
  // the probes queue up, unanswered.
  for (const at of [RELAY_PROBE_MS, 2 * RELAY_PROBE_MS, 3 * RELAY_PROBE_MS]) {
    p.now = at;
    p.a.tick(p.now);
    await p.sendAll();
  }
  assert.equal(p.a.stats().relayRttMs, 2 * RELAY_PROBE_MS, "the oldest unanswered probe's age, not 3 ms");
  // When the hub carries again, the answers are round trips again.
  p.now = 4 * RELAY_PROBE_MS;
  p.a.tick(p.now);
  await p.sendAll();
  await p.drain(0, "relay");
  await p.sendAll();
  await p.drain(1, "relay");
  assert.equal(p.a.stats().relayRttMs, 0, "measured afresh: the latest probe was answered at once");
});

test("an acknowledgement that frees nothing sent on the direct path is no progress on it", async () => {
  const p = await Pair.create();
  const sid = await openStream(p);
  // Only a has the direct path (b has not attached it yet, or already let it go): b answers over the hub.
  const [it] = await transports();
  p.a.attachDirect(it, p.now);
  p.now = 1_000;
  const sent = p.now;
  p.a.mux.send(sid, utf8("k"));
  await p.sendAll();
  p.direct[0] = []; // The keystroke was lost on the direct path.
  // b keeps talking over the hub: every frame carries an acknowledgement, none of which covers the keystroke.
  for (let at = sent + 200; at < sent + UNACKED_TIMEOUT_MS - 100; at += 200) {
    p.now = at;
    p.b.mux.send(sid, utf8("."));
    await p.sendAll();
    await p.drain(1, "relay");
    p.a.tick(p.now);
  }
  assert.ok(p.a.directActive, "not yet");
  p.now = sent + UNACKED_TIMEOUT_MS + 1;
  p.a.tick(p.now);
  assert.ok(!p.a.directActive, "given up on time: acknowledgements that free nothing sent there did not keep it");
});

test("frames that go unacknowledged give the direct path up before the silence timeout", async () => {
  const p = await Pair.create();
  const sid = await openStream(p);
  await p.attach();
  p.now = 1_000;
  const sent = p.now;
  p.a.mux.send(sid, utf8("k"));
  await p.sendAll();
  p.direct[0] = []; // The keystroke was lost and is never acknowledged…
  const deadline = p.a.nextDeadlineMs()!;
  assert.ok(deadline <= p.now + UNACKED_TIMEOUT_MS + 1, "a timer for it");
  // …while probes still come and go (the path is not silent): they say the
  // peer is there, not that the keystroke got through.
  p.now += 700;
  p.a.tick(p.now);
  p.b.tick(p.now);
  for (let round = 0; round < 2; round++) {
    await p.sendAll();
    while ((await p.deliver(0, "direct")) || (await p.deliver(1, "direct"))) {
      // both ways
    }
  }
  p.now = sent + UNACKED_TIMEOUT_MS - 100;
  p.a.tick(p.now);
  assert.ok(p.a.directActive, "not yet");
  p.now += 200;
  p.a.tick(p.now);
  assert.ok(!p.a.directActive, `given up after ${UNACKED_TIMEOUT_MS} ms, not ${DIRECT_TIMEOUT_MS}`);
  await p.sendAll();
  await p.drain(0, "relay");
  assert.equal(new TextDecoder().decode(Uint8Array.from(p.got[1])), "k", "and resent over the hub");
});

test("a direct path that drains a backlog slowly is kept, both ways, however long the backlog takes", async () => {
  const p = await Pair.create();
  const sid = await openStream(p);
  await p.attach();
  // b (the agent) has 40 chunks for a, and the direct path hands a one every
  // 100 ms: four seconds of backlog, far past UNACKED_TIMEOUT_MS — a slow
  // cellular link, with the DataChannel's buffer filled faster than it drains.
  const chunks = 40;
  for (let k = 0; k < chunks; k++) p.b.mux.send(sid, new Uint8Array(512).fill(k));
  await p.sendAll();
  const stepMs = 100;
  for (let step = 1; step * stepMs <= 6_000; step++) {
    p.now = step * stepMs;
    // a types while the backlog drains: b's acknowledgement of it can only
    // come behind the backlog, seconds later.
    if (step === 2) p.a.mux.send(sid, utf8("k"));
    p.a.tick(p.now);
    p.b.tick(p.now);
    await p.sendAll();
    await p.deliver(1, "direct"); // b → a: one message per step
    await p.drain(0, "direct"); // a → b: at once
    await p.sendAll();
    assert.ok(p.a.directActive && p.b.directActive, `still direct at ${p.now} ms (${JSON.stringify([p.a.stats(), p.b.stats()])})`);
  }
  assert.equal(p.direct[1].length, 0, "the backlog drained");
  const expected = Array.from({ length: chunks }, (_, k) => Array<number>(512).fill(k)).flat();
  assert.deepEqual(p.got[0], expected, "every chunk, once, in order");
  assert.equal(new TextDecoder().decode(Uint8Array.from(p.got[1])), "k");
  for (const side of [p.a, p.b]) {
    const s = side.stats();
    assert.deepEqual([s.downs, s.lanes.resent], [0, 0], "nothing given up, nothing sent twice");
  }
});

test("a probe is answered with any acknowledgement owed, on the path it came on", async () => {
  const p = await Pair.create();
  const sid = await openStream(p);
  await p.attach();
  p.a.mux.send(sid, utf8("k"));
  await p.sendAll();
  await p.drain(0, "direct");
  assert.equal(p.b.stats().lanes.delivered > 0, true);
  // b owes an acknowledgement but its timers are held (a background tab): no tick, no poll yet.
  p.a.tick(p.now);
  await p.sendAll();
  assert.equal(p.direct[0].length, 1, "a's probe");
  await p.drain(0, "direct");
  // b answers the probe — and acknowledges with it, before its ack delay.
  for (let q = await p.b.poll({ relay: true, direct: true }, p.now, 1); q; q = await p.b.poll({ relay: true, direct: true }, p.now, 1)) {
    if (q.wire.type === "direct") p.direct[1].push(q.wire.message);
  }
  assert.equal(p.direct[1].length, 2, "the probe's answer, then the acknowledgement");
  await p.drain(1, "direct");
  assert.equal(p.a.nextDeadlineMs()! >= p.now + 1, true);
  p.now += UNACKED_TIMEOUT_MS + 1;
  p.a.tick(p.now);
  assert.ok(p.a.directActive, "nothing waits for an acknowledgement any more");
  assert.notEqual(p.a.stats().directRttMs, null);
});

test("a garbled direct message drops the direct path, not the channel", async () => {
  const p = await Pair.create();
  const sid = await openStream(p);
  await p.attach();
  p.a.mux.send(sid, utf8("x"));
  await p.sendAll();
  const m = Uint8Array.from(p.direct[0].shift()!);
  m[m.length - 1]! ^= 1;
  assert.deepEqual(await p.b.receiveDirect(m, 0), []);
  assert.ok(!p.b.directActive);
  // b told a over the hub; a resends "x" there.
  await p.sendAll();
  await p.drain(1, "relay");
  await p.sendAll();
  await p.drain(0, "relay");
  assert.equal(new TextDecoder().decode(Uint8Array.from(p.got[1])), "x");
});

test("a session without paths refuses a direct path and speaks plain frames", async () => {
  const [it, rt] = await transports();
  const a = new Session(1, it, "initiator");
  const b = new Session(1, rt, "responder");
  const [d1] = await transports();
  assert.throws(() => a.attachDirect(d1, 0));
  const sid = a.mux.open("rpc", utf8("GET /"));
  a.mux.send(sid, utf8("hi"));
  const got: unknown[] = [];
  for (const f of await a.flush(0)) if (f.type === "data") got.push(await b.receive(f.payload));
  assert.equal(got.length, 2);
  assert.equal(a.nextDeadlineMs(), null);
});

// ---------------------------------------------------------------------------
// Two running drivers (driver.rs's tests)
// ---------------------------------------------------------------------------

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** Two drivers with paths, back to back through a "hub" that delays every frame by `delayMs`, in order; closed when the test ends. */
async function wirePaths(t: TestContext, delayMs: number) {
  const [it, rt] = await transports();
  let device!: ChannelDriver;
  let agent!: ChannelDriver;
  const hub = (to: () => ChannelDriver) => (f: Frame) => setTimeout(() => to().receive(f), delayMs);
  device = new ChannelDriver(1, it, "initiator", hub(() => agent), Date.now, { paths: { probeRelay: true } });
  agent = new ChannelDriver(1, rt, "responder", hub(() => device), Date.now, { paths: { probeRelay: false } });
  t.after(() => {
    device.close();
    agent.close();
  });
  return { device, agent };
}

/**
 * A DataChannel between two drivers: `blackhole` drops everything silently
 * (a network that stopped carrying UDP); either driver letting its end go
 * closes the other's (a peer connection torn down), and `released` says
 * which let go, and why. `blind` counts messages a driver handed its end
 * without having just seen room for them (`bufferedAmount` under
 * `DIRECT_BUDGET`, read since its last message): the driver checks before
 * each one, so this stays 0 whatever was in flight when the buffer filled.
 *
 * `dark[i]` drops only what end `i` (0: the device, 1: the agent) sends — a
 * path dead one way. `spacingMs[i]` hands what end `i` sends to the other one
 * message per that many ms, in order: a link slower than what is put on it,
 * so a backlog builds in the DataChannel's buffer and drains at the link's pace.
 */
function fakeDc(device: ChannelDriver, agent: ChannelDriver, opts: { spacingMs?: [number, number] } = {}) {
  const state = {
    blackhole: false,
    dark: [false, false],
    closed: false,
    released: [false, false],
    reasons: [null, null] as (DownReason | null)[],
    buffered: 0,
    sent: 0,
    blind: 0,
  };
  const drivers = [device, agent];
  /** Whether each end's latest `bufferedAmount` reading, since its last message, left room for one. */
  const room = [false, false];
  /** What each end sent on a spaced link, not yet handed over. */
  const backlog: Uint8Array[][] = [[], []];
  const draining = [false, false];
  const drain = (i: number) => {
    const next = backlog[i]!.shift();
    if (!next || state.closed) {
      draining[i] = false;
      backlog[i] = [];
      return;
    }
    drivers[1 - i]!.receiveDirect(next, links[1 - i]);
    setTimeout(() => drain(i), opts.spacingMs![i]);
  };
  const links: DirectLink[] = [0, 1].map((i) => ({
    send(message: Bytes) {
      if (state.closed) throw new Error("the data channel is closed");
      if (!room[i]) state.blind++;
      room[i] = false;
      state.sent++;
      if (state.blackhole || state.dark[i]) return;
      const copy = Uint8Array.from(message);
      const spacing = opts.spacingMs?.[i] ?? 0;
      if (spacing > 0) {
        backlog[i]!.push(copy);
        if (!draining[i]) {
          draining[i] = true;
          setTimeout(() => drain(i), spacing);
        }
        return;
      }
      setTimeout(() => drivers[1 - i]!.receiveDirect(copy, links[1 - i]), 0);
    },
    bufferedAmount: () => {
      const buffered = i === 0 ? state.buffered : 0;
      room[i] = buffered < DIRECT_BUDGET;
      return buffered;
    },
    released(reason) {
      state.released[i] = true;
      state.reasons[i] = reason;
      if (state.closed) return;
      state.closed = true;
      // The other end sees its DataChannel close.
      setTimeout(() => drivers[1 - i]!.detachDirect("failed", links[1 - i]), 0);
    },
  }));
  return { state, deviceLink: links[0]!, agentLink: links[1]! };
}

/** Wait for `cond`; `timeoutMs` only keeps a broken test from hanging (it is no claim about how fast). */
async function until(cond: () => boolean, what: string, timeoutMs = 10_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!cond()) {
    if (Date.now() > deadline) throw new Error(`timed out: ${what}`);
    await sleep(5);
  }
}

async function waitFor(driver: ChannelDriver, what: (s: PathStats) => boolean, timeoutMs = 10_000): Promise<PathStats> {
  const until = Date.now() + timeoutMs;
  for (;;) {
    const s = driver.paths!;
    if (what(s)) return s;
    if (Date.now() > until) throw new Error(`the paths did not get there: ${JSON.stringify(s)}`);
    await sleep(2);
  }
}

/** An echoing terminal on the agent's side; the device's reader collects what comes back. */
function echoTerminal(agent: ChannelDriver, term: ChannelStream): { echoed: number[]; done: (n: number, timeoutMs: number) => Promise<number[]> } {
  agent.onincoming = (stream) => {
    stream.ondata = (data) => stream.send(data);
  };
  const echoed: number[] = [];
  term.ondata = (data) => echoed.push(...data);
  term.onclose = (code) => {
    throw new Error(`the terminal closed (${code}) after ${echoed.length} bytes`);
  };
  return {
    echoed,
    async done(n, timeoutMs) {
      const until = Date.now() + timeoutMs;
      while (echoed.length < n) {
        if (Date.now() > until) throw new Error(`only ${echoed.length} of ${n} echoed`);
        await sleep(5);
      }
      return echoed;
    },
  };
}

async function typeInto(term: ChannelStream, from: number, to: number): Promise<void> {
  for (let k = from; k < to; k++) {
    term.send(Uint8Array.of(k));
    await sleep(2);
  }
}

const range = (n: number) => Array.from({ length: n }, (_, i) => i);

test("typing survives a switch to direct and its silent death", async (t) => {
  // The hub's delay is what tells the paths' round trips apart below: it is
  // long enough that scheduling under a loaded machine (tens of ms) cannot
  // make the direct path look the slower one.
  const hubDelayMs = 50;
  const { device, agent } = await wirePaths(t, hubDelayMs);
  const term = device.open("term", utf8("t-1"));
  const reader = echoTerminal(agent, term);
  await typeInto(term, 0, 30);

  const dc = fakeDc(device, agent);
  const [dt, at] = await transports();
  agent.attachDirect(at, dc.agentLink);
  device.attachDirect(dt, dc.deviceLink);
  await waitFor(device, (s) => s.direct);
  await typeInto(term, 30, 60);
  const up = await waitFor(device, (s) => s.directRttMs !== null && s.relayRttMs !== null);
  // The hub adds its delay each way; the direct path adds nothing.
  assert.ok(up.directRttMs! < up.relayRttMs!, `the direct path is the fast one: ${JSON.stringify(up)}`);

  // The network stops carrying the direct path; nobody says a word.
  dc.state.blackhole = true;
  await typeInto(term, 60, 120);
  assert.deepEqual(await reader.done(120, 20_000), range(120), "every key, once, in order");
  const d = await waitFor(device, (s) => !s.direct);
  const a = await waitFor(agent, (s) => !s.direct);
  assert.deepEqual([d.ups, d.downs], [1, 1]);
  assert.ok(d.resent + a.resent > 0, `something was resent over the hub: ${JSON.stringify([d, a])}`);
  // Both let their links go, which tears the peer connection down.
  await until(() => dc.state.released[0]! && dc.state.released[1]!, "both links let go");
  assert.deepEqual(dc.state.released, [true, true]);
  assert.equal(dc.state.blind, 0);
});

test("a closed DataChannel falls back at once, and nothing is lost", async (t) => {
  const { device, agent } = await wirePaths(t, 2);
  const term = device.open("term", utf8("t-1"));
  const reader = echoTerminal(agent, term);
  const dc = fakeDc(device, agent);
  const [dt, at] = await transports();
  agent.attachDirect(at, dc.agentLink);
  device.attachDirect(dt, dc.deviceLink);
  await waitFor(device, (s) => s.direct);
  await typeInto(term, 0, 30);
  agent.detachDirect("failed");
  await waitFor(device, (s) => !s.direct);
  // At once: on the close itself, not on a timeout (an unacknowledged frame
  // or the path's silence) that would have given it up later anyway.
  assert.deepEqual(dc.state.reasons, ["failed", "failed"], "both let go because the DataChannel failed, neither on a timeout");
  await typeInto(term, 30, 60);
  assert.deepEqual(await reader.done(60, 10_000), range(60));
});

test("closing a channel closes its direct path", async (t) => {
  const { device, agent } = await wirePaths(t, 1);
  const dc = fakeDc(device, agent);
  const [dt, at] = await transports();
  agent.attachDirect(at, dc.agentLink);
  device.attachDirect(dt, dc.deviceLink);
  await waitFor(device, (s) => s.direct);
  agent.close(4403, "this device was removed from the account");
  // The links go at once; the hub's close reaches the device a moment later.
  await until(() => dc.state.released[0]! && dc.state.released[1]! && device.isClosed, "both links let go and the device's channel closed");
  assert.deepEqual(dc.state.released, [true, true], "both let their direct links go");
  assert.ok(device.isClosed, "and the device's channel closed with the agent's");
});

test("the direct path waits while its DataChannel holds the budget, and wakes when it drains", async (t) => {
  const { device, agent } = await wirePaths(t, 1);
  const term = device.open("term", utf8("t-1"));
  const reader = echoTerminal(agent, term);
  const dc = fakeDc(device, agent);
  const [dt, at] = await transports();
  agent.attachDirect(at, dc.agentLink);
  device.attachDirect(dt, dc.deviceLink);
  await waitFor(device, (s) => s.direct);
  // Whatever the drivers still have on its way (the attach's probes and
  // their answers) may go on; nothing new is sealed for a full DataChannel.
  dc.state.buffered = DIRECT_BUDGET;
  await typeInto(term, 0, 10);
  await sleep(20);
  assert.equal(dc.state.blind, 0, "nothing handed to a full DataChannel");
  assert.equal(reader.echoed.length, 0, "and nothing went the other way round it");
  dc.state.buffered = 0;
  device.wake();
  assert.deepEqual(await reader.done(10, 10_000), range(10));
  assert.ok(device.paths!.direct);
  assert.equal(dc.state.blind, 0);
});

/** The agent answers every `rpc` stream's first message with `re:` and that message, and ends the stream. */
function rpcAgent(agent: ChannelDriver): void {
  agent.onincoming = (stream) => {
    stream.ondata = (data) => {
      stream.send(new Uint8Array([...utf8("re:"), ...data]));
      stream.close(0);
    };
  };
}

/** Send one request on a fresh `rpc` stream; resolves with the answer and how long it took. */
function rpc(device: ChannelDriver, body: string): Promise<{ answer: string; ms: number }> {
  const stream = device.open("rpc", utf8("POST /api/sessions/claude/s-1/message"));
  const started = Date.now();
  return new Promise((resolve, reject) => {
    const got: number[] = [];
    stream.ondata = (data) => got.push(...data);
    stream.onclose = (code) => {
      if (code !== 0) reject(new Error(`the request's stream closed with ${code}`));
      else resolve({ answer: new TextDecoder().decode(Uint8Array.from(got)), ms: Date.now() - started });
    };
    stream.send(utf8(body));
  });
}

// What happens to a request (a sent message, `POST …/message`) that is on the
// direct path when it dies: it goes again over the hub, and the answer comes
// back there — at once when the DataChannel says it closed, and within
// UNACKED_TIMEOUT_MS when the path goes dark without a word, whichever way.
for (const [how, kill, boundMs] of [
  ["the DataChannel closes", (dc: ReturnType<typeof fakeDc>, agent: ChannelDriver) => agent.detachDirect("failed"), 500],
  ["it goes dark both ways", (dc: ReturnType<typeof fakeDc>) => (dc.state.blackhole = true), UNACKED_TIMEOUT_MS + 1_000],
  ["it goes dark from the device to the agent", (dc: ReturnType<typeof fakeDc>) => (dc.state.dark[0] = true), UNACKED_TIMEOUT_MS + 1_000],
  ["it goes dark from the agent to the device", (dc: ReturnType<typeof fakeDc>) => (dc.state.dark[1] = true), UNACKED_TIMEOUT_MS + 1_000],
] as const) {
  test(`a request in flight when ${how} is answered over the hub within ${boundMs} ms`, async (t) => {
    const hubDelayMs = 20;
    const { device, agent } = await wirePaths(t, hubDelayMs);
    rpcAgent(agent);
    const dc = fakeDc(device, agent);
    const [dt, at] = await transports();
    agent.attachDirect(at, dc.agentLink);
    device.attachDirect(dt, dc.deviceLink);
    await waitFor(device, (s) => s.direct && s.directRttMs !== null);
    const before = await rpc(device, "hello");
    assert.equal(before.answer, "re:hello");
    assert.ok(device.paths!.direct, "answered over the direct path");

    const pending = rpc(device, "are you there");
    kill(dc, agent);
    const { answer, ms } = await pending;
    assert.equal(answer, "re:are you there", "answered once, whole");
    assert.ok(ms < boundMs, `answered in ${ms} ms, not within ${boundMs}`);
    const d = await waitFor(device, (s) => !s.direct);
    await waitFor(agent, (s) => !s.direct);
    assert.equal(d.downs, 1);
  });
}

test("a request on a direct path slowly draining a backlog is answered there, and the path is kept", async (t) => {
  const { device, agent } = await wirePaths(t, 20);
  // The agent → device way carries one message per 100 ms (a cellular
  // downlink slower than what is put on it); the way back is quick.
  const dc = fakeDc(device, agent, { spacingMs: [0, 100] });
  const chunks = 25;
  agent.onincoming = (stream) => {
    if (stream.cls === "mux") {
      // A feed: as soon as it opens, a burst far bigger than the link carries in UNACKED_TIMEOUT_MS.
      for (let k = 0; k < chunks; k++) stream.send(new Uint8Array(1024).fill(k));
      return;
    }
    stream.ondata = (data) => {
      stream.send(new Uint8Array([...utf8("re:"), ...data]));
      stream.close(0);
    };
  };
  const [dt, at] = await transports();
  agent.attachDirect(at, dc.agentLink);
  device.attachDirect(dt, dc.deviceLink);
  await waitFor(device, (s) => s.direct);
  const feed = device.open("mux", utf8("GET /api/events"));
  let fed = 0;
  feed.ondata = (data) => (fed += data.length);
  await sleep(200);
  // A message sent mid-burst: its acknowledgement, and its answer, queue behind the burst.
  const { answer, ms } = await rpc(device, "sent mid-burst");
  assert.equal(answer, "re:sent mid-burst");
  await until(() => fed === chunks * 1024, "the whole burst", 15_000);
  for (const [who, driver] of [["device", device], ["agent", agent]] as const) {
    const s = driver.paths!;
    assert.ok(s.direct, `the ${who} still uses the direct path: ${JSON.stringify(s)}`);
    assert.deepEqual([s.downs, s.resent], [0, 0], `the ${who} gave nothing up and sent nothing twice: ${JSON.stringify(s)}`);
  }
  assert.deepEqual(dc.state.released, [false, false], "nobody let the DataChannel go");
  assert.ok(ms > UNACKED_TIMEOUT_MS, `the answer did wait behind the burst, longer than UNACKED_TIMEOUT_MS (${ms} ms)`);
});

test("a direct link offered to a channel without paths is released at once", async () => {
  const [it, rt] = await transports();
  const plain = new ChannelDriver(1, it, "initiator", () => undefined);
  assert.equal(plain.paths, null);
  assert.equal(plain.hasPaths, false);
  let released: string | null = null;
  plain.attachDirect(rt, { send: () => undefined, bufferedAmount: () => 0, released: (r) => (released = r) });
  await sleep(5);
  assert.equal(released, "closed");
  plain.close();
});
