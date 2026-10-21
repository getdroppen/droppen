import type { KeyStore } from "@/lib/e2e";
import type { CloudAccount } from "@/lib/hub/account";
import { clearSession, type HubApi } from "@/lib/hub/api";
import { deviceMirrorStore, forgetMirrorKey } from "@/lib/hub/mirror";
import { forgetPasskeySignIn } from "@/lib/hub/passkey-join";
import { setRenewalRunner } from "@/lib/hub/renewal";
import { cacheWipe } from "@/lib/local-cache";
import { SIGN_IN_PATH } from "@/features/landing/route";
import { hubShell } from "./hub-shell";

/** Where a sign-out that could not clear everything leaves the list of what, for the sign-in screen. */
export const SIGN_OUT_FAILURES_KEY = "rh.cloud.signout-failures";

/** What failed, as one short line: `offline copies: …` or `offline copy key: …`. */
function failure(what: string, error: unknown): string {
  const detail = error instanceof Error ? error.message : String(error);
  return `${what}: ${detail}`.slice(0, 200);
}

/**
 * Clear what this device keeps of the account's machines: the offline copies
 * (the whole device cache — mirror copies and feed copies — emptied and
 * sealed, so a save still scheduled cannot put any of it back) and the mirror
 * key that sealed them, in `store` and in the default device store
 * (`deviceMirrorStore()`) if that is another. Every clear is waited for,
 * however long it takes. Returns what failed; empty when all of it went.
 */
export async function clearDeviceCopies(store?: KeyStore): Promise<string[]> {
  const stores = [...new Set([store, deviceMirrorStore()].filter((s): s is KeyStore => s !== undefined))];
  const results = await Promise.allSettled([cacheWipe(), ...stores.map((s) => forgetMirrorKey(s))]);
  const failures: string[] = [];
  results.forEach((result, i) => {
    if (result.status === "rejected") failures.push(failure(i === 0 ? "offline copies" : "offline copy key", result.reason));
  });
  return failures;
}

function rememberFailures(failures: string[], storage: Storage | undefined = globalThis.sessionStorage): void {
  try {
    storage?.setItem(SIGN_OUT_FAILURES_KEY, JSON.stringify(failures.slice(0, 8)));
  } catch (error) {
    console.warn("cloud: sign-out could not clear:", failures, error);
  }
}

/**
 * What the last sign-out in this tab could not clear (`null`: nothing failed,
 * or it was already taken). Read once: it is removed as it is read.
 */
export function takeSignOutFailures(storage: Storage | undefined = globalThis.sessionStorage): string[] | null {
  try {
    const raw = storage?.getItem(SIGN_OUT_FAILURES_KEY);
    if (raw === null || raw === undefined) return null;
    storage!.removeItem(SIGN_OUT_FAILURES_KEY);
    const list: unknown = JSON.parse(raw);
    const out = Array.isArray(list) ? list.filter((x): x is string => typeof x === "string").slice(0, 8) : [];
    return out.length ? out : null;
  } catch {
    return null;
  }
}

/** A hub session a screen holds without an open account (one that could not be opened): ended on sign-out too. */
export interface HeldSession {
  api: Pick<HubApi, "logout">;
  token: string;
}

/**
 * End the hub session (the device keys stay: they are this browser's). The
 * offline copies on this device go with it — cleared and sealed, so a save
 * still scheduled cannot put any of it back, and their key forgotten — and
 * the page is replaced only once all of that has landed. Whatever could not
 * be cleared is kept for the sign-in screen ({@link takeSignOutFailures}).
 */
export async function signOut(from?: CloudAccount, held?: HeldSession): Promise<void> {
  const shell = hubShell();
  const account = shell?.account ?? from;
  // Every machine's copy stops following (nothing is saved after this), and its channel goes.
  shell?.stop();
  setRenewalRunner(null);
  account?.stopSession?.();
  // Which passkey this tab signed in with is the signed-in person's: not kept past them.
  forgetPasskeySignIn();
  // Sealed at once: nothing scheduled can write a copy back from here on.
  const clearing = clearDeviceCopies(account?.store);
  try {
    const api = shell?.api ?? account?.api ?? held?.api;
    // A screen with no account open (it could not open one) still holds the session it was opening with.
    const token = account ? account.session.token : held?.token;
    if (api && token) await api.logout(token);
  } catch {
    // signed out locally either way
  }
  shell?.link.stop();
  clearSession();
  const failures = await clearing;
  if (failures.length) {
    console.warn("cloud: sign-out could not clear:", failures);
    rememberFailures(failures);
  }
  // The sign-in page, which also shows what could not be cleared.
  location.replace(SIGN_IN_PATH);
}

/**
 * Where the pairing screen was when it asked to sign in again (the tab:
 * `qr` or `code`), for the sign-in screen to say why and the pairing screen
 * to open on once this browser is back (`sessionStorage`: this tab only).
 */
export const PAIRING_RESUME_KEY = "rh.cloud.pairing-resume";

export type PairingResume = "qr" | "code";

/** The pairing screen's tab to come back to after a sign-in, if it asked for one (not taken: see {@link clearPairingResume}). */
export function pairingResume(storage: Storage | undefined = globalThis.sessionStorage): PairingResume | null {
  try {
    const v = storage?.getItem(PAIRING_RESUME_KEY);
    return v === "qr" || v === "code" ? v : null;
  } catch {
    return null;
  }
}

export function clearPairingResume(storage: Storage | undefined = globalThis.sessionStorage): void {
  try {
    storage?.removeItem(PAIRING_RESUME_KEY);
  } catch {
    // nothing kept
  }
}

/**
 * "Sign in again" from the pairing screen: this browser's device never
 * joined the chain and may no longer open a numeric code (it signed in too
 * long ago), so the next sign-in registers it afresh — a new device, a new
 * window — and the pairing screen opens on `mode` again. Then an ordinary
 * sign-out (`signOut`). A device the chain names by then keeps its id
 * (`CloudAccount.registerAfreshOnSignIn`).
 */
export async function signInAgainToPair(
  account: CloudAccount,
  mode: PairingResume,
  opts: { storage?: Storage; signOut?: (account: CloudAccount) => Promise<void> } = {},
): Promise<void> {
  account.registerAfreshOnSignIn();
  try {
    (opts.storage ?? globalThis.sessionStorage)?.setItem(PAIRING_RESUME_KEY, mode);
  } catch {
    // the sign-in screen says nothing, and the pairing screen opens on its QR code
  }
  await (opts.signOut ?? signOut)(account);
}
