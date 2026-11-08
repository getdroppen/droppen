// The transport the page uses while its machine is away
// (`lib/transport/offline.ts`): the routes it answers from the
// offline copy, in the machine's own shapes; the 503 everything else gets;
// and the feed socket it plays — checked frame by frame, and through the real
// `FeedHub` so the app's own reader is what accepts them.
import assert from "node:assert/strict";
import test from "node:test";
import type { components } from "../src/api/schema.js";
import { ApiError, errorText } from "../src/api/errors.js";
import { FeedHub } from "../src/app/feed-hub.js";
import {
  OfflineMuxSocket,
  type OfflineOptions,
  type OfflineSource,
  offlineTransport,
  PROGRESS_EVERY_MS,
  RefusedSocket,
} from "../src/lib/transport/offline.js";
import type { MirrorChange, SessionCopy } from "../src/lib/hub/mirror.js";
import type { FeedCopy, StatusState, SubscriptionProgress } from "../src/lib/hub/streams.js";
import { snapshotOf } from "../src/features/sessions/session-feed-pool.js";
import { flush, until } from "./stream-fixtures.js";

type SessionSummary = components["schemas"]["SessionSummary"];

const summary = (id: string, updatedAt: string, extra: Partial<SessionSummary> = {}) =>
  ({ source: "claude", id, title: `title ${id}`, cwd: `/work/${id}`, updatedAt, activity: "working", status: "running", scope: "owned", resident: { phase: "working" }, ...extra }) as unknown as SessionSummary;
const items = (n: number) => Array.from({ length: n }, (_, i) => ({ kind: "user", text: `item ${i}` }));
const copyOf = (n: number, seq: number): SessionCopy => ({
  source: "claude",
  id: "a",
  seq,
  freshestMs: 1,
  state: { started: true, startIndex: 0, endIndex: n, items: items(n), fp: `fp${n}`, prefixFp: `pre${n - 3}` } as FeedCopy,
});

class FakeSource implements OfflineSource {
  list: SessionSummary[] = [summary("a", "2026-10-15T00:00:00.000Z"), summary("b", "2026-10-14T00:00:00.000Z", { source: "codex", title: "Fix the build" }), summary("x", "2026-10-13T00:00:00.000Z", { scope: "external" })];
  statusState: StatusState | null = {
    asOf: 1,
    settings: { hostname: "mac", serverVersion: "0.2.99", serverStartedAt: "2026-10-15T00:00:00.000Z" },
    accounts: {
      accounts: [{ id: "acc-1", label: "work", kind: "oauth_token", provider: "claude", email: "w@x", rotation: true }],
      defaultAccountId: "acc-1",
      local: { claude: { state: "signed_out", isDefault: true }, codex: { state: "unknown", isDefault: null } },
    },
    quota: { claude: null },
    cli: { claude: "2.1", codex: null },
    mkEpoch: 0,
  };
  copies = new Map<string, SessionCopy>([["claude/a", copyOf(250, 7)]]);
  /** Sessions still on their way from the hub: `session()` answers when the test says. */
  pending = new Map<string, Promise<SessionCopy | null>>();
  /** Each followed session's progress, as the test sets it (`progress`). */
  progressOf = new Map<string, SubscriptionProgress>();
  progressListeners = new Map<string, Set<() => void>>();
  touched: string[] = [];
  listeners = new Set<(c: MirrorChange) => void>();
  whenLoaded = async () => undefined;
  rows() {
    return this.list;
  }
  row(source: string, id: string) {
    return this.list.find((r) => r.source === source && r.id === id);
  }
  get status() {
    return { state: this.statusState };
  }
  async session(source: string, id: string) {
    return (await this.pending.get(`${source}/${id}`)) ?? this.copies.get(`${source}/${id}`) ?? null;
  }
  sessionNow(source: string, id: string) {
    return this.copies.get(`${source}/${id}`) ?? null;
  }
  sessionProgress(source: string, id: string) {
    return this.progressOf.get(`${source}/${id}`) ?? null;
  }
  onSessionProgress(source: string, id: string, fn: () => void) {
    const key = `${source}/${id}`;
    if (!this.progressListeners.has(key)) this.progressListeners.set(key, new Set());
    this.progressListeners.get(key)!.add(fn);
    return () => void this.progressListeners.get(key)!.delete(fn);
  }
  /** A session's follow moved (as the mirror says it, at most once a frame). */
  progress(key: string, progress: SubscriptionProgress) {
    this.progressOf.set(key, progress);
    for (const fn of [...(this.progressListeners.get(key) ?? [])]) fn();
  }
  touch(source: string, id: string) {
    this.touched.push(`${source}/${id}`);
  }
  onChange(fn: (c: MirrorChange) => void) {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }
  change(c: MirrorChange) {
    for (const fn of this.listeners) fn(c);
  }
}

const opts: OfflineOptions = {
  user: "cloud:d-1",
  messages: { readOnly: "offline: read-only copy", noCopy: "offline: no copy", unavailable: "the machine is offline" },
  machineOnline: () => false,
};

const get = async (source: OfflineSource | null, path: string, init?: RequestInit) => {
  const res = await offlineTransport(source, opts).fetch(new Request(`http://page${path}`, init));
  return { status: res.status, body: (await res.json()) as Record<string, any> };
};

test("the list: newest first, filtered like the machine filters it, and nothing shown as running", async () => {
  const source = new FakeSource();
  const all = await get(source, "/api/sessions");
  assert.equal(all.status, 200);
  assert.deepEqual(all.body.sessions.map((s: SessionSummary) => s.id), ["a", "b"], "owned by default");
  assert.equal(all.body.total, 2);
  assert.equal(all.body.sessions[0].activity, "idle", "a machine that is off runs nothing");
  assert.equal(all.body.sessions[0].status, "idle");
  assert.equal(all.body.sessions[0].resident, null);
  assert.deepEqual((await get(source, "/api/sessions?scope=all")).body.sessions.map((s: SessionSummary) => s.id), ["a", "b", "x"]);
  assert.deepEqual((await get(source, "/api/sessions?scope=external")).body.sessions.map((s: SessionSummary) => s.id), ["x"]);
  assert.deepEqual((await get(source, "/api/sessions?source=codex")).body.sessions.map((s: SessionSummary) => s.id), ["b"]);
  assert.deepEqual((await get(source, "/api/sessions?q=fix%20THE")).body.sessions.map((s: SessionSummary) => s.id), ["b"], "title, case-insensitive");
  assert.deepEqual((await get(source, "/api/sessions?q=work/a")).body.sessions.map((s: SessionSummary) => s.id), ["a"], "and cwd");
  const one = await get(source, "/api/sessions?limit=1&scope=all");
  assert.equal(one.body.sessions.length, 1);
  assert.equal(one.body.total, 3);
  assert.equal((await get(source, "/api/sessions?limit=0")).status, 400);
  // While the machine is up, its rows are its own word.
  const live = await offlineTransport(source, { ...opts, machineOnline: () => true }).fetch(new Request("http://page/api/sessions"));
  assert.equal(((await live.json()) as { sessions: SessionSummary[] }).sessions[0]!.activity, "working");
});

test("a session's detail is the copy's last page, read-only, and its history pages hang off the copy's own window", async () => {
  const source = new FakeSource();
  const detail = await get(source, "/api/sessions/claude/a");
  assert.equal(detail.status, 200);
  assert.equal(detail.body.canSend, false);
  assert.equal(detail.body.sendBlockReason, "offline: read-only copy");
  assert.equal(detail.body.sig, "offline:7");
  assert.deepEqual([detail.body.startIndex, detail.body.endIndex, detail.body.totalItems, detail.body.items.length], [50, 250, 250, 200]);
  assert.equal(detail.body.hasMoreBefore, true);
  assert.equal(detail.body.summary.id, "a");
  assert.deepEqual(source.touched, [], "a REST read is not an open");
  const same = await get(source, "/api/sessions/claude/a?after=250&sig=offline%3A7");
  assert.deepEqual([same.body.items.length, same.body.itemsChanged, same.body.canSend], [0, false, false]);
  assert.equal((await get(source, "/api/sessions/claude/nope")).status, 404);
  // A session in the list with no copy on the device: says so.
  const bare = await get(source, "/api/sessions/codex/b");
  assert.equal(bare.body.sendBlockReason, "offline: no copy");
  assert.equal(bare.body.items.length, 0);

  // History: the window cut from this copy proves itself with its resume marker.
  const page = await get(source, "/api/sessions/claude/a/history?before=50&fp=pre247&since=250");
  assert.equal(page.status, 200);
  assert.deepEqual([page.body.startIndex, page.body.endIndex, page.body.items.length, page.body.hasMoreBefore, page.body.sig], [0, 50, 50, false, "offline:7"]);
  assert.equal(page.body.items[0].text, "item 0");
  assert.equal((await get(source, "/api/sessions/claude/a/history?before=50&fp=other&since=250")).status, 409, "another window: transcript_changed");
  assert.equal((await get(source, "/api/sessions/claude/a/history?before=50&fp=fp250")).status, 409, "a start fingerprint the copy cannot check");
  assert.equal((await get(source, "/api/sessions/claude/a/history?before=250&fp=fp250")).status, 200, "except at its very end");
  assert.equal((await get(source, "/api/sessions/claude/a/history?before=120")).body.startIndex, 0, "unchecked without fp, like the machine");
});

test("settings, accounts and quota come from the status; everything else is a 503 the app can read", async () => {
  const source = new FakeSource();
  assert.equal((await get(source, "/api/settings")).body.hostname, "mac");
  assert.deepEqual((await get(source, "/api/stats/quota")).body, { claude: null });
  const accounts = await get(source, "/api/accounts");
  assert.equal(accounts.body.defaultAccountId, "acc-1");
  assert.equal(accounts.body.defaultCodexAccountId, null);
  assert.deepEqual(accounts.body.accounts[0].rest, [], "every field an account is read with is there");
  assert.equal(accounts.body.accounts[0].label, "work");
  assert.equal(accounts.body.accounts[0].rotation, true, "which accounts may stand in, as the status carries it");
  assert.deepEqual(
    accounts.body.local,
    { claude: { state: "signed_out", rest: [], isDefault: true }, codex: { state: "unknown", rest: [] } },
    "the local logins' state and choice, as far as the status carries them (an older machine's null choice is no choice)",
  );
  assert.deepEqual(await get(source, "/api/auth/status"), {
    status: 200,
    body: { authRequired: false, setupRequired: false, user: "cloud:d-1", desktopApp: false, authOff: false },
  });

  source.statusState = null;
  assert.equal((await get(source, "/api/settings")).status, 503);
  for (const [path, method] of [
    ["/api/sessions/claude/a/message", "POST"],
    ["/api/terminals", "POST"],
    ["/api/terminals", "GET"],
    ["/api/fs/list", "GET"],
  ] as const) {
    const res = await get(source, path, { method, ...(method === "POST" ? { body: "{}" } : {}) });
    assert.equal(res.status, 503, `${method} ${path}`);
    assert.equal(res.body.error, "machine_offline");
    assert.equal(res.body.message, "the machine is offline");
    assert.equal(errorText(new ApiError(res.body)), "the machine is offline", "what the app shows for it");
  }
  assert.equal((await get(null, "/api/sessions")).status, 503, "no copy at all");
});

/** A mux socket and every frame it said. */
function mux(source: FakeSource) {
  const socket = offlineTransport(source, opts).openSocket("/api/events?v=2&z=1") as OfflineMuxSocket;
  const frames: Record<string, any>[] = [];
  let opened = false;
  let closed = false;
  socket.onopen = () => (opened = true);
  socket.onmessage = (e) => frames.push(JSON.parse(e.data as string));
  socket.onclose = () => (closed = true);
  return { socket, frames, isOpen: () => opened, isClosed: () => closed, of: (sub: number) => frames.filter((f) => f.sub === sub) };
}

test("the feed socket: hello first, then settings and the list; a sub is a read-only snapshot, a resume, a reset", async () => {
  const source = new FakeSource();
  const m = mux(source);
  assert.ok(m.socket instanceof OfflineMuxSocket);
  await until(() => m.frames.length >= 3);
  assert.ok(m.isOpen());
  assert.deepEqual(m.frames[0], { type: "hello", features: ["sub", "ping"], compress: null });
  assert.equal(m.frames[1]!.type, "settings");
  assert.equal(m.frames[1]!.hostname, "mac");
  assert.equal(m.frames[1]!.fromCopy, true, "marked, so its version is not taken for the machine's build");
  assert.equal(m.frames[2]!.type, "sessions:snapshot");
  assert.deepEqual(m.frames[2]!.sessions.map((s: SessionSummary) => s.id), ["a", "b", "x"]);

  m.socket.send(JSON.stringify({ type: "sub", feeds: [{ sub: 1, source: "claude", id: "a", limit: 100 }] }));
  await until(() => m.of(1).length === 1);
  const snap = m.of(1)[0]!;
  assert.equal(snap.type, "snapshot");
  assert.deepEqual([snap.startIndex, snap.endIndex, snap.items.length, snap.totalItems, snap.omittedItems], [150, 250, 100, 250, 150]);
  assert.equal(snap.prefixFp, "pre247");
  assert.deepEqual(source.touched, ["claude/a"], "a feed opened offline counts as opened here");
  const state = snapshotOf(snap);
  assert.equal(state?.canSend, false, "the app reads it as read-only");
  assert.equal(state?.sendBlockReason, "offline: read-only copy");
  assert.equal(state?.summary.activity, "idle");

  // A window that is this copy's is resumed: the overlap, then a read-only state.
  m.socket.send(JSON.stringify({ type: "sub", feeds: [{ sub: 2, source: "claude", id: "a", since: 250, fp: "pre247" }] }));
  await until(() => m.of(2).length === 2);
  assert.deepEqual([m.of(2)[0]!.type, m.of(2)[0]!.fromIndex, m.of(2)[0]!.items.length], ["delta", 247, 3]);
  assert.deepEqual([m.of(2)[1]!.type, m.of(2)[1]!.canSend], ["state", false]);
  // A window from elsewhere gets a snapshot.
  m.socket.send(JSON.stringify({ type: "sub", feeds: [{ sub: 3, source: "claude", id: "a", since: 250, fp: "live-marker" }] }));
  await until(() => m.of(3).length === 1);
  assert.equal(m.of(3)[0]!.type, "snapshot");
  // A number used before is refused, as the machine refuses it.
  m.socket.send(JSON.stringify({ type: "sub", feeds: [{ sub: 2, source: "claude", id: "a" }] }));
  await until(() => m.frames.some((f) => f.sub === 2 && f.type === "closed"));
  // A session the copy does not list.
  m.socket.send(JSON.stringify({ type: "sub", feeds: [{ sub: 4, source: "claude", id: "zzz" }] }));
  await until(() => m.of(4).length === 1);
  assert.equal(m.of(4)[0]!.type, "closed");

  m.socket.send(JSON.stringify({ type: "resync", sub: 1 }));
  await until(() => m.of(1).length === 2);
  assert.deepEqual([m.of(1)[1]!.type, m.of(1)[1]!.endIndex, m.of(1)[1]!.sig], ["reset", 250, "offline:7"]);
  m.socket.send(JSON.stringify({ type: "ping" }));
  await until(() => m.frames.at(-1)?.type === "pong");

  // The copy learns more from the hub: the list, the feed and its state follow.
  source.list = [summary("a", "2026-10-15T01:00:00.000Z", { title: "renamed" }), source.list[1]!];
  source.change({ kind: "index" });
  const delta = m.frames.find((f) => f.type === "sessions:delta")!;
  assert.deepEqual(delta.upserts.map((s: SessionSummary) => s.title), ["renamed"]);
  assert.deepEqual(delta.removed, ["claude/x"]);
  assert.equal(m.of(1).at(-1)!.type, "state");
  assert.equal(m.of(1).at(-1)!.summary.title, "renamed");
  const grown = copyOf(252, 8);
  grown.state.items[249] = { kind: "user", text: "rewritten" } as never;
  source.copies.set("claude/a", grown);
  source.change({ kind: "session", key: "claude/a", fromIndex: 249 });
  const d = m.of(1).at(-1)!;
  assert.deepEqual([d.type, d.fromIndex, d.items.length, d.endIndex, d.sig], ["delta", 249, 3, 252, "offline:8"]);
  source.change({ kind: "session", key: "claude/a", fromIndex: null });
  assert.equal(m.of(1).at(-1)!.type, "reset", "a copy rebuilt from nothing replaces the window");

  m.socket.send(JSON.stringify({ type: "unsub", subs: [1] }));
  source.change({ kind: "session", key: "claude/a", fromIndex: 251 });
  assert.equal(m.of(1).at(-1)!.type, "reset", "nothing after an unsub");
  m.socket.close();
  await until(() => m.isClosed());
  assert.equal(source.listeners.size, 0, "a closed socket stops listening to the copy");
});

test("a feed whose session is not on the device says how its copy is coming: at once, then at most four times a second, until its snapshot", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout", "Date"], now: 100_000 });
  // `until` waits on `setTimeout`, faked here: the socket's promises are let run with `setImmediate`.
  const settle = async () => {
    for (let i = 0; i < 10; i++) await new Promise((resolve) => setImmediate(resolve));
  };
  const source = new FakeSource();
  let arrive!: (copy: SessionCopy | null) => void;
  source.pending.set("codex/b", new Promise((resolve) => (arrive = resolve)));
  const m = mux(source);
  await settle();
  const pages = (done: number, total: number): SubscriptionProgress => ({ phase: "catchingUp", pages: { done, total }, taken: done + 1 });

  m.socket.send(JSON.stringify({ type: "sub", feeds: [{ sub: 5, source: "codex", id: "b", limit: 100 }] }));
  await settle();
  assert.deepEqual(m.of(5), [], "nothing until the session is known not to be on the device");
  // Followed, with nothing read back from the device: the first frame at once, on the feed's own `sub`.
  source.progress("codex/b", { phase: "catchingUp", pages: null, taken: 0 });
  assert.deepEqual(m.of(5), [{ sub: 5, type: "progress", stage: "copy" }]);

  // A checkpoint's pages as they open: held within a quarter second of the last frame, then sent as they stand.
  source.progress("codex/b", pages(0, 2000));
  source.progress("codex/b", pages(1, 2000));
  assert.equal(m.of(5).length, 1, "held");
  t.mock.timers.tick(PROGRESS_EVERY_MS);
  assert.deepEqual(m.of(5).at(-1), { sub: 5, type: "progress", stage: "copy", done: 1, total: 2000 });
  // A page every millisecond for two seconds: eight frames, not two thousand.
  const before = m.of(5).length;
  for (let done = 2; done < 2002; done++) {
    source.progress("codex/b", pages(done, 2000));
    t.mock.timers.tick(1);
  }
  const burst = m.of(5).slice(before);
  assert.equal(burst.length, 8, "four a second");
  assert.ok(burst.every((f, i) => i === 0 || f.done > burst[i - 1]!.done), "each one as things stood when it went");

  // The download over, the records after it coming: no count to show, and the same frame is not sent twice.
  t.mock.timers.tick(PROGRESS_EVERY_MS);
  source.progress("codex/b", { phase: "catchingUp", pages: null, taken: 3000 });
  assert.deepEqual(m.of(5).at(-1), { sub: 5, type: "progress", stage: "copy" }, "pages only while a checkpoint is being fetched");
  const said = m.of(5).length;
  for (let taken = 3001; taken < 3010; taken++) {
    t.mock.timers.tick(PROGRESS_EVERY_MS);
    source.progress("codex/b", { phase: "catchingUp", pages: null, taken });
  }
  assert.equal(m.of(5).length, said, "records move nothing the frame says");

  // One more change still held when the snapshot goes: the snapshot ends it, and nothing comes after.
  source.progress("codex/b", pages(0, 4));
  source.progress("codex/b", pages(1, 4));
  arrive(null);
  await settle();
  const snapshot = m.of(5).at(-1)!;
  assert.equal(snapshot.type, "snapshot");
  assert.equal(snapshotOf(snapshot)?.sendBlockReason, "offline: no copy");
  source.progress("codex/b", pages(2, 4));
  t.mock.timers.tick(10_000);
  assert.equal(m.of(5).at(-1), snapshot, "nothing of it after the snapshot");
  assert.equal(source.progressListeners.get("codex/b")?.size, 0, "and nobody listening any more");

  // A session that is on the device says nothing of the kind, followed as it may be.
  source.progressOf.set("claude/a", { phase: "catchingUp", pages: { done: 1, total: 9 }, taken: 2 });
  m.socket.send(JSON.stringify({ type: "sub", feeds: [{ sub: 6, source: "claude", id: "a", limit: 100 }] }));
  await settle();
  source.progress("claude/a", { phase: "catchingUp", pages: { done: 2, total: 9 }, taken: 3 });
  t.mock.timers.tick(1_000);
  assert.deepEqual(m.of(6).map((f) => f.type), ["snapshot"]);

  // A feed let go of while its copy is still coming: nothing more for it either.
  let late!: (copy: SessionCopy | null) => void;
  source.pending.set("codex/b", new Promise((resolve) => (late = resolve)));
  source.progressOf.delete("codex/b");
  m.socket.send(JSON.stringify({ type: "sub", feeds: [{ sub: 7, source: "codex", id: "b", limit: 100 }] }));
  await settle();
  source.progress("codex/b", pages(0, 4));
  source.progress("codex/b", pages(1, 4));
  m.socket.send(JSON.stringify({ type: "unsub", subs: [7] }));
  t.mock.timers.tick(1_000);
  late(null);
  await settle();
  assert.deepEqual(m.of(7), [{ sub: 7, type: "progress", stage: "copy", done: 0, total: 4 }]);
  m.socket.close();
});

test("terminal sockets close without ever opening", async () => {
  const socket = offlineTransport(new FakeSource(), opts).openSocket("/api/terminals/t-1/ws");
  assert.ok(socket instanceof RefusedSocket);
  let opened = false;
  let closed = false;
  socket.onopen = () => (opened = true);
  socket.onclose = () => (closed = true);
  await until(() => closed);
  assert.equal(opened, false);
});

test("the app's own feed reader takes the offline socket as a machine's", async () => {
  const source = new FakeSource();
  const transport = offlineTransport(source, opts);
  const hub = new FeedHub({ socketUrl: (p) => transport.socketUrl(p), openSocket: (u) => transport.openSocket(u), noteFailure: () => undefined, inflate: null });
  const list: Record<string, unknown>[] = [];
  let open = false;
  // A feed asks for its channel first, as a transcript mounting before the list does.
  const channel = hub.open({ source: "claude", id: "a", limit: 100 })!;
  const got: Record<string, any>[] = [];
  channel.onmessage = (e) => got.push(e.data as Record<string, any>);
  hub.start({ onFrame: (f) => list.push(f), onOpen: () => (open = true), onClose: () => (open = false) });
  await until(() => got.length === 1 && list.some((f) => f.type === "sessions:snapshot"));
  assert.ok(open);
  assert.equal(hub.mode, "mux");
  assert.equal(got[0]!.type, "snapshot");
  assert.equal(got[0]!.canSend, false);
  assert.equal(list[0]!.type, "settings");
  hub.stop();
  await flush(2);
});
