// The M2 fix re-review's account-level findings, as regressions against a
// fake hub. The re-review's proofs of concept each worked before; ported
// here, each must now fail:
//
// - PoC A: a browser closed across a recovery join and its
//   self-activation was locked out for good — `sync` threw `tooEarly` before
//   it saved its first sighting of the join, so every load saw the join anew.
// - PoC C: `confirmRemoval` verified with every entry as history, so
//   a hub with the recovery words could join, self-activate at once and
//   remove the owner — whose page then wiped its keys.
// - PoC B / B2: a device that missed a rotation's user key (the hub
//   dropped the mail, or the rotation died after its commit) wrapped a new
//   machine's key under the old user key, which the removed device holds.
// - Two more: a device about to be removed squatted the next rotation's
//   keyring names, or filled the others' mailboxes, so the rotation skipped
//   a machine or bounced the new user key.
import assert from "node:assert/strict";
import test from "node:test";
import { ChainClock, ChainState, type DeviceInfo } from "../src/lib/e2e/index.js";
import { AccountError, type CloudAccount, requestedFirst } from "../src/lib/hub/account.js";
import { MAIL_KEY_REQUEST, MAIL_USER_KEY } from "../src/lib/hub/api.js";
import { flushPendingReencrypts, forgetPendingReencrypts, notePendingReencrypt, pendingReencrypts, reencryptSettled } from "../src/lib/hub/deliver.js";
import { keyName, wrapMachineKey } from "../src/lib/hub/keyring.js";
import { candidateFromOffer, completePairing, PairingResponder } from "../src/lib/hub/pairing.js";
import { createV2Genesis, FakeCourier, FakeKeysHub, RecordingKeyStore } from "./fake-keys-hub.js";

const DAY = 24 * 60 * 60 * 1000;

/** A (genesis, recovery words) and its hub; B, a second browser, recovers while A is closed and activates itself a day later by the hub's clock. */
async function recoveredWhileClosed(t: { mock: { timers: { enable(o: { apis: string[]; now: number }): void; setTime(n: number): void } } }) {
  const t0 = Date.now();
  t.mock.timers.enable({ apis: ["Date"], now: t0 });
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  const words = (await createV2Genesis(a, true))!.join(" ");
  await a.sync();
  const seen = a.chain!.seq;
  const z = await hub.device("tok", new RecordingKeyStore());
  await z.recover(words);
  t.mock.timers.setTime(t0 + DAY + 60_000);
  await z.activateRecovered(words);
  assert.equal(hub.state!.device(z.deviceId)?.status.type, "active", "the hub took the activation");
  return { t0, hub, a, z, words, seen };
}

test("PoC A: a browser that missed a join and its activation is not locked out; the join is pending, vetoable, and taken a day after it first saw it", async (t) => {
  const { t0, hub, a, z, seen } = await recoveredWhileClosed(t);
  // a reopens (a day after the join, by every clock but its own).
  const back = await hub.reload(a);
  assert.equal(back.standing, "active", "the page opens");
  assert.equal(back.chain!.seq, seen + 1, "with the join in, and not its activation");
  assert.equal(back.chain!.deviceRecord(z.deviceId)?.status.type, "pending", "shown as pending (the banner's approve / remove)");
  assert.equal(back.heldBack?.device, z.deviceId);
  const firstSeen = back.chainClock.firstSeen.get(seen + 1)!;
  assert.equal(back.heldBack?.readyMs, firstSeen + DAY, "a day from this browser's own first sighting");
  // Reloads within that day keep the first sighting: they do not start it again.
  t.mock.timers.setTime(t0 + 2 * DAY);
  const later = await hub.reload(back);
  assert.equal(later.heldBack?.readyMs, firstSeen + DAY, "the first sighting was saved");
  // Once the day has passed here, the activation is taken, and the rest of the chain with it.
  t.mock.timers.setTime(firstSeen + DAY + 1);
  const done = await hub.reload(later);
  assert.equal(done.heldBack, null);
  assert.equal(done.chain!.seq, hub.chain.length - 1);
  assert.equal(done.chain!.device(z.deviceId)?.status.type, "active");
});

test("a held-back activation can be vetoed — the removal goes on the hub's chain, and the device is out everywhere", async (t) => {
  const { hub, a, z } = await recoveredWhileClosed(t);
  const back = await hub.reload(a);
  assert.equal(back.heldBack?.device, z.deviceId);
  // Anything but a veto or an approval waits.
  await assert.rejects(
    back.setRecoveryWords(),
    (e: unknown) => e instanceof AccountError && e.kind === "heldBack",
  );
  const report = await back.removeDevice(z.deviceId, new FakeCourier());
  assert.equal(back.heldBack, null);
  assert.equal(back.chain!.deviceRecord(z.deviceId)?.status.type, "removed");
  assert.equal(hub.state!.deviceRecord(z.deviceId)?.status.type, "removed", "on the hub too");
  assert.equal(back.chain!.seq, hub.state!.seq, "this browser follows the hub's chain again");
  assert.equal(report.ukEpoch, 1, "and the keys the recovered device opened are retired");
  assert.deepEqual(await back.vault.epochs(), [0, 1]);
  assert.equal(back.chain!.currentRecoveryKey, null, "the words it came back with stop counting");
  // A reload resumes from there, with nothing held.
  const again = await hub.reload(back);
  assert.equal(again.heldBack, null);
  assert.equal(again.chain!.seq, hub.state!.seq);
});

test("approving a held-back activation takes the hub's chain on the user's word", async (t) => {
  const { hub, a, z } = await recoveredWhileClosed(t);
  const back = await hub.reload(a);
  await back.approveDevice(z.deviceId);
  assert.equal(back.heldBack, null);
  assert.equal(back.chain!.device(z.deviceId)?.status.type, "active");
  assert.equal(back.chain!.seq, hub.state!.seq, "nothing was appended: the activation is the hub's");
  // And the account goes on as usual from there.
  await back.setRecoveryWords();
  assert.equal(hub.state!.seq, back.chain!.seq);
});

test("PoC C: a hub with the recovery words cannot have the owner's page confirm its removal and wipe it", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  const words = (await createV2Genesis(a, true))!.join(" ");
  await a.sync();
  // The thief joins; the (malicious) hub takes its activation at once — its own clock says a day has passed.
  const z = await hub.device("tok", new RecordingKeyStore());
  await z.recover(words);
  hub.chainNow = () => Date.now() + DAY + 60_000;
  await z.activateRecovered(words, Date.now() + DAY + 60_000);
  hub.chainNow = () => Date.now();
  await z.append({ type: "removeDevice", id: a.deviceId });
  // The owner signs in with the password: the hub refuses the device ("removed"), and the
  // page reads the chain with the login token to see. It is verified as every load is.
  const back: CloudAccount = await hub.device("tok", a.store);
  assert.equal(back.refusal, "removed", "the hub's word");
  assert.notEqual(back.standing, "removed", "not the chain's: the removal is behind an activation too early here");
  assert.equal(back.heldBack?.device, z.deviceId);
  assert.equal(back.chain!.deviceRecord(z.deviceId)?.status.type, "pending");
  assert.deepEqual(await back.vault.epochs(), [0], "nothing wiped: cloud-app wipes only a removal the chain shows");
});

test("a removal is confirmed only on a chain that keeps the head this device saw (a hub serving less is a fork, not a removal)", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await createV2Genesis(a, false);
  await a.sync();
  const seenChain = hub.chain.slice();
  // The hub serves less than a verified, and refuses its session.
  hub.chain = seenChain.slice(0, 1);
  hub.claimRemoved.add(a.deviceId);
  const back = await hub.device("tok", a.store);
  assert.equal(back.refusal, "removed");
  assert.equal(back.chain, null, "the served chain was not taken");
  const api = hub.api();
  const login = await api.login("someone@example.test", "a password");
  await assert.rejects(back.confirmRemoval(login.token), (e: unknown) => e instanceof AccountError && e.kind === "fork");
  assert.notEqual(back.standing, "removed");
  assert.deepEqual(await back.vault.epochs(), [0]);
});

// ---------------------------------------------------------------- the rotation's key mail and keyring names

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

/** A (genesis with words, machine m-1 with its key), B and D paired. */
async function threeDevices() {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  const m = await hub.enroll(a);
  await a.handOverMachineKey(m.id, new FakeCourier());
  const b = await pair(hub, a);
  const d = await pair(hub, a);
  await b.sync();
  await d.sync();
  return { hub, a, b, d, m };
}

test("PoC B: a device that missed the rotation's user key wraps nothing under the old one; it asks for the key and gets it", async () => {
  const { hub, a, b, d } = await threeDevices();
  const report = await a.removeDevice(d.deviceId, new FakeCourier());
  assert.equal(report.ukEpoch, 1);
  // The hub (with d) drops b's UK-1 mail.
  hub.mail = hub.mail.filter((x) => !(x.recipient === b.deviceId && x.purpose === MAIL_USER_KEY));
  await b.sync();
  await b.settle();
  assert.deepEqual(await b.vault.epochs(), [0]);
  assert.equal(b.missingUserKey, 1, "missing key, said");
  // It asked the account's other full device (a), and only it.
  const asked = hub.mailLog.filter((x) => x.sender === b.deviceId && x.purpose === MAIL_KEY_REQUEST);
  assert.deepEqual(asked.map((x) => [x.recipient, x.epoch]), [[a.deviceId, 1]]);
  // Enrolling a machine now refuses rather than wrapping its key under UK 0 (which d holds).
  await assert.rejects(hub.enroll(b, "m-2"), (e: unknown) => e instanceof AccountError && e.kind === "missingKey");
  assert.equal(hub.keysAt("mk/m-2/0/0").length, 0, "nothing under the old user key");
  // a takes the request and mails b every epoch it holds; b takes UK 1, and wraps under it.
  await a.receiveMail();
  await b.receiveMail();
  assert.deepEqual(await b.vault.epochs(), [0, 1]);
  assert.equal(await b.checkUserKey(), null);
  assert.equal(b.missingUserKey, null);
  const m2 = await hub.enroll(b, "m-3");
  assert.deepEqual(hub.keysAt(`mk/${m2.id}/0/1`), [`mk/${m2.id}/0/1/${b.deviceId}`], "wrapped under the newest user key");
  assert.equal(hub.keysAt(`mk/${m2.id}/0/0`).length, 0);
});

test("PoC B2: a rotation that died after its commit leaves no device wrapping under the retired key; rotating again mends it", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  const m = await hub.enroll(a);
  await a.handOverMachineKey(m.id, new FakeCourier());
  const d = await pair(hub, a);
  await d.sync();
  let once = true;
  hub.afterAppend = async () => {
    if (once) {
      once = false;
      throw new Error("hub: 502 after storing");
    }
  };
  await assert.rejects(a.removeDevice(d.deviceId, new FakeCourier()));
  hub.afterAppend = null;
  await a.sync();
  assert.equal(a.chain!.latestUserEpoch(), 1, "UK 1 committed…");
  assert.deepEqual(await a.vault.epochs(), [0], "…and held by nobody");
  assert.equal(await a.ensureUserKey(), null, "no wrapping under UK 0, which the removed device holds");
  assert.equal(a.missingUserKey, 1);
  await assert.rejects(hub.enroll(a, "m-2"), (e: unknown) => e instanceof AccountError && e.kind === "missingKey");
  // "Rotate the keys again": a new epoch this device holds.
  const report = await a.rotate(new FakeCourier());
  assert.equal(report.ukEpoch, 2);
  assert.equal(await a.ensureUserKey(), 2);
  const m2 = await hub.enroll(a, "m-3");
  assert.deepEqual(hub.keysAt(`mk/${m2.id}/0/2`), [`mk/${m2.id}/0/2/${a.deviceId}`]);
});

test("a device about to be removed cannot squat the next rotation's keyring names, and its blobs are swept to one per epoch with its removal", async () => {
  const { hub, a, d, m } = await threeDevices();
  const rsk = a.chain!.currentRecoveryKey!;
  // The squats the PoC made: the next epochs' names, under d's own name and under a's.
  for (const name of [`mk/${m.id}/1/1/${d.deviceId}`, `mk/${m.id}/1/1/${a.deviceId}`, `mk/${m.id}/1/1`, keyName.written(keyName.recovery(1, d.deviceId, rsk), d.deviceId)]) {
    await assert.rejects(d.api.putKey(d.session.token, name, new Uint8Array(60)), { status: 403 }, name);
  }
  // What d may write: its own names for committed epochs — padded with retries, say.
  const base = keyName.machine(m.id, 0, 0);
  const uk0 = await d.vault.key(0);
  const raw = await a.machineKeyBytes(m.id, 0);
  const blob = await wrapMachineKey(uk0, a.account, m.id, 0, 0, raw!);
  for (const sfx of ["", ".0001", ".0002"]) await d.api.putKey(d.session.token, `${base}/${d.deviceId}${sfx}`, blob);
  await d.ensureRecoveryPackages();
  assert.ok(hub.keys.has(keyName.written(keyName.recovery(0, d.deviceId, rsk), d.deviceId)), "d's recovery package");
  const report = await a.removeDevice(d.deviceId, new FakeCourier());
  // The rotation's names were all free: every machine got its new key in the keyring.
  assert.deepEqual(report.machines.map((x) => [x.machine, x.mkEpoch, x.error]), [[m.id, 1, undefined]]);
  assert.deepEqual(hub.keysAt(`mk/${m.id}/1/1`), [`mk/${m.id}/1/1/${a.deviceId}`]);
  // Of d's blobs one per epoch stays — its newest try of the wrap a
  // holds too (the hub cannot tell a good copy from a bad one: readers try
  // both against the chain), and its package — the retries go.
  assert.deepEqual(
    [...hub.keys.keys()].filter((n) => n.endsWith(d.deviceId) || n.includes(`${d.deviceId}.`)).sort(),
    [`${base}/${d.deviceId}.0002`, keyName.written(keyName.recovery(0, d.deviceId, rsk), d.deviceId)].sort(),
  );
  assert.ok(hub.keys.has(`${base}/${a.deviceId}`), "a's own wrap of it stays");
});

test("a device about to be removed cannot fill the others' mailboxes so the rotation's key mail bounces", async () => {
  const { hub, a, b, d } = await threeDevices();
  // d fills b's mailbox as far as it may: one sender's share, and only so much keyless mail.
  hub.mailRatePerMin = 10_000;
  let sent = 0;
  for (let i = 0; i < 300; i++) {
    try {
      await d.api.mail(d.session.token, { recipient: b.deviceId, sender: d.deviceId, purpose: i % 2 ? 9 : MAIL_USER_KEY, epoch: 0, sealed: new Uint8Array(8) });
      sent++;
    } catch (error) {
      assert.equal((error as { status?: number }).status, 507);
    }
  }
  assert.ok(sent <= 128, `one sender's share (${sent})`);
  assert.ok(hub.mail.filter((x) => x.recipient === b.deviceId && x.purpose === 9).length <= 32, "keyless mail is capped");
  // a removes d: d's mail is purged with it, and the new user key reaches b.
  const report = await a.removeDevice(d.deviceId, new FakeCourier());
  assert.deepEqual(report.failedTo, []);
  assert.deepEqual(report.sealedTo, [b.deviceId]);
  assert.equal(hub.mail.filter((x) => x.sender === d.deviceId).length, 0, "purged with the removal");
  // Mail of a purpose b does not handle is acknowledged, not left to fill the box.
  hub.mail.push({ id: 90_001, recipient: b.deviceId, sender: a.deviceId, purpose: 9, epoch: 0, sealed: "00" });
  await b.sync();
  assert.equal(await b.receiveMail(), 1, "UK 1");
  assert.equal(hub.mail.filter((x) => x.recipient === b.deviceId).length, 0, "and nothing left behind");
});

// ---------------------------------------------------------------- re-encryption by mailbox

test("a re-encryption asked for by mailbox is dropped only when the agent took it or is past it — not on any 409", async () => {
  const { a, m } = await threeDevices();
  const courier = new FakeCourier();
  courier.online = false;
  const issued = await a.issueMachineKey(m.id, courier, true);
  assert.equal(issued.result.via, "mailbox");
  assert.equal(pendingReencrypts(a.account).length, 1);
  const flush = (status: number, body: unknown) => flushPendingReencrypts(a.account, m.id, (item) => a.pinnedMachineKey(item), async () => ({ status, body }));
  // The agent's chain is behind (a hub can hold the commitment back to make it say so): kept.
  await flush(409, { error: "uncommitted", message: "the chain commits no such content key epoch (yet)", mkEpoch: 0 });
  assert.equal(pendingReencrypts(a.account).length, 1, "uncommitted: asked again later");
  await flush(409, { error: "key_mismatch", message: "not the key the chain committed", mkEpoch: 0 });
  assert.equal(pendingReencrypts(a.account).length, 1, "key_mismatch: asked again later");
  // It holds a newer epoch already: moot.
  await flush(409, { error: "stale_epoch", mkEpoch: issued.mkEpoch + 1 });
  assert.equal(pendingReencrypts(a.account).length, 0);
  // And a 200 settles it, as before.
  notePendingReencrypt(a.account, { machine: m.id, mkEpoch: issued.mkEpoch, ukEpoch: issued.ukEpoch, mkHash: "00".repeat(32) });
  assert.equal(reencryptSettled({ status: 200, body: {} }, issued.mkEpoch), true);
  assert.equal(reencryptSettled({ status: 409, body: { mkEpoch: issued.mkEpoch } }, issued.mkEpoch), false);
  assert.equal(reencryptSettled({ status: 403, body: null }, issued.mkEpoch), false);
  forgetPendingReencrypts(a.account);
});

// ---------------------------------------------------------------- the review of the fix

test("N1 (REVIEW): a veto over a held activation takes out what the thief added after it, and mails it nothing", async (t) => {
  const t0 = Date.now();
  t.mock.timers.enable({ apis: ["Date"], now: t0 });
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  const words = (await createV2Genesis(a, true))!.join(" ");
  await a.sync();
  const seen = a.chain!.seq;
  // Thief with the recovery words; the malicious hub takes its activation at once.
  const z = await hub.device("tok", new RecordingKeyStore());
  await z.recover(words);
  hub.chainNow = () => Date.now() + DAY + 60_000;
  await z.activateRecovered(words, Date.now() + DAY + 60_000);
  hub.chainNow = () => Date.now();
  // The (by the hub's chain) active thief adds a second device of its own.
  const z2 = await hub.device("tok", new RecordingKeyStore());
  await z.append({ type: "addDevice", device: (z2 as unknown as { info(): DeviceInfo }).info() });
  const back = await hub.reload(a);
  assert.equal(back.chain!.seq, seen + 1);
  assert.equal(back.heldBack?.device, z.deviceId);
  assert.deepEqual(
    back.heldChanges().map((c) => [c.kind, c.by, c.subject]),
    [
      ["activateRecovered", null, z.deviceId],
      ["addDevice", z.deviceId, z2.deviceId],
    ],
    "what approving would take in, for the page",
  );
  const before = hub.mailLog.length;
  const report = await back.removeDevice(z.deviceId, new FakeCourier());
  assert.deepEqual(report.vetoed, { devices: [z2.deviceId], machines: [] });
  assert.equal(back.chain!.deviceRecord(z.deviceId)?.status.type, "removed");
  assert.equal(back.chain!.deviceRecord(z2.deviceId)?.status.type, "removed", "z2 is out with it");
  assert.ok(!report.sealedTo.includes(z2.deviceId) && !report.sealedTo.includes(z.deviceId));
  const keyMail = hub.mailLog.slice(before).filter((m) => m.purpose === MAIL_USER_KEY);
  assert.deepEqual(keyMail.filter((m) => m.recipient === z2.deviceId || m.recipient === z.deviceId), [], "no user key to either");
  assert.equal(hub.state!.deviceRecord(z2.deviceId)?.status.type, "removed", "on the hub's chain too");
});

test("N1 (REVIEW2): while an activation is held, removing another device is refused — it would take in the thief's activation", async (t) => {
  const t0 = Date.now();
  t.mock.timers.enable({ apis: ["Date"], now: t0 });
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  const words = (await createV2Genesis(a, true))!.join(" ");
  await a.sync();
  const b = await hub.device("tok", new RecordingKeyStore());
  await a.append({ type: "addDevice", device: (b as unknown as { info(): DeviceInfo }).info() });
  const z = await hub.device("tok", new RecordingKeyStore());
  await z.recover(words);
  hub.chainNow = () => Date.now() + DAY + 60_000;
  await z.activateRecovered(words, Date.now() + DAY + 60_000);
  hub.chainNow = () => Date.now();
  const back = await hub.reload(a);
  assert.equal(back.heldBack?.device, z.deviceId);
  const before = hub.mailLog.length;
  await assert.rejects(back.removeDevice(b.deviceId, new FakeCourier()), (e: unknown) => e instanceof AccountError && e.kind === "heldBack");
  assert.equal(back.chain!.deviceRecord(z.deviceId)?.status.type, "pending", "the activation is still held back");
  assert.equal(back.chain!.deviceRecord(b.deviceId)?.status.type, "active");
  assert.equal(hub.mailLog.length, before, "nothing mailed");
  assert.equal(back.heldBack?.device, z.deviceId);
});

test("N1: a veto takes out what the thief brought in, but stands what a device this view trusted signed meanwhile", async (t) => {
  const t0 = Date.now();
  t.mock.timers.enable({ apis: ["Date"], now: t0 });
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  const words = (await createV2Genesis(a, true))!.join(" ");
  await a.sync();
  const b = await hub.device("tok", new RecordingKeyStore());
  await a.append({ type: "addDevice", device: (b as unknown as { info(): DeviceInfo }).info() });
  await b.sync();
  const z = await hub.device("tok", new RecordingKeyStore());
  await z.recover(words);
  // b was open all along: it saw the join then, and takes the activation a day later.
  await b.sync();
  t.mock.timers.setTime(t0 + DAY + 60_000);
  await z.activateRecovered(words);
  // Meanwhile b (which a trusts) adds c; the thief adds z2.
  const c = await hub.device("tok", new RecordingKeyStore());
  await b.sync();
  assert.equal(b.heldBack, null);
  await b.append({ type: "addDevice", device: (c as unknown as { info(): DeviceInfo }).info() });
  const z2 = await hub.device("tok", new RecordingKeyStore());
  await z.sync();
  await z.append({ type: "addDevice", device: (z2 as unknown as { info(): DeviceInfo }).info() });
  const back = await hub.reload(a);
  const scope = back.vetoScope(back.heldBack!, await ChainState.verify([...back.chain!.allEntries(), ...back.heldBack!.entries], undefined, ChainClock.history()));
  assert.deepEqual(scope.devices, [z.deviceId, z2.deviceId]);
  const report = await back.removeDevice(z.deviceId, new FakeCourier());
  assert.equal(back.chain!.device(c.deviceId)?.status.type, "active", "b's addition stands");
  assert.ok(report.sealedTo.includes(c.deviceId) && report.sealedTo.includes(b.deviceId));
  assert.equal(back.chain!.deviceRecord(z2.deviceId)?.status.type, "removed");
});

test("N2: a rotation whose keyring write is refused (full) still hands the new machine key over, and says so loudly", async () => {
  const { hub, a, d, m } = await threeDevices();
  hub.keyringFull = true;
  const courier = new FakeCourier();
  const report = await a.removeDevice(d.deviceId, courier);
  hub.keyringFull = false;
  const machine = report.machines.find((x) => x.machine === m.id)!;
  assert.equal(machine.keyring, "keyring_full", "the report says the keyring did not take it");
  assert.deepEqual(machine.result, { via: "channel", changed: true }, "and the agent got the new key all the same");
  assert.deepEqual(courier.posted.map((p) => [p.machine, p.epoch]), [[m.id, 1]], "it stops sealing under the key the removed device holds");
});

test("N2: key epochs committed on their own are an alert on the other devices; a removal's rotation is said by the removal", async () => {
  const { a, b, d, m } = await threeDevices();
  const courier = new FakeCourier();
  courier.online = false;
  await d.issueMachineKey(m.id, courier);
  await a.sync();
  assert.deepEqual(
    a.alerts().map((x) => [x.kind, x.by]),
    [["keyEpochs", d.deviceId]],
  );
  for (const x of a.alerts()) a.acknowledge(x.seq);
  await a.removeDevice(d.deviceId, new FakeCourier());
  await b.sync();
  assert.deepEqual(
    b.alerts().filter((x) => x.seq > a.chain!.seq - 2).map((x) => x.kind),
    ["removeDevice"],
    "the rotation's key epochs ride on the removal's alert",
  );
});

test("N5: a key request is answered with the asked-for epoch first, then the rest newest first", async () => {
  const { hub, a, b, d } = await threeDevices();
  await a.rotate(new FakeCourier());
  await a.rotate(new FakeCourier());
  await a.removeDevice(d.deviceId, new FakeCourier());
  // b lost its mail for epochs 1..3; it asks for the newest.
  hub.mail = hub.mail.filter((x) => x.recipient !== b.deviceId);
  await b.sync();
  await b.checkUserKey();
  const before = hub.mailLog.length;
  await a.receiveMail();
  const sent = hub.mailLog.slice(before).filter((x) => x.recipient === b.deviceId && x.purpose === MAIL_USER_KEY).map((x) => x.epoch);
  assert.deepEqual(sent, [3, 2, 1, 0]);
  assert.deepEqual(requestedFirst([{ epoch: 0 }, { epoch: 1 }, { epoch: 2 }], new Set([1])).map((k) => k.epoch), [1, 2, 0]);
});
