// Signing out of cloud mode (`features/cloud/sign-out.ts`, `lib/local-cache.ts`):
// every clear — the device cache holding the offline copies, the mirror key
// that sealed them — is waited for however long it takes (no deadline lets the
// page move on while one is still running), the cache is sealed at once so a
// save still scheduled cannot put anything back, and whatever could not be
// cleared is not swallowed: it is kept in sessionStorage for the sign-in
// screen, which takes it once.
import assert from "node:assert/strict";
import test from "node:test";
import { type KeyStore, MemoryKeyStore } from "../src/lib/e2e/index.js";
import { deviceMirrorStore, MIRROR_KEY_NAME, mirrorSeal, streamRecordKey } from "../src/lib/hub/mirror.js";
import { cacheSealed, cacheSet, cacheUnseal, cacheWipe } from "../src/lib/local-cache.js";
import type { HubShell } from "../src/features/cloud/hub-shell.js";
import { setHubShell } from "../src/features/machines/shells.js";
import { clearDeviceCopies, SIGN_OUT_FAILURES_KEY, signOut, takeSignOutFailures } from "../src/features/cloud/sign-out.js";

// The default mirror store is memory here: made before IndexedDB is faked.
const defaultStore = deviceMirrorStore();

/** Just enough of IndexedDB for `local-cache.ts`: one database, one store, transactions that complete when told. */
class FakeIdb {
  data = new Map<string, unknown>();
  /** A clear fails with this. */
  failClear: string | null = null;
  /** A clear lands only once this resolves. */
  clearGate: Promise<void> = Promise.resolve();
  clears = 0;

  open(_name: string, _version: number) {
    const req: Record<string, unknown> & { onsuccess?: (() => void) | null } = { result: null, onsuccess: null, onerror: null, onblocked: null, onupgradeneeded: null };
    const idb = this;
    const database = {
      objectStoreNames: { contains: () => true },
      createObjectStore() {},
      transaction(_store: string, mode: string) {
        const tx: { oncomplete: (() => void) | null; onerror: (() => void) | null; onabort: (() => void) | null; error: Error | null; objectStore(): unknown } = {
          oncomplete: null,
          onerror: null,
          onabort: null,
          error: null,
          objectStore: () => store,
        };
        const work: (() => void)[] = [];
        let clearing = false;
        const request = <T,>(read: () => T) => {
          const r: { result: T | undefined; onsuccess: (() => void) | null } = { result: undefined, onsuccess: null };
          work.push(() => {
            r.result = read();
            r.onsuccess?.();
          });
          return r;
        };
        const store = {
          get: (key: string) => request(() => idb.data.get(key)),
          getAllKeys: () => request(() => [...idb.data.keys()]),
          put: (value: unknown, key: string) => {
            if (mode !== "readwrite") throw new Error("read-only");
            work.push(() => void idb.data.set(key, value));
          },
          delete: (key: string) => work.push(() => void idb.data.delete(key)),
          clear: () => {
            clearing = true;
            work.push(() => {
              idb.clears++;
              if (idb.failClear) throw new Error(idb.failClear);
              idb.data.clear();
            });
          },
        };
        setTimeout(async () => {
          if (clearing) await idb.clearGate;
          try {
            while (work.length) work.shift()!();
            tx.oncomplete?.();
          } catch (error) {
            tx.error = error as Error;
            tx.onerror?.();
            tx.onabort?.();
          }
        }, 0);
        return tx;
      },
    };
    queueMicrotask(() => {
      req.result = database;
      req.onsuccess?.();
    });
    return req;
  }
}

const idb = new FakeIdb();
(globalThis as { indexedDB?: unknown }).indexedDB = idb;

class FakeStorage {
  readonly values = new Map<string, string>();
  getItem(key: string) {
    return this.values.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    this.values.set(key, value);
  }
  removeItem(key: string) {
    this.values.delete(key);
  }
}

/** A key store whose mirror key is there to forget. */
async function storeWithKey(): Promise<MemoryKeyStore> {
  const store = new MemoryKeyStore();
  await mirrorSeal(store).seal({ account: "a-1", machine: "m-1", key: streamRecordKey("a-1", "m-1", "ab") }, { n: 1 });
  assert.ok(await store.get(MIRROR_KEY_NAME));
  return store;
}

test("clearing the device copies waits for the clear however long it takes, seals the cache, and forgets the mirror key", async () => {
  cacheUnseal();
  idb.data.set("stream/a-1/m-1/ab", { v: 2 });
  idb.data.set("feeds", ["claude/x"]);
  const store = await storeWithKey();
  await mirrorSeal(defaultStore).seal({ account: "a-1", machine: "m-1", key: "k" }, 1);
  assert.ok(await defaultStore.get(MIRROR_KEY_NAME));

  let release!: () => void;
  idb.clearGate = new Promise((resolve) => (release = resolve));
  let done = false;
  const clearing = clearDeviceCopies(store).then((failures) => {
    done = true;
    return failures;
  });
  assert.equal(cacheSealed(), true, "sealed at once");
  await cacheSet("stream/a-1/m-1/cd", { late: true });
  // Well past the old one-second cap: still waiting, as it should.
  await new Promise((resolve) => setTimeout(resolve, 1300));
  assert.equal(done, false, "nothing moves on while the clear is still running");
  release();
  assert.deepEqual(await clearing, []);
  assert.equal(idb.data.size, 0, "the cache is empty");
  assert.equal(await store.get(MIRROR_KEY_NAME), undefined, "the given store's mirror key is gone");
  assert.equal(await defaultStore.get(MIRROR_KEY_NAME), undefined, "and the default store's");

  // Still sealed: a save scheduled before the sign-out cannot put anything back.
  await cacheSet("stream/a-1/m-1/ef", { late: true });
  assert.equal(idb.data.size, 0);
  // Nor is a new mirror key made for it.
  await assert.rejects(mirrorSeal(store).seal({ account: "a-1", machine: "m-1", key: "k" }, 1));
  assert.equal(await store.get(MIRROR_KEY_NAME), undefined);
  idb.clearGate = Promise.resolve();
});

test("a clear that fails is reported, not swallowed", async () => {
  idb.failClear = "QuotaExceededError: the disk is full";
  try {
    await assert.rejects(cacheWipe(), /could not be cleared: QuotaExceededError/);
    const failing: KeyStore = {
      get: async () => undefined,
      put: async () => undefined,
      delete: async () => {
        throw new Error("the key store is gone");
      },
    };
    const failures = await clearDeviceCopies(failing);
    assert.equal(failures.length, 2);
    assert.match(failures[0]!, /^offline copies: .*QuotaExceededError/);
    assert.match(failures[1]!, /^offline copy key: the key store is gone/);
  } finally {
    idb.failClear = null;
  }
});

test("sign-out keeps what it could not clear for the sign-in screen, and replaces the page only after every clear", async () => {
  const session = new FakeStorage();
  const g = globalThis as { sessionStorage?: unknown; location?: unknown };
  g.sessionStorage = session;
  const replaced: string[] = [];
  let clearsAtReplace = -1;
  g.location = {
    replace: (url: string) => {
      replaced.push(url);
      clearsAtReplace = idb.clears;
    },
  };
  const calls: string[] = [];
  const store = await (async () => {
    cacheUnseal();
    return storeWithKey();
  })();
  // droppen.app's shell: every machine's copy stops with it.
  const shell = {
    api: { logout: async () => void calls.push("logout") },
    account: { session: { token: "t" }, store },
    link: { stop: () => void calls.push("link.stop") },
    stop: () => void calls.push("shell.stop"),
  } as unknown as HubShell;

  // All of it clears: nothing kept.
  setHubShell(shell);
  const before = idb.clears;
  await signOut();
  // To the sign-in page (not the landing page at `/`), which shows what could not be cleared.
  assert.deepEqual(replaced, ["/signin"]);
  assert.equal(clearsAtReplace, before + 1, "the cache was cleared before the page was replaced");
  assert.deepEqual(calls, ["shell.stop", "logout", "link.stop"]);
  assert.equal(await store.get(MIRROR_KEY_NAME), undefined);
  assert.equal(takeSignOutFailures(), null);

  // The clear fails: the page still goes, and the sign-in screen is told what.
  idb.failClear = "the database is broken";
  try {
    setHubShell(shell);
    await signOut();
  } finally {
    idb.failClear = null;
    setHubShell(null);
  }
  assert.deepEqual(replaced, ["/signin", "/signin"]);
  assert.ok(session.values.has(SIGN_OUT_FAILURES_KEY));
  const failures = takeSignOutFailures();
  assert.equal(failures?.length, 1);
  assert.match(failures![0]!, /^offline copies: .*the database is broken/);
  assert.equal(takeSignOutFailures(), null, "taken once");
  assert.equal(session.values.has(SIGN_OUT_FAILURES_KEY), false);

  // Whatever else is under the key is not trusted as a list.
  session.setItem(SIGN_OUT_FAILURES_KEY, "{not json");
  assert.equal(takeSignOutFailures(), null);
  session.setItem(SIGN_OUT_FAILURES_KEY, JSON.stringify([1, "x", { y: 1 }]));
  assert.deepEqual(takeSignOutFailures(), ["x"]);
});

test("sign-out from a screen with no account open (keys unreadable, a fork, an error) ends the session it held, and keeps the device keys", async () => {
  const g = globalThis as { sessionStorage?: unknown; location?: unknown };
  g.sessionStorage = new FakeStorage();
  const replaced: string[] = [];
  g.location = { replace: (url: string) => void replaced.push(url) };
  const ended: string[] = [];
  const keys = new MemoryKeyStore();
  await keys.put("device-keys/v2", { kept: true });
  setHubShell(null);
  cacheUnseal();
  await signOut(undefined, { api: { logout: async (token: string) => void ended.push(token) }, token: "held-token" });
  assert.deepEqual(ended, ["held-token"], "the hub session is ended");
  assert.deepEqual(replaced, ["/signin"]);
  assert.deepEqual(await keys.get("device-keys/v2"), { kept: true }, "the device keys stay");
  // With nothing held, there is nothing to end (and nothing fails).
  await signOut();
  assert.deepEqual(ended, ["held-token"]);
  takeSignOutFailures();
});
