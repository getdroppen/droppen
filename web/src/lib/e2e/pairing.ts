/*
 * Adding a second device (`droppen-proto/src/pairing.rs` has the why):
 *
 * - QR / pasted text: the new device's {@link Offer} — account, its id, its
 *   two public keys and a one-time secret `s` — as `RHP1-<base32>` (`DRP1-`
 *   is read too);
 * - numeric code: CPace (`pake.ts`); {@link PakeHello} from the existing
 *   device, {@link PakeAnswer} (with the new device's keys, under the
 *   responder's confirmation key) from the new one;
 * - either way the existing device signs "add device" and mails a
 *   {@link Delivery}: every `UK` epoch sealed to the new device, MAC'd with `s`
 *   (or the run's pairing secret). The new device opens it only against a
 *   verified chain that lists both of them ({@link openDelivery}).
 */
import { type Bytes, base32, concat, copy, DecodeError, equal, fromBase32, Reader, utf8, Writer } from "./bytes";
import { type ChainState, MAX_ID_LEN } from "./chain";
import { fingerprint } from "./keys";
import type { NoiseKeyPair } from "./noise";
import type { PakeKeys } from "./pake";
import { openFrom, pairingTag, requireCommitted, type SealContext, sealTo, verifyPairingTag } from "./wrap";

/** What {@link offerText} writes. */
export const OFFER_PREFIX = "RHP1-";
/** What {@link offerFromText} reads, in any case: {@link OFFER_PREFIX} and the newer `DRP1-`. */
export const OFFER_PREFIXES: readonly string[] = [OFFER_PREFIX, "DRP1-"];
const VERSION = 1;
const CI_LABEL = utf8("rh/v1 pair-ci\0");
const ANSWER_LABEL = utf8("rh/v1 pake answer\0");
const DELIVERY = utf8("delivery");
export const MAX_KEYS = 256;

export class PairingError extends Error {
  constructor(
    readonly kind: "badOffer" | "badTag" | "wrongAccount" | "fingerprint" | "sender" | "notListed" | "badAnswer",
    message: string,
  ) {
    super(message);
    this.name = "PairingError";
  }
}

export interface Offer {
  account: string;
  device: string;
  signKey: Bytes;
  dhKey: Bytes;
  secret: Bytes;
}

export function encodeOffer(o: Offer): Bytes {
  return new Writer().u8(VERSION).str16(o.account).str16(o.device).raw(o.signKey).raw(o.dhKey).raw(o.secret).finish();
}

export function offerText(o: Offer): string {
  return OFFER_PREFIX + base32(encodeOffer(o));
}

export function offerFromText(text: string): Offer {
  const t = text.replace(/\s+/g, "");
  const prefix = OFFER_PREFIXES.find((p) => t.slice(0, p.length).toUpperCase() === p);
  const body = prefix === undefined ? null : fromBase32(t.slice(prefix.length));
  if (!body) throw new PairingError("badOffer", "not a pairing code");
  try {
    const r = new Reader(body);
    if (r.u8() !== VERSION) throw new PairingError("badOffer", "not a pairing code");
    const offer = { account: r.str16("account id", MAX_ID_LEN), device: r.str16("device id", MAX_ID_LEN), signKey: r.bytes(32), dhKey: r.bytes(32), secret: r.bytes(32) };
    r.finish();
    return offer;
  } catch (error) {
    if (error instanceof PairingError) throw error;
    throw new PairingError("badOffer", "not a pairing code");
  }
}

/** The CPace channel identifier for pairing `device` into `account`. */
export function channelId(account: string, device: string): Bytes {
  return concat(CI_LABEL, utf8(account), Uint8Array.of(0), utf8(device));
}

export interface PakeHello {
  sid: Bytes;
  share: Bytes;
}

export function encodePakeHello(h: PakeHello): Bytes {
  return concat(h.sid, h.share);
}

export function decodePakeHello(data: Uint8Array): PakeHello {
  const r = new Reader(data);
  const h = { sid: r.bytes(16), share: r.bytes(32) };
  r.finish();
  return h;
}

export interface PakeAnswer {
  share: Bytes;
  device: string;
  signKey: Bytes;
  dhKey: Bytes;
  tag: Bytes;
}

function answerBody(hello: PakeHello, share: Uint8Array, device: string, signKey: Uint8Array, dhKey: Uint8Array): Bytes {
  return new Writer().raw(ANSWER_LABEL).raw(hello.sid).raw(hello.share).raw(share).str16(device).raw(signKey).raw(dhKey).finish();
}

async function hmac(key: Uint8Array, data: Uint8Array): Promise<Bytes> {
  const k = await crypto.subtle.importKey("raw", copy(key), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return new Uint8Array(await crypto.subtle.sign("HMAC", k, copy(data)));
}

/** The new device's answer to `hello` after finishing its run. */
export async function pakeAnswer(hello: PakeHello, share: Uint8Array, device: string, signKey: Uint8Array, dhKey: Uint8Array, keys: PakeKeys): Promise<PakeAnswer> {
  return { share: copy(share), device, signKey: copy(signKey), dhKey: copy(dhKey), tag: await hmac(keys.confirmResponder, answerBody(hello, share, device, signKey, dhKey)) };
}

/** The existing device's check that the same code made the answer. */
export async function verifyPakeAnswer(answer: PakeAnswer, hello: PakeHello, keys: PakeKeys): Promise<void> {
  const want = await hmac(keys.confirmResponder, answerBody(hello, answer.share, answer.device, answer.signKey, answer.dhKey));
  if (!equal(want, answer.tag)) throw new PairingError("badAnswer", "the answer was not made with the same code");
}

export function encodePakeAnswer(a: PakeAnswer): Bytes {
  return new Writer().raw(a.share).str16(a.device).raw(a.signKey).raw(a.dhKey).raw(a.tag).finish();
}

export function decodePakeAnswer(data: Uint8Array): PakeAnswer {
  const r = new Reader(data);
  const a = { share: r.bytes(32), device: r.str16("device id", MAX_ID_LEN), signKey: r.bytes(32), dhKey: r.bytes(32), tag: r.bytes(32) };
  r.finish();
  return a;
}

export interface Delivery {
  account: string;
  fingerprint: Bytes;
  sender: string;
  senderSign: Bytes;
  senderDh: Bytes;
  keys: { epoch: number; sealed: Bytes }[];
  tag: Bytes;
}

function deliveryBody(d: Omit<Delivery, "tag">): Bytes {
  if (d.keys.length > MAX_KEYS) throw new DecodeError("too many delivered keys");
  const w = new Writer().u8(VERSION).str16(d.account).raw(d.fingerprint).str16(d.sender).raw(d.senderSign).raw(d.senderDh).u16(d.keys.length);
  for (const k of d.keys) w.u32(k.epoch).u16(k.sealed.length).raw(k.sealed);
  return w.finish();
}

/**
 * Seal every `UK` epoch in `userKeys` from `sender` (a full device of `chain`,
 * with Noise key pair `senderDh`) to `recipient`, and MAC it with `secret`.
 * `opts` fixes randomness for test vectors only.
 */
export async function createDelivery(
  chain: ChainState,
  sender: string,
  senderSign: Uint8Array,
  senderDh: NoiseKeyPair,
  recipient: string,
  recipientDh: Uint8Array,
  userKeys: { epoch: number; key: Uint8Array }[],
  secret: Uint8Array,
): Promise<Delivery> {
  const keys = [];
  for (const { epoch, key } of userKeys) {
    const ctx: SealContext = { purpose: "userKey", account: chain.account, sender, recipient, epoch };
    keys.push({ epoch, sealed: await sealTo(senderDh, recipientDh, ctx, key) });
  }
  const body = { account: chain.account, fingerprint: await fingerprint(chain.accountKey), sender, senderSign: copy(senderSign), senderDh: copy(senderDh.publicKey), keys };
  return { ...body, tag: await pairingTag(secret, [DELIVERY, deliveryBody(body)]) };
}

export function encodeDelivery(d: Delivery): Bytes {
  return concat(deliveryBody(d), d.tag);
}

export function decodeDelivery(data: Uint8Array): Delivery {
  const r = new Reader(data);
  if (r.u8() !== VERSION) throw new DecodeError("pairing delivery version");
  const account = r.str16("account id", MAX_ID_LEN);
  const fp = r.bytes(16);
  const sender = r.str16("device id", MAX_ID_LEN);
  const senderSign = r.bytes(32);
  const senderDh = r.bytes(32);
  const count = r.u16();
  if (count > MAX_KEYS) throw new DecodeError("too many delivered keys");
  const keys = [];
  for (let i = 0; i < count; i++) {
    const epoch = r.u32();
    keys.push({ epoch, sealed: r.bytes(r.u16()) });
  }
  const tag = r.bytes(32);
  r.finish();
  return { account, fingerprint: fp, sender, senderSign, senderDh, keys, tag };
}

/** The MAC alone: whoever made it knew `secret`. */
export async function checkDeliveryTag(d: Delivery, secret: Uint8Array): Promise<void> {
  if (!(await verifyPairingTag(secret, [DELIVERY, deliveryBody(d)], d.tag))) throw new PairingError("badTag", "the delivery was not made by whoever saw this device's code");
}

/**
 * What the new device checks before it says "paired", and the `UK` epochs it
 * was handed (raw, to be wrapped at once by the caller): the MAC, then the
 * verified `chain` (account, the account key's fingerprint, a current full
 * sender with the delivery's keys, this device listed with its own keys),
 * then each sealed key, which must be the one the chain committed for its
 * epoch (a `WrapError` of kind `uncommitted` if not).
 */
export async function openDelivery(d: Delivery, secret: Uint8Array, chain: ChainState, me: string, meSign: Uint8Array, meDh: NoiseKeyPair): Promise<{ epoch: number; key: Bytes }[]> {
  await checkDeliveryTag(d, secret);
  if (chain.account !== d.account) throw new PairingError("wrongAccount", "the delivery is for another account than the chain the hub serves");
  if (!equal(await fingerprint(chain.accountKey), d.fingerprint)) throw new PairingError("fingerprint", "the account key does not have the fingerprint the delivery names");
  const sender = chain.admin(d.sender);
  if (!sender || !equal(sender.info.signKey, d.senderSign) || !equal(sender.info.dhKey, d.senderDh)) {
    throw new PairingError("sender", "the sender is not a current admin (full) device with the keys the delivery names");
  }
  const mine = chain.device(me);
  if (!mine || !equal(mine.info.signKey, meSign) || !equal(mine.info.dhKey, meDh.publicKey)) throw new PairingError("notListed", "the chain does not list this device with its own keys (yet)");
  const out = [];
  for (const { epoch, sealed } of d.keys) {
    const ctx: SealContext = { purpose: "userKey", account: d.account, sender: d.sender, recipient: me, epoch };
    // Exactly the key the chain committed for the epoch (else WrapError "uncommitted").
    const key = await requireCommitted(await openFrom(meDh, d.senderDh, ctx, sealed), (k) => chain.checkUserKey(epoch, k));
    out.push({ epoch, key });
  }
  return out;
}
