import type { KeyStore } from "@/lib/e2e";
import type { CloudAccount, SelfRemoval } from "@/lib/hub/account";
import { clearSession, type HubApi } from "@/lib/hub/api";
import { deviceMirrorStore, forgetMirrorKey } from "@/lib/hub/mirror";
import { forgetPasskeySignIn } from "@/lib/hub/passkey-join";
import { setRenewalRunner } from "@/lib/hub/renewal";
import { cacheWipe } from "@/lib/local-cache";
import { disablePushOnSignOut } from "@/lib/push";
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
 * After "sign out and remove this browser": its keys are gone, but the
 * account's devices still list it (it could not take itself off, or taking
 * itself off failed) — for the sign-in screen to say so, once.
 */
export const SIGN_OUT_STILL_LISTED_KEY = "rh.cloud.signout-still-listed";

/** Why a removed browser is still listed: what it could not do ({@link SelfRemoval}), or `failed: <why>`. */
export type StillListed = Exclude<SelfRemoval, "removable"> | `failed: ${string}`;

/** Why the account still lists the browser that was just signed out and wiped (`null`: it does not, or it was taken). Read once. */
export function takeStillListed(storage: Storage | undefined = globalThis.sessionStorage): StillListed | null {
  try {
    const raw = storage?.getItem(SIGN_OUT_STILL_LISTED_KEY);
    if (raw === null || raw === undefined) return null;
    storage!.removeItem(SIGN_OUT_STILL_LISTED_KEY);
    return /^(notActive|notAdmin|lastAdmin|keepsSignIn|heldBack)$/.test(raw) || raw.startsWith("failed: ") ? (raw.slice(0, 240) as StillListed) : null;
  } catch {
    return null;
  }
}

export interface SignOutOptions {
  /**
   * "Sign out and remove this browser" (a shared or borrowed computer): it
   * takes itself out of the account's devices where it may
   * (`CloudAccount.selfRemoval`), then forgets its device keys, user keys and
   * everything else it kept for the account (`CloudAccount.wipe`), whether it
   * could take itself out or not. Without it (the default: a computer of
   * one's own) the device keys stay, and signing in again opens it as before.
   */
  removeBrowser?: boolean;
  /** Stops this browser's notifications ({@link disablePushOnSignOut}): stands in for the page's in tests. */
  dropPush?: () => Promise<void>;
}

/**
 * End the hub session. Either way this browser's notifications stop and the
 * offline copies on it go — cleared and sealed, so a save still scheduled
 * cannot put any of it back, and their key forgotten. The device keys stay
 * (they are this browser's: signing in again opens it as before), unless
 * `removeBrowser` ({@link SignOutOptions}). The page is replaced only once all
 * of that has landed; whatever could not be cleared is kept for the sign-in
 * screen ({@link takeSignOutFailures}), and so is a removed browser the
 * account still lists ({@link takeStillListed}).
 */
export async function signOut(from?: CloudAccount, held?: HeldSession, opts: SignOutOptions = {}): Promise<void> {
  const shell = hubShell();
  const account = shell?.account ?? from;
  const remove = opts.removeBrowser === true;
  // First, while the devices are still reached: the subscription goes (its device's row, then the browser's own).
  await (opts.dropPush ?? (() => disablePushOnSignOut()))().catch(() => undefined);
  // Every machine's copy stops following (nothing is saved after this), and its channel goes.
  shell?.stop();
  setRenewalRunner(null);
  let stillListed: StillListed | null = null;
  if (remove && account) {
    // Off the account's devices, while its session is still renewed: signed by itself, its last entry. The
    // link goes first, so the hub closing it for the removal is not taken for a removal by another device.
    shell?.link.stop();
    const why = account.selfRemoval();
    if (why === "removable") {
      try {
        await account.removeSelf();
      } catch (error) {
        stillListed = `failed: ${error instanceof Error ? error.message : String(error)}`;
      }
    } else if (account.standing === "active" || account.standing === "pending") stillListed = why;
  }
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
    // signed out locally either way (a browser that removed itself has no session left to end)
  }
  shell?.link.stop();
  clearSession();
  const failures = await clearing;
  if (remove && account) {
    try {
      await account.wipe();
    } catch (error) {
      failures.push(failure("device keys", error));
    }
  }
  if (failures.length) {
    console.warn("cloud: sign-out could not clear:", failures);
    rememberFailures(failures);
  }
  if (stillListed) {
    try {
      globalThis.sessionStorage?.setItem(SIGN_OUT_STILL_LISTED_KEY, stillListed);
    } catch {
      // the sign-in screen does not say it
    }
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
