// User and machine keys at rest (`lib/hub/keyring.ts`) and machine-key
// delivery (`lib/hub/deliver.ts`), against a fake hub.
//
// What these pin: a UK epoch is never written to the key store as bytes (only
// encrypted under the device-local key) and an MK never at all (only its
// keyring blob, wrapped under a UK); the keyring names are exactly
// `mk/<machine>/<mkEpoch>/<ukEpoch>` and `rk/<ukEpoch>/<device>/<rsk tag>`,
// parsed with bounded numbers, and the blobs open with the right contexts;
// M1's raw `mk/<machine>/<epoch>` entries are deleted only once the hub holds
// a blob that unwraps to them; every machine key epoch is committed in the
// chain before it is used; an MK goes inside the channel when the machine is
// online, by sealed mailbox when it is not (or the channel fails), and not at
// all when the agent refuses it.
import assert from "node:assert/strict";
import test from "node:test";
import {
  context,
  fromHex,
  hex,
  importMachineKey,
  machineKeyCommit,
  indexStream,
  MemoryKeyStore,
  openFrom,
  openFromRecovery,
  RECOVERY_RECIPIENT,
  recoveryKeys,
  unwrapUnder,
} from "../src/lib/e2e/index.js";
import { recoverySecret } from "../src/lib/hub/recovery-words.js";
import { ChannelUnavailable, deliverMachineKey, flushPendingReencrypts, mailMachineKey, pendingReencrypts } from "../src/lib/hub/deliver.js";
import { keyName, legacyMachineKeyName, parseKeyName, UserKeyVault, validKeyName } from "../src/lib/hub/keyring.js";
import { containsBytes, createV2Genesis, FakeCourier, FakeKeysHub, RecordingKeyStore } from "./fake-keys-hub.js";

async function ukFromRecovery(hub: FakeKeysHub, words: string[], epoch: number, sender: string) {
  const a = await hub.device();
  const rk = await recoverySecret(words.join(" "));
  const keys = await recoveryKeys(rk);
  const blob = fromHex(hub.keyAt(keyName.recovery(epoch, sender, a.chain!.currentRecoveryKey!))!.blob);
  return openFromRecovery(keys.seal, a.chain!, { purpose: "userKey", account: hub.account, sender, recipient: RECOVERY_RECIPIENT, epoch }, blob);
}

test("keyring names are the hub's, and parse back", () => {
  assert.equal(keyName.machine("m-abc", 2, 1), "mk/m-abc/2/1");
  const rsk = new Uint8Array(32).fill(0xab);
  assert.equal(keyName.recovery(3, "d-xyz", rsk), "rk/3/d-xyz/abababababababab");
  // Today's names end with their writer (M2 re-review); older ones parse with none.
  assert.equal(keyName.written("mk/m-abc/2/1", "d-1"), "mk/m-abc/2/1/d-1");
  assert.equal(keyName.written("mk/m-abc/2/1", "d-1", "0a0b"), "mk/m-abc/2/1/d-1.0a0b");
  assert.deepEqual(parseKeyName("mk/m-abc/2/1/d-1"), { kind: "machine", machine: "m-abc", mkEpoch: 2, ukEpoch: 1, base: "mk/m-abc/2/1", writer: "d-1" });
  assert.deepEqual(parseKeyName("mk/m-abc/2/1/d-1.0a0b"), { kind: "machine", machine: "m-abc", mkEpoch: 2, ukEpoch: 1, base: "mk/m-abc/2/1", writer: "d-1" });
  assert.deepEqual(parseKeyName("rk/3/d-xyz/abababababababab/d-xyz"), {
    kind: "recovery",
    ukEpoch: 3,
    sender: "d-xyz",
    tag: "abababababababab",
    base: "rk/3/d-xyz/abababababababab",
    writer: "d-xyz",
  });
  assert.equal(parseKeyName("rk/3/d-xyz/abababababababab/d-other"), null, "a package's sender is its writer");
  assert.deepEqual(parseKeyName("mk/m-abc/2/1"), { kind: "machine", machine: "m-abc", mkEpoch: 2, ukEpoch: 1, base: "mk/m-abc/2/1", writer: null });
  assert.deepEqual(parseKeyName("rk/3/d-xyz/abababababababab"), { kind: "recovery", ukEpoch: 3, sender: "d-xyz", tag: "abababababababab", base: "rk/3/d-xyz/abababababababab", writer: null });
  assert.deepEqual(parseKeyName("rk/3/d-xyz"), { kind: "recovery", ukEpoch: 3, sender: "d-xyz", tag: null, base: "rk/3/d-xyz", writer: null });
  assert.deepEqual(parseKeyName("rk/4294967295/x"), { kind: "recovery", ukEpoch: 4294967295, sender: "x", tag: null, base: "rk/4294967295/x", writer: null });
  for (const bad of ["mk/m/1", "mk/m/x/1", "rk/1", "uk/1", "mk//1/1", "rk/-1/d", "rk/4294967296/x", "mk/m/99999999999/0", "mk/m/0/1e3", "rk/1/d/xyz", "rk/1/d/ab/cd", "mk/m/0/0/", "mk/m/0/0/.x"]) {
    assert.equal(parseKeyName(bad), null, bad);
  }
  assert.ok(validKeyName("mk/m-abc/0/0"));
  assert.ok(!validKeyName("mk/M-ABC/0/0"));
  assert.ok(!validKeyName("mk/../0/0"));
  assert.ok(!validKeyName("x".repeat(129)));
});

test("no raw user or machine key is ever written to the key store", async () => {
  const hub = new FakeKeysHub();
  const store = new RecordingKeyStore();
  const a = await hub.device("tok", store);
  const words = await createV2Genesis(a, true);
  assert.equal(words?.length, 12);
  const machine = await hub.enroll(a);
  const courier = new FakeCourier();
  // Enrolling committed MK epoch 0 and put its blob; it goes to the agent from there.
  const handed = await a.handOverMachineKey(machine.id, courier);
  assert.deepEqual(handed, { mkEpoch: 0, result: { via: "channel", changed: true } });

  const uk = await ukFromRecovery(hub, words!, 0, a.deviceId);
  const mk = fromHex(courier.posted[0]!.key);
  assert.equal(uk.length, 32);
  assert.equal(mk.length, 32);
  assert.ok(store.puts.length > 0);
  for (const { name, value } of store.puts) {
    assert.ok(!containsBytes(value, uk), `UK bytes at rest under ${name}`);
    assert.ok(!containsBytes(value, mk), `MK bytes at rest under ${name}`);
  }
  // What is there: the local wrap key (a CryptoKey), the encrypted UK, its
  // index, the device keys, and the keyring cache (wrapped blobs only).
  const names = new Set(store.puts.map((p) => p.name));
  assert.ok(names.has("local-wrap/v1"));
  assert.ok(names.has(`uk/${hub.account}/0`));
  assert.ok(names.has(`keyring/${hub.account}`));
  assert.ok(![...names].some((n) => /^mk\//.test(n)), "no machine key entries at all");
  const local = await store.get("local-wrap/v1");
  assert.equal((local as CryptoKey).extractable, false);

  // The keyring blob is the MK under UK epoch 0, in exactly its context.
  const blob = fromHex(hub.keyAt("mk/m-1/0/0")!.blob);
  assert.equal(hex(await unwrapUnder(uk, context.machineKeyUnderUserKey(hub.account, "m-1", 0, 0), blob)), hex(mk));
  await assert.rejects(unwrapUnder(uk, context.machineKeyUnderUserKey(hub.account, "m-1", 0, 1), blob), { name: "WrapError" });

  // And the page reads the machine's streams with it (non-extractable).
  const keys = await a.machineKeys("m-1", store);
  assert.deepEqual([...keys.keys()], [0]);
  assert.equal(keys.get(0)!.hkdf.extractable, false);
  assert.equal(hex(await indexStream(keys.get(0)!)), hex(await indexStream(await importMachineKey(mk))));
});

test("a UK epoch at rest opens only for its own account and epoch", async () => {
  const store = new MemoryKeyStore();
  const vault = new UserKeyVault(store, "a-1");
  const raw = crypto.getRandomValues(new Uint8Array(32));
  assert.ok(await vault.put(0, raw));
  assert.equal(await vault.put(0, crypto.getRandomValues(new Uint8Array(32))), false, "an epoch held is kept");
  assert.equal(hex(await vault.withRaw(0, async (b) => b.slice())), hex(raw));
  const key = await vault.key(0);
  assert.equal(key.extractable, false);
  assert.equal(key.algorithm.name, "HKDF");
  // Moved under another account's or epoch's name, it does not open.
  await store.put("uk/a-2/0", await store.get("uk/a-1/0"));
  await store.put("uk-epochs/a-2", [0]);
  await assert.rejects(new UserKeyVault(store, "a-2").key(0));
  await vault.wipe();
  assert.deepEqual(await vault.epochs(), []);
  assert.equal(await store.get("uk/a-1/0"), undefined);
});

test("M1's raw machine keys are deleted only once the hub holds a blob that unwraps to them", async () => {
  const hub = new FakeKeysHub();
  const store = new RecordingKeyStore();
  const a = await hub.device("tok", store);
  await createV2Genesis(a, false);
  await hub.enroll(a);
  await a.rotate(new FakeCourier()); // UK epoch 1 is the newest: legacy keys go under it
  const committed0 = await a.machineKeyBytes("m-1", 0);
  // As M1 left it: an MK as hex in the key store.
  const mk = crypto.getRandomValues(new Uint8Array(32));
  await store.put(legacyMachineKeyName("m-1", 0), hex(mk));
  const before = store.puts.length;
  // The hub holds garbage under this device's own name already (tampering:
  // only this device writes names ending with it). The blob goes under a
  // new name of its own (M2 re-review: a taken name used to mean "skip"),
  // and the raw copy goes only once a blob the hub holds unwraps to it.
  hub.keys.set(`mk/m-1/0/1/${a.deviceId}`, { blob: hex(new Uint8Array(60).fill(7)), updated_ms: 0, writer: a.deviceId });
  const again = await hub.reload(a);
  assert.equal(again.deviceId, a.deviceId);
  assert.equal(await store.get("mk/m-1/0"), undefined, "the raw entry is gone");
  assert.equal(hub.keysAt("mk/m-1/0/1").length, 2, "the garbage, and the blob under a new name");
  assert.match(hub.keysAt("mk/m-1/0/1")[1]!, new RegExp(`^mk/m-1/0/1/${a.deviceId}\\.[0-9a-f]{8}$`));
  for (const { value } of store.puts.slice(before)) assert.ok(!containsBytes(value, mk));
  // But the streams are read only with the key the chain committed for epoch 0
  // (the blob under UK 1 holds another one: tried, and passed over).
  const keys = await again.machineKeys("m-1", store);
  assert.deepEqual([...keys.keys()].sort(), [0, 1]);
  assert.equal(hex(await indexStream(keys.get(0)!)), hex(await indexStream(await importMachineKey(committed0!))));
});

test("an MK goes in the channel when the machine is online, else by sealed mailbox", async () => {
  const mk = crypto.getRandomValues(new Uint8Array(32));
  const online = new FakeCourier();
  assert.deepEqual(await deliverMachineKey(online, "m-1", 3, mk, true), { via: "channel", changed: true });
  assert.deepEqual(online.posted, [{ machine: "m-1", epoch: 3, key: hex(mk), reencrypt: true }]);
  assert.equal(online.mailed.length, 0);
  assert.deepEqual(online.waited, [10_000], "waits about ten seconds for the machine");

  const offline = new FakeCourier();
  offline.online = false;
  assert.deepEqual(await deliverMachineKey(offline, "m-1", 3, mk, false), { via: "mailbox", reason: "offline" });
  assert.equal(offline.posted.length, 0);
  assert.deepEqual(offline.mailed, [{ machine: "m-1", epoch: 3, key: hex(mk) }]);

  const broken = new FakeCourier();
  broken.channelThrows = true;
  assert.deepEqual(await deliverMachineKey(broken, "m-1", 3, mk, false), { via: "mailbox", reason: "failed" });
  assert.equal(broken.mailed.length, 1);

  // Why the channel would not open is said as it is: the hub says offline (4404), or the agent wants a fresh attestation.
  const gone = new FakeCourier();
  gone.channelError = new ChannelUnavailable("offline", "the machine is offline");
  assert.deepEqual(await deliverMachineKey(gone, "m-1", 3, mk, false), { via: "mailbox", reason: "offline" });
  const stale = new FakeCourier();
  stale.channelError = new ChannelUnavailable("unconfirmed", "the machine refused the channel");
  assert.deepEqual(await deliverMachineKey(stale, "m-1", 3, mk, false), { via: "mailbox", reason: "unconfirmed" });

  const refused = new FakeCourier();
  refused.channelStatus = 409;
  const r = await deliverMachineKey(refused, "m-1", 3, mk, false);
  assert.equal(r.via, "refused");
  assert.equal(refused.mailed.length, 0, "an agent that refused it in the channel would refuse it by mail too");
});

test("the mailbox fallback is sealed from this device to the agent, which opens it", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device();
  await createV2Genesis(a, false);
  const machine = await hub.enroll(a);
  const mk = crypto.getRandomValues(new Uint8Array(32));
  await mailMachineKey(a.api, a.session.token, a.account, a.deviceId, a.noiseKeys, machine.id, machine.dh.publicKey, 2, mk);
  const mail = hub.mail.find((m) => m.recipient === "m-1")!;
  assert.equal(mail.purpose, 2);
  assert.equal(mail.epoch, 2);
  const opened = await openFrom(machine.dh, a.keys.dhPublic, { purpose: "machineKey", account: a.account, sender: a.deviceId, recipient: "m-1", epoch: 2 }, fromHex(mail.sealed));
  assert.equal(hex(opened), hex(mk));
});

test("issuing a machine key commits it, puts its blob before delivering it, and numbers epochs from the chain", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device();
  await createV2Genesis(a, false);
  await hub.enroll(a);
  assert.equal(a.chain!.latestMachineEpoch("m-1"), 0, "enrolling committed epoch 0");
  // A name for a far epoch in the keyring numbers nothing.
  hub.keys.set("mk/m-1/4000000000/0", { blob: hex(new Uint8Array(60)), updated_ms: 0 });
  await a.keyring.refresh();
  const courier = new FakeCourier();
  courier.online = false;
  const first = await a.issueMachineKey("m-1", courier);
  const second = await a.issueMachineKey("m-1", courier, true);
  assert.equal(first.mkEpoch, 1);
  assert.equal(second.mkEpoch, 2);
  assert.equal(a.chain!.latestMachineEpoch("m-1"), 2);
  assert.deepEqual(
    [...hub.keys.keys()].filter((k) => k.startsWith("mk/") && !k.includes("4000000000")).sort(),
    ["mk/m-1/0/0", "mk/m-1/1/0", "mk/m-1/2/0"].map((base) => `${base}/${a.deviceId}`),
  );
  // The commitment landed before the blob, the blob before the delivery.
  assert.deepEqual(courier.mailed.map((m) => m.epoch), [1, 2]);
  for (const m of courier.mailed) await a.chain!.checkMachineKey("m-1", m.epoch, fromHex(m.key));
  const keys = await a.machineKeys("m-1");
  assert.deepEqual([...keys.keys()].sort(), [0, 1, 2]);
});

test("a re-encryption that could only go by mailbox is noted, and asked again in the next channel", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device();
  await createV2Genesis(a, false);
  await hub.enroll(a);
  const courier = new FakeCourier();
  courier.channelError = new ChannelUnavailable("unconfirmed", "stale");
  const { mkEpoch, result } = await a.issueMachineKey("m-1", courier, true);
  assert.deepEqual(result, { via: "mailbox", reason: "unconfirmed", pending: true });
  const mailed = courier.mailed[0]!.key;
  assert.deepEqual(pendingReencrypts(a.account), [
    { machine: "m-1", mkEpoch, ukEpoch: 0, mkHash: hex(await machineKeyCommit(a.account, "m-1", mkEpoch, fromHex(mailed))) },
  ]);
  assert.equal(hex((await a.machineKeyBytes("m-1", mkEpoch))!), mailed, "the key is read back from the keyring");
  assert.equal(await a.machineKeyBytes("m-1", 9), null);

  // The next channel fails too: kept.
  const asked: unknown[] = [];
  assert.equal(await flushPendingReencrypts(a.account, "m-1", (item) => a.pinnedMachineKey(item), async (body) => (asked.push(body), { status: 503, body: null })), 1);
  assert.equal(pendingReencrypts(a.account).length, 1);
  // Then one answers: asked once with the same key, and cleared.
  await flushPendingReencrypts(a.account, "m-1", (item) => a.pinnedMachineKey(item), async (body) => (asked.push(body), { status: 200, body: { changed: false, reencrypt: true } }));
  assert.deepEqual(asked.at(-1), { epoch: mkEpoch, key: mailed, reencrypt: true });
  assert.deepEqual(pendingReencrypts(a.account), []);
  assert.equal(await flushPendingReencrypts(a.account, "m-1", (item) => a.pinnedMachineKey(item), async () => ({ status: 200, body: null })), 0);

  // Without a re-encryption asked for, nothing is noted.
  courier.channelError = new ChannelUnavailable("offline", "offline");
  const plain = await a.issueMachineKey("m-1", courier, false);
  assert.deepEqual(plain.result, { via: "mailbox", reason: "offline" });
  assert.deepEqual(pendingReencrypts(a.account), []);
});
