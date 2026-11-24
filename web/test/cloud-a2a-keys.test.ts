// An A2A proxy (docs/a2a.md §7.1) is sealed no key, on any path: in this
// code "may control" also chooses who is sealed a host's machine keys
// (`hostKeyRecipients`, `sealHostKeys`, the purpose-4 answer, every re-send),
// and an A2A proxy controls nothing — `all` does not take it in, and no
// `setAccept` list may name it. Against the fake hub, every mailbox post that
// reaches it is logged (`mailAttempts`), refused or not: none may be for the
// proxy. The policy halves (`acceptChange`, `replayChain`, `hostsToRotate`,
// `reEnrollCulprits`, `limitedDevice`) never count it as a controller either —
// but a host that took its A2A calls is exposed to it (`a2aExposures`): its
// removal rotates the content keys whatever was asked, and renewing the host's
// identity key is suggested, never done by itself.
import assert from "node:assert/strict";
import test from "node:test";
import { type Action, type ChainState, generateDeviceKeys, hex, PAUSE_START, RULES } from "../src/lib/e2e/index.js";
import { AccountError, limitedDevice } from "../src/lib/hub/account.js";
import { MAIL_HOST_KEY, MAIL_KEY_REQUEST, MAIL_USER_KEY } from "../src/lib/hub/api.js";
import { type Candidate, completePairing } from "../src/lib/hub/pairing.js";
import { a2aExposures, a2aHostsOf, acceptChange, acceptNames, acceptsA2a, admits, hostsToRotate, pauseReturns, reEnrollCulprits, replayChain } from "../src/lib/hub/chain-policy.js";
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

test("the policy halves never count an A2A proxy as a controller; removing it rotates the content keys of the hosts it could call, and suggests their renewal", async () => {
  const { hub, a } = await account();
  const chain = a.chain!;
  assert.equal(a.controlsAHost(PROXY), false);
  assert.ok(!acceptChange(chain, "m-2", { type: "all" }).admitted.includes(PROXY));
  assert.deepEqual(acceptChange(chain, "m-1", { type: "admins" }).shutOut, ["m-2"], "all let in the other host, never the proxy");
  const replay = replayChain(chain, { recoveryJoin3Admin: (f) => f.admin, lost: () => {} });
  assert.equal(replay.devs.get(PROXY)?.a2a, true);
  assert.ok(!replay.could(PROXY, "m-1") && !replay.could(PROXY, "m-2"));
  assert.ok(replay.could("m-2", "m-1"));
  assert.equal(a2aExposures(chain).size, 0, "nothing lost while it still may call");
  // Removing it: m-1 took its calls, so a session it started there could have read m-1's keys off the disk — the
  // content keys rotate even though the person asked not to.
  const epochs = new Map(["m-1", "m-2"].map((h) => [h, chain.machineKeyEpochs(h)]));
  const removal = a.chain!.seq + 1;
  await a.removeDevice(PROXY, new FakeCourier(), false, { rotateMachineKeys: false });
  assert.equal(a.chain!.deviceRecord(PROXY)?.status.type, "removed");
  assert.deepEqual(a.chain!.a2aAccept("m-1"), [], "and it is off every A2A list");
  for (const [h, n] of epochs) assert.equal(a.chain!.machineKeyEpochs(h), n + 1, `${h}'s content key turned over`);
  // m-1 is exposed to it: a suggestion, never an automatic renewal or a re-enroll warning.
  assert.deepEqual([...a.a2aExposures()].map(([h, m]) => [h, [...m]]), [["m-1", [[PROXY, removal]]]]);
  assert.equal(hostsToRotate(a.chain!).size, 0);
  assert.equal(reEnrollCulprits(a.chain!).size, 0);
  assert.deepEqual(mailTo(hub, PROXY), []);
});

test("a proxy no host takes calls from is removed with the content keys left as asked: taking it off the list rotated them already", async () => {
  const { a } = await account();
  const before = a.chain!.machineKeyEpochs("m-1");
  await a.setA2aAccept("m-1", [], new FakeCourier());
  assert.equal(a.chain!.machineKeyEpochs("m-1"), before + 1, "the narrowing turned m-1's content key over");
  assert.deepEqual(a.a2aHostsOf(PROXY), []);
  const epochs = a.chain!.machineKeyEpochs("m-1");
  await a.removeDevice(PROXY, new FakeCourier(), false, { rotateMachineKeys: false });
  assert.equal(a.chain!.machineKeyEpochs("m-1"), epochs);
  // It had left m-1's list before: m-1 was exposed to it then, and still is.
  assert.deepEqual([...(a.a2aExposures().get("m-1")?.keys() ?? [])], [PROXY]);
});

/** `chain` with `entries` applied, each signed by `a` (the admin) — through the chain's own verifier. */
async function exposuresAfter(a: Awaited<ReturnType<typeof account>>["a"], actions: Parameters<typeof a.append>[0][]): Promise<ChainState> {
  for (const action of actions) await a.append(action);
  return a.chain!;
}

test("a2aExposures: who leaves a host's A2A list, comes back, is renewed past, or goes with the host", async () => {
  const { hub, a } = await account();
  const keys = await generateDeviceKeys();
  hub.devices.set("a-2", { sign: hex(keys.signPublic), dh: hex(keys.dhPublic) });
  let chain = await exposuresAfter(a, [
    { type: "addDevice3", device: { id: "a-2", signKey: keys.signPublic, dhKey: keys.dhPublic, flags: { host: false, admin: false, a2a: true } } },
    { type: "setA2aAccept", host: "m-1", devices: [PROXY, "a-2"] },
    { type: "setA2aAccept", host: "m-2", devices: ["a-2"] },
  ]);
  assert.equal(a2aExposures(chain).size, 0);
  // Taken off m-1's list: m-1 is exposed to it.
  const off = chain.seq + 1;
  await a.setA2aAccept("m-1", ["a-2"], new FakeCourier());
  chain = a.chain!;
  assert.deepEqual([...a2aExposures(chain)].map(([h, m]) => [h, [...m]]), [["m-1", [[PROXY, off]]]]);
  // Named again: not lost any more.
  chain = await exposuresAfter(a, [{ type: "setA2aAccept", host: "m-1", devices: [PROXY, "a-2"] }]);
  assert.equal(a2aExposures(chain).size, 0);
  // A pause that holds its call is no loss: the list still names it.
  chain = await exposuresAfter(a, [{ type: "renewalPause", op: new Uint8Array(16).fill(7), phase: PAUSE_START, removals: [{ host: "m-1", ids: [PROXY] }] }]);
  assert.ok(!acceptsA2a(chain, "m-1", PROXY), "the pause holds the call");
  assert.equal(a2aExposures(chain).size, 0);
  assert.deepEqual(a2aHostsOf(chain, PROXY), ["m-1"], "the list as written: still exposed while paused");
  // a-2 removed: both hosts that took its calls are exposed.
  const gone = chain.seq + 1;
  chain = await exposuresAfter(a, [{ type: "removeDevice3", id: "a-2" }]);
  assert.deepEqual(
    [...a2aExposures(chain)].map(([h, m]) => [h, [...m]]),
    [
      ["m-1", [["a-2", gone]]],
      ["m-2", [["a-2", gone]]],
    ],
  );
  // m-2 removed: it is gone from the list, with what it was exposed to.
  chain = await exposuresAfter(a, [{ type: "removeDevice3", id: "m-2" }]);
  assert.deepEqual([...a2aExposures(chain).keys()], ["m-1"]);
});

test("a2aExposures: a renewal staged after the loss clears it, one staged before does not; a host that stops running AI keeps it", () => {
  // Only the entries and which hosts are current are read: a chain of just the actions that matter here.
  const chainOf = (actions: Action[], current: string[]) =>
    ({
      allEntries: () => actions.map((action, seq) => ({ draft: { seq, action } })),
      machine: (id: string) => (current.includes(id) ? {} : undefined),
    }) as unknown as ChainState;
  const rotate = (host: string, stagedSeq: number): Action => ({
    type: "rotateHostKey",
    rotation: { host, stagedSeq, signKey: new Uint8Array(32), dhKey: new Uint8Array(32), stagedHead: new Uint8Array(32), nonce: new Uint8Array(16), assurance: 0, oldSig: new Uint8Array(64), newSig: new Uint8Array(64) },
  });
  const base: Action[] = [
    { type: "keyEpochs", user: null, machines: [] },
    { type: "setA2aAccept", host: "h", devices: ["p"] },
    { type: "removeDevice3", id: "p" },
  ];
  const lost = (actions: Action[], current = ["h"]) => [...a2aExposures(chainOf(actions, current))].map(([h, m]) => [h, [...m]]);
  assert.deepEqual(lost(base), [["h", [["p", 2]]]]);
  assert.deepEqual(lost([...base, rotate("h", 3)]), [], "a renewal staged on a view past the loss");
  assert.deepEqual(lost([...base, rotate("h", 1)]), [["h", [["p", 2]]]], "staged before it: the old seed could have been read");
  assert.deepEqual(lost([...base, rotate("other", 3)]), [["h", [["p", 2]]]]);
  // Stopping running AI: the list goes, its proxies leave it — shown once it is a host again.
  const stopped: Action[] = [{ type: "keyEpochs", user: null, machines: [] }, { type: "setA2aAccept", host: "h", devices: ["p"] }, { type: "setHost", id: "h", host: false }];
  assert.deepEqual(lost(stopped, []), [], "not a current host: nothing to say");
  assert.deepEqual(lost(stopped), [["h", [["p", 2]]]]);
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
