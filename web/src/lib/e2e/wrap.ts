/*
 * Key wrapping, browser side (`server-rs/crates/rh-proto/src/wrap.rs` — read
 * that for the why).
 *
 * - Sealed from one key pair to another, sender-authenticated (HPKE Auth-mode
 *   style): `ephemeral(32) ‖ nonce(12) ‖ ct`, with
 *   `kek = HKDF(salt = "rh/v1 seal-auth", ikm = X25519(e, R) ‖ X25519(S, R), info = e ‖ S ‖ R ‖ context)`.
 *   The recipient names the sender it expects; `openFromChain` accepts only a
 *   current admin ("full") device of the verified chain, so the hub cannot substitute a
 *   `UK` or `MK` of its own — and `openUserKey`, `openMachineKey`,
 *   `openFromRecovery` accept only the key the chain committed for its epoch
 *   (`ChainState.checkUserKey` / `checkMachineKey`), so neither can a device
 *   removed since, nor one that seals a key of its own choosing.
 * - Wrapped under a symmetric key (`UK` over `MK`, the recovery wrap key over
 *   `UK`): `nonce(12) ‖ ct`, `kek = HKDF(salt = "rh/v1 wrap-sym", ikm = K, info = context)`.
 * - The context is the AAD and part of the KDF input either way.
 * - Chain v3: a host's `MK` epoch sealed by an admin for a device that is not
 *   an admin (`sealHostKey` / `openHostKey`, purpose byte 3, mailed as
 *   `MAIL_HOST_KEY` with `encodeHostKeyMail`), accepted only as committed.
 * - `pairingTag`: the QR secret's MAC over a pairing delivery.
 *
 * Unwrapping goes through `unwrapKey`, so a secret lands in a non-extractable
 * `CryptoKey` without becoming a JS byte array. The exception is `openFrom`
 * (bytes), needed only to re-seal `UK` to a newly paired device: WebCrypto
 * wraps only extractable keys. Note what non-extractability does and does not
 * buy: a script running in this origin cannot *export* the device key, but it
 * can still *use* it — to unwrap anything sealed to this device. Protecting the
 * page's code is the delivery problem in the threat model, not something a
 * key flag solves.
 */
import { type Bytes, bytes, concat, copy, DecodeError, equal, Reader, utf8, Writer } from "./bytes";
import { type ChainError, type ChainState, MAX_ID_LEN } from "./chain";
import { edwardsToMontgomery } from "./field";
import { dh, exportPublic } from "./keys";
import type { NoiseKeyPair } from "./noise";
import { type MachineKey, machineKeyFrom } from "./record";

const AUTH_SALT = utf8("rh/v1 seal-auth");
const SYM_SALT = utf8("rh/v1 wrap-sym");
const PAIR_LABEL = utf8("rh/v1 pair\0");

export class WrapError extends Error {
  constructor(
    /**
     * `uncommitted`: the key opened but is not the one the chain committed for
     * its epoch (`cause` is the `ChainError`, kind `uncommittedKey` or `keyMismatch`).
     */
    readonly kind: "open" | "wrongRecipient" | "wrongAccount" | "unknownSender" | "uncommitted" | "wrongPurpose",
    message: string,
    options?: { cause?: unknown },
  ) {
    super(message, options);
    this.name = "WrapError";
  }
}

/**
 * `key`, if `check` (a chain commitment check such as
 * `(k) => chain.checkUserKey(epoch, k)`) passes; else it is zeroed and a
 * `WrapError` of kind `uncommitted` is thrown with the chain's error as `cause`.
 */
export async function requireCommitted(key: Bytes, check: (key: Bytes) => Promise<void>): Promise<Bytes> {
  try {
    await check(key);
  } catch (error) {
    key.fill(0);
    throw new WrapError("uncommitted", `the key is not the one the chain committed: ${(error as ChainError).message}`, { cause: error });
  }
  return key;
}

export type Purpose = "userKey" | "machineKey";

export interface SealContext {
  purpose: Purpose;
  account: string;
  sender: string;
  recipient: string;
  epoch: number;
}

export function encodeSealContext(c: SealContext): Bytes {
  return new Writer()
    .raw(utf8("rh/v1 seal\0"))
    .u8(c.purpose === "userKey" ? 1 : 2)
    .str16(c.account)
    .str16(c.sender)
    .str16(c.recipient)
    .u32(c.epoch)
    .finish();
}

export const context = {
  machineKeyUnderUserKey: (account: string, machine: string, mkEpoch: number, ukEpoch: number): Bytes =>
    new Writer().raw(utf8("rh/v1 MK under UK\0")).raw(utf8(account)).u8(0).raw(utf8(machine)).u8(0).u32(mkEpoch).u32(ukEpoch).finish(),
  userKeyUnderRecovery: (account: string, ukEpoch: number): Bytes =>
    new Writer().raw(utf8("rh/v1 UK under RK\0")).raw(utf8(account)).u8(0).u32(ukEpoch).finish(),
};

type KekUse = "encrypt" | "decrypt" | "unwrapKey";

async function authKek(dhE: Uint8Array, dhS: Uint8Array, eph: Uint8Array, sender: Uint8Array, recipient: Uint8Array, ctx: Uint8Array, use: KekUse[]): Promise<CryptoKey> {
  const ikmBytes = concat(dhE, dhS);
  try {
    const ikm = await crypto.subtle.importKey("raw", ikmBytes, "HKDF", false, ["deriveKey"]);
    return await crypto.subtle.deriveKey(
      { name: "HKDF", hash: "SHA-256", salt: AUTH_SALT, info: concat(eph, sender, recipient, ctx) },
      ikm,
      { name: "AES-GCM", length: 256 },
      false,
      use,
    );
  } finally {
    ikmBytes.fill(0);
  }
}

async function symKek(key: Uint8Array | CryptoKey, ctx: Uint8Array, use: KekUse[]): Promise<CryptoKey> {
  const ikm = key instanceof Uint8Array ? await crypto.subtle.importKey("raw", copy(key), "HKDF", false, ["deriveKey"]) : key;
  return crypto.subtle.deriveKey({ name: "HKDF", hash: "SHA-256", salt: SYM_SALT, info: copy(ctx) }, ikm, { name: "AES-GCM", length: 256 }, false, use);
}

function split(sealed: Uint8Array, prefix: number): [Bytes, Bytes] {
  if (sealed.length < prefix + 16) throw new DecodeError("a wrapped key is truncated");
  return [copy(sealed.subarray(0, prefix)), copy(sealed.subarray(prefix))];
}

/** Seal `secret` from `sender` to `recipient`. `ephemeral`/`nonce` are for test vectors only. */
export async function sealTo(
  sender: NoiseKeyPair,
  recipient: Uint8Array,
  ctx: SealContext,
  secret: Uint8Array,
  opts: { ephemeral?: NoiseKeyPair; nonce?: Uint8Array } = {},
): Promise<Bytes> {
  return sealRaw(sender, recipient, encodeSealContext(ctx), secret, opts);
}

async function sealRaw(sender: NoiseKeyPair, recipient: Uint8Array, c: Bytes, secret: Uint8Array, opts: { ephemeral?: NoiseKeyPair; nonce?: Uint8Array }): Promise<Bytes> {
  let eph = opts.ephemeral;
  if (!eph) {
    const pair = (await crypto.subtle.generateKey({ name: "X25519" }, false, ["deriveBits"])) as CryptoKeyPair;
    eph = { privateKey: pair.privateKey, publicKey: await exportPublic(pair.publicKey) };
  }
  const dhE = await dh(eph.privateKey, recipient);
  const dhS = await dh(sender.privateKey, recipient);
  const kek = await authKek(dhE, dhS, eph.publicKey, sender.publicKey, recipient, c, ["encrypt"]);
  dhE.fill(0);
  dhS.fill(0);
  const nonce = opts.nonce ? copy(opts.nonce) : crypto.getRandomValues(bytes(12));
  const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv: nonce, additionalData: c }, kek, copy(secret)));
  return concat(eph.publicKey, nonce, ct);
}

async function sealedKek(recipient: NoiseKeyPair, senderPublic: Uint8Array, c: Bytes, sealed: Uint8Array, use: KekUse[]) {
  const [head, ct] = split(sealed, 44);
  const eph = head.subarray(0, 32);
  const dhE = await dh(recipient.privateKey, eph);
  const dhS = await dh(recipient.privateKey, senderPublic);
  const kek = await authKek(dhE, dhS, eph, senderPublic, recipient.publicKey, c, use);
  dhE.fill(0);
  dhS.fill(0);
  return { kek, nonce: copy(head.subarray(32)), ct, aad: c };
}

const OPEN_FAILED = "the wrapped key does not open (wrong sender or recipient, wrong context, or tampered)";

async function openRaw(recipient: NoiseKeyPair, senderPublic: Uint8Array, c: Bytes, sealed: Uint8Array): Promise<Bytes> {
  const { kek, nonce, ct, aad } = await sealedKek(recipient, senderPublic, c, sealed, ["decrypt"]);
  try {
    return new Uint8Array(await crypto.subtle.decrypt({ name: "AES-GCM", iv: nonce, additionalData: aad }, kek, ct));
  } catch {
    throw new WrapError("open", OPEN_FAILED);
  }
}

/** Open to bytes: only for re-sealing `UK` to a new device. Prefer `unwrapSealed`. */
export async function openFrom(recipient: NoiseKeyPair, senderPublic: Uint8Array, ctx: SealContext, sealed: Uint8Array): Promise<Bytes> {
  return openRaw(recipient, senderPublic, encodeSealContext(ctx), sealed);
}

export type Target = { kind: "aes-gcm"; usages: KeyUsage[] } | { kind: "hkdf" };

async function unwrapWith(kek: CryptoKey, nonce: Bytes, ct: Bytes, aad: Bytes, target: Target): Promise<CryptoKey> {
  const params: AesGcmParams = { name: "AES-GCM", iv: nonce, additionalData: aad };
  try {
    switch (target.kind) {
      case "aes-gcm":
        return await crypto.subtle.unwrapKey("raw", ct, kek, params, { name: "AES-GCM" }, false, target.usages);
      case "hkdf":
        return await crypto.subtle.unwrapKey("raw", ct, kek, params, "HKDF", false, ["deriveKey", "deriveBits"]);
    }
  } catch {
    throw new WrapError("open", OPEN_FAILED);
  }
}

/** Unwrap a sealed secret from `senderPublic` straight into a non-extractable key. */
export async function unwrapSealed(recipient: NoiseKeyPair, senderPublic: Uint8Array, ctx: SealContext, sealed: Uint8Array, target: Target): Promise<CryptoKey> {
  const { kek, nonce, ct, aad } = await sealedKek(recipient, senderPublic, encodeSealContext(ctx), sealed, ["unwrapKey"]);
  return unwrapWith(kek, nonce, ct, aad, target);
}

/** The checks a recipient makes before trusting a delivery: addressed to `me`, from a current full device. */
export function senderFromChain(me: string, chain: ChainState, ctx: SealContext): Bytes {
  if (ctx.recipient !== me) throw new WrapError("wrongRecipient", `the delivery is for ${ctx.recipient}, not ${me}`);
  if (ctx.account !== chain.account) throw new WrapError("wrongAccount", "the delivery belongs to another account");
  try {
    return chain.fullDeviceKey(ctx.sender);
  } catch (error) {
    throw new WrapError("unknownSender", (error as Error).message);
  }
}

/** Open a delivery addressed to `me` (to bytes), only if its sender is a current full device of `chain`. */
export async function openFromChain(recipient: NoiseKeyPair, me: string, chain: ChainState, ctx: SealContext, sealed: Uint8Array): Promise<Bytes> {
  return openFrom(recipient, senderFromChain(me, chain, ctx), ctx, sealed);
}

/**
 * A `UK` epoch mailed to device `me` (purpose `userKey`): from a current full
 * device (`openFromChain`) and exactly the key the chain committed for the
 * epoch. Raw, to be wrapped or imported at once by the caller.
 */
export async function openUserKey(recipient: NoiseKeyPair, me: string, chain: ChainState, ctx: SealContext, sealed: Uint8Array): Promise<Bytes> {
  if (ctx.purpose !== "userKey") throw new WrapError("wrongPurpose", "the delivery is not a user key");
  const key = await openFromChain(recipient, me, chain, ctx, sealed);
  return requireCommitted(key, (k) => chain.checkUserKey(ctx.epoch, k));
}

/**
 * A machine key delivered to agent `me` (purpose `machineKey`): from a current
 * full device and exactly the key the chain committed for `me` at that epoch.
 * The commitment, not the sender, is what makes it the account's key: a device
 * removed since, or the hub replaying old mail, can hand over only a key the
 * chain already names — never one of its own, nor an epoch out of reach.
 */
export async function openMachineKey(recipient: NoiseKeyPair, me: string, chain: ChainState, ctx: SealContext, sealed: Uint8Array): Promise<Bytes> {
  if (ctx.purpose !== "machineKey") throw new WrapError("wrongPurpose", "the delivery is not a machine key");
  const key = await openFromChain(recipient, me, chain, ctx, sealed);
  return requireCommitted(key, (k) => chain.checkMachineKey(me, ctx.epoch, k));
}

/** Unwrap a delivery only if its sender is a current full device of `chain`. */
export async function unwrapFromChain(recipient: NoiseKeyPair, me: string, chain: ChainState, ctx: SealContext, sealed: Uint8Array, target: Target): Promise<CryptoKey> {
  return unwrapSealed(recipient, senderFromChain(me, chain, ctx), ctx, sealed, target);
}

/**
 * An `MK` delivery as the record layer's key pair, sender checked against the
 * chain. Unwrapped straight into a non-extractable key, so it cannot check the
 * chain's commitment: an agent uses `openMachineKey`.
 */
export async function unwrapMachineKeyFromChain(recipient: NoiseKeyPair, me: string, chain: ChainState, ctx: SealContext, sealed: Uint8Array): Promise<MachineKey> {
  return machineKeyFrom(await unwrapFromChain(recipient, me, chain, ctx, sealed, { kind: "hkdf" }));
}

/**
 * The seal-context purpose byte of a host's machine key sealed for *another*
 * device (chain v3: a device that is not an admin holds no `UK`, so an admin
 * hands it each host's committed `MK` epochs instead,
 * `docs/local-aggregator.md` §3.3).
 */
export const HOST_KEY_PURPOSE = 3;

/** Who sealed which host's machine key epoch for whom. */
export interface HostKeyContext {
  account: string;
  /** The sealing admin device. */
  sender: string;
  /** The device it is for. */
  recipient: string;
  /** The host whose `MK` this is. */
  host: string;
  epoch: number;
}

/**
 * `"rh/v1 seal\0" | 3 | str16 account | str16 sender | str16 recipient | u32 epoch | str16 host`:
 * `SealContext`'s layout with the host after it, under a purpose byte of its
 * own, so it can be neither a `UK` nor the host's own `MK` delivery.
 */
export function encodeHostKeyContext(c: HostKeyContext): Bytes {
  return new Writer()
    .raw(utf8("rh/v1 seal\0"))
    .u8(HOST_KEY_PURPOSE)
    .str16(c.account)
    .str16(c.sender)
    .str16(c.recipient)
    .u32(c.epoch)
    .str16(c.host)
    .finish();
}

/** Seal host `ctx.host`'s machine key epoch `ctx.epoch` from admin device `sender` to device `recipient` (its X25519 key). `ephemeral`/`nonce` are for test vectors only. */
export function sealHostKey(
  sender: NoiseKeyPair,
  recipient: Uint8Array,
  ctx: HostKeyContext,
  key: Uint8Array,
  opts: { ephemeral?: NoiseKeyPair; nonce?: Uint8Array } = {},
): Promise<Bytes> {
  return sealRaw(sender, recipient, encodeHostKeyContext(ctx), key, opts);
}

/**
 * Open a host's machine key sealed for device `me`: from a current *admin*
 * device of `chain`, and exactly the key the chain committed for that host
 * and epoch (`ChainState.checkMachineKey`) — so neither the hub nor a
 * non-admin (nor a device removed since) can plant a key of its own. Raw, to
 * be imported at once by the caller.
 */
export async function openHostKey(recipient: NoiseKeyPair, me: string, chain: ChainState, ctx: HostKeyContext, sealed: Uint8Array): Promise<Bytes> {
  if (ctx.recipient !== me) throw new WrapError("wrongRecipient", `the delivery is for ${ctx.recipient}, not ${me}`);
  if (ctx.account !== chain.account) throw new WrapError("wrongAccount", "the delivery belongs to another account");
  let sender: Bytes;
  try {
    sender = chain.fullDeviceKey(ctx.sender);
  } catch (error) {
    throw new WrapError("unknownSender", (error as Error).message, { cause: error });
  }
  const key = await openRaw(recipient, sender, encodeHostKeyContext(ctx), sealed);
  return requireCommitted(key, (k) => chain.checkMachineKey(ctx.host, ctx.epoch, k));
}

/** A host key as the mailbox carries it (purpose `MAIL_HOST_KEY`): `str16 host | sealed` — the mail row says the epoch, the blob which host. */
export function encodeHostKeyMail(host: string, sealed: Uint8Array): Bytes {
  return new Writer().str16(host).raw(sealed).finish();
}

/** The host and the sealed key of a `MAIL_HOST_KEY` mail. */
export function decodeHostKeyMail(data: Uint8Array): { host: string; sealed: Bytes } {
  const r = new Reader(data);
  const host = r.str16("host id", MAX_ID_LEN);
  const sealed = r.rest();
  if (sealed.length < 32 + 12 + 16) throw new DecodeError("truncated input");
  return { host, sealed };
}

/** The recipient a `UK` epoch is sealed to for the recovery package. */
export const RECOVERY_RECIPIENT = "recovery";

/**
 * Seal `UK` epoch `epoch` into the recovery package: from `sender` (device
 * `senderId`) to the X25519 twin of the chain's recovery key `rskPublic`
 * (`edwardsToMontgomery`). Only the holder of `RK` can open it.
 */
export async function sealForRecovery(sender: NoiseKeyPair, senderId: string, account: string, rskPublic: Uint8Array, epoch: number, userKey: Uint8Array): Promise<Bytes> {
  const recipient = edwardsToMontgomery(rskPublic);
  const ctx: SealContext = { purpose: "userKey", account, sender: senderId, recipient: RECOVERY_RECIPIENT, epoch };
  return sealTo(sender, recipient, ctx, userKey);
}

/**
 * A recovery-package `UK`, opened with `RK`'s keys: sealed by a device the
 * chain names — a current admin, or one demoted or removed since — and
 * exactly the key the chain committed for that epoch, as in Rust. The
 * commitment, not the sender, makes it the account's key: a device removed
 * since still holds old user keys and the public `RSK`, but it can seal only a
 * key the chain already names. A package an admin no longer current sealed can
 * be the only way back in for its epoch, so the hub keeps it and this opens it.
 */
export async function openFromRecovery(seal: NoiseKeyPair, chain: ChainState, ctx: SealContext, sealed: Uint8Array): Promise<Bytes> {
  if (ctx.recipient !== RECOVERY_RECIPIENT || ctx.purpose !== "userKey") throw new WrapError("wrongRecipient", `the delivery is for ${ctx.recipient}`);
  if (ctx.account !== chain.account) throw new WrapError("wrongAccount", "the delivery belongs to another account");
  const sender = chain.deviceRecord(ctx.sender);
  if (!sender) throw new WrapError("unknownSender", `device ${ctx.sender} is not in the account's chain`);
  const key = await openFrom(seal, sender.info.dhKey, ctx, sealed);
  return requireCommitted(key, (k) => chain.checkUserKey(ctx.epoch, k));
}

/** Wrap under a symmetric key. `nonce` is for test vectors only. */
export async function wrapUnder(key: Uint8Array | CryptoKey, ctx: Uint8Array, secret: Uint8Array, nonce: Uint8Array = crypto.getRandomValues(bytes(12))): Promise<Bytes> {
  const kek = await symKek(key, ctx, ["encrypt"]);
  const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv: copy(nonce), additionalData: copy(ctx) }, kek, copy(secret)));
  return concat(nonce, ct);
}

export async function unwrapUnder(key: Uint8Array | CryptoKey, ctx: Uint8Array, wrapped: Uint8Array): Promise<Bytes> {
  const [nonce, ct] = split(wrapped, 12);
  const kek = await symKek(key, ctx, ["decrypt"]);
  try {
    return new Uint8Array(await crypto.subtle.decrypt({ name: "AES-GCM", iv: nonce, additionalData: copy(ctx) }, kek, ct));
  } catch {
    throw new WrapError("open", "the wrapped key does not open (wrong key, wrong context, or tampered)");
  }
}

async function pairingMac(s: Uint8Array, fields: Uint8Array[]): Promise<Bytes> {
  const key = await crypto.subtle.importKey("raw", copy(s), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const w = new Writer().raw(PAIR_LABEL);
  for (const f of fields) w.u32(f.length).raw(f);
  return new Uint8Array(await crypto.subtle.sign("HMAC", key, w.finish()));
}

/** The QR secret `s`'s MAC over a pairing delivery: `HMAC(s, "rh/v1 pair\0" ‖ (u32 len ‖ field)*)`. */
export function pairingTag(s: Uint8Array, fields: Uint8Array[]): Promise<Bytes> {
  return pairingMac(s, fields);
}

export async function verifyPairingTag(s: Uint8Array, fields: Uint8Array[], tag: Uint8Array): Promise<boolean> {
  return equal(await pairingMac(s, fields), tag);
}
