// A reload that cannot read the device keys must not lose the device.
//
// On iOS (Chrome, every browser there is WebKit) a page reloaded after a while
// in the background twice came up without its keys (production, 2026-10-21:
// one device at 04:50 UTC, another at 12:17 UTC — each a
// reload straight to the sign-in form, no request to the hub first, the
// session and device id still in localStorage, the same browser having read
// the same keys on reloads before). The read that did not give the keys was
// taken for keys that were gone: new ones were made over them, the device id
// forgotten, and the browser signed in as a new device, for good.
//
// WebKit seals every `CryptoKey` it stores in IndexedDB under a master key of
// the app's (in its keychain) and hands back `null` for one it cannot unseal:
// while that master key is out of reach, every stored `CryptoKey` reads as
// `null` and none can be stored (an outage, which passes); once the app has
// made a new one, the old keys read as `null` for good while new ones work
// (the simulator's `keychain reset` does exactly this).
//
// What these pin: a seal key handed back as nothing, a read that fails (a
// dropped connection) or keys that will not open are read again on a fresh
// connection, and if they still do not open they are left exactly as they
// are and the account says why (`keysUnreadable`: still stored, gone for good,
// or the store not answering) instead of registering a new device. New keys
// only when the store answers and holds none, or when the person chooses so
// — and then the old record is kept aside and its seal key never overwritten.
import assert from "node:assert/strict";
import test from "node:test";
import { hex, type KeyStore } from "../src/lib/e2e/index.js";
import { DeviceKeysUnreadable, forgetDeviceKeys, indexedDbKeyStore, keepStorage, loadDeviceKeys, readDeviceKeys, storeKeepsCryptoKeys } from "../src/lib/e2e/keystore.js";
import { CloudAccount, KeysUnreadableError } from "../src/lib/hub/account.js";
import { HostKeyVault, UserKeyVault } from "../src/lib/hub/keyring.js";
import { browserOf, browserSeen, browserStorage, FakeKeysHub, HUB_ORIGIN } from "./fake-keys-hub.js";

const WRAP = "device-keys/wrap";
const V2 = "device-keys/v2";
const QUICK = [0, 0, 0];

/**
 * One page's view of a browser's key store as WebKit keeps it: `disk` is what
 * the browser keeps (shared by every page made over it). During an `outage`
 * (the master key out of reach) every stored `CryptoKey` reads back as `null`
 * and storing one fails; a record named in `nulls` reads back as `null` that
 * many more times (a key sealed under a master key that is gone: `Infinity`);
 * one named in `passThenNull` reads fine that many more times, then as `null`;
 * one named in `script` reads as the values listed, in turn, before the
 * record itself; one named in `failPut` cannot be stored; `throws` reads fail
 * outright first.
 */
class PageStore implements KeyStore {
  readonly nulls = new Map<string, number>();
  readonly passThenNull = new Map<string, number>();
  readonly script = new Map<string, unknown[]>();
  readonly failPut = new Set<string>();
  outage = false;
  throws = 0;
  reopened = 0;
  constructor(readonly disk: Map<string, unknown> = new Map()) {}
  async get(name: string): Promise<unknown> {
    if (this.throws > 0) {
      this.throws--;
      throw new DOMException("Connection to Indexed Database server lost. Refresh the page to try again", "UnknownError");
    }
    const scripted = this.script.get(name);
    if (scripted?.length) return scripted.shift();
    const value = this.disk.get(name);
    if (this.outage && value instanceof CryptoKey) return null;
    const left = this.nulls.get(name) ?? 0;
    if (left > 0 && this.disk.has(name)) {
      this.nulls.set(name, left - 1);
      return null;
    }
    const pass = this.passThenNull.get(name);
    if (pass !== undefined && this.disk.has(name)) {
      if (pass === 0) return null;
      this.passThenNull.set(name, pass - 1);
    }
    return value;
  }
  async put(name: string, value: unknown): Promise<void> {
    if ((this.outage && value instanceof CryptoKey) || this.failPut.has(name)) throw new DOMException("the key could not be sealed for storage", "DataCloneError");
    this.disk.set(name, value);
  }
  async delete(name: string): Promise<void> {
    this.disk.delete(name);
  }
  reopen(): void {
    this.reopened++;
  }
  async list(prefix: string): Promise<string[]> {
    return [...this.disk.keys()].filter((name) => name.startsWith(prefix));
  }
}

test("a seal key handed back as nothing on the first reads is read again, and the same keys come back", async () => {
  const store = new PageStore();
  const first = await loadDeviceKeys(store);
  assert.equal(first.created, true);
  store.nulls.set(WRAP, 2);
  const again = await loadDeviceKeys(store, { expected: true, retryMs: QUICK });
  assert.equal(again.created, false, "no new keys over the stored ones");
  assert.equal(hex(again.keys.signPublic), hex(first.keys.signPublic));
  assert.ok(store.reopened >= 1, "read again on a fresh connection");
});

test("keys that will not open while the browser keeps no keys at all are left exactly as they are, and say why", async () => {
  const store = new PageStore();
  const first = await loadDeviceKeys(store);
  const sealed = store.disk.get(V2);
  const wrap = store.disk.get(WRAP);
  store.outage = true;
  assert.equal(await storeKeepsCryptoKeys(store), false);
  await assert.rejects(
    loadDeviceKeys(store, { expected: true, retryMs: QUICK }),
    (e: unknown) => e instanceof DeviceKeysUnreadable && e.reason === "stored" && /handed back nothing/.test(e.why),
  );
  assert.equal(store.disk.get(V2), sealed, "the sealed keys were not replaced");
  assert.equal(store.disk.get(WRAP), wrap, "nor the key they are sealed under");
  store.outage = false;
  const healed = await loadDeviceKeys(store, { expected: true, retryMs: QUICK });
  assert.equal(healed.created, false);
  assert.equal(hex(healed.keys.signPublic), hex(first.keys.signPublic), "the next read has them again");
});

test("a seal key that never opens again while new keys are kept is said to be gone, and still nothing is replaced: a person decides", async () => {
  const store = new PageStore();
  const first = await loadDeviceKeys(store);
  const sealed = store.disk.get(V2);
  const wrap = store.disk.get(WRAP);
  store.nulls.set(WRAP, Infinity);
  assert.equal(await storeKeepsCryptoKeys(store), true);
  await assert.rejects(
    loadDeviceKeys(store, { expected: true, retryMs: QUICK }),
    (e: unknown) => e instanceof DeviceKeysUnreadable && e.reason === "gone" && /key it protects stored keys with changed/.test(e.why),
  );
  assert.equal(store.disk.get(V2), sealed, "not replaced on its own");
  assert.equal(store.disk.has("device-keys/probe"), false, "the probe leaves nothing behind");
  // The person chooses a new device: the old record is kept aside, and its seal key — handed back as nothing — is not overwritten.
  const next = await loadDeviceKeys(store, { expected: true, replaceUnreadable: true, retryMs: QUICK, now: () => 1234 });
  assert.equal(next.created, true);
  assert.notEqual(hex(next.keys.signPublic), hex(first.keys.signPublic));
  assert.match(next.instead ?? "", /^replaced keys that would not open \(the browser no longer opens/);
  assert.equal(store.disk.get(`${V2}.prev-1234`), sealed, "the old keys kept aside");
  assert.equal(store.disk.get(WRAP), wrap, "the old seal key left as it is");
  const stored = store.disk.get(V2) as { wrapName?: string };
  assert.equal(stored.wrapName, `${WRAP}-1234`, "the new keys under a seal key of their own");
  // And they come back on the next load (the old seal key still reads as nothing).
  const again = await loadDeviceKeys(store, { expected: true, retryMs: QUICK });
  assert.equal(again.created, false);
  assert.equal(hex(again.keys.signPublic), hex(next.keys.signPublic));
  // Forgetting the device keys forgets all of it.
  await forgetDeviceKeys(store);
  assert.deepEqual([...store.disk.keys()].filter((k) => k.startsWith("device-keys/")), []);
});

test("a store that keeps failing to answer is said so, never as keys still stored", async () => {
  const store = new PageStore();
  await loadDeviceKeys(store);
  store.throws = 10;
  await assert.rejects(
    loadDeviceKeys(store, { expected: true, retryMs: QUICK }),
    (e: unknown) => e instanceof DeviceKeysUnreadable && e.reason === "store" && /key store failed/.test(e.why),
  );
});

test("a seal key handed back as nothing four times and then fine is the same keys, not gone (read once more after the probe)", async () => {
  const store = new PageStore();
  const first = await loadDeviceKeys(store);
  // The first read and the three more hand back nothing; the store keeps new keys; the read after that has it.
  store.nulls.set(WRAP, 4);
  const again = await loadDeviceKeys(store, { expected: true, retryMs: QUICK });
  assert.equal(again.created, false);
  assert.equal(hex(again.keys.signPublic), hex(first.keys.signPublic));
});

test("stored keys handed back as nothing themselves are not replaced even when asked: they cannot be kept aside", async () => {
  const store = new PageStore();
  await loadDeviceKeys(store);
  const sealed = store.disk.get(V2);
  store.nulls.set(V2, Infinity);
  await assert.rejects(
    loadDeviceKeys(store, { expected: true, replaceUnreadable: true, retryMs: QUICK }),
    (e: unknown) => e instanceof DeviceKeysUnreadable && e.reason === "stored" && /cannot be kept aside/.test(e.why),
  );
  assert.equal(store.disk.get(V2), sealed);
});

test("keys kept aside already are not copied aside again (a replacement the store would not finish, tried again)", async () => {
  const store = new PageStore();
  await loadDeviceKeys(store);
  store.outage = true;
  let at = 500;
  for (let i = 0; i < 3; i++) {
    await assert.rejects(loadDeviceKeys(store, { expected: true, replaceUnreadable: true, retryMs: QUICK, now: () => ++at }), DeviceKeysUnreadable);
  }
  assert.deepEqual([...store.disk.keys()].filter((k) => k.startsWith(`${V2}.prev-`)), [`${V2}.prev-501`], "one copy of the same keys");
});

test("at most the newest two replaced keys are kept aside", async () => {
  const store = new PageStore();
  await loadDeviceKeys(store);
  let at = 100;
  for (let i = 0; i < 3; i++) {
    await swapSealKey(store);
    await loadDeviceKeys(store, { expected: true, replaceUnreadable: true, retryMs: QUICK, now: () => ++at });
  }
  assert.deepEqual([...store.disk.keys()].filter((k) => k.startsWith(`${V2}.prev-`)).sort(), [`${V2}.prev-102`, `${V2}.prev-103`]);
});

test("replacing keys during an outage is the unreadable screen (the store would take none), not a bare error, and keeps the old keys", async () => {
  const store = new PageStore();
  const first = await loadDeviceKeys(store);
  const sealed = store.disk.get(V2);
  store.outage = true;
  await assert.rejects(
    loadDeviceKeys(store, { expected: true, replaceUnreadable: true, retryMs: QUICK }),
    (e: unknown) => e instanceof DeviceKeysUnreadable && e.reason === "store" && /would not store new keys/.test(e.why),
  );
  assert.equal(store.disk.get(V2), sealed);
  store.outage = false;
  assert.equal(hex((await loadDeviceKeys(store, { expected: true, retryMs: QUICK })).keys.signPublic), hex(first.keys.signPublic));
});

test("a read that fails (a dropped connection) is retried on a fresh connection", async () => {
  const store = new PageStore();
  const first = await loadDeviceKeys(store);
  store.throws = 1;
  const again = await loadDeviceKeys(store, { expected: true, retryMs: QUICK });
  assert.equal(again.created, false);
  assert.equal(hex(again.keys.signPublic), hex(first.keys.signPublic));
  assert.equal(store.reopened, 1);
});

/** Put a different AES-GCM key where the seal key was: the sealed keys no longer decrypt. */
async function swapSealKey(store: KeyStore): Promise<void> {
  await store.put(WRAP, await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]));
}

test("sealed keys that do not open under their seal key are unreadable, not gone", async () => {
  const store = new PageStore();
  await loadDeviceKeys(store);
  await swapSealKey(store);
  const read = await readDeviceKeys(store);
  assert.equal(read.kind, "unreadable");
  await assert.rejects(loadDeviceKeys(store, { retryMs: [] }), (e: unknown) => e instanceof DeviceKeysUnreadable && e.reason === "stored");
});

test("a store that answers and holds none still means new keys, and says so", async () => {
  const store = new PageStore();
  await loadDeviceKeys(store);
  await forgetDeviceKeys(store);
  assert.deepEqual(await readDeviceKeys(store), { kind: "none", afresh: false });
  const next = await loadDeviceKeys(store, { expected: true, retryMs: QUICK });
  assert.equal(next.created, true);
  assert.equal(next.instead, "no device keys were stored in this browser");
});

test("only the person's choice replaces keys that will not open", async () => {
  const store = new PageStore();
  const first = await loadDeviceKeys(store);
  await swapSealKey(store);
  const sealed = store.disk.get(V2);
  const replaced = await loadDeviceKeys(store, { expected: true, replaceUnreadable: true, retryMs: QUICK, now: () => 99 });
  assert.equal(replaced.created, true);
  assert.notEqual(hex(replaced.keys.signPublic), hex(first.keys.signPublic));
  assert.match(replaced.instead ?? "", /^replaced keys that would not open/);
  assert.equal(store.disk.get(`${V2}.prev-99`), sealed, "the old keys kept aside");
  assert.equal((store.disk.get(V2) as { wrapName?: string }).wrapName, undefined, "a seal key that reads fine is used again");
});

test("a reload that cannot read its keys keeps the device: no new device, no lost notice, and the next reload signs in as before", async () => {
  const hub = new FakeKeysHub();
  hub.deviceSessionMs = 1_000; // expiring at once: every open signs a new session
  const store = new PageStore();
  const a = await hub.device("tok", store);
  const storage = browserStorage(store);
  const registered = hub.registeredMs.size;
  store.outage = true;
  await assert.rejects(
    CloudAccount.open(hub.api(), { ...a.session }, store, { origin: HUB_ORIGIN, storage, seen: browserSeen(store), keyReadRetryMs: QUICK }),
    (e: unknown) => e instanceof KeysUnreadableError && e.kind === "keysUnreadable" && e.reason === "stored",
  );
  assert.equal(storage.getItem(`rh.hub.device.${a.account}`), a.deviceId, "the device id is kept");
  assert.equal(storage.getItem(`rh.hub.lost.${a.account}`), null, "nothing is said to be lost");
  assert.equal(hub.registeredMs.size, registered, "no new device registered");
  store.outage = false;
  const reloaded = await CloudAccount.open(hub.api(), { ...a.session }, store, { origin: HUB_ORIGIN, storage, seen: browserSeen(store), keyReadRetryMs: QUICK });
  assert.equal(reloaded.refusal, null, "the hub took its signature");
  assert.equal(reloaded.deviceId, a.deviceId, "the same device");
  assert.equal(reloaded.lostDevice, null);
});

test("a reload whose seal key reads back as nothing twice signs in as the same device (the iOS reloads that lost theirs)", async () => {
  const hub = new FakeKeysHub();
  hub.deviceSessionMs = 1_000;
  const store = new PageStore();
  const a = await hub.device("tok", store);
  const storage = browserStorage(store);
  const registered = hub.registeredMs.size;
  store.nulls.set(WRAP, 2);
  const reloaded = await CloudAccount.open(hub.api(), { ...a.session }, store, { origin: HUB_ORIGIN, storage, seen: browserSeen(store), keyReadRetryMs: QUICK });
  assert.equal(reloaded.refusal, null);
  assert.equal(reloaded.deviceId, a.deviceId, "the same device");
  assert.equal(reloaded.lostDevice, null, "nothing said to be lost");
  assert.equal(hub.registeredMs.size, registered, "no new device registered");
});

test("choosing to use the browser as a new device registers one and names the device it replaced", async () => {
  const hub = new FakeKeysHub();
  const store = new PageStore();
  const a = await hub.device("tok", store);
  await swapSealKey(store);
  const login = await hub.api().login("someone@example.test", "a password");
  const b = await CloudAccount.open(hub.api(), login, store, { origin: HUB_ORIGIN, ...browserOf(store), keyReadRetryMs: QUICK, replaceUnreadableKeys: true });
  assert.equal(b.refusal, null);
  assert.notEqual(b.deviceId, a.deviceId, "a new device");
  assert.equal(b.lostDevice, a.deviceId, "and it names the one it replaced");
  assert.match(b.lostWhy ?? "", /^replaced keys that would not open/);
});

test("a sign-in that replaces unreadable keys and then registers afresh never replaces the keys a second time", async () => {
  // A browser on the pairing screen (not in the chain) asked to sign in again; its keys are fine on this
  // sign-in's first read and handed back as nothing from then on (the browser's key for stored keys changed).
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await a.createGenesis(true);
  const store = new PageStore();
  const b = await hub.device("tok", store);
  assert.equal(b.standing, "absent");
  assert.equal(b.registerAfreshOnSignIn(), true);
  store.passThenNull.set(WRAP, 1);
  const registered = hub.registeredMs.size;
  const login = await hub.api().login("someone@example.test", "a password");
  // The person had chosen "use this browser as a new device": the second open (registered afresh) must not replace again.
  await assert.rejects(
    CloudAccount.open(hub.api(), login, store, { origin: HUB_ORIGIN, ...browserOf(store), keyReadRetryMs: QUICK, replaceUnreadableKeys: true }),
    (e: unknown) => e instanceof KeysUnreadableError && e.reason === "gone",
  );
  assert.equal(hub.registeredMs.size, registered + 1, "registered afresh once, with the keys it signed with");
  assert.equal([...store.disk.keys()].filter((k) => k.startsWith(`${V2}.prev-`)).length, 0, "no keys replaced");
});

test("the key user keys are sealed under, handed back as nothing once, is read again rather than replaced", async () => {
  const disk = new Map<string, unknown>();
  assert.equal(await new UserKeyVault(new PageStore(disk), "a-1").put(0, crypto.getRandomValues(new Uint8Array(32))), true);
  const flaky = new PageStore(disk);
  flaky.nulls.set("local-wrap/v1", 1);
  assert.equal(await new UserKeyVault(flaky, "a-1").put(1, crypto.getRandomValues(new Uint8Array(32))), true);
  const later = new UserKeyVault(new PageStore(disk), "a-1");
  assert.equal((await later.key(0)).algorithm.name, "HKDF", "epoch 0 still opens: its seal key was kept");
  assert.equal((await later.key(1)).algorithm.name, "HKDF");
});

test("the key user keys are sealed under is not replaced during an outage, and is once it never opens again", async () => {
  const disk = new Map<string, unknown>();
  const raw = crypto.getRandomValues(new Uint8Array(32));
  assert.equal(await new UserKeyVault(new PageStore(disk), "a-1").put(0, raw.slice()), true);
  // The next page, during an outage: storing another epoch fails, and nothing is replaced.
  const out = new PageStore(disk);
  out.outage = true;
  await assert.rejects(new UserKeyVault(out, "a-1").put(1, crypto.getRandomValues(new Uint8Array(32))));
  // A page after that reads it again: epoch 0 still opens.
  const key = await new UserKeyVault(new PageStore(disk), "a-1").key(0);
  assert.equal(key.algorithm.name, "HKDF");
});

test("the user keys' seal key is read once more under the lock before it is replaced: four nothings in a row and then the key is no replacement", async () => {
  const disk = new Map<string, unknown>();
  assert.equal(await new UserKeyVault(new PageStore(disk), "a-1").put(0, crypto.getRandomValues(new Uint8Array(32))), true);
  const flaky = new PageStore(disk);
  // Every read readKept makes (one and three more) hands back nothing; the one under the lock gets it.
  flaky.nulls.set("local-wrap/v1", 4);
  assert.equal(await new UserKeyVault(flaky, "a-1").put(1, crypto.getRandomValues(new Uint8Array(32))), true);
  assert.equal((await new UserKeyVault(new PageStore(disk), "a-1").key(0)).algorithm.name, "HKDF", "epoch 0 still opens: not replaced");
});

test("a user-key seal key that never opens again is replaced, what it sealed forgotten, and keys sent again restore it all", async () => {
  const disk = new Map<string, unknown>();
  const raw = crypto.getRandomValues(new Uint8Array(32));
  const hostRaw = crypto.getRandomValues(new Uint8Array(32));
  const first = new PageStore(disk);
  assert.equal(await new UserKeyVault(first, "a-1").put(0, raw.slice()), true);
  assert.equal(await new HostKeyVault(first, "a-1").put("m-1", 0, hostRaw.slice()), true);
  // The next page: the seal key never opens again (the browser's key for stored keys changed), new keys go in fine.
  const gone = new PageStore(disk);
  gone.nulls.set("local-wrap/v1", Infinity);
  const vault = new UserKeyVault(gone, "a-1");
  await assert.rejects(vault.key(0), /does not hold user key epoch 0/, "what the old key sealed is forgotten");
  assert.deepEqual(await vault.epochs(), [], "and no longer listed: the device says it lacks keys");
  assert.equal(await new HostKeyVault(gone, "a-1").has("m-1", 0), false, "the hosts' keys too");
  // Another device sends the keys again: taken (not skipped as held), and they open.
  assert.equal(await vault.put(0, raw.slice()), true);
  assert.equal((await vault.key(0)).algorithm.name, "HKDF");
  const later = new PageStore(disk);
  later.nulls.set("local-wrap/v1", 0);
  assert.equal((await new UserKeyVault(later, "a-1").key(0)).algorithm.name, "HKDF", "and on the next page too");
});

test("a user-key seal key that never opens again is not replaced when the new one cannot be stored: what it sealed stays", async () => {
  const disk = new Map<string, unknown>();
  const raw = crypto.getRandomValues(new Uint8Array(32));
  assert.equal(await new UserKeyVault(new PageStore(disk), "a-1").put(0, raw.slice()), true);
  const page = new PageStore(disk);
  page.nulls.set("local-wrap/v1", Infinity);
  page.failPut.add("local-wrap/v1");
  await assert.rejects(new UserKeyVault(page, "a-1").put(1, crypto.getRandomValues(new Uint8Array(32))));
  assert.deepEqual(disk.get("uk-epochs/a-1"), [0], "epoch 0 still listed");
  assert.ok(disk.has("uk/a-1/0"), "and still stored");
  assert.equal((await new UserKeyVault(new PageStore(disk), "a-1").key(0)).algorithm.name, "HKDF", "and it opens once the key reads again");
});

test("a user-key seal key read as missing, then as nothing under the lock, then fine after the probe is kept: nothing it sealed is forgotten", async () => {
  const disk = new Map<string, unknown>();
  assert.equal(await new UserKeyVault(new PageStore(disk), "a-1").put(0, crypto.getRandomValues(new Uint8Array(32))), true);
  const wrap = disk.get("local-wrap/v1");
  const page = new PageStore(disk);
  // First read: missing (no retries, no probe); under the lock: nothing; the store keeps new keys; the read after that has it.
  page.script.set("local-wrap/v1", [undefined, null]);
  assert.equal(await new UserKeyVault(page, "a-1").put(1, crypto.getRandomValues(new Uint8Array(32))), true);
  assert.equal(disk.get("local-wrap/v1"), wrap, "not replaced");
  assert.deepEqual(disk.get("uk-epochs/a-1"), [0, 1], "nothing forgotten");
  assert.equal((await new UserKeyVault(new PageStore(disk), "a-1").key(0)).algorithm.name, "HKDF");
});

test("a user-key seal key read as missing and then as nothing under the lock is not replaced unless the store keeps new keys", async () => {
  const disk = new Map<string, unknown>();
  assert.equal(await new UserKeyVault(new PageStore(disk), "a-1").put(0, crypto.getRandomValues(new Uint8Array(32))), true);
  const wrap = disk.get("local-wrap/v1");
  const page = new PageStore(disk);
  // First read: missing (no retries, no probe); the read under the lock: nothing; and the store keeps no new keys.
  page.script.set("local-wrap/v1", [undefined, null]);
  page.outage = true;
  await assert.rejects(new UserKeyVault(page, "a-1").put(1, crypto.getRandomValues(new Uint8Array(32))), /could not read the key its user keys are sealed under/);
  assert.equal(disk.get("local-wrap/v1"), wrap, "the seal key is not replaced");
  assert.deepEqual(disk.get("uk-epochs/a-1"), [0], "nothing it sealed is forgotten");
});

/** An `IDBFactory` with one object store in memory: `failOpens` opens fail first; `opened` and the last connection are kept. */
function fakeIndexedDb(failOpens = 0) {
  const data = new Map<string, unknown>();
  let exists = false;
  const state = { opened: 0, last: null as null | { onclose: (() => void) | null; closed: boolean } };
  const answer = <T>(result: T) => {
    const req = { result, onsuccess: null as null | (() => void), onerror: null as null | (() => void) };
    queueMicrotask(() => req.onsuccess?.());
    return req;
  };
  const factory = {
    open() {
      const req = { result: null as unknown, error: null as unknown, onupgradeneeded: null as null | ((e: { oldVersion: number }) => void), onsuccess: null as null | (() => void), onerror: null as null | (() => void) };
      state.opened++;
      queueMicrotask(() => {
        if (failOpens > 0) {
          failOpens--;
          req.error = new DOMException("Connection to Indexed Database server lost. Refresh the page to try again", "UnknownError");
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
          transaction: () => ({
            objectStore: () => ({
              get: (k: string) => answer(data.get(k)),
              put: (v: unknown, k: string) => answer(void data.set(k, v)),
              delete: (k: string) => answer(void data.delete(k)),
            }),
          }),
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
  return { factory: factory as unknown as IDBFactory, state };
}

test("an IndexedDB key store whose open failed, or whose connection the browser closed, opens afresh on the next call", async () => {
  const { factory, state } = fakeIndexedDb(1);
  const store = indexedDbKeyStore(factory, "rh-e2e-test");
  await assert.rejects(store.get("x"), /Connection to Indexed Database server lost/);
  await store.put("x", 1);
  assert.equal(await store.get("x"), 1, "the failed open was not kept");
  assert.equal(state.opened, 2);
  assert.equal(store.createdAfresh?.(), true, "it says the store was made just now");
  state.last!.onclose!();
  assert.equal(await store.get("x"), 1);
  assert.equal(state.opened, 3, "a closed connection is let go of");
  const before = state.last!;
  store.reopen!();
  assert.equal(await store.get("x"), 1);
  assert.equal(state.opened, 4);
  assert.equal(before.closed, true, "reopening closes the old connection");
});

const SAFARI_IOS = "Mozilla/5.0 (iPhone; CPU iPhone OS 26_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.5 Mobile/15E148 Safari/604.1";
const CHROME_IOS = "Mozilla/5.0 (iPhone; CPU iPhone OS 27_2 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/87.0.4280.77 Mobile/15E148 Safari/604.1";
const CHROME = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/155.0.0.0 Safari/537.36";
const FIREFOX = "Mozilla/5.0 (Macintosh; Intel Mac OS X 15.7; rv:150.0) Gecko/20100101 Firefox/150.0";

test("persistent storage is asked for once it is not had, and a browser without it is fine", async () => {
  assert.equal(await keepStorage(undefined, SAFARI_IOS), null);
  let asked = 0;
  const granted = { persisted: async () => false, persist: async () => (asked++, true) } as unknown as StorageManager;
  assert.equal(await keepStorage(granted, CHROME_IOS), true);
  assert.equal(asked, 1);
  const already = { persisted: async () => true, persist: async () => (asked++, true) } as unknown as StorageManager;
  assert.equal(await keepStorage(already, SAFARI_IOS), true);
  assert.equal(asked, 1, "not asked again when already kept");
  const throwing = { persist: async () => Promise.reject(new Error("no")) } as unknown as StorageManager;
  assert.equal(await keepStorage(throwing, CHROME), null);
});

test("persistent storage is asked for on WebKit and Chromium only: Firefox would prompt the person", async () => {
  let asked = 0;
  const storage = { persisted: async () => false, persist: async () => (asked++, true) } as unknown as StorageManager;
  assert.equal(await keepStorage(storage, FIREFOX), null);
  assert.equal(asked, 0, "Firefox is never asked");
  for (const ua of [SAFARI_IOS, CHROME_IOS, CHROME]) assert.equal(await keepStorage(storage, ua), true, ua);
  assert.equal(asked, 3);
});
