// The page's queue of snapshot-page downloads (`lib/hub/page-queue.ts`): a
// session somebody waits on goes ahead of the background; on a slow link the
// background runs one page at a time and the foreground two, on a fast one
// four; the background gives way while a live request is in flight, a while
// after it, and after a channel opens — trickling a page through a pause
// that never ends; a page that stalls is given up on; and a page let go of
// before its turn is never fetched.
import assert from "node:assert/strict";
import test from "node:test";
import { connLog } from "../src/lib/connection-log.js";
import {
  COOLDOWN_MS,
  FAST_LIMITS,
  type LinkInfo,
  OPENED_MS,
  type PageClock,
  type PageJob,
  type PageLane,
  PageQueue,
  SLOW_LIMITS,
  slowLink,
  STALL_MS,
  TRICKLE_MS,
} from "../src/lib/hub/page-queue.js";

/** A clock the test moves by hand: timers fire as it passes them. */
class HandClock implements PageClock {
  t = 0;
  private timers = new Map<number, { at: number; fn: () => void }>();
  private next = 1;
  now() {
    return this.t;
  }
  setTimeout(fn: () => void, ms: number) {
    const id = this.next++;
    this.timers.set(id, { at: this.t + ms, fn });
    return id;
  }
  clearTimeout(id: unknown) {
    this.timers.delete(id as number);
  }
  /** Move on `ms`, firing every timer due on the way, in order. */
  advance(ms: number) {
    const end = this.t + ms;
    for (;;) {
      const due = [...this.timers].filter(([, x]) => x.at <= end).sort((a, b) => a[1].at - b[1].at)[0];
      if (!due) break;
      this.timers.delete(due[0]);
      this.t = due[1].at;
      due[1].fn();
    }
    this.t = end;
  }
}

/** Let the queue's promises run. */
const settle = async () => {
  for (let i = 0; i < 5; i++) await new Promise((resolve) => setImmediate(resolve));
};

/** Jobs the test finishes by hand: which started (in order, with their lane), and a way to end each. */
function jobs(queue: PageQueue) {
  const started: { name: string; lane: PageLane; ctx: PageJob }[] = [];
  const ends = new Map<string, (bytes?: number) => void>();
  const fails = new Map<string, (error: unknown) => void>();
  const results = new Map<string, Promise<Uint8Array>>();
  const add = (name: string, lane: PageLane | (() => PageLane), signal?: AbortSignal) => {
    const p = queue.run(
      lane,
      (ctx) =>
        new Promise<Uint8Array>((resolve, reject) => {
          started.push({ name, lane: ctx.lane, ctx });
          ends.set(name, (bytes = 61_476) => resolve(new Uint8Array(bytes)));
          fails.set(name, reject);
          ctx.signal.addEventListener("abort", () => reject(ctx.signal.reason));
        }),
      signal,
    );
    p.catch(() => undefined);
    results.set(name, p);
    return p;
  };
  const names = () => started.map((s) => s.name);
  const end = async (name: string, bytes?: number) => {
    ends.get(name)!(bytes);
    await settle();
  };
  return { add, names, end, started, fails, results };
}

const fixed = (n: number) => new PageQueue({ limits: { fg: n, bg: n }, log: false });

test("slow links: saveData, 3g or slower, a downlink under 5 Mbps, a round trip of 250 ms or more", () => {
  const cases: [LinkInfo, boolean][] = [
    [{ effectiveType: "4g", downlink: 10, rtt: 50 }, false],
    [{ effectiveType: "4g", downlink: 10, rtt: 50, saveData: true }, true],
    [{ effectiveType: "3g", downlink: 10, rtt: 50 }, true],
    [{ effectiveType: "2g" }, true],
    [{ effectiveType: "slow-2g" }, true],
    [{ effectiveType: "4g", downlink: 1.5, rtt: 100 }, true],
    [{ effectiveType: "4g", downlink: 10, rtt: 250 }, true],
    [{ effectiveType: "4g", downlink: 10, rtt: 249 }, false],
    // Chrome says 0 when it does not know (offline): not taken for slow.
    [{ effectiveType: "4g", downlink: 0, rtt: 0 }, false],
  ];
  for (const [link, slow] of cases) assert.equal(slowLink(link), slow, JSON.stringify(link));
  const q = (link: LinkInfo) => new PageQueue({ link: () => link, log: false }).limits();
  assert.deepEqual(q({ effectiveType: "3g", downlink: 1.6, rtt: 400 }), { limits: SLOW_LIMITS, slow: true, basis: "connection" });
  assert.deepEqual(q({ effectiveType: "4g", downlink: 10, rtt: 50 }), { limits: FAST_LIMITS, slow: false, basis: "connection" });
  assert.deepEqual(SLOW_LIMITS, { fg: 2, bg: 1 });
  assert.deepEqual(FAST_LIMITS, { fg: 4, bg: 4 });
});

test("a session somebody waits on jumps the queue, and a download that becomes foreground moves ahead with what it still needs", async () => {
  const queue = fixed(1);
  const j = jobs(queue);
  for (const name of ["warm-1", "warm-2", "warm-3"]) j.add(name, "bg");
  await settle();
  assert.deepEqual(j.names(), ["warm-1"]);
  // A reader opens a session not on the device: its page goes next, ahead of the warm ones queued before it.
  j.add("opened-1", "fg");
  j.add("opened-2", "fg");
  await j.end("warm-1");
  assert.deepEqual(j.names(), ["warm-1", "opened-1"]);
  await j.end("opened-1");
  assert.deepEqual(j.names(), ["warm-1", "opened-1", "opened-2"]);
  await j.end("opened-2");
  assert.deepEqual(j.names(), ["warm-1", "opened-1", "opened-2", "warm-2"], "the background goes on once nobody is waiting");

  // A lane is asked when the page's turn comes: a warm download a reader starts waiting on mid-way moves ahead.
  let lane: PageLane = "bg";
  j.add("other-bg", "bg");
  j.add("became-fg", () => lane);
  lane = "fg";
  queue.relane();
  await j.end("warm-2");
  assert.equal(j.names().at(-1), "became-fg", "ahead of the warm-3 and other-bg queued before it");
  assert.equal(j.started.at(-1)!.lane, "fg");
});

test("on a slow link the background runs one page at a time and the foreground two; on a fast one, four", async () => {
  let link: LinkInfo = { effectiveType: "3g", downlink: 1.5, rtt: 400 };
  const queue = new PageQueue({ link: () => link, log: false });
  const j = jobs(queue);
  for (let i = 0; i < 6; i++) j.add(`bg-${i}`, "bg");
  await settle();
  assert.deepEqual(j.names(), ["bg-0"], "one background page at a time");
  j.add("fg-0", "fg");
  j.add("fg-1", "fg");
  await settle();
  assert.deepEqual(j.names(), ["bg-0", "fg-0"], "a second page for the foreground, beside the one in flight");
  await j.end("bg-0");
  assert.deepEqual(j.names(), ["bg-0", "fg-0", "fg-1"], "the foreground first: two of it");
  await j.end("fg-0");
  await j.end("fg-1");
  assert.deepEqual(j.names().slice(3), ["bg-1"]);
  assert.equal(j.started.find((s) => s.name === "bg-1")!.lane, "bg", "a background fetch is told so (low priority)");
  // The link gets better: four at once.
  link = { effectiveType: "4g", downlink: 10, rtt: 60 };
  await j.end("bg-1");
  assert.deepEqual(j.names().slice(4), ["bg-2", "bg-3", "bg-4", "bg-5"]);
});

test("where the browser says nothing of the link, measured throughput decides: slow until the pages say otherwise", async () => {
  const clock = new HandClock();
  const queue = new PageQueue({ link: () => undefined, clock, log: false });
  assert.deepEqual(queue.limits(), { limits: SLOW_LIMITS, slow: true, basis: "unmeasured" });
  const page = async (headMs: number, bodyMs: number) => {
    const done = queue.run("bg", async (ctx) => {
      clock.advance(headMs);
      ctx.head();
      clock.advance(bodyMs);
      return new Uint8Array(61_476);
    });
    await done;
  };
  // 61 476 bytes in 400 ms of body: about 1.2 Mbps.
  for (let i = 0; i < 3; i++) await page(400, 400);
  assert.equal(queue.limits().basis, "measured");
  assert.ok(queue.measuredBps! > 1_000_000 && queue.measuredBps! < 1_400_000, String(queue.measuredBps));
  assert.deepEqual(queue.limits().limits, SLOW_LIMITS);
  // The round trip before the head is not the link's throughput: 30 ms of body each is about 16 Mbps.
  for (let i = 0; i < 20; i++) await page(300, 30);
  assert.deepEqual(queue.limits(), { limits: FAST_LIMITS, slow: false, basis: "measured" });
});

test("measured: pages that come at once (the browser's cache, a hub next door) count as a fast link, and a body that hangs does not drag it down", async () => {
  const clock = new HandClock();
  const queue = new PageQueue({ link: () => undefined, clock, log: false });
  const instant = () => queue.run("bg", async (ctx) => (ctx.head(), new Uint8Array(61_476)));
  for (let i = 0; i < 3; i++) await instant();
  assert.deepEqual(queue.limits(), { limits: FAST_LIMITS, slow: false, basis: "measured" }, "not slow forever for want of a round trip");
  // A page whose body stops for 25 s half-way and then goes on: the gap counts for 2 s at most.
  await queue.run("bg", async (ctx) => {
    clock.advance(100);
    ctx.head();
    clock.advance(15);
    ctx.bytes(30_000);
    clock.advance(25_000);
    ctx.bytes(31_476);
    return new Uint8Array(61_476);
  });
  assert.deepEqual(queue.limits().limits, FAST_LIMITS, "one stalled body is not a slow link");
  // A link that really is slow says so within a few pages, whatever came before.
  const slow = () =>
    queue.run("bg", async (ctx) => {
      clock.advance(400);
      ctx.head();
      for (let i = 0; i < 4; i++) {
        clock.advance(100);
        ctx.bytes(15_369);
      }
      return new Uint8Array(61_476);
    });
  for (let i = 0; i < 5; i++) await slow();
  assert.deepEqual(queue.limits(), { limits: SLOW_LIMITS, slow: true, basis: "measured" });
});

test("background gives way while a live request is in flight and 2 s after it; the foreground never does", async () => {
  const clock = new HandClock();
  const queue = new PageQueue({ limits: { fg: 4, bg: 4 }, clock, log: false });
  const j = jobs(queue);
  const release = queue.hold("live");
  assert.equal(queue.paused, "live");
  j.add("bg-0", "bg");
  j.add("fg-0", "fg");
  await settle();
  assert.deepEqual(j.names(), ["fg-0"], "the foreground goes; the background waits for the live request");
  clock.advance(5_000);
  await settle();
  assert.deepEqual(j.names(), ["fg-0"], "still in flight: still waiting");
  release();
  await settle();
  assert.deepEqual(j.names(), ["fg-0"], "just answered: the page's next requests often follow at once");
  clock.advance(COOLDOWN_MS - 1);
  await settle();
  assert.deepEqual(j.names(), ["fg-0"]);
  clock.advance(1);
  await settle();
  assert.deepEqual(j.names(), ["fg-0", "bg-0"], "quiet for 2 s: the background goes on");
  assert.equal(queue.paused, null);
  // What is in flight finishes when a live request starts: only what has not started waits.
  const again = queue.hold("live");
  j.add("bg-1", "bg");
  await j.end("bg-0");
  await settle();
  assert.deepEqual(j.names(), ["fg-0", "bg-0"]);
  again();
  clock.advance(COOLDOWN_MS);
  await settle();
  assert.deepEqual(j.names(), ["fg-0", "bg-0", "bg-1"]);
});

test("background waits 10 s after a channel opens", async () => {
  const clock = new HandClock();
  const queue = new PageQueue({ limits: { fg: 4, bg: 4 }, clock, log: false });
  const j = jobs(queue);
  queue.pauseFor("opened");
  j.add("bg-0", "bg");
  await settle();
  assert.deepEqual(j.names(), []);
  assert.equal(queue.paused, "opened");
  clock.advance(OPENED_MS - 1);
  await settle();
  assert.deepEqual(j.names(), []);
  clock.advance(1);
  await settle();
  assert.deepEqual(j.names(), ["bg-0"]);
});

test("a pause that never ends still lets one background page through every 15 s", async () => {
  const clock = new HandClock();
  const queue = new PageQueue({ limits: { fg: 4, bg: 4 }, clock, log: false });
  const j = jobs(queue);
  queue.hold("live");
  for (let i = 0; i < 3; i++) j.add(`bg-${i}`, "bg");
  clock.advance(TRICKLE_MS - 1);
  await settle();
  assert.deepEqual(j.names(), []);
  clock.advance(1);
  await settle();
  assert.deepEqual(j.names(), ["bg-0"], "one page, not the whole queue");
  clock.advance(2_000);
  await j.end("bg-0");
  assert.deepEqual(j.names(), ["bg-0"], "the next one 15 s after this one started");
  clock.advance(TRICKLE_MS - 2_000);
  await settle();
  assert.deepEqual(j.names(), ["bg-0", "bg-1"]);
});

test("a page with nothing arriving for 30 s is aborted, its caller told, and the background goes on without waiting for its fetch to notice", async () => {
  const clock = new HandClock();
  const queue = new PageQueue({ link: () => ({ effectiveType: "3g", downlink: 1.5, rtt: 400 }), clock, log: false });
  // Jobs that ignore their abort: the queue must not wait on them.
  const started: { name: string; ctx: PageJob }[] = [];
  const results: Promise<Uint8Array>[] = [];
  for (let i = 0; i < 6; i++) {
    const p = queue.run("bg", (ctx) => {
      started.push({ name: `bg-${i}`, ctx });
      return new Promise<Uint8Array>(() => undefined);
    });
    p.catch(() => undefined);
    results.push(p);
  }
  await settle();
  assert.deepEqual(started.map((s) => s.name), ["bg-0"]);
  // Its head, then a piece of its body every 20 s: slow, not stalled.
  started[0]!.ctx.head();
  for (let i = 0; i < 3; i++) {
    clock.advance(20_000);
    started[0]!.ctx.bytes(1_000);
  }
  await settle();
  assert.equal(started[0]!.ctx.signal.aborted, false, "bytes keep coming: never given up on");
  clock.advance(STALL_MS - 1);
  await settle();
  assert.equal(started.length, 1);
  clock.advance(1);
  await settle();
  assert.equal(started[0]!.ctx.signal.aborted, true);
  assert.equal((started[0]!.ctx.signal.reason as DOMException).name, "TimeoutError");
  await assert.rejects(results[0]!, { name: "TimeoutError", message: "the page stalled" }, "its caller is told, though its fetch never settles");
  assert.deepEqual(started.map((s) => s.name), ["bg-0", "bg-1"], "and the next one goes in its place");
  // Ten minutes of nothing but hung pages: each is given up on in turn; none holds the rest back for good.
  for (let at = 0; at < 10 * 60_000; at += 10_000) {
    clock.advance(10_000);
    await settle();
  }
  assert.equal(started.length, 6);
  assert.equal(queue.queued, 0);
  assert.equal(queue.inFlight, 0);
});

test("a pause that ran out unseen is over: a live request after a quiet while holds the background back as a new pause, said for what it was", async () => {
  const clock = new HandClock();
  const queue = new PageQueue({ limits: { fg: 4, bg: 4 }, clock });
  const since = connLog.events().at(-1)?.seq ?? 0;
  const mine = () => connLog.events().filter((e) => e.seq > since && e.name.startsWith("pages."));
  // A channel opens with nothing to download; two minutes pass.
  queue.pauseFor("opened");
  clock.advance(120_000);
  await settle();
  assert.equal(queue.paused, null);
  // A live request, and then a download.
  const release = queue.hold("live");
  const j = jobs(queue);
  j.add("bg-0", "bg");
  await settle();
  assert.deepEqual(j.names(), [], "nothing under the live request: the old pause's two minutes do not count towards a trickle");
  clock.advance(TRICKLE_MS - 1);
  await settle();
  assert.deepEqual(j.names(), []);
  release();
  clock.advance(COOLDOWN_MS);
  await settle();
  assert.deepEqual(j.names(), ["bg-0"]);
  const said = mine().map((e) => [e.name, e.fields.cause, e.fields.pausedMs]);
  assert.deepEqual(said, [
    ["pages.pause", "live", undefined],
    ["pages.resume", "live", TRICKLE_MS - 1 + COOLDOWN_MS],
  ]);
});

test("a pause is timed on the page's monotonic clock: the wall clock stepping back does not hold the background back", async () => {
  const queue = new PageQueue({ limits: { fg: 4, bg: 4 }, cooldownMs: 20, log: false });
  const wall = Date.now;
  try {
    queue.hold("live")();
    // An hour back, as a clock set right by the network might.
    const back = wall() - 3_600_000;
    Date.now = () => back;
    const j = jobs(queue);
    j.add("bg-0", "bg");
    const deadline = wall() + 2_000;
    while (j.names().length === 0 && wall() < deadline) await new Promise((resolve) => setTimeout(resolve, 5));
    assert.deepEqual(j.names(), ["bg-0"], "20 ms after the live request, not an hour");
  } finally {
    Date.now = wall;
  }
});

test("a page let go of before its turn is never fetched; one in flight is handed the abort", async () => {
  const queue = fixed(1);
  const j = jobs(queue);
  const abort = new AbortController();
  j.add("a-0", "bg", abort.signal);
  j.add("a-1", "bg", abort.signal);
  j.add("b-0", "bg");
  await settle();
  assert.deepEqual(j.names(), ["a-0"]);
  abort.abort(new DOMException("closed", "AbortError"));
  await settle();
  await assert.rejects(j.results.get("a-0")!, { name: "AbortError" }, "the fetch in flight is aborted");
  await assert.rejects(j.results.get("a-1")!, { name: "AbortError" });
  assert.deepEqual(j.names(), ["a-0", "b-0"], "a-1 was never fetched; the next one goes in its place");
  assert.equal(j.started[0]!.ctx.signal.aborted, true);
  // Already aborted: never queued at all.
  await assert.rejects(queue.run("bg", async () => new Uint8Array(), abort.signal), { name: "AbortError" });
  queue.close();
  await assert.rejects(queue.run("bg", async () => new Uint8Array()), { name: "AbortError" }, "a closed queue takes nothing");
});

test("the queue says in the connection log when it holds the background back, when it lets go, and the limits it runs at", async () => {
  const clock = new HandClock();
  const queue = new PageQueue({ link: () => ({ effectiveType: "3g", downlink: 1.5, rtt: 400 }), clock });
  const since = connLog.events().at(-1)?.seq ?? 0;
  const mine = () => connLog.events().filter((e) => e.seq > since && e.name.startsWith("pages."));
  const release = queue.hold("live");
  assert.equal(mine().length, 0, "a live request with nothing held back is not news");
  const j = jobs(queue);
  j.add("bg-0", "bg");
  await settle();
  assert.deepEqual(
    mine().map((e) => [e.name, e.fields]),
    [
      ["pages.limits", { fg: 2, bg: 1, state: "slow", basis: "connection", kbps: null }],
      ["pages.pause", { cause: "live", queued: 1, inFlight: 0 }],
    ],
  );
  clock.advance(500);
  release();
  clock.advance(COOLDOWN_MS);
  await settle();
  const resume = mine().at(-1)!;
  assert.equal(resume.name, "pages.resume");
  assert.deepEqual(resume.fields, { cause: "live", pausedMs: 500 + COOLDOWN_MS, queued: 1, inFlight: 0 }, "said as it lets go, before the page it held back starts");
  assert.deepEqual(j.names(), ["bg-0"]);
});
