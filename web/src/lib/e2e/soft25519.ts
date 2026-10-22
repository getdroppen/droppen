/*
 * Compatibility mode's curves: Ed25519 and X25519 in JavaScript
 * (`@noble/curves`), behind WebCrypto's own `crypto.subtle`, for a browser
 * whose WebCrypto lacks them (`support.ts`). Installed only once the person
 * agreed to it (`compat.ts`), and only for the algorithms the browser lacks:
 * every other algorithm, and either curve the browser has, stays native.
 *
 * Every caller (`keys.ts`, `keystore.ts`, Noise, the chain, ...) goes on
 * calling `crypto.subtle` exactly as before and gets the same bytes: RFC 8032
 * signatures are deterministic and X25519 is a function, so a key made here
 * signs and agrees exactly as the browser's own would
 * (`test/e2e-soft-curves.test.ts` holds both to the RFC vectors and to each
 * other). What differs is where a private key lives: a native one is a handle
 * the browser will not hand out (made non-extractable); here its 32 bytes sit
 * in this page's memory, in a `WeakMap` only this module reads, which code
 * that got into the page could still find and copy. That is the risk the
 * person accepts. At rest nothing changes: device keys are kept as PKCS #8
 * sealed under a non-extractable AES-GCM key (`keystore.ts`), which stays
 * native.
 *
 * A key from here is not a `CryptoKey`: it has a `CryptoKey`'s `type`,
 * `extractable`, `algorithm` and `usages`, and only these methods take it.
 * Stored in IndexedDB it would keep those four fields and none of its bytes.
 */
import { ed25519, x25519 } from "@noble/curves/ed25519.js";
import { ED25519_PKCS8_PREFIX, X25519_PKCS8_PREFIX } from "./keys";
import type { SoftAlgorithm } from "./support";

const ED25519_SPKI_PREFIX = Uint8Array.from([0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00]);
const X25519_SPKI_PREFIX = Uint8Array.from([0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x6e, 0x03, 0x21, 0x00]);

interface Held {
  /** The Ed25519 seed or the X25519 scalar (private keys only). */
  secret?: Uint8Array;
  publicKey: Uint8Array;
}

const held = new WeakMap<SoftKey, Held>();

/** A key of compatibility mode: a `CryptoKey`'s fields, its bytes kept apart. */
class SoftKey {
  readonly algorithm: KeyAlgorithm;
  readonly usages: KeyUsage[];
  constructor(
    readonly type: "public" | "private",
    readonly extractable: boolean,
    curve: SoftAlgorithm,
    usages: KeyUsage[],
    bytes: Held,
  ) {
    this.algorithm = Object.freeze({ name: curve });
    this.usages = usages;
    held.set(this, bytes);
  }
}

const fail = (name: string, message: string) => new DOMException(message, name);

function nameOf(alg: unknown): string | null {
  const name = typeof alg === "string" ? alg : typeof alg === "object" && alg !== null ? (alg as { name?: unknown }).name : null;
  return typeof name === "string" ? name : null;
}

function curveNamed(alg: unknown): SoftAlgorithm | null {
  const name = nameOf(alg)?.toLowerCase();
  return name === "ed25519" ? "Ed25519" : name === "x25519" ? "X25519" : null;
}

/** A copy of `data`'s bytes (a buffer or a view of one, from any realm). */
function bytesOf(data: unknown): Uint8Array {
  if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength).slice();
  const tag = Object.prototype.toString.call(data);
  if (tag === "[object ArrayBuffer]" || tag === "[object SharedArrayBuffer]") return new Uint8Array(data as ArrayBuffer).slice();
  throw new TypeError("not a BufferSource");
}

/** `bytes` as a fresh `ArrayBuffer`, what WebCrypto's promises resolve to. */
function buffer(bytes: Uint8Array): ArrayBuffer {
  return bytes.slice().buffer;
}

function startsWith(data: Uint8Array, prefix: Uint8Array): boolean {
  return data.length >= prefix.length && prefix.every((b, i) => data[i] === b);
}

function base64Url(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function fromBase64Url(text: unknown): Uint8Array {
  if (typeof text !== "string") throw fail("DataError", "a JWK member is missing");
  const b64 = text.replace(/-/g, "+").replace(/_/g, "/");
  try {
    return Uint8Array.from(atob(b64 + "=".repeat((4 - (b64.length % 4)) % 4)), (c) => c.charCodeAt(0));
  } catch {
    throw fail("DataError", "a JWK member is not base64url");
  }
}

function publicOf(curve: SoftAlgorithm, secret: Uint8Array): Uint8Array {
  return curve === "Ed25519" ? ed25519.getPublicKey(secret) : x25519.getPublicKey(secret);
}

/** What each kind of key of `curve` may be used for. */
function allowed(curve: SoftAlgorithm, type: "public" | "private"): KeyUsage[] {
  if (curve === "Ed25519") return type === "private" ? ["sign"] : ["verify"];
  return type === "private" ? ["deriveKey", "deriveBits"] : [];
}

function checkUsages(curve: SoftAlgorithm, type: "public" | "private", usages: readonly KeyUsage[]): KeyUsage[] {
  const ok = allowed(curve, type);
  if (usages.some((u) => !ok.includes(u))) throw fail("SyntaxError", `a ${curve} ${type} key cannot be used for that`);
  if (type === "private" && usages.length === 0) throw fail("SyntaxError", `a ${curve} private key needs a usage`);
  return [...new Set(usages)];
}

function privateKey(curve: SoftAlgorithm, secret: Uint8Array, extractable: boolean, usages: KeyUsage[]): SoftKey {
  if (secret.length !== 32) throw fail("DataError", `a ${curve} private key is 32 bytes`);
  return new SoftKey("private", extractable, curve, usages, { secret, publicKey: publicOf(curve, secret) });
}

function publicKey(curve: SoftAlgorithm, raw: Uint8Array, extractable: boolean, usages: KeyUsage[]): SoftKey {
  if (raw.length !== 32) throw fail("DataError", `a ${curve} public key is 32 bytes`);
  return new SoftKey("public", extractable, curve, usages, { publicKey: raw });
}

/** `key`, held here, of `curve` and `type`, allowed `usage`. */
function own(key: unknown, curve: SoftAlgorithm, type: "public" | "private", usage?: KeyUsage): Held {
  const bytes = key instanceof SoftKey ? held.get(key) : undefined;
  if (!bytes || !(key instanceof SoftKey) || key.algorithm.name !== curve || key.type !== type) throw fail("InvalidAccessError", `not a ${curve} ${type} key`);
  if (usage && !key.usages.includes(usage)) throw fail("InvalidAccessError", `the key may not ${usage}`);
  return bytes;
}

type Method = "generateKey" | "importKey" | "exportKey" | "sign" | "verify" | "deriveBits";
const METHODS: readonly Method[] = ["generateKey", "importKey", "exportKey", "sign", "verify", "deriveBits"];
type Call = (...args: unknown[]) => Promise<unknown>;

/**
 * `native`'s six key methods, with `curves` done here: the methods
 * {@link installSoftCurves} puts on `crypto.subtle`, and what the tests hold
 * against the browser's own.
 */
export function softSubtle(native: SubtleCrypto, curves: Iterable<SoftAlgorithm>): Record<Method, Call> {
  const active = curves instanceof Set ? (curves as Set<SoftAlgorithm>) : new Set(curves);
  const base = Object.fromEntries(METHODS.map((m) => [m, (native[m] as unknown as Call).bind(native)])) as Record<Method, Call>;
  const soft = (alg: unknown): SoftAlgorithm | null => {
    const curve = curveNamed(alg);
    return curve && active.has(curve) ? curve : null;
  };
  return {
    async generateKey(alg, extractable, usages) {
      const curve = soft(alg);
      if (!curve) return base.generateKey(alg, extractable, usages);
      const wanted = usages as KeyUsage[];
      const forPrivate = allowed(curve, "private");
      const forPublic = allowed(curve, "public");
      if (wanted.some((u) => !forPrivate.includes(u) && !forPublic.includes(u))) throw fail("SyntaxError", `a ${curve} key cannot be used for that`);
      const priv = privateKey(curve, crypto.getRandomValues(new Uint8Array(32)), !!extractable, checkUsages(curve, "private", wanted.filter((u) => forPrivate.includes(u))));
      // A public key is always extractable (WebCrypto's rule too).
      const pub = publicKey(curve, held.get(priv)!.publicKey.slice(), true, [...new Set(wanted.filter((u) => forPublic.includes(u)))]);
      return { privateKey: priv, publicKey: pub };
    },
    async importKey(format, data, alg, extractable, usages) {
      const curve = soft(alg);
      if (!curve) return base.importKey(format, data, alg, extractable, usages);
      const wanted = usages as KeyUsage[];
      if (format === "raw" || format === "spki") {
        let raw = bytesOf(data);
        if (format === "spki") {
          const prefix = curve === "Ed25519" ? ED25519_SPKI_PREFIX : X25519_SPKI_PREFIX;
          if (raw.length !== prefix.length + 32 || !startsWith(raw, prefix)) throw fail("DataError", `not a ${curve} SPKI`);
          raw = raw.slice(prefix.length);
        }
        return publicKey(curve, raw, !!extractable, checkUsages(curve, "public", wanted));
      }
      if (format === "pkcs8") {
        const der = bytesOf(data);
        const prefix = curve === "Ed25519" ? ED25519_PKCS8_PREFIX : X25519_PKCS8_PREFIX;
        try {
          if (der.length !== prefix.length + 32 || !startsWith(der, prefix)) throw fail("DataError", `not a ${curve} PKCS #8`);
          return privateKey(curve, der.slice(prefix.length), !!extractable, checkUsages(curve, "private", wanted));
        } finally {
          der.fill(0);
        }
      }
      if (format === "jwk") {
        const jwk = data as JsonWebKey;
        if (jwk?.kty !== "OKP" || jwk.crv !== curve) throw fail("DataError", `not a ${curve} JWK`);
        if (jwk.d === undefined) return publicKey(curve, fromBase64Url(jwk.x), !!extractable, checkUsages(curve, "public", wanted));
        const key = privateKey(curve, fromBase64Url(jwk.d), !!extractable, checkUsages(curve, "private", wanted));
        const x = jwk.x === undefined ? null : fromBase64Url(jwk.x);
        const pub = held.get(key)!.publicKey;
        if (x && (x.length !== pub.length || x.some((b, i) => b !== pub[i]))) throw fail("DataError", "the JWK's public half does not match its private half");
        return key;
      }
      throw fail("NotSupportedError", `${curve} keys do not import as ${String(format)}`);
    },
    async exportKey(format, key) {
      if (!(key instanceof SoftKey)) return base.exportKey(format, key);
      const curve = key.algorithm.name as SoftAlgorithm;
      const bytes = held.get(key)!;
      if (!key.extractable) throw fail("InvalidAccessError", "the key is not extractable");
      if (format === "raw" || format === "spki") {
        if (key.type !== "public") throw fail("InvalidAccessError", `a private key does not export as ${format}`);
        const prefix = curve === "Ed25519" ? ED25519_SPKI_PREFIX : X25519_SPKI_PREFIX;
        return buffer(format === "raw" ? bytes.publicKey : Uint8Array.from([...prefix, ...bytes.publicKey]));
      }
      if (format === "pkcs8") {
        if (key.type !== "private") throw fail("InvalidAccessError", "a public key does not export as pkcs8");
        const prefix = curve === "Ed25519" ? ED25519_PKCS8_PREFIX : X25519_PKCS8_PREFIX;
        const der = new Uint8Array(prefix.length + 32);
        der.set(prefix);
        der.set(bytes.secret!, prefix.length);
        return der.buffer;
      }
      if (format === "jwk") {
        const jwk: JsonWebKey = { kty: "OKP", crv: curve, x: base64Url(bytes.publicKey), key_ops: [...key.usages], ext: key.extractable };
        if (key.type === "private") jwk.d = base64Url(bytes.secret!);
        return jwk;
      }
      throw fail("NotSupportedError", `${curve} keys do not export as ${String(format)}`);
    },
    async sign(alg, key, data) {
      if (soft(alg) !== "Ed25519") return base.sign(alg, key, data);
      const { secret } = own(key, "Ed25519", "private", "sign");
      return buffer(ed25519.sign(bytesOf(data), secret!));
    },
    async verify(alg, key, signature, data) {
      if (soft(alg) !== "Ed25519") return base.verify(alg, key, signature, data);
      const { publicKey: pub } = own(key, "Ed25519", "public", "verify");
      const sig = bytesOf(signature);
      if (sig.length !== 64) return false;
      try {
        // RFC 8032's checks (canonical encodings, a key not of small order), not ZIP 215's looser ones.
        return ed25519.verify(sig, bytesOf(data), pub, { zip215: false });
      } catch {
        return false;
      }
    },
    async deriveBits(alg, baseKey, length) {
      if (soft(alg) !== "X25519") return base.deriveBits(alg, baseKey, length);
      const { secret } = own(baseKey, "X25519", "private", "deriveBits");
      const { publicKey: peer } = own((alg as { public?: unknown }).public, "X25519", "public");
      const bits = length === null || length === undefined ? 256 : Number(length);
      if (!Number.isInteger(bits) || bits < 0 || bits > 256) throw fail("OperationError", "X25519 gives at most 256 bits");
      let shared: Uint8Array;
      try {
        shared = x25519.getSharedSecret(secret!, peer);
      } catch {
        // A low-order peer: the all-zero secret, which WebCrypto refuses too.
        throw fail("OperationError", "the X25519 result is all zero");
      }
      try {
        if (shared.every((b) => b === 0)) throw fail("OperationError", "the X25519 result is all zero");
        const out = shared.slice(0, Math.ceil(bits / 8));
        if (bits % 8) out[out.length - 1] = out[out.length - 1]! & (0xff << (8 - (bits % 8)));
        return out.buffer;
      } finally {
        shared.fill(0);
      }
    },
  };
}

const installed = new WeakMap<SubtleCrypto, { curves: Set<SoftAlgorithm>; before: Map<Method, PropertyDescriptor | undefined> }>();

/**
 * Do `curves` in JavaScript on `subtle` (the page's `crypto.subtle`) from now
 * on, for this page's life: its own methods for everything else. Called again,
 * it adds to the curves.
 */
export function installSoftCurves(curves: readonly SoftAlgorithm[], subtle: SubtleCrypto = globalThis.crypto.subtle): void {
  const done = installed.get(subtle);
  if (done) {
    for (const c of curves) done.curves.add(c);
    return;
  }
  const active = new Set(curves);
  const before = new Map(METHODS.map((m) => [m, Object.getOwnPropertyDescriptor(subtle, m)] as const));
  const methods = softSubtle(subtle, active);
  for (const m of METHODS) Object.defineProperty(subtle, m, { configurable: true, writable: true, value: methods[m] });
  installed.set(subtle, { curves: active, before });
}

/** The curves `subtle` does in JavaScript (none when not installed). */
export function softCurvesOn(subtle: SubtleCrypto = globalThis.crypto.subtle): SoftAlgorithm[] {
  return [...(installed.get(subtle)?.curves ?? [])];
}

/** Undo {@link installSoftCurves} (tests: one file holds both paths). */
export function uninstallSoftCurves(subtle: SubtleCrypto = globalThis.crypto.subtle): void {
  const done = installed.get(subtle);
  if (!done) return;
  for (const m of METHODS) {
    const d = done.before.get(m);
    if (d) Object.defineProperty(subtle, m, d);
    else delete (subtle as unknown as Record<string, unknown>)[m];
  }
  installed.delete(subtle);
}
