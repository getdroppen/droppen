import { indexedDbKeyStore, type KeyStore } from "@/lib/e2e";
import { CloudAccount, wipeFailures } from "@/lib/hub/account";
import { HubApi, type HubSession, saveSession } from "@/lib/hub/api";
import { type DeviceHere, notedDevice } from "@/lib/hub/device-here";
import type { RemoveCase } from "@/features/cloud/sign-out-dialog";
import { type HeldSession, signOut, type SignOutResult } from "@/features/cloud/sign-out";
import { forgetMoveUnfinished, moveUnfinished } from "@/features/move";

/*
 * "Remove this browser" on the move notice (`legacy.tsx`): the app's own
 * "sign out and remove this browser" (`features/cloud/sign-out.ts`), on the
 * hub's origin, for the device this browser keeps there. The account is
 * opened with that device's key — a device sign-in, so no password and no
 * passkey are asked — so that it can take itself off the account's devices
 * where it may; whatever it may not, its keys and everything else it kept go
 * from this origin all the same.
 *
 * Except after a move of this device that did not finish (`localOnly`): the
 * app host may be using the same device, so only this origin's copy goes and
 * the account is not opened at all.
 */

/** The removal, ready to confirm: what it can do here ({@link RemoveCase}), for the account it opened or could not. */
export interface Removal {
  account?: CloudAccount;
  held?: HeldSession;
  why: RemoveCase;
  /** Whether the account has recovery words (the chain's current key): the last full device's warning says so. */
  recoveryWords: boolean;
  /** Further accounts this browser noted a device for: their keys go too (`CloudAccount.wipeLocal`). */
  others: string[];
  store: KeyStore;
  /**
   * A move of this very device did not finish (`features/move`
   * `moveUnfinished`): the app host may be using it (`sent`), or is (`verified`).
   * Then only this origin's copy goes — the device is never taken off the
   * account, which would remove it from under the app host.
   */
  localOnly?: "sent" | "verified";
}

/** Open the account of the device `here` names, to remove it; `unopened` when that cannot be done. Never throws. */
export async function prepareRemoval(here: DeviceHere, origin: string, opts: { api?: HubApi; store?: KeyStore } = {}): Promise<Removal> {
  const api = opts.api ?? new HubApi();
  const store = opts.store ?? indexedDbKeyStore();
  const [account = null, ...others] = here.accounts;
  const device = account ? (notedDevice(account) ?? (here.session?.account === account ? (here.session.device ?? null) : null)) : null;
  const unfinished = moveUnfinished();
  if (account && device && unfinished?.account === account && unfinished.device === device) {
    // The same device may be live on the app host: nothing is opened (nothing at the hub is touched but this origin's session).
    return { held: { api, token: here.session?.token ?? "", account, store }, why: "unopened", recoveryWords: false, others, store, localOnly: unfinished.state };
  }
  if (account && device) {
    // The kept session if it is this device's (it may have run out: it is renewed with the device key), else none.
    const session: HubSession =
      here.session?.account === account && here.session.device === device ? here.session : { account, token: "", expiresMs: 0, device };
    try {
      const opened = await CloudAccount.open(api, session, store, { origin, onSession: saveSession });
      return { account: opened, why: opened.selfRemoval(), recoveryWords: !!opened.chain?.currentRecoveryKey, others, store };
    } catch (error) {
      console.warn("move notice: could not open the account to remove this browser:", error);
    }
  }
  return { held: { api, token: here.session?.token ?? "", account: account ?? undefined, store }, why: "unopened", recoveryWords: false, others, store };
}

/** Remove this browser ({@link Removal}): what could not be cleared, and why the account still lists it, if it does. */
export async function removeHere(removal: Removal): Promise<SignOutResult> {
  const result = await new Promise<SignOutResult>((resolve) => {
    signOut(removal.account, removal.held, { removeBrowser: true, finish: resolve }).catch((error: unknown) =>
      resolve({ failures: [error instanceof Error ? error.message : String(error)], stillListed: null }),
    );
  });
  // Keys this browser kept with no account noted, and those of any other account it noted.
  const rest = removal.account || removal.held?.account ? removal.others : ["", ...removal.others];
  for (const account of rest) {
    await CloudAccount.wipeLocal(removal.store, account).catch((error: unknown) => result.failures.push(...wipeFailures(error).map((f) => `device keys: ${f}`)));
  }
  if (removal.localOnly) {
    // Only this copy went, by design: the account still lists the device because the app host may be using it.
    forgetMoveUnfinished();
    return { ...result, stillListed: null };
  }
  return result;
}
