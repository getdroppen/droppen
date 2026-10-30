/*
 * The page's one websocket: the session list, and every session feed on top of
 * it.
 *
 * Every websocket a page opens through the tunnel is a connection of its own —
 * a TCP and a TLS handshake to the edge, a ticket and an upgrade — so a page
 * that opened one per session it showed paid seconds of handshaking for every
 * session it switched to on a weak link. The server can carry session feeds on
 * the list's socket instead (`server-rs/crates/droppen-drive/src/feed/mux.rs`), and this is the
 * client half: one socket, kept alive and reconnected here, and a *channel* per
 * session feed that looks to `session-feed-pool.ts` exactly like the socket it
 * used to open, so none of its resume, resync or backoff logic had to change.
 *
 * - `/api/events?v=2` asks for subscriptions, and `&z=1` for large frames as
 *   raw deflate (decoded with `DecompressionStream`), which matters because
 *   nothing between here and the server compresses a websocket. A server too
 *   old for either answers with `settings` instead of `hello`, and every
 *   channel then reports itself closed so its feed falls back to a socket of
 *   its own.
 * - Never `z=1` in cloud mode. There the socket is a pipe of the end-to-end
 *   channel through the hub, and compressing before encrypting lets whoever
 *   sees the ciphertext's lengths (the hub) learn secrets in a frame from
 *   text it can put next to them — a session title, a file name, a message
 *   (CRIME). A frame that arrives compressed on a socket that did not ask is
 *   refused like one that will not decode.
 * - A channel opened while the socket is down waits for it; the reconnect then
 *   re-subscribes every waiting feed in one message, each with its own resume
 *   marker, so a dropped connection costs the few items each feed missed.
 * - A browser never sees the server's pings, so a socket that died under a
 *   network change can look open for minutes. A `ping` every
 *   {@link HEARTBEAT_MS} answered by anything at all is what tells the two
 *   apart; silence for {@link SILENCE_MS} closes it and reconnects.
 */
import { isCloud } from "@/lib/hub/config";
import type { Transport } from "@/lib/transport/types";

/** The parts of a browser `WebSocket` the hub uses. */
export interface HubSocket {
  readonly readyState: number;
  binaryType: string;
  send(data: string): void;
  close(): void;
  onopen: ((event: unknown) => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onclose: ((event: unknown) => void) | null;
}

/** A session feed's view of the hub: the slice of a `WebSocket` the feed pool uses. */
export interface FeedChannel {
  readonly readyState: number;
  send(data: string): void;
  close(): void;
  onopen: ((event: unknown) => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onclose: ((event: unknown) => void) | null;
}

/** What a channel subscribes to. */
export interface FeedTarget {
  source: string;
  id: string;
  /** The resume marker of the window already held, if any. */
  since?: number;
  fp?: string | null;
  /** The most items an opening snapshot should carry. */
  limit?: number;
}

export interface FeedHubDeps {
  /** The URL to open `path` with (minting a ticket where a login is required). */
  socketUrl(path: string): Promise<string>;
  openSocket(url: string): HubSocket;
  /** A failed attempt, with the streak it ends; see `noteWsFailure`. */
  noteFailure(streak: number): void;
  /** Raw deflate to text, or `null` where the page cannot decode it (no `z=1` then). */
  inflate: ((data: ArrayBuffer) => Promise<string>) | null;
  /**
   * Whether this attempt may ask for compressed frames at all; asked per
   * attempt. The page's hubs pass {@link compressionAllowed} for their
   * machine's transport; absent, it is "not in cloud mode".
   */
  compress?: () => boolean;
  random?: () => number;
  now?: () => number;
  /** How long a wake-up ping waits for an answer; {@link WAKE_PROBE_MS} unless a test says otherwise. */
  probeMs?: number;
}

/** What the page wants from the list half of the socket. */
export interface HubHandlers {
  /** Every frame that is not a session feed's. */
  onFrame(frame: Record<string, unknown>): void;
  onOpen(): void;
  onClose(): void;
}

export type HubMode = "unknown" | "mux" | "legacy";

export const HEARTBEAT_MS = 25_000;
export const SILENCE_MS = 60_000;
/**
 * How long a socket the page has come back to has to answer a ping before it is
 * replaced. Without it a phone that slept through the socket's death waited a
 * whole {@link SILENCE_MS} before anything happened.
 */
export const WAKE_PROBE_MS = 2_000;
/** How many of the list's frames a connection keeps for a list that is not listening yet. */
export const EARLY_FRAMES = 32;
const OPEN = 1;
const CLOSED = 3;

/** The reconnect delay after `retry` failures in a row: 0.5 s doubling to 8 s, with jitter. */
export function hubBackoff(retry: number, random: () => number = Math.random): number {
  const ceiling = Math.min(500 * 2 ** Math.max(0, retry - 1), 8000);
  // Half fixed, half random: a server that restarts under a hundred tabs is
  // not then reconnected to by all of them in the same millisecond.
  return Math.round(ceiling / 2 + (random() * ceiling) / 2);
}

/**
 * Whether a feed socket through `via` may ask for compressed frames: only when
 * the page talks to its own server, never through a hub (cloud mode, a
 * tunneled transport), where compression next to secrets is a length side
 * channel.
 */
export function compressionAllowed(via: Transport): boolean {
  return !isCloud() && via.tunneled !== true;
}

const notCloud = () => !isCloud();

/** `DecompressionStream("deflate-raw")`, where the page has it. */
export function browserInflate(): ((data: ArrayBuffer) => Promise<string>) | null {
  if (typeof DecompressionStream !== "function") return null;
  try {
    new DecompressionStream("deflate-raw");
  } catch {
    return null;
  }
  return (data) =>
    new Response(new Blob([data]).stream().pipeThrough(new DecompressionStream("deflate-raw"))).text();
}

class Channel implements FeedChannel {
  readyState = 0;
  /** Its `sub` is on the wire of the current socket, confirmed or not. */
  requested = false;
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: unknown) => void) | null = null;

  constructor(
    readonly sub: number,
    readonly target: FeedTarget,
    private readonly hub: FeedHub,
  ) {}

  send(data: string): void {
    if (this.readyState !== OPEN) return;
    let message: Record<string, unknown>;
    try {
      message = JSON.parse(data);
    } catch {
      return;
    }
    this.hub.sendRaw({ ...message, sub: this.sub });
  }

  close(): void {
    if (this.readyState === CLOSED) return;
    const subscribed = this.requested;
    this.readyState = CLOSED;
    this.hub.forget(this, subscribed);
    // Asynchronously, like a socket's: the feed reads the close as news, and
    // must not be re-entered from inside the call that caused it.
    queueMicrotask(() => this.onclose?.({}));
  }

  /** The server has it now. */
  opened(): void {
    if (this.readyState !== 0) return;
    this.readyState = OPEN;
    this.onopen?.({});
  }

  /** The connection under it went away, or the server cannot carry it. */
  dropped(): void {
    if (this.readyState === CLOSED) return;
    this.readyState = CLOSED;
    this.onclose?.({});
  }
}

export class FeedHub {
  private ws: HubSocket | null = null;
  private handlers: HubHandlers | null = null;
  private retry = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private heartbeat: ReturnType<typeof setInterval> | undefined;
  private lastHeard = 0;
  /** The wake-up ping sent on coming back: unless answered, replace the socket now. */
  private probeTimer: ReturnType<typeof setTimeout> | undefined;
  private probeAnswered = false;
  private nextSub = 1;
  private readonly channels = new Map<number, Channel>();
  /** `unknown` until the first frame of a connection says which server this is. */
  mode: HubMode = "unknown";
  /** Whether this connection's large frames arrive compressed. */
  compressed = false;
  /** Whether this connection asked for them (`z=1`). */
  private askedCompression = false;

  constructor(private readonly deps: FeedHubDeps) {}

  private now(): number {
    return (this.deps.now ?? Date.now)();
  }

  /**
   * Whether the socket is wanted: by the page's list ([`start`]), or by a feed
   * that opened a channel first — which is the usual order, since React runs a
   * child's effects before its parent's, and the transcript is a child of the
   * provider that starts the list.
   */
  private running = false;

  /** Set by {@link shutdown}: nothing may start the socket again. */
  private dead = false;

  /**
   * Where the page's list is: not started yet (`idle`), started, or stopped
   * after it was started. A feed may start the socket itself only before the
   * list has ever been started (React runs the transcript's effect before its
   * provider's). Once the list has stopped, a feed asking for a channel gets
   * one that waits for the next {@link start}: when a tree goes, React cleans
   * up the provider before the transcript inside it, and the transcript's
   * feed, finding its channel dropped and still watched, would otherwise
   * reconnect a socket nobody would ever stop.
   */
  private list: "idle" | "started" | "stopped" = "idle";

  private ensure(): void {
    if (this.running || this.dead || this.list === "stopped") return;
    this.running = true;
    this.connect();
  }

  start(handlers: HubHandlers): void {
    this.handlers = handlers;
    this.list = "started";
    this.ensure();
    if (this.ws?.readyState !== OPEN) return;
    // A socket a feed started got here first. What it said meanwhile is
    // handed over — unless there was more of it than was kept, and the list
    // would be missing whatever was dropped: then a new connection, whose
    // snapshot is the list as it is now.
    if (this.earlyLost) return this.fresh();
    handlers.onOpen();
    for (const frame of this.early.splice(0)) handlers.onFrame(frame);
  }

  /** The list's frames this connection carried before anybody was listening for them. */
  private early: Record<string, unknown>[] = [];
  /** More arrived than {@link early} keeps: what it holds is not the list any more. */
  private earlyLost = false;

  stop(): void {
    this.list = "stopped";
    this.running = false;
    this.handlers = null;
    this.early = [];
    this.earlyLost = false;
    clearTimeout(this.timer);
    clearInterval(this.heartbeat);
    this.answered();
    const ws = this.ws;
    this.ws = null;
    ws?.close();
    for (const channel of [...this.channels.values()]) channel.dropped();
    this.channels.clear();
  }

  /**
   * Stop for good: the login this socket ran under is gone, and the page is on
   * its way to the login screen. Feeds that ask for a channel from here on get
   * one that never opens, rather than a socket of their own.
   */
  shutdown(): void {
    this.dead = true;
    this.stop();
  }

  /**
   * Drop this connection and open another at once (not a failure: no backoff,
   * nothing noted). The channels it carried are closed and their feeds come
   * back with their resume markers; the ones still waiting are asked for on
   * the new one.
   */
  private fresh(): void {
    const ws = this.ws;
    if (!ws) return;
    ws.onopen = null;
    ws.onmessage = null;
    ws.onclose = null;
    this.ws = null;
    clearInterval(this.heartbeat);
    clearTimeout(this.timer);
    this.answered();
    for (const channel of [...this.channels.values()]) {
      if (channel.readyState === OPEN) {
        this.channels.delete(channel.sub);
        channel.dropped();
      } else {
        channel.requested = false;
      }
    }
    try {
      ws.close();
    } catch {
      /* already closing */
    }
    this.retry = 0;
    this.connect();
  }

  /**
   * The page is back (in view, online again): reconnect now rather than at the
   * end of the backoff, or, on a socket that is still open, ask it to answer
   * within {@link WAKE_PROBE_MS} and replace it if it does not. The socket a
   * phone slept through often still looks open while everything sent on it is
   * lost; waiting for its heartbeat would cost a whole silence.
   */
  kick(): void {
    if (!this.running) return;
    const ws = this.ws;
    if (!ws) {
      clearTimeout(this.timer);
      this.retry = 0;
      this.connect();
      return;
    }
    // A server too old to answer pings (no `hello`) is left as it was: there is
    // nothing to probe it with, and its feeds are their own sockets anyway.
    if (ws.readyState !== OPEN || this.mode !== "mux" || this.probeTimer) return;
    this.sendRaw({ type: "ping" });
    this.probeAnswered = false;
    this.probeTimer = setTimeout(() => {
      this.probeTimer = undefined;
      if (this.ws !== ws || this.probeAnswered) return;
      // No answer: let go of it now, rather than at the next silence check.
      this.fresh();
    }, this.deps.probeMs ?? WAKE_PROBE_MS);
  }

  /** The socket answered (or is gone): no probe is pending. */
  private answered(): void {
    clearTimeout(this.probeTimer);
    this.probeTimer = undefined;
  }

  /**
   * A channel for one session's feed, or `null` when it should open a socket
   * of its own: a server that cannot carry feeds.
   */
  open(target: FeedTarget): FeedChannel | null {
    if (this.mode === "legacy") return null;
    this.ensure();
    const channel = new Channel(this.nextSub++, target, this);
    this.channels.set(channel.sub, channel);
    if (this.ws?.readyState === OPEN) {
      this.request([channel]);
      // Not now: the feed that asked for this channel has not been handed it
      // yet, let alone set its `onopen`, and an open it never hears about is a
      // retry streak it never resets. A socket opens asynchronously too.
      if (this.mode === "mux") {
        queueMicrotask(() => {
          if (this.channels.get(channel.sub) === channel && this.mode === "mux") channel.opened();
        });
      }
    }
    return channel;
  }

  /** @internal */
  sendRaw(message: Record<string, unknown>): void {
    if (this.ws?.readyState === OPEN) this.ws.send(JSON.stringify(message));
  }

  /** @internal A channel closed by its feed. */
  forget(channel: Channel, subscribed: boolean): void {
    this.channels.delete(channel.sub);
    if (subscribed) this.sendRaw({ type: "unsub", subs: [channel.sub] });
  }

  /**
   * Put these channels' subscriptions on the wire, in one message.
   *
   * Sent the moment the socket opens rather than after `hello`: a server too
   * old to carry feeds ignores whatever a client says on this socket, so asking
   * early costs nothing there and saves a round trip everywhere else — on every
   * cold start and every reconnect. A channel only counts as open once `hello`
   * has said the request was understood.
   */
  private request(channels: Channel[]): void {
    if (channels.length === 0) return;
    for (const channel of channels) channel.requested = true;
    this.sendRaw({
      type: "sub",
      feeds: channels.map(({ sub, target }) => ({
        sub,
        source: target.source,
        id: target.id,
        ...(target.since !== undefined && target.fp ? { since: target.since, fp: target.fp } : {}),
        ...(target.limit ? { limit: target.limit } : {}),
      })),
    });
  }

  private connect(): void {
    const compress = this.deps.inflate !== null && (this.deps.compress ?? notCloud)();
    this.deps.socketUrl(`/api/events?v=2${compress ? "&z=1" : ""}`).then(
      (url) => this.openSocket(url, compress),
      () => {
        if (this.running) this.retryLater();
      },
    );
  }

  private retryLater(): void {
    this.retry += 1;
    this.deps.noteFailure(this.retry);
    this.timer = setTimeout(() => this.connect(), hubBackoff(this.retry, this.deps.random));
  }

  private openSocket(url: string, compress: boolean): void {
    if (!this.running) return;
    const ws = this.deps.openSocket(url);
    ws.binaryType = "arraybuffer";
    this.ws = ws;
    // Frames kept for a list that was not listening are this connection's
    // alone: the next one starts with its own snapshot.
    this.early = [];
    this.earlyLost = false;
    this.mode = "unknown";
    this.compressed = false;
    this.askedCompression = compress;
    // Decoding is asynchronous, and frames must still be read in order. Per
    // socket: a frame still decoding for a socket that has since been replaced
    // must neither hold up the new one's frames nor be read into it.
    let decoding: Promise<void> = Promise.resolve();
    let queued = 0;
    ws.onopen = () => {
      if (this.ws !== ws) return;
      this.retry = 0;
      this.lastHeard = this.now();
      this.request([...this.channels.values()].filter((channel) => channel.readyState === 0));
      this.handlers?.onOpen();
      clearInterval(this.heartbeat);
      this.heartbeat = setInterval(() => this.beat(ws), HEARTBEAT_MS);
    };
    ws.onmessage = (e) => {
      if (this.ws !== ws) return;
      this.lastHeard = this.now();
      this.probeAnswered = true;
      this.answered();
      const data = e.data;
      // The common case, a text frame with nothing being decoded ahead of it,
      // is read at once; anything else queues behind whatever is.
      if (typeof data === "string" && queued === 0) {
        this.read(ws, data);
        return;
      }
      queued += 1;
      decoding = decoding
        .then(async () => {
          if (this.ws !== ws) return;
          // Compressed frames only on a socket that asked for them.
          if (typeof data !== "string" && !compress) throw new Error("a compressed frame on a socket that did not ask for one");
          const text =
            typeof data === "string" ? data : this.deps.inflate ? await this.deps.inflate(data as ArrayBuffer) : null;
          if (text !== null && this.ws === ws) this.read(ws, text);
        })
        .catch(() => {
          // A frame that will not decode leaves every feed on this socket
          // holding a window it can no longer trust; start over.
          if (this.ws === ws) ws.close();
        })
        .finally(() => {
          queued -= 1;
        });
    };
    ws.onclose = () => this.lost(ws);
  }

  /**
   * `ws` is gone, whether it said so or was given up on: forget it, close the
   * channels it carried, and start reconnecting.
   */
  private lost(ws: HubSocket): void {
    if (this.ws !== ws) return;
    this.ws = null;
    clearInterval(this.heartbeat);
    this.answered();
    // Every channel the server held is gone with the socket. A channel still
    // waiting to be subscribed keeps waiting for the next one.
    for (const channel of [...this.channels.values()]) {
      if (channel.readyState === OPEN) {
        this.channels.delete(channel.sub);
        channel.dropped();
      } else {
        // Asked on a socket that is gone: ask again on the next one.
        channel.requested = false;
      }
    }
    this.handlers?.onClose();
    if (this.running) this.retryLater();
  }

  private beat(ws: HubSocket): void {
    // Only a server that answers pings can be held to an answer: an older one
    // may have nothing to say for minutes on a perfectly good socket.
    if (this.ws !== ws || this.mode !== "mux") return;
    if (this.now() - this.lastHeard > SILENCE_MS) {
      // Not `close()` and wait: on a link that is black-holing packets the
      // browser waits out the closing handshake — up to a minute — before
      // `onclose`, and until then this would still hold the dead socket and
      // a `kick()` would do nothing. Let go of it now; it closes in its own
      // time with nobody listening.
      ws.onopen = null;
      ws.onmessage = null;
      ws.onclose = null;
      this.lost(ws);
      try {
        ws.close();
      } catch {
        /* already closing */
      }
      return;
    }
    this.sendRaw({ type: "ping" });
  }

  private read(ws: HubSocket, text: string): void {
    let m: Record<string, unknown>;
    try {
      m = JSON.parse(text);
    } catch {
      return;
    }
    if (this.mode === "unknown") {
      if (m.type === "hello") {
        this.mode = "mux";
        this.compressed = this.askedCompression && m.compress === "deflate-raw";
        const waiting = [...this.channels.values()].filter((channel) => channel.readyState === 0);
        // Normally all of them were asked for on open already.
        this.request(waiting.filter((channel) => !channel.requested));
        // The server answers every subscription in order on this socket, so a
        // channel is as good as open once its request is understood.
        for (const channel of waiting) channel.opened();
        return;
      }
      // No hello: a server from before subscriptions. Its feeds are each a
      // socket of their own, and the channels waiting here have to go and
      // open one.
      this.mode = "legacy";
      for (const channel of [...this.channels.values()]) {
        this.channels.delete(channel.sub);
        channel.dropped();
      }
    }
    if (typeof m.sub === "number") {
      // A frame of a channel that has since been closed is dropped: whatever
      // replaced it has its own subscription and its own frames.
      // Handed over parsed: an opening page is a few hundred kilobytes of
      // JSON, and parsing it twice is a frame the reader waits for.
      this.channels.get(m.sub)?.onmessage?.({ data: m });
      return;
    }
    if (m.type === "pong") return;
    if (this.ws !== ws) return;
    if (this.handlers) this.handlers.onFrame(m);
    else if (this.early.length < EARLY_FRAMES) this.early.push(m);
    else this.earlyLost = true;
  }
}
