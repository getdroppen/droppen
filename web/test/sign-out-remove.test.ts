// Signing out of cloud mode, the two ways (`features/cloud/sign-out.ts`,
// `CloudAccount.selfRemoval` / `removeSelf`):
// - "sign out" (a computer of one's own): the hub session ends, notifications
//   stop, and the device keys stay — signing in again opens it as before;
// - "sign out and remove this browser" (a shared or borrowed one): it takes
//   itself off the account's devices where that takes nothing else with it
//   (an admin device, not the last one, that set no recovery words or passkey
//   slots), and forgets its keys whatever it is — what keeps it listed is said
//   on the sign-in screen, once.
// Against the fake hub: the hub then gives the wiped browser no session, as the
// chain shows it removed, and the other full devices see the user key stale.
import assert from "node:assert/strict";
import test from "node:test";
import { MemoryKeyStore } from "../src/lib/e2e/index.js";
import type { CloudAccount } from "../src/lib/hub/account.js";
import { userKeyStale } from "../src/lib/hub/account.js";
import { candidateFromOffer, PairingResponder } from "../src/lib/hub/pairing.js";
import { setHubShell } from "../src/features/cloud/hub-shell.js";
import { SIGN_OUT_STILL_LISTED_KEY, signOut, takeStillListed } from "../src/features/cloud/sign-out.js";
import { browserStorage, createV2Genesis, FakeKeysHub, RecordingKeyStore } from "./fake-keys-hub.js";

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

const g = globalThis as { sessionStorage?: unknown; location?: unknown };
const tab = new FakeStorage();
g.sessionStorage = tab;
const replaced: string[] = [];
g.location = { replace: (url: string) => void replaced.push(url) };

/** A new browser paired from `a` (QR), as a full device or not. */
async function pairAs(hub: FakeKeysHub, a: CloudAccount, opts: { admin: boolean }): Promise<CloudAccount> {
  const b = await hub.device("tok", new RecordingKeyStore());
  const r = new PairingResponder({
    api: b.api,
    token: b.session.token,
    account: b.account,
    device: b.deviceId,
    signPublic: b.keys.signPublic,
    noise: b.noiseKeys,
    chain: async () => (await b.sync(), b.chain),
    storeUserKeys: (keys) => b.storeUserKeys(keys),
    pollMs: 1,
  });
  try {
    const c = await candidateFromOffer(await r.startOffer(), a.chain!);
    await a.pairDevice(c, opts);
    for (let i = 0; i < 10 && r.state.kind !== "paired"; i++) await r.tick();
    assert.equal(r.state.kind, "paired", JSON.stringify(r.state));
  } finally {
    r.stop();
  }
  await b.sync();
  return b;
}

/** A v3 account founded by `a` (with recovery words, which `a` then knows), and a host. */
async function account() {
  const hub = new FakeKeysHub();
  hub.chainV3Mode = "write";
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  await hub.enroll(a, "m-1");
  await a.upgradeToV3({ writable: true });
  return { hub, a };
}

/** What this browser keeps for the account: its device keys and its device id. */
async function kept(b: CloudAccount): Promise<{ keys: boolean; id: boolean }> {
  return { keys: (await b.store.get("device-keys/v2")) !== undefined, id: browserStorage(b.store).values.has(`rh.hub.device.${b.account}`) };
}

test("whether a device may take itself off the account: only an admin that is not the last, and that set no recovery words or passkeys", async () => {
  const { hub, a } = await account();
  assert.equal(a.selfRemoval(), "lastAdmin", "the account's only full device stays one");
  const c = await pairAs(hub, a, { admin: true });
  const d = await pairAs(hub, a, { admin: false });
  await a.sync();
  assert.equal(c.selfRemoval(), "removable");
  assert.equal(d.selfRemoval(), "notAdmin", "a device that signs nothing cannot sign its own removal");
  assert.equal(a.selfRemoval(), "keepsSignIn", "its removal would void the recovery words it set");
  const stranger = await hub.device("tok", new MemoryKeyStore());
  assert.equal(stranger.selfRemoval(), "notActive", "a browser the chain does not name");
  await assert.rejects(d.removeSelf(), (e: unknown) => e instanceof Error && /notAdmin/.test(e.message));
  await assert.rejects(a.removeSelf(), /keepsSignIn/);

  // Taking itself off: one entry, signed by itself, nothing rotated by it.
  const at = a.chain!.seq;
  await c.removeSelf();
  await a.sync();
  const entries = a.chain!.allEntries().slice(at + 1);
  assert.deepEqual(entries.map((e) => e.draft.action.type), ["removeDevice3"]);
  assert.deepEqual(entries[0]!.draft.signer, { type: "device", id: c.deviceId });
  assert.equal(a.chain!.deviceRecord(c.deviceId)?.status.type, "removed");
  assert.equal(userKeyStale(a.chain!), true, "the user key it held is stale for the others: their next keys batch replaces it");
  // The fake hub revokes a removed device's sessions as the removal lands, as rh-hub does.
  await assert.rejects(c.api.me(c.session.token, { noRenew: true }));
});

test("“sign out” keeps this browser a device: its keys and id stay, the session ends, notifications stop", async () => {
  const { hub, a } = await account();
  const c = await pairAs(hub, a, { admin: true });
  setHubShell(null);
  let pushes = 0;
  replaced.length = 0;
  await signOut(c, undefined, { dropPush: async () => void pushes++ });
  assert.equal(pushes, 1, "the push subscription goes on either sign-out");
  assert.deepEqual(replaced, ["/signin"]);
  assert.deepEqual(await kept(c), { keys: true, id: true });
  await a.sync();
  assert.equal(a.chain!.deviceRecord(c.deviceId)?.status.type, "active", "still a device of the account");
  assert.equal(takeStillListed(), null);
  // Signing in again opens it as the same device.
  const again = await hub.device("tok", c.store);
  assert.equal(again.deviceId, c.deviceId);
  assert.equal(again.standing, "active");
});

test("“sign out and remove this browser” takes it off the account and forgets its keys", async () => {
  const { hub, a } = await account();
  const c = await pairAs(hub, a, { admin: true });
  setHubShell(null);
  let pushes = 0;
  replaced.length = 0;
  await signOut(c, undefined, { removeBrowser: true, dropPush: async () => void pushes++ });
  assert.equal(pushes, 1);
  assert.deepEqual(replaced, ["/signin"]);
  assert.deepEqual(await kept(c), { keys: false, id: false }, "no device key or id is left in this browser");
  await a.sync();
  assert.equal(a.chain!.deviceRecord(c.deviceId)?.status.type, "removed", "and it is off the account");
  assert.equal(takeStillListed(), null, "nothing to say: it is not listed");
  // A sign-in here now is a new browser, not that device.
  const again = await hub.device("tok", c.store);
  assert.notEqual(again.deviceId, c.deviceId);
  assert.equal(again.standing, "absent");
});

test("a device that cannot take itself off is wiped all the same, and the sign-in screen says it is still listed", async () => {
  const { hub, a } = await account();
  const d = await pairAs(hub, a, { admin: false });
  setHubShell(null);
  await signOut(d, undefined, { removeBrowser: true, dropPush: async () => undefined });
  assert.deepEqual(await kept(d), { keys: false, id: false });
  await a.sync();
  assert.equal(a.chain!.deviceRecord(d.deviceId)?.status.type, "active", "only a full device can remove it");
  assert.equal(tab.values.get(SIGN_OUT_STILL_LISTED_KEY), "notAdmin");
  assert.equal(takeStillListed(), "notAdmin");
  assert.equal(takeStillListed(), null, "said once");

  // The account's last full device: it stays listed (the chain keeps one), and is wiped.
  assert.equal(a.selfRemoval(), "lastAdmin");
  await signOut(a, undefined, { removeBrowser: true, dropPush: async () => undefined });
  assert.deepEqual(await kept(a), { keys: false, id: false });
  assert.equal(takeStillListed(), "lastAdmin");

  // What else is under the key is not taken for a reason.
  tab.setItem(SIGN_OUT_STILL_LISTED_KEY, "<script>");
  assert.equal(takeStillListed(), null);
  tab.setItem(SIGN_OUT_STILL_LISTED_KEY, "failed: the hub did not answer");
  assert.equal(takeStillListed(), "failed: the hub did not answer");
});
