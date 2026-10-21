/*
 * The rest of an encrypted stream, browser side: stream names, snapshot pages,
 * the signed manifests that name them, and drafts that are relayed but never
 * stored. `server-rs/crates/rh-proto/src/stream.rs` has the why; this is the
 * same bytes.
 *
 * - index stream `("rh.index", "")`, status stream `("rh.status", "")`, a
 *   session's stream `(source, CLI session id)` — all `streamId` under `MK`:
 *   of the epoch the stream was created or moved in (`streams/2`,
 *   relay-and-plans §5) — the index and status under the newest, a session's
 *   under its index entry's `se` — or, a stream from before, the first.
 * - page: AAD `"rh/v1 page\0" ‖ stream ‖ epoch`, plaintext
 *   `[flags][fmt u16][body_len u32][body][zero padding]` padded to a bucket,
 *   encoded `epoch u32 | nonce 12 | ct_len u32 | ct`, named by SHA-256 of that.
 * - manifest: `signed head | count u16 | pages[32]… | sig 64`, the signature
 *   under `rh/v1 stream-manifest` over everything before it. It carries no
 *   format: a reader takes each page's from its plaintext header (a cleartext
 *   `fmt` told index, status and session streams apart).
 * - draft: `epoch u32 | nonce 12 | ct | sig 64`, AAD `"rh/v1 ephemeral\0" ‖
 *   stream ‖ epoch`, the machine's signature under `rh/v1 stream-draft` over
 *   `stream | epoch | nonce | ct`; accepted only under the reader's current
 *   epoch.
 */
import { type Bytes, bytes, concat, copy, DecodeError, equal, Reader, utf8, Writer } from "./bytes";
import { sha256 } from "./keys";
import {
  BUCKETS,
  bucket,
  decodeSignedHead,
  deflateRaw,
  encodeSignedHead,
  inflateRaw,
  LogReader,
  type MachineKey,
  MAX_BODY,
  MAX_CT,
  MAX_PLAINTEXT,
  RecordError,
  type SignedHead,
  streamId,
  verifyStreamHead,
  type Compression,
} from "./record";
import { signLabelled, STREAM_DRAFT, STREAM_MANIFEST, verifyLabelled } from "./sig";

export const INDEX_SOURCE = "rh.index";
export const STATUS_SOURCE = "rh.status";
export const FMT_INDEX_V1 = 2;
export const FMT_STATUS_V1 = 3;
export const FMT_DRAFT_V1 = 4;
export const MAX_PAGES = 1024;
const PAGE_HEADER = 1 + 2 + 4;
const TAG_LEN = 16;
export const MAX_PAGE_BODY = MAX_PLAINTEXT - PAGE_HEADER;
export const MAX_PAGE_LEN = 4 + 12 + 4 + MAX_CT;
const PAGE_AAD = utf8("rh/v1 page\0");
const EPHEMERAL_AAD = utf8("rh/v1 ephemeral\0");
const FLAG_DEFLATE = 0x01;
const SIGNED_HEAD_LEN = 32 + 8 + 4 + 32 + 8 + 64;

/**
 * What a device's channel hello says (and an agent's features) when it reads
 * streams named per key epoch: once a device has said so, the agent names
 * the index and status under the newest epoch and each session's stream under
 * the epoch its index entry gives (`se`), and moves them on a rotation — so a
 * device removed with the old key cannot compute the names written after it.
 */
export const STREAMS_FEATURE = "streams/2";

/**
 * The first (lowest) epoch's key: what every stream was named under before
 * {@link STREAMS_FEATURE}, and what an index entry without `se` names its
 * session's stream under still.
 */
export function namingKey(epochs: ReadonlyMap<number, MachineKey>): MachineKey | null {
  let first: number | undefined;
  for (const epoch of epochs.keys()) if (first === undefined || epoch < first) first = epoch;
  return first === undefined ? null : (epochs.get(first) ?? null);
}

/** The epochs held, newest first: where a reader looks for the index and status, in order. */
export function epochsNewestFirst(epochs: ReadonlyMap<number, MachineKey>): [number, MachineKey][] {
  return [...epochs].sort((a, b) => b[0] - a[0]);
}

/**
 * The key a session's stream is named under, from its index entry's `se`
 * (absent: the first epoch); `null` when this device does not hold it.
 */
export function sessionNamingKey(epochs: ReadonlyMap<number, MachineKey>, se: unknown): MachineKey | null {
  if (se === undefined || se === null) return namingKey(epochs);
  return Number.isInteger(se) ? (epochs.get(se as number) ?? null) : null;
}

export function indexStream(mk: MachineKey): Promise<Bytes> {
  return streamId(mk, INDEX_SOURCE, "");
}

export function statusStream(mk: MachineKey): Promise<Bytes> {
  return streamId(mk, STATUS_SOURCE, "");
}

export interface SealedPage {
  epoch: number;
  nonce: Bytes;
  ct: Bytes;
}

export interface OpenedPage {
  fmt: number;
  body: Bytes;
}

export function encodePage(p: SealedPage): Bytes {
  if (p.nonce.length !== 12) throw new DecodeError("a page nonce is 12 bytes");
  return new Writer().u32(p.epoch).raw(p.nonce).u32(p.ct.length).raw(p.ct).finish();
}

export function decodePage(data: Uint8Array): SealedPage {
  const r = new Reader(data);
  const epoch = r.u32();
  const nonce = r.bytes(12);
  const len = r.u32();
  if (len > MAX_CT) throw new DecodeError("page ciphertext too large");
  if (len < TAG_LEN) throw new DecodeError("page ciphertext shorter than its tag");
  const ct = r.bytes(len);
  r.finish();
  return { epoch, nonce, ct };
}

/** A page's name: the SHA-256 of its encoding. */
export function pageHash(encoded: Uint8Array): Promise<Bytes> {
  return sha256(encoded);
}

function boundAad(label: Uint8Array, stream: Uint8Array, epoch: number): Bytes {
  return new Writer().raw(label).raw(stream).u32(epoch).finish();
}

async function pack(fmt: number, body: Uint8Array, compression: Compression): Promise<Bytes> {
  if (body.length > MAX_BODY) throw new RecordError("tooLarge", "body too large");
  const compressed = compression === "deflate" ? await deflateRaw(body) : null;
  const [flags, stored] = compressed && compressed.length < body.length ? [FLAG_DEFLATE, compressed] : [0, body];
  const padded = bucket(PAGE_HEADER + stored.length);
  if (padded === null) throw new RecordError("tooLarge", `the body does not fit a page (${stored.length} bytes)`);
  const pt = bytes(padded);
  pt.set(new Writer().u8(flags).u16(fmt).u32(stored.length).raw(stored).finish());
  return pt;
}

async function unpack(pt: Uint8Array): Promise<OpenedPage> {
  if (!(BUCKETS as readonly number[]).includes(pt.length)) throw new RecordError("malformed", "plaintext is not a bucket size");
  const r = new Reader(pt);
  const flags = r.u8();
  if ((flags & ~FLAG_DEFLATE) !== 0) throw new RecordError("malformed", "unknown flags");
  const fmt = r.u16();
  const len = r.u32();
  if (len > r.remaining) throw new RecordError("malformed", "body longer than the page");
  const stored = r.bytes(len);
  if (r.rest().some((b) => b !== 0)) throw new RecordError("malformed", "non-zero padding");
  return { fmt, body: flags & FLAG_DEFLATE ? await inflateRaw(stored) : stored };
}

async function decrypt(key: CryptoKey, nonce: Uint8Array, aad: Uint8Array, ct: Uint8Array): Promise<Bytes> {
  try {
    return new Uint8Array(await crypto.subtle.decrypt({ name: "AES-GCM", iv: copy(nonce), additionalData: copy(aad) }, key, copy(ct)));
  } catch {
    throw new RecordError("decrypt", "does not decrypt under this stream's key (tampered, moved, or the wrong epoch)");
  }
}

export async function sealPage(
  key: CryptoKey,
  stream: Uint8Array,
  epoch: number,
  fmt: number,
  body: Uint8Array,
  compression: Compression = "none",
  nonce: Uint8Array = crypto.getRandomValues(bytes(12)),
): Promise<SealedPage> {
  const pt = await pack(fmt, body, compression);
  const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv: copy(nonce), additionalData: boundAad(PAGE_AAD, stream, epoch) }, key, pt));
  pt.fill(0);
  return { epoch, nonce: copy(nonce), ct };
}

/** Decrypt one page; check its hash against the manifest first. */
export async function openPage(key: CryptoKey, stream: Uint8Array, page: SealedPage): Promise<OpenedPage> {
  return unpack(await decrypt(key, page.nonce, boundAad(PAGE_AAD, stream, page.epoch), page.ct));
}

/**
 * A stream's checkpoint: a signed head and the pages holding the state there,
 * signed together. The pages' format is inside each page only.
 */
export interface Manifest {
  head: SignedHead;
  pages: Bytes[];
  signature: Bytes;
}

/** `signed head | count u16 | pages`: what the manifest signature covers. */
function manifestBody(head: SignedHead, pages: Uint8Array[]): Bytes {
  if (pages.length > MAX_PAGES) throw new RecordError("tooLarge", "too many pages");
  const w = new Writer().raw(encodeSignedHead(head)).u16(pages.length);
  for (const p of pages) {
    if (p.length !== 32) throw new DecodeError("a page hash is 32 bytes");
    w.raw(p);
  }
  return w.finish();
}

/** Sign a manifest: `head` must already be signed by the same machine key. */
export async function signManifest(head: SignedHead, pages: Uint8Array[], machine: CryptoKey): Promise<Manifest> {
  const signature = await signLabelled(machine, STREAM_MANIFEST, manifestBody(head, pages));
  return { head, pages: pages.map(copy), signature };
}

export function encodeManifest(m: Manifest): Bytes {
  return concat(manifestBody(m.head, m.pages), m.signature);
}

export function decodeManifest(data: Uint8Array): Manifest {
  if (data.length < SIGNED_HEAD_LEN) throw new DecodeError("truncated input");
  const head = decodeSignedHead(data.subarray(0, SIGNED_HEAD_LEN));
  const r = new Reader(data.subarray(SIGNED_HEAD_LEN));
  const count = r.u16();
  if (count > MAX_PAGES) throw new DecodeError("too many manifest pages");
  const pages: Bytes[] = [];
  for (let i = 0; i < count; i++) pages.push(r.bytes(32));
  const signature = r.bytes(64);
  r.finish();
  return { head, pages, signature };
}

/** Both signatures are the machine's: the head's and the manifest's. */
export async function verifyManifest(m: Manifest, machineKey: Uint8Array): Promise<boolean> {
  return (await verifyStreamHead(m.head, machineKey)) && (await verifyLabelled(machineKey, STREAM_MANIFEST, manifestBody(m.head, m.pages), m.signature));
}

/** The seq the manifest's state stands at. */
export function manifestUpto(m: Manifest): number {
  return m.head.head.nextSeq;
}

/**
 * A reader starting from a manifest (its pages, in order, are the starting
 * state). A manifest only ever comes from the network, so `machineKey` is the
 * host's current key alone, never one it renewed away from
 * (`docs/host-identity-rotation.md` §5.4).
 */
export async function readerFromManifest(machineKey: Uint8Array, m: Manifest): Promise<LogReader> {
  if (!(await verifyManifest(m, machineKey))) throw new RecordError("badHead", "the manifest is not signed by the machine");
  return LogReader.fromHead(machineKey, m.head);
}

/** Check that `encoded` is page `index` of `m`, and open it. */
export async function openManifestPage(key: CryptoKey, m: Manifest, index: number, encoded: Uint8Array): Promise<OpenedPage> {
  const want = m.pages[index];
  if (!want || !equal(await pageHash(encoded), want)) throw new RecordError("forged", `page ${index} is not the one the manifest names`);
  return openPage(key, m.head.head.stream, decodePage(encoded));
}

const SIG_LEN = 64;

/**
 * Seal a draft for a stream's live readers (`key` is epoch `epoch`'s stream
 * key, `machine` the machine's Ed25519 signing key): `epoch u32 | nonce 12 |
 * ct | sig 64`, the signature over `stream | epoch | nonce | ct` — so a holder
 * of the stream key that is not the machine (a device removed since, say)
 * cannot put words in the machine's mouth. `nonce` is for test vectors only.
 */
export async function sealEphemeral(
  key: CryptoKey,
  stream: Uint8Array,
  epoch: number,
  body: Uint8Array,
  machine: CryptoKey,
  nonce: Uint8Array = crypto.getRandomValues(bytes(12)),
): Promise<Bytes> {
  const pt = await pack(FMT_DRAFT_V1, body, "none");
  const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv: copy(nonce), additionalData: boundAad(EPHEMERAL_AAD, stream, epoch) }, key, pt));
  pt.fill(0);
  const sealed = new Writer().u32(epoch).raw(nonce).raw(ct).finish();
  return concat(sealed, await signLabelled(machine, STREAM_DRAFT, concat(stream, sealed)));
}

/**
 * Open a draft: the epoch it is in and its body. It must carry `machineKey`'s
 * signature (else `badHead`) and be sealed under `currentEpoch`, the newest
 * the reader holds (else `malformed`): a draft under an older key is someone
 * who still has that key, not the machine. `keyFor` gives an epoch's stream key.
 */
export async function openEphemeral(
  keyFor: (epoch: number) => CryptoKey | undefined,
  stream: Uint8Array,
  machineKey: Uint8Array,
  currentEpoch: number,
  data: Uint8Array,
): Promise<{ epoch: number; body: Bytes }> {
  if (data.length < 4 + 12 + TAG_LEN + SIG_LEN) throw new RecordError("malformed", "draft size");
  const sealed = data.subarray(0, data.length - SIG_LEN);
  const signature = data.subarray(data.length - SIG_LEN);
  if (!(await verifyLabelled(machineKey, STREAM_DRAFT, concat(stream, sealed), signature))) {
    throw new RecordError("badHead", "the draft is not signed by the machine");
  }
  const r = new Reader(sealed);
  const epoch = r.u32();
  const nonce = r.bytes(12);
  const ct = r.rest();
  if (ct.length < TAG_LEN || ct.length > MAX_CT) throw new RecordError("malformed", "draft ciphertext size");
  if (epoch !== currentEpoch) throw new RecordError("malformed", "a draft under an old key");
  const key = keyFor(epoch);
  if (!key) throw new RecordError("noKey", `no key for epoch ${epoch}`);
  const opened = await unpack(await decrypt(key, nonce, boundAad(EPHEMERAL_AAD, stream, epoch), ct));
  if (opened.fmt !== FMT_DRAFT_V1) throw new RecordError("malformed", "not a draft");
  return { epoch, body: opened.body };
}
