/*
 * The direct path: a WebRTC DataChannel between this device and its machine,
 * next to the hub, carrying the same channel faster when the two can reach
 * each other. The browser's half of `server-rs/crates/droppen-proto/src/path.rs`,
 * byte for byte (read that for the why); everything here is sans-IO.
 *
 * - `Signal`: offer, answer, ICE candidates and goodbyes, as binary messages
 *   on a `mux` sub-stream of the existing end-to-end channel
 *   (`SIGNAL_PATH`). The hub relays them as ciphertext.
 * - Fingerprint binding: each side states its own DTLS certificate's SHA-256
 *   next to its SDP, and the receiver refuses an SDP naming anything else
 *   (`checkFingerprint`).
 * - Noise on the DataChannel (`DirectBinding`): a fresh IK handshake with the
 *   same static keys, under a prologue that binds the account, device and
 *   machine, the hub channel's handshake hash, the agent's nonce for this
 *   attempt and both fingerprints.
 * - `Envelope` and `Lanes`: sequenced, acknowledged frames, so the
 *   multiplexer can move between the hub and the direct path with nothing
 *   lost, repeated or reordered.
 * - `DcMessage`: what one DataChannel message is.
 */
import { type Bytes, concat, DecodeError, equal, fromUtf8, Reader, utf8, Writer } from "./bytes";
import { MAX_PAYLOAD_LEN } from "./frame";
import { type Binding, prologue as bindingPrologue } from "./noise";

/** What a device puts in its hello's client info, and an agent in its features: sequenced frames. */
export const CLIENT_FEATURE = "paths/1";
/** In an agent's features: it answers direct-path signaling on `SIGNAL_PATH`. */
export const DIRECT_FEATURE = "direct/1";
/** The agent-local route of the signaling sub-stream (`mux` class). */
export const SIGNAL_PATH = "/rh/direct";
/** The DataChannel's label. */
export const DC_LABEL = "rh";

/** `kind u8 + seq u64 + ack u64`: what a data envelope adds to a frame. */
export const ENVELOPE_HEADER = 1 + 8 + 8;
/** The most bytes a receiver holds while it waits for a gap to fill. */
export const MAX_HELD_BYTES = 16 * 1024 * 1024;
/** How far ahead of the next expected frame a sequence number may be. */
export const MAX_AHEAD = 2 ** 20;
/** No new frames go on the direct path while this much sent there is unacknowledged. */
export const UNACKED_LIMIT = 4 * 1024 * 1024;
/** A frame received on the direct path is acknowledged within this long… */
export const ACK_DELAY_MS = 15;
/** …or after this many, whichever is first. */
export const ACK_EVERY = 16;
/** The direct path is probed this often (also its keep-alive)… */
export const DIRECT_PROBE_MS = 1_000;
/** …and declared dead when nothing has come over it for this long. */
export const DIRECT_TIMEOUT_MS = 3_500;
/**
 * While frames sent on the direct path wait for an acknowledgement, it is
 * declared dead once it has made no progress for this long (or ten round
 * trips, whichever is longer): sooner than `DIRECT_TIMEOUT_MS` of silence, so
 * a typist waits less for a path that went dark mid-typing. Progress is data
 * from the peer arriving over it, or an acknowledgement freeing what was sent
 * over it — not the mere age of the oldest frame: a path slowly draining a
 * backlog (a DataChannel buffer filled faster than a cellular link empties it,
 * with the peer's acknowledgements queued behind its own) is not dead.
 */
export const UNACKED_TIMEOUT_MS = 1_500;
/** The hub path is probed this often for its round-trip time (for display). */
export const RELAY_PROBE_MS = 5_000;
/**
 * A relay path whose oldest probe has gone unanswered this long is treated as
 * gone and the channel is replaced. Without it the path only raised the round
 * trip it showed (the probe's age) until the relay link's own 60 s silence,
 * so a dead peer kept a "20005 ms" indicator and a stalled channel for most
 * of a minute.
 */
export const RELAY_SILENT_MS = 2 * RELAY_PROBE_MS;

/** The longest SDP a signal carries. */
export const MAX_SDP = 16 * 1024;
/** The longest ICE candidate line. */
export const MAX_CANDIDATE = 1024;
/** The longest goodbye detail. */
export const MAX_DETAIL = 256;
/** The prologue's leading label for a direct-path handshake. */
export const DIRECT_PROLOGUE_LABEL = utf8("rh/v1 direct\0");

const ENV_DATA = 0x01;
const ENV_ACK = 0x02;
const ENV_PROBE = 0x03;
const ENV_PROBE_ACK = 0x04;
const ENV_DOWN = 0x05;

const DC_HELLO = 0x01;
const DC_REPLY = 0x02;
const DC_SEALED = 0x03;

const SIG_OFFER = 0x01;
const SIG_ANSWER = 0x02;
const SIG_CANDIDATE = 0x03;
const SIG_BYE = 0x04;

export class PathError extends Error {
  constructor(
    readonly kind: "protocol" | "fingerprint" | "overflow",
    message: string,
  ) {
    super(message);
    this.name = "PathError";
  }
}

/** Which way a message went. */
export type Via = "relay" | "direct";

/** Why a side stopped using the direct path. */
export type DownReason = "closed" | "timeout" | "failed";
const DOWN_CODE: Record<DownReason, number> = { closed: 0, timeout: 1, failed: 2 };
const DOWN_NAME: DownReason[] = ["closed", "timeout", "failed"];

/**
 * One transport plaintext on a channel that negotiated `CLIENT_FEATURE`.
 * `seq` counts data envelopes per direction from 1; `ack` is the highest
 * `seq` the sender has received with nothing missing before it. `down` goes
 * on the hub path only.
 */
export type Envelope =
  | { type: "data"; seq: number; ack: number; frame: Bytes }
  | { type: "ack"; ack: number }
  | { type: "probe"; nonce: number }
  | { type: "probeAck"; nonce: number }
  | { type: "down"; reason: DownReason };

export function encodeEnvelope(e: Envelope): Bytes {
  const w = new Writer();
  switch (e.type) {
    case "data":
      return w.u8(ENV_DATA).u64(e.seq).u64(e.ack).raw(e.frame).finish();
    case "ack":
      return w.u8(ENV_ACK).u64(e.ack).finish();
    case "probe":
      return w.u8(ENV_PROBE).u64(e.nonce).finish();
    case "probeAck":
      return w.u8(ENV_PROBE_ACK).u64(e.nonce).finish();
    case "down":
      return w.u8(ENV_DOWN).u8(DOWN_CODE[e.reason]).finish();
  }
}

export function decodeEnvelope(data: Uint8Array): Envelope {
  const r = new Reader(data);
  let env: Envelope;
  const kind = r.u8();
  switch (kind) {
    case ENV_DATA: {
      const seq = r.u64();
      const ack = r.u64();
      if (seq === 0) throw new DecodeError("a data envelope with seq 0");
      env = { type: "data", seq, ack, frame: r.rest() };
      break;
    }
    case ENV_ACK:
      env = { type: "ack", ack: r.u64() };
      break;
    case ENV_PROBE:
      env = { type: "probe", nonce: r.u64() };
      break;
    case ENV_PROBE_ACK:
      env = { type: "probeAck", nonce: r.u64() };
      break;
    case ENV_DOWN: {
      const code = r.u8();
      const reason = DOWN_NAME[code];
      if (!reason) throw new DecodeError(`unknown down reason ${code}`);
      env = { type: "down", reason };
      break;
    }
    default:
      throw new DecodeError(`unknown envelope ${kind}`);
  }
  r.finish();
  return env;
}

/** One DataChannel message: the direct handshake's two messages, then sealed envelopes. */
export interface DcMessage {
  type: "hello" | "reply" | "sealed";
  body: Bytes;
}
const DC_CODE: Record<DcMessage["type"], number> = { hello: DC_HELLO, reply: DC_REPLY, sealed: DC_SEALED };
const DC_NAME: Record<number, DcMessage["type"]> = { [DC_HELLO]: "hello", [DC_REPLY]: "reply", [DC_SEALED]: "sealed" };

export function encodeDcMessage(m: DcMessage): Bytes {
  return concat(Uint8Array.of(DC_CODE[m.type]), m.body);
}

export function decodeDcMessage(data: Uint8Array): DcMessage {
  if (data.length < 1) throw new DecodeError("truncated input");
  const body = data.subarray(1);
  if (body.length > MAX_PAYLOAD_LEN) throw new DecodeError(`a direct message of ${body.length} bytes, more than ${MAX_PAYLOAD_LEN}`);
  const type = DC_NAME[data[0]!];
  if (!type) throw new DecodeError(`unknown direct message ${data[0]}`);
  return { type, body: concat(body) };
}

/** Why a side ended a direct attempt. */
export type ByeReason = "done" | "disabled" | "busy" | "failed" | "badFingerprint" | "refused";
const BYE_CODE: Record<ByeReason, number> = { done: 0, disabled: 1, busy: 2, failed: 3, badFingerprint: 4, refused: 5 };
const BYE_NAME: ByeReason[] = ["done", "disabled", "busy", "failed", "badFingerprint", "refused"];

/**
 * A signaling message on the channel's `SIGNAL_PATH` sub-stream. `id` names
 * one attempt (the device counts them per channel); `fp` is the sender's own
 * DTLS certificate's SHA-256; `nonce` is the agent's per-attempt value for
 * the direct handshake's prologue; a `null` candidate means no more.
 */
export type Signal =
  | { type: "offer"; id: number; fp: Bytes; sdp: string }
  | { type: "answer"; id: number; fp: Bytes; nonce: Bytes; sdp: string }
  | { type: "candidate"; id: number; candidate: string | null }
  | { type: "bye"; id: number; reason: ByeReason; detail: string };

function bounded(what: string, text: string, max: number): void {
  const len = utf8(text).length;
  if (len > max) throw new DecodeError(`${what} is ${len} bytes, more than ${max}`);
}

function fp32(what: string, b: Uint8Array): Uint8Array {
  if (b.length !== 32) throw new DecodeError(`${what} is ${b.length} bytes, not 32`);
  return b;
}

export function encodeSignal(s: Signal): Bytes {
  const w = new Writer();
  switch (s.type) {
    case "offer":
      bounded("sdp", s.sdp, MAX_SDP);
      return w.u8(SIG_OFFER).u32(s.id).raw(fp32("fp", s.fp)).str16(s.sdp).finish();
    case "answer":
      bounded("sdp", s.sdp, MAX_SDP);
      return w.u8(SIG_ANSWER).u32(s.id).raw(fp32("fp", s.fp)).raw(fp32("nonce", s.nonce)).str16(s.sdp).finish();
    case "candidate": {
      const c = s.candidate ?? "";
      bounded("candidate", c, MAX_CANDIDATE);
      return w.u8(SIG_CANDIDATE).u32(s.id).str16(c).finish();
    }
    case "bye":
      bounded("detail", s.detail, MAX_DETAIL);
      return w.u8(SIG_BYE).u32(s.id).u8(BYE_CODE[s.reason]).str16(s.detail).finish();
  }
}

export function decodeSignal(data: Uint8Array): Signal {
  const r = new Reader(data);
  let s: Signal;
  const kind = r.u8();
  switch (kind) {
    case SIG_OFFER:
      s = { type: "offer", id: r.u32(), fp: r.bytes(32), sdp: r.str16("sdp", MAX_SDP) };
      break;
    case SIG_ANSWER:
      s = { type: "answer", id: r.u32(), fp: r.bytes(32), nonce: r.bytes(32), sdp: r.str16("sdp", MAX_SDP) };
      break;
    case SIG_CANDIDATE: {
      const id = r.u32();
      const c = r.str16("candidate", MAX_CANDIDATE);
      s = { type: "candidate", id, candidate: c === "" ? null : c };
      break;
    }
    case SIG_BYE: {
      const id = r.u32();
      const code = r.u8();
      const reason = BYE_NAME[code];
      if (!reason) throw new DecodeError(`unknown bye reason ${code}`);
      s = { type: "bye", id, reason, detail: r.str16("detail", MAX_DETAIL) };
      break;
    }
    default:
      throw new DecodeError(`unknown signal ${kind}`);
  }
  r.finish();
  return s;
}

/** `AB:CD:…` (32 colon-separated bytes) as bytes, or `null`. */
export function parseFingerprintHex(text: string): Bytes | null {
  const parts = text.split(":");
  if (parts.length !== 32) return null;
  const out = new Uint8Array(new ArrayBuffer(32));
  for (let i = 0; i < 32; i++) {
    const part = parts[i]!;
    if (!/^[0-9a-fA-F]{2}$/.test(part)) return null;
    out[i] = parseInt(part, 16);
  }
  return out;
}

/** `AB:CD:…`, the SDP spelling. */
export function fingerprintHex(fp: Uint8Array): string {
  return [...fp].map((b) => b.toString(16).toUpperCase().padStart(2, "0")).join(":");
}

/** The `sha-256` fingerprint an SDP names (`a=fingerprint:sha-256 AB:CD:…`), which must be the only one it names. */
export function sdpFingerprint(sdp: string): Bytes {
  let found: Bytes | null = null;
  for (const raw of sdp.split("\n")) {
    const line = raw.replace(/\r+$/, "");
    if (!line.startsWith("a=fingerprint:")) continue;
    const value = line.slice("a=fingerprint:".length).trim();
    const space = value.indexOf(" ");
    if (space < 0) throw new PathError("fingerprint", "malformed a=fingerprint");
    if (!/^sha-256$/i.test(value.slice(0, space))) throw new PathError("fingerprint", "only sha-256 fingerprints are accepted");
    const fp = parseFingerprintHex(value.slice(space + 1).trim());
    if (!fp) throw new PathError("fingerprint", "malformed sha-256 fingerprint");
    if (found && !equal(found, fp)) throw new PathError("fingerprint", "an SDP with two different fingerprints");
    found = fp;
  }
  if (!found) throw new PathError("fingerprint", "an SDP without a fingerprint");
  return found;
}

/** Refuse `sdp` unless its fingerprint is exactly the `stated` one. */
export function checkFingerprint(sdp: string, stated: Uint8Array): void {
  if (!equal(sdpFingerprint(sdp), stated)) throw new PathError("fingerprint", "the SDP names a different certificate than the one stated");
}

/** An SDP without its ICE candidates (they trickle as signals). */
export function stripCandidates(sdp: string): string {
  return sdp
    .split("\r\n")
    .filter((l) => !l.startsWith("a=candidate:") && l !== "a=end-of-candidates")
    .join("\r\n");
}

/** What a direct-path handshake is bound to (its Noise prologue). */
export interface DirectBinding {
  /** The same `account | device | machine` as the hub channel. */
  binding: Binding;
  /** The hub channel's Noise handshake hash. */
  relayHash: Uint8Array;
  /** The agent's nonce for this attempt (from its answer). */
  nonce: Uint8Array;
  /** The device's DTLS certificate fingerprint, as its offer stated it. */
  deviceFp: Uint8Array;
  /** The agent's DTLS certificate fingerprint, as its answer stated it. */
  agentFp: Uint8Array;
}

/** `"rh/v1 direct\0" | str16 account | str16 device | str16 machine | relay_hash | nonce | device_fp | agent_fp`. */
export function directPrologue(b: DirectBinding): Bytes {
  // The hub channel's prologue checks the ids the same way.
  bindingPrologue(b.binding);
  return new Writer()
    .raw(DIRECT_PROLOGUE_LABEL)
    .str16(b.binding.account)
    .str16(b.binding.device)
    .str16(b.binding.machine)
    .raw(fp32("relay hash", b.relayHash))
    .raw(fp32("nonce", b.nonce))
    .raw(fp32("device fingerprint", b.deviceFp))
    .raw(fp32("agent fingerprint", b.agentFp))
    .finish();
}

/** Whether `info` (a hello's client info, space-separated tokens) names `CLIENT_FEATURE`. */
export function clientWantsPaths(info: Uint8Array): boolean {
  try {
    return fromUtf8(info)
      .split(/\s+/)
      .some((t) => t === CLIENT_FEATURE);
  } catch {
    return false;
  }
}

/** The `features` an agent's server info (`{"agent","version","features":[…]}`) lists; none for anything else. */
export function serverFeatures(server: Uint8Array): string[] {
  try {
    const parsed: unknown = JSON.parse(fromUtf8(server));
    if (typeof parsed !== "object" || parsed === null) return [];
    const features = (parsed as { features?: unknown }).features;
    return Array.isArray(features) ? features.filter((f): f is string => typeof f === "string") : [];
  } catch {
    return [];
  }
}

/** What the sequencing has done so far. */
export interface LaneStats {
  /** Frames handed up in order. */
  delivered: number;
  /** Frames that arrived again and were dropped. */
  duplicates: number;
  /** Frames that arrived ahead of a gap and were held. */
  held: number;
  /** Frames sent again over the hub after the direct path failed. */
  resent: number;
}

/**
 * Sequencing for one channel's two directions: numbers outgoing frames, keeps
 * the ones sent on the direct path until they are acknowledged, and hands
 * incoming frames up exactly once, in order, whichever path brought them.
 */
export class Lanes {
  private nextSeq = 1;
  /** `[seq, frame, sent at ms]` for what went over the direct path. */
  private unacked: [number, Bytes, number][] = [];
  private unackedLen = 0;
  private peerAcked = 0;
  private rxNext = 1;
  private readonly held = new Map<number, Bytes>();
  private heldBytes = 0;
  private owedCount = 0;
  private owedSinceMs: number | null = null;
  private readonly counts: LaneStats = { delivered: 0, duplicates: 0, held: 0, resent: 0 };

  stats(): LaneStats {
    return { ...this.counts };
  }

  /** The highest `seq` received with nothing missing before it; the acknowledgement owed is paid. */
  takeAck(): number {
    this.owedCount = 0;
    this.owedSinceMs = null;
    return this.rxNext - 1;
  }

  /** Frames received on the direct path that have not been acknowledged yet. */
  get owed(): number {
    return this.owedCount;
  }

  /** When a standalone acknowledgement becomes due, if one is owed. */
  ackDueMs(): number | null {
    if (this.owedSinceMs === null) return null;
    return this.owedCount >= ACK_EVERY ? this.owedSinceMs : this.owedSinceMs + ACK_DELAY_MS;
  }

  /** Bytes sent on the direct path and not acknowledged yet. */
  get unackedBytes(): number {
    return this.unackedLen;
  }

  /** Number the next outgoing frame; `keep` it until acknowledged when its path can lose it. */
  stamp(frame: Bytes, keep: boolean): Envelope {
    return this.stampAt(frame, keep, 0);
  }

  /** `stamp`, noting when a kept frame left, so a direct path that stops acknowledging can be given up early. */
  stampAt(frame: Bytes, keep: boolean, nowMs: number): Envelope {
    const seq = this.nextSeq++;
    if (keep) {
      this.unackedLen += frame.length;
      this.unacked.push([seq, frame, nowMs]);
    }
    return { type: "data", seq, ack: this.takeAck(), frame };
  }

  /** When the oldest frame still waiting for an acknowledgement left. */
  oldestUnackedMs(): number | null {
    return this.unacked[0]?.[2] ?? null;
  }

  /**
   * The peer has everything up to `ack`: how many of the frames kept for the
   * direct path that frees. An ack for something never sent is a protocol error.
   */
  onAck(ack: number): number {
    if (ack >= this.nextSeq) throw new PathError("protocol", "an acknowledgement for a frame never sent");
    if (ack <= this.peerAcked) return 0;
    this.peerAcked = ack;
    let n = 0;
    while (n < this.unacked.length && this.unacked[n]![0] <= ack) {
      this.unackedLen -= this.unacked[n]![1].length;
      n++;
    }
    if (n > 0) this.unacked.splice(0, n);
    return n;
  }

  /** One incoming frame: those now ready to hand up, in order. `owe` when it came on the direct path. */
  receive(seq: number, frame: Bytes, owe: boolean, nowMs: number): Bytes[] {
    if (seq === 0) throw new PathError("protocol", "seq 0");
    if (owe) {
      this.owedCount += 1;
      this.owedSinceMs ??= nowMs;
    }
    if (seq < this.rxNext || this.held.has(seq)) {
      this.counts.duplicates += 1;
      return [];
    }
    if (seq - this.rxNext > MAX_AHEAD) throw new PathError("protocol", "a sequence number far beyond the next expected");
    if (seq > this.rxNext) {
      this.heldBytes += frame.length;
      if (this.heldBytes > MAX_HELD_BYTES) throw new PathError("overflow", `more than ${MAX_HELD_BYTES} bytes held waiting for a gap`);
      this.held.set(seq, frame);
      this.counts.held += 1;
      return [];
    }
    const out = [frame];
    this.rxNext += 1;
    for (let next = this.held.get(this.rxNext); next; next = this.held.get(this.rxNext)) {
      this.held.delete(this.rxNext);
      this.heldBytes -= next.length;
      out.push(next);
      this.rxNext += 1;
    }
    this.counts.delivered += out.length;
    return out;
  }

  /** Everything sent on the direct path and not acknowledged, to send again over the hub. */
  takeUnacked(): Envelope[] {
    const ack = this.takeAck();
    this.unackedLen = 0;
    const out: Envelope[] = this.unacked.map(([seq, frame]) => ({ type: "data", seq, ack, frame }));
    this.unacked = [];
    this.counts.resent += out.length;
    return out;
  }
}
