/*
 * What a sender keeps in flight on one path: about what that path carries in
 * a round trip, not the whole window its receiver allows. The browser's half
 * of `crates/droppen-proto/src/pace.rs` (read that for the why), rule for
 * rule; the shared vector (`rust.json` `pace`) holds both to the same numbers.
 *
 *   cap = clamp(GAIN × rate × minRtt, PACE_FLOOR, ceiling)
 *
 * - The sender follows its data with markers: probes, which every receiver
 *   that speaks paths answers at once on the path they came by. An answer
 *   says everything before the marker was read: what is in flight, a
 *   delivery-rate sample (BBR's: delivered between the delivery last heard of
 *   when the marker left and its answer, over the longer of the send and the
 *   delivery time), and a round trip.
 * - The rate is the highest sample of the last `PACE_RATE_WINDOW_MS`; an
 *   app-limited sample counts only when it is higher.
 * - The minimum round trip only falls on a queued sample; a clean marker (no
 *   more than `PACE_SMALL` ahead of it) refreshes it every
 *   `PACE_MIN_RTT_REFRESH_MS`; a path busy for `PACE_MIN_RTT_WINDOW_MS`
 *   without one is drained for one (until no more than `PACE_SMALL` is out).
 * - What counts is stream data (and resends), not the multiplexer's own
 *   frames: a side that mostly reads sends a steady trickle of credits.
 * - Both ways at once: more than `PACE_QUIET_RX` received during a marker's
 *   round trip and its answer came back loaded, behind the peer's traffic.
 *   Such a round trip never refreshes the minimum (less what a pacing peer
 *   keeps queued, `PACE_FLOOR` at the rate the peer's traffic came, it may
 *   lower it), nor starts a drain. Bulk from a peer that does not mark it —
 *   an older peer, which keeps its whole window in front of the answers — makes
 *   the cap defer to the window while it lasts (`PACE_UNMARKED_SPANS`).
 *   While the answers come back loaded, markers go every sixteenth of the cap
 *   (`PACE_LOADED_SPACING` at least): room comes back a spacing late.
 * - Markers go before the first data of a flight when the minimum is due,
 *   every quarter of the cap (at least `PACE_MIN_SPACING`), and after the last
 *   data when sending stops (held, or out of data with more than `PACE_SMALL`
 *   unmarked); a small unmarked tail counts as delivered after a quiet round
 *   trip.
 * - A new path starts at the floor with `PACE_STARTUP_GAIN` until three
 *   rounds bring less than a quarter more rate.
 * - Term, rpc and mux data may go `PACE_HEADROOM` past the cap, blob data
 *   only up to it (`mux.ts` `Budget`).
 *
 * Times are milliseconds of the path clock.
 */
import { type Budget, BUDGET_NONE } from "./mux";

/** The least a cap ever is: one whole frame (`MAX_PAYLOAD_LEN`) in flight. */
export const PACE_FLOOR = 64 * 1024;
/** The cap is this many times the measured bandwidth-delay product… */
export const PACE_GAIN = 2;
/** …and this many while a new path's rate is still growing (BBR's startup gain, 2/ln 2). */
export const PACE_STARTUP_GAIN = 2.885;
/** A round's rate counts as growth when it is this much more than the last that did… */
export const PACE_STARTUP_GROWTH = 1.25;
/** …and startup ends after this many rounds without. */
export const PACE_STARTUP_ROUNDS = 3;
/** How far past the cap term, rpc and mux data may go (blob may not). */
export const PACE_HEADROOM = 16 * 1024;
/** A tail this small needs no marker; a marker with no more than this ahead of it is clean. */
export const PACE_SMALL = 4 * 1024;
/** Markers go every quarter of the cap, never closer than this. */
export const PACE_MIN_SPACING = 16 * 1024;
/** The rate is the highest sample of this long. */
export const PACE_RATE_WINDOW_MS = 1_000;
/** A clean marker refreshes the minimum round trip once it is this old. */
export const PACE_MIN_RTT_REFRESH_MS = 5_000;
/** A minimum round trip this old, on a path with no clean marker since, drains it for one. */
export const PACE_MIN_RTT_WINDOW_MS = 10_000;
/** Before a round trip is known, how long a path must be quiet for a small unmarked tail to count as delivered. */
export const PACE_QUIET_MS = 250;
/** The most markers out at once; past it the oldest is forgotten. */
export const PACE_MAX_MARKERS = 64;
/** What sealing adds to a multiplexer frame on its way to the wire, rounded up. */
export const PACE_OVERHEAD = 64;
/** More than this received during a marker's round trip, and its answer came back behind the peer's traffic. */
export const PACE_QUIET_RX = 8 * 1024;
/** The peer's bulk is unmarked (an older peer's) past this many of a pacing peer's marker spacings without a probe. */
export const PACE_UNMARKED_SPANS = 4;
/** While the answers come back loaded, markers go every sixteenth of the cap, never closer than this… */
export const PACE_LOADED_SPACING = 4 * 1024;
/** …and the cap is this much more: two ends both sending share their queues, and each needs some of it. */
export const PACE_LOADED_SLACK = 24 * 1024;

/** What the cap lets go next, given that there is data waiting. */
export type PaceNext = { type: "marker" } | { type: "data"; budget: Budget };

/** A path's pace, for logs. */
export interface PaceStats {
  /** What may be in flight now (bytes). */
  cap: number;
  /** Data sent and not yet known to be delivered (bytes). */
  inflight: number;
  /** The delivery rate measured (bytes a second). */
  rate: number | null;
  /** The minimum round trip measured (ms). */
  minRttMs: number | null;
  markers: number;
  draining: boolean;
  startup: boolean;
  /** Deferring to the window: the peer's bulk is an older peer's. */
  deferring: boolean;
}

interface Marker {
  nonce: number;
  offset: number;
  sentMs: number;
  delivered: number;
  deliveredAtMs: number;
  firstSentMs: number;
  appLimited: boolean;
  clean: boolean;
  /** What had been received on the path when it left. */
  rx: number;
}

/** The highest sample of a window: falling in rate, rising in time. */
class MaxFilter {
  private readonly samples: [number, number][] = [];

  push(atMs: number, rate: number): void {
    while (this.samples.length > 0 && this.samples[this.samples.length - 1]![1] <= rate) this.samples.pop();
    this.samples.push([atMs, rate]);
    // The newest always stays: across a quiet spell the last estimate holds.
    while (this.samples.length > 1 && atMs - this.samples[0]![0] > PACE_RATE_WINDOW_MS) this.samples.shift();
  }

  max(): [number, number] | null {
    return this.samples[0] ?? null;
  }
}

/** One path's in-flight cap and what it measures. */
export class Pace {
  private readonly ceiling: number;
  private sent = 0;
  private delivered = 0;
  private unmarked = 0;
  private lastDataMs: number | null = null;
  private deliveredAtMs = 0;
  private firstSentMs = 0;
  private appLimitedUntil = 0;
  private markers: Marker[] = [];
  private readonly rateFilter = new MaxFilter();
  /** The minimum round trip and when it was taken (ms). */
  private minRtt: [number, number] | null = null;
  private draining = false;
  private startup = true;
  private fullRate = 0;
  private flatRounds = 0;
  private nextRound = 0;
  /** Received on the path, whether the last answer came back loaded and the rate the peer's traffic came at then, where the peer's last probe came and how far apart its last two came. */
  private rx = 0;
  private loaded = false;
  private rxRate = 0;
  private rxAtProbe = 0;
  private probeGap = 0;

  /** A fresh path's pace under `ceiling` (the receiver's own limit). */
  constructor(ceiling: number) {
    this.ceiling = Math.max(ceiling, PACE_FLOOR);
  }

  /** A new path's pace that starts from another path's rate (`[when, bytes a second]`); its own round trip is measured by its first marker. */
  static seeded(ceiling: number, rate: [number, number] | null): Pace {
    const pace = new Pace(ceiling);
    pace.seedRate(rate);
    return pace;
  }

  /** Take another path's rate when this one has none of its own. */
  seedRate(rate: [number, number] | null): void {
    if (rate && !this.rateFilter.max()) this.rateFilter.push(rate[0], rate[1]);
  }

  /** The rate estimate and when its sample was taken. */
  rateSample(): [number, number] | null {
    const m = this.rateFilter.max();
    return m ? [m[0], m[1]] : null;
  }

  get inflight(): number {
    return this.sent - this.delivered;
  }

  cap(): number {
    if (this.deferring()) return this.ceiling;
    const own = this.ownCap();
    return this.answersLoaded() ? Math.min(own + PACE_LOADED_SLACK, this.ceiling) : own;
  }

  /** The cap from what this path measured, deferring aside. */
  private ownCap(): number {
    const rate = this.rateFilter.max();
    if (!rate || !this.minRtt) return PACE_FLOOR;
    const bdp = (rate[1] * this.minRtt[0]) / 1e3;
    const gain = this.startup ? PACE_STARTUP_GAIN : PACE_GAIN;
    return Math.min(Math.max(Math.trunc(gain * bdp), PACE_FLOOR), this.ceiling);
  }

  /** Whether the cap defers to the window: the answers come back loaded (or the one awaited will), and the peer's bulk carries no marker where a pacing peer at its rate would have put several. */
  /** The last answer came back loaded, or the one still awaited will (as much came in since it left). */
  private answersLoaded(): boolean {
    const first = this.markers[0];
    return this.loaded || (first !== undefined && this.rx - first.rx > PACE_QUIET_RX);
  }

  private deferring(): boolean {
    if (!this.answersLoaded()) return false;
    const rttMs = this.minRtt ? this.minRtt[0] : 0;
    const peerCap = Math.max(Math.trunc((PACE_STARTUP_GAIN * this.rxRate * rttMs) / 1e3), PACE_FLOOR);
    const spacing = Math.max(PACE_MIN_SPACING, Math.trunc(peerCap / 4));
    const unmarked = Math.max(this.rx - this.rxAtProbe, this.probeGap);
    return unmarked > PACE_UNMARKED_SPANS * spacing;
  }

  stats(): PaceStats {
    const rate = this.rateFilter.max();
    return {
      cap: this.cap(),
      inflight: this.inflight,
      rate: rate ? rate[1] : null,
      minRttMs: this.minRtt ? this.minRtt[0] : null,
      markers: this.markers.length,
      draining: this.draining,
      startup: this.startup,
      deferring: this.deferring(),
    };
  }

  /** With data waiting to go: a marker first, or the data's budget. */
  next(nowMs: number): PaceNext {
    this.fold(nowMs);
    const cleanOut = this.markers.some((m) => m.clean);
    if (this.inflight <= PACE_SMALL) {
      // A new flight, or a drain's end: time the (all but) empty path first when the minimum is due.
      this.draining = false;
      const due = this.minRtt === null || nowMs - this.minRtt[1] >= PACE_MIN_RTT_REFRESH_MS;
      if (due && !cleanOut) return { type: "marker" };
    } else if (this.draining || (!cleanOut && !this.loaded && this.minRtt !== null && nowMs - this.minRtt[1] > PACE_MIN_RTT_WINDOW_MS)) {
      // Nothing new until no more than PACE_SMALL is out (the tail is marked when the sender finds itself held). Not while the answers come back loaded.
      this.draining = true;
      return { type: "data", budget: BUDGET_NONE };
    }
    // Spaced by the cap this path measured, deferring or not; closer while the answers come back loaded.
    const spacing = this.answersLoaded() ? Math.max(PACE_LOADED_SPACING, Math.trunc(this.ownCap() / 16)) : Math.max(PACE_MIN_SPACING, Math.trunc(this.ownCap() / 4));
    if (this.unmarked >= spacing) return { type: "marker" };
    const cap = this.cap();
    const used = this.inflight + PACE_OVERHEAD;
    return { type: "data", budget: { interactive: Math.max(0, cap + PACE_HEADROOM - used), bulk: Math.max(0, cap - used) } };
  }

  /** The path carries new frames again after another did: what this pace measures next starts out app-limited. */
  resume(): void {
    this.appLimitedUntil = Math.max(this.sent, 1);
  }

  /** `len` bytes came in on the path (anything the peer sent this way). */
  received(len: number): void {
    this.rx += len;
  }

  /** A probe of the peer's came in on the path (a pacing peer's marker, or an older one's liveness probe). */
  peerProbe(): void {
    this.probeGap = this.rx - this.rxAtProbe;
    this.rxAtProbe = this.rx;
  }

  /** `len` bytes of stream data (a sequenced envelope, sealed) left on the path. */
  dataSent(len: number, nowMs: number): void {
    if (this.inflight === 0) {
      // A flight starts its own sampling interval (BBR).
      this.deliveredAtMs = nowMs;
      this.firstSentMs = nowMs;
    }
    this.sent += len;
    this.unmarked += len;
    this.lastDataMs = nowMs;
  }

  /** A marker with `nonce` left on the path. */
  markerSent(nonce: number, nowMs: number): void {
    this.markers.push({
      nonce,
      offset: this.sent,
      sentMs: nowMs,
      delivered: this.delivered,
      deliveredAtMs: this.deliveredAtMs,
      firstSentMs: this.firstSentMs,
      appLimited: this.appLimitedUntil !== 0,
      clean: this.inflight <= PACE_SMALL,
      rx: this.rx,
    });
    this.unmarked = 0;
    if (this.markers.length > PACE_MAX_MARKERS) this.markers.shift();
  }

  /** Sending stopped: `held` with data waiting to go, `idle` with none queued at all (data the peer's stream window holds back is not idle). Whether a marker should follow what went. */
  stopped(held: boolean, idle: boolean): boolean {
    if (idle && this.inflight < this.cap()) this.appLimitedUntil = Math.max(this.sent, 1);
    return this.unmarked > 0 && (held || this.unmarked > PACE_SMALL);
  }

  /** The answer to a probe with `nonce` came on this path: whether it was one of this pace's markers. */
  answered(nonce: number, nowMs: number): boolean {
    const i = this.markers.findIndex((m) => m.nonce === nonce);
    if (i < 0) return false;
    // Those before it were read before it: its answer covers them.
    const m = this.markers[i]!;
    this.markers = this.markers.slice(i + 1);
    const rttMs = Math.max(0, nowMs - m.sentMs);
    // What came in meanwhile: the answer path loaded, and at what rate.
    const rx = this.rx - m.rx;
    this.loaded = rx > PACE_QUIET_RX;
    if (rttMs > 0) this.rxRate = (rx * 1e3) / rttMs;
    if (!this.loaded) {
      this.noteRtt(rttMs, m.clean, nowMs);
    } else if (this.minRtt !== null) {
      // Loaded: the whole round trip may lower the minimum, never refresh it.
      if (rttMs <= this.minRtt[0]) this.noteRtt(rttMs, false, nowMs);
    } else if (this.rxRate > 0) {
      // Loaded from the first marker on: less what a pacing peer keeps queued (its floor), it seeds the minimum.
      const leftMs = Math.floor(rttMs - (PACE_FLOOR * 1e3) / this.rxRate);
      if (leftMs >= 1) this.noteRtt(leftMs, false, nowMs);
    }
    if (m.offset > this.delivered) {
      this.delivered = m.offset;
      this.deliveredAtMs = nowMs;
      this.firstSentMs = m.sentMs;
    }
    const data = m.offset - m.delivered;
    const interval = Math.max(Math.max(0, m.sentMs - m.firstSentMs), Math.max(0, nowMs - m.deliveredAtMs));
    if (data > 0 && interval > 0) {
      const rate = (data * 1e3) / interval;
      const max = this.rateFilter.max();
      if (!m.appLimited || max === null || rate >= max[1]) this.rateFilter.push(nowMs, rate);
    }
    if (m.delivered >= this.nextRound) {
      // A round trip's worth delivered since the last round ended (BBR's rounds).
      this.nextRound = this.delivered;
      if (this.startup && !m.appLimited) {
        const rate = this.rateFilter.max()?.[1] ?? 0;
        if (rate >= this.fullRate * PACE_STARTUP_GROWTH) {
          this.fullRate = rate;
          this.flatRounds = 0;
        } else {
          this.flatRounds += 1;
          this.startup = this.flatRounds < PACE_STARTUP_ROUNDS;
        }
      }
    }
    if (this.appLimitedUntil !== 0 && this.delivered >= this.appLimitedUntil) this.appLimitedUntil = 0;
    return true;
  }

  private noteRtt(rttMs: number, clean: boolean, nowMs: number): void {
    const take = this.minRtt === null || rttMs <= this.minRtt[0] || (clean && nowMs - this.minRtt[1] >= PACE_MIN_RTT_REFRESH_MS);
    if (take) this.minRtt = [rttMs, nowMs];
  }

  /** A small tail nobody marked, sent a quiet round trip ago with nothing else out, counts as delivered. */
  private fold(nowMs: number): void {
    const quiet = this.minRtt ? this.minRtt[0] : PACE_QUIET_MS;
    if (
      this.markers.length === 0 &&
      this.unmarked > 0 &&
      this.unmarked <= PACE_SMALL &&
      this.inflight === this.unmarked &&
      this.lastDataMs !== null &&
      nowMs - this.lastDataMs >= quiet
    ) {
      this.delivered = this.sent;
      this.unmarked = 0;
    }
  }
}
