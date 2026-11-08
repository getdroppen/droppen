// The page's session feeds: what a switch between sessions costs on the wire.
//
// A view used to own its socket, so every switch was a new connection and a
// whole snapshot, and switching back paid it again. These pin the two tiers that
// replaced that — a few sockets kept open for the sessions just read, and a
// longer tail of windows that a reopened socket resumes from — and the rule that
// a feed nobody is looking at never reconnects on its own.
import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import {
  createSessionFeedPool,
  type FeedPoolDeps,
  type FeedSocket,
} from "../src/features/sessions/session-feed-pool.js";

class FakeSocket implements FeedSocket {
  readyState = 0;
  closed = false;
  sent: string[] = [];
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: unknown) => void) | null = null;
  constructor(readonly url: string) {}
  send(data: string) {
    this.sent.push(data);
  }
  close() {
    if (this.closed) return;
    this.closed = true;
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
  /** The server going away under the socket. */
  drop() {
    this.close();
  }
}

function harness() {
  const sockets: FakeSocket[] = [];
  const deps: FeedPoolDeps = {
    socketUrl: async (path) => path,
    openSocket: (url) => {
      const socket = new FakeSocket(url);
      sockets.push(socket);
      return socket;
    },
    noteFailure: () => undefined,
    fetchHistory: async () => ({ status: 500, error: "unused" }),
    fetchState: async () => null,
  };
  return { sockets, deps };
}

/** A pool whose feeds are all stopped when the test ends, so no idle timer outlives it. */
function poolFor(t: TestContext, deps: FeedPoolDeps, limits: Parameters<typeof createSessionFeedPool>[1]) {
  const pool = createSessionFeedPool(deps, limits);
  t.after(() => {
    for (const feed of pool.held()) feed.stop();
  });
  return pool;
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

function snapshot(id: string, endIndex: number) {
  const items = Array.from({ length: 3 }, (_, i) => ({ kind: "message", role: "user", text: `${id} ${i}` }));
  return {
    type: "snapshot",
    sig: `${id}:sig`,
    summary: { id, source: "claude", title: id, status: "idle" },
    items,
    startIndex: endIndex - items.length,
    endIndex,
    totalItems: endIndex,
    hasMoreBefore: endIndex > items.length,
    prefixFp: `${id}-fp`,
    startFp: null,
  };
}

const pin = () => ({ current: true });

test("switching back draws the kept window at once and resumes the socket instead of re-snapshotting", async (t) => {
  const { sockets, deps } = harness();
  const pool = poolFor(t, deps, { cached: 8, idleSockets: 0, idleMs: 60_000 });
  const a = pool.feed("claude", "a");
  const releaseA = pool.acquire(a, pin());
  await flush();
  assert.equal(sockets.length, 1);
  assert.equal(sockets[0]!.url, "/api/sessions/claude/a/ws", "a first visit holds nothing to resume from");
  sockets[0]!.open();
  sockets[0]!.frame(snapshot("a", 40));
  const shown = a.getView().data;
  assert.ok(shown, "the snapshot is the window");

  releaseA();
  // No idle sockets allowed here, so leaving closes it — and keeps the window.
  assert.equal(sockets[0]!.closed, true);
  assert.equal(pool.feed("claude", "a"), a, "the same feed is handed back");
  assert.equal(a.getView().data, shown, "the window outlives the socket");

  pool.acquire(a, pin());
  assert.equal(a.getView().data, shown, "drawn from the kept window before anything is sent");
  await flush();
  assert.equal(sockets.length, 2);
  assert.equal(sockets[1]!.url, "/api/sessions/claude/a/ws?since=40&fp=a-fp", "the reopened socket resumes");
});

test("the last few sessions keep their sockets open, and the oldest idle one is closed first", async (t) => {
  const { sockets, deps } = harness();
  const pool = poolFor(t, deps, { cached: 8, idleSockets: 2, idleMs: 60_000 });
  const opened = new Map<string, FakeSocket>();
  for (const id of ["a", "b", "c", "d"]) {
    const feed = pool.feed("claude", id);
    const release = pool.acquire(feed, pin());
    await flush();
    const socket = sockets.at(-1)!;
    opened.set(id, socket);
    socket.open();
    socket.frame(snapshot(id, 10));
    if (id !== "d") release();
  }
  // d is on screen; c and b are the two idle sockets allowed; a was closed.
  assert.equal(opened.get("a")!.closed, true, "the oldest idle socket goes first");
  assert.equal(opened.get("b")!.closed, false);
  assert.equal(opened.get("c")!.closed, false);
  assert.equal(opened.get("d")!.closed, false);

  // Back to b: no new socket at all.
  const before = sockets.length;
  pool.acquire(pool.feed("claude", "b"), pin());
  await flush();
  assert.equal(sockets.length, before, "switching back to a pooled session opens nothing");
});

test("an idle socket closes after the idle time, keeping its window", async (t) => {
  const { sockets, deps } = harness();
  const pool = poolFor(t, deps, { cached: 8, idleSockets: 3, idleMs: 20 });
  const a = pool.feed("claude", "a");
  const release = pool.acquire(a, pin());
  await flush();
  sockets[0]!.open();
  sockets[0]!.frame(snapshot("a", 5));
  release();
  assert.equal(sockets[0]!.closed, false, "kept open for a quick switch back");
  await new Promise((resolve) => setTimeout(resolve, 40));
  assert.equal(sockets[0]!.closed, true, "and let go once nobody came back");
  assert.ok(a.getView().data, "the window stays for the next resume");
});

test("a hidden feed whose socket drops is not reconnected until it is shown again", async (t) => {
  const { sockets, deps } = harness();
  const pool = poolFor(t, deps, { cached: 8, idleSockets: 3, idleMs: 60_000 });
  const a = pool.feed("claude", "a");
  const release = pool.acquire(a, pin());
  await flush();
  sockets[0]!.open();
  sockets[0]!.frame(snapshot("a", 7));
  release();
  sockets[0]!.drop();
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(sockets.length, 1, "nobody is looking, so nothing reconnects");
  assert.equal(a.running, false);

  pool.acquire(a, pin());
  await flush();
  assert.equal(sockets.length, 2);
  assert.equal(sockets[1]!.url, "/api/sessions/claude/a/ws?since=7&fp=a-fp");
});

test("a pooled socket keeps the hidden window current", async (t) => {
  const { sockets, deps } = harness();
  const pool = poolFor(t, deps, { cached: 8, idleSockets: 3, idleMs: 60_000 });
  const a = pool.feed("claude", "a");
  const release = pool.acquire(a, pin());
  await flush();
  sockets[0]!.open();
  sockets[0]!.frame(snapshot("a", 3));
  release();
  sockets[0]!.frame({
    type: "delta",
    sig: "a:sig2",
    fromIndex: 3,
    items: [{ kind: "message", role: "assistant", text: "while away" }],
    endIndex: 4,
    totalItems: 4,
    prefixFp: "a-fp2",
  });
  sockets[0]!.frame({ type: "state", canSend: true });
  const data = a.getView().data!;
  assert.equal(data.endIndex, 4, "the delta landed while hidden");
  assert.equal(data.items.length, 4);
  assert.equal(data.canSend, true, "and so did the state");
});

test("past the cache size the oldest windows are dropped, never one on screen", async (t) => {
  const { sockets, deps } = harness();
  const pool = poolFor(t, deps, { cached: 2, idleSockets: 0, idleMs: 60_000 });
  const first = pool.feed("claude", "first");
  pool.acquire(first, pin()); // stays on screen throughout
  for (const id of ["a", "b", "c"]) {
    const feed = pool.feed("claude", id);
    const release = pool.acquire(feed, pin());
    await flush();
    sockets.at(-1)!.open();
    sockets.at(-1)!.frame(snapshot(id, 4));
    release();
  }
  const held = pool.held().map((feed) => feed.id);
  assert.deepEqual(held.sort(), ["b", "c", "first"].sort(), `held ${held}`);
});

test("refresh throws the window away and re-attaches without a resume marker", async (t) => {
  const { sockets, deps } = harness();
  const pool = poolFor(t, deps, { cached: 8, idleSockets: 3, idleMs: 60_000 });
  const a = pool.feed("claude", "a");
  pool.acquire(a, pin());
  await flush();
  sockets[0]!.open();
  sockets[0]!.frame(snapshot("a", 9));
  a.reattach();
  assert.equal(a.getView().data, null);
  assert.equal(sockets[0]!.closed, true);
  await flush();
  assert.equal(sockets[1]!.url, "/api/sessions/claude/a/ws");
});

test("a cold start draws the device copy marked as such, then resumes from it", async (t) => {
  const { sockets, deps } = harness();
  const saved: { key: string; data: unknown }[] = [];
  const copy = {
    ...snapshot("a", 30),
    type: undefined,
    canSend: true,
    omittedItems: 27,
  } as unknown as import("../src/features/sessions/session-feed-pool.js").FeedState;
  const pool = poolFor(
    t,
    {
      ...deps,
      persist: {
        load: async (key) => (key === "claude/a" ? copy : null),
        save: (key, data) => saved.push({ key, data }),
      },
    },
    { cached: 8, idleSockets: 3, idleMs: 60_000 },
  );
  const a = pool.feed("claude", "a");
  pool.acquire(a, pin());
  await flush();
  assert.equal(a.getView().data, copy, "drawn from the device before any frame");
  assert.equal(a.getView().connection, "cached");
  assert.equal(sockets[0]!.url, "/api/sessions/claude/a/ws?since=30&fp=a-fp", "the attach waited for the copy, and resumes from it");
  sockets[0]!.open();
  sockets[0]!.frame({ type: "delta", sig: "a:2", fromIndex: 27, items: copy.items, endIndex: 30, totalItems: 30, prefixFp: "a-fp" });
  assert.equal(a.getView().connection, "live", "a frame confirms it");
  await new Promise((resolve) => setTimeout(resolve, 2100));
  assert.equal(saved.at(-1)?.key, "claude/a", "and the confirmed window is saved back");
});

test("the device copy keeps the newest items and the resume marker, and forgets the start fingerprint", async () => {
  const { persistable } = await import("../src/features/sessions/session-feed-pool.js");
  const items = Array.from({ length: 100 }, (_, i) => ({ kind: "message", role: "user", text: String(i) }));
  const data = {
    ...snapshot("a", 300),
    items,
    startIndex: 200,
    endIndex: 300,
    omittedItems: 200,
    startFp: "start",
  } as unknown as import("../src/features/sessions/session-feed-pool.js").FeedState;
  const kept = persistable(data, 60);
  assert.equal(kept.items.length, 60);
  assert.equal(kept.items[0], items[40]);
  assert.equal(kept.startIndex, 240);
  assert.equal(kept.omittedItems, 240);
  assert.equal(kept.endIndex, 300, "the resume marker is untouched");
  assert.equal(kept.prefixFp, "a-fp");
  assert.equal(kept.startFp, null, "the new start is not one the server fingerprinted for us");
  assert.equal(kept.hasMoreBefore, true);
  assert.equal(persistable(kept, 60), kept, "already short enough is kept as is");
});

test("a device copy that never arrives does not keep the transcript from connecting", async (t) => {
  const { HYDRATE_WAIT_MS } = await import("../src/features/sessions/session-feed-pool.js");
  const { sockets, deps } = harness();
  const pool = poolFor(
    t,
    { ...deps, persist: { load: () => new Promise(() => undefined), save: () => undefined } },
    { cached: 8, idleSockets: 3, idleMs: 60_000 },
  );
  const a = pool.feed("claude", "a");
  pool.acquire(a, pin());
  await flush();
  assert.equal(sockets.length, 0, "a moment's wait for the copy");
  await new Promise((resolve) => setTimeout(resolve, HYDRATE_WAIT_MS + 50));
  assert.equal(sockets.length, 1, "and then on without it");
  assert.equal(sockets[0]!.url, "/api/sessions/claude/a/ws", "nothing to resume from");
});

test("a subscription the server refuses as unreadable is a verdict, not a retry loop", async (t) => {
  const { sockets, deps } = harness();
  const pool = poolFor(t, deps, { cached: 8, idleSockets: 3, idleMs: 60_000 });
  const a = pool.feed("claude", "a");
  pool.acquire(a, pin());
  await flush();
  sockets[0]!.open();
  sockets[0]!.frame({ type: "closed", reason: "invalid" });
  await new Promise((resolve) => setTimeout(resolve, 1200));
  assert.equal(a.getView().connection, "gone", "said at once, so the view shows it");
  assert.equal(sockets.length, 1, "and not asked again");
  assert.equal(a.running, false);
});

test("a subscription refused for want of room goes on over a socket of the feed's own", async (t) => {
  const { sockets, deps } = harness();
  const channels: FakeSocket[] = [];
  const pool = poolFor(
    t,
    {
      ...deps,
      openFeed: () => {
        const channel = new FakeSocket("channel");
        channels.push(channel);
        return channel;
      },
    },
    { cached: 8, idleSockets: 3, idleMs: 60_000 },
  );
  const a = pool.feed("claude", "a");
  pool.acquire(a, pin());
  await flush();
  assert.equal(channels.length, 1);
  assert.equal(sockets.length, 0, "the shared socket first");
  channels[0]!.open();
  channels[0]!.frame({ type: "closed", reason: "limit" });
  await new Promise((resolve) => setTimeout(resolve, 1200));
  assert.equal(channels.length, 1, "not the shared socket again");
  assert.equal(sockets.length, 1, "a socket of its own");
  assert.equal(sockets[0]!.url, "/api/sessions/claude/a/ws");
});

test("only a window somebody is reading is saved, and it is saved as they leave it", async (t) => {
  const { sockets, deps } = harness();
  const saved: string[] = [];
  const pool = poolFor(
    t,
    { ...deps, persist: { load: async () => null, save: (key) => saved.push(key) } },
    { cached: 8, idleSockets: 3, idleMs: 60_000 },
  );
  const a = pool.feed("claude", "a");
  const release = pool.acquire(a, pin());
  await flush();
  sockets[0]!.open();
  sockets[0]!.frame(snapshot("a", 3));
  release();
  assert.deepEqual(saved, ["claude/a"], "saved the moment the reader left, not two seconds later");

  // Hidden now, and still moving on its pooled socket: not saved.
  sockets[0]!.frame({ type: "delta", sig: "s2", fromIndex: 3, items: [{ kind: "message", role: "user", text: "x" }], endIndex: 4, totalItems: 4, prefixFp: "p2" });
  await new Promise((resolve) => setTimeout(resolve, 2100));
  assert.deepEqual(saved, ["claude/a"], "a background window does not push out one that was read");
});

test("disposing the pool (its machine closed) stops every feed: sockets closed, no reconnect, nothing held", async (t) => {
  const { sockets, deps } = harness();
  const pool = poolFor(t, deps, { cached: 8, idleSockets: 2, idleMs: 60_000 });
  const shown = pool.feed("claude", "a");
  pool.acquire(shown, pin());
  const idle = pool.feed("claude", "b");
  const release = pool.acquire(idle, pin());
  await flush();
  for (const socket of sockets) socket.open();
  release();
  assert.equal(sockets.filter((s) => !s.closed).length, 2, "one on screen, one kept idle");
  pool.dispose();
  assert.ok(sockets.every((s) => s.closed), "every socket closed, the idle one's timer with it");
  assert.equal(pool.held().length, 0);
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(sockets.length, 2, "nothing reconnects after");
});

// ---------------------------------------------------------------------------
// What an opening feed is waiting for (`FeedView.opening`), which the view says
// under its spinner: the socket connecting, the server's answer, the offline
// copy's download from the hub, or another try after `closed`.

/** A feed on screen, and every view its store has handed out since. */
async function opened(t: TestContext, extra: Partial<FeedPoolDeps> = {}) {
  const { sockets, deps } = harness();
  const pool = poolFor(t, { ...deps, ...extra }, { cached: 8, idleSockets: 3, idleMs: 60_000 });
  const feed = pool.feed("claude", "a");
  const views: ReturnType<typeof feed.getView>[] = [];
  feed.subscribe(() => views.push(feed.getView()));
  pool.acquire(feed, pin());
  await flush();
  return { sockets, feed, views, opening: () => feed.getView().opening };
}

test("an opening feed says it is connecting, then waits for the server's answer, and its window ends the wait", async (t) => {
  const { sockets, opening } = await opened(t);
  assert.deepEqual(opening(), { stage: "connecting" }, "the socket is not open yet");
  sockets[0]!.open();
  assert.deepEqual(opening(), { stage: "loading" }, "asked, and nothing has answered");
  sockets[0]!.frame(snapshot("a", 3));
  assert.equal(opening(), null, "a window is nothing to wait for");
});

test("a feed drawn from the device copy says no opening, and its resuming delta keeps it so", async (t) => {
  const { sockets, deps } = harness();
  const copy = { ...snapshot("a", 3), type: undefined } as unknown as import("../src/features/sessions/session-feed-pool.js").FeedState;
  const pool = poolFor(t, { ...deps, persist: { load: async () => copy, save: () => undefined } }, { cached: 8, idleSockets: 3, idleMs: 60_000 });
  const feed = pool.feed("claude", "a");
  pool.acquire(feed, pin());
  await flush();
  assert.equal(feed.getView().opening, null, "the copy is drawn, so nothing is said over it");
  sockets[0]!.open();
  assert.equal(feed.getView().opening, null);
  sockets[0]!.frame({ type: "delta", sig: "a:2", fromIndex: 3, items: [], endIndex: 3, totalItems: 3, prefixFp: "a-fp" });
  assert.equal(feed.getView().opening, null);
});

test("a shared-socket channel already open when it is handed over waits for the answer at once", async (t) => {
  const channel = new FakeSocket("channel");
  channel.readyState = 1;
  const { opening } = await opened(t, { openFeed: () => channel });
  assert.deepEqual(opening(), { stage: "loading" });
});

test("the offline copy's progress frames say its download, with the pages or without", async (t) => {
  const { sockets, opening } = await opened(t);
  sockets[0]!.open();
  sockets[0]!.frame({ type: "progress", stage: "copy" });
  assert.deepEqual(opening(), { stage: "copy" }, "fetching, with no download running yet");
  sockets[0]!.frame({ type: "progress", stage: "copy", done: 12, total: 256 });
  assert.deepEqual(opening(), { stage: "copy", done: 12, total: 256 });
  sockets[0]!.frame({ type: "progress", stage: "copy", done: 300, total: 256 });
  assert.deepEqual(opening(), { stage: "copy", done: 256, total: 256 }, "never past the total");
  sockets[0]!.frame({ type: "progress", stage: "copy", done: 3 });
  assert.deepEqual(opening(), { stage: "copy" }, "a count with no total is no bar");
  sockets[0]!.frame({ type: "progress", stage: "copy", done: 1.5, total: "9" });
  assert.deepEqual(opening(), { stage: "copy" }, "numbers that are not whole counts are dropped");
  sockets[0]!.frame(snapshot("a", 3));
  assert.equal(opening(), null);
});

test("a server's read progress is the loading stage, in bytes; a stage this page does not know is ignored", async (t) => {
  const { sockets, opening, views } = await opened(t);
  sockets[0]!.open();
  sockets[0]!.frame({ type: "progress", stage: "read", done: 40_000_000, total: 127_000_000 });
  assert.deepEqual(opening(), { stage: "loading", done: 40_000_000, total: 127_000_000 });
  const seen = views.length;
  sockets[0]!.frame({ type: "progress", stage: "index", done: 1, total: 2 });
  sockets[0]!.frame({ type: "progress" });
  assert.equal(views.length, seen, "nothing changed, so no view was woken");
  assert.deepEqual(opening(), { stage: "loading", done: 40_000_000, total: 127_000_000 });
});

test("an opening that changes wakes the view, and one said again with the same numbers does not", async (t) => {
  const { sockets, views } = await opened(t);
  sockets[0]!.open();
  const before = views.length;
  for (const done of [1, 1, 2, 2]) sockets[0]!.frame({ type: "progress", stage: "copy", done, total: 10 });
  assert.deepEqual(
    views.slice(before).map((view) => view.opening),
    [
      { stage: "copy", done: 1, total: 10 },
      { stage: "copy", done: 2, total: 10 },
    ],
    "one view per change, none for a frame that repeats itself",
  );
});

test("a pooled feed with a window that reconnects says nothing of it", async (t) => {
  const { sockets, feed, views } = await opened(t);
  sockets[0]!.open();
  sockets[0]!.frame(snapshot("a", 3));
  const before = views.length;
  sockets[0]!.drop();
  await new Promise((resolve) => setTimeout(resolve, 600));
  assert.equal(sockets.length, 2, "reconnected");
  sockets[1]!.open();
  sockets[1]!.frame({ type: "progress", stage: "copy", done: 1, total: 2 });
  assert.equal(feed.getView().opening, null);
  assert.ok(views.slice(before).every((view) => view.opening === null), "no view saw an opening");
});

test("closed with nothing on screen is retrying, through every attempt, until a window arrives", async (t) => {
  const { sockets, opening } = await opened(t);
  sockets[0]!.open();
  sockets[0]!.frame({ type: "closed" });
  assert.deepEqual(opening(), { stage: "retrying" });
  await new Promise((resolve) => setTimeout(resolve, 1100));
  assert.equal(sockets.length, 2, "asked again after the backoff");
  assert.deepEqual(opening(), { stage: "retrying" }, "the same wait while it connects");
  sockets[1]!.open();
  assert.deepEqual(opening(), { stage: "retrying" }, "and while it waits for the answer");
  sockets[1]!.frame({ type: "progress", stage: "copy" });
  assert.deepEqual(opening(), { stage: "copy" }, "a download says more than the retry");
  sockets[1]!.frame(snapshot("a", 3));
  assert.equal(opening(), null);
});

test("a refusal for want of room is no retry: the feed connects on a socket of its own", async (t) => {
  const channel = new FakeSocket("channel");
  const { sockets, opening } = await opened(t, { openFeed: () => channel });
  channel.open();
  channel.frame({ type: "closed", reason: "limit" });
  assert.deepEqual(opening(), { stage: "connecting" });
  await new Promise((resolve) => setTimeout(resolve, 1100));
  assert.equal(sockets.length, 1);
  assert.deepEqual(opening(), { stage: "connecting" });
  sockets[0]!.open();
  assert.deepEqual(opening(), { stage: "loading" });
});

test("a session refused for good waits for nothing: the view says it is not found", async (t) => {
  const { sockets, feed, opening } = await opened(t);
  sockets[0]!.open();
  sockets[0]!.frame({ type: "closed", reason: "invalid" });
  assert.equal(feed.getView().connection, "gone");
  assert.equal(opening(), null);
});

test("closed over a window on screen keeps the window and says no opening", async (t) => {
  const { sockets, opening } = await opened(t);
  sockets[0]!.open();
  sockets[0]!.frame(snapshot("a", 3));
  sockets[0]!.frame({ type: "closed" });
  assert.equal(opening(), null);
});
