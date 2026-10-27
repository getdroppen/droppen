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
import { CloudAccount, userKeyStale, WipeError } from "../src/lib/hub/account.js";
import { setHubShell } from "../src/features/machines/shells.js";
import { SIGN_OUT_FAILURES_KEY, SIGN_OUT_STILL_LISTED_KEY, signOut, takeSignOutFailures, takeStillListed } from "../src/features/cloud/sign-out.js";
import { browserStorage, createV2Genesis, FakeKeysHub, pairBrowser } from "./fake-keys-hub.js";
import { MemoryStorage } from "./storage.js";

const g = globalThis as { sessionStorage?: unknown; location?: unknown };
const tab = new MemoryStorage();
g.sessionStorage = tab;
const replaced: string[] = [];
g.location = { replace: (url: string) => void replaced.push(url) };

/** A new browser paired from `a` (QR), as a full device or not. */
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
  const c = await pairBrowser(hub, a, { admin: true });
  const d = await pairBrowser(hub, a, { admin: false });
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
  const c = await pairBrowser(hub, a, { admin: true });
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
  const c = await pairBrowser(hub, a, { admin: true });
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
  const d = await pairBrowser(hub, a, { admin: false });
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

test("a sign-out told where to go (the move notice on the hub's origin) hands over what it would keep for the sign-in screen, and stays", async () => {
  const { hub, a } = await account();
  const d = await pairBrowser(hub, a, { admin: false });
  setHubShell(null);
  replaced.length = 0;
  tab.values.clear();
  const results: unknown[] = [];
  await signOut(d, undefined, { removeBrowser: true, dropPush: async () => undefined, finish: (result) => void results.push(result) });
  assert.deepEqual(results, [{ failures: [], stillListed: "notAdmin" }]);
  assert.deepEqual(replaced, [], "the page is not sent to its sign-in screen");
  assert.equal(tab.values.size, 0, "nothing kept for a sign-in screen on this origin");
  assert.deepEqual(await kept(d), { keys: false, id: false });
});

test("a hub that does not answer cannot keep the keys here: the wipe follows the removal at once, and each hub call is capped", async () => {
  const { hub, a } = await account();
  const c = await pairBrowser(hub, a, { admin: true });
  setHubShell(null);
  const never = () => new Promise<never>(() => undefined);
  // The hub takes the request and never answers: neither the removal nor the logout.
  (c.api as unknown as { appendChain: unknown }).appendChain = never;
  (c.api as unknown as { logout: unknown }).logout = never;
  replaced.length = 0;
  const started = Date.now();
  await signOut(c, undefined, { removeBrowser: true, dropPush: never, hubWithinMs: 50 });
  assert.ok(Date.now() - started < 8_000, "on without the hub");
  assert.deepEqual(replaced, ["/signin"]);
  assert.deepEqual(await kept(c), { keys: false, id: false }, "wiped all the same");
  assert.match(takeStillListed() ?? "", /^failed: the hub did not answer within/);
  await a.sync();
  assert.equal(a.chain!.deviceRecord(c.deviceId)?.status.type, "active", "it is still listed: the sign-in screen said so");
});

test("a wipe tries every step and says each that failed", async () => {
  const { hub, a } = await account();
  const c = await pairBrowser(hub, a, { admin: true });
  // The key store will not delete the device keys; every later step still runs.
  const store = c.store as unknown as { delete(name: string): Promise<void> };
  const del = store.delete.bind(store);
  store.delete = async (name: string) => {
    if (name === "device-keys/v2") throw new Error(`cannot delete ${name}`);
    return del(name);
  };
  const error = await c.wipe().then(
    () => null,
    (e: unknown) => e,
  );
  assert.ok(error instanceof WipeError, String(error));
  assert.ok(error.failures.some((f) => /^device keys: cannot delete device-keys\/v2/.test(f)), error.failures.join(" | "));
  assert.equal(error.failures.length, 1, "only that step failed");
  assert.equal(browserStorage(c.store).values.has(`rh.hub.device.${c.account}`), false, "the steps after the failed ones ran: its id is gone");
  // Through a sign-out, each failure is kept for the sign-in screen.
  tab.values.delete(SIGN_OUT_FAILURES_KEY);
  await signOut(c, undefined, { removeBrowser: true, dropPush: async () => undefined, hubWithinMs: 2_000 });
  const failures = takeSignOutFailures() ?? [];
  assert.ok(failures.some((f) => f.startsWith("device keys: device keys: cannot delete")), failures.join(" | "));
  takeStillListed();
});

test("a screen that could not open the account removes the browser as a local wipe", async () => {
  const { hub, a } = await account();
  const c = await pairBrowser(hub, a, { admin: true });
  const ended: string[] = [];
  setHubShell(null);
  replaced.length = 0;
  // Nothing kept for an account: nothing to fail.
  await CloudAccount.wipeLocal(new MemoryKeyStore(), "nobody", { storage: browserStorage(new MemoryKeyStore()) });
  // No account object: only the session it held, whose account it was, and the browser's key store.
  const held = { api: { logout: async (token: string) => void ended.push(token) }, token: c.session.token, account: c.account, store: c.store };
  // `wipeLocal` reads the browser's storage: the page's localStorage, which these tests give the fake hub's.
  const g2 = globalThis as { localStorage?: unknown };
  const before = g2.localStorage;
  g2.localStorage = browserStorage(c.store);
  try {
    await signOut(undefined, held, { removeBrowser: true, dropPush: async () => undefined });
  } finally {
    g2.localStorage = before;
  }
  assert.deepEqual(ended, [c.session.token], "the session it held is ended");
  assert.deepEqual(replaced, ["/signin"]);
  assert.deepEqual(await kept(c), { keys: false, id: false });
  assert.equal(takeStillListed(), "unopened");
  // Without "remove", the same screen keeps them.
  const d = await pairBrowser(hub, a, { admin: false });
  await signOut(undefined, { api: { logout: async () => undefined }, token: d.session.token, account: d.account, store: d.store }, { dropPush: async () => undefined });
  assert.deepEqual(await kept(d), { keys: true, id: true });
  assert.equal(takeStillListed(), null);
});
