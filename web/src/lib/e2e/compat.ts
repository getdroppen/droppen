/*
 * Compatibility mode: the end-to-end encryption on a browser whose WebCrypto
 * lacks X25519 and/or Ed25519 — and nothing else (`compatCovers`) — with those
 * two done in JavaScript (`soft25519.ts`). Only once the person, told what it
 * risks, agreed to it on this browser; remembered per browser profile (the
 * device keys are this browser's, whatever account signs in). Decided once
 * per page, before any key is read or made (`CloudApp`).
 */
import { checkE2eSupport, compatCovers, type E2eSupport, SOFT_ALGORITHMS, type SoftAlgorithm } from "./support";

/** Where the agreement is kept (`localStorage`). */
export const COMPAT_CONSENT_KEY = "droppen.e2e-compat";

type KeptIn = Pick<Storage, "getItem" | "setItem" | "removeItem">;

/** The person agreed to compatibility mode on this browser, when, and what it lacked then. */
export interface CompatConsent {
  at: number;
  missing: string[];
}

/** Agreed in this page, whether or not the browser kept it (a storage that refuses writes). */
let consentedHere: CompatConsent | null = null;

/** The agreement on this browser to compatibility mode, if any. */
export function compatConsent(storage: KeptIn | undefined = globalThis.localStorage): CompatConsent | null {
  try {
    const raw = storage?.getItem(COMPAT_CONSENT_KEY);
    if (raw) {
      const v = JSON.parse(raw) as Partial<CompatConsent> | null;
      if (v && typeof v.at === "number" && Array.isArray(v.missing)) return { at: v.at, missing: v.missing.filter((m): m is string => typeof m === "string") };
    }
  } catch {
    // Unreadable: as if never agreed, unless this page was told.
  }
  return consentedHere;
}

/**
 * Whether the person agreed to compatibility mode on this browser for all of
 * `missing`: agreed while it lacked Ed25519 only, a browser that now lacks
 * X25519 too is asked again.
 */
export function compatAgreedFor(missing: readonly string[], storage: KeptIn | undefined = globalThis.localStorage): boolean {
  const consent = compatConsent(storage);
  return consent !== null && missing.every((m) => consent.missing.includes(m));
}

/** The person agreed to compatibility mode on this browser (lacking `missing`): kept for the next pages. */
export function consentToCompat(missing: string[], storage: KeptIn | undefined = globalThis.localStorage, now: () => number = Date.now): void {
  consentedHere = { at: now(), missing: [...missing] };
  try {
    storage?.setItem(COMPAT_CONSENT_KEY, JSON.stringify(consentedHere));
  } catch {
    console.warn("compat: this browser would not keep the agreement to compatibility mode; it is asked again on the next page");
  }
}

/** Forget the agreement (the person withdrew it): the next page asks again before any key is used. */
export function forgetCompatConsent(storage: KeptIn | undefined = globalThis.localStorage): void {
  consentedHere = null;
  try {
    storage?.removeItem(COMPAT_CONSENT_KEY);
  } catch {
    // Nothing kept.
  }
}

let soft: SoftAlgorithm[] = [];

/** The algorithms this page does in JavaScript: none unless in compatibility mode. */
export function compatAlgorithms(): readonly SoftAlgorithm[] {
  return soft;
}

/** Whether this page runs in compatibility mode (its device's private keys in the page's memory). */
export function compatModeActive(): boolean {
  return soft.length > 0;
}

/** Why compatibility mode did not start: its code could not be fetched (offline, the first time). An i18n key. */
export const COMPAT_LOAD_FAILED = "compatibility mode could not be loaded: connect to the internet, then reload the page";

/** Do `algorithms` in JavaScript from now on, in this page (the code is loaded only here). */
export async function enterCompatMode(algorithms: readonly SoftAlgorithm[]): Promise<void> {
  let loaded: typeof import("./soft25519");
  try {
    loaded = await import("./soft25519");
  } catch (error) {
    console.warn("compat: loading compatibility mode failed:", error);
    throw new Error(COMPAT_LOAD_FAILED);
  }
  loaded.installSoftCurves(algorithms);
  soft = SOFT_ALGORITHMS.filter((a) => soft.includes(a) || algorithms.includes(a));
}

/**
 * Whether the page may go on to the keys: natively; in compatibility mode —
 * entered here, when this browser agreed to it, and then held to the same
 * check — or not, with what is missing and whether compatibility mode could
 * stand in (`compat`: ask the person).
 */
export type E2eReadiness = { kind: "native" } | { kind: "compat"; algorithms: readonly SoftAlgorithm[] } | { kind: "unsupported"; missing: string[]; compat: boolean };

export async function e2eReadiness(
  support: E2eSupport,
  { storage = globalThis.localStorage, check = () => checkE2eSupport() }: { storage?: KeptIn; check?: () => Promise<E2eSupport> } = {},
): Promise<E2eReadiness> {
  if (support.ok) return { kind: "native" };
  if (!compatCovers(support)) return { kind: "unsupported", missing: support.missing, compat: false };
  if (!compatAgreedFor(support.missing, storage)) return { kind: "unsupported", missing: support.missing, compat: true };
  await enterCompatMode(support.missing as SoftAlgorithm[]);
  const again = await check();
  if (!again.ok) return { kind: "unsupported", missing: again.missing, compat: false };
  return { kind: "compat", algorithms: soft };
}
