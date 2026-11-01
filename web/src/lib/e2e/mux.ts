/*
 * Sub-streams inside one end-to-end channel: `rpc`, `mux`, `term` and `blob`,
 * each with a credit window, drained by priority — control > term > rpc/mux >
 * blob — and round-robin within one. The browser's half of
 * `server-rs/crates/droppen-proto/src/mux.rs`, frame for frame; the shared vectors
 * hold both to the same encoding *and* the same scheduling order.
 *
 * Stream ids: odd for streams the device (initiator) opens, even for the
 * agent's, increasing, never reused. `OPEN` carries the opener's receive
 * window; the opener may send `INITIAL_WINDOW` before hearing otherwise.
 * `CLOSE` queues behind the stream's data and is answered by the peer's own.
 */
import { type Bytes, copy, DecodeError, Reader, Writer } from "./bytes";
import { MAX_TRANSPORT_DATA } from "./noise";
import { ENVELOPE_HEADER } from "./path";

export const INITIAL_WINDOW = 256 * 1024;
export const DATA_HEADER = 5;
/**
 * The most data one `DATA` frame carries: what fits in one transport message
 * after the frame's header and, on a channel with two paths, the sequencing
 * envelope's (`path.ts` `ENVELOPE_HEADER`).
 */
export const MAX_CHUNK = MAX_TRANSPORT_DATA - ENVELOPE_HEADER - DATA_HEADER;
export const BLOB_CHUNK = 16 * 1024;
export const MAX_META = 8 * 1024;
export const MAX_STREAMS = 1024;

export type StreamClass = "rpc" | "mux" | "term" | "blob";

const CLASS_CODE: Record<StreamClass, number> = { rpc: 1, mux: 2, term: 3, blob: 4 };
const CLASS_BY_CODE: Record<number, StreamClass> = { 1: "rpc", 2: "mux", 3: "term", 4: "blob" };

/** 1 is drained first; control frames are 0. */
export function priority(cls: StreamClass): 1 | 2 | 3 {
  return cls === "term" ? 1 : cls === "blob" ? 3 : 2;
}

function chunkSize(cls: StreamClass): number {
  return cls === "blob" ? BLOB_CHUNK : MAX_CHUNK;
}

export type MuxFrame =
  | { type: "open"; sid: number; class: StreamClass; window: number; meta: Bytes }
  | { type: "data"; sid: number; data: Bytes }
  | { type: "credit"; sid: number; delta: number }
  | { type: "close"; sid: number; code: number }
  | { type: "ping"; nonce: number }
  | { type: "pong"; nonce: number };

export function encodeMuxFrame(f: MuxFrame): Bytes {
  const w = new Writer();
  switch (f.type) {
    case "open":
      return w.u8(1).u32(f.sid).u8(CLASS_CODE[f.class]).u32(f.window).raw(f.meta).finish();
    case "data":
      return w.u8(2).u32(f.sid).raw(f.data).finish();
    case "credit":
      return w.u8(3).u32(f.sid).u32(f.delta).finish();
    case "close":
      return w.u8(4).u32(f.sid).u16(f.code).finish();
    case "ping":
      return w.u8(5).u64(f.nonce).finish();
    case "pong":
      return w.u8(6).u64(f.nonce).finish();
  }
}

export function decodeMuxFrame(data: Uint8Array): MuxFrame {
  const r = new Reader(data);
  let f: MuxFrame;
  const kind = r.u8();
  switch (kind) {
    case 1: {
      const sid = r.u32();
      const code = r.u8();
      const cls = CLASS_BY_CODE[code];
      if (!cls) throw new DecodeError(`unknown stream class ${code}`);
      const window = r.u32();
      const meta = r.rest();
      if (meta.length > MAX_META) throw new DecodeError("open metadata too large");
      f = { type: "open", sid, class: cls, window, meta };
      break;
    }
    case 2:
      f = { type: "data", sid: r.u32(), data: r.rest() };
      break;
    case 3:
      f = { type: "credit", sid: r.u32(), delta: r.u32() };
      break;
    case 4:
      f = { type: "close", sid: r.u32(), code: r.u16() };
      break;
    case 5:
      f = { type: "ping", nonce: r.u64() };
      break;
    case 6:
      f = { type: "pong", nonce: r.u64() };
      break;
    default:
      throw new DecodeError(`unknown mux frame ${kind}`);
  }
  r.finish();
  return f;
}

export type MuxEvent =
  | { type: "opened"; sid: number; class: StreamClass; meta: Bytes }
  | { type: "data"; sid: number; data: Bytes }
  | { type: "closed"; sid: number; code: number }
  | { type: "pong"; nonce: number };

export class MuxError extends Error {
  constructor(
    readonly kind: "protocol" | "flowControl" | "unknownStream" | "tooManyStreams",
    message: string,
  ) {
    super(message);
    this.name = "MuxError";
  }
}

interface Stream {
  cls: StreamClass;
  sendCredit: number;
  recvWindow: number;
  recvUnacked: number;
  pending: Bytes[];
  pendingClose: number | null;
  localClosed: boolean;
}

export type Role = "initiator" | "responder";

/** One end's sub-stream state. Feed it frames with `receive`, take frames to send from `poll`. */
export class Mux {
  private nextSid: number;
  private peerNextSid: number;
  private readonly streams = new Map<number, Stream>();
  private readonly control: MuxFrame[] = [];
  private readonly lastServed = [0, 0, 0, 0];

  constructor(
    readonly role: Role,
    private readonly window: number = INITIAL_WINDOW,
  ) {
    this.nextSid = role === "initiator" ? 1 : 2;
    this.peerNextSid = role === "initiator" ? 2 : 1;
  }

  private isMine(sid: number): boolean {
    return (sid % 2 === 1) === (this.role === "initiator");
  }

  private sorted(): [number, Stream][] {
    return [...this.streams.entries()].sort((a, b) => a[0] - b[0]);
  }

  open(cls: StreamClass, meta: Uint8Array = new Uint8Array(0)): number {
    if (meta.length > MAX_META) throw new MuxError("protocol", "open metadata too large");
    if (this.streams.size >= MAX_STREAMS) throw new MuxError("tooManyStreams", "too many open streams");
    const sid = this.nextSid;
    this.nextSid += 2;
    this.streams.set(sid, {
      cls,
      sendCredit: INITIAL_WINDOW,
      recvWindow: this.window,
      recvUnacked: 0,
      pending: [],
      pendingClose: null,
      localClosed: false,
    });
    this.control.push({ type: "open", sid, class: cls, window: this.window, meta: copy(meta) });
    return sid;
  }

  send(sid: number, data: Uint8Array): void {
    const s = this.streams.get(sid);
    if (!s || s.pendingClose !== null || s.localClosed) throw new MuxError("unknownStream", `no such stream ${sid}`);
    const size = chunkSize(s.cls);
    for (let at = 0; at < data.length; at += size) s.pending.push(copy(data.subarray(at, at + size)));
  }

  close(sid: number, code: number): void {
    const s = this.streams.get(sid);
    if (!s || s.localClosed) throw new MuxError("unknownStream", `no such stream ${sid}`);
    s.pendingClose ??= code;
  }

  consumed(sid: number, n: number): void {
    const s = this.streams.get(sid);
    if (!s) return;
    const window = this.isMine(sid) ? this.window : INITIAL_WINDOW;
    s.recvUnacked += n;
    if (s.recvUnacked >= window / 2) {
      const delta = Math.min(s.recvUnacked, 0xffffffff);
      s.recvUnacked -= delta;
      s.recvWindow += delta;
      this.control.push({ type: "credit", sid, delta });
    }
  }

  ping(nonce: number): void {
    this.control.push({ type: "ping", nonce });
  }

  private static ready(s: Stream): boolean {
    return (s.pending.length > 0 && s.sendCredit > 0) || (s.pending.length === 0 && s.pendingClose !== null);
  }

  hasPending(): boolean {
    return this.control.length > 0 || [...this.streams.values()].some(Mux.ready);
  }

  poll(): MuxFrame | null {
    const ctl = this.control.shift();
    if (ctl) return ctl;
    for (const p of [1, 2, 3]) {
      const ready = this.sorted()
        .filter(([, s]) => priority(s.cls) === p && Mux.ready(s))
        .map(([sid]) => sid);
      if (ready.length === 0) continue;
      const sid = ready.find((x) => x > this.lastServed[p]!) ?? ready[0]!;
      this.lastServed[p] = sid;
      const s = this.streams.get(sid)!;
      const front = s.pending[0];
      if (front) {
        const n = Math.min(front.length, s.sendCredit);
        let data: Bytes;
        if (n === front.length) {
          data = s.pending.shift()!;
        } else {
          data = copy(front.subarray(0, n));
          s.pending[0] = copy(front.subarray(n));
        }
        s.sendCredit -= n;
        return { type: "data", sid, data };
      }
      const code = s.pendingClose!;
      s.pendingClose = null;
      s.localClosed = true;
      return { type: "close", sid, code };
    }
    return null;
  }

  receive(f: MuxFrame): MuxEvent | null {
    switch (f.type) {
      case "open": {
        if (this.isMine(f.sid)) throw new MuxError("protocol", "the peer opened a stream with this end's parity");
        if (f.sid < this.peerNextSid) throw new MuxError("protocol", "a stream id was reused or went backwards");
        if (this.streams.size >= MAX_STREAMS) throw new MuxError("tooManyStreams", "too many open streams");
        this.peerNextSid = f.sid + 2;
        this.streams.set(f.sid, {
          cls: f.class,
          sendCredit: f.window,
          recvWindow: INITIAL_WINDOW,
          recvUnacked: 0,
          pending: [],
          pendingClose: null,
          localClosed: false,
        });
        return { type: "opened", sid: f.sid, class: f.class, meta: f.meta };
      }
      case "data": {
        const s = this.streams.get(f.sid);
        if (!s) return this.forgotten(f.sid);
        if (f.data.length > s.recvWindow) throw new MuxError("flowControl", `stream ${f.sid} sent past its window`);
        s.recvWindow -= f.data.length;
        return { type: "data", sid: f.sid, data: f.data };
      }
      case "credit": {
        const s = this.streams.get(f.sid);
        if (!s) return this.forgotten(f.sid);
        if (s.sendCredit + f.delta > 0xffffffff) throw new MuxError("protocol", "credit overflow");
        s.sendCredit += f.delta;
        return null;
      }
      case "close": {
        const s = this.streams.get(f.sid);
        if (!s) return this.forgotten(f.sid);
        this.streams.delete(f.sid);
        if (s.localClosed) return null;
        this.control.push({ type: "close", sid: f.sid, code: f.code });
        return { type: "closed", sid: f.sid, code: f.code };
      }
      case "ping":
        this.control.push({ type: "pong", nonce: f.nonce });
        return null;
      case "pong":
        return { type: "pong", nonce: f.nonce };
    }
  }

  private forgotten(sid: number): null {
    const existed = this.isMine(sid) ? sid < this.nextSid : sid < this.peerNextSid;
    if (!existed) throw new MuxError("unknownStream", `no such stream ${sid}`);
    return null;
  }

  isOpen(sid: number): boolean {
    return this.streams.has(sid);
  }
}
