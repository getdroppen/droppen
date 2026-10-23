import type { KeyStore } from "@/lib/e2e";
import { CloudAccount, type SelfRemoval, wipeFailures } from "@/lib/hub/account";
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

/**
 * A hub session a screen holds without an open account (one that could not be
 * opened): ended on sign-out too. `account` and `store`: whose keys "sign out
 * and remove this browser" forgets there (`CloudAccount.wipeLocal`).
 */
export interface HeldSession {
  api: Pick<HubApi, "logout">;
  token: string;
  account?: string;
  store?: KeyStore;
}

/** How long a sign-out waits on the hub for each call (the removal, the logout): it goes on without it after that. */
export const SIGN_OUT_HUB_MS = 10_000;
/** How long it waits for the push subscription to go (the device's row is itself capped shorter). */
const SIGN_OUT_PUSH_CAP_MS = 6_000;

/** `work`, or a rejection once `ms` have passed without it settling. */
function capped<T>(work: Promise<T>, ms: number, what: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${what} did not answer within ${Math.round(ms / 1000)} s`)), ms);
  });
  return Promise.race([work, late]).finally(() => clearTimeout(timer));
}

/**
 * After "sign out and remove this browser": its keys are gone, but the
 * account's devices still list it (it could not take itself off, or taking
 * itself off failed) — for the sign-in screen to say so, once.
 */
export const SIGN_OUT_STILL_LISTED_KEY = "rh.cloud.signout-still-listed";

/** Why a removed browser is still listed: what it could not do ({@link SelfRemoval}), or `failed: <why>`. */
export type StillListed = Exclude<SelfRemoval, "removable"> | "unopened" | `failed: ${string}`;

/** Why the account still lists the browser that was just signed out and wiped (`null`: it does not, or it was taken). Read once. */
export function takeStillListed(storage: Storage | undefined = globalThis.sessionStorage): StillListed | null {
  try {
    const raw = storage?.getItem(SIGN_OUT_STILL_LISTED_KEY);
    if (raw === null || raw === undefined) return null;
    storage!.removeItem(SIGN_OUT_STILL_LISTED_KEY);
    return /^(notActive|notAdmin|lastAdmin|keepsSignIn|heldBack|unopened)$/.test(raw) || raw.startsWith("failed: ") ? (raw.slice(0, 240) as StillListed) : null;
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
  /** How long each hub call may take ({@link SIGN_OUT_HUB_MS}; tests make it short). */
  hubWithinMs?: number;
}

/**
 * End the hub session. Either way this browser's notifications stop and the
 * offline copies on it go — cleared and sealed, so a save still scheduled
 * cannot put any of it back, and their key forgotten. The device keys stay
 * (they are this browser's: signing in again opens it as before), unless
 * `removeBrowser` ({@link SignOutOptions}). The page is replaced only once all
 * of that has landed; whatever could not be cleared is kept for the sign-in
 * screen ({@link takeSignOutFailures}), and so is a removed browser the
 * account still lists ({@link takeStillListed}). The hub gets
 * {@link SIGN_OUT_HUB_MS} for each call; removing the browser wipes it in a
 * `finally` right after its removal is tried, before the logout, so a hub
 * that does not answer cannot keep the keys here.
 */
export async function signOut(from?: CloudAccount, held?: HeldSession, opts: SignOutOptions = {}): Promise<void> {
  const shell = hubShell();
  const account = shell?.account ?? from;
  const remove = opts.removeBrowser === true;
  const hubMs = opts.hubWithinMs ?? SIGN_OUT_HUB_MS;
  const failures: string[] = [];
  // First, while the devices are still reached: the subscription goes (its device's row, then the browser's own).
  await capped((opts.dropPush ?? (() => disablePushOnSignOut()))(), SIGN_OUT_PUSH_CAP_MS, "the push subscription").catch(() => undefined);
  // Every machine's copy stops following (nothing is saved after this), and its channel goes.
  shell?.stop();
  setRenewalRunner(null);
  let stillListed: StillListed | null = null;
  if (remove && account) {
    // Off the account's devices, while its session is still renewed: signed by itself, its last entry. The
    // link goes first, so the hub closing it for the removal is not taken for a removal by another device.
    shell?.link.stop();
    try {
      const why = account.selfRemoval();
      if (why === "removable") {
        try {
          await capped(account.removeSelf(), hubMs, "the hub");
        } catch (error) {
          stillListed = `failed: ${error instanceof Error ? error.message : String(error)}`;
        }
      } else if (account.standing === "active" || account.standing === "pending") stillListed = why;
    } finally {
      // Right after the attempt, whatever came of it, and before anything else waits on the hub: a page
      // closed from here on leaves no key behind.
      await account.wipe().catch((error: unknown) => failures.push(...wipeFailures(error).map((f) => `device keys: ${f}`)));
    }
  } else if (remove && held?.account && held.store) {
    // A screen that could not open the account: its keys, as this browser noted them, go all the same.
    await CloudAccount.wipeLocal(held.store, held.account).catch((error: unknown) => failures.push(...wipeFailures(error).map((f) => `device keys: ${f}`)));
    stillListed = "unopened";
  }
  account?.stopSession?.();
  // Which passkey this tab signed in with is the signed-in person's: not kept past them.
  forgetPasskeySignIn();
  // Sealed at once: nothing scheduled can write a copy back from here on.
  const clearing = clearDeviceCopies(account?.store ?? (remove ? held?.store : undefined));
  try {
    const api = shell?.api ?? account?.api ?? held?.api;
    // A screen with no account open (it could not open one) still holds the session it was opening with.
    const token = account ? account.session.token : held?.token;
    if (api && token) await capped(api.logout(token), hubMs, "the hub");
  } catch {
    // signed out locally either way (a browser that removed itself has no session left to end)
  }
  shell?.link.stop();
  clearSession();
  failures.unshift(...(await clearing));
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
