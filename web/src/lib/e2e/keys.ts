/*
 * Curve25519 keys in WebCrypto.
 *
 * - A device's own keys (Ed25519 for signing chain entries, X25519 for Noise
 *   and for receiving wrapped keys) are used as **non-extractable**
 *   `CryptoKey`s: WebCrypto will sign and agree with them but never hand out
 *   the private half. Neither algorithm forces extractability: `deriveBits`
 *   takes a non-extractable X25519 private key, and only the *public* half is
 *   ever exported (public keys are always exportable). At rest they are
 *   sealed PKCS #8 (`keystore.ts`, where WebKit's IndexedDB drove that), which
 *   code in this origin can open: the flag guards a loaded key, not the page.
 * - Importing a private key from raw bytes (test vectors, the relay demo) goes
 *   through PKCS#8, the only private format WebCrypto accepts besides JWK.
 *
 * Two checks keep this side agreeing with the Rust side about what a valid key
 * is (`rh-proto/src/keys.rs`): a low-order X25519 key is refused (WebCrypto
 * already throws when one yields the all-zero secret), and Ed25519 verification
 * is strict — canonical `S`, and neither the key nor `R` one of the small-order
 * encodings `verify_strict` refuses — in front of WebCrypto's own `verify`,
 * whose edge cases differ between browsers.
 */
import { type Bytes, concat, copy, equal, fromHex, hex, utf8 } from "./bytes";
import { decodeLe, inv0, isSquare, P as FIELD_P } from "./field";

export const ED25519_PKCS8_PREFIX = fromHex("302e020100300506032b657004220420");
export const X25519_PKCS8_PREFIX = fromHex("302e020100300506032b656e04220420");

function subtle(): SubtleCrypto {
  const s = globalThis.crypto?.subtle;
  if (!s) throw new Error("WebCrypto is unavailable (an insecure context?)");
  return s;
}

export class LowOrderKeyError extends Error {
  constructor() {
    super("the peer offered a low-order X25519 key");
    this.name = "LowOrderKeyError";
  }
}

function fromBase64Url(text: string): Bytes {
  const b64 = text.replace(/-/g, "+").replace(/_/g, "/");
  const bin = atob(b64 + "=".repeat((4 - (b64.length % 4)) % 4));
  return copy(Uint8Array.from(bin, (c) => c.charCodeAt(0)));
}

export async function importX25519Private(secret: Uint8Array, extractable = false): Promise<CryptoKey> {
  return subtle().importKey("pkcs8", concat(X25519_PKCS8_PREFIX, secret), { name: "X25519" }, extractable, ["deriveBits"]);
}

export async function importX25519Public(key: Uint8Array): Promise<CryptoKey> {
  return subtle().importKey("raw", copy(key), { name: "X25519" }, true, []);
}

export async function importEd25519Private(seed: Uint8Array, extractable = false): Promise<CryptoKey> {
  return subtle().importKey("pkcs8", concat(ED25519_PKCS8_PREFIX, seed), { name: "Ed25519" }, extractable, ["sign"]);
}

export async function importEd25519Public(key: Uint8Array): Promise<CryptoKey> {
  return subtle().importKey("raw", copy(key), { name: "Ed25519" }, true, ["verify"]);
}

export async function exportPublic(key: CryptoKey): Promise<Bytes> {
  return new Uint8Array(await subtle().exportKey("raw", key));
}

/** The public key of a raw private key — vectors and demos only, where the private key is known anyway. */
export async function publicFromSecret(alg: "X25519" | "Ed25519", secret: Uint8Array): Promise<Bytes> {
  const key = alg === "X25519" ? await importX25519Private(secret, true) : await importEd25519Private(secret, true);
  const jwk = await subtle().exportKey("jwk", key);
  return fromBase64Url(jwk.x!);
}

/** X25519 with a private key in WebCrypto; a low-order peer key is refused. */
export async function dh(privateKey: CryptoKey, peer: Uint8Array): Promise<Bytes> {
  const pub = await importX25519Public(peer);
  try {
    return new Uint8Array(await subtle().deriveBits({ name: "X25519", public: pub }, privateKey, 256));
  } catch {
    // WebCrypto refuses exactly the case where the result would be all zeros.
    throw new LowOrderKeyError();
  }
}

/** The field prime 2^255 - 19, little-endian. */
const P = fromHex("edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f");

/**
 * Whether a little-endian field element, top bit ignored, is below p: the one
 * canonical spelling. Libraries (and browsers) differ on the others, so every
 * key that enters the protocol must be canonical, here as in Rust.
 */
function canonicalField(key: Uint8Array): boolean {
  if (key.length !== 32) return false;
  for (let i = 31; i >= 0; i--) {
    const b = i === 31 ? key[i]! & 0x7f : key[i]!;
    if (b !== P[i]) return b < P[i]!;
  }
  return false;
}

let probe: Promise<CryptoKeyPair> | null = null;

/** Whether `key` is a low-order X25519 public key (any shared secret with it is zero). */
export async function isLowOrderX25519(key: Uint8Array): Promise<boolean> {
  // A probe the browser would not make (no X25519 before compatibility mode) is not kept: the next call tries again.
  probe ??= (subtle().generateKey({ name: "X25519" }, false, ["deriveBits"]) as Promise<CryptoKeyPair>).catch((error: unknown) => {
    probe = null;
    throw error;
  });
  try {
    await dh((await probe).privateKey, key);
    return false;
  } catch (error) {
    if (error instanceof LowOrderKeyError) return true;
    throw error;
  }
}

/** An X25519 public key the protocol accepts: canonical (top bit clear, u < p) and not of low order. */
export async function isValidX25519(key: Uint8Array): Promise<boolean> {
  if (key.length !== 32 || (key[31]! & 0x80) !== 0 || !canonicalField(key)) return false;
  try {
    return !(await isLowOrderX25519(key));
  } catch {
    return false;
  }
}

/** The encodings `ed25519-dalek` treats as small-order (weak) keys; shared via the vectors. */
const ED25519_WEAK = new Set([
  "0000000000000000000000000000000000000000000000000000000000000000",
  "0000000000000000000000000000000000000000000000000000000000000080",
  "0100000000000000000000000000000000000000000000000000000000000000",
  "0100000000000000000000000000000000000000000000000000000000000080",
  "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05",
  "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc85",
  "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a",
  "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac03fa",
  "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
  "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
  "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
  "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
  "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
  "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
]);

export function ed25519WeakEncodings(): string[] {
  return [...ED25519_WEAK];
}

export function isWeakEd25519(key: Uint8Array): boolean {
  return key.length !== 32 || ED25519_WEAK.has(hex(key));
}

/** The order of the Ed25519 group, little-endian. */
const L = fromHex("edd3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010");

/** Whether a little-endian 32-byte scalar is below L. */
export function isCanonicalScalar(s: Uint8Array): boolean {
  for (let i = 31; i >= 0; i--) {
    if (s[i]! < L[i]!) return true;
    if (s[i]! > L[i]!) return false;
  }
  return false;
}

export async function ed25519Sign(privateKey: CryptoKey, message: Uint8Array): Promise<Bytes> {
  return new Uint8Array(await subtle().sign({ name: "Ed25519" }, privateKey, copy(message)));
}

/** Ed25519 verification as strict as `ed25519-dalek`'s `verify_strict`. */
export async function ed25519VerifyStrict(key: Uint8Array, message: Uint8Array, signature: Uint8Array): Promise<boolean> {
  if (signature.length !== 64 || !canonicalField(key) || isWeakEd25519(key) || !ed25519PointDecodes(key)) return false;
  if (isWeakEd25519(signature.subarray(0, 32)) || !isCanonicalScalar(signature.subarray(32))) return false;
  try {
    return await subtle().verify({ name: "Ed25519" }, await importEd25519Public(key), copy(signature), copy(message));
  } catch {
    return false;
  }
}

const EDWARDS_D = 37095705934669439343138083508754565189542113879843219016388785533085940283555n;

/**
 * Whether a canonical encoding names a point on the curve: x² = (y² - 1) / (d·y² + 1)
 * must have a square root. Some WebCrypto implementations import any 32 bytes
 * as a public key, so this is checked here rather than left to them.
 */
function ed25519PointDecodes(key: Uint8Array): boolean {
  // The top bit is the encoding's sign, not part of y.
  const y = decodeLe(Uint8Array.from(key, (b, i) => (i === 31 ? b & 0x7f : b)));
  const y2 = (y * y) % FIELD_P;
  const x2 = (((y2 - 1n + FIELD_P) % FIELD_P) * inv0((EDWARDS_D * y2 + 1n) % FIELD_P)) % FIELD_P;
  return x2 === 0n || isSquare(x2);
}

/** Whether `key` decodes as an Ed25519 point that is not of small order. */
export async function isValidEd25519Key(key: Uint8Array): Promise<boolean> {
  if (!canonicalField(key) || isWeakEd25519(key) || !ed25519PointDecodes(key)) return false;
  try {
    await importEd25519Public(key);
    return true;
  } catch {
    return false;
  }
}

export async function sha256(...parts: Uint8Array[]): Promise<Bytes> {
  return new Uint8Array(await subtle().digest("SHA-256", concat(...parts)));
}

/** The first 16 bytes of `SHA-256("rh/v1 fingerprint\0" ‖ key)`. */
export async function fingerprint(key: Uint8Array): Promise<Bytes> {
  return (await sha256(utf8("rh/v1 fingerprint\0"), key)).slice(0, 16);
}

/** A device's own keys: private halves non-extractable. */
export interface DeviceKeys {
  sign: CryptoKeyPair;
  dh: CryptoKeyPair;
  signPublic: Bytes;
  dhPublic: Bytes;
}

export async function generateDeviceKeys(): Promise<DeviceKeys> {
  const sign = (await subtle().generateKey({ name: "Ed25519" }, false, ["sign", "verify"])) as CryptoKeyPair;
  const dhPair = (await subtle().generateKey({ name: "X25519" }, false, ["deriveBits"])) as CryptoKeyPair;
  return { sign, dh: dhPair, signPublic: await exportPublic(sign.publicKey), dhPublic: await exportPublic(dhPair.publicKey) };
}
