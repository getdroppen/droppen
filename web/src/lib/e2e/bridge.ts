/*
 * What a sub-stream carries when the agent bridges it to its own loopback API
 * (`crates/droppen-proto/src/bridge.rs` — read that for the why):
 *
 * - every stream opens with a request head as its `open` metadata: method,
 *   path (with query) and headers;
 * - `rpc`: the device sends the body as `binary` messages and then `end`; the
 *   agent answers with one `head` (status and headers), the body as `binary`
 *   messages, and closes the stream with code 0;
 * - `mux` / `term`: the agent's `head` with status 101 is the socket's `open`
 *   (any other status is a refused upgrade, followed by a close); after it,
 *   each websocket message either way is one `text` or `binary` message.
 *
 * A stream's bytes are `[kind u8][len u32][payload]` messages, so boundaries
 * survive however the multiplexer chunks them (`MessageReader`).
 */
import { type Bytes, bytes, concat, DecodeError, Reader, Writer } from "./bytes";

export const BRIDGE_CLOSE_DONE = 0;
export const BRIDGE_CLOSE_BAD_REQUEST = 4400;
export const BRIDGE_CLOSE_FORBIDDEN = 4403;
export const BRIDGE_CLOSE_UPSTREAM = 4502;
export const BRIDGE_MAX_MESSAGE = 32 * 1024 * 1024;
export const BRIDGE_MAX_HEADERS = 64;
const MAX_FIELD = 4096;
const HEAD_VERSION = 1;

export type BridgeKind = "text" | "binary" | "head" | "end";
const KIND_TAG: Record<BridgeKind, number> = { text: 1, binary: 2, head: 3, end: 4 };
const KIND_NAME: Record<number, BridgeKind> = { 1: "text", 2: "binary", 3: "head", 4: "end" };

export interface RequestHead {
  method: string;
  path: string;
  headers: [string, string][];
}

export interface ResponseHead {
  status: number;
  headers: [string, string][];
}

function putHeaders(w: Writer, headers: [string, string][]): void {
  if (headers.length > BRIDGE_MAX_HEADERS) throw new DecodeError("too many headers");
  w.u16(headers.length);
  for (const [name, value] of headers) w.str16(name).str16(value);
}

function getHeaders(r: Reader): [string, string][] {
  const n = r.u16();
  if (n > BRIDGE_MAX_HEADERS) throw new DecodeError("too many headers");
  const out: [string, string][] = [];
  for (let i = 0; i < n; i++) out.push([r.str16("header name", MAX_FIELD), r.str16("header value", MAX_FIELD)]);
  return out;
}

export function encodeRequestHead(h: RequestHead): Bytes {
  const w = new Writer().u8(HEAD_VERSION).str16(h.method).str16(h.path);
  putHeaders(w, h.headers);
  return w.finish();
}

export function decodeRequestHead(data: Uint8Array): RequestHead {
  const r = new Reader(data);
  if (r.u8() !== HEAD_VERSION) throw new DecodeError("request head version");
  const head = { method: r.str16("method", MAX_FIELD), path: r.str16("path", MAX_FIELD), headers: getHeaders(r) };
  r.finish();
  return head;
}

export function encodeResponseHead(h: ResponseHead): Bytes {
  const w = new Writer().u8(HEAD_VERSION).u16(h.status);
  putHeaders(w, h.headers);
  return w.finish();
}

export function decodeResponseHead(data: Uint8Array): ResponseHead {
  const r = new Reader(data);
  if (r.u8() !== HEAD_VERSION) throw new DecodeError("response head version");
  const head = { status: r.u16(), headers: getHeaders(r) };
  r.finish();
  return head;
}

/** One message, framed for a stream. */
export function bridgeMessage(kind: BridgeKind, payload: Uint8Array = bytes(0)): Bytes {
  return concat(new Writer().u8(KIND_TAG[kind]).u32(payload.length).finish(), payload);
}

/** Reassembles messages from a stream's data, however it was chunked. An error poisons the stream. */
export class MessageReader {
  private buf: Bytes = bytes(0);

  push(data: Uint8Array): void {
    this.buf = concat(this.buf, data);
  }

  next(): { kind: BridgeKind; payload: Bytes } | null {
    if (this.buf.length < 5) return null;
    const kind = KIND_NAME[this.buf[0]!];
    if (!kind) throw new DecodeError(`unknown bridge message kind ${this.buf[0]}`);
    const len = new DataView(this.buf.buffer, this.buf.byteOffset + 1, 4).getUint32(0);
    if (len > BRIDGE_MAX_MESSAGE) throw new DecodeError("bridge message too large");
    if (this.buf.length < 5 + len) return null;
    const payload = this.buf.slice(5, 5 + len);
    this.buf = this.buf.slice(5 + len);
    return { kind, payload };
  }
}
