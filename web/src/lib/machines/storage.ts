/*
 * Where this browser keeps a machine's things: the `localStorage` and
 * `sessionStorage` keys of drafts, outboxes, dismissals and the like, and the
 * device copy's IndexedDB keys (`features/sessions/feed-persist.ts`).
 *
 * Before machines had keys every one of these named no machine — `rh.draft.
 * claude.<id>` — because the page had one. Now:
 *
 * - The local machine keeps exactly those keys. A local page is the same page
 *   it was, byte for byte in its storage, and an older build served by a
 *   rolled-back server still finds everything.
 * - Any other machine's are under `rh.m/<machine>/…` (`m/<machine>/…` in
 *   IndexedDB), so two machines' drafts for sessions that happen to share an
 *   id, or two machines' last folders, never meet.
 * - On droppen.app there is no local machine, and the keys from before were
 *   written for whichever machine was open at the time. Each is taken over by
 *   the first machine that asks for it ({@link claimMachineItem}): a session's
 *   draft by the machine of that session — the only one that ever asks for it
 *   — and a page-wide one (the last folder) by the machine open first after
 *   the upgrade. Taken over means moved: the old key is gone, so a draft
 *   cleared later cannot come back from it.
 * - A machine's key is only written after a claim of it, and a claim removes
 *   the old key, so finding both means a tab still on an older build — open
 *   across a deploy, or after a rollback — wrote the old key since. What it
 *   wrote is not dropped; a {@link ClaimResolver} says what the machine keeps.
 *   A page-wide value is written only through a claim, right before the
 *   write, so the old one is the newer and wins ({@link keepOld}). A draft or
 *   an outbox is claimed when its composer opens and then written straight to
 *   the machine's key, so either may be the newer: the newer draft revision
 *   wins (`newerSessionDraft`), and two outboxes are merged by message id
 *   (`mergeStoredOutboxes`).
 * - droppen.app is not rollback-safe for drafts and outboxes: an older build
 *   reads only the old keys, so what was moved to `rh.m/…` is out of its
 *   sight (until the newer build is back, and claims what the older one wrote
 *   meanwhile). The local page moves nothing, so a rollback there loses nothing.
 *
 * What belongs to this browser rather than a machine — the theme, the panel
 * widths, the hub account — keeps its key and does not come through here
 * (`DEVICE_KEYS` in `scripts/check-machine-keys.mjs`).
 */
import { LOCAL, type MachineKey } from "./key";
import { shellKind } from "./registry";

/** The slice of `Storage` used here. */
export interface KeyValueStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

/**
 * The prefix of a machine's browser state: an older build and a rollback read
 * it, so the keys keep their names.
 */
const LEGACY = "rh.";

/** The `localStorage`/`sessionStorage` key of `name` on `machine`: `rh.<name>` locally, `rh.m/<machine>/<name>` otherwise. */
export function machineStorageKey(machine: MachineKey, name: string): string {
  return machine === LOCAL ? `${LEGACY}${name}` : `${LEGACY}m/${encodeURIComponent(machine)}/${name}`;
}

/** Whether keys that name no machine are `machine`'s to take over: only where there is no local machine for them to be. */
function claims(machine: MachineKey): boolean {
  return machine !== LOCAL && shellKind() === "hub";
}

/**
 * The value a claim keeps when the machine's key and the key from before
 * machines had keys both hold one: given the machine's value and the old one.
 */
export type ClaimResolver = (mine: string, old: string) => string;

/**
 * The old value: right for anything only ever written through a claim — every
 * helper here, and `files-prefs.ts`. Each write claims first, which removes the
 * old key, so an old key that is there again was written after the last claim,
 * by a tab still on an older build, and is the newer of the two.
 */
export const keepOld: ClaimResolver = (_mine, old) => old;

/**
 * `machine`'s key for `name`, after moving a value kept under the key from
 * before machines had keys (`rh.<name>`) there, if this machine may take it
 * over. When the machine already has a value, `resolve` says which to keep
 * (the old one by default, {@link keepOld}); the old key goes either way.
 * Never throws: storage that cannot be read has nothing to move.
 */
export function claimMachineItem(
  storage: KeyValueStorage,
  machine: MachineKey,
  name: string,
  resolve: ClaimResolver = keepOld,
): string {
  const key = machineStorageKey(machine, name);
  if (!claims(machine)) return key;
  const legacy = `${LEGACY}${name}`;
  try {
    const old = storage.getItem(legacy);
    if (old === null) return key;
    const mine = storage.getItem(key);
    const kept = mine === null ? old : resolve(mine, old);
    if (kept !== mine) storage.setItem(key, kept);
    storage.removeItem(legacy);
  } catch {
    // blocked or full: the old key stays where it is
  }
  return key;
}

/** `name` on `machine`, taking over a value from before machines had keys (see {@link claimMachineItem}). */
export function readMachineItem(
  storage: KeyValueStorage,
  machine: MachineKey,
  name: string,
  resolve?: ClaimResolver,
): string | null {
  return storage.getItem(claimMachineItem(storage, machine, name, resolve));
}

export function writeMachineItem(storage: KeyValueStorage, machine: MachineKey, name: string, value: string): void {
  storage.setItem(claimMachineItem(storage, machine, name), value);
}

export function removeMachineItem(storage: KeyValueStorage, machine: MachineKey, name: string): void {
  storage.removeItem(claimMachineItem(storage, machine, name));
}

/** The device copy's IndexedDB key of `name` on `machine`: `name` itself locally, `m/<machine>/<name>` otherwise. */
export function machineCacheKey(machine: MachineKey, name: string): string {
  return machine === LOCAL ? name : `m/${encodeURIComponent(machine)}/${name}`;
}
