/*
 * Enrollment codes, device side (`droppen-proto/src/identity.rs` — read that for
 * the why). `RH1-<account fingerprint>-<secret>` (`DR1-` is read too, in any
 * case; `RH1-` is what is written): the device generates the 128-bit secret
 * and shows the code; the hub only ever learns
 * `token = HMAC(secret, "rh/v1 enroll-token")[..16]`; the agent proves it holds
 * the code with an HMAC over the account, machine id, its keys and the token,
 * and the device checks that proof before it signs "add machine".
 *
 * `DA1-<account fingerprint>-<secret>` (`A2aEnrollCode`) joins an A2A proxy
 * (`docs/a2a.md` §7.2) the same way, with the same token, and a proof under
 * its own label, `"rh/v1 enroll-proof a2a\0"`: a proof made for one kind of
 * code never verifies as the other's, and each reader takes only its own
 * version.
 */
import { type Bytes, base32, bytes, copy, equal, fromBase32, utf8, Writer } from "./bytes";

export interface EnrollCode {
  accountFingerprint: Bytes;
  secret: Bytes;
}

/**
 * An A2A proxy's code (`DA1-`, Rust's `A2aEnrollCode`). `kind` keeps it apart
 * from an {@link EnrollCode} by type: what the page adds for it — an A2A proxy,
 * never a device that runs AI — follows from the code it made, not from what
 * the hub says the enrollment is.
 */
export interface A2aEnrollCode {
  kind: "a2a";
  accountFingerprint: Bytes;
  secret: Bytes;
}

const PROOF_LABEL = "rh/v1 enroll-proof\0";
/** The proof label of an A2A proxy's code (Rust's `A2A_PROOF_LABEL`). */
const A2A_PROOF_LABEL = "rh/v1 enroll-proof a2a\0";

export function generateEnrollCode(accountFingerprint: Uint8Array): EnrollCode {
  return { accountFingerprint: copy(accountFingerprint), secret: crypto.getRandomValues(bytes(16)) };
}

export function formatEnrollCode(c: EnrollCode): string {
  return `RH1-${base32(c.accountFingerprint)}-${base32(c.secret)}`;
}

/** The version tags {@link parseEnrollCode} reads, in any case; the first is what {@link formatEnrollCode} writes. */
export const ENROLL_CODE_VERSIONS: readonly string[] = ["RH1", "DR1"];

/** The version tag an A2A proxy's code starts with ({@link parseA2aEnrollCode} reads it in any case). */
export const A2A_CODE_VERSIONS: readonly string[] = ["DA1"];

/** `<version>-<fingerprint>-<secret>` with one of `versions` (any case), as its two 16-byte parts. */
function parseCode(text: string, versions: readonly string[]): { accountFingerprint: Bytes; secret: Bytes } {
  const parts = text.trim().split("-");
  if (parts.length !== 3) throw new Error("an enrollment code looks like <version>-<fingerprint>-<secret>");
  const version = parts[0]!.toUpperCase();
  if (!versions.includes(version)) {
    if (A2A_CODE_VERSIONS.includes(version)) throw new Error("a DA1 code joins an A2A proxy, not a device that runs AI");
    if (ENROLL_CODE_VERSIONS.includes(version)) throw new Error("an RH1 code adds a device that runs AI; an A2A proxy joins with a DA1 code");
    throw new Error("not an enrollment code of a version this app understands");
  }
  const fp = fromBase32(parts[1]!);
  const secret = fromBase32(parts[2]!);
  if (fp?.length !== 16 || secret?.length !== 16) throw new Error("an enrollment code's parts are 16 bytes of base32 each");
  return { accountFingerprint: fp, secret };
}

/** `RH1-` or `DR1-` only: a `DA1-` code is an A2A proxy's. */
export function parseEnrollCode(text: string): EnrollCode {
  return parseCode(text, ENROLL_CODE_VERSIONS);
}

/** A fresh code that joins an A2A proxy to the account whose key has fingerprint `accountFingerprint`. */
export function generateA2aEnrollCode(accountFingerprint: Uint8Array): A2aEnrollCode {
  return { kind: "a2a", accountFingerprint: copy(accountFingerprint), secret: crypto.getRandomValues(bytes(16)) };
}

export function formatA2aEnrollCode(c: A2aEnrollCode): string {
  return `DA1-${base32(c.accountFingerprint)}-${base32(c.secret)}`;
}

/** `DA1-` only: an `RH1-` or `DR1-` code adds a device that runs AI. */
export function parseA2aEnrollCode(text: string): A2aEnrollCode {
  return { kind: "a2a", ...parseCode(text, A2A_CODE_VERSIONS) };
}

async function hmac(secret: Uint8Array, data: Uint8Array): Promise<Bytes> {
  const key = await crypto.subtle.importKey("raw", copy(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return new Uint8Array(await crypto.subtle.sign("HMAC", key, copy(data)));
}

/** What the device registers with the hub, which never sees the secret (the same derivation for both kinds of code). */
export async function enrollToken(c: Pick<EnrollCode, "secret">): Promise<Bytes> {
  return (await hmac(c.secret, utf8("rh/v1 enroll-token"))).slice(0, 16);
}

/** `HMAC(secret, label ‖ str16 account ‖ str16 id ‖ sign key ‖ dh key ‖ token)`. */
async function proofMac(c: Pick<EnrollCode, "secret">, label: string, account: string, id: string, signKey: Uint8Array, dhKey: Uint8Array): Promise<Bytes> {
  const body = new Writer()
    .raw(utf8(label))
    .str16(account)
    .str16(id)
    .raw(signKey)
    .raw(dhKey)
    .raw(await enrollToken(c))
    .finish();
  return hmac(c.secret, body);
}

export async function enrollProof(c: EnrollCode, account: string, machine: string, signKey: Uint8Array, dhKey: Uint8Array): Promise<Bytes> {
  return proofMac(c, PROOF_LABEL, account, machine, signKey, dhKey);
}

/** The device's check before it signs "add machine" with exactly these keys. */
export async function verifyEnrollProof(c: EnrollCode, account: string, machine: string, signKey: Uint8Array, dhKey: Uint8Array, proof: Uint8Array): Promise<boolean> {
  return equal(await enrollProof(c, account, machine, signKey, dhKey), proof);
}

/** The proxy's proof that it holds `c`, bound to the device id the hub assigned and the keys it joins with. */
export async function a2aEnrollProof(c: A2aEnrollCode, account: string, device: string, signKey: Uint8Array, dhKey: Uint8Array): Promise<Bytes> {
  return proofMac(c, A2A_PROOF_LABEL, account, device, signKey, dhKey);
}

/** The page's check of that proof before it signs "add this A2A proxy" with exactly these keys. */
export async function verifyA2aEnrollProof(c: A2aEnrollCode, account: string, device: string, signKey: Uint8Array, dhKey: Uint8Array, proof: Uint8Array): Promise<boolean> {
  return equal(await a2aEnrollProof(c, account, device, signKey, dhKey), proof);
}
