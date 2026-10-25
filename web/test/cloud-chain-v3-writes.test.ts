// What the browser writes once chain v3 is on (docs/local-aggregator.md §3,
// §4.3 step 7), against the fake hub.
//
// What these pin: what is written follows the chain (review D4) — v2's
// entries until it holds a v3 entry, and never a v2 device that is not full;
// the first v3 entry is the confirmed upgrade (a user key rotation written as
// v3), only while the hub takes v3 entries (`chain_v3.writable`); a host never
// manages the account (review D1); every batch is verified here before it is
// signed away (review 13); recovery words work on a v3 chain; a device that does not
// manage the account is mailed no user key ever (not in its pairing, not when
// it asks, not in a rotation), only the hosts' committed machine keys
// (purpose 5) — when it joins, when a host joins or runs AI again, after a
// rotation, and when it asks (purpose 4, at most once per 10 minutes); it
// takes them only from a current admin, for itself, as the chain committed
// them; removing it rotates machine keys alone; an enrollment waits a moment
// for the new host's `chain/3`; the 「可管理账号」 switch rules.
import assert from "node:assert/strict";
import test from "node:test";
import {
  type Action,
  ChainClock,
  ChainState,
  encodeEntry,
  encodeHostKeyMail,
  fromHex,
  generateDeviceKeys,
  hex,
  PASSKEY_DELAY_MS,
  PAUSE_COMPLETED,
  PAUSE_START,
  passkeySlotProof,
  RECOVERY_DELAY_MS,
  RULES,
  sealHostKey,
  signDraft,
} from "../src/lib/e2e/index.js";
import {
  AccountError,
  type CloudAccount,
  hasRunAi,
  heldUserKey,
  hostsToReEnroll,
  joinAction,
  reEnrollCulprits,
  reEnrollEvidence,
  KEY_REQUEST_EVERY_MS,
  limitedDevice,
  rotationDueIn,
  userKeyStale,
} from "../src/lib/hub/account.js";
import { ChainFollower, type FollowLink } from "../src/lib/hub/follow.js";
import { legacyMachineKeyName } from "../src/lib/hub/keyring.js";
import { HubApiError, MAIL_HOST_KEY, MAIL_KEY_REQUEST, MAIL_USER_KEY } from "../src/lib/hub/api.js";
import type { ControllersRecord } from "../src/lib/hub/streams.js";
import { candidateFromOffer, PairingResponder } from "../src/lib/hub/pairing.js";
import { addPasskeySlot, browserStorage, containsBytes, createV2Genesis, FakeCourier, FakeKeysHub, passkeyActivateEarly, passkeyJoin, postAsPasskey, RecordingKeyStore } from "./fake-keys-hub.js";
import { cloudDevicesZhCN } from "../src/i18n/cloud-devices.js";

const keys = (id: string) => ({ id, signKey: new Uint8Array(32).fill(1), dhKey: new Uint8Array(32).fill(2) });
const kinds = (a: CloudAccount, from = 0): Action["type"][] =>
  a
    .chain!.allEntries()
    .slice(from)
    .map((e) => e.draft.action.type);
const versions = (a: CloudAccount, from = 0): number[] =>
  a
    .chain!.allEntries()
    .slice(from)
    .map((e) => e.draft.version);
const mailTo = (hub: FakeKeysHub, id: string) => hub.mailLog.filter((m) => m.recipient === id);

const invalid = (rule: string) => (e: unknown) => {
  assert.ok(e instanceof AccountError && e.kind === "invalidEntry", String(e));
  assert.equal(e.rule, rule);
  return true;
};
const kindIs = (kind: AccountError["kind"]) => (e: unknown) => e instanceof AccountError && e.kind === kind;

/** A new browser paired from `a` (QR), as an admin or not. */
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

/** An admin browser with a host (m-1) enrolled the v2 way, and the hub as asked: a v2 chain. */
async function v2Account(mode: "read" | "write" = "write") {
  const hub = new FakeKeysHub();
  hub.chainV3Mode = mode;
  const a = await hub.device("tok");
  const words = (await createV2Genesis(a, true))!;
  await hub.enroll(a, "m-1");
  return { hub, a, words };
}

/** `v2Account`, upgraded to chain v3 (the confirmed step: review D4). */
async function account(mode: "read" | "write" = "write") {
  const { hub, a, words } = await v2Account("write");
  await a.upgradeToV3({ writable: true });
  hub.chainV3Mode = mode;
  return { hub, a, words };
}

/** Let `ids` control host `host` (they are handed its keys from then on: review D2). */
async function admit(a: CloudAccount, host: string, ...ids: string[]): Promise<void> {
  const now = a.chain!.acceptPolicy(host);
  const before = now.type === "devices" ? now.ids : [];
  await a.setAccept(host, { type: "devices", ids: [...before, ...ids] });
}


/**
 * Let `a`'s throttle on answering `to`'s key requests run out (security
 * review of S1-4, S4): `to` was sealed every key it may hold within the last
 * KEY_REQUEST_EVERY_MS (its pairing did that), so until then a request of
 * its gets only what is newer — as if the window were over.
 */
function windowOver(a: CloudAccount, to: string): void {
  const storage = (a as unknown as { storage: { getItem(k: string): string | null; setItem(k: string, v: string): void } }).storage;
  const key = `rh.hub.hostkeysent.${a.account}.${a.deviceId}.${to}`;
  const sent = JSON.parse(storage.getItem(key) ?? '{"atMs":0,"through":{}}') as { atMs: number; through: Record<string, number> };
  storage.setItem(key, JSON.stringify({ ...sent, atMs: Date.now() - KEY_REQUEST_EVERY_MS - 1 }));
}

function fromHexHost(sealed: string): string {
  const b = fromHex(sealed);
  const len = (b[0]! << 8) | b[1]!;
  return new TextDecoder().decode(b.subarray(2, 2 + len));
}

test("write selection follows the chain: v3 only on a v3 chain, v2's otherwise — never a v2 device that is not full, never a host that manages the account", () => {
  const d = keys("d-9");
  const v2 = { isV3: false };
  const v3 = { isV3: true };
  assert.deepEqual(joinAction(d, { host: false, admin: true }, v2), { type: "addDevice", device: { ...d, full: true } });
  assert.deepEqual(joinAction(d, { host: true, admin: false }, v2), { type: "addMachine", machine: d });
  assert.deepEqual(joinAction(d, { host: false, admin: false }, v3), { type: "addDevice3", device: { ...d, flags: { host: false, admin: false } } });
  assert.deepEqual(joinAction(d, { host: true, admin: false }, v3), { type: "addDevice3", device: { ...d, flags: { host: true, admin: false } } });
  assert.deepEqual(joinAction(d, { host: false, admin: true }, v3), { type: "addDevice3", device: { ...d, flags: { host: false, admin: true } } });
  assert.throws(() => joinAction(d, { host: false, admin: false }, v2), kindIs("notUpgraded"), "a v2 limited device is never written");
  for (const chain of [v2, v3]) assert.throws(() => joinAction(d, { host: true, admin: true }, chain), invalid(RULES.hostNotAdmin), "a host never manages the account (D1)");
});

test("D4: a v2 account stays v2 even while the hub takes v3 entries — every ordinary flow writes v2, and what only v3 says waits for the upgrade", async () => {
  const { hub, a } = await v2Account("write");
  const before = a.chain!.seq + 1;
  const b = await pairAs(hub, a, { admin: true });
  await hub.enroll(a, "m-2", { writable: true });
  const c = await pairAs(hub, a, { admin: true });
  const courier = new FakeCourier();
  await a.removeDevice(c.deviceId, courier);
  await a.rotate(courier);
  await a.removeDevice("m-2", courier);
  assert.deepEqual(kinds(a, before), ["addDevice", "addMachine", "keyEpochs", "addDevice", "removeDevice", "keyEpochs", "keyEpochs", "removeMachine"]);
  assert.ok(versions(a).every((v) => v === 2), "only v2 entries");
  assert.equal(a.chain!.isV3, false);
  assert.equal(b.isAdmin, true);
  assert.deepEqual(await b.vault.epochs(), [0], "an admin gets the user key in its delivery");
  // Only v3 says these: refused before the hub is asked anything.
  const seq = a.chain!.seq;
  const posts = hub.chainPosts;
  await assert.rejects(a.setAdmin(b.deviceId, false), kindIs("notUpgraded"));
  await assert.rejects(a.setHost("m-1", false, courier, { writable: true }), kindIs("notUpgraded"));
  await assert.rejects(a.setAccept("m-1", { type: "all" }), kindIs("notUpgraded"));
  const d = await hub.device("tok");
  await assert.rejects(a.pairDevice({ device: d.deviceId, signKey: d.keys.signPublic, dhKey: d.keys.dhPublic, secret: new Uint8Array(32), fingerprint: "" }, { admin: false }), kindIs("notUpgraded"));
  assert.equal(a.chain!.seq, seq, "nothing written");
  assert.equal(hub.chainPosts, posts, "nothing even posted");
  assert.equal(hub.notReady.length, 0, "no v3 entry was even tried");
});

test("D4: the upgrade is its own step — from an admin, while the hub takes v3 entries — and writes one v3 entry, a new user key", async () => {
  const { hub, a } = await v2Account("read");
  const c = await pairAs(hub, a, { admin: true });
  await c.receiveMail();
  assert.deepEqual(a.upgradePlan(), { version: 3, ukEpoch: 1, machines: 0 }, "what the page shows before the person confirms");
  // The hub does not take v3 entries: not offered, and refused if tried.
  await assert.rejects(a.upgradeToV3({ writable: false }), kindIs("notWritable"));
  await assert.rejects(a.upgradeToV3({ writable: true }), (e: unknown) => e instanceof HubApiError && e.code === "chain_v3_not_ready", "the hub's word is the hub's");
  assert.equal(a.chain!.isV3, false);
  hub.chainV3Mode = "write";
  const at = a.chain!.seq + 1;
  const report = await a.upgradeToV3({ writable: true });
  assert.deepEqual(kinds(a, at), ["keyEpochs"]);
  assert.deepEqual(versions(a, at), [3]);
  const ke = a.chain!.entryAt(at)!.draft.action;
  assert.ok(ke.type === "keyEpochs" && ke.user?.epoch === 1 && ke.machines.length === 0, "a user key alone: no machine key changes hands");
  assert.equal(a.chain!.isV3, true);
  assert.equal(a.chain!.v3From, at);
  assert.equal(report.ukEpoch, 1);
  assert.deepEqual(report.sealedTo, [c.deviceId], "the other admin gets the new user key");
  assert.deepEqual(report.machines, []);
  assert.equal(a.upgradePlan(), null);
  await assert.rejects(a.upgradeToV3({ writable: true }), kindIs("invalidEntry"), "once");
  await c.sync();
  await c.receiveMail();
  assert.deepEqual(await c.vault.epochs(), [0, 1]);
  assert.deepEqual(
    c.alerts().map((x) => x.kind),
    ["upgrade"],
    "the other admin hears of it as the upgrade",
  );
  // From now on everything is v3, the shared actions too.
  await a.rotate(new FakeCourier());
  assert.ok(versions(a, at).every((v) => v === 3));
});

test("pairing a device that does not manage the account (a v3 chain): addDevice3{admin: false}, no user key ever, a host's keys by purpose 5 once it accepts the device", async () => {
  const { hub, a } = await account();
  const before = a.chain!.seq + 1;
  const b = await pairAs(hub, a, { admin: false });
  assert.deepEqual(kinds(a, before), ["addDevice3"]);
  const added = a.chain!.allEntries().at(-1)!.draft.action;
  assert.ok(added.type === "addDevice3" && !added.device.flags.admin && !added.device.flags.host);
  assert.equal(b.isAdmin, false);
  assert.ok(b.pinned, "the empty delivery pins the account all the same");
  assert.ok(!mailTo(hub, b.deviceId).some((m) => m.purpose === MAIL_HOST_KEY), "no host's keys: none accepts it yet (D2)");
  await admit(a, "m-1", b.deviceId);
  assert.ok(!mailTo(hub, b.deviceId).some((m) => m.purpose === MAIL_USER_KEY), "never a user key");
  await b.sync();
  await b.receiveMail();
  assert.deepEqual(await b.vault.epochs(), [], "it holds no user key");
  const latest = a.chain!.latestMachineEpoch("m-1")!;
  assert.ok((await b.machineKeys("m-1")).has(latest), "and reads m-1 with its keys");
  assert.equal(await b.lacksKeys(), false, "no user key is not a lack for it");
  assert.deepEqual(await b.checkHostKeys(), []);
  // The key at rest is encrypted under the device-local key, never bytes.
  const mk = (await a.machineKeyBytes("m-1", latest))!;
  const { containsBytes } = await import("./fake-keys-hub.js");
  assert.ok(!(b.store as RecordingKeyStore).puts.some((p) => containsBytes(p.value, mk)), "no raw machine key in the key store");
});

test("a device that does not manage the account moves raw machine keys from before the keyring into its sealed vault — every one the chain committed, past a gap too; one it did not commit is left", async () => {
  const { hub, a } = await account();
  await hub.enroll(a, "m-2", { writable: true });
  const b = await pairAs(hub, a, { admin: false });
  await admit(a, "m-1", b.deviceId);
  await a.rotate(new FakeCourier());
  // What an admin mailed it is not what this is about: as if it never came.
  hub.mail = hub.mail.filter((m) => m.recipient !== b.deviceId);
  await b.sync();
  assert.equal(b.isAdmin, false);
  assert.deepEqual(await b.hostKeys.epochs("m-1"), []);
  const n1 = a.chain!.machineKeyEpochs("m-1");
  assert.ok(n1 >= 2, String(n1));
  // As M1/M2a left them, as hex — but m-1's epoch 0 already went in a run cut short; and m-2's first, a host that does not let it in now.
  const store = b.store as RecordingKeyStore;
  const m1 = new Map<number, Uint8Array>();
  for (let e = 1; e < n1; e++) m1.set(e, (await a.machineKeyBytes("m-1", e))!);
  const m2 = (await a.machineKeyBytes("m-2", 0))!;
  for (const [e, key] of m1) await store.put(legacyMachineKeyName("m-1", e), hex(key));
  const uncommitted = hex(crypto.getRandomValues(new Uint8Array(32)));
  await store.put(legacyMachineKeyName("m-1", n1), uncommitted);
  await store.put(legacyMachineKeyName("m-2", 0), hex(m2));
  const before = store.puts.length;

  await b.settle();
  for (const e of m1.keys()) assert.equal(await store.get(legacyMachineKeyName("m-1", e)), undefined, `m-1 epoch ${e} is not left raw`);
  assert.equal(await store.get(legacyMachineKeyName("m-2", 0)), undefined, "nor m-2's");
  assert.equal(await store.get(legacyMachineKeyName("m-1", n1)), uncommitted, "a key the chain did not commit is left as it was");
  assert.deepEqual(await b.hostKeys.epochs("m-1"), [...m1.keys()], "m-1's, sealed in its vault");
  assert.deepEqual([...(await b.machineKeys("m-1")).keys()].sort((x, y) => x - y), [...m1.keys()], "and it reads m-1 with them");
  assert.deepEqual(await b.hostKeys.epochs("m-2"), [0], "m-2's too, though m-2 does not let it in now: no raw copy goes without a sealed one");
  assert.deepEqual(b.missingHostKeys, []);
  for (const { name, value } of store.puts.slice(before)) {
    for (const key of [...m1.values(), m2]) assert.ok(!containsBytes(value, key), `no raw machine key written under ${name}`);
  }
});

test("a raw machine key from before the keyring stays while another page is moving them, or while its sealed copy does not open to it", async () => {
  const { hub, a } = await account();
  const b = await pairAs(hub, a, { admin: false });
  await admit(a, "m-1", b.deviceId);
  hub.mail = hub.mail.filter((m) => m.recipient !== b.deviceId);
  await b.sync();
  const store = b.store as RecordingKeyStore;
  const mk = (await a.machineKeyBytes("m-1", 0))!;
  await store.put(legacyMachineKeyName("m-1", 0), hex(mk));
  type Locks = { request<T>(name: string, options: { mode: "exclusive"; ifAvailable?: boolean }, callback: (lock?: unknown) => T | Promise<T>): Promise<T> };
  const page = b as unknown as { locks: Locks | null };
  // Another page holds the migration's lock: this one skips it (no waiting on that page), and leaves it to the next settle.
  const asked: { name: string; ifAvailable?: boolean }[] = [];
  page.locks = {
    request: async (name, options, callback) => {
      asked.push({ name, ifAvailable: options.ifAvailable });
      return callback(options.ifAvailable ? null : {});
    },
  };
  assert.equal(await b.migrate(), 0);
  assert.deepEqual(asked, [{ name: "rh-legacy-machine-keys", ifAvailable: true }]);
  assert.equal(await store.get(legacyMachineKeyName("m-1", 0)), hex(mk), "left for the next settle");
  // The lock is free, but another page replaced the device-local key this page sealed under: the sealed copy would not open — the raw one stays.
  page.locks = { request: async (_name, _options, callback) => callback({}) };
  await b.hostKeys.key("m-1", 99).catch(() => undefined);
  await store.put("local-wrap/v1", await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt", "unwrapKey"]));
  assert.equal(await b.migrate(), 0);
  assert.equal(await store.get(legacyMachineKeyName("m-1", 0)), hex(mk), "kept: its sealed copy does not open to it");
});

test("enrollment on a v3 chain: [addDevice3{host}, keyEpochs] in one append — only while the hub takes v3 entries; a host never manages the account (D1)", async () => {
  const { hub, a } = await account();
  const before = a.chain!.seq + 1;
  const appends = hub.chain.length;
  await hub.enroll(a, "m-2", { writable: true });
  assert.deepEqual(kinds(a, before), ["addDevice3", "keyEpochs"]);
  assert.equal(hub.chain.length, appends + 2);
  const added = a.chain!.entryAt(before)!.draft.action;
  assert.ok(added.type === "addDevice3" && added.device.flags.host && !added.device.flags.admin);
  assert.ok(a.chain!.machine("m-2"));
  // The hub gates a new host on a v3 chain: said before anything is asked of it.
  const posts = hub.chainPosts;
  await assert.rejects(hub.enroll(a, "m-3", { writable: false }), kindIs("notWritable"));
  assert.equal(hub.chainPosts, posts);
  // Never an admin, by any way in.
  await assert.rejects(a.setAdmin("m-2", true), invalid(RULES.hostNotAdmin));
  const b = await pairAs(hub, a, { admin: true });
  await assert.rejects(a.setHost(b.deviceId, true, new FakeCourier(), { writable: true }), invalid(RULES.hostNotAdmin));
  assert.equal(hub.chainPosts, posts + 1, "only the pairing reached the hub");
});

test("409 chain_v3_not_ready naming only the host being added is tried again for a moment; naming another host is not", async () => {
  const { hub, a } = await account();
  // The new host's agent declares chain/3 a moment after its proof.
  hub.undeclared.add("m-2");
  hub.onNotReady = (hosts) => {
    if (hosts.join() === "m-2") setTimeout(() => hub.undeclared.delete("m-2"), 15);
  };
  await hub.enroll(a, "m-2", { writable: true, wait: { forMs: 2000, everyMs: 10 } });
  assert.ok(hub.notReady.length >= 1 && hub.notReady.every((h) => h.join() === "m-2"));
  assert.ok(a.chain!.machine("m-2"));
  // A current host still on an old Droppen: said at once (update it first), not waited for.
  hub.undeclared.add("m-1");
  const tries = hub.notReady.length;
  const started = Date.now();
  await assert.rejects(hub.enroll(a, "m-3", { writable: true, wait: { forMs: 2000, everyMs: 10 } }), (e: unknown) => {
    assert.ok(e instanceof HubApiError && e.code === "chain_v3_not_ready", String(e));
    assert.deepEqual(e.waitingHosts, ["m-1"]);
    return true;
  });
  assert.equal(hub.notReady.length, tries + 1, "asked once");
  assert.ok(Date.now() - started < 1500);
  // What the hub does not gate on a v3 chain goes on all the same (a removal, a rotation).
  await a.removeDevice("m-2", new FakeCourier());
  assert.equal(hub.notReady.length, tries + 1);
  // And it waits no longer than it says.
  hub.undeclared.clear();
  hub.undeclared.add("m-4");
  hub.onNotReady = null;
  await assert.rejects(hub.enroll(a, "m-4", { writable: true, wait: { forMs: 60, everyMs: 10 } }), (e: unknown) => e instanceof HubApiError && e.code === "chain_v3_not_ready");
});

test("13: a batch the chain would refuse is refused here, by its rule, before anything is signed away", async () => {
  const { hub, a } = await account();
  const posts = hub.chainPosts;
  const seq = a.chain!.seq;
  // A host that manages the account (D1).
  await assert.rejects(a.append({ type: "setAdmin", id: "m-1", admin: true }), invalid(RULES.hostNotAdmin));
  // A v2 entry on a v3 chain.
  await assert.rejects(a.append({ type: "removeMachine", id: "m-1" }), invalid(RULES.v2AfterV3));
  // Nothing at all.
  await assert.rejects(a.append({ type: "setAccept", host: "m-1", from: { type: "admins" } }), invalid(RULES.acceptSame));
  // A host made one again without its next machine key epoch (the hub refuses a batch that ends owing it).
  await a.setHost("m-1", false, new FakeCourier());
  const stopped = hub.chainPosts;
  await assert.rejects(a.append({ type: "setHost", id: "m-1", host: true }), invalid(RULES.hostKeyNext));
  // The last admin.
  await assert.rejects(a.append({ type: "setAdmin", id: a.deviceId, admin: false }), invalid("lastAdmin"));
  assert.equal(hub.chainPosts, stopped, "none of them reached the hub");
  assert.equal(stopped, posts + 1);
  assert.equal(a.chain!.seq, seq + 1);
});

test("removal follows the chain, never the hub's say (D4): v2 on a v2 chain the hub would take v3 for, v3 on a v3 chain the hub reads only", async () => {
  const { hub, a } = await v2Account("write");
  const courier = new FakeCourier();
  const c = await pairAs(hub, a, { admin: true });
  const at = a.chain!.seq + 1;
  await a.removeDevice(c.deviceId, courier);
  assert.deepEqual(kinds(a, at), ["removeDevice", "keyEpochs"]);
  assert.deepEqual(versions(a, at), [2, 2]);
  const v3 = await account("read");
  const d = await pairAs(v3.hub, v3.a, { admin: true });
  const at3 = v3.a.chain!.seq + 1;
  await v3.a.removeDevice(d.deviceId, courier);
  assert.deepEqual(kinds(v3.a, at3), ["removeDevice3", "keyEpochs"]);
  assert.deepEqual(versions(v3.a, at3), [3, 3]);
  await v3.a.removeDevice("m-1", courier);
  assert.equal(kinds(v3.a).at(-1), "removeDevice3", "a host too, by removeDevice3");
});

test("D4: the recovery words on a v3 chain — a v3 join (an admin, never a host), then approval, then the user keys", async () => {
  const { hub, a, words } = await account("read");
  const r = await hub.device("tok");
  const { epochs } = await r.recover(words.join(" "));
  assert.deepEqual(epochs, [0, 1], "the packages open, the upgrade's epoch included");
  const join = hub.state!.entryAt(hub.state!.seq)!;
  assert.equal(join.draft.version, 3);
  assert.equal(join.draft.signer.type, "recovery");
  assert.ok(join.draft.action.type === "recoveryJoin3" && join.draft.action.device.flags.admin && !join.draft.action.device.flags.host);
  assert.equal(r.standing, "pending");
  await a.sync();
  assert.deepEqual(a.chain!.pendingRecoveries().map((p) => p.id), [r.deviceId]);
  await a.approveDevice(r.deviceId);
  const approve = a.chain!.allEntries().at(-1)!.draft;
  assert.ok(approve.action.type === "approveDevice" && approve.version === 3);
  assert.ok(mailTo(hub, r.deviceId).some((m) => m.purpose === MAIL_USER_KEY), "the user keys to the approved admin");
  await r.sync();
  assert.equal(r.standing, "active");
  assert.equal(r.isAdmin, true);
  await r.receiveMail();
  assert.deepEqual(await r.vault.epochs(), [0, 1]);
  // It manages the account: it signs (v3) entries.
  await r.setRecoveryWords();
  assert.equal(r.chain!.allEntries().at(-1)!.draft.version, 3);
});

test("a key request: an admin asking gets user keys; a device that is not one gets the keys of the hosts that accept it — never a user key", async () => {
  const { hub, a } = await account();
  const b = await pairAs(hub, a, { admin: false });
  const c = await pairAs(hub, a, { admin: true });
  await c.receiveMail();
  await admit(a, "m-1", b.deviceId);
  // b lost its mail (say): it asks the admins.
  hub.mail = hub.mail.filter((m) => m.recipient !== b.deviceId);
  await b.sync();
  const asked = await b.requestHostKeys(true);
  assert.deepEqual(asked.sort(), [a.deviceId, c.deviceId].sort());
  assert.ok(hub.mailLog.filter((m) => m.sender === b.deviceId).every((m) => m.purpose === MAIL_KEY_REQUEST && m.epoch === 0));
  const before = mailTo(hub, b.deviceId).length;
  // Within the window of its pairing's keys a request brings only what is newer: nothing here.
  await a.receiveMail();
  assert.equal(mailTo(hub, b.deviceId).length, before, "answered in full at most once per window (S4)");
  await b.requestHostKeys(true);
  windowOver(a, b.deviceId);
  await a.receiveMail();
  const answers = mailTo(hub, b.deviceId).slice(before);
  assert.ok(answers.length >= 1);
  assert.ok(answers.every((m) => m.purpose === MAIL_HOST_KEY), "host keys only");
  assert.ok(!hub.mailLog.some((m) => m.recipient === b.deviceId && m.purpose === MAIL_USER_KEY), "never a user key to it");
  await b.receiveMail();
  assert.ok((await b.machineKeys("m-1")).has(a.chain!.latestMachineEpoch("m-1")!));
  // An admin missing a user key epoch asks with its epoch, and gets user keys.
  const ukBefore = mailTo(hub, c.deviceId).filter((m) => m.purpose === MAIL_USER_KEY).length;
  await c.requestUserKeys(0, true);
  await a.receiveMail();
  assert.ok(mailTo(hub, c.deviceId).filter((m) => m.purpose === MAIL_USER_KEY).length > ukBefore);
  // The hub would refuse a user key to b in any case.
  await assert.rejects(
    a.api.mail(a.session.token, { recipient: b.deviceId, sender: a.deviceId, purpose: MAIL_USER_KEY, epoch: 0, sealed: new Uint8Array(80) }),
    (e: unknown) => e instanceof HubApiError && e.code === "not_an_admin",
  );
});

test("9: key requests go to admins only — never to a device that does not manage the account, nor to a host", async () => {
  const { hub, a } = await account();
  const b = await pairAs(hub, a, { admin: false });
  const d = await pairAs(hub, a, { admin: false });
  await a.sync();
  const asking = (who: CloudAccount) => hub.mailAttempts.filter((m) => m.startsWith(`${who.deviceId} `) && m.endsWith(` ${MAIL_KEY_REQUEST}`)).map((m) => m.split(" ")[1]);
  await b.sync();
  await b.requestHostKeys(true);
  assert.deepEqual(asking(b), [a.deviceId], "b asks a, and only a");
  const c = await pairAs(hub, a, { admin: true });
  await c.receiveMail();
  assert.deepEqual(await c.requestUserKeys(0, true), [a.deviceId], "an admin asks the other admins only");
  assert.deepEqual(asking(c), [a.deviceId], "not b, d, nor m-1");
  assert.ok(d.deviceId);
});

test("a device that is not an admin asks for a missing host key at most once per 10 minutes, and only admins", async () => {
  const { hub, a } = await account();
  const b = await pairAs(hub, a, { admin: false });
  await admit(a, "m-1", b.deviceId);
  await b.sync();
  hub.mail = hub.mail.filter((m) => m.recipient !== b.deviceId);
  assert.deepEqual(await b.checkHostKeys(), ["m-1"]);
  assert.deepEqual(b.missingHostKeys, ["m-1"]);
  const asks = () => hub.mailLog.filter((m) => m.sender === b.deviceId && m.purpose === MAIL_KEY_REQUEST);
  assert.equal(asks().length, 1);
  assert.equal(asks()[0]!.recipient, a.deviceId, "an admin");
  await b.checkHostKeys();
  assert.equal(asks().length, 1, "not again within the window");
  assert.ok(KEY_REQUEST_EVERY_MS === 10 * 60 * 1000);
  // The admin answers (its own window for b, from b's pairing, over); the gap closes.
  windowOver(a, b.deviceId);
  await a.receiveMail();
  assert.deepEqual(await b.checkHostKeys(), []);
  assert.deepEqual(b.missingHostKeys, []);
  // It never asks for a user key.
  assert.equal(await b.checkUserKey(), null);
  assert.equal(b.missingUserKey, null);
});

test("host keys: taken only from a current admin, addressed to this device, for the host and epoch the chain committed", async () => {
  const { hub, a } = await account();
  const b = await pairAs(hub, a, { admin: false });
  const d = await pairAs(hub, a, { admin: false });
  await hub.enroll(a, "m-2", { writable: true });
  hub.mail = hub.mail.filter((m) => m.recipient !== b.deviceId);
  await b.sync();
  const mk0 = (await a.machineKeyBytes("m-1", 0))!;
  let id = 90_000;
  const plant = async (opts: { sender: CloudAccount; host: string; sealedHost?: string; epoch: number; sealedEpoch?: number; recipient?: string; key?: Uint8Array }) => {
    const sealed = await sealHostKey(
      opts.sender.noiseKeys,
      b.keys.dhPublic,
      { account: hub.account, sender: opts.sender.deviceId, recipient: opts.recipient ?? b.deviceId, host: opts.sealedHost ?? opts.host, epoch: opts.sealedEpoch ?? opts.epoch },
      opts.key ?? mk0,
    );
    hub.mail.push({ id: ++id, recipient: b.deviceId, sender: opts.sender.deviceId, purpose: MAIL_HOST_KEY, epoch: opts.epoch, sealed: hex(encodeHostKeyMail(opts.host, sealed)) });
  };
  // m-1's epoch-0 key, named as m-2's: not what the chain committed for m-2.
  await plant({ sender: a, host: "m-2", epoch: 0 });
  // The right key under another epoch.
  await plant({ sender: a, host: "m-1", epoch: 1, sealedEpoch: 1 });
  // From a device that is not an admin.
  await plant({ sender: d, host: "m-1", epoch: 0 });
  // Sealed for another device.
  await plant({ sender: a, host: "m-1", epoch: 0, recipient: d.deviceId });
  assert.equal(await b.receiveMail(), 0, "all dropped");
  assert.deepEqual(await b.hostKeys.hosts(), []);
  assert.equal(hub.mail.filter((m) => m.recipient === b.deviceId).length, 0, "and acknowledged");
  // The real thing.
  await plant({ sender: a, host: "m-1", epoch: 0 });
  assert.equal(await b.receiveMail(), 1);
  assert.deepEqual(await b.hostKeys.epochs("m-1"), [0]);
  mk0.fill(0);
});

test("setAdmin, setHost and setAccept write v3 entries; granting manage mails the user keys", async () => {
  const { hub, a } = await account();
  const b = await pairAs(hub, a, { admin: false });
  await assert.rejects(a.setAdmin(a.deviceId, false), kindIs("lastAdmin"));
  await a.setAdmin(b.deviceId, true);
  assert.ok(a.chain!.admin(b.deviceId));
  assert.ok(mailTo(hub, b.deviceId).some((m) => m.purpose === MAIL_USER_KEY), "the user key, once it manages the account");
  await b.sync();
  await b.receiveMail();
  assert.deepEqual(await b.vault.epochs(), [0, 1]);
  assert.equal(b.isAdmin, true);
  await b.setAdmin(a.deviceId, false);
  await a.sync();
  assert.equal(a.isAdmin, false);
  // Never on for itself (and a device that is not an admin changes nothing).
  await assert.rejects(a.setAdmin(a.deviceId, true), kindIs("notAdmin"));
  await b.setAdmin(a.deviceId, true);
  await a.sync();
  // Hosting.
  const courier = new FakeCourier();
  const at = a.chain!.seq + 1;
  await a.setHost("m-1", false, courier);
  assert.equal(a.chain!.machine("m-1"), undefined);
  assert.ok(a.chain!.device("m-1"), "still a device");
  await assert.rejects(a.setHost("m-1", true, courier, { writable: false }), kindIs("notWritable"), "the hub gates a host made one again");
  const again = await a.setHost("m-1", true, courier, { writable: true });
  assert.deepEqual(kinds(a, at), ["setHost", "setHost", "keyEpochs"]);
  assert.equal(again?.mkEpoch, a.chain!.latestMachineEpoch("m-1"));
  assert.equal(courier.posted.at(-1)?.epoch, again?.mkEpoch);
  await assert.rejects(a.setHost(b.deviceId, true, courier, { writable: true }), (e: unknown) => e instanceof AccountError, "an admin never runs AI (and it never did)");
  // Whom a host accepts.
  const c = await pairAs(hub, a, { admin: false });
  await a.setAccept("m-1", { type: "devices", ids: [c.deviceId, c.deviceId, "m-1"] });
  assert.deepEqual(a.chain!.acceptPolicy("m-1"), { type: "devices", ids: [c.deviceId] });
  await a.setAccept("m-1", { type: "devices", ids: [] }, courier);
  assert.deepEqual(a.chain!.acceptPolicy("m-1"), { type: "admins" });
  assert.ok(a.chain!.allEntries().every((e) => e.draft.seq < a.chain!.v3From! || e.draft.version === 3));
});

test("a limited device is a current browser or phone that is not a full device; a full device makes it one — not its own row, nothing until the account is upgraded", () => {
  const rec = (id: string, admin: boolean, host = false, status: "active" | "removed" | "pending" = "active", ranAi = host) =>
    ({ info: { id, signKey: new Uint8Array(32), dhKey: new Uint8Array(32) }, host, admin, ranAi, status: { type: status }, viaRecovery: false, viaPasskey: null, addedSeq: 0, activatedSeq: 0 }) as const;
  const of = (viewer: string, viewerAdmin: boolean, device: ReturnType<typeof rec>, upgraded = true, ranAi?: boolean) =>
    limitedDevice({ viewer, viewerAdmin, device: device as never, upgraded, ranAi });
  assert.deepEqual(of("d-1", true, rec("d-2", false)), { block: null }, "a full device may make it one");
  assert.deepEqual(of("d-1", true, rec("d-2", false), false), { block: "notUpgraded" }, "chain v2: upgrade first");
  assert.deepEqual(of("d-3", false, rec("d-2", false)), { block: "notAdmin" }, "a limited device sees a read-only list");
  assert.deepEqual(of("d-2", false, rec("d-2", false)), { block: "self" }, "its own row: another device makes it one");
  assert.equal(of("d-1", true, rec("d-2", true)), null, "a full device");
  assert.equal(of("d-1", true, rec("m-1", false, true)), null, "a computer that runs AI is never one (D1)");
  assert.equal(of("d-1", true, rec("m-2", false, false, "active", true)), null, "nor one that ran it and stopped (D1 extended: the record's own ranAi)");
  assert.equal(of("d-1", true, rec("m-3", false), true, true), null, "nor one the chain committed machine keys for (hasRunAi, passed in)");
  assert.equal(of("d-1", true, rec("d-4", false, false, "removed")), null, "not a current device");
  assert.equal(of("d-1", true, rec("d-5", false, false, "pending")), null, "nor one waiting for approval");
});

test("“make it a full device” writes setAdmin true and mails its user keys — and says so when the mail did not go, the entry written all the same", async () => {
  const { hub, a } = await account();
  const b = await pairAs(hub, a, { admin: false });
  // The hub does not take the mail: the entry stands, and the reason comes back (not only a console line).
  hub.failMail = 1;
  const failed = await a.promote(b.deviceId);
  assert.ok(a.chain!.admin(b.deviceId), "a full device all the same");
  assert.ok(failed.keysFailed instanceof HubApiError, String(failed.keysFailed));
  assert.equal(mailTo(hub, b.deviceId).filter((m) => m.purpose === MAIL_USER_KEY).length, 0, "no user key reached its mailbox");
  // "re-send keys" on its row then sends them.
  assert.ok((await a.resendKeys(b.deviceId)) > 0);
  assert.ok(mailTo(hub, b.deviceId).some((m) => m.purpose === MAIL_USER_KEY));
  // Mailed: nothing to say. Already one: nothing written, nothing to say.
  const c = await pairAs(hub, a, { admin: false });
  assert.deepEqual(await a.promote(c.deviceId), { keysFailed: null });
  assert.ok(mailTo(hub, c.deviceId).some((m) => m.purpose === MAIL_USER_KEY), "its keys wait in its mailbox: it need not be online");
  const seq = a.chain!.seq;
  assert.deepEqual(await a.promote(c.deviceId), { keysFailed: null });
  assert.equal(a.chain!.seq, seq);
  // The rules are setAdmin's: never a device that runs AI (D1).
  await assert.rejects(a.promote("m-1"), invalid(RULES.hostNotAdmin));
});

test("alerts name chain v3's changes, with the flags in words for an addition and setAdmin true as its own kind", async () => {
  const { hub, a } = await account();
  const c = await pairAs(hub, a, { admin: true });
  await c.receiveMail();
  const b = await pairAs(hub, a, { admin: false });
  await hub.enroll(a, "m-2", { writable: true });
  await a.setAdmin(b.deviceId, true);
  await a.setHost("m-2", false, new FakeCourier());
  await a.setAccept("m-1", { type: "all" });
  await a.removeDevice(b.deviceId, new FakeCourier());
  await c.sync();
  const seen = c.alerts().map((x) => ({ kind: x.kind, subject: x.subject, flags: x.flags, on: x.on, accept: x.accept }));
  assert.deepEqual(seen, [
    { kind: "addDevice", subject: b.deviceId, flags: { host: false, admin: false }, on: undefined, accept: undefined },
    { kind: "addDevice", subject: "m-2", flags: { host: true, admin: false }, on: undefined, accept: undefined },
    { kind: "setAdmin", subject: b.deviceId, flags: undefined, on: true, accept: undefined },
    { kind: "setHost", subject: "m-2", flags: undefined, on: false, accept: undefined },
    { kind: "setAccept", subject: "m-1", flags: undefined, on: undefined, accept: { type: "all" } },
    { kind: "removeDevice", subject: b.deviceId, flags: undefined, on: undefined, accept: undefined },
  ]);
  assert.ok(!c.alerts().some((x) => x.kind === "keyEpochs"), "the keys that come with an addition or a removal are not alerts of their own");
});

test("a renewal pause's alerts — its start names what it pauses; its completion only the control that comes back", async () => {
  const { hub, a } = await account();
  const c = await pairAs(hub, a, { admin: true });
  await c.receiveMail();
  const [b, d] = [await pairAs(hub, a, { admin: false }), await pairAs(hub, a, { admin: false })];
  const ids = [b.deviceId, d.deviceId].sort();
  await a.setAccept("m-1", { type: "devices", ids });
  const op = new Uint8Array(16).fill(5);
  await a.append({ type: "renewalPause", op, phase: PAUSE_START, removals: [{ host: "m-1", ids }] });
  // A hand setAccept during the pause drops b from the base: its control does not come back with the completion.
  await a.setAccept("m-1", { type: "devices", ids: [d.deviceId] }, new FakeCourier());
  await a.append({ type: "renewalPause", op, phase: PAUSE_COMPLETED, removals: [] });
  await c.sync();
  assert.deepEqual(
    c.alerts().filter((x) => x.kind === "renewalPause").map((x) => x.pause),
    [
      { phase: "start", removals: [{ host: "m-1", ids }] },
      { phase: "completed", removals: [{ host: "m-1", ids: [d.deviceId] }] },
    ],
  );
});

/** Seal attempts the hub saw for `id` with `purpose` (taken or refused). */
const attemptsTo = (hub: FakeKeysHub, id: string, purpose: number) => hub.mailAttempts.filter((m) => m.split(" ")[1] === id && m.endsWith(` ${purpose}`));

test("D2: a host's keys go only to the devices it accepts — on a rotation, an enrollment, a request — never to the others", async () => {
  const { hub, a } = await account();
  await hub.enroll(a, "m-2", { writable: true });
  const b = await pairAs(hub, a, { admin: false });
  const d = await pairAs(hub, a, { admin: false });
  await admit(a, "m-1", b.deviceId);
  const courier = new FakeCourier();
  const report = await a.rotate(courier);
  assert.deepEqual(report.hostKeysTo, [b.deviceId], "m-1's new key to b alone");
  const hostOf = (m: { sealed: string }) => fromHexHost(m.sealed);
  assert.deepEqual([...new Set(mailTo(hub, b.deviceId).filter((m) => m.purpose === MAIL_HOST_KEY).map(hostOf))], ["m-1"], "never m-2's to b");
  assert.equal(attemptsTo(hub, d.deviceId, MAIL_HOST_KEY).length, 0, "nothing even tried for d");
  // b asks: it lacks only what m-1 has; the answer is m-1's.
  hub.mail = hub.mail.filter((m) => m.recipient !== b.deviceId);
  await b.sync();
  assert.deepEqual(await b.checkHostKeys(), ["m-1"], "m-2 is none of its business");
  await a.receiveMail();
  assert.deepEqual([...new Set(mailTo(hub, b.deviceId).filter((m) => m.purpose === MAIL_HOST_KEY).map(hostOf))], ["m-1"]);
  // d asks: there is nothing for it.
  await d.sync();
  assert.deepEqual(await d.checkHostKeys(), [], "it misses nothing it may have");
  await d.requestHostKeys(true);
  await a.receiveMail();
  assert.equal(attemptsTo(hub, d.deviceId, MAIL_HOST_KEY).length, 0);
});

test("D2: a narrowed accept list writes the host's next key epoch in the same call, hands it to the agent, and seals it to those still let in", async () => {
  const { hub, a } = await account();
  const b = await pairAs(hub, a, { admin: false });
  const d = await pairAs(hub, a, { admin: false });
  await admit(a, "m-1", b.deviceId, d.deviceId);
  const courier = new FakeCourier();
  const posts = hub.chainPosts;
  await assert.rejects(a.setAccept("m-1", { type: "devices", ids: [b.deviceId] }), kindIs("invalidEntry"), "a narrowing needs a way to hand the key over");
  assert.equal(hub.chainPosts, posts, "and nothing went out");
  const at = a.chain!.seq + 1;
  const epoch = a.chain!.machineKeyEpochs("m-1");
  const before = attemptsTo(hub, d.deviceId, MAIL_HOST_KEY).length;
  const change = await a.setAccept("m-1", { type: "devices", ids: [b.deviceId] }, courier);
  assert.deepEqual(kinds(a, at), ["setAccept", "keyEpochs"], "one call");
  const ke = a.chain!.entryAt(at + 1)!.draft.action;
  assert.ok(ke.type === "keyEpochs" && ke.user === null && ke.machines.length === 1 && ke.machines[0]!.machine === "m-1" && ke.machines[0]!.epoch === epoch);
  assert.deepEqual({ narrowed: change.narrowed, shutOut: change.shutOut, mkEpoch: change.mkEpoch, to: change.hostKeysTo }, { narrowed: true, shutOut: [d.deviceId], mkEpoch: epoch, to: [b.deviceId] });
  assert.equal(courier.posted.at(-1)?.epoch, epoch, "the agent has its new key");
  assert.equal(attemptsTo(hub, d.deviceId, MAIL_HOST_KEY).length, before, "not to the device shut out");
  assert.ok(a.reEnrollDue().has("m-1"), "and the computer should get a new identity (D3)");
  // Widening is one entry, and hands the newly let in the host's keys.
  const at2 = a.chain!.seq + 1;
  const widened = await a.setAccept("m-1", { type: "all" });
  assert.deepEqual(kinds(a, at2), ["setAccept"]);
  assert.deepEqual(widened.admitted, [d.deviceId]);
  assert.ok(attemptsTo(hub, d.deviceId, MAIL_HOST_KEY).length > before);
});

test("D2: removing a device a host accepts rotates that host's key even when asked not to; the new key reaches only those still let in", async () => {
  const { hub, a } = await account();
  const b = await pairAs(hub, a, { admin: false });
  const d = await pairAs(hub, a, { admin: false });
  const e = await pairAs(hub, a, { admin: false });
  await admit(a, "m-1", b.deviceId, d.deviceId);
  assert.equal(a.reEnrollDue().size, 0);
  const at = a.chain!.seq + 1;
  const report = await a.removeDevice(b.deviceId, new FakeCourier(), false, { rotateMachineKeys: false });
  assert.deepEqual(kinds(a, at), ["removeDevice3", "keyEpochs"]);
  assert.equal(report.ukEpoch, null, "it held no user key");
  assert.deepEqual(report.hostKeysTo, [d.deviceId]);
  assert.equal(attemptsTo(hub, e.deviceId, MAIL_HOST_KEY).length, 0);
  assert.ok(a.reEnrollDue().has("m-1"), "it could control m-1: m-1 should get a new identity (D3)");
  // One no host accepts held nothing: its removal alone, if asked.
  const at2 = a.chain!.seq + 1;
  await a.removeDevice(e.deviceId, new FakeCourier(), false, { rotateMachineKeys: false });
  assert.deepEqual(kinds(a, at2), ["removeDevice3"]);
});

test("D3: a host to re-enroll is one a device that could control it no longer can — an admin removed — until it is enrolled again", async () => {
  const { hub, a } = await account();
  const c = await pairAs(hub, a, { admin: true });
  assert.deepEqual([...a.reEnrollDue()], []);
  await a.removeDevice(c.deviceId, new FakeCourier());
  assert.deepEqual([...a.reEnrollDue()], ["m-1"], "an admin could control every host");
  await a.removeDevice("m-1", new FakeCourier());
  await hub.enroll(a, "m-9", { writable: true });
  assert.deepEqual([...a.reEnrollDue()], [], "a new identity is a new device");
});

/** What a host's status would say opened a channel to it (`devices`: id → first seen), complete from seq `from` (since `fromMs`), written at `a`'s chain seq `seq` (its newest by default). */
function controllers(a: CloudAccount, devices: Record<string, number>, opts: { from?: number; fromMs?: number; seq?: number } = {}): ControllersRecord {
  return { from: opts.from ?? 0, fromMs: opts.fromMs ?? 1_000, seq: opts.seq ?? a.chain!.seq, devices: Object.entries(devices).map(([id, firstMs]) => ({ id, firstMs })) };
}

/** Dismiss the re-enroll warning about `device` on `host` as the page does: for the loss its line shows. */
function dismiss(a: CloudAccount, host: string, device: string): void {
  const line = a.reEnrollWarnings().get(host)?.find((l) => l.device === device);
  assert.ok(line, `a warning about ${device} on ${host}`);
  a.dismissReEnroll(host, device, line.lostSeq);
}

/** The seq of the last entry of `a`'s chain that `what` picks. */
const seqOf = (a: CloudAccount, what: (action: Action) => boolean) =>
  a
    .chain!.allEntries()
    .filter((e) => what(e.draft.action))
    .at(-1)!.draft.seq;

test("D3: what a host's own record says of a device it should be re-enrolled for — evidence shown, never a reason to drop the warning", async () => {
  const { hub, a } = await account();
  const c = await pairAs(hub, a, { admin: true });
  await a.removeDevice(c.deviceId, new FakeCourier());
  const added = seqOf(a, (x) => x.type === "addDevice3" && x.device.id === c.deviceId);
  const lostSeq = seqOf(a, (x) => x.type === "removeDevice3" && x.id === c.deviceId);
  assert.deepEqual([...reEnrollCulprits(a.chain!).get("m-1")!], [[c.deviceId, lostSeq]]);
  const of = (record?: ControllersRecord, keptMs?: number) => reEnrollEvidence(a.chain!, "m-1", c.deviceId, lostSeq, record, keptMs);
  assert.deepEqual(of(), { kind: "unknown" }, "an older agent says nothing");
  assert.deepEqual(of(controllers(a, { [c.deviceId]: 5_000 })), { kind: "connected", firstMs: 5_000 });
  assert.deepEqual(of(controllers(a, {}, { fromMs: 2_000 })), { kind: "notSeen", sinceMs: 2_000 }, "complete since before it joined, written after its removal");
  assert.deepEqual(of(controllers(a, {}, { seq: lostSeq - 1 })), { kind: "unknown" }, "written before the agent saw the removal");
  assert.deepEqual(of(controllers(a, {}, { from: added })), { kind: "unknown" }, "begun once it could already connect (an agent upgraded since)");
  assert.equal(of(controllers(a, {}, { from: added - 1 })).kind, "notSeen");
  assert.deepEqual(of(controllers(a, {}), 4_000), { kind: "connected", firstMs: 4_000 }, "a record seen before named it");
  // Whatever a record says, the host stays due: only re-enrolling it or a person's dismissal drops the warning.
  const evidence = (record?: ControllersRecord) => a.reEnrollWarnings(() => record).get("m-1")!.map((l) => [l.device, l.evidence.kind, l.dismissed]);
  assert.deepEqual(evidence(), [[c.deviceId, "unknown", false]]);
  assert.deepEqual(evidence(controllers(a, {})), [[c.deviceId, "notSeen", false]]);
  assert.deepEqual([...a.reEnrollDue()], ["m-1"], "no record of it connecting: still due");
  assert.deepEqual(evidence(controllers(a, { [c.deviceId]: 5_000 })), [[c.deviceId, "connected", false]]);
  assert.deepEqual(evidence(controllers(a, {})), [[c.deviceId, "connected", false]], "kept: a later record that leaves it out cannot unsay it");
  assert.deepEqual([...(await hub.reload(a)).reEnrollWarnings().get("m-1")!.map((l) => l.evidence.kind)], ["connected"], "kept for the account in this browser");
  assert.deepEqual([...hostsToReEnroll(a.chain!)], ["m-1"]);
});

test("D3: a person can say a device never connected — two phones removed before any record — kept, shown quietly, undone", async () => {
  const { hub, a } = await account();
  const p1 = await pairAs(hub, a, { admin: true });
  const p2 = await pairAs(hub, a, { admin: true });
  await a.removeDevice(p1.deviceId, new FakeCourier());
  await a.removeDevice(p2.deviceId, new FakeCourier());
  assert.deepEqual([...a.reEnrollDue()], ["m-1"]);
  let changes = 0;
  a.onChange(() => void changes++);
  dismiss(a, "m-1", p1.deviceId);
  assert.ok(changes > 0, "the page is told");
  assert.deepEqual([...a.reEnrollDue()], ["m-1"], "the other phone still warns");
  dismiss(a, "m-1", p2.deviceId);
  assert.deepEqual([...a.reEnrollDue()], [], "both dismissed: nothing due");
  assert.deepEqual(
    a.reEnrollWarnings().get("m-1")!.map((l) => l.dismissed),
    [true, true],
    "still listed, dismissed",
  );
  assert.deepEqual([...hostsToReEnroll(a.chain!)], ["m-1"], "the chain's own answer is unchanged");
  // Kept for the account: the same browser after a reload.
  const again = await hub.reload(a);
  assert.deepEqual([...again.reEnrollDue()], []);
  // Undone.
  again.warnAgainReEnroll("m-1", p1.deviceId);
  assert.deepEqual([...again.reEnrollDue()], ["m-1"]);
  dismiss(again, "m-1", p1.deviceId);
  // A record that names a dismissed device brings the warning back.
  const line = again
    .reEnrollWarnings(() => controllers(again, { [p2.deviceId]: 9_000 }))
    .get("m-1")!
    .find((l) => l.device === p2.deviceId)!;
  assert.deepEqual([line.dismissed, line.evidence.kind], [false, "connected"]);
  assert.deepEqual([...again.reEnrollDue()], ["m-1"], "and stays: what a record named is kept");
  assert.deepEqual([...again.reEnrollDue(() => undefined)], ["m-1"]);
  // Nothing to dismiss where nothing is due.
  again.dismissReEnroll("m-1", "d-nobody", a.chain!.seq);
  again.dismissReEnroll("m-9", p1.deviceId, a.chain!.seq);
  assert.ok(!again.reEnrollWarnings().get("m-1")!.some((l) => l.device === "d-nobody"));
  // Signing out of the account on this browser forgets both.
  const storage = browserStorage(again.store);
  const keys = () => [...storage.values.keys()].filter((k) => k.startsWith("rh.hub.reenroll."));
  assert.equal(keys().length, 2, "dismissed, and named");
  await again.wipe();
  assert.deepEqual(keys(), []);
});

test("D3: a record padded with made-up ids cannot push a device it once named out of what this browser keeps", async () => {
  const { hub, a } = await account();
  const x = await pairAs(hub, a, { admin: true });
  await a.removeDevice(x.deviceId, new FakeCourier());
  const evidence = (record: ControllersRecord) => a.reEnrollWarnings(() => record).get("m-1")!.find((l) => l.device === x.deviceId)!;
  assert.equal(evidence(controllers(a, { [x.deviceId]: 1_000 })).evidence.kind, "connected", "m-1's record named it once");
  // X, with a shell on m-1, drops itself from the agent's record and pads it with made-up ids seen just now.
  const now = Date.now();
  const flood = Object.fromEntries(Array.from({ length: 1100 }, (_, i) => [`d-made-up-${i}`, now + i]));
  const line = evidence(controllers(a, flood));
  assert.deepEqual([line.evidence.kind, line.dismissed], ["connected", false], "still kept as connected, so still not dismissable");
  const stored = JSON.parse(browserStorage(a.store).values.get(`rh.hub.reenroll.connected.${a.account}`)!) as Record<string, Record<string, number>>;
  assert.deepEqual(Object.keys(stored["m-1"]!), [x.deviceId], "only devices the chain names are kept");
  assert.equal((await hub.reload(a)).reEnrollWarnings().get("m-1")!.find((l) => l.device === x.deviceId)!.evidence.kind, "connected", "and after a reload");
});

test("D3: what a tab whose chain is behind cannot place is ignored, never deleted — the tabs share it", async () => {
  const { hub, a } = await account();
  const x = await pairAs(hub, a, { admin: true });
  await a.removeDevice(x.deviceId, new FakeCourier());
  const key = `rh.hub.reenroll.connected.${a.account}`;
  const storage = browserStorage(a.store);
  // Y: a device another tab, its chain newer, saw m-1's record name — not in this tab's chain yet.
  const y = await hub.device("tok");
  assert.equal(a.chain!.deviceRecord(y.deviceId), undefined);
  storage.setItem(key, JSON.stringify({ "m-1": { [y.deviceId]: 5_000, [x.deviceId]: 1_000 } }));
  const line = a.reEnrollWarnings().get("m-1")!.find((l) => l.device === x.deviceId)!;
  assert.equal(line.evidence.kind, "connected", "what this chain names is read");
  // A write (a record naming a device this chain knows) keeps the other tab's entry too.
  a.reEnrollWarnings(() => controllers(a, { [a.deviceId]: 2_000 }));
  assert.deepEqual(JSON.parse(storage.getItem(key)!), { "m-1": { [y.deviceId]: 5_000, [x.deviceId]: 1_000, [a.deviceId]: 2_000 } });
  // This tab's chain catches up — Y joined — and then Y is removed: what was kept about it while it could not be placed counts.
  await a.append(joinAction({ id: y.deviceId, signKey: y.keys.signPublic, dhKey: y.keys.dhPublic }, { host: false, admin: true }, a.chain!));
  await a.removeDevice(y.deviceId, new FakeCourier());
  const ys = a.reEnrollWarnings().get("m-1")!.find((l) => l.device === y.deviceId)!;
  assert.deepEqual([ys.evidence, ys.dismissed], [{ kind: "connected", firstMs: 5_000 }, false], "kept while unknown, read once known");
});

test("D3: a record that names nobody new writes nothing — renders do not rewrite the list", async () => {
  const { hub, a } = await account();
  const x = await pairAs(hub, a, { admin: true });
  await a.removeDevice(x.deviceId, new FakeCourier());
  const key = `rh.hub.reenroll.connected.${a.account}`;
  const storage = browserStorage(a.store);
  const setItem = storage.setItem;
  const writes: string[] = [];
  storage.setItem = (k, v) => {
    writes.push(k);
    setItem(k, v);
  };
  try {
    for (let i = 0; i < 3; i++) a.reEnrollWarnings(() => controllers(a, {}));
    assert.deepEqual(writes.filter((k) => k === key), [], "a record that names nobody: nothing written");
    assert.equal(storage.getItem(key), null);
    a.reEnrollWarnings(() => controllers(a, { [a.deviceId]: 2_000 }));
    a.reEnrollWarnings(() => controllers(a, { [a.deviceId]: 2_000 }));
    a.reEnrollWarnings(() => controllers(a, {}));
    assert.equal(writes.filter((k) => k === key).length, 1, "a new name is written once; the same again, or nobody, is not");
  } finally {
    storage.setItem = setItem;
  }
});

test("D3: a computer that stops running AI keeps what its record named; only its removal from the account forgets it", async () => {
  const { hub, a } = await account();
  const x = await pairAs(hub, a, { admin: true });
  await hub.enroll(a, "m-2", { writable: true });
  await a.removeDevice(x.deviceId, new FakeCourier());
  const key = `rh.hub.reenroll.connected.${a.account}`;
  const storage = browserStorage(a.store);
  a.reEnrollWarnings((h) => (h === "m-1" ? controllers(a, { [x.deviceId]: 1_000 }) : undefined));
  await a.setHost("m-1", false, new FakeCourier(), { writable: true });
  // Any write while m-1 runs no AI (a record of m-2's naming a device) keeps m-1's list.
  a.reEnrollWarnings((h) => (h === "m-2" ? controllers(a, { [a.deviceId]: 2_000 }) : undefined));
  assert.deepEqual(Object.keys(JSON.parse(storage.getItem(key)!)).sort(), ["m-1", "m-2"], "m-1's list stays while it is stopped");
  // Removed from the account: its list goes with it.
  await a.removeDevice("m-1", new FakeCourier());
  a.reEnrollWarnings((h) => (h === "m-2" ? controllers(a, { [a.deviceId]: 3_000 }) : undefined));
  assert.deepEqual(Object.keys(JSON.parse(storage.getItem(key)!)), ["m-2"]);
});

test("D3: the live record goes with the banner's question too — a dismissed device a host's status names warns again", async () => {
  const { hub, a } = await account();
  const p = await pairAs(hub, a, { admin: true });
  await a.removeDevice(p.deviceId, new FakeCourier());
  dismiss(a, "m-1", p.deviceId);
  assert.deepEqual([...a.reEnrollDue()], []);
  assert.deepEqual([...a.reEnrollDue((h) => (h === "m-1" ? controllers(a, {}) : undefined))], [], "a record that leaves it out changes nothing");
  assert.deepEqual([...a.reEnrollDue((h) => (h === "m-1" ? controllers(a, { [p.deviceId]: 3_000 }) : undefined))], ["m-1"], "one that names it does");
});

test("D3: a dismissal is for the loss it was made for — the same device shut out again warns again", async () => {
  const { hub, a } = await account();
  const b = await pairAs(hub, a, { admin: false });
  await admit(a, "m-1", b.deviceId);
  await a.setAccept("m-1", { type: "admins" }, new FakeCourier());
  assert.deepEqual([...a.reEnrollDue()], ["m-1"], "shut out: due");
  dismiss(a, "m-1", b.deviceId);
  assert.deepEqual([...a.reEnrollDue()], []);
  await admit(a, "m-1", b.deviceId);
  assert.deepEqual([...a.reEnrollDue()], [], "let in again: nothing new lost");
  await a.setAccept("m-1", { type: "admins" }, new FakeCourier());
  assert.deepEqual([...a.reEnrollDue()], ["m-1"], "a new loss warns again");
});

test("D3: a device let back in is not lost any more — it may connect then — and a later loss is a new warning, with what it did", async () => {
  const { hub, a } = await account();
  const p = await pairAs(hub, a, { admin: false });
  await admit(a, "m-1", p.deviceId);
  await a.setAccept("m-1", { type: "admins" }, new FakeCourier());
  dismiss(a, "m-1", p.deviceId);
  await admit(a, "m-1", p.deviceId);
  assert.equal(reEnrollCulprits(a.chain!).get("m-1"), undefined, "let back in: off the list");
  // It connects while let in, as it may: its host's record names it.
  const connected = controllers(a, { [p.deviceId]: 8_000 });
  assert.equal(a.reEnrollWarnings(() => connected).get("m-1"), undefined, "no warning about a device that may control m-1");
  assert.deepEqual([...a.reEnrollDue(() => connected)], []);
  await a.setAccept("m-1", { type: "admins" }, new FakeCourier());
  const line = a.reEnrollWarnings(() => connected).get("m-1")!.find((l) => l.device === p.deviceId)!;
  assert.deepEqual([line.evidence.kind, line.dismissed], ["connected", false], "shut out again, after it connected");
});

test("D3: a dismissal covers the loss the person was shown, not one that came while they read it", async () => {
  const { hub, a } = await account();
  const b = await pairAs(hub, a, { admin: false });
  await admit(a, "m-1", b.deviceId);
  await a.setAccept("m-1", { type: "admins" }, new FakeCourier());
  const shown = a.reEnrollWarnings().get("m-1")!.find((l) => l.device === b.deviceId)!.lostSeq;
  // Meanwhile, on another device: let back in, and shut out again.
  await admit(a, "m-1", b.deviceId);
  await a.setAccept("m-1", { type: "admins" }, new FakeCourier());
  a.dismissReEnroll("m-1", b.deviceId, shown);
  assert.deepEqual([...a.reEnrollDue()], ["m-1"], "the newer loss still warns");
});

test("1: the user key goes with any device that ever held it — demoted, then removed — and a demotion retires it too", async () => {
  const { hub, a } = await account();
  const c = await pairAs(hub, a, { admin: true });
  const b = await pairAs(hub, a, { admin: false });
  assert.equal(heldUserKey(a.chain!, c.deviceId), true);
  assert.equal(heldUserKey(a.chain!, b.deviceId), false);
  // Demoted without a courier: the rotation is left due, and said so.
  const uk = a.chain!.userKeyEpochs();
  assert.equal(await a.setAdmin(c.deviceId, false), null);
  assert.equal(a.rotationDue, true, "a demotion leaves the user key it held current");
  assert.equal(a.chain!.admin(c.deviceId), undefined);
  assert.equal(heldUserKey(a.chain!, c.deviceId), true, "by the chain's history, not its flags now");
  // Then removed: everything rotates (it used to rotate machine keys only).
  const at = a.chain!.seq + 1;
  const report = await a.removeDevice(c.deviceId, new FakeCourier(), false, { rotateMachineKeys: false });
  assert.deepEqual(kinds(a, at), ["removeDevice3", "keyEpochs"]);
  const ke = a.chain!.entryAt(at + 1)!.draft.action;
  assert.ok(ke.type === "keyEpochs" && ke.user?.epoch === uk, "a new user key");
  assert.equal(report.ukEpoch, uk);
  assert.equal(a.rotationDue, false);
  // A device promoted, then demoted with a courier: the rotation is in the same call.
  await a.setAdmin(b.deviceId, true);
  await b.sync();
  await b.receiveMail();
  assert.ok((await b.vault.epochs()).length > 0, "it held the user key");
  const at2 = a.chain!.seq + 1;
  const asAdmin = attemptsTo(hub, b.deviceId, MAIL_USER_KEY).length;
  const demoted = await a.setAdmin(b.deviceId, false, { courier: new FakeCourier() });
  assert.deepEqual(kinds(a, at2), ["setAdmin", "keyEpochs"]);
  assert.equal(demoted?.ukEpoch, uk + 1);
  assert.ok(!demoted!.sealedTo.includes(b.deviceId), "the new user key not to the demoted device");
  assert.equal(attemptsTo(hub, b.deviceId, MAIL_USER_KEY).length, asAdmin, "only those it got as an admin");
  assert.equal(a.rotationDue, false);
  // Its own demotion: it may sign nothing after it, so the rotation is left due for another admin.
  const d = await pairAs(hub, a, { admin: true });
  await d.receiveMail();
  await d.setAdmin(d.deviceId, false);
  await a.sync();
  assert.equal(a.rotationDue, true);
});

test("9: a rotation seals the user key to the admins only; a passkey's pending device is not approved by its id, and gets no user key", async () => {
  const { hub, a } = await account();
  const b = await pairAs(hub, a, { admin: false });
  const c = await pairAs(hub, a, { admin: true });
  const report = await a.rotate(new FakeCourier());
  assert.deepEqual(report.sealedTo, [c.deviceId]);
  assert.deepEqual(report.failedTo, []);
  assert.equal(attemptsTo(hub, b.deviceId, MAIL_USER_KEY).length, 0, "not even tried for b");
  // A passkey slot and a device it adds: pending, not an admin.
  const pak = (await crypto.subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"])) as CryptoKeyPair;
  const pakPublic = new Uint8Array(await crypto.subtle.exportKey("raw", pak.publicKey));
  const credHash = new Uint8Array(32).fill(7);
  const saltNonce = new Uint8Array(32).fill(9);
  const slot = "p-testslot0001";
  const proof = await passkeySlotProof(pak.privateKey, pakPublic, a.account, a.chain!.accountKey, slot, credHash, saltNonce);
  await a.append({ type: "passkeySlotSet", slot: { slot, key: pakPublic, proof, credHash, saltNonce, grants: 0, selfActivate: true } });
  const p = await hub.device("tok");
  await p.sync();
  const join = await signDraft(
    p.chain!.draft({ type: "passkey", slot }, { type: "addDevice3", device: { id: p.deviceId, signKey: p.keys.signPublic, dhKey: p.keys.dhPublic, flags: { host: false, admin: false } } }, Date.now()),
    pak.privateKey,
  );
  await p.api.appendChain(p.session.token, [encodeEntry(join)]);
  await a.sync();
  assert.equal(a.chain!.deviceRecord(p.deviceId)?.status.type, "pending");
  // Approved only by pairing with it (the keys on its screen: passkey-join.test.ts), never by its id.
  await assert.rejects(a.approveDevice(p.deviceId), (e: unknown) => e instanceof Error && (e as { kind?: string }).kind === "notPending");
  assert.equal(a.chain!.deviceRecord(p.deviceId)?.status.type, "pending");
  assert.equal(attemptsTo(hub, p.deviceId, MAIL_USER_KEY).length, 0, "no user key for a device that does not manage the account");
});

test("9: only a device that manages the account vouches for the chain head (the follower's attestations)", async () => {
  const { hub, a } = await account();
  const b = await pairAs(hub, a, { admin: false });
  const attested: number[] = [];
  const link = {
    onState: () => () => undefined,
    onChain: () => () => undefined,
    onControl: () => () => undefined,
    sendControl: () => true,
    attest: (x: { seq: number }) => (attested.push(x.seq), true),
    state: "closed",
    lastClose: null,
  } as unknown as FollowLink;
  await new ChainFollower({ link, account: b, onRemoved: () => undefined }).attest();
  assert.deepEqual(attested, [], "not b");
  await new ChainFollower({ link, account: a, onRemoved: () => undefined }).attest();
  assert.deepEqual(attested, [a.chain!.seq], "a does");
});

// ------------------------------------------------------------ the re-review of the chain v3 fixes

/**
 * `account()` with two more admins, d and c — then c turns its own
 * 「可管理账号」 off (no courier: it may sign nothing after it), so the newest
 * user key is one c still holds, and the rotation is left due until another
 * admin runs it.
 */
async function demotedAccount() {
  const { hub, a, words } = await account();
  const d = await pairAs(hub, a, { admin: true });
  await d.receiveMail();
  const c = await pairAs(hub, a, { admin: true });
  await c.receiveMail();
  await c.setAdmin(c.deviceId, false);
  await a.sync();
  await d.sync();
  assert.equal(a.rotationDue, true);
  assert.equal(a.userKeyStale, true);
  const stale = a.chain!.latestUserEpoch()!;
  assert.ok(await c.vault.has(stale), "c holds the newest user key");
  return { hub, a, c, d, words, stale };
}

/** Whether `who` can open host `host`'s key epoch `epoch` from the keyring now (with the user keys it holds). */
async function opens(who: CloudAccount, host: string, epoch: number): Promise<boolean> {
  await who.sync();
  await who.keyring.refresh();
  const key = await who.machineKeyBytes(host, epoch);
  key?.fill(0);
  return key !== null;
}

/** What a batch that issues machine keys while the user key is stale must show: a new user key epoch in the same entry, shared as a rotation shares it — and the new machine key under it alone. */
async function assertFreshUserKey(t: { hub: FakeKeysHub; a: CloudAccount; c: CloudAccount; d: CloudAccount; stale: number }, at: number, host: string, ucBefore: number) {
  const { hub, a, c, d, stale } = t;
  const fresh = stale + 1;
  const ke = a.chain!.allEntries().slice(at).find((e) => e.draft.action.type === "keyEpochs")!.draft.action;
  assert.ok(ke.type === "keyEpochs" && ke.user?.epoch === fresh, `a new user key epoch in the batch: ${JSON.stringify(ke.type === "keyEpochs" ? ke.user : null)}`);
  const mkEpoch = ke.machines.find((m) => m.machine === host)!.epoch;
  assert.equal(a.chain!.latestUserEpoch(), fresh);
  assert.equal(hub.keysAt(`mk/${host}/${mkEpoch}/${fresh}`).length, 1, "the new machine key under the new user key");
  assert.equal(hub.keysAt(`mk/${host}/${mkEpoch}/${stale}`).length, 0, "and never under the one c holds");
  assert.ok(await opens(a, host, mkEpoch));
  assert.equal(await opens(c, host, mkEpoch), false, "the demoted device cannot open it");
  // Shared as a rotation shares it: to the admins, for the recovery words — never to the device that left.
  assert.ok(hub.mailLog.some((m) => m.recipient === d.deviceId && m.purpose === MAIL_USER_KEY && m.epoch === fresh), "the other admin gets the new user key");
  assert.equal(attemptsTo(hub, c.deviceId, MAIL_USER_KEY).length, ucBefore, "not even tried for c");
  assert.ok(hub.keysAt(`rk/${fresh}/${a.deviceId}`).length > 0, "sealed for the recovery words");
  await d.receiveMail();
  assert.ok(await opens(d, host, mkEpoch), "the other admin reads the host with it");
  assert.equal(a.userKeyStale, false);
  return { fresh, mkEpoch };
}

test("re-review 1: enrolling a host while the user key is one a demoted admin holds carries a new user key, and the rotation stays due for the other hosts", async () => {
  const t = await demotedAccount();
  const { hub, a, c } = t;
  const ucBefore = attemptsTo(hub, c.deviceId, MAIL_USER_KEY).length;
  const at = a.chain!.seq + 1;
  await hub.enroll(a, "m-2", { writable: true });
  assert.deepEqual(kinds(a, at), ["addDevice3", "keyEpochs"], "still one call");
  await assertFreshUserKey(t, at, "m-2", ucBefore);
  // m-1 still seals under a key c could read: the rotation is not done — only the user key is fresh.
  assert.equal(a.rotationDue, true, "m-1 has had no new key since c left");
  assert.equal(rotationDueIn(a.chain!), true);
  // The user key is fresh now: the next machine key needs no other.
  const at2 = a.chain!.seq + 1;
  await a.issueMachineKey("m-2", new FakeCourier());
  const ke2 = a.chain!.entryAt(at2)!.draft.action;
  assert.ok(ke2.type === "keyEpochs" && ke2.user === null, "no second new user key");
  await a.rotate(new FakeCourier());
  assert.equal(a.rotationDue, false, "every host on a new key under a new user key");
});

test("re-review 1: running AI again on a host while the user key is stale carries a new user key; the agent gets a key the demoted admin cannot open", async () => {
  const t = await demotedAccount();
  const { hub, a, c } = t;
  const courier = new FakeCourier();
  await a.setHost("m-1", false, courier);
  const ucBefore = attemptsTo(hub, c.deviceId, MAIL_USER_KEY).length;
  const at = a.chain!.seq + 1;
  const again = await a.setHost("m-1", true, courier, { writable: true });
  assert.deepEqual(kinds(a, at), ["setHost", "keyEpochs"]);
  const { fresh, mkEpoch } = await assertFreshUserKey(t, at, "m-1", ucBefore);
  assert.equal(again?.mkEpoch, mkEpoch);
  assert.equal(courier.posted.at(-1)?.epoch, mkEpoch, "handed to its agent");
  assert.equal(again?.userKey?.epoch, fresh);
  assert.deepEqual(again?.userKey?.sealedTo, [t.d.deviceId]);
  assert.equal(again?.userKey?.recovery, true);
  // Its only host has a new key under a new user key: the rotation is done.
  assert.equal(a.rotationDue, false);
});

test("re-review 1: narrowing a host's accept list while the user key is stale carries a new user key with the host's next epoch", async () => {
  const t = await demotedAccount();
  const { hub, a, c } = t;
  const b = await pairAs(hub, a, { admin: false });
  const e = await pairAs(hub, a, { admin: false });
  await admit(a, "m-1", b.deviceId, e.deviceId);
  assert.equal(a.userKeyStale, true, "letting devices in issues no key");
  const ucBefore = attemptsTo(hub, c.deviceId, MAIL_USER_KEY).length;
  const at = a.chain!.seq + 1;
  const change = await a.setAccept("m-1", { type: "devices", ids: [b.deviceId] }, new FakeCourier());
  assert.deepEqual(kinds(a, at), ["setAccept", "keyEpochs"], "one call");
  const { fresh, mkEpoch } = await assertFreshUserKey(t, at, "m-1", ucBefore);
  assert.equal(change.mkEpoch, mkEpoch);
  assert.equal(change.userKey?.epoch, fresh);
  assert.deepEqual(change.hostKeysTo, [b.deviceId], "the host's new key to the device still let in");
  await b.sync();
  await b.receiveMail();
  assert.ok((await b.machineKeys("m-1")).has(mkEpoch));
});

test("re-review 1: removing a device that held no user key, while the user key is stale, rotates the user key too", async () => {
  const t = await demotedAccount();
  const { hub, a, c } = t;
  const b = await pairAs(hub, a, { admin: false });
  await admit(a, "m-1", b.deviceId);
  const ucBefore = attemptsTo(hub, c.deviceId, MAIL_USER_KEY).length;
  const at = a.chain!.seq + 1;
  const report = await a.removeDevice(b.deviceId, new FakeCourier(), false, { rotateMachineKeys: false });
  assert.deepEqual(kinds(a, at), ["removeDevice3", "keyEpochs"]);
  const { fresh } = await assertFreshUserKey(t, at, "m-1", ucBefore);
  assert.equal(report.ukEpoch, fresh, "it held none, but the newest was c's");
  assert.deepEqual(report.sealedTo, [t.d.deviceId]);
  assert.equal(a.rotationDue, false, "every host rotated under the new user key: a whole rotation");
});

test("re-review 1: with no stale user key, those batches carry none (the machine keys go under the newest)", async () => {
  const { hub, a } = await account();
  const b = await pairAs(hub, a, { admin: false });
  await admit(a, "m-1", b.deviceId);
  assert.equal(userKeyStale(a.chain!), false);
  const uk = a.chain!.latestUserEpoch();
  await hub.enroll(a, "m-2", { writable: true });
  const report = await a.removeDevice(b.deviceId, new FakeCourier(), false, { rotateMachineKeys: false });
  assert.equal(report.ukEpoch, null);
  assert.equal(a.chain!.latestUserEpoch(), uk, "no new user key epoch");
});

/** Have `c` turn its own 「可管理账号」 off just before `a`'s next batch reaches the hub: a's post meets a newer head (409), and is built again on it. */
function demoteBeforeNextPost(a: CloudAccount, c: CloudAccount): { landed: () => boolean } {
  let done = false;
  const post = a.api.appendChain.bind(a.api);
  a.api.appendChain = async (token, entries) => {
    if (!done) {
      done = true;
      await c.setAdmin(c.deviceId, false);
    }
    return post(token, entries);
  };
  return { landed: () => done };
}

test("re-review 1: a demotion that lands while a batch is on its way is seen when it is built again — the batch then carries a new user key", async () => {
  const { hub, a } = await account();
  const d = await pairAs(hub, a, { admin: true });
  await d.receiveMail();
  const c = await pairAs(hub, a, { admin: true });
  await c.receiveMail();
  const b = await pairAs(hub, a, { admin: false });
  await admit(a, "m-1", b.deviceId);
  assert.equal(a.userKeyStale, false, "nothing stale when the batch is planned");
  const stale = a.chain!.latestUserEpoch()!;
  // A removal that rotates machine keys alone (b held no user key)…
  const race = demoteBeforeNextPost(a, c);
  const report = await a.removeDevice(b.deviceId, new FakeCourier(), false, { rotateMachineKeys: false });
  assert.ok(race.landed());
  assert.equal(report.ukEpoch, stale + 1, "…built again after c left: a new user key with them");
  assert.equal(hub.keysAt(`mk/m-1/${report.machines[0]!.mkEpoch}/${stale}`).length, 0);
  assert.equal(await opens(c, "m-1", report.machines[0]!.mkEpoch!), false);
  // And a machine key on its own (the same, through `commitHostKey`).
  const e = await pairAs(hub, a, { admin: true });
  await e.receiveMail();
  await a.rotate(new FakeCourier());
  assert.equal(a.userKeyStale, false);
  const before = a.chain!.latestUserEpoch()!;
  demoteBeforeNextPost(a, e);
  const at = a.chain!.seq + 1;
  const issued = await a.issueMachineKey("m-1", new FakeCourier());
  const ke = a.chain!.allEntries().slice(at).find((x) => x.draft.action.type === "keyEpochs")!.draft.action;
  assert.ok(ke.type === "keyEpochs" && ke.user?.epoch === before + 1, "a new user key in the batch built again");
  assert.equal(issued.ukEpoch, before + 1);
  assert.equal(issued.userKey?.epoch, before + 1);
  assert.equal(await opens(e, "m-1", issued.mkEpoch), false, "the device that left as it was posted cannot open it");
});

test("re-review 1: an admin that never got the stale newest user key still issues machine keys — under the new one it carries", async () => {
  const { hub, a } = await account();
  const c = await pairAs(hub, a, { admin: true });
  await c.receiveMail();
  const b = await pairAs(hub, a, { admin: false });
  await admit(a, "m-1", b.deviceId);
  // c rotates; its mail to a is lost; then c turns its own 「可管理账号」 off.
  await c.sync();
  await c.rotate(new FakeCourier());
  hub.mail = hub.mail.filter((m) => m.recipient !== a.deviceId);
  await c.setAdmin(c.deviceId, false);
  await a.sync();
  const stale = a.chain!.latestUserEpoch()!;
  assert.equal(await a.vault.has(stale), false, "a never got it");
  assert.equal(a.userKeyStale, true);
  // Enrolling: nothing to wrap under but a new user key — which the batch carries.
  await hub.enroll(a, "m-2", { writable: true });
  assert.equal(a.chain!.latestUserEpoch(), stale + 1);
  assert.equal(hub.keysAt(`mk/m-2/0/${stale + 1}`).length, 1);
  assert.equal(await opens(c, "m-2", 0), false);
  assert.equal(a.userKeyStale, false, "fresh now");
  // Again — another admin rotates, a misses it, it leaves — and a removal that would rotate machine keys alone:
  const d = await pairAs(hub, a, { admin: true });
  await d.receiveMail();
  await d.rotate(new FakeCourier());
  hub.mail = hub.mail.filter((m) => m.recipient !== a.deviceId);
  await d.setAdmin(d.deviceId, false);
  await a.sync();
  const stale2 = a.chain!.latestUserEpoch()!;
  assert.equal(await a.vault.has(stale2), false);
  const report = await a.removeDevice(b.deviceId, new FakeCourier(), false, { rotateMachineKeys: false });
  assert.equal(report.ukEpoch, stale2 + 1, "a new user key, though a never held the stale one");
  assert.equal(await opens(d, "m-1", report.machines.find((m) => m.machine === "m-1")!.mkEpoch!), false);
});

test("D1 extended: a device that ever ran AI is never made an admin — refused before anything is signed, by the chain's rule", async () => {
  const { hub, a } = await account();
  await a.setHost("m-1", false, new FakeCourier());
  const rec = a.chain!.device("m-1")!;
  assert.equal(rec.host, false);
  assert.equal(rec.ranAi, true);
  assert.equal(hasRunAi(a.chain!, rec), true);
  // Count what is signed: nothing may be.
  let signed = 0;
  const inner = a as unknown as { signChained: (...args: unknown[]) => Promise<unknown> };
  const original = inner.signChained.bind(a);
  inner.signChained = (...args: unknown[]) => {
    signed++;
    return original(...args);
  };
  const posts = hub.chainPosts;
  const seq = a.chain!.seq;
  await assert.rejects(a.setAdmin("m-1", true), (e: unknown) => {
    invalid(RULES.ranAiNotAdmin)(e);
    assert.match((e as Error).message, /ran AI before/, "said in words");
    // …in the reader's language too: its message is an i18n key, and it says nothing of managing the account.
    assert.equal((e as AccountError).message, "m-1 ran AI before, and its agent kept its key in a file: it cannot be a full device. Add a new browser or phone instead.");
    assert.ok(cloudDevicesZhCN[(e as AccountError).key], "translated");
    return true;
  });
  assert.equal(signed, 0, "refused before signing");
  assert.equal(hub.chainPosts, posts, "nothing posted");
  assert.equal(a.chain!.seq, seq);
  // The chain's verifier says the same of an entry signed anyway (review 13).
  await assert.rejects(a.append({ type: "setAdmin", id: "m-1", admin: true }), invalid(RULES.ranAiNotAdmin));
  assert.equal(hub.chainPosts, posts);
  // The devices page uses the same notion: no limited device, nothing offers to make it a full device.
  assert.equal(limitedDevice({ viewer: a.deviceId, viewerAdmin: true, device: rec, upgraded: true, ranAi: hasRunAi(a.chain!, rec) }), null);
  assert.equal(limitedDevice({ viewer: a.deviceId, viewerAdmin: true, device: rec, upgraded: true }), null, "by its record alone too");
  // A device that never ran AI still may be.
  const b = await pairAs(hub, a, { admin: false });
  assert.equal(hasRunAi(a.chain!, a.chain!.device(b.deviceId)!), false);
  await a.setAdmin(b.deviceId, true);
  assert.ok(a.chain!.admin(b.deviceId));
});

/**
 * Plant on the hub's chain what a lying hub could serve but never takes: a
 * host added (signed by `a`) with no machine key epoch after it. Whatever is
 * signed next is refused by the chain's rule (`hostKeyNext`).
 */
async function plantHostOwedItsKey(hub: FakeKeysHub, a: CloudAccount): Promise<void> {
  const keys = await generateDeviceKeys();
  const action: Action = { type: "addDevice3", device: { id: "m-owed", signKey: keys.signPublic, dhKey: keys.dhPublic, flags: { host: true, admin: false } } };
  const entry = await signDraft(hub.state!.draft({ type: "device", id: a.deviceId }, action, Date.now()), a.keys.sign.privateKey);
  await hub.state!.apply(entry, ChainClock.history());
  hub.chain.push(hex(encodeEntry(entry)));
}

test("13: recovering with the words checks the join with the chain's own verifier before posting it", async () => {
  const { hub, a, words } = await account();
  await plantHostOwedItsKey(hub, a);
  const r = await hub.device("tok");
  const posts = hub.chainPosts;
  await assert.rejects(r.recover(words.join(" ")), invalid(RULES.hostKeyNext));
  assert.equal(hub.chainPosts, posts, "nothing posted");
  assert.equal(r.standing, "absent");
});

test("13: a recovered device's self-activation is checked with the chain's own verifier before it is posted", async () => {
  const { hub, a, words } = await account();
  const r = await hub.device("tok");
  await r.recover(words.join(" "));
  assert.equal(r.standing, "pending");
  await plantHostOwedItsKey(hub, a);
  const posts = hub.chainPosts;
  await assert.rejects(r.activateRecovered(words.join(" "), Date.now() + RECOVERY_DELAY_MS + 60_000), invalid(RULES.hostKeyNext));
  assert.equal(hub.chainPosts, posts, "nothing posted");
  assert.equal(r.standing, "pending");
});

test("13: a new account's genesis is checked with the chain's own verifier before it is posted", async () => {
  const hub = new FakeKeysHub();
  // A hub that names the account with an id no chain takes (a space).
  (hub as { account: string }).account = "an account";
  const a = await hub.device("tok");
  await assert.rejects(a.createGenesis(true), (e: unknown) => e instanceof AccountError && e.kind === "invalidEntry" && e.rule === "badId");
  assert.equal(hub.chainPosts, 0, "nothing posted");
  assert.equal(a.chain, null);
  // The same hub with a good id: the genesis goes, checked the same way.
  const ok = new FakeKeysHub();
  const b = await ok.device("tok");
  assert.equal((await b.createGenesis(true))?.length, 12);
  assert.equal(ok.chainPosts, 1);
  assert.equal(b.chain?.seq, 1);
});

// ------------------------------------------------------------ before chain v3 `write`: the write gate

test("write gate: a narrowing built on a view from before another admin's removal is built again on the new head — and its machine key never goes under the user key the removed admin holds", async () => {
  const { hub, a } = await account();
  const b = await pairAs(hub, a, { admin: true });
  await b.receiveMail();
  const c = await pairAs(hub, a, { admin: true });
  await c.receiveMail();
  const e = await pairAs(hub, a, { admin: false });
  await admit(a, "m-1", e.deviceId);
  // a syncs; then b removes c — a whole rotation, a new user key c never had.
  await a.sync();
  const held = a.chain!.latestUserEpoch()!;
  assert.ok(await c.vault.has(held), "c holds the newest user key a sees");
  await b.sync();
  const removal = await b.removeDevice(c.deviceId, new FakeCourier());
  assert.equal(removal.ukEpoch, held + 1);
  assert.equal(a.chain!.latestUserEpoch(), held, "a's view is from before it");
  assert.equal(a.userKeyStale, false, "and nothing in it is stale");
  // a narrows m-1's list on that view: its first post meets the newer head (409) and is built again.
  const posts = hub.chainPosts;
  const at = a.chain!.seq + 1;
  const courier = new FakeCourier();
  const change = await a.setAccept("m-1", { type: "admins" }, courier);
  assert.equal(hub.chainPosts, posts + 2, "refused once on the old head, then taken");
  assert.deepEqual(change.shutOut, [e.deviceId]);
  assert.equal(a.chain!.deviceRecord(c.deviceId)?.status.type, "removed", "built again on the head with c removed");
  const ke = a.chain!.allEntries().slice(at).find((x) => x.draft.action.type === "keyEpochs" && x.draft.signer.type === "device" && x.draft.signer.id === a.deviceId)!.draft.action;
  assert.ok(ke.type === "keyEpochs", "a's own batch");
  const mkEpoch = ke.machines.find((m) => m.machine === "m-1")!.epoch;
  assert.equal(change.mkEpoch, mkEpoch);
  // Its machine key wrapped only under a user key c never held.
  assert.ok(ke.user !== null, `a new user key in the batch built again (it checked epoch ${held}; the head's newest is ${held + 1})`);
  assert.equal(ke.user!.epoch, held + 2);
  const wraps = hub.keysAt(`mk/m-1/${mkEpoch}`);
  assert.ok(wraps.length > 0, "the new machine key is in the keyring");
  for (const name of wraps) {
    const uk = Number(name.split("/")[3]);
    assert.notEqual(uk, held, `${name}: never under the user key c holds`);
    assert.equal(await c.vault.has(uk), false, `${name}: c cannot open it`);
  }
  assert.equal(change.userKey?.epoch, held + 2);
  assert.deepEqual(change.userKey?.sealedTo, [b.deviceId], "shared with the admin still there, never with c");
  assert.equal(courier.posted.at(-1)?.epoch, mkEpoch, "handed to the agent");
});

/** Have `b` remove admin `c` (a whole rotation) just before `a`'s next batch reaches the hub: a's post meets a newer head (409), and is built again on it. */
function removeBeforeNextPost(a: CloudAccount, b: CloudAccount, c: CloudAccount): { landed: () => boolean } {
  let done = false;
  const post = a.api.appendChain.bind(a.api);
  a.api.appendChain = async (token, entries) => {
    if (!done) {
      done = true;
      await b.sync();
      await b.removeDevice(c.deviceId, new FakeCourier());
    }
    return post(token, entries);
  };
  return { landed: () => done };
}

test("write gate: a rotation of machine keys alone, built before another admin's removal lands, is built again with a new user key — never under the one the removed admin holds", async () => {
  const { hub, a } = await account();
  const b = await pairAs(hub, a, { admin: true });
  await b.receiveMail();
  const c = await pairAs(hub, a, { admin: true });
  await c.receiveMail();
  const e = await pairAs(hub, a, { admin: false });
  await admit(a, "m-1", e.deviceId);
  const held = a.chain!.latestUserEpoch()!;
  assert.ok(await c.vault.has(held));
  // Removing e (it held no user key) rotates the machine keys alone, under the newest user key — until c's removal lands first.
  const race = removeBeforeNextPost(a, b, c);
  const report = await a.removeDevice(e.deviceId, new FakeCourier(), false, { rotateMachineKeys: false });
  assert.ok(race.landed());
  assert.equal(a.chain!.deviceRecord(c.deviceId)?.status.type, "removed");
  assert.equal(report.ukEpoch, held + 2, `built again on the head with c's rotation (epoch ${held + 1}): a new user key with it`);
  const mkEpoch = report.machines.find((m) => m.machine === "m-1")!.mkEpoch!;
  const wraps = hub.keysAt(`mk/m-1/${mkEpoch}`);
  assert.ok(wraps.length > 0);
  for (const name of wraps) assert.equal(await c.vault.has(Number(name.split("/")[3])), false, `${name}: c cannot open it`);
});

/** `account()` with another admin (b) and a passkey slot a set; p joins through it, and a hub whose clock is a day ahead takes p's self-activation at once. */
async function heldPasskeyAccount() {
  const { hub, a } = await account();
  const b = await pairAs(hub, a, { admin: true });
  await b.receiveMail();
  const slot = await addPasskeySlot(a);
  await b.sync();
  const p = await hub.device("tok", new RecordingKeyStore());
  await passkeyJoin(p, slot);
  await passkeyActivateEarly(hub, p, slot);
  assert.equal(hub.state!.device(p.deviceId)?.status.type, "active", "the hub took the activation");
  return { hub, a, b, p, slot };
}

test("write gate: a passkey device's self-activation too early here is held back with its device — and any admin can veto it", async () => {
  const { hub, b, p, slot } = await heldPasskeyAccount();
  const seen = b.chain!.seq;
  await b.sync();
  assert.equal(b.chain!.seq, seen + 1, "the join is in, its activation is not");
  assert.equal(b.chain!.deviceRecord(p.deviceId)?.status.type, "pending");
  assert.equal(b.heldBack?.device, p.deviceId, "the held activation names its device (docs/passkeys.md §4.4: any admin can veto)");
  assert.deepEqual(
    b.heldChanges().map((c) => [c.kind, c.by, c.subject]),
    [["passkeyActivate", slot.slot, p.deviceId]],
  );
  // Anything but the veto (or approving) waits — and says how the device came in.
  await assert.rejects(b.setRecoveryWords(), (e: unknown) => e instanceof AccountError && e.kind === "heldBack" && e.message.includes("joined through a passkey"));
  // The veto: on the hub's chain, the device out everywhere, and its slot with it.
  const report = await b.removeDevice(p.deviceId, new FakeCourier());
  assert.equal(b.heldBack, null);
  assert.equal(b.chain!.deviceRecord(p.deviceId)?.status.type, "removed");
  assert.equal(hub.state!.deviceRecord(p.deviceId)?.status.type, "removed", "on the hub too");
  assert.equal(b.chain!.seq, hub.state!.seq, "this browser follows the hub's chain again");
  assert.equal(hub.state!.slot(slot.slot), undefined, "the slot it came in through goes with it");
  assert.deepEqual(report.vetoed, { devices: [], machines: [] });
});

test("write gate: approving a held passkey activation takes the hub's chain on the user's word", async () => {
  const { hub, b, p } = await heldPasskeyAccount();
  await b.sync();
  assert.equal(b.heldBack?.device, p.deviceId);
  await b.approveDevice(p.deviceId);
  assert.equal(b.heldBack, null);
  assert.equal(b.chain!.device(p.deviceId)?.status.type, "active");
  assert.equal(b.chain!.device(p.deviceId)?.admin, false, "a passkey never grants the account");
  assert.equal(b.chain!.seq, hub.state!.seq, "nothing was appended: the activation is the hub's");
});

test("write gate: vetoing a held passkey activation takes out what came in through a passkey slot after it — what a trusted admin signed stands", async (t) => {
  const t0 = Date.now();
  t.mock.timers.enable({ apis: ["Date"], now: t0 });
  const { hub, a } = await account();
  const b = await pairAs(hub, a, { admin: true });
  await b.receiveMail();
  const slot = await addPasskeySlot(a);
  const other = await addPasskeySlot(a, "p-testslot0002");
  await b.sync();
  // p joins through one slot, q through another.
  const p = await hub.device("tok", new RecordingKeyStore());
  await passkeyJoin(p, slot);
  const q = await hub.device("tok", new RecordingKeyStore());
  await passkeyJoin(q, other);
  // a was open all along: it saw the joins then, and takes both activations a day later.
  await a.sync();
  t.mock.timers.setTime(t0 + PASSKEY_DELAY_MS + 60_000);
  await postAsPasskey(p, slot, { type: "passkeyActivate", id: p.deviceId });
  await postAsPasskey(q, other, { type: "passkeyActivate", id: q.deviceId });
  await a.sync();
  assert.equal(a.heldBack, null);
  assert.equal(a.chain!.device(p.deviceId)?.status.type, "active");
  assert.equal(a.chain!.device(q.deviceId)?.status.type, "active");
  // Meanwhile a (which b trusts) adds d; the thief's second device joins through the same slot, now that p is active.
  const d = await hub.device("tok", new RecordingKeyStore());
  await a.append(joinAction({ id: d.deviceId, signKey: d.keys.signPublic, dhKey: d.keys.dhPublic }, { host: false, admin: false }, a.chain!));
  const p2 = await hub.device("tok", new RecordingKeyStore());
  await passkeyJoin(p2, slot);
  assert.equal(hub.state!.deviceRecord(p2.deviceId)?.status.type, "pending");
  // b first sees the joins only now: p's activation is too early here, and all that follows it.
  await b.sync();
  assert.equal(b.heldBack?.device, p.deviceId);
  assert.equal(b.chain!.deviceRecord(q.deviceId)?.status.type, "pending");
  const scope = b.vetoScope(b.heldBack!, await ChainState.verify([...b.chain!.allEntries(), ...b.heldBack!.entries], undefined, ChainClock.history()));
  assert.deepEqual(scope.devices, [p.deviceId, q.deviceId, p2.deviceId], "p, and what a passkey slot activated or brought in after it");
  // The page's list (#17): a's addition is offered apart, by whom; nothing here knows the recovery words.
  const plan = (await b.vetoPlan())!;
  assert.deepEqual(plan.scope.map((x) => [x.id, x.by]), [[p.deviceId, null], [q.deviceId, other.slot], [p2.deviceId, slot.slot]]);
  assert.deepEqual(plan.trusted, [{ id: d.deviceId, host: false, by: a.deviceId }]);
  assert.deepEqual([plan.voidsWords, plan.trustedVoidsWords], [false, false]);
  assert.deepEqual([plan.voidsPasskeys, plan.trustedVoidsPasskeys], [true, false], "the slots p, q and p2 came in through go with them");
  const report = await b.removeDevice(p.deviceId, new FakeCourier());
  assert.deepEqual(report.vetoed, { devices: [q.deviceId, p2.deviceId], machines: [] });
  for (const x of [p, q, p2]) assert.equal(hub.state!.deviceRecord(x.deviceId)?.status.type, "removed", `${x.deviceId} is out`);
  assert.equal(hub.state!.device(d.deviceId)?.status.type, "active", "a's addition stands");
});

test("N1 (#17) on a v3 chain: asked, the veto takes out a device that runs AI a trusted admin enrolled meanwhile, and the recovery words a device it added set — and says so first", async (t) => {
  const t0 = Date.now();
  t.mock.timers.enable({ apis: ["Date"], now: t0 });
  const { hub, a, words } = await account();
  const b = await pairAs(hub, a, { admin: true });
  await b.receiveMail();
  await a.sync();
  // z comes back with the words; b, open all along, sees the join and takes the activation a day later.
  const z = await hub.device("tok", new RecordingKeyStore());
  await z.recover(words.join(" "));
  await b.sync();
  t.mock.timers.setTime(t0 + RECOVERY_DELAY_MS + 60_000);
  await z.activateRecovered(words.join(" "));
  await b.sync();
  assert.equal(b.heldBack, null);
  // Meanwhile b adds c, an admin, which makes new recovery words; and b enrolls m-9.
  const c = await pairAs(hub, b, { admin: true });
  await c.receiveMail();
  await c.setRecoveryWords();
  await b.sync();
  await hub.enroll(b, "m-9", { writable: true });
  // a opens only now: z's activation, and all after it, are held back here.
  const back = await hub.reload(a);
  assert.equal(back.heldBack?.device, z.deviceId);
  const plan = (await back.vetoPlan())!;
  assert.deepEqual(plan.scope, [{ id: z.deviceId, host: false, by: null }]);
  assert.deepEqual(plan.trusted, [
    { id: c.deviceId, host: false, by: b.deviceId },
    { id: "m-9", host: true, by: b.deviceId },
  ]);
  assert.equal(plan.voidsWords, false, "z knows only the words c has replaced");
  assert.equal(plan.trustedVoidsWords, true, "c set the current ones");
  const report = await back.removeDevice(z.deviceId, new FakeCourier(), false, { alsoTrusted: plan.trusted.map((x) => x.id) });
  for (const id of [z.deviceId, c.deviceId, "m-9"]) assert.equal(hub.state!.deviceRecord(id)?.status.type, "removed", `${id} is out`);
  assert.ok(hub.state!.admin(a.deviceId) && hub.state!.admin(b.deviceId), "a and b still manage the account");
  assert.ok(hub.state!.machine("m-1"), "m-1, which a already trusted, stays");
  assert.deepEqual(report.vetoed, { devices: [c.deviceId], machines: ["m-9"] });
  assert.ok(!report.machines.some((m) => m.machine === "m-9") && report.machines.some((m) => m.machine === "m-1"), "new keys for m-1 only");
  const removals = hub.state!.allEntries().filter((e) => e.draft.action.type === "removeDevice3");
  assert.ok(removals.length === 3 && removals.every((e) => e.draft.version === 3 && e.draft.signer.type === "device" && e.draft.signer.id === a.deviceId));
  assert.equal(back.chain!.currentRecoveryKey, null, "the words c set stop counting");
});
