/*
 * CPace over X25519 for pairing by numeric code (`droppen-proto/src/pake.rs` has the
 * construction and why). X25519 with an arbitrary point is WebCrypto's own
 * `deriveBits`; only Elligator 2 is written out (`field.ts`).
 */
import { type Bytes, concat, copy, utf8, Writer } from "./bytes";
import { elligator2 } from "./field";
import { dh, exportPublic, importX25519Private } from "./keys";

export const SID_LEN = 16;
const DSI = utf8("rh/v1 cpace");
const ISK_LABEL = utf8("rh/v1 cpace isk\0");
const KEYS_SALT = utf8("rh/v1 cpace keys");

export type PakeRole = "initiator" | "responder";

export interface PakeKeys {
  confirmInitiator: Bytes;
  confirmResponder: Bytes;
  pairSecret: Bytes;
}

function lv(w: Writer, data: Uint8Array): Writer {
  return w.u32(data.length).raw(data);
}

async function sha512(data: Uint8Array): Promise<Bytes> {
  return new Uint8Array(await crypto.subtle.digest("SHA-512", copy(data)));
}

/** `G = elligator2(SHA-512(DSI ‖ lv(prs) ‖ lv(ci) ‖ lv(sid))[..32])`. */
export async function cpaceGenerator(prs: Uint8Array, ci: Uint8Array, sid: Uint8Array): Promise<Bytes> {
  const hash = await sha512(lv(lv(lv(new Writer().raw(DSI), prs), ci), sid).finish());
  return copy(elligator2(hash.subarray(0, 32)));
}

async function expand(ikm: Uint8Array, info: string): Promise<Bytes> {
  const key = await crypto.subtle.importKey("raw", copy(ikm), "HKDF", false, ["deriveBits"]);
  return new Uint8Array(await crypto.subtle.deriveBits({ name: "HKDF", hash: "SHA-256", salt: KEYS_SALT, info: utf8(info) }, key, 256));
}

/** One end of a run. */
export class Cpace {
  private constructor(
    readonly role: PakeRole,
    private readonly sid: Bytes,
    private readonly secret: CryptoKey,
    readonly share: Bytes,
  ) {}

  /** Start a run; `scalar` only for test vectors (otherwise a fresh WebCrypto key). */
  static async start(role: PakeRole, prs: Uint8Array, ci: Uint8Array, sid: Uint8Array, scalar?: Uint8Array): Promise<Cpace> {
    const g = await cpaceGenerator(prs, ci, sid);
    let secret: CryptoKey;
    if (scalar) secret = await importX25519Private(scalar);
    else secret = ((await crypto.subtle.generateKey({ name: "X25519" }, false, ["deriveBits"])) as CryptoKeyPair).privateKey;
    // X25519(y, G); a low-order G or share is refused by `dh`.
    const share = await dh(secret, g);
    return new Cpace(role, copy(sid), secret, share);
  }

  async finish(peer: Uint8Array): Promise<PakeKeys> {
    const k = await dh(this.secret, peer);
    const [ya, yb] = this.role === "initiator" ? [this.share, peer] : [peer, this.share];
    const hash = await sha512(concat(lv(new Writer().raw(ISK_LABEL), this.sid).finish(), k, ya, yb));
    k.fill(0);
    const isk = hash.slice(0, 32);
    hash.fill(0);
    try {
      return {
        confirmInitiator: await expand(isk, "confirm initiator"),
        confirmResponder: await expand(isk, "confirm responder"),
        pairSecret: await expand(isk, "pair secret"),
      };
    } finally {
      isk.fill(0);
    }
  }
}

/** A fresh random session id. */
export function newSid(): Bytes {
  return crypto.getRandomValues(new Uint8Array(SID_LEN));
}

/** A uniformly random 8-digit code (rejection-sampled). */
export function newPairingCode(): string {
  const limit = 4_200_000_000; // a multiple of 10^8 below 2^32
  for (;;) {
    const [v] = crypto.getRandomValues(new Uint32Array(1));
    if (v! < limit) return String(v! % 100_000_000).padStart(8, "0");
  }
}

export { exportPublic };
