/*
 * Encrypted log records, browser side: how a device reads the streams an
 * agent wrote to the hub (`server-rs/crates/rh-proto/src/record.rs` — read that
 * for the why).
 *
 * - `stream_id = HMAC-SHA256(K_id, source ‖ 0 ‖ id)`, `K_id = HKDF(salt = "rh/v1 stream-id", ikm = MK)`
 * - `SK = HKDF-SHA256(salt = "rh/v1 stream-key", ikm = MK, info = stream_id)`
 * - AAD `"rh/v1 record\0" ‖ stream_id ‖ seq ‖ epoch`; plaintext
 *   `[flags][fmt u16][prev_link 32][body_len u32][body][zero padding]`.
 * - `link_n = SHA-256("rh/v1 record-link\0" ‖ stream ‖ link_{n-1} ‖ record_n)`.
 * - Records are released only under a stream head the machine signed, which
 *   also says how current the view is (`LogReader.confirm`, `freshestMs`).
 * - A host that renewed its identity key (`docs/host-identity-rotation.md`
 *   §5.4): a head from the network counts under its current key only; a
 *   position this device verified and kept, under any key it ever held
 *   (`LogReader.fromHead`'s `storedKeys`, `verifyStreamHeadUnder`).
 *
 * `MK` is a non-extractable HKDF `CryptoKey`; the stream-id HMAC key and every
 * stream key are derived from it straight into non-extractable keys.
 * `LogReader` is serialised (two concurrent `push`es once took a record twice).
 */
import { type Bytes, bytes, concat, copy, DecodeError, equal, Reader, utf8, Writer } from "./bytes";
import { sha256 } from "./keys";
import { Serial } from "./serial";
import { signLabelled, STREAM_HEAD, verifyLabelled } from "./sig";

export interface SealedRecord {
  seq: number;
  epoch: number;
  nonce: Bytes;
  ct: Bytes;
}

export interface OpenedRecord {
  seq: number;
  fmt: number;
  prevLink: Bytes;
  body: Bytes;
}

export const FMT_FEED_V1 = 1;
export const BUCKETS = [256, 512, 1024, 2048, 4096, 8192, 16_384, 32_768, 61_440] as const;
export const MAX_PLAINTEXT = 61_440;
export const MAX_BODY = 4 * 1024 * 1024;
const TAG_LEN = 16;
export const MAX_CT = MAX_PLAINTEXT + TAG_LEN;
export const MAX_SOURCE_LEN = 32;
export const MAX_STREAM_ID_LEN = 256;
const HEADER = 1 + 2 + 32 + 4;
const FLAG_DEFLATE = 0x01;
const STREAM_ID_SALT = utf8("rh/v1 stream-id");
const STREAM_KEY_SALT = utf8("rh/v1 stream-key");
const AAD_LABEL = utf8("rh/v1 record\0");
const LINK_LABEL = utf8("rh/v1 record-link\0");

export class RecordError extends Error {
  constructor(
    readonly kind: "decrypt" | "noKey" | "malformed" | "tooLarge" | "outOfOrder" | "brokenChain" | "epochRollback" | "badName" | "badHead" | "forged" | "incomplete",
    message: string,
  ) {
    super(message);
    this.name = "RecordError";
  }
}

export type Compression = "none" | "deflate";

export function encodeSealedRecord(r: SealedRecord): Bytes {
  if (r.nonce.length !== 12) throw new DecodeError("a record nonce is 12 bytes");
  return new Writer().u64(r.seq).u32(r.epoch).raw(r.nonce).u32(r.ct.length).raw(r.ct).finish();
}

export function decodeSealedRecord(r: Reader): SealedRecord {
  const seq = r.u64();
  const epoch = r.u32();
  const nonce = r.bytes(12);
  const len = r.u32();
  if (len > MAX_CT) throw new DecodeError("record ciphertext too large");
  if (len < TAG_LEN) throw new DecodeError("record ciphertext shorter than its tag");
  return { seq, epoch, nonce, ct: r.bytes(len) };
}

export function decodeSealedRecordBytes(data: Uint8Array): SealedRecord {
  const r = new Reader(data);
  const rec = decodeSealedRecord(r);
  r.finish();
  return rec;
}

/** One epoch of a machine key: the HKDF key, and the stream-id HMAC key derived from it. */
export interface MachineKey {
  hkdf: CryptoKey;
  hmac: CryptoKey;
}

/** Derive the record layer's keys from `MK` as an HKDF `CryptoKey` (as `wrap.ts` unwraps it). */
export async function machineKeyFrom(hkdf: CryptoKey): Promise<MachineKey> {
  const hmac = await crypto.subtle.deriveKey(
    { name: "HKDF", hash: "SHA-256", salt: STREAM_ID_SALT, info: new Uint8Array(0) },
    hkdf,
    { name: "HMAC", hash: "SHA-256", length: 256 },
    false,
    ["sign"],
  );
  return { hkdf, hmac };
}

/** From raw bytes (vectors, tests). */
export async function importMachineKey(raw: Uint8Array): Promise<MachineKey> {
  return machineKeyFrom(await crypto.subtle.importKey("raw", copy(raw), "HKDF", false, ["deriveKey", "deriveBits"]));
}

export async function streamId(mk: MachineKey, source: string, id: string): Promise<Bytes> {
  const s = utf8(source);
  const i = utf8(id);
  if (s.length === 0 || s.length > MAX_SOURCE_LEN || i.length > MAX_STREAM_ID_LEN || s.includes(0) || i.includes(0)) {
    throw new RecordError("badName", "a stream name's source must be 1-32 bytes and neither part may contain NUL");
  }
  return new Uint8Array(await crypto.subtle.sign("HMAC", mk.hmac, concat(s, Uint8Array.of(0), i)));
}

export async function streamKey(mk: MachineKey, stream: Uint8Array): Promise<CryptoKey> {
  return crypto.subtle.deriveKey({ name: "HKDF", hash: "SHA-256", salt: STREAM_KEY_SALT, info: copy(stream) }, mk.hkdf, { name: "AES-GCM", length: 256 }, false, [
    "encrypt",
    "decrypt",
  ]);
}

function aad(stream: Uint8Array, seq: number, epoch: number): Bytes {
  return new Writer().raw(AAD_LABEL).raw(stream).u64(seq).u32(epoch).finish();
}

export async function recordLink(stream: Uint8Array, prevLink: Uint8Array, record: SealedRecord): Promise<Bytes> {
  return sha256(LINK_LABEL, stream, prevLink, encodeSealedRecord(record));
}

export function bucket(len: number): number | null {
  return BUCKETS.find((b) => b >= len) ?? null;
}

async function pipe(data: Uint8Array, stream: CompressionStream | DecompressionStream, limit: number): Promise<Bytes> {
  const reader = new Blob([copy(data)]).stream().pipeThrough(stream).getReader();
  const parts: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.length;
    if (total > limit) {
      await reader.cancel();
      throw new RecordError("malformed", "body inflates past the limit");
    }
    parts.push(value);
  }
  return concat(...parts);
}

/** Raw deflate, as a record or page body is compressed. */
export function deflateRaw(data: Uint8Array): Promise<Bytes> {
  return pipe(data, new CompressionStream("deflate-raw"), MAX_BODY * 2);
}

/** Raw inflate, refusing anything that inflates past {@link MAX_BODY}. */
export async function inflateRaw(data: Uint8Array): Promise<Bytes> {
  try {
    return await pipe(data, new DecompressionStream("deflate-raw"), MAX_BODY);
  } catch (error) {
    if (error instanceof RecordError) throw error;
    throw new RecordError("malformed", "body does not inflate");
  }
}

export async function sealRecord(
  key: CryptoKey,
  stream: Uint8Array,
  seq: number,
  epoch: number,
  fmt: number,
  prevLink: Uint8Array,
  body: Uint8Array,
  compression: Compression = "none",
  nonce: Uint8Array = crypto.getRandomValues(bytes(12)),
): Promise<SealedRecord> {
  if (body.length > MAX_BODY) throw new RecordError("tooLarge", "body too large");
  const compressed = compression === "deflate" ? await pipe(body, new CompressionStream("deflate-raw"), MAX_BODY * 2) : null;
  const [flags, stored] = compressed && compressed.length < body.length ? [FLAG_DEFLATE, compressed] : [0, body];
  const padded = bucket(HEADER + stored.length);
  if (padded === null) throw new RecordError("tooLarge", `the body does not fit a record (${stored.length} bytes)`);
  const pt = bytes(padded);
  pt.set(new Writer().u8(flags).u16(fmt).raw(prevLink).u32(stored.length).raw(stored).finish());
  const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv: copy(nonce), additionalData: aad(stream, seq, epoch) }, key, pt));
  pt.fill(0);
  return { seq, epoch, nonce: copy(nonce), ct };
}

export async function openRecord(key: CryptoKey, stream: Uint8Array, record: SealedRecord): Promise<OpenedRecord> {
  let pt: Bytes;
  try {
    pt = new Uint8Array(
      await crypto.subtle.decrypt({ name: "AES-GCM", iv: copy(record.nonce), additionalData: aad(stream, record.seq, record.epoch) }, key, copy(record.ct)),
    );
  } catch {
    throw new RecordError("decrypt", "the record does not decrypt under this stream's key (tampered, moved, or the wrong epoch)");
  }
  if (!(BUCKETS as readonly number[]).includes(pt.length)) throw new RecordError("malformed", "plaintext is not a bucket size");
  const r = new Reader(pt);
  const flags = r.u8();
  if ((flags & ~FLAG_DEFLATE) !== 0) throw new RecordError("malformed", "unknown flags");
  const fmt = r.u16();
  const prevLink = r.bytes(32);
  const len = r.u32();
  if (len > r.remaining) throw new RecordError("malformed", "body longer than the record");
  const stored = r.bytes(len);
  if (r.rest().some((b) => b !== 0)) throw new RecordError("malformed", "non-zero padding");
  let body: Bytes;
  if (flags & FLAG_DEFLATE) {
    try {
      body = await pipe(stored, new DecompressionStream("deflate-raw"), MAX_BODY);
    } catch (error) {
      if (error instanceof RecordError) throw error;
      throw new RecordError("malformed", "body does not inflate");
    }
  } else {
    body = stored;
  }
  return { seq: record.seq, fmt, prevLink, body };
}

/** The machine's signed statement about a stream (see the Rust side). */
export interface StreamHead {
  stream: Bytes;
  nextSeq: number;
  epoch: number;
  link: Bytes;
  timeMs: number;
}

export interface SignedHead {
  head: StreamHead;
  signature: Bytes;
}

export function encodeStreamHead(h: StreamHead): Bytes {
  return new Writer().raw(h.stream).u64(h.nextSeq).u32(h.epoch).raw(h.link).u64(h.timeMs).finish();
}

export function encodeSignedHead(s: SignedHead): Bytes {
  return concat(encodeStreamHead(s.head), s.signature);
}

export function decodeSignedHead(data: Uint8Array): SignedHead {
  const r = new Reader(data);
  const head = { stream: r.bytes(32), nextSeq: r.u64(), epoch: r.u32(), link: r.bytes(32), timeMs: r.u64() };
  const signature = r.bytes(64);
  r.finish();
  return { head, signature };
}

export async function signStreamHead(head: StreamHead, machine: CryptoKey): Promise<SignedHead> {
  return { head, signature: await signLabelled(machine, STREAM_HEAD, encodeStreamHead(head)) };
}

export function verifyStreamHead(head: SignedHead, machineKey: Uint8Array): Promise<boolean> {
  return verifyLabelled(machineKey, STREAM_HEAD, encodeStreamHead(head.head), head.signature);
}

/**
 * Whether any of `keys` signed `head`: for a position this device verified
 * and kept itself, which may carry a key the host has renewed away from since
 * (`docs/host-identity-rotation.md` §5.4). Never for a head from the network:
 * that is held to the host's current key alone.
 */
export async function verifyStreamHeadUnder(head: SignedHead, keys: readonly Uint8Array[]): Promise<boolean> {
  for (const key of keys) if (await verifyStreamHead(head, key)) return true;
  return false;
}

/**
 * Every record, in order, linked to the last — and none released until a
 * machine-signed head covers it. The same rules as the Rust `LogReader`, check
 * for check, including what happens on a refusal: a head that says records are
 * missing or forged drops everything not yet released and puts the reader back
 * at {@link LogReader.confirmed}, so its owner can fetch from there again.
 */
export class LogReader {
  private readonly keys = new Map<number, CryptoKey>();
  private readonly serial = new Serial();
  private pending: { opened: OpenedRecord; link: Bytes; epoch: number }[] = [];
  private confirmedNext: number;
  private confirmedLink: Bytes;
  private confirmedEpoch: number;
  private nextSeq: number;
  private link: Bytes;
  private epoch: number;
  private freshest: number | null;
  /** The newest signed head at exactly {@link confirmed}: what a saved reader restarts from. */
  private anchor: SignedHead | null;
  /** Bumped by every change of {@link machineKey}: a head checked under one key never commits under the next (Codex round 7). */
  private keyGen = 0;

  private constructor(
    readonly stream: Bytes,
    private signKey: Bytes,
    start: { nextSeq: number; link: Bytes; epoch: number; timeMs: number | null; anchor: SignedHead | null },
  ) {
    this.confirmedNext = start.nextSeq;
    this.confirmedLink = copy(start.link);
    this.confirmedEpoch = start.epoch;
    this.nextSeq = start.nextSeq;
    this.link = copy(start.link);
    this.epoch = start.epoch;
    this.freshest = start.timeMs;
    this.anchor = start.anchor;
  }

  static fromStart(stream: Uint8Array, machineKey: Uint8Array): LogReader {
    return new LogReader(copy(stream), copy(machineKey), { nextSeq: 0, link: bytes(32), epoch: 0, timeMs: null, anchor: null });
  }

  /**
   * Start from a signed head (older records compacted away, or a reader saved
   * at it). Every head after it is held to `machineKey`, the host's current
   * key; `head` itself may instead carry one of `storedKeys` — keys the host
   * renewed away from — when it is a position this device verified and kept
   * (a saved copy, a restart from the confirmed head), never one just
   * received.
   */
  static async fromHead(machineKey: Uint8Array, head: SignedHead, storedKeys: readonly Uint8Array[] = []): Promise<LogReader> {
    if (!(await verifyStreamHeadUnder(head, [machineKey, ...storedKeys]))) throw new RecordError("badHead", "the stream head's signature is not the machine's");
    const h = head.head;
    return new LogReader(copy(h.stream), copy(machineKey), { nextSeq: h.nextSeq, link: copy(h.link), epoch: h.epoch, timeMs: h.timeMs, anchor: head });
  }

  /** The key every head it is given must carry: the host's current one. */
  get machineKey(): Bytes {
    return copy(this.signKey);
  }

  /** The host renewed its identity key: heads from now on are held to `key` (the chain's current one), not the old. */
  setMachineKey(key: Uint8Array): void {
    if (!equal(key, this.signKey)) this.keyGen++;
    this.signKey = copy(key);
  }

  async addEpoch(epoch: number, mk: MachineKey): Promise<void> {
    this.keys.set(epoch, await streamKey(mk, this.stream));
  }

  /** The stream key of `epoch`, if this reader has it (for pages and drafts of the same stream). */
  keyFor(epoch: number): CryptoKey | undefined {
    return this.keys.get(epoch);
  }

  get confirmed(): number {
    return this.confirmedNext;
  }

  /** The running hash at {@link confirmed}. */
  get confirmedChainLink(): Bytes {
    return copy(this.confirmedLink);
  }

  /** The newest machine-signed head standing exactly at {@link confirmed}, if any. */
  get confirmedHead(): SignedHead | null {
    return this.anchor;
  }

  /** The time of the newest machine-signed head: how current this view is known to be. */
  get freshestMs(): number | null {
    return this.freshest;
  }

  /** Take the next record: checked and held, not yet released. Serialised. */
  push(record: SealedRecord): Promise<void> {
    return this.serial.run(async () => {
      if (record.seq !== this.nextSeq) {
        throw new RecordError("outOfOrder", `expected seq ${this.nextSeq}, got ${record.seq}: a record was replayed, reordered or dropped`);
      }
      if (record.epoch < this.epoch) throw new RecordError("epochRollback", `epoch went backwards at seq ${record.seq}`);
      const key = this.keys.get(record.epoch);
      if (!key) throw new RecordError("noKey", `no key for epoch ${record.epoch}`);
      const opened = await openRecord(key, this.stream, record);
      if (!equal(opened.prevLink, this.link)) throw new RecordError("brokenChain", `the link is broken at seq ${record.seq}`);
      this.link = await recordLink(this.stream, this.link, record);
      this.nextSeq += 1;
      this.epoch = record.epoch;
      this.pending.push({ opened, link: this.link, epoch: record.epoch });
    });
  }

  /** Release what a signed head covers (see the Rust side for each refusal). Serialised. */
  confirm(head: SignedHead): Promise<OpenedRecord[]> {
    return this.serial.run(async () => {
      const gen = this.keyGen;
      if (!equal(head.head.stream, this.stream) || !(await verifyStreamHead(head, this.signKey))) {
        throw new RecordError("badHead", "the stream head's signature is not the machine's");
      }
      // The machine renewed its key while the head was being checked: it was checked under a key that is retired now, and
      // is held to the new one instead (refused here; the caller tries it again under the current key).
      if (gen !== this.keyGen) throw new RecordError("badHead", "the machine renewed its key while its head was checked");
      const h = head.head;
      if (h.nextSeq < this.confirmedNext) return [];
      if (h.nextSeq > this.nextSeq) {
        const have = this.nextSeq;
        this.rollBack();
        throw new RecordError("incomplete", `the machine signed records up to ${h.nextSeq} but only ${have} arrived`);
      }
      const covered = h.nextSeq - this.confirmedNext;
      const [link, epoch] = covered > 0 ? [this.pending[covered - 1]!.link, this.pending[covered - 1]!.epoch] : [this.confirmedLink, this.confirmedEpoch];
      // Nothing was ever written: no record carries an epoch to compare.
      const epochMatters = h.nextSeq > 0;
      if (!equal(link, h.link) || (epochMatters && epoch !== h.epoch)) {
        this.rollBack();
        throw new RecordError("forged", "the signed head does not match the records received");
      }
      this.freshest = Math.max(this.freshest ?? 0, h.timeMs);
      this.confirmedNext = h.nextSeq;
      this.confirmedLink = copy(link);
      if (epochMatters) this.confirmedEpoch = epoch;
      if (!this.anchor || this.anchor.head.nextSeq !== h.nextSeq || this.anchor.head.timeMs <= h.timeMs) this.anchor = head;
      return this.pending.splice(0, covered).map((p) => p.opened);
    });
  }

  /** Forget everything received but not released. */
  private rollBack(): void {
    this.pending = [];
    this.nextSeq = this.confirmedNext;
    this.link = copy(this.confirmedLink);
    this.epoch = this.confirmedEpoch;
  }
}
