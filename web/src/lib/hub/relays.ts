/*
 * The relays this page may use (`docs/relay-and-plans.md` §2.6–§2.8): the
 * Hub's list, this device's round trip to each, which one a new channel goes
 * through, tickets, and the connections themselves.
 *
 * **Automatic (the default, no prompt).** For each new channel the pool
 * scores every relay that is `up` — this device's round trip plus the one
 * the machine reported (`GET /v1/relays` `agents`), half again when its load
 * is over 0.8 — and takes the lowest. While the relay it used last still
 * works, it moves only for one at least 20% and 15 ms better; it never moves
 * a live channel. A relay the machine is signed in on (presence `via`) wins
 * unless another is clearly better: the Hub keeps one relay in the account's
 * set (two with the standby preference) and the machine is on it, so a page
 * goes where the machine already is and need not wait for it to move. With
 * the account's set full (`relay.max`), only members
 * (and relays that may replace a member idle for a week) are candidates. The
 * members the Hub put in the set itself, for the agents (§2.7), have
 * `last_used_ms` 0 — no device used them — so they count as idle: they never
 * keep a page from the relay it would choose.
 *
 * **Manual.** The user may pick a relay; the choice is this device's alone
 * (`localStorage` `rh.relays.manual`) and holds until they go back to
 * automatic, or the relay leaves the list, or it fails (down, three failed
 * sign-ins or channels in a row): then the pool goes back to automatic and
 * says why in one quiet line (`notice`), no dialog.
 *
 * **Measuring.** Four `GET /v1/ping` per relay, the first dropped (DNS,
 * TLS), the median of the rest: at start, when the network changes, when the
 * picker opens, and every 15 minutes; a connected relay's `ping`/`pong` is
 * used as it comes.
 *
 * **Tickets** (`POST /v1/relays/tickets`) last 15 minutes; open connections
 * are renewed every 10 minutes. **Connections** are made when a channel
 * needs one and let go two minutes after their last channel closed — a phone
 * keeps only its Hub connection up.
 *
 * **Failures** bench a relay for automatic choices after three in a row (a
 * failed sign-in counts once), for {@link FAILURE_FORGET_MS}: then it is a
 * candidate again — a relay that failed while the phone slept or was offline
 * is not kept from the page for good. A channel with nowhere else to go
 * (the hub carries none) may try benched relays anyway
 * (`choose(…, {benched: true})`). {@link RelayPool.wake}, when the page comes
 * back, forgives them all, fetches the list again if it is old, and checks
 * the connections (a dead idle one is let go).
 */
import type { HubApi } from "./api";
import { RelayLink } from "./relay-link";

export interface RelayEntry {
  id: string;
  /** The display name's i18n key, e.g. `singapore`. */
  city: string;
  region: string;
  origin: string;
  state: "up" | "draining" | string;
  load: number | null;
}

export interface RelayDirectory {
  relays: RelayEntry[];
  /** `relay.max`: relays the account may use at once. */
  limit: number;
  /** The relays the account uses now, with when each last got a ticket. */
  accountSet: { id: string; lastUsedMs: number }[];
  /** Relays outside the set the account may use for now (its set does not work). */
  emergency: string[];
  /** Each machine's round trip to each relay (ms), as it reported. */
  agents: Record<string, Record<string, number>>;
  /** The account used up its free transfer: relayed connections are slowed. */
  throttled: boolean;
}

export interface RelayChoice {
  relay: RelayEntry;
  mode: "auto" | "manual";
  /** Outside the account's set while none of its members works. */
  emergency: boolean;
  /** This device's round trip plus the machine's, ms (`null`: not measured yet). */
  score: number | null;
}

/** Why the page went back to choosing automatically. */
export interface RelayNotice {
  relay: string;
  city: string | null;
  why: "gone" | "failed";
}

export const MANUAL_KEY = "rh.relays.manual";
const IDLE_REPLACE_MS = 7 * 24 * 60 * 60 * 1000;
const MEASURE_EVERY_MS = 15 * 60 * 1000;
const RENEW_EVERY_MS = 10 * 60 * 1000;
const IDLE_CLOSE_MS = 2 * 60 * 1000;
const FAILURES_TO_GIVE_UP = 3;
/** How long three failures in a row keep a relay from automatic choices. */
export const FAILURE_FORGET_MS = 60_000;
/** A list older than this is fetched again when the page comes back ({@link RelayPool.wake}). */
const WAKE_REFRESH_MS = 60_000;
/** …and an open connection's ticket with less than this left is renewed then. */
const TICKET_RENEW_WITHIN_MS = 5 * 60 * 1000;
/** A manual relay unreachable this long gives way to automatic. */
const UNREACHABLE_GIVE_UP_MS = 30_000;
const MANUAL_CHECK_EVERY_MS = 10_000;
/** A ticket is fetched again with less than this left. */
const TICKET_MARGIN_MS = 60_000;

interface Ticket {
  ticket: string;
  origin: string;
  expiresMs: number;
  emergency: boolean;
  throttled: boolean;
}

/** A refusal of a ticket (`relay.max` full, the relay down…). */
export class RelayRefused extends Error {
  constructor(
    readonly relay: string,
    readonly code: string,
    readonly accountSet: string[] = [],
  ) {
    super(`no ticket for ${relay}: ${code}`);
    this.name = "RelayRefused";
  }
}

export interface RelayPoolOptions {
  api: HubApi;
  account: { account: string; deviceId: string; session: { token: string }; keys: { sign: { privateKey: CryptoKey } } };
  fetchImpl?: (input: string, init?: RequestInit) => Promise<Response>;
  now?: () => number;
  storage?: Pick<Storage, "getItem" | "setItem" | "removeItem"> | null;
  makeLink?: (relay: RelayEntry, ticket: () => Promise<string>) => RelayLink;
}

function defaultStorage(): RelayPoolOptions["storage"] {
  try {
    const s = globalThis.localStorage;
    return s && typeof s.getItem === "function" ? s : null;
  } catch {
    return null;
  }
}

/** `wss://x` → `https://x`. */
export function httpOf(origin: string): string {
  if (origin.startsWith("ws://")) return `http://${origin.slice(5)}`;
  if (origin.startsWith("wss://")) return `https://${origin.slice(6)}`;
  return origin;
}

/** The median of the last `n - 1` of `samples` (the first is dropped: DNS, TLS). */
export function medianAfterFirst(samples: number[]): number | null {
  const rest = samples.slice(1).sort((a, b) => a - b);
  return rest[Math.floor(rest.length / 2)] ?? null;
}

/** Whether a relay scored `a` is clearly better than one scored `b`: 20% and 15 ms. */
function clearlyBetter(a: number, b: number): boolean {
  return a < b * 0.8 && b - a >= 15;
}

/**
 * The automatic choice (§2.6), pure: candidates, scores, hysteresis.
 * `current` is the relay the last channel went through; `on`, the relays the
 * machine is signed in on now (presence `via`).
 */
export function chooseAuto(input: {
  directory: RelayDirectory;
  machine: string | null;
  deviceRtt: (id: string) => number | null;
  usable: (id: string) => boolean;
  current: string | null;
  now: number;
  on?: ReadonlySet<string>;
}): RelayChoice | null {
  const { directory, machine, deviceRtt, usable, current, now, on } = input;
  const up = directory.relays.filter((r) => r.state === "up" && usable(r.id));
  if (up.length === 0) return null;
  const members = directory.accountSet.map((m) => m.id);
  const full = members.length >= directory.limit;
  const replaceable = directory.accountSet.some((m) => now - m.lastUsedMs >= IDLE_REPLACE_MS);
  const membersUp = up.filter((r) => members.includes(r.id));
  let candidates = up;
  let emergency = false;
  if (full && !replaceable) {
    if (membersUp.length > 0) candidates = membersUp;
    else emergency = true; // none of the set works: the Hub gives an emergency ticket
  }
  const agent = machine ? directory.agents[machine] : undefined;
  const score = (r: RelayEntry): number | null => {
    const mine = deviceRtt(r.id);
    if (mine === null) return null;
    let s = mine + (agent?.[r.id] ?? 0);
    if ((r.load ?? 0) > 0.8) s *= 1.5;
    return s;
  };
  const scored = candidates.map((r) => ({ r, s: score(r) }));
  // Measured ones first, lowest score; unmeasured ones keep the list's order.
  scored.sort((a, b) => (a.s ?? Number.POSITIVE_INFINITY) - (b.s ?? Number.POSITIVE_INFINITY));
  let best = scored[0];
  if (!best) return null;
  const held = scored.find((x) => x.r.id === current);
  if (held && best.r.id !== held.r.id && held.s !== null && best.s !== null) {
    if (!clearlyBetter(best.s, held.s)) best = held;
  }
  // A relay the machine is signed in on wins unless another is clearly better
  // (both measured): one it is not on means waiting for it to sign in there,
  // and one more relay in the account's set (§2.7: the Hub keeps one).
  const attached = on ? scored.find((x) => on.has(x.r.id)) : undefined;
  if (attached && on && !on.has(best.r.id)) {
    if (best.s === null || attached.s === null || !clearlyBetter(best.s, attached.s)) best = attached;
  }
  return { relay: best.r, mode: "auto", emergency: emergency && !members.includes(best.r.id), score: best.s };
}

export class RelayPool {
  directory: RelayDirectory | null = null;
  /** The notice after a manual choice fell back to automatic. */
  notice: RelayNotice | null = null;
  /** The relay the last channel went through. */
  current: string | null = null;
  private readonly rtts = new Map<string, number | null>();
  private readonly failures = new Map<string, { count: number; why: string; atMs: number }>();
  /** When the list was last fetched (the page's clock). */
  private loadedAt: number | null = null;
  private readonly tickets = new Map<string, Ticket>();
  private readonly links = new Map<string, RelayLink>();
  private readonly idleSince = new Map<string, number>();
  private readonly listeners = new Set<() => void>();
  private readonly timers: ReturnType<typeof setInterval>[] = [];
  private loading: Promise<void> | null = null;
  private measuring: Promise<void> | null = null;
  private readonly onlineHandler = () => void this.measure();
  /** Since when the manual relay has not answered its ping. */
  private unreachableSince: number | null = null;
  /** Bumped on every change, for `useSyncExternalStore`. */
  version = 0;

  constructor(private readonly opts: RelayPoolOptions) {}

  private now(): number {
    return (this.opts.now ?? Date.now)();
  }

  private storage() {
    return this.opts.storage === undefined ? defaultStorage() : this.opts.storage;
  }

  onChange(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private changed(): void {
    this.version++;
    for (const fn of [...this.listeners]) fn();
  }

  /** Whether a relay this page is connected to has `machine` signed in (relay-and-plans §2.9). */
  online(machine: string): boolean {
    return this.openLinks().some((l) => l.presence.get(machine) === "online");
  }

  start(): void {
    void this.refresh().then(() => this.measure());
    this.timers.push(setInterval(() => void this.refresh().then(() => this.measure()), MEASURE_EVERY_MS));
    this.timers.push(setInterval(() => void this.renewOpen(), RENEW_EVERY_MS));
    this.timers.push(setInterval(() => this.sweepIdle(), 30_000));
    this.timers.push(setInterval(() => void this.checkManualReachable(), MANUAL_CHECK_EVERY_MS));
    globalThis.addEventListener?.("online", this.onlineHandler);
  }

  stop(): void {
    for (const t of this.timers.splice(0)) clearInterval(t);
    globalThis.removeEventListener?.("online", this.onlineHandler);
    for (const link of this.links.values()) link.close();
    this.links.clear();
  }

  /** The Hub's list (`GET /v1/relays`). */
  refresh(): Promise<void> {
    this.loading ??= (async () => {
      try {
        this.directory = await this.opts.api.relays(this.opts.account.session.token);
        this.loadedAt = this.now();
        this.checkManual();
        this.changed();
      } finally {
        this.loading = null;
      }
    })();
    return this.loading;
  }

  /**
   * Loaded at least once (now, if it already was) — and, with `maxAgeMs`,
   * fetched again if it is older than that (or being fetched now). A list
   * that cannot be had now leaves the one there was.
   */
  async ready(maxAgeMs?: number): Promise<RelayDirectory | null> {
    if (!this.directory || this.loading || (maxAgeMs !== undefined && this.ageMs() > maxAgeMs)) {
      try {
        await this.refresh();
      } catch {
        // no list: no relay
      }
    }
    return this.directory;
  }

  /** How old the list is (ms; infinite before the first). */
  ageMs(): number {
    return this.loadedAt === null ? Number.POSITIVE_INFINITY : this.now() - this.loadedAt;
  }

  /**
   * The page is back (in view, online again, shown from the back-forward
   * cache): what failed before says little about now — every failure is
   * forgiven; a list older than a minute is fetched again (and the relays
   * timed); each connection is checked (`RelayLink.wake`).
   */
  wake(): void {
    if (this.failures.size > 0) {
      this.failures.clear();
      this.changed();
    }
    if (this.ageMs() > WAKE_REFRESH_MS) void this.refresh().then(() => this.measure(), () => undefined);
    for (const link of [...this.links.values()]) link.wake();
    // Tickets of open connections the renewal (its timer held meanwhile) did not get to.
    void this.renewOpen(TICKET_RENEW_WITHIN_MS);
  }

  /** This device's round trip to `id` (ms): a connected relay's ping, else the last measurement. */
  rtt(id: string): number | null {
    const link = this.links.get(id);
    if (link?.state === "open" && link.rttMs !== null) return link.rttMs;
    return this.rtts.get(id) ?? null;
  }

  /** Time every listed relay (`GET /v1/ping` ×4). */
  measure(): Promise<void> {
    const dir = this.directory;
    if (!dir) return Promise.resolve();
    this.measuring ??= (async () => {
      try {
        await Promise.all(dir.relays.map(async (r) => this.rtts.set(r.id, await this.ping(r.origin))));
        this.changed();
      } finally {
        this.measuring = null;
      }
    })();
    return this.measuring;
  }

  private async ping(origin: string): Promise<number | null> {
    const fetchImpl = this.opts.fetchImpl ?? ((input: string, init?: RequestInit) => globalThis.fetch(input, init));
    const url = `${httpOf(origin)}/v1/ping`;
    const samples: number[] = [];
    for (let i = 0; i < 4; i++) {
      const t0 = performance.now();
      try {
        const res = await fetchImpl(url, { cache: "no-store", mode: "cors", credentials: "omit" });
        if (!res.ok) return null;
      } catch {
        return null;
      }
      samples.push(performance.now() - t0);
    }
    return medianAfterFirst(samples);
  }

  /** The manual choice of this device (`null`: automatic). */
  get manual(): string | null {
    try {
      return this.storage()?.getItem(MANUAL_KEY) ?? null;
    } catch {
      return null;
    }
  }

  /** Choose a relay by hand (or go back to automatic with `null`). */
  setManual(id: string | null): void {
    try {
      if (id) this.storage()?.setItem(MANUAL_KEY, id);
      else this.storage()?.removeItem(MANUAL_KEY);
    } catch {
      // nowhere to keep it
    }
    this.notice = null;
    if (id) this.failures.delete(id);
    this.changed();
  }

  /** The manual relay, pinged: unreachable for 30 s in a row, it gives way to automatic. */
  async checkManualReachable(): Promise<void> {
    const manual = this.manual;
    const entry = manual ? this.directory?.relays.find((r) => r.id === manual) : undefined;
    if (!manual || !entry) {
      this.unreachableSince = null;
      return;
    }
    const rtt = await this.ping(entry.origin);
    if (rtt !== null) {
      this.unreachableSince = null;
      this.rtts.set(manual, rtt);
      return;
    }
    const now = this.now();
    this.unreachableSince ??= now;
    if (now - this.unreachableSince >= UNREACHABLE_GIVE_UP_MS) {
      this.unreachableSince = null;
      this.giveUpManual(manual, entry.city, "failed");
      this.changed();
    }
  }

  private giveUpManual(relay: string, city: string | null, why: RelayNotice["why"]): void {
    this.notice = { relay, city, why };
    try {
      this.storage()?.removeItem(MANUAL_KEY);
    } catch {
      // nowhere to keep it
    }
  }

  /** A manual choice that left the list, or failed, falls back to automatic, saying why. */
  private checkManual(): void {
    const manual = this.manual;
    if (!manual || !this.directory) return;
    const entry = this.directory.relays.find((r) => r.id === manual);
    const failed = (this.failures.get(manual)?.count ?? 0) >= FAILURES_TO_GIVE_UP;
    if (!entry || entry.state !== "up" || failed) this.giveUpManual(manual, entry?.city ?? null, entry ? "failed" : "gone");
  }

  /** Whether `id` can be used: not given up on after failures (three in a row, within {@link FAILURE_FORGET_MS}). */
  usable(id: string): boolean {
    const f = this.failures.get(id);
    return !f || f.count < FAILURES_TO_GIVE_UP || this.now() - f.atMs >= FAILURE_FORGET_MS;
  }

  /** The relays given up on for now ({@link usable}). */
  benched(): string[] {
    return [...this.failures.keys()].filter((id) => !this.usable(id));
  }

  /** What went wrong with `id` last, if anything (the picker's "last failure"). */
  failure(id: string): string | null {
    return this.failures.get(id)?.why ?? null;
  }

  noteFailure(id: string, why: string): void {
    const f = this.failures.get(id) ?? { count: 0, why, atMs: 0 };
    f.count += 1;
    f.why = why;
    f.atMs = this.now();
    this.failures.set(id, f);
    this.checkManual();
    this.changed();
  }

  noteSuccess(id: string): void {
    this.failures.delete(id);
    this.current = id;
  }

  /**
   * The relay for a new channel to `machine` (§2.6): the manual one while it
   * holds, else the automatic choice — the relays the machine is `on` first;
   * `exclude`d relays (one draining, one that just failed) are never chosen.
   * `benched`: relays given up on after failures are candidates too (a
   * channel with nowhere else to go).
   */
  choose(machine: string | null, exclude: ReadonlySet<string> = new Set(), on?: ReadonlySet<string>, opts: { benched?: boolean } = {}): RelayChoice | null {
    const dir = this.directory;
    if (!dir) return null;
    this.checkManual();
    const manual = this.manual;
    if (manual && !exclude.has(manual)) {
      const entry = dir.relays.find((r) => r.id === manual && r.state === "up");
      const members = dir.accountSet.map((m) => m.id);
      const allowed =
        members.includes(manual) ||
        members.length < dir.limit ||
        dir.accountSet.some((m) => this.now() - m.lastUsedMs >= IDLE_REPLACE_MS);
      if (entry && allowed && this.usable(manual)) {
        const mine = this.rtt(manual);
        return { relay: entry, mode: "manual", emergency: false, score: mine === null ? null : mine + (machine ? (dir.agents[machine]?.[manual] ?? 0) : 0) };
      }
    }
    return chooseAuto({
      directory: dir,
      machine,
      deviceRtt: (id) => this.rtt(id),
      usable: (id) => (opts.benched || this.usable(id)) && !exclude.has(id),
      on,
      current: this.current,
      now: this.now(),
    });
  }

  /** A ticket for `id` (cached while it has a minute left). */
  async ticket(id: string): Promise<Ticket> {
    const held = this.tickets.get(id);
    if (held && held.expiresMs - TICKET_MARGIN_MS > this.now()) return held;
    const got = await this.opts.api.relayTickets(this.opts.account.session.token, [id]);
    for (const t of got.tickets) {
      this.tickets.set(t.relay, { ticket: t.ticket, origin: t.origin, expiresMs: t.expiresMs, emergency: t.emergency, throttled: t.throttled });
    }
    const mine = this.tickets.get(id);
    if (mine) {
      // The set may have changed (this relay joined it): the list says so.
      void this.refresh().catch(() => undefined);
      return mine;
    }
    const refusal = got.errors.find((e) => e.relay === id);
    throw new RelayRefused(id, refusal?.error ?? "unknown", refusal?.accountSet ?? []);
  }

  /** Whether the ticket for `id` is an emergency one (outside the set). */
  emergency(id: string): boolean {
    return this.tickets.get(id)?.emergency ?? false;
  }

  /** The connection to `id`, signed in (made if there is none). */
  async link(entry: RelayEntry): Promise<RelayLink> {
    let link = this.links.get(entry.id);
    if (!link || link.state === "closed") {
      const ticket = async () => (await this.ticket(entry.id)).ticket;
      link =
        this.opts.makeLink?.(entry, ticket) ??
        new RelayLink({
          relay: entry.id,
          origin: entry.origin,
          account: this.opts.account.account,
          deviceId: this.opts.account.deviceId,
          signKey: this.opts.account.keys.sign.privateKey,
          ticket,
        });
      this.links.set(entry.id, link);
      const opened = link;
      let signedIn = false;
      opened.onState((state) => {
        if (state === "open") signedIn = true;
        // A connection that went away on its own (not closed by this page) is a failure; one that never got in is counted below, once.
        if (state === "closed" && signedIn && opened.lastClose !== 1000 && this.links.get(entry.id) === opened) {
          this.noteFailure(entry.id, opened.lastError ?? `closed (${opened.lastClose ?? "?"})`);
        }
        this.changed();
      });
      link.onPresence(() => this.changed());
    }
    try {
      await link.open();
    } catch (error) {
      this.noteFailure(entry.id, error instanceof Error ? error.message : String(error));
      throw error;
    }
    this.idleSince.delete(entry.id);
    return link;
  }

  /** The open connections (for presence: a machine on any of them is online). */
  openLinks(): RelayLink[] {
    return [...this.links.values()].filter((l) => l.state === "open");
  }

  /** Renew the tickets of open connections (only those with less than `within` left, if given). */
  private async renewOpen(within?: number): Promise<void> {
    for (const link of this.openLinks()) {
      const held = this.tickets.get(link.relay);
      if (within !== undefined && held && held.expiresMs - this.now() > within) continue;
      this.tickets.delete(link.relay);
      try {
        link.renew((await this.ticket(link.relay)).ticket);
      } catch {
        // the relay closes the connection when its ticket runs out
      }
    }
  }

  /** Let go of connections idle for two minutes. */
  private sweepIdle(): void {
    const now = this.now();
    for (const [id, link] of this.links) {
      if (link.state !== "open" || link.channels > 0) {
        this.idleSince.delete(id);
        continue;
      }
      const since = this.idleSince.get(id) ?? now;
      this.idleSince.set(id, since);
      if (now - since >= IDLE_CLOSE_MS) {
        link.close();
        this.links.delete(id);
        this.idleSince.delete(id);
      }
    }
  }
}
