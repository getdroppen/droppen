// How far a machine's copy has caught up from the hub (`lib/hub/streams.ts`,
// `StreamClient.progress`): each subscription's phase — waiting for a `ch`,
// catching up on it, synced, or waiting out a backoff — counted by kind; a
// checkpoint's pages counted as they open, and over the whole catch-up; the
// events coalesced, so a burst is one; and the copy source's pass-through,
// which copes with a client made only once the machine's keys arrive. And
// one subscription's own (`StreamSubscription.progress`): its download's
// pages and the records it takes, said in the same frames, and a session's
// follow handing it on.
import assert from "node:assert/strict";
import test from "node:test";
import { type Bytes, FMT_FEED_V1, FMT_INDEX_V1, FMT_STATUS_V1, hex, indexStream, type MachineKey, statusStream, streamId } from "../src/lib/e2e/index.js";
import { HubCopySource } from "../src/lib/hub/copy-source.js";
import {
  feedFold,
  indexFold,
  isCatchingUp,
  NO_PROGRESS,
  statusFold,
  StreamClient,
  type StreamClientOptions,
  type StreamProgress,
  type SubscriptionProgress,
} from "../src/lib/hub/streams.js";
import { FakeStreamLink, flush, type Machine, makeMachine, StreamWriter, until } from "./stream-fixtures.js";

const row = (id: string) => ({ source: "claude", id, title: `title ${id}`, updatedAt: "2026-11-08T00:00:00.000Z" });
const items = (from: number, to: number) => Array.from({ length: to - from }, (_, i) => ({ kind: "user", text: `item ${from + i}` }));
const feed = (from: number, to: number) => ({ fromIndex: from, items: items(from, to), endIndex: to, fp: `f${to}`, prefixFp: `p${to}` });

function client(machine: Machine, link: FakeStreamLink, opts: Partial<StreamClientOptions> = {}) {
  return new StreamClient({
    link,
    machineSignKey: machine.signPublic,
    epochs: machine.epochs,
    fetchPage: async () => {
      throw new Error("404");
    },
    retryDelay: () => 0,
    progressFrame: queueMicrotask,
    ...opts,
  });
}

/** The latest `sub` for `stream`. */
function subFor(link: FakeStreamLink, stream: Bytes) {
  const sub = link.subs().filter((s) => hex(s.stream) === hex(stream)).at(-1);
  if (!sub) throw new Error("not subscribed");
  return sub;
}

/** Play the hub's catch-up on `writer`'s latest `sub`: its records (from `from`, past a checkpoint), a head, `stream_synced`. */
async function catchUpOn(link: FakeStreamLink, writer: StreamWriter, from?: number): Promise<void> {
  const sub = subFor(link, writer.stream);
  const records = writer.records.slice(from ?? sub.fromSeq);
  if (records.length) link.deliver({ type: "recs", ch: sub.ch, stream: writer.stream, records });
  link.deliver({ type: "head", ch: sub.ch, head: await writer.head() });
  link.control({ t: "stream_synced", ch: sub.ch, stream: hex(writer.stream), next_seq: writer.seq, exists: true });
}

const only = (kind: "index" | "status" | "session", n = 1) => ({ index: 0, status: 0, session: 0, [kind]: n });

test("each subscription says what it is doing: waiting, catching up, synced, waiting again when the link drops, retrying while it waits out a backoff", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const stream = await indexStream(machine.mk);
  const writer = await StreamWriter.open(machine, stream, FMT_INDEX_V1);
  await writer.append({ upserts: [row("a")], removed: [] });
  const c = client(machine, link);
  const sub = c.subscribe(stream, indexFold);
  assert.equal(sub.phase, "waiting", "its reader is still being made: no ch yet");
  assert.deepEqual(c.progress.waiting, only("index"));

  await until(() => link.subs().length === 1);
  assert.equal(sub.phase, "catchingUp", "SUB sent, stream_synced not heard yet");
  assert.deepEqual(c.progress.catchingUp, only("index"));
  assert.equal(isCatchingUp(c.progress), true);

  await catchUpOn(link, writer);
  await until(() => sub.phase === "synced");
  assert.deepEqual(c.progress.synced, only("index"));
  assert.deepEqual(c.progress.catchingUp, only("index", 0));
  assert.equal(isCatchingUp(c.progress), false);

  link.drop();
  assert.equal(sub.phase, "waiting", "the link went with its ch: back to waiting, not still \"synced\"");
  assert.deepEqual(c.progress.waiting, only("index"));
  assert.equal(isCatchingUp(c.progress), false, "nothing is being caught up on with the link down");

  link.reopen();
  await until(() => link.subs().length === 2);
  assert.equal(sub.phase, "catchingUp", "back, it catches up again from where it stands");

  link.control({ t: "stream_refused", ch: link.lastSub().ch, stream: hex(stream), code: "rate", retry_ms: 60_000 });
  await until(() => sub.phase === "retrying");
  assert.deepEqual(c.progress.retrying, only("index"));
  assert.equal(isCatchingUp(c.progress), false, "a stream waiting out a backoff is not getting anywhere");

  sub.close();
  assert.deepEqual(c.progress, NO_PROGRESS, "a subscription closed is not counted");
  c.close();
});

test("the client counts the index, the status and the sessions apart, and says when the counts change", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const index = await StreamWriter.open(machine, await indexStream(machine.mk), FMT_INDEX_V1);
  const status = await StreamWriter.open(machine, await statusStream(machine.mk), FMT_STATUS_V1);
  const session = await StreamWriter.open(machine, await streamId(machine.mk, "claude", "s-1"), FMT_FEED_V1);
  await index.append({ upserts: [row("s-1")], removed: [] });
  await status.append({ asOf: 1 });
  await session.append(feed(0, 2));
  const c = client(machine, link);
  const heard: StreamProgress[] = [];
  c.onProgress((p) => heard.push(p));
  c.subscribe(index.stream, indexFold);
  c.subscribe(status.stream, statusFold);
  c.subscribe(session.stream, feedFold);
  await until(() => heard.at(-1)?.catchingUp.session === 1 && heard.at(-1)?.catchingUp.index === 1 && heard.at(-1)?.catchingUp.status === 1);
  assert.deepEqual(heard[0]!.waiting, { index: 1, status: 1, session: 1 }, "first heard: all three, waiting for their ch");

  await catchUpOn(link, index);
  await until(() => heard.at(-1)?.synced.index === 1);
  assert.deepEqual(heard.at(-1)!.catchingUp, { index: 0, status: 1, session: 1 });
  await catchUpOn(link, status);
  await catchUpOn(link, session);
  await until(() => heard.at(-1)?.synced.session === 1 && heard.at(-1)?.synced.status === 1);
  assert.deepEqual(heard.at(-1)!.catchingUp, { index: 0, status: 0, session: 0 });
  assert.equal(isCatchingUp(heard.at(-1)!), false);
  assert.ok(heard.every((p, i) => i === 0 || p !== heard[i - 1]), "every event says something new");
  c.close();
});

test("a checkpoint's pages are counted as they open, and the catch-up keeps the count until it is over", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const stream = await streamId(machine.mk, "claude", "s-cp");
  const writer = await StreamWriter.open(machine, stream, FMT_FEED_V1);
  for (const [from, to] of [[0, 2], [2, 4], [4, 6]] as const) await writer.append(feed(from, to));
  const cp = await writer.manifest([{ fromIndex: 0, items: items(0, 2), endIndex: 2 }, { fromIndex: 2, items: items(2, 4), endIndex: 4 }, feed(4, 6)]);
  // One page at a time, each let through by the test.
  const gates: (() => void)[] = [];
  const c = client(machine, link, {
    pageConcurrency: 1,
    fetchPage: async (_stream, page) => {
      await new Promise<void>((resolve) => gates.push(resolve));
      return cp.pages.get(hex(page))!;
    },
  });
  const heard: StreamProgress[] = [];
  c.onProgress((p) => heard.push(p));
  const sub = c.subscribe(stream, feedFold);
  await until(() => link.subs().length === 1);
  const before = Date.now();
  link.deliver({ type: "checkpoint", ch: link.lastSub().ch, stream, uptoSeq: cp.upto, manifest: cp.manifest });
  await until(() => gates.length === 1);
  let p = c.progress;
  assert.deepEqual([p.pages, p.episode], [{ done: 0, total: 3 }, { done: 0, total: 3 }], "the manifest names three pages");
  assert.ok(p.downloadingSince !== null && p.downloadingSince >= before);
  assert.equal(isCatchingUp(p), true);

  gates.shift()!();
  await until(() => c.progress.pages.done === 1);
  assert.deepEqual(c.progress.episode, { done: 1, total: 3 });
  await until(() => gates.length === 1);
  gates.shift()!();
  await until(() => gates.length === 1);
  assert.deepEqual(c.progress.pages, { done: 2, total: 3 });
  gates.shift()!();
  await until(() => c.progress.downloadingSince === null);
  p = c.progress;
  assert.deepEqual(p.pages, { done: 0, total: 0 }, "the download is over");
  assert.deepEqual(p.episode, { done: 3, total: 3 }, "the catch-up is not: the stream has not synced yet");
  assert.equal(sub.phase, "catchingUp");

  await catchUpOn(link, writer, cp.upto);
  await until(() => sub.phase === "synced");
  assert.deepEqual(c.progress.episode, { done: 0, total: 0 }, "caught up: the next catch-up counts from nothing");
  const done = heard.map((h) => h.pages.done);
  assert.deepEqual(
    done.filter((d, i) => d !== done[i - 1]),
    [0, 1, 2, 3, 0],
    "each page heard as it opened, then the download gone",
  );
  c.close();
});

test("a second download adds to the first; a failed one stops being owed what it never fetched, so the share may drop", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  // The index keeps catching up throughout: one catch-up episode.
  const index = await StreamWriter.open(machine, await indexStream(machine.mk), FMT_INDEX_V1);
  const stream = await streamId(machine.mk, "claude", "s-fail");
  const writer = await StreamWriter.open(machine, stream, FMT_FEED_V1);
  for (const [from, to] of [[0, 2], [2, 4], [4, 6]] as const) await writer.append(feed(from, to));
  const cp = await writer.manifest([{ fromIndex: 0, items: items(0, 2), endIndex: 2 }, { fromIndex: 2, items: items(2, 4), endIndex: 4 }, feed(4, 6)]);
  // Pages are fetched last first: the first page, fetched last, fails once.
  // The download after that is let through a page at a time by the test.
  const first = [...cp.pages.keys()][0]!;
  let failed = false;
  const gates: (() => void)[] = [];
  const c = client(machine, link, {
    pageConcurrency: 1,
    fetchPage: async (_stream, page) => {
      if (!failed) {
        if (hex(page) !== first) return cp.pages.get(hex(page))!;
        failed = true;
        throw new Error("503");
      }
      await new Promise<void>((resolve) => gates.push(resolve));
      return cp.pages.get(hex(page))!;
    },
  });
  c.subscribe(index.stream, indexFold);
  c.subscribe(stream, feedFold);
  await until(() => link.subs().length === 2);
  link.deliver({ type: "checkpoint", ch: subFor(link, stream).ch, stream, uptoSeq: cp.upto, manifest: cp.manifest });
  // Two pages opened, the third failed: the stream starts over (backing off 0 ms here).
  await until(() => failed && c.progress.downloadingSince === null);
  assert.deepEqual(c.progress.episode, { done: 2, total: 2 }, "what it fetched stays counted; the page it never will is not owed");

  await until(() => link.subs().length === 3);
  link.deliver({ type: "checkpoint", ch: subFor(link, stream).ch, stream, uptoSeq: cp.upto, manifest: cp.manifest });
  await until(() => gates.length === 1);
  assert.deepEqual(c.progress.episode, { done: 2, total: 5 }, "the second download's three pages add to the first's: 2 of 5 (from 100 % to 40 %), not 0 of 3");
  assert.deepEqual(c.progress.pages, { done: 0, total: 3 }, "while the download in flight counts its own");
  for (let i = 0; i < 3; i++) {
    await until(() => gates.length === 1);
    gates.shift()!();
  }
  await until(() => c.progress.downloadingSince === null);
  assert.deepEqual(c.progress.episode, { done: 5, total: 5 });
  c.close();
});

test("a burst of changes is one event, said when the frame comes; one that ends where it began says nothing", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const frames: (() => void)[] = [];
  const stream = await streamId(machine.mk, "claude", "s-big");
  const writer = await StreamWriter.open(machine, stream, FMT_FEED_V1);
  const bodies = Array.from({ length: 32 }, (_, i) => ({ fromIndex: i, items: items(i, i + 1), endIndex: i + 1 }));
  for (const body of bodies) await writer.append(body);
  const cp = await writer.manifest(bodies);
  const c = client(machine, link, { progressFrame: (fn) => frames.push(fn), fetchPage: async (_stream, page) => cp.pages.get(hex(page))! });
  const heard: StreamProgress[] = [];
  c.onProgress((p) => heard.push(p));

  const others = await Promise.all([1, 2, 3, 4].map((i) => streamId(machine.mk, "claude", `s-${i}`)));
  for (const s of others) c.subscribe(s, feedFold);
  c.subscribe(stream, feedFold);
  assert.equal(frames.length, 1, "one frame asked for, however many changed");
  await until(() => link.subs().length === 5);
  assert.equal(frames.length, 1, "and no more while it has not come");
  frames.shift()!();
  assert.equal(heard.length, 1);
  assert.deepEqual(heard[0]!.catchingUp, only("session", 5), "said as things stand when the frame comes");

  // A checkpoint of 32 pages: dozens of changes, one event.
  link.deliver({ type: "checkpoint", ch: subFor(link, stream).ch, stream, uptoSeq: cp.upto, manifest: cp.manifest });
  await until(() => frames.length === 1 && c.progress.downloadingSince === null && c.progress.episode.done === 32);
  frames.shift()!();
  assert.equal(heard.length, 2, "32 pages opened between two frames: one event");
  assert.deepEqual(heard[1]!.episode, { done: 32, total: 32 });

  // Something that came and went before the frame: nothing to say.
  const passing = c.subscribe(await streamId(machine.mk, "claude", "s-passing"), feedFold);
  passing.close();
  assert.equal(frames.length, 1);
  frames.shift()!();
  assert.equal(heard.length, 2, "the counts are as they were");
  c.close();
});

test("one subscription says how far it alone has come: its checkpoint's pages as they open, the records after them, and a link drop", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const stream = await streamId(machine.mk, "claude", "s-one");
  const writer = await StreamWriter.open(machine, stream, FMT_FEED_V1);
  for (const [from, to] of [[0, 2], [2, 4], [4, 6]] as const) await writer.append(feed(from, to));
  const cp = await writer.manifest([{ fromIndex: 0, items: items(0, 2), endIndex: 2 }, { fromIndex: 2, items: items(2, 4), endIndex: 4 }, feed(4, 6)]);
  // Two records after the checkpoint: no count of them is known until the head.
  await writer.append(feed(6, 7));
  await writer.append(feed(7, 8));
  const gates: (() => void)[] = [];
  const c = client(machine, link, {
    pageConcurrency: 1,
    fetchPage: async (_stream, page) => {
      await new Promise<void>((resolve) => gates.push(resolve));
      return cp.pages.get(hex(page))!;
    },
  });
  // Another session over the same client: what it does is not this one's.
  const other = c.subscribe(await streamId(machine.mk, "claude", "s-other"), feedFold);
  const sub = c.subscribe(stream, feedFold);
  assert.deepEqual(sub.progress, { phase: "waiting", pages: null, taken: 0 });
  const heard: SubscriptionProgress[] = [];
  sub.onProgress((p) => heard.push(p));
  await until(() => heard.at(-1)?.phase === "catchingUp");

  link.deliver({ type: "checkpoint", ch: subFor(link, stream).ch, stream, uptoSeq: cp.upto, manifest: cp.manifest });
  await until(() => gates.length === 1);
  assert.deepEqual(sub.progress.pages, { done: 0, total: 3 }, "the manifest names its three pages");
  assert.equal(other.progress.pages, null, "the other session is downloading nothing");
  for (let i = 1; i <= 3; i++) {
    await until(() => gates.length === 1);
    gates.shift()!();
    await until(() => sub.progress.pages?.done === i || sub.progress.pages === null);
  }
  await until(() => sub.progress.pages === null);
  assert.deepEqual(sub.progress, { phase: "catchingUp", pages: null, taken: 4 }, "the download over: the manifest and its three pages taken");

  link.deliver({ type: "recs", ch: subFor(link, stream).ch, stream, records: writer.records.slice(cp.upto) });
  await until(() => heard.at(-1)?.taken === 6);
  assert.equal(sub.progress.pages, null, "records are taken, not counted against a total");
  const pages = heard.map((p) => p.pages?.done ?? null);
  assert.deepEqual(
    pages.filter((d, i) => i === 0 || d !== pages[i - 1]),
    [null, 0, 1, 2, 3, null],
    "each page heard as it opened, then the download gone",
  );

  link.drop();
  assert.equal(sub.progress.phase, "waiting", "the link went with its ch");
  await until(() => heard.at(-1)?.phase === "waiting");
  assert.equal(heard.at(-1)!.taken, 6, "what it took stays counted");
  link.reopen();
  await until(() => heard.at(-1)?.phase === "catchingUp");
  assert.equal(other.progress.taken, 0);

  const said = heard.length;
  sub.close();
  c.subscribe(await streamId(machine.mk, "claude", "s-third"), feedFold);
  await flush(5);
  assert.equal(heard.length, said, "closed: nothing more");
  c.close();
});

test("a subscription's progress is said in the machine's frame, once for a burst, and only when it moved", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const frames: (() => void)[] = [];
  const stream = await streamId(machine.mk, "claude", "s-burst");
  const writer = await StreamWriter.open(machine, stream, FMT_FEED_V1);
  for (let i = 0; i < 12; i++) await writer.append(feed(i, i + 1));
  const c = client(machine, link, { progressFrame: (fn) => frames.push(fn) });
  const sub = c.subscribe(stream, feedFold);
  const still = c.subscribe(await streamId(machine.mk, "claude", "s-still"), feedFold);
  const heard: SubscriptionProgress[] = [];
  const stillHeard: SubscriptionProgress[] = [];
  sub.onProgress((p) => heard.push(p));
  still.onProgress((p) => stillHeard.push(p));
  await until(() => link.subs().length === 2);
  assert.equal(frames.length, 1, "no machine-wide listener, yet a frame is asked for: the subscriptions' listeners");
  frames.shift()!();
  assert.deepEqual([heard.at(-1)?.phase, stillHeard.at(-1)?.phase], ["catchingUp", "catchingUp"]);

  // Twelve records in three frames from the hub, between two of the page's frames: one event.
  for (const at of [0, 4, 8]) link.deliver({ type: "recs", ch: subFor(link, stream).ch, stream, records: writer.records.slice(at, at + 4) });
  await until(() => sub.progress.taken === 12);
  const before = heard.length;
  frames.shift()!();
  assert.equal(heard.length, before + 1);
  assert.equal(heard.at(-1)!.taken, 12);
  assert.equal(stillHeard.length, 1, "the other subscription did not move: it hears nothing");
  c.close();
});

test("the copy source hands a session's follow its progress through; the index's says none", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const c = client(machine, link);
  const source = HubCopySource.over({ mk: machine.mk, client: c });
  const name = { kind: "session" as const, source: "claude", id: "s-1" };
  const follow = source.follow(name, await source.places(name), { at: 0, from: null }, { onState: () => undefined })!;
  let heard = 0;
  follow.onProgress!(() => heard++);
  await until(() => follow.progress?.phase === "catchingUp" && heard > 0);
  const writer = await StreamWriter.open(machine, await streamId(machine.mk, "claude", "s-1"), FMT_FEED_V1);
  await writer.append(feed(0, 2));
  link.deliver({ type: "recs", ch: subFor(link, writer.stream).ch, stream: writer.stream, records: writer.records });
  await until(() => follow.progress?.taken === 1);
  const index = source.follow({ kind: "index" }, await source.places({ kind: "index" }), { at: 0, from: null }, { onState: () => undefined })!;
  assert.equal(index.progress, undefined, "followed under several names: it shows as the machine's");
  assert.equal(index.onProgress, undefined);
  follow.close();
  index.close();
  c.close();
});

test("by default a burst is said once, after it (a microtask where there are no animation frames)", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const c = client(machine, link, { progressFrame: undefined });
  const heard: StreamProgress[] = [];
  c.onProgress((p) => heard.push(p));
  const streams = await Promise.all(["a", "b", "c", "d"].map((id) => streamId(machine.mk, "claude", id)));
  for (const s of streams.slice(0, 3)) c.subscribe(s, feedFold);
  assert.equal(heard.length, 0, "not while the burst is still going");
  await Promise.resolve();
  assert.equal(heard.length, 1, "three subscriptions, one event");
  assert.deepEqual(heard[0]!.waiting, only("session", 3));
  await until(() => heard.at(-1)?.catchingUp.session === 3);
  const said = heard.length;
  c.close();
  c.subscribe(streams[3]!, feedFold);
  await flush(5);
  assert.equal(heard.length, said, "a closed client says nothing more");
});

/** An account that holds no key for the machine until `grant()`, and says so when it changes. */
function account(signKey: Uint8Array, epochs: Map<number, MachineKey>) {
  let held = false;
  const listeners = new Set<() => void>();
  return {
    machine: (id: string) => (held && id === "m-1" ? { signKey } : undefined),
    onChange(fn: () => void) {
      listeners.add(fn);
      return () => void listeners.delete(fn);
    },
    keys: async () => (held ? epochs : new Map<number, MachineKey>()),
    grant() {
      held = true;
      for (const fn of [...listeners]) fn();
    },
  };
}

test("the copy source passes the progress through, from a client made only once the machine's keys arrive", async () => {
  const machine = await makeMachine();
  const acct = account(machine.signPublic, machine.epochs);
  const link = new FakeStreamLink();
  const source = HubCopySource.following({
    account: acct,
    machine: "m-1",
    keys: acct.keys,
    client: { link, fetchPage: async () => new Uint8Array(), retryDelay: () => 0, progressFrame: queueMicrotask },
  });
  await source.ready();
  assert.equal(source.keyed, false);
  assert.deepEqual(source.progress, NO_PROGRESS, "no client yet: nothing to count");
  let heard = 0;
  const off = source.onProgress(() => heard++);

  acct.grant();
  await until(() => source.keyed);
  assert.equal(heard, 1, "told once the client is there, though nothing changed on it yet");
  const places = await source.places({ kind: "index" });
  const follow = source.follow({ kind: "index" }, places, { at: 0, from: null }, { onState: () => undefined })!;
  await until(() => source.progress.catchingUp.index === 1);
  await until(() => heard > 1);
  // A listener added now goes straight onto the client.
  let late = 0;
  const offLate = source.onProgress(() => late++);
  const writer = await StreamWriter.open(machine, await indexStream(machine.mk), FMT_INDEX_V1);
  await writer.append({ upserts: [row("a")], removed: [] });
  await catchUpOn(link, writer);
  await until(() => source.progress.synced.index === 1 && late > 0);

  off();
  offLate();
  const [was, wasLate] = [heard, late];
  follow.close();
  await flush(5);
  assert.deepEqual([heard, late], [was, wasLate], "a listener taken off hears nothing more");
  assert.deepEqual(source.progress.synced, only("index", 0));
  source.close();
});
