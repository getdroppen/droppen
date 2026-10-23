/*
 * The Web Locks a move holds, so no other tab of the origin writes what it
 * is copying while it copies it (or what it is writing while it writes it):
 * the move's own (`rh.hub.move`: one move per origin at a time), and the app's
 * — the device keys', the local wrap key's and each account's seen record's.
 * They are taken all at once or not at all (each `ifAvailable`, all let go
 * and tried again a moment later if one is held), so a tab holding one and
 * waiting for another can never hold the move up for good. A renewal of a
 * host's identity key running in another tab (its lock is per host) makes the
 * move wait, then say it is busy.
 */
import type { MoveLocks } from "./env";

export const MOVE_LOCK = "rh.hub.move";
/** The app's locks around the device keys and the local wrap key (`keystore.ts`, `keyring.ts`). */
export const DEVICE_KEYS_LOCK = "rh-device-keys";
export const LOCAL_WRAP_LOCK = "rh-local-wrap";
/** What the renewal of a host's identity key holds in a tab (`account.ts` `renewalLockName`). */
const RENEWAL_LOCK_PREFIX = "rh.hub.renewal.";

export const BUSY: unique symbol = Symbol("busy");

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** Every one of `names` or none, now (`BUSY` when one is held elsewhere). */
function tryAll<T>(locks: MoveLocks, names: readonly string[], work: () => Promise<T>): Promise<T | typeof BUSY> {
  const step = (i: number): Promise<T | typeof BUSY> =>
    i === names.length ? work() : locks.request(names[i]!, { mode: "exclusive", ifAvailable: true }, async (lock) => (lock ? step(i + 1) : BUSY));
  return step(0);
}

/**
 * Run `work` holding every lock in `names`, trying for up to `waitMs`;
 * `BUSY` when they could not all be had. No Web Locks: `work` runs as is.
 */
export async function holdLocks<T>(locks: MoveLocks | null, names: readonly string[], work: () => Promise<T>, waitMs: number): Promise<T | typeof BUSY> {
  if (!locks) return work();
  const until = Date.now() + waitMs;
  for (;;) {
    let ran = false;
    const out = await tryAll(locks, [...new Set(names)], () => {
      ran = true;
      return work();
    });
    if (ran || out !== BUSY) return out;
    if (Date.now() >= until) return BUSY;
    await sleep(150);
  }
}

/** Whether another tab is renewing a host's identity key now. */
export async function renewalRunning(locks: MoveLocks | null): Promise<boolean> {
  try {
    const state = await locks?.query?.();
    return !!state?.held?.some((l) => typeof l.name === "string" && l.name.startsWith(RENEWAL_LOCK_PREFIX));
  } catch {
    return false;
  }
}

/** Wait up to `waitMs` for no renewal to run: whether none does. */
export async function noRenewal(locks: MoveLocks | null, waitMs: number): Promise<boolean> {
  const until = Date.now() + waitMs;
  while (await renewalRunning(locks)) {
    if (Date.now() >= until) return false;
    await sleep(250);
  }
  return true;
}
