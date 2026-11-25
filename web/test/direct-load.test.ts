// The direct path's deadlines under load (`lib/e2e/path.ts` `directDeadline`,
// `channel.ts`): two sessions, sans-IO, over an access link both paths share,
// on a clock the test moves. The ports of `droppen-proto/src/channel.rs`'s
// `load` tests.
//
// What these pin (a field report: a 1 Mbit/s link, the direct path's round
// trip measured idle at 41–82 ms, given up after 1.5 s "without progress"
// again and again while it was only full): a direct path carrying 600 KB at
// 1 Mbit/s keeps it as long as acknowledgements keep coming, however far
// apart — when the link is shared with other traffic, when it stalls for two
// seconds the way SCTP waits out a lost packet, and right after a switch,
// when what the hub path still had on the link goes first; a path that
// carries nothing much is still given up on the floors (1.5 s with a
// keystroke waiting, 3.5 s of silence); one that dies under load is given up
// within `DIRECT_TIMEOUT_MAX_MS`, and what it did not deliver comes over the
// hub path, once and in order; and the deadline's inputs ride on the event.
import assert from "node:assert/strict";
import test from "node:test";
import {
  DeadlineRate,
  decayedRate,
  DEFAULT_REKEY,
  DIRECT_BUDGET,
  DIRECT_TIMEOUT_MAX_MS,
  DIRECT_TIMEOUT_MS,
  directDeadline,
  generateDeviceKeys,
  INITIAL_CREDIT,
  Initiator,
  MAX_PAYLOAD_LEN,
  type MuxEvent,
  type NoiseKeyPair,
  type PathEvent,
  RATE_DECAY_MS,
  RELAY_BULK_AHEAD_MAX_MS,
  Responder,
  Session,
  type Transport,
  UNACKED_TIMEOUT_MS,
  utf8,
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

/** How far the clock moves at a time (ms). */
const STEP_MS = 10;
/** 1 Mbit/s. */
const MBIT = 125;
/** What the WebSocket or SCTP, DTLS, UDP and IP add to a message. */
const OVERHEAD = 60;

type Via = "relay" | "direct";

/**
 * One direction of the access link both paths share: one FIFO, `bytesPerMs`
 * through it (nothing while `stalled`), then `delayMs` on the way. A message
 * arrives whole once its last byte is through. Traffic this channel does not
 * carry (the page's own fetches) is what it takes of `bytesPerMs`.
 */
class Lane {
  private readonly queue: { via: Via; left: number; len: number; deliver: (() => Promise<void>) | null }[] = [];
  private readonly flying: { at: number; via: Via; deliver: () => Promise<void> }[] = [];
  stalled = false;
  readonly cut: Record<Via, boolean> = { relay: false, direct: false };

  constructor(
    public bytesPerMs: number,
    readonly delayMs: number,
  ) {}

  put(via: Via, len: number, deliver: (() => Promise<void>) | null): void {
    if (this.cut[via]) return;
    this.queue.push({ via, left: len + OVERHEAD, len, deliver });
  }

  /** The link carries one step's worth. */
  step(nowMs: number): void {
    if (this.stalled) return;
    let budget = this.bytesPerMs * STEP_MS;
    while (budget > 0 && this.queue.length > 0) {
      const head = this.queue[0]!;
      const take = Math.min(budget, head.left);
      head.left -= take;
      budget -= take;
      if (head.left > 0) break;
      this.queue.shift();
      if (head.deliver) this.flying.push({ at: nowMs + this.delayMs, via: head.via, deliver: head.deliver });
    }
  }

  /** What has arrived by `nowMs`, in order. */
  due(nowMs: number): (() => Promise<void>)[] {
    const out: (() => Promise<void>)[] = [];
    while (this.flying[0] && this.flying[0].at <= nowMs) out.push(this.flying.shift()!.deliver);
    return out;
  }

  /** A DataChannel's `bufferedAmount`: what of `via`'s is not through the link yet. */
  buffered(via: Via): number {
    let n = 0;
    for (const q of this.queue) if (q.via === via) n += q.len;
    return n;
  }

  /** `via` is gone this way, with what it had on the link. */
  kill(via: Via): void {
    this.cut[via] = true;
    for (let i = this.queue.length - 1; i >= 0; i--) if (this.queue[i]!.via === via) this.queue.splice(i, 1);
    for (let i = this.flying.length - 1; i >= 0; i--) if (this.flying[i]!.via === via) this.flying.splice(i, 1);
  }
}

interface Down {
  side: "device" | "agent";
  atMs: number;
  event: Extract<PathEvent, { type: "directDown" }>;
}

/**
 * A device (`a`, the page) and an agent (`b`) on one channel with a direct
 * path, over `down` (agent → device) and `up`. Each side's hub window is
 * `INITIAL_CREDIT` (credit comes back as the other reads), its DataChannel
 * takes new frames while under `DIRECT_BUDGET`, and both read every stream
 * as data comes (so the streams' windows stay open).
 */
class Sim {
  now = 0;
  readonly down: Lane;
  readonly up: Lane;
  readonly got: [number, number] = [0, 0];
  readonly downs: Down[] = [];
  /** Hub-path bytes in flight each way (`[device → agent, agent → device]`). */
  private readonly relayOut: [number, number] = [0, 0];

  private constructor(
    readonly a: Session,
    readonly b: Session,
    bytesPerMs: number,
    delayMs: number,
  ) {
    this.down = new Lane(bytesPerMs, delayMs);
    this.up = new Lane(bytesPerMs, delayMs);
  }

  static async create(bytesPerMs = MBIT, delayMs = 25): Promise<Sim> {
    const [it, rt] = await transports();
    return new Sim(Session.withPaths(1, it, "initiator", { probeRelay: true }, 0), Session.withPaths(1, rt, "responder", { probeRelay: false }, 0), bytesPerMs, delayMs);
  }

  async attach(): Promise<void> {
    const [it, rt] = await transports();
    this.a.attachDirect(it, this.now);
    this.b.attachDirect(rt, this.now);
  }

  private side(i: 0 | 1): Session {
    return i === 0 ? this.a : this.b;
  }

  private take(i: 0 | 1, events: MuxEvent[]): void {
    for (const e of events) {
      if (e.type !== "data") continue;
      this.got[i] += e.data.length;
      this.side(i).mux.consumed(e.sid, e.data.length);
    }
  }

  /** Everything each side can send now, onto the link. */
  async pump(): Promise<void> {
    for (const i of [0, 1] as const) {
      const s = this.side(i);
      const lane = i === 0 ? this.up : this.down;
      const to = i === 0 ? 1 : 0;
      for (;;) {
        const room = { relay: this.relayOut[i] + MAX_PAYLOAD_LEN <= INITIAL_CREDIT, direct: s.directActive && lane.buffered("direct") < DIRECT_BUDGET };
        const p = await s.poll(room, this.now, 1);
        if (!p) break;
        if (p.wire.type === "relay") {
          const f = p.wire.frame;
          if (f.type !== "data") continue;
          const payload = f.payload;
          this.relayOut[i] += payload.length;
          lane.put("relay", payload.length, async () => {
            this.relayOut[i] -= payload.length;
            this.take(to, await this.side(to).receiveRelay(payload, this.now));
          });
        } else {
          const m = p.wire.message;
          lane.put("direct", m.length, async () => this.take(to, await this.side(to).receiveDirect(m, this.now)));
        }
      }
    }
  }

  /** Move the clock to `toMs`: the link carries, both sides read, run their timers and send. */
  async run(toMs: number, each?: (nowMs: number) => void): Promise<void> {
    while (this.now < toMs) {
      this.now += STEP_MS;
      this.down.step(this.now);
      this.up.step(this.now);
      for (const deliver of [...this.down.due(this.now), ...this.up.due(this.now)]) await deliver();
      each?.(this.now);
      for (const [side, s] of [["device", this.a], ["agent", this.b]] as const) {
        s.tick(this.now);
        for (const e of s.takeEvents()) if (e.type === "directDown") this.downs.push({ side, atMs: this.now, event: e });
      }
      await this.pump();
    }
  }
}

/** The streams' share of a transfer: three answers, so that a stream's window (`INITIAL_WINDOW`) is not what holds it back. */
const STREAMS = 3;

/** The agent answers three streams the device opened with `bytes` in all; the device keeps typing (a keystroke every `typeEveryMs`) on another. */
async function transfer(s: Sim, bytes: number, typeEveryMs = 500): Promise<{ typed: number; type: (nowMs: number) => void }> {
  const feeds = Array.from({ length: STREAMS }, (_, k) => s.a.mux.open("rpc", utf8(`GET /api/sessions/claude/s-${k}/history`)));
  const term = s.a.mux.open("term", utf8("t-1"));
  await s.pump();
  await s.run(s.now + 200);
  for (const feed of feeds) s.b.mux.send(feed, new Uint8Array(bytes / STREAMS).fill(7));
  const out = {
    typed: 0,
    type: (nowMs: number) => {
      if (typeEveryMs > 0 && nowMs % typeEveryMs === 0) {
        s.a.mux.send(term, utf8("k"));
        out.typed++;
      }
    },
  };
  return out;
}

/** Attached, idle for three seconds: the direct round trip measured on an idle path. */
async function idleDirect(bytesPerMs = MBIT): Promise<Sim> {
  const s = await Sim.create(bytesPerMs);
  await s.attach();
  await s.run(3_000);
  const rtt = s.a.stats().directRttMs;
  assert.ok(rtt !== null && rtt < 100, `measured idle: ${rtt} ms`);
  return s;
}

function kept(s: Sim, what: string): void {
  assert.deepEqual(s.downs, [], `${what}: nobody gave the direct path up`);
  for (const side of [s.a, s.b]) {
    const st = side.stats();
    assert.ok(st.direct, `${what}: still direct`);
    assert.deepEqual([st.downs, st.lanes.resent], [0, 0], `${what}: nothing given up, nothing sent twice`);
  }
}

// -- (a) a full path is not a dead one ------------------------------------------

test("600 KB on a 1 Mbit/s direct path shared with other traffic: acknowledgements come seconds apart, and the path is kept", async () => {
  const s = await idleDirect();
  const bytes = 600 * 1024;
  const typing = await transfer(s, bytes);
  // The page's own fetches take two thirds of the downlink while the agent's
  // 600 KB go: each of its 64 KiB messages takes about 1.6 s to come — longer
  // than UNACKED_TIMEOUT_MS, with the device's keystrokes and the agent's
  // frames both waiting for an acknowledgement all that time.
  s.down.bytesPerMs = MBIT / 3;
  let gapMs = 0;
  let lastGot = 0;
  let lastAt = s.now;
  await s.run(s.now + 25_000, (now) => {
    typing.type(now);
    if (s.got[0] !== lastGot) {
      gapMs = Math.max(gapMs, now - lastAt);
      lastGot = s.got[0];
      lastAt = now;
    }
  });
  kept(s, "shared");
  assert.equal(s.got[0], bytes, "all of it, over the direct path");
  assert.ok(gapMs > UNACKED_TIMEOUT_MS, `data came as much as ${gapMs} ms apart: more than UNACKED_TIMEOUT_MS`);
});

test("600 KB on a 1 Mbit/s direct path that stalls for two seconds (a lost packet's retransmission timeout) is kept", async () => {
  const s = await idleDirect();
  const bytes = 600 * 1024;
  const typing = await transfer(s, bytes);
  const stallAt = s.now + 1_500;
  await s.run(s.now + 15_000, (now) => {
    s.down.stalled = now >= stallAt && now < stallAt + 2_000;
    typing.type(now);
  });
  kept(s, "stalled");
  assert.equal(s.got[0], bytes);
});

test("right after a switch, the direct path's frames wait behind what the hub path still has on the link, and the path is kept", async () => {
  const s = await Sim.create();
  const feeds = Array.from({ length: STREAMS }, (_, k) => s.a.mux.open("rpc", utf8(`GET /api/sessions/claude/s-${k}/history`)));
  const term = s.a.mux.open("term", utf8("t-1"));
  await s.pump();
  await s.run(500);
  // The agent's answers go over the hub path until the direct path comes
  // up: a whole hub window of them on the link, four seconds at 1 Mbit/s.
  for (const feed of feeds) s.b.mux.send(feed, new Uint8Array((600 * 1024) / STREAMS).fill(7));
  await s.pump();
  await s.run(s.now + 100);
  await s.attach();
  // The device types at once; the agent's acknowledgement and the rest of its
  // answer come over the direct path, behind the hub path's backlog.
  await s.run(s.now + 15_000, (now) => {
    if (now % 500 === 0) s.a.mux.send(term, utf8("k"));
  });
  kept(s, "switch");
  assert.equal(s.got[0], 600 * 1024);
});

// -- (b) a dead path is still let go ---------------------------------------------

test("a path that carries nothing much is given up on the floors: 3.5 s of silence, 1.5 s with a keystroke waiting", async () => {
  // Idle: only probes.
  const idle = await idleDirect();
  const quietFrom = idle.now;
  idle.down.kill("direct");
  idle.up.kill("direct");
  await idle.run(idle.now + 6_000);
  for (const side of ["device", "agent"] as const) {
    const d = idle.downs.find((x) => x.side === side)!;
    assert.deepEqual([d.event.timer, d.event.remote], ["DIRECT_TIMEOUT_MS", false], side);
    assert.equal(d.event.deadline!.limitMs, DIRECT_TIMEOUT_MS, `${side}: nothing carried, nothing added`);
    assert.ok(d.atMs - quietFrom <= DIRECT_TIMEOUT_MS + 1_000 + 2 * STEP_MS, `${side}: let go ${d.atMs - quietFrom} ms after the path died`);
  }
  // A keystroke on an idle path.
  const typing = await idleDirect();
  typing.down.kill("direct");
  typing.up.kill("direct");
  const term = typing.a.mux.open("term", utf8("t-1"));
  typing.a.mux.send(term, utf8("k"));
  const sentAt = typing.now;
  await typing.run(typing.now + 3_000);
  const d = typing.downs.find((x) => x.side === "device")!;
  assert.equal(d.event.timer, "UNACKED_TIMEOUT_MS");
  assert.ok(d.event.deadline!.limitMs < UNACKED_TIMEOUT_MS + 10, `a keystroke adds next to nothing: ${d.event.deadline!.limitMs} ms`);
  assert.ok(d.atMs - sentAt <= UNACKED_TIMEOUT_MS + 2 * STEP_MS, `let go ${d.atMs - sentAt} ms after the keystroke`);
});

test("a path that dies under load is given up within DIRECT_TIMEOUT_MAX_MS, both ways, and what it did not deliver comes over the hub", async () => {
  const s = await idleDirect();
  const bytes = 600 * 1024;
  const typing = await transfer(s, bytes);
  await s.run(s.now + 2_000, typing.type);
  assert.ok(s.got[0] > 0 && s.got[0] < bytes, "mid-transfer");
  // The network stops carrying UDP: nothing more either way on the direct path.
  s.down.kill("direct");
  s.up.kill("direct");
  const diedAt = s.now;
  await s.run(s.now + 30_000, typing.type);
  // The agent, sending, knows soonest: the device's acknowledgements and
  // probes stopped coming, and its own backlog is no reason to wait for them.
  const agentDown = s.downs.find((x) => x.side === "agent")!;
  assert.deepEqual([agentDown.event.timer, agentDown.event.remote], ["DIRECT_TIMEOUT_MS", false]);
  assert.ok(agentDown.atMs - diedAt <= DIRECT_TIMEOUT_MS + 1_000 + 2 * STEP_MS, `the agent in ${agentDown.atMs - diedAt} ms`);
  // The device lets go on its own deadline or on the agent's word, whichever comes first, within the bound either way.
  const deviceDown = s.downs.find((x) => x.side === "device")!;
  assert.ok(deviceDown, "the device let it go");
  assert.ok(deviceDown.atMs - diedAt <= DIRECT_TIMEOUT_MAX_MS + 2 * STEP_MS, `the device after ${deviceDown.atMs - diedAt} ms (${JSON.stringify(deviceDown.event)})`);
  assert.equal(s.got[0], bytes, "all of it, once: what the direct path did not deliver came over the hub");
  assert.ok(!s.a.directActive && !s.b.directActive);
});

// -- (c) a full stall under load ---------------------------------------------------

test("a downlink that stalls for good under load is given up within DIRECT_TIMEOUT_MAX_MS by both sides, and nothing is lost when it comes back", async () => {
  const s = await idleDirect();
  const bytes = 600 * 1024;
  const typing = await transfer(s, bytes);
  await s.run(s.now + 2_000, typing.type);
  assert.ok(s.got[0] > 0 && s.got[0] < bytes, "mid-transfer");
  // Nothing comes down any more, on either path; the device's probes still go up.
  s.down.stalled = true;
  const stuckAt = s.now;
  await s.run(s.now + DIRECT_TIMEOUT_MAX_MS + 1_000, typing.type);
  // The device hears nothing: let go on its own deadline (silence, stretched
  // by the message that was on its way), and says so over the hub path, which
  // still carries that way.
  const deviceDown = s.downs.find((x) => x.side === "device")!;
  assert.deepEqual([deviceDown.event.remote, deviceDown.event.timer], [false, "DIRECT_TIMEOUT_MS"]);
  assert.ok(deviceDown.atMs - stuckAt <= DIRECT_TIMEOUT_MAX_MS + 2 * STEP_MS, `the device after ${deviceDown.atMs - stuckAt} ms (${JSON.stringify(deviceDown.event.deadline)})`);
  // The agent, whose acknowledgements stopped, lets go on its own no-progress deadline or the device's word.
  const agentDown = s.downs.find((x) => x.side === "agent")!;
  assert.ok(agentDown, "the agent let it go");
  assert.ok(agentDown.atMs - stuckAt <= DIRECT_TIMEOUT_MAX_MS + 2 * STEP_MS, `the agent after ${agentDown.atMs - stuckAt} ms`);
  s.down.stalled = false;
  await s.run(s.now + 30_000, typing.type);
  assert.equal(s.got[0], bytes, "all of it, once, in the end");
});

// -- the deadline itself ---------------------------------------------------------

test("the deadlines: the floors when nothing is carried, stretched by the bytes ahead at the measured rate, never past DIRECT_TIMEOUT_MAX_MS", () => {
  const none = { rttMs: 50, unackedBytes: 0, bytesPerMs: null, relayBulkSent: false, relayBulkReceived: false, directBulkReceived: false };
  assert.deepEqual(directDeadline("DIRECT_TIMEOUT_MS", none), { limitMs: DIRECT_TIMEOUT_MS, floorMs: DIRECT_TIMEOUT_MS, loadMs: 0, aheadBytes: 0, bytesPerMs: 64 });
  assert.equal(directDeadline("UNACKED_TIMEOUT_MS", none).limitMs, UNACKED_TIMEOUT_MS);
  // Ten round trips, when longer, for no progress only (as before).
  assert.equal(directDeadline("UNACKED_TIMEOUT_MS", { ...none, rttMs: 485 }).floorMs, 4_850);
  assert.equal(directDeadline("DIRECT_TIMEOUT_MS", { ...none, rttMs: 485 }).floorMs, DIRECT_TIMEOUT_MS);
  // 300 KB unacknowledged at 1 Mbit/s: twice the 2.4 s they take, capped.
  const full = directDeadline("UNACKED_TIMEOUT_MS", { ...none, unackedBytes: 300_000, bytesPerMs: MBIT });
  assert.deepEqual([full.aheadBytes, full.loadMs, full.limitMs], [300_000, 4_800, UNACKED_TIMEOUT_MS + 4_800]);
  assert.equal(directDeadline("UNACKED_TIMEOUT_MS", { ...none, unackedBytes: 600_000, bytesPerMs: MBIT }).limitMs, DIRECT_TIMEOUT_MAX_MS);
  // What is unacknowledged does not hold the peer's answers back: no stretch to silence.
  assert.equal(directDeadline("DIRECT_TIMEOUT_MS", { ...none, unackedBytes: 600_000, bytesPerMs: MBIT }).limitMs, DIRECT_TIMEOUT_MS);
  // The peer's bulk over the direct path: one message ahead, both deadlines.
  const msg = directDeadline("DIRECT_TIMEOUT_MS", { ...none, directBulkReceived: true, bytesPerMs: MBIT });
  assert.deepEqual([msg.aheadBytes, msg.loadMs], [MAX_PAYLOAD_LEN, (2 * MAX_PAYLOAD_LEN) / MBIT]);
  // Bulk over the hub path: a whole hub window ahead.
  assert.equal(directDeadline("DIRECT_TIMEOUT_MS", { ...none, relayBulkReceived: true, bytesPerMs: 10 * MBIT }).aheadBytes, INITIAL_CREDIT);
  assert.equal(directDeadline("UNACKED_TIMEOUT_MS", { ...none, relayBulkSent: true, bytesPerMs: 10 * MBIT }).aheadBytes, INITIAL_CREDIT);
  assert.equal(directDeadline("DIRECT_TIMEOUT_MS", { ...none, relayBulkSent: true, bytesPerMs: 10 * MBIT }).aheadBytes, 0, "this side's own hub bulk does not hold the peer's probes back");
  // Never past the cap, whatever the round trip.
  assert.equal(directDeadline("UNACKED_TIMEOUT_MS", { ...none, rttMs: 5_000 }).limitMs, DIRECT_TIMEOUT_MAX_MS);
});

test("the deadlines' rate takes only backlogged samples over two round trips, fades towards the assumed rate, and starts afresh on a new path", () => {
  const r = new DeadlineRate();
  // One 20 KiB upload acknowledged a round trip later, nothing behind it: no sample (it read 146 times too slow before).
  r.acked(20 * 1024, 0, 0, 220, 220);
  assert.equal(r.value(220), null);
  // A backlog: 64 KiB freed a second, 500 KB still waiting, over 50 ms round trips.
  for (let k = 1; k <= 3; k++) r.acked(65_536, 1_000, 500_000, 1_000 + 1_000 * k, 50);
  const rate = r.value(4_000)!;
  assert.ok(rate > 60 && rate < 70, `about 65.5 B/ms: ${rate}`);
  // Bulk back to back over the direct path, too quick for two round trips: no sample yet.
  r.heard("direct", 65_536, 5_000, 500);
  r.heard("direct", 65_536, 5_100, 500);
  assert.equal(r.value(5_100), decayedRate(rate, 1_100));
  // Fades with the time to the latest arrival, halfway at RATE_DECAY_MS; a new path starts afresh.
  assert.equal(decayedRate(1_064, RATE_DECAY_MS), 564);
  assert.ok(r.value(64_000)! < rate);
  r.reset();
  assert.equal(r.value(64_000), null);
});

test("a new direct path starts its deadlines' rate afresh, and this side's hub bulk counts as ahead for RELAY_BULK_AHEAD_MAX_MS at most", async () => {
  const s = await idleDirect();
  const bytes = 600 * 1024;
  const typing = await transfer(s, bytes);
  await s.run(s.now + 6_000, typing.type);
  assert.notEqual(s.a.directLoad()!.bytesPerMs, null, "measured on the first path");
  await s.attach();
  assert.equal(s.a.directLoad()!.bytesPerMs, null, "not carried over to the next");
  // However slow the link reads, a hub window this side sent long ago is not still ahead.
  const far = directDeadline("UNACKED_TIMEOUT_MS", { ...s.a.directLoad()!, bytesPerMs: 1 });
  assert.ok(far.limitMs <= DIRECT_TIMEOUT_MAX_MS);
  assert.ok(RELAY_BULK_AHEAD_MAX_MS <= 10_000);
});

