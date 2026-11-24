// The offline copy's page downloads and the live channel on one slow link,
// simulated (`lib/hub/page-queue.ts`): one bottleneck of 1.5 Mbps and a
// 400 ms round trip, a drop-tail queue that serves bytes in the order they
// reach it (a phone's radio link, a deep buffer), carrying a catch-up of
// 300 snapshot pages (about 18 MiB) and, at the same time, the requests a
// person makes of their device live — a burst as the channel opens, then a
// few every few seconds.
//
// Before: four pages always in flight, nothing else considered (the queue
// at fixed limits, as `StreamClient` had them). After: the page's queue as
// the shell runs it — the link read as slow, one background page at a time,
// held back while a live request is in flight and for 2 s after, and for
// 10 s after the channel opens. The time is the test's own, so the numbers
// are the same on every run.
//
// What it pins: the live round trip's 95th percentile falls from seconds
// (behind three or four pages) to about one round trip and one page, and the
// copy still arrives, later.
import assert from "node:assert/strict";
import test from "node:test";
import { type PageClock, PageQueue } from "../src/lib/hub/page-queue.js";

const RATE_BPS = 1_500_000;
const RTT_MS = 400;
const PAGE_BYTES = 61_476;
const PAGES = 300;
/** A live answer: a small JSON body, its head and framing. */
const LIVE_BYTES = 1_500;
const MTU = 1_460;
/** The time one page takes to cross the bottleneck. */
const PAGE_MS = (PAGE_BYTES * 8 * 1000) / RATE_BPS;

/** Discrete-event time: what is due runs in order, and the promises it settles run before the next. */
class Sim implements PageClock {
  t = 0;
  private seq = 0;
  private readonly events: { at: number; seq: number; fn: () => void; dead?: boolean }[] = [];
  now() {
    return this.t;
  }
  at(at: number, fn: () => void) {
    const e = { at, seq: this.seq++, fn };
    this.events.push(e);
    return e;
  }
  setTimeout(fn: () => void, ms: number) {
    return this.at(this.t + ms, fn);
  }
  clearTimeout(handle: unknown) {
    if (handle) (handle as { dead?: boolean }).dead = true;
  }
  async run(until: () => boolean) {
    while (!until()) {
      this.events.sort((a, b) => a.at - b.at || a.seq - b.seq);
      const next = this.events.shift();
      if (!next) throw new Error("nothing left to happen, and not done");
      if (next.dead) continue;
      this.t = Math.max(this.t, next.at);
      next.fn();
      await new Promise((resolve) => setImmediate(resolve));
    }
  }
}

/** The bottleneck: bytes cross it one answer after another, at its rate, half a round trip from either end. */
class Bottleneck {
  private busyUntil = 0;
  constructor(private readonly sim: Sim) {}
  fetch(bytes: number, onHead?: () => void): Promise<void> {
    const { sim } = this;
    return new Promise((resolve) => {
      sim.at(sim.t + RTT_MS / 2, () => {
        const start = Math.max(sim.t, this.busyUntil);
        const end = start + (bytes * 8 * 1000) / RATE_BPS;
        this.busyUntil = end;
        sim.at(start + (Math.min(bytes, MTU) * 8 * 1000) / RATE_BPS + RTT_MS / 2, () => onHead?.());
        sim.at(end + RTT_MS / 2, resolve);
      });
    });
  }
}

/** A seeded generator (mulberry32): the same person every run. */
function seeded(seed: number) {
  let a = seed;
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const quantile = (xs: number[], q: number) => {
  const sorted = [...xs].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.ceil(q * sorted.length) - 1)]!;
};

interface Outcome {
  p50: number;
  p95: number;
  max: number;
  live: number;
  syncMs: number;
}

/**
 * One run: the catch-up's pages through `queue` (all asked for at once, as a
 * manifest's are), and the person's requests beside them — six one after
 * another as the channel opens, then a burst of one to three every two to
 * six seconds for two minutes — each holding the queue while in flight when
 * `gives` (as the shell's channel does).
 */
async function run(after: boolean): Promise<Outcome> {
  const sim = new Sim();
  const link = new Bottleneck(sim);
  const queue = after
    ? new PageQueue({ clock: sim, link: () => ({ effectiveType: "3g", downlink: 1.5, rtt: RTT_MS }), log: false })
    : new PageQueue({ clock: sim, limits: { fg: 4, bg: 4 }, log: false });
  if (after) queue.pauseFor("opened");
  let pagesDone = 0;
  for (let i = 0; i < PAGES; i++) {
    void queue
      .run("bg", (job) => link.fetch(PAGE_BYTES, job.head).then(() => new Uint8Array(PAGE_BYTES)))
      .then(() => {
        pagesDone += 1;
      });
  }
  let syncMs = 0;
  const rtts: number[] = [];
  let pending = 0;
  const live = (n: number) => {
    if (n === 0) return;
    pending += 1;
    const release = after ? queue.hold("live") : null;
    const startedAt = sim.t;
    void link.fetch(LIVE_BYTES).then(() => {
      rtts.push(sim.t - startedAt);
      release?.();
      pending -= 1;
      live(n - 1);
    });
  };
  const random = seeded(637);
  sim.at(0, () => live(6));
  for (let at = 4_000; at < 120_000; at += 2_000 + random() * 4_000) {
    const n = 1 + Math.floor(random() * 3);
    sim.at(at, () => live(n));
  }
  let scripted = true;
  sim.at(120_000, () => (scripted = false));
  await sim.run(() => {
    if (pagesDone === PAGES && syncMs === 0) syncMs = sim.t;
    return pagesDone === PAGES && !scripted && pending === 0;
  });
  return { p50: quantile(rtts, 0.5), p95: quantile(rtts, 0.95), max: Math.max(...rtts), live: rtts.length, syncMs };
}

test("on a slow link, live round trips drop from seconds to about one round trip and one page while the copy downloads, and the copy still arrives", async (t) => {
  const before = await run(false);
  const after = await run(true);
  const say = (label: string, o: Outcome) =>
    t.diagnostic(
      `${label}: live p50 ${Math.round(o.p50)} ms, p95 ${Math.round(o.p95)} ms, max ${Math.round(o.max)} ms over ${o.live} requests; ${PAGES} pages done at ${(o.syncMs / 1000).toFixed(1)} s`,
    );
  say("before (4 in flight)", before);
  say("after (page queue)", after);
  assert.equal(before.live, after.live, "the same person both times");
  assert.ok(before.p95 > 1_000, `before: behind three or four pages, over a second (${before.p95} ms)`);
  // One round trip, one page that may be crossing the bottleneck already, and the answer's own bytes.
  const bound = RTT_MS + PAGE_MS + (LIVE_BYTES * 8 * 1000) / RATE_BPS + 1;
  assert.ok(after.p95 <= bound, `after: p95 ${after.p95} ms, at most a round trip and a page (${Math.round(bound)} ms)`);
  assert.ok(after.max <= bound, `after: never more (${after.max} ms)`);
  // The copy arrives all the same: later, one page per round trip while the link is quiet.
  assert.ok(after.syncMs > 0 && after.syncMs < 4 * before.syncMs, `the copy still completes (${after.syncMs} ms vs ${before.syncMs} ms)`);
});
