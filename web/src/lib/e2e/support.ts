/*
 * Can this browser do end-to-end encryption natively?
 *
 * Everything in the suite is WebCrypto, but X25519 and Ed25519 arrived late
 * (MDN browser-compat-data, October 2026):
 *
 * |               | X25519 | Ed25519 |
 * |---------------|--------|---------|
 * | Chrome / Edge | 133    | 137     |
 * | Firefox       | 130    | 129     |
 * | Safari (macOS and iOS) | 17 | 17 |
 *
 * so current versions of all three have both, and an older browser has to be
 * told so rather than silently falling back to something weaker. Raw deflate
 * (`DecompressionStream("deflate-raw")`, Chrome 103 / Firefox 113 / Safari
 * 16.4) and IndexedDB are checked alongside, since records and key storage need
 * them. The check *exercises* each primitive — a browser that knows an
 * algorithm's name but not how to make a non-extractable key with it fails
 * here, not halfway through a handshake.
 */

export const MINIMUM_BROWSERS = { chrome: 137, edge: 137, firefox: 130, safari: 17 } as const;

export interface E2eSupport {
  ok: boolean;
  missing: string[];
}

export async function checkE2eSupport(env: { crypto?: Crypto; indexedDB?: IDBFactory; requireIndexedDb?: boolean } = {}): Promise<E2eSupport> {
  const c = env.crypto ?? globalThis.crypto;
  const missing: string[] = [];
  const subtle = c?.subtle;
  if (!subtle) return { ok: false, missing: ["WebCrypto (crypto.subtle; needs a secure context)"] };
  const attempt = async (name: string, work: () => Promise<unknown>) => {
    try {
      await work();
    } catch {
      missing.push(name);
    }
  };
  await attempt("X25519", async () => {
    const a = (await subtle.generateKey({ name: "X25519" }, false, ["deriveBits"])) as CryptoKeyPair;
    const b = (await subtle.generateKey({ name: "X25519" }, false, ["deriveBits"])) as CryptoKeyPair;
    await subtle.deriveBits({ name: "X25519", public: b.publicKey }, a.privateKey, 256);
  });
  await attempt("Ed25519", async () => {
    const k = (await subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"])) as CryptoKeyPair;
    const msg = new Uint8Array(4);
    const sig = await subtle.sign({ name: "Ed25519" }, k.privateKey, msg);
    if (!(await subtle.verify({ name: "Ed25519" }, k.publicKey, sig, msg))) throw new Error("verify");
  });
  await attempt("AES-GCM", async () => {
    const k = await subtle.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt"]);
    await subtle.encrypt({ name: "AES-GCM", iv: new Uint8Array(12) }, k, new Uint8Array(1));
  });
  await attempt("HKDF", async () => {
    const k = await subtle.importKey("raw", new Uint8Array(32), "HKDF", false, ["deriveKey"]);
    await subtle.deriveKey({ name: "HKDF", hash: "SHA-256", salt: new Uint8Array(0), info: new Uint8Array(0) }, k, { name: "AES-GCM", length: 256 }, false, ["decrypt"]);
  });
  await attempt("HMAC-SHA-256", async () => {
    const k = await subtle.importKey("raw", new Uint8Array(32), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
    await subtle.sign("HMAC", k, new Uint8Array(0));
  });
  await attempt("deflate-raw streams", async () => {
    new DecompressionStream("deflate-raw");
    new CompressionStream("deflate-raw");
  });
  if (env.requireIndexedDb !== false && !(env.indexedDB ?? globalThis.indexedDB)) missing.push("IndexedDB");
  return { ok: missing.length === 0, missing };
}

/** The two algorithms compatibility mode can stand in for (`compat.ts`); nothing else ever is. */
export const SOFT_ALGORITHMS = ["X25519", "Ed25519"] as const;
export type SoftAlgorithm = (typeof SOFT_ALGORITHMS)[number];

/**
 * Whether only X25519 and/or Ed25519 are missing — everything else (AES-GCM,
 * HKDF, HMAC, raw deflate, IndexedDB) native — so compatibility mode could
 * stand in. Anything else missing: no.
 */
export function compatCovers(support: E2eSupport): boolean {
  return !support.ok && support.missing.length > 0 && support.missing.every((m) => (SOFT_ALGORITHMS as readonly string[]).includes(m));
}

/**
 * Whether `error` is a browser saying it does not know an algorithm
 * (`NotSupportedError`, Chromium's "Algorithm: Unrecognized name"), as
 * opposed to a store or a key that failed: no retry ever helps it.
 */
export function isAlgorithmUnsupported(error: unknown): boolean {
  if (typeof error !== "object" || error === null) return false;
  const { name, message } = error as { name?: unknown; message?: unknown };
  return name === "NotSupportedError" || (typeof message === "string" && /unrecognized (?:algorithm )?name|algorithm(?: is)? not supported|unsupported algorithm/i.test(message));
}

/** This browser cannot do the end-to-end encryption (`missing`, as {@link checkE2eSupport} names it): nothing was read or made. */
export class E2eUnsupportedError extends Error {
  constructor(
    readonly missing: string[],
    detail?: string,
  ) {
    super(`this browser cannot run the end-to-end encryption${missing.length ? ` (missing: ${missing.join(", ")})` : ""}${detail ? `: ${detail}` : ""}`);
    this.name = "E2eUnsupportedError";
  }
}

/** `error` (an algorithm this browser does not know) as an {@link E2eUnsupportedError} naming what is missing, checked afresh. */
export async function unsupportedFrom(error: unknown): Promise<E2eUnsupportedError> {
  const detail = error instanceof Error ? `${error.name}: ${error.message}` : String(error);
  const { missing } = await checkE2eSupport({ requireIndexedDb: false });
  return new E2eUnsupportedError(missing, detail);
}
