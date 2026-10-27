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
import { decodeFrame, type Frame, fromHex, hex, signHubLogin } from "@/lib/e2e";
import { LINK_LOST, type PresenceState } from "./link";
import { SocketLink, type SocketLike } from "./socket-link";

export type RelayLinkState = "connecting" | "open" | "closed";

const ROUTED = new Set<Frame["type"]>(["data", "credit", "close"]);
const HEARTBEAT_MS = 20_000;
/** Silence after which the relay is presumed gone (the channel layer notices sooner on its own). */
const SILENCE_MS = 60_000;
/**
 * How long a wake-up ping (`wake`) has to be answered before the connection is
 * let go. A phone's pause can leave a socket that looks open and carries
 * nothing; waiting the whole {@link SILENCE_MS} is what made a return to the
 * app cost tens of seconds.
 */
export const WAKE_PROBE_MS = 2_000;

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
  /** How long the wake-up ping waits for an answer; {@link WAKE_PROBE_MS} unless a test says otherwise. */
  probeMs?: number;
}

type Listener<T> = (value: T) => void;

export class RelayLink extends SocketLink<RelayLinkState> {
  /** The close code, when it closed. */
  lastClose: number | null = null;
  /** Why it closed or was refused, for the picker's "last failure". */
  lastError: string | null = null;
  /** The relay said it is draining. */
  goingAway = false;
  /** The latest `ping`/`pong` round trip (ms). */
  rttMs: number | null = null;
  readonly presence = new Map<string, PresenceState>();

  private nonce = 0;
  private pings = new Map<number, number>();
  private heartbeat: ReturnType<typeof setInterval> | null = null;
  private readonly presenceListeners = new Set<Listener<Map<string, PresenceState>>>();
  private readonly goawayListeners = new Set<Listener<{ retryMs: number; reason: string }>>();
  /** Resolves once signed in; rejects if the sign-in fails. */
  private opening: Promise<void> | null = null;

  constructor(private readonly opts: RelayLinkOptions) {
    super();
  }

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

  /**
   * The page is back (in view, online again): one carrying no channel and
   * not heard from for a beat is let go at once — the page may have slept
   * through its death, and the next channel would wait on it for nothing,
   * where a new one costs a sign-in; any other is checked now, and let go
   * too unless it answers within {@link WAKE_PROBE_MS}: a beat alone would
   * let a socket that died in the pause stand for a whole silence, which is
   * what every return to the app cost on a phone.
   */
  wake(): void {
    if (this.state !== "open") return;
    if (this.routes.size === 0 && this.now() - this.lastHeard >= (this.opts.heartbeatMs ?? HEARTBEAT_MS)) {
      this.close();
      return;
    }
    if (this.probeTimer) return;
    this.beat();
    if (this.state !== "open") return;
    this.startProbe(this.opts.probeMs ?? WAKE_PROBE_MS, () => {
      if (this.state !== "open") return;
      const socket = this.socket;
      this.drop(4410, "the relay did not answer the wake-up ping");
      try {
        socket?.close(1000, "silent");
      } catch {
        // already gone
      }
    });
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

  /** Tell the state listeners over a copy: one may unsubscribe as it hears. */
  protected notifyState(next: RelayLinkState): void {
    for (const fn of [...this.stateListeners]) fn(next);
  }

  protected now(): number {
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
        if (!this.heard(socket)) return;
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
    // As on the hub link: a beat far later than due means this page's timers
    // were held (a background tab, a phone asleep) — what was not heard
    // meanwhile says nothing about the relay, so it gets one more interval.
    const { now, late } = this.markBeat(heartbeat, silence);
    if (!late && this.lastHeard && now - this.lastHeard >= silence) {
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
    this.route(frame);
  }

  /** Forget the connection: every channel on it is told it is gone. */
  private drop(code: number, reason: string): void {
    if (this.heartbeat) clearInterval(this.heartbeat);
    this.heartbeat = null;
    this.answered();
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
