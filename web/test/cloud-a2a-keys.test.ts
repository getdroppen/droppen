// An A2A proxy (docs/a2a.md §7.1) is sealed no key, on any path: in this
// code "may control" also chooses who is sealed a host's machine keys
// (`hostKeyRecipients`, `sealHostKeys`, the purpose-4 answer, every re-send),
// and an A2A proxy controls nothing — whatever `setAccept` says, `all` or a
// list that names it. Against the fake hub, every mailbox post that reaches it
// is logged (`mailAttempts`), refused or not: none may be for the proxy. The
// policy halves (`acceptChange`, `replayChain`, `hostsToRotate`,
// `reEnrollCulprits`, `limitedDevice`) never count it either.
import assert from "node:assert/strict";
import test from "node:test";
import { generateDeviceKeys, hex, RULES } from "../src/lib/e2e/index.js";
import { AccountError, limitedDevice } from "../src/lib/hub/account.js";
import { MAIL_HOST_KEY, MAIL_KEY_REQUEST, MAIL_USER_KEY } from "../src/lib/hub/api.js";
import { type Candidate, completePairing } from "../src/lib/hub/pairing.js";
import { acceptChange, acceptNames, acceptsA2a, admits, hostsToRotate, reEnrollCulprits, replayChain } from "../src/lib/hub/chain-policy.js";
import { createV2Genesis, FakeCourier, FakeKeysHub, pairBrowser } from "./fake-keys-hub.js";

const PROXY = "a-1";

/**
 * An admin browser on a v3 account with hosts m-1 and m-2, and A2A proxy a-1 —
 * registered at the hub as a device, so it is never the hub that stops a mail
 * to it here. m-1 accepts control from every device and takes A2A calls from
 * a-1; m-2's accept list names a-1 (which does not count).
 */
async function account() {
  const hub = new FakeKeysHub();
  hub.chainV3Mode = "write";
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  await hub.enroll(a, "m-1");
  await a.upgradeToV3({ writable: true });
  await hub.enroll(a, "m-2", { writable: true });
  const keys = await generateDeviceKeys();
  hub.devices.set(PROXY, { sign: hex(keys.signPublic), dh: hex(keys.dhPublic) });
  await a.append({ type: "addDevice3", device: { id: PROXY, signKey: keys.signPublic, dhKey: keys.dhPublic, flags: { host: false, admin: false, a2a: true } } });
  await a.setAccept("m-1", { type: "all" });
  await a.setAccept("m-2", { type: "devices", ids: [PROXY] });
  await a.append({ type: "setA2aAccept", host: "m-1", devices: [PROXY] });
  hub.mailAttempts.length = 0;
  return { hub, a };
}

/** Every mailbox post that reached the hub for `id`, taken or refused. */
const mailTo = (hub: FakeKeysHub, id: string) => hub.mailAttempts.filter((m) => m.split(" ")[1] === id);

test("the chain the account reads: m-1 takes A2A calls from the proxy, and neither host lets it control it", async () => {
  const { a } = await account();
  const chain = a.chain!;
  assert.equal(chain.device(PROXY)?.a2a, true);
  assert.deepEqual(chain.acceptPolicy("m-1"), { type: "all" });
  assert.deepEqual(chain.acceptPolicy("m-2"), { type: "devices", ids: [PROXY] });
  assert.ok(acceptsA2a(chain, "m-1", PROXY));
  assert.ok(!acceptsA2a(chain, "m-2", PROXY));
  assert.ok(!admits(chain, "m-1", PROXY), "every device of the account, but never an A2A proxy");
  assert.ok(!admits(chain, "m-2", PROXY), "a list that names it does not count");
  assert.ok(!acceptNames({ type: "all" }, PROXY, true) && !acceptNames({ type: "devices", ids: [PROXY] }, PROXY, true));
  assert.ok(acceptNames({ type: "all" }, "m-2", false));
});

test("no seal-recipient path ever selects an A2A proxy: no host key, no user key, no mail at all", async () => {
  const { hub, a } = await account();
  hub.signInHost("m-2");
  assert.deepEqual(
    a.hostKeyRecipients("m-1").map((d) => d.info.id),
    ["m-2"],
    "m-1 accepts every device: the other host's device role, never the proxy",
  );
  assert.deepEqual(a.hostKeyRecipients("m-2"), [], "a list naming only the proxy hands its keys to nobody");
  const sent = await a.mailHostKey("m-1", a.chain!.latestMachineEpoch("m-1")!, (await a.machineKeyBytes("m-1", a.chain!.latestMachineEpoch("m-1")!))!);
  assert.ok(!sent.sent.includes(PROXY) && !sent.waiting.includes(PROXY) && !(PROXY in sent.failed));
  assert.equal(await a.sendHostKeys(PROXY), 0);
  assert.equal(await a.sendHostKeys(PROXY, "m-1"), 0);
  assert.equal(await a.resendKeys(PROXY), 0);
  assert.equal(await a.answerHostKeyRequest(PROXY), 0);
  assert.equal(await a.handHostKeys(PROXY, { forMs: 50, everyMs: 10 }), 0);
  // A rotation of every host's content key: sealed to the other host only.
  const report = await a.rotate(new FakeCourier(), false, [], { userKey: false });
  assert.ok(!report.hostKeysTo?.includes(PROXY));
  assert.ok(!(PROXY in (report.hostKeysFailed ?? {})));
  assert.deepEqual(mailTo(hub, PROXY), [], `nothing posted for ${PROXY}`);
  assert.ok(!hub.mailLog.some((m) => m.recipient === PROXY));
});

test("an A2A proxy's key request is acknowledged and dropped: it is sealed nothing", async () => {
  const { hub, a } = await account();
  hub.mail.push({ id: 900_001, recipient: a.deviceId, sender: PROXY, purpose: MAIL_KEY_REQUEST, epoch: 0, sealed: "" });
  await a.receiveMail();
  assert.ok(!hub.mail.some((m) => m.id === 900_001), "acknowledged");
  assert.deepEqual(mailTo(hub, PROXY), []);
  for (const purpose of [MAIL_HOST_KEY, MAIL_USER_KEY]) assert.ok(!hub.mailLog.some((m) => m.recipient === PROXY && m.purpose === purpose));
});

test("an A2A proxy stays one: never made a full device nor a device that runs AI, never offered as a limited device", async () => {
  const { hub, a } = await account();
  const refusedA2a = (e: unknown) => e instanceof AccountError && e.kind === "invalidEntry" && e.rule === RULES.a2aPlain;
  const seq = a.chain!.seq;
  await assert.rejects(a.promote(PROXY), refusedA2a);
  await assert.rejects(a.setAdmin(PROXY, true), refusedA2a);
  await assert.rejects(a.setHost(PROXY, true, new FakeCourier(), { writable: true }), refusedA2a);
  assert.equal(a.chain!.seq, seq, "nothing was signed");
  const record = a.chain!.device(PROXY)!;
  assert.equal(limitedDevice({ viewer: a.deviceId, viewerAdmin: true, device: record, upgraded: true }), null);
  assert.deepEqual(mailTo(hub, PROXY), []);
});

test("the policy halves never count an A2A proxy as a controller: no narrowing, no exposure, no re-enrollment", async () => {
  const { hub, a } = await account();
  const chain = a.chain!;
  assert.equal(a.controlsAHost(PROXY), false);
  assert.ok(!acceptChange(chain, "m-2", { type: "all" }).admitted.includes(PROXY));
  assert.deepEqual(acceptChange(chain, "m-2", { type: "admins" }).shutOut, [], "taking it off the list shuts nobody out");
  const replay = replayChain(chain, { recoveryJoin3Admin: (f) => f.admin, lost: () => {} });
  assert.equal(replay.devs.get(PROXY)?.a2a, true);
  assert.ok(!replay.could(PROXY, "m-1") && !replay.could(PROXY, "m-2"));
  assert.ok(replay.could("m-2", "m-1"));
  // Narrowing m-2's list off the proxy owes m-2 no new content key: no courier is needed.
  await a.setAccept("m-2", { type: "admins" });
  assert.equal(a.chain!.hostKeyDue, null);
  // Removing it exposes no host and asks for no re-enrollment: it never could control one.
  await a.removeDevice(PROXY, new FakeCourier(), false, { rotateMachineKeys: false });
  assert.equal(a.chain!.deviceRecord(PROXY)?.status.type, "removed");
  assert.deepEqual(a.chain!.a2aAccept("m-1"), [], "and it is off every A2A list");
  assert.equal(hostsToRotate(a.chain!).size, 0);
  assert.equal(reEnrollCulprits(a.chain!).size, 0);
  assert.deepEqual(mailTo(hub, PROXY), []);
});

test("another full device is told of an A2A proxy's join and of whom a host takes A2A calls from, as an A2A proxy", async () => {
  const hub = new FakeKeysHub();
  hub.chainV3Mode = "write";
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  await hub.enroll(a, "m-1");
  await a.upgradeToV3({ writable: true });
  const c = await pairBrowser(hub, a, { admin: true });
  await c.receiveMail();
  const keys = await generateDeviceKeys();
  hub.devices.set(PROXY, { sign: hex(keys.signPublic), dh: hex(keys.dhPublic) });
  await a.append({ type: "addDevice3", device: { id: PROXY, signKey: keys.signPublic, dhKey: keys.dhPublic, flags: { host: false, admin: false, a2a: true } } });
  await a.append({ type: "setA2aAccept", host: "m-1", devices: [PROXY] });
  await a.append({ type: "setA2aAccept", host: "m-1", devices: [] });
  await c.sync();
  assert.deepEqual(
    c.alerts().map((x) => ({ kind: x.kind, subject: x.subject, flags: x.flags, a2a: x.a2a })),
    [
      { kind: "addDevice", subject: PROXY, flags: { host: false, admin: false, a2a: true }, a2a: undefined },
      { kind: "a2aAccept", subject: "m-1", flags: undefined, a2a: [PROXY] },
      { kind: "a2aAccept", subject: "m-1", flags: undefined, a2a: [] },
    ],
  );
});

test("pairing again with an A2A proxy's id and keys is refused before anything is signed or mailed", async () => {
  const { hub, a } = await account();
  const record = a.chain!.device(PROXY)!;
  const candidate: Candidate = { device: PROXY, signKey: record.info.signKey, dhKey: record.info.dhKey, secret: new Uint8Array(32).fill(9), fingerprint: "" };
  const never = () => Promise.reject(new Error("never reached"));
  await assert.rejects(
    completePairing(
      { api: { mail: never }, token: "t", device: a.deviceId, signPublic: new Uint8Array(32), noise: null as never, chain: () => a.chain!, append: never, withUserKeys: never },
      candidate,
      { admin: true },
    ),
    (e) => e instanceof Error && e.name === "PairingError",
  );
  assert.deepEqual(mailTo(hub, PROXY), []);
});
