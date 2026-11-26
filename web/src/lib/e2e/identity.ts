/*
 * Enrollment codes, device side (`droppen-proto/src/identity.rs` — read that for
 * the why). `RH1-<account fingerprint>-<secret>` (`DR1-` is read too, in any
 * case; `RH1-` is what is written): the device generates the 128-bit secret
 * and shows the code; the hub only ever learns
 * `token = HMAC(secret, "rh/v1 enroll-token")[..16]`; the agent proves it holds
 * the code with an HMAC over the account, machine id, its keys and the token,
 * and the device checks that proof before it signs "add machine".
 */
import { type Bytes, base32, bytes, copy, equal, fromBase32, utf8, Writer } from "./bytes";

export interface EnrollCode {
  accountFingerprint: Bytes;
  secret: Bytes;
}

export function generateEnrollCode(accountFingerprint: Uint8Array): EnrollCode {
  return { accountFingerprint: copy(accountFingerprint), secret: crypto.getRandomValues(bytes(16)) };
}

export function formatEnrollCode(c: EnrollCode): string {
  return `RH1-${base32(c.accountFingerprint)}-${base32(c.secret)}`;
}

/** The version tags {@link parseEnrollCode} reads, in any case; the first is what {@link formatEnrollCode} writes. */
export const ENROLL_CODE_VERSIONS: readonly string[] = ["RH1", "DR1"];

export function parseEnrollCode(text: string): EnrollCode {
  const parts = text.trim().split("-");
  if (parts.length !== 3) throw new Error("an enrollment code looks like <version>-<fingerprint>-<secret>");
  if (!ENROLL_CODE_VERSIONS.includes(parts[0]!.toUpperCase())) throw new Error("not an enrollment code of a version this app understands");
  const fp = fromBase32(parts[1]!);
  const secret = fromBase32(parts[2]!);
  if (fp?.length !== 16 || secret?.length !== 16) throw new Error("an enrollment code's parts are 16 bytes of base32 each");
  return { accountFingerprint: fp, secret };
}

async function hmac(secret: Uint8Array, data: Uint8Array): Promise<Bytes> {
  const key = await crypto.subtle.importKey("raw", copy(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return new Uint8Array(await crypto.subtle.sign("HMAC", key, copy(data)));
}

/** What the device registers with the hub, which never sees the secret. */
export async function enrollToken(c: EnrollCode): Promise<Bytes> {
  return (await hmac(c.secret, utf8("rh/v1 enroll-token"))).slice(0, 16);
}

export async function enrollProof(c: EnrollCode, account: string, machine: string, signKey: Uint8Array, dhKey: Uint8Array): Promise<Bytes> {
  const body = new Writer()
    .raw(utf8("rh/v1 enroll-proof\0"))
    .str16(account)
    .str16(machine)
    .raw(signKey)
    .raw(dhKey)
    .raw(await enrollToken(c))
    .finish();
  return hmac(c.secret, body);
}

/** The device's check before it signs "add machine" with exactly these keys. */
export async function verifyEnrollProof(c: EnrollCode, account: string, machine: string, signKey: Uint8Array, dhKey: Uint8Array, proof: Uint8Array): Promise<boolean> {
  return equal(await enrollProof(c, account, machine, signKey, dhKey), proof);
}
