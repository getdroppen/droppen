// The page's hub connection (`lib/hub/link.ts`) noticing that it died
// without a word: a phone's network can drop a TCP connection so that no
// close ever arrives, and the socket then sits "open" while everything sent
// on it is lost — the channel over it, and every request on the channel,
// waiting for nothing.
//
// What these pin: a connection that stops bringing anything (the hub answers
// every ping) is replaced within SILENCE_MS and a beat, and its channels hear
// LINK_LOST at once; one that keeps answering is kept; and a beat that comes
// far later than due (this page's timers held in a background tab) gives the
// connection one more interval instead of dropping it for a silence nobody
// was there to hear. The relay link (`lib/hub/relay-link.ts`) takes a late
// beat the same way.
import assert from "node:assert/strict";
import test from "node:test";
import { decodeFrame, encodeFrame, type Frame, generateDeviceKeys } from "../src/lib/e2e/index.js";
import { HubLink, LINK_LOST } from "../src/lib/hub/link.js";
import { RelayLink } from "../src/lib/hub/relay-link.js";

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

async function until(what: () => boolean, label: string, timeoutMs = 5_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!what()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`);
    await sleep(2);
  }
}

/** A hub socket: signs the page in, answers pings while `answering`, and, half-open, says nothing at all. */
class HubSocket {
  binaryType = "blob";
  readyState = 0;
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: { code?: number; reason?: string }) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  answering = true;
  /** Sign the page in, then say nothing at all (a hub that took the sign-in and hung). */
  silentAfterWelcome = false;
  /** Say where the machines are a moment after the welcome, as the hub does. */
  presenceAfterWelcome = false;
  /** Unreachable: closes before its challenge (the phone is offline). */
  unreachable = false;
  readonly openedAt = performance.now();
  pings = 0;
  closed: number | null = null;

  constructor() {
    setTimeout(() => {
      if (this.unreachable) return this.onclose?.({ code: 1006 });
      this.readyState = 1;
      this.onopen?.({});
      this.text({ t: "challenge", nonce: "00".repeat(32) });
    }, 0);
  }

  private text(m: unknown): void {
    if (this.answering) this.onmessage?.({ data: JSON.stringify(m) });
  }

  send(data: string | ArrayBufferLike | ArrayBufferView): void {
    if (typeof data === "string") {
      if ((JSON.parse(data) as { t?: string }).t === "login")
        setTimeout(() => {
          this.text({ t: "welcome" });
          if (this.silentAfterWelcome) this.answering = false;
          if (this.presenceAfterWelcome) setTimeout(() => this.text({ t: "presence", machines: [{ id: "m-1", state: "online", since_ms: 1, via: [{ relay: "r-1" }] }] }), 5);
        }, 0);
      return;
    }
    const frame = decodeFrame(data as Uint8Array);
    if (frame.type !== "ping") return;
    this.pings++;
    const pong = encodeFrame({ type: "pong", nonce: frame.nonce } as Frame);
    setTimeout(() => {
      if (this.answering) this.onmessage?.({ data: pong.buffer.slice(pong.byteOffset, pong.byteOffset + pong.byteLength) });
    }, 0);
  }

  /** A half-open socket never finishes a close: no `onclose` comes. */
  close(code?: number): void {
    this.closed = code ?? 1000;
  }
}

async function linkWith(
  t: { after(fn: () => void): void },
  opts: { heartbeatMs: number; silenceMs: number; stableMs?: number; now?: () => number; random?: () => number },
  configure: (socket: HubSocket) => void = () => undefined,
) {
  const sockets: HubSocket[] = [];
  const keys = await generateDeviceKeys();
  const link = new HubLink({
    url: "wss://hub.test/v1/client",
    origin: "https://hub.test",
    account: "a-1",
    deviceId: "d-1",
    token: "session",
    signKey: keys.sign.privateKey,
    makeSocket: () => {
      const s = new HubSocket();
      configure(s);
      sockets.push(s);
      return s;
    },
    random: () => 0,
    ...opts,
  });
  t.after(() => link.stop());
  link.start();
  await link.waitOpen(5_000);
  return { link, sockets };
}

test("a hub connection that goes silent without closing is replaced, and its channels hear LINK_LOST at once", async (t) => {
  const heartbeatMs = 40;
  const silenceMs = 100;
  const { link, sockets } = await linkWith(t, { heartbeatMs, silenceMs });
  const heard: Frame[] = [];
  link.reserve((f) => heard.push(f));
  await sleep(heartbeatMs * 3);
  assert.ok(sockets[0]!.pings >= 2, "it pings");
  assert.equal(sockets.length, 1, "and keeps a connection that answers");

  // The network drops the connection without a word: nothing more arrives, and no close.
  const wentSilent = Date.now();
  sockets[0]!.answering = false;
  await until(() => sockets.length === 2 && link.state === "open", "a new connection, signed in");
  const tookMs = Date.now() - wentSilent;
  assert.ok(tookMs < silenceMs + heartbeatMs * 3, `replaced in ${tookMs} ms`);
  assert.equal(sockets[0]!.closed, 4000, "the silent socket is closed on the way out");
  assert.deepEqual(
    heard.map((f) => (f.type === "close" ? f.code : f.type)),
    [LINK_LOST],
    "the channel on it is told it is gone, rather than waiting on it",
  );
});

test("a connection that keeps answering is kept, however long", async (t) => {
  const { sockets } = await linkWith(t, { heartbeatMs: 30, silenceMs: 75 });
  await sleep(600);
  assert.equal(sockets.length, 1);
  assert.ok(sockets[0]!.pings >= 10);
});

test("a beat far later than due (timers held in the background) gives the connection another interval, not a drop", async (t) => {
  let offset = 0;
  const heartbeatMs = 40;
  const { link, sockets } = await linkWith(t, { heartbeatMs, silenceMs: 100, now: () => Date.now() + offset });
  await sleep(heartbeatMs * 2);
  // Ten minutes pass with this page asleep: no beat ran, nothing was read.
  offset += 10 * 60_000;
  await sleep(heartbeatMs * 6);
  assert.equal(sockets.length, 1, "the connection answered the ping sent on waking, so it stays");
  assert.equal(link.state, "open");
  // And if it does not answer after waking, it goes one interval later.
  sockets[0]!.answering = false;
  offset += 10 * 60_000;
  await until(() => sockets.length === 2, "replaced once the wake-up ping went unanswered");
});

test("a hub that signs the page in and then says nothing is retried ever less often, not reset to the shortest wait", async (t) => {
  // Every connection is dropped for silence a little after it opened — later
  // than `stableMs`, which used to count it as a connection that held and
  // reset the backoff, so the page hammered such a hub at the shortest wait.
  // random 0.99: each wait is (nearly) the whole backoff, 0.5 s doubling.
  const { sockets } = await linkWith(t, { heartbeatMs: 20, silenceMs: 50, stableMs: 40, random: () => 0.99 }, (s) => (s.silentAfterWelcome = true));
  await until(() => sockets.length >= 4, "three reconnects", 8_000);
  const gaps = sockets.slice(1, 4).map((s, i) => s.openedAt - sockets[i]!.openedAt);
  assert.ok(gaps[1]! > gaps[0]! * 1.5 && gaps[2]! > gaps[1]! * 1.5, `the waits grow: ${gaps.map(Math.round).join(", ")} ms`);
});

test("after waking, a connection that does not answer is replaced one interval later, not after a whole silence", async (t) => {
  let offset = 0;
  const heartbeatMs = 40;
  const silenceMs = 400;
  const { sockets } = await linkWith(t, { heartbeatMs, silenceMs, now: () => Date.now() + offset });
  await sleep(heartbeatMs * 2);
  // The page sleeps for ten minutes, and the connection died meanwhile.
  sockets[0]!.answering = false;
  const woke = Date.now();
  offset += 10 * 60_000;
  await until(() => sockets.length === 2, "replaced");
  const tookMs = Date.now() - woke;
  // The late beat (within one interval) pings and allows one more interval; a
  // grace of the whole silence would take ten intervals.
  assert.ok(tookMs < heartbeatMs * 4, `replaced ${tookMs} ms after waking, within about two intervals, not ${silenceMs} ms`);
});

test("what the hub says of the machines is fresh once said on this connection, and in doubt after a pause until the connection answers", async (t) => {
  let offset = 0;
  const heartbeatMs = 40;
  const { link, sockets } = await linkWith(t, { heartbeatMs, silenceMs: 400, now: () => Date.now() + offset }, (s) => (s.presenceAfterWelcome = true));
  assert.equal(link.presenceFresh, false, "signed in, but the hub has not said where the machines are yet");
  assert.equal(await link.waitPresence(1_000), true);
  assert.deepEqual(link.presence.get("m-1")?.via, [{ relay: "r-1", rttMs: null }]);

  // Ten minutes asleep; the page is back. The connection may have died meanwhile: in doubt until it answers.
  offset += 10 * 60_000;
  link.wake();
  assert.equal(link.presenceFresh, false, "what it said before the pause is not taken as what holds now");
  assert.equal(await link.waitPresence(1_000), true, "it answered: it lived through the pause, and said every change meanwhile");
  assert.equal(sockets.length, 1);

  // Asleep again, and this time the connection died: replaced, and fresh once the new one's hub says where the machines are.
  sockets[0]!.answering = false;
  offset += 10 * 60_000;
  link.wake();
  assert.equal(link.presenceFresh, false);
  assert.equal(await link.waitPresence(2_000), true);
  assert.equal(sockets.length, 2);
  // Nothing said in time: `false`, not a hang.
  sockets[1]!.answering = false;
  offset += 10 * 60_000;
  link.wake();
  assert.equal(await link.waitPresence(10), false);
});

test("a link that is down reconnects the moment the page comes back, not at the end of its backoff", async (t) => {
  let offline = false;
  // random 0.99: each wait is (nearly) the whole backoff, 0.5 s doubling.
  const { link, sockets } = await linkWith(t, { heartbeatMs: 1_000, silenceMs: 2_500, random: () => 0.99 }, (s) => (s.unreachable = offline));
  offline = true;
  sockets[0]!.onclose?.({ code: 1006 });
  await until(() => sockets.length === 3 && link.state === "closed", "two tries while offline", 5_000);
  // Online again, the page back in view: the next try would be 2 s away.
  offline = false;
  const woke = Date.now();
  link.wake();
  await link.waitOpen(5_000);
  const tookMs = Date.now() - woke;
  assert.ok(tookMs < 500, `signed in again ${tookMs} ms after the page came back`);
  assert.equal(sockets.length, 4);
});

/** A relay socket: lets the device in, and answers pings while `answering`. */
class RelaySocket {
  binaryType = "blob";
  readyState = 0;
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: { code?: number; reason?: string }) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  answering = true;

  constructor() {
    setTimeout(() => {
      this.readyState = 1;
      this.onopen?.({});
      this.onmessage?.({ data: JSON.stringify({ t: "challenge", nonce: "00".repeat(32), relay: "r-1", origin: "wss://relay.test", version: 1 }) });
    }, 0);
  }

  send(data: string | ArrayBufferLike | ArrayBufferView): void {
    if (typeof data === "string") {
      if ((JSON.parse(data) as { t?: string }).t === "login") setTimeout(() => this.onmessage?.({ data: JSON.stringify({ t: "welcome" }) }), 0);
      return;
    }
    const frame = decodeFrame(data as Uint8Array);
    if (frame.type !== "ping") return;
    const pong = encodeFrame({ type: "pong", nonce: frame.nonce });
    setTimeout(() => {
      if (this.answering) this.onmessage?.({ data: pong.buffer.slice(pong.byteOffset, pong.byteOffset + pong.byteLength) });
    }, 0);
  }

  close(): void {}
}

test("the relay link, too, gives a late beat another interval, and drops an unanswering relay one interval after waking", async (t) => {
  let offset = 0;
  const heartbeatMs = 40;
  const sockets: RelaySocket[] = [];
  const keys = await generateDeviceKeys();
  const link = new RelayLink({
    relay: "r-1",
    origin: "wss://relay.test",
    account: "a-1",
    deviceId: "d-1",
    signKey: keys.sign.privateKey,
    ticket: async () => "ticket",
    makeSocket: () => {
      const s = new RelaySocket();
      sockets.push(s);
      return s;
    },
    now: () => Date.now() + offset,
    heartbeatMs,
    silenceMs: 400,
  });
  t.after(() => link.close());
  await link.open(5_000);
  await sleep(heartbeatMs * 2);
  // Ten minutes asleep, the relay still there: the late beat pings, the answer comes, it stays.
  offset += 10 * 60_000;
  await sleep(heartbeatMs * 4);
  assert.equal(link.state, "open", "not dropped for a silence nobody was there to hear");
  // Asleep again, and this time the relay is gone: dropped one interval after waking, not a whole silence.
  sockets[0]!.answering = false;
  const woke = Date.now();
  offset += 10 * 60_000;
  await until(() => link.state === "closed", "dropped");
  const tookMs = Date.now() - woke;
  assert.ok(tookMs < heartbeatMs * 4, `dropped ${tookMs} ms after waking, within about two intervals`);
});
