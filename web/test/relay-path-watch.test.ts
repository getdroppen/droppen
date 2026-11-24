// The page's watch on a channel's hub path (`lib/hub/machine.ts`
// `watchRelayPath`, the driver's `onpaths` in `MachineChannel`), on two
// running drivers (`lib/e2e/driver.ts`) through a slow link, on a clock the
// test moves — the drivers' timers with it (mocked `setTimeout`).
//
// What these pin: a slow hub path that keeps delivering is never replaced,
// however old its probes get — a download whose probe answers wait behind
// the channel's window, an upload whose probes wait behind the page's own
// data, an upload the agent answers with nothing but its CREDITs; and a path
// that dies is replaced (and its relay link woken, once) within 15 s of the
// last thing it brought — the relay gone with what it held, the agent gone
// while the relay still drains a backlog, the page uploading with its window
// spent so that its next probe cannot even leave.
//
// The link is one FIFO per direction letting so many bytes a second through,
// with 40 ms each way: what a channel queues on a slow link (the relay's
// queue and the sockets' buffers) as the protocol's windows bound it.
import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import {
  ChannelDriver,
  DEFAULT_REKEY,
  type Frame,
  generateDeviceKeys,
  Initiator,
  type NoiseKeyPair,
  RELAY_SILENT_MS,
  Responder,
  type Transport,
  utf8,
} from "../src/lib/e2e/index.js";
import { RELAY_PATH_GONE, watchRelayPath } from "../src/lib/hub/machine.js";

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

/** How far the clock moves at a time. */
const STEP_MS = 50;
/** The most a dead path may go on after the last thing it brought: the third probe tick after it. */
const GONE_WITHIN_MS = 15_000 + STEP_MS;

/** One direction of the link: a FIFO letting `bytesPerSec` through, then 40 ms. */
class Lane {
  private freeAt = 0;
  private readonly queue: { at: number; frame: Frame }[] = [];
  /** Nothing more gets on (that end, or the relay, is gone). */
  cut = false;
  /** Only frames this lets through get on. */
  only: ((frame: Frame) => boolean) | null = null;
  /** When the last frame came off. */
  lastArrivalMs = 0;

  constructor(private readonly bytesPerSec: number) {}

  put(nowMs: number, frame: Frame): void {
    if (this.cut || (this.only && !this.only(frame))) return;
    // The frame header and the WebSocket's, TLS's and TCP's.
    const len = (frame.type === "data" ? frame.payload.length : 16) + 7 + 60;
    this.freeAt = Math.max(this.freeAt, nowMs) + (len * 1000) / this.bytesPerSec;
    this.queue.push({ at: this.freeAt + 40, frame });
  }

  /** What has come off by `nowMs`, in order. */
  take(nowMs: number): Frame[] {
    const out: Frame[] = [];
    while (this.queue[0] && this.queue[0].at <= nowMs) out.push(this.queue.shift()!.frame);
    if (out.length > 0) this.lastArrivalMs = nowMs;
    return out;
  }

  /** What was on it is lost (the relay went away with it). */
  lose(): void {
    this.queue.length = 0;
  }

  /** Nothing gets through for `ms` from `nowMs` (a radio link's stall): everything on it arrives that much later. */
  stall(nowMs: number, ms: number): void {
    for (const q of this.queue) q.at = Math.max(q.at, nowMs) + ms;
    this.freeAt = Math.max(this.freeAt, nowMs) + ms;
  }
}

/** The driver's serial queue's tail (private): a rename must fail here, not let `settle` stop waiting. */
function tailOf(driver: ChannelDriver): Promise<unknown> {
  const tail = (driver as unknown as { serial?: { tail?: unknown } }).serial?.tail;
  assert.ok(tail instanceof Promise, "ChannelDriver's `serial.tail` is what settle() waits on: was it renamed?");
  return tail;
}

/** Until neither driver has anything left to do: every frame and timer goes through its serial queue. */
async function settle(drivers: ChannelDriver[]): Promise<void> {
  for (;;) {
    const tails = drivers.map(tailOf);
    await Promise.all(tails);
    await new Promise((resolve) => setImmediate(resolve));
    if (drivers.every((d, i) => tailOf(d) === tails[i])) return;
  }
}

/**
 * A device's channel to an agent, as `MachineChannel` runs it (probing the hub
 * path, watched by `watchRelayPath`), over `down` (agent → device) and `up`.
 */
async function slowChannel(t: TestContext, downBytesPerSec: number, upBytesPerSec: number) {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const clock = { now: 0 };
  const down = new Lane(downBytesPerSec);
  const up = new Lane(upBytesPerSec);
  const [it, rt] = await transports();
  const device = new ChannelDriver(1, it, "initiator", (f) => up.put(clock.now, f), Date.now, { paths: { probeRelay: true }, clock: () => clock.now });
  const agent = new ChannelDriver(1, rt, "responder", (f) => down.put(clock.now, f), Date.now, { paths: { probeRelay: false }, clock: () => clock.now });
  const state = {
    closed: null as { atMs: number; code: number; reason: string } | null,
    wakes: 0,
    /** The oldest a hub probe got: past RELAY_SILENT_MS, its age alone would have closed the channel. */
    maxProbeAgeMs: 0,
    /** Runs on every paths change, after the watch. */
    onpaths: null as (() => void) | null,
  };
  const watch = watchRelayPath(device, { wake: () => void state.wakes++ });
  device.onpaths = (stats) => {
    state.maxProbeAgeMs = Math.max(state.maxProbeAgeMs, stats.relayProbeAgeMs ?? 0);
    watch(stats);
    state.onpaths?.();
  };
  device.onclose = (code, reason) => {
    state.closed ??= { atMs: clock.now, code, reason };
  };
  const drivers = [device, agent];
  await settle(drivers);
  return {
    device,
    agent,
    down,
    up,
    clock,
    state,
    /** Move the clock (and the timers) to `toMs`, handing over what the link delivers on the way. */
    async advance(toMs: number): Promise<void> {
      while (clock.now < toMs) {
        const step = Math.min(STEP_MS, toMs - clock.now);
        clock.now += step;
        t.mock.timers.tick(step);
        for (const f of down.take(clock.now)) device.receive(f);
        for (const f of up.take(clock.now)) agent.receive(f);
        await settle(drivers);
      }
    },
  };
}

/** The agent answers each stream the device opens with `bytes`; what the device reads is counted. */
function download(c: Awaited<ReturnType<typeof slowChannel>>, streams: number, bytes: number): { read: number } {
  c.agent.onincoming = (stream) => stream.send(new Uint8Array(bytes));
  const got = { read: 0 };
  for (let k = 0; k < streams; k++) c.device.open("rpc", utf8(`GET /history/${k}`)).ondata = (data) => void (got.read += data.length);
  return got;
}

/** The device sends `bytes` on each of `streams` streams; what the agent reads is counted. */
function upload(c: Awaited<ReturnType<typeof slowChannel>>, streams: number, bytes: number): { read: number } {
  const got = { read: 0 };
  c.agent.onincoming = (stream) => {
    stream.ondata = (data) => void (got.read += data.length);
  };
  for (let k = 0; k < streams; k++) c.device.open("rpc", utf8(`POST /upload/${k}`)).send(new Uint8Array(bytes));
  return got;
}

test("a slow download over the hub path is never taken for gone, however late its probes are answered", async (t) => {
  // 240 kbps: two history ranges fill the channel's window, ~15 s of it.
  const c = await slowChannel(t, 30_000, 30_000);
  const got = download(c, 2, 640 * 1024);
  await c.advance(60_000);
  assert.equal(got.read, 2 * 640 * 1024, "all of it arrived");
  assert.ok(c.state.maxProbeAgeMs >= RELAY_SILENT_MS, `a probe went ${c.state.maxProbeAgeMs} ms unanswered: its age alone would have closed the channel`);
  assert.equal(c.state.closed, null, "never replaced");
  assert.equal(c.state.wakes, 0);
});

test("a slow download through a 7.5 s stall is not taken for gone, whenever the stall comes", async (t) => {
  // A mobile link stalls. While the oldest probe was 10–15 s old, a rule
  // that took 5 s of silence for a death closed this channel at some start
  // times. The stall holds back the frame being sent too, and a 64 KiB frame
  // takes 2.2 s at this rate: the silence it leaves is up to 9.7 s, short of
  // RELAY_SILENT_MS (an 8 s stall can leave 10.2 s, and close it).
  for (let at = 10_000; at < 30_000; at += 1_000) {
    const c = await slowChannel(t, 30_000, 30_000);
    const got = download(c, 2, 640 * 1024);
    await c.advance(at);
    c.down.stall(c.clock.now, 7_500);
    await c.advance(70_000);
    assert.equal(c.state.closed, null, `a 7.5 s stall from ${at} ms closed it`);
    assert.equal(got.read, 2 * 640 * 1024, "all of it arrived");
    t.mock.timers.reset();
  }
});

test("a slow download whose relay dies is replaced within 15 s of the last thing it brought", async (t) => {
  const c = await slowChannel(t, 30_000, 30_000);
  download(c, 2, 640 * 1024);
  await c.advance(20_000);
  assert.equal(c.state.closed, null, "slow, not gone");
  // The relay goes, with what it held.
  c.up.cut = c.down.cut = true;
  c.up.lose();
  c.down.lose();
  await c.advance(60_000);
  const closed = c.state.closed;
  assert.ok(closed, "replaced");
  assert.equal(closed.code, RELAY_PATH_GONE);
  assert.equal(c.state.wakes, 1, "and its relay link woken, once");
  const after = closed.atMs - c.down.lastArrivalMs;
  assert.ok(after >= RELAY_SILENT_MS && after <= GONE_WITHIN_MS, `replaced ${after} ms after the last frame`);
});

test("a download whose agent goes away while the relay drains a backlog is replaced within 15 s of the backlog's end", async (t) => {
  const c = await slowChannel(t, 30_000, 30_000);
  download(c, 2, 640 * 1024);
  await c.advance(20_000);
  // The agent's side goes: nothing more gets on, what the relay holds for the device still drains (~15 s of it).
  c.down.cut = c.up.cut = true;
  c.up.lose();
  await c.advance(80_000);
  const closed = c.state.closed;
  assert.ok(closed, "replaced");
  assert.ok(c.down.lastArrivalMs >= 30_000, `the backlog drained until ${c.down.lastArrivalMs} ms`);
  const after = closed.atMs - c.down.lastArrivalMs;
  assert.ok(after >= RELAY_SILENT_MS && after <= GONE_WITHIN_MS, `replaced ${after} ms after the backlog's last frame, not while it drained`);
  assert.equal(c.state.wakes, 1);
});

test("a slow upload is never taken for gone: the agent's window updates and CREDITs are the hub path carrying", async (t) => {
  // 240 kbps up: the page's probes wait behind its own upload.
  const c = await slowChannel(t, 30_000, 30_000);
  const got = upload(c, 2, 512 * 1024);
  await c.advance(50_000);
  assert.equal(got.read, 2 * 512 * 1024, "all of it arrived");
  assert.ok(c.state.maxProbeAgeMs >= RELAY_SILENT_MS, `a probe went ${c.state.maxProbeAgeMs} ms unanswered`);
  assert.equal(c.state.closed, null, "never replaced");
});

test("a CREDIT the driver reads counts as the hub path carrying", async (t) => {
  // Only the agent's CREDITs reach the device (its probe answers and window
  // updates are held back here): what is pinned is that a CREDIT alone counts.
  const c = await slowChannel(t, 100_000, 100_000);
  c.down.only = (f) => f.type === "credit";
  upload(c, 8, 256 * 1024);
  await c.advance(20_000);
  assert.ok(c.state.maxProbeAgeMs >= RELAY_SILENT_MS, `no probe answered for ${c.state.maxProbeAgeMs} ms`);
  assert.equal(c.state.closed, null, "a CREDIT every 2.6 s keeps it");
});

test("a page uploading when its relay dies is replaced within 15 s, though its next probe cannot even leave", async (t) => {
  const c = await slowChannel(t, 100_000, 100_000);
  upload(c, 2, 8 * 1024 * 1024);
  await c.advance(10_000);
  // The relay dies right after a probe's answer came: nothing older is out,
  // and with the window spent and no CREDIT to come, the next probe stays queued.
  let rtt = c.device.paths!.relayRttMs;
  let diedAt: number | null = null;
  c.state.onpaths = () => {
    const now = c.device.paths!.relayRttMs;
    if (diedAt !== null || now === rtt) return;
    rtt = now;
    diedAt = c.clock.now;
    c.up.cut = c.down.cut = true;
    c.up.lose();
    c.down.lose();
  };
  await c.advance(70_000);
  assert.ok(diedAt !== null, "an answer came, and the relay died");
  const closed = c.state.closed;
  assert.ok(closed, "replaced");
  const after = closed.atMs - c.down.lastArrivalMs;
  assert.ok(after >= RELAY_SILENT_MS && after <= GONE_WITHIN_MS, `replaced ${after} ms after the last frame`);
});

