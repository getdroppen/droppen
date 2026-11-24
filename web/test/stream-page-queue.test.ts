// A checkpoint download through the page's queue (`StreamClient.pages`,
// `lib/hub/page-queue.ts`): a subscription closed mid-way fetches nothing
// more of it, and its fetches in flight are aborted; a page that fails ends
// the download, and the stream starts over only once none of the old
// download's fetches is still running; a session somebody waits on goes
// ahead of every machine's background, which asks the browser for low
// priority; a page that stalls fails its download into the stream's backoff;
// a machine's index and status follow the lane they are given; and each
// download says in the connection log how it went.
import assert from "node:assert/strict";
import test from "node:test";
import { connLog, maskId } from "../src/lib/connection-log.js";
import { type Bytes, FMT_FEED_V1, hex, streamId } from "../src/lib/e2e/index.js";
import { PageQueue } from "../src/lib/hub/page-queue.js";
import { HubCopySource } from "../src/lib/hub/copy-source.js";
import { feedFold, type PageFetch, StreamClient, type StreamClientOptions, type StreamSubscription } from "../src/lib/hub/streams.js";
import { FakeStreamLink, flush, type Machine, makeMachine, StreamWriter, until } from "./stream-fixtures.js";

const item = (n: number) => ({ kind: "user", text: `item ${n}` });
const page = (from: number, to: number) => ({ fromIndex: from, items: Array.from({ length: to - from }, (_, i) => item(from + i)), endIndex: to });

/** A session stream of `n` items whose manifest holds them one page each. */
async function session(machine: Machine, id: string, n: number) {
  const stream = await streamId(machine.mk, "claude", id);
  const writer = await StreamWriter.open(machine, stream, FMT_FEED_V1);
  for (let i = 0; i < n; i++) await writer.append(page(i, i + 1));
  const cp = await writer.manifest(Array.from({ length: n }, (_, i) => page(i, i + 1)));
  return { stream, writer, cp };
}

/** Page fetches the test answers by hand: each one as it was asked, with what it was handed. */
class Fetches {
  readonly asked: { stream: string; page: string; opts: PageFetch; answer(): void; fail(error: unknown): void; settled: boolean }[] = [];
  readonly log: string[] = [];
  constructor(private readonly pages: Map<string, Bytes>) {}
  readonly fetch = (stream: Uint8Array, hash: Uint8Array, opts: PageFetch = {}) =>
    new Promise<Uint8Array>((resolve, reject) => {
      const entry = {
        stream: hex(stream),
        page: hex(hash),
        opts,
        settled: false,
        answer: () => {
          if (entry.settled) return;
          entry.settled = true;
          opts.onHead?.();
          resolve(this.pages.get(hex(hash))!);
        },
        fail: (error: unknown) => {
          if (entry.settled) return;
          entry.settled = true;
          this.log.push(`settled ${hex(hash).slice(0, 6)}`);
          reject(error);
        },
      };
      this.log.push(`asked ${hex(hash).slice(0, 6)}`);
      opts.signal?.addEventListener("abort", () => entry.fail(opts.signal!.reason));
      this.asked.push(entry);
    });
  open() {
    return this.asked.filter((a) => !a.settled);
  }
}

function client(machine: Machine, link: FakeStreamLink, fetches: Fetches, opts: Partial<StreamClientOptions> = {}) {
  return new StreamClient({ link, machineSignKey: machine.signPublic, epochs: machine.epochs, fetchPage: fetches.fetch, retryDelay: () => 0, ...opts });
}

const subOf = (link: FakeStreamLink, stream: Bytes) => link.subs().filter((s) => hex(s.stream) === hex(stream)).at(-1)!;
const since = () => connLog.events().at(-1)?.seq ?? 0;
const logged = (from: number, name: string) => connLog.events().filter((e) => e.seq > from && e.name === name);

test("a subscription closed during its download fetches nothing more of it, aborts what is in flight, and says it was cancelled", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const { stream, cp } = await session(machine, "closed", 5);
  const fetches = new Fetches(cp.pages);
  const errors: unknown[] = [];
  const c = client(machine, link, fetches, { pageConcurrency: 2 });
  const sub = c.subscribe(stream, feedFold, { onError: (e) => errors.push(e) });
  await until(() => link.subs().length === 1);
  const mark = since();
  link.deliver({ type: "checkpoint", ch: subOf(link, stream).ch, stream, uptoSeq: cp.upto, manifest: cp.manifest });
  await until(() => fetches.asked.length === 2);
  fetches.asked[0]!.answer();
  await until(() => fetches.asked.length === 3);
  sub.close();
  await flush(20);
  assert.equal(fetches.asked.length, 3, "the two pages still waiting were never fetched");
  assert.ok(fetches.asked.slice(1).every((a) => a.opts.signal?.aborted && a.settled), "the two in flight were aborted");
  assert.deepEqual(errors, [], "closing is not a failure, and nothing is asked again");
  assert.equal(link.subs().length, 1);
  const done = logged(mark, "stream.done").at(-1)!;
  assert.equal(done.fields.state, "cancelled");
  assert.equal(done.fields.done, 1);
  assert.equal(done.fields.total, 5);
  c.close();
});

test("a page that fails ends the download, and the stream starts over only once none of its fetches is still running", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const { stream, cp } = await session(machine, "fails", 6);
  const fetches = new Fetches(cp.pages);
  const c = client(machine, link, fetches, { pageConcurrency: 2 });
  c.subscribe(stream, feedFold);
  await until(() => link.subs().length === 1);
  const mark = since();
  link.deliver({ type: "checkpoint", ch: subOf(link, stream).ch, stream, uptoSeq: cp.upto, manifest: cp.manifest });
  await until(() => fetches.asked.length === 2);
  // The newest page fails; the one beside it is still coming.
  fetches.asked[0]!.fail(new Error("503"));
  await until(() => link.subs().length === 2);
  assert.equal(fetches.asked.length, 2, "not one of the four pages still waiting was fetched");
  assert.equal(fetches.asked[1]!.opts.signal?.aborted, true, "the one in flight was aborted");
  assert.deepEqual(fetches.log, [`asked ${fetches.asked[0]!.page.slice(0, 6)}`, `asked ${fetches.asked[1]!.page.slice(0, 6)}`, `settled ${fetches.asked[0]!.page.slice(0, 6)}`, `settled ${fetches.asked[1]!.page.slice(0, 6)}`]);
  assert.equal(logged(mark, "stream.done").at(-1)!.fields.state, "failed");

  // Asked again: a download of its own, beside nothing left of the first.
  link.deliver({ type: "checkpoint", ch: subOf(link, stream).ch, stream, uptoSeq: cp.upto, manifest: cp.manifest });
  await until(() => fetches.asked.length === 4);
  assert.equal(fetches.open().length, 2);
  for (let answered = 0; answered < 6; answered++) {
    await until(() => fetches.open().length > 0);
    fetches.open()[0]!.answer();
    await flush(2);
  }
  await until(() => logged(mark, "stream.done").length === 2);
  assert.equal(logged(mark, "stream.done").at(-1)!.fields.state, "done");
  c.close();
});

test("a session somebody waits on goes ahead of every machine's background, one queue for the page; background asks for low priority", async () => {
  const [one, two] = [await makeMachine(), await makeMachine()];
  const link = new FakeStreamLink();
  const warm = await session(one, "warm", 3);
  const opened = await session(two, "opened", 2);
  const fetches = new Fetches(new Map([...warm.cp.pages, ...opened.cp.pages]));
  const queue = new PageQueue({ limits: { fg: 1, bg: 1 }, log: false });
  const a = client(one, link, fetches, { pageQueue: queue });
  const b = client(two, link, fetches, { pageQueue: queue });
  a.subscribe(warm.stream, feedFold);
  const waited = b.subscribe(opened.stream, feedFold);
  await until(() => link.subs().length === 2);
  link.deliver({ type: "checkpoint", ch: subOf(link, warm.stream).ch, stream: warm.stream, uptoSeq: warm.cp.upto, manifest: warm.cp.manifest });
  await until(() => fetches.asked.length === 1);
  link.deliver({ type: "checkpoint", ch: subOf(link, opened.stream).ch, stream: opened.stream, uptoSeq: opened.cp.upto, manifest: opened.cp.manifest });
  await flush(20);
  assert.equal(fetches.asked.length, 1, "one page at a time for the whole page, across both machines");
  // Somebody opens the session on the second machine and waits for it.
  waited.setLane("fg");
  const order = () => fetches.asked.map((f) => (f.stream === hex(warm.stream) ? "warm" : "opened"));
  for (let i = 0; i < 5; i++) {
    await until(() => fetches.open().length === 1);
    fetches.open()[0]!.answer();
    await until(() => fetches.asked.length === Math.min(i + 2, 5) && (i === 4 || fetches.open().length === 1));
  }
  assert.deepEqual(order(), ["warm", "opened", "opened", "warm", "warm"], "the waited-on pages next, then the warm ones");
  assert.deepEqual(
    fetches.asked.map((f) => f.opts.priority ?? "default"),
    ["low", "default", "default", "low", "low"],
    "a background fetch asks the browser for low priority",
  );
  a.close();
  b.close();
});

test("a checkpoint download says its start and end in the connection log: pages, lane, why, bytes, time — the device only as its mask", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const { stream, cp } = await session(machine, "logged", 3);
  const fetches = new Fetches(cp.pages);
  const id = "machine-id-that-is-never-logged";
  const c = client(machine, link, fetches, { logId: maskId(id) });
  c.subscribe(stream, feedFold);
  await until(() => link.subs().length === 1);
  const mark = since();
  link.deliver({ type: "gone", ch: subOf(link, stream).ch, stream });
  link.deliver({ type: "checkpoint", ch: subOf(link, stream).ch, stream, uptoSeq: cp.upto, manifest: cp.manifest });
  await until(() => fetches.asked.length === 3);
  for (const f of fetches.asked) f.answer();
  await until(() => logged(mark, "stream.done").length === 1);
  const start = logged(mark, "stream.checkpoint")[0]!;
  assert.deepEqual(start.fields, { device: maskId(id), kind: "session", pages: 3, lane: "bg", cause: "gone" });
  const done = logged(mark, "stream.done")[0]!;
  const bytes = [...cp.pages.values()].reduce((n, p) => n + p.length, 0);
  assert.deepEqual({ ...done.fields, ms: 0 }, { device: maskId(id), kind: "session", state: "done", done: 3, total: 3, bytes, ms: 0 });
  assert.ok(!JSON.stringify(connLog.events().filter((e) => e.seq > mark)).includes(id));
  c.close();
});

test("a page that stalls fails its download, which starts over through the stream's backoff", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const { stream, cp } = await session(machine, "stalls", 3);
  const fetches = new Fetches(cp.pages);
  const errors: unknown[] = [];
  const delays: number[] = [];
  const queue = new PageQueue({ limits: { fg: 1, bg: 1 }, stallMs: 40, log: false });
  const c = client(machine, link, fetches, {
    pageQueue: queue,
    retryDelay: (attempt) => (delays.push(attempt), 0),
  });
  c.subscribe(stream, feedFold, { onError: (e) => errors.push(e) });
  await until(() => link.subs().length === 1);
  const mark = since();
  link.deliver({ type: "checkpoint", ch: subOf(link, stream).ch, stream, uptoSeq: cp.upto, manifest: cp.manifest });
  // The first page's answer never comes.
  await until(() => fetches.asked.length === 1);
  await until(() => link.subs().length === 2, 2_000);
  assert.equal(fetches.asked[0]!.opts.signal?.aborted, true, "given up on: its fetch aborted");
  assert.equal(fetches.asked.length, 1, "nothing more of the stalled download was fetched");
  assert.equal((errors[0] as DOMException).name, "TimeoutError");
  assert.deepEqual(delays, [0], "the stream's own backoff decides when it is asked again");
  const done = logged(mark, "stream.done").at(-1)!;
  assert.deepEqual({ state: done.fields.state, errName: done.fields.errName, error: done.fields.error }, { state: "failed", errName: "TimeoutError", error: "the page stalled" });
  c.close();
});

test("a machine's index and status are fetched in the lane they are given, the names followed later too", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const fetches = new Fetches(new Map());
  const c = client(machine, link, fetches);
  const subs: StreamSubscription<unknown>[] = [];
  const subscribe = c.subscribe.bind(c);
  c.subscribe = ((...args: Parameters<typeof c.subscribe>) => {
    const sub = subscribe(...args);
    subs.push(sub as StreamSubscription<unknown>);
    return sub;
  }) as typeof c.subscribe;
  const source = HubCopySource.over({ mk: machine.mk, client: c });
  const places = await source.places({ kind: "index" });
  const follow = source.follow({ kind: "index" }, places, { at: 0, from: null }, { onState: () => undefined })!;
  assert.ok(subs.length >= 1);
  assert.ok(subs.every((s) => s.lane === "bg"), "background by default");
  follow.setForeground!(true);
  assert.ok(subs.every((s) => s.lane === "fg"));
  follow.setForeground!(false);
  assert.ok(subs.every((s) => s.lane === "bg"));
  follow.close();
  c.close();
});
