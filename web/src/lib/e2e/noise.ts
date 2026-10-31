/*
 * The end-to-end channel, browser side: `Noise_IK_25519_AESGCM_SHA256` on
 * WebCrypto, byte-compatible with the agent's `snow`
 * (`server-rs/crates/droppen-proto/src/noise.rs` — read that for the why).
 *
 * The device is the initiator: it knows the agent's static key from the
 * device chain, so its first message is encrypted to the agent. The prologue
 * binds `account | device | machine`; chain views travel inside the payloads
 * (`handshake.ts`), and the first payload never carries a request — it is
 * replayable and open to key-compromise impersonation. The responder role is
 * here too, for test vectors and loopback tests.
 *
 * What lives where:
 * - The device's static X25519 private key is a non-extractable `CryptoKey`;
 *   ephemerals are generated non-extractable too.
 * - What is *not* protected, stated plainly: the handshake's chaining key and
 *   DH outputs are raw bytes in JS memory (Noise's `MixKey` feeds a DH result
 *   into HMAC as data), and so are the transport keys for the moment between
 *   `Split()` (or each `REKEY()`, which is an encryption whose output becomes
 *   the next key) and their import as non-extractable AES-GCM keys. Buffers we
 *   own are zeroed after use, but a JS engine may have copied them, and a
 *   script running in this origin can read them while they exist. The
 *   non-extractable flag keeps a loaded key from being exported through
 *   WebCrypto; it does not keep such a script from the stored device keys
 *   (`keystore.ts`), nor make the page's own code trustworthy.
 */
import { type Bytes, bytes, concat, copy, Writer } from "./bytes";
import { MAX_PAYLOAD_LEN } from "./frame";
import { dh, exportPublic, importX25519Private, isLowOrderX25519, LowOrderKeyError, publicFromSecret } from "./keys";

export const PATTERN = "Noise_IK_25519_AESGCM_SHA256";
export const TAG_LEN = 16;
export const FLAG_REKEY = 0x01;
/** The longest account, device or machine id a binding carries. */
const MAX_BINDING_ID_LEN = 64;
export const MAX_TRANSPORT_DATA = MAX_PAYLOAD_LEN - TAG_LEN - 1;
export const MAX_HELLO_PAYLOAD = MAX_PAYLOAD_LEN - 32 - (32 + TAG_LEN) - 8 - TAG_LEN;

export class ChannelError extends Error {
  constructor(
    readonly kind: "decrypt" | "poisoned" | "badBinding" | "lowOrderKey" | "malformed" | "tooLarge" | "rekeyOverdue",
    message: string,
  ) {
    super(message);
    this.name = "ChannelError";
  }
}

export interface Binding {
  account: string;
  device: string;
  machine: string;
}

/** `"rh/v1\0" | str16 account | str16 device | str16 machine`. */
export function prologue(b: Binding): Bytes {
  for (const id of [b.account, b.device, b.machine]) {
    const len = new TextEncoder().encode(id).length;
    if (len === 0 || len > MAX_BINDING_ID_LEN) throw new ChannelError("badBinding", "a binding id is empty or too long");
  }
  return new Writer().raw(new TextEncoder().encode("rh/v1\0")).str16(b.account).str16(b.device).str16(b.machine).finish();
}

export interface RekeyPolicy {
  maxMessages: number;
  maxAgeMs: number;
}

export const DEFAULT_REKEY: RekeyPolicy = { maxMessages: 2 ** 20, maxAgeMs: 60 * 60 * 1000 };

/** An X25519 key pair whose private half stays in WebCrypto. */
export interface NoiseKeyPair {
  privateKey: CryptoKey;
  publicKey: Bytes;
}

export async function keyPairFromSecret(secret: Uint8Array): Promise<NoiseKeyPair> {
  return { privateKey: await importX25519Private(secret), publicKey: await publicFromSecret("X25519", secret) };
}

async function generateKeyPair(): Promise<NoiseKeyPair> {
  const pair = (await crypto.subtle.generateKey({ name: "X25519" }, false, ["deriveBits"])) as CryptoKeyPair;
  return { privateKey: pair.privateKey, publicKey: await exportPublic(pair.publicKey) };
}

async function hmac(key: Uint8Array, data: Uint8Array): Promise<Bytes> {
  // Copies so WebCrypto gets ArrayBuffer-backed views; zeroed once used, since
  // these are chaining keys and DH outputs.
  const k = copy(key);
  const d = copy(data);
  try {
    const imported = await crypto.subtle.importKey("raw", k, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
    return new Uint8Array(await crypto.subtle.sign("HMAC", imported, d));
  } finally {
    k.fill(0);
    d.fill(0);
  }
}

/** Noise's HKDF with two outputs. */
async function hkdf2(ck: Uint8Array, ikm: Uint8Array): Promise<[Bytes, Bytes]> {
  const temp = await hmac(ck, ikm);
  const one = await hmac(temp, Uint8Array.of(1));
  const two = await hmac(temp, concat(one, Uint8Array.of(2)));
  return [one, two];
}

async function sha256(data: Uint8Array): Promise<Bytes> {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", copy(data)));
}

const MAX_NONCE = 2n ** 64n - 1n;

/** 32 zero bits and the counter, big-endian: Noise's AESGCM nonce. */
function nonceBytes(n: bigint): Bytes {
  const iv = bytes(12);
  new DataView(iv.buffer).setBigUint64(4, n);
  return iv;
}

class CipherState {
  private key: CryptoKey | null = null;
  n = 0n;

  /** Import `raw` and zero it: the caller's buffer is the only copy we made. */
  async initializeKey(raw: Bytes): Promise<void> {
    try {
      this.key = await crypto.subtle.importKey("raw", raw, { name: "AES-GCM" }, false, ["encrypt", "decrypt"]);
    } finally {
      raw.fill(0);
    }
    this.n = 0n;
  }

  get hasKey(): boolean {
    return this.key !== null;
  }

  private params(n: bigint, ad: Uint8Array): AesGcmParams {
    return ad.length > 0 ? { name: "AES-GCM", iv: nonceBytes(n), additionalData: copy(ad) } : { name: "AES-GCM", iv: nonceBytes(n) };
  }

  async encryptWithAd(ad: Uint8Array, plaintext: Uint8Array): Promise<Bytes> {
    if (!this.key) return copy(plaintext);
    if (this.n >= MAX_NONCE) throw new ChannelError("poisoned", "nonce exhausted");
    const out = new Uint8Array(await crypto.subtle.encrypt(this.params(this.n, ad), this.key, copy(plaintext)));
    this.n += 1n;
    return out;
  }

  async decryptWithAd(ad: Uint8Array, ciphertext: Uint8Array): Promise<Bytes> {
    if (!this.key) return copy(ciphertext);
    if (this.n >= MAX_NONCE) throw new ChannelError("poisoned", "nonce exhausted");
    let out: Bytes;
    try {
      out = new Uint8Array(await crypto.subtle.decrypt(this.params(this.n, ad), this.key, copy(ciphertext)));
    } catch {
      throw new ChannelError("decrypt", "a message failed to decrypt (forged, replayed, reordered or dropped)");
    }
    this.n += 1n;
    return out;
  }

  /** Noise §4.2: the new key is the first 32 bytes of ENCRYPT(k, 2^64-1, "", zeros). */
  async rekey(): Promise<void> {
    if (!this.key) return;
    const out = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv: nonceBytes(MAX_NONCE) }, this.key, bytes(32)));
    const n = this.n;
    await this.initializeKey(copy(out.subarray(0, 32)));
    out.fill(0);
    this.n = n;
  }
}

class SymmetricState {
  private ck: Bytes;
  h: Bytes;
  readonly cs = new CipherState();

  constructor(name: string) {
    const raw = new TextEncoder().encode(name);
    this.h = bytes(32);
    this.h.set(raw.subarray(0, 32));
    this.ck = copy(this.h);
  }

  async mixKey(ikm: Uint8Array): Promise<void> {
    const [ck, k] = await hkdf2(this.ck, ikm);
    ikm.fill(0); // a DH output, used once
    this.ck.fill(0);
    this.ck = ck;
    await this.cs.initializeKey(k);
  }

  async mixHash(data: Uint8Array): Promise<void> {
    this.h = await sha256(concat(this.h, data));
  }

  async encryptAndHash(plaintext: Uint8Array): Promise<Bytes> {
    const ct = await this.cs.encryptWithAd(this.h, plaintext);
    await this.mixHash(ct);
    return ct;
  }

  async decryptAndHash(ciphertext: Uint8Array): Promise<Bytes> {
    const pt = await this.cs.decryptWithAd(this.h, ciphertext);
    await this.mixHash(ciphertext);
    return pt;
  }

  async split(): Promise<[CipherState, CipherState]> {
    const [k1, k2] = await hkdf2(this.ck, new Uint8Array(0));
    const c1 = new CipherState();
    const c2 = new CipherState();
    await c1.initializeKey(k1);
    await c2.initializeKey(k2);
    this.ck.fill(0);
    return [c1, c2];
  }
}

async function checkEphemeral(message: Uint8Array, minLen: number): Promise<Bytes> {
  if (message.length < minLen) throw new ChannelError("malformed", "handshake message too short");
  if (message.length > MAX_PAYLOAD_LEN) throw new ChannelError("tooLarge", "handshake message too large");
  const e = copy(message.subarray(0, 32));
  if (await isLowOrderX25519(e)) throw new ChannelError("lowOrderKey", "the peer offered a low-order X25519 key");
  return e;
}

async function agree(priv: CryptoKey, pub: Uint8Array): Promise<Bytes> {
  try {
    return await dh(priv, pub);
  } catch (error) {
    if (error instanceof LowOrderKeyError) throw new ChannelError("lowOrderKey", error.message);
    throw error;
  }
}

/** The device's half of the handshake. */
export class Initiator {
  private e: NoiseKeyPair | null = null;

  private constructor(
    private readonly ss: SymmetricState,
    private readonly s: NoiseKeyPair,
    private readonly rs: Bytes,
    private readonly fixedE: NoiseKeyPair | null,
  ) {}

  /** `fixedEphemeral` is for test vectors only. */
  static async create(local: NoiseKeyPair, responder: Uint8Array, binding: Binding, fixedEphemeral?: NoiseKeyPair): Promise<Initiator> {
    return Initiator.withPrologue(local, responder, prologue(binding), fixedEphemeral);
  }

  /** The same handshake under another prologue (the direct path's, `path.ts` `directPrologue`). */
  static async withPrologue(local: NoiseKeyPair, responder: Uint8Array, prologueBytes: Uint8Array, fixedEphemeral?: NoiseKeyPair): Promise<Initiator> {
    if (await isLowOrderX25519(responder)) throw new ChannelError("lowOrderKey", "the responder key is of low order");
    const ss = new SymmetricState(PATTERN);
    await ss.mixHash(prologueBytes);
    await ss.mixHash(responder);
    return new Initiator(ss, local, copy(responder), fixedEphemeral ?? null);
  }

  /** `e, es, s, ss` and `timestamp ‖ payload`. */
  async writeHello(timestampMs: number, payload: Uint8Array): Promise<Bytes> {
    if (payload.length > MAX_HELLO_PAYLOAD) throw new ChannelError("tooLarge", "first payload too large");
    const e = this.fixedE ?? (await generateKeyPair());
    this.e = e;
    await this.ss.mixHash(e.publicKey);
    await this.ss.mixKey(await agree(e.privateKey, this.rs));
    const s = await this.ss.encryptAndHash(this.s.publicKey);
    await this.ss.mixKey(await agree(this.s.privateKey, this.rs));
    const body = await this.ss.encryptAndHash(new Writer().u64(timestampMs).raw(payload).finish());
    return concat(e.publicKey, s, body);
  }

  /** `e, ee, se` and the agent's payload; completes the handshake. */
  async readReply(message: Uint8Array, policy: RekeyPolicy, nowMs: number): Promise<{ transport: Transport; payload: Bytes }> {
    if (!this.e) throw new ChannelError("malformed", "readReply before writeHello");
    const re = await checkEphemeral(message, 32 + TAG_LEN);
    await this.ss.mixHash(re);
    await this.ss.mixKey(await agree(this.e.privateKey, re));
    await this.ss.mixKey(await agree(this.s.privateKey, re));
    const payload = await this.ss.decryptAndHash(message.subarray(32));
    const hash = copy(this.ss.h);
    const [send, recv] = await this.ss.split();
    return { transport: new Transport(send, recv, policy, nowMs, hash, this.rs), payload };
  }
}

export interface Hello {
  initiatorStatic: Bytes;
  ephemeral: Bytes;
  timestampMs: number;
  payload: Bytes;
}

/** The agent's half; here for vectors and loopback tests. */
export class Responder {
  private re: Bytes | null = null;
  private rs: Bytes | null = null;

  private constructor(
    private readonly ss: SymmetricState,
    private readonly s: NoiseKeyPair,
    private readonly fixedE: NoiseKeyPair | null,
  ) {}

  static async create(local: NoiseKeyPair, binding: Binding, fixedEphemeral?: NoiseKeyPair): Promise<Responder> {
    return Responder.withPrologue(local, prologue(binding), fixedEphemeral);
  }

  /** The same handshake under another prologue (the direct path's). */
  static async withPrologue(local: NoiseKeyPair, prologueBytes: Uint8Array, fixedEphemeral?: NoiseKeyPair): Promise<Responder> {
    const ss = new SymmetricState(PATTERN);
    await ss.mixHash(prologueBytes);
    await ss.mixHash(local.publicKey);
    return new Responder(ss, local, fixedEphemeral ?? null);
  }

  async readHello(message: Uint8Array): Promise<Hello> {
    const re = await checkEphemeral(message, 32 + 32 + TAG_LEN + 8 + TAG_LEN);
    await this.ss.mixHash(re);
    await this.ss.mixKey(await agree(this.s.privateKey, re));
    const rs = await this.ss.decryptAndHash(message.subarray(32, 32 + 32 + TAG_LEN));
    if (await isLowOrderX25519(rs)) throw new ChannelError("lowOrderKey", "the initiator's static key is of low order");
    await this.ss.mixKey(await agree(this.s.privateKey, rs));
    const body = await this.ss.decryptAndHash(message.subarray(32 + 32 + TAG_LEN));
    if (body.length < 8) throw new ChannelError("malformed", "first message without a timestamp");
    this.re = re;
    this.rs = rs;
    const ts = new DataView(body.buffer, body.byteOffset, 8).getBigUint64(0);
    return { initiatorStatic: rs, ephemeral: re, timestampMs: Number(ts), payload: copy(body.subarray(8)) };
  }

  async writeReply(payload: Uint8Array, policy: RekeyPolicy, nowMs: number): Promise<{ message: Bytes; transport: Transport }> {
    if (!this.re || !this.rs) throw new ChannelError("malformed", "writeReply before readHello");
    if (payload.length > MAX_HELLO_PAYLOAD) throw new ChannelError("tooLarge", "reply payload too large");
    const e = this.fixedE ?? (await generateKeyPair());
    await this.ss.mixHash(e.publicKey);
    await this.ss.mixKey(await agree(e.privateKey, this.re));
    await this.ss.mixKey(await agree(e.privateKey, this.rs));
    const body = await this.ss.encryptAndHash(payload);
    const hash = copy(this.ss.h);
    const [recv, send] = await this.ss.split();
    return { message: concat(e.publicKey, body), transport: new Transport(send, recv, policy, nowMs, hash, this.rs) };
  }
}

/** An established channel. Calls are serialised: a nonce is a counter, and two in flight would race for it. */
export class Transport {
  private sent = 0;
  private received = 0;
  private poisoned = false;
  private queue: Promise<unknown> = Promise.resolve();

  constructor(
    private readonly send: CipherState,
    private readonly recv: CipherState,
    private readonly policy: RekeyPolicy,
    private lastRekeyMs: number,
    readonly handshakeHash: Bytes,
    readonly remoteStatic: Bytes,
  ) {}

  get isPoisoned(): boolean {
    return this.poisoned;
  }

  private serial<T>(work: () => Promise<T>): Promise<T> {
    const next = this.queue.then(work, work);
    this.queue = next.catch(() => undefined);
    return next;
  }

  seal(nowMs: number, data: Uint8Array): Promise<Bytes> {
    return this.serial(async () => {
      if (this.poisoned) throw new ChannelError("poisoned", "the channel already failed");
      if (data.length > MAX_TRANSPORT_DATA) throw new ChannelError("tooLarge", `${data.length} bytes is more than one message can carry`);
      const rekey = this.sent + 1 >= this.policy.maxMessages || nowMs - this.lastRekeyMs >= this.policy.maxAgeMs;
      let out: Bytes;
      try {
        out = await this.send.encryptWithAd(new Uint8Array(0), concat(Uint8Array.of(rekey ? FLAG_REKEY : 0), data));
      } catch (error) {
        this.poisoned = true;
        throw error;
      }
      if (rekey) {
        await this.send.rekey();
        this.sent = 0;
        this.lastRekeyMs = nowMs;
      } else {
        this.sent += 1;
      }
      return out;
    });
  }

  open(message: Uint8Array): Promise<Bytes> {
    return this.serial(async () => {
      if (this.poisoned) throw new ChannelError("poisoned", "the channel already failed");
      try {
        return await this.openInner(message);
      } catch (error) {
        this.poisoned = true;
        throw error;
      }
    });
  }

  private async openInner(message: Uint8Array): Promise<Bytes> {
    if (message.length < 1 + TAG_LEN) throw new ChannelError("malformed", "transport message too short");
    const plaintext = await this.recv.decryptWithAd(new Uint8Array(0), message);
    const flags = plaintext[0]!;
    if ((flags & ~FLAG_REKEY) !== 0) throw new ChannelError("malformed", "unknown transport flags");
    if (flags & FLAG_REKEY) {
      await this.recv.rekey();
      this.received = 0;
    } else {
      this.received += 1;
      if (this.received >= this.policy.maxMessages) throw new ChannelError("rekeyOverdue", "the peer went past the rekey limit");
    }
    return copy(plaintext.subarray(1));
  }
}

