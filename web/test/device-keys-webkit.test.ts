// A browser's device keys across a reload where IndexedDB loses X25519
// private keys (WebKit, https://bugs.webkit.org/show_bug.cgi?id=312279:
// Safari 26 and every iOS browser hand back `null` for a record holding a
// non-extractable X25519 `CryptoKey`; Ed25519 and AES-GCM come back fine).
//
// What these pin: the keys a device registered with are the keys it signs
// with after a reload, in such a store too; and a browser whose keys are gone
// (or are not the ones its device id was registered with) never goes on
// signing for that id — it says so and registers as a new device instead of
// being refused by the hub forever.
import assert from "node:assert/strict";
import test from "node:test";
import { hex, MemoryKeyStore } from "../src/lib/e2e/index.js";
import { forgetDeviceKeys, loadDeviceKeys } from "../src/lib/e2e/keystore.js";
import { CloudAccount } from "../src/lib/hub/account.js";
import { browserOf, FakeKeysHub, HUB_ORIGIN } from "./fake-keys-hub.js";

/** Whether `value` holds a private X25519 `CryptoKey` anywhere inside. */
function holdsX25519Private(value: unknown, seen = new Set<unknown>()): boolean {
  if (value === null || typeof value !== "object" || seen.has(value)) return false;
  seen.add(value);
  if (value instanceof CryptoKey) return value.type === "private" && value.algorithm.name === "X25519";
  return Object.values(value as Record<string, unknown>).some((v) => holdsX25519Private(v, seen));
}

/** A key store that behaves as WebKit's IndexedDB does: a record holding a private X25519 key reads back as `null`. */
class WebKitLikeKeyStore extends MemoryKeyStore {
  override async get(name: string): Promise<unknown> {
    const value = await super.get(name);
    return holdsX25519Private(value) ? null : value;
  }
}

test("keys survive a store that loses X25519 private keys, and still sign and agree", async () => {
  const store = new WebKitLikeKeyStore();
  const first = await loadDeviceKeys(store);
  assert.equal(first.created, true);
  const again = await loadDeviceKeys(store);
  assert.equal(again.created, false, "the stored keys came back");
  assert.equal(hex(again.keys.signPublic), hex(first.keys.signPublic));
  assert.equal(hex(again.keys.dhPublic), hex(first.keys.dhPublic));
  const msg = new Uint8Array([1, 2, 3]);
  const sig = new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, again.keys.sign.privateKey, msg));
  assert.ok(await crypto.subtle.verify({ name: "Ed25519" }, first.keys.sign.publicKey, sig, msg));
  const peer = (await crypto.subtle.generateKey({ name: "X25519" }, false, ["deriveBits"])) as CryptoKeyPair;
  const ours = new Uint8Array(await crypto.subtle.deriveBits({ name: "X25519", public: peer.publicKey }, again.keys.dh.privateKey, 256));
  const theirs = new Uint8Array(await crypto.subtle.deriveBits({ name: "X25519", public: first.keys.dh.publicKey }, peer.privateKey, 256));
  assert.equal(hex(ours), hex(theirs), "the X25519 private half matches its public half");
  assert.equal(again.keys.sign.privateKey.extractable, false, "loaded private keys are not extractable");
  assert.equal(again.keys.dh.privateKey.extractable, false, "loaded private keys are not extractable");
});

test("forgetting the device keys forgets every form of them", async () => {
  const store = new MemoryKeyStore();
  const first = await loadDeviceKeys(store);
  await forgetDeviceKeys(store);
  const next = await loadDeviceKeys(store);
  assert.equal(next.created, true);
  assert.notEqual(hex(next.keys.signPublic), hex(first.keys.signPublic));
});

test("a reload in a WebKit-like store renews the device session with the registered keys", async () => {
  const hub = new FakeKeysHub();
  hub.deviceSessionMs = 1_000; // expiring at once: every open signs a new session
  const store = new WebKitLikeKeyStore();
  const a = await hub.device("tok", store);
  assert.equal(a.refusal, null);
  const reloaded = await hub.reload(a);
  assert.equal(reloaded.refusal, null, "the hub took the reloaded browser's signature");
  assert.equal(reloaded.deviceId, a.deviceId, "the same device, not a new one");
});

test("a browser whose device keys are gone registers as a new device and names the one it lost", async () => {
  const hub = new FakeKeysHub();
  hub.deviceSessionMs = 1_000;
  const store = new MemoryKeyStore();
  const a = await hub.device("tok", store);
  const lost = a.deviceId;
  // The keys go (another account's wipe, a browser that dropped them); the stored device id stays.
  await forgetDeviceKeys(store);
  const login = await hub.api().login("someone@example.test", "a password");
  const b = await CloudAccount.open(hub.api(), login, store, { origin: HUB_ORIGIN, ...browserOf(store), keyReadRetryMs: [0] });
  assert.equal(b.refusal, null, "not refused: it never signs for the old id with new keys");
  assert.notEqual(b.deviceId, lost, "registered as a new device");
  assert.equal(b.lostDevice, lost, "and says which device it lost");
  assert.equal(b.lostWhy, "no device keys were stored in this browser", "and what the store held instead");
});
