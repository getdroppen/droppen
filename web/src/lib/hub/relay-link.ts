/*
 * The page's connection to one relay (`droppen-relay`, `docs/relay-and-plans.md`
 * §2.4): `wss://<relay>/v1/connect`, signed in with a ticket from the Hub.
 *
 * 1. relay → `challenge {nonce, relay, origin}`;
 * 2. page → `login {ticket, time_ms, sig}`: a `HubLogin` with role
 *    `relayDevice` and the relay's origin as this page dialled it, signed by
 *    the device key — good on this relay only, never at the Hub;
 * 3. relay → `welcome`, then `presence {machines}` (the ticket's machines,
 *    `online` while signed in here), then only changes.
 *
 * Then channels exactly as on the Hub: `open`/`data`/`credit`/`close` frames
 * routed by `ch` (one counter per connection, never reused), `ping`/`pong`
 * every 20 s — which is also how the relay's round trip is measured while it
 * is connected. `goaway {retry_ms, reason}` says the relay is draining: the
 * page opens new channels elsewhere and moves (make before break).
 *
 * Unlike the Hub's link this one does not reconnect by itself: the relay pool
 * (`relays.ts`) opens a connection when a channel needs one, and lets it go
 * two minutes after its last channel closed.
 */
import { decodeFrame, encodeFrame, type Frame, fromHex, hex, signHubLogin } from "@/lib/e2e";
import { LINK_LOST, type PresenceState } from "./link";

export type RelayLinkState = "connecting" | "open" | "closed";

const ROUTED = new Set<Frame["type"]>(["data", "credit", "close"]);
const HEARTBEAT_MS = 20_000;
/** Silence after which the relay is presumed gone (the channel layer notices sooner on its own). */
const SILENCE_MS = 60_000;

interface SocketLike {
  binaryType: string;
  readyState: number;
  send(data: string | ArrayBufferLike | ArrayBufferView): void;
  close(code?: number, reason?: string): void;
  onopen: ((ev: unknown) => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
  onclose: ((ev: { code?: number; reason?: string }) => void) | null;
  onerror: ((ev: unknown) => void) | null;
}

export interface RelayLinkOptions {
  relay: string;
  /** `wss://…`, the relay's origin as the Hub lists it (and as this page dials it). */
  origin: string;
  account: string;
  deviceId: string;
  signKey: CryptoKey;
  /** A current ticket for this relay. */
  ticket: () => Promise<string>;
  makeSocket?: (url: string) => SocketLike;
  now?: () => number;
  /** 20 s and 60 s unless a test says otherwise. */
  heartbeatMs?: number;
  silenceMs?: number;
}

type Listener<T> = (value: T) => void;

export class RelayLink {
  state: RelayLinkState = "closed";
  /** The close code, when it closed. */
  lastClose: number | null = null;
  /** Why it closed or was refused, for the picker's "last failure". */
  lastError: string | null = null;
  /** The relay said it is draining. */
  goingAway = false;
  /** The latest `ping`/`pong` round trip (ms). */
  rttMs: number | null = null;
  readonly presence = new Map<string, PresenceState>();

  private socket: SocketLike | null = null;
  private nextCh = 1;
  private nonce = 0;
  private pings = new Map<number, number>();
  private heartbeat: ReturnType<typeof setInterval> | null = null;
  private lastHeard = 0;
  private lastBeat = 0;
  private readonly routes = new Map<number, (frame: Frame) => void>();
  private readonly stateListeners = new Set<Listener<RelayLinkState>>();
  private readonly presenceListeners = new Set<Listener<Map<string, PresenceState>>>();
  private readonly goawayListeners = new Set<Listener<{ retryMs: number; reason: string }>>();
  /** Resolves once signed in; rejects if the sign-in fails. */
  private opening: Promise<void> | null = null;

  constructor(private readonly opts: RelayLinkOptions) {}

  get relay(): string {
    return this.opts.relay;
  }

  get origin(): string {
    return this.opts.origin;
  }

  /** Channels open on this connection now. */
  get channels(): number {
    return this.routes.size;
  }

  onState(fn: Listener<RelayLinkState>): () => void {
    this.stateListeners.add(fn);
    return () => this.stateListeners.delete(fn);
  }

  onPresence(fn: Listener<Map<string, PresenceState>>): () => void {
    this.presenceListeners.add(fn);
    return () => this.presenceListeners.delete(fn);
  }

  onGoaway(fn: Listener<{ retryMs: number; reason: string }>): () => void {
    this.goawayListeners.add(fn);
    return () => this.goawayListeners.delete(fn);
  }

  /** Connect and sign in (once; again only after it closed). */
  open(timeoutMs = 10_000): Promise<void> {
    if (this.state === "open") return Promise.resolve();
    this.opening ??= this.connect(timeoutMs).finally(() => {
      this.opening = null;
    });
    return this.opening;
  }

  /** Send a ticket renewal (the relay answers `ticket_ok`). */
  renew(ticket: string): boolean {
    if (this.state !== "open" || !this.socket) return false;
    this.socket.send(JSON.stringify({ t: "ticket", ticket }));
    return true;
  }

  reserve(route: (frame: Frame) => void): number {
    const ch = this.nextCh++;
    this.routes.set(ch, route);
    return ch;
  }

  release(ch: number): void {
    this.routes.delete(ch);
  }

  send(frame: Frame): boolean {
    if (this.state !== "open" || !this.socket) return false;
    this.socket.send(encodeFrame(frame));
    return true;
  }

  /**
   * The page is back (in view, online again): one carrying no channel and
   * not heard from for a beat is let go at once — the page may have slept
   * through its death, and the next channel would wait on it for nothing,
   * where a new one costs a sign-in; any other is checked now (a beat).
   */
  wake(): void {
    if (this.state !== "open") return;
    if (this.routes.size === 0 && this.now() - this.lastHeard >= (this.opts.heartbeatMs ?? HEARTBEAT_MS)) {
      this.close();
      return;
    }
    this.beat();
  }

  close(): void {
    const socket = this.socket;
    this.drop(1000, "done");
    try {
      socket?.close(1000, "bye");
    } catch {
      // already closing
    }
  }

  private setState(next: RelayLinkState): void {
    if (this.state === next) return;
    this.state = next;
    for (const fn of [...this.stateListeners]) fn(next);
  }

  private now(): number {
    return (this.opts.now ?? Date.now)();
  }

  private connect(timeoutMs: number): Promise<void> {
    return new Promise((resolve, reject) => {
      this.setState("connecting");
      this.goingAway = false;
      this.lastError = null;
      const make = this.opts.makeSocket ?? ((url: string) => new WebSocket(url) as unknown as SocketLike);
      let socket: SocketLike;
      try {
        socket = make(`${this.opts.origin}/v1/connect`);
      } catch (error) {
        this.setState("closed");
        reject(error instanceof Error ? error : new Error(String(error)));
        return;
      }
      socket.binaryType = "arraybuffer";
      this.socket = socket;
      let settled = false;
      const fail = (why: string) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        this.lastError = why;
        reject(new Error(why));
      };
      const timer = setTimeout(() => {
        fail("the relay did not let this device in in time");
        try {
          socket.close(1000, "timeout");
        } catch {
          // already closing
        }
        this.drop(4408, "timeout");
      }, timeoutMs);
      socket.onmessage = (ev) => {
        if (this.socket !== socket) return;
        this.lastHeard = this.now();
        if (typeof ev.data === "string") {
          void this.onText(socket, ev.data, () => {
            if (settled) return;
            settled = true;
            clearTimeout(timer);
            resolve();
          }, fail);
        } else if (ev.data instanceof ArrayBuffer) this.onBinary(new Uint8Array(ev.data));
      };
      socket.onclose = (ev) => {
        if (this.socket !== socket) return;
        this.lastClose = ev.code ?? null;
        fail(ev.reason || `the relay closed the connection (${ev.code ?? "?"})`);
        this.drop(ev.code ?? 1006, ev.reason ?? "");
      };
      socket.onerror = () => undefined;
    });
  }

  private async onText(socket: SocketLike, text: string, signedIn: () => void, fail: (why: string) => void): Promise<void> {
    let message: Record<string, unknown>;
    try {
      message = JSON.parse(text) as Record<string, unknown>;
    } catch {
      return;
    }
    switch (message.t) {
      case "challenge": {
        const nonce = typeof message.nonce === "string" ? fromHex(message.nonce) : null;
        if (!nonce || nonce.length !== 32) return socket.close(4400, "bad challenge");
        try {
          const ticket = await this.opts.ticket();
          const timeMs = this.now();
          const sig = await signHubLogin(
            { role: "relayDevice", account: this.opts.account, id: this.opts.deviceId, hub: this.opts.origin, nonce, timeMs },
            this.opts.signKey,
          );
          if (this.socket !== socket) return;
          socket.send(JSON.stringify({ t: "login", ticket, time_ms: timeMs, sig: hex(sig) }));
        } catch (error) {
          fail(error instanceof Error ? error.message : String(error));
          socket.close(1000, "no ticket");
        }
        return;
      }
      case "welcome":
        this.lastHeard = this.lastBeat = this.now();
        this.heartbeat = setInterval(() => this.beat(), this.opts.heartbeatMs ?? HEARTBEAT_MS);
        this.setState("open");
        signedIn();
        // A first round trip right away, for the indicator.
        this.beat();
        return;
      case "presence": {
        const machines = Array.isArray(message.machines) ? (message.machines as { id?: unknown; state?: unknown }[]) : [];
        for (const m of machines) {
          if (typeof m.id === "string" && (m.state === "online" || m.state === "offline")) this.presence.set(m.id, m.state);
        }
        for (const fn of [...this.presenceListeners]) fn(this.presence);
        return;
      }
      case "goaway":
        this.goingAway = true;
        for (const fn of [...this.goawayListeners]) fn({ retryMs: Number(message.retry_ms) || 0, reason: String(message.reason ?? "") });
        return;
      case "error":
        this.lastError = typeof message.message === "string" ? message.message : String(message.code ?? "error");
        return;
      default:
        return;
    }
  }

  private beat(): void {
    if (this.state !== "open") return;
    const heartbeat = this.opts.heartbeatMs ?? HEARTBEAT_MS;
    const silence = this.opts.silenceMs ?? SILENCE_MS;
    const now = this.now();
    // As on the hub link: a beat far later than due means this page's timers
    // were held (a background tab, a phone asleep) — what was not heard
    // meanwhile says nothing about the relay, so it gets one more interval.
    const late = now - this.lastBeat > heartbeat * 1.5;
    this.lastBeat = now;
    if (late) this.lastHeard = Math.max(this.lastHeard, now - silence + heartbeat);
    else if (this.lastHeard && now - this.lastHeard >= silence) {
      const socket = this.socket;
      this.drop(4410, "the relay went silent");
      socket?.close(1000, "silent");
      return;
    }
    const nonce = ++this.nonce;
    this.pings.set(nonce, performance.now());
    if (this.pings.size > 8) this.pings.delete(this.pings.keys().next().value as number);
    this.send({ type: "ping", nonce });
  }

  private onBinary(data: Uint8Array): void {
    let frame: Frame;
    try {
      frame = decodeFrame(data);
    } catch {
      return;
    }
    if (frame.type === "pong") {
      const sent = this.pings.get(frame.nonce);
      if (sent !== undefined) {
        this.pings.delete(frame.nonce);
        this.rttMs = performance.now() - sent;
      }
      return;
    }
    if (!ROUTED.has(frame.type) || !("ch" in frame)) return;
    const route = this.routes.get(frame.ch);
    if (!route) return;
    if (frame.type === "close") this.routes.delete(frame.ch);
    route(frame);
  }

  /** Forget the connection: every channel on it is told it is gone. */
  private drop(code: number, reason: string): void {
    if (this.heartbeat) clearInterval(this.heartbeat);
    this.heartbeat = null;
    if (this.socket) {
      this.socket.onmessage = null;
      this.socket.onclose = null;
    }
    this.socket = null;
    this.lastClose = code;
    const routes = [...this.routes.entries()];
    this.routes.clear();
    for (const [ch, route] of routes) route({ type: "close", ch, code: LINK_LOST, reason: reason || "the connection to the relay was lost" });
    this.presence.clear();
    for (const fn of [...this.presenceListeners]) fn(this.presence);
    this.setState("closed");
  }
}
