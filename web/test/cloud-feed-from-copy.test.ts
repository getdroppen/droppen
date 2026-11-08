// droppen.app on the machine itself (`live`) opening a session the page's feed
// pool does not hold: drawn at once from the machine's offline copy on this
// device (`features/sessions/feed-from-copy.ts`) under the "saved on this
// device" banner, and brought up to date by the live socket resuming from the
// copy's own marker. Against a real mirror reading sealed, machine-signed
// copies out of a store the test can see into; nothing new is written.
import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import { copyFeedState, copyFeedNow } from "../src/features/sessions/feed-from-copy.js";
import { devicePersistFor } from "../src/features/sessions/feed-persist.js";
import { createSessionFeedPool, HYDRATE_WAIT_MS, OPENING_ITEMS, type FeedPoolDeps, type FeedSocket } from "../src/features/sessions/session-feed-pool.js";
import { setHubShell } from "../src/features/machines/shells.js";
import type { HubShell } from "../src/features/cloud/hub-shell.js";
import { HubCopySource } from "../src/lib/hub/copy-source.js";
import { FMT_FEED_V1, FMT_INDEX_V1, hex, indexStream, streamId } from "../src/lib/e2e/index.js";
import { MachineMirror } from "../src/lib/hub/mirror.js";
import { feedFold, type FeedCopy, StreamClient } from "../src/lib/hub/streams.js";
import { LOCAL } from "../src/lib/machines/key.js";
import { FakeKv, FakeStreamLink, flush, makeMachine, putCopy, StreamWriter, until } from "./stream-fixtures.js";

type FeedState = import("../src/features/sessions/session-feed-pool.js").FeedState;

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
}

function harness(extra: Partial<FeedPoolDeps> = {}) {
  const sockets: FakeSocket[] = [];
  const history: unknown[] = [];
  const deps: FeedPoolDeps = {
    socketUrl: async (path) => path,
    openSocket: (url) => {
      const socket = new FakeSocket(url);
      sockets.push(socket);
      return socket;
    },
    noteFailure: () => undefined,
    fetchHistory: async (_source, _id, query) => {
      history.push(query);
      return { status: 500, error: "unused" };
    },
    fetchState: async () => null,
    ...extra,
  };
  return { sockets, history, deps };
}

function poolFor(t: TestContext, deps: FeedPoolDeps) {
  const pool = createSessionFeedPool(deps, { cached: 8, idleSockets: 3, idleMs: 60_000 });
  t.after(() => pool.dispose());
  return pool;
}

const pin = () => ({ current: true });
const item = (n: number, text = `item ${n}`) => ({ role: "user", kind: "text", text });
const items = (from: number, to: number) => Array.from({ length: to - from }, (_, i) => item(from + i));
const row = (id: string) => ({ source: "claude", id, title: `t ${id}`, updatedAt: "2026-11-01T00:00:00.000Z", cwd: "/w", scope: "owned", activity: "idle", status: "idle" });
const copyOf = (startIndex: number, endIndex: number, extra: Partial<FeedCopy> = {}): FeedCopy => ({
  started: true,
  startIndex,
  endIndex,
  items: items(startIndex, endIndex),
  fp: `fp${endIndex}`,
  prefixFp: `fp${endIndex - 3}`,
  ...extra,
});

/**
 * A machine whose index lists `sessions` and whose copies of them (`copies`)
 * are on the device only, sealed and under heads the machine signed — and its
 * mirror, read off the device and following nothing but its index, as
 * droppen.app keeps a machine out of view.
 */
async function deviceWith(t: TestContext, sessions: string[], copies: Record<string, FeedCopy>) {
  const machine = await makeMachine();
  const kv = new FakeKv();
  const link = new FakeStreamLink();
  const client = new StreamClient({ link, machineSignKey: machine.signPublic, epochs: machine.epochs, fetchPage: async () => new Uint8Array(), retryDelay: () => 0 });
  const index = await StreamWriter.open(machine, await indexStream(machine.mk), FMT_INDEX_V1);
  const rows = Object.fromEntries(sessions.map((id) => [`claude/${id}`, row(id)]));
  await index.append({ upserts: Object.values(rows), removed: [], asOf: 1 });
  await putCopy(kv, "a-1", "m-1", hex(index.stream), { kind: "index", head: hex(await index.head()), state: { rows, asOf: 1 }, freshestMs: 1, savedAt: 1 });
  const streams = new Map<string, string>();
  for (const [id, state] of Object.entries(copies)) {
    const writer = await StreamWriter.open(machine, await streamId(machine.mk, "claude", id), FMT_FEED_V1);
    await writer.append({ fromIndex: state.startIndex, items: state.items, endIndex: state.endIndex });
    await putCopy(kv, "a-1", "m-1", hex(writer.stream), { kind: "session", source: "claude", id, head: hex(await writer.head()), state, freshestMs: 1, savedAt: 1 });
    streams.set(id, hex(writer.stream));
  }
  const mirror = new MachineMirror({ account: "a-1", machine: "m-1", source: HubCopySource.over({ mk: machine.mk, client }), transact: kv.transact, followSessions: false, sweep: false });
  await mirror.start();
  t.after(() => mirror.close());
  /** What the device holds: every key written, and how many transactions wrote. */
  const writes = () => kv.txs.filter((tx) => tx.mode === "readwrite").length;
  return { mirror, link, kv, streams, writes };
}

/** droppen.app holding `mirror` as machine `m-1`'s copy, on the machine (`live`) or its copy. */
function shellWith(t: TestContext, mirror: MachineMirror, mode: { current: "live" | "offline" } = { current: "live" }) {
  setHubShell({ kind: "hub", machine: (key: string | null | undefined) => (key === "m-1" ? { mirror, transport: mode } : null) } as unknown as HubShell);
  t.after(() => setHubShell(null));
  return mode;
}

// ---------------------------------------------------------------------------
// The window a copy gives

test("a copy's window is its newest page, with the copy's own resume marker and nothing the copy cannot vouch for", () => {
  const copy = copyOf(0, 250);
  const data = copyFeedState({ state: copy, seq: 7 }, row("a") as never)!;
  assert.equal(data.items.length, OPENING_ITEMS, "the size of a live opening snapshot");
  assert.equal(data.items[0], copy.items[150], "the copy's own items, not copies of them");
  assert.deepEqual([data.startIndex, data.endIndex, data.totalItems, data.omittedItems], [150, 250, 250, 150]);
  assert.equal(data.prefixFp, "fp247", "the marker the agent signed with this end");
  assert.equal(data.startFp, null, "nobody fingerprinted this start for us");
  assert.equal(data.hasMoreBefore, true);
  assert.equal(data.sig, "offline:7", "the copy's version, as its own snapshot names it");
  assert.equal(data.canSend, true, "at rest, as the machine last listed it");
  assert.equal(data.sendBlockReason, null);
  assert.equal(data.summary.id, "a");
  assert.deepEqual([data.questions, data.receipts, data.injection, data.account], [[], [], null, null]);
  const busy = copyFeedState({ state: copy, seq: 7 }, { ...row("a"), status: "running", activity: "working" } as never)!;
  assert.equal(busy.canSend, false, "a session the row shows busy queues what is typed until the live state says otherwise");
  const external = copyFeedState({ state: copy, seq: 7 }, { ...row("a"), scope: "external" } as never)!;
  assert.equal(external.canSend, false, "another instance's session is not this one's to send to");

  const short = copyFeedState({ state: copyOf(0, 4), seq: 1 }, row("a") as never)!;
  assert.deepEqual([short.startIndex, short.items.length, short.hasMoreBefore], [0, 4, false], "a whole short session has nothing before it");
});

test("a copy the retention window trimmed starts where the copy does, and the machine is asked for what is before it", () => {
  // As the fold leaves a trimmed copy: the window-start page at 40, then the items kept.
  let copy = feedFold.empty();
  copy = feedFold.apply(copy, { fromIndex: 40, items: [], endIndex: 40, fp: "fp40", prefixFp: "fp37", base: 40, before: "2026-10-01T08:00:00.000Z" });
  copy = feedFold.apply(copy, { fromIndex: 40, items: items(40, 52), endIndex: 52, fp: "fp52", prefixFp: "fp49" });
  const data = copyFeedState({ state: copy, seq: 2 }, row("long") as never)!;
  assert.deepEqual([data.startIndex, data.endIndex, data.items.length], [40, 52, 12]);
  assert.equal(data.hasMoreBefore, true, "the earlier entries are on the machine, which this page is on");
  assert.equal(data.prefixFp, "fp49");
});

test("nothing to draw is no window: an empty copy, one whose numbers do not add up", () => {
  assert.equal(copyFeedState({ state: feedFold.empty(), seq: 0 }, row("a") as never), null);
  assert.equal(copyFeedState({ state: copyOf(40, 40, { items: [] }), seq: 1 }, row("a") as never), null, "a window-start page and nothing after it");
  assert.equal(copyFeedState({ state: { ...copyOf(0, 10), endIndex: 12 }, seq: 1 }, row("a") as never), null);
});

test("an item the agent cut down to fit the copy is not resumed over: the live socket sends the window whole", () => {
  const copy = copyOf(0, 10);
  copy.items[8] = { ...copy.items[8]!, text: "[too large for the offline copy]", truncated: true, rhOfflineTruncated: true } as never;
  const data = copyFeedState({ state: copy, seq: 1 }, row("a") as never)!;
  assert.equal(data.items.length, 10, "drawn all the same");
  assert.equal(data.prefixFp, null);
});

// ---------------------------------------------------------------------------
// The feed

test("live, with the copy in memory: the first frame is the transcript under the banner, with no opening said", async (t) => {
  const { mirror, writes } = await deviceWith(t, ["a"], { a: copyOf(0, 10) });
  shellWith(t, mirror);
  await mirror.sessionHeld("claude", "a");
  const { sockets, deps } = harness({ persist: devicePersistFor("m-1") });
  const pool = poolFor(t, deps);
  const written = writes();

  const feed = pool.feed("claude", "a");
  const view = feed.getView();
  assert.ok(view.data, "drawn when the feed is made, in the render that asks for it");
  assert.deepEqual(view.data.items.map((i) => (i as { text: string }).text), items(0, 10).map((i) => i.text));
  assert.equal(view.connection, "cached", "the banner: the copy saved on this device, updating");
  assert.equal(view.opening, null, "no loading line over a drawn transcript");

  pool.acquire(feed, pin());
  await flush();
  assert.equal(sockets[0]!.url, "/api/sessions/claude/a/ws?since=10&fp=fp7", "the live socket resumes from the copy's marker");
  sockets[0]!.open();
  assert.equal(feed.getView().opening, null);
  assert.equal(feed.getView().connection, "cached", "until a frame confirms it");
  await new Promise((resolve) => setTimeout(resolve, 2100));
  assert.equal(writes(), written, "nothing is written: not the copy, not a second one");
});

test("live, with the copy only on the device: read back and drawn within the hydrate wait, and resumed from", async (t) => {
  const { mirror, link, streams } = await deviceWith(t, ["a"], { a: copyOf(0, 30) });
  shellWith(t, mirror);
  assert.equal(mirror.sessionNow("claude", "a"), null, "not in memory");
  const { sockets, deps } = harness({ persist: devicePersistFor("m-1") });
  const pool = poolFor(t, deps);
  const feed = pool.feed("claude", "a");
  assert.equal(feed.getView().data, null, "nothing to draw on the first frame");
  const started = Date.now();
  pool.acquire(feed, pin());
  await until(() => feed.getView().data !== null, HYDRATE_WAIT_MS);
  assert.ok(Date.now() - started < HYDRATE_WAIT_MS, "inside the wait");
  assert.equal(feed.getView().connection, "cached");
  assert.equal(feed.getView().data!.startIndex, 0, "the copy's 30 items are within a page");
  await until(() => sockets.length === 1);
  assert.equal(sockets[0]!.url, "/api/sessions/claude/a/ws?since=30&fp=fp27", "the attach waited for the copy, and resumes from it");
  assert.equal(link.subs().some((s) => hex(s.stream) === streams.get("a")), false, "read, not followed from the hub for this");
});

test("live, with no copy of the session here: nothing drawn, and the view says what it waits for", async (t) => {
  const { mirror } = await deviceWith(t, ["a", "b"], { a: copyOf(0, 10) });
  shellWith(t, mirror);
  const { sockets, deps } = harness({ persist: devicePersistFor("m-1") });
  const pool = poolFor(t, deps);
  const feed = pool.feed("claude", "b");
  pool.acquire(feed, pin());
  await until(() => sockets.length === 1, HYDRATE_WAIT_MS);
  assert.equal(sockets[0]!.url, "/api/sessions/claude/b/ws", "nothing to resume from");
  assert.equal(feed.getView().data, null);
  assert.deepEqual(feed.getView().opening, { stage: "connecting" });
  sockets[0]!.open();
  assert.deepEqual(feed.getView().opening, { stage: "loading" }, "the loading line, as before");
});

test("the live socket's resume delta merges onto the copy, and its state replaces the copy's", async (t) => {
  const { mirror } = await deviceWith(t, ["a"], { a: copyOf(0, 10) });
  shellWith(t, mirror);
  await mirror.sessionHeld("claude", "a");
  const { sockets, deps } = harness({ persist: devicePersistFor("m-1") });
  const pool = poolFor(t, deps);
  const feed = pool.feed("claude", "a");
  const drawn = feed.getView().data!;
  pool.acquire(feed, pin());
  await flush();
  sockets[0]!.open();
  // A delta the copy's window cannot place is answered by asking for one that fits, from the copy's own marker.
  sockets[0]!.frame({ type: "delta", sig: "live:3", fromIndex: 20, items: [item(20)], endIndex: 21, totalItems: 21, prefixFp: "fp18" });
  await flush();
  assert.deepEqual(JSON.parse(sockets[0]!.sent.at(-1)!), { type: "resync", since: 10, fp: "fp7" });
  assert.equal(feed.getView().data, drawn, "and nothing is placed meanwhile");
  // What the server sends a client that holds `since=10&fp=fp7`: from 10 - 3, the overlap re-sent, and what is new.
  sockets[0]!.frame({ type: "delta", sig: "live:4", fromIndex: 7, items: [item(7), item(8), item(9, "item 9 (done)"), item(10)], endIndex: 11, totalItems: 11, prefixFp: "fp8" });
  const data = feed.getView().data!;
  assert.equal(feed.getView().connection, "live");
  assert.deepEqual(data.items.map((i) => (i as { text: string }).text), [...items(0, 9).map((i) => i.text), "item 9 (done)", "item 10"]);
  assert.equal(data.items[0], drawn.items[0], "what the copy held before the overlap stays, by reference");
  assert.deepEqual([data.startIndex, data.endIndex, data.prefixFp, data.sig], [0, 11, "fp8", "live:4"]);
  sockets[0]!.frame({ type: "state", summary: { ...row("a"), status: "running", title: "renamed" }, canSend: false, sendBlockReason: "a message from Droppen is still running", contextTokens: 1234 });
  assert.deepEqual([data.canSend, data.sendBlockReason], [true, null], "what the copy's row said");
  const live = feed.getView().data!;
  assert.deepEqual([live.canSend, live.sendBlockReason, live.contextTokens], [false, "a message from Droppen is still running", 1234], "the live state replaces it");
  assert.equal(live.summary.title, "renamed");
  assert.equal(live.items, data.items, "and leaves the window as it is");
});

test("a session no longer what the copy holds (its fingerprint differs) is answered with a snapshot that replaces the copy", async (t) => {
  const { mirror } = await deviceWith(t, ["a"], { a: copyOf(0, 10) });
  shellWith(t, mirror);
  await mirror.sessionHeld("claude", "a");
  const { sockets, deps } = harness({ persist: devicePersistFor("m-1") });
  const pool = poolFor(t, deps);
  const feed = pool.feed("claude", "a");
  pool.acquire(feed, pin());
  await flush();
  sockets[0]!.open();
  const fresh = [item(0, "rewritten 0"), item(1, "rewritten 1")];
  sockets[0]!.frame({ type: "snapshot", sig: "live:9", summary: row("a"), canSend: false, sendBlockReason: "a message from Droppen is still running", items: fresh, startIndex: 0, endIndex: 2, totalItems: 2, hasMoreBefore: false, prefixFp: "other", startFp: "s0" });
  const data = feed.getView().data!;
  assert.equal(feed.getView().connection, "live");
  assert.deepEqual(data.items, fresh, "nothing of the copy is left");
  assert.deepEqual([data.startIndex, data.endIndex, data.totalItems, data.hasMoreBefore, data.prefixFp, data.startFp, data.sig], [0, 2, 2, false, "other", "s0", "live:9"]);
  assert.deepEqual([data.canSend, data.sendBlockReason], [false, "a message from Droppen is still running"], "its state too");
});

test("a copy the retention window trimmed keeps \"load earlier\": the machine is asked for the page before it, proven by the copy's marker", async (t) => {
  let trimmed = feedFold.empty();
  trimmed = feedFold.apply(trimmed, { fromIndex: 40, items: [], endIndex: 40, fp: "fp40", prefixFp: "fp37", base: 40, before: "2026-10-01T08:00:00.000Z" });
  trimmed = feedFold.apply(trimmed, { fromIndex: 40, items: items(40, 45), endIndex: 45, fp: "fp45", prefixFp: "fp42" });
  const { mirror } = await deviceWith(t, ["long"], { long: trimmed });
  shellWith(t, mirror);
  await mirror.sessionHeld("claude", "long");
  const page = { sig: "live:1", items: items(0, 40), startIndex: 0, endIndex: 40, totalItems: 45, hasMoreBefore: false, startFp: "s0" };
  const { sockets, history, deps } = harness({ persist: devicePersistFor("m-1") });
  deps.fetchHistory = async (_source, _id, query) => {
    history.push(query);
    return { status: 200, page };
  };
  const pool = poolFor(t, deps);
  const feed = pool.feed("claude", "long");
  const data = feed.getView().data!;
  assert.deepEqual([data.startIndex, data.hasMoreBefore], [40, true]);
  pool.acquire(feed, pin());
  await flush();
  sockets[0]!.open();
  sockets[0]!.frame({ type: "delta", sig: "live:1", fromIndex: 42, items: items(42, 45), endIndex: 45, totalItems: 45, prefixFp: "fp42" });
  await feed.loadEarlier();
  assert.deepEqual(history, [{ before: 40, fp: "fp42", since: 45 }]);
  const after = feed.getView().data!;
  assert.deepEqual([after.startIndex, after.items.length, after.hasMoreBefore, feed.getView().historyError], [0, 45, false, null]);
});

test("on the machine's copy, or without droppen.app, or for the local app's own server: nothing is drawn from the copy", async (t) => {
  const { mirror } = await deviceWith(t, ["a"], { a: copyOf(0, 10) });
  await mirror.sessionHeld("claude", "a");
  const persist = devicePersistFor("m-1");
  // No shell: the local app, or droppen.app before it is set up.
  assert.equal(persist.peek!("claude/a"), null);
  assert.equal(await persist.load("claude/a"), null);
  // On the copy: the feed socket answers from the copy itself (`lib/transport/offline.ts`).
  const mode = shellWith(t, mirror, { current: "offline" });
  assert.equal(persist.peek!("claude/a"), null);
  assert.equal(await persist.load("claude/a"), null);
  mode.current = "live";
  assert.ok(persist.peek!("claude/a"));
  assert.equal(copyFeedNow("m-2", "claude/a"), null, "only the machine the copy belongs to");
  // The local app's own server keeps its windows as it always did, and nobody's copy stands in for them.
  assert.equal(devicePersistFor(LOCAL).peek!("claude/a"), null);
});

// ---------------------------------------------------------------------------
// A switch between the machine and its copy

test("a feed whose socket a switch between the machine and its copy closed goes back at once, not after a backoff", async (t) => {
  let switched: (() => void) | null = null;
  const { sockets, deps } = harness({
    onSwitch: (fn) => {
      switched = fn;
      return () => {
        switched = null;
      };
    },
  });
  const pool = poolFor(t, deps);
  const a = pool.feed("claude", "a");
  pool.acquire(a, pin());
  await flush();
  sockets[0]!.open();
  sockets[0]!.frame({ type: "snapshot", sig: "s", summary: row("a"), items: items(0, 3), startIndex: 0, endIndex: 3, totalItems: 3, prefixFp: "fp0", startFp: null });

  // An ordinary drop backs off.
  sockets[0]!.close();
  await flush(50);
  assert.equal(sockets.length, 1, "half a second's wait for a socket that merely dropped");
  await until(() => sockets.length === 2, 1000);
  sockets[1]!.open();

  // The switch closes the socket, then says so: no wait at all.
  sockets[1]!.close();
  switched!();
  await flush();
  await flush();
  assert.equal(sockets.length, 3, "back at once, through whichever side the page is on now");
  assert.equal(sockets[2]!.url, "/api/sessions/claude/a/ws?since=3&fp=fp0", "resuming its window");

  pool.dispose();
  assert.equal(switched, null, "a pool let go of stops listening");
});
