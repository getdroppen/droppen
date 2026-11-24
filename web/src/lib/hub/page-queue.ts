/*
 * The page's queue of snapshot-page downloads (`GET /v1/streams/…/pages/…`,
 * `StreamClient.pages`): every machine's offline copy fetches its checkpoint
 * pages through one of these, so how much of the person's link they take is
 * decided in one place, for the whole page.
 *
 * Why: a copy that catches up on a rebuilt or compacted stream downloads the
 * whole of it, hundreds of 60 KB pages. Four of them at a time on a link of a
 * megabit or two keep a second or more of bytes queued ahead of everything
 * else the page receives — the live channel to a device, its probes and the
 * direct path included — for as long as the download lasts. That is not
 * worth it while somebody is using the device live: the copy is what the page
 * shows when the device is *not* there.
 *
 * - Two lanes. Foreground (`fg`): a session a reader is waiting on, because
 *   it is being opened and its copy is not on this device yet
 *   (`MachineMirror.session`). It goes first and is never paused. Background
 *   (`bg`): everything else — the warm sessions kept ready, the index, the
 *   status. A job's lane is asked when its turn comes, so a download that
 *   becomes foreground half-way moves ahead with the pages it still needs.
 * - Limits by link ({@link PageQueue.limits}). A slow link — `saveData`, 3g
 *   or slower, a downlink under {@link SLOW_DOWNLINK_MBPS} or a round trip of
 *   {@link SLOW_RTT_MS} or more as `navigator.connection` says it, or, where
 *   the browser says nothing (Safari, Firefox), a measured throughput under
 *   the same, or nothing measured yet — gets {@link SLOW_LIMITS}; any other
 *   {@link FAST_LIMITS}. A background page starts only while fewer than its
 *   limit of pages of either lane are in flight.
 * - Background gives way to live traffic ({@link PageQueue.hold},
 *   {@link PageQueue.pauseFor}): it starts nothing while a channel to a
 *   device has a request in flight, for {@link COOLDOWN_MS} after the last
 *   one, and for {@link OPENED_MS} after a channel opens. What is in flight
 *   already finishes. A pause that lasts {@link TRICKLE_MS} lets one page
 *   through each {@link TRICKLE_MS}, so a copy still moves under a page that
 *   is never quiet.
 * - Cancelled: a job whose signal aborts before its turn is dropped and never
 *   fetched; one already running is handed the signal, for its fetch.
 *
 * It only decides when a page is fetched. What is fetched, and every check of
 * it against the manifest the machine signed, is the caller's.
 */
import { connLog } from "@/lib/connection-log";

/** Which lane a page is fetched in: someone is waiting on it, or not. */
export type PageLane = "fg" | "bg";

/** Why background pages are held back: a live request in flight (and just after), or a channel just opened. */
export type PauseCause = "live" | "opened";

/** How many pages may be in flight at once: of either lane for a foreground page to start, for a background one. */
export interface PageLimits {
  readonly fg: number;
  readonly bg: number;
}

/** The slice of `navigator.connection` (Network Information API) read here. */
export interface LinkInfo {
  readonly saveData?: boolean;
  readonly effectiveType?: string;
  /** Mbps, as the browser estimates it. */
  readonly downlink?: number;
  /** Milliseconds, as the browser estimates it. */
  readonly rtt?: number;
}

/** A downlink under this (Mbps) is a slow link. */
export const SLOW_DOWNLINK_MBPS = 5;
/** A round trip of this or more (ms) is a slow link. */
export const SLOW_RTT_MS = 250;
/** On a slow link: two foreground pages at once, one background. */
export const SLOW_LIMITS: PageLimits = { fg: 2, bg: 1 };
/** Otherwise: four, as before there were lanes. */
export const FAST_LIMITS: PageLimits = { fg: 4, bg: 4 };
/** Background waits this long after the last live request ends. */
export const COOLDOWN_MS = 2_000;
/** Background waits this long after a channel to a device opens: its first requests come then. */
export const OPENED_MS = 10_000;
/** A pause this long lets one background page through, and one more each time again. */
export const TRICKLE_MS = 15_000;
/** Pages measured before a measured throughput decides the link. */
const MEASURED_MIN = 3;
/** How much body time the measurement remembers (ms): the last few seconds of downloading. */
const MEASURE_WINDOW_MS = 10_000;
/** A page whose head came sooner than this (ms) came from the browser's cache, not the link: not measured. */
const CACHED_HEAD_MS = 10;

/** Whether `link` is a slow one ({@link SLOW_DOWNLINK_MBPS}, {@link SLOW_RTT_MS}). */
export function slowLink(link: LinkInfo): boolean {
  if (link.saveData === true) return true;
  if (typeof link.effectiveType === "string" && /^(?:slow-2g|2g|3g)$/.test(link.effectiveType)) return true;
  if (typeof link.downlink === "number" && link.downlink > 0 && link.downlink < SLOW_DOWNLINK_MBPS) return true;
  if (typeof link.rtt === "number" && link.rtt >= SLOW_RTT_MS) return true;
  return false;
}

/** Whether `link` says anything at all (a browser without the API has none; one with it may leave fields out). */
function saysAnything(link: LinkInfo | null | undefined): link is LinkInfo {
  return !!link && (link.saveData !== undefined || link.effectiveType !== undefined || link.downlink !== undefined || link.rtt !== undefined);
}

/** `navigator.connection`, where the browser has it. */
function browserLink(): LinkInfo | undefined {
  try {
    return (globalThis.navigator as (Navigator & { connection?: LinkInfo }) | undefined)?.connection ?? undefined;
  } catch {
    return undefined;
  }
}

/** The time and timers the queue runs on (a test's own, for a simulated link). */
export interface PageClock {
  now(): number;
  setTimeout(fn: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

const realClock: PageClock = {
  now: () => Date.now(),
  setTimeout: (fn, ms) => {
    const timer = setTimeout(fn, ms);
    // Nothing waits on the queue's timers to exit (a test runner).
    (timer as { unref?: () => void }).unref?.();
    return timer;
  },
  clearTimeout: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

export interface PageQueueOptions {
  /** Fixed limits: no link is read and nothing measured decides (a client of its own, tests). Default: by link. */
  limits?: PageLimits;
  /** What the link says (default `navigator.connection`). */
  link?: () => LinkInfo | null | undefined;
  clock?: PageClock;
  cooldownMs?: number;
  openedMs?: number;
  trickleMs?: number;
  /** Say its pauses, resumes and limits in the connection log (`pages.*`; default yes). */
  log?: boolean;
}

/** What a job is told as it starts. */
export interface PageJob {
  /** Aborted when whoever asked for the page lets go of it: hand it to the fetch. */
  readonly signal: AbortSignal;
  /** The lane it started in: a background fetch asks the browser for low priority. */
  readonly lane: PageLane;
  /** Say when the answer's head is in: the body's time, not the round trip, is what measures the link. */
  head(): void;
}

/** How the queue's limits were decided: given, from what the browser says, from what was measured, or nothing measured yet. */
export type LimitBasis = "fixed" | "connection" | "measured" | "unmeasured";

interface Queued {
  lane: () => PageLane;
  start: (lane: PageLane) => void;
  cancel: (reason: unknown) => void;
}

const NEVER = new AbortController().signal;

/** Why an aborted signal was aborted (an `AbortError`, as `fetch` rejects with). */
export function abortReason(signal: AbortSignal): unknown {
  return signal.reason ?? new DOMException("aborted", "AbortError");
}

export class PageQueue {
  private readonly queue: Queued[] = [];
  private running = 0;
  private runningBg = 0;
  private readonly holds: Record<PauseCause, number> = { live: 0, opened: 0 };
  private readonly until: Record<PauseCause, number> = { live: 0, opened: 0 };
  /** The pause under way: since when, and whether it held anything back (then it was said, and its end is too). */
  private pause: { cause: PauseCause; since: number; said: boolean } | null = null;
  private lastBgStart = Number.NEGATIVE_INFINITY;
  private timer: unknown = undefined;
  private timerAt = Number.POSITIVE_INFINITY;
  /** Body time and bytes of the pages measured ({@link measuredBps}). */
  private bodies = 0;
  private bodyMark = 0;
  private measured = { bytes: 0, ms: 0, pages: 0 };
  /** The limits last said in the log. */
  private limitsSaid = "";
  private closed = false;
  private readonly clock: PageClock;

  constructor(private readonly opts: PageQueueOptions = {}) {
    this.clock = opts.clock ?? realClock;
  }

  /**
   * Fetch one page through the queue: `job` runs when its turn comes (its
   * lane — asked then — and the limits allowing), and gets a signal for its
   * fetch. `signal` aborting before then drops it unrun; the promise rejects
   * with the signal's reason.
   */
  run(lane: PageLane | (() => PageLane), job: (ctx: PageJob) => Promise<Uint8Array>, signal?: AbortSignal): Promise<Uint8Array> {
    if (this.closed) return Promise.reject(new DOMException("the page queue is closed", "AbortError"));
    if (signal?.aborted) return Promise.reject(abortReason(signal));
    const laneOf = typeof lane === "function" ? lane : () => lane;
    return new Promise<Uint8Array>((resolve, reject) => {
      const onAbort = () => entry.cancel(abortReason(signal!));
      const entry: Queued = {
        lane: laneOf,
        cancel: (reason) => {
          const at = this.queue.indexOf(entry);
          if (at < 0) return;
          this.queue.splice(at, 1);
          signal?.removeEventListener("abort", onAbort);
          reject(reason);
          this.pump();
        },
        start: (started) => {
          signal?.removeEventListener("abort", onAbort);
          this.running += 1;
          if (started === "bg") {
            this.runningBg += 1;
            this.lastBgStart = this.clock.now();
          }
          const startedAt = this.clock.now();
          let headAt: number | null = null;
          const ctx: PageJob = {
            signal: signal ?? NEVER,
            lane: started,
            head: () => {
              if (headAt !== null) return;
              headAt = this.clock.now();
              if (headAt - startedAt >= CACHED_HEAD_MS) this.bodyStarted(headAt);
            },
          };
          const measuring = () => headAt !== null && headAt - startedAt >= CACHED_HEAD_MS;
          let run: Promise<Uint8Array>;
          try {
            run = job(ctx);
          } catch (error) {
            run = Promise.reject(error);
          }
          run
            .then(
              (bytes) => {
                if (measuring()) this.bodyEnded(bytes.length);
                resolve(bytes);
              },
              (error: unknown) => {
                if (measuring()) this.bodyEnded(null);
                reject(error);
              },
            )
            .finally(() => {
              this.running -= 1;
              if (started === "bg") this.runningBg -= 1;
              this.pump();
            });
        },
      };
      signal?.addEventListener("abort", onAbort, { once: true });
      this.queue.push(entry);
      this.pump();
    });
  }

  /**
   * A live request is in flight on a channel to a device (or something else
   * worth giving way to): background starts nothing until every hold is
   * released and {@link COOLDOWN_MS} more have passed. Returns the release.
   */
  hold(cause: PauseCause = "live"): () => void {
    this.holds[cause] += 1;
    this.pump();
    let done = false;
    return () => {
      if (done) return;
      done = true;
      this.holds[cause] -= 1;
      this.until[cause] = Math.max(this.until[cause], this.clock.now() + (this.opts.cooldownMs ?? COOLDOWN_MS));
      this.pump();
    };
  }

  /** Background starts nothing for `ms` from now (default {@link OPENED_MS}: a channel just opened). */
  pauseFor(cause: PauseCause, ms = cause === "opened" ? (this.opts.openedMs ?? OPENED_MS) : (this.opts.cooldownMs ?? COOLDOWN_MS)): void {
    this.until[cause] = Math.max(this.until[cause], this.clock.now() + ms);
    this.pump();
  }

  /** Why background is held back now (`null`: it is not). */
  get paused(): PauseCause | null {
    return this.pausedBy(this.clock.now());
  }

  /** Pages waiting for their turn. */
  get queued(): number {
    return this.queue.length;
  }

  /** Pages in flight. */
  get inFlight(): number {
    return this.running;
  }

  /** The link's throughput as the pages measured it (bits/s; `null`: too few measured yet). */
  get measuredBps(): number | null {
    const m = this.measured;
    return m.pages >= MEASURED_MIN && m.ms > 0 ? (m.bytes * 8 * 1000) / m.ms : null;
  }

  /** The limits now, whether the link is slow, and how that was decided. */
  limits(): { limits: PageLimits; slow: boolean | null; basis: LimitBasis } {
    if (this.opts.limits) return { limits: this.opts.limits, slow: null, basis: "fixed" };
    let link: LinkInfo | null | undefined;
    try {
      link = (this.opts.link ?? browserLink)();
    } catch {
      link = undefined;
    }
    if (saysAnything(link)) {
      const slow = slowLink(link);
      return { limits: slow ? SLOW_LIMITS : FAST_LIMITS, slow, basis: "connection" };
    }
    const bps = this.measuredBps;
    if (bps === null) return { limits: SLOW_LIMITS, slow: true, basis: "unmeasured" };
    const slow = bps < SLOW_DOWNLINK_MBPS * 1_000_000;
    return { limits: slow ? SLOW_LIMITS : FAST_LIMITS, slow, basis: "measured" };
  }

  /** A waiting page's lane may have changed (a reader started or stopped waiting on its session): look again. */
  relane(): void {
    this.pump();
  }

  /** Drop every page still waiting (the page is going); what is in flight finishes. */
  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.clock.clearTimeout(this.timer);
    this.timer = undefined;
    for (const entry of [...this.queue]) entry.cancel(new DOMException("the page queue is closed", "AbortError"));
  }

  private pausedBy(now: number): PauseCause | null {
    for (const cause of ["live", "opened"] as const) if (this.holds[cause] > 0 || now < this.until[cause]) return cause;
    return null;
  }

  /** When nothing holds background back any more (`Infinity`: a hold is still on). */
  private pauseEnds(now: number): number {
    let end = now;
    for (const cause of ["live", "opened"] as const) {
      if (this.holds[cause] > 0) return Number.POSITIVE_INFINITY;
      end = Math.max(end, this.until[cause]);
    }
    return end;
  }

  /** Start whatever may start now, in lane order, and set the timer for what may start later. */
  private pump(): void {
    if (this.closed) return;
    const now = this.clock.now();
    const cause = this.pausedBy(now);
    this.notePause(cause, now);
    let limits: PageLimits | null = null;
    while (this.queue.length > 0) {
      limits ??= this.sayLimits();
      const fg = this.queue.findIndex((e) => e.lane() === "fg");
      if (fg >= 0) {
        if (this.running >= limits.fg) break;
        this.take(fg).start("fg");
        continue;
      }
      if (this.running >= limits.bg) break;
      if (cause !== null && !this.trickleDue(now)) {
        this.withheld();
        break;
      }
      this.take(0).start("bg");
    }
    this.schedule(now, cause);
  }

  private take(at: number): Queued {
    return this.queue.splice(at, 1)[0]!;
  }

  /** A pause long enough to let one background page through: none of its own is in flight, and none started for a while. */
  private trickleDue(now: number): boolean {
    const trickle = this.opts.trickleMs ?? TRICKLE_MS;
    return this.runningBg === 0 && this.pause !== null && now - this.pause.since >= trickle && now - this.lastBgStart >= trickle;
  }

  private notePause(cause: PauseCause | null, now: number): void {
    if (cause !== null) {
      this.pause ??= { cause, since: now, said: false };
      return;
    }
    const pause = this.pause;
    if (!pause) return;
    this.pause = null;
    if (pause.said && this.opts.log !== false) connLog.emit("pages.resume", { cause: pause.cause, pausedMs: now - pause.since, queued: this.queue.length, inFlight: this.running });
  }

  /** A background page was held back by the pause: said once a pause. */
  private withheld(): void {
    const pause = this.pause;
    if (!pause || pause.said) return;
    pause.said = true;
    if (this.opts.log !== false) connLog.emit("pages.pause", { cause: pause.cause, queued: this.queue.length, inFlight: this.running });
  }

  /** The limits now, said in the log when they change while there is something to fetch. */
  private sayLimits(): PageLimits {
    const { limits, slow, basis } = this.limits();
    const bps = basis === "measured" ? this.measuredBps : null;
    const key = `${limits.fg}/${limits.bg}/${basis}/${slow}`;
    if (key !== this.limitsSaid && this.opts.log !== false && basis !== "fixed") {
      this.limitsSaid = key;
      connLog.emit("pages.limits", { fg: limits.fg, bg: limits.bg, state: slow ? "slow" : "fast", basis, kbps: bps === null ? null : Math.round(bps / 1000) });
    }
    return limits;
  }

  /** Wake when the pause ends, or when it has lasted long enough to trickle — while background is waiting, or a pause was said. */
  private schedule(now: number, cause: PauseCause | null): void {
    let at = Number.POSITIVE_INFINITY;
    if (cause !== null && (this.queue.length > 0 || this.pause?.said)) {
      at = this.pauseEnds(now);
      if (this.queue.length > 0 && this.pause && this.runningBg === 0) {
        // Due already, yet nothing started: the pages in flight are what
        // holds it back, and the end of one pumps again.
        const trickle = this.opts.trickleMs ?? TRICKLE_MS;
        const due = Math.max(this.pause.since + trickle, this.lastBgStart + trickle);
        if (due > now) at = Math.min(at, due);
      }
    }
    if (at === this.timerAt) return;
    this.clock.clearTimeout(this.timer);
    this.timer = undefined;
    this.timerAt = at;
    if (at === Number.POSITIVE_INFINITY) return;
    this.timer = this.clock.setTimeout(
      () => {
        this.timer = undefined;
        this.timerAt = Number.POSITIVE_INFINITY;
        this.pump();
      },
      Math.max(0, at - now),
    );
  }

  /** Count body time while any measured page's body is coming: the union, so pages in parallel are not counted twice. */
  private tick(now: number): void {
    if (this.bodies > 0) this.measured.ms += Math.max(0, now - this.bodyMark);
    this.bodyMark = now;
  }

  private bodyStarted(now: number): void {
    this.tick(now);
    this.bodies += 1;
  }

  /** A measured page's body ended: with its bytes, or (`null`) failed. */
  private bodyEnded(bytes: number | null): void {
    this.tick(this.clock.now());
    this.bodies = Math.max(0, this.bodies - 1);
    const m = this.measured;
    if (bytes !== null) {
      m.bytes += bytes;
      m.pages += 1;
    }
    if (m.ms > MEASURE_WINDOW_MS) {
      m.bytes *= MEASURE_WINDOW_MS / m.ms;
      m.ms = MEASURE_WINDOW_MS;
    }
  }
}
