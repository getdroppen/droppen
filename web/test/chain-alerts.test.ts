// The device-change alerts (`CloudAccount.alerts`, `acknowledge`): a change
// another device made to who is in the account is told to the person until
// they say it was them, so an alert that went missing would hide a hostile
// device. Every kind is pinned here appearing and being acknowledged (a
// renewed identity key in host-rotation-renewal.test.ts, which can renew one),
// and so is where alerts start: the acknowledgement written when a device pins
// the account.
import assert from "node:assert/strict";
import test from "node:test";
import { PASSKEY_DELAY_MS, PAUSE_CANCELLED, PAUSE_COMPLETED, PAUSE_START, RECOVERY_DELAY_MS } from "../src/lib/e2e/index.js";
import type { CloudAccount } from "../src/lib/hub/account.js";
import { SeenStoreError } from "../src/lib/hub/seen-store.js";
import { addPasskeySlot, browserSeen, browserStorage, createV2Genesis, FakeCourier, FakeKeysHub, pairBrowser, passkeyJoin, postAsPasskey, RecordingKeyStore } from "./fake-keys-hub.js";

/** An account `a` made on chain v2 (with recovery words and host m-1), as the hub takes v3 entries. */
async function v2Account() {
  const hub = new FakeKeysHub();
  hub.chainV3Mode = "write";
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  await hub.enroll(a, "m-1");
  return { hub, a };
}

/** What `x` keeps as acknowledged, as stored. */
const ackOf = (x: CloudAccount) => {
  const raw = browserStorage(x.store).getItem(`rh.hub.ack.${x.account}.${x.deviceId}`);
  return raw === null ? null : (JSON.parse(raw) as { through: number; extra: number[] });
};

/** Acknowledge `x`'s alerts newest first: each goes alone, the older ones stay as they were. */
function acknowledgeNewestFirst(x: CloudAccount): void {
  const all = x.alerts();
  let told = 0;
  const stop = x.onChange(() => void told++);
  for (let i = all.length - 1; i >= 0; i--) {
    x.acknowledge(all[i]!.seq);
    assert.deepEqual(
      x.alerts().map((a) => a.seq),
      all.slice(0, i).map((a) => a.seq),
      `acknowledging the ${all[i]!.kind} at ${all[i]!.seq} takes it alone`,
    );
    assert.equal(told, all.length - i, "and the page is told, once each time");
  }
  stop();
}

test("alerts: every kind another admin's change raises — from the upgrade to chain v3 on — appears in order and is acknowledged on its own", async () => {
  const { hub, a } = await v2Account();
  const c = await pairBrowser(hub, a, { admin: true });
  await c.receiveMail();
  assert.deepEqual(ackOf(c), { through: c.chain!.seq, extra: [] }, "pinning the account wrote where its alerts start");
  assert.deepEqual(c.alerts(), [], "what came before is history");
  await a.upgradeToV3({ writable: true });
  const b = await pairBrowser(hub, a, { admin: false });
  const d = await pairBrowser(hub, a, { admin: false });
  await a.setAdmin(b.deviceId, true);
  await hub.enroll(a, "m-2", { writable: true });
  await a.setHost("m-2", false, new FakeCourier(), { writable: true });
  await a.setAccept("m-1", { type: "devices", ids: [d.deviceId] });
  await a.setRecoveryWords();
  const offline = new FakeCourier();
  offline.online = false;
  await a.issueMachineKey("m-1", offline);
  await a.setRenewalPolicy(true);
  const op = new Uint8Array(16).fill(7);
  await a.append({ type: "renewalPause", op, phase: PAUSE_START, removals: [{ host: "m-1", ids: [d.deviceId] }] });
  await a.append({ type: "renewalPause", op, phase: PAUSE_CANCELLED, removals: [] });
  await a.removeDevice(b.deviceId, new FakeCourier());
  await c.sync();
  const seen = c.alerts().map((x) => ({ kind: x.kind, by: x.by, subject: x.subject, remove: x.remove, flags: x.flags, on: x.on, accept: x.accept, pause: x.pause }));
  const none = { flags: undefined, on: undefined, accept: undefined, pause: undefined };
  const by = a.deviceId;
  const paused = [{ host: "m-1", ids: [d.deviceId] }];
  assert.deepEqual(seen, [
    { ...none, kind: "upgrade", by, subject: null, remove: by },
    { ...none, kind: "addDevice", by, subject: b.deviceId, remove: null, flags: { host: false, admin: false } },
    { ...none, kind: "addDevice", by, subject: d.deviceId, remove: d.deviceId, flags: { host: false, admin: false } },
    { ...none, kind: "setAdmin", by, subject: b.deviceId, remove: null, on: true },
    { ...none, kind: "addDevice", by, subject: "m-2", remove: "m-2", flags: { host: true, admin: false } },
    { ...none, kind: "setHost", by, subject: "m-2", remove: null, on: false },
    { ...none, kind: "setAccept", by, subject: "m-1", remove: null, accept: { type: "devices", ids: [d.deviceId] } },
    { ...none, kind: "setRecoveryKey", by, subject: null, remove: by },
    { ...none, kind: "keyEpochs", by, subject: null, remove: by },
    { ...none, kind: "renewalPolicy", by, subject: null, remove: null, on: true },
    { ...none, kind: "renewalPause", by, subject: null, remove: null, pause: { phase: "start", removals: paused } },
    { ...none, kind: "renewalPause", by, subject: null, remove: null, pause: { phase: "cancelled", removals: paused } },
    { ...none, kind: "removeDevice", by, subject: b.deviceId, remove: null },
  ]);
  assert.ok(c.alerts().every((x) => x.atMs > 0));
  // Out of order: the newest one is kept past what is still open.
  const all = c.alerts();
  c.acknowledge(all.at(-1)!.seq);
  assert.deepEqual(ackOf(c), { through: all[0]!.seq - 1, extra: [all.at(-1)!.seq] });
  acknowledgeNewestFirst(c);
  assert.deepEqual(ackOf(c), { through: c.chain!.seq, extra: [] }, "all of them: through the head, nothing extra");
  assert.deepEqual((await hub.reload(c)).alerts(), [], "kept for this device across a reload");
});

test("alerts: a passkey slot, a device's join through it, that device's activation and the slot's removal — acknowledged oldest first", async (t) => {
  t.mock.timers.enable({ apis: ["Date"], now: Date.now() });
  const { hub, a } = await v2Account();
  await a.upgradeToV3({ writable: true });
  const c = await pairBrowser(hub, a, { admin: true });
  await c.receiveMail();
  const slot = await addPasskeySlot(a);
  const p = await hub.device("tok", new RecordingKeyStore());
  await passkeyJoin(p, slot);
  // a and c see the join a day before the activation, so they take the activation as it comes.
  await a.sync();
  await c.sync();
  t.mock.timers.tick(PASSKEY_DELAY_MS + 60_000);
  await postAsPasskey(p, slot, { type: "passkeyActivate", id: p.deviceId });
  await a.disablePasskeyJoin(slot.slot);
  await c.sync();
  assert.equal(c.heldBack, null);
  assert.deepEqual(
    c.alerts().map((x) => [x.kind, x.passkey, x.by, x.subject, x.remove]),
    [
      ["passkey", "slotSet", a.deviceId, slot.slot, a.deviceId],
      ["passkey", "join", slot.slot, p.deviceId, p.deviceId],
      ["passkey", "activate", slot.slot, p.deviceId, p.deviceId],
      ["passkey", "slotRemove", a.deviceId, slot.slot, null],
    ],
  );
  const all = c.alerts();
  for (let i = 0; i < all.length; i++) {
    c.acknowledge(all[i]!.seq);
    assert.deepEqual(
      c.alerts().map((x) => x.seq),
      all.slice(i + 1).map((x) => x.seq),
      `acknowledging the ${all[i]!.passkey} takes it alone`,
    );
    // In order: where alerts start moves up, and nothing is kept past it.
    assert.deepEqual(ackOf(c), { through: i + 1 < all.length ? all[i + 1]!.seq - 1 : c.chain!.seq, extra: [] });
  }
});

test("alerts: a recovery join waits in its own banner — no alert, and nothing acknowledged past it — and is one once it activated", async (t) => {
  const t0 = Date.now();
  t.mock.timers.enable({ apis: ["Date"], now: t0 });
  const { hub, a } = await v2Account();
  await a.upgradeToV3({ writable: true });
  const c = await pairBrowser(hub, a, { admin: true });
  await c.receiveMail();
  const words = (await a.setRecoveryWords()).join(" ");
  const z = await hub.device("tok", new RecordingKeyStore());
  await z.recover(words);
  await a.sync();
  await c.sync();
  const joinSeq = c.chain!.allEntries().find((e) => e.draft.action.type === "recoveryJoin3")!.draft.seq;
  assert.deepEqual(c.alerts().map((x) => x.kind), ["setRecoveryKey"], "the waiting join is the pending banner's");
  c.acknowledge(c.alerts()[0]!.seq);
  assert.deepEqual(c.alerts(), []);
  assert.deepEqual(ackOf(c), { through: joinSeq - 1, extra: [] }, "acknowledged up to the waiting join, not past it");
  t.mock.timers.setTime(t0 + RECOVERY_DELAY_MS + 60_000);
  await z.activateRecovered(words);
  await c.sync();
  assert.deepEqual(
    c.alerts().map((x) => [x.seq, x.kind, x.by, x.subject, x.remove, x.flags]),
    [[joinSeq, "recoveryJoin", null, z.deviceId, z.deviceId, { host: false, admin: true }]],
    "activated: the join is an alert, by the recovery words",
  );
  c.acknowledge(joinSeq);
  assert.deepEqual(c.alerts(), []);
  assert.deepEqual(ackOf(c), { through: c.chain!.seq, extra: [] });
});

test("alerts start where a device pinned the account — unless it already kept where they start, which pinning leaves as it is", async (t) => {
  const t0 = Date.now();
  t.mock.timers.enable({ apis: ["Date"], now: t0 });
  const { hub, a } = await v2Account();
  await a.upgradeToV3({ writable: true });
  const c = await pairBrowser(hub, a, { admin: false });
  await a.removeDevice(c.deviceId, new FakeCourier());
  const words = (await a.setRecoveryWords()).join(" ");
  // y and z come back with the words; z already keeps where its alerts start (as a device that ran here before would).
  const y = await hub.device("tok", new RecordingKeyStore());
  const z = await hub.device("tok", new RecordingKeyStore());
  const zKey = `rh.hub.ack.${z.account}.${z.deviceId}`;
  browserStorage(z.store).setItem(zKey, JSON.stringify({ through: 0, extra: [] }));
  await y.recover(words);
  await z.recover(words);
  const yJoin = y.chain!.allEntries().find((e) => e.draft.action.type === "recoveryJoin3" && e.draft.action.device.id === y.deviceId)!.draft.seq;
  assert.deepEqual(ackOf(y), { through: yJoin - 1, extra: [] }, "pinned on the words: alerts start at the chain it trusted");
  assert.deepEqual(ackOf(z), { through: 0, extra: [] }, "kept as it was");
  assert.equal(z.standing, "pending");
  assert.deepEqual(z.alerts(), [], "a device still waiting to be let in is told of nothing yet");
  await a.sync();
  t.mock.timers.setTime(t0 + RECOVERY_DELAY_MS + 60_000);
  await y.activateRecovered(words);
  await z.activateRecovered(words);
  await y.sync();
  await z.sync();
  assert.deepEqual(y.alerts(), [], "what came before y trusted the account is history to it");
  assert.deepEqual(
    z.alerts().map((x) => [x.kind, x.subject]),
    [
      ["keyEpochs", null],
      ["upgrade", null],
      ["addDevice", c.deviceId],
      ["removeDevice", c.deviceId],
      ["setRecoveryKey", null],
      ["recoveryJoin", y.deviceId],
    ],
    "z's alerts start where it kept them, not where it pinned",
  );});

test("alerts start where the founding device pinned its genesis, also when the seen store would not take the pin", async (t) => {
  const warned = t.mock.method(console, "warn", () => undefined);
  const hub = new FakeKeysHub();
  hub.chainV3Mode = "write";
  const store = new RecordingKeyStore();
  const a = await hub.device("tok", store);
  const seen = browserSeen(store);
  const update = seen.update;
  seen.update = async () => {
    throw new SeenStoreError("the store is full");
  };
  await a.createGenesis(true, { writable: true });
  seen.update = update;
  assert.equal(warned.mock.callCount(), 1, "the pin went where older pages keep it");
  assert.equal(a.pinned, true);
  assert.deepEqual(ackOf(a), { through: a.chain!.seq, extra: [] }, "and alerts start from the genesis it made");
  const c = await pairBrowser(hub, a, { admin: true });
  await a.sync();
  assert.deepEqual(a.alerts(), [], "its own pairing is no alert");
  await c.setRecoveryWords();
  await a.sync();
  assert.deepEqual(a.alerts().map((x) => [x.kind, x.by]), [["setRecoveryKey", c.deviceId]]);
});

test("alerts: the founding device's fallback pin leaves where its alerts start as it was, when it kept that already", async (t) => {
  t.mock.method(console, "warn", () => undefined);
  const hub = new FakeKeysHub();
  hub.chainV3Mode = "write";
  const store = new RecordingKeyStore();
  const a = await hub.device("tok", store);
  browserStorage(store).setItem(`rh.hub.ack.${a.account}.${a.deviceId}`, JSON.stringify({ through: 0, extra: [] }));
  const seen = browserSeen(store);
  const update = seen.update;
  seen.update = async () => {
    throw new SeenStoreError("the store is full");
  };
  await a.createGenesis(true, { writable: true });
  seen.update = update;
  assert.equal(a.pinned, true, "pinned where older pages keep it");
  assert.ok(a.chain!.seq > 0);
  assert.deepEqual(ackOf(a), { through: 0, extra: [] }, "kept as it was, not moved to the genesis it made");
});

test("alerts: a waiting recovery join acknowledged by its seq keeps where alerts start below it", async () => {
  const { hub, a } = await v2Account();
  await a.upgradeToV3({ writable: true });
  const c = await pairBrowser(hub, a, { admin: true });
  await c.receiveMail();
  const words = (await a.setRecoveryWords()).join(" ");
  const z = await hub.device("tok", new RecordingKeyStore());
  await z.recover(words);
  await c.sync();
  const joinSeq = c.chain!.allEntries().find((e) => e.draft.action.type === "recoveryJoin3")!.draft.seq;
  c.acknowledge(c.alerts()[0]!.seq);
  assert.deepEqual(ackOf(c), { through: joinSeq - 1, extra: [] });
  c.acknowledge(joinSeq);
  assert.deepEqual(ackOf(c), { through: joinSeq - 1, extra: [joinSeq] }, "the join is still open: where alerts start does not pass it");
});

test("alerts: a device whose browser no longer trusts the account's genesis is told of nothing", async () => {
  const { hub, a } = await v2Account();
  const c = await pairBrowser(hub, a, { admin: true });
  await a.setRecoveryWords();
  await c.sync();
  assert.deepEqual(c.alerts().map((x) => x.kind), ["setRecoveryKey"]);
  browserSeen(c.store).values.clear();
  for (const key of [`rh.hub.genesis.${c.account}`, `rh.hub.seen.${c.account}`]) browserStorage(c.store).removeItem(key);
  const again = await hub.reload(c);
  assert.equal(again.pinned, false);
  assert.equal(again.standing, "active");
  assert.ok(ackOf(again)!.through < again.chain!.seq, "its acknowledgements are still kept");
  assert.deepEqual(again.alerts(), [], "an unpinned view is told of no change");
});

test("alerts: setAdmin offers to remove a device made full, not one taken back; a completed pause names only control still let in; each alert dates from when this device first saw it", async (t) => {
  const t0 = Date.now();
  t.mock.timers.enable({ apis: ["Date"], now: t0 });
  const { hub, a } = await v2Account();
  await a.upgradeToV3({ writable: true });
  const c = await pairBrowser(hub, a, { admin: true });
  await c.receiveMail();
  await hub.enroll(a, "m-2", { writable: true });
  const b = await pairBrowser(hub, a, { admin: false });
  const [d, e] = [(await pairBrowser(hub, a, { admin: false })).deviceId, (await pairBrowser(hub, a, { admin: false })).deviceId].sort();
  await a.setAdmin(b.deviceId, true);
  await a.setAdmin(b.deviceId, false);
  await a.setAccept("m-1", { type: "devices", ids: [d!, e!] });
  await a.setAccept("m-2", { type: "devices", ids: [d!] });
  const op = new Uint8Array(16).fill(9);
  await a.append({ type: "renewalPause", op, phase: PAUSE_START, removals: [{ host: "m-1", ids: [d!, e!] }, { host: "m-2", ids: [d!] }] });
  // By hand during the pause: d is let into m-1 and m-2 no more, so the completion gives back e's control of m-1 alone.
  await a.setAccept("m-1", { type: "devices", ids: [e!] }, new FakeCourier());
  await a.setAccept("m-2", { type: "admins" }, new FakeCourier());
  await a.append({ type: "renewalPause", op, phase: PAUSE_COMPLETED, removals: [] });
  // c sees all of it five seconds after it was written.
  t.mock.timers.setTime(t0 + 5_000);
  await c.sync();
  const by = a.deviceId;
  assert.deepEqual(
    c.alerts().map((x) => [x.kind, x.by, x.subject, x.on, x.remove, x.pause]),
    [
      ["addDevice", by, "m-2", undefined, "m-2", undefined],
      ["addDevice", by, b.deviceId, undefined, b.deviceId, undefined],
      ["addDevice", by, d, undefined, d, undefined],
      ["addDevice", by, e, undefined, e, undefined],
      ["setAdmin", by, b.deviceId, true, b.deviceId, undefined],
      ["setAdmin", by, b.deviceId, false, null, undefined],
      ["setAccept", by, "m-1", undefined, null, undefined],
      ["setAccept", by, "m-2", undefined, null, undefined],
      ["renewalPause", by, null, undefined, null, { phase: "start", removals: [{ host: "m-1", ids: [d, e] }, { host: "m-2", ids: [d] }] }],
      ["setAccept", by, "m-1", undefined, null, undefined],
      ["keyEpochs", by, null, undefined, by, undefined],
      ["setAccept", by, "m-2", undefined, null, undefined],
      ["keyEpochs", by, null, undefined, by, undefined],
      ["renewalPause", by, null, undefined, null, { phase: "completed", removals: [{ host: "m-1", ids: [e] }] }],
    ],
  );
  for (const x of c.alerts()) {
    assert.equal(c.chain!.entryAt(x.seq)!.draft.createdMs, t0, "written at t0");
    assert.equal(x.atMs, t0 + 5_000, `the ${x.kind} at ${x.seq} dates from when c first saw it`);
  }
  acknowledgeNewestFirst(c);
  assert.deepEqual(ackOf(c), { through: c.chain!.seq, extra: [] });
});
