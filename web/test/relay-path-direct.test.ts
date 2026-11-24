// The page's watch on a channel's hub path (`lib/hub/machine.ts`
// `watchRelayPath`) while a direct path carries the channel, on two running
// drivers (`lib/e2e/driver.ts`) whose hub path and direct path share one slow
// access link, on a clock the test moves (mocked `setTimeout`).
//
// What these pin (a field report: a 1–2 Mbps cellular link, the channel on
// its direct path, closed 4000 "the relay path stopped answering" with
// something received 0.66 s before): while the direct path carries, the hub
// path carries nothing but its probes, and on a link the direct path fills
// they go unanswered for longer than RELAY_SILENT_MS — the channel is kept,
// direct path and all; a channel on which nothing arrives either way is still
// replaced as before; and once the direct path is let go, a hub path silent
// all that while is gone at once.
//
// The link is one FIFO per direction that both paths' traffic shares,
// letting so many bytes a second through, with 40 ms each way; the
// DataChannel's `bufferedAmount` is what its sender put on it and the FIFO
// has not sent yet.
import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import {
  ChannelDriver,
  DEFAULT_REKEY,
  DIRECT_TIMEOUT_MAX_MS,
  DIRECT_TIMEOUT_MS,
  type DirectLink,
  type Frame,
  generateDeviceKeys,
  Initiator,
  LOAD_FACTOR,
  MAX_PAYLOAD_LEN,
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

type Via = "relay" | "direct";

/** One direction of the access link both paths share: a FIFO letting `bytesPerSec` through, then 40 ms. */
class SharedLane {
  private freeAt = 0;
  private queue: { at: number; leftAt: number; len: number; via: Via; deliver: () => void }[] = [];
  /** Nothing more gets on that path (its end, or the relay, is gone). */
  readonly cut: Record<Via, boolean> = { relay: false, direct: false };
  /** When something last came off, on either path. */
  lastArrivalMs = 0;

  constructor(private readonly bytesPerSec: number) {}

  put(nowMs: number, via: Via, len: number, deliver: () => void): void {
    if (this.cut[via]) return;
    // What the WebSocket or SCTP, DTLS, UDP and IP add.
    const wire = len + 60;
    const leftAt = Math.max(this.freeAt, nowMs) + (wire * 1000) / this.bytesPerSec;
    this.freeAt = leftAt;
    this.queue.push({ at: leftAt + 40, leftAt, len, via, deliver });
  }

  /** The DataChannel's `bufferedAmount`: what was put on it and has not left yet. */
  buffered(nowMs: number): number {
    let n = 0;
    for (const q of this.queue) if (q.via === "direct" && q.leftAt > nowMs) n += q.len;
    return n;
  }

  /** Hand over what has come off by `nowMs`, in order. */
  deliverDue(nowMs: number): void {
    while (this.queue[0] && this.queue[0].at <= nowMs) {
      const q = this.queue.shift()!;
      this.lastArrivalMs = nowMs;
      q.deliver();
    }
  }

  /**
   * What that path had on the link is lost (its end, or the relay, went away
   * with it). The link time it took is not given back (`freeAt` stays): a
   * simulation that flaps a path often looks busier than it is.
   */
  lose(via: Via): void {
    this.queue = this.queue.filter((q) => q.via !== via);
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
 * A device's channel to an agent, as `MachineChannel` runs it (probing the
 * hub path, watched by `watchRelayPath`), with a direct path attached, both
 * over `down` (agent → device) and `up`; the agent answers each stream the
 * device opens with `bytes`.
 */
async function directChannel(t: TestContext, bytesPerSec: number) {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const clock = { now: 0 };
  const down = new SharedLane(bytesPerSec);
  const up = new SharedLane(bytesPerSec);
  const [it, rt] = await transports();
  const frameLen = (f: Frame) => (f.type === "data" ? f.payload.length : 16) + 7;
  let device: ChannelDriver | null = null;
  let agent: ChannelDriver | null = null;
  device = new ChannelDriver(1, it, "initiator", (f) => up.put(clock.now, "relay", frameLen(f), () => agent!.receive(f)), Date.now, {
    paths: { probeRelay: true },
    clock: () => clock.now,
  });
  agent = new ChannelDriver(1, rt, "responder", (f) => down.put(clock.now, "relay", frameLen(f), () => device!.receive(f)), Date.now, {
    paths: { probeRelay: false },
    clock: () => clock.now,
  });
  const state = {
    closed: null as { atMs: number; code: number } | null,
    wakes: 0,
    /** The oldest a hub probe got. */
    maxProbeAgeMs: 0,
    /** Whether the hub path was ever as old and as quiet as the relay rule alone closes a channel on (both RELAY_SILENT_MS). */
    relayAloneGone: false,
    /** When the device let its direct path go. */
    directReleasedAtMs: null as number | null,
    /** Runs on every change of the device's paths, after the watch. */
    onpaths: null as (() => void) | null,
  };
  const [dt, at] = await transports();
  const deviceLink: DirectLink = {
    send(message) {
      const copy = Uint8Array.from(message);
      up.put(clock.now, "direct", copy.length, () => agent!.receiveDirect(copy, agentLink));
    },
    bufferedAmount: () => up.buffered(clock.now),
    released() {
      state.directReleasedAtMs ??= clock.now;
    },
  };
  const agentLink: DirectLink = {
    send(message) {
      const copy = Uint8Array.from(message);
      down.put(clock.now, "direct", copy.length, () => device!.receiveDirect(copy, deviceLink));
    },
    bufferedAmount: () => down.buffered(clock.now),
  };
  agent.attachDirect(at, agentLink);
  device.attachDirect(dt, deviceLink);
  const watch = watchRelayPath(device, { wake: () => void state.wakes++ });
  device.onpaths = (stats) => {
    state.maxProbeAgeMs = Math.max(state.maxProbeAgeMs, stats.relayProbeAgeMs ?? 0);
    if ((stats.relayProbeAgeMs ?? 0) >= RELAY_SILENT_MS && (stats.relayQuietMs ?? 0) >= RELAY_SILENT_MS) state.relayAloneGone = true;
    watch(stats);
    state.onpaths?.();
  };
  device.onclose = (code) => {
    state.closed ??= { atMs: clock.now, code };
  };
  const drivers = [device, agent];
  await settle(drivers);
  const got = { read: 0 };
  return {
    device,
    agent,
    down,
    up,
    clock,
    state,
    got,
    /** The agent answers each of `streams` streams the device opens with `bytes`. */
    download(streams: number, bytes: number): void {
      agent!.onincoming = (stream) => stream.send(new Uint8Array(bytes));
      for (let k = 0; k < streams; k++) device!.open("rpc", utf8(`GET /history/${k}`)).ondata = (data) => void (got.read += data.length);
    },
    /** The device sends `bytes` on each of `streams` streams; what the agent reads is counted. */
    upload(streams: number, bytes: number): { read: number } {
      const sent = { read: 0 };
      agent!.onincoming = (stream) => {
        stream.ondata = (data) => void (sent.read += data.length);
      };
      for (let k = 0; k < streams; k++) device!.open("rpc", utf8(`POST /upload/${k}`)).send(new Uint8Array(bytes));
      return sent;
    },
    /** Move the clock (and the timers) to `toMs`, handing over what the link delivers on the way. */
    async advance(toMs: number): Promise<void> {
      while (clock.now < toMs) {
        const step = Math.min(STEP_MS, toMs - clock.now);
        clock.now += step;
        t.mock.timers.tick(step);
        down.deliverDue(clock.now);
        up.deliverDue(clock.now);
        // What a DataChannel's `bufferedamountlow` does: the drivers look again for room.
        device!.wake();
        agent!.wake();
        await settle(drivers);
      }
    },
    /** That path is gone both ways, with what it had on the link. */
    kill(via: Via): void {
      down.cut[via] = up.cut[via] = true;
      down.lose(via);
      up.lose(via);
    },
  };
}

test("a channel whose direct path carries is kept, however long its hub path's probes go unanswered behind that traffic", async (t) => {
  // 640 kbps, filled by the direct path's downloads (four streams, a megabyte of them in flight): the hub path's probe answers wait behind them.
  const c = await directChannel(t, 80_000);
  c.download(4, 1024 * 1024);
  await c.advance(60_000);
  assert.ok(c.device.paths!.direct, "the direct path carried it throughout");
  assert.equal(c.state.directReleasedAtMs, null);
  assert.ok(c.state.relayAloneGone, `the hub path was ${RELAY_SILENT_MS} ms old and quiet (its oldest probe ${c.state.maxProbeAgeMs} ms): the relay rule alone closed the channel there`);
  assert.equal(c.state.closed, null, "never closed under the direct path");
  assert.equal(c.state.wakes, 0);
  assert.equal(c.got.read, 4 * 1024 * 1024, "all of it arrived");
});

test("a channel on which nothing arrives either way is replaced as before, direct path or not", async (t) => {
  const c = await directChannel(t, 80_000);
  c.download(4, 1024 * 1024);
  await c.advance(20_000);
  // Both paths go, with what they had on the link.
  c.kill("relay");
  c.kill("direct");
  const lastMs = Math.max(c.down.lastArrivalMs, c.up.lastArrivalMs);
  await c.advance(60_000);
  const closed = c.state.closed;
  assert.ok(closed, "replaced");
  assert.equal(closed.code, RELAY_PATH_GONE);
  assert.equal(c.state.wakes, 1);
  const after = closed.atMs - c.down.lastArrivalMs;
  assert.ok(c.down.lastArrivalMs <= lastMs + STEP_MS, "nothing came after both went");
  assert.ok(after <= 15_000 + STEP_MS, `replaced ${after} ms after the last frame (within 15 s, as before)`);
});

test("once the direct path goes, a hub path that has been silent all along is gone at once", async (t) => {
  const c = await directChannel(t, 80_000);
  c.download(4, 1024 * 1024);
  await c.advance(15_000);
  // The relay dies; the direct path carries on for 20 s, and the channel with it.
  c.kill("relay");
  await c.advance(35_000);
  assert.equal(c.state.closed, null, "kept while the direct path carries");
  // Then the direct path goes dark: let go after DIRECT_TIMEOUT_MS of silence —
  // stretched by the message the download may have had on its way, at the
  // link's rate (`directDeadline`) — and the channel with it.
  // The hub path's numbers as the device let the direct path go: they alone judge it from then on.
  let ageAtRelease: number | null = null;
  c.state.onpaths = () => {
    const s = c.device.paths!;
    if (!s.direct && ageAtRelease === null) ageAtRelease = s.relayProbeAgeMs ?? 0;
  };
  c.kill("direct");
  await c.advance(60_000);
  const released = c.state.directReleasedAtMs;
  const closed = c.state.closed;
  assert.ok(released !== null, "the direct path was let go");
  assert.ok(ageAtRelease !== null);
  const silentMs = released - c.down.lastArrivalMs;
  const oneMessageMs = (LOAD_FACTOR * MAX_PAYLOAD_LEN) / (80_000 / 1000);
  assert.ok(silentMs > DIRECT_TIMEOUT_MS, `the download in flight stretched the deadline: after ${silentMs} ms of silence`);
  assert.ok(silentMs <= DIRECT_TIMEOUT_MS + 1.25 * oneMessageMs + 2 * STEP_MS, `by about one message at the link's rate (${oneMessageMs} ms): after ${silentMs} ms of silence`);
  assert.ok(silentMs <= DIRECT_TIMEOUT_MAX_MS + 2 * STEP_MS);
  assert.ok(closed, "replaced");
  assert.equal(closed.code, RELAY_PATH_GONE);
  // At once, or as soon as its oldest probe is RELAY_SILENT_MS old: the direct
  // path's probes, seconds late behind the download, can have pushed the
  // hub's oldest out (`MAX_PROBES`), and a younger one is what it is judged by.
  const waitMs = Math.max(0, RELAY_SILENT_MS - ageAtRelease);
  assert.ok(closed.atMs - released <= waitMs + STEP_MS, `replaced ${closed.atMs - released} ms after the direct path went (its oldest probe then ${ageAtRelease} ms old)`);
});

test("when the agent lets the direct path go, the hub path that brought its `down` is not taken for silent", async (t) => {
  // The page uploads over the direct path and fills the uplink: its hub
  // probes wait behind the upload, so the hub path reads old and quiet — the
  // direct path carries, so the channel is kept. The agent gives the direct
  // path up right then; its `down` comes over the hub path, which judges
  // alone from then on: by what it just delivered, not by the last probe
  // tick's numbers.
  const c = await directChannel(t, 80_000);
  const sent = c.upload(4, 1024 * 1024);
  let droppedAtMs: number | null = null;
  c.state.onpaths = () => {
    const s = c.device.paths!;
    if (droppedAtMs !== null || !s.direct || (s.relayProbeAgeMs ?? 0) < RELAY_SILENT_MS || (s.relayQuietMs ?? 0) < RELAY_SILENT_MS) return;
    droppedAtMs = c.clock.now;
    c.agent.detachDirect("failed");
  };
  await c.advance(90_000);
  assert.ok(droppedAtMs !== null, "the hub path read old and quiet while the direct path carried, and the agent let the direct path go then");
  assert.ok(c.state.directReleasedAtMs !== null, "the device heard it, and let the direct path go too");
  assert.equal(c.state.closed, null, "the hub path had just delivered the `down`: not gone");
  assert.equal(sent.read, 4 * 1024 * 1024, "all of it arrived, over the hub path in the end");
});
