// A browser whose WebCrypto lacks Ed25519 and X25519 (the Android phone in
// the report: "NotSupportedError: Algorithm: Unrecognized name"), and
// compatibility mode (`lib/e2e/compat.ts`, `soft25519.ts`).
//
// Pinned: key generation that fails so is said as the browser lacking the
// algorithms (`E2eUnsupportedError`), never as the store not answering —
// while a store that will not take new keys still is; no key is read or made
// in compatibility mode until the person agreed, and the agreement is kept
// for the browser; once agreed, the whole thing works end to end on the
// harness's hub — a new account, a device that runs AI, a second browser
// paired by QR code that then reads the same keys, a reload — with the
// device keys at rest only sealed, and keys made so opening natively too once
// the browser has the algorithms.
//
// Tests run in order: the first ones have no compatibility mode in this process.
import assert from "node:assert/strict";
import test from "node:test";
import { hex, type KeyStore, MemoryKeyStore, utf8 } from "../src/lib/e2e/index.js";
import { compatAgreedFor, compatConsent, compatModeActive, consentToCompat, COMPAT_CONSENT_KEY, e2eReadiness, forgetCompatConsent } from "../src/lib/e2e/compat.js";
import { DeviceKeysUnreadable, loadDeviceKeys, readDeviceKeys } from "../src/lib/e2e/keystore.js";
import { softCurvesOn, uninstallSoftCurves } from "../src/lib/e2e/soft25519.js";
import { checkE2eSupport, compatCovers, E2eUnsupportedError } from "../src/lib/e2e/support.js";
import { type CloudAccount, KeysUnreadableError } from "../src/lib/hub/account.js";
import { chainV3Writable } from "../src/lib/hub/api.js";
import { candidateFromOffer, completePairing, type CompleteDeps, PairingResponder, type ResponderState } from "../src/lib/hub/pairing.js";
import { containsBytes, FakeKeysHub, RecordingKeyStore } from "./fake-keys-hub.js";
import { removeNative } from "./no-native-curves.js";

const QUICK = [0, 0, 0];
const check = () => checkE2eSupport({ requireIndexedDb: false });

/** A browser's `localStorage`. */
function memoryStorage(): Storage & { values: Map<string, string> } {
  const values = new Map<string, string>();
  return {
    values,
    get length() {
      return values.size;
    },
    clear: () => values.clear(),
    key: (i: number) => [...values.keys()][i] ?? null,
    getItem: (k: string) => values.get(k) ?? null,
    setItem: (k: string, v: string) => void values.set(k, v),
    removeItem: (k: string) => void values.delete(k),
  };
}

test("no Ed25519 in WebCrypto: making the device keys fails as the browser lacking it, not as the store", async () => {
  const restore = removeNative(["Ed25519", "X25519"]);
  try {
    const store = new MemoryKeyStore();
    const error = await loadDeviceKeys(store, { retryMs: QUICK }).then(
      () => assert.fail("keys were made"),
      (e: unknown) => e,
    );
    assert.ok(error instanceof E2eUnsupportedError, `an E2eUnsupportedError, not ${String(error)}`);
    assert.ok(!(error instanceof DeviceKeysUnreadable));
    assert.deepEqual(error.missing, ["X25519", "Ed25519"]);
    assert.match(error.message, /NotSupportedError: .*Unrecognized name/);
    assert.equal(await store.get("device-keys/v2"), undefined, "no device keys stored");
    assert.deepEqual(await store.list("device-keys/"), [], "nothing written at all: the keys are made before the store is touched");
    // Signing in on such a browser: the account says the same, not "keys unreadable".
    const hub = new FakeKeysHub();
    await assert.rejects(hub.device("tok", new MemoryKeyStore()), (e: unknown) => e instanceof E2eUnsupportedError && !(e instanceof KeysUnreadableError));
    assert.equal(hub.registrations.length, 0, "no device registered");
  } finally {
    restore();
  }
});

test("a store that will not take new keys is still the storage's failure, with a retry", async () => {
  class RefusingStore extends MemoryKeyStore {
    override async put(): Promise<void> {
      throw new DOMException("the object could not be cloned", "DataCloneError");
    }
  }
  const error = await loadDeviceKeys(new RefusingStore(), { retryMs: QUICK }).then(
    () => assert.fail("keys were made"),
    (e: unknown) => e,
  );
  assert.ok(error instanceof DeviceKeysUnreadable, String(error));
  assert.equal(error.reason, "store");
  assert.match(error.message, /would not store new keys: DataCloneError/);
});

test("compatibility mode: offered only when just the curves are missing, and nothing is made before the person agrees", async () => {
  const restoreHkdf = removeNative(["Ed25519", "HKDF"]);
  try {
    const support = await check();
    assert.deepEqual(support.missing, ["Ed25519", "HKDF"]);
    assert.equal(compatCovers(support), false, "HKDF must stay native");
    assert.deepEqual(await e2eReadiness(support, { storage: memoryStorage(), check }), { kind: "unsupported", missing: ["Ed25519", "HKDF"], compat: false });
  } finally {
    restoreHkdf();
  }
  const restore = removeNative(["Ed25519", "X25519"]);
  try {
    const support = await check();
    assert.equal(compatCovers(support), true);
    const storage = memoryStorage();
    assert.deepEqual(await e2eReadiness(support, { storage, check }), { kind: "unsupported", missing: ["X25519", "Ed25519"], compat: true });
    assert.equal(compatModeActive(), false);
    assert.deepEqual(softCurvesOn(), [], "nothing installed");
    assert.equal(storage.values.size, 0);
  } finally {
    restore();
  }
});

test("an agreement covers what the browser lacked then: lacking more, it is asked again", async () => {
  const restore = removeNative(["Ed25519", "X25519"]);
  try {
    const storage = memoryStorage();
    consentToCompat(["Ed25519"], storage);
    assert.equal(compatAgreedFor(["Ed25519"], storage), true);
    assert.equal(compatAgreedFor(["X25519", "Ed25519"], storage), false);
    assert.deepEqual(await e2eReadiness(await check(), { storage, check }), { kind: "unsupported", missing: ["X25519", "Ed25519"], compat: true });
    assert.equal(compatModeActive(), false, "not entered on an agreement to less");
    assert.deepEqual(softCurvesOn(), []);
  } finally {
    restore();
  }
});

test("the agreement is kept for this browser, and can be withdrawn", () => {
  const storage = memoryStorage();
  forgetCompatConsent(storage); // this page's own memory of the last test's agreement
  assert.equal(compatConsent(storage), null);
  consentToCompat(["X25519", "Ed25519"], storage, () => 1_700_000_000_000);
  assert.deepEqual(JSON.parse(storage.values.get(COMPAT_CONSENT_KEY)!), { at: 1_700_000_000_000, missing: ["X25519", "Ed25519"] });
  assert.deepEqual(compatConsent(storage), { at: 1_700_000_000_000, missing: ["X25519", "Ed25519"] });
  forgetCompatConsent(storage);
  assert.equal(storage.values.size, 0);
  assert.equal(compatConsent(storage), null);
});

/** Run the responder's rounds by hand until `done` (no timers). */
async function until(r: PairingResponder, done: (s: ResponderState) => boolean, rounds = 20): Promise<ResponderState> {
  for (let i = 0; i < rounds && !done(r.state); i++) await r.tick();
  return r.state;
}

function completeDeps(a: CloudAccount): CompleteDeps {
  return {
    api: a.api,
    token: a.session.token,
    device: a.deviceId,
    signPublic: a.keys.signPublic,
    noise: a.noiseKeys,
    chain: () => a.chain!,
    append: (action) => a.append(action),
    withUserKeys: (use) => a.withUserKeys(use),
  };
}

/** The device's Ed25519 seed as `store` keeps it, opened the way the page does (sealed PKCS #8 under the store's AES-GCM key). */
async function storedSeed(store: KeyStore): Promise<Uint8Array> {
  const v2 = (await store.get("device-keys/v2")) as { sign: { iv: Uint8Array; sealed: Uint8Array } };
  const wrap = (await store.get("device-keys/wrap")) as CryptoKey;
  const pkcs8 = new Uint8Array(await crypto.subtle.decrypt({ name: "AES-GCM", iv: v2.sign.iv, additionalData: utf8("rh/v1 device key\0sign") }, wrap, v2.sign.sealed));
  return pkcs8.slice(16);
}

test("agreed: the whole thing works in compatibility mode — a new account, a device that runs AI, a QR pairing, a reload", async () => {
  const restore = removeNative(["Ed25519", "X25519"]);
  const storage = memoryStorage();
  consentToCompat(["X25519", "Ed25519"], storage);
  const ready = await e2eReadiness(await check(), { storage, check });
  assert.deepEqual(ready, { kind: "compat", algorithms: ["X25519", "Ed25519"] });
  assert.equal(compatModeActive(), true);

  const hub = new FakeKeysHub();
  const storeA = new RecordingKeyStore();
  const a = await hub.device("tok", storeA);
  assert.equal(a.keys.sign.privateKey.extractable, false);
  const words = await a.createGenesis(true);
  assert.equal(words?.length, 12);
  await hub.enroll(a, "m-1", { writable: chainV3Writable(await a.api.me(a.session.token)) });
  const b = await hub.device("tok", new RecordingKeyStore());
  assert.equal(b.standing, "absent");

  // The new browser shows its QR code; the first one scans it, approves, and sends the keys.
  const r = new PairingResponder({
    api: b.api,
    token: b.session.token,
    account: b.account,
    device: b.deviceId,
    signPublic: b.keys.signPublic,
    noise: b.noiseKeys,
    chain: async () => {
      await b.sync();
      return b.chain;
    },
    storeUserKeys: (keys) => b.storeUserKeys(keys),
    pollMs: 1,
    chainRetries: 1,
  });
  try {
    const text = await r.startOffer();
    await completePairing(completeDeps(a), await candidateFromOffer(text, a.chain!));
    const state = await until(r, (s) => s.kind === "paired" || s.kind === "failed");
    assert.deepEqual(state, { kind: "paired", epochs: [0] });
  } finally {
    r.stop();
  }
  assert.equal(b.standing, "active");
  await b.keyring.refresh();
  const { indexStream } = await import("../src/lib/e2e/index.js");
  const theirs = await b.machineKeys("m-1");
  const mine = await a.machineKeys("m-1");
  assert.equal(hex(await indexStream(theirs.get(0)!)), hex(await indexStream(mine.get(0)!)), "the same machine key on both");

  // A reload opens the same keys from storage.
  const again = await hub.reload(a);
  assert.equal(hex(again.keys.signPublic), hex(a.keys.signPublic));
  assert.equal(again.standing, "active");

  // At rest the private keys are only ever sealed.
  const seed = await storedSeed(storeA);
  assert.equal(seed.length, 32);
  for (const { name, value } of storeA.puts) assert.ok(!containsBytes(value, seed), `the device's private key in the clear under ${name}`);

  // The same browser, updated to one with the algorithms: its keys open natively, and sign as they did.
  uninstallSoftCurves();
  restore();
  const opened = await readDeviceKeys(storeA);
  assert.equal(opened.kind, "found");
  if (opened.kind !== "found") return;
  assert.equal(hex(opened.keys.signPublic), hex(a.keys.signPublic));
  assert.ok(opened.keys.sign.privateKey instanceof CryptoKey, "a native key now");
  const msg = utf8("signed natively by a key made in compatibility mode");
  const sig = await crypto.subtle.sign({ name: "Ed25519" }, opened.keys.sign.privateKey, msg);
  const pub = await crypto.subtle.importKey("raw", a.keys.signPublic, { name: "Ed25519" }, true, ["verify"]);
  assert.equal(await crypto.subtle.verify({ name: "Ed25519" }, pub, sig, msg), true);
});
