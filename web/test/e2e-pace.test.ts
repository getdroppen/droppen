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
        assert.equal(pace.stopped(a[0] === "1") ? "1" : "0", a[2], line);
        break;
      case "ans":
        assert.equal(pace.answered(Number(a[0]), Number(a[1])) ? "1" : "0", a[3], line);
        break;
      default:
        assert.fail(`unknown op in ${line}`);
    }
    const s = pace.stats();
    const got = `${s.cap} ${s.inflight} ${s.rate === null ? "-" : Math.floor(s.rate)} ${s.minRttMs ?? "-"} ${s.markers} ${s.draining ? 1 : 0} ${s.startup ? 1 : 0}`;
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
    for (let i = 0; i < ms; i++) {
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
          if (pace.stopped(true)) this.mark(pace);
          break;
        }
      }
      this.drained ||= pace.stats().draining;
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
        if (front[1] !== null) this.answers.push([this.now + this.rtt, front[1]]);
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
  assert.equal(quiet.stopped(false), false, "a keystroke's echo is too small to mark");
  quiet.next(100);
  assert.equal(quiet.inflight, 60);
  quiet.next(200);
  assert.equal(quiet.inflight, 0, "a quiet round trip later it counts as delivered");
  quiet.dataSent(5000, 200);
  assert.equal(quiet.stopped(false), true, "a bigger tail is marked");
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
}

const pct = (sorted: number[], p: number) => sorted[Math.min(sorted.length, Math.max(1, Math.ceil(sorted.length * p))) - 1]!;

/**
 * A 1 Mbps link, 70 ms round trip, deep queue, each way; the agent (a
 * responder driver) sends `bulk` on a blob stream and a 2 KB answer on each
 * rpc stream the page opens, one a second.
 */
async function slowLink(t: TestContext, bulk: number, agentPaces: boolean, devicePaces: boolean): Promise<Report> {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let now = 0;
  const clock = () => now;
  const wall = () => 1_700_000_000_000 + now;
  const bitsPerMs = 1000;
  const oneWay = 35;
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
  const rpcMs: number[] = [];
  let asked = 0;
  await quiet();
  await tick(500);
  while (doneAt === null) {
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
    assert.ok(now < 120_000, "the download ends");
  }
  while (rpcMs.length < asked) await tick(100);
  device.close();
  agent.close();
  await tick(10);
  t.mock.timers.reset();
  return { rpcMs: rpcMs.sort((a, b) => a - b), mbps: (bulk * 8) / doneAt! / 1000 };
}

const line = (name: string, r: Report) =>
  `${name.padEnd(26)} rpc n=${r.rpcMs.length} p50=${pct(r.rpcMs, 0.5)} ms p95=${pct(r.rpcMs, 0.95)} ms max=${r.rpcMs.at(-1)} ms  bulk ${r.mbps.toFixed(2)} Mbps`;

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
