// A new account is on chain v3 from its genesis (docs/local-aggregator.md
// §3.5), against the fake hub.
//
// What these pin: `createGenesis` writes `genesis3` (this browser the first
// admin, a device that runs no AI: review D1) and user key epoch 0 as a v3
// `keyEpochs` — where `upgradeToV3` takes an account from before, so nothing
// offers or makes an upgrade; a hub that takes no v3 entries gets no account
// at all, never a v2 one in its place, in words that say why; and the
// everyday flows on such an account (a computer, a paired browser, the
// recovery words, a removal) write v3 only.
import assert from "node:assert/strict";
import test from "node:test";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "../src/i18n/cloud-devices.js";
import { AccountError, type CloudAccount } from "../src/lib/hub/account.js";
import { chainV3Writable, type HubApi, MAIL_USER_KEY } from "../src/lib/hub/api.js";
import { candidateFromOffer, PairingResponder } from "../src/lib/hub/pairing.js";
import { createV2Genesis, FakeCourier, FakeKeysHub, pairBrowser } from "./fake-keys-hub.js";

const kinds = (a: CloudAccount, from = 0) =>
  a
    .chain!.allEntries()
    .slice(from)
    .map((e) => e.draft.action.type);
const versions = (a: CloudAccount, from = 0) =>
  a
    .chain!.allEntries()
    .slice(from)
    .map((e) => e.draft.version);

const NO_NEW_ACCOUNTS = "New accounts cannot be set up right now: the hub does not take chain v3 changes yet — that is on Droppen's side. Try again later.";
const noNewAccounts = (e: unknown) => {
  assert.ok(e instanceof AccountError && e.kind === "notWritable", String(e));
  assert.equal(e.key, NO_NEW_ACCOUNTS);
  return true;
};

/** What the page passes for `writable`: `/v1/me`'s `chain_v3.writable`. */
const writable = async (a: CloudAccount) => chainV3Writable(await a.api.me(a.session.token));

/** A new browser paired from `a` by QR, as a full device (every browser is one: local-aggregator §3.1). */
test("a new account is chain v3 from its genesis: genesis3 names this browser its first admin, user key 0 goes as v3 — nothing is left to upgrade", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  const words = await a.createGenesis(true);
  assert.equal(words?.length, 12, "the recovery words, shown once");
  assert.deepEqual(kinds(a), ["genesis3", "keyEpochs"]);
  assert.deepEqual(versions(a), [3, 3]);
  assert.equal(hub.chainPosts, 1, "in one call");
  const genesis = a.chain!.entryAt(0)!.draft;
  assert.equal(genesis.signer.type, "account");
  assert.ok(genesis.action.type === "genesis3" && genesis.action.recoveryKey, "the recovery key is in genesis");
  assert.ok(genesis.action.type === "genesis3" && genesis.action.device.id === a.deviceId);
  assert.deepEqual(genesis.action.type === "genesis3" && genesis.action.device.flags, { host: false, admin: true }, "an admin that runs no AI (D1)");
  const first = a.chain!.entryAt(1)!.draft.action;
  assert.ok(first.type === "keyEpochs" && first.user?.epoch === 0 && first.machines.length === 0);
  assert.equal(a.chain!.isV3, true);
  assert.equal(a.chain!.v3From, 0);
  assert.ok(a.inChain && a.pinned && a.isAdmin);
  assert.deepEqual(await a.vault.epochs(), [0], "user key 0 kept, as committed");
  assert.ok(
    [...hub.keys.keys()].some((name) => name.startsWith(`rk/0/${a.deviceId}/`)),
    "and sealed for the recovery words",
  );
  // Where `upgradeToV3` takes an account from before: nothing to offer, nothing to make.
  assert.equal(a.upgradePlan(), null);
  await assert.rejects(a.upgradeToV3({ writable: true }), (e: unknown) => e instanceof AccountError && e.rule === "upgraded");
  assert.equal(hub.chainPosts, 1, "nothing more written");
  // The hub says so too.
  const view = (await a.api.me(a.session.token)).chainV3;
  assert.deepEqual(view, { mode: "write", writable: true, waiting: [], inUse: true });
  // An account from before chain v3 (`createV2Genesis` stands for one) still has its one step to take.
  const old = await new FakeKeysHub().device("tok");
  await createV2Genesis(old, true);
  assert.deepEqual(versions(old), [2, 2]);
  assert.deepEqual(old.upgradePlan(), { version: 3, ukEpoch: 1, machines: 0 });
});

test("while the hub takes no chain v3 entries no account is made — and no v2 one in its place; said in words", async () => {
  for (const mode of ["read", "off"] as const) {
    const hub = new FakeKeysHub();
    hub.chainV3Mode = mode;
    const a = await hub.device("tok");
    await assert.rejects(a.createGenesis(true), noNewAccounts);
    assert.equal(hub.chainPosts, 0, `${mode}: refused before anything is signed`);
    assert.equal(a.chain, null);
    assert.equal(hub.chain.length, 0);
  }
  // A hub from before chain v3 (its `/v1/me` says nothing of it): the same.
  const before = new FakeKeysHub();
  const b = await before.device("tok");
  const me = b.api.me.bind(b.api);
  b.api.me = async (...args: Parameters<HubApi["me"]>) => ({ ...(await me(...args)), chainV3: null });
  await assert.rejects(b.createGenesis(false), noNewAccounts);
  assert.equal(before.chainPosts, 0);
  // The hub turned v3 writes off between `/v1/me` and the append (409 `chain_v3_not_ready`): the same words, and still no chain.
  const flip = new FakeKeysHub();
  flip.chainV3Mode = "read";
  const c = await flip.device("tok");
  await assert.rejects(c.createGenesis(true, { writable: true }), noNewAccounts);
  assert.equal(flip.chainPosts, 1, "the hub refused it");
  assert.equal(c.chain, null);
  assert.equal(flip.chain.length, 0);
  // Back on: the same browser makes it, on v3.
  flip.chainV3Mode = "write";
  assert.equal((await c.createGenesis(true))?.length, 12);
  assert.deepEqual(versions(c), [3, 3]);
  for (const table of [cloudDevicesZhCN, cloudDevicesZhTW, cloudDevicesJa]) assert.ok(table[NO_NEW_ACCOUNTS], "in every language the page speaks");
});

test("on an account made on v3, a computer, a paired browser, the recovery words and a removal all write v3", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  const words = (await a.createGenesis(true))!;
  // A computer: a device that runs AI, its first content key in the same call.
  let at = a.chain!.seq + 1;
  await hub.enroll(a, "m-1", { writable: await writable(a) });
  assert.deepEqual(kinds(a, at), ["addDevice3", "keyEpochs"]);
  const host = a.chain!.entryAt(at)!.draft.action;
  assert.ok(host.type === "addDevice3" && host.device.flags.host && !host.device.flags.admin);
  assert.ok(a.chain!.machine("m-1"));
  // A browser, paired: a full device, with the user key.
  at = a.chain!.seq + 1;
  const b = await pairBrowser(hub, a);
  assert.deepEqual(kinds(a, at), ["addDevice3"]);
  assert.ok(b.isAdmin && b.standing === "active");
  assert.deepEqual(await b.vault.epochs(), [0]);
  assert.equal(b.upgradePlan(), null, "nothing to upgrade there either");
  // The recovery words: a v3 join (an admin, never a host), approved, then the user keys.
  const r = await hub.device("tok");
  assert.deepEqual((await r.recover(words.join(" "))).epochs, [0]);
  const join = hub.state!.entryAt(hub.state!.seq)!.draft;
  assert.ok(join.action.type === "recoveryJoin3" && join.action.device.flags.admin && !join.action.device.flags.host);
  await a.sync();
  await a.approveDevice(r.deviceId);
  assert.ok(hub.mailLog.some((m) => m.recipient === r.deviceId && m.purpose === MAIL_USER_KEY));
  await r.sync();
  assert.ok(r.standing === "active" && r.isAdmin);
  // A removal: the user key and the computer's content key turn over.
  at = a.chain!.seq + 1;
  await a.removeDevice(b.deviceId, new FakeCourier());
  assert.deepEqual(kinds(a, at), ["removeDevice3", "keyEpochs"]);
  assert.ok(versions(a).every((v) => v === 3), `every entry v3: ${versions(a).join(",")}`);
  assert.equal(a.chain!.v3From, 0);
});
