/*
 * The device chain, browser side: build, sign and verify the signed,
 * append-only list of an account's devices. The encoding and the rules are
 * `server-rs/crates/rh-proto/src/chain.rs`'s, check for check and in the same
 * order, so both sides refuse the same chains for the same reason — read that
 * file for the reasoning; in short:
 *
 * - Chain v3 (`docs/local-aggregator.md` §3): every member is a **device**
 *   with two flags, `host` (runs AI: an agent, machine keys) and `admin`
 *   (manages the account: signs entries, holds `UK`, attests, delivers keys).
 *   Entries carry a version byte (2 or 3, signed). A v2 chain maps onto the
 *   same state: a full device is an admin, a limited one a plain device, a
 *   machine a host that is not an admin. Once a chain holds a v3 entry, no v2
 *   entry may follow (`isV3`, `v3From`).
 * - `AK` signs genesis (`genesis` / `genesis3`) and nothing else, and is
 *   destroyed after it; it is the account's root identity (pinned by
 *   machines), not a standing authority.
 * - Ordinary entries are signed by an active admin device (in a v2 entry, a
 *   host is never a signer). No device grants itself admin or host; the last
 *   admin can be neither demoted nor removed.
 * - The recovery key `RSK` (derived only from the user's `RK`, committed in
 *   genesis, replaceable with `setRecoveryKey`) signs `recoveryJoin` /
 *   `recoveryJoin3`, which add a *pending* admin, and `activateRecovered`,
 *   which a verifier accepts only once it has itself known about the join for
 *   `RECOVERY_DELAY_MS`. An active admin can `approveDevice` sooner, or veto
 *   with a removal. The chain holds `RSK` with its own signature over the
 *   account and `AK` (`RecoveryKey`), so a hub cannot build a chain of its own
 *   around the account's public `RSK`.
 * - A passkey slot (`passkeySlotSet`, `docs/passkeys.md` §4.4) signs only a
 *   flag-less pending `addDevice3` and that device's `passkeyActivate` (after
 *   `PASSKEY_DELAY_MS` on the verifier's own clock).
 * - `keyEpochs` commits each new user key epoch and each current host's key
 *   epoch, one past the last; every `UK` or `MK` a device or agent accepts
 *   must hash to its commitment (`checkUserKey`, `checkMachineKey`). A host
 *   that joins (or restarts hosting) gets its machine key epoch in the very
 *   next entry (`hostKeyDue`).
 * - A host accepts control from admin devices, and from others only as an
 *   admin's `setAccept` says (`mayControl`); never from itself.
 * - A current host renews its identity key with `rotateHostKey`
 *   (`docs/host-identity-rotation.md` §3.4): an admin signs the entry, the
 *   host's current and new keys both sign the proof inside it
 *   (`hostRotateBody`), the new keys are ones the chain never named
 *   (`everNamed`), and its next machine key epoch comes next. The id stays; the
 *   old keys are kept (`retiredKeys`, `keysAt`) and never name a device again
 *   (`RULES.keyRetired`).
 * - Attestations let a verifier tell whether an admin device has vouched for
 *   its head recently (a hub withholding entries shows up as staleness):
 *   within `CONTROL_FRESHNESS_MS` to be controlled, `MANAGE_FRESHNESS_MS` for
 *   management.
 *
 * Signing happens with non-extractable Ed25519 `CryptoKey`s. `apply` is
 * serialised: two concurrent calls must not both extend the same head.
 */
import { base32, type Bytes, bytes, concat, copy, DecodeError, equal, fromHex, hex, Reader, utf8, Writer } from "./bytes";
import { dh, fingerprint, importEd25519Private, importX25519Private, isValidEd25519Key, isValidX25519, LowOrderKeyError, publicFromSecret, sha256 } from "./keys";
import { Serial } from "./serial";
import { CHAIN_ATTEST, CHAIN_ENTRY, HOST_ROTATE, PASSKEY_PROOF, RECOVERY_PROOF, ROTATE_ACK, ROTATE_DISPUTE, signLabelled, verifyLabelled } from "./sig";

/** The v2 entry encoding's version byte (recovery keys carry their proof, and key epochs are committed). */
export const CHAIN_VERSION = 2;
/** The v3 entry encoding's version byte: one kind of member, the device, with flags. */
export const CHAIN_VERSION_3 = 3;
export const MAX_ID_LEN = 64;
export const RECOVERY_DELAY_MS = 24 * 60 * 60 * 1000;
/** How long a verifier must have known about a passkey join before it accepts the device activating itself. */
export const PASSKEY_DELAY_MS = 24 * 60 * 60 * 1000;
export const ATTESTATION_SKEW_MS = 5 * 60 * 1000;
/** The most machines one `keyEpochs` entry may move on. */
export const MAX_KEY_COMMITS = 256;
/** The most passkey slots an account may have at once. */
export const MAX_PASSKEY_SLOTS = 8;
/** The most devices one `setAccept` may name. */
export const MAX_ACCEPT_IDS = 64;
/** How recently an admin must have vouched for a host's head for it to accept control at all. */
export const CONTROL_FRESHNESS_MS = 7 * 24 * 60 * 60 * 1000;
/** How recently an admin must have vouched for a host's head for it to accept a management request. */
export const MANAGE_FRESHNESS_MS = 12 * 60 * 60 * 1000;
const HASH_LABEL = utf8("rh/v1 chain-hash\0");
export const KEY_COMMIT_LABEL = utf8("rh/v1 key-commit\0");

/**
 * What a request asks a host to do, for the freshness it needs
 * (`docs/local-aggregator.md` §3.4): open a channel and use it (`control`), or
 * change keys or the host's identity (`manage`).
 */
export type Operation = "control" | "manage";

/** The freshness window `op` needs (`CONTROL_FRESHNESS_MS` / `MANAGE_FRESHNESS_MS`). */
export function operationMaxAgeMs(op: Operation): number {
  return op === "control" ? CONTROL_FRESHNESS_MS : MANAGE_FRESHNESS_MS;
}

/**
 * A recovery key `RSK` as the chain holds it, with `RSK`'s own signature over
 * the account and its root key (`recoveryProofBody`): only someone holding
 * `RK` can put this `RSK` into a chain, and only into the account whose `AK`
 * it signed.
 */
export interface RecoveryKey {
  key: Bytes;
  proof: Bytes;
}

/** What a recovery key's proof signs: `str16 account | AK 32 | RSK 32`. */
export function recoveryProofBody(account: string, accountKey: Uint8Array, rsk: Uint8Array): Bytes {
  return new Writer().str16(account).raw(accountKey).raw(rsk).finish();
}

/** `RSK`'s proof (`signing` is its private half, `rskPublic` its public key) for `account` with root key `accountKey`. */
export function recoveryKeyProof(signing: CryptoKey, rskPublic: Uint8Array, account: string, accountKey: Uint8Array): Promise<Bytes> {
  return signLabelled(signing, RECOVERY_PROOF, recoveryProofBody(account, accountKey, rskPublic));
}

/** The recovery key to put in genesis or `setRecoveryKey` (where `accountKey` is `chain.accountKey`). */
export async function recoveryKeyFor(keys: Pick<RecoveryKeys, "signing" | "signingPublic">, account: string, accountKey: Uint8Array): Promise<RecoveryKey> {
  return { key: copy(keys.signingPublic), proof: await recoveryKeyProof(keys.signing, keys.signingPublic, account, accountKey) };
}

/** Whether `rk.proof` is `rk.key`'s signature over this account and root. */
export function verifyRecoveryKey(rk: RecoveryKey, account: string, accountKey: Uint8Array): Promise<boolean> {
  return verifyLabelled(rk.key, RECOVERY_PROOF, recoveryProofBody(account, accountKey, rk.key), rk.proof);
}

/**
 * A passkey slot as `passkeySlotSet` registers it (`docs/passkeys.md` §4.2):
 * the passkey authority key `PAK` (Ed25519, from the passkey's PRF output)
 * with `PAK`'s own proof over the account, its root `AK`, the slot, the
 * credential and the salt.
 */
export interface PasskeySlot {
  /** `"p-" + 12 random characters`, never reused. */
  slot: string;
  /** `PAK`. */
  key: Bytes;
  proof: Bytes;
  /** `SHA-256(credential id)`. */
  credHash: Bytes;
  /** The PRF salt's random part. */
  saltNonce: Bytes;
  /** What a device joining with the slot may become: must be 0 (passkeys never grant admin). */
  grants: number;
  /** Whether a device joined with the slot may activate itself after `PASSKEY_DELAY_MS`. */
  selfActivate: boolean;
}

/** What a passkey slot's proof signs: `str16 account | AK 32 | PAK 32 | str16 slot | cred_hash 32 | salt_nonce 32`. */
export function passkeyProofBody(account: string, accountKey: Uint8Array, pak: Uint8Array, slot: string, credHash: Uint8Array, saltNonce: Uint8Array): Bytes {
  return new Writer().str16(account).raw(accountKey).raw(pak).str16(slot).raw(credHash).raw(saltNonce).finish();
}

/** `PAK`'s proof (`signing` its private half) for slot `slot` of `account` with root key `accountKey`. */
export function passkeySlotProof(
  signing: CryptoKey,
  pak: Uint8Array,
  account: string,
  accountKey: Uint8Array,
  slot: string,
  credHash: Uint8Array,
  saltNonce: Uint8Array,
): Promise<Bytes> {
  return signLabelled(signing, PASSKEY_PROOF, passkeyProofBody(account, accountKey, pak, slot, credHash, saltNonce));
}

/** Whether `s.proof` is `s.key`'s signature over this account, root, slot, credential and salt. */
export function verifyPasskeySlot(s: PasskeySlot, account: string, accountKey: Uint8Array): Promise<boolean> {
  return verifyLabelled(s.key, PASSKEY_PROOF, passkeyProofBody(account, accountKey, s.key, s.slot, s.credHash, s.saltNonce), s.proof);
}

/** What the HMAC proving a host holds its new DH key covers after the key (`docs/host-identity-rotation.md` §3.2). */
export const ROTATE_CONFIRM_LABEL = utf8("rh/v1 rotate-confirm\0");
/** What the high-assurance code hashes under (§3.7). */
export const ROTATE_SAS_LABEL = utf8("rh/v1 rotate-sas\0");

/**
 * How a host's identity renewal was confirmed (Rust's `Assurance`):
 * `ASSURANCE_AUTO` (0) by the admin's channel and the hub's window,
 * `ASSURANCE_ON_DEVICE` (1) by the code the device showed, typed in on the
 * admin device. Any other byte does not decode.
 */
export type Assurance = 0 | 1;
/** The admin's request nonce in a renewal: 16 bytes, in the proof and in the entry. */
export const RENEWAL_NONCE_LEN = 16;
export const ASSURANCE_AUTO = 0;
export const ASSURANCE_ON_DEVICE = 1;

/**
 * `rotateHostKey` (chain v3, kind 20): a current host renews its identity
 * key — sign and DH — and keeps its id and everything the chain says about
 * it. An admin signs the entry; the host's current key **and** its new key
 * sign the proof (`hostRotateBody`), so neither an admin alone nor a holder of
 * the old key alone can rebind the id.
 */
export interface HostRotation {
  host: string;
  /** The new Ed25519 key. */
  signKey: Bytes;
  /** The new X25519 key. */
  dhKey: Bytes;
  /** The chain view the proof was made against: `stagedHead` is entry `stagedSeq`'s hash. */
  stagedSeq: number;
  stagedHead: Bytes;
  /** The admin's request nonce (16 bytes). */
  nonce: Bytes;
  assurance: Assurance;
  /** The host's current sign key over the proof. */
  oldSig: Bytes;
  /** The new sign key over the same proof. */
  newSig: Bytes;
}

/**
 * What both of a host's identity keys sign when it renews them (`rh/v1 host-rotate`, §3.2):
 * `str16 account | str16 host | u64 staged_seq | staged_head 32 | old_sign 32 | old_dh 32 | new_sign 32 | new_dh 32 | nonce 16 | u8 assurance`.
 */
export function hostRotateBody(
  account: string,
  host: string,
  stagedSeq: number,
  stagedHead: Uint8Array,
  oldSign: Uint8Array,
  oldDh: Uint8Array,
  newSign: Uint8Array,
  newDh: Uint8Array,
  nonce: Uint8Array,
  assurance: Assurance,
): Bytes {
  if (nonce.length !== RENEWAL_NONCE_LEN) throw new Error("a renewal nonce is 16 bytes");
  return new Writer().str16(account).str16(host).u64(stagedSeq).raw(stagedHead).raw(oldSign).raw(oldDh).raw(newSign).raw(newDh).raw(nonce).u8(assurance).finish();
}

/** `r`'s proof (Rust's `HostRotation::body`), for a host whose keys on the chain are `oldSign` / `oldDh`. */
export function hostRotationBody(account: string, r: Omit<HostRotation, "oldSig" | "newSig">, oldSign: Uint8Array, oldDh: Uint8Array): Bytes {
  return hostRotateBody(account, r.host, r.stagedSeq, r.stagedHead, oldSign, oldDh, r.signKey, r.dhKey, r.nonce, r.assurance);
}

/** One identity key's signature over a renewal's proof (`key`: the current one for `oldSig`, the new one for `newSig`). */
export function signHostRotate(key: CryptoKey, body: Uint8Array): Promise<Bytes> {
  return signLabelled(key, HOST_ROTATE, body);
}

/** Whether `r.oldSig` is `oldSign`'s and `r.newSig` the new key's signature over its proof (rule 5, `RULES.rotateProof`). */
export async function verifyHostRotation(account: string, r: HostRotation, oldSign: Uint8Array, oldDh: Uint8Array): Promise<boolean> {
  const body = hostRotationBody(account, r, oldSign, oldDh);
  return (await verifyLabelled(oldSign, HOST_ROTATE, body, r.oldSig)) && (await verifyLabelled(r.signKey, HOST_ROTATE, body, r.newSig));
}

function hmacKey(shared: Uint8Array, usage: "sign" | "verify"): Promise<CryptoKey> {
  return crypto.subtle.importKey("raw", copy(shared), { name: "HMAC", hash: "SHA-256" }, false, [usage]);
}

/**
 * `HMAC-SHA256(shared, "rh/v1 rotate-confirm\0" ‖ body)`: the proof that the
 * host holds its new DH key, which no signature shows (§3.2). `shared` is
 * `X25519(new_dh, eph)` from either side (`rotateConfirmFor`, `verifyRotateConfirm`).
 */
export async function rotateConfirmMac(shared: Uint8Array, body: Uint8Array): Promise<Bytes> {
  return new Uint8Array(await crypto.subtle.sign("HMAC", await hmacKey(shared, "sign"), concat(ROTATE_CONFIRM_LABEL, body)));
}

/** The host's side of `rotateConfirmMac`: its new DH private key with the admin's ephemeral public key. */
export async function rotateConfirmFor(newDhPrivate: CryptoKey, eph: Uint8Array, body: Uint8Array): Promise<Bytes> {
  return rotateConfirmMac(await dh(newDhPrivate, eph), body);
}

/** The admin's side: whether `mac` is `rotateConfirmMac` under its ephemeral private key and the host's new DH key. */
export async function verifyRotateConfirm(ephPrivate: CryptoKey, newDh: Uint8Array, body: Uint8Array, mac: Uint8Array): Promise<boolean> {
  let shared: Bytes;
  try {
    shared = await dh(ephPrivate, newDh);
  } catch (error) {
    if (error instanceof LowOrderKeyError) return false;
    throw error;
  }
  return crypto.subtle.verify("HMAC", await hmacKey(shared, "verify"), copy(mac), concat(ROTATE_CONFIRM_LABEL, body));
}

/**
 * The code a host shows for a high-assurance renewal (§3.7): RFC 4648 base32
 * (upper case, no padding) of `SHA-256("rh/v1 rotate-sas\0" ‖ body)[..5]` —
 * 40 bits, 8 characters — written `XXXX-XXXX`.
 */
export async function rotateSas(body: Uint8Array): Promise<string> {
  const text = base32((await sha256(ROTATE_SAS_LABEL, body)).slice(0, 5));
  return `${text.slice(0, 4)}-${text.slice(4)}`;
}

/**
 * What a host's new key signs to confirm a staged renewal (`rh/v1 rotate-ack`, §3.3):
 * `str16 account | str16 host | SHA-256(proof body) 32 | login nonce 32` — the hub's challenge that connection answered.
 */
export async function rotateAckBody(account: string, host: string, proofBody: Uint8Array, loginNonce: Uint8Array): Promise<Bytes> {
  if (loginNonce.length !== 32) throw new Error("a hub nonce is 32 bytes");
  return new Writer().str16(account).str16(host).raw(await sha256(proofBody)).raw(loginNonce).finish();
}

/** The new key's `rotateAckBody` signature. */
export async function signRotateAck(newKey: CryptoKey, account: string, host: string, proofBody: Uint8Array, loginNonce: Uint8Array): Promise<Bytes> {
  return signLabelled(newKey, ROTATE_ACK, await rotateAckBody(account, host, proofBody, loginNonce));
}

/**
 * What a host's retired sign key signs to dispute the renewal that retired it (`rh/v1 rotate-dispute`, §3.8
 * 「证据：退役钥签的争议声明」): `str16 account | str16 host | u64 seq | entry_hash 32 | new_sign 32` — `seq` the disputed
 * `rotateHostKey`, `entryHash` its chain hash (`entryHash`), `newSign` the sign key it put in. Bound to the entry's
 * hash, a statement can only be signed once that entry exists: no clocks to compare.
 */
export function rotateDisputeBody(account: string, host: string, seq: number, entryHash: Uint8Array, newSign: Uint8Array): Bytes {
  if (entryHash.length !== 32 || newSign.length !== 32) throw new Error("a dispute names a 32-byte entry hash and sign key");
  return new Writer().str16(account).str16(host).u64(seq).raw(entryHash).raw(newSign).finish();
}

/** The retired key's `rotateDisputeBody` signature (`key`: the sign key the renewal at `seq` retired, `keysAt(host, seq - 1)`). */
export function signRotateDispute(key: CryptoKey, account: string, host: string, seq: number, entryHash: Uint8Array, newSign: Uint8Array): Promise<Bytes> {
  return signLabelled(key, ROTATE_DISPUTE, rotateDisputeBody(account, host, seq, entryHash, newSign));
}

/** Whether `signature` is `newSign`'s `rotateAckBody` signature. */
export async function verifyRotateAck(newSign: Uint8Array, account: string, host: string, proofBody: Uint8Array, loginNonce: Uint8Array, signature: Uint8Array): Promise<boolean> {
  return verifyLabelled(newSign, ROTATE_ACK, await rotateAckBody(account, host, proofBody, loginNonce), signature);
}

/** One key epoch committed in the chain: the epoch and its key's hash (`userKeyCommit`, `machineKeyCommit`). */
export interface KeyCommit {
  epoch: number;
  hash: Bytes;
}

export interface MachineKeyCommit extends KeyCommit {
  machine: string;
}

function keyCommit(kind: number, account: string, id: string, epoch: number, key: Uint8Array): Promise<Bytes> {
  return sha256(KEY_COMMIT_LABEL, new Writer().u8(kind).str16(account).str16(id).u32(epoch).finish(), key);
}

/** The commitment to user key epoch `epoch`: `SHA-256("rh/v1 key-commit\0" | 1 | str16 account | str16 "" | u32 epoch | key)`. */
export function userKeyCommit(account: string, epoch: number, key: Uint8Array): Promise<Bytes> {
  return keyCommit(1, account, "", epoch, key);
}

/** The commitment to host `machine`'s key epoch `epoch` (kind 2, id = the host). */
export function machineKeyCommit(account: string, machine: string, epoch: number, key: Uint8Array): Promise<Bytes> {
  return keyCommit(2, account, machine, epoch, key);
}

/** A device as a **v2** entry names it; `full` is an admin in the v3 state. */
export interface DeviceInfo {
  id: string;
  signKey: Bytes;
  dhKey: Bytes;
  full: boolean;
}

/** A machine as a **v2** `addMachine` names it: a host (not an admin) in the v3 state. */
export interface MachineInfo {
  id: string;
  signKey: Bytes;
  dhKey: Bytes;
}

/** A device's identity in the state: its id and its two public keys. */
export interface DeviceIdentity {
  id: string;
  /** Ed25519: signs chain entries, attestations, stream heads (a host). */
  signKey: Bytes;
  /** X25519: the Noise static key, and what keys are sealed to and from. */
  dhKey: Bytes;
}

/** A device's two flags (a `DeviceV3`'s last byte: bit 0 host, bit 1 admin; every other bit must be 0). */
export interface Flags {
  /** 可运行 AI: runs an agent, can be controlled, has machine keys. */
  host: boolean;
  /** 可管理账号: signs entries, holds `UK`, attests. */
  admin: boolean;
}

export const FLAG_HOST = 0b01;
export const FLAG_ADMIN = 0b10;

export function flagsToByte(f: Flags): number {
  return (f.host ? FLAG_HOST : 0) | (f.admin ? FLAG_ADMIN : 0);
}

/** Unknown bits are refused: one byte, one meaning. */
export function flagsFromByte(b: number): Flags {
  if ((b & ~(FLAG_HOST | FLAG_ADMIN)) !== 0) throw new DecodeError("invalid device flags");
  return { host: (b & FLAG_HOST) !== 0, admin: (b & FLAG_ADMIN) !== 0 };
}

/** A device as a **v3** entry names it. */
export interface DeviceV3 {
  id: string;
  signKey: Bytes;
  dhKey: Bytes;
  flags: Flags;
}

/** Whom a host accepts control from (`setAccept`) besides admin devices, which it always accepts. */
export type Accept =
  /** Admin devices only: the default. */
  | { type: "admins" }
  /** Every current device of the account. */
  | { type: "all" }
  /** Admin devices and these (ascending by bytes, no repeats, 1–`MAX_ACCEPT_IDS`). */
  | { type: "devices"; ids: string[] };

export type Action =
  // ---- v2 only
  | { type: "genesis"; accountKey: Bytes; device: DeviceInfo; recoveryKey: RecoveryKey | null }
  | { type: "addDevice"; device: DeviceInfo }
  | { type: "removeDevice"; id: string }
  | { type: "addMachine"; machine: MachineInfo }
  | { type: "removeMachine"; id: string }
  | { type: "recoveryJoin"; device: DeviceInfo }
  // ---- either version (the draft's version says which)
  | { type: "approveDevice"; id: string }
  | { type: "activateRecovered"; id: string }
  | { type: "setRecoveryKey"; key: RecoveryKey | null }
  /** A new user key epoch and/or new machine key epochs, each one past the last committed. */
  | { type: "keyEpochs"; user: KeyCommit | null; machines: MachineKeyCommit[] }
  // ---- v3 only
  /** Kind 6 in a v3 entry: the recovered device must be an admin, and may be a host. */
  | { type: "recoveryJoin3"; device: DeviceV3 }
  | { type: "genesis3"; accountKey: Bytes; device: DeviceV3; recoveryKey: RecoveryKey | null }
  | { type: "addDevice3"; device: DeviceV3 }
  | { type: "setAdmin"; id: string; admin: boolean }
  | { type: "setHost"; id: string; host: boolean }
  | { type: "removeDevice3"; id: string }
  | { type: "passkeySlotSet"; slot: PasskeySlot }
  | { type: "passkeySlotRemove"; slot: string }
  | { type: "passkeyActivate"; id: string }
  | { type: "setAccept"; host: string; from: Accept }
  /** Kind 20: a current host renews its identity key (`docs/host-identity-rotation.md` §3.4). */
  | { type: "rotateHostKey"; rotation: HostRotation }
  /** Kind 21: the account's renewal policy (§3.4 rule 9) — while on, every renewal must be confirmed on the device. Byte 0 or 1. */
  | { type: "setRenewalPolicy"; alwaysAssured: boolean }
  /**
   * Kind 22: control paused over hosts while identity keys are renewed (§3.4 rule 10) — an overlay, not an edit: a start
   * names, per host, the devices it stops accepting for now; while the pause is open, the host's effective accept is its
   * `setAccept` base minus those (`mayControl`, so `authorizeControl`; `pausedEdges`); a completion or cancellation lifts
   * it. A `setAccept` only ever changes the base. The whole entry fits every transport (`MAX_PAUSE_ENTRY_LEN`).
   */
  | { type: "renewalPause"; op: Bytes; phase: PausePhase; removals: PauseRemoval[] };

/**
 * One host and the ids it stops accepting while a renewal pause is open (Rust's `(host, ids)`): never a current admin;
 * hosts, and each host's ids, in byte order.
 */
export interface PauseRemoval {
  host: string;
  ids: string[];
}

/** What a `renewalPause` marker says: `PAUSE_START` (0), `PAUSE_COMPLETED` (1), `PAUSE_CANCELLED` (2); any other byte does not decode. */
export type PausePhase = 0 | 1 | 2;
export const PAUSE_START = 0;
export const PAUSE_COMPLETED = 1;
export const PAUSE_CANCELLED = 2;
/** The most hosts one renewal pause may name. */
export const MAX_PAUSE_HOSTS = 256;
/** The most edges (host, id) one renewal pause may remove. */
export const MAX_PAUSE_EDGES = 1024;
/**
 * The most bytes a renewal pause entry may take, encoded with its signature (§3.4 rule 10, 大小上限; Rust's
 * `MAX_PAUSE_ENTRY_LEN`, the same number): what every transport that carries chain entries holds — `MAX_HELLO_ENTRIES`
 * of them, each behind its u16 length, fit a hello beside the most client info and a reply beside the most server info,
 * and one fits a `chain` frame. A larger one is refused (`RULES.pauseTooLarge`): a cycle that needs more is split into
 * several pauses (`encodeEntry(entry).length`, or `encodeDraft(draft).length + 64`, is what counts).
 */
export const MAX_PAUSE_ENTRY_LEN = 4000;

/** A renewal pause as the chain has it (Rust's `PauseRecord`). */
export interface PauseRecord {
  /** Each host and the ids it stops accepting while the pause is open. */
  removals: PauseRemoval[];
  startSeq: number;
  /** The admin that started it (it stays in force if that admin is later demoted or removed). */
  signer: string;
  /** How and where it ended, once it did. */
  end: { phase: PausePhase; seq: number } | null;
}

/** A renewal pause still open (Rust's `OpenPause`). */
export interface OpenPause {
  op: Bytes;
  startSeq: number;
  signer: string;
  removals: PauseRemoval[];
}

const copyRemovals = (removals: PauseRemoval[]): PauseRemoval[] => removals.map((r) => ({ host: r.host, ids: [...r.ids] }));

/** `passkey` (v3 only): a passkey slot's `PAK`, for its own pending join and activation. */
export type Signer = { type: "account" } | { type: "device"; id: string } | { type: "recovery" } | { type: "passkey"; slot: string };

export interface Draft {
  /** `CHAIN_VERSION` (2) or `CHAIN_VERSION_3`; covered by the signature. */
  version: number;
  account: string;
  seq: number;
  prev: Bytes;
  createdMs: number;
  signer: Signer;
  action: Action;
}

export interface Entry {
  draft: Draft;
  signature: Bytes;
}

/** The names Rust's `vectors::chain_error_kind` gives every `ChainError`. */
export type ChainErrorKind =
  | "decode"
  | "version"
  | "badSignature"
  | "wrongAccount"
  | "outOfOrder"
  | "brokenLink"
  | "notGenesis"
  | "lateGenesis"
  | "pinMismatch"
  | "genesisMismatch"
  | "notAllowed"
  | "duplicateId"
  | "unknownId"
  | "lastFullDevice"
  | "lastAdmin"
  | "unknownSlot"
  | "weakKey"
  | "badId"
  | "tooEarly"
  | "rollback"
  | "fork"
  | "behind"
  | "unknownDevice"
  | "badAttestation"
  | "uncommittedKey"
  | "keyMismatch"
  | "notAccepted"
  | "selfControl";

/**
 * The reasons of `notAllowed` that chain v3 added, by rule — Rust's
 * `chain::rules`, word for word: the interop vectors name them, so both halves
 * refuse for the same rule.
 */
export const RULES = {
  /** Review D1: a device that runs AI does not manage the account (its agent never attests nor signs). */
  hostNotAdmin: "a device that runs AI cannot manage the account",
  /** D1 extended (re-review): nor one that ran AI and stopped — its agent kept its key in a file. */
  ranAiNotAdmin: "a device that has run AI cannot manage the account",
  selfAdmin: "a device cannot make itself an admin",
  selfHost: "a device cannot make itself a host",
  noChange: "the device already is what this entry says",
  hostKeyNext: "a new host's machine key epoch must come next",
  onlyAdminSigns: "only an admin device may sign",
  recoveredAdmin: "a recovered device is an admin device",
  passkeyPlain: "a passkey adds only a device that neither runs AI nor manages the account",
  slotGrants: "a passkey slot grants nothing: its devices neither run AI nor manage the account",
  v2AfterV3: "a v2 entry after the chain moved to v3",
  acceptSelf: "a host does not control itself",
  acceptSame: "the host already accepts exactly that",
  acceptNotHost: "only a host accepts control",
  /** `rotateHostKey` (`docs/host-identity-rotation.md` §3.4), rule 2: only a current, active host renews its identity key. */
  rotateNotHost: "only a current device that runs AI renews its identity key",
  /** Rule 3: neither new key is one any device of the chain holds or held (so no host's identity lands on another device's key). */
  rotateKeyReused: "a renewed identity key must be one the chain never named",
  /** Rule 4: the proof names an entry before this one, by its hash. */
  rotateStaged: "the renewal was proved against another chain",
  /** Rule 5: the host's current key and its new key both signed the proof. */
  rotateProof: "the renewal is not signed by both the old and the new identity key",
  /** A key a renewal retired never names a device again — not added, not joining, no pending device holding it approved or activated (a key a current device holds is not refused: chains from before may name one twice). */
  keyRetired: "a retired identity key never comes back",
  /** `setRenewalPolicy` (§3.4 rule 9): while the account's policy is on, a renewal not confirmed on the device is refused. */
  rotateNeedsAssurance: "the account asks for every renewal to be confirmed on the device",
  /** `renewalPause` (§3.4 rule 10): a start names an op the chain never used. */
  pauseOpReused: "a renewal pause starts under an op the chain never used",
  /** A start names current hosts, at least one and at most `MAX_PAUSE_HOSTS`, each once, in byte order (one list, one encoding). */
  pauseHosts: "a renewal pause names current hosts, at least one, each once, in byte order",
  /** A start removes at most `MAX_PAUSE_EDGES` edges in all. */
  pauseEdges: "a renewal pause removes too many edges",
  /** A start, encoded with its signature, takes at most `MAX_PAUSE_ENTRY_LEN` bytes, so every transport carries it. */
  pauseTooLarge: "a renewal pause is too large for every transport to carry",
  /**
   * From each host, a start removes devices the chain named, at least one, each once, in byte order — never the host
   * itself, nor a current admin (D1: an admin's control does not go by any list; pausing it is not needed, and would
   * escape the exposure check).
   */
  pauseIds: "a renewal pause removes, from each host, devices the chain named, at least one, each once, in byte order, never the host itself nor an admin",
  /** A completion or cancellation ends an open pause, once. */
  pauseNotOpen: "only an open renewal pause ends, and only once",
  /** A completion or cancellation names no removals. */
  pauseEndRemovals: "a renewal pause ends without naming removals",
} as const;

export class ChainError extends Error {
  constructor(
    readonly kind: ChainErrorKind,
    message: string,
    readonly detail: { seq?: number; readyMs?: number; id?: string; epoch?: number; slot?: string; device?: string; host?: string; reason?: string } = {},
  ) {
    super(message);
    this.name = "ChainError";
  }
}

const KIND: Record<Action["type"], number> = {
  genesis: 1,
  addDevice: 2,
  removeDevice: 3,
  addMachine: 4,
  removeMachine: 5,
  recoveryJoin: 6,
  recoveryJoin3: 6,
  approveDevice: 7,
  activateRecovered: 8,
  setRecoveryKey: 9,
  keyEpochs: 10,
  genesis3: 11,
  addDevice3: 12,
  setAdmin: 13,
  setHost: 14,
  removeDevice3: 15,
  passkeySlotSet: 16,
  passkeySlotRemove: 17,
  passkeyActivate: 18,
  setAccept: 19,
  rotateHostKey: 20,
  setRenewalPolicy: 21,
  renewalPause: 22,
};

/**
 * The one version `action` exists in, or `null` for the four both versions
 * share (`approveDevice`, `activateRecovered`, `setRecoveryKey`,
 * `keyEpochs`: the draft's version byte decides those).
 */
export function fixedVersion(action: Action): number | null {
  switch (action.type) {
    case "genesis":
    case "addDevice":
    case "removeDevice":
    case "addMachine":
    case "removeMachine":
    case "recoveryJoin":
      return CHAIN_VERSION;
    case "approveDevice":
    case "activateRecovered":
    case "setRecoveryKey":
    case "keyEpochs":
      return null;
    default:
      return CHAIN_VERSION_3;
  }
}

function putDevice(w: Writer, d: DeviceInfo): void {
  w.str16(d.id).raw(d.signKey).raw(d.dhKey).u8(d.full ? 1 : 0);
}

function getDevice(r: Reader): DeviceInfo {
  const id = r.str16("device id", MAX_ID_LEN);
  const signKey = r.bytes(32);
  const dhKey = r.bytes(32);
  const flags = r.u8();
  if (flags > 1) throw new DecodeError("invalid device flags");
  return { id, signKey, dhKey, full: flags === 1 };
}

/** `str16 id | sign_key 32 | dh_key 32 | u8 flags`. */
function putDevice3(w: Writer, d: DeviceV3): void {
  w.str16(d.id).raw(d.signKey).raw(d.dhKey).u8(flagsToByte(d.flags));
}

function getDevice3(r: Reader): DeviceV3 {
  return { id: r.str16("device id", MAX_ID_LEN), signKey: r.bytes(32), dhKey: r.bytes(32), flags: flagsFromByte(r.u8()) };
}

/** `str16 host | sign_key 32 | dh_key 32 | u64 staged_seq | staged_head 32 | nonce 16 | u8 assurance (0 or 1) | old_sig 64 | new_sig 64`. */
function putRotation(w: Writer, r: HostRotation): void {
  if (r.nonce.length !== RENEWAL_NONCE_LEN) throw new Error("a renewal nonce is 16 bytes");
  w.str16(r.host).raw(r.signKey).raw(r.dhKey).u64(r.stagedSeq).raw(r.stagedHead).raw(r.nonce).u8(r.assurance).raw(r.oldSig).raw(r.newSig);
}

function getRotation(r: Reader): HostRotation {
  const host = r.str16("device id", MAX_ID_LEN);
  const signKey = r.bytes(32);
  const dhKey = r.bytes(32);
  const stagedSeq = r.u64();
  const stagedHead = r.bytes(32);
  const nonce = r.bytes(16);
  const assurance = r.u8();
  if (assurance !== ASSURANCE_AUTO && assurance !== ASSURANCE_ON_DEVICE) throw new DecodeError("invalid renewal assurance");
  return { host, signKey, dhKey, stagedSeq, stagedHead, nonce, assurance, oldSig: r.bytes(64), newSig: r.bytes(64) };
}

/**
 * `op 16 | u8 phase (0 start, 1 completed, 2 cancelled) | u16 n | (str16 host | u16 m | str16 id × m) × n`: at most
 * `MAX_PAUSE_HOSTS` hosts and `MAX_PAUSE_EDGES` ids in all.
 */
function getPause(r: Reader): Action {
  const op = r.bytes(16);
  const phase = r.u8();
  if (phase !== PAUSE_START && phase !== PAUSE_COMPLETED && phase !== PAUSE_CANCELLED) throw new DecodeError("invalid renewal pause phase");
  const n = r.u16();
  if (n > MAX_PAUSE_HOSTS) throw new DecodeError(`renewal pause hosts too large: ${n} > ${MAX_PAUSE_HOSTS}`);
  const removals: PauseRemoval[] = [];
  let edges = 0;
  for (let i = 0; i < n; i++) {
    const host = r.str16("device id", MAX_ID_LEN);
    const m = r.u16();
    edges += m;
    if (edges > MAX_PAUSE_EDGES) throw new DecodeError(`renewal pause edges too large: ${edges} > ${MAX_PAUSE_EDGES}`);
    const ids: string[] = [];
    for (let j = 0; j < m; j++) ids.push(r.str16("device id", MAX_ID_LEN));
    removals.push({ host, ids });
  }
  return { type: "renewalPause", op, phase, removals };
}

function getBool(r: Reader, what: string): boolean {
  const b = r.u8();
  if (b === 0) return false;
  if (b === 1) return true;
  throw new DecodeError(`invalid ${what}`);
}

/** `str16 slot | PAK 32 | proof 64 | cred_hash 32 | salt_nonce 32 | u8 grants | u8 self_activate`. */
function putSlot(w: Writer, s: PasskeySlot): void {
  w.str16(s.slot).raw(s.key).raw(s.proof).raw(s.credHash).raw(s.saltNonce).u8(s.grants).u8(s.selfActivate ? 1 : 0);
}

function getSlot(r: Reader): PasskeySlot {
  return {
    slot: r.str16("passkey slot", MAX_ID_LEN),
    key: r.bytes(32),
    proof: r.bytes(64),
    credHash: r.bytes(32),
    saltNonce: r.bytes(32),
    grants: r.u8(),
    selfActivate: getBool(r, "self-activate flag"),
  };
}

/** Whether `items` are strictly ascending by bytes: each once, in the one order a renewal pause's removals take (§3.4 rule 10). */
function ascending(items: string[]): boolean {
  for (let i = 1; i < items.length; i++) if (compareUtf8(items[i - 1]!, items[i]!) >= 0) return false;
  return true;
}

/** Byte order of two strings' UTF-8 (what Rust's `as_bytes()` comparison is). */
function compareUtf8(a: string, b: string): number {
  const x = utf8(a);
  const y = utf8(b);
  for (let i = 0; i < Math.min(x.length, y.length); i++) if (x[i] !== y[i]) return x[i]! - y[i]!;
  return x.length - y.length;
}

/** `u8 kind (0 admins, 1 all, 2 list) [| u16 n | str16 id × n]`, the list non-empty, at most `MAX_ACCEPT_IDS`, strictly ascending. */
function putAccept(w: Writer, from: Accept): void {
  if (from.type === "admins") w.u8(0);
  else if (from.type === "all") w.u8(1);
  else {
    w.u8(2).u16(from.ids.length);
    for (const id of from.ids) w.str16(id);
  }
}

function getAccept(r: Reader): Accept {
  const kind = r.u8();
  if (kind === 0) return { type: "admins" };
  if (kind === 1) return { type: "all" };
  if (kind !== 2) throw new DecodeError(`unknown accept policy ${kind}`);
  const n = r.u16();
  if (n === 0) throw new DecodeError("invalid: an empty accept list");
  if (n > MAX_ACCEPT_IDS) throw new DecodeError(`accept list too large: ${n} > ${MAX_ACCEPT_IDS}`);
  const ids: string[] = [];
  for (let i = 0; i < n; i++) {
    const id = r.str16("device id", MAX_ID_LEN);
    if (ids.length && compareUtf8(ids[ids.length - 1]!, id) >= 0) throw new DecodeError("invalid accept list order");
    ids.push(id);
  }
  return { type: "devices", ids };
}

function sameAccept(a: Accept, b: Accept): boolean {
  if (a.type !== b.type) return false;
  if (a.type !== "devices" || b.type !== "devices") return true;
  return a.ids.length === b.ids.length && a.ids.every((id, i) => id === b.ids[i]);
}

/** `u8 hasUser [u32 epoch | hash 32] | u16 n | n × (str16 machine | u32 epoch | hash 32)`. */
function putKeyCommits(w: Writer, user: KeyCommit | null, machines: MachineKeyCommit[]): void {
  if (user) w.u8(1).u32(user.epoch).raw(user.hash);
  else w.u8(0);
  w.u16(machines.length);
  for (const m of machines) w.str16(m.machine).u32(m.epoch).raw(m.hash);
}

function getKeyCommits(r: Reader): { user: KeyCommit | null; machines: MachineKeyCommit[] } {
  const flag = r.u8();
  let user: KeyCommit | null = null;
  if (flag === 1) user = { epoch: r.u32(), hash: r.bytes(32) };
  else if (flag !== 0) throw new DecodeError("user key presence flag");
  const n = r.u16();
  if (n > MAX_KEY_COMMITS) throw new DecodeError(`machine key commits too large: ${n} > ${MAX_KEY_COMMITS}`);
  const machines: MachineKeyCommit[] = [];
  for (let i = 0; i < n; i++) machines.push({ machine: r.str16("machine id", MAX_ID_LEN), epoch: r.u32(), hash: r.bytes(32) });
  return { user, machines };
}

/** `u8 flag (0/1) [key 32 | proof 64]`. */
function putRecoveryKey(w: Writer, key: RecoveryKey | null): void {
  if (key) w.u8(1).raw(key.key).raw(key.proof);
  else w.u8(0);
}

function getRecoveryKey(r: Reader): RecoveryKey | null {
  const flag = r.u8();
  if (flag === 0) return null;
  if (flag === 1) return { key: r.bytes(32), proof: r.bytes(64) };
  throw new DecodeError("key presence flag");
}

/** The bytes that are signed (after the label) and that, with the signature, are the entry. */
export function encodeDraft(d: Draft): Bytes {
  const w = new Writer().u8(d.version).str16(d.account).u64(d.seq).raw(d.prev).u64(d.createdMs);
  switch (d.signer.type) {
    case "account":
      w.u8(0);
      break;
    case "device":
      w.u8(1).str16(d.signer.id);
      break;
    case "recovery":
      w.u8(2);
      break;
    case "passkey":
      w.u8(3).str16(d.signer.slot);
      break;
  }
  const a = d.action;
  w.u8(KIND[a.type]);
  switch (a.type) {
    case "genesis":
      w.raw(a.accountKey);
      putDevice(w, a.device);
      putRecoveryKey(w, a.recoveryKey);
      break;
    case "genesis3":
      w.raw(a.accountKey);
      putDevice3(w, a.device);
      putRecoveryKey(w, a.recoveryKey);
      break;
    case "addDevice":
    case "recoveryJoin":
      putDevice(w, a.device);
      break;
    case "addDevice3":
    case "recoveryJoin3":
      putDevice3(w, a.device);
      break;
    case "removeDevice":
    case "removeMachine":
    case "approveDevice":
    case "activateRecovered":
    case "removeDevice3":
    case "passkeyActivate":
      w.str16(a.id);
      break;
    case "passkeySlotRemove":
      w.str16(a.slot);
      break;
    case "addMachine":
      w.str16(a.machine.id).raw(a.machine.signKey).raw(a.machine.dhKey);
      break;
    case "setRecoveryKey":
      putRecoveryKey(w, a.key);
      break;
    case "keyEpochs":
      putKeyCommits(w, a.user, a.machines);
      break;
    case "setAdmin":
      w.str16(a.id).u8(a.admin ? 1 : 0);
      break;
    case "setHost":
      w.str16(a.id).u8(a.host ? 1 : 0);
      break;
    case "passkeySlotSet":
      putSlot(w, a.slot);
      break;
    case "setAccept":
      w.str16(a.host);
      putAccept(w, a.from);
      break;
    case "rotateHostKey":
      putRotation(w, a.rotation);
      break;
    case "setRenewalPolicy":
      w.u8(a.alwaysAssured ? 1 : 0);
      break;
    case "renewalPause":
      if (a.op.length !== 16) throw new Error("a renewal pause op is 16 bytes");
      w.raw(a.op).u8(a.phase).u16(a.removals.length);
      for (const { host, ids } of a.removals) {
        w.str16(host).u16(ids.length);
        for (const id of ids) w.str16(id);
      }
      break;
  }
  return w.finish();
}

export function encodeEntry(e: Entry): Bytes {
  return concat(encodeDraft(e.draft), e.signature);
}

/**
 * Decode one entry. The version byte decides which actions and signers exist:
 * a v3 action (or a passkey signer) in a v2 entry, or a v2-only action in a
 * v3 entry, does not decode — one byte string, one reading.
 */
export function decodeEntry(data: Uint8Array): Entry {
  try {
    const r = new Reader(data);
    const version = r.u8();
    if (version !== CHAIN_VERSION && version !== CHAIN_VERSION_3) throw new ChainError("version", `unsupported chain entry version ${version}`);
    const v3 = version === CHAIN_VERSION_3;
    const account = r.str16("account id", MAX_ID_LEN);
    const seq = r.u64();
    const prev = r.bytes(32);
    const createdMs = r.u64();
    const signerTag = r.u8();
    let signer: Signer;
    if (signerTag === 0) signer = { type: "account" };
    else if (signerTag === 1) signer = { type: "device", id: r.str16("signer id", MAX_ID_LEN) };
    else if (signerTag === 2) signer = { type: "recovery" };
    else if (signerTag === 3 && v3) signer = { type: "passkey", slot: r.str16("passkey slot", MAX_ID_LEN) };
    else throw new DecodeError(`unknown signer ${signerTag}`);
    const id = (what: string) => r.str16(what, MAX_ID_LEN);
    const kind = r.u8();
    let action: Action;
    switch (v3 ? `3:${kind}` : `2:${kind}`) {
      case "2:1":
        action = { type: "genesis", accountKey: r.bytes(32), device: getDevice(r), recoveryKey: getRecoveryKey(r) };
        break;
      case "2:2":
        action = { type: "addDevice", device: getDevice(r) };
        break;
      case "2:3":
        action = { type: "removeDevice", id: id("device id") };
        break;
      case "2:4":
        action = { type: "addMachine", machine: { id: id("machine id"), signKey: r.bytes(32), dhKey: r.bytes(32) } };
        break;
      case "2:5":
        action = { type: "removeMachine", id: id("machine id") };
        break;
      case "2:6":
        action = { type: "recoveryJoin", device: getDevice(r) };
        break;
      case "3:6":
        action = { type: "recoveryJoin3", device: getDevice3(r) };
        break;
      case "2:7":
      case "3:7":
        action = { type: "approveDevice", id: id("device id") };
        break;
      case "2:8":
      case "3:8":
        action = { type: "activateRecovered", id: id("device id") };
        break;
      case "2:9":
      case "3:9":
        action = { type: "setRecoveryKey", key: getRecoveryKey(r) };
        break;
      case "2:10":
      case "3:10":
        action = { type: "keyEpochs", ...getKeyCommits(r) };
        break;
      case "3:11":
        action = { type: "genesis3", accountKey: r.bytes(32), device: getDevice3(r), recoveryKey: getRecoveryKey(r) };
        break;
      case "3:12":
        action = { type: "addDevice3", device: getDevice3(r) };
        break;
      case "3:13":
        action = { type: "setAdmin", id: id("device id"), admin: getBool(r, "admin flag") };
        break;
      case "3:14":
        action = { type: "setHost", id: id("device id"), host: getBool(r, "host flag") };
        break;
      case "3:15":
        action = { type: "removeDevice3", id: id("device id") };
        break;
      case "3:16":
        action = { type: "passkeySlotSet", slot: getSlot(r) };
        break;
      case "3:17":
        action = { type: "passkeySlotRemove", slot: id("passkey slot") };
        break;
      case "3:18":
        action = { type: "passkeyActivate", id: id("device id") };
        break;
      case "3:19":
        action = { type: "setAccept", host: id("device id"), from: getAccept(r) };
        break;
      case "3:20":
        action = { type: "rotateHostKey", rotation: getRotation(r) };
        break;
      case "3:21":
        action = { type: "setRenewalPolicy", alwaysAssured: getBool(r, "renewal policy") };
        break;
      case "3:22":
        action = getPause(r);
        break;
      default:
        throw new DecodeError(`unknown chain action ${kind}`);
    }
    const signature = r.bytes(64);
    r.finish();
    return { draft: { version, account, seq, prev, createdMs, signer, action }, signature };
  } catch (error) {
    if (error instanceof DecodeError) throw new ChainError("decode", error.message);
    throw error;
  }
}

export async function signDraft(draft: Draft, key: CryptoKey): Promise<Entry> {
  return { draft, signature: await signLabelled(key, CHAIN_ENTRY, encodeDraft(draft)) };
}

export async function entryHash(e: Entry): Promise<Bytes> {
  return sha256(HASH_LABEL, encodeEntry(e));
}

/** Whether `e` is a v3 entry. */
export function isV3Entry(e: Entry): boolean {
  return e.draft.version === CHAIN_VERSION_3;
}

function verifyWith(e: Entry, key: Uint8Array): Promise<boolean> {
  return verifyLabelled(key, CHAIN_ENTRY, encodeDraft(e.draft), e.signature);
}

function validId(id: string): boolean {
  const b = utf8(id);
  return b.length > 0 && b.length <= MAX_ID_LEN && b.every((c) => c >= 0x21 && c <= 0x7e);
}

function copyRecoveryKey(k: RecoveryKey | null): RecoveryKey | null {
  return k ? { key: copy(k.key), proof: copy(k.proof) } : null;
}

/** The genesis draft for a new v2 account; the creating device signs it with `AK` and then forgets `AK`. */
export function genesisDraft(account: string, accountKey: Uint8Array, device: DeviceInfo, recoveryKey: RecoveryKey | null, createdMs: number): Draft {
  return {
    version: CHAIN_VERSION,
    account,
    seq: 0,
    prev: bytes(32),
    createdMs,
    signer: { type: "account" },
    action: { type: "genesis", accountKey: copy(accountKey), device, recoveryKey: copyRecoveryKey(recoveryKey) },
  };
}

/** The genesis draft for a new v3 account (its device must be an admin). */
export function genesis3Draft(account: string, accountKey: Uint8Array, device: DeviceV3, recoveryKey: RecoveryKey | null, createdMs: number): Draft {
  return {
    version: CHAIN_VERSION_3,
    account,
    seq: 0,
    prev: bytes(32),
    createdMs,
    signer: { type: "account" },
    action: { type: "genesis3", accountKey: copy(accountKey), device, recoveryKey: copyRecoveryKey(recoveryKey) },
  };
}

/** `RSK` (signs recovery entries) and the recovery package's wrap key, from the user's `RK`. */
export interface RecoveryKeys {
  signing: CryptoKey;
  signingPublic: Bytes;
  wrap: CryptoKey;
  /** The X25519 twin of `RSK`, which the recovery package of `UK` epochs is sealed to (see `wrap.ts`). */
  seal: { privateKey: CryptoKey; publicKey: Bytes };
}

async function hkdf32(ikm: CryptoKey, salt: Uint8Array, info: Uint8Array): Promise<Bytes> {
  return new Uint8Array(await crypto.subtle.deriveBits({ name: "HKDF", hash: "SHA-256", salt: copy(salt), info: copy(info) }, ikm, 256));
}

export async function recoveryKeys(rk: Uint8Array): Promise<RecoveryKeys> {
  if (rk.length < 16) throw new Error("a recovery secret is at least 128 bits");
  const ikm = await crypto.subtle.importKey("raw", copy(rk), "HKDF", false, ["deriveBits"]);
  const salt = utf8("rh/v1 recovery");
  const seed = await hkdf32(ikm, salt, utf8("signing key"));
  const wrapRaw = await hkdf32(ikm, salt, utf8("wrap key"));
  // As in Rust: Ed25519's scalar is clamp(SHA-512(seed)[..32]); X25519 clamps
  // the same bytes, so this pair's public half is RSK's on the u-line.
  const scalar = new Uint8Array(await crypto.subtle.digest("SHA-512", seed)).slice(0, 32);
  try {
    return {
      signing: await importEd25519Private(seed),
      signingPublic: await publicFromSecret("Ed25519", seed),
      wrap: await crypto.subtle.importKey("raw", wrapRaw, "HKDF", false, ["deriveKey", "deriveBits"]),
      seal: { privateKey: await importX25519Private(scalar), publicKey: await publicFromSecret("X25519", scalar) },
    };
  } finally {
    seed.fill(0);
    wrapRaw.fill(0);
    scalar.fill(0);
  }
}

/** `pending`: joined at `joinSeq` with the recovery key or a passkey slot; not yet approved or activated. */
export type DeviceStatus = { type: "active" } | { type: "pending"; joinSeq: number } | { type: "removed" };

/** A device as the chain has it now (hosts are devices too). */
export interface DeviceRecord {
  info: DeviceIdentity;
  /** Runs an agent and has machine keys (v2: a machine). */
  host: boolean;
  /** Manages the account (v2: a full device). */
  admin: boolean;
  /** Has run AI here at some point (been a host: its agent held its key in a file); such a device never becomes an admin. */
  ranAi: boolean;
  status: DeviceStatus;
  viaRecovery: boolean;
  /** The recovery key it came back with — the account's at its recovery join — so that only the words it knows go with it (v3; write gate). */
  viaRecoveryKey: Bytes | null;
  /** The passkey slot it joined with, if it did. */
  viaPasskey: string | null;
  addedSeq: number;
  /** The entry that made it active (its addition, or a pending device's approval / self-activation). */
  activatedSeq: number | null;
  /** The identity keys it renewed away from, oldest first (`rotateHostKey`). */
  retired: RetiredKeys[];
  /** The entry of its latest renewal, if it renewed. */
  rotatedSeq: number | null;
}

/** Identity keys a device held before a renewal (Rust's `RetiredKeys`). */
export interface RetiredKeys {
  signKey: Bytes;
  dhKey: Bytes;
  /** The renewal that retired them: they were the device's keys for every entry before this one (`keysAt`). */
  untilSeq: number;
  /** Whether that renewal was confirmed on the device itself (`ASSURANCE_ON_DEVICE`): such a key showing up again is recorded, not isolating (§3.8). */
  byAssured: boolean;
}

/** A passkey slot as the chain has it. */
export interface SlotRecord {
  slot: PasskeySlot;
  /** The admin that registered it: removing that device voids the slot. */
  setBy: string;
  addedSeq: number;
  /** Removed, or voided by a removal; a slot id is never used again. */
  removed: boolean;
}

export interface Checkpoint {
  seq: number;
  head: Bytes;
}

/**
 * A verifier's own sense of time for the rules that depend on it: a device
 * that joined with the recovery key (or a passkey) may activate itself only
 * once the verifier has known about its join for `RECOVERY_DELAY_MS`
 * (`PASSKEY_DELAY_MS`) — by its own clock when it first applied the join,
 * never a timestamp anyone else wrote (Rust's `ChainClock`).
 *
 * An activation that is itself history (before `liveFromSeq`) is taken as it
 * is. A *pending* join in that history is not: `apply` observes every entry
 * it accepts on a live clock, history included, so the delay runs from when
 * this verifier first applied the join.
 *
 * The rule a browser follows (never `history()` for a chain it acts on):
 *
 * - A fresh browser (nothing saved for this account) verifies its first chain
 *   with `ChainClock.live(now, head + 1)`: every entry is history, but each is
 *   observed now, so a join still pending there waits a full delay from now.
 * - It persists `firstSeen` and the seq it has verified through after each
 *   merge, and a reload resumes with `ChainClock.resume(now, seenThrough,
 *   firstSeen)` (`live(now, seenThrough + 1)` with the saved sightings), so
 *   a hub cannot hand it a join and its activation together at the next page
 *   load and have the second taken as history.
 */
export class ChainClock {
  constructor(
    public nowMs: number,
    readonly liveFromSeq: number,
    readonly firstSeen: Map<number, number> = new Map(),
  ) {}

  /** For verifying history the verifier was not present for: no time gates. */
  static history(): ChainClock {
    return new ChainClock(0, Number.POSITIVE_INFINITY);
  }

  /** For a verifier that has followed the chain live since `liveFromSeq`. */
  static live(nowMs: number, liveFromSeq: number): ChainClock {
    return new ChainClock(nowMs, liveFromSeq);
  }

  /** A verifier coming back: live from the seq after `seenThrough`, with the first sightings it saved. */
  static resume(nowMs: number, seenThrough: number, firstSeen: Iterable<readonly [number, number]>): ChainClock {
    return ChainClock.live(nowMs, seenThrough + 1).restore(firstSeen);
  }

  /** Whether this clock gates anything (a live verifier must never use `history()`). */
  get isLive(): boolean {
    return this.liveFromSeq !== Number.POSITIVE_INFINITY;
  }

  /** Note that entry `seq` was first seen now, unless it was seen before. A history clock records nothing. */
  observe(seq: number): void {
    if (this.isLive && !this.firstSeen.has(seq)) this.firstSeen.set(seq, this.nowMs);
  }

  /** Replace the first sightings with saved ones (`[seq, ms]` pairs, e.g. `[...clock.firstSeen]`). */
  restore(firstSeen: Iterable<readonly [number, number]>): this {
    this.firstSeen.clear();
    for (const [seq, ms] of firstSeen) this.firstSeen.set(seq, ms);
    return this;
  }

  /** `null` when the join at `joinSeq` may be activated by the entry at `activationSeq` now (after `delayMs`), or the time it may. */
  readyAt(joinSeq: number, activationSeq: number, delayMs: number): number | null {
    if (activationSeq < this.liveFromSeq) return null;
    const ready = (this.firstSeen.get(joinSeq) ?? this.nowMs) + delayMs;
    return this.nowMs < ready ? ready : null;
  }

  /** `readyAt` for a recovery join (`RECOVERY_DELAY_MS`). */
  recoveryReadyAt(joinSeq: number, activationSeq: number): number | null {
    return this.readyAt(joinSeq, activationSeq, RECOVERY_DELAY_MS);
  }
}

/** An admin device's signed statement that, at `timeMs`, the chain it saw ended at `seq` with `head`. */
export interface Attestation {
  account: string;
  device: string;
  seq: number;
  head: Bytes;
  timeMs: number;
  signature: Bytes;
}

function attestationBody(a: Omit<Attestation, "signature">): Bytes {
  return new Writer().str16(a.account).str16(a.device).u64(a.seq).raw(a.head).u64(a.timeMs).finish();
}

export function encodeAttestation(a: Attestation): Bytes {
  return concat(attestationBody(a), a.signature);
}

export function decodeAttestation(data: Uint8Array): Attestation {
  const r = new Reader(data);
  const a = { account: r.str16("account id", MAX_ID_LEN), device: r.str16("device id", MAX_ID_LEN), seq: r.u64(), head: r.bytes(32), timeMs: r.u64(), signature: r.bytes(64) };
  r.finish();
  return a;
}

export type Freshness = { type: "fresh" } | { type: "stale"; newestMs: number | null } | { type: "behind"; seq: number };

/** Who signed an entry, as `apply` established. */
type Authority = { type: "recovery" } | { type: "device"; id: string } | { type: "passkey"; slot: string };

const byId = <T>(list: T[], id: (t: T) => string): T[] => list.sort((a, b) => (id(a) < id(b) ? -1 : id(a) > id(b) ? 1 : 0));

/** A chain verified from genesis: who is in the account right now. */
export class ChainState {
  private readonly serial = new Serial();
  private recoveryKey: Bytes | null;
  /** The device that set the current recovery key (the genesis device for genesis's): removing it clears the key. */
  private recoverySetBy: string | null = null;
  /** Committed user key epochs, by epoch: their `userKeyCommit`s. */
  private readonly userKeys: Bytes[] = [];
  /** Committed key epochs per host. */
  private readonly machineKeys = new Map<string, Bytes[]>();
  /** Every device the chain ever named, hosts included. */
  private readonly devices = new Map<string, DeviceRecord>();
  /** Every passkey slot the chain ever named. */
  private readonly slots = new Map<string, SlotRecord>();
  /** Hosts whose accept policy is not the default (`admins`). */
  private readonly accept = new Map<string, Accept>();
  /** The first v3 entry's seq: no v2 entry may follow it. */
  private v3FromSeq: number | null;
  /** A host that joined or restarted hosting in the last entry: the next entry must commit its next machine key epoch. */
  private hostKeyDueId: string | null;
  /** The account's renewal policy (`setRenewalPolicy`), and the entry that last set it. */
  private alwaysAssuredOn = false;
  private renewalPolicySeqValue: number | null = null;
  /** Every renewal pause the chain started, by op (hex). */
  private readonly pauses = new Map<string, PauseRecord>();

  private constructor(
    readonly account: string,
    readonly accountKey: Bytes,
    recoveryKey: Bytes | null,
    private readonly entries: Entry[],
    private readonly hashes: Bytes[],
    v3From: number | null,
    hostKeyDue: string | null,
  ) {
    this.recoveryKey = recoveryKey;
    this.v3FromSeq = v3From;
    this.hostKeyDueId = hostKeyDue;
  }

  /** Start from a genesis entry (`genesis` or `genesis3`). `pinnedFingerprint`: what an agent pinned at enrollment. */
  static async genesis(entry: Entry, pinnedFingerprint?: Uint8Array): Promise<ChainState> {
    const d = entry.draft;
    const a = d.action;
    let keys: DeviceIdentity;
    let flags: Flags;
    if (a.type === "genesis") {
      keys = { id: a.device.id, signKey: a.device.signKey, dhKey: a.device.dhKey };
      flags = { host: false, admin: a.device.full };
    } else if (a.type === "genesis3") {
      keys = { id: a.device.id, signKey: a.device.signKey, dhKey: a.device.dhKey };
      flags = { host: a.device.flags.host, admin: a.device.flags.admin };
    } else {
      throw new ChainError("notGenesis", "the first entry is not a genesis");
    }
    if (fixedVersion(a) !== d.version) throw new ChainError("version", `unsupported chain entry version ${d.version}`);
    if (d.seq !== 0) throw new ChainError("outOfOrder", `expected entry 0, got ${d.seq}`);
    if (!equal(d.prev, bytes(32))) throw new ChainError("brokenLink", "entry 0: does not follow the entry before it", { seq: 0 });
    if (!validId(d.account)) throw new ChainError("badId", "entry 0: invalid id", { seq: 0 });
    if (d.signer.type !== "account") throw new ChainError("notAllowed", "entry 0: genesis must be signed by the account key", { seq: 0, reason: "genesis must be signed by the account key" });
    if (pinnedFingerprint && !equal(await fingerprint(a.accountKey), pinnedFingerprint)) {
      throw new ChainError("pinMismatch", "the account key does not match the pinned fingerprint");
    }
    if (!(await isValidEd25519Key(a.accountKey)) || (a.recoveryKey && !(await isValidEd25519Key(a.recoveryKey.key)))) {
      throw new ChainError("weakKey", "entry 0: invalid or small-order key", { seq: 0 });
    }
    if (a.recoveryKey && !(await verifyRecoveryKey(a.recoveryKey, d.account, a.accountKey))) {
      throw new ChainError("notAllowed", "entry 0: the recovery key's proof does not hold", { seq: 0, reason: "the recovery key's proof does not hold" });
    }
    if (!(await verifyWith(entry, a.accountKey))) throw new ChainError("badSignature", "entry 0: bad signature", { seq: 0 });
    if (!flags.admin) {
      const reason = d.version === CHAIN_VERSION ? "the first device must be a full device" : "the first device must be an admin device";
      throw new ChainError("notAllowed", `entry 0: ${reason}`, { seq: 0, reason });
    }
    if (flags.host && flags.admin) throw new ChainError("notAllowed", `entry 0: ${RULES.hostNotAdmin}`, { seq: 0, reason: RULES.hostNotAdmin });
    const state = new ChainState(
      d.account,
      copy(a.accountKey),
      a.recoveryKey ? copy(a.recoveryKey.key) : null,
      [entry],
      [await entryHash(entry)],
      d.version === CHAIN_VERSION_3 ? 0 : null,
      flags.host ? keys.id : null,
    );
    state.recoverySetBy = a.recoveryKey ? keys.id : null;
    await state.checkNew(0, keys);
    state.devices.set(keys.id, {
      info: keys,
      host: flags.host,
      admin: flags.admin,
      ranAi: flags.host,
      status: { type: "active" },
      viaRecovery: false,
      viaRecoveryKey: null,
      viaPasskey: null,
      addedSeq: 0,
      activatedSeq: 0,
      retired: [],
      rotatedSeq: null,
    });
    return state;
  }

  static async verify(entries: Entry[], pinnedFingerprint: Uint8Array | undefined, clock: ChainClock): Promise<ChainState> {
    const [first, ...rest] = entries;
    if (!first) throw new ChainError("notGenesis", "an empty chain");
    const state = await ChainState.genesis(first, pinnedFingerprint);
    for (const e of rest) await state.apply(e, clock);
    return state;
  }

  /**
   * `verify` as far as it goes: the genesis must verify (else it throws);
   * after it, the longest run that verifies is the state, and `error` says
   * why it stopped short, if it did (a self-activation too early by `clock`,
   * typically — the join before it is in, first seen now).
   */
  static async verifyPrefix(entries: Entry[], pinnedFingerprint: Uint8Array | undefined, clock: ChainClock): Promise<{ state: ChainState; error: ChainError | null }> {
    const [first, ...rest] = entries;
    if (!first) throw new ChainError("notGenesis", "an empty chain");
    const state = await ChainState.genesis(first, pinnedFingerprint);
    const { error } = await state.mergePrefix(rest, clock);
    return { state, error };
  }

  /** A new device's id and keys: a valid id never used before, canonical keys of full order. */
  private async checkNew(seq: number, keys: DeviceIdentity): Promise<void> {
    if (!validId(keys.id)) throw new ChainError("badId", `entry ${seq}: invalid id`, { seq });
    if (this.devices.has(keys.id)) throw new ChainError("duplicateId", `entry ${seq}: id ${keys.id} was already used`, { seq, id: keys.id });
    if (!(await isValidEd25519Key(keys.signKey)) || !(await isValidX25519(keys.dhKey))) {
      throw new ChainError("weakKey", `entry ${seq}: invalid or small-order key`, { seq });
    }
    // As in Rust: every entry that names a device's keys (genesis, addDevice, addMachine, recoveryJoin,
    // recoveryJoin3, addDevice3 by an admin or a passkey) comes through here; an approval or activation checks the
    // pending device's keys again.
    this.checkNotRetired(seq, keys);
  }

  /**
   * Neither of `keys` is one a renewal retired (`RULES.keyRetired`): checked when keys name a new device and again
   * when a pending one becomes active (`approveDevice`, `activateRecovered`, `passkeyActivate`) — a join made with a
   * host's key before its renewal does not come alive after it.
   */
  private checkNotRetired(seq: number, keys: { signKey: Uint8Array; dhKey: Uint8Array }): void {
    if (this.isRetiredKey(keys.signKey) || this.isRetiredKey(keys.dhKey)) {
      throw new ChainError("notAllowed", `entry ${seq}: ${RULES.keyRetired}`, { seq, reason: RULES.keyRetired });
    }
  }

  /** Who may sign `entry`, checked (signature included); `v2` entries see the v2 view of the state (a host is no signer). */
  private async authority(entry: Entry): Promise<Authority> {
    const d = entry.draft;
    const seq = d.seq;
    const a = d.action;
    const recoveryAction = a.type === "recoveryJoin" || a.type === "recoveryJoin3" || a.type === "activateRecovered";
    const notAllowed = (reason: string) => new ChainError("notAllowed", `entry ${seq}: ${reason}`, { seq, reason });
    if (a.type === "genesis" || a.type === "genesis3") throw new ChainError("lateGenesis", `entry ${seq}: a genesis after the first entry`, { seq });
    switch (d.signer.type) {
      case "account":
        throw notAllowed("the account key signs genesis and nothing else");
      case "recovery":
        if (!recoveryAction) throw notAllowed("the recovery key signs only recovery entries");
        if (!this.recoveryKey) throw notAllowed("this account has no recovery key");
        if (!(await verifyWith(entry, this.recoveryKey))) throw new ChainError("badSignature", `entry ${seq}: bad signature`, { seq });
        return { type: "recovery" };
      case "passkey": {
        if (a.type !== "addDevice3" && a.type !== "passkeyActivate") throw notAllowed("a passkey slot signs only its own joins and activations");
        const slot = d.signer.slot;
        const record = this.slot(slot);
        if (!record) throw new ChainError("unknownSlot", `entry ${seq}: no current passkey slot ${JSON.stringify(slot)}`, { seq, slot });
        if (!(await verifyWith(entry, record.slot.key))) throw new ChainError("badSignature", `entry ${seq}: bad signature`, { seq });
        return { type: "passkey", slot };
      }
      case "device": {
        if (recoveryAction) throw notAllowed("recovery entries are signed by the recovery key");
        if (a.type === "passkeyActivate") throw notAllowed("a passkey activation is signed by its slot");
        const id = d.signer.id;
        const v2 = d.version === CHAIN_VERSION;
        const signer = this.devices.get(id);
        // v2 knew no signing machines: a host is no v2 signer.
        if (!signer || signer.status.type !== "active" || (v2 && signer.host)) {
          throw new ChainError("unknownId", `entry ${seq}: no current device or machine ${JSON.stringify(id)}`, { seq, id });
        }
        if (!signer.admin) throw notAllowed(v2 ? "only a full device may sign" : "only an admin device may sign");
        if (!(await verifyWith(entry, signer.info.signKey))) throw new ChainError("badSignature", `entry ${seq}: bad signature`, { seq });
        return { type: "device", id };
      }
    }
  }

  /**
   * Apply the next entry, or refuse it and leave the state as it was. An
   * accepted entry is recorded in `clock` as first seen now, so the recovery
   * and passkey delays always run from the moment *this* verifier applied
   * the join. Serialised.
   */
  apply(entry: Entry, clock: ChainClock): Promise<void> {
    return this.serial.run(() => this.applyNow(entry, clock));
  }

  private async applyNow(entry: Entry, clock: ChainClock): Promise<void> {
    const d = entry.draft;
    const seq = d.seq;
    const expected = this.hashes.length;
    const notAllowed = (reason: string) => new ChainError("notAllowed", `entry ${seq}: ${reason}`, { seq, reason });
    const unknownId = (id: string) => new ChainError("unknownId", `entry ${seq}: no current device or machine ${JSON.stringify(id)}`, { seq, id });
    if (seq !== expected) throw new ChainError("outOfOrder", `expected entry ${expected}, got ${seq}`);
    if (d.account !== this.account) throw new ChainError("wrongAccount", `entry ${seq}: belongs to another account`, { seq });
    if (!equal(d.prev, this.head)) throw new ChainError("brokenLink", `entry ${seq}: does not follow the entry before it`, { seq });
    const fixed = fixedVersion(d.action);
    if (fixed !== null ? fixed !== d.version : d.version !== CHAIN_VERSION && d.version !== CHAIN_VERSION_3) {
      throw new ChainError("version", `unsupported chain entry version ${d.version}`);
    }
    if (d.signer.type === "passkey" && d.version !== CHAIN_VERSION_3) throw new ChainError("version", `unsupported chain entry version ${d.version}`);
    if (d.version === CHAIN_VERSION && this.v3FromSeq !== null) throw notAllowed("a v2 entry after the chain moved to v3");
    // A host that just joined (or restarted hosting) gets its next machine
    // key epoch in the very next entry, before anything else.
    const due = this.hostKeyDueId;
    if (due !== null && !(d.action.type === "keyEpochs" && d.action.machines.some((m) => m.machine === due))) {
      throw notAllowed("a new host's machine key epoch must come next");
    }
    const authority = await this.authority(entry);
    const signerId = authority.type === "device" ? authority.id : null;
    // What the action may do. Every check happens before any change.
    let hostKeyDue: string | null = null;
    const a = d.action;
    switch (a.type) {
      case "genesis":
      case "genesis3":
        throw new Error("unreachable: a late genesis is refused above");
      case "addDevice": {
        const keys = identity(a.device);
        await this.checkNew(seq, keys);
        this.insert(keys, { host: false, admin: a.device.full }, { type: "active" }, seq);
        break;
      }
      case "addMachine": {
        const keys = identity(a.machine);
        await this.checkNew(seq, keys);
        this.insert(keys, { host: true, admin: false }, { type: "active" }, seq);
        break;
      }
      case "recoveryJoin": {
        const keys = identity(a.device);
        await this.checkNew(seq, keys);
        if (!a.device.full) throw notAllowed("a recovered device is a full device");
        this.markRecovered(this.insert(keys, { host: false, admin: true }, { type: "pending", joinSeq: seq }, seq));
        break;
      }
      case "recoveryJoin3": {
        const keys = identity(a.device);
        await this.checkNew(seq, keys);
        if (!a.device.flags.admin) throw notAllowed(RULES.recoveredAdmin);
        if (a.device.flags.host) throw notAllowed(RULES.hostNotAdmin);
        this.markRecovered(this.insert(keys, a.device.flags, { type: "pending", joinSeq: seq }, seq));
        break;
      }
      case "addDevice3": {
        const keys = identity(a.device);
        await this.checkNew(seq, keys);
        if (authority.type === "passkey") {
          if (a.device.flags.host || a.device.flags.admin) throw notAllowed(RULES.passkeyPlain);
          // One pending join per slot: a newer one replaces it.
          for (const record of this.devices.values()) {
            if (record.viaPasskey === authority.slot && record.status.type === "pending") record.status = { type: "removed" };
          }
          this.insert(keys, { host: false, admin: false }, { type: "pending", joinSeq: seq }, seq).viaPasskey = authority.slot;
        } else {
          if (a.device.flags.host && a.device.flags.admin) throw notAllowed(RULES.hostNotAdmin);
          if (a.device.flags.host) hostKeyDue = a.device.id;
          this.insert(keys, a.device.flags, { type: "active" }, seq);
        }
        break;
      }
      case "approveDevice": {
        const record = this.devices.get(a.id);
        if (!record) throw unknownId(a.id);
        if (record.status.type !== "pending") throw notAllowed("only a pending device can be approved");
        this.checkNotRetired(seq, record.info);
        record.status = { type: "active" };
        record.activatedSeq = seq;
        break;
      }
      case "activateRecovered": {
        const record = this.devices.get(a.id);
        if (!record) throw unknownId(a.id);
        if (record.status.type !== "pending") throw notAllowed("only a pending device can be activated");
        this.checkNotRetired(seq, record.info);
        if (!record.viaRecovery) throw notAllowed("only a device that came back with the recovery words activates with them");
        const readyMs = clock.readyAt(record.status.joinSeq, seq, RECOVERY_DELAY_MS);
        if (readyMs !== null) throw new ChainError("tooEarly", `entry ${seq}: a pending device may activate itself only from ${readyMs} on`, { seq, readyMs });
        record.status = { type: "active" };
        record.activatedSeq = seq;
        break;
      }
      case "passkeyActivate": {
        if (authority.type !== "passkey") throw new Error("unreachable: only a slot signs an activation");
        const record = this.devices.get(a.id);
        if (!record) throw unknownId(a.id);
        if (record.status.type !== "pending") throw notAllowed("only a pending device can be activated");
        this.checkNotRetired(seq, record.info);
        if (record.viaPasskey !== authority.slot) throw notAllowed("a passkey slot activates only the device it added");
        if (!this.slot(authority.slot)?.slot.selfActivate) throw notAllowed("this passkey slot does not let its devices activate themselves");
        const readyMs = clock.readyAt(record.status.joinSeq, seq, PASSKEY_DELAY_MS);
        if (readyMs !== null) throw new ChainError("tooEarly", `entry ${seq}: a pending device may activate itself only from ${readyMs} on`, { seq, readyMs });
        record.status = { type: "active" };
        record.activatedSeq = seq;
        break;
      }
      case "removeDevice": {
        // v2: a device (never a machine).
        const record = this.devices.get(a.id);
        if (!record || record.status.type === "removed" || record.host) throw unknownId(a.id);
        if (isCurrentAdmin(record) && this.adminsExcept(a.id) === 0) throw new ChainError("lastFullDevice", `entry ${seq}: would remove the last full device`, { seq });
        this.remove(a.id, "v2");
        break;
      }
      case "removeMachine": {
        // v2: a current machine.
        const record = this.devices.get(a.id);
        if (!record || !isCurrentHost(record)) throw unknownId(a.id);
        record.status = { type: "removed" };
        break;
      }
      case "removeDevice3": {
        const record = this.devices.get(a.id);
        if (!record || record.status.type === "removed") throw unknownId(a.id);
        if (isCurrentAdmin(record) && this.adminsExcept(a.id) === 0) throw new ChainError("lastAdmin", `entry ${seq}: would leave the account without an admin device`, { seq });
        this.remove(a.id, "v3");
        break;
      }
      case "setAdmin": {
        const record = this.devices.get(a.id);
        if (!record || record.status.type !== "active") throw unknownId(a.id);
        if (a.admin && signerId === a.id) throw notAllowed(RULES.selfAdmin);
        if (record.admin === a.admin) throw notAllowed(RULES.noChange);
        if (a.admin && record.host) throw notAllowed(RULES.hostNotAdmin);
        if (a.admin && (record.ranAi || this.machineKeyEpochs(a.id) > 0)) throw notAllowed(RULES.ranAiNotAdmin);
        if (!a.admin && this.adminsExcept(a.id) === 0) throw new ChainError("lastAdmin", `entry ${seq}: would leave the account without an admin device`, { seq });
        record.admin = a.admin;
        // As in Rust: a demoted admin's passkey slots go with its standing,
        // and so does the recovery key if it knows the current words (it set
        // them, or came back with them), as on its removal; words set since stand.
        if (!a.admin) {
          for (const slot of this.slots.values()) if (!slot.removed && slot.setBy === a.id) slot.removed = true;
          if (this.knowsCurrentRecoveryKey(a.id)) {
            this.recoveryKey = null;
            this.recoverySetBy = null;
          }
        }
        break;
      }
      case "setHost": {
        const record = this.devices.get(a.id);
        if (!record || record.status.type !== "active") throw unknownId(a.id);
        if (a.host && signerId === a.id) throw notAllowed(RULES.selfHost);
        if (record.host === a.host) throw notAllowed(RULES.noChange);
        if (a.host && record.admin) throw notAllowed(RULES.hostNotAdmin);
        if (a.host) hostKeyDue = a.id;
        // As in Rust: its accept policy goes with its hosting.
        else this.accept.delete(a.id);
        record.host = a.host;
        record.ranAi ||= a.host;
        break;
      }
      case "setRecoveryKey":
        if (a.key && !(await isValidEd25519Key(a.key.key))) throw new ChainError("weakKey", `entry ${seq}: invalid or small-order key`, { seq });
        if (a.key && !(await verifyRecoveryKey(a.key, this.account, this.accountKey))) throw notAllowed("the recovery key's proof does not hold");
        this.recoveryKey = a.key ? copy(a.key.key) : null;
        this.recoverySetBy = a.key && signerId !== null ? signerId : null;
        break;
      case "keyEpochs": {
        if (!a.user && a.machines.length === 0) throw notAllowed("a key epoch entry moves no key");
        if (a.machines.length > MAX_KEY_COMMITS) throw notAllowed("too many machine keys in one entry");
        if (a.user && a.user.epoch !== this.userKeys.length) throw notAllowed("a user key epoch must be the one after the last");
        const seen = new Set<string>();
        for (const m of a.machines) {
          if (!this.machine(m.machine)) throw unknownId(m.machine);
          if (seen.has(m.machine)) throw notAllowed("a machine twice in one entry");
          seen.add(m.machine);
          if (m.epoch !== (this.machineKeys.get(m.machine)?.length ?? 0)) throw notAllowed("a machine key epoch must be the one after the last");
        }
        if (a.user) this.userKeys.push(copy(a.user.hash));
        for (const m of a.machines) {
          const list = this.machineKeys.get(m.machine);
          if (list) list.push(copy(m.hash));
          else this.machineKeys.set(m.machine, [copy(m.hash)]);
        }
        break;
      }
      case "passkeySlotSet": {
        const s = a.slot;
        if (!validId(s.slot)) throw new ChainError("badId", `entry ${seq}: invalid id`, { seq });
        if (this.slots.has(s.slot)) throw new ChainError("duplicateId", `entry ${seq}: id ${s.slot} was already used`, { seq, id: s.slot });
        if (s.grants !== 0) throw notAllowed(RULES.slotGrants);
        if (!(await isValidEd25519Key(s.key))) throw new ChainError("weakKey", `entry ${seq}: invalid or small-order key`, { seq });
        if (!(await verifyPasskeySlot(s, this.account, this.accountKey))) throw notAllowed("the passkey slot's proof does not hold");
        const current = [...this.slots.values()].filter((r) => !r.removed);
        if (current.length >= MAX_PASSKEY_SLOTS) throw notAllowed("too many passkey slots");
        if (current.some((r) => equal(r.slot.credHash, s.credHash))) throw notAllowed("that passkey already has a current slot");
        if (signerId === null) throw new Error("unreachable: an admin signs a slot");
        this.slots.set(s.slot, { slot: s, setBy: signerId, addedSeq: seq, removed: false });
        break;
      }
      case "passkeySlotRemove": {
        const record = this.slot(a.slot);
        if (!record) throw new ChainError("unknownSlot", `entry ${seq}: no current passkey slot ${JSON.stringify(a.slot)}`, { seq, slot: a.slot });
        record.removed = true;
        break;
      }
      case "setAccept": {
        const record = this.devices.get(a.host);
        if (!record || record.status.type !== "active") throw unknownId(a.host);
        if (!record.host) throw notAllowed(RULES.acceptNotHost);
        if (a.from.type === "devices") {
          for (const id of a.from.ids) {
            if (!validId(id)) throw new ChainError("badId", `entry ${seq}: invalid id`, { seq });
            if (!this.device(id)) throw unknownId(id);
            if (id === a.host) throw notAllowed(RULES.acceptSelf);
          }
        }
        if (sameAccept(this.acceptPolicy(a.host), a.from)) throw notAllowed(RULES.acceptSame);
        // As in Rust (review D2): a narrowing takes a new machine key epoch
        // for the host in the very next entry.
        const before = this.extraControllers(a.host, this.acceptPolicy(a.host));
        const after = this.extraControllers(a.host, a.from);
        if ([...before].some((d) => !after.has(d))) hostKeyDue = a.host;
        if (a.from.type === "admins") this.accept.delete(a.host);
        else this.accept.set(a.host, a.from);
        break;
      }
      case "rotateHostKey": {
        // As in Rust (`docs/host-identity-rotation.md` §3.4). Rule 1 (v3 only, an active admin signs) and rule 7 (a
        // host that owes a machine key epoch gets that first) are checked above, as for every entry.
        const r = a.rotation;
        const record = this.machine(r.host);
        if (!record) throw notAllowed(RULES.rotateNotHost);
        if (!(await isValidEd25519Key(r.signKey)) || !(await isValidX25519(r.dhKey))) throw new ChainError("weakKey", `entry ${seq}: invalid or small-order key`, { seq });
        if (this.everNamed(r.signKey) || this.everNamed(r.dhKey)) throw notAllowed(RULES.rotateKeyReused);
        const staged = r.stagedSeq < seq ? this.hashAt(r.stagedSeq) : undefined;
        if (!staged || !equal(staged, r.stagedHead)) throw notAllowed(RULES.rotateStaged);
        const old = record.info;
        if (!(await verifyHostRotation(this.account, r, old.signKey, old.dhKey))) throw notAllowed(RULES.rotateProof);
        // Rule 9: the account's policy may ask for every renewal to be confirmed on the device.
        if (this.alwaysAssuredOn && r.assurance !== ASSURANCE_ON_DEVICE) throw notAllowed(RULES.rotateNeedsAssurance);
        // Rule 6: its next machine key epoch — sealed to the new key only — comes in the very next entry.
        hostKeyDue = r.host;
        record.retired = [...record.retired, { signKey: old.signKey, dhKey: old.dhKey, untilSeq: seq, byAssured: r.assurance === ASSURANCE_ON_DEVICE }];
        record.info = { id: old.id, signKey: copy(r.signKey), dhKey: copy(r.dhKey) };
        record.rotatedSeq = seq;
        break;
      }
      case "setRenewalPolicy":
        // As in Rust (§3.4 rule 9): an active admin signs it (checked above); setting what is already set is allowed.
        this.alwaysAssuredOn = a.alwaysAssured;
        this.renewalPolicySeqValue = seq;
        break;
      case "renewalPause": {
        // As in Rust (§3.4 rule 10): a current, active admin signs it — a start or an end, not necessarily the one that
        // started it (checked above; a passkey or the recovery words sign nothing of it).
        if (signerId === null) throw new Error("unreachable: an admin signs a renewal pause");
        const op = hex(a.op);
        if (a.phase === PAUSE_START) {
          if (this.pauses.has(op)) throw notAllowed(RULES.pauseOpReused);
          // Canonical: hosts, and each host's ids, strictly ascending by bytes — so each once, and one list has one encoding.
          if (a.removals.length === 0 || a.removals.length > MAX_PAUSE_HOSTS || !ascending(a.removals.map((r) => r.host)) || a.removals.some((r) => !this.machine(r.host))) {
            throw notAllowed(RULES.pauseHosts);
          }
          if (a.removals.reduce((n, r) => n + r.ids.length, 0) > MAX_PAUSE_EDGES) throw notAllowed(RULES.pauseEdges);
          if (encodeEntry(entry).length > MAX_PAUSE_ENTRY_LEN) throw notAllowed(RULES.pauseTooLarge);
          for (const { host, ids } of a.removals) {
            if (ids.length === 0 || !ascending(ids) || ids.some((id) => id === host || !this.devices.has(id) || this.admin(id) !== undefined)) throw notAllowed(RULES.pauseIds);
          }
          this.pauses.set(op, { removals: copyRemovals(a.removals), startSeq: seq, signer: signerId, end: null });
        } else {
          const record = this.pauses.get(op);
          if (!record || record.end !== null) throw notAllowed(RULES.pauseNotOpen);
          if (a.removals.length !== 0) throw notAllowed(RULES.pauseEndRemovals);
          record.end = { phase: a.phase, seq };
        }
        break;
      }
    }
    this.hostKeyDueId = hostKeyDue;
    if (d.version === CHAIN_VERSION_3 && this.v3FromSeq === null) this.v3FromSeq = seq;
    this.entries.push(entry);
    this.hashes.push(await entryHash(entry));
    // As in Rust: an accepted entry is recorded as first seen now, so the
    // recovery and passkey delays run from when *this* verifier applied the join.
    clock.observe(seq);
  }

  /**
   * Mark `id` removed, with what removing it takes along: a recovery key it
   * set or came back with (it knows `RK`), and a passkey slot it registered
   * or joined with (it computed the slot's PRF).
   */
  /** The current devices other than admins and the host itself that `from` lets control `host`. */
  private extraControllers(host: string, from: Accept): Set<string> {
    const out = new Set<string>();
    for (const d of this.devices.values()) {
      if (d.status.type !== "active" || d.admin || d.info.id === host) continue;
      if (from.type === "all" || (from.type === "devices" && from.ids.includes(d.info.id))) out.add(d.info.id);
    }
    return out;
  }

  /** A device that came back with the recovery words: which words (the account's at its join). */
  private markRecovered(record: DeviceRecord): void {
    record.viaRecovery = true;
    record.viaRecoveryKey = this.recoveryKey ? copy(this.recoveryKey) : null;
  }

  /**
   * Whether taking `id` out — demoting it, or removing it — voids the account's recovery words, as this verifier
   * will: v3 only when it knows the current words (it set them, or came back with them); a v2 removal whenever it
   * set or came back with any. For the dialogs that say so.
   */
  voidsRecoveryWords(id: string, by: "demotion" | "removal"): boolean {
    if (!this.recoveryKey) return false;
    if (by === "removal" && !this.isV3) {
      const record = this.devices.get(id);
      return !!record && (record.viaRecovery || this.recoverySetBy === id);
    }
    return this.knowsCurrentRecoveryKey(id);
  }

  /** Whether `id` knows the account's current recovery words: it set them, or came back with them (chain v3; write gate). */
  private knowsCurrentRecoveryKey(id: string): boolean {
    const current = this.recoveryKey;
    if (!current) return false;
    if (this.recoverySetBy === id) return true;
    const came = this.devices.get(id)?.viaRecoveryKey;
    return !!came && equal(came, current);
  }

  /** Remove `id`. v2's `removeDevice` voids whatever recovery words the account has if the device set or came back with any (unchanged, so v2 chains replay as before); v3's only the words it knows. */
  private remove(id: string, rule: "v2" | "v3"): void {
    const record = this.devices.get(id)!;
    if (rule === "v2" ? record.viaRecovery || this.recoverySetBy === id : this.knowsCurrentRecoveryKey(id)) {
      this.recoveryKey = null;
      this.recoverySetBy = null;
    }
    for (const slot of this.slots.values()) {
      if (!slot.removed && (slot.setBy === id || record.viaPasskey === slot.slot.slot)) slot.removed = true;
    }
    record.status = { type: "removed" };
  }

  private insert(info: DeviceIdentity, flags: Flags, status: DeviceStatus, seq: number): DeviceRecord {
    const record: DeviceRecord = {
      info,
      host: flags.host,
      admin: flags.admin,
      ranAi: flags.host,
      status,
      viaRecovery: false,
      viaRecoveryKey: null,
      viaPasskey: null,
      addedSeq: seq,
      activatedSeq: status.type === "active" ? seq : null,
      retired: [],
      rotatedSeq: null,
    };
    this.devices.set(info.id, record);
    return record;
  }

  /** Active admins other than `id`. */
  private adminsExcept(id: string): number {
    let n = 0;
    for (const d of this.devices.values()) if (isCurrentAdmin(d) && d.info.id !== id) n++;
    return n;
  }

  /**
   * Apply whatever of `entries` this view lacks; known ones must match (else a
   * fork). Serialised. **Not atomic** (as Rust's): on an error, the entries
   * before the one refused stay applied — act on the seq before and after,
   * not only on success (`mergePrefix` says both).
   */
  async merge(entries: Entry[], clock: ChainClock): Promise<number> {
    const { applied, error } = await this.mergePrefix(entries, clock);
    if (error) throw error;
    return applied;
  }

  /**
   * `merge`, saying how far it got and why it stopped (Rust's
   * `merge_partial`). Partial on purpose: a recovery join followed by a
   * self-activation this verifier's clock says is too early leaves the join
   * applied — first seen now, shown, and vetoable — or the verifier would
   * never see the join, and so never be able to take the activation either.
   */
  mergePrefix(entries: Entry[], clock: ChainClock): Promise<{ applied: number; error: ChainError | null }> {
    return this.serial.run(async () => {
      let applied = 0;
      for (const e of entries) {
        if (e.draft.seq <= this.seq) {
          if (!equal(this.hashes[e.draft.seq]!, await entryHash(e))) {
            return { applied, error: new ChainError("fork", `the chain forked at entry ${e.draft.seq}`, { seq: e.draft.seq }) };
          }
          continue;
        }
        try {
          await this.applyNow(e, clock);
        } catch (error) {
          if (error instanceof ChainError) return { applied, error };
          throw error;
        }
        applied += 1;
      }
      return { applied, error: null };
    });
  }

  /**
   * A copy of this view that entries can be applied to without touching it
   * (a writer tries its batch on one before signing it away: the same
   * verifier, so a batch it would refuse never reaches the hub).
   */
  clone(): ChainState {
    const c = new ChainState(this.account, this.accountKey, this.recoveryKey, [...this.entries], [...this.hashes], this.v3FromSeq, this.hostKeyDueId);
    c.recoverySetBy = this.recoverySetBy;
    c.alwaysAssuredOn = this.alwaysAssuredOn;
    c.renewalPolicySeqValue = this.renewalPolicySeqValue;
    for (const [op, p] of this.pauses) c.pauses.set(op, { ...p, removals: copyRemovals(p.removals), end: p.end ? { ...p.end } : null });
    c.userKeys.push(...this.userKeys);
    for (const [m, keys] of this.machineKeys) c.machineKeys.set(m, [...keys]);
    for (const [id, d] of this.devices) c.devices.set(id, { ...d, status: { ...d.status }, retired: [...d.retired] });
    for (const [id, s] of this.slots) c.slots.set(id, { ...s });
    for (const [host, from] of this.accept) c.accept.set(host, from);
    return c;
  }

  /** Every entry of this view, oldest first. */
  allEntries(): Entry[] {
    return [...this.entries];
  }

  get seq(): number {
    return this.hashes.length - 1;
  }

  get head(): Bytes {
    return this.hashes[this.hashes.length - 1]!;
  }

  get currentRecoveryKey(): Bytes | null {
    return this.recoveryKey;
  }

  /** Whether the chain holds a v3 entry (and so takes no more v2 ones). */
  get isV3(): boolean {
    return this.v3FromSeq !== null;
  }

  /** The seq of the chain's first v3 entry. */
  get v3From(): number | null {
    return this.v3FromSeq;
  }

  /** A host that joined or restarted hosting in the last entry and still awaits its machine key epoch: a batch must not end here. */
  get hostKeyDue(): string | null {
    return this.hostKeyDueId;
  }

  /** Whether the account asks for every renewal to be confirmed on the device (`setRenewalPolicy`; off until an admin turns it on). A getter, as Rust's `always_assured()`. */
  get alwaysAssured(): boolean {
    return this.alwaysAssuredOn;
  }

  /** The entry that last set the renewal policy, if any did (Rust's `renewal_policy_seq()`). */
  get renewalPolicySeq(): number | null {
    return this.renewalPolicySeqValue;
  }

  /** The renewal pauses still open (§3.4 rule 10), oldest first (Rust's `open_pauses()`). */
  openPauses(): OpenPause[] {
    const open: OpenPause[] = [];
    for (const [op, p] of this.pauses) if (p.end === null) open.push({ op: fromHex(op), startSeq: p.startSeq, signer: p.signer, removals: copyRemovals(p.removals) });
    return open.sort((x, y) => x.startSeq - y.startSeq);
  }

  /** Any renewal pause the chain started, by op, open or ended (Rust's `pause(op)`). */
  pause(op: Uint8Array): PauseRecord | undefined {
    const p = this.pauses.get(hex(op));
    return p ? { ...p, removals: copyRemovals(p.removals), end: p.end ? { ...p.end } : null } : undefined;
  }

  /**
   * The ids host `host` stops accepting while renewal pauses are open (§3.4 rule 10): every open pause's removals for
   * it, sorted by bytes, once each (Rust's `paused_edges`). `mayControl` refuses them, whatever `baseAccept` says.
   */
  pausedEdges(host: string): string[] {
    const ids = new Set<string>();
    for (const p of this.pauses.values()) {
      if (p.end !== null) continue;
      for (const r of p.removals) if (r.host === host) for (const id of r.ids) ids.add(id);
    }
    return [...ids].sort(compareUtf8);
  }

  /** The accept policy `setAccept` set for `host` — the base a renewal pause overlays, and never edits (Rust's `base_accept`). */
  baseAccept(host: string): Accept {
    return this.acceptPolicy(host);
  }

  /** How many user key epochs are committed (the next one's number). */
  userKeyEpochs(): number {
    return this.userKeys.length;
  }

  /** The newest committed user key epoch. */
  latestUserEpoch(): number | null {
    return this.userKeys.length > 0 ? this.userKeys.length - 1 : null;
  }

  /** How many key epochs are committed for `machine` (the next one's number). */
  machineKeyEpochs(machine: string): number {
    return this.machineKeys.get(machine)?.length ?? 0;
  }

  /** The newest committed key epoch of `machine`. */
  latestMachineEpoch(machine: string): number | null {
    const n = this.machineKeyEpochs(machine);
    return n > 0 ? n - 1 : null;
  }

  /** `key` is user key epoch `epoch` as the chain committed it; else `uncommittedKey` / `keyMismatch`. */
  async checkUserKey(epoch: number, key: Uint8Array): Promise<void> {
    const committed = Number.isInteger(epoch) && epoch >= 0 ? this.userKeys[epoch] : undefined;
    if (!committed) throw new ChainError("uncommittedKey", `no key epoch ${epoch} is committed for ""`, { id: "", epoch });
    if (!equal(committed, await userKeyCommit(this.account, epoch, key))) {
      throw new ChainError("keyMismatch", `the key for "" epoch ${epoch} is not the one the chain committed`, { id: "", epoch });
    }
  }

  /** `key` is `machine`'s key epoch `epoch` as the chain committed it; else `uncommittedKey` / `keyMismatch`. */
  async checkMachineKey(machine: string, epoch: number, key: Uint8Array): Promise<void> {
    const committed = Number.isInteger(epoch) && epoch >= 0 ? this.machineKeys.get(machine)?.[epoch] : undefined;
    if (!committed) throw new ChainError("uncommittedKey", `no key epoch ${epoch} is committed for ${JSON.stringify(machine)}`, { id: machine, epoch });
    if (!equal(committed, await machineKeyCommit(this.account, machine, epoch, key))) {
      throw new ChainError("keyMismatch", `the key for ${JSON.stringify(machine)} epoch ${epoch} is not the one the chain committed`, { id: machine, epoch });
    }
  }

  hashAt(seq: number): Bytes | undefined {
    return this.hashes[seq];
  }

  /** Entry `seq`, if this view has it. */
  entryAt(seq: number): Entry | undefined {
    return this.entries[seq];
  }

  checkpoint(): Checkpoint {
    return { seq: this.seq, head: copy(this.head) };
  }

  /** The last `n` entries: what a device sends an agent that may lag behind. */
  suffix(n: number): Entry[] {
    return this.entries.slice(Math.max(0, this.entries.length - n));
  }

  checkExtends(seen: Checkpoint): void {
    if (this.seq < seen.seq) throw new ChainError("rollback", `the chain was rolled back: seen ${seen.seq}, offered ${this.seq}`);
    if (!equal(this.hashes[seen.seq]!, seen.head)) throw new ChainError("fork", `the chain forked at entry ${seen.seq}`, { seq: seen.seq });
  }

  /** A current, active device (any flags: hosts are devices too). */
  device(id: string): DeviceRecord | undefined {
    const d = this.devices.get(id);
    return d?.status.type === "active" ? d : undefined;
  }

  /** Any device the chain has ever named, in whatever state. */
  deviceRecord(id: string): DeviceRecord | undefined {
    return this.devices.get(id);
  }

  /** The identity keys device `id` renewed away from, oldest first (none for a device the chain does not name). */
  retiredKeys(id: string): RetiredKeys[] {
    return [...(this.devices.get(id)?.retired ?? [])];
  }

  /**
   * Device `id`'s keys as they stood once entry `seq` had taken effect: the old keys before a renewal, the new ones
   * from it on; `undefined` before the device joined, or past this view (Rust's `keys_at`). What a stored stream head
   * or a dispute's signed login is checked against (`docs/host-identity-rotation.md` §3.8, §5.4).
   */
  keysAt(id: string, seq: number): DeviceIdentity | undefined {
    const d = this.devices.get(id);
    if (!d || !Number.isSafeInteger(seq) || seq < d.addedSeq || seq > this.seq) return undefined;
    const r = d.retired.find((k) => seq < k.untilSeq);
    return r ? { id, signKey: r.signKey, dhKey: r.dhKey } : { id, signKey: d.info.signKey, dhKey: d.info.dhKey };
  }

  /**
   * Whether `signature` disputes the renewal at `seq` of `host` (Rust's `verify_rotate_dispute`): entry `seq` of this
   * view is a `rotateHostKey` for `host`, the statement is over that entry's hash and the sign key it put in, and it is
   * signed by the sign key that entry retired (`keysAt(host, seq - 1)`). A hub cannot forge one; it can only withhold it.
   */
  async verifyRotateDispute(host: string, seq: number, signature: Uint8Array): Promise<boolean> {
    if (!Number.isSafeInteger(seq) || seq < 1) return false;
    const entry = this.entries[seq];
    const a = entry?.draft.action;
    if (a?.type !== "rotateHostKey" || a.rotation.host !== host) return false;
    const hash = this.hashAt(seq);
    const retired = this.keysAt(host, seq - 1);
    if (!hash || !retired) return false;
    return verifyLabelled(retired.signKey, ROTATE_DISPUTE, rotateDisputeBody(this.account, host, seq, hash, a.rotation.signKey), signature);
  }

  /** Whether `key` is one a renewal retired — any device's, sign or DH: such a key never names a device again (`RULES.keyRetired`). */
  isRetiredKey(key: Uint8Array): boolean {
    for (const d of this.devices.values()) if (d.retired.some((r) => equal(r.signKey, key) || equal(r.dhKey, key))) return true;
    return false;
  }

  /** Whether any device the chain ever named — in whatever state — holds or held `key`, as its sign or its DH key. */
  everNamed(key: Uint8Array): boolean {
    for (const d of this.devices.values()) {
      if (equal(d.info.signKey, key) || equal(d.info.dhKey, key)) return true;
      if (d.retired.some((r) => equal(r.signKey, key) || equal(r.dhKey, key))) return true;
    }
    return false;
  }

  /** A current host: an active device that runs AI (v2: a current machine). */
  machine(id: string): DeviceRecord | undefined {
    const d = this.devices.get(id);
    return d && isCurrentHost(d) ? d : undefined;
  }

  /** A current admin device (v2: a current full device). */
  admin(id: string): DeviceRecord | undefined {
    const d = this.devices.get(id);
    return d && isCurrentAdmin(d) ? d : undefined;
  }

  /** Every device the chain ever named, hosts included, by id. */
  allDevices(): DeviceRecord[] {
    return byId([...this.devices.values()], (d) => d.info.id);
  }

  /** Every device the chain ever named that is (or was, when removed) a host, by id (v2: every machine). */
  hosts(): DeviceRecord[] {
    return this.allDevices().filter((d) => d.host);
  }

  /** The active admin devices, by id. */
  admins(): DeviceRecord[] {
    return this.allDevices().filter(isCurrentAdmin);
  }

  /** A current passkey slot. */
  slot(slot: string): SlotRecord | undefined {
    const s = this.slots.get(slot);
    return s && !s.removed ? s : undefined;
  }

  /** Every passkey slot the chain ever named, by id. */
  allSlots(): SlotRecord[] {
    return byId([...this.slots.values()], (s) => s.slot.slot);
  }

  /** Whom host `host` accepts control from besides admin devices. */
  acceptPolicy(host: string): Accept {
    return this.accept.get(host) ?? { type: "admins" };
  }

  private pending(which: (d: DeviceRecord) => boolean): { id: string; joinSeq: number }[] {
    return this.allDevices().flatMap((d) => (which(d) && d.status.type === "pending" ? [{ id: d.info.id, joinSeq: d.status.joinSeq }] : []));
  }

  /** Devices that joined with the recovery key and are not yet active: alert about these. */
  pendingRecoveries(): { id: string; joinSeq: number }[] {
    return this.pending((d) => d.viaRecovery);
  }

  /** Devices that joined with a passkey slot and are not yet active. */
  pendingPasskeyJoins(): { id: string; joinSeq: number }[] {
    return this.pending((d) => d.viaPasskey !== null);
  }

  /** The check behind a handshake: `dhKey` is the key of current, active device `id` (a pending one is refused). */
  authorizeDevice(id: string, dhKey: Uint8Array): void {
    const d = this.device(id);
    if (!d || !equal(d.info.dhKey, dhKey)) throw new ChainError("unknownDevice", `device ${id} is not a current, active device of this account`);
  }

  /**
   * Whether device `device` may control host `host` (Rust's `may_control`,
   * "谁可以控制我"): never itself; only a current device to a current host;
   * an admin device always; any other device only when the host's accept
   * policy names it (or all).
   */
  mayControl(device: string, host: string): void {
    if (device === host) throw new ChainError("selfControl", "a device does not control itself");
    const requester = this.device(device);
    if (!requester) throw new ChainError("unknownDevice", `device ${device} is not a current, active device of this account`);
    const refuse = (reason: string) => new ChainError("notAccepted", `device ${device} may not control ${host}: ${reason}`, { device, host });
    if (!this.machine(host)) throw refuse("not a current host of this account");
    // As in Rust (§3.4 rule 10): while a renewal pause is open, the host's effective accept is its base minus the
    // pause's removals. A start never names a current admin (D1); a device made admin while paused stays paused until
    // the pause ends (a pause only ever narrows).
    if (this.pausedEdges(host).includes(device)) throw refuse("control is paused while identity keys are renewed");
    if (requester.admin) return;
    const policy = this.accept.get(host);
    if (!policy || policy.type === "admins") throw refuse("the host accepts control only from admin devices");
    if (policy.type === "all") return;
    if (!policy.ids.includes(device)) throw refuse("the host does not accept control from this device");
  }

  /** `authorizeDevice` and `mayControl`: what an agent checks before it lets a device in. */
  authorizeControl(device: string, dhKey: Uint8Array, host: string): void {
    if (device === host) throw new ChainError("selfControl", "a device does not control itself");
    this.authorizeDevice(device, dhKey);
    this.mayControl(device, host);
  }

  /** The X25519 key of an active *admin* device: the only senders whose sealed keys are accepted. */
  fullDeviceKey(id: string): Bytes {
    const d = this.admin(id);
    if (!d) throw new ChainError("unknownDevice", `device ${id} is not a current full device`);
    return d.info.dhKey;
  }

  /** Check one attestation against this view: only an admin attests. */
  async checkAttestation(att: Attestation): Promise<void> {
    if (att.account !== this.account) throw new ChainError("badAttestation", "another account");
    const device = this.admin(att.device);
    if (!device) throw new ChainError("badAttestation", "not a current full device");
    if (!(await verifyLabelled(device.info.signKey, CHAIN_ATTEST, attestationBody(att), att.signature))) throw new ChainError("badAttestation", "bad signature");
    const hash = this.hashAt(att.seq);
    if (!hash) throw new ChainError("behind", `seen up to ${att.seq}`, { seq: att.seq });
    if (!equal(hash, att.head)) throw new ChainError("fork", `the chain forked at entry ${att.seq}`, { seq: att.seq });
  }

  /**
   * Whether some admin device vouched for exactly this head within
   * `maxAgeMs` (Rust's `ChainState::freshness`; `operationMaxAgeMs` gives the
   * window for control or management): every other current admin counts, and
   * the requester's own attestation only when it is the sole active admin.
   * (M2's day of distrust after recoveries and removals is gone: a thief
   * skipped it with a puppet device while it locked the owner out; alerts
   * about every change to who is in the account replace it.)
   */
  async freshness(requester: string, attestations: Attestation[], nowMs: number, maxAgeMs: number): Promise<Freshness> {
    const sole = this.admins().length === 1;
    const selfCounts = sole;
    let newest: number | null = null;
    let behind: number | null = null;
    for (const att of attestations) {
      try {
        await this.checkAttestation(att);
      } catch (error) {
        if (error instanceof ChainError && error.kind === "behind") {
          behind = Math.max(behind ?? 0, att.seq);
          continue;
        }
        if (error instanceof ChainError && error.kind === "fork") throw error;
        continue;
      }
      const counts = att.device !== requester || selfCounts;
      if (!counts || att.seq !== this.seq || att.timeMs > nowMs + ATTESTATION_SKEW_MS) continue;
      newest = Math.max(newest ?? 0, att.timeMs);
    }
    if (behind !== null) return { type: "behind", seq: behind };
    if (newest !== null && nowMs - newest <= maxAgeMs) return { type: "fresh" };
    return { type: "stale", newestMs: newest };
  }

  /** The version the next entry for `action` has: the action's own, or — for the four both versions share — the chain's. */
  versionFor(action: Action): number {
    return fixedVersion(action) ?? (this.isV3 ? CHAIN_VERSION_3 : CHAIN_VERSION);
  }

  /** The next entry, unsigned (its version from `versionFor`). */
  draft(signer: Signer, action: Action, createdMs: number): Draft {
    return { version: this.versionFor(action), account: this.account, seq: this.seq + 1, prev: copy(this.head), createdMs, signer, action };
  }
}

function identity(d: { id: string; signKey: Bytes; dhKey: Bytes }): DeviceIdentity {
  return { id: d.id, signKey: d.signKey, dhKey: d.dhKey };
}

/** An active host: what v2 called a current machine. */
export function isCurrentHost(d: DeviceRecord): boolean {
  return d.status.type === "active" && d.host;
}

/** An active admin: what v2 called a current full device. */
export function isCurrentAdmin(d: DeviceRecord): boolean {
  return d.status.type === "active" && d.admin;
}

export async function createAttestation(state: ChainState, device: string, key: CryptoKey, timeMs: number): Promise<Attestation> {
  const body = { account: state.account, device, seq: state.seq, head: copy(state.head), timeMs };
  return { ...body, signature: await signLabelled(key, CHAIN_ATTEST, attestationBody(body)) };
}
