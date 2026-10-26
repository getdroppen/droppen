// The page's one socket, and session feeds carried on it.
//
// What these pin: a server that cannot carry feeds is noticed from its first
// frame and every feed falls back to a socket of its own; one that can gets
// every waiting feed subscribed in one message with its resume marker, its
// frames routed by subscription and handed over parsed; compressed frames are
// read in order with the text around them; a dropped connection closes the
// channels it carried and the next one re-subscribes them; and silence closes a
// socket that only looks open.
import assert from "node:assert/strict";
import test from "node:test";
import { deflateRawSync, inflateRawSync } from "node:zlib";
import { compressionAllowed, FeedHub, HEARTBEAT_MS, hubBackoff, SILENCE_MS, type FeedHubDeps, type HubSocket } from "../src/app/feed-hub.js";
import { homeMachine, openMachine, transportFor } from "../src/lib/machines/registry.js";
import { createSessionFeedPool } from "../src/features/sessions/session-feed-pool.js";

class FakeSocket implements HubSocket {
  readyState = 0;
  binaryType = "blob";
  sent: Record<string, unknown>[] = [];
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: unknown) => void) | null = null;
  constructor(readonly url: string) {}
  send(data: string) {
    this.sent.push(JSON.parse(data));
  }
  close() {
    if (this.readyState === 3) return;
    this.readyState = 3;
    this.onclose?.({});
  }
  open() {
    this.readyState = 1;
    this.onopen?.({});
  }
  frame(value: unknown) {
    this.onmessage?.({ data: JSON.stringify(value) });
  }
  binary(value: unknown) {
    const bytes = deflateRawSync(Buffer.from(JSON.stringify(value)));
    this.onmessage?.({ data: bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) });
  }
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

/** Wait for `cond`; `ms` only keeps a broken test from hanging (it is no claim about how fast). */
async function until(cond: () => boolean, what: string, ms = 10_000): Promise<void> {
  const deadline = Date.now() + ms;
  while (!cond()) {
    if (Date.now() > deadline) throw new Error(`timed out: ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 2));
  }
}

function harness(overrides: Partial<FeedHubDeps> = {}) {
  const sockets: FakeSocket[] = [];
  const frames: Record<string, unknown>[] = [];
  const events: string[] = [];
  const hub = new FeedHub({
    socketUrl: async (path) => path,
    openSocket: (url) => {
      const socket = new FakeSocket(url);
      sockets.push(socket);
      return socket;
    },
    noteFailure: () => undefined,
    // Slower than a text frame on purpose, so ordering is actually tested.
    inflate: async (data) => {
      await new Promise((resolve) => setTimeout(resolve, 5));
      return inflateRawSync(Buffer.from(data)).toString();
    },
    random: () => 0,
    ...overrides,
  });
  hub.start({
    onFrame: (m) => frames.push(m),
    onOpen: () => events.push("open"),
    onClose: () => events.push("close"),
  });
  return { hub, sockets, frames, events };
}

test("a server that answers without hello carries no feeds, and every channel falls back", async (t) => {
  const { hub, sockets, frames } = harness();
  t.after(() => hub.stop());
  await flush();
  assert.equal(sockets[0]!.url, "/api/events?v=2&z=1");
  const channel = hub.open({ source: "claude", id: "a" })!;
  let closed = false;
  channel.onclose = () => (closed = true);
  sockets[0]!.open();
  sockets[0]!.frame({ type: "settings", hostname: "h" });
  assert.equal(hub.mode, "legacy");
  assert.equal(closed, true, "the waiting channel is told to go and open its own socket");
  assert.equal(hub.open({ source: "claude", id: "b" }), null, "and so is every later one");
  assert.equal(frames[0]!.type, "settings", "the list half works as it always did");
  // The subscription went out on open, before the server said what it was:
  // an old server ignores anything said on this socket, and asking early is a
  // round trip saved everywhere else. Nothing more is asked of it after that.
  assert.deepEqual(sockets[0]!.sent.map((m) => m.type), ["sub"]);
});

test("hello subscribes every waiting feed in one message and routes their frames parsed", async (t) => {
  const { hub, sockets, frames } = harness();
  t.after(() => hub.stop());
  await flush();
  const a = hub.open({ source: "claude", id: "a", since: 40, fp: "fp-a", limit: 100 })!;
  const b = hub.open({ source: "codex", id: "b" })!;
  const got: unknown[] = [];
  a.onmessage = (e) => got.push(e.data);
  let opened = 0;
  a.onopen = () => opened++;
  b.onopen = () => opened++;
  sockets[0]!.open();
  assert.equal(sockets[0]!.sent.length, 1, "asked for on open, without waiting for hello");
  assert.equal(opened, 0, "but not open until the server has said it understood");
  sockets[0]!.frame({ type: "hello", features: ["sub", "ping"], compress: "deflate-raw" });
  assert.equal(hub.mode, "mux");
  assert.equal(hub.compressed, true);
  assert.equal(opened, 2);
  assert.equal(sockets[0]!.sent.length, 1, "one message for both, and not again on hello");
  const [sub] = sockets[0]!.sent;
  const feeds = sub!.feeds as Record<string, unknown>[];
  assert.equal(sub!.type, "sub");
  assert.deepEqual(feeds[0], { sub: 1, source: "claude", id: "a", since: 40, fp: "fp-a", limit: 100 });
  assert.deepEqual(feeds[1], { sub: 2, source: "codex", id: "b" }, "no marker when nothing is held");

  sockets[0]!.frame({ sub: 1, type: "delta", fromIndex: 37 });
  sockets[0]!.frame({ type: "sessions:delta", upserts: [] });
  sockets[0]!.frame({ type: "pong" });
  assert.deepEqual(got, [{ sub: 1, type: "delta", fromIndex: 37 }], "an object, not text to parse again");
  assert.deepEqual(
    frames.map((f) => f.type),
    ["sessions:delta"],
    "the list's frames go to the list, and pongs to nobody",
  );

  // A channel opened while the socket is up is subscribed at once.
  hub.open({ source: "claude", id: "c" });
  assert.equal(sockets[0]!.sent.at(-1)!.type, "sub");
});

test("closing a channel unsubscribes it and drops its late frames; resync is addressed", async (t) => {
  const { hub, sockets } = harness();
  t.after(() => hub.stop());
  await flush();
  sockets[0]!.open();
  sockets[0]!.frame({ type: "hello", features: ["sub"], compress: null });
  const a = hub.open({ source: "claude", id: "a" })!;
  const got: unknown[] = [];
  a.onmessage = (e) => got.push(e.data);
  assert.equal(a.readyState, 0, "open is told after the caller has had the channel, as a socket's is");
  await flush();
  assert.equal(a.readyState, 1);
  a.send(JSON.stringify({ type: "resync", since: 5, fp: "x" }));
  assert.deepEqual(sockets[0]!.sent.at(-1), { type: "resync", since: 5, fp: "x", sub: 1 });
  let closes = 0;
  a.onclose = () => closes++;
  a.close();
  assert.deepEqual(sockets[0]!.sent.at(-1), { type: "unsub", subs: [1] });
  sockets[0]!.frame({ sub: 1, type: "state" });
  assert.deepEqual(got, [], "a frame already on the wire for a closed channel goes nowhere");
  await flush();
  assert.equal(closes, 1, "the close is reported, asynchronously, like a socket's");
});

test("compressed frames are read in order with the text frames around them", async (t) => {
  const { hub, sockets, frames } = harness();
  t.after(() => hub.stop());
  await flush();
  sockets[0]!.open();
  sockets[0]!.frame({ type: "hello", features: ["sub"], compress: "deflate-raw" });
  sockets[0]!.binary({ type: "sessions:snapshot", sessions: [], n: 1 });
  sockets[0]!.frame({ type: "sessions:delta", n: 2 });
  sockets[0]!.binary({ type: "sessions:delta", n: 3 });
  sockets[0]!.frame({ type: "sessions:delta", n: 4 });
  // Each inflate takes a timer's turn, so how long the four take depends on
  // the machine: wait for all of them, then look at the order.
  await until(() => frames.length >= 4, "all four frames read");
  assert.deepEqual(
    frames.map((f) => f.n),
    [1, 2, 3, 4],
  );
});

test("a dropped connection closes its channels, and the next one re-subscribes the feeds that come back", async (t) => {
  const { hub, sockets, events } = harness();
  t.after(() => hub.stop());
  await flush();
  sockets[0]!.open();
  sockets[0]!.frame({ type: "hello", features: ["sub"], compress: null });
  const a = hub.open({ source: "claude", id: "a" })!;
  let dropped = false;
  a.onclose = () => (dropped = true);
  await flush();
  sockets[0]!.close();
  assert.equal(dropped, true);
  assert.deepEqual(events, ["open", "close"]);

  // The feed comes back while the hub is still reconnecting: it waits.
  const again = hub.open({ source: "claude", id: "a", since: 9, fp: "f" })!;
  assert.equal(again.readyState, 0);
  await new Promise((resolve) => setTimeout(resolve, 300));
  assert.equal(sockets.length, 2, "reconnected after the backoff");
  sockets[1]!.open();
  assert.equal(sockets[1]!.sent.length, 1, "re-asked for the moment the new socket opens");
  sockets[1]!.frame({ type: "hello", features: ["sub"], compress: null });
  assert.equal(again.readyState, 1);
  assert.deepEqual((sockets[1]!.sent[0]!.feeds as unknown[])[0], { sub: 2, source: "claude", id: "a", since: 9, fp: "f" });
});

test("kick probes a socket that only looks open, and replaces it when nothing answers", async (t) => {
  const { hub, sockets } = harness({ probeMs: 20 });
  t.after(() => hub.stop());
  await flush();
  sockets[0]!.open();
  sockets[0]!.frame({ type: "hello", features: ["sub", "ping"], compress: null });
  hub.kick();
  assert.deepEqual(sockets[0]!.sent.at(-1), { type: "ping" }, "the page coming back asks the socket to prove it");
  await until(() => sockets.length === 2, "replaced after the wake probe", 1_500);
});

test("kick keeps a socket that answers the probe", async (t) => {
  const { hub, sockets } = harness({ probeMs: 20 });
  t.after(() => hub.stop());
  await flush();
  sockets[0]!.open();
  sockets[0]!.frame({ type: "hello", features: ["sub", "ping"], compress: null });
  hub.kick();
  sockets[0]!.frame({ type: "pong" });
  await new Promise((resolve) => setTimeout(resolve, 150));
  assert.equal(sockets.length, 1, "it answered: it lived through the pause");
  assert.equal(sockets[0]!.readyState, 1);
});

test("kick reconnects at once instead of at the end of the backoff", async (t) => {
  const { hub, sockets } = harness({ random: () => 1 });
  t.after(() => hub.stop());
  await flush();
  sockets[0]!.open();
  sockets[0]!.close();
  hub.kick();
  await flush();
  assert.equal(sockets.length, 2);
});

test("the backoff doubles to a ceiling, with jitter in its upper half", () => {
  assert.equal(hubBackoff(1, () => 0), 250);
  assert.equal(hubBackoff(1, () => 1), 500);
  assert.equal(hubBackoff(2, () => 1), 1000);
  assert.equal(hubBackoff(20, () => 1), 8000);
  assert.equal(hubBackoff(20, () => 0), 4000);
});

test("a feed pool on the hub opens no socket of its own and resumes over it", async (t) => {
  const { hub, sockets } = harness();
  t.after(() => hub.stop());
  const pool = createSessionFeedPool(
    {
      openFeed: (target) => hub.open(target),
      socketUrl: async () => {
        throw new Error("no socket of its own");
      },
      openSocket: () => {
        throw new Error("no socket of its own");
      },
      noteFailure: () => undefined,
      fetchHistory: async () => ({ status: 500 }),
      fetchState: async () => null,
    },
    { cached: 8, idleSockets: 0, idleMs: 60_000 },
  );
  t.after(() => {
    for (const feed of pool.held()) feed.stop();
  });
  await flush();
  sockets[0]!.open();
  sockets[0]!.frame({ type: "hello", features: ["sub"], compress: null });
  const feed = pool.feed("claude", "a");
  const release = pool.acquire(feed, { current: true });
  const [first] = sockets[0]!.sent.slice(-1);
  assert.deepEqual(first, { type: "sub", feeds: [{ sub: 1, source: "claude", id: "a", limit: 100 }] });
  sockets[0]!.frame({
    sub: 1,
    type: "snapshot",
    sig: "s",
    summary: { id: "a", source: "claude" },
    items: [{ kind: "message", role: "user", text: "hi" }],
    startIndex: 9,
    endIndex: 10,
    totalItems: 10,
    prefixFp: "fp",
  });
  assert.equal(feed.getView().data?.endIndex, 10);

  release(); // no idle sockets allowed: the subscription goes, the window stays
  assert.deepEqual(sockets[0]!.sent.at(-1), { type: "unsub", subs: [1] });
  pool.acquire(feed, { current: true });
  assert.equal(feed.getView().data?.endIndex, 10, "drawn at once");
  assert.deepEqual(sockets[0]!.sent.at(-1), {
    type: "sub",
    feeds: [{ sub: 2, source: "claude", id: "a", since: 10, fp: "fp", limit: 100 }],
  });
  assert.equal(sockets.length, 1, "one socket throughout");
});

test("a feed that opens a channel before the page starts the list starts the socket itself", async (t) => {
  const sockets: FakeSocket[] = [];
  const hub = new FeedHub({
    socketUrl: async (path) => path,
    openSocket: (url) => {
      const socket = new FakeSocket(url);
      sockets.push(socket);
      return socket;
    },
    noteFailure: () => undefined,
    inflate: null,
  });
  t.after(() => hub.stop());
  const channel = hub.open({ source: "claude", id: "a" })!;
  await flush();
  assert.equal(sockets.length, 1, "the transcript's effect runs before the provider's");
  assert.equal(sockets[0]!.url, "/api/events?v=2", "no compression asked for where it cannot be read");
  sockets[0]!.open();
  sockets[0]!.frame({ type: "hello", features: ["sub"], compress: null });
  sockets[0]!.frame({ type: "settings", hostname: "h" });
  assert.equal(channel.readyState, 1);
  const frames: Record<string, unknown>[] = [];
  let opened = 0;
  hub.start({ onFrame: (m) => frames.push(m), onOpen: () => opened++, onClose: () => undefined });
  assert.equal(opened, 1, "told the socket is already open");
  assert.deepEqual(frames, [{ type: "settings", hostname: "h" }], "and handed what arrived before it listened");
  assert.equal(sockets.length, 1, "one socket, not a second for the list");
});

test("a socket that only looks open is pinged, and closed once it has been silent too long", async (t) => {
  let now = 0;
  t.mock.timers.enable({ apis: ["setInterval"] });
  const { hub, sockets } = harness({ now: () => now });
  t.after(() => hub.stop());
  await flush();
  sockets[0]!.open();
  sockets[0]!.frame({ type: "hello", features: ["sub", "ping"], compress: null });
  now += HEARTBEAT_MS;
  t.mock.timers.tick(HEARTBEAT_MS);
  assert.deepEqual(sockets[0]!.sent.at(-1), { type: "ping" });
  sockets[0]!.frame({ type: "pong" });
  assert.equal(sockets[0]!.readyState, 1, "an answered ping keeps it");

  // Nothing at all for longer than the silence limit: a dead connection.
  now += SILENCE_MS + 1;
  t.mock.timers.tick(HEARTBEAT_MS);
  assert.equal(sockets[0]!.readyState, 3, "closed, so the reconnect starts now rather than whenever TCP notices");
});

// The reviewer's reproduction, made fast: a feed whose channel is opened on a
// hub that is already up used to be told "open" before it had set its
// `onopen`, so its retry streak never reset — every later hub drop grew its
// backoff towards eight seconds and set off an auth check every third time.
test("a channel opened on a live hub resets its feed's retry streak", async (t) => {
  const { hub, sockets } = harness();
  t.after(() => hub.stop());
  await flush();
  sockets[0]!.open();
  sockets[0]!.frame({ type: "hello", features: ["sub"], compress: null });
  const streaks: number[] = [];
  const pool = createSessionFeedPool(
    {
      openFeed: (target) => hub.open(target),
      socketUrl: async (path) => path,
      openSocket: () => {
        throw new Error("no socket of its own expected");
      },
      noteFailure: (streak) => streaks.push(streak),
      fetchHistory: async () => ({ status: 500 }),
      fetchState: async () => null,
    },
    { cached: 8, idleSockets: 3, idleMs: 60_000 },
  );
  t.after(() => {
    for (const feed of pool.held()) feed.stop();
  });
  pool.acquire(pool.feed("claude", "a"), { current: true });
  await flush();
  for (let i = 0; i < 3; i++) {
    sockets.at(-1)!.close(); // the hub drops; the feed backs off and comes back
    await new Promise((resolve) => setTimeout(resolve, 300)); // the hub's backoff (random 0: 250 ms)
    sockets.at(-1)!.open();
    sockets.at(-1)!.frame({ type: "hello", features: ["sub"], compress: null });
    await new Promise((resolve) => setTimeout(resolve, 600)); // the feed's (500 ms): onto a live hub
  }
  assert.deepEqual(streaks, [1, 1, 1], "each drop starts the streak over, because each reconnect was heard");
});

test("silence gives the socket up at once, even if it never reports closing", async (t) => {
  let now = 0;
  t.mock.timers.enable({ apis: ["setInterval"] });
  const { hub, sockets, events } = harness({ now: () => now });
  t.after(() => hub.stop());
  await flush();
  const stuck = sockets[0]!;
  stuck.close = () => {
    // A black-holed link: the browser sits in its closing handshake and says
    // nothing for a minute.
    stuck.readyState = 2;
  };
  stuck.open();
  stuck.frame({ type: "hello", features: ["sub"], compress: null });
  const channel = hub.open({ source: "claude", id: "a" })!;
  let dropped = false;
  channel.onclose = () => (dropped = true);
  await flush();
  now += SILENCE_MS + 1;
  t.mock.timers.tick(HEARTBEAT_MS);
  assert.equal(dropped, true, "its feeds are told now, not when the browser gets round to it");
  assert.deepEqual(events, ["open", "close"]);
  hub.kick();
  await flush();
  assert.equal(sockets.length, 2, "and a kick reconnects straight away");
});

test("a frame still decoding for a replaced socket neither holds up nor leaks into the next", async (t) => {
  let release: (() => void) | undefined;
  const { hub, sockets, frames } = harness({
    inflate: (data) =>
      new Promise((resolve) => {
        release = () => resolve(inflateRawSync(Buffer.from(data)).toString());
      }),
  });
  t.after(() => hub.stop());
  await flush();
  sockets[0]!.open();
  sockets[0]!.frame({ type: "hello", features: ["sub"], compress: "deflate-raw" });
  sockets[0]!.binary({ type: "sessions:snapshot", sessions: [], from: "old" });
  sockets[0]!.close();
  await new Promise((resolve) => setTimeout(resolve, 300));
  sockets[1]!.open();
  sockets[1]!.frame({ type: "hello", features: ["sub"], compress: "deflate-raw" });
  sockets[1]!.frame({ type: "sessions:delta", from: "new" });
  assert.deepEqual(
    frames.map((f) => f.from),
    ["new"],
    "read at once: the old socket's queue is not the new one's",
  );
  release?.();
  await flush();
  await flush();
  assert.deepEqual(
    frames.map((f) => f.from),
    ["new"],
    "and the old frame, decoded late, goes nowhere",
  );
});

// Cloud mode: the socket is a pipe of the end-to-end channel through the hub,
// and deflate next to secrets is a length side channel (CRIME) for whoever
// sees the ciphertext. It never asks for `z=1` there, and takes no compressed
// frame it did not ask for.
test("in cloud mode the socket asks for no compression, and refuses a compressed frame", async (t) => {
  const g = globalThis as { document?: unknown };
  const before = g.document;
  g.document = {
    querySelector: (selector: string) => (selector === 'meta[name="rh-hub"]' ? { getAttribute: () => "wss://hub.test" } : null),
  };
  t.after(() => {
    g.document = before;
  });
  assert.equal(compressionAllowed(transportFor(homeMachine())), false);
  const { hub, sockets, frames } = harness();
  t.after(() => hub.stop());
  await flush();
  assert.equal(sockets[0]!.url, "/api/events?v=2", "no z=1 through the hub");
  sockets[0]!.open();
  // A server that says it compresses anyway is not believed…
  sockets[0]!.frame({ type: "hello", features: ["sub", "ping"], compress: "deflate-raw" });
  assert.equal(hub.compressed, false);
  sockets[0]!.frame({ type: "sessions:snapshot", sessions: [], n: 1 });
  // …and a compressed frame on this socket is refused: the socket starts over.
  sockets[0]!.binary({ type: "sessions:delta", n: 2 });
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.deepEqual(frames.map((f) => f.n), [1], "the compressed frame was not read");
  assert.equal(sockets[0]!.readyState, 3, "and the socket was closed");
});

test("a tunneled transport, or a caller that says so, also means no compression; the page's own server keeps it", async (t) => {
  assert.equal(compressionAllowed(transportFor(homeMachine())), true, "local mode: the page talks to its own server");
  const direct = transportFor(homeMachine());
  openMachine(homeMachine(), { ...direct, tunneled: true });
  t.after(() => openMachine(homeMachine(), direct));
  assert.equal(compressionAllowed(transportFor(homeMachine())), false, "a transport through a channel");
  openMachine(homeMachine(), direct);
  {
    const { hub, sockets } = harness();
    await flush();
    assert.equal(sockets[0]!.url, "/api/events?v=2&z=1");
    hub.stop();
  }
  {
    const { hub, sockets } = harness({ compress: () => false });
    await flush();
    assert.equal(sockets[0]!.url, "/api/events?v=2");
    hub.stop();
  }
});

/** A feed pool over `hub`, as `session-detail.tsx` makes one. */
function poolOver(hub: FeedHub, idleSockets = 2) {
  return createSessionFeedPool(
    {
      openFeed: (target) => hub.open(target),
      socketUrl: async () => {
        throw new Error("no socket of its own");
      },
      openSocket: () => {
        throw new Error("no socket of its own");
      },
      noteFailure: () => undefined,
      fetchHistory: async () => ({ status: 500 }),
      fetchState: async () => null,
    },
    { cached: 8, idleSockets, idleMs: 60_000 },
  );
}

test("a tree going away cleans up the list before the transcript in it: the transcript's feed does not bring the socket back", async (t) => {
  const { hub, sockets } = harness();
  const pool = poolOver(hub);
  t.after(() => {
    for (const feed of pool.held()) feed.stop();
    hub.stop();
  });
  await flush();
  sockets[0]!.open();
  sockets[0]!.frame({ type: "hello", features: ["sub"], compress: null });
  const feed = pool.feed("claude", "a");
  const release = pool.acquire(feed, { current: true });
  await flush();
  assert.equal(sockets.length, 1);

  // React, deleting the tree: the provider's cleanup first (parent first, in
  // a deleted subtree), then the transcript's.
  hub.stop();
  release();
  // The feed's channel was dropped while it was still watched: it retries
  // after half a second. Nothing may open a socket for it.
  await new Promise((resolve) => setTimeout(resolve, 700));
  assert.equal(sockets.length, 1, "no socket reopened for a list that has stopped");
  assert.equal(sockets[0]!.readyState, 3);
});

test("away and back: the list comes back from a new connection's snapshot, never from frames kept while nobody listened", async (t) => {
  const { hub, sockets, frames } = harness();
  const pool = poolOver(hub);
  t.after(() => {
    for (const feed of pool.held()) feed.stop();
    hub.stop();
  });
  await flush();
  sockets[0]!.open();
  sockets[0]!.frame({ type: "hello", features: ["sub"], compress: null });
  sockets[0]!.frame({ type: "sessions:snapshot", sessions: [{ id: "before" }] });
  const release = pool.acquire(pool.feed("claude", "a"), { current: true });
  await flush();
  hub.stop();
  release();
  await new Promise((resolve) => setTimeout(resolve, 700));
  // Whatever socket exists now and says anything, says it to nobody.
  for (const socket of sockets.slice(1)) {
    socket.open();
    socket.frame({ type: "hello", features: ["sub"], compress: null });
    socket.frame({ type: "sessions:snapshot", sessions: [{ id: "stale" }] });
    for (let i = 0; i < 40; i++) socket.frame({ type: "sessions:delta", upserts: [{ id: `stale-${i}` }] });
  }

  // Back: the list starts again, on a connection of its own, and the feed back on screen rides it.
  frames.length = 0;
  hub.start({ onFrame: (m) => frames.push(m), onOpen: () => undefined, onClose: () => undefined });
  pool.acquire(pool.feed("claude", "a"), { current: true });
  await flush();
  const now = sockets.at(-1)!;
  assert.equal(now.readyState, 0, "a new connection, opened on the way back");
  now.open();
  now.frame({ type: "hello", features: ["sub"], compress: null });
  now.frame({ type: "sessions:snapshot", sessions: [{ id: "fresh" }] });
  await flush();
  assert.deepEqual(frames, [{ type: "sessions:snapshot", sessions: [{ id: "fresh" }] }], "only what the new connection said");
  assert.ok(now.sent.some((m) => m.type === "sub"), "the feed is subscribed on it");
});

test("a list that starts on a socket a feed opened gets a new connection if more arrived than was kept", async (t) => {
  const sockets: FakeSocket[] = [];
  const hub = new FeedHub({
    socketUrl: async (path) => path,
    openSocket: (url) => {
      const socket = new FakeSocket(url);
      sockets.push(socket);
      return socket;
    },
    noteFailure: () => undefined,
    inflate: null,
  });
  t.after(() => hub.stop());
  const channel = hub.open({ source: "claude", id: "a" })!;
  await flush();
  sockets[0]!.open();
  sockets[0]!.frame({ type: "hello", features: ["sub"], compress: null });
  sockets[0]!.frame({ type: "sessions:snapshot", sessions: [{ id: "old" }] });
  for (let i = 0; i < 40; i++) sockets[0]!.frame({ type: "sessions:delta", upserts: [{ id: `d-${i}` }] });
  const frames: Record<string, unknown>[] = [];
  hub.start({ onFrame: (m) => frames.push(m), onOpen: () => undefined, onClose: () => undefined });
  await flush();
  assert.deepEqual(frames, [], "nothing of a list that is missing frames is handed over");
  assert.equal(sockets[0]!.readyState, 3);
  assert.equal(sockets.length, 2, "a new connection instead, for a snapshot of the list as it is now");
  assert.equal(channel.readyState, 3, "the feed's channel is dropped with the old connection: its feed resumes on the new one");
  sockets[1]!.open();
  sockets[1]!.frame({ type: "hello", features: ["sub"], compress: null });
  sockets[1]!.frame({ type: "sessions:snapshot", sessions: [{ id: "now" }] });
  assert.deepEqual(frames, [{ type: "sessions:snapshot", sessions: [{ id: "now" }] }]);
});
