// The sender's pace (`lib/e2e/pace.ts`, `mux.ts` `pollWithin`, `channel.ts`
// with pacing): the port of `droppen-proto/src/pace.rs` and `link_sim.rs`.
//
// What these pin: the estimator says what the Rust one says, call for call,
// over the shared trace (`rust.json` `pace`); on a slow path it holds what is
// in flight at the floor and keeps the link busy, on a fast one it grows to
// twice the product within a few round trips and falls back when the path
// slows; a busy path is drained for a clean round trip; a small tail needs no
// marker; the multiplexer cuts bulk to its budget and lets term, rpc and mux
// go past it; and two running drivers on a 1 Mbps link, in virtual time,
// answer a question a second within a second behind a download — paced or
// not on either end, everything arrives.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test, { type TestContext } from "node:test";
import {
  BUDGET_UNLIMITED,
  ChannelDriver,
  type ChannelStream,
  DEFAULT_REKEY,
  type Frame,
  generateDeviceKeys,
  INITIAL_CREDIT,
  Initiator,
  MIN_CUT,
  Mux,
  type MuxFrame,
  type NoiseKeyPair,
  Pace,
  PACE_FLOOR,
  PACE_HEADROOM,
  PACE_LOADED_SLACK,
  PACE_OVERHEAD,
  PACE_STARTUP_GAIN,
  Responder,
  Session,
  type Transport,
  utf8,
} from "../src/lib/e2e/index.js";

const V = JSON.parse(readFileSync(new URL("../../crates/droppen-proto/vectors/rust.json", import.meta.url), "utf8")) as { pace: string[] };

test("the pace says what droppen-proto's says, call for call (rust.json pace)", () => {
  const pace = new Pace(INITIAL_CREDIT);
  assert.ok(V.pace.length > 500);
  for (const line of V.pace) {
    const [call, want] = line.split(" | ") as [string, string];
    const [op, ...a] = call.split(" ");
    switch (op) {
      case "next": {
        const n = pace.next(Number(a[0]));
        assert.equal(n.type === "marker" ? "M" : `D ${n.budget.interactive} ${n.budget.bulk}`, a.slice(2).join(" "), line);
        break;
      }
      case "mark":
        pace.markerSent(Number(a[0]), Number(a[1]));
        break;
      case "data":
        pace.dataSent(Number(a[0]), Number(a[1]));
        break;
      case "stop":
        assert.equal(pace.stopped(a[0] === "1", a[1] === "1") ? "1" : "0", a[3], line);
        break;
      case "rx":
        pace.received(Number(a[0]));
        break;
      case "probe":
        pace.peerProbe();
        break;
      case "ans":
        assert.equal(pace.answered(Number(a[0]), Number(a[1])) ? "1" : "0", a[3], line);
        break;
      default:
        assert.fail(`unknown op in ${line}`);
    }
    const s = pace.stats();
    const got = `${s.cap} ${s.inflight} ${s.rate === null ? "-" : Math.floor(s.rate)} ${s.minRttMs ?? "-"} ${s.markers} ${s.draining ? 1 : 0} ${s.startup ? 1 : 0} ${s.deferring ? 1 : 0}`;
    assert.equal(got, want, line);
  }
});

/** A sender with endless 16 KiB chunks on a path draining `rate` bytes a ms into a deep queue; a marker is answered `rtt` ms after what is ahead of it drained. */
class Sim {
  now = 0;
  private queue: [number, number | null][] = [];
  private answers: [number, number][] = [];
  private nonce = 0;
  private credit = 0;
  delivered = 0;
  drained = false;
  deferred = false;
  /** The peer's traffic this way: bytes a ms, a probe in every so many (0: none), and how long its queue holds the answers back (ms). */
  rx = 0;
  probeEvery = 0;
  back = 0;
  private unprobed = 0;

  constructor(
    public rate: number,
    public rtt: number,
  ) {}

  private mark(pace: Pace): void {
    this.nonce += 1;
    pace.markerSent(this.nonce, this.now);
    this.queue.push([0, this.nonce]);
  }

  run(pace: Pace, ms: number): this {
    this.delivered = 0;
    this.drained = false;
    this.deferred = false;
    for (let i = 0; i < ms; i++) {
      if (this.rx > 0) {
        pace.received(this.rx);
        this.unprobed += this.rx;
        if (this.probeEvery > 0 && this.unprobed >= this.probeEvery) {
          pace.peerProbe();
          this.unprobed = 0;
        }
      }
      while (this.answers.length > 0 && this.answers[0]![0] <= this.now) pace.answered(this.answers.shift()![1], this.now);
      for (;;) {
        const n = pace.next(this.now);
        if (n.type === "marker") {
          this.mark(pace);
        } else if (n.budget.bulk >= MIN_CUT) {
          const len = Math.min(n.budget.bulk, 16 * 1024);
          pace.dataSent(len, this.now);
          this.queue.push([len, null]);
        } else {
          if (pace.stopped(true, false)) this.mark(pace);
          break;
        }
      }
      this.drained ||= pace.stats().draining;
      this.deferred ||= pace.stats().deferring;
      this.credit += this.rate;
      while (this.queue.length > 0) {
        const front = this.queue[0]!;
        if (front[0] > this.credit) {
          front[0] -= this.credit;
          this.delivered += this.credit;
          this.credit = 0;
          break;
        }
        this.credit -= front[0];
        this.delivered += front[0];
        this.queue.shift();
        if (front[1] !== null) this.answers.push([this.now + this.rtt + this.back, front[1]]);
      }
      if (this.queue.length === 0) this.credit = 0;
      this.now += 1;
    }
    return this;
  }

  get queuedMs(): number {
    return this.queue.reduce((n, [b]) => n + b, 0) / this.rate;
  }
}

test("a slow path is held at the floor and kept busy; a fast one grows to twice its product and falls back when it slows", () => {
  const slow = new Pace(INITIAL_CREDIT);
  const s1 = new Sim(125, 70);
  const delivered = s1.run(slow, 20_000).delivered;
  assert.equal(slow.cap(), PACE_FLOOR);
  assert.ok(s1.queuedMs <= 550, `${s1.queuedMs}`);
  assert.ok(delivered >= 0.97 * 125 * 20_000, `${delivered}`);
  assert.ok(slow.stats().minRttMs! <= 110, "a clean round trip, not the queue's");

  const fast = new Pace(INITIAL_CREDIT);
  const s2 = new Sim(6250, 20);
  assert.ok(s2.run(fast, 100).delivered >= 0.75 * 6250 * 100, "busy within five round trips");
  assert.ok(s2.run(fast, 1_900).delivered >= 0.99 * 6250 * 1_900);
  const s = fast.stats();
  assert.ok(!s.startup && s.cap >= 230_000 && s.cap <= 330_000, JSON.stringify(s));
  // The same path at 1 Mbps: the cap falls to the floor and the queue drains.
  s2.rate = 125;
  s2.run(fast, 4_000);
  assert.equal(fast.cap(), PACE_FLOOR);
  assert.ok(s2.queuedMs <= 550, `${s2.queuedMs}`);
  s2.rate = 6250;
  assert.ok(s2.run(fast, 1_000).delivered >= 0.9 * 6250 * 1_000, "and grows back within a second");
});

test("a path busy past the window is drained once for a clean round trip; a small tail needs no marker", () => {
  const pace = new Pace(INITIAL_CREDIT);
  const sim = new Sim(125, 70);
  assert.equal(sim.run(pace, 9_000).drained, false);
  assert.equal(sim.run(pace, 3_000).drained, true);
  assert.ok(sim.delivered >= 0.95 * 125 * 3_000, `${sim.delivered}`);
  assert.equal(sim.run(pace, 5_000).drained, false, "not again so soon");

  const quiet = new Pace(INITIAL_CREDIT);
  assert.deepEqual(quiet.next(0), { type: "marker" }, "the first flight is timed first");
  quiet.markerSent(1, 0);
  quiet.answered(1, 50);
  quiet.dataSent(60, 60);
  assert.equal(quiet.stopped(false, true), false, "a keystroke's echo is too small to mark");
  quiet.next(100);
  assert.equal(quiet.inflight, 60);
  quiet.next(200);
  assert.equal(quiet.inflight, 0, "a quiet round trip later it counts as delivered");
  quiet.dataSent(5000, 200);
  assert.equal(quiet.stopped(false, true), true, "a bigger tail is marked");
});

test("a drain ends with a small tail still out, not only with nothing out", () => {
  const pace = new Pace(INITIAL_CREDIT);
  assert.deepEqual(pace.next(0), { type: "marker" });
  pace.markerSent(1, 0);
  pace.answered(1, 70);
  pace.dataSent(60_000, 100);
  pace.markerSent(2, 100);
  pace.dataSent(2_000, 150);
  assert.deepEqual(pace.next(10_200), { type: "data", budget: { interactive: 0, bulk: 0 } });
  assert.equal(pace.stats().draining, true);
  assert.equal(pace.stopped(true, false), true, "the tail is marked");
  pace.markerSent(3, 10_200);
  assert.equal(pace.answered(2, 10_300), true);
  assert.equal(pace.inflight, 2_000);
  assert.deepEqual(pace.next(10_300), { type: "marker" }, "a clean marker, now");
  assert.equal(pace.stats().draining, false);
  pace.markerSent(4, 10_300);
  const next = pace.next(10_300);
  assert.ok(next.type === "data" && next.budget.bulk > 0, "and data behind it");
});

test("interactive data has headroom past the cap, bulk has none; a seeded path measures its own round trip", () => {
  const pace = new Pace(INITIAL_CREDIT);
  pace.next(0);
  pace.markerSent(1, 0);
  pace.dataSent(PACE_FLOOR - PACE_OVERHEAD, 0);
  assert.deepEqual(pace.next(0), { type: "marker" }, "a spacing marker is due");
  pace.markerSent(2, 0);
  assert.deepEqual(pace.next(0), { type: "data", budget: { interactive: PACE_HEADROOM, bulk: 0 } });
  assert.equal(pace.answered(2, 70), true);
  assert.equal(pace.inflight, 0);
  assert.equal(pace.answered(1, 70), false, "covered by the later one");

  const lan = Pace.seeded(4 << 20, [0, 50e6]);
  assert.equal(lan.cap(), PACE_FLOOR, "no round trip of its own yet");
  lan.markerSent(1, 0);
  lan.answered(1, 10);
  assert.equal(lan.cap(), Math.trunc(PACE_STARTUP_GAIN * 500_000));
});

test("both ways at once: a pacing peer's loaded answers change nothing but drains; an older peer's unmarked bulk makes the cap defer to the window", () => {
  const paced = new Pace(INITIAL_CREDIT);
  const s1 = new Sim(125, 70);
  s1.run(paced, 1_000);
  s1.rx = 125;
  s1.probeEvery = 16 * 1024;
  s1.back = 220;
  const r1 = s1.run(paced, 30_000);
  assert.equal(r1.drained || r1.deferred, false, JSON.stringify(paced.stats()));
  assert.ok(r1.delivered >= 0.95 * 125 * 30_000, `${r1.delivered}`);
  assert.equal(paced.cap(), PACE_FLOOR + PACE_LOADED_SLACK, "its own, and the slack two ends sending share");
  assert.ok(paced.stats().minRttMs! < 110, "the path's own round trip, not the peer's queue");

  const old = new Pace(INITIAL_CREDIT);
  const s2 = new Sim(125, 70);
  s2.run(old, 1_000);
  s2.rx = 125;
  s2.back = 3_700;
  const r2 = s2.run(old, 30_000);
  assert.equal(r2.deferred, true);
  assert.ok(r2.delivered >= 0.85 * 125 * 30_000, `${r2.delivered} ${JSON.stringify(old.stats())}`);
  assert.equal(old.cap(), INITIAL_CREDIT);
  // An older page's direct path probes once a second: still unmarked bulk; and quiet again, back to its own cap.
  const again = new Pace(INITIAL_CREDIT);
  const s3 = new Sim(125, 70);
  s3.run(again, 1_000);
  s3.rx = 125;
  s3.probeEvery = 125 * 1000;
  s3.back = 3_700;
  assert.equal(s3.run(again, 20_000).deferred, true);
  s3.rx = 0;
  s3.back = 0;
  s3.run(again, 10_000);
  assert.equal(again.cap(), PACE_FLOOR, JSON.stringify(again.stats()));
});

test("a trickle from the peer does not freeze the minimum; a pacing sender's small writes are marked every 16 KiB", () => {
  const pace = new Pace(INITIAL_CREDIT);
  const sim = new Sim(2500, 20);
  sim.run(pace, 5_000);
  sim.rtt = 200;
  sim.rx = 40;
  sim.probeEvery = 16 * 1024;
  sim.run(pace, 15_000);
  assert.ok(pace.stats().minRttMs! >= 150, JSON.stringify(pace.stats()));
  const tail = sim.run(pace, 5_000).delivered;
  assert.ok(tail >= 0.9 * 2500 * 5_000, `${tail}`);

  const writer = new Pace(INITIAL_CREDIT);
  writer.next(0);
  writer.markerSent(1, 0);
  writer.answered(1, 70);
  let [nonce, unprobed, most] = [1, 0, 0];
  for (let k = 1; k <= 100; k++) {
    const t = k * 100;
    if (writer.next(t).type === "marker") {
      nonce += 1;
      writer.markerSent(nonce, t);
      writer.answered(nonce, t + 70);
      unprobed = 0;
    }
    writer.dataSent(3_000, t);
    unprobed += 3_000;
    most = Math.max(most, unprobed);
    assert.equal(writer.stopped(false, true), false);
  }
  assert.ok(most <= 16 * 1024 + 3_000, `${most}`);
  assert.ok(nonce >= 15, `${nonce}`);
});

function kinds(frames: MuxFrame[]): string[] {
  return frames.map((f) => (f.type === "data" ? `data${f.sid}:${f.data.length}` : `${f.type}${"sid" in f ? f.sid : ""}`));
}

test("the multiplexer polls within a budget: bulk cut to fit but not to a sliver, interactive by its own, control always", () => {
  const mux = new Mux("initiator");
  const blob = mux.open("blob", utf8("file"));
  const rpc = mux.open("rpc", utf8("GET /api/screens"));
  mux.send(blob, new Uint8Array(40_000));
  mux.send(rpc, new Uint8Array(2048));
  const out: MuxFrame[] = [];
  // As the session does: what goes out of either budget comes off both (it is all in flight).
  const take = (interactive: number, bulk: number) => {
    for (let f = mux.pollWithin({ interactive, bulk }); f; f = mux.pollWithin({ interactive, bulk })) {
      out.push(f);
      const len = f.type === "data" ? f.data.length : 0;
      interactive = Math.max(0, interactive - len);
      bulk = Math.max(0, bulk - len);
    }
  };
  take(0, 0);
  assert.deepEqual(kinds(out), ["open1", "open3"], "control needs no budget");
  out.length = 0;
  take(1000, 21_000);
  // The rpc answer does not fit 1000 (less than MIN_CUT, so not cut either): it waits; blob takes a chunk and the 4616 left.
  assert.deepEqual(kinds(out), ["data1:16384", "data1:4616"]);
  out.length = 0;
  take(2048, 0);
  assert.deepEqual(kinds(out), ["data3:2048"], "interactive by its own budget, ahead of blob");
  out.length = 0;
  take(MIN_CUT - 1, MIN_CUT - 1);
  assert.deepEqual(kinds(out), [], "nothing cut below MIN_CUT");
  for (let f = mux.pollWithin(BUDGET_UNLIMITED); f; f = mux.pollWithin(BUDGET_UNLIMITED)) out.push(f);
  assert.deepEqual(kinds(out), ["data1:11768", "data1:7232"], "the rest, as poll would");
});

// ---------------------------------------------------------------------------
// Sessions with pacing, by hand (the ports of `channel.rs`'s tests).
// ---------------------------------------------------------------------------

/** Two sessions with paths, their wires, and what each read. */
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

  side(i: number): Session {
    return i === 0 ? this.a : this.b;
  }

  async sendAll(): Promise<void> {
    for (const i of [0, 1]) {
      for (let p = await this.side(i).poll({ relay: true, direct: true }, this.now, 1); p; p = await this.side(i).poll({ relay: true, direct: true }, this.now, 1)) {
        if (p.wire.type === "relay") this.relay[i]!.push(p.wire.frame);
        else this.direct[i]!.push(p.wire.message);
      }
    }
  }

  async deliver(from: number, via: "relay" | "direct"): Promise<boolean> {
    const to = 1 - from;
    const events =
      via === "relay"
        ? await (async () => {
            const f = this.relay[from]!.shift();
            return f && f.type === "data" ? this.side(to).receiveRelay(f.payload, this.now) : null;
          })()
        : await (async () => {
            const m = this.direct[from]!.shift();
            return m ? this.side(to).receiveDirect(m, this.now) : null;
          })();
    if (!events) return false;
    for (const e of events) if (e.type === "data") for (const byte of e.data) this.got[to]!.push(byte);
    return true;
  }

  /** One round trip each way: what each side sends, half a round trip on the wire, delivered and read. */
  async round(sid: number): Promise<void> {
    for (let i = 0; i < 2; i++) {
      await this.sendAll();
      this.now += 35;
      for (const from of [0, 1]) while ((await this.deliver(from, "relay")) || (await this.deliver(from, "direct"))) continue;
      this.a.mux.consumed(sid, this.got[0].length);
      this.got[0] = [];
    }
  }
}

const relayBytes = (frames: Frame[]) => frames.reduce((n, f) => n + (f.type === "data" ? f.payload.length : 0), 0);

test("a paced session stops at its cap; its markers bring the room back and touch no probe's numbers", async () => {
  const p = await Pair.create();
  p.b.setPacing(true);
  const sid = p.a.mux.open("blob", utf8("GET /api/fs/download"));
  await p.sendAll();
  while (await p.deliver(0, "relay")) continue;
  p.b.mux.send(sid, new Uint8Array(1 << 20));
  await p.sendAll();
  const sent = relayBytes(p.relay[1]);
  const pace = p.b.paceStats()!;
  assert.ok(sent <= PACE_FLOOR + 1024 && sent >= PACE_FLOOR - 4096, `the floor and its markers: ${sent}`);
  assert.ok(pace.markers >= 4, JSON.stringify(pace));
  assert.equal(pace.inflight, sent - pace.markers * 26, "data less the markers' own bytes");
  await p.sendAll();
  assert.equal(p.relay[1].length, pace.markers + 4, "nothing more until answers come");
  p.b.takeEvents();
  await p.round(sid);
  const after = p.b.paceStats()!;
  assert.ok(after.minRttMs !== null && after.rate !== null, JSON.stringify(after));
  assert.equal(p.b.stats().relayRttMs, null, "a marker's answer is not a probe's");
  assert.deepEqual(p.b.takeEvents(), [], "nor an event");
  for (let i = 0; i < 20; i++) await p.round(sid);
  assert.ok(p.b.paceStats()!.inflight <= PACE_FLOOR);
});

test("a paced side that mostly reads: the multiplexer's credits for what it read do not count against its cap", async () => {
  const p = await Pair.create();
  p.a.setPacing(true);
  const sid = p.a.mux.open("blob", utf8("GET /api/fs/download"));
  await p.sendAll();
  while (await p.deliver(0, "relay")) continue;
  p.b.mux.send(sid, new Uint8Array(1 << 20));
  for (let i = 0; i < 20; i++) {
    await p.round(sid);
    const pace = p.a.paceStats()!;
    assert.deepEqual([pace.inflight, pace.markers], [0, 0], JSON.stringify(pace));
  }
});

test("after a fallback the hub path's control goes ahead of the resends, and they ahead of anything new", async () => {
  const p = await Pair.create();
  const sid = p.a.mux.open("term", utf8("t"));
  await p.sendAll();
  while (await p.deliver(0, "relay")) continue;
  const [it, rt] = await transports();
  p.a.attachDirect(it, p.now);
  p.b.attachDirect(rt, p.now);
  p.a.mux.send(sid, utf8("abc"));
  await p.sendAll();
  p.direct[0] = [];
  p.a.detachDirect("failed", true);
  p.now += 1;
  p.a.tick(p.now);
  p.a.mux.send(sid, utf8("d"));
  await p.sendAll();
  assert.equal(p.relay[0].length, 4, "down, probe, resend, new");
  await p.deliver(0, "relay");
  await p.deliver(0, "relay");
  assert.deepEqual(p.got[1], [], "nothing read yet");
  assert.equal(p.b.diagnostics(p.now)!.outRelay, 1, "the probe's answer queued before any resend arrived");
  while (await p.deliver(0, "relay")) continue;
  assert.equal(String.fromCharCode(...p.got[1]), "abcd");
});

// ---------------------------------------------------------------------------
// Two running drivers on a slow link, in virtual time.
// ---------------------------------------------------------------------------

const binding = { account: "a-1", device: "d-1", machine: "m-1" };

async function keys(): Promise<NoiseKeyPair> {
  const k = await generateDeviceKeys();
  return { privateKey: k.dh.privateKey, publicKey: k.dhPublic };
}

/** Crypto in flight on the counted transports: zero, with microtasks drained, is a quiet page. */
let busy = 0;
function counted(t: Transport): Transport {
  const seal = t.seal.bind(t);
  const open = t.open.bind(t);
  const track = async <T>(p: Promise<T>): Promise<T> => {
    busy += 1;
    try {
      return await p;
    } finally {
      busy -= 1;
    }
  };
  t.seal = (nowMs, data) => track(seal(nowMs, data));
  t.open = (message) => track(open(message));
  return t;
}

async function transports(): Promise<[Transport, Transport]> {
  const device = await keys();
  const agent = await keys();
  const i = await Initiator.create(device, agent.publicKey, binding);
  const r = await Responder.create(agent, binding);
  await r.readHello(await i.writeHello(1, utf8("")));
  const { message, transport: rt } = await r.writeReply(utf8(""), DEFAULT_REKEY, 1);
  const { transport: it } = await i.readReply(message, DEFAULT_REKEY, 1);
  return [counted(it), counted(rt)];
}

const nextTurn = () => new Promise<void>((resolve) => setImmediate(resolve));

async function quiet(): Promise<void> {
  for (let i = 0; i < 100_000; i++) {
    await nextTurn();
    if (busy === 0) {
      await nextTurn();
      if (busy === 0) return;
    }
  }
  throw new Error("never quiet");
}

interface Report {
  rpcMs: number[];
  mbps: number;
  upMbps: number | null;
}

const pct = (sorted: number[], p: number) => sorted[Math.min(sorted.length, Math.max(1, Math.ceil(sorted.length * p))) - 1]!;

/**
 * A link (1 Mbps, 70 ms round trip by default), deep queue, each way; the
 * agent (a responder driver) sends `bulk` on a blob stream and a 2 KB answer
 * on each rpc stream the page opens, one a second; with `upload`, the page
 * sends that much on a blob stream of its own at the same time, which the
 * agent reads whole.
 */
async function slowLink(
  t: TestContext,
  bulk: number,
  agentPaces: boolean,
  devicePaces: boolean,
  link: { mbps: number; rttMs: number; upload: number } = { mbps: 1, rttMs: 70, upload: 0 },
): Promise<Report> {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let now = 0;
  const clock = () => now;
  const wall = () => 1_700_000_000_000 + now;
  const bitsPerMs = link.mbps * 1000;
  const oneWay = link.rttMs / 2;
  let upDoneAt: number | null = null;
  const free = { toAgent: 0, toDevice: 0 };
  let agent: ChannelDriver;
  let device: ChannelDriver;
  const carry = (to: "toAgent" | "toDevice", frame: Frame) => {
    const bytes = frame.type === "data" ? frame.payload.length + 15 : 15;
    free[to] = Math.max(free[to], now) + (bytes * 8) / bitsPerMs;
    const at = free[to] + oneWay;
    setTimeout(() => (to === "toAgent" ? agent : device).receive(frame), Math.max(0, at - now));
  };
  const [it, rt] = await transports();
  device = new ChannelDriver(7, it, "initiator", (f) => carry("toAgent", f), wall, { paths: { probeRelay: true }, pace: devicePaces, clock });
  agent = new ChannelDriver(7, rt, "responder", (f) => carry("toDevice", f), wall, { paths: { probeRelay: false }, pace: agentPaces, clock });
  agent.onincoming = (stream: ChannelStream) => {
    if (stream.cls === "blob" && new TextDecoder().decode(stream.meta).startsWith("POST")) {
      let read = 0;
      stream.ondata = (d) => {
        read += d.length;
        if (read >= link.upload && upDoneAt === null) {
          upDoneAt = now;
          stream.send(utf8("ok"));
          stream.close(0);
        }
      };
      return;
    }
    const len = stream.cls === "blob" ? bulk : 2048;
    for (let at = 0; at < len; at += 64 * 1024) stream.send(new Uint8Array(Math.min(64 * 1024, len - at)));
    stream.close(0);
  };
  const tick = async (ms: number) => {
    for (let i = 0; i < ms; i++) {
      now += 1;
      t.mock.timers.tick(1);
      await quiet();
    }
  };
  let got = 0;
  let doneAt: number | null = null;
  const blob = device.open("blob", utf8("GET /api/fs/download?path=%2Ftmp%2Fbig"));
  blob.ondata = (d) => {
    got += d.length;
    if (got >= bulk && doneAt === null) doneAt = now;
  };
  if (link.upload > 0) {
    const up = device.open("blob", utf8("POST /api/fs/upload?path=%2Ftmp%2Fbig"));
    for (let at = 0; at < link.upload; at += 64 * 1024) up.send(new Uint8Array(Math.min(64 * 1024, link.upload - at)));
  }
  const rpcMs: number[] = [];
  let asked = 0;
  await quiet();
  await tick(500);
  while (doneAt === null || (link.upload > 0 && upDoneAt === null)) {
    const at = now;
    asked += 1;
    const s = device.open("rpc", utf8("GET /api/screens"));
    let answer = 0;
    s.ondata = (d) => (answer += d.length);
    s.onclose = () => {
      assert.equal(answer, 2048, "the whole answer");
      rpcMs.push(now - at);
    };
    await tick(1000);
    assert.ok(now < 120_000, "the transfers end");
  }
  while (rpcMs.length < asked) await tick(100);
  device.close();
  agent.close();
  await tick(10);
  t.mock.timers.reset();
  return {
    rpcMs: rpcMs.sort((a, b) => a - b),
    mbps: (bulk * 8) / doneAt! / 1000,
    upMbps: link.upload > 0 ? (link.upload * 8) / upDoneAt! / 1000 : null,
  };
}

const line = (name: string, r: Report) =>
  `${name.padEnd(26)} rpc n=${r.rpcMs.length} p50=${pct(r.rpcMs, 0.5)} ms p95=${pct(r.rpcMs, 0.95)} ms max=${r.rpcMs.at(-1)} ms  bulk ${r.mbps.toFixed(2)} Mbps${r.upMbps === null ? "" : `  up ${r.upMbps.toFixed(2)} Mbps`}`;

test("two drivers on a 1 Mbps link: a question waits behind a fraction of a second of download, paced on either end or both", async (t) => {
  const bulk = 1536 * 1024;
  const before = await slowLink(t, bulk, false, false);
  const after = await slowLink(t, bulk, true, true);
  const agentOnly = await slowLink(t, bulk, true, false);
  const pageOnly = await slowLink(t, bulk, false, true);
  t.diagnostic(line("before (no pace)", before));
  t.diagnostic(line("after (paced)", after));
  t.diagnostic(line("pacing agent, old page", agentOnly));
  t.diagnostic(line("old agent, pacing page", pageOnly));
  assert.ok(pct(before.rpcMs, 0.95) > 1500, line("before", before));
  assert.ok(pct(after.rpcMs, 0.95) < 1000, line("after", after));
  assert.ok(pct(agentOnly.rpcMs, 0.95) < 1000, line("agent only", agentOnly));
  assert.ok(after.mbps >= 0.85 * before.mbps, `${after.mbps} against ${before.mbps}`);
  assert.ok(pageOnly.mbps > 0.85 && agentOnly.mbps > 0.85, "both mixed pairs carry the download");
});

test("two drivers, bulk both ways at once: in every pairing of a pacing end and an older one, each direction keeps within a tenth of its speed unpaced", async (t) => {
  for (const [mbps, rttMs, size] of [
    [1, 70, 768 * 1024],
    [4, 50, 2048 * 1024],
  ] as const) {
    const link = { mbps, rttMs, upload: size };
    const unpaced = await slowLink(t, size, false, false, link);
    t.diagnostic(`${mbps} Mbps, ${rttMs} ms, ${size >> 10} KiB each way:`);
    t.diagnostic(line("neither paces", unpaced));
    for (const [agentPaces, devicePaces, name] of [
      [true, true, "both pace"],
      [true, false, "pacing agent, old page"],
      [false, true, "old agent, pacing page"],
    ] as const) {
      const r = await slowLink(t, size, agentPaces, devicePaces, link);
      t.diagnostic(line(name, r));
      assert.ok(r.mbps >= 0.9 * unpaced.mbps, `${name}, down: ${r.mbps} against ${unpaced.mbps}`);
      assert.ok(r.upMbps! >= 0.9 * unpaced.upMbps!, `${name}, up: ${r.upMbps} against ${unpaced.upMbps}`);
    }
  }
});
