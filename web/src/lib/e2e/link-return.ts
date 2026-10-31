/*
 * Signing a device in to Droppen through the system browser, browser side
 * (`server-rs/crates/droppen-proto/src/link_return.rs` — read that for the why):
 * droppen.app seals the enrollment code to the sign-in attempt's one-off
 * X25519 key `E`, bound to the attempt's `state` and to what the consent page
 * showed, and the device's own page hands it to its server.
 *
 * `sealed = w ‖ nonce(12) ‖ AES-256-GCM(kek, nonce, aad = context, plaintext)`,
 * `kek = HKDF-SHA256(salt = "droppen/v1 link-return", ikm = X25519(w, E) ‖ state, info = w ‖ E ‖ context)`,
 * `context = "droppen/v1 link-return\0" | str16 attempt | str16 origin | sign key(32) | dh key(32)`.
 */
import { type Bytes, bytes, concat, copy, utf8, Writer } from "./bytes";
import { dh, exportPublic } from "./keys";
import type { NoiseKeyPair } from "./noise";

const SALT = utf8("droppen/v1 link-return");
const CONTEXT_LABEL = utf8("droppen/v1 link-return\0");
/** The `state`'s length. */
export const LINK_RETURN_STATE_LEN = 16;
/** The longest plaintext sealed. */
export const LINK_RETURN_MAX_PLAINTEXT = 1024;

/** What a reply is bound to. */
export interface LinkReturnContext {
  /** The sign-in attempt's id. */
  attempt: string;
  /** The origin of the page the reply goes back to. */
  origin: string;
  /** The device's Ed25519 key, as the consent page showed it. */
  signKey: Uint8Array;
  /** The device's X25519 key, as the consent page showed it. */
  dhKey: Uint8Array;
}

export function encodeLinkReturnContext(c: LinkReturnContext): Bytes {
  if (c.signKey.length !== 32 || c.dhKey.length !== 32) throw new Error("a device key is 32 bytes");
  return new Writer().raw(CONTEXT_LABEL).str16(c.attempt).str16(c.origin).raw(c.signKey).raw(c.dhKey).finish();
}

/**
 * Seal `plaintext` to the attempt's key `recipient` with its `state`.
 * `ephemeral`/`nonce` are for test vectors only.
 */
export async function sealLinkReturn(
  recipient: Uint8Array,
  state: Uint8Array,
  ctx: LinkReturnContext,
  plaintext: Uint8Array,
  opts: { ephemeral?: NoiseKeyPair; nonce?: Uint8Array } = {},
): Promise<Bytes> {
  if (state.length !== LINK_RETURN_STATE_LEN) throw new Error("the state is 16 bytes");
  if (plaintext.length > LINK_RETURN_MAX_PLAINTEXT) throw new Error("the reply is too large");
  let eph = opts.ephemeral;
  if (!eph) {
    const pair = (await crypto.subtle.generateKey({ name: "X25519" }, false, ["deriveBits"])) as CryptoKeyPair;
    eph = { privateKey: pair.privateKey, publicKey: await exportPublic(pair.publicKey) };
  }
  const context = encodeLinkReturnContext(ctx);
  const shared = await dh(eph.privateKey, recipient);
  const ikmBytes = concat(shared, state);
  shared.fill(0);
  let kek: CryptoKey;
  try {
    const ikm = await crypto.subtle.importKey("raw", ikmBytes, "HKDF", false, ["deriveKey"]);
    kek = await crypto.subtle.deriveKey(
      { name: "HKDF", hash: "SHA-256", salt: SALT, info: concat(eph.publicKey, recipient, context) },
      ikm,
      { name: "AES-GCM", length: 256 },
      false,
      ["encrypt"],
    );
  } finally {
    ikmBytes.fill(0);
  }
  const nonce = opts.nonce ? copy(opts.nonce) : crypto.getRandomValues(bytes(12));
  const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv: nonce, additionalData: context }, kek, copy(plaintext)));
  return concat(eph.publicKey, nonce, ct);
}
