/*
 * An established channel, running: the browser's `droppen-proto/src/driver.rs`.
 *
 * A `ChannelDriver` owns a `Session` (Noise transport and sub-stream
 * multiplexer) for one hub `ch`. The connection that carries it hands it
 * every frame for that `ch` (`receive`) and takes the frames it produces
 * (`send`, given at construction); the page opens `ChannelStream`s on it.
 *
 * - Hub credit (`flow.ts`): a new frame is sealed only while the hub window
 *   could take a whole one, and credit is handed back as soon as data is read.
 * - Stream windows (`mux.ts`): data is acknowledged as it is handed to the
 *   stream's `ondata`, so a page that is slow to read slows nobody but the
 *   tab itself; nothing a page reads is held back.
 * - Everything is serialised: WebCrypto is asynchronous, and Noise nonces
 *   must be sealed and opened strictly in order.
 *
 * Two paths (`options.paths`, when both ends negotiated `CLIENT_FEATURE`):
 * every frame is sequenced, and a direct path (a Noise session over a WebRTC
 * DataChannel, `lib/transport/p2p.ts`) can be attached with `attachDirect`.
 * New frames go that way while it lasts. When the session stops using it —
 * it timed out, the peer gave it up, a message on it did not open, its owner
 * said the DataChannel closed (`detachDirect`), or the channel ended — the
 * link's `released` is called so its owner tears the peer connection down,
 * and what it did not deliver goes again over the hub. Sending on it pauses
 * while the DataChannel has `DIRECT_BUDGET` buffered; `wake` resumes.
 *
 * Any failure (a hub message that does not decrypt, a protocol violation)
 * ends the channel: it is closed at the hub, and every stream hears
 * `onclose`. Trouble with the direct path alone only lets that path go.
 */
import type { Bytes } from "./bytes";
import { type PathOptions, type Room, Session } from "./channel";
import { RecvWindow, SendWindow } from "./flow";
import { type Frame, MAX_PAYLOAD_LEN } from "./frame";
import type { MuxEvent, Role, StreamClass } from "./mux";
import type { Transport } from "./noise";
import type { DownReason } from "./path";
import { Serial } from "./serial";

/** A stream's reason for ending when its channel went first. */
export const CHANNEL_GONE = 4502;
/** What may sit in a direct path's DataChannel buffer before the driver stops handing it more. */
export const DIRECT_BUDGET = 1024 * 1024;

/** The paths' state, for the path indicator and logs. */
export interface PathStats {
  /** Whether the direct path carries the channel now. */
  direct: boolean;
  /** The latest round trip measured on each path (ms), once there is one. */
  relayRttMs: number | null;
  directRttMs: number | null;
  /** How often a direct path was attached, and let go. */
  ups: number;
  downs: number;
  /** Frames sent again over the hub after a direct path failed. */
  resent: number;
  /** Frames that arrived twice (a resend of something the direct path had delivered) and were dropped. */
  duplicates: number;
}

/**
 * A direct path's DataChannel as the driver uses it once the handshake on it
 * is done: `send` one message, how much is still buffered, and `released`,
 * called once when the driver stops using it (it failed, timed out, the peer
 * gave it up, another path replaced it, or the channel ended).
 */
export interface DirectLink {
  send(message: Bytes): void;
  bufferedAmount(): number;
  released?(reason: DownReason): void;
}

export interface DriverOptions {
  /** Speak `paths/1`: every frame sequenced, a direct path can be attached. */
  paths?: PathOptions;
  /** The monotonic clock path timers and round trips are measured on (ms); `performance.now` by default. */
  clock?: () => number;
}

export class ChannelStream {
  ondata: ((data: Bytes) => void) | null = null;
  onclose: ((code: number) => void) | null = null;
  private ended = false;

  constructor(
    readonly sid: number,
    readonly cls: StreamClass,
    readonly meta: Bytes,
    private readonly driver: ChannelDriver,
  ) {}

  /** Queue `data` (sent as credit allows). Nothing once the stream ended. */
  send(data: Uint8Array): void {
    if (this.ended) return;
    this.driver.streamSend(this, data);
  }

  /** Close once what was sent before has gone. */
  close(code = 0): void {
    if (this.ended) return;
    this.driver.streamClose(this, code);
  }

  get isEnded(): boolean {
    return this.ended;
  }

  /** @internal */
  end(code: number): void {
    if (this.ended) return;
    this.ended = true;
    this.onclose?.(code);
  }
}

function sameStats(a: PathStats | null, b: PathStats): boolean {
  return (
    a !== null &&
    a.direct === b.direct &&
    a.relayRttMs === b.relayRttMs &&
    a.directRttMs === b.directRttMs &&
    a.ups === b.ups &&
    a.downs === b.downs &&
    a.resent === b.resent &&
    a.duplicates === b.duplicates
  );
}

export class ChannelDriver {
  /** A stream the peer opened. */
  onincoming: ((stream: ChannelStream) => void) | null = null;
  /** The channel ended (`code` from the hub's or the peer's close, or a failure). */
  onclose: ((code: number, reason: string) => void) | null = null;
  /** The paths' state changed (a channel with paths only). */
  onpaths: ((stats: PathStats) => void) | null = null;

  private readonly session: Session;
  private readonly streams = new Map<number, ChannelStream>();
  private readonly serial = new Serial();
  private readonly sendWindow = new SendWindow();
  private readonly recvWindow = new RecvWindow();
  private readonly clock: () => number;
  private held: Frame | null = null;
  private closed = false;
  private direct: DirectLink | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private stats: PathStats | null = null;

  constructor(
    readonly ch: number,
    transport: Transport,
    role: Role,
    private readonly send: (frame: Frame) => void,
    private readonly now: () => number = Date.now,
    options: DriverOptions = {},
  ) {
    this.clock = options.clock ?? (() => performance.now());
    if (options.paths) {
      this.session = Session.withPaths(ch, transport, role, options.paths, this.clock());
      this.stats = this.readStats();
      // Its first timers (the first hub probe) are due at once.
      this.kick();
    } else {
      this.session = new Session(ch, transport, role);
    }
  }

  get isClosed(): boolean {
    return this.closed;
  }

  /** Whether this channel sequences its frames (and so can take a direct path). */
  get hasPaths(): boolean {
    return this.session.hasPaths;
  }

  /** The paths' state; `null` on a channel without paths. */
  get paths(): PathStats | null {
    return this.stats;
  }

  /** The hub channel's Noise handshake hash, which a direct path is bound to. */
  get handshakeHash(): Bytes {
    return this.session.transport.handshakeHash;
  }

  open(cls: StreamClass, meta: Uint8Array): ChannelStream {
    if (this.closed) {
      const dead = new ChannelStream(-1, cls, Uint8Array.from(meta), this);
      queueMicrotask(() => dead.end(CHANNEL_GONE));
      return dead;
    }
    const sid = this.session.mux.open(cls, meta);
    const stream = new ChannelStream(sid, cls, Uint8Array.from(meta), this);
    this.streams.set(sid, stream);
    this.kick();
    return stream;
  }

  /** @internal */
  streamSend(stream: ChannelStream, data: Uint8Array): void {
    if (this.closed || !this.streams.has(stream.sid)) return;
    try {
      this.session.mux.send(stream.sid, data);
    } catch {
      return;
    }
    this.kick();
  }

  /** @internal */
  streamClose(stream: ChannelStream, code: number): void {
    if (this.closed || !this.streams.has(stream.sid)) return;
    try {
      this.session.mux.close(stream.sid, code);
    } catch {
      // already closing
    }
    this.kick();
  }

  /** One frame from the hub on this channel's `ch`. */
  receive(frame: Frame): void {
    this.run(async () => {
      switch (frame.type) {
        case "data": {
          const grant = this.recvWindow.received(frame.payload.length);
          if (grant !== null) this.send({ type: "credit", ch: this.ch, bytes: grant });
          for (const event of await this.session.receiveRelay(frame.payload, this.clock())) this.dispatch(event);
          this.sweep();
          return true;
        }
        case "credit":
          this.sendWindow.grant(frame.bytes);
          return true;
        case "close":
          this.finish(frame.code, frame.reason, false);
          return false;
        default:
          return false;
      }
    });
  }

  /**
   * Carry the channel over a direct path whose handshake has completed:
   * new frames go that way until it fails. A path already attached is let
   * go first. On a channel without paths, or one that ended, `link` is
   * released at once.
   */
  attachDirect(transport: Transport, link: DirectLink): void {
    void this.serial.run(async () => {
      if (this.closed || !this.session.hasPaths) {
        link.released?.("closed");
        return;
      }
      const old = this.direct;
      this.direct = null;
      if (old && old !== link) old.released?.("closed");
      try {
        this.session.attachDirect(transport, this.clock());
        this.direct = link;
        await this.step();
      } catch (error) {
        this.finish(4400, error instanceof Error ? error.message : String(error), true);
      }
    });
  }

  /**
   * One message from the direct path's DataChannel. With `link`, only while
   * that link is the one attached: a late message from a path already let go
   * is dropped rather than read with the next path's keys.
   */
  receiveDirect(message: Uint8Array, link?: DirectLink): void {
    this.run(async () => {
      if (link && link !== this.direct) return false;
      for (const event of await this.session.receiveDirect(message, this.clock())) this.dispatch(event);
      this.sweep();
      return true;
    });
  }

  /** Stop using the direct path (`link`, if given, only while it is the one attached), and tell the peer. */
  detachDirect(reason: DownReason = "failed", link?: DirectLink): void {
    this.run(async () => {
      if (!this.direct || (link && link !== this.direct)) return false;
      this.session.detachDirect(reason, true);
      return true;
    });
  }

  /** Something that held sending back has cleared (the DataChannel's buffer drained). */
  wake(): void {
    this.kick();
  }

  /** End the channel from this side. */
  close(code = 1000, reason = ""): void {
    void this.serial.run(async () => this.finish(code, reason, true));
  }

  private kick(): void {
    this.run(async () => true);
  }

  /** `work`, in turn; then, unless it says nothing moved, publish the paths, send what can go and set the timer. */
  private run(work: () => Promise<boolean>): void {
    void this.serial.run(async () => {
      if (this.closed) return;
      try {
        if (await work()) await this.step();
      } catch (error) {
        this.finish(4400, error instanceof Error ? error.message : String(error), true);
      }
    });
  }

  private async step(): Promise<void> {
    if (this.closed) return;
    this.pathsChanged();
    await this.pump();
    this.schedule();
  }

  private dispatch(event: MuxEvent): void {
    switch (event.type) {
      case "opened": {
        const stream = new ChannelStream(event.sid, event.class, event.meta, this);
        this.streams.set(event.sid, stream);
        if (this.onincoming) this.onincoming(stream);
        else stream.close(4403);
        break;
      }
      case "data": {
        const stream = this.streams.get(event.sid);
        this.session.mux.consumed(event.sid, event.data.length);
        stream?.ondata?.(event.data);
        break;
      }
      case "closed": {
        const stream = this.streams.get(event.sid);
        this.streams.delete(event.sid);
        stream?.end(event.code);
        break;
      }
      case "pong":
        break;
    }
  }

  /** Streams this end closed are forgotten when the peer answers: tell their readers. */
  private sweep(): void {
    for (const [sid, stream] of this.streams) {
      if (!this.session.mux.isOpen(sid)) {
        this.streams.delete(sid);
        stream.end(0);
      }
    }
  }

  private readStats(): PathStats {
    const s = this.session.stats();
    return {
      direct: s.direct,
      relayRttMs: s.relayRttMs,
      directRttMs: s.directRttMs,
      ups: s.ups,
      downs: s.downs,
      resent: s.lanes.resent,
      duplicates: s.lanes.duplicates,
    };
  }

  /** Let go of a direct link the session stopped using, and publish the paths' state when it changed. */
  private pathsChanged(): void {
    if (!this.session.hasPaths) return;
    let reason: DownReason = "closed";
    for (const event of this.session.takeEvents()) if (event.type === "directDown") reason = event.reason;
    if (this.direct && !this.session.directActive) {
      const link = this.direct;
      this.direct = null;
      link.released?.(reason);
    }
    const stats = this.readStats();
    if (!sameStats(this.stats, stats)) {
      this.stats = stats;
      this.onpaths?.(stats);
    }
  }

  private room(): Room {
    return {
      relay: this.held === null && this.sendWindow.canSend(MAX_PAYLOAD_LEN),
      direct: this.direct !== null && this.direct.bufferedAmount() < DIRECT_BUDGET,
    };
  }

  /** Send what the hub's credit and the direct path's budget allow. Only seals a hub frame when any would fit. */
  private async pump(): Promise<void> {
    for (;;) {
      if (this.held) {
        const len = this.held.type === "data" ? this.held.payload.length : 0;
        if (this.sendWindow.canSend(len)) {
          this.sendWindow.consume(len);
          this.send(this.held);
          this.held = null;
        }
      }
      const room = this.room();
      if (!room.relay && !room.direct) return;
      const polled = await this.session.poll(room, this.clock(), this.now());
      if (!polled) return;
      if (polled.wire.type === "relay") {
        this.held = polled.wire.frame;
        continue;
      }
      try {
        if (!this.direct) throw new Error("no direct path");
        this.direct.send(polled.wire.message);
      } catch {
        // Its DataChannel is gone: what it did not deliver goes over the hub.
        this.session.detachDirect("failed", true);
        this.pathsChanged();
      }
    }
  }

  /** A timer for the session's next deadline: probes, the direct path's timeout, a due acknowledgement. */
  private schedule(): void {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    if (this.closed) return;
    const at = this.session.nextDeadlineMs(this.room());
    if (at === null) return;
    this.timer = setTimeout(
      () => {
        this.timer = null;
        this.run(async () => {
          this.session.tick(this.clock());
          return true;
        });
      },
      Math.max(0, Math.ceil(at - this.clock())),
    );
  }

  private finish(code: number, reason: string, tellHub: boolean): void {
    if (this.closed) return;
    this.closed = true;
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    if (tellHub) this.send({ type: "close", ch: this.ch, code, reason: reason.slice(0, 120) });
    // Whatever ended the channel ends its direct path with it.
    const link = this.direct;
    this.direct = null;
    link?.released?.("closed");
    for (const stream of this.streams.values()) stream.end(code === 1000 ? CHANNEL_GONE : code);
    this.streams.clear();
    this.onclose?.(code, reason);
  }
}
