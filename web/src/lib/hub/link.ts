/*
 * The page's one connection to its hub: `/v1/client` (docs/hub-api/websocket.md).
 *
 * Text messages are the hub's control plane (docs/hub-api/websocket.md): the
 * sign-in challenge — answered with a `HubLogin` the device key signs, which
 * names the origin this page dialled, never the one the hub claims — then
 * presence, errors, the replies about stream subscriptions
 * (`stream_synced`, `stream_refused`) and a device's new mail (`mail`), handed
 * to {@link HubLink.onControl}. The account's chain, once subscribed to
 * (`chain_sub`), arrives as `chain` frames (`ch` 0, {@link HubLink.onChain}).
 * Binary messages are one frame each (`lib/e2e/frame.ts`), routed by `ch`: a
 * channel's `data`/`credit`/`close`, and a stream subscription's
 * `recs`/`head`/`gone`/`checkpoint`/`ephemeral` (`lib/hub/streams.ts`) — the
 * two share one counter, so a `ch` is never both.
 *
 * A channel goes through a relay when one can carry it; the hub carries it
 * otherwise, while it is configured to (`RH_HUB_CHANNEL_RELAY`, on by
 * default). Switched off, it answers an `OPEN` with `CLOSE`
 * {@link HUB_NO_CHANNELS}: the link remembers that
 * ({@link HubLink.carriesChannels}) until its next connection — a deploy may
 * flip the switch — so nothing asks it again meanwhile.
 *
 * The sign-in names this device's hub session, which must be a device
 * session of this very device (`HubApi.deviceSession`); a sign-in the hub
 * refuses (4401) renews the session once before the next try.
 *
 * It reconnects by itself, with full-jitter backoff (0.5 s doubling to 30 s,
 * reset once a connection has stayed up a while): a hub deploy closes every
 * socket with 1012, and the page should not notice more than a blip. Every
 * channel on a connection that went away is told so (a synthetic `close`
 * with {@link LINK_LOST}); channels re-handshake on the next connection.
 *
 * It pings every {@link HEARTBEAT_MS}, which the hub answers, and a
 * connection that has brought nothing at all for {@link SILENCE_MS} is taken
 * for dead and replaced — a socket a phone's network dropped without a word
 * (a half-open TCP connection) would otherwise sit "open" for minutes, with
 * every frame sent on it lost and every request over it waiting for nothing.
 * A beat that comes far later than due means this page's timers were held (a
 * background tab, a phone asleep): what was not heard meanwhile says nothing
 * about the connection, so it gets one more interval from then.
 *
 * What the hub says of the machines ({@link HubLink.presence}) is kept across
 * connections, for display, but it is only *fresh* ({@link
 * HubLink.presenceFresh}) once the hub has said it on the connection now
 * signed in — the hub sends it right after its `welcome`, a moment later —
 * and while nothing makes that doubtful: after a late beat (the page was
 * away, and the socket may have died meanwhile) it is not, until the
 * connection answers again. What a page heard before a pause is not where
 * a machine is now: a channel is placed on fresh presence only
 * ({@link HubLink.waitPresence}). {@link HubLink.wake} is for the page coming
 * back (in view, online again): it reconnects at once rather than at the end
 * of a backoff that grew while it was away, or checks the connection now.
 */
import { decodeFrame, encodeAttestation, type Attestation, type Frame, fromHex, hex, signHubLogin } from "@/lib/e2e";
import { SocketLink, type SocketLike } from "./socket-link";

export type PresenceState = "online" | "offline" | "unknown";
export type LinkState = "connecting" | "open" | "closed";

/** A relay a machine says it is signed in on, and its round trip to it (relay-and-plans §2.9: information, never authority). */
export interface Via {
  relay: string;
  rttMs: number | null;
}

/** What the hub says of a machine. */
export interface MachinePresence {
  state: PresenceState;
  sinceMs: number;
  /** The relays it is on (empty: none, or an agent from before relays). */
  via: Via[];
  /** Whether its agent attaches to relays at all (`false`: too old to be reached; `undefined`: not said yet). */
  relays?: boolean;
}

/** The close a channel hears when the connection under it went away. */
export const LINK_LOST = 4410;

/** The hub's close for an `OPEN` while it carries no channels (`RH_HUB_CHANNEL_RELAY=off`): channels go through a relay. */
export const HUB_NO_CHANNELS = 4426;

/** The frame types routed to a `ch`'s owner; anything else on a `ch` is dropped. */
const ROUTED = new Set<Frame["type"]>(["data", "credit", "close", "recs", "head", "gone", "checkpoint", "ephemeral"]);

const BACKOFF_MIN = 500;
const BACKOFF_MAX = 30_000;
const STABLE_MS = 30_000;
/** How often the page pings the hub (it answers every ping). */
export const HEARTBEAT_MS = 10_000;
/** No message at all from the hub for this long, and the connection is taken for dead. */
export const SILENCE_MS = 25_000;
/**
 * How long a wake-up ping (`wake`) has to be answered before the connection is
 * replaced. A phone's pause can leave a socket that looks open and carries
 * nothing; waiting the whole {@link SILENCE_MS} for a connection the page has
 * just come back to is what made every wake cost tens of seconds.
 */
export const WAKE_PROBE_MS = 2_000;
/** The hub's close for a refused sign-in (a bad or expired session among other things). */
export const SIGN_IN_REFUSED = 4401;
/** At most one session renewal per this long for refused sign-ins. */
const RENEW_EVERY_MS = 30_000;

export interface LinkOptions {
  /** `wss://hub/v1/client`. */
  url: string;
  /** The origin sign-ins name (the page's own hub, `wss://hub`). */
  origin: string;
  account: string;
  deviceId: string;
  /** This device's session token (asked for at each sign-in: it is renewed as it goes). */
  token: string | (() => string);
  signKey: CryptoKey;
  /** Renew the device session (after the hub refused a sign-in with 4401). */
  renew?: () => Promise<void>;
  makeSocket?: (url: string) => SocketLike;
  random?: () => number;
  now?: () => number;
  /** {@link HEARTBEAT_MS} and {@link SILENCE_MS} unless a test says otherwise. */
  heartbeatMs?: number;
  silenceMs?: number;
  /** How long a connection must keep answering to reset the backoff; 30 s unless a test says otherwise. */
  stableMs?: number;
  /** How long the wake-up ping waits for an answer; {@link WAKE_PROBE_MS} unless a test says otherwise. */
  probeMs?: number;
}

type Listener<T> = (value: T) => void;

export class HubLink extends SocketLink<LinkState> {
  /** The last close code from the hub (4401: the sign-in was refused). */
  lastClose: number | null = null;
  readonly presence = new Map<string, MachinePresence>();
  /**
   * Whether the hub may carry a channel on this connection: `false` once it
   * answered an `OPEN` with {@link HUB_NO_CHANNELS}, until the next connection
   * signs in (a deploy may have turned relaying back on) or the page reloads.
   */
  carriesChannels = true;

  private stopped = true;
  private backoff = BACKOFF_MIN;
  private openedAt = 0;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private heartbeat: ReturnType<typeof setInterval> | null = null;
  private nonce = 0;
  private lastRenewMs = 0;
  /** The hub said where the machines are on this connection. */
  private presenceHeard = false;
  /** A late beat: the page was away, and this connection may have died meanwhile — until it answers. */
  private doubted = false;
  /** Those waiting for presence to be fresh ({@link waitPresence}). */
  private readonly freshWaiters = new Set<() => void>();
  private readonly presenceListeners = new Set<Listener<Map<string, MachinePresence>>>();
  private readonly controlListeners = new Set<Listener<Record<string, unknown>>>();
  private readonly chainListeners = new Set<Listener<Uint8Array[]>>();

  constructor(private readonly opts: LinkOptions) {
    super();
  }

  start(): void {
    if (!this.stopped) return;
    this.stopped = false;
    this.connect();
  }

  stop(): void {
    this.stopped = true;
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
    this.socket?.close(1000, "bye");
    this.dropConnection();
  }

  onPresence(fn: Listener<Map<string, MachinePresence>>): () => void {
    this.presenceListeners.add(fn);
    return () => this.presenceListeners.delete(fn);
  }

  onControl(fn: Listener<Record<string, unknown>>): () => void {
    this.controlListeners.add(fn);
    return () => this.controlListeners.delete(fn);
  }

  /**
   * Chain entries the hub pushes (`chain` frames, after a `chain_sub`): the
   * account's new entries as they land, encoded, unverified.
   */
  onChain(fn: Listener<Uint8Array[]>): () => void {
    this.chainListeners.add(fn);
    return () => this.chainListeners.delete(fn);
  }

  /** Resolves once signed in (now, if already), rejects after `timeoutMs`. */
  waitOpen(timeoutMs = 15_000): Promise<void> {
    if (this.state === "open") return Promise.resolve();
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        off();
        reject(new Error("the hub is not reachable"));
      }, timeoutMs);
      const off = this.onState((s) => {
        if (s !== "open") return;
        clearTimeout(timer);
        off();
        resolve();
      });
    });
  }

  /**
   * Whether {@link presence} is what the hub says now: said on the connection
   * signed in now, and not put in doubt since by a pause the connection may
   * not have survived. Not before the first `presence` after a reconnect.
   */
  get presenceFresh(): boolean {
    return this.state === "open" && this.presenceHeard && !this.doubted;
  }

  /** Resolves `true` once presence is fresh (now, if it is), `false` if it is not after `timeoutMs`. */
  waitPresence(timeoutMs: number): Promise<boolean> {
    if (this.presenceFresh) return Promise.resolve(true);
    return new Promise((resolve) => {
      const done = () => {
        clearTimeout(timer);
        this.freshWaiters.delete(check);
        resolve(this.presenceFresh);
      };
      const check = () => {
        if (this.presenceFresh) done();
      };
      const timer = setTimeout(done, timeoutMs);
      this.freshWaiters.add(check);
    });
  }

  private freshened(): void {
    for (const fn of [...this.freshWaiters]) fn();
  }

  /**
   * The page is back (in view again, online again, shown from the back-forward
   * cache): closed, connect now — not at the end of a backoff that grew while
   * it was away; open, ask it to prove it in {@link WAKE_PROBE_MS}: a beat
   * alone would let a socket that died in the pause stand for up to a whole
   * silence, which on a phone is what every return to the app cost.
   */
  wake(): void {
    if (this.stopped) return;
    if (this.state === "closed") {
      if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
      this.backoff = BACKOFF_MIN;
      this.connect();
      return;
    }
    if (this.state !== "open" || !this.socket) return;
    const socket = this.socket;
    this.beat(socket);
    this.probe(socket);
  }

  /** The beat just sent is the probe: unless anything is heard back in `probeMs`, replace the connection at once. */
  private probe(socket: SocketLike): void {
    if (this.socket !== socket || this.state !== "open" || this.probeTimer) return;
    this.startProbe(this.opts.probeMs ?? WAKE_PROBE_MS, () => {
      if (this.socket !== socket || this.state !== "open") return;
      this.lastClose = null;
      this.dropConnection();
      // A dead connection will not finish a closing handshake: let it go in its own time.
      try {
        socket.close(4000, "the wake-up ping went unanswered");
      } catch {
        // already gone
      }
      this.reconnectNow();
    });
  }

  /** The next connection now, not at the end of a backoff that was waiting for a socket this page has given up on. */
  private reconnectNow(): void {
    if (this.stopped) return;
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
    this.backoff = BACKOFF_MIN;
    this.connect();
  }

  /** Send a control message (a text frame, `{t: …}`) now; `false` when not signed in. */
  sendControl(message: Record<string, unknown>): boolean {
    if (this.state !== "open" || !this.socket) return false;
    this.socket.send(JSON.stringify(message));
    return true;
  }

  /** Hand the hub this device's attestation; it relays it to the account's agents. */
  attest(attestation: Attestation): boolean {
    return this.send({ type: "attest", ch: 0, attestation: encodeAttestation(attestation) });
  }

  private connect(): void {
    if (this.stopped) return;
    this.setState("connecting");
    const make = this.opts.makeSocket ?? ((url: string) => new WebSocket(url) as unknown as SocketLike);
    let socket: SocketLike;
    try {
      socket = make(this.opts.url);
    } catch {
      this.scheduleReconnect();
      return;
    }
    socket.binaryType = "arraybuffer";
    this.socket = socket;
    socket.onmessage = (ev) => {
      if (!this.heard(socket)) return;
      if (this.doubted) {
        // It answered after the pause: it lived through it, and told this page every change meanwhile (before this).
        this.doubted = false;
        this.freshened();
      }
      if (typeof ev.data === "string") void this.onText(socket, ev.data);
      else if (ev.data instanceof ArrayBuffer) this.onBinary(new Uint8Array(ev.data));
    };
    socket.onclose = (ev) => {
      if (this.socket !== socket) return;
      this.lastClose = ev.code ?? null;
      this.dropConnection();
      const now = (this.opts.now ?? Date.now)();
      if (ev.code === SIGN_IN_REFUSED && this.opts.renew && now - this.lastRenewMs > RENEW_EVERY_MS) {
        // The session may have expired or been replaced: a fresh one for the next sign-in.
        this.lastRenewMs = now;
        void this.opts.renew().then(
          () => this.scheduleReconnect(),
          () => this.scheduleReconnect(),
        );
        return;
      }
      this.scheduleReconnect();
    };
    socket.onerror = () => undefined;
  }

  private async onText(socket: SocketLike, text: string): Promise<void> {
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
        const timeMs = (this.opts.now ?? Date.now)();
        const sig = await signHubLogin(
          { role: "device", account: this.opts.account, id: this.opts.deviceId, hub: this.opts.origin, nonce, timeMs },
          this.opts.signKey,
        );
        if (this.socket !== socket) return;
        const session = typeof this.opts.token === "function" ? this.opts.token() : this.opts.token;
        socket.send(JSON.stringify({ t: "login", role: "device", account: this.opts.account, id: this.opts.deviceId, time_ms: timeMs, sig: hex(sig), session }));
        return;
      }
      case "welcome":
        // A new connection: whether this hub carries channels is not known yet,
        // nor where the machines are now (its `presence` comes next).
        this.carriesChannels = true;
        this.presenceHeard = false;
        this.doubted = false;
        this.openedAt = (this.opts.now ?? Date.now)();
        this.lastHeard = this.lastBeat = this.now();
        this.heartbeat = setInterval(() => this.beat(socket), this.opts.heartbeatMs ?? HEARTBEAT_MS);
        this.setState("open");
        return;
      case "presence": {
        const machines = Array.isArray(message.machines)
          ? (message.machines as { id: string; state: PresenceState; since_ms: number; via?: { relay?: unknown; rtt_ms?: unknown }[]; relays?: unknown }[])
          : [];
        for (const m of machines) {
          const via: Via[] = Array.isArray(m.via)
            ? m.via.filter((v) => typeof v?.relay === "string").map((v) => ({ relay: v.relay as string, rttMs: typeof v.rtt_ms === "number" ? v.rtt_ms : null }))
            : [];
          const known = this.presence.get(m.id)?.relays;
          this.presence.set(m.id, { state: m.state, sinceMs: m.since_ms, via, relays: typeof m.relays === "boolean" ? m.relays : known });
        }
        this.presenceHeard = true;
        for (const fn of this.presenceListeners) fn(this.presence);
        this.freshened();
        return;
      }
      default:
        for (const fn of this.controlListeners) fn(message);
    }
  }

  private onBinary(data: Uint8Array): void {
    let frame: Frame;
    try {
      frame = decodeFrame(data);
    } catch {
      return;
    }
    if (frame.type === "chain") {
      for (const fn of this.chainListeners) fn(frame.entries);
      return;
    }
    if (!ROUTED.has(frame.type) || !("ch" in frame)) return;
    // Remembered before the channel hears it, whoever asked.
    if (frame.type === "close" && frame.code === HUB_NO_CHANNELS) this.carriesChannels = false;
    this.route(frame);
  }

  protected now(): number {
    return (this.opts.now ?? Date.now)();
  }

  /** Ping the hub — unless the connection has gone silent, when it is replaced. */
  private beat(socket: SocketLike): void {
    if (this.socket !== socket || this.state !== "open") return;
    const heartbeat = this.opts.heartbeatMs ?? HEARTBEAT_MS;
    const silence = this.opts.silenceMs ?? SILENCE_MS;
    const { now, late } = this.markBeat(heartbeat, silence);
    if (late) {
      // Until it answers, what it said of the machines may be from before a socket that died in the pause.
      this.doubted = true;
    } else if (now - this.lastHeard >= silence) {
      this.lastClose = null;
      this.dropConnection();
      // A dead connection will not finish a closing handshake: nobody waits for it.
      try {
        socket.close(4000, "the hub went silent");
      } catch {
        // already gone
      }
      this.scheduleReconnect();
      return;
    }
    this.send({ type: "ping", nonce: ++this.nonce });
  }

  /** Forget the connection: stop the heartbeat and tell every channel it is gone. */
  private dropConnection(): void {
    if (this.heartbeat) clearInterval(this.heartbeat);
    this.heartbeat = null;
    this.answered();
    // Stable means it kept answering that long, not that it was open that
    // long: a hub that signs the page in and then says nothing is dropped for
    // silence after about as long, and must not reset the backoff each time.
    const stable = this.openedAt > 0 && this.lastHeard - this.openedAt >= (this.opts.stableMs ?? STABLE_MS);
    if (stable) this.backoff = BACKOFF_MIN;
    this.openedAt = 0;
    this.presenceHeard = false;
    this.doubted = false;
    if (this.socket) {
      this.socket.onmessage = null;
      this.socket.onclose = null;
    }
    this.socket = null;
    const routes = [...this.routes.entries()];
    this.routes.clear();
    for (const [ch, route] of routes) route({ type: "close", ch, code: LINK_LOST, reason: "the connection to the hub was lost" });
    this.setState("closed");
  }

  private scheduleReconnect(): void {
    if (this.stopped || this.reconnectTimer) return;
    const wait = Math.floor((this.opts.random ?? Math.random)() * this.backoff);
    this.backoff = Math.min(this.backoff * 2, BACKOFF_MAX);
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.connect();
    }, wait);
  }
}
