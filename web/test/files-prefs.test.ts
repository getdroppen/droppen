// The file browser's two client-side decisions: which rows to draw, and which
// directory to open on.
//
// Both used to be nothing at all — every dot-file was listed, and the view
// always started at the home directory — so what is pinned here is the new
// behaviour *and* its escape hatches: a hidden directory stays reachable when
// it is asked for by path, and a remembered directory that has since gone away
// must not strand anybody on an error page.
import assert from "node:assert/strict";
import test from "node:test";
import {
  clearLastFilesDir,
  isHiddenEntryName,
  isRestorableDir,
  readLastFilesDir,
  readShowHiddenFiles,
  restoreFilesDir,
  shouldAbandonRestoredDir,
  SHOW_HIDDEN_KEY,
  visibleEntries,
  writeLastFilesDir,
  writeShowHiddenFiles,
  type PrefStorage,
  LAST_DIR_NAME,
} from "../src/features/files/files-prefs.js";
import { LOCAL } from "../src/lib/machines/key.js";
import { machineStorageKey } from "../src/lib/machines/storage.js";

class MemoryStorage implements PrefStorage {
  readonly values = new Map<string, string>();

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }

  removeItem(key: string): void {
    this.values.delete(key);
  }
}

const blocked: PrefStorage = {
  getItem: () => {
    throw new Error("blocked");
  },
  setItem: () => {
    throw new Error("blocked");
  },
  removeItem: () => {
    throw new Error("blocked");
  },
};

const listing = [
  { name: ".config" },
  { name: ".zshrc" },
  { name: "Projects" },
  { name: "notes.md" },
  { name: "archive.tar.gz" },
  { name: "..weird" },
];

test("dot-entries are dropped by default and kept when the switch is on", () => {
  assert.deepEqual(
    visibleEntries(listing, false).map((e) => e.name),
    ["Projects", "notes.md", "archive.tar.gz"],
  );
  // A dot inside the name is not a hidden file, and ".." leading is still one.
  assert.equal(isHiddenEntryName("archive.tar.gz"), false);
  assert.equal(isHiddenEntryName("..weird"), true);

  assert.equal(visibleEntries(listing, true), listing);
});

test("show-hidden defaults to off and survives a round trip", () => {
  const storage = new MemoryStorage();
  assert.equal(readShowHiddenFiles(storage), false);

  writeShowHiddenFiles(storage, true);
  assert.equal(storage.getItem(SHOW_HIDDEN_KEY), "1");
  assert.equal(readShowHiddenFiles(storage), true);

  writeShowHiddenFiles(storage, false);
  assert.equal(readShowHiddenFiles(storage), false);
});

test("unavailable storage falls back to the defaults without throwing", () => {
  assert.equal(readShowHiddenFiles(blocked), false);
  assert.doesNotThrow(() => writeShowHiddenFiles(blocked, true));
  assert.equal(readLastFilesDir(blocked, LOCAL), null);
  assert.doesNotThrow(() => writeLastFilesDir(blocked, LOCAL, "/root/notes"));
  assert.doesNotThrow(() => clearLastFilesDir(blocked, LOCAL));
});

test("only a path-shaped value is stored or restored", () => {
  assert.equal(isRestorableDir("/root/projects"), true);
  assert.equal(isRestorableDir("~"), true);
  assert.equal(isRestorableDir("~/projects"), true);
  assert.equal(isRestorableDir("projects"), false);
  assert.equal(isRestorableDir(""), false);

  const storage = new MemoryStorage();
  writeLastFilesDir(storage, LOCAL, "projects/relative");
  assert.equal(storage.values.has(machineStorageKey(LOCAL, LAST_DIR_NAME)), false);

  writeLastFilesDir(storage, LOCAL, "/root/projects");
  assert.equal(readLastFilesDir(storage, LOCAL), "/root/projects");

  // Corruption from an older build reads as "nothing remembered".
  storage.setItem(machineStorageKey(LOCAL, LAST_DIR_NAME), "not a path");
  assert.equal(readLastFilesDir(storage, LOCAL), null);

  clearLastFilesDir(storage, LOCAL);
  assert.equal(readLastFilesDir(storage, LOCAL), null);
});

test("the remembered directory is only used for a bare /files", () => {
  assert.equal(restoreFilesDir(undefined, "/root/projects"), "/root/projects");
  // A hidden directory is a legitimate place to be restored to.
  assert.equal(restoreFilesDir(undefined, "/root/.config"), "/root/.config");
  assert.equal(restoreFilesDir(undefined, null), null);
  assert.equal(restoreFilesDir(undefined, "garbage"), null);
  // An explicit ?path= is a destination the user asked for, restore or not.
  assert.equal(restoreFilesDir("/root/other", "/root/projects"), null);
  assert.equal(restoreFilesDir("/root/projects", "/root/projects"), null);
});

test("a restored directory that will not list is abandoned, a chosen one is not", () => {
  const restored = "/root/deleted";
  assert.equal(
    shouldAbandonRestoredDir({ isError: true, path: restored, restoredPath: restored }),
    true,
  );
  // Listing succeeded: stay put.
  assert.equal(
    shouldAbandonRestoredDir({ isError: false, path: restored, restoredPath: restored }),
    false,
  );
  // The user has since navigated somewhere else that fails — their error to see.
  assert.equal(
    shouldAbandonRestoredDir({ isError: true, path: "/root/typo", restoredPath: restored }),
    false,
  );
  // Nothing was restored: a failing path was typed or linked.
  assert.equal(
    shouldAbandonRestoredDir({ isError: true, path: "/root/typo", restoredPath: null }),
    false,
  );
  assert.equal(
    shouldAbandonRestoredDir({ isError: true, path: undefined, restoredPath: null }),
    false,
  );
});
