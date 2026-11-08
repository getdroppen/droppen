// A machine's offline copy (`lib/hub/mirror.ts`) against a store the test can
// see into: each stream's state is written together with the signed head it
// stands at, in one transaction; a copy read back is drawn at once and
// followed from its own position; what is not this account's, or no longer
// verifies, is never shown; and only the kept sessions stay on the device.
// A session not on the device is waited for while its copy keeps coming —
// with its progress said to whoever listens — and given up on once it stops
// moving, or at a cap.
import assert from "node:assert/strict";
import test from "node:test";
import { type CopyEvents, type CopyFollow, type CopyName, type CopySource, HubCopySource } from "../src/lib/hub/copy-source.js";
import { decodeSignedHead, FMT_FEED_V1, FMT_INDEX_V1, FMT_STATUS_V1, fromHex, generateDeviceKeys, hex, indexStream, statusStream, streamId } from "../src/lib/e2e/index.js";
import { isStoredCopy, MachineMirror, metaKey, MIRROR_SCHEMA, SESSION_WAIT_CAP_MS, SESSION_WAIT_MS, streamRecordKey } from "../src/lib/hub/mirror.js";
import { copyWindow, type FeedCopy, type IndexState, StreamClient, type SubscriptionProgress } from "../src/lib/hub/streams.js";
import { catchUp, FakeKv, FakeStreamLink, flush, makeMachine, type Machine, putCopy, readCopy, readOpened, StreamWriter, until } from "./stream-fixtures.js";

const row = (id: string, updatedAt: string) => ({ source: "claude", id, title: `t ${id}`, updatedAt, cwd: "/w" });

async function world(machine: Machine, kv: FakeKv, account = "a-1") {
  const link = new FakeStreamLink();
  const client = new StreamClient({ link, machineSignKey: machine.signPublic, epochs: machine.epochs, fetchPage: async () => new Uint8Array(), retryDelay: () => 0 });
  const mirror = new MachineMirror({
    account,
    machine: "m-1",
    source: HubCopySource.over({ mk: machine.mk, client }),
    transact: kv.transact,
    saveDelayMs: { small: 1, session: 1 },
    sessionWaitMs: 500,
  });
  return { link, client, mirror };
}

async function writers(machine: Machine) {
  const index = await StreamWriter.open(machine, await indexStream(machine.mk), FMT_INDEX_V1);
  const status = await StreamWriter.open(machine, await statusStream(machine.mk), FMT_STATUS_V1);
  return { index, status };
}

/** The `sub` frame the link last sent for `writer`'s stream. */
const subFor = (link: FakeStreamLink, writer: StreamWriter) => link.subs().filter((s) => hex(s.stream) === hex(writer.stream)).at(-1);

async function catchUpStream(link: FakeStreamLink, writer: StreamWriter, timeMs?: number) {
  await until(() => subFor(link, writer) !== undefined);
  const sub = subFor(link, writer)!;
  const records = writer.records.slice(sub.fromSeq);
  if (records.length) link.deliver({ type: "recs", ch: sub.ch, stream: writer.stream, records });
  link.deliver({ type: "head", ch: sub.ch, head: await writer.head(timeMs) });
  link.control({ t: "stream_synced", ch: sub.ch, stream: hex(writer.stream), next_seq: writer.seq, exists: true });
}

test("each copy is saved as one record — its state and the signed head it stands at — and read back instantly", async () => {
  const machine = await makeMachine();
  const kv = new FakeKv();
  const w = await writers(machine);
  await w.index.append({ upserts: [row("old", "2026-10-01T00:00:00.000Z"), row("new", "2026-10-14T00:00:00.000Z")], removed: [], asOf: 1 });
  await w.status.append({ asOf: 1, settings: { hostname: "mac" }, accounts: null, quota: null, cli: { claude: "2", codex: null } });

  const first = await world(machine, kv);
  await first.mirror.start();
  assert.equal(first.mirror.hasCopy, false, "nothing on a fresh device");
  await catchUpStream(first.link, w.index, 111_000);
  await catchUpStream(first.link, w.status, 112_000);
  await until(() => first.mirror.hasCopy && first.mirror.status?.state !== null);
  assert.deepEqual(first.mirror.rows()?.map((r) => r.id), ["new", "old"], "newest first");
  assert.equal(first.mirror.freshestMs, 111_000);

  const indexKey = streamRecordKey("a-1", "m-1", hex(w.index.stream));
  await until(() => kv.data.has(indexKey) && kv.data.has(streamRecordKey("a-1", "m-1", hex(w.status.stream))));
  const stored = kv.data.get(indexKey) as { v: number; owner: string; machine: string };
  assert.ok(isStoredCopy(stored), "sealed");
  assert.equal(stored.v, MIRROR_SCHEMA);
  assert.equal(stored.owner, "a-1");
  assert.deepEqual(Object.keys(stored).sort(), ["ct", "iv", "machine", "owner", "v"], "nothing else in the clear");
  const record = (await readCopy<IndexState>(kv, "a-1", "m-1", hex(w.index.stream), "index"))!;
  assert.equal(record.kind, "index");
  const head = decodeSignedHead(fromHex(record.head));
  assert.equal(head.head.nextSeq, 1, "the head is the position of exactly this state");
  assert.equal(hex(head.head.stream), hex(w.index.stream));
  assert.ok(record.state.rows["claude/new"]);
  const writes = kv.txs.filter((t) => t.puts.includes(indexKey));
  assert.ok(writes.length >= 1 && writes.every((t) => t.mode === "readwrite"), "written in a transaction of its own making");
  first.mirror.close();

  // A second load draws from the device before the hub says anything, and
  // asks the hub only for what came after the saved position.
  await w.index.append({ upserts: [row("newer", "2026-10-15T00:00:00.000Z")], removed: ["claude/old"], asOf: 2 });
  const second = await world(machine, kv);
  await second.mirror.start();
  assert.deepEqual(second.mirror.rows()?.map((r) => r.id), ["new", "old"]);
  assert.equal(second.mirror.freshestMs, 111_000, "as of the saved head's own time");
  assert.equal(second.mirror.status?.state?.settings?.hostname, "mac");
  await until(() => subFor(second.link, w.index) !== undefined);
  assert.equal(subFor(second.link, w.index)!.fromSeq, 1);
  await catchUpStream(second.link, w.index, 222_000);
  await until(() => second.mirror.rows()?.[0]?.id === "newer");
  assert.deepEqual(second.mirror.rows()?.map((r) => r.id), ["newer", "new"]);
  second.mirror.close();
});

test("a copy that is not this account's, or whose head no longer verifies, is never shown", async () => {
  const machine = await makeMachine();
  const w = await writers(machine);
  await w.index.append({ upserts: [row("secret", "2026-10-14T00:00:00.000Z")], removed: [] });
  const head = await w.index.head(5_000);
  const forger = await generateDeviceKeys();
  const forgedHead = await w.index.head(5_000, undefined, forger.sign.privateKey);
  const state = { rows: { "claude/secret": row("secret", "2026-10-14T00:00:00.000Z") }, asOf: null };
  const record = (h: Uint8Array) => ({ kind: "index" as const, head: hex(h), state, freshestMs: 5_000, savedAt: 1 });

  // Another account's copy under this account's key: ignored.
  {
    const kv = new FakeKv();
    const theirs = await putCopy(kv, "a-2", "m-1", hex(w.index.stream), record(head));
    kv.data.set(streamRecordKey("a-1", "m-1", hex(w.index.stream)), kv.data.get(theirs));
    kv.data.delete(theirs);
    const { mirror, link } = await world(machine, kv);
    await mirror.start();
    assert.equal(mirror.hasCopy, false);
    assert.equal(subFor(link, w.index)?.fromSeq ?? (await until(() => subFor(link, w.index) !== undefined), subFor(link, w.index)!.fromSeq), 0);
    mirror.close();
  }
  // A head the machine did not sign: ignored, and the stream read from the start.
  {
    const kv = new FakeKv();
    await putCopy(kv, "a-1", "m-1", hex(w.index.stream), record(forgedHead));
    const { mirror, link } = await world(machine, kv);
    await mirror.start();
    assert.equal(mirror.hasCopy, false);
    await until(() => subFor(link, w.index) !== undefined);
    assert.equal(subFor(link, w.index)!.fromSeq, 0);
    mirror.close();
  }
  // Another account's copies on the device are swept when this account opens.
  {
    const kv = new FakeKv();
    await putCopy(kv, "a-2", "m-9", "ab", record(head));
    kv.data.set(metaKey("a-2", "m-9"), { v: MIRROR_SCHEMA, owner: "a-2", machine: "m-9", opened: [] });
    kv.data.set("feed:claude/x", { v: 1 });
    await putCopy(kv, "a-1", "m-1", hex(w.index.stream), record(head));
    const { mirror } = await world(machine, kv);
    await mirror.start();
    assert.equal(mirror.hasCopy, true, "this account's own copy is shown");
    assert.deepEqual([...kv.data.keys()].sort(), ["feed:claude/x", streamRecordKey("a-1", "m-1", hex(w.index.stream))].sort());
    mirror.close();
  }
});

test("sessions: the most recent few are kept warm, an opened one is followed and kept, and the rest are swept in the saving transaction", async () => {
  const machine = await makeMachine();
  const kv = new FakeKv();
  const w = await writers(machine);
  const ids = ["s1", "s2", "s3", "s4", "s5", "s6", "s7"];
  await w.index.append({ upserts: ids.map((id, i) => row(id, `2026-10-${String(10 + i).padStart(2, "0")}T00:00:00.000Z`)), removed: [] });
  const sessions = new Map<string, StreamWriter>();
  for (const id of ids) {
    const writer = await StreamWriter.open(machine, await streamId(machine.mk, "claude", id), FMT_FEED_V1);
    await writer.append({ fromIndex: 0, items: [{ kind: "user", text: `hello ${id}` }], endIndex: 1, fp: "f1", prefixFp: "p0" });
    sessions.set(id, writer);
  }
  // A stale copy of a session nobody keeps any more.
  const staleKey = streamRecordKey("a-1", "m-1", "00".repeat(32));
  await putCopy(kv, "a-1", "m-1", "00".repeat(32), { kind: "session", head: "", state: {}, freshestMs: 0, savedAt: 0 });

  const { mirror, link } = await world(machine, kv);
  await mirror.start();
  await catchUpStream(link, w.index);
  await until(() => mirror.hasCopy);
  // The five newest (s3…s7) are followed without anybody asking.
  const warm = ["s3", "s4", "s5", "s6", "s7"];
  await until(() => warm.every((id) => subFor(link, sessions.get(id)!) !== undefined));
  assert.equal(subFor(link, sessions.get("s1")!), undefined, "an old session nobody opened is not");
  for (const id of warm) await catchUpStream(link, sessions.get(id)!);
  await until(() => warm.every((id) => mirror.sessionNow("claude", id) !== null));

  // Opening s1 (offline or live) follows it and keeps it.
  mirror.touch("claude", "s1");
  await until(() => subFor(link, sessions.get("s1")!) !== undefined);
  await catchUpStream(link, sessions.get("s1")!);
  const s1Key = streamRecordKey("a-1", "m-1", hex(sessions.get("s1")!.stream));
  await until(() => kv.data.has(s1Key));
  const record = (await readCopy<FeedCopy>(kv, "a-1", "m-1", hex(sessions.get("s1")!.stream), "session"))!;
  assert.equal(record.kind, "session");
  assert.equal(record.source, "claude");
  assert.equal(record.id, "s1");
  assert.equal(decodeSignedHead(fromHex(record.head)).head.nextSeq, 1);
  assert.deepEqual(record.state.items, [{ kind: "user", text: "hello s1" }]);
  // The copy, the opened list and the sweep: one transaction.
  const tx = kv.txs.find((t) => t.puts.includes(s1Key))!;
  assert.ok(tx.puts.includes(metaKey("a-1", "m-1")), "the opened list rides in the same transaction");
  assert.ok(!kv.data.has(staleKey), "a copy nobody keeps is swept");
  assert.deepEqual((await readOpened(kv, "a-1", "m-1")).map((o) => o.id), ["s1"]);
  mirror.close();

  // Next load: an opened session's copy is there before the hub answers.
  const again = await world(machine, kv);
  await again.mirror.start();
  const copy = await again.mirror.session("claude", "s1", 0);
  assert.deepEqual(copy?.state.items, [{ kind: "user", text: "hello s1" }]);
  assert.equal(copy?.seq, 1);
  again.mirror.close();
});

test("a session with no copy on the device waits for the hub, and one the hub has not got answers null", async () => {
  const machine = await makeMachine();
  const kv = new FakeKv();
  const { mirror, link } = await world(machine, kv);
  await mirror.start();
  const writer = await StreamWriter.open(machine, await streamId(machine.mk, "codex", "c-1"), FMT_FEED_V1);
  await writer.append({ fromIndex: 0, items: [{ kind: "user", text: "fetched" }], endIndex: 1 });
  const asked = mirror.session("codex", "c-1");
  await until(() => subFor(link, writer) !== undefined);
  await catchUp(link, writer);
  const copy = await asked;
  assert.deepEqual(copy?.state.items, [{ kind: "user", text: "fetched" }]);

  const none = mirror.session("codex", "c-2");
  await until(() => link.subs().length >= 4);
  const sub = link.lastSub();
  link.control({ t: "stream_synced", ch: sub.ch, stream: hex(sub.stream), next_seq: 0, exists: false });
  assert.equal(await none, null);
  await flush(2);
  mirror.close();
});

/** A source whose sessions the test moves by hand: each follow, its progress, and what it was told to say. */
class HandSource implements CopySource {
  readonly keyed = true;
  reachable = true;
  readonly follows: { name: CopyName; events: CopyEvents<unknown>; progress: SubscriptionProgress; listeners: Set<() => void>; closed: boolean }[] = [];
  ready() {
    return Promise.resolve();
  }
  onKeys() {
    return () => undefined;
  }
  async places(name: CopyName) {
    return [name.kind === "session" ? `session/${name.source}/${name.id}` : name.kind];
  }
  async resume() {
    return null;
  }
  follow<S>(name: CopyName, _places: readonly string[], _start: unknown, events: CopyEvents<S>): CopyFollow {
    const f = { name, events: events as CopyEvents<unknown>, progress: { phase: "catchingUp", pages: null, taken: 0 } as SubscriptionProgress, listeners: new Set<() => void>(), closed: false };
    this.follows.push(f);
    return {
      close: () => {
        f.closed = true;
        f.listeners.clear();
      },
      get progress() {
        return f.progress;
      },
      onProgress: (fn) => {
        f.listeners.add(fn);
        return () => void f.listeners.delete(fn);
      },
    };
  }
  /** The open follow of session `id`. */
  session(id: string) {
    return this.follows.find((f) => f.name.kind === "session" && f.name.id === id && !f.closed);
  }
  /** Something arrived on `id`'s stream — said (as a frame says it), or not (a background tab has no frames). */
  move(id: string, said = true, pages: SubscriptionProgress["pages"] = null) {
    const f = this.session(id)!;
    f.progress = { ...f.progress, pages, taken: f.progress.taken + 1 };
    if (said) for (const fn of [...f.listeners]) fn();
  }
  close() {}
}

/** A mirror over a {@link HandSource}, in memory, started. */
async function handWorld() {
  const source = new HandSource();
  const mirror = new MachineMirror({ account: "a-1", machine: "m-1", source, persist: false, sweep: false, followSessions: false });
  await mirror.start();
  return { source, mirror };
}

/** Let the mirror's promises run (with `setTimeout` faked, `until` would never wake). */
const settle = async () => {
  for (let i = 0; i < 10; i++) await new Promise((resolve) => setImmediate(resolve));
};

test("a session's progress is its follow's, and who listens hears its follow start, move and stop", async () => {
  const { source, mirror } = await handWorld();
  assert.equal(mirror.sessionProgress("claude", "big"), null, "not followed: nothing to say");
  let heard = 0;
  let elsewhere = 0;
  const off = mirror.onSessionProgress("claude", "big", () => heard++);
  mirror.onSessionProgress("claude", "else", () => elsewhere++);
  const asked = mirror.session("claude", "big");
  await settle();
  assert.equal(heard, 1, "its follow started");
  assert.deepEqual(mirror.sessionProgress("claude", "big"), { phase: "catchingUp", pages: null, taken: 0 });
  source.move("big", true, { done: 3, total: 80 });
  assert.equal(heard, 2);
  assert.deepEqual(mirror.sessionProgress("claude", "big")?.pages, { done: 3, total: 80 });
  assert.equal(elsewhere, 0, "another session hears nothing of this one");
  // The copy is in: the wait is over at once.
  source.session("big")!.events.onState({ state: { started: true, startIndex: 0, endIndex: 1, items: [{ kind: "user", text: "hi" }], fp: null, prefixFp: null }, at: { mark: "m", seq: 1, timeMs: 1 }, reset: true, bodies: [], freshestMs: 1 });
  assert.deepEqual((await asked)?.state.items, [{ kind: "user", text: "hi" }]);
  // Nothing keeps it followed (neither warm nor opened here): let go of.
  mirror.setFollowSessions(true);
  assert.equal(heard, 3, "its follow stopped");
  assert.equal(mirror.sessionProgress("claude", "big"), null);
  off();
  mirror.setFollowSessions(false);
  mirror.close();
});

test("a session not on the device is waited for while it moves: still at 12 s while pages keep coming, given up 8 s after the last", async (t) => {
  const { source, mirror } = await handWorld();
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let answered: unknown = "waiting";
  void mirror.session("claude", "big").then((copy) => (answered = copy));
  await settle();
  assert.ok(source.session("big"), "asked of the source");
  // A page every half second, for twelve seconds: past the old 8 s give-up.
  for (let ms = 0; ms < 12_000; ms += 500) {
    source.move("big", true, { done: ms / 500, total: 250 });
    t.mock.timers.tick(500);
  }
  await settle();
  assert.equal(answered, "waiting", "still waiting at 12 s: pages keep coming");
  // Then nothing: given up 8 s after the last page (at 11.5 s), not before.
  t.mock.timers.tick(SESSION_WAIT_MS - 500 - 1);
  await settle();
  assert.equal(answered, "waiting");
  t.mock.timers.tick(1);
  await settle();
  assert.equal(answered, null, "8 s with nothing arriving: no copy, for now");
  mirror.close();
});

test("a background tab hears no progress, so the wait looks itself when its 8 s run out", async (t) => {
  const { source, mirror } = await handWorld();
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let answered: unknown = "waiting";
  void mirror.session("claude", "quiet").then((copy) => (answered = copy));
  await settle();
  t.mock.timers.tick(1_000);
  source.move("quiet", false);
  t.mock.timers.tick(SESSION_WAIT_MS - 1_000);
  await settle();
  assert.equal(answered, "waiting", "it moved in those 8 s, though nobody said so: it waits on");
  t.mock.timers.tick(SESSION_WAIT_MS);
  await settle();
  assert.equal(answered, null, "and gives up after 8 s more with nothing");
  mirror.close();
});

test("a stream that never stops moving is given up on at the cap all the same", async (t) => {
  const { source, mirror } = await handWorld();
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let answered: unknown = "waiting";
  void mirror.session("claude", "endless").then((copy) => (answered = copy));
  await settle();
  for (let ms = 0; ms < SESSION_WAIT_CAP_MS - 1_000; ms += 1_000) {
    source.move("endless");
    t.mock.timers.tick(1_000);
  }
  source.move("endless");
  t.mock.timers.tick(999);
  await settle();
  assert.equal(answered, "waiting", "a moment before the cap, still moving: still waiting");
  t.mock.timers.tick(1);
  await settle();
  assert.equal(answered, null, `at ${SESSION_WAIT_CAP_MS / 1000} s, moving or not: no copy, for now`);
  assert.ok(source.session("endless"), "still followed: the copy replaces the empty one once it is in");
  mirror.close();
});

test("a trimmed session copy keeps its window on the device: the next load still knows what is only on the machine", async () => {
  const machine = await makeMachine();
  const kv = new FakeKv();
  const first = await world(machine, kv);
  await first.mirror.start();
  const writer = await StreamWriter.open(machine, await streamId(machine.mk, "claude", "long"), FMT_FEED_V1);
  // What a fresh reader folds from a trimmed copy (here as records; the agent sends the window page first in a manifest).
  await writer.append({ fromIndex: 40, items: [], endIndex: 40, base: 40, before: "2026-10-01T08:00:00.000Z" });
  await writer.append({ fromIndex: 40, items: [{ kind: "user", text: "kept" }], endIndex: 41 });
  first.mirror.touch("claude", "long");
  const asked = first.mirror.session("claude", "long");
  await until(() => subFor(first.link, writer) !== undefined);
  await catchUp(first.link, writer);
  const copy = await asked;
  assert.deepEqual(copyWindow(copy?.state), { base: 40, before: "2026-10-01T08:00:00.000Z" });
  assert.deepEqual([copy?.state.startIndex, copy?.state.items.length], [40, 1]);
  await until(() => kv.data.has(streamRecordKey("a-1", "m-1", hex(writer.stream))));
  first.mirror.close();

  const again = await world(machine, kv);
  await again.mirror.start();
  const saved = await again.mirror.session("claude", "long", 0);
  assert.deepEqual(copyWindow(saved?.state), { base: 40, before: "2026-10-01T08:00:00.000Z" });
  again.mirror.close();
});
