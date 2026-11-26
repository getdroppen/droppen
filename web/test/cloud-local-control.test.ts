// S1-4 (docs/local-aggregator.md §8): the local install controls the other
// devices with its one identity — a host that is not an admin (review D1) —
// so an admin seals it the content keys of the hosts that accept its control
// (purpose 5, review D2) once its device role signed in at the hub, answers
// its key requests (purpose 4) with those and never a user key, hands them
// over as part of its registration (the /link popup's `addHost`, and the
// enrollment code's flow), and on its removal turns every other host's key
// over once. The consent page's 「可管理账号」 switch is off and held off for
// a device that runs AI.
//
// What these pin, against the fake hub (which, like rh-hub, takes mail for a
// host's device role only once it signed in as a device: a 400 before):
// - a host is a host-key recipient like any device that is not an admin, and
//   a 400 for one that has no device role yet is "waiting", not a failure;
// - its request gets the keys of the hosts that accept it, only those;
// - `handHostKeys` waits for its device role, gives up quietly after;
// - `addHost` writes `[addDevice3{host, admin: false}, keyEpochs]`, refuses
//   `admin: true` before any code is issued, and hands the keys over;
// - removing it rotates every other host's content key, not the user key, and
//   it is sealed none of the new keys.
import assert from "node:assert/strict";
import test from "node:test";
import { decodeHostKeyMail, fromHex, generateDeviceKeys, hex, type NoiseKeyPair, enrollProof, openHostKey, type EnrollCode } from "../src/lib/e2e/index.js";
import { AccountError, type CloudAccount, KEY_REQUEST_EVERY_MS, noDeviceRoleYet } from "../src/lib/hub/account.js";
import { HubApiError, MAIL_HOST_KEY, MAIL_KEY_REQUEST, MAIL_USER_KEY } from "../src/lib/hub/api.js";
import { addHost } from "../src/features/link/add-host.js";
import { createV2Genesis, FakeCourier, FakeKeysHub } from "./fake-keys-hub.js";

/** An admin browser on a v3 account with hosts m-1 and m-2 enrolled. */
async function account() {
  const hub = new FakeKeysHub();
  hub.chainV3Mode = "write";
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  await hub.enroll(a, "m-1");
  await a.upgradeToV3({ writable: true });
  const m2 = await hub.enroll(a, "m-2", { writable: true });
  return { hub, a, m2 };
}

/** Let `ids` control host `host`. */
async function admit(a: CloudAccount, host: string, ...ids: string[]): Promise<void> {
  const now = a.chain!.acceptPolicy(host);
  const before = now.type === "devices" ? now.ids : [];
  await a.setAccept(host, { type: "devices", ids: [...before, ...ids] });
}

/**
 * Sign `id`'s device role in `afterMs` from now, or as soon as the chain lists
 * it after that: under a loaded test run the batch that adds it may land later
 * than the timer (a fixed delay made these tests flaky).
 */
const signInSoon = (hub: FakeKeysHub, id: string, afterMs: number) => {
  const attempt = () => {
    try {
      hub.signInHost(id);
    } catch {
      setTimeout(attempt, 5);
    }
  };
  setTimeout(attempt, afterMs);
};
const hostKeysTo = (hub: FakeKeysHub, id: string) => hub.mailLog.filter((m) => m.recipient === id && m.purpose === MAIL_HOST_KEY);

/** Open every host key mailed to host `me` (its DH key), as its device role would: `host/epoch` → key hex. */
async function opened(hub: FakeKeysHub, a: CloudAccount, me: string, dh: NoiseKeyPair): Promise<Map<string, string>> {
  const out = new Map<string, string>();
  for (const m of hostKeysTo(hub, me)) {
    const { host, sealed } = decodeHostKeyMail(fromHex(m.sealed));
    const key = await openHostKey(dh, me, a.chain!, { account: a.account, sender: m.sender, recipient: me, host, epoch: m.epoch }, sealed);
    out.set(`${host}/${m.epoch}`, hex(key));
  }
  return out;
}

test("S1-4: a host that is not an admin gets the content keys of the hosts that accept it — once its device role signed in; before, that is no failure", async () => {
  const { hub, a, m2 } = await account();
  assert.equal(a.chain!.device("m-2")!.host, true);
  assert.equal(a.chain!.device("m-2")!.admin, false, "D1");
  // m-1 lets m-2 in: m-2 is a recipient of m-1's keys now — but its device role never signed in.
  await admit(a, "m-1", "m-2");
  assert.deepEqual(
    a.hostKeyRecipients("m-1").map((d) => d.info.id),
    ["m-2"],
  );
  assert.deepEqual(a.hostKeyRecipients("m-2"), [], "m-2 accepts admins only");
  assert.equal(hostKeysTo(hub, "m-2").length, 0, "the hub took none: m-2 is no device there yet");
  const mk = new Uint8Array(32).fill(7);
  const before = await a.mailHostKey("m-1", 0, mk);
  assert.deepEqual(before, { sent: [], failed: {}, waiting: ["m-2"] }, "waiting, not failed");
  // Its device role signs in: re-sent, every committed epoch of m-1 opens as the chain committed it.
  hub.signInHost("m-2");
  assert.ok((await a.resendKeys("m-2")) >= 1);
  const keys = await opened(hub, a, "m-2", m2.dh);
  const epochs = a.chain!.machineKeyEpochs("m-1");
  assert.equal(keys.size, epochs);
  for (let e = 0; e < epochs; e++) assert.equal(keys.get(`m-1/${e}`), hex((await a.machineKeyBytes("m-1", e))!), `m-1 epoch ${e}`);
  assert.ok(![...keys.keys()].some((k) => k.startsWith("m-2/")), "never its own");
  // A rotation seals m-1's new key to it, and nobody is reported as failed.
  const report = await a.rotate(new FakeCourier(), false, [], { userKey: false });
  assert.deepEqual(report.hostKeysTo, ["m-2"]);
  assert.deepEqual(report.hostKeysFailed, {});
  assert.equal((await opened(hub, a, "m-2", m2.dh)).get(`m-1/${epochs}`), hex((await a.machineKeyBytes("m-1", epochs))!));
  assert.ok(!hub.mailLog.some((m) => m.recipient === "m-2" && m.purpose === MAIL_USER_KEY), "never a user key");
});

test("S1-4: a host's key request is answered with the keys of the hosts that accept it — never a user key, never another host's", async () => {
  const { hub, a, m2 } = await account();
  const m3 = await hub.enroll(a, "m-3", { writable: true });
  await admit(a, "m-1", "m-2");
  const token = hub.signInHost("m-2");
  hub.mailLog.length = 0;
  // m-2 asks the admin (purpose 4: no secret).
  await a.api.mail(token, { recipient: a.deviceId, sender: "m-2", purpose: MAIL_KEY_REQUEST, epoch: 0, sealed: new Uint8Array(0) });
  await a.receiveMail();
  const keys = await opened(hub, a, "m-2", m2.dh);
  assert.ok(keys.size >= 1);
  assert.ok([...keys.keys()].every((k) => k.startsWith("m-1/")), `only m-1's: ${[...keys.keys()]}`);
  assert.ok(!hub.mailLog.some((m) => m.recipient === "m-2" && m.purpose === MAIL_USER_KEY));
  // A host that asks others than the admins is refused at the hub.
  await assert.rejects(
    a.api.mail(token, { recipient: "m-3", sender: "m-2", purpose: MAIL_KEY_REQUEST, epoch: 0, sealed: new Uint8Array(0) }),
    (e) => e instanceof HubApiError && e.code === "not_an_admin",
  );
  assert.equal((await opened(hub, a, "m-3", m3.dh)).size, 0);
});

test("S1-4: registration hands the new host the others' keys once its device role shows up; it waits a bounded time and gives up quietly", async () => {
  const { hub, a } = await account();
  // Nobody accepts m-2: nothing to hand, at once.
  assert.equal(await a.handHostKeys("m-2", { forMs: 1_000, everyMs: 10 }), 0);
  await admit(a, "m-1", "m-2");
  // Its device role never signs in: `null` after the wait, nothing thrown.
  const t0 = Date.now();
  assert.equal(await a.handHostKeys("m-2", { forMs: 60, everyMs: 20 }), null);
  assert.ok(Date.now() - t0 < 1_000);
  // It signs in a moment later: handed.
  signInSoon(hub, "m-2", 40);
  const sent = await a.handHostKeys("m-2", { forMs: 2_000, everyMs: 10 });
  assert.equal(sent, a.chain!.machineKeyEpochs("m-1"));
  // A refusal other than "no device role yet" is not swallowed: only the hub's own words for it (N2).
  const notYet = "this mail goes only to a device of this account (a computer gets it once it signs in as a device)";
  assert.equal(noDeviceRoleYet({ host: true }, new HubApiError(400, "bad_request", notYet)), true);
  assert.equal(noDeviceRoleYet({ host: true }, new HubApiError(400, "bad_request", "sealed is ≤ 8 KiB of hex")), false, "another 400");
  assert.equal(noDeviceRoleYet({ host: false }, new HubApiError(400, "bad_request", notYet)), false);
  assert.equal(noDeviceRoleYet({ host: true }, new HubApiError(403, "not_accepted", notYet)), false);
});

/** The local install's agent, as the popup sees it: keys shown up front, the proof once the code arrives. */
async function localInstall(hub: FakeKeysHub, a: CloudAccount, id: string) {
  const keys = await generateDeviceKeys();
  hub.machines.set(id, { sign: hex(keys.signPublic), dh: hex(keys.dhPublic) });
  const prove = async (code: EnrollCode) => {
    const token = [...hub.enrollments.keys()].at(-1)!;
    const proof = await enrollProof(code, a.account, id, keys.signPublic, keys.dhPublic);
    hub.enrollments.set(token, { device: a.deviceId, machine: id, sign: keys.signPublic, dh: keys.dhPublic, proof });
  };
  return { keys, prove, dh: { privateKey: keys.dh.privateKey, publicKey: keys.dhPublic } as NoiseKeyPair };
}

test("S1-4: the popup adds the local install as a host that is never a full device, hands it its key, then the others' — asked to make it one, refused (D1)", async () => {
  const { hub, a } = await account();
  await a.setAccept("m-1", { type: "all" });
  await a.setAccept("m-2", { type: "all" });
  // Asked to make it a full device anyway: refused before any code is issued.
  const local = await localInstall(hub, a, "m-local");
  const enrollmentsBefore = hub.enrollments.size;
  const courier = new FakeCourier();
  await assert.rejects(
    addHost(a, { keys: { signKey: local.keys.signPublic, dhKey: local.keys.dhPublic }, admin: true, writable: true, courier, sendCode: () => assert.fail("no code"), pollMs: 1 }),
    (e) => e instanceof AccountError && e.kind === "invalidEntry",
  );
  assert.equal(hub.enrollments.size, enrollmentsBefore, "no code was issued, nothing was added");
  assert.equal(a.chain!.deviceRecord("m-local"), undefined);
  // The real thing: its device role signs in as soon as the chain lists it.
  const seq = a.chain!.seq;
  const steps: string[] = [];
  const added = await addHost(a, {
    keys: { signKey: local.keys.signPublic, dhKey: local.keys.dhPublic },
    admin: false,
    writable: true,
    courier,
    sendCode: (code) => {
      void local.prove(code);
      signInSoon(hub, "m-local", 30);
    },
    onStep: (s) => steps.push(s),
    pollMs: 1,
    hostKeysWait: { forMs: 2_000, everyMs: 10 },
  });
  assert.deepEqual(steps, ["code", "proof", "key", "hostKeys"]);
  assert.equal(added.machine, "m-local");
  const batch = a.chain!.allEntries().slice(seq + 1).map((e) => e.draft.action);
  assert.equal(batch[0]!.type, "addDevice3");
  assert.deepEqual(batch[0]!.type === "addDevice3" && batch[0]!.device.flags, { host: true, admin: false });
  assert.equal(batch[1]!.type, "keyEpochs");
  assert.equal(batch.length, 2, "nothing else is written: no new entry to get the others' keys");
  assert.ok(courier.posted.some((p) => p.machine === "m-local") || courier.mailed.some((p) => p.machine === "m-local"), "its own key handed over");
  const keys = await opened(hub, a, "m-local", local.dh);
  assert.equal(added.hostKeys, keys.size);
  for (const host of ["m-1", "m-2"]) {
    const latest = a.chain!.latestMachineEpoch(host)!;
    assert.equal(keys.get(`${host}/${latest}`), hex((await a.machineKeyBytes(host, latest))!), `${host}'s newest`);
  }
  assert.equal(a.chain!.seq, seq + 2, "the chain did not move after the registration's batch");
});

test("S1-4: removing the local install turns every other host's content key over once — not the user key — and it is sealed none of the new ones", async () => {
  const { hub, a } = await account();
  await a.setAccept("m-1", { type: "all" });
  await a.setAccept("m-2", { type: "all" });
  const local = await localInstall(hub, a, "m-local");
  await addHost(a, {
    keys: { signKey: local.keys.signPublic, dhKey: local.keys.dhPublic },
    admin: false,
    writable: true,
    courier: new FakeCourier(),
    sendCode: (code) => {
      void local.prove(code);
      signInSoon(hub, "m-local", 30);
    },
    pollMs: 1,
    hostKeysWait: { forMs: 2_000, everyMs: 10 },
  });
  const before = new Map(["m-1", "m-2"].map((h) => [h, a.chain!.machineKeyEpochs(h)]));
  const uk = a.chain!.userKeyEpochs();
  const mailedBefore = hostKeysTo(hub, "m-local").length;
  // Asked not to rotate: rotated anyway — it could control both (review D2).
  const report = await a.removeDevice("m-local", new FakeCourier(), false, { rotateMachineKeys: false });
  assert.equal(a.chain!.deviceRecord("m-local")!.status.type, "removed");
  for (const [h, n] of before) assert.equal(a.chain!.machineKeyEpochs(h), n + 1, `${h} advanced one epoch`);
  assert.equal(a.chain!.userKeyEpochs(), uk, "it never held the user key: that stays");
  assert.equal(report.ukEpoch, null);
  assert.equal(hostKeysTo(hub, "m-local").length, mailedBefore, "none of the new keys went to it");
  assert.ok(!hub.mail.some((m) => m.recipient === "m-local"), "its mail went with it");
});

const hostKeyMailTo = (hub: FakeKeysHub, id: string) => hub.mailLog.filter((m) => m.recipient === id && m.purpose === MAIL_HOST_KEY).length;

test("S4: a device's key requests are answered in full at most once per KEY_REQUEST_EVERY_MS — in between with only what is newer than what it was sealed", async () => {
  const { hub, a, m2 } = await account();
  await admit(a, "m-1", "m-2");
  const token = hub.signInHost("m-2");
  const ask = async () => {
    await a.api.mail(token, { recipient: a.deviceId, sender: "m-2", purpose: MAIL_KEY_REQUEST, epoch: 0, sealed: new Uint8Array(0) });
    await a.receiveMail();
  };
  await ask();
  const full = hostKeyMailTo(hub, "m-2");
  assert.equal(full, a.chain!.machineKeyEpochs("m-1"), "every epoch of m-1, once");
  // Asked again at once (or by a hostile device, over and over): nothing more.
  await ask();
  await ask();
  assert.equal(hostKeyMailTo(hub, "m-2"), full);
  // m-1's key turns over: the rotation seals it the new epoch itself — a request after that adds nothing either.
  await a.rotate(new FakeCourier(), false, [], { userKey: false });
  assert.equal(hostKeyMailTo(hub, "m-2"), full + 1);
  await ask();
  assert.equal(hostKeyMailTo(hub, "m-2"), full + 1, "the new epoch went once");
  // A newer epoch it was not sealed (a rotation from another admin, say): the next request brings only that one.
  const store = (a as unknown as { storage: { getItem(k: string): string | null; setItem(k: string, v: string): void } }).storage;
  const key = `rh.hub.hostkeysent.${a.account}.${a.deviceId}.m-2`;
  const sent = JSON.parse(store.getItem(key)!) as { atMs: number; through: Record<string, number> };
  store.setItem(key, JSON.stringify({ ...sent, through: { "m-1": sent.through["m-1"]! - 1 } }));
  await ask();
  assert.equal(hostKeyMailTo(hub, "m-2"), full + 2, "only the newest");
  // A window that starts in the future (the clock went back, or a spoiled record) is no window: in full.
  store.setItem(key, JSON.stringify({ ...JSON.parse(store.getItem(key)!), atMs: Date.now() + 24 * 60 * 60 * 1000 }));
  const beforeFuture = hostKeyMailTo(hub, "m-2");
  await ask();
  assert.equal(hostKeyMailTo(hub, "m-2"), beforeFuture + a.chain!.machineKeyEpochs("m-1"), "a future window answers in full");
  const answered = JSON.parse(store.getItem(key)!) as { atMs: number };
  assert.ok(answered.atMs <= Date.now(), "and starts a real window");
  await ask();
  assert.equal(hostKeyMailTo(hub, "m-2"), beforeFuture + a.chain!.machineKeyEpochs("m-1"), "which then holds");
  // The window over: in full again.
  store.setItem(key, JSON.stringify({ ...JSON.parse(store.getItem(key)!), atMs: Date.now() - KEY_REQUEST_EVERY_MS - 1 }));
  await ask();
  assert.equal(hostKeyMailTo(hub, "m-2"), beforeFuture + 2 * a.chain!.machineKeyEpochs("m-1"));
  void m2;
});

test("N7: the keys the registration handed over are not sealed again when the new host asks right after", async () => {
  const { hub, a } = await account();
  await a.setAccept("m-1", { type: "all" });
  const token = hub.signInHost("m-2");
  assert.ok((await a.handHostKeys("m-2", { forMs: 1_000, everyMs: 10 }))! >= 1);
  const handed = hostKeyMailTo(hub, "m-2");
  await a.api.mail(token, { recipient: a.deviceId, sender: "m-2", purpose: MAIL_KEY_REQUEST, epoch: 0, sealed: new Uint8Array(0) });
  await a.receiveMail();
  assert.equal(hostKeyMailTo(hub, "m-2"), handed, "nothing sealed twice");
});
