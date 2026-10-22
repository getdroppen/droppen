// The seen record's stores (`lib/hub/seen-store.ts`, #183): IndexedDB, where
// a read-compare-write is one transaction and so one step across every page
// of the browser; the `BroadcastChannel` the pages tell one another on; and
// the `localStorage` fallback where IndexedDB is not to be had.
import assert from "node:assert/strict";
import test from "node:test";
import { indexedDbSeenStore, localStorageSeenStore, pageSeenStore, SEEN_IN_IDB, type SeenStore, SeenStoreError } from "../src/lib/hub/seen-store.js";

const task = (fn: () => void) => void setTimeout(fn, 0);

/**
 * An `IDBFactory` holding one database in memory, with IndexedDB's
 * transaction rules as far as the seen store meets them:
 * - transactions run one after another, in the order they were made, across
 *   every connection (every page);
 * - each request is answered a task later; a transaction takes requests only
 *   while it is being made (until the first microtask after) and inside its
 *   requests' callbacks — one asked for after an `await` throws
 *   `TransactionInactiveError`, stricter than browsers are, as WebKit was;
 * - it commits once no request of it is pending after a callback ran, or is
 *   aborted (`abort()`), and then nothing it wrote is kept.
 * `failOpens`: the names of the errors the first opens fail with.
 */
function fakeIndexedDb(failOpens: string[] = []) {
  const data = new Map<string, unknown>();
  let exists = false;
  let last: Promise<void> = Promise.resolve();
  const state = { opened: 0, transactions: 0, last: null as null | { onclose: (() => void) | null; closed: boolean } };

  const transaction = (mode: IDBTransactionMode) => {
    state.transactions++;
    const staged = new Map<string, { value: unknown } | null>();
    let active = true;
    let pending = 0;
    let finished = false;
    queueMicrotask(() => (active = false));
    const before = last;
    let finish!: () => void;
    last = new Promise<void>((r) => (finish = r));
    const tx = {
      oncomplete: null as null | (() => void),
      onabort: null as null | (() => void),
      error: null as unknown,
      objectStore: () => store,
      abort() {
        if (finished) throw new DOMException("the transaction is over", "InvalidStateError");
        finished = true;
        task(() => {
          tx.onabort?.();
          finish();
        });
      },
    };
    const commit = () => {
      finished = true;
      for (const [k, v] of staged) {
        if (v === null) data.delete(k);
        else data.set(k, v.value);
      }
      task(() => {
        tx.oncomplete?.();
        finish();
      });
    };
    const request = (run: () => unknown) => {
      if (!active || finished) throw new DOMException("the transaction is not active", "TransactionInactiveError");
      pending++;
      const req = { result: undefined as unknown, error: null, onsuccess: null as null | (() => void), onerror: null as null | (() => void) };
      void before.then(() =>
        task(() => {
          if (finished) return;
          req.result = run();
          pending--;
          active = true;
          try {
            req.onsuccess?.();
          } finally {
            active = false;
          }
          if (pending === 0 && !finished) commit();
        }),
      );
      return req;
    };
    const store = {
      get: (key: string) => request(() => (staged.has(key) ? staged.get(key)?.value : structuredClone(data.get(key)))),
      put: (value: unknown, key: string) => {
        if (mode === "readonly") throw new DOMException("read only", "ReadOnlyError");
        const copy = structuredClone(value);
        return request(() => void staged.set(key, { value: copy }));
      },
      delete: (key: string) => request(() => void staged.set(key, null)),
    };
    return tx;
  };

  const factory = {
    open() {
      state.opened++;
      const req = {
        result: null as unknown,
        error: null as unknown,
        onupgradeneeded: null as null | ((e: { oldVersion: number }) => void),
        onsuccess: null as null | (() => void),
        onerror: null as null | (() => void),
      };
      task(() => {
        const failure = failOpens.shift();
        if (failure) {
          req.error = new DOMException(`open failed (${failure})`, failure);
          return req.onerror?.();
        }
        const conn = {
          onclose: null as (() => void) | null,
          onversionchange: null as (() => void) | null,
          closed: false,
          createObjectStore() {},
          close() {
            conn.closed = true;
          },
          transaction: (_store: string, mode: IDBTransactionMode) => {
            if (conn.closed) throw new DOMException("the connection is closing", "InvalidStateError");
            return transaction(mode);
          },
        };
        req.result = conn;
        if (!exists) req.onupgradeneeded?.({ oldVersion: 0 });
        exists = true;
        state.last = conn;
        req.onsuccess?.();
      });
      return req;
    },
  };
  return { factory: factory as unknown as IDBFactory, data, state };
}

/** A page's store on `factory` (no channel unless named). */
const pageOn = (factory: IDBFactory, channel: string | null = null): SeenStore => indexedDbSeenStore({ factory, dbName: "rh-hub-seen-test", channel });

/** Keep the higher number: what a page that verified `to` writes. */
const raiseTo = (to: number) => (stored: unknown) => (typeof stored === "number" && stored >= to ? undefined : to);

function memoryStorage() {
  const values = new Map<string, string>();
  return { values, getItem: (k: string) => values.get(k) ?? null, setItem: (k: string, v: string) => void values.set(k, v), removeItem: (k: string) => void values.delete(k) };
}

async function until(done: () => boolean, what: string): Promise<void> {
  for (let i = 0; i < 200 && !done(); i++) await new Promise((r) => setTimeout(r, 5));
  assert.ok(done(), what);
}

test("IndexedDB: pages updating the record at once never leave it lower — each reads, compares and writes in one transaction", async () => {
  const { factory, state } = fakeIndexedDb();
  const pages = [pageOn(factory), pageOn(factory), pageOn(factory)];
  await pages[0]!.update("a-1", raiseTo(6));
  const before = state.transactions;
  // Every page asks at once, each with the view it verified; the lower ones last as often as first.
  await Promise.all([pages[1]!.update("a-1", raiseTo(9)), pages[2]!.update("a-1", raiseTo(7)), pages[0]!.update("a-1", raiseTo(3)), pages[2]!.update("a-1", raiseTo(8))]);
  assert.equal(state.transactions - before, 4, "one transaction each: the read and the write in it");
  assert.equal(await pages[0]!.get("a-1"), 9);
  for (const order of [
    [5, 2],
    [2, 5],
  ]) {
    await Promise.all(order.map((to, i) => pages[i]!.update(`b-${order.join("")}`, raiseTo(to))));
    assert.equal(await pages[2]!.get(`b-${order.join("")}`), 5, `in the order ${order.join(", ")}`);
  }
});

test("IndexedDB: a change that throws writes nothing, and the update rejects with what it threw", async () => {
  const { factory } = fakeIndexedDb();
  const page = pageOn(factory);
  await page.update("a-1", () => ({ seenThrough: 4 }));
  const fork = new Error("a fork");
  await assert.rejects(
    page.update("a-1", () => {
      throw fork;
    }),
    (e: unknown) => e === fork,
  );
  assert.deepEqual(await page.get("a-1"), { seenThrough: 4 });
  // Left as it is.
  await page.update("a-1", () => undefined);
  assert.deepEqual(await page.get("a-1"), { seenThrough: 4 });
});

test("IndexedDB: a page that writes the record says so on the channel; the other pages hear it, the writer does not, and nothing is said when nothing was written", async () => {
  const { factory } = fakeIndexedDb();
  const channel = `rh.hub.seen.test.${Math.random().toString(36).slice(2)}`;
  const writer = pageOn(factory, channel);
  const other = pageOn(factory, channel);
  const heardByOther: (string | null)[] = [];
  const heardByWriter: (string | null)[] = [];
  const stopOther = other.watch((account) => heardByOther.push(account));
  const stopWriter = writer.watch((account) => heardByWriter.push(account));
  try {
    await writer.update("a-1", raiseTo(3));
    await until(() => heardByOther.length > 0, "the other page heard the write");
    assert.deepEqual(heardByOther, ["a-1"]);
    await writer.update("a-1", raiseTo(2));
    await other.update("a-1", raiseTo(5));
    await until(() => heardByWriter.length > 0, "the writer hears the other page in turn");
    await new Promise((r) => setTimeout(r, 30));
    assert.deepEqual(heardByOther, ["a-1"], "a change that kept the record said nothing");
    assert.deepEqual(heardByWriter, ["a-1"], "a page does not hear itself");
  } finally {
    stopOther();
    stopWriter();
  }
});

test("IndexedDB: an open that failed, or a connection the browser closed, is opened afresh on the next call", async () => {
  const { factory, state } = fakeIndexedDb(["UnknownError"]);
  const page = indexedDbSeenStore({ factory, dbName: "rh-hub-seen-test", channel: null, fallback: () => assert.fail("a failure that may pass is not the fallback") });
  await assert.rejects(page.get("a-1"), /open failed/);
  await page.update("a-1", raiseTo(2));
  assert.equal(await page.get("a-1"), 2);
  assert.equal(page.atomic, true);
  const opened = state.opened;
  state.last!.closed = true;
  await page.update("a-1", raiseTo(3));
  assert.equal(await page.get("a-1"), 3, "a connection that would not make a transaction is let go of");
  assert.equal(state.opened, opened + 1);
  state.last!.onclose!();
  assert.equal(await page.get("a-1"), 3);
  assert.equal(state.opened, opened + 2, "a closed connection is let go of");
});

test("IndexedDB not to be had (the first open says so): the pin and the record stay in localStorage where older pages keep them, and the page says so", async (t) => {
  const warned = t.mock.method(console, "warn", () => undefined);
  const storage = memoryStorage();
  storage.setItem("rh.hub.genesis.a-1", "aa");
  storage.setItem("rh.hub.seen.a-1", JSON.stringify({ seenThrough: 4 }));
  const { factory } = fakeIndexedDb(["InvalidStateError"]);
  const page = indexedDbSeenStore({ factory, dbName: "rh-hub-seen-test", channel: null, fallback: () => localStorageSeenStore(storage, null) });
  assert.deepEqual(await page.get("a-1"), { v: 1, genesis: "aa", seen: { seenThrough: 4 } }, "what localStorage holds, as one value");
  assert.equal(page.atomic, false, "not one step across pages any more");
  await page.update("a-1", (stored) => ({ ...(stored as object), seen: { seenThrough: 6 } }));
  assert.equal(storage.getItem("rh.hub.seen.a-1"), JSON.stringify({ seenThrough: 6 }));
  assert.equal(storage.getItem("rh.hub.genesis.a-1"), "aa");
  await page.update("a-1", (stored) => ({ ...(stored as object), seen: null }));
  assert.equal(storage.getItem("rh.hub.seen.a-1"), null, "a record forgotten");
  assert.equal(storage.getItem("rh.hub.genesis.a-1"), "aa", "the pin never");
  assert.ok(
    warned.mock.calls.some((c) => /localStorage/.test(String(c.arguments[0]))),
    "said on the console",
  );
});

test("a browser that kept its values in IndexedDB does not fall back to localStorage: every read fails, to be tried again", async () => {
  const { factory } = fakeIndexedDb(["InvalidStateError"]);
  let opened = 0;
  const page = indexedDbSeenStore({ factory, dbName: "rh-hub-seen-test", channel: null, fallback: () => null, onOpen: () => opened++ });
  await assert.rejects(page.get("a-1"), (e: unknown) => e instanceof SeenStoreError && /not read from localStorage/.test(e.message));
  assert.equal(page.atomic, true);
  await page.update("a-1", raiseTo(1));
  assert.equal(await page.get("a-1"), 1, "the next open, which works, is used");
  assert.equal(opened, 1, "and said once");
});

test("pageSeenStore: with no IndexedDB at all, the record stays in localStorage — unless this browser kept it in IndexedDB before", async (t) => {
  t.mock.method(console, "warn", () => undefined);
  if (typeof (globalThis as { indexedDB?: unknown }).indexedDB !== "undefined") return t.skip("this runner has IndexedDB");
  const fresh = memoryStorage();
  assert.equal(pageSeenStore(fresh).atomic, false, "the localStorage fallback");
  const used = memoryStorage();
  used.setItem(SEEN_IN_IDB, "1");
  const refused = pageSeenStore(used);
  await assert.rejects(refused.get("a-1"), (e: unknown) => e instanceof SeenStoreError);
  await assert.rejects(
    refused.update("a-1", () => 1),
    (e: unknown) => e instanceof SeenStoreError,
  );
});

test("localStorage fallback: pages hear one another through storage events", () => {
  const storage = memoryStorage();
  const events = new EventTarget();
  const page = localStorageSeenStore(storage, events);
  const heard: (string | null)[] = [];
  const stop = page.watch((account) => heard.push(account));
  events.dispatchEvent(Object.assign(new Event("storage"), { key: "rh.hub.seen.a-1" }));
  events.dispatchEvent(Object.assign(new Event("storage"), { key: "rh.hub.genesis.b-2" }));
  events.dispatchEvent(Object.assign(new Event("storage"), { key: "rh.hub.device.a-1" }));
  events.dispatchEvent(Object.assign(new Event("storage"), { key: null }));
  stop();
  events.dispatchEvent(Object.assign(new Event("storage"), { key: "rh.hub.seen.a-1" }));
  assert.deepEqual(heard, ["a-1", "b-2", null]);
});
