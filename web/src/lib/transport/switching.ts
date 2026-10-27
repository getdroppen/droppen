/*
 * Which way the page reaches its machine in cloud mode, and when that changes.
 *
 * Two transports under one: `live`, the end-to-end channel to the machine
 * (`lib/transport/tunnel.ts`), and `offline`, the machine's offline copy
 * (`offline.ts`). The page starts offline — drawn at once from the
 * copy on the device — and goes live the moment the channel is open; it goes
 * back to the copy only once the channel has been down for a while
 * ({@link DOWN_MS}), so a blip (a hub deploy, a network change) is reconnected
 * through rather than flipped over and back.
 *
 * A switch closes every socket the other transport opened: the feed hub and
 * the session feeds reconnect on their own, through the new one (the
 * transport is looked up per attempt, never captured), and a session window
 * the new side recognises is resumed rather than re-sent. The caller re-runs
 * the app's queries (`invalidateQueries`).
 */
import type { LinkState, PresenceState } from "@/lib/hub/link";
import type { MachineStatus } from "@/lib/hub/machine";
import { onPageResume } from "@/lib/page-resume";
import type { FetchOptions, SocketLike, Transport } from "@/lib/transport";

export type CloudMode = "offline" | "live";

/** How long the channel must stay down before the page goes back to the copy. */
export const DOWN_MS = 3000;

const CLOSED = 3;

export interface SwitchHooks {
  /** A session feed was subscribed on the live feed socket: it was opened on this device. */
  onOpened?(source: string, id: string): void;
}

/** What a disposed transport answers: its machine is no longer on the page. */
export class MachineClosedError extends Error {
  constructor() {
    super("the device is no longer open on this page");
  }
}

export class SwitchingTransport implements Transport {
  private mode: CloudMode;
  /** Set by {@link dispose}: nothing goes out through it any more. */
  private disposed = false;
  private readonly sockets: Record<CloudMode, Set<SocketLike>> = { offline: new Set(), live: new Set() };
  private readonly listeners = new Set<(mode: CloudMode) => void>();

  constructor(
    private readonly transports: Record<CloudMode, Transport>,
    initial: CloudMode = "offline",
    private readonly hooks: SwitchHooks = {},
  ) {
    this.mode = initial;
  }

  get current(): CloudMode {
    return this.mode;
  }

  onMode(fn: (mode: CloudMode) => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  onChange(fn: () => void): () => void {
    return this.onMode(() => fn());
  }

  /** A new connection to the machine is up (`live`'s, while the page is on it or not). */
  onReconnect(fn: () => void): () => void {
    return this.transports.live.onReconnect?.(fn) ?? (() => undefined);
  }

  /** Both sides are reached only through `fetch`: neither the machine nor its copy is at the page's origin. */
  readonly tunneled = true;

  fetch(request: Request, options?: FetchOptions): Promise<Response> {
    if (this.disposed) return Promise.reject(new MachineClosedError());
    return this.transports[this.mode].fetch(request, options);
  }

  socketUrl(path: string): Promise<string> {
    if (this.disposed) return Promise.reject(new MachineClosedError());
    return this.transports[this.mode].socketUrl(path);
  }

  openSocket(url: string): SocketLike {
    if (this.disposed) throw new MachineClosedError();
    const mode = this.mode;
    const socket = this.transports[mode].openSocket(url);
    const open = this.sockets[mode];
    for (const s of open) if (s.readyState === CLOSED) open.delete(s);
    open.add(socket);
    if (mode === "live" && this.hooks.onOpened && url.startsWith("/api/events")) {
      const send = socket.send.bind(socket);
      const onOpened = this.hooks.onOpened;
      socket.send = (data: string) => {
        send(data);
        noteSubs(data, onOpened);
      };
    }
    return socket;
  }

  /**
   * The machine is gone from the page (closed, or the account signed out):
   * every socket either side opened is closed, and a request still to come —
   * a retry, a late refetch — is refused rather than dialling the machine
   * again.
   */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.listeners.clear();
    for (const side of ["live", "offline"] as const) {
      const open = [...this.sockets[side]];
      this.sockets[side].clear();
      for (const socket of open) {
        try {
          socket.close();
        } catch {
          // already closing
        }
      }
    }
  }

  /** Go `next`: the other side's sockets are closed, and whoever held them reconnects through this one. */
  setMode(next: CloudMode): void {
    if (next === this.mode || this.disposed) return;
    const previous = this.mode;
    this.mode = next;
    const open = [...this.sockets[previous]];
    this.sockets[previous].clear();
    for (const socket of open) {
      try {
        socket.close();
      } catch {
        // already closing
      }
    }
    for (const fn of [...this.listeners]) fn(next);
  }
}

/** The sessions a feed socket's `sub` message names. */
function noteSubs(data: string, onOpened: (source: string, id: string) => void): void {
  let m: { type?: unknown; feeds?: unknown };
  try {
    m = JSON.parse(data) as typeof m;
  } catch {
    return;
  }
  if (m.type !== "sub" || !Array.isArray(m.feeds)) return;
  for (const feed of m.feeds as { source?: unknown; id?: unknown }[]) {
    if (typeof feed?.source === "string" && typeof feed.id === "string") onOpened(feed.source, feed.id);
  }
}

export interface ModeControllerOptions {
  link: {
    readonly state: LinkState;
    readonly presence: ReadonlyMap<string, { state: PresenceState }>;
    onState(fn: (s: LinkState) => void): () => void;
    onPresence(fn: (p: unknown) => void): () => void;
  };
  channel: {
    readonly status: MachineStatus;
    onStatus(fn: (s: MachineStatus) => void): () => void;
    ensure(): Promise<unknown>;
  };
  machine: string;
  transport: SwitchingTransport;
  /** After every switch (re-run the app's queries). */
  onSwitch?(mode: CloudMode): void;
  downMs?: number;
  /** The wait before the `attempt`th retry (0-based) of a channel that would not open while the machine is online. */
  retryMs?: (attempt: number) => number;
  /** Whether a relay this page is on has the machine signed in (it is online whatever the hub says meanwhile, §2.9). */
  relayOnline?: () => boolean;
  /**
   * Whether the page wants the machine live at all (default: always). A
   * machine nobody has asked for (`MachineRuntime.ensureLive`) is shown from
   * its copy and nothing dials it; {@link ModeController.poke} once it is
   * wanted.
   */
  wanted?: () => boolean;
  /**
   * Hear the page come back — in view, shown again, focused, online again
   * (default {@link onPageResume}): a try then, not at the end of a backoff
   * whose timer the phone held while it slept.
   */
  resume?: (fn: () => void) => () => void;
}

/** 2 s doubling to 30 s. */
export const channelRetryMs = (attempt: number) => Math.min(2000 * 2 ** attempt, 30_000);

/**
 * Watches the machine's presence, the hub link and the channel, and moves the
 * page between the copy and the machine: live as soon as the channel is open;
 * back to the copy once it has been down for {@link DOWN_MS}. While on the
 * copy with the machine not known to be offline — and wanted — it keeps
 * opening the channel, backing off — and tries at once whenever the hub says
 * anything new of the machines, the link signs in, or the page comes back
 * ({@link ModeControllerOptions.resume}): a channel that would not open (no
 * relay could carry it, say) is never given up on.
 */
export class ModeController {
  private downTimer: ReturnType<typeof setTimeout> | undefined;
  private retryTimer: ReturnType<typeof setTimeout> | undefined;
  private trying = false;
  private attempts = 0;
  private offs: (() => void)[] = [];
  private stopped = false;

  constructor(private readonly opts: ModeControllerOptions) {}

  start(): void {
    const evaluate = () => this.evaluate();
    this.offs = [
      this.opts.link.onState(evaluate),
      this.opts.link.onPresence(() => {
        // The machine came back (or went): try straight away, not at the end of a backoff.
        this.attempts = 0;
        clearTimeout(this.retryTimer);
        this.retryTimer = undefined;
        this.evaluate();
      }),
      this.opts.channel.onStatus(evaluate),
      (this.opts.resume ?? onPageResume)(() => this.poke()),
    ];
    this.evaluate();
  }

  /** Look again now (the machine became wanted): a channel attempt starts at once rather than after a backoff. */
  poke(): void {
    this.attempts = 0;
    clearTimeout(this.retryTimer);
    this.retryTimer = undefined;
    this.evaluate();
  }

  /** Go to the copy now — the machine is being let go of — without waiting for the channel to stay down. */
  toCopy(): void {
    if (this.stopped) return;
    this.switchTo("offline");
  }

  stop(): void {
    this.stopped = true;
    for (const off of this.offs) off();
    clearTimeout(this.downTimer);
    clearTimeout(this.retryTimer);
  }

  private presence(): PresenceState | null {
    if (this.opts.relayOnline?.()) return "online";
    return this.opts.link.presence.get(this.opts.machine)?.state ?? null;
  }

  private evaluate(): void {
    if (this.stopped) return;
    const { transport, channel, link } = this.opts;
    const open = channel.status === "open";
    if (transport.current === "offline") {
      if (open) return this.switchTo("live");
      // Only the hub's word that the machine is offline stops the attempts:
      // "unknown" (a stale heartbeat) or no word yet is worth a try, which
      // the hub refuses at once if it is wrong.
      if (link.state === "open" && this.presence() !== "offline" && (this.opts.wanted?.() ?? true)) this.tryChannel();
      return;
    }
    const down = !open || this.presence() === "offline";
    if (!down) {
      clearTimeout(this.downTimer);
      this.downTimer = undefined;
      return;
    }
    this.downTimer ??= setTimeout(() => {
      this.downTimer = undefined;
      const stillDown = this.opts.channel.status !== "open" || this.presence() === "offline";
      if (stillDown && !this.stopped) this.switchTo("offline");
      this.evaluate();
    }, this.opts.downMs ?? DOWN_MS);
  }

  private switchTo(mode: CloudMode): void {
    clearTimeout(this.downTimer);
    this.downTimer = undefined;
    if (this.opts.transport.current === mode) return;
    this.opts.transport.setMode(mode);
    this.opts.onSwitch?.(mode);
  }

  private tryChannel(): void {
    if (this.trying || this.retryTimer !== undefined || this.opts.channel.status === "connecting") return;
    this.trying = true;
    this.opts.channel.ensure().then(
      () => {
        this.trying = false;
        this.attempts = 0;
        this.evaluate();
      },
      () => {
        this.trying = false;
        if (this.stopped) return;
        const wait = (this.opts.retryMs ?? channelRetryMs)(this.attempts++);
        this.retryTimer = setTimeout(() => {
          this.retryTimer = undefined;
          this.evaluate();
        }, wait);
      },
    );
  }
}
