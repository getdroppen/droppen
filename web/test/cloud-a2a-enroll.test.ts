// Adding an A2A proxy from this page (docs/a2a.md §7.2), against the fake
// hub: the page makes the DA1 code and announces it as `kind: "a2a"`; the
// proxy's claim is checked (claimed once, its proof under the A2A label); the
// fingerprint a person pastes from the proxy's log decides whether anything is
// added; and what is written is one batch — `addDevice3{a2a}` and the hosts'
// `setA2aAccept` — with no key epoch and no key for the proxy. Which devices
// that run AI must be updated first is read from the hub's word (`a2a/1`).
import assert from "node:assert/strict";
import test from "node:test";
import { enrollProof, enrollToken, formatA2aEnrollCode, generateDeviceKeys, hex, parseA2aEnrollCode } from "../src/lib/e2e/index.js";
import { a2aHostsOf, a2aUpgradeGate, AccountError, deviceFingerprint, fingerprintDigits, sameFingerprint } from "../src/lib/hub/account.js";
import { createV2Genesis, FakeCourier, FakeKeysHub, pairBrowser } from "./fake-keys-hub.js";

/** An admin browser on a v3 account with hosts m-1 and m-2 (and, with `other`, A2A proxy a-0 that m-2 takes calls from). */
async function account(opts: { other?: boolean } = {}) {
  const hub = new FakeKeysHub();
  hub.chainV3Mode = "write";
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  await hub.enroll(a, "m-1");
  await a.upgradeToV3({ writable: true });
  await hub.enroll(a, "m-2", { writable: true });
  if (opts.other) {
    const keys = await generateDeviceKeys();
    hub.devices.set("a-0", { sign: hex(keys.signPublic), dh: hex(keys.dhPublic) });
    await a.append({ type: "addDevice3", device: { id: "a-0", signKey: keys.signPublic, dhKey: keys.dhPublic, flags: { host: false, admin: false, a2a: true } } });
    await a.append({ type: "setA2aAccept", host: "m-2", devices: ["a-0"] });
  }
  hub.mailAttempts.length = 0;
  hub.enrollmentPosts.length = 0;
  return { hub, a };
}

const refused = (kind: string) => (e: unknown) => e instanceof AccountError && e.kind === kind;

test("the page makes the DA1 code and announces only its token, as an A2A proxy's", async () => {
  const { hub, a } = await account();
  const { code, expiresMs } = await a.startA2aEnrollment();
  const text = formatA2aEnrollCode(code);
  assert.match(text, /^DA1-[A-Z2-7]{26}-[A-Z2-7]{26}$/);
  assert.equal(code.kind, "a2a");
  assert.deepEqual(code.accountFingerprint, await a.fingerprint(), "the account's fingerprint, which the proxy pins");
  assert.ok(expiresMs !== null && expiresMs > Date.now());
  assert.deepEqual(hub.enrollmentPosts, [{ device: a.deviceId, token: hex(await enrollToken(code)), kind: "a2a" }]);
  assert.ok(!JSON.stringify(hub.enrollmentPosts).includes(hex(code.secret)), "the secret never reaches the hub");
});

test("a hub from before A2A proxies, which takes the code as a computer's, goes no further", async () => {
  const { hub, a } = await account();
  hub.noEnrollmentKinds = true;
  await assert.rejects(a.startA2aEnrollment(), refused("noA2aProxies"));
  // Announced once, and refused on the hub's answer: the page shows no code to set.
  assert.equal(hub.enrollmentPosts.length, 1);
});

test("only a full device of an upgraded account makes one: refused before anything is announced", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  await hub.enroll(a, "m-1");
  await assert.rejects(a.startA2aEnrollment(), refused("notUpgraded"));
  await a.upgradeToV3({ writable: true });
  const limited = await pairBrowser(hub, a, { admin: false });
  await assert.rejects(limited.startA2aEnrollment(), refused("notAdmin"));
  assert.deepEqual(
    hub.enrollmentPosts.filter((p) => p.kind !== undefined),
    [],
  );
});

test("a claim with a valid proof, and the right fingerprint pasted: one append adds the proxy, with no key and no host list", async () => {
  const { hub, a } = await account({ other: true });
  const { code } = await a.startA2aEnrollment();
  const proxy = await hub.claimA2a(a, code, { id: "a-1" });
  const claim = await a.awaitA2aClaim(code, undefined, 1);
  assert.deepEqual({ device: claim.device, signKey: hex(claim.signKey), dhKey: hex(claim.dhKey) }, { device: "a-1", signKey: hex(proxy.signKey), dhKey: hex(proxy.dhKey) });
  const fp = await deviceFingerprint(proxy.signKey, proxy.dhKey);
  const seq = a.chain!.seq;
  const posts = hub.chainPosts;
  const keyring = hub.keys.size;
  // Pasted as the log line says it, lower case: read the same.
  const added = await a.addA2aProxy(claim, `INFO waiting to be added to the account device=a-1 fingerprint=${fp.toLowerCase()}`);
  assert.deepEqual(added, { device: "a-1" });
  assert.equal(hub.chainPosts, posts + 1, "one append");
  const entries = a.chain!.allEntries().slice(seq + 1).map((e) => e.draft.action);
  assert.deepEqual(entries.map((x) => x.type), ["addDevice3"], "no list, no keyEpochs: every host may call it");
  const join = entries[0]!;
  assert.ok(join.type === "addDevice3");
  assert.deepEqual(join.device.flags, { host: false, admin: false, a2a: true }, "an A2A proxy, by this page's own record of the code");
  assert.equal(hex(join.device.signKey), hex(proxy.signKey));
  assert.equal(hex(join.device.dhKey), hex(proxy.dhKey));
  const chain = a.chain!;
  assert.equal(chain.device("a-1")?.a2a, true);
  assert.deepEqual(a2aHostsOf(chain, "a-1").sort(), ["m-1", "m-2"]);
  assert.doesNotThrow(() => chain.mayCallA2a("a-1", "m-1"));
  assert.equal(chain.hostKeyDue, null);
  // No key: nothing mailed to it, nothing put into the keyring for it.
  assert.deepEqual(
    hub.mailAttempts.filter((m) => m.split(" ")[1] === "a-1"),
    [],
  );
  assert.ok(!hub.mailLog.some((m) => m.recipient === "a-1"));
  assert.equal(hub.keys.size, keyring, "no keyring blob was written");
  // The code is spent here: it adds nothing again.
  await assert.rejects(a.addA2aProxy(claim, fp), refused("proof"));
});

test("a fingerprint that is not the claimer's adds nothing, and the code is given up on", async () => {
  const { hub, a } = await account();
  const { code } = await a.startA2aEnrollment();
  const proxy = await hub.claimA2a(a, code);
  const claim = await a.awaitA2aClaim(code, undefined, 1);
  const other = await generateDeviceKeys();
  const wrong = await deviceFingerprint(other.signPublic, other.dhPublic);
  const seq = a.chain!.seq;
  const posts = hub.chainPosts;
  await assert.rejects(a.addA2aProxy(claim, wrong), refused("fingerprintMismatch"));
  // Not a retry with another paste either: the right one is refused now too.
  await assert.rejects(a.addA2aProxy(claim, await deviceFingerprint(proxy.signKey, proxy.dhKey)), refused("proof"));
  assert.equal(a.chain!.seq, seq, "nothing signed");
  assert.equal(hub.chainPosts, posts, "nothing posted");
  assert.equal(a.chain!.device(proxy.id), undefined);
});

test("a code claimed more than once is abandoned, as is one from a hub that does not say how often", async () => {
  for (const claims of [2, 3, null]) {
    const { hub, a } = await account();
    const { code } = await a.startA2aEnrollment();
    await hub.claimA2a(a, code, { claims });
    const posts = hub.chainPosts;
    await assert.rejects(a.awaitA2aClaim(code, undefined, 1), refused("claims"), String(claims));
    await assert.rejects(a.awaitA2aClaim(code, undefined, 1), refused("proof"), "the code is forgotten");
    assert.equal(hub.chainPosts, posts);
  }
});

test("a proof under the machine label, or over other keys, is no claim", async () => {
  const { hub, a } = await account();
  const { code } = await a.startA2aEnrollment();
  // An RH1 agent's proof for the same secret: the labels keep the two kinds apart.
  await hub.claimA2a(a, code, { proof: (keys, id) => enrollProof({ accountFingerprint: code.accountFingerprint, secret: code.secret }, a.account, id, keys.signPublic, keys.dhPublic) });
  await assert.rejects(a.awaitA2aClaim(code, undefined, 1), refused("proof"));
  const second = await a.startA2aEnrollment();
  const swapped = await generateDeviceKeys();
  await hub.claimA2a(a, second.code, {
    proof: async (_keys, id) => (await import("../src/lib/e2e/index.js")).a2aEnrollProof(second.code, a.account, id, swapped.signPublic, swapped.dhPublic),
  });
  await assert.rejects(a.awaitA2aClaim(second.code, undefined, 1), refused("proof"));
});

test("only a code this page made as an A2A proxy's is waited on: a copy read from text adds nothing", async () => {
  const { hub, a } = await account();
  const { code } = await a.startA2aEnrollment();
  await hub.claimA2a(a, code);
  const copy = parseA2aEnrollCode(formatA2aEnrollCode(code));
  await assert.rejects(a.awaitA2aClaim(copy, undefined, 1), refused("proof"));
  // An expired one says so, and is forgotten.
  const late = await a.startA2aEnrollment();
  hub.enrollments.get(hex(await enrollToken(late.code)))!.expired = true;
  await assert.rejects(a.awaitA2aClaim(late.code, undefined, 1), refused("expired"));
});

test("a fingerprint is read as a person pastes it: groups, case and the log's fingerprint= do not matter", () => {
  const fp = "ABCD-EFGH-IJKL-MN23";
  for (const pasted of [fp, fp.toLowerCase(), " abcd efgh ijkl mn23 ", "ABCDEFGHIJKLMN23", `INFO … device=a-1 fingerprint=${fp}`, `fingerprint: ${fp}\n`]) {
    assert.ok(sameFingerprint(pasted, fp), pasted);
  }
  for (const pasted of ["", "ABCD-EFGH-IJKL-MN24", "ABCD-EFGH-IJKL", "ABCD-EFGH-IJKL-MN23-AAAA", "ABCD-EFGH-IJKL-MN18"]) assert.equal(sameFingerprint(pasted, fp), false, pasted);
  assert.equal(fingerprintDigits("not one"), null);
});

test("the upgrade gate: every current device that runs AI declared a2a/1, or the page names those that did not", async () => {
  const { a } = await account();
  const chain = a.chain!;
  assert.deepEqual(a2aUpgradeGate(chain, [{ id: "m-1", a2a: true }, { id: "m-2", a2a: true }]), null);
  assert.deepEqual(a2aUpgradeGate(chain, [{ id: "m-1", a2a: true }, { id: "m-2", a2a: false }]), { kind: "devices", hosts: ["m-2"] });
  assert.deepEqual(a2aUpgradeGate(chain, [{ id: "m-1", a2a: true }]), { kind: "devices", hosts: ["m-2"] }, "one the hub does not list has not declared it");
  assert.deepEqual(a2aUpgradeGate(chain, [{ id: "m-1", a2a: null }, { id: "m-2", a2a: null }]), { kind: "hub" }, "a hub that says nothing of a2a/1 takes no A2A proxy yet");
  assert.deepEqual(a2aUpgradeGate(chain, null), { kind: "hub" });
  // A removed or stopped host is not waited for.
  await a.removeDevice("m-2", new (await import("./fake-keys-hub.js")).FakeCourier(), false);
  assert.deepEqual(a2aUpgradeGate(a.chain!, [{ id: "m-1", a2a: true }, { id: "m-2", a2a: false }]), null);
});

test("editing who takes its calls: a widening is setA2aAccept alone; a narrowing brings the host's next content key in the same batch", async () => {
  const { hub, a } = await account();
  const { code } = await a.startA2aEnrollment();
  const proxy = await hub.claimA2a(a, code, { id: "a-1" });
  await a.addA2aProxy(await a.awaitA2aClaim(code, undefined, 1), await deviceFingerprint(proxy.signKey, proxy.dhKey));
  const courier = new FakeCourier();
  const entriesOf = async (work: () => Promise<unknown>) => {
    const seq = a.chain!.seq;
    const posts = hub.chainPosts;
    await work();
    return { actions: a.chain!.allEntries().slice(seq + 1).map((e) => e.draft.action), posts: hub.chainPosts - posts };
  };
  // Widening: both lists get the proxy (the add writes none) — no key epoch, nothing handed over.
  const wide = await entriesOf(async () => assert.deepEqual(await a.setA2aHosts("a-1", ["m-1", "m-2"], courier), { widened: ["m-1", "m-2"], narrowed: [] }));
  assert.equal(wide.posts, 2, "a list per host");
  assert.deepEqual(
    wide.actions.map((x) => (x.type === "setA2aAccept" ? [x.type, x.host, x.devices] : [x.type])),
    [
      ["setA2aAccept", "m-1", ["a-1"]],
      ["setA2aAccept", "m-2", ["a-1"]],
    ],
  );
  assert.deepEqual(courier.posted.concat(courier.mailed as never[]), []);
  // Narrowing: m-1 stops — its list without the proxy, then m-1's next machine key epoch, in one append, the key handed to m-1.
  console.log("lists after widen", a.chain!.a2aAccept("m-1"), a.chain!.a2aAccept("m-2"));
  const epoch = a.chain!.latestMachineEpoch("m-1")!;
  let change: Awaited<ReturnType<typeof a.setA2aHosts>> | null = null;
  const narrow = await entriesOf(async () => void (change = await a.setA2aHosts("a-1", ["m-2"], courier)));
  assert.equal(narrow.posts, 1, "one batch");
  assert.deepEqual(
    narrow.actions.map((x) => x.type),
    ["setA2aAccept", "keyEpochs"],
  );
  const [list, keys] = narrow.actions;
  assert.ok(list!.type === "setA2aAccept" && list!.host === "m-1" && list!.devices.length === 0);
  assert.ok(keys!.type === "keyEpochs" && keys!.machines.length === 1 && keys!.machines[0]!.machine === "m-1" && keys!.machines[0]!.epoch === epoch + 1);
  assert.equal(change!.narrowed[0]?.host, "m-1");
  assert.equal(change!.narrowed[0]?.mkEpoch, epoch + 1);
  assert.deepEqual(change!.widened, []);
  assert.ok([...courier.posted, ...courier.mailed].some((k) => k.machine === "m-1" && k.epoch === epoch + 1), "the new key reached m-1");
  assert.doesNotThrow(() => a.chain!.mayCallA2a("a-1", "m-1"), "the list no longer gates");
  assert.doesNotThrow(() => a.chain!.mayCallA2a("a-1", "m-2"));
  // Still never a key for the proxy.
  assert.ok(!hub.mailLog.some((m) => m.recipient === "a-1"));
  // Refused before anything is written: a device that is no A2A proxy, a host that is not one.
  const seq = a.chain!.seq;
  await assert.rejects(a.setA2aHosts("m-2", [], courier), refused("notInChain"));
  await assert.rejects(a.setA2aHosts("a-1", ["m-9"], courier), refused("notInChain"));
  assert.equal(a.chain!.seq, seq);
});
