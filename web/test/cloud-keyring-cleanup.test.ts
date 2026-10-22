// The page's side of the mailbox and keyring cleanup (relay-and-plans §6;
// `lib/hub/account.ts`, `keyring.ts`, `pairing.ts`) and of the device limit
// on the chain (§4.3):
//
// - after a rotation to a new user key, the rotating device rewrites every
//   machine-key wrap it can read under the new key and its own name, so the
//   hub can reclaim its own older wraps (a removed device keeps one per epoch)
//   and old ciphertext stays readable from the newest user key alone;
// - recovery packages: a rotation writes its own; the settle steps only fill
//   an epoch no current device covers (the hub keeps the newest per epoch, so
//   two devices would otherwise take turns rewriting theirs);
// - mail of a purpose the page does not understand is acknowledged, on the
//   pairing screen too;
// - a 409 `limit` from the chain is not retried as a stale head is.
import assert from "node:assert/strict";
import test from "node:test";
import { fromHex, hex, MemoryKeyStore } from "../src/lib/e2e/index.js";
import type { CloudAccount } from "../src/lib/hub/account.js";
import { HubApiError, knownMailPurpose, MAIL_KEY_REQUEST, MAIL_MACHINE_KEY, MAIL_USER_KEY } from "../src/lib/hub/api.js";
import { keyName, Keyring, parseKeyName, rewrapTargets } from "../src/lib/hub/keyring.js";
import { candidateFromOffer, completePairing, PairingResponder } from "../src/lib/hub/pairing.js";
import { limitOf } from "../src/lib/hub/plan.js";
import { createV2Genesis, FakeCourier, FakeKeysHub, RecordingKeyStore } from "./fake-keys-hub.js";

async function pair(hub: FakeKeysHub, a: CloudAccount): Promise<CloudAccount> {
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
    await completePairing(
      {
        api: a.api,
        token: a.session.token,
        device: a.deviceId,
        signPublic: a.keys.signPublic,
        noise: a.noiseKeys,
        chain: () => a.chain!,
        append: (action) => a.append(action),
        withUserKeys: (use) => a.withUserKeys(use),
      },
      c,
    );
    for (let i = 0; i < 10 && r.state.kind !== "paired"; i++) await r.tick();
  } finally {
    r.stop();
  }
  assert.equal(r.state.kind, "paired");
  return b;
}

async function setup(withRecovery = true) {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await createV2Genesis(a, withRecovery);
  const m1 = await hub.enroll(a, "m-1");
  await a.handOverMachineKey(m1.id, new FakeCourier());
  const m2 = await hub.enroll(a, "m-2");
  await a.handOverMachineKey(m2.id, new FakeCourier());
  const b = await pair(hub, a);
  return { hub, a, b, m1, m2 };
}

/** The `mk/…` names the hub holds, as `machine/mk/uk/writer`. */
function wraps(hub: FakeKeysHub): string[] {
  return [...hub.keys.keys()]
    .map((n) => parseKeyName(n))
    .flatMap((p) => (p?.kind === "machine" ? [`${p.machine}/${p.mkEpoch}/${p.ukEpoch}/${p.writer}`] : []))
    .sort();
}

test("a rotation rewrites every machine-key wrap it can read under the new user key, in its own name", async () => {
  const { hub, a, b, m1, m2 } = await setup();
  const report = await a.removeDevice(b.deviceId, new FakeCourier());
  assert.equal(report.ukEpoch, 1);
  // Each machine's epoch 0 again under user key 1; epoch 1 was made under it.
  assert.deepEqual(report.rewrapped, { done: [{ machine: m1.id, mkEpoch: 0 }, { machine: m2.id, mkEpoch: 0 }], failed: [] });
  const me = a.deviceId;
  assert.deepEqual(wraps(hub), [`${m1.id}/0/0/${me}`, `${m1.id}/0/1/${me}`, `${m1.id}/1/1/${me}`, `${m2.id}/0/0/${me}`, `${m2.id}/0/1/${me}`, `${m2.id}/1/1/${me}`]);
  // In the keyring before the new user key is mailed (a device reading it then finds them all).
  const mailedAt = hub.requests.findIndex((r) => r.startsWith("mail ") && r.endsWith(` ${MAIL_USER_KEY} 1`));
  for (const m of [m1.id, m2.id]) {
    const putAt = hub.requests.indexOf(`put ${keyName.written(keyName.machine(m, 0, 1), me)}`);
    assert.ok(putAt >= 0 && (mailedAt < 0 || putAt < mailedAt), hub.requests.join("\n"));
  }

  // Rotating again: every epoch again, under user key 2 — and nothing twice.
  const again = await a.rotate(new FakeCourier());
  assert.equal(again.ukEpoch, 2);
  assert.deepEqual(
    again.rewrapped?.done.map((r) => `${r.machine}/${r.mkEpoch}`),
    [`${m1.id}/0`, `${m1.id}/1`, `${m2.id}/0`, `${m2.id}/1`],
  );
  assert.deepEqual(rewrapTargets(a.keyring, [{ id: m1.id, epochs: 3 }, { id: m2.id, epochs: 3 }], new Set([0, 1, 2]), 2), [], "written once: nothing left to do");

  // The hub reclaims what the newest user key supersedes (relay-and-plans §6):
  // every epoch still opens from user key 2 alone, here and on a fresh read.
  for (const name of [...hub.keys.keys()]) {
    const p = parseKeyName(name);
    if (p?.kind === "machine" && p.ukEpoch < 2) hub.keys.delete(name);
  }
  await a.keyring.refresh();
  for (const m of [m1.id, m2.id]) assert.deepEqual([...(await a.machineKeys(m)).keys()].sort(), [0, 1, 2], m);
  const reloaded = await hub.reload(a);
  assert.deepEqual([...(await reloaded.machineKeys(m1.id)).keys()].sort(), [0, 1, 2]);
});

test("a wrap the hub already holds under that name with other bytes goes under a new suffix; the same bytes are a retry", async () => {
  const { hub, a, b, m1 } = await setup(false);
  const me = a.deviceId;
  // Planted as this device's own earlier blob (another tab, or the hub): other bytes.
  hub.looseKeyNames = true;
  const taken = keyName.written(keyName.machine(m1.id, 0, 1), me);
  hub.keys.set(taken, { blob: hex(new Uint8Array(60)), updated_ms: 0, writer: me });
  hub.looseKeyNames = false;
  const report = await a.removeDevice(b.deviceId, new FakeCourier());
  assert.ok(report.rewrapped?.done.some((r) => r.machine === m1.id && r.mkEpoch === 0));
  const names = hub.keysAt(keyName.machine(m1.id, 0, 1));
  assert.equal(names.length, 2, names.join(", "));
  assert.ok(names.some((n) => n.startsWith(`${taken}.`)), "the next suffix");
  await a.keyring.refresh();
  assert.ok(await a.machineKeyBytes(m1.id, 0, 1), "the suffixed one opens to the committed key");
  // The same bytes under a name already written: a retry, no new name.
  const base = keyName.machine(m1.id, 1, 1);
  const name = keyName.written(base, me);
  const before = hub.keysAt(base);
  assert.equal(await a.keyring.put(base, fromHex(hub.keys.get(name)!.blob)), name);
  assert.deepEqual(hub.keysAt(base), before);
});

test("the rewrite list: readable epochs only, the ones not written yet, machine by machine", () => {
  const store = new MemoryKeyStore();
  const keyring = new Keyring({ list: async () => [], put: async () => ({ stored: true }) }, store, "a-1", "d-me");
  const entries = keyring as unknown as { entries: Map<string, { name: string; blob: Uint8Array; updatedMs: number }> };
  for (const name of [
    "mk/m-1/0/0/d-old", // readable (UK 0 held)
    "mk/m-1/1/1/d-old", // readable
    "mk/m-1/1/2/d-me", // already rewritten
    "mk/m-1/2/2/d-me", // this rotation's own new epoch
    "mk/m-1/5/0/d-old", // not committed (the chain has 3 epochs)
    "mk/m-2/0/7/d-old", // under a user key this device does not hold
    "mk/m-2/1/1/d-old.0a0b", // a retry's name
  ]) {
    entries.entries.set(name, { name, blob: new Uint8Array(1), updatedMs: 0 });
  }
  assert.deepEqual(
    rewrapTargets(keyring, [{ id: "m-1", epochs: 3 }, { id: "m-2", epochs: 2 }], new Set([0, 1, 2]), 2),
    [
      { machine: "m-1", mkEpoch: 0 },
      { machine: "m-2", mkEpoch: 1 },
    ],
  );
});

test("recovery packages: a rotation writes its own for every epoch; the settle steps only fill an epoch nobody current covers", async () => {
  const { hub, a, b } = await setup();
  const rsk = () => a.chain!.currentRecoveryKey!;
  const own = (who: CloudAccount, epoch: number) => hub.keys.has(keyName.written(keyName.recovery(epoch, who.deviceId, rsk()), who.deviceId));
  assert.ok(own(a, 0));
  await b.keyring.refresh();
  // b's settle: a current device covers epoch 0 already — no second package (the hub would keep one, and they would take turns).
  assert.equal(await b.ensureRecoveryPackages("any"), 0);
  assert.ok(!own(b, 0));
  // A rotation by b: its own package for every epoch it holds.
  const c = await pair(hub, a);
  await b.sync();
  await b.removeDevice(c.deviceId, new FakeCourier());
  assert.ok(own(b, 0) && own(b, 1), "the rotating device's own, for every epoch");
  // The package of a device no longer current does not count: here the only
  // one left for epoch 0 is c's (removed) — as the hub serves a wrap whose
  // writer left when nobody current holds that name.
  await a.sync();
  await a.receiveMail();
  assert.deepEqual(await a.vault.epochs(), [0, 1]);
  const tag = hex(rsk().slice(0, 8));
  for (const name of [...hub.keys.keys()]) if (name.startsWith("rk/0/")) hub.keys.delete(name);
  hub.keys.set(`rk/0/${c.deviceId}/${tag}/${c.deviceId}`, { blob: hex(new Uint8Array(80)), updated_ms: hub.now(), writer: c.deviceId });
  await a.keyring.refresh();
  assert.equal(await a.ensureRecoveryPackages("any"), 1, "epoch 0 (only a removed device's): filled; epoch 1 (b's): left");
  assert.ok(own(a, 0) && !own(a, 1));
});

test("mail of a purpose the page does not understand is acknowledged, on the pairing screen too", async () => {
  // 5: a host's machine key for a device that is not an admin (chain v3).
  assert.deepEqual([1, 2, 3, 4, 5, 6, 9, 0].map(knownMailPurpose), [true, true, true, true, true, false, false, false]);
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await createV2Genesis(a, false);
  const c = await hub.device("tok");
  const r = new PairingResponder({
    api: c.api,
    token: c.session.token,
    account: c.account,
    device: c.deviceId,
    signPublic: c.keys.signPublic,
    noise: c.noiseKeys,
    chain: async () => (await c.sync(), c.chain),
    storeUserKeys: (keys) => c.storeUserKeys(keys),
    pollMs: 1,
  });
  try {
    await r.startOffer();
    for (const [id, purpose] of [
      [80_001, 9],
      [80_002, MAIL_MACHINE_KEY],
      [80_003, MAIL_USER_KEY],
      [80_004, MAIL_KEY_REQUEST],
    ] as const) {
      hub.mail.push({ id, recipient: c.deviceId, sender: a.deviceId, purpose, epoch: 0, sealed: "00" });
    }
    await r.tick();
  } finally {
    r.stop();
  }
  assert.deepEqual(
    hub.mail.filter((m) => m.recipient === c.deviceId).map((m) => m.purpose).sort(),
    [MAIL_USER_KEY, MAIL_KEY_REQUEST],
    "keys and requests wait for the device to be in; the rest is acknowledged",
  );
});

test("a used-up device limit is not retried as a stale head is", async () => {
  const hub = new FakeKeysHub();
  let refuse = false;
  const appends: string[] = [];
  const inner = hub.fetch;
  hub.fetch = async (input, init) => {
    if (refuse && new URL(input, "https://hub.test").pathname === "/v1/chain" && init?.method === "POST") {
      appends.push(String(init.body));
      return hub.json(409, { error: "limit", key: "devices.max", limit: 1, used: 1, message: "this account has 1 devices, as many as its plan allows: remove one first" });
    }
    return inner(input, init);
  };
  const a = await hub.device("tok");
  await createV2Genesis(a, false);
  const c = await hub.device("tok");
  refuse = true;
  await assert.rejects(
    pairWith(a, c),
    (error: unknown) => error instanceof HubApiError && limitOf(error)?.key === "devices.max" && limitOf(error)?.limit === 1,
  );
  assert.equal(appends.length, 1, "one try: a limit is not a newer head");
});

async function pairWith(a: CloudAccount, c: CloudAccount): Promise<void> {
  const r = new PairingResponder({
    api: c.api,
    token: c.session.token,
    account: c.account,
    device: c.deviceId,
    signPublic: c.keys.signPublic,
    noise: c.noiseKeys,
    chain: async () => (await c.sync(), c.chain),
    storeUserKeys: (keys) => c.storeUserKeys(keys),
    pollMs: 1,
  });
  try {
    const candidate = await candidateFromOffer(await r.startOffer(), a.chain!);
    await completePairing(
      {
        api: a.api,
        token: a.session.token,
        device: a.deviceId,
        signPublic: a.keys.signPublic,
        noise: a.noiseKeys,
        chain: () => a.chain!,
        append: (action) => a.append(action),
        withUserKeys: (use) => a.withUserKeys(use),
      },
      candidate,
    );
  } finally {
    r.stop();
  }
}
