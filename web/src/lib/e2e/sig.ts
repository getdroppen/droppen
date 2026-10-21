/*
 * Every Ed25519 signature's label (`server-rs/crates/rh-proto/src/sig.rs`):
 * each signature covers a fixed NUL-terminated label and then a fixed binary
 * structure, so no key ever signs bytes the hub chose and no signed structure
 * can be read as another.
 */
import { type Bytes, concat, utf8, Writer } from "./bytes";
import { ed25519Sign, ed25519VerifyStrict } from "./keys";

export const CHAIN_ENTRY = utf8("rh/v1 chain-entry\0");
export const CHAIN_ATTEST = utf8("rh/v1 chain-attest\0");
export const STREAM_HEAD = utf8("rh/v1 stream-head\0");
export const STREAM_MANIFEST = utf8("rh/v1 stream-manifest\0");
export const HUB_LOGIN = utf8("rh/v1 hub-login\0");
export const PUSH_PAYLOAD = utf8("rh/v1 push\0");
/** `RSK`'s proof that it belongs to an account and its root key (`chain.ts` `recoveryProofBody`). */
export const RECOVERY_PROOF = utf8("rh/v1 recovery-proof\0");
/** A machine's signature over a draft: `stream | epoch | nonce | ct` (`stream.ts` `sealEphemeral`). */
export const STREAM_DRAFT = utf8("rh/v1 stream-draft\0");
/** The Hub's signature over a relay ticket (`ticket.rs`); browsers never verify one — tickets are opaque here. */
export const RELAY_TICKET = utf8("rh/v1 relay-ticket\0");
/** A relay's proof that it answers at its origin. */
export const RELAY_ORIGIN = utf8("rh/v1 relay-origin\0");
/** A passkey slot's `PAK` proving it belongs to an account, its root, the slot, credential and salt (`chain.ts` `passkeyProofBody`). */
export const PASSKEY_PROOF = utf8("rh/v1 passkey-proof\0");
/** A host's current and its new identity key over its renewal's proof (`chain.ts` `hostRotateBody`; `docs/host-identity-rotation.md` §3.2). */
export const HOST_ROTATE = utf8("rh/v1 host-rotate\0");
/** A host's new identity key confirming a staged renewal on a connection signed in with the old one (`chain.ts` `rotateAckBody`, §3.3). */
export const ROTATE_ACK = utf8("rh/v1 rotate-ack\0");
/** The sign key a renewal retired, disputing that renewal by its hash (`chain.ts` `rotateDisputeBody`, §3.8). */
export const ROTATE_DISPUTE = utf8("rh/v1 rotate-dispute\0");
export const LABELS = [
  CHAIN_ENTRY,
  CHAIN_ATTEST,
  STREAM_HEAD,
  STREAM_MANIFEST,
  HUB_LOGIN,
  PUSH_PAYLOAD,
  RECOVERY_PROOF,
  STREAM_DRAFT,
  RELAY_TICKET,
  RELAY_ORIGIN,
  PASSKEY_PROOF,
  HOST_ROTATE,
  ROTATE_ACK,
  ROTATE_DISPUTE,
] as const;

export function signLabelled(key: CryptoKey, label: Uint8Array, body: Uint8Array): Promise<Bytes> {
  if (!LABELS.includes(label as (typeof LABELS)[number])) throw new Error("an unregistered signature label");
  return ed25519Sign(key, concat(label, body));
}

export function verifyLabelled(key: Uint8Array, label: Uint8Array, body: Uint8Array, signature: Uint8Array): Promise<boolean> {
  if (!LABELS.includes(label as (typeof LABELS)[number])) throw new Error("an unregistered signature label");
  return ed25519VerifyStrict(key, concat(label, body), signature);
}

/**
 * Who answers a hub's (or a relay's) challenge, as its role byte: an agent's
 * WebSocket sign-in (1), a device's WebSocket sign-in (2), a device's HTTP
 * session exchange `POST /v1/session` (3), a device signing in on a relay
 * with its ticket (4; `hub` is then the relay's origin), a machine doing the
 * same (5), or a relay on the Hub's control link (6) — kept apart so an
 * answer to one is never an answer to another. Browsers sign 2, 3 and 4.
 */
export type LoginRole = "agent" | "device" | "deviceSession" | "relayDevice" | "relayAgent" | "relay";
const LOGIN_ROLE: Record<LoginRole, number> = { agent: 1, device: 2, deviceSession: 3, relayDevice: 4, relayAgent: 5, relay: 6 };

/** The answer to a hub's sign-in challenge: the hub's nonce inside a signed structure, never alone. */
export interface HubLogin {
  role: LoginRole;
  account: string;
  id: string;
  hub: string;
  nonce: Uint8Array;
  timeMs: number;
}

export function encodeHubLogin(l: HubLogin): Bytes {
  if (l.nonce.length !== 32) throw new Error("a hub nonce is 32 bytes");
  return new Writer().u8(LOGIN_ROLE[l.role]).str16(l.account).str16(l.id).str16(l.hub).raw(l.nonce).u64(l.timeMs).finish();
}

export function signHubLogin(l: HubLogin, key: CryptoKey): Promise<Bytes> {
  return signLabelled(key, HUB_LOGIN, encodeHubLogin(l));
}

export function verifyHubLogin(l: HubLogin, key: Uint8Array, signature: Uint8Array): Promise<boolean> {
  return verifyLabelled(key, HUB_LOGIN, encodeHubLogin(l), signature);
}
