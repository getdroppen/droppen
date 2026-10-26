/**
 * The file browser's client-side preferences.
 *
 * Both of these describe this browser rather than the account — which directory
 * you were last looking at, and whether you want dot-files listed — so they
 * live in localStorage instead of the server-side `Prefs`. The last directory
 * is a path on one machine, so it is kept per machine; dot-files are this
 * browser's taste everywhere. Every decision here is a pure function over a
 * storage handle, so it can be tested without a DOM.
 */
import type { MachineKey } from "@/lib/machines/key";
import { claimMachineItem } from "@/lib/machines/storage";

export interface PrefStorage {
  getItem: (key: string) => string | null;
  setItem: (key: string, value: string) => void;
  removeItem: (key: string) => void;
}

/**
 * Whether entries whose name starts with "." are listed. Default off: a home
 * directory is mostly dot-directories, and they bury the files somebody
 * actually came to look at. Hiding is a rendering choice only — the directories
 * themselves stay reachable by typing the path or following a link.
 */
export const SHOW_HIDDEN_KEY = "rh.files.showHidden";

/**
 * The last directory listed on a machine, used only as an initial location.
 * Each mounted browser then owns its own path (route or pane), so panes never
 * steer one another. `rh.files.lastDir` on the local machine, as it always was.
 */
export const LAST_DIR_NAME = "files.lastDir";

const showHiddenListeners = new Set<() => void>();

function emitShowHidden(): void {
  for (const listener of showHiddenListeners) listener();
}

if (typeof window !== "undefined") {
  // `storage` fires in the *other* tabs, so this is how a second window learns
  // the switch moved; the tab that wrote it is told by emitShowHidden().
  window.addEventListener("storage", (event) => {
    if (event.key === null || event.key === SHOW_HIDDEN_KEY) emitShowHidden();
  });
}

/** Fires whenever `readShowHiddenFiles()` may have changed. */
export function subscribeShowHiddenFiles(onChange: () => void): () => void {
  showHiddenListeners.add(onChange);
  return () => {
    showHiddenListeners.delete(onChange);
  };
}

export function readShowHiddenFiles(storage: PrefStorage): boolean {
  try {
    return storage.getItem(SHOW_HIDDEN_KEY) === "1";
  } catch {
    // storage unavailable: the safe answer is the default, hidden
    return false;
  }
}

export function writeShowHiddenFiles(storage: PrefStorage, value: boolean): void {
  try {
    storage.setItem(SHOW_HIDDEN_KEY, value ? "1" : "0");
  } catch {
    // storage unavailable: the switch still holds for this page
  }
  emitShowHidden();
}

/** Unix's own rule, and the one every entry in the listing is named by. */
export function isHiddenEntryName(name: string): boolean {
  return name.startsWith(".");
}

/** The rows to draw. Returns the input untouched when nothing is filtered, so
 *  the common case adds no copy in front of the virtualizer. */
export function visibleEntries<T extends { name: string }>(
  entries: readonly T[],
  showHidden: boolean,
): readonly T[] {
  return showHidden ? entries : entries.filter((entry) => !isHiddenEntryName(entry.name));
}

/** A stored value worth navigating to. The API answers absolute paths and
 *  accepts "~"-relative ones; anything else is corruption or an old build. */
export function isRestorableDir(value: string): boolean {
  return value.startsWith("/") || value === "~" || value.startsWith("~/");
}

export function readLastFilesDir(storage: PrefStorage, machine: MachineKey): string | null {
  try {
    const raw = storage.getItem(claimMachineItem(storage, machine, LAST_DIR_NAME));
    return raw !== null && isRestorableDir(raw) ? raw : null;
  } catch {
    return null;
  }
}

export function writeLastFilesDir(storage: PrefStorage, machine: MachineKey, path: string): void {
  if (!isRestorableDir(path)) return;
  try {
    storage.setItem(claimMachineItem(storage, machine, LAST_DIR_NAME), path);
  } catch {
    // storage unavailable: this session simply will not be remembered
  }
}

export function clearLastFilesDir(storage: PrefStorage, machine: MachineKey): void {
  try {
    storage.removeItem(claimMachineItem(storage, machine, LAST_DIR_NAME));
  } catch {
    // storage unavailable: there is nothing stored to forget either
  }
}

/**
 * Where the file view should open, or null to stay at the default root.
 *
 * An explicit `?path=` is a deliberate destination — a shared link, the back
 * button, or the sidebar's own "Files" entry resetting to home — and always
 * wins. Only a bare `/files` mount falls back to the remembered directory.
 */
export function restoreFilesDir(
  searchPath: string | undefined,
  stored: string | null,
): string | null {
  if (searchPath !== undefined) return null;
  return stored !== null && isRestorableDir(stored) ? stored : null;
}

/**
 * Whether to give up on a restored directory and go back to the root.
 *
 * The remembered path may have been deleted, renamed or moved out of reach
 * since it was stored, and listing it then fails. Restoring it was our
 * decision rather than the user's, so we take it back: forget the value and
 * fall back to the default root instead of parking them on an error. A path
 * the user navigated to themselves is left alone — the error is the truth
 * about where they asked to go.
 */
export function shouldAbandonRestoredDir(args: {
  isError: boolean;
  path: string | undefined;
  restoredPath: string | null;
}): boolean {
  return args.isError && args.restoredPath !== null && args.path === args.restoredPath;
}
