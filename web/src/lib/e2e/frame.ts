/*
 * The frames the hub can see: `[type u8][ch u32][len u16][payload]`.
 *
 * The browser's half of `server-rs/crates/rh-proto/src/frame.rs`; see there for
 * what each type means. The hub routes on the envelope and copies `DATA`
 * payloads (Noise messages) without reading them. Types at or above 0x80 are
 * optional extensions a peer may skip; an unknown type below that is fatal.
 */
import { type Bytes, concat, DecodeError, fromUtf8, Reader, Writer } from "./bytes";
import { decodeSealedRecord, encodeSealedRecord, type SealedRecord } from "./record";

export const CLIENT_PATH = "/v1/client";
export const HEADER_LEN = 7;
export const MAX_FRAME_LEN = 64 * 1024;
export const MAX_PAYLOAD_LEN = MAX_FRAME_LEN - HEADER_LEN;
export const MAX_PEER_ID_LEN = 64;
export const MAX_CLOSE_REASON_LEN = 123;
export const MAX_SUBSCRIPTION_ID_LEN = 128;
export const IGNORABLE_TYPES = 0x80;
/**
 * Hub-level flow control (`rh-proto/src/flow.rs`): a sender may have this many
 * `data` payload bytes outstanding per channel and direction; the receiver
 * returns them with `credit` frames (half a window at a time) once it has read
 * them, and the hub closes a channel whose sender overruns its window.
 */
export const INITIAL_CREDIT = 512 * 1024;

export const FrameType = {
  open: 0x01,
  data: 0x02,
  close: 0x03,
  credit: 0x04,
  ping: 0x05,
  pong: 0x06,
  append: 0x10,
  ack: 0x11,
  conflict: 0x12,
  sub: 0x13,
  recs: 0x14,
  gone: 0x15,
  checkpoint: 0x16,
  ephemeral: 0x17,
  head: 0x18,
  page: 0x19,
  chain: 0x20,
  pushSend: 0x21,
  attest: 0x22,
} as const;

export type Frame =
  | { type: "open"; ch: number; peer: string }
  | { type: "data"; ch: number; payload: Bytes }
  | { type: "close"; ch: number; code: number; reason: string }
  | { type: "credit"; ch: number; bytes: number }
  | { type: "ping"; nonce: number }
  | { type: "pong"; nonce: number }
  | { type: "append"; ch: number; stream: Bytes; expectSeq: number; records: SealedRecord[] }
  | { type: "ack"; ch: number; stream: Bytes; head: number }
  | { type: "conflict"; ch: number; stream: Bytes; head: number }
  | { type: "sub"; ch: number; stream: Bytes; fromSeq: number }
  | { type: "recs"; ch: number; stream: Bytes; records: SealedRecord[] }
  | { type: "gone"; ch: number; stream: Bytes; firstSeq: number }
  | { type: "checkpoint"; ch: number; stream: Bytes; uptoSeq: number; manifest: Bytes }
  | { type: "ephemeral"; ch: number; stream: Bytes; payload: Bytes }
  | { type: "chain"; ch: number; entries: Bytes[] }
  | { type: "pushSend"; ch: number; subscription: string; ttl: number; payload: Bytes }
  | { type: "head"; ch: number; head: Bytes }
  | { type: "page"; ch: number; stream: Bytes; hash: Bytes; data: Bytes }
  | { type: "attest"; ch: number; attestation: Bytes }
  | { type: "unknown"; ty: number; ch: number; payload: Bytes };

/** A frame with its payload unparsed: all a relay needs. */
export interface RawFrame {
  ty: number;
  ch: number;
  payload: Bytes;
}

function checkLen(what: string, len: number, max: number): void {
  if (len > max) throw new DecodeError(`${what} is ${len} bytes, more than ${max}`);
}

function stream32(stream: Bytes): Bytes {
  if (stream.length !== 32) throw new DecodeError("a stream id is 32 bytes");
  return stream;
}

function putRecords(w: Writer, records: SealedRecord[]): void {
  checkLen("records", records.length, 0xffff);
  w.u16(records.length);
  for (const r of records) w.raw(encodeSealedRecord(r));
}

function getRecords(r: Reader): SealedRecord[] {
  const count = r.u16();
  const out: SealedRecord[] = [];
  for (let i = 0; i < count; i++) out.push(decodeSealedRecord(r));
  return out;
}

export function encodeRaw(frame: RawFrame): Bytes {
  checkLen("frame payload", frame.payload.length, MAX_PAYLOAD_LEN);
  return new Writer().u8(frame.ty).u32(frame.ch).u16(frame.payload.length).raw(frame.payload).finish();
}

/** The first frame in `buf` and how many bytes it took, or `null` if it has not all arrived. */
export function decodeRawPrefix(buf: Uint8Array): { frame: RawFrame; used: number } | null {
  if (buf.length < HEADER_LEN) return null;
  const view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  const len = view.getUint16(5);
  checkLen("frame payload", len, MAX_PAYLOAD_LEN);
  if (buf.length < HEADER_LEN + len) return null;
  const payload = concat(buf.subarray(HEADER_LEN, HEADER_LEN + len));
  return { frame: { ty: view.getUint8(0), ch: view.getUint32(1), payload }, used: HEADER_LEN + len };
}

export function toRaw(frame: Frame): RawFrame {
  const w = new Writer();
  let ty: number;
  let ch: number;
  switch (frame.type) {
    case "open":
      checkLen("peer id", new TextEncoder().encode(frame.peer).length, MAX_PEER_ID_LEN);
      w.str8(frame.peer);
      [ty, ch] = [FrameType.open, frame.ch];
      break;
    case "data":
      w.raw(frame.payload);
      [ty, ch] = [FrameType.data, frame.ch];
      break;
    case "close":
      checkLen("close reason", new TextEncoder().encode(frame.reason).length, MAX_CLOSE_REASON_LEN);
      w.u16(frame.code).raw(new TextEncoder().encode(frame.reason));
      [ty, ch] = [FrameType.close, frame.ch];
      break;
    case "credit":
      w.u32(frame.bytes);
      [ty, ch] = [FrameType.credit, frame.ch];
      break;
    case "ping":
    case "pong":
      w.u64(frame.nonce);
      [ty, ch] = [FrameType[frame.type], 0];
      break;
    case "append":
      w.raw(stream32(frame.stream)).u64(frame.expectSeq);
      putRecords(w, frame.records);
      [ty, ch] = [FrameType.append, frame.ch];
      break;
    case "ack":
    case "conflict":
      w.raw(stream32(frame.stream)).u64(frame.head);
      [ty, ch] = [FrameType[frame.type], frame.ch];
      break;
    case "sub":
      w.raw(stream32(frame.stream)).u64(frame.fromSeq);
      [ty, ch] = [FrameType.sub, frame.ch];
      break;
    case "recs":
      w.raw(stream32(frame.stream));
      putRecords(w, frame.records);
      [ty, ch] = [FrameType.recs, frame.ch];
      break;
    case "gone":
      w.raw(stream32(frame.stream)).u64(frame.firstSeq);
      [ty, ch] = [FrameType.gone, frame.ch];
      break;
    case "checkpoint":
      w.raw(stream32(frame.stream)).u64(frame.uptoSeq).raw(frame.manifest);
      [ty, ch] = [FrameType.checkpoint, frame.ch];
      break;
    case "ephemeral":
      w.raw(stream32(frame.stream)).raw(frame.payload);
      [ty, ch] = [FrameType.ephemeral, frame.ch];
      break;
    case "chain":
      checkLen("chain entries", frame.entries.length, 0xffff);
      w.u16(frame.entries.length);
      for (const e of frame.entries) {
        checkLen("chain entry", e.length, 0xffff);
        w.u16(e.length).raw(e);
      }
      [ty, ch] = [FrameType.chain, frame.ch];
      break;
    case "pushSend":
      checkLen("subscription id", new TextEncoder().encode(frame.subscription).length, MAX_SUBSCRIPTION_ID_LEN);
      w.str8(frame.subscription).u32(frame.ttl).raw(frame.payload);
      [ty, ch] = [FrameType.pushSend, frame.ch];
      break;
    case "head":
      w.raw(frame.head);
      [ty, ch] = [FrameType.head, frame.ch];
      break;
    case "page":
      if (frame.hash.length !== 32) throw new DecodeError("a page hash is 32 bytes");
      w.raw(stream32(frame.stream)).raw(frame.hash).raw(frame.data);
      [ty, ch] = [FrameType.page, frame.ch];
      break;
    case "attest":
      w.raw(frame.attestation);
      [ty, ch] = [FrameType.attest, frame.ch];
      break;
    case "unknown":
      if (frame.ty < IGNORABLE_TYPES) throw new DecodeError(`unknown critical frame type ${frame.ty}`);
      w.raw(frame.payload);
      [ty, ch] = [frame.ty, frame.ch];
      break;
  }
  const payload = w.finish();
  checkLen("frame payload", payload.length, MAX_PAYLOAD_LEN);
  return { ty, ch, payload };
}

export function fromRaw(raw: RawFrame): Frame {
  const { ch } = raw;
  const r = new Reader(raw.payload);
  let frame: Frame;
  switch (raw.ty) {
    case FrameType.open:
      frame = { type: "open", ch, peer: r.str8("peer id", MAX_PEER_ID_LEN) };
      break;
    case FrameType.data:
      frame = { type: "data", ch, payload: r.rest() };
      break;
    case FrameType.close: {
      const code = r.u16();
      const reason = r.rest();
      checkLen("close reason", reason.length, MAX_CLOSE_REASON_LEN);
      frame = { type: "close", ch, code, reason: fromUtf8(reason) };
      break;
    }
    case FrameType.credit:
      frame = { type: "credit", ch, bytes: r.u32() };
      break;
    case FrameType.ping:
    case FrameType.pong:
      if (ch !== 0) throw new DecodeError("ping on a channel other than 0");
      frame = { type: raw.ty === FrameType.ping ? "ping" : "pong", nonce: r.u64() };
      break;
    case FrameType.append: {
      const stream = r.bytes(32);
      const expectSeq = r.u64();
      const records = getRecords(r);
      records.forEach((rec, i) => {
        // Both are below 2^53 (the reader refuses more), so the sum is exact;
        // one past the bound is refused as Rust's checked add refuses it.
        if (expectSeq + i > Number.MAX_SAFE_INTEGER || rec.seq !== expectSeq + i) {
          throw new DecodeError("append records that are not consecutive from expect_seq");
        }
      });
      frame = { type: "append", ch, stream, expectSeq, records };
      break;
    }
    case FrameType.ack:
      frame = { type: "ack", ch, stream: r.bytes(32), head: r.u64() };
      break;
    case FrameType.conflict:
      frame = { type: "conflict", ch, stream: r.bytes(32), head: r.u64() };
      break;
    case FrameType.sub:
      frame = { type: "sub", ch, stream: r.bytes(32), fromSeq: r.u64() };
      break;
    case FrameType.recs:
      frame = { type: "recs", ch, stream: r.bytes(32), records: getRecords(r) };
      break;
    case FrameType.gone:
      frame = { type: "gone", ch, stream: r.bytes(32), firstSeq: r.u64() };
      break;
    case FrameType.checkpoint:
      frame = { type: "checkpoint", ch, stream: r.bytes(32), uptoSeq: r.u64(), manifest: r.rest() };
      break;
    case FrameType.ephemeral:
      frame = { type: "ephemeral", ch, stream: r.bytes(32), payload: r.rest() };
      break;
    case FrameType.chain: {
      const count = r.u16();
      const entries: Bytes[] = [];
      for (let i = 0; i < count; i++) entries.push(r.bytes(r.u16()));
      frame = { type: "chain", ch, entries };
      break;
    }
    case FrameType.pushSend:
      frame = { type: "pushSend", ch, subscription: r.str8("subscription id", MAX_SUBSCRIPTION_ID_LEN), ttl: r.u32(), payload: r.rest() };
      break;
    case FrameType.head:
      frame = { type: "head", ch, head: r.rest() };
      break;
    case FrameType.page:
      frame = { type: "page", ch, stream: r.bytes(32), hash: r.bytes(32), data: r.rest() };
      break;
    case FrameType.attest:
      frame = { type: "attest", ch, attestation: r.rest() };
      break;
    default:
      if (raw.ty >= IGNORABLE_TYPES) return { type: "unknown", ty: raw.ty, ch, payload: raw.payload };
      throw new DecodeError(`unknown critical frame type ${raw.ty}`);
  }
  r.finish();
  return frame;
}

export function encodeFrame(frame: Frame): Bytes {
  return encodeRaw(toRaw(frame));
}

/** Exactly one frame, and nothing after it. */
export function decodeFrame(buf: Uint8Array): Frame {
  const got = decodeRawPrefix(buf);
  if (!got) throw new DecodeError("truncated input");
  if (got.used !== buf.length) throw new DecodeError(`${buf.length - got.used} trailing byte(s) after a frame`);
  return fromRaw(got.frame);
}

/** Splits a byte stream (or several frames batched in one websocket message) into frames. */
export class FrameDecoder {
  private buf: Bytes = new Uint8Array(new ArrayBuffer(0));

  push(data: Uint8Array): void {
    this.buf = concat(this.buf, data);
  }

  /** The next whole frame, or `null`. Throws once framing is lost; the caller must close. */
  next(): RawFrame | null {
    const got = decodeRawPrefix(this.buf);
    if (!got) return null;
    this.buf = concat(this.buf.subarray(got.used));
    return got.frame;
  }
}
