// An A2A proxy (docs/a2a.md §7.1) is sealed no key, on any path: in this
// code "may control" also chooses who is sealed a host's machine keys
// (`hostKeyRecipients`, `sealHostKeys`, the purpose-4 answer, every re-send),
// and an A2A proxy controls nothing — `all` does not take it in, and no
// `setAccept` list may name it. Against the fake hub, every mailbox post that
// reaches it is logged (`mailAttempts`), refused or not: none may be for the
// proxy. The policy halves (`acceptChange`, `replayChain`, `hostsToRotate`,
// `reEnrollCulprits`, `limitedDevice`) never count it as a controller either —
// but a host that took its A2A calls is exposed to it, as to a lost controller
// (`hostsToRotate`): its removal rotates the content keys whatever was asked,
// and the host renews its identity key after it, with every host that lets it
// control them.
import assert from "node:assert/strict";
import test from "node:test";
import { type Action, type ChainState, generateDeviceKeys, hex, PAUSE_START, RULES } from "../src/lib/e2e/index.js";
import { AccountError, limitedDevice } from "../src/lib/hub/account.js";
import { MAIL_HOST_KEY, MAIL_KEY_REQUEST, MAIL_USER_KEY } from "../src/lib/hub/api.js";
import { type Candidate, completePairing } from "../src/lib/hub/pairing.js";
import { a2aHostsOf, acceptChange, acceptNames, acceptsA2a, admits, hostsExposedBy, hostsToRotate, pauseReturns, reEnrollCulprits, replayChain } from "../src/lib/hub/chain-policy.js";
import { createV2Genesis, FakeCourier, FakeKeysHub, pairBrowser } from "./fake-keys-hub.js";

const PROXY = "a-1";

/**
 * An admin browser on a v3 account with hosts m-1 and m-2, and A2A proxy a-1 —
 * registered at the hub as a device, so it is never the hub that stops a mail
 * to it here. m-1 accepts control from every device (which never takes a-1 in)
 * and takes A2A calls from a-1; m-2 accepts its full devices only.
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
  assert.ok(acceptsA2a(chain, "m-1", PROXY));
  assert.ok(!acceptsA2a(chain, "m-2", PROXY));
  assert.ok(!admits(chain, "m-1", PROXY), "every device of the account, but never an A2A proxy");
  assert.ok(!admits(chain, "m-2", PROXY));
  assert.ok(!acceptNames({ type: "all" }, PROXY, true) && !acceptNames({ type: "devices", ids: [PROXY] }, PROXY, true));
  assert.ok(acceptNames({ type: "all" }, "m-2", false));
  assert.deepEqual(a2aHostsOf(chain, PROXY), ["m-1"]);
  assert.deepEqual(a.a2aHostsOf(PROXY), ["m-1"]);
});

test("no accept list names an A2A proxy: refused here before anything is signed, and by the chain's own rule", async () => {
  const { hub, a } = await account();
  const seq = a.chain!.seq;
  const refused = (e: unknown) => e instanceof AccountError && e.kind === "invalidEntry" && e.rule === RULES.a2aNotController;
  await assert.rejects(a.setAccept("m-2", { type: "devices", ids: [PROXY] }), refused);
  await assert.rejects(a.setAccept("m-2", { type: "devices", ids: ["m-1", PROXY] }), refused);
  assert.equal(a.chain!.seq, seq, "nothing was signed");
  // Written by hand, the chain takes it no more than this page does.
  await assert.rejects(a.append({ type: "setAccept", host: "m-2", from: { type: "devices", ids: [PROXY] } }), refused);
  assert.equal(a.chain!.seq, seq);
  assert.deepEqual(mailTo(hub, PROXY), []);
});

test("no seal-recipient path ever selects an A2A proxy: no host key, no user key, no mail at all", async () => {
  const { hub, a } = await account();
  hub.signInHost("m-2");
  assert.deepEqual(
    a.hostKeyRecipients("m-1").map((d) => d.info.id),
    ["m-2"],
    "m-1 accepts every device: the other host's device role, never the proxy",
  );
  assert.deepEqual(a.hostKeyRecipients("m-2"), [], "its full devices only: no one is handed its keys this way");
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

/** What `hostsToRotate` says of each exposed host: the devices it lost (a lost controller, or an A2A proxy) and the exposed hosts it lets control it. */
const exposure = (chain: ChainState, renews?: (host: string) => boolean) =>
  [...hostsToRotate(chain, renews)].map(([h, x]) => [h, { lost: [...x.lost], via: [...x.via.keys()], blockedBy: [...x.blockedBy.keys()] }]);

test("the policy halves never count an A2A proxy as a controller; removing it rotates the content keys, and the hosts it could call renew", async () => {
  const { hub, a } = await account();
  const chain = a.chain!;
  assert.equal(a.controlsAHost(PROXY), false);
  assert.ok(!acceptChange(chain, "m-2", { type: "all" }).admitted.includes(PROXY));
  assert.deepEqual(acceptChange(chain, "m-1", { type: "admins" }).shutOut, ["m-2"], "all let in the other host, never the proxy");
  const replay = replayChain(chain, { recoveryJoin3Admin: (f) => f.admin, lost: () => {} });
  assert.equal(replay.devs.get(PROXY)?.a2a, true);
  assert.ok(!replay.could(PROXY, "m-1") && !replay.could(PROXY, "m-2"));
  assert.ok(replay.callsA2a(PROXY, "m-1") && !replay.callsA2a(PROXY, "m-2"));
  assert.ok(replay.could("m-2", "m-1"));
  assert.equal(hostsToRotate(chain).size, 0, "nothing lost while it still may call");
  // What its removal would make renew, for the dialog to say: m-1, whose A2A list names it.
  assert.deepEqual(hostsExposedBy(chain, PROXY), ["m-1"]);
  // Removing it: m-1 took its calls, so a session it started there could have read m-1's keys off the disk — the
  // content keys rotate even though the person asked not to, and m-1 renews its identity key (as for a lost controller).
  const epochs = new Map(["m-1", "m-2"].map((h) => [h, chain.machineKeyEpochs(h)]));
  const removal = a.chain!.seq + 1;
  await a.removeDevice(PROXY, new FakeCourier(), false, { rotateMachineKeys: false });
  assert.equal(a.chain!.deviceRecord(PROXY)?.status.type, "removed");
  assert.deepEqual(a.chain!.a2aAccept("m-1"), [], "and it is off every A2A list");
  for (const [h, n] of epochs) assert.equal(a.chain!.machineKeyEpochs(h), n + 1, `${h}'s content key turned over`);
  assert.deepEqual(exposure(a.chain!), [["m-1", { lost: [[PROXY, removal]], via: [], blockedBy: [] }]]);
  // Never a re-enroll warning: the agent's record of who connected does not list A2A callers.
  assert.equal(reEnrollCulprits(a.chain!).size, 0);
  assert.deepEqual(mailTo(hub, PROXY), []);
});

test("an exposed host exposes every host that lets it control it, through the A2A loss too (review F1)", async () => {
  // m-2 lets m-1 control it, so m-1 holds m-2's keys and offline copies on its disk; m-1 takes a-1's A2A calls.
  const { a } = await account();
  await a.setAccept("m-2", { type: "devices", ids: ["m-1"] });
  // Before anything is lost, removing the proxy would have both renew.
  assert.deepEqual(hostsExposedBy(a.chain!, PROXY), ["m-1", "m-2"]);
  assert.deepEqual(hostsExposedBy(a.chain!, PROXY, a2aHostsOf(a.chain!, PROXY)), ["m-1", "m-2"]);
  const off = a.chain!.seq + 1;
  await a.setA2aAccept("m-1", [], new FakeCourier());
  const exposed = new Map(exposure(a.chain!) as [string, { lost: [string, number][]; via: string[]; blockedBy: string[] }][]);
  assert.deepEqual(exposed.get("m-1")?.lost, [[PROXY, off]]);
  assert.deepEqual(exposed.get("m-2")?.via, ["m-1"], "m-2 through m-1, as for a plain controller");
  assert.equal(reEnrollCulprits(a.chain!).size, 0);
});

test("a host that stops running AI with a proxy on its list stays exposed, and so does every host that still lets it control it", async () => {
  const { a } = await account();
  await a.setAccept("m-2", { type: "devices", ids: ["m-1"] });
  const stop = a.chain!.seq + 1;
  await a.setHost("m-1", false, new FakeCourier());
  assert.deepEqual(a.chain!.a2aAccept("m-1"), [], "its list went with its hosting");
  // m-1 is no current host (no row of its own to renew), but m-2 still lets it control it: m-2 is exposed through it —
  // through a renewal of m-1's, could it renew; blocked on it, as it cannot now (narrow m-2's list, or add m-1 again).
  assert.deepEqual(exposure(a.chain!), [["m-2", { lost: [], via: ["m-1"], blockedBy: [] }]]);
  assert.deepEqual(exposure(a.chain!, (h) => h !== "m-1"), [["m-2", { lost: [], via: [], blockedBy: ["m-1"] }]]);
  // Running AI again, it is a host whose identity a proxy may have copied (the same identity): it renews too.
  await a.setHost("m-1", true, new FakeCourier(), { writable: true });
  const back = new Map(exposure(a.chain!) as [string, { lost: [string, number][] }][]);
  assert.deepEqual(back.get("m-1")?.lost, [[PROXY, stop]]);
});

test("a proxy no host takes calls from is removed with the content keys left as asked: taking it off the list rotated them already", async () => {
  const { a } = await account();
  const before = a.chain!.machineKeyEpochs("m-1");
  const off = a.chain!.seq + 1;
  await a.setA2aAccept("m-1", [], new FakeCourier());
  assert.equal(a.chain!.machineKeyEpochs("m-1"), before + 1, "the narrowing turned m-1's content key over");
  assert.deepEqual(a.a2aHostsOf(PROXY), []);
  const epochs = a.chain!.machineKeyEpochs("m-1");
  await a.removeDevice(PROXY, new FakeCourier(), false, { rotateMachineKeys: false });
  assert.equal(a.chain!.machineKeyEpochs("m-1"), epochs);
  // It had left m-1's list before: m-1 was exposed to it then, and still is.
  assert.deepEqual(exposure(a.chain!), [["m-1", { lost: [[PROXY, off]], via: [], blockedBy: [] }]]);
});

/** `chain` with `entries` applied, each signed by `a` (the admin) — through the chain's own verifier. */
async function exposuresAfter(a: Awaited<ReturnType<typeof account>>["a"], actions: Parameters<typeof a.append>[0][]): Promise<ChainState> {
  for (const action of actions) await a.append(action);
  return a.chain!;
}

test("hostsToRotate counts A2A losses: who leaves a host's A2A list, comes back, or goes with the host", async () => {
  const { hub, a } = await account();
  const keys = await generateDeviceKeys();
  hub.devices.set("a-2", { sign: hex(keys.signPublic), dh: hex(keys.dhPublic) });
  let chain = await exposuresAfter(a, [
    { type: "addDevice3", device: { id: "a-2", signKey: keys.signPublic, dhKey: keys.dhPublic, flags: { host: false, admin: false, a2a: true } } },
  ]);
  await a.setA2aAccept("m-1", [PROXY, "a-2"]);
  await a.setA2aAccept("m-2", ["a-2"]);
  chain = a.chain!;
  const lostOf = (c: ChainState) => [...hostsToRotate(c)].map(([h, x]) => [h, [...x.lost]]);
  assert.deepEqual(lostOf(chain), []);
  // Taken off m-1's list: m-1 is exposed to it.
  const off = chain.seq + 1;
  await a.setA2aAccept("m-1", ["a-2"], new FakeCourier());
  chain = a.chain!;
  assert.deepEqual(lostOf(chain), [["m-1", [[PROXY, off]]]]);
  // Named again: not lost any more.
  await a.setA2aAccept("m-1", [PROXY, "a-2"]);
  chain = a.chain!;
  assert.deepEqual(lostOf(chain), []);
  // A pause that holds its call is no loss: the list still names it.
  chain = await exposuresAfter(a, [{ type: "renewalPause", op: new Uint8Array(16).fill(7), phase: PAUSE_START, removals: [{ host: "m-1", ids: [PROXY] }] }]);
  assert.ok(!acceptsA2a(chain, "m-1", PROXY), "the pause holds the call");
  assert.deepEqual(lostOf(chain), []);
  assert.deepEqual(a2aHostsOf(chain, PROXY), ["m-1"], "the list as written: still exposed while paused");
  // a-2 removed: both hosts that took its calls are exposed (m-1 also through m-2, which it lets control it).
  const gone = chain.seq + 1;
  chain = await exposuresAfter(a, [{ type: "removeDevice3", id: "a-2" }]);
  assert.deepEqual(lostOf(chain), [
    ["m-1", [["a-2", gone]]],
    ["m-2", [["a-2", gone]]],
  ]);
  // m-2 removed: it is gone, with what it was exposed to.
  chain = await exposuresAfter(a, [{ type: "removeDevice3", id: "m-2" }]);
  assert.deepEqual(
    lostOf(chain).map(([h]) => h),
    ["m-1"],
  );
});

test("ending a pause gives an A2A proxy's held call back when the host's A2A list names it", async () => {
  const { a } = await account();
  const op = new Uint8Array(16).fill(9);
  const chain = await exposuresAfter(a, [{ type: "renewalPause", op, phase: PAUSE_START, removals: [{ host: "m-1", ids: [PROXY] }] }]);
  const open = chain.openPauses().find((p) => p.op[0] === 9)!;
  assert.deepEqual(pauseReturns(chain, open), { edges: [{ host: "m-1", id: PROXY }], all: [] }, "its A2A list, not m-1's `all`, gives it back");
  await a.setA2aAccept("m-1", [], new FakeCourier());
  const after = a.chain!;
  assert.deepEqual(pauseReturns(after, after.openPauses().find((p) => p.op[0] === 9)!).edges, [], "off the list: nothing comes back");
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
  await a.setA2aAccept("m-1", [PROXY]);
  await a.setA2aAccept("m-1", [], new FakeCourier());
  await c.sync();
  assert.deepEqual(
    c
      .alerts()
      .filter((x) => x.kind !== "keyEpochs")
      .map((x) => ({ kind: x.kind, subject: x.subject, flags: x.flags, a2a: x.a2a })),
    [
      { kind: "addDevice", subject: PROXY, flags: { host: false, admin: false, a2a: true }, a2a: undefined },
      { kind: "a2aAccept", subject: "m-1", flags: undefined, a2a: [PROXY] },
      { kind: "a2aAccept", subject: "m-1", flags: undefined, a2a: [] },
    ],
  );
  // A pause that holds its calls: the proxy is said apart from control, as A2A calls.
  await a.setA2aAccept("m-1", [PROXY]);
  const op = new Uint8Array(16).fill(5);
  await a.append({ type: "renewalPause", op, phase: PAUSE_START, removals: [{ host: "m-1", ids: [PROXY] }] });
  await c.sync();
  const pause = c.alerts().find((x) => x.kind === "renewalPause");
  assert.deepEqual(pause?.pause, { phase: "start", removals: [{ host: "m-1", ids: [], a2a: [PROXY] }] });
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

test("setA2aAccept: a narrowing writes the host's next key epoch in the same batch; widening writes no key; the same list writes nothing", async () => {
  const { hub, a } = await account();
  const keys = await generateDeviceKeys();
  hub.devices.set("a-2", { sign: hex(keys.signPublic), dh: hex(keys.dhPublic) });
  await a.append({ type: "addDevice3", device: { id: "a-2", signKey: keys.signPublic, dhKey: keys.dhPublic, flags: { host: false, admin: false, a2a: true } } });
  // The same list, however it is written: nothing.
  let posts = hub.chainPosts;
  let seq = a.chain!.seq;
  assert.deepEqual(await a.setA2aAccept("m-1", [PROXY, PROXY]), { narrowed: false, admitted: [], dropped: [] });
  assert.equal(hub.chainPosts, posts, "nothing posted");
  assert.equal(a.chain!.seq, seq);
  // Widening (written in any order, ascending on the chain): the list alone, no key.
  const epochs = a.chain!.machineKeyEpochs("m-1");
  const wide = await a.setA2aAccept("m-1", ["a-2", PROXY]);
  assert.deepEqual(wide, { narrowed: false, admitted: ["a-2"], dropped: [] });
  assert.equal(a.chain!.seq, seq + 1);
  assert.deepEqual(a.chain!.entryAt(seq + 1)?.draft.action, { type: "setA2aAccept", host: "m-1", devices: ["a-1", "a-2"] });
  assert.equal(a.chain!.machineKeyEpochs("m-1"), epochs, "a proxy let in is sealed nothing, and no key turns over");
  // Narrowing: [setA2aAccept, keyEpochs] in one post.
  posts = hub.chainPosts;
  seq = a.chain!.seq;
  const narrow = await a.setA2aAccept("m-1", ["a-2"], new FakeCourier());
  assert.equal(narrow.narrowed, true);
  assert.deepEqual([narrow.admitted, narrow.dropped], [[], [PROXY]]);
  assert.equal(narrow.mkEpoch, epochs);
  assert.ok(!narrow.hostKeysTo?.includes(PROXY) && !narrow.hostKeysTo?.includes("a-2"), "the new key is sealed to no proxy");
  assert.equal(hub.chainPosts, posts + 1, "one batch");
  assert.equal(a.chain!.seq, seq + 2);
  assert.deepEqual(a.chain!.entryAt(seq + 1)?.draft.action, { type: "setA2aAccept", host: "m-1", devices: ["a-2"] });
  const keyed = a.chain!.entryAt(seq + 2)?.draft.action;
  assert.ok(keyed?.type === "keyEpochs" && keyed.machines.some((m) => m.machine === "m-1" && m.epoch === epochs), "m-1's next key epoch right after");
  assert.equal(a.chain!.machineKeyEpochs("m-1"), epochs + 1);
  assert.equal(a.chain!.hostKeyDue, null);
  assert.deepEqual(mailTo(hub, PROXY), []);
  assert.deepEqual(mailTo(hub, "a-2"), []);
});

test("setA2aAccept: a narrowing without a way to hand the host its new key is refused before anything is signed", async () => {
  const { hub, a } = await account();
  const posts = hub.chainPosts;
  const seq = a.chain!.seq;
  await assert.rejects(a.setA2aAccept("m-1", []), (e) => e instanceof AccountError && e.kind === "invalidEntry" && e.rule === RULES.hostKeyNext);
  assert.equal(hub.chainPosts, posts, "nothing posted");
  assert.equal(a.chain!.seq, seq, "nothing signed");
  // Nor does the chain take the list without the key, written by hand.
  await assert.rejects(a.append({ type: "setA2aAccept", host: "m-1", devices: [] }), (e) => e instanceof AccountError && e.rule === RULES.hostKeyNext);
  assert.equal(a.chain!.seq, seq);
  // Only for a current host.
  await assert.rejects(a.setA2aAccept(PROXY, []), (e) => e instanceof AccountError && e.kind === "notInChain");
});
