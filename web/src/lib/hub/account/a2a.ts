/*
 * Adding and managing an A2A proxy from this page (docs/a2a.md §7.2): what a
 * claim of its `DA1-` code is once checked, how the fingerprint a person pastes
 * from the proxy's log is compared, which devices that run AI must be updated
 * before one can be added, and what changing the devices that take a proxy's
 * calls wrote (who takes them is `chain-policy.ts`'s `a2aHostsOf`).
 */
import type { A2aEnrollCode, Bytes, ChainState } from "@/lib/e2e";
import type { MachineSummary } from "../api";
import type { DeliveryResult } from "../deliver";
import type { FreshUserKey } from "./views";

/**
 * An A2A proxy's claim of a code this page made, checked (`awaitA2aClaim`):
 * its proof verified under the A2A label over this account, the device id the
 * hub assigned and these keys, and the hub saw the code claimed once. Not
 * added yet: the fingerprint from the proxy's own log decides that
 * (`addA2aProxy`).
 */
export interface A2aClaim {
  code: A2aEnrollCode;
  device: string;
  signKey: Bytes;
  dhKey: Bytes;
}

/**
 * A device fingerprint as a person pastes it, made comparable: the letters and
 * digits of base32, upper case — whatever groups, spaces or a `fingerprint=`
 * from the log line came around them. `null` when it is no fingerprint at all.
 */
export function fingerprintDigits(text: string): string | null {
  const bare = text
    .trim()
    .replace(/^.*fingerprint\s*[=:]\s*/i, "")
    .toUpperCase()
    .replace(/[\s-]/g, "");
  return /^[A-Z2-7]{16}$/.test(bare) ? bare : null;
}

/** Whether `pasted` is `expected` (a `deviceFingerprint`, groups of four), read as {@link fingerprintDigits} reads it. */
export function sameFingerprint(pasted: string, expected: string): boolean {
  const a = fingerprintDigits(pasted);
  const b = fingerprintDigits(expected);
  return a !== null && a === b;
}

/**
 * Why an A2A proxy cannot be added yet, for availability only (docs/a2a.md
 * §7.1): a device that runs AI on a version that does not read the new entry
 * stops at it and falls behind on the chain. `hub`: the hub says nothing of
 * the devices' `a2a/1` (it does not take A2A proxies yet); `devices`: these
 * current hosts did not declare it; `null`: nothing in the way. The hub can
 * lie here, and nothing secure rests on it.
 */
export type A2aUpgradeGate = { kind: "hub" } | { kind: "devices"; hosts: string[] } | null;

export function a2aUpgradeGate(chain: Pick<ChainState, "hosts">, machines: readonly Pick<MachineSummary, "id" | "a2a">[] | null | undefined): A2aUpgradeGate {
  const hosts = chain
    .hosts()
    .filter((h) => h.status.type === "active")
    .map((h) => h.info.id);
  if (!hosts.length) return null;
  const said = new Map((machines ?? []).map((m) => [m.id, m.a2a]));
  if (hosts.every((h) => (said.get(h) ?? null) === null)) return { kind: "hub" };
  const behind = hosts.filter((h) => said.get(h) !== true).sort();
  return behind.length ? { kind: "devices", hosts: behind } : null;
}

/** What changing the hosts that take an A2A proxy's calls wrote (`setA2aHosts`). */
export interface A2aHostsChange {
  /** Hosts that take its calls now and did not: each written alone, with no key. */
  widened: string[];
  /** Hosts that no longer take them: each written with its next machine key epoch, handed to it. */
  narrowed: { host: string; mkEpoch?: number; result?: DeliveryResult; hostKeysTo?: string[]; userKey?: FreshUserKey }[];
}
