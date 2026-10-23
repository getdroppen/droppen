import type { KeyStore } from "@/lib/e2e";
import type { CloudAccount } from "@/lib/hub/account";
import { clearSession, type HubApi, type Proof } from "@/lib/hub/api";
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

/**
 * Forget everything this browser holds for `account`: its keys
 * (`CloudAccount.wipe`: user keys, the cached keyring, the device keys) and
 * every offline copy with the key that sealed them (`clearDeviceCopies`).
 * Every clear is waited for; what failed comes back, to be shown. What a
 * removed device does, and a device whose account was deleted.
 */
export async function wipeAccountHere(account: Pick<CloudAccount, "wipe">, store?: KeyStore): Promise<string[]> {
  const failures: string[] = [];
  const [wiped, copies] = await Promise.allSettled([account.wipe(), clearDeviceCopies(store)]);
  if (wiped.status === "rejected") failures.push(failure("keys", wiped.reason));
  if (copies.status === "fulfilled") failures.push(...copies.value);
  else failures.push(failure("offline copies", copies.reason));
  return failures;
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

// ------------------------------------------------------------ deleting the account

/** Where this tab says the account it was in has just been deleted, for the sign-in screen (once). */
export const ACCOUNT_DELETED_KEY = "rh.cloud.account-deleted";

let deleting = false;

/**
 * Whether this page is deleting its account right now: the hub ending the
 * account's sessions and sockets meanwhile is that deletion, not a refusal
 * of this device to show (`cloud-app.tsx`).
 */
export function deletingAccount(): boolean {
  return deleting;
}

/**
 * Delete the account at the hub (`DELETE /v1/account`: `confirm`, the
 * account's e-mail address as typed, and proof it is still the person), then
 * leave it here ({@link leaveDeletedAccount}). A refusal changes nothing
 * here, and is thrown. `leave` stands in for the page's leaving (tests).
 */
export async function deleteAccount(
  account: CloudAccount,
  input: { confirm: string; proof: Proof; email: string | null },
  leave: (account: CloudAccount, email: string | null) => Promise<void> = leaveDeletedAccount,
): Promise<void> {
  deleting = true;
  try {
    await account.api.deleteAccount(account.session.token, input.confirm.trim(), input.proof);
  } catch (error) {
    deleting = false;
    throw error;
  }
  await leave(account, input.email);
}

/**
 * The account was deleted at the hub: forget it in this browser — its keys
 * and every offline copy, as a removed device does ({@link wipeAccountHere}),
 * the session, the passkey this tab signed in with and the known-client
 * token kept for its address — and leave as a sign-out does, for the sign-in
 * screen, which says the account was deleted (and what could not be
 * cleared). Nothing is asked of the hub: it ended every session already.
 */
export async function leaveDeletedAccount(account: CloudAccount, email: string | null): Promise<void> {
  const shell = hubShell();
  shell?.stop();
  setRenewalRunner(null);
  account.stopSession();
  shell?.link.stop();
  forgetPasskeySignIn();
  clearSession();
  if (email) account.api.forgetKnownClient(email);
  const failures = await wipeAccountHere(account, account.store);
  if (failures.length) {
    console.warn("cloud: the deleted account could not be cleared from this browser:", failures);
    rememberFailures(failures);
  }
  markAccountDeleted();
  location.replace(SIGN_IN_PATH);
}

/** Say on this tab's next sign-in screen that its account was deleted. */
export function markAccountDeleted(storage: Storage | undefined = globalThis.sessionStorage): void {
  try {
    storage?.setItem(ACCOUNT_DELETED_KEY, "1");
  } catch {
    // the sign-in screen just says nothing of it
  }
}

/** Whether this tab's account was just deleted (read once: it is removed as it is read). */
export function takeAccountDeleted(storage: Storage | undefined = globalThis.sessionStorage): boolean {
  try {
    if (!storage || storage.getItem(ACCOUNT_DELETED_KEY) === null) return false;
    storage.removeItem(ACCOUNT_DELETED_KEY);
    return true;
  } catch {
    return false;
  }
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
