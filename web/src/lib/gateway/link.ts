/*
 * The local gateway's socket (`GET /api/agg/ws`, `docs/local-aggregator.md`
 * §6.3, S1-7): the account's other devices as the local install holds them,
 * for the local app — which holds no key and never reaches the hub.
 *
 * One socket per page, over the page's own transport (a one-shot `ws` ticket
 * where a login is required). It says which devices there are (`dir`, the
 * whole list on every change) and carries their copies by name: a
 * subscription gets the copy as it stands (`reset`), then each change as the
 * device wrote it, or `missing` while there is none. Read-only: there is
 * nothing to send but subscriptions.
 *
 * It reconnects with backoff and subscribes again, each subscription from
 * where its page stands (`from`: the copy's epoch and seq — the install then
 * sends no snapshot when nothing moved). A subscription the install dropped
 * because the page fell behind (`overrun`) or asked too fast (`busy`) is made
 * again after a moment; any other `copy_error` ends it (the install has let go
 * of it too). `bye` — the local account may no longer see other devices, its
 * login was signed out, or local sign-in went off — ends the socket for good.
 *
 * Plain JSON over a WebSocket: nothing here needs a secure context, so a
 * phone at the computer's LAN address works the same.
 */
import { connLog, errorFields } from "@/lib/connection-log";
import { createStore, type Store, type WritableStore } from "@/lib/machines/store";
import type { SocketLike } from "@/lib/transport/types";

/** A device the gateway shows (one of its `dir` entries). */
export interface GatewayDevice {
  readonly id: string;
  /** Its own name (its status's), or `null`: it said none. */
  readonly name: string | null;
  readonly state: "online" | "offline" | "unknown";
  /** Since when (ms) it has been in `state`. */
  readonly sinceMs: number | null;
  /** How current its copy is: the newest time it signed its index (ms). */
  readonly copyAsOfMs: number | null;
  readonly os: string | null;
  readonly agentVersion: string | null;
}

/** A copy by name, as the gateway takes it. */
export type GatewayName = { kind: "index" } | { kind: "status" } | { kind: "session"; source: string; id: string };

/** What a subscription hears. */
export interface GatewayCopyEvents {
  /** The copy (`reset`: rebuilt from `bodies`; else `bodies` go on top). */
  onCopy(change: { reset: boolean; bodies: unknown[]; headSeq: number; epoch: number; headTimeMs: number }): void;
  /** The device has no such copy (yet); it may come later. */
  onMissing?(): void;
  /** It ended: the device is gone, the copy too large, or the name refused. */
  onEnd?(code: string): void;
}

/** How the gateway was left for good. */
export interface GatewayBye {
  code: string;
  message: string;
}

export interface GatewayLinkOptions {
  /** The address to open (`transport.socketUrl("/api/agg/ws")`). */
  socketUrl: () => Promise<string>;
  openSocket: (url: string) => SocketLike;
  /** Reconnect delays (ms), the last repeated; tests pass short ones. */
  backoffMs?: readonly number[];
  /** How often the page pings while open (ms). */
  pingMs?: number;
  /** How long without any inbound frame before a silent socket is dropped and reconnected (ms). */
  silenceMs?: number;
  /** Clock for silence detection (tests). */
  now?: () => number;
  /**
   * Every third attempt in a row that failed: the gateway may be refusing
   * this page (its access was taken away between two looks) — the caller
   * looks again at whether it should be open at all.
   */
  onTrouble?: () => void;
  /** How long (ms) to wait before asking again for a subscription dropped as `overrun` or `busy`. */
  againMs?: number;
}

/** Where a page's copy stands: the epoch its stream is named under, and the seq of its next record. */
export interface GatewayFrom {
  epoch: number;
  seq: number;
}

const BACKOFF_MS = [500, 1000, 2000, 4000, 8000];
const PING_MS = 25_000;
const SILENCE_MS = 60_000;
const AGAIN_MS = 1000;
const OPEN = 1;

interface Sub {
  machine: string;
  name: GatewayName;
  /** Where the page stands, for a resubscription. */
  from: () => GatewayFrom | null;
  events: GatewayCopyEvents;
  /** A resubscription waiting (after `overrun` or `busy`). */
  again?: ReturnType<typeof setTimeout>;
}

function wire(s: number, sub: Sub): string {
  const from = sub.from();
  return JSON.stringify({ t: "sub", s, machine: sub.machine, name: sub.name, ...(from !== null ? { from: from.seq, fromEpoch: from.epoch } : {}) });
}

function sameDevices(a: readonly GatewayDevice[] | null, b: readonly GatewayDevice[] | null): boolean {
  if (a === b) return true;
  if (!a || !b || a.length !== b.length) return false;
  return a.every((d, i) => {
    const e = b[i]!;
    return (
      d.id === e.id &&
      d.name === e.name &&
      d.state === e.state &&
      d.sinceMs === e.sinceMs &&
      d.copyAsOfMs === e.copyAsOfMs &&
      d.os === e.os &&
      d.agentVersion === e.agentVersion
    );
  });
}

const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
const str = (v: unknown): string | null => (typeof v === "string" && v ? v : null);

/** The `dir` entries the page can use; anything else is skipped. */
export function parseDir(machines: unknown): GatewayDevice[] {
  if (!Array.isArray(machines)) return [];
  const out: GatewayDevice[] = [];
  for (const m of machines) {
    if (!m || typeof m !== "object") continue;
    const r = m as Record<string, unknown>;
    const id = str(r.id);
    if (!id) continue;
    const state = r.state === "online" || r.state === "offline" ? r.state : "unknown";
    out.push({ id, name: str(r.name), state, sinceMs: num(r.sinceMs), copyAsOfMs: num(r.copyAsOfMs), os: str(r.os), agentVersion: str(r.agentVersion) });
  }
  return out;
}

export class GatewayLink {
  /** The devices, as the gateway last listed them (`null` until it has). */
  readonly dir: WritableStore<readonly GatewayDevice[] | null> = createStore<readonly GatewayDevice[] | null>(null, sameDevices);
  private readonly openStore: WritableStore<boolean> = createStore(false);
  private socket: SocketLike | null = null;
  private readonly subs = new Map<number, Sub>();
  private next = 1;
  private failures = 0;
  private connectGen = 0;
  private connecting = false;
  private lastHeard = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private ping: ReturnType<typeof setInterval> | undefined;
  private stopped = false;
  private byeInfo: GatewayBye | null = null;
  private readonly byeListeners = new Set<(bye: GatewayBye) => void>();

  constructor(private readonly opts: GatewayLinkOptions) {}

  private now(): number {
    return (this.opts.now ?? Date.now)();
  }

  /** Whether the socket is open now, as it changes. */
  get open(): Store<boolean> {
    return this.openStore;
  }

  /** Why it ended for good, if it did. */
  get bye(): GatewayBye | null {
    return this.byeInfo;
  }

  /** Hear when the gateway sends this page away for good. */
  onBye(fn: (bye: GatewayBye) => void): () => void {
    this.byeListeners.add(fn);
    return () => this.byeListeners.delete(fn);
  }

  start(): void {
    if (this.stopped || this.socket || this.connecting || this.timer !== undefined) return;
    void this.connect();
  }

  /** Follow `name` of `machine`; the returned function stops it. */
  sub(machine: string, name: GatewayName, from: () => GatewayFrom | null, events: GatewayCopyEvents): () => void {
    const s = this.next++;
    const sub: Sub = { machine, name, from, events };
    this.subs.set(s, sub);
    this.send(wire(s, sub));
    return () => {
      clearTimeout(sub.again);
      if (!this.subs.delete(s)) return;
      this.send(JSON.stringify({ t: "unsub", s }));
    };
  }

  /** The devices listed now include `machine`. */
  lists(machine: string): boolean {
    return (this.dir.get() ?? []).some((d) => d.id === machine);
  }

  /** Stop: close the socket, never reconnect. */
  close(): void {
    this.stopped = true;
    this.connectGen += 1;
    this.connecting = false;
    clearTimeout(this.timer);
    this.timer = undefined;
    this.drop();
    for (const sub of this.subs.values()) clearTimeout(sub.again);
    this.subs.clear();
  }

  private send(text: string): void {
    if (this.socket?.readyState === OPEN) this.socket.send(text);
  }

  private drop(): void {
    clearInterval(this.ping);
    this.ping = undefined;
    const socket = this.socket;
    this.socket = null;
    if (socket) {
      socket.onopen = socket.onmessage = socket.onclose = null;
      try {
        socket.close();
      } catch {
        // already gone
      }
    }
    this.openStore.set(false);
  }

  private async connect(): Promise<void> {
    const gen = ++this.connectGen;
    this.connecting = true;
    const dialledAt = performance.now();
    connLog.emit("gateway.connecting", { failures: this.failures });
    let url: string;
    try {
      url = await this.opts.socketUrl();
    } catch (error) {
      if (gen !== this.connectGen) return;
      this.connecting = false;
      connLog.emit("gateway.close", { stage: "ticket", ...errorFields(error) });
      return this.retry();
    }
    if (this.stopped || gen !== this.connectGen) return;
    this.connecting = false;
    const socket = this.opts.openSocket(url);
    this.socket = socket;
    let openedAt: number | null = null;
    socket.onopen = () => {
      if (this.socket !== socket) return;
      openedAt = performance.now();
      this.lastHeard = this.now();
      connLog.emit("gateway.open", { ms: openedAt - dialledAt, subs: this.subs.size });
      this.failures = 0;
      this.openStore.set(true);
      for (const [s, sub] of this.subs) {
        clearTimeout(sub.again);
        sub.again = undefined;
        socket.send(wire(s, sub));
      }
      clearInterval(this.ping);
      const silenceMs = this.opts.silenceMs ?? SILENCE_MS;
      this.ping = setInterval(() => {
        if (this.socket !== socket) return;
        if (this.now() - this.lastHeard > silenceMs) {
          connLog.emit("gateway.close", { stage: "silence", openMs: openedAt === null ? null : performance.now() - openedAt });
          this.drop();
          if (!this.stopped) this.retry();
          return;
        }
        this.send(JSON.stringify({ t: "ping" }));
      }, this.opts.pingMs ?? PING_MS);
    };
    socket.onmessage = (event) => {
      if (this.socket !== socket || typeof event.data !== "string") return;
      this.lastHeard = this.now();
      let message: Record<string, unknown>;
      try {
        message = JSON.parse(event.data) as Record<string, unknown>;
      } catch {
        return;
      }
      this.take(message);
    };
    socket.onclose = (event) => {
      if (this.socket !== socket) return;
      const ev = (event ?? {}) as { code?: number; reason?: string; wasClean?: boolean };
      connLog.emit("gateway.close", { code: ev.code ?? null, reason: ev.reason ?? "", clean: ev.wasClean ?? null, openMs: openedAt === null ? null : performance.now() - openedAt });
      this.drop();
      if (!this.stopped) this.retry();
    };
  }

  private retry(): void {
    if (this.stopped) return;
    const steps = this.opts.backoffMs ?? BACKOFF_MS;
    const wait = steps[Math.min(this.failures, steps.length - 1)] ?? 1000;
    this.failures += 1;
    connLog.emit("gateway.retry", { waitMs: wait, failures: this.failures });
    if (this.failures % 3 === 0) this.opts.onTrouble?.();
    this.timer = setTimeout(() => {
      this.timer = undefined;
      void this.connect();
    }, wait);
  }

  private take(m: Record<string, unknown>): void {
    switch (m.t) {
      case "dir":
        this.dir.set(parseDir(m.machines));
        return;
      case "copy": {
        const sub = this.subs.get(m.s as number);
        if (!sub) return;
        sub.events.onCopy({
          reset: m.reset === true,
          bodies: Array.isArray(m.bodies) ? m.bodies : [],
          headSeq: num(m.headSeq) ?? 0,
          epoch: num(m.epoch) ?? 0,
          headTimeMs: num(m.headTimeMs) ?? 0,
        });
        return;
      }
      case "missing":
        this.subs.get(m.s as number)?.events.onMissing?.();
        return;
      case "copy_error": {
        const s = m.s as number;
        const sub = this.subs.get(s);
        if (!sub) return;
        if (m.code === "overrun" || m.code === "busy") {
          // The page fell behind, or asked too fast: the same subscription
          // again in a moment, from where it stands.
          clearTimeout(sub.again);
          sub.again = setTimeout(() => {
            sub.again = undefined;
            if (this.subs.get(s) === sub) this.send(wire(s, sub));
          }, this.opts.againMs ?? AGAIN_MS);
          return;
        }
        // Ended: the install let go of it already; said once more, in case.
        this.subs.delete(s);
        this.send(JSON.stringify({ t: "unsub", s }));
        sub.events.onEnd?.(typeof m.code === "string" ? m.code : "error");
        return;
      }
      case "bye": {
        const bye = { code: typeof m.code === "string" ? m.code : "bye", message: typeof m.message === "string" ? m.message : "" };
        this.byeInfo = bye;
        this.close();
        this.dir.set([]);
        for (const fn of [...this.byeListeners]) {
          try {
            fn(bye);
          } catch {
            // one listener failing must not starve the others
          }
        }
        return;
      }
      default:
        return;
    }
  }
}
