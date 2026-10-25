// Reading a machine's encrypted streams (`lib/hub/streams.ts`): the three folds
// (index, status, a session's replace-from transcript), and a subscription
// against a hub the test plays — records released only under a signed head,
// manifests and their pages, the live-checkpoint rule, and what happens to a
// copy that stops checking out (a forged head, a tampered record, a gap).
import assert from "node:assert/strict";
import test from "node:test";
import {
  decodeSignedHead,
  encodeFrame,
  type Frame,
  FMT_FEED_V1,
  FMT_INDEX_V1,
  FMT_STATUS_V1,
  generateDeviceKeys,
  hex,
  indexStream,
  statusStream,
  streamId,
} from "../src/lib/e2e/index.js";
import { HubLink } from "../src/lib/hub/link.js";
import {
  copyWindow,
  type FeedCopy,
  feedFold,
  FoldError,
  indexFold,
  type IndexState,
  Limiter,
  statusFold,
  StreamClient,
  type StreamEvents,
} from "../src/lib/hub/streams.js";
import { catchUp, FakeStreamLink, flush, makeMachine, type Machine, StreamWriter, until } from "./stream-fixtures.js";

const row = (id: string, updatedAt = "2026-10-15T00:00:00.000Z", extra: Record<string, unknown> = {}) => ({
  source: "claude",
  id,
  title: `title ${id}`,
  updatedAt,
  ...extra,
});
const item = (n: number) => ({ kind: "user", text: `item ${n}` });
const items = (from: number, to: number) => Array.from({ length: to - from }, (_, i) => item(from + i));

test("the index fold: reset clears, removed goes, upserts land by source/id", () => {
  let s = indexFold.empty();
  s = indexFold.apply(s, { upserts: [row("a"), row("b")], removed: [], asOf: 1 });
  assert.deepEqual(Object.keys(s.rows).sort(), ["claude/a", "claude/b"]);
  const before = s;
  s = indexFold.apply(s, { upserts: [row("c")], removed: ["claude/a"], asOf: 2 });
  assert.deepEqual(Object.keys(s.rows).sort(), ["claude/b", "claude/c"]);
  assert.equal(s.asOf, 2);
  assert.deepEqual(Object.keys(before.rows).sort(), ["claude/a", "claude/b"], "the fold never mutates the state it is given");
  s = indexFold.apply(s, { reset: true, upserts: [row("d")], removed: [], asOf: 3 });
  assert.deepEqual(Object.keys(s.rows), ["claude/d"], "a reset starts the list over");
  // A body removes before it upserts: a row removed and re-added in one body stays.
  s = indexFold.apply(s, { upserts: [row("d", "2026-10-16T00:00:00.000Z")], removed: ["claude/d"] });
  assert.equal(s.rows["claude/d"]?.updatedAt, "2026-10-16T00:00:00.000Z");
  assert.throws(() => indexFold.apply(s, "nope"), FoldError);
});

test("the status fold: each body replaces the last, and every field is optional", () => {
  let s = statusFold.apply(statusFold.empty(), { asOf: 5, settings: { hostname: "mac" }, accounts: null, quota: { claude: {} }, cli: { claude: "2.1", codex: null } });
  assert.equal(s.settings?.hostname, "mac");
  assert.deepEqual(s.cli, { claude: "2.1", codex: null });
  assert.equal(s.mkEpoch, null, "an agent that does not say its key epoch");
  s = statusFold.apply(s, { asOf: 6 });
  assert.deepEqual(s, { asOf: 6, settings: null, accounts: null, quota: null, cli: null, mkEpoch: null });
});

test("the status fold carries the key epoch the agent seals under, and nothing that is not one", () => {
  const fold = (mkEpoch: unknown) => statusFold.apply(statusFold.empty(), { asOf: 1, mkEpoch }).mkEpoch;
  assert.equal(statusFold.empty().mkEpoch, null);
  assert.equal(fold(0), 0);
  assert.equal(fold(3), 3);
  assert.equal(fold(null), null);
  assert.equal(fold(undefined), null);
  assert.equal(fold(-1), null);
  assert.equal(fold(1.5), null);
  assert.equal(fold("2"), null);
  assert.equal(fold(2 ** 32), null);
  // Each body replaces the last: a later body that does not say forgets it.
  const s = statusFold.apply(statusFold.apply(statusFold.empty(), { asOf: 1, mkEpoch: 2 }), { asOf: 2 });
  assert.equal(s.mkEpoch, null);
});

test("the status fold carries the machine's system and its agent's version (S1-5, #108), defensively: an older agent says neither", () => {
  const fold = (body: Record<string, unknown>) => statusFold.apply(statusFold.empty(), { asOf: 1, ...body });
  const s = fold({ os: { name: "linux", version: "24.04", distro: "ubuntu", arch: "x86_64", more: 1 }, agentVersion: " 0.2.131 " });
  assert.deepEqual(s.os, { name: "linux", version: "24.04", distro: "ubuntu", arch: "x86_64" }, "known fields only");
  assert.equal(s.agentVersion, "0.2.131");
  assert.deepEqual(fold({ os: { name: "macos", version: "15.3.1", arch: "aarch64" } }).os, { name: "macos", version: "15.3.1", distro: null, arch: "aarch64" });
  assert.deepEqual(fold({ os: { name: "windows", version: null, arch: 64 } }).os, { name: "windows", version: null, distro: null, arch: null }, "what is not text is not known");
  assert.equal(fold({ os: { name: "x".repeat(500) } }).os?.name.length, 64, "cut to size");
  // An agent from before them: neither field, so the settings stand in.
  const old = fold({ settings: { platform: "darwin", serverVersion: "0.2.120" } });
  assert.ok(!("os" in old) && !("agentVersion" in old));
  for (const bad of [null, "macos", [], {}, { name: "" }, { name: "  " }, { name: 3 }, { version: "15.1", arch: "arm64" }]) {
    assert.ok(!("os" in fold({ os: bad })), JSON.stringify(bad));
  }
  for (const bad of [null, "", "   ", 131, {}, ["0.2.131"]]) {
    assert.ok(!("agentVersion" in fold({ agentVersion: bad })), JSON.stringify(bad));
  }
  // Each body replaces the last: one without them forgets them.
  const later = statusFold.apply(s, { asOf: 2 });
  assert.ok(!("os" in later) && !("agentVersion" in later));
});

test("the status fold carries who opened a channel to the machine (review D3), strictly: anything malformed is no record", () => {
  const record = { from: 2, fromMs: 1_000, seq: 7, devices: [{ id: "d-1", firstMs: 1_500 }, { id: "d-2", firstMs: 1_200 }] };
  const fold = (controllers: unknown) => statusFold.apply(statusFold.empty(), { asOf: 1, controllers }).controllers;
  assert.deepEqual(fold(record), record);
  assert.deepEqual(fold({ ...record, devices: [] }), { ...record, devices: [] }, "nobody yet is a record too");
  assert.deepEqual(fold({ ...record, extra: true, devices: [{ id: "d-1", firstMs: 3, more: 1 }] }), { ...record, devices: [{ id: "d-1", firstMs: 3 }] }, "known fields only");
  assert.equal(statusFold.apply(statusFold.empty(), { asOf: 1 }).controllers, undefined, "an agent that does not keep one");
  assert.ok(!("controllers" in statusFold.apply(statusFold.empty(), { asOf: 1, controllers: null })), "nor a field that is null");
  for (const bad of [
    "x",
    [],
    { ...record, from: -1 },
    { ...record, from: 8 },
    { ...record, seq: 1.5 },
    { ...record, seq: "7" },
    { ...record, fromMs: undefined },
    { ...record, devices: "d-1" },
    { ...record, devices: ["d-1"] },
    { ...record, devices: [{ id: "d-1" }] },
    { ...record, devices: [{ id: "", firstMs: 1 }] },
    { ...record, devices: [{ id: "d-1", firstMs: -1 }] },
    { ...record, devices: Array.from({ length: 1025 }, (_, i) => ({ id: `d-${i}`, firstMs: 1 })) },
    { from: 2, fromMs: 1, seq: 7, ids: ["d-1"] },
  ]) {
    assert.equal(fold(bad), undefined, JSON.stringify(bad).slice(0, 80));
  }
  // Each body replaces the last: one without the record forgets it.
  const s = statusFold.apply(statusFold.apply(statusFold.empty(), { asOf: 1, controllers: record }), { asOf: 2 });
  assert.equal(s.controllers, undefined);
});

test("the session fold: the first body sets the start, then replace-from; a gap is refused", () => {
  let s: FeedCopy = feedFold.empty();
  s = feedFold.apply(s, { fromIndex: 0, items: items(0, 5), endIndex: 5, fp: "f5", prefixFp: "p2" });
  assert.deepEqual(s, { started: true, startIndex: 0, endIndex: 5, items: items(0, 5), fp: "f5", prefixFp: "p2" });
  // Replace from 3: items 3 and 4 rewritten, and two more.
  s = feedFold.apply(s, { fromIndex: 3, items: [{ kind: "user", text: "new 3" }, ...items(4, 7)], endIndex: 7, fp: "f7", prefixFp: "p4" });
  assert.equal(s.items.length, 7);
  assert.deepEqual(s.items[3], { kind: "user", text: "new 3" });
  assert.equal(s.endIndex, 7);
  assert.equal(s.prefixFp, "p4");
  // A body that ends the transcript earlier (the file shrank) truncates it.
  s = feedFold.apply(s, { fromIndex: 6, items: [], endIndex: 6, fp: "f6", prefixFp: "p3" });
  assert.equal(s.items.length, 6);
  assert.throws(() => feedFold.apply(s, { fromIndex: 9, items: [item(9)], endIndex: 10 }), (e: unknown) => e instanceof FoldError && e.kind === "gap");
  assert.throws(() => feedFold.apply(s, { fromIndex: 2, items: [item(2)], endIndex: 5 }), (e: unknown) => e instanceof FoldError && e.kind === "malformed");
  // A copy that starts later than 0 (the first body said so) keeps its start.
  let late = feedFold.apply(feedFold.empty(), { fromIndex: 10, items: items(10, 12), endIndex: 12 });
  assert.equal(late.startIndex, 10);
  late = feedFold.apply(late, { fromIndex: 11, items: items(11, 14), endIndex: 14 });
  assert.equal(late.items.length, 4);
  late = feedFold.apply(late, { fromIndex: 4, items: items(4, 6), endIndex: 6 });
  assert.deepEqual([late.startIndex, late.endIndex, late.items.length], [4, 6, 2], "a body from before the start replaces the copy");
});

test("the index fold takes the agent's daily {asOf} as the time and nothing else", () => {
  let s = indexFold.apply(indexFold.empty(), { upserts: [row("a"), row("b")], removed: [], asOf: 1 });
  const rows = s.rows;
  s = indexFold.apply(s, { asOf: 86_400_001 });
  assert.equal(s.asOf, 86_400_001);
  assert.deepEqual(s.rows, rows, "no row moves");
});

// relay-and-plans §3.4 (window trimming): a copy the retention window
// trimmed starts with a window-start page — an empty replace-from at `base`,
// plus `base` and `before`.
const windowPage = (base: number, before: string | null = "2026-10-01T08:00:00.000Z") => ({ fromIndex: base, items: [], endIndex: base, fp: `f${base}`, prefixFp: `p${base - 3}`, base, before });

test("the session fold: a window-start page says where a trimmed copy starts and what it leaves on the machine", () => {
  let s: FeedCopy = feedFold.apply(feedFold.empty(), windowPage(14));
  assert.deepEqual([s.started, s.startIndex, s.endIndex, s.items.length], [true, 14, 14, 0]);
  assert.deepEqual(copyWindow(s), { base: 14, before: "2026-10-01T08:00:00.000Z" });
  // The kept pages, then records after them: the window holds.
  s = feedFold.apply(s, { fromIndex: 14, items: items(14, 21), endIndex: 21, fp: "f21", prefixFp: "p18" });
  s = feedFold.apply(s, { fromIndex: 21, items: items(21, 28), endIndex: 28, fp: "f28", prefixFp: "p25" });
  s = feedFold.apply(s, { fromIndex: 27, items: [{ kind: "user", text: "rewritten 27" }, ...items(28, 30)], endIndex: 30, fp: "f30", prefixFp: "p27" });
  assert.deepEqual([s.startIndex, s.endIndex, s.items.length, s.base, s.before], [14, 30, 16, 14, "2026-10-01T08:00:00.000Z"]);
  assert.deepEqual(s.items[0], item(14));
  assert.deepEqual(copyWindow(s), { base: 14, before: "2026-10-01T08:00:00.000Z" });
  // A body from before the window (a copy holding more again) drops it.
  const wider = feedFold.apply(s, { fromIndex: 3, items: items(3, 5), endIndex: 5 });
  assert.equal(copyWindow(wider), null);
  assert.equal(wider.base, undefined);
  // `before` is optional (no entry left out was dated).
  assert.deepEqual(copyWindow(feedFold.apply(feedFold.empty(), windowPage(5, null))), { base: 5, before: null });
  // A copy that starts late for the item cap (no window page) is not "trimmed".
  assert.equal(copyWindow(feedFold.apply(feedFold.empty(), { fromIndex: 10, items: items(10, 12), endIndex: 12 })), null);
  assert.equal(copyWindow(feedFold.empty()), null);
  assert.equal(copyWindow(null), null);
});

test("a reader from before window pages folds a trimmed copy as one that simply starts later", () => {
  // What v0.2.100 did with the same bodies: `base`/`before` were unknown fields.
  const { base: _b, before: _w, ...plain } = windowPage(14);
  let old = feedFold.apply(feedFold.empty(), plain);
  old = feedFold.apply(old, { fromIndex: 14, items: items(14, 21), endIndex: 21 });
  let now = feedFold.apply(feedFold.empty(), windowPage(14));
  now = feedFold.apply(now, { fromIndex: 14, items: items(14, 21), endIndex: 21 });
  assert.deepEqual([old.startIndex, old.endIndex, old.items], [now.startIndex, now.endIndex, now.items]);
});

interface Seen<S> {
  states: { state: S; reset: boolean; seq: number; freshestMs: number | null }[];
  synced: boolean[];
  errors: unknown[];
}

function watcher<S>(): { events: StreamEvents<S>; seen: Seen<S> } {
  const seen: Seen<S> = { states: [], synced: [], errors: [] };
  return {
    seen,
    events: {
      onState: (state, change) => seen.states.push({ state, reset: change.reset, seq: change.seq, freshestMs: change.freshestMs }),
      onSynced: (exists) => seen.synced.push(exists),
      onError: (error) => seen.errors.push(error),
    },
  };
}

function client(machine: Machine, link: FakeStreamLink, pages = new Map<string, Uint8Array>(), fetched: string[] = []) {
  return new StreamClient({
    link,
    machineSignKey: machine.signPublic,
    epochs: machine.epochs,
    fetchPage: async (_stream, page) => {
      fetched.push(hex(page));
      const got = pages.get(hex(page));
      if (!got) throw new Error("404");
      return got;
    },
    retryDelay: () => 0,
  });
}

test("a subscription shows only what a signed head released, and says when it has caught up", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const stream = await indexStream(machine.mk);
  const writer = await StreamWriter.open(machine, stream, FMT_INDEX_V1);
  await writer.append({ upserts: [row("a")], removed: [], asOf: 1 });
  await writer.append({ upserts: [row("b")], removed: [], asOf: 2 });
  const coveredAt = { nextSeq: writer.seq, link: writer.link };
  await writer.append({ upserts: [row("evil")], removed: [], asOf: 3 });

  const { events, seen } = watcher<IndexState>();
  const sub = client(machine, link).subscribe(stream, indexFold, events);
  await until(() => link.subs().length === 1);
  const first = link.lastSub();
  assert.equal(first.fromSeq, 0);
  assert.equal(hex(first.stream), hex(stream));

  link.deliver({ type: "recs", ch: first.ch, stream, records: writer.records });
  await flush(5);
  assert.equal(seen.states.length, 0, "records alone release nothing");

  // The machine signed the first two only: the third, though it decrypts and
  // chains, is never shown.
  link.deliver({ type: "head", ch: first.ch, head: await writer.head(1_000, coveredAt) });
  link.control({ t: "stream_synced", ch: first.ch, stream: hex(stream), next_seq: 3, exists: true });
  await until(() => seen.synced.length === 1);
  assert.equal(seen.states.length, 1);
  assert.deepEqual(Object.keys(seen.states[0]!.state.rows).sort(), ["claude/a", "claude/b"]);
  assert.equal(seen.states[0]!.reset, true, "the first state of a fresh reader replaces whatever was shown");
  assert.equal(seen.states[0]!.freshestMs, 1_000);
  assert.equal(sub.confirmed, 2);
  assert.deepEqual(seen.synced, [true]);

  // The head that covers the third arrives: now it is released.
  link.deliver({ type: "head", ch: first.ch, head: await writer.head(2_000) });
  await until(() => seen.states.length === 2);
  assert.ok(seen.states[1]!.state.rows["claude/evil"]);
  assert.equal(seen.states[1]!.reset, false);

  // A heartbeat (same position, later time) is news about freshness only.
  link.deliver({ type: "head", ch: first.ch, head: await writer.head(3_000) });
  await until(() => seen.states.length === 3);
  assert.equal(seen.states[2]!.freshestMs, 3_000);
  sub.close();
  assert.deepEqual(link.controls.at(-1), { t: "stream_unsub", ch: first.ch });
  assert.equal(link.routes.size, 0, "its ch is released");
});

test("a head the machine did not sign, or a tampered record, is refused and the stream starts over from nothing", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const stream = await statusStream(machine.mk);
  const writer = await StreamWriter.open(machine, stream, FMT_STATUS_V1);
  await writer.append({ asOf: 1, settings: { hostname: "real" } });
  const { events, seen } = watcher<{ settings: Record<string, unknown> | null }>();
  client(machine, link).subscribe(stream, statusFold, events);
  await until(() => link.subs().length === 1);
  const forger = await generateDeviceKeys();

  link.deliver({ type: "recs", ch: link.lastSub().ch, stream, records: writer.records });
  link.deliver({ type: "head", ch: link.lastSub().ch, head: await writer.head(1, undefined, forger.sign.privateKey) });
  await until(() => link.subs().length === 2);
  assert.equal(seen.states.length, 0, "nothing under a forged head is shown");
  assert.equal(link.lastSub().fromSeq, 0, "it subscribes again from the start");
  assert.notEqual(link.lastSub().ch, link.subs()[0]!.ch, "on a ch never used before");
  assert.equal(seen.errors.length, 1);

  // Tamper with the record's ciphertext: it no longer opens.
  const tampered = { ...writer.records[0]!, ct: writer.records[0]!.ct.slice() };
  tampered.ct[0]! ^= 1;
  link.deliver({ type: "recs", ch: link.lastSub().ch, stream, records: [tampered] });
  await until(() => link.subs().length === 3);
  assert.equal(seen.states.length, 0);

  // The honest catch-up goes through.
  await catchUp(link, writer);
  await until(() => seen.states.length === 1);
  assert.equal(seen.states[0]!.state.settings?.hostname, "real");
});

test("a checkpoint is a manifest the machine signed: its pages, checked against it, then the records after it", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const stream = await streamId(machine.mk, "claude", "s-1");
  const writer = await StreamWriter.open(machine, stream, FMT_FEED_V1);
  // Four records, compacted into two pages at seq 4, then two more records.
  for (const [from, to] of [[0, 2], [2, 4], [4, 6], [6, 8]] as const) await writer.append({ fromIndex: from, items: items(from, to), endIndex: to, fp: `f${to}`, prefixFp: `p${to - 3}` });
  const cp = await writer.manifest([
    { fromIndex: 0, items: items(0, 4), endIndex: 4 },
    { fromIndex: 4, items: items(4, 8), endIndex: 8, fp: "f8", prefixFp: "p5" },
  ]);
  await writer.append({ fromIndex: 8, items: items(8, 9), endIndex: 9, fp: "f9", prefixFp: "p6" });
  await writer.append({ fromIndex: 7, items: [{ kind: "user", text: "rewritten 7" }, ...items(8, 10)], endIndex: 10, fp: "f10", prefixFp: "p7" });

  const fetched: string[] = [];
  const pages = new Map(cp.pages);
  const { events, seen } = watcher<FeedCopy>();
  client(machine, link, pages, fetched).subscribe(stream, feedFold, events);
  await until(() => link.subs().length === 1);
  await catchUp(link, writer, { manifest: cp });
  await until(() => seen.synced.length === 1 && seen.states.length === 2);
  assert.equal(fetched.length, 2, "both pages fetched");
  assert.equal(fetched[0], [...cp.pages.keys()][1], "the last page first");
  assert.equal(seen.states[0]!.reset, true);
  assert.equal(seen.states[0]!.seq, 4);
  assert.equal(seen.states[0]!.state.items.length, 8, "the pages, in order, are the state at the manifest");
  const final = seen.states[1]!.state;
  assert.deepEqual([final.startIndex, final.endIndex, final.items.length, final.prefixFp], [0, 10, 10, "p7"]);
  assert.deepEqual(final.items[7], { kind: "user", text: "rewritten 7" });
});

test("a trimmed copy's manifest starts with its window page: a fresh reader holds the window, and the records after it", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const stream = await streamId(machine.mk, "claude", "s-window");
  const writer = await StreamWriter.open(machine, stream, FMT_FEED_V1);
  for (const [from, to] of [[0, 14], [14, 21]] as const) await writer.append({ fromIndex: from, items: items(from, to), endIndex: to });
  // The agent's checkpoint under a 7-day plan: the window page, then the page kept.
  const cp = await writer.manifest([windowPage(14), { fromIndex: 14, items: items(14, 21), endIndex: 21, fp: "f21", prefixFp: "p18" }]);
  await writer.append({ fromIndex: 21, items: items(21, 23), endIndex: 23, fp: "f23", prefixFp: "p20" });
  const { events, seen } = watcher<FeedCopy>();
  client(machine, link, new Map(cp.pages)).subscribe(stream, feedFold, events);
  await until(() => link.subs().length === 1);
  await catchUp(link, writer, { manifest: cp });
  await until(() => seen.synced.length === 1 && seen.states.length === 2);
  assert.equal(seen.states[0]!.reset, true);
  const final = seen.states[1]!.state;
  assert.deepEqual([final.startIndex, final.endIndex, final.items.length], [14, 23, 9]);
  assert.deepEqual(copyWindow(final), { base: 14, before: "2026-10-01T08:00:00.000Z" });
});

test("a page that is not the one the manifest names, or a manifest the machine did not sign, is never used", async () => {
  const machine = await makeMachine();
  const stream = await indexStream(machine.mk);
  const writer = await StreamWriter.open(machine, stream, FMT_INDEX_V1);
  await writer.append({ upserts: [row("a")], removed: [] });
  const honest = await writer.manifest([{ upserts: [row("a")], removed: [] }]);

  // The hub swaps the page for another sealed under the same stream key.
  {
    const link = new FakeStreamLink();
    const other = await writer.manifest([{ upserts: [row("swapped")], removed: [] }]);
    const swapped = new Map([[[...honest.pages.keys()][0]!, [...other.pages.values()][0]!]]);
    const { events, seen } = watcher<IndexState>();
    client(machine, link, swapped).subscribe(stream, indexFold, events);
    await until(() => link.subs().length === 1);
    await catchUp(link, writer, { manifest: honest });
    await until(() => seen.errors.length >= 1 && link.subs().length >= 2);
    assert.equal(seen.states.length, 0, "a page whose hash differs is refused");
  }
  // A manifest signed by anyone but the machine.
  {
    const link = new FakeStreamLink();
    const forger = await generateDeviceKeys();
    const forged = await writer.manifest([{ upserts: [row("forged")], removed: [] }], 1, forger.sign.privateKey);
    const fetched: string[] = [];
    const { events, seen } = watcher<IndexState>();
    client(machine, link, forged.pages, fetched).subscribe(stream, indexFold, events);
    await until(() => link.subs().length === 1);
    await catchUp(link, writer, { manifest: forged });
    await until(() => seen.errors.length >= 1 && link.subs().length >= 2);
    assert.equal(seen.states.length, 0, "a forged manifest is refused");
    assert.equal(fetched.length, 0, "before a single page is fetched");
  }
});

test("a manifest carries no format: each page says its own, and a page of another kind of stream is refused", async () => {
  const machine = await makeMachine();
  const stream = await indexStream(machine.mk);
  // Pages sealed under the index stream's key, but of the status format.
  const writer = await StreamWriter.open(machine, stream, FMT_STATUS_V1);
  await writer.append({ asOf: 1 });
  const cp = await writer.manifest([{ upserts: [row("a")], removed: [] }]);
  const link = new FakeStreamLink();
  const { events, seen } = watcher<IndexState>();
  client(machine, link, cp.pages).subscribe(stream, indexFold, events);
  await until(() => link.subs().length === 1);
  await catchUp(link, writer, { manifest: cp });
  await until(() => seen.errors.length >= 1 && link.subs().length >= 2);
  assert.equal(seen.states.length, 0, "a status page is not an index page");
  assert.match(String(seen.errors[0]), /format/);
});

test("a live checkpoint this copy is already past, or standing on, is ignored; one ahead of it rebuilds and resubscribes", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const stream = await indexStream(machine.mk);
  const writer = await StreamWriter.open(machine, stream, FMT_INDEX_V1);
  await writer.append({ upserts: [row("a")], removed: [] });
  await writer.append({ upserts: [row("b")], removed: [] });
  const old = await writer.manifest([{ upserts: [row("a"), row("b")], removed: [] }]);
  await writer.append({ upserts: [row("c")], removed: [] });
  const fetched: string[] = [];
  const pages = new Map(old.pages);
  const { events, seen } = watcher<IndexState>();
  client(machine, link, pages, fetched).subscribe(stream, indexFold, events);
  await until(() => link.subs().length === 1);
  const ch = await catchUp(link, writer);
  await until(() => seen.synced.length === 1);
  assert.equal(seen.states.length, 1);

  // Past it (confirmed 3 > 2): ignored.
  link.deliver({ type: "checkpoint", ch, stream, uptoSeq: old.upto, manifest: old.manifest });
  // Standing on it (3, same link): ignored too.
  const here = await writer.manifest([{ upserts: [row("a"), row("b"), row("c")], removed: [] }]);
  for (const [k, v] of here.pages) pages.set(k, v);
  link.deliver({ type: "checkpoint", ch, stream, uptoSeq: here.upto, manifest: here.manifest });
  await flush(10);
  assert.equal(fetched.length, 0, "nothing fetched for a compaction this copy is consistent with");
  assert.equal(link.subs().length, 1);

  // Ahead of it: the writer moved on and compacted; this copy missed the records.
  await writer.append({ upserts: [row("d")], removed: [] });
  await writer.append({ upserts: [], removed: ["claude/a"] });
  const ahead = await writer.manifest([{ upserts: [row("b"), row("c"), row("d")], removed: [] }]);
  for (const [k, v] of ahead.pages) pages.set(k, v);
  link.deliver({ type: "checkpoint", ch, stream, uptoSeq: ahead.upto, manifest: ahead.manifest });
  await until(() => seen.states.length === 2 && link.subs().length === 2);
  assert.equal(seen.states[1]!.reset, true);
  assert.deepEqual(Object.keys(seen.states[1]!.state.rows).sort(), ["claude/b", "claude/c", "claude/d"]);
  assert.equal(link.lastSub().fromSeq, 5, "and asks again from the manifest's seq");
});

test("a reader that has confirmed nothing yet rebuilds from a manifest at 0, even one carrying the empty stream's link", async () => {
  // A stream subscribed to before it existed, then backfilled straight to pages.
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const stream = await indexStream(machine.mk);
  const writer = await StreamWriter.open(machine, stream, FMT_INDEX_V1);
  const backfill = await writer.manifest([{ reset: true, upserts: [row("a"), row("b")], removed: [] }]);
  const { events, seen } = watcher<IndexState>();
  client(machine, link, backfill.pages).subscribe(stream, indexFold, events);
  await until(() => link.subs().length === 1);
  const ch = link.lastSub().ch;
  link.control({ t: "stream_synced", ch, stream: hex(stream), next_seq: 0, exists: false });
  await until(() => seen.synced.length === 1);
  // Live: the manifest at seq 0, whose link is the empty stream's (all zeros).
  link.deliver({ type: "checkpoint", ch, stream, uptoSeq: 0, manifest: backfill.manifest });
  await until(() => seen.states.length === 1);
  assert.deepEqual(Object.keys(seen.states[0]!.state.rows).sort(), ["claude/a", "claude/b"]);
});

test("a saved copy restarts from its signed head; a forged one is dropped for a fresh start", async () => {
  const machine = await makeMachine();
  const stream = await indexStream(machine.mk);
  const writer = await StreamWriter.open(machine, stream, FMT_INDEX_V1);
  await writer.append({ upserts: [row("a")], removed: [] });
  await writer.append({ upserts: [row("b")], removed: [] });
  const savedHead = decodeSignedHead(await writer.head(10));
  const savedState: IndexState = { rows: { "claude/a": row("a"), "claude/b": row("b") } as unknown as IndexState["rows"], asOf: null };
  await writer.append({ upserts: [row("c")], removed: [] });

  {
    const link = new FakeStreamLink();
    const { events, seen } = watcher<IndexState>();
    client(machine, link).subscribe(stream, indexFold, events, { head: savedHead, state: savedState });
    await until(() => link.subs().length === 1);
    assert.equal(link.lastSub().fromSeq, 2, "from the saved position");
    await catchUp(link, writer);
    await until(() => seen.states.length === 1);
    assert.equal(seen.states[0]!.reset, false, "an update of the saved copy");
    assert.deepEqual(Object.keys(seen.states[0]!.state.rows).sort(), ["claude/a", "claude/b", "claude/c"]);
  }
  {
    const link = new FakeStreamLink();
    const forger = await generateDeviceKeys();
    const forgedHead = decodeSignedHead(await writer.head(10, undefined, forger.sign.privateKey));
    client(machine, link).subscribe(stream, indexFold, {}, { head: forgedHead, state: savedState });
    await until(() => link.subs().length === 1);
    assert.equal(link.lastSub().fromSeq, 0, "a saved head the machine did not sign is not a starting point");
  }
});

test("a gap in a session's items breaks the copy; a head ahead of its records resubscribes from the confirmed seq", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const stream = await streamId(machine.mk, "codex", "s-2");
  const writer = await StreamWriter.open(machine, stream, FMT_FEED_V1);
  await writer.append({ fromIndex: 0, items: items(0, 2), endIndex: 2 });
  const { events, seen } = watcher<FeedCopy>();
  client(machine, link).subscribe(stream, feedFold, events);
  await until(() => link.subs().length === 1);
  const ch = await catchUp(link, writer);
  await until(() => seen.states.length === 1);

  // The head arrives before the record it covers: rolled back, asked again from 1.
  await writer.append({ fromIndex: 2, items: items(2, 3), endIndex: 3 });
  link.deliver({ type: "head", ch, head: await writer.head() });
  await until(() => link.subs().length === 2);
  assert.equal(link.lastSub().fromSeq, 1);
  await catchUp(link, writer);
  await until(() => seen.states.length === 2);
  assert.equal(seen.states[1]!.state.endIndex, 3);

  // A body past the end of what is held: the copy is rebuilt from nothing.
  await writer.append({ fromIndex: 9, items: items(9, 10), endIndex: 10 });
  const live = link.lastSub().ch;
  link.deliver({ type: "recs", ch: live, stream, records: writer.records.slice(2) });
  link.deliver({ type: "head", ch: live, head: await writer.head() });
  await until(() => link.subs().length === 3);
  assert.equal(link.lastSub().fromSeq, 0);
  assert.ok(seen.errors.some((e) => e instanceof FoldError && e.kind === "gap"));
  assert.equal(seen.states.length, 2, "and the broken state is never handed out");
});

test("the link dropping and coming back resubscribes from where the copy stands", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const stream = await indexStream(machine.mk);
  const writer = await StreamWriter.open(machine, stream, FMT_INDEX_V1);
  await writer.append({ upserts: [row("a")], removed: [] });
  const { events, seen } = watcher<IndexState>();
  client(machine, link).subscribe(stream, indexFold, events);
  await until(() => link.subs().length === 1);
  await catchUp(link, writer);
  await until(() => seen.states.length === 1);
  // A record the page took but no head released when the link went.
  await writer.append({ upserts: [row("b")], removed: [] });
  link.deliver({ type: "recs", ch: link.lastSub().ch, stream, records: writer.records.slice(1) });
  link.drop();
  await flush(5);
  link.reopen();
  await until(() => link.subs().length === 2);
  assert.equal(link.lastSub().fromSeq, 1);
  await catchUp(link, writer);
  await until(() => seen.states.length === 2);
  assert.deepEqual(Object.keys(seen.states[1]!.state.rows).sort(), ["claude/a", "claude/b"], "the record is taken once, from the resend");
  const synced = link.controls.filter((c) => c.t === "stream_unsub");
  assert.equal(synced.length, 0, "a ch lost with the link is not unsubscribed");
});

test("the hub link routes stream frames by ch and sends stream controls as text", async () => {
  const sockets: { sent: (string | Uint8Array)[]; onmessage: ((ev: { data: unknown }) => void) | null; onclose: ((ev: { code?: number }) => void) | null }[] = [];
  const keys = await generateDeviceKeys();
  const link = new HubLink({
    url: "wss://hub.test/v1/client",
    origin: "wss://hub.test",
    account: "a-1",
    deviceId: "d-1",
    token: "tok",
    signKey: keys.sign.privateKey,
    makeSocket: () => {
      const s = {
        binaryType: "blob",
        readyState: 1,
        sent: [] as (string | Uint8Array)[],
        send(data: string | ArrayBufferLike | ArrayBufferView) {
          s.sent.push(typeof data === "string" ? data : new Uint8Array(data as ArrayBuffer));
        },
        close() {},
        onopen: null,
        onmessage: null as ((ev: { data: unknown }) => void) | null,
        onclose: null as ((ev: { code?: number }) => void) | null,
        onerror: null,
      };
      sockets.push(s);
      return s;
    },
  });
  link.start();
  const socket = sockets[0]!;
  socket.onmessage?.({ data: JSON.stringify({ t: "welcome" }) });
  assert.equal(link.state, "open");
  const got: Frame[] = [];
  const ch = link.reserve((f) => got.push(f));
  const other = link.reserve((f) => assert.equal(f.type, "close", "another ch hears nothing but the link's close"));
  const stream = new Uint8Array(32).fill(7);
  const deliver = (f: Frame) => socket.onmessage?.({ data: encodeFrame(f).slice().buffer });
  deliver({ type: "recs", ch, stream, records: [] });
  deliver({ type: "head", ch, head: new Uint8Array(4) });
  deliver({ type: "gone", ch, stream, firstSeq: 3 });
  deliver({ type: "checkpoint", ch, stream, uptoSeq: 3, manifest: new Uint8Array(2) });
  deliver({ type: "ephemeral", ch, stream, payload: new Uint8Array(1) });
  deliver({ type: "recs", ch: 999, stream, records: [] });
  assert.deepEqual(got.map((f) => f.type), ["recs", "head", "gone", "checkpoint", "ephemeral"]);
  const controls: Record<string, unknown>[] = [];
  link.onControl((m) => controls.push(m));
  socket.onmessage?.({ data: JSON.stringify({ t: "stream_synced", ch, stream: hex(stream), next_seq: 3, exists: true }) });
  assert.equal(controls[0]?.t, "stream_synced");
  assert.equal(link.sendControl({ t: "stream_unsub", ch }), true);
  assert.deepEqual(JSON.parse(socket.sent.at(-1) as string), { t: "stream_unsub", ch });
  void other;
  link.stop();
  assert.equal(got.at(-1)?.type, "close", "a lost link closes every route, streams included");
  assert.equal(link.sendControl({ t: "x" }), false);
});

test("the limiter runs at most n at once, in order", async () => {
  const limiter = new Limiter(2);
  let running = 0;
  let peak = 0;
  const order: number[] = [];
  await Promise.all(
    [0, 1, 2, 3, 4].map((i) =>
      limiter.run(async () => {
        running++;
        peak = Math.max(peak, running);
        order.push(i);
        await flush(2);
        running--;
      }),
    ),
  );
  assert.equal(peak, 2);
  assert.deepEqual(order, [0, 1, 2, 3, 4]);
});

test("a refused subscription keeps its copy and asks again later, from where it stands", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const stream = await indexStream(machine.mk);
  const writer = await StreamWriter.open(machine, stream, FMT_INDEX_V1);
  await writer.append({ upserts: [row("a")], removed: [] });
  const { events, seen } = watcher<IndexState>();
  client(machine, link).subscribe(stream, indexFold, events);
  await until(() => link.subs().length === 1);
  await catchUp(link, writer);
  await until(() => seen.states.length === 1);
  link.control({ t: "stream_refused", ch: link.lastSub().ch, stream: hex(stream), code: "rate", retry_ms: 20 });
  await until(() => link.subs().length === 2);
  assert.equal(link.lastSub().fromSeq, 1, "from the confirmed seq: nothing about the copy was wrong");
  assert.equal(seen.states.length, 1, "and what it showed stays");
  assert.match(String(seen.errors[0]), /rate/);
});
