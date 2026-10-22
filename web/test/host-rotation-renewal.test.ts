// Renewing a host's identity key from the browser (docs/host-identity-rotation.md
// §3.1–§3.8, §4.1–§4.3), against the fake hub and fake agents.
//
// What these pin: which hosts must renew (`hostsToRotate`: the losses, and
// transitively every host that let an exposed one control it — X → H → H2 —
// cleared only by a later renewal of that host); `renewIdentity` checks the
// host's answer before anything is written (both signatures, the DH
// confirmation, a key the chain never named, the view asked about, the code
// typed for a renewal confirmed on the device) and waits for the hub's window;
// a dispute signed by a retired key isolates the host until a renewal
// confirmed on the device (or its removal); a host waiting to renew or isolated
// is sealed no host keys and let control no other host; the runner renews one
// host at a time, the ones a waiting host depends on first, retries after 1, 5
// and 30 minutes and then asks, and waits for an offline host to come back;
// The re-enroll warning stays only for hosts that cannot renew.
import assert from "node:assert/strict";
import test from "node:test";
import { ASSURANCE_AUTO, ASSURANCE_ON_DEVICE, type ChainState, encodeEntry, fromHex, hex, MAX_PAUSE_ENTRY_LEN, signHubLogin, signRotateDispute } from "../src/lib/e2e/index.js";
import { AccountError, CloudAccount, type CrossTab, hostsExposedBy, hostsToRotate, RenewalError, renewalHelps, renewalRefusal, splitPauseRemovals } from "../src/lib/hub/account.js";
import { type AccountView, hostDispute, HubApiError, MAIL_HOST_KEY, rotationPush } from "../src/lib/hub/api.js";
import { ABANDONED_MS, checkRetiredLogin, checkStatement, isolatedHosts, RenewalRunner, renewalOrder } from "../src/lib/hub/renewal.js";
import { followPushRenewals, followRenewedKey, type PushEnv, pushRenewalNeeded, readPushRecord } from "../src/lib/push.js";
import { candidateFromOffer, PairingResponder } from "../src/lib/hub/pairing.js";
import { browserStorage, createV2Genesis, FakeAgents, FakeCourier, FakeKeysHub, HUB_ORIGIN, RecordingKeyStore } from "./fake-keys-hub.js";

const rotated = (a: CloudAccount): string[] =>
  a.chain!.allEntries().flatMap((e) => (e.draft.action.type === "rotateHostKey" ? [e.draft.action.rotation.host] : []));
const exposedIds = (chain: ChainState): string[] => [...hostsToRotate(chain).keys()].sort();
const renewal = (kind: RenewalError["kind"], detail?: string) => (e: unknown) => {
  assert.ok(e instanceof RenewalError, String(e));
  assert.equal(e.kind, kind, e.message);
  if (detail !== undefined) assert.equal(e.detail, detail);
  return true;
};
const quick = { pollMs: 1, hostKeysWait: { forMs: 0, everyMs: 1 } };

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

/**
 * A v3 account: admin `a`, hosts m-1, m-2 and m-3 (each declared `rotate/1`),
 * the agents that answer for them, and browser X (not an admin) that m-1 lets
 * control it. m-2 lets m-1 control it (`chain`: true). The account chose its
 * renewal policy — automatic — unless `policy` is false (§5.6 「打开的顺序」).
 */
async function account(opts: { chain?: boolean; policy?: boolean } = {}) {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  const agents = new FakeAgents(hub, () => a.chain);
  const h1 = await hub.enroll(a, "m-1");
  await a.upgradeToV3({ writable: true });
  const h2 = await hub.enroll(a, "m-2", { writable: true });
  const h3 = await hub.enroll(a, "m-3", { writable: true });
  for (const h of [h1, h2, h3]) {
    agents.hold(h.id, { sign: h.signPrivate, signPublic: h.sign, dhPublic: h.dh.publicKey });
    hub.rotates.add(h.id);
  }
  if (opts.policy !== false) await a.setRenewalPolicy(false);
  const x = await pairAs(hub, a, { admin: false });
  await a.setAccept("m-1", { type: "devices", ids: [x.deviceId] });
  if (opts.chain !== false) await a.setAccept("m-2", { type: "devices", ids: ["m-1"] });
  return { hub, a, agents, x, h1, h2, h3 };
}

/** The page's hub link, as the runner hears it. */
class FakeLink {
  readonly presence = new Map<string, { state: "online" | "offline" | "unknown"; sinceMs: number; via: never[] }>();
  private readonly presenceFns = new Set<(p: Map<string, unknown>) => void>();
  private readonly controlFns = new Set<(m: Record<string, unknown>) => void>();
  onPresence(fn: (p: Map<string, unknown>) => void) {
    this.presenceFns.add(fn);
    return () => this.presenceFns.delete(fn);
  }
  onControl(fn: (m: Record<string, unknown>) => void) {
    this.controlFns.add(fn);
    return () => this.controlFns.delete(fn);
  }
  onState() {
    return () => undefined;
  }
  set(id: string, state: "online" | "offline") {
    this.presence.set(id, { state, sinceMs: 0, via: [] });
    for (const fn of this.presenceFns) fn(this.presence);
  }
  push(message: Record<string, unknown>) {
    for (const fn of this.controlFns) fn(message);
  }
}

/** `/v1/me` as the runner reads it: which hosts declared `rotate/1`. */
const meOf = (rotates: Record<string, boolean>, chainSeq: number | null = null) => ({
  view: { chainSeq, machines: Object.entries(rotates).map(([id, r]) => ({ id, rotates: r })) } as unknown as AccountView,
  onChange: () => () => undefined,
});

async function until(what: () => boolean, ms = 4000): Promise<void> {
  const deadline = Date.now() + ms;
  while (!what()) {
    if (Date.now() > deadline) throw new Error("timed out waiting");
    await new Promise((resolve) => setTimeout(resolve, 2));
  }
}

/**
 * The dispute statement of `host`'s renewal at `seq` (its latest when not given), as the hub hands it on (§3.8):
 * `rh/v1 rotate-dispute` over that entry's hash and the key it put in, signed with `key` — honestly the key it retired.
 */
async function statementJson(a: CloudAccount, host: string, key: CryptoKey, seq = a.chain!.deviceRecord(host)!.rotatedSeq!) {
  const entry = a.chain!.entryAt(seq)!.draft.action;
  assert.ok(entry.type === "rotateHostKey");
  const hash = a.chain!.hashAt(seq)!;
  const sig = await signRotateDispute(key, a.account, host, seq, hash, entry.rotation.signKey);
  return { host, kind: "disputed", seen_ms: Date.now(), seq, entry_hash: hex(hash), sig: hex(sig) };
}

/** The kept form of `host`'s dispute statement of its renewal at `seq` (its latest when not given), signed by `key` — checked as the runner checks it. */
async function keptOf(a: CloudAccount, host: string, key: CryptoKey, seq?: number) {
  const kept = await checkStatement(a.chain!, hostDispute(await statementJson(a, host, key, seq))!);
  assert.ok(kept, `a statement for ${host}`);
  return kept;
}

/** What isolates `host` after its last (automatic) renewal: the statement its retired key signs. */
async function isolatingEvidence(a: CloudAccount, host: string, h: { signPrivate: CryptoKey }) {
  return statementJson(a, host, h.signPrivate);
}

/** A retired key's sign-in as the hub records it (§3.8): `HubLogin` signed by `key`, for `host`, retired at `retiredSeq`. */
async function disputeJson(a: CloudAccount, host: string, key: CryptoKey, signKey: Uint8Array, retiredSeq: number, timeMs?: number) {
  // Signed after the renewal, past the clock skew allowed (R6 review) — unless a test says when.
  const after = (a.chain!.entryAt(retiredSeq)?.draft.createdMs ?? Date.now()) + 6 * 60_000;
  const login = { role: "agent" as const, account: a.account, id: host, hub: HUB_ORIGIN, nonce: crypto.getRandomValues(new Uint8Array(32)), timeMs: timeMs ?? after };
  const sig = await signHubLogin(login, key);
  return {
    host,
    kind: "retired_login",
    seen_ms: Date.now(),
    retired_seq: retiredSeq,
    role: "agent",
    login: { account: login.account, id: login.id, hub: login.hub, nonce: hex(login.nonce), time_ms: login.timeMs },
    sig: hex(sig),
    sign_key: hex(signKey),
  };
}

test("exposure is transitive: X → m-1 → m-2, and a host let control another while exposed exposes it too", async () => {
  const { a, x } = await account();
  assert.deepEqual(exposedIds(a.chain!), [], "nothing lost yet");
  // What the removal's confirmation says before it is written.
  assert.deepEqual(hostsExposedBy(a.chain!, x.deviceId), ["m-1", "m-2"]);
  await a.removeDevice(x.deviceId, new FakeCourier());
  const exposed = hostsToRotate(a.chain!);
  assert.deepEqual([...exposed.keys()].sort(), ["m-1", "m-2"]);
  assert.deepEqual([...exposed.get("m-1")!.lost.keys()], [x.deviceId]);
  assert.deepEqual([...exposed.get("m-2")!.via.keys()], ["m-1"], "m-2 through m-1, whose copy is a pass to it");
  // Let in while m-1 is exposed (no renewal here yet: the guard is off): m-3 is exposed as well.
  await a.setAccept("m-3", { type: "devices", ids: ["m-1"] });
  assert.deepEqual(exposedIds(a.chain!), ["m-1", "m-2", "m-3"]);
  assert.deepEqual([...hostsToRotate(a.chain!).get("m-3")!.via.keys()], ["m-1"]);
});

test("a renewal clears that host only; what it let in stays exposed until each renews — controllers first: renewIdentity refuses one ahead of an exposed host it lets in (§4.1)", async () => {
  const { a, agents, x } = await account();
  await a.removeDevice(x.deviceId, new FakeCourier());
  // m-1 first: m-2 stays exposed (m-1's renewal does not unexpose what m-2 may have been copied), then m-2.
  await a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick });
  assert.deepEqual(exposedIds(a.chain!), ["m-2"]);
  await a.renewIdentity("m-2", agents, { assurance: ASSURANCE_AUTO, ...quick });
  assert.deepEqual(exposedIds(a.chain!), []);
  // m-1 renewed: letting it control m-3 now exposes nothing.
  await a.setAccept("m-3", { type: "devices", ids: ["m-1"] });
  assert.deepEqual(exposedIds(a.chain!), []);

  // The other order is refused (§4.1 「次序先于收窄」): m-2 lets m-1 (exposed, not renewed yet) control it — whoever asks,
  // automatic or confirmed on the device, nothing is asked of m-2 or staged at the hub.
  const other = await account();
  await other.a.removeDevice(other.x.deviceId, new FakeCourier());
  assert.deepEqual(other.a.renewFirst("m-2"), ["m-1"]);
  assert.deepEqual(other.a.renewFirst("m-1"), []);
  for (const assurance of [ASSURANCE_AUTO, ASSURANCE_ON_DEVICE]) {
    await assert.rejects(other.a.renewIdentity("m-2", other.agents, { assurance, askCode: async () => "", ...quick }), (e: unknown) => {
      assert.ok(e instanceof RenewalError && e.kind === "renewFirst", String(e));
      assert.deepEqual(e.hosts, ["m-1"]);
      return true;
    });
  }
  assert.deepEqual(other.agents.rotateCalls, [], "m-2 was not asked");
  assert.deepEqual(other.hub.rotationPuts.filter((p) => p.startsWith("m-2 ")), [], "nothing staged for m-2");
  await other.a.renewIdentity("m-1", other.agents, { assurance: ASSURANCE_AUTO, ...quick });
  assert.deepEqual(other.a.renewFirst("m-2"), []);
  await other.a.renewIdentity("m-2", other.agents, { assurance: ASSURANCE_AUTO, ...quick });
  assert.deepEqual(exposedIds(other.a.chain!), []);
  assert.deepEqual(rotated(other.a), ["m-1", "m-2"], "each once, the controller first");
  // A host that is not exposed is never held back (a renewal nobody asked for).
  await other.a.setAccept("m-3", { type: "devices", ids: ["m-2"] });
  assert.deepEqual(other.a.renewFirst("m-3"), []);
  await other.a.renewIdentity("m-3", other.agents, { assurance: ASSURANCE_AUTO, ...quick });
  assert.deepEqual(rotated(other.a), ["m-1", "m-2", "m-3"]);
  const x0 = (via: [string, number][] = []) => ({ sinceSeq: 5, lost: new Map(), via: new Map(via), blockedBy: new Map() });
  assert.deepEqual(renewalOrder(["m-2", "m-1"], new Map([["m-1", x0()], ["m-2", x0([["m-1", 5]])]]), () => true), ["m-1"], "the one depended on first");
});

test("after its renewal a host is exposed again only by a new loss, or an exposed host added to its list afterwards (§4.1)", async () => {
  const { hub, a, agents, x } = await account({ chain: false });
  const y = await pairAs(hub, a, { admin: false });
  await a.setAccept("m-1", { type: "devices", ids: [x.deviceId, y.deviceId] });
  await a.removeDevice(x.deviceId, new FakeCourier());
  await a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick });
  assert.deepEqual(exposedIds(a.chain!), []);
  // (1) A new loss after the renewal.
  await a.removeDevice(y.deviceId, new FakeCourier());
  assert.deepEqual(exposedIds(a.chain!), ["m-1"]);
  assert.deepEqual([...hostsToRotate(a.chain!).get("m-1")!.lost.keys()], [y.deviceId]);
  // (2) m-3 renewed, then an exposed host (m-1) added to its list after that (an older page could write it).
  await a.renewIdentity("m-3", agents, { assurance: ASSURANCE_AUTO, ...quick });
  await a.setAccept("m-3", { type: "devices", ids: ["m-1"] });
  assert.deepEqual([...hostsToRotate(a.chain!).get("m-3")!.via.keys()], ["m-1"]);
});

/** The kinds of the entries after `from`, with the host each is about. */
const writtenAfter = (a: CloudAccount, from: number) =>
  a.chain!.allEntries().slice(from + 1).map((e) => {
    const x = e.draft.action;
    if (x.type === "renewalPause") {
      if (x.phase === 1) return "pause completed";
      if (x.phase === 2) return "pause cancelled";
      return `pause start ${x.removals.map((r) => `${r.host}<${r.ids.join("+")}`).join(",")}`;
    }
    return x.type === "setAccept" ? `setAccept ${x.host}` : x.type === "keyEpochs" ? `keyEpochs ${x.machines.map((m) => m.machine).join(",")}` : x.type === "rotateHostKey" ? `rotate ${x.rotation.host}` : x.type;
  });
const admitsOn = (chain: ChainState, host: string, id: string) => {
  try {
    chain.mayControl(id, host);
    return true;
  } catch {
    return false;
  }
};

test("§4.1 overlay: two hosts that let each other control one another — ONE RenewalPause start takes the edges out, each renews once, and the pause completes only after both did; no list is ever written", async () => {
  const { a, agents, x } = await account();
  await a.setAccept("m-1", { type: "devices", ids: [x.deviceId, "m-2"] });
  const base = { m1: a.chain!.baseAccept("m-1"), m2: a.chain!.baseAccept("m-2") };
  await a.removeDevice(x.deviceId, new FakeCourier());
  const removedAt = a.chain!.seq;
  const exposed = hostsToRotate(a.chain!);
  assert.deepEqual([...exposed.keys()].sort(), ["m-1", "m-2"]);
  const link = new FakeLink();
  link.set("m-1", "online");
  link.set("m-2", "offline");
  const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }), renewOptions: quick });
  runner.start();
  await until(() => rotated(a).includes("m-1"));
  await runner.idle();
  const written = writtenAfter(a, removedAt);
  assert.equal(written[0], "pause start m-1<m-2,m-2<m-1", written.join(" | "));
  assert.ok(!written.some((k) => k.startsWith("setAccept")), "no list written");
  assert.equal(written.indexOf("rotate m-1") > 0, true);
  // On the chain, neither lets the other in now (whatever carries it) — while both lists (the base) stay as they were.
  assert.equal(admitsOn(a.chain!, "m-1", "m-2"), false);
  assert.equal(admitsOn(a.chain!, "m-2", "m-1"), false);
  assert.equal(sameAcceptText(a.chain!.baseAccept("m-2")), sameAcceptText(base.m2));
  assert.deepEqual(a.chain!.pausedEdges("m-1"), ["m-2"]);
  assert.deepEqual(runner.pausedWith("m-1"), ["m-1", "m-2"]);
  // m-2 back: it renews, then the pause completes and the edges come back.
  link.set("m-2", "online");
  await until(() => rotated(a).includes("m-2") && a.chain!.openPauses().length === 0);
  await runner.idle();
  const all = writtenAfter(a, removedAt);
  assert.equal(all.filter((k) => k.startsWith("pause")).length, 2, all.join(" | "));
  assert.ok(all.indexOf("pause completed") > all.indexOf("rotate m-2"), "only after the last renewal");
  assert.ok(!all.some((k) => k.startsWith("setAccept")), "still no list written");
  assert.equal(admitsOn(a.chain!, "m-1", "m-2"), true);
  assert.equal(admitsOn(a.chain!, "m-2", "m-1"), true);
  assert.deepEqual(rotated(a).sort(), ["m-1", "m-2"], "each exactly once");
  assert.deepEqual(exposedIds(a.chain!), [], "pausing and resuming exposed nothing of their own");
  void base.m1;
  runner.stop();
});

const sameAcceptText = (p: { type: string; ids?: string[] }) => `${p.type}:${[...(p.ids ?? [])].sort().join(",")}`;

test("§4.1 overlay: a list that lets every device in stays `All` — the pause only takes the cycle's edge out, and gives it back", async () => {
  const { a, agents, x } = await account();
  await a.setAccept("m-1", { type: "all" });
  await a.removeDevice(x.deviceId, new FakeCourier());
  assert.deepEqual(exposedIds(a.chain!), ["m-1", "m-2"]);
  const link = new FakeLink();
  for (const h of ["m-1", "m-2", "m-3"]) link.set(h, "online");
  const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }), renewOptions: quick });
  runner.start();
  await until(() => rotated(a).length === 1);
  assert.equal(a.chain!.baseAccept("m-1").type, "all", "the base is untouched");
  assert.equal(admitsOn(a.chain!, "m-1", "m-2"), false, "m-2 paused");
  assert.equal(admitsOn(a.chain!, "m-1", "m-3"), true, "everyone else still let in");
  await until(() => rotated(a).length === 2 && a.chain!.openPauses().length === 0);
  await runner.idle();
  assert.equal(admitsOn(a.chain!, "m-1", "m-2"), true);
  assert.deepEqual(exposedIds(a.chain!), []);
  runner.stop();
});

test("§4.1 a host that cannot renew: in an acyclic part it blocks the hosts that let it in (never renewed out of order); in a cycle it keeps its peers narrowed", async () => {
  // Acyclic: m-1 lets m-3 (an older agent, exposed) control it — m-1 waits, blocked, and nothing is narrowed.
  {
    const { hub, a, agents, x } = await account({ chain: false });
    hub.rotates.delete("m-3");
    await a.setAccept("m-3", { type: "devices", ids: [x.deviceId] });
    await a.setAccept("m-1", { type: "devices", ids: [x.deviceId, "m-3"] });
    await a.removeDevice(x.deviceId, new FakeCourier());
    const removedAt = a.chain!.seq;
    const link = new FakeLink();
    link.set("m-1", "online");
    link.set("m-3", "online");
    const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": false }), renewOptions: quick });
    runner.start();
    await runner.idle();
    await new Promise((resolve) => setTimeout(resolve, 20));
    await runner.idle();
    assert.deepEqual(runner.status("m-1"), { kind: "blocked", by: ["m-3"] });
    assert.deepEqual(agents.rotateCalls, [], "not renewed before m-3, which never renews");
    assert.deepEqual(writtenAfter(a, removedAt).filter((k) => k.startsWith("setAccept")), [], "acyclic: nothing narrowed");
    runner.stop();
  }
  // A cycle: m-1 and m-3 let each other in. Narrowed; m-1 renews; restored never, while m-3 is exposed.
  {
    const { hub, a, agents, x } = await account({ chain: false });
    hub.rotates.delete("m-3");
    await a.setAccept("m-3", { type: "devices", ids: [x.deviceId, "m-1"] });
    await a.setAccept("m-1", { type: "devices", ids: [x.deviceId, "m-3"] });
    await a.removeDevice(x.deviceId, new FakeCourier());
    const link = new FakeLink();
    link.set("m-1", "online");
    link.set("m-3", "online");
    const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": false }), renewOptions: quick });
    runner.start();
    await until(() => rotated(a).includes("m-1"));
    await runner.idle();
    await new Promise((resolve) => setTimeout(resolve, 20));
    await runner.idle();
    assert.equal(admitsOn(a.chain!, "m-1", "m-3"), false, "m-3 no longer controls m-1");
    assert.deepEqual(agents.rotateCalls, ["m-1"]);
    assert.deepEqual(runner.status("m-3"), { kind: "tooOld" });
    assert.equal(admitsOn(a.chain!, "m-1", "m-3"), false, "and stays narrowed while m-3 is exposed");
    assert.deepEqual(runner.pausedWith("m-1"), ["m-1", "m-3"]);
    runner.stop();
  }
});

test("§4.1 order before narrowing: a one-directional pair renews the controller first and narrows nothing; a host never renews before an exposed host it lets in", async () => {
  // account(): m-2 lets m-1 control it. m-1 is away at first: m-2 waits for it.
  const { a, agents, x } = await account();
  await a.removeDevice(x.deviceId, new FakeCourier());
  const removedAt = a.chain!.seq;
  assert.deepEqual(exposedIds(a.chain!), ["m-1", "m-2"]);
  const link = new FakeLink();
  link.set("m-1", "offline");
  link.set("m-2", "online");
  const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }), renewOptions: quick });
  runner.start();
  await runner.idle();
  await new Promise((resolve) => setTimeout(resolve, 20));
  await runner.idle();
  assert.deepEqual(agents.rotateCalls, [], "m-2 does not renew before m-1, which it lets control it");
  assert.deepEqual(runner.status("m-2"), { kind: "queued", after: ["m-1"], stuck: [{ host: "m-1", why: "offline" }] }, "and says why m-1 is not renewing");
  // The person's "renew identity key" of m-2 does not renew it ahead of m-1 either.
  await assert.rejects(runner.renew("m-2", { assurance: ASSURANCE_AUTO }), (e: unknown) => e instanceof RenewalError && e.kind === "renewFirst" && e.hosts?.join() === "m-1");
  await runner.idle();
  assert.deepEqual(agents.rotateCalls, [], "still nothing renewed");
  link.set("m-1", "online");
  await until(() => rotated(a).length === 2);
  await runner.idle();
  assert.deepEqual(rotated(a), ["m-1", "m-2"], "the controller first");
  assert.deepEqual(writtenAfter(a, removedAt).filter((k) => k.startsWith("setAccept")), [], "ordered, not narrowed");
  assert.equal(admitsOn(a.chain!, "m-2", "m-1"), true, "m-2 still lets m-1 in");
  assert.deepEqual(exposedIds(a.chain!), []);
  runner.stop();
});

test("§4.1 a queued host's row says why the host it waits for is not renewing: it needs the code, is offline, or is blocked", async () => {
  const { a, agents, x } = await account();
  await a.removeDevice(x.deviceId, new FakeCourier());
  const link = new FakeLink();
  link.set("m-1", "online");
  link.set("m-2", "online");
  // This device signs only renewals confirmed on the device: m-1 waits for its code, so m-2 waits for m-1.
  a.setLocalAssured(true);
  const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }), renewOptions: quick });
  runner.start();
  await runner.idle();
  await new Promise((resolve) => setTimeout(resolve, 20));
  await runner.idle();
  assert.deepEqual(runner.status("m-1"), { kind: "needsCode", why: "device" });
  assert.deepEqual(runner.status("m-2"), { kind: "queued", after: ["m-1"], stuck: [{ host: "m-1", why: "needsCode" }] });
  assert.deepEqual(agents.rotateCalls, []);
  // Offline instead.
  a.setLocalAssured(false);
  link.set("m-1", "offline");
  await runner.idle();
  assert.deepEqual(runner.status("m-2"), { kind: "queued", after: ["m-1"], stuck: [{ host: "m-1", why: "offline" }] });
  runner.stop();
  // Blocked: m-1 lets m-3 (an older agent, exposed) control it; m-2 waits for m-1.
  {
    const b = await account();
    b.hub.rotates.delete("m-3");
    await b.a.setAccept("m-3", { type: "devices", ids: [b.x.deviceId] });
    await b.a.setAccept("m-1", { type: "devices", ids: [b.x.deviceId, "m-3"] });
    await b.a.removeDevice(b.x.deviceId, new FakeCourier());
    const link2 = new FakeLink();
    for (const h of ["m-1", "m-2", "m-3"]) link2.set(h, "online");
    const r2 = new RenewalRunner({ account: b.a, link: link2 as never, courier: () => b.agents, me: meOf({ "m-1": true, "m-2": true, "m-3": false }), renewOptions: quick });
    r2.start();
    await r2.idle();
    await new Promise((resolve) => setTimeout(resolve, 20));
    await r2.idle();
    assert.deepEqual(r2.status("m-1"), { kind: "blocked", by: ["m-3"] });
    assert.deepEqual(r2.status("m-2"), { kind: "queued", after: ["m-1"], stuck: [{ host: "m-1", why: "blocked" }] });
    assert.deepEqual(b.agents.rotateCalls, []);
    r2.stop();
  }
});

test("§4.1 overlay: a cycle of three is paused by one start and resumed by one completion once all three renewed", async () => {
  const { a, agents, x } = await account({ chain: false });
  // m-1 lets X and m-3 in; m-3 lets m-2 in; m-2 lets m-1 in: X → m-1 → m-2 → m-3 → m-1.
  await a.setAccept("m-1", { type: "devices", ids: [x.deviceId, "m-3"] });
  await a.setAccept("m-3", { type: "devices", ids: ["m-2"] });
  await a.setAccept("m-2", { type: "devices", ids: ["m-1"] });
  await a.removeDevice(x.deviceId, new FakeCourier());
  const removedAt = a.chain!.seq;
  assert.deepEqual(exposedIds(a.chain!), ["m-1", "m-2", "m-3"]);
  const link = new FakeLink();
  for (const h of ["m-1", "m-2", "m-3"]) link.set(h, "online");
  const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }), renewOptions: quick });
  runner.start();
  await until(() => rotated(a).length === 3 && a.chain!.openPauses().length === 0);
  await runner.idle();
  const written = writtenAfter(a, removedAt);
  assert.equal(written[0], "pause start m-1<m-3,m-2<m-1,m-3<m-2", written.join(" | "));
  assert.deepEqual(written.filter((k) => k.startsWith("pause")), ["pause start m-1<m-3,m-2<m-1,m-3<m-2", "pause completed"], "one start, one completion");
  const lastRenewal = Math.max(written.indexOf("rotate m-1"), written.indexOf("rotate m-2"), written.indexOf("rotate m-3"));
  assert.ok(written.indexOf("pause completed") > lastRenewal);
  assert.ok(admitsOn(a.chain!, "m-1", "m-3") && admitsOn(a.chain!, "m-2", "m-1") && admitsOn(a.chain!, "m-3", "m-2"));
  assert.deepEqual(exposedIds(a.chain!), []);
  runner.stop();
});

test("§4.1 a cycle inside an acyclic part: only the cycle is narrowed; the host it controls renews after it", async () => {
  const { a, agents, x } = await account();
  // m-1 ⇄ m-2 (a cycle), and m-3 lets m-1 control it (acyclic).
  await a.setAccept("m-1", { type: "devices", ids: [x.deviceId, "m-2"] });
  await a.setAccept("m-3", { type: "devices", ids: ["m-1"] });
  await a.removeDevice(x.deviceId, new FakeCourier());
  const removedAt = a.chain!.seq;
  assert.deepEqual(exposedIds(a.chain!), ["m-1", "m-2", "m-3"]);
  const link = new FakeLink();
  for (const h of ["m-1", "m-2", "m-3"]) link.set(h, "online");
  const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }), renewOptions: quick });
  runner.start();
  await until(() => rotated(a).length === 3 && admitsOn(a.chain!, "m-2", "m-1"));
  await runner.idle();
  const written = writtenAfter(a, removedAt);
  assert.equal(written[0], "pause start m-1<m-2,m-2<m-1", written.join(" | "));
  assert.ok(!written.some((k) => k.includes("m-3<")), "m-3's control is never paused");
  assert.ok(written.indexOf("rotate m-3") > written.indexOf("rotate m-1"), "m-3 renews after m-1, which it lets control it");
  assert.equal(admitsOn(a.chain!, "m-3", "m-1"), true);
  assert.deepEqual(exposedIds(a.chain!), []);
  runner.stop();
});

test("§4.1 another admin page joining mid-group reads the chain: it does not narrow again, and restores once, after the last renewal", async () => {
  const { hub, a, agents, x } = await account();
  const b = await pairAs(hub, a, { admin: true });
  await a.setAccept("m-1", { type: "devices", ids: [x.deviceId, "m-2"] });
  await a.removeDevice(x.deviceId, new FakeCourier());
  const removedAt = a.chain!.seq;
  // Page A: narrows, renews m-1 (m-2 is away), and closes.
  const linkA = new FakeLink();
  linkA.set("m-1", "online");
  linkA.set("m-2", "offline");
  const pageA = new RenewalRunner({ account: a, link: linkA as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }), renewOptions: quick });
  pageA.start();
  await until(() => rotated(a).includes("m-1"));
  await pageA.idle();
  pageA.stop();
  assert.deepEqual(writtenAfter(a, removedAt).filter((k) => k.startsWith("pause")), ["pause start m-1<m-2,m-2<m-1"], "one start");
  // Page B, on another browser: m-2 comes back while it is open.
  await b.sync();
  const linkB = new FakeLink();
  linkB.set("m-1", "online");
  linkB.set("m-2", "online");
  const pageB = new RenewalRunner({ account: b, link: linkB as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }), renewOptions: quick });
  pageB.start();
  await until(() => rotated(b).includes("m-2") && admitsOn(b.chain!, "m-1", "m-2"));
  await pageB.idle();
  pageB.stop();
  const after = writtenAfter(b, removedAt);
  assert.deepEqual(after.filter((k) => k.startsWith("pause")), ["pause start m-1<m-2,m-2<m-1", "pause completed"], `paused once, resumed once: ${after.join(" | ")}`);
  assert.ok(after.indexOf("pause completed") > after.indexOf("rotate m-2"), "resumed only after the last renewal");
  assert.deepEqual(exposedIds(b.chain!), []);
});

test("§4.1 a narrowing made by hand is never restored: one list, or lists split across signers, stay narrowed after the renewals", async () => {
  // One list, by hand (say, following "narrow who can control it"): m-2 stops letting m-1 (exposed) control it.
  {
    const { a, agents, x } = await account();
    await a.removeDevice(x.deviceId, new FakeCourier());
    assert.deepEqual(exposedIds(a.chain!), ["m-1", "m-2"]);
    await a.setAccept("m-2", { type: "admins" }, new FakeCourier());
    assert.equal(admitsOn(a.chain!, "m-2", "m-1"), false);
    await a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick });
    await a.renewIdentity("m-2", agents, { assurance: ASSURANCE_AUTO, ...quick });
    assert.deepEqual(exposedIds(a.chain!), []);
    assert.deepEqual(a.chain!.openPauses(), [], "no pause: a person's narrowing");
    const runner = new RenewalRunner({ account: a, link: new FakeLink() as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }), renewOptions: quick });
    runner.start();
    await runner.idle();
    await runner.idle();
    runner.stop();
    assert.equal(a.chain!.acceptPolicy("m-2").type, "admins", "the person's narrowing stands");
  }
  // Two lists, narrowed by two admins: two runs, two signers — neither restored.
  {
    const { hub, a, agents, x } = await account();
    const b = await pairAs(hub, a, { admin: true });
    await a.setAccept("m-1", { type: "devices", ids: [x.deviceId, "m-2"] });
    await a.removeDevice(x.deviceId, new FakeCourier());
    await a.setAccept("m-1", { type: "admins" }, new FakeCourier());
    await b.sync();
    await b.setAccept("m-2", { type: "admins" }, new FakeCourier());
    await a.sync();
    await a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick });
    await a.renewIdentity("m-2", agents, { assurance: ASSURANCE_AUTO, ...quick });
    assert.deepEqual(a.chain!.openPauses(), []);
    const runner = new RenewalRunner({ account: a, link: new FakeLink() as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }), renewOptions: quick });
    runner.start();
    await runner.idle();
    await runner.idle();
    runner.stop();
    assert.equal(a.chain!.acceptPolicy("m-1").type, "admins");
    assert.equal(a.chain!.acceptPolicy("m-2").type, "admins");
  }
});

test("§4.1 overlay: a hand edit during the pause changes only the base; the completion lifts the overlay and leaves the base as the person set it", async () => {
  const { a, agents, x } = await account();
  // m-1 lets X, m-2 and m-3 in; m-2 lets m-1 in: m-1 ⇄ m-2.
  await a.setAccept("m-1", { type: "devices", ids: [x.deviceId, "m-2", "m-3"] });
  await a.removeDevice(x.deviceId, new FakeCourier());
  const link = new FakeLink();
  link.set("m-1", "online");
  link.set("m-2", "offline");
  const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }), renewOptions: quick });
  runner.start();
  await until(() => rotated(a).includes("m-1"));
  await runner.idle();
  const [pause] = a.chain!.openPauses();
  assert.ok(pause);
  assert.deepEqual(pause.removals, [{ host: "m-1", ids: ["m-2"] }, { host: "m-2", ids: ["m-1"] }]);
  // A person widens m-2's base meanwhile: the pause still holds m-1 out; the list shows the base, the pause apart.
  await a.setAccept("m-2", { type: "devices", ids: ["m-1", "m-3"] });
  assert.equal(sameAcceptText(a.chain!.baseAccept("m-2")), sameAcceptText({ type: "devices", ids: ["m-1", "m-3"] }));
  assert.equal(admitsOn(a.chain!, "m-2", "m-1"), false, "still paused");
  assert.deepEqual(a.chain!.pausedEdges("m-2"), ["m-1"]);
  link.set("m-2", "online");
  await until(() => rotated(a).includes("m-2") && a.chain!.openPauses().length === 0);
  await runner.idle();
  assert.equal(a.chain!.pause(pause.op)?.end?.phase, 1, "completed");
  assert.equal(sameAcceptText(a.chain!.baseAccept("m-1")), sameAcceptText({ type: "devices", ids: [x.deviceId, "m-2", "m-3"] }), "m-1's base as it was: nothing ever rewrote it");
  assert.equal(sameAcceptText(a.chain!.baseAccept("m-2")), sameAcceptText({ type: "devices", ids: ["m-1", "m-3"] }), "m-2's base as the person set it");
  assert.equal(admitsOn(a.chain!, "m-2", "m-1"), true);
  assert.deepEqual(runner.pausedWith("m-1"), []);
  runner.stop();
});

test("§4.1 decision A: two back-to-back hand edits by one admin that narrow a whole cycle are never restored — no marker, no restore", async () => {
  const { a, agents, x } = await account();
  await a.setAccept("m-1", { type: "devices", ids: [x.deviceId, "m-2"] });
  await a.removeDevice(x.deviceId, new FakeCourier());
  // One admin, two consecutive entries each shutting the other out: what a runner's narrowing looked like without a marker.
  await a.setAccept("m-1", { type: "admins" }, new FakeCourier());
  await a.setAccept("m-2", { type: "admins" }, new FakeCourier());
  await a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick });
  await a.renewIdentity("m-2", agents, { assurance: ASSURANCE_AUTO, ...quick });
  const runner = new RenewalRunner({ account: a, link: new FakeLink() as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }), renewOptions: quick });
  runner.start();
  await runner.idle();
  await new Promise((resolve) => setTimeout(resolve, 20));
  await runner.idle();
  runner.stop();
  assert.equal(a.chain!.acceptPolicy("m-1").type, "admins", "the person's edits stand");
  assert.equal(a.chain!.acceptPolicy("m-2").type, "admins");
});

test("§4.1 decision A (Codex 2): a later single restriction that matches an old, completed narrowing is never restored", async () => {
  const { a, agents, x } = await account();
  await a.setAccept("m-1", { type: "devices", ids: [x.deviceId, "m-2"] });
  await a.removeDevice(x.deviceId, new FakeCourier());
  const link = new FakeLink();
  link.set("m-1", "online");
  link.set("m-2", "online");
  const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }), renewOptions: quick });
  runner.start();
  await until(() => rotated(a).length === 2 && a.chain!.openPauses().length === 0 && admitsOn(a.chain!, "m-1", "m-2"));
  await runner.idle();
  // Later, a person restricts m-1 to exactly what the narrowing once wrote: it stays restricted.
  await a.setAccept("m-1", { type: "admins" }, new FakeCourier());
  await until(() => rotated(a).length === 3);
  await runner.idle();
  await new Promise((resolve) => setTimeout(resolve, 20));
  await runner.idle();
  runner.stop();
  assert.equal(a.chain!.acceptPolicy("m-1").type, "admins", "not given back");
  assert.equal(admitsOn(a.chain!, "m-1", "m-2"), false);
});

test("renewIdentity: requested with a nonce's hash, the answer checked, written with the next machine key epoch, the key handed over re-encrypting", async () => {
  const { hub, a, agents, x } = await account({ chain: false });
  await a.removeDevice(x.deviceId, new FakeCourier());
  const before = a.chain!.machine("m-1")!.info;
  const epochs = a.chain!.machineKeyEpochs("m-1");
  const report = await a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick });
  const record = a.chain!.machine("m-1")!;
  assert.notEqual(hex(record.info.signKey), hex(before.signKey));
  assert.equal(hex(record.info.signKey), hex(report.signKey));
  assert.equal(record.retired.length, 1);
  assert.equal(hex(record.retired[0]!.signKey), hex(before.signKey));
  assert.equal(record.retired[0]!.byAssured, false);
  // [rotateHostKey, keyEpochs{m-1: next}] in one call.
  const entries = a.chain!.allEntries();
  const at = entries.findIndex((e) => e.draft.action.type === "rotateHostKey");
  const next = entries[at + 1]!.draft.action;
  assert.ok(next.type === "keyEpochs" && next.machines.some((m) => m.machine === "m-1" && m.epoch === epochs));
  assert.equal(report.mkEpoch, epochs);
  const handed = agents.posted.at(-1)!;
  assert.deepEqual([handed.machine, handed.epoch, handed.reencrypt], ["m-1", epochs, true]);
  assert.equal(hub.rotationRows.get("m-1")!.state, "committed");
  assert.ok(hub.rotationPuts.includes("m-1 attempt 0"));
  assert.ok(agents.attests >= 1, "the new head is vouched for");
  assert.match(report.fingerprint, /^[A-Z2-7]{4}(-[A-Z2-7]{4})+$/);
  assert.deepEqual(exposedIds(a.chain!), []);
});

test("renewIdentity refuses an answer that does not check out, and withdraws the attempt: nothing is written", async () => {
  const cases: [FakeAgents["tamper"] | "reuse", string][] = [
    ["oldSig", "signature"],
    ["newSig", "signature"],
    ["mac", "mac"],
    ["view", "view"],
    ["reuse", "reused"],
  ];
  for (const [how, detail] of cases) {
    const { hub, a, agents, x } = await account({ chain: false });
    await a.removeDevice(x.deviceId, new FakeCourier());
    if (how === "reuse") agents.reuse = { signPublic: a.keys.signPublic, dhPublic: a.keys.dhPublic };
    else agents.tamper = how;
    await assert.rejects(a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick }), renewal("badAnswer", detail), String(how));
    assert.deepEqual(rotated(a), [], `${how}: nothing written`);
    assert.deepEqual(hub.rotationDeletes, ["m-1"], `${how}: the attempt withdrawn`);
    assert.deepEqual(exposedIds(a.chain!), ["m-1"], `${how}: still exposed`);
  }
});

test("confirmed on the device: the code must be typed, and be the one its screen shows", async () => {
  const { hub, a, agents, x } = await account({ chain: false });
  await a.removeDevice(x.deviceId, new FakeCourier());
  let asked = 0;
  await assert.rejects(
    a.renewIdentity("m-1", agents, { assurance: ASSURANCE_ON_DEVICE, askCode: async () => (asked++, "AAAA-AAAA"), ...quick }),
    renewal("wrongCode"),
  );
  assert.equal(asked, 1);
  assert.ok(hub.rotationDeletes.includes("m-1"), "a wrong code withdraws the attempt (R6 review)");
  await assert.rejects(a.renewIdentity("m-1", agents, { assurance: ASSURANCE_ON_DEVICE, askCode: async () => null, ...quick }), renewal("cancelled"));
  assert.deepEqual(rotated(a), []);
  // Typed as people type it (lower case, no dash): it goes, as assurance 1.
  await a.renewIdentity("m-1", agents, { assurance: ASSURANCE_ON_DEVICE, askCode: async (h) => agents.sas.get(h)!.toLowerCase().replace("-", " "), ...quick });
  const last = a.chain!.allEntries().find((e) => e.draft.action.type === "rotateHostKey")!.draft.action;
  assert.ok(last.type === "rotateHostKey" && last.rotation.assurance === ASSURANCE_ON_DEVICE);
  assert.equal(a.chain!.machine("m-1")!.retired[0]!.byAssured, true);
});

test("a code prompt nobody answers gives up after `codeWaitMs`: the renewal is cancelled and withdrawn, and the runner reads the hub's renewals again", async () => {
  const { hub, a, agents, x } = await account({ chain: false });
  await a.removeDevice(x.deviceId, new FakeCourier());
  const runner = new RenewalRunner({ account: a, link: new FakeLink() as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }), renewOptions: { ...quick, codeWaitMs: 50 } });
  const late = <T>(p: Promise<T>, ms: number) => Promise.race([p.then((v) => ({ v }), (e: unknown) => ({ e })), new Promise<"late">((resolve) => setTimeout(() => resolve("late"), ms))]);
  let asked = 0;
  // A prompt left open (its panel closed, say): it never answers.
  const renewing = runner.renew("m-1", { assurance: ASSURANCE_ON_DEVICE, askCode: () => (asked++, new Promise<string | null>(() => undefined)) });
  const outcome = await late(renewing, 3_000);
  assert.equal(asked, 1);
  assert.ok(outcome !== "late" && "e" in outcome && outcome.e instanceof RenewalError && outcome.e.kind === "cancelled", `the prompt timed out: ${JSON.stringify(outcome)}`);
  assert.ok(hub.rotationDeletes.includes("m-1"), "the attempt withdrawn");
  assert.deepEqual(rotated(a), []);
  // The queue is not held: a dispute the hub hands on is read and checked as before.
  assert.notEqual(await late(runner.refresh(), 3_000), "late");
  runner.stop();
});

test("renewIdentity waits for the hub's window (409 rotation_not_ready window → its ready_ms), and says why it cannot go otherwise", async () => {
  const { hub, a, agents, x } = await account({ chain: false });
  await a.removeDevice(x.deviceId, new FakeCourier());
  hub.windowRefusals = [Date.now() + 30];
  const slept: number[] = [];
  const sleep = (ms: number) => (slept.push(ms), new Promise<void>((resolve) => setTimeout(resolve, Math.min(ms, 50))));
  await a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick, sleep });
  assert.deepEqual(hub.windowRefusals, [], "refused once for the window");
  assert.ok(slept.some((ms) => ms > 0), `it waited for ready_ms (${slept})`);
  assert.deepEqual(rotated(a), ["m-1"]);

  // An older agent: no `/rh/rotate` (404), or never declared rotate/1 (the hub says so).
  const old = await account({ chain: false });
  await old.a.removeDevice(old.x.deviceId, new FakeCourier());
  old.agents.rotateStatus = { status: 404 };
  await assert.rejects(old.a.renewIdentity("m-1", old.agents, { assurance: ASSURANCE_AUTO, ...quick }), renewal("tooOld"));
  old.hub.rotates.delete("m-1");
  await assert.rejects(old.a.renewIdentity("m-1", old.agents, { assurance: ASSURANCE_AUTO, ...quick }), renewal("tooOld", "agent_too_old"));
  old.hub.rotationEnabled = false;
  await assert.rejects(old.a.renewIdentity("m-2", old.agents, { assurance: ASSURANCE_AUTO, ...quick }), renewal("disabled"));
});

test("a staging the hub voided (another holder of the old key: not_acked) is not written, and asks for a renewal confirmed on the device", async () => {
  const { hub, a, agents, x } = await account({ chain: false });
  await a.removeDevice(x.deviceId, new FakeCourier());
  const answer = agents.rotate.bind(agents);
  agents.rotate = async (m, b) => {
    const r = await answer(m, b);
    // A sign-in with the old key that could not confirm with the new one, within the window: voided, and recorded.
    Object.assign(hub.rotationRows.get(m)!, { state: "pending", failure: { reason: "not_acked", at_ms: Date.now() }, needs_assurance: true });
    hub.disputes.push({ host: m, kind: "not_acked", seen_ms: Date.now() });
    return r;
  };
  await assert.rejects(a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick }), renewal("voided", "not_acked"));
  assert.deepEqual(rotated(a), []);
  const runner = new RenewalRunner({ account: a, link: new FakeLink() as never, courier: () => agents, me: meOf({ "m-1": true }) });
  await runner.refresh();
  await runner.idle();
  assert.deepEqual(runner.status("m-1"), { kind: "needsCode", why: "evidence" });
  runner.stop();
});

test("a dispute statement signed by the retired key isolates the host — checked on this page's chain, not taken on the hub's word — until a renewal confirmed on the device, or its removal", async () => {
  const { hub, a, agents, x, h1 } = await account({ chain: false });
  await a.removeDevice(x.deviceId, new FakeCourier());
  await a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick });
  const seq = a.chain!.deviceRecord("m-1")!.rotatedSeq!;
  const json = await statementJson(a, "m-1", h1.signPrivate);
  const checked = await checkStatement(a.chain!, hostDispute(json)!);
  assert.deepEqual(checked, { host: "m-1", seq, sig: json.sig }, "kept with its signature (R6 review)");
  assert.deepEqual([...isolatedHosts(a.chain!, [checked!])], ["m-1"]);
  // Signed by the new key, by another device, about another entry, or not a signature: nothing.
  const current = agents.current("m-1")!;
  assert.equal(await checkStatement(a.chain!, hostDispute(await statementJson(a, "m-1", current.sign))!), null, "the new key");
  assert.equal(await checkStatement(a.chain!, hostDispute(await statementJson(a, "m-1", a.keys.sign.privateKey))!), null, "another device");
  assert.equal(await checkStatement(a.chain!, hostDispute({ ...json, seq: seq - 1 })!), null, "not a renewal");
  assert.equal(await checkStatement(a.chain!, hostDispute({ ...json, sig: hex(new Uint8Array(64).fill(7)) })!), null, "not its signature");
  assert.equal(await checkStatement(a.chain!, hostDispute({ ...json, host: "m-2" })!), null, "another host");

  // The runner: isolated on every page that reads it — no keys, no other host's list, renewal only with the code.
  hub.disputes.push(json);
  const runner = new RenewalRunner({ account: a, link: new FakeLink() as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }) });
  await runner.refresh();
  await runner.idle();
  assert.equal(a.isolated("m-1"), true);
  assert.equal(a.keysWithheldFrom("m-1"), true);
  assert.deepEqual(runner.status("m-1"), { kind: "isolated" });
  await assert.rejects(a.setAccept("m-3", { type: "devices", ids: ["m-1"] }), (e: unknown) => e instanceof AccountError && e.kind === "exposed");
  await assert.rejects(a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick }), renewal("needsAssurance"));
  const issued = await a.issueMachineKey("m-1", agents);
  assert.equal(issued.result.via, "refused", "an isolated host is handed no key");

  // Confirmed on the device: released.
  await runner.renew("m-1", { assurance: ASSURANCE_ON_DEVICE, askCode: async (h) => agents.sas.get(h)! });
  await runner.refresh();
  await runner.idle();
  assert.equal(a.isolated("m-1"), false);
  assert.deepEqual([...isolatedHosts(a.chain!, [checked!])], [], "an assurance-1 renewal after the disputed one");
  // A statement disputing that renewal (confirmed on the device) isolates nothing (R6 review).
  const assured = await checkStatement(a.chain!, hostDispute(await statementJson(a, "m-1", current.sign))!);
  assert.deepEqual({ host: assured?.host, seq: assured?.seq }, { host: "m-1", seq: a.chain!.deviceRecord("m-1")!.rotatedSeq! });
  assert.deepEqual([...isolatedHosts(a.chain!, [assured!])], []);
  // Removal ends it too, whatever the hub says.
  await a.removeDevice("m-1", new FakeCourier());
  assert.deepEqual([...isolatedHosts(a.chain!, [checked!])], []);
  runner.stop();
});

test("a sign-in with a retired key is information only — never isolation, whenever it was signed", async () => {
  const { hub, a, agents, x, h1 } = await account({ chain: false });
  await a.removeDevice(x.deviceId, new FakeCourier());
  await a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick });
  const retiredSeq = a.chain!.deviceRecord("m-1")!.rotatedSeq!;
  const renewedAt = a.chain!.entryAt(retiredSeq)!.draft.createdMs;
  // No time rule any more: before or after the renewal, its signature is all that is checked — and it isolates nothing.
  for (const t of [renewedAt - 60_000, renewedAt + 3_600_000]) {
    assert.deepEqual(await checkRetiredLogin(a.chain!, hostDispute(await disputeJson(a, "m-1", h1.signPrivate, h1.sign, retiredSeq, t))!), { host: "m-1", retiredSeq, timeMs: t });
  }
  assert.equal(await checkRetiredLogin(a.chain!, hostDispute({ ...(await disputeJson(a, "m-1", h1.signPrivate, h1.sign, retiredSeq)), sig: hex(new Uint8Array(64)) })!), null);
  hub.disputes.push(await disputeJson(a, "m-1", h1.signPrivate, h1.sign, retiredSeq, renewedAt + 3_600_000));
  const runner = new RenewalRunner({ account: a, link: new FakeLink() as never, courier: () => agents, me: meOf({ "m-1": true }) });
  await runner.refresh();
  await runner.idle();
  assert.equal(a.isolated("m-1"), false, "a sign-in never isolates");
  assert.deepEqual(a.keptDisputes(), [], "nor is it kept as evidence");
  assert.equal(runner.retiredLoginAt("m-1"), renewedAt + 3_600_000, "shown as information");
  assert.notEqual(runner.status("m-1").kind, "isolated");
  runner.stop();
});

test("a host waiting to renew, or isolated, is sealed no host keys (purpose 5, no answer to purpose 4); one that cannot renew still is", async () => {
  const { hub, a, x } = await account();
  hub.signInHost("m-1");
  await a.removeDevice(x.deviceId, new FakeCourier());
  // Not renewable here (the runner said nothing): m-1 still gets m-2's keys.
  assert.deepEqual(a.hostKeyRecipients("m-2").map((d) => d.info.id), ["m-1"]);
  // Renewable and exposed: none.
  a.noteRenewals({ renews: new Set(["m-1", "m-2", "m-3"]), pending: new Set(), isolated: new Set() });
  assert.equal(a.keysWithheldFrom("m-1"), true);
  assert.deepEqual(a.hostKeyRecipients("m-2"), []);
  const mailed = hub.mailLog.length;
  await a.issueMachineKey("m-2", new FakeCourier());
  assert.equal(hub.mailLog.slice(mailed).filter((m) => m.recipient === "m-1" && m.purpose === 5).length, 0);
  assert.equal(await a.answerHostKeyRequest("m-1"), 0);
  // Pending at the hub alone is enough.
  a.noteRenewals({ renews: new Set(), pending: new Set(["m-1"]), isolated: new Set() });
  assert.equal(a.keysWithheldFrom("m-1"), true);
});

test("an exposed host that can renew is let control no other host until it did; the re-enroll warning stays only for hosts that cannot renew", async () => {
  const { a, x } = await account({ chain: false });
  await a.removeDevice(x.deviceId, new FakeCourier());
  assert.deepEqual([...a.reEnrollWarnings().keys()], ["m-1"], "the re-enroll warning while it cannot renew");
  a.noteRenewals({ renews: new Set(["m-1", "m-2", "m-3"]), pending: new Set(), isolated: new Set() });
  assert.deepEqual([...a.reEnrollWarnings().keys()], [], "it renews instead");
  assert.equal(a.mustRenewFirst("m-1"), true);
  await assert.rejects(a.setAccept("m-2", { type: "devices", ids: ["m-1"] }), (e: unknown) => {
    assert.ok(e instanceof AccountError && e.kind === "exposed", String(e));
    assert.equal(e.vars?.ids, "m-1");
    return true;
  });
  assert.equal(a.chain!.acceptPolicy("m-2").type, "admins", "nothing written");
  // Another host that is not exposed goes.
  await a.setAccept("m-2", { type: "devices", ids: ["m-3"] });
});

test("the runner: marks each exposed host pending, renews one at a time — the one others depend on first", async () => {
  const { hub, a, agents, x } = await account();
  await a.removeDevice(x.deviceId, new FakeCourier());
  const link = new FakeLink();
  link.set("m-1", "online");
  link.set("m-2", "online");
  link.set("m-3", "online");
  const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }), renewOptions: quick });
  runner.start();
  await until(() => rotated(a).length === 2);
  await runner.idle();
  assert.deepEqual(rotated(a), ["m-1", "m-2"], "m-1 first: m-2 lets it control it");
  assert.deepEqual(agents.rotateCalls, ["m-1", "m-2"]);
  assert.equal(agents.maxActive, 1, "one at a time");
  assert.ok(hub.rotationPuts.indexOf("m-1 pending 0") < hub.rotationPuts.indexOf("m-1 attempt 0"));
  assert.ok(hub.rotationPuts.includes("m-2 pending 0"), "every exposed host marked first");
  assert.deepEqual(exposedIds(a.chain!), []);
  assert.equal(runner.status("m-1").kind, "renewed");
  assert.equal(runner.status("m-3").kind, "none");
  runner.stop();
});

test("the runner: an offline host is renewed once it is back; a failing one is tried again after each delay, then the person is asked", async () => {
  const { a, agents, x } = await account({ chain: false });
  await a.removeDevice(x.deviceId, new FakeCourier());
  const link = new FakeLink();
  link.set("m-1", "offline");
  const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true }), renewOptions: quick, retryMs: [5, 10, 15] });
  runner.start();
  await runner.idle();
  await runner.idle();
  assert.deepEqual(agents.rotateCalls, [], "not while offline");
  assert.deepEqual(runner.status("m-1"), { kind: "offline" });
  assert.deepEqual(runner.notice(), { renewing: ["m-1"], needYou: [] });
  // Back, but its channel fails: 1 + 3 tries, then it asks.
  agents.rotateError = new Error("the device did not answer");
  link.set("m-1", "online");
  await until(() => runner.status("m-1").kind === "stuck");
  assert.equal(agents.rotateCalls.length, 4);
  assert.deepEqual(runner.notice(), { renewing: [], needYou: ["m-1"] });
  // The person tries again, and it goes.
  agents.rotateError = null;
  await runner.renew("m-1", { assurance: ASSURANCE_AUTO });
  assert.deepEqual(rotated(a), ["m-1"]);
  assert.equal(runner.status("m-1").kind, "renewed");
  runner.stop();
});

test("the runner: an older agent is not tried again; the account's setting asks for the code; a v2 account keeps the re-enroll path", async () => {
  const { hub, a, agents, x } = await account({ chain: false });
  await a.removeDevice(x.deviceId, new FakeCourier());
  const link = new FakeLink();
  link.set("m-1", "online");
  agents.rotateStatus = { status: 404 };
  const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true }), renewOptions: quick });
  runner.start();
  await until(() => runner.status("m-1").kind === "tooOld");
  await runner.idle();
  runner.stop();
  assert.equal(agents.rotateCalls.length, 1);
  // Declared nothing: too old, and it is never asked.
  const declared = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": false }), renewOptions: quick });
  await declared.refresh();
  assert.deepEqual(declared.status("m-1"), { kind: "tooOld" });
  assert.equal(agents.rotateCalls.length, 1);
  assert.ok(!hub.rotationPuts.some((p) => p.startsWith("m-1") && p.endsWith("pending 1")));
  // 「总在设备上确认」: nothing goes on its own.
  const other = await account({ chain: false });
  await other.a.removeDevice(other.x.deviceId, new FakeCourier());
  // The account's policy, on the chain (setRenewalPolicy).
  await other.a.setRenewalPolicy(true);
  const l2 = new FakeLink();
  l2.set("m-1", "online");
  const r2 = new RenewalRunner({ account: other.a, link: l2 as never, courier: () => other.agents, me: meOf({ "m-1": true }), renewOptions: quick });
  r2.start();
  await r2.idle();
  await r2.idle();
  assert.deepEqual(other.agents.rotateCalls, []);
  assert.deepEqual(r2.status("m-1"), { kind: "needsCode", why: "policy" });
  assert.ok(other.hub.rotationPuts.includes("m-1 pending 1"), "marked, as a renewal confirmed on the device");
  await r2.setAlwaysAssured(false);
  await until(() => rotated(other.a).length === 1);
  r2.stop();
});

test("a v2 account renews nothing: the re-enroll warning stays", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  await hub.enroll(a, "m-1");
  hub.rotates.add("m-1");
  a.noteRenewals({ renews: new Set(["m-1"]), pending: new Set(), isolated: new Set() });
  assert.equal(a.renews("m-1"), false, "never on a v2 chain");
  await assert.rejects(a.renewIdentity("m-1", new FakeAgents(hub, () => a.chain), { assurance: ASSURANCE_AUTO, ...quick }), (e: unknown) => e instanceof AccountError && e.kind === "notUpgraded");
});

/** A row another admin page left at the hub (its tab closed in the window, or its withdrawal never arrived). */
const leftBehind = (host: string, state: "staged" | "attempt", ageMs: number) => {
  const now = Date.now();
  return state === "staged"
    ? { host, state, assurance: 0, requested_ms: now - ageMs - 120_000, requested_by: "d-gone", nonce_hash: "00".repeat(32), staged_ms: now - ageMs - 120_000, ready_ms: now - ageMs, sign_key: "11".repeat(32), dh_key: "22".repeat(32), staged_seq: 1, needs_assurance: false, isolated: false, failure: null }
    : { host, state, assurance: 0, requested_ms: now - 600_000 - ageMs, requested_by: "d-gone", nonce_hash: "00".repeat(32), attempt_expires_ms: now - ageMs, needs_assurance: false, isolated: false, failure: null };
};

test("an attempt or staging nobody will finish is withdrawn and the renewal started again; one still live elsewhere is left alone (§4.2)", async () => {
  // Abandoned: a staging whose window closed more than ABANDONED_MS ago — the hub refuses any new nonce while it stands.
  for (const row of [leftBehind("m-1", "staged", ABANDONED_MS + 60_000), leftBehind("m-1", "attempt", 1_000)]) {
    const { hub, a, agents, x } = await account({ chain: false });
    await a.removeDevice(x.deviceId, new FakeCourier());
    hub.rotationRows.set("m-1", { ...row });
    if (row.state === "staged") await assert.rejects(a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick }), renewal("inProgress"));
    const link = new FakeLink();
    link.set("m-1", "online");
    const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true }), renewOptions: quick });
    runner.start();
    await until(() => rotated(a).includes("m-1"));
    await runner.idle();
    assert.ok(hub.rotationDeletes.includes("m-1"), `${row.state}: withdrawn first`);
    assert.equal(runner.status("m-1").kind, "renewed");
    runner.stop();
  }
  // Live: another page's staging still in (or just past) its window — renewing elsewhere, not touched.
  const { hub, a, agents, x } = await account({ chain: false });
  await a.removeDevice(x.deviceId, new FakeCourier());
  hub.rotationRows.set("m-1", leftBehind("m-1", "staged", 60_000));
  const link = new FakeLink();
  link.set("m-1", "online");
  const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true }), renewOptions: quick });
  runner.start();
  await runner.idle();
  await runner.idle();
  assert.deepEqual(runner.status("m-1"), { kind: "renewing", elsewhere: true });
  assert.deepEqual(agents.rotateCalls, []);
  assert.deepEqual(hub.rotationDeletes, []);
  runner.stop();
});

test("a renewal the hub committed that this page's chain lacks: synced first, the host never marked again", async () => {
  const { hub, a, agents, x } = await account({ chain: false });
  const b = await pairAs(hub, a, { admin: true });
  await a.removeDevice(x.deviceId, new FakeCourier());
  await b.sync();
  await a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick });
  // b has not heard yet: on its chain m-1 is still exposed.
  assert.deepEqual(exposedIds(b.chain!), ["m-1"]);
  const before = hub.rotationPuts.length;
  const link = new FakeLink();
  link.set("m-1", "online");
  const runner = new RenewalRunner({ account: b, link: link as never, courier: () => agents, me: meOf({ "m-1": true }), renewOptions: quick });
  runner.start();
  await runner.idle();
  await runner.idle();
  assert.deepEqual(exposedIds(b.chain!), [], "synced");
  assert.deepEqual(hub.rotationPuts.slice(before), [], "never marked: it would have been set back to pending");
  assert.equal(hub.rotationRows.get("m-1")!.state, "committed");
  assert.equal(runner.status("m-1").kind, "renewed");
  runner.stop();
});

test("a row pending at the hub for a host nothing exposes any more is withdrawn — judged on the hub's head read just then, never on a lagging chain", async () => {
  const { hub, a, agents, x } = await account({ chain: false });
  const b = await pairAs(hub, a, { admin: true });
  const pending = () => ({ host: "m-2", state: "pending", assurance: 0, requested_ms: Date.now(), requested_by: a.deviceId, needs_assurance: false, isolated: false, failure: null });
  // Another page marked m-2 for an exposure b has not seen: X, which m-2 lets control it, removed after b last synced.
  await a.setAccept("m-2", { type: "devices", ids: [x.deviceId] });
  await b.sync();
  await a.removeDevice(x.deviceId, new FakeCourier());
  hub.rotationRows.set("m-2", pending());
  // Whatever an old /v1/me said (as new as b's chain), the hub is read again: b lags, so the mark stays.
  const stale = new RenewalRunner({ account: b, link: new FakeLink() as never, courier: () => agents, me: meOf({ "m-2": true }, b.chain!.seq), renewOptions: quick });
  stale.start();
  await stale.idle();
  await stale.idle();
  assert.deepEqual(hub.rotationDeletes, [], "another page's mark for a newer exposure is left alone");
  assert.equal(hub.rotationRows.get("m-2")!.state, "pending");
  stale.stop();
  // A host nothing exposes, on a chain as new as the hub's: withdrawn, its device role back.
  hub.rotationRows.set("m-3", { ...pending(), host: "m-3" });
  const runner = new RenewalRunner({ account: a, link: new FakeLink() as never, courier: () => agents, me: meOf({ "m-2": true, "m-3": true }), renewOptions: quick });
  runner.start();
  await until(() => hub.rotationDeletes.includes("m-3"));
  await runner.idle();
  assert.deepEqual(hub.rotationDeletes, ["m-3"], "m-2 stays: it is exposed");
  assert.equal(hub.rotationRows.has("m-3"), false);
  runner.stop();
});

test("an attempt withdrawn while it waits to be staged fails at once (no ten-minute wait), and is tried again with the usual backoff", async () => {
  const { hub, a, agents, x } = await account({ chain: false });
  await a.removeDevice(x.deviceId, new FakeCourier());
  // The agent answers but stages nothing; meanwhile the attempt is withdrawn (another page, the hub).
  agents.stageAtHub = false;
  const answer = agents.rotate.bind(agents);
  agents.rotate = async (m, body) => {
    const r = await answer(m, body);
    hub.rotationRows.delete(m);
    return r;
  };
  const slept: number[] = [];
  const sleep = async (ms: number) => void slept.push(ms);
  await assert.rejects(a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick, pollMs: 60_000, sleep }), renewal("voided", "withdrawn"));
  assert.deepEqual(slept, [], "not polled for");
  assert.deepEqual(rotated(a), []);
  const link = new FakeLink();
  link.set("m-1", "online");
  const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true }), renewOptions: { ...quick, sleep }, retryMs: [60_000, 60_000, 60_000] });
  runner.start();
  await until(() => runner.status("m-1").kind === "retrying");
  await runner.idle();
  assert.equal(agents.rotateCalls.length, 2, "one more try, then the wait");
  runner.stop();
});

test("the hub's refusals of a renewal: conflict and window are tried again later, v2 goes the re-enroll way, disabled is disabled", async () => {
  const refusal = (reason: string) => renewalRefusal(new HubApiError(409, "rotation_not_ready", "no", { reason })) as RenewalError;
  assert.deepEqual([refusal("conflict").kind, refusal("conflict").detail], ["refused", "conflict"]);
  assert.deepEqual([refusal("window").kind, refusal("window").detail], ["refused", "window"]);
  assert.deepEqual([refusal("v2").kind, refusal("v2").detail], ["refused", "v2"]);
  assert.equal(refusal("disabled").kind, "disabled");
  assert.equal(refusal("agent_too_old").kind, "tooOld");
  // The window refused past the page's tries: a later retry, not a new staged seed at once.
  const { hub, a, agents, x } = await account({ chain: false });
  await a.removeDevice(x.deviceId, new FakeCourier());
  hub.windowRefusals = Array.from({ length: 6 }, () => Date.now() + 1);
  const link = new FakeLink();
  link.set("m-1", "online");
  const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true }), renewOptions: { ...quick, sleep: async () => undefined }, retryMs: [60_000, 60_000, 60_000] });
  runner.start();
  await until(() => runner.status("m-1").kind === "retrying");
  await runner.idle();
  assert.equal(agents.rotateCalls.length, 1, "one staged seed, then the wait");
  runner.stop();
  // A `rotation` push saying `none` (a withdrawal) is one too.
  assert.deepEqual(rotationPush({ t: "rotation", host: "m-1", state: "none" }), { host: "m-1", state: "none", reason: null });
});

// ------------------------------------------------------------ the R6 security review

test("R6 review: the hub's isolated flag alone isolates nothing — keys are withheld and only the code renews it, but no “remove it first”", async () => {
  const { hub, a, agents } = await account({ chain: false });
  // The hub says m-1 is isolated, and shows no evidence this page can check.
  hub.rotationRows.set("m-1", { host: "m-1", state: "committed", assurance: 0, requested_ms: Date.now(), requested_by: "d-gone", committed_ms: Date.now(), committed_seq: null, needs_assurance: false, isolated: true, failure: null });
  const runner = new RenewalRunner({ account: a, link: new FakeLink() as never, courier: () => agents, me: meOf({ "m-1": true }) });
  await runner.refresh();
  await runner.idle();
  assert.equal(a.isolated("m-1"), false, "no evidence checked here");
  assert.equal(a.flagged("m-1"), true);
  assert.equal(a.keysWithheldFrom("m-1"), true, "withholding keys from it is fine");
  assert.equal(a.mustRenewFirst("m-1"), true, "but let control no other host (R6 review)");
  assert.notEqual(runner.status("m-1").kind, "isolated", "not the isolated state, with its removal first");
  assert.equal(runner.needsCode("m-1"), "evidence", "renewed only confirmed on the device");
  runner.stop();
});

test("R6 review: rows' ages are judged by the hub's clock (now_ms), never the page's", async () => {
  // The page's clock two hours ahead: another page's staging still live by the hub is not abandoned.
  {
    const { hub, a, agents, x } = await account({ chain: false });
    await a.removeDevice(x.deviceId, new FakeCourier());
    hub.rotationRows.set("m-1", leftBehind("m-1", "staged", 60_000));
    const link = new FakeLink();
    link.set("m-1", "online");
    const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true }), renewOptions: quick, now: () => Date.now() + 2 * 3_600_000 });
    runner.start();
    await runner.idle();
    await runner.idle();
    assert.deepEqual(runner.status("m-1"), { kind: "renewing", elsewhere: true });
    assert.deepEqual(hub.rotationDeletes, []);
    runner.stop();
  }
  // The hub's clock past the abandon margin while the page's says it is fresh: abandoned, withdrawn, renewed.
  {
    const { hub, a, agents, x } = await account({ chain: false });
    await a.removeDevice(x.deviceId, new FakeCourier());
    hub.rotationRows.set("m-1", leftBehind("m-1", "staged", 60_000));
    hub.nowMsOffset = ABANDONED_MS + 120_000;
    const link = new FakeLink();
    link.set("m-1", "online");
    const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true }), renewOptions: quick });
    runner.start();
    await until(() => rotated(a).includes("m-1"));
    assert.ok(hub.rotationDeletes.includes("m-1"));
    runner.stop();
  }
});

test("R6 review: a staging voided as interrupted or replaced is tried again with a fresh nonce — never the code; not_acked still asks for it", async () => {
  for (const reason of ["interrupted", "replaced"]) {
    const { hub, a, agents, x } = await account({ chain: false });
    await a.removeDevice(x.deviceId, new FakeCourier());
    const answer = agents.rotate.bind(agents);
    let voids = 1;
    agents.rotate = async (m, body) => {
      const r = await answer(m, body);
      if (voids-- > 0) Object.assign(hub.rotationRows.get(m)!, { state: "pending", failure: { reason, at_ms: Date.now() } });
      return r;
    };
    const link = new FakeLink();
    link.set("m-1", "online");
    const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true }), renewOptions: quick, retryMs: [5, 5, 5] });
    runner.start();
    await until(() => rotated(a).includes("m-1"));
    await runner.idle();
    assert.equal(agents.rotateCalls.length, 2, `${reason}: once more, with a fresh nonce`);
    assert.equal(new Set(hub.rotationPuts.filter((p) => p.startsWith("m-1 attempt"))).size >= 1, true);
    assert.equal(runner.needsCode("m-1"), null, `${reason}: never the code`);
    runner.stop();
  }
});

test("R6 review: a host that lets an isolated host control it says to remove that one first", async () => {
  const { hub, a, agents, x, h1 } = await account();
  await a.removeDevice(x.deviceId, new FakeCourier());
  await a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick });
  hub.disputes.push(await isolatingEvidence(a, "m-1", h1));
  const runner = new RenewalRunner({ account: a, link: new FakeLink() as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }) });
  await runner.refresh();
  await runner.idle();
  assert.equal(a.isolated("m-1"), true);
  assert.deepEqual(runner.isolatedControllers("m-2"), ["m-1"], "m-2 lets m-1 control it");
  assert.deepEqual(runner.isolatedControllers("m-3"), []);
  runner.stop();
});

/** A browser's push side, played: its worker's subscription, the permission, its storage, and the device's push routes. */
function pushEnv(over: { permission?: NotificationPermission; vapid?: string; subscribeFails?: boolean } = {}) {
  const values = new Map<string, string>();
  const log: string[] = [];
  let vapid = over.vapid ?? "BBBB";
  const sub = (endpoint: string) => ({
    endpoint,
    unsubscribe: async () => (log.push(`unsubscribe ${endpoint}`), true),
    toJSON: () => ({ endpoint, keys: { p256dh: "p", auth: "a" } }),
  });
  let current: ReturnType<typeof sub> | null = sub("https://push.example/old");
  const env: PushEnv = {
    ready: async () =>
      ({
        pushManager: {
          getSubscription: async () => current,
          subscribe: async () => {
            if (over.subscribeFails) throw new DOMException("needs a user gesture", "NotAllowedError");
            current = sub("https://push.example/new");
            log.push("subscribe");
            return current;
          },
        },
      }) as never,
    permission: () => over.permission ?? "granted",
    storage: { getItem: (k) => values.get(k) ?? null, setItem: (k, v) => void values.set(k, v), removeItem: (k) => void values.delete(k) },
    api: () => ({
      GET: async () => ({ data: { publicKey: vapid } }),
      POST: async (_path, init) => (log.push(`post ${init.body.endpoint}`), {}),
    }),
  };
  return { env, log, values, setVapid: (v: string) => void (vapid = v) };
}

test("R6 review: a renewal of the device this browser's notifications come from drops the old subscription and subscribes again against its new VAPID key", async () => {
  const subscribed = (seq: number | null = null) => JSON.stringify({ machine: "m-1", vapid: "AAAA", seq });
  // Notifications allowed: no prompt, no tap.
  const ok = pushEnv();
  ok.values.set("rh.hub.push.subscribed", subscribed());
  assert.equal(await followRenewedKey("m-1", 12, ok.env), "renewed");
  assert.deepEqual(ok.log, ["unsubscribe https://push.example/old", "subscribe", "post https://push.example/new"]);
  assert.deepEqual(readPushRecord(ok.env), { machine: "m-1", vapid: "BBBB", seq: 12 });
  assert.equal(await followRenewedKey("m-1", 12, ok.env), "none", "once per renewal");
  // Another device's renewal, or the same VAPID key: nothing dropped.
  const other = pushEnv({ vapid: "AAAA" });
  other.values.set("rh.hub.push.subscribed", subscribed());
  assert.equal(await followRenewedKey("m-2", 12, other.env), "none");
  assert.equal(await followRenewedKey("m-1", 12, other.env), "current");
  assert.deepEqual(other.log, []);
  // Not allowed now, or the browser wants a tap to subscribe: the old one goes all the same, and a tap is asked for.
  for (const env of [pushEnv({ permission: "default" }), pushEnv({ subscribeFails: true })]) {
    env.values.set("rh.hub.push.subscribed", subscribed());
    assert.equal(await followRenewedKey("m-1", 12, env.env), "needsTap");
    assert.ok(env.log.includes("unsubscribe https://push.example/old"), "the push service drops the old endpoint");
    assert.equal(pushRenewalNeeded(env.env), "m-1");
    assert.equal(readPushRecord(env.env), null);
  }
  // Followed from the account's chain: a renewal it learns of is acted on.
  const watched = pushEnv();
  watched.values.set("rh.hub.push.subscribed", subscribed(5));
  let rotatedSeq: number | null = 5;
  const listeners = new Set<() => void>();
  const off = followPushRenewals({ chain: { machine: () => ({ rotatedSeq }) }, onChange: (fn) => (listeners.add(fn), () => listeners.delete(fn)) }, watched.env);
  assert.deepEqual(watched.log, [], "nothing new at 5");
  rotatedSeq = 9;
  for (const fn of listeners) fn();
  await until(() => watched.log.includes("subscribe"));
  assert.equal(readPushRecord(watched.env)?.seq, 9);
  off();
});

test("isolation this page verified stays — the hub omitting the evidence, a 404, a reload or a stale tab do not undo it; only an assured renewal or the removal does", async () => {
  const { hub, a, agents, x, h1 } = await account({ chain: false });
  await a.removeDevice(x.deviceId, new FakeCourier());
  await a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick });
  hub.disputes.push(await isolatingEvidence(a, "m-1", h1));
  const runner = new RenewalRunner({ account: a, link: new FakeLink() as never, courier: () => agents, me: meOf({ "m-1": true }) });
  await runner.refresh();
  await runner.idle();
  assert.equal(a.isolated("m-1"), true, "verified here");
  // The hub drops the evidence, then answers 404: still isolated, and still sealed nothing.
  hub.disputes = [];
  for (const row of hub.rotationRows.values()) row.isolated = false;
  await runner.refresh();
  await runner.idle();
  assert.equal(a.isolated("m-1"), true, "the hub omitting it does not clear it");
  hub.rotationsGone = true;
  await runner.refresh();
  await runner.idle();
  assert.equal(a.isolated("m-1"), true, "nor a 404");
  assert.equal(a.keysWithheldFrom("m-1"), true);
  runner.stop();
  // A reload of the same browser: isolated before any runner has read the hub.
  const again = await hub.reload(a);
  assert.equal(again.isolated("m-1"), true, "known before a channel or a key");
  // A stale tab — another page of this browser that never saw the evidence — refreshing on a hub that withholds it.
  hub.rotationsGone = false;
  const stale = await hub.reload(a);
  const staleRunner = new RenewalRunner({ account: stale, link: new FakeLink() as never, courier: () => agents, me: meOf({ "m-1": true }) });
  await staleRunner.refresh();
  await staleRunner.idle();
  staleRunner.stop();
  assert.equal((await hub.reload(a)).isolated("m-1"), true, "a stale tab does not erase it");
  // Only a renewal confirmed on the device, after the disputed one, on this page's own chain clears it.
  const confirm = new RenewalRunner({ account: again, link: new FakeLink() as never, courier: () => agents, me: meOf({ "m-1": true }) });
  await confirm.refresh();
  await confirm.renew("m-1", { assurance: ASSURANCE_ON_DEVICE, askCode: async (h) => agents.sas.get(h)! });
  confirm.stop();
  assert.equal(again.isolated("m-1"), false);
  assert.equal((await hub.reload(a)).isolated("m-1"), false, "and stays cleared after a reload");
});

test("the account's policy lives on the chain — written as setRenewalPolicy, read back from the verified chain, and while on no automatic renewal is signed", async () => {
  const { hub, a, agents, x } = await account({ chain: false });
  await a.removeDevice(x.deviceId, new FakeCourier());
  assert.equal(a.alwaysAssured, false, "off until an admin turns it on");
  await a.setRenewalPolicy(true);
  const last = a.chain!.allEntries().at(-1)!.draft.action;
  assert.deepEqual(last, { type: "setRenewalPolicy", alwaysAssured: true });
  // Another page of the account reads it off its own verified chain — not from the hub.
  const other = await hub.reload(a);
  assert.equal(other.alwaysAssured, true);
  await assert.rejects(a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick }), renewal("needsAssurance", "policy"));
  assert.deepEqual(rotated(a), []);
  await a.renewIdentity("m-1", agents, { assurance: ASSURANCE_ON_DEVICE, askCode: async (h) => agents.sas.get(h)!, ...quick });
  assert.deepEqual(rotated(a), ["m-1"], "confirmed on the device, it goes");
  // Off again: automatic renewals are signed again.
  await a.setRenewalPolicy(false);
  assert.equal(a.alwaysAssured, false);
  await a.setRenewalPolicy(false);
  // The first choice (automatic, made with the account), on, off: setting it to what it is writes nothing.
  assert.equal(a.chain!.allEntries().filter((e) => e.draft.action.type === "setRenewalPolicy").length, 3, "setting it to what it is writes nothing");
});

test("this device's own guard — on it, no automatic renewal is signed here, whatever the chain or the hub says", async () => {
  const { hub, a, agents, x } = await account({ chain: false });
  await a.removeDevice(x.deviceId, new FakeCourier());
  a.setLocalAssured(true);
  assert.equal(a.alwaysAssured, false, "the account's policy is off");
  await assert.rejects(a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick }), renewal("needsAssurance", "local"));
  // Kept in this browser: a reload holds to it; another browser of the account does not.
  assert.equal((await hub.reload(a)).localAssured(), true);
  const b = await pairAs(hub, a, { admin: true });
  assert.equal(b.localAssured(), false);
  // The runner here waits for the code, and marks the host as a renewal confirmed on the device.
  const link = new FakeLink();
  link.set("m-1", "online");
  const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true }), renewOptions: quick });
  runner.start();
  await runner.idle();
  await runner.idle();
  assert.deepEqual(runner.status("m-1"), { kind: "needsCode", why: "device" });
  assert.deepEqual(agents.rotateCalls, []);
  assert.ok(hub.rotationPuts.includes("m-1 pending 1"));
  runner.stop();
  a.setLocalAssured(false);
  assert.equal(a.localAssured(), false);
});

test("hub JSON: rows, disputes and pushes parse as the contract says; anything else is left out", () => {
  const d = hostDispute({ host: "m-1", kind: "retired_login", seen_ms: 5, retired_seq: 9, role: "device", login: { account: "a", id: "m-1", hub: "h", nonce: "00".repeat(32), time_ms: 3 }, sig: "11".repeat(64), sign_key: "22".repeat(32) })!;
  assert.equal(d.retiredSeq, 9);
  assert.equal(d.role, "device");
  assert.equal(hex(d.login!.nonce), "00".repeat(32));
  assert.equal(hex(d.signKey!), "22".repeat(32));
  assert.equal(hostDispute({ host: "m-1", kind: "who knows" }), null);
  const statement = hostDispute({ host: "m-1", kind: "disputed", seen_ms: 7, seq: 12, entry_hash: "33".repeat(32), sig: "44".repeat(64) })!;
  assert.deepEqual([statement.kind, statement.seq, hex(statement.entryHash!), hex(statement.sig!)], ["disputed", 12, "33".repeat(32), "44".repeat(64)]);
  assert.deepEqual(fromHex(hex(d.sig!)).length, 64);
});

/** Another tab of the same browser as `a`: its storage (`storage` to fail writes in), the same device, its own cross-tab end. */
async function tabOf(a: CloudAccount, crossTab: CrossTab | null, storage = browserStorage(a.store)) {
  const tab = await CloudAccount.open(a.api, a.session, a.store, { origin: HUB_ORIGIN, storage, crossTab });
  await tab.sync();
  return tab;
}

/** A BroadcastChannel among tabs: what one posts, the others hear (never itself). */
function tabsBus() {
  const ends = new Set<(m: unknown) => void>();
  return (): CrossTab => {
    let mine: ((m: unknown) => void) | null = null;
    return {
      post: (m) => {
        for (const fn of [...ends]) if (fn !== mine) fn(structuredClone(m));
      },
      subscribe: (fn) => {
        mine = fn;
        ends.add(fn);
        return () => ends.delete(fn);
      },
    };
  };
}

/** Storage that refuses to write the kept disputes (full, or not allowed). */
function refusingDisputes(base: ReturnType<typeof browserStorage>) {
  return {
    getItem: (k: string) => base.getItem(k),
    setItem: (k: string, v: string) => {
      if (k.startsWith("rh.hub.renewal.disputes.")) throw new Error("QuotaExceededError");
      base.setItem(k, v);
    },
    removeItem: (k: string) => base.removeItem(k),
  };
}

test("Codex 1: kept disputes are one per renewal; copies, or many others, never evict one that still isolates; a failed write or another tab loses none", async () => {
  const { a, agents, x, h1, h2 } = await account();
  await a.removeDevice(x.deviceId, new FakeCourier());
  await a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick });
  await a.renewIdentity("m-2", agents, { assurance: ASSURANCE_AUTO, ...quick });
  const s1 = a.chain!.deviceRecord("m-1")!.rotatedSeq!;
  const s2 = a.chain!.deviceRecord("m-2")!.rotatedSeq!;
  const d1 = await keptOf(a, "m-1", h1.signPrivate, s1);
  const d2 = await keptOf(a, "m-2", h2.signPrivate, s2);
  a.keepDisputes([d1]);
  // 600 copies of one valid statement for m-2 (a hub may repeat it): one kept, m-1's stays.
  a.keepDisputes(Array.from({ length: 600 }, () => d2));
  assert.deepEqual(a.keptDisputes(), [d1, d2]);
  assert.equal(a.isolated("m-1"), true);
  // Many resolved ones (hosts long gone): only those are dropped past the cap, the oldest first.
  a.keepDisputes(Array.from({ length: 600 }, (_, i) => ({ host: `gone-${i}`, seq: 1, sig: "00".repeat(64) })));
  const kept = a.keptDisputes();
  assert.ok(kept.length <= 512, String(kept.length));
  assert.ok(kept.some((d) => d.host === "m-1") && kept.some((d) => d.host === "m-2"), "the isolating ones stay");
  assert.equal(a.isolated("m-1"), true);
  assert.equal(a.isolated("m-2"), true);

  // Storage that cannot be written: kept in memory all the same.
  const { a: b, agents: bAgents, x: bx, h1: bh1 } = await account();
  await b.removeDevice(bx.deviceId, new FakeCourier());
  await b.renewIdentity("m-1", bAgents, { assurance: ASSURANCE_AUTO, ...quick });
  const b1 = b.chain!.deviceRecord("m-1")!.rotatedSeq!;
  const bus = tabsBus();
  const tab1 = await tabOf(b, bus(), refusingDisputes(browserStorage(b.store)));
  const tab2 = await tabOf(b, bus(), refusingDisputes(browserStorage(b.store)));
  let heard = 0;
  tab2.onChange(() => heard++);
  tab1.keepDisputes([await keptOf(b, "m-1", bh1.signPrivate, b1)]);
  assert.equal(tab1.isolated("m-1"), true, "the write failed; memory keeps it");
  // Another tab hears of it at once — and tells its listeners (its channels among them).
  assert.equal(tab2.isolated("m-1"), true, "the other tab heard of it");
  assert.ok(heard > 0, "and told its listeners");
});

test("Codex 1: the runner's evidence from the hub now never stands in for what this page kept", async () => {
  const { hub, a, agents, x, h1 } = await account();
  await a.removeDevice(x.deviceId, new FakeCourier());
  await a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick });
  const s1 = a.chain!.deviceRecord("m-1")!.rotatedSeq!;
  a.keepDisputes([await keptOf(a, "m-1", h1.signPrivate, s1)]);
  // The hub flags it isolated but hands no statement on now: what this page kept still isolates it — not a mere flag.
  hub.rotationRows.set("m-1", { host: "m-1", state: "committed", committed_seq: s1, isolated: true });
  const runner = new RenewalRunner({ account: a, link: new FakeLink() as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }) });
  await runner.refresh();
  await runner.idle();
  assert.equal(a.isolated("m-1"), true);
  assert.equal(a.flagged("m-1"), false, "isolated by evidence kept here, not flagged on the hub's word");
  runner.stop();
});

test("Codex 3: a 409 and a newer head never let a renewal ahead of an exposed host it lets in — checked on the very chain each build signs on", async () => {
  const { hub, a, agents, x } = await account({ chain: false });
  // m-2 lets m-1 control it; nothing is exposed yet, so m-2 may renew.
  await a.setAccept("m-2", { type: "devices", ids: ["m-1"] });
  const other = await tabOf(a, null);
  hub.windowRefusals = [Date.now() + 30];
  let removed = false;
  const sleep = async (ms: number) => {
    // While m-2's renewal waits for the window, another page removes X: m-1 is exposed, so m-2 must wait for it.
    if (!removed) {
      removed = true;
      await other.removeDevice(x.deviceId, new FakeCourier());
    }
    await new Promise((resolve) => setTimeout(resolve, Math.min(ms, 20)));
  };
  await assert.rejects(a.renewIdentity("m-2", agents, { assurance: ASSURANCE_AUTO, ...quick, sleep }), (e: unknown) => {
    assert.ok(e instanceof RenewalError && e.kind === "renewFirst", String(e));
    assert.deepEqual(e.hosts, ["m-1"]);
    return true;
  });
  assert.ok(removed);
  assert.deepEqual(rotated(a), [], "m-2 not written on the newer head");
  assert.ok(hub.rotationDeletes.includes("m-2"), "the attempt withdrawn");
});

test("Codex 4: a loss after the view a renewal was staged on is not remedied by it — the page starts again with a fresh nonce and seed; an entry an older page wrote leaves it exposed", async () => {
  const { hub, a, agents, x } = await account({ chain: false });
  const y = await pairAs(hub, a, { admin: false });
  await a.setAccept("m-1", { type: "devices", ids: [x.deviceId, y.deviceId] });
  await a.removeDevice(x.deviceId, new FakeCourier());
  hub.windowRefusals = [Date.now() + 30];
  let removed = false;
  const sleep = async (ms: number) => {
    if (!removed) {
      removed = true;
      await a.removeDevice(y.deviceId, new FakeCourier());
    }
    await new Promise((resolve) => setTimeout(resolve, Math.min(ms, 20)));
  };
  await a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick, sleep });
  assert.deepEqual(agents.rotateCalls, ["m-1", "m-1"], "asked again, with a fresh nonce");
  assert.deepEqual(rotated(a), ["m-1"], "written once, the second time");
  const r = a.chain!.entryAt(a.chain!.deviceRecord("m-1")!.rotatedSeq!)!.draft.action;
  assert.ok(r.type === "rotateHostKey");
  assert.ok(r.rotation.stagedSeq >= a.chain!.seq - 2, "staged on a view with Y's removal");
  assert.deepEqual(exposedIds(a.chain!), []);

  // An older page wrote a renewal staged before a loss (it did not check): that loss stays — the host is still exposed by it.
  const old = await account({ chain: false });
  const z = await pairAs(old.hub, old.a, { admin: false });
  await old.a.setAccept("m-1", { type: "devices", ids: [old.x.deviceId, z.deviceId] });
  await old.a.removeDevice(old.x.deviceId, new FakeCourier());
  (old.a as unknown as { checkRenewalOn: () => void }).checkRenewalOn = () => undefined;
  old.hub.windowRefusals = [Date.now() + 30];
  let gone = false;
  await old.a.renewIdentity("m-1", old.agents, {
    assurance: ASSURANCE_AUTO,
    ...quick,
    sleep: async (ms: number) => {
      if (!gone) {
        gone = true;
        await old.a.removeDevice(z.deviceId, new FakeCourier());
      }
      await new Promise((resolve) => setTimeout(resolve, Math.min(ms, 20)));
    },
  });
  assert.deepEqual(rotated(old.a), ["m-1"]);
  const left = hostsToRotate(old.a.chain!).get("m-1");
  assert.ok(left, "still exposed");
  assert.deepEqual([...left.lost.keys()], [z.deviceId], "by the loss after its staged view only");
});

test("Codex 5: this device's guard, or isolation learned while a renewal waits, refuses it at the commit", async () => {
  {
    const { hub, a, agents, x } = await account({ chain: false });
    await a.removeDevice(x.deviceId, new FakeCourier());
    hub.windowRefusals = [Date.now() + 30];
    const sleep = async (ms: number) => {
      a.setLocalAssured(true);
      await new Promise((resolve) => setTimeout(resolve, Math.min(ms, 20)));
    };
    await assert.rejects(a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick, sleep }), renewal("needsAssurance", "local"));
    assert.deepEqual(rotated(a), []);
  }
  {
    const { hub, a, agents, x, h1 } = await account({ chain: false });
    await a.removeDevice(x.deviceId, new FakeCourier());
    await a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick });
    const s1 = a.chain!.deviceRecord("m-1")!.rotatedSeq!;
    const d1 = await keptOf(a, "m-1", h1.signPrivate, s1);
    hub.windowRefusals = [Date.now() + 30];
    const sleep = async (ms: number) => {
      // Its retired key's dispute arrives meanwhile: isolated now.
      a.keepDisputes([d1]);
      await new Promise((resolve) => setTimeout(resolve, Math.min(ms, 20)));
    };
    await assert.rejects(a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick, sleep }), renewal("needsAssurance"));
    assert.deepEqual(rotated(a), ["m-1"], "only the first renewal");
  }
});

test("nothing renews until an admin chose the account's renewal policy, either way", async () => {
  const { a, agents, x } = await account({ policy: false });
  assert.equal(a.chain!.renewalPolicySeq, null);
  assert.equal(a.renewalPolicyChosen, false);
  await a.removeDevice(x.deviceId, new FakeCourier());
  await assert.rejects(a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick }), renewal("policyUnset"));
  const link = new FakeLink();
  link.set("m-1", "online");
  link.set("m-2", "online");
  const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }), renewOptions: quick });
  runner.start();
  await runner.idle();
  await new Promise((resolve) => setTimeout(resolve, 20));
  await runner.idle();
  assert.deepEqual(agents.rotateCalls, [], "not renewed before the choice");
  assert.deepEqual(runner.status("m-1"), { kind: "policyUnset" });
  // Chosen — automatic, the value it would have had — is written all the same, and renewals start.
  await runner.setAlwaysAssured(false);
  assert.notEqual(a.chain!.renewalPolicySeq, null, "written on the chain");
  await until(() => rotated(a).length === 2);
  await runner.idle();
  assert.deepEqual(rotated(a), ["m-1", "m-2"]);
  runner.stop();
});

/** Latch `a`'s fork alarm as a racing page's record would (another head at this very height), through main's own check. */
async function latchFork(a: CloudAccount): Promise<void> {
  const seq = a.chain!.seq;
  browserStorage(a.store).setItem(`rh.hub.seen.${a.account}`, JSON.stringify({ v: 1, seenThrough: seq, head: "ff".repeat(32), heads: [[seq, "ff".repeat(32)]], firstSeen: [] }));
  await assert.rejects(a.completeHandshake(async () => "open"), (e: unknown) => e instanceof AccountError && e.kind === "fork");
  assert.ok(a.forked);
}

test("main's fork latch holds renewals too: a forked page asks nothing of the host, and one forked while a renewal waits writes nothing", async () => {
  {
    const { hub, a, agents, x } = await account();
    await a.removeDevice(x.deviceId, new FakeCourier());
    await latchFork(a);
    await assert.rejects(a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick }), (e: unknown) => e instanceof AccountError && e.kind === "fork");
    assert.deepEqual(agents.rotateCalls, [], "nothing reached the host");
    assert.deepEqual(hub.rotationPuts.filter((p) => p.startsWith("m-1 ")), [], "nothing asked of the hub");
  }
  {
    const { hub, a, agents, x } = await account();
    await a.removeDevice(x.deviceId, new FakeCourier());
    hub.windowRefusals = [Date.now() + 30];
    let latched = false;
    const sleep = async (ms: number) => {
      if (!latched) {
        latched = true;
        await latchFork(a);
      }
      await new Promise((resolve) => setTimeout(resolve, Math.min(ms, 20)));
    };
    await assert.rejects(a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick, sleep }), (e: unknown) => e instanceof AccountError && e.kind === "fork");
    assert.deepEqual(rotated(a), [], "not written from a forked view");
  }
  // The courier the runner hands in is wrapped: its `/rh/rotate` is refused once forked, whoever calls it.
  {
    const { a, agents, x } = await account();
    await a.removeDevice(x.deviceId, new FakeCourier());
    await latchFork(a);
    const guarded = (a as unknown as { guardedRenewal(c: typeof agents): typeof agents }).guardedRenewal(agents);
    assert.throws(() => guarded.rotate("m-1", { nonce: "00", eph: "00", chainSeq: 0, chainHead: "00", assurance: 0 }), (e: unknown) => e instanceof AccountError && e.kind === "fork");
    assert.deepEqual(agents.rotateCalls, []);
  }
});

test("R6 review: kept disputes carry their signature and are verified again when read back: a tampered one isolates nothing once checked; a real one survives a reload", async () => {
  const { hub, a, agents, x, h1 } = await account();
  await a.removeDevice(x.deviceId, new FakeCourier());
  await a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick });
  const d1 = await keptOf(a, "m-1", h1.signPrivate);
  // Written into storage with a signature that is not the retired key's.
  browserStorage(a.store).setItem(`rh.hub.renewal.disputes.${a.account}`, JSON.stringify([{ ...d1, sig: "ab".repeat(64) }]));
  const page = await hub.reload(a);
  assert.equal(page.isolated("m-1"), true, "until checked, it counts: isolation fails safe");
  await until(() => !page.isolated("m-1"));
  assert.deepEqual(page.keptDisputes(), [], "dropped once checked");
  // From another tab, forged: the same.
  const bus = tabsBus();
  const tab1 = await tabOf(a, bus());
  const tab2 = await tabOf(a, bus());
  browserStorage(a.store).removeItem(`rh.hub.renewal.disputes.${a.account}`);
  tab1.keepDisputes([{ ...d1, sig: "cd".repeat(64) }]);
  await until(() => !tab2.isolated("m-1"));
  // The real one: kept, read back on a reload, verified again — and isolating.
  a.keepDisputes([d1]);
  const again = await hub.reload(a);
  await new Promise((resolve) => setTimeout(resolve, 50));
  assert.equal(again.isolated("m-1"), true, "it checks out");
  assert.ok(again.keptDisputes().some((d) => d.sig === d1.sig));
});

test("R6 review: a host the hub flags isolated is let control no other host either", async () => {
  const { a } = await account({ chain: false });
  a.noteRenewals({ renews: new Set(["m-1", "m-2", "m-3"]), pending: new Set(), isolated: new Set(), flagged: new Set(["m-1"]) });
  assert.equal(a.flagged("m-1"), true);
  assert.equal(a.mustRenewFirst("m-1"), true);
  await assert.rejects(a.setAccept("m-3", { type: "devices", ids: ["m-1"] }), (e: unknown) => e instanceof AccountError && e.kind === "exposed");
  assert.equal(admitsOn(a.chain!, "m-3", "m-1"), false);
});

test("Codex 6: a hand edit of who can control a host checks exposure on the chain every rebuilt batch goes on — a 409 lets no exposed host in", async () => {
  const { hub, a, x } = await account({ chain: false });
  // Every host renews here (what the runner notes): an exposed one must renew before it is let in anywhere.
  a.noteRenewals({ renews: new Set(["m-1", "m-2", "m-3"]), pending: new Set(), isolated: new Set() });
  const other = await tabOf(a, null);
  let removed = false;
  hub.gate = async (request) => {
    if (request !== "POST /v1/chain" || removed) return;
    removed = true;
    hub.gate = null;
    // Another page removes X just as this one appends: m-1 (which lets X in) is exposed on the newer head.
    await other.removeDevice(x.deviceId, new FakeCourier());
  };
  await assert.rejects(a.setAccept("m-3", { type: "devices", ids: ["m-1"] }), (e: unknown) => e instanceof AccountError && e.kind === "exposed");
  assert.ok(removed);
  await a.sync();
  assert.equal(admitsOn(a.chain!, "m-3", "m-1"), false, "m-1, exposed now, is let in nowhere");
});

test("Codex 5: this device's guard, or a dispute verified, while the batch is being signed is checked again right before publication", async () => {
  for (const flip of ["guard", "dispute"] as const) {
    const { a, agents, x, h1 } = await account({ chain: false });
    await a.removeDevice(x.deviceId, new FakeCourier());
    let d1: Awaited<ReturnType<typeof keptOf>> | null = null;
    if (flip === "dispute") {
      await a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick });
      d1 = await keptOf(a, "m-1", h1.signPrivate);
    }
    const subtle = crypto.subtle as unknown as { sign: (...args: unknown[]) => Promise<ArrayBuffer> };
    const sign = subtle.sign;
    let armed = false;
    // The batch's signatures (Ed25519) come after its build's checks: the flip lands between those and publication.
    subtle.sign = function (...args: unknown[]) {
      const alg = args[0] as string | { name?: string };
      if (armed && (alg === "Ed25519" || (typeof alg === "object" && alg?.name === "Ed25519"))) {
        armed = false;
        if (flip === "guard") a.setLocalAssured(true);
        else a.keepDisputes([d1!]);
      }
      return sign.apply(crypto.subtle, args);
    };
    try {
      await assert.rejects(
        a.renewIdentity("m-1", agents, {
          assurance: ASSURANCE_AUTO,
          ...quick,
          onStep: (st) => {
            if (st === "writing") armed = true;
          },
        }),
        renewal("needsAssurance"),
      );
    } finally {
      delete (subtle as { sign?: unknown }).sign;
    }
    assert.deepEqual(rotated(a), flip === "guard" ? [] : ["m-1"], `${flip}: nothing published`);
  }
});

test("Codex 4 cap: a renewal superseded on every try is started again at most 3 times in all, then it says so", async () => {
  const { hub, a, agents, x } = await account({ chain: false });
  const ys = [await pairAs(hub, a, { admin: false }), await pairAs(hub, a, { admin: false }), await pairAs(hub, a, { admin: false })];
  await a.setAccept("m-1", { type: "devices", ids: [x.deviceId, ...ys.map((y) => y.deviceId)] });
  await a.removeDevice(x.deviceId, new FakeCourier());
  hub.windowRefusals = [Date.now() + 30, Date.now() + 30, Date.now() + 30, Date.now() + 30];
  const left = [...ys];
  const sleep = async (ms: number) => {
    // Each try: a device that could control m-1 is removed while it waits — a loss after its staged view.
    const y = left.shift();
    if (y) await a.removeDevice(y.deviceId, new FakeCourier());
    await new Promise((resolve) => setTimeout(resolve, Math.min(ms, 20)));
  };
  await assert.rejects(a.renewIdentity("m-1", agents, { assurance: ASSURANCE_AUTO, ...quick, sleep }), renewal("superseded"));
  assert.deepEqual(agents.rotateCalls, ["m-1", "m-1", "m-1"], "three tries, no fourth");
  assert.deepEqual(rotated(a), []);
  assert.equal(hub.rotationDeletes.filter((h) => h === "m-1").length, 3, "each withdrawn");
});

test("§4.1 overlay: before ending a pause the runner judges exposure on its own chain — a hub-flagged host's edges stay paused (said why), a person's 「结束暂停」 is refused, and it resumes once nothing holds it", async () => {
  const { hub, a, agents, x } = await account();
  await a.setAccept("m-1", { type: "devices", ids: [x.deviceId, "m-2"] });
  await a.removeDevice(x.deviceId, new FakeCourier());
  // The hub holds m-2 isolated (no statement this page could check): it renews only confirmed on the device.
  hub.rotationRows.set("m-2", { host: "m-2", state: "pending", assurance: 1, requested_ms: Date.now(), requested_by: a.deviceId, needs_assurance: true, isolated: true, failure: null });
  const link = new FakeLink();
  link.set("m-1", "online");
  link.set("m-2", "online");
  const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }), renewOptions: quick });
  runner.start();
  await until(() => rotated(a).includes("m-1"));
  await runner.idle();
  await new Promise((resolve) => setTimeout(resolve, 20));
  await runner.idle();
  assert.equal(a.flagged("m-2"), true);
  const [pause] = a.chain!.openPauses();
  assert.ok(pause, "still paused");
  const of = runner.pausesOf("m-1");
  assert.equal(of.length, 1);
  assert.deepEqual(of[0]!.ending.edges, [{ host: "m-1", id: "m-2" }, { host: "m-2", id: "m-1" }]);
  assert.deepEqual(of[0]!.ending.held.map((h) => [h.host, h.id, h.who, h.why]), [["m-1", "m-2", "m-2", "flagged"], ["m-2", "m-1", "m-2", "flagged"]]);
  await assert.rejects(a.endPause(pause.op), (e: unknown) => e instanceof AccountError && e.kind === "exposed");
  // m-2 renewed on the device, and the hub no longer holds it: the pause completes on its own.
  await a.renewIdentity("m-2", agents, { assurance: ASSURANCE_ON_DEVICE, askCode: async (h) => agents.sas.get(h)!, ...quick });
  hub.rotationRows.set("m-2", { host: "m-2", state: "committed", assurance: 1, committed_seq: a.chain!.seq, needs_assurance: false, isolated: false, failure: null });
  await runner.refresh();
  await until(() => a.chain!.openPauses().length === 0);
  await runner.idle();
  assert.equal(a.chain!.pause(pause.op)?.end?.phase, 1);
  assert.equal(admitsOn(a.chain!, "m-1", "m-2"), true);
  runner.stop();
});

test("§4.1 overlay: a cycle member that cannot renew keeps only the edges that involve it paused — the rest come back, in one batch with a new start", async () => {
  const { hub, a, agents, x } = await account({ chain: false });
  hub.rotates.delete("m-3");
  // X → m-1 → m-2 → m-3 → m-1, as above; m-3 runs an older agent.
  await a.setAccept("m-1", { type: "devices", ids: [x.deviceId, "m-3"] });
  await a.setAccept("m-3", { type: "devices", ids: ["m-2"] });
  await a.setAccept("m-2", { type: "devices", ids: ["m-1"] });
  await a.removeDevice(x.deviceId, new FakeCourier());
  const removedAt = a.chain!.seq;
  const link = new FakeLink();
  for (const h of ["m-1", "m-2", "m-3"]) link.set(h, "online");
  const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": false }), renewOptions: quick });
  runner.start();
  await until(() => rotated(a).length === 2 && admitsOn(a.chain!, "m-2", "m-1"));
  await runner.idle();
  const written = writtenAfter(a, removedAt).filter((k) => k.startsWith("pause"));
  assert.deepEqual(written, ["pause start m-1<m-3,m-2<m-1,m-3<m-2", "pause start m-1<m-3,m-3<m-2", "pause completed"], written.join(" | "));
  assert.equal(admitsOn(a.chain!, "m-1", "m-3"), false, "m-3, which cannot renew, still paused");
  assert.equal(admitsOn(a.chain!, "m-3", "m-2"), false);
  assert.equal(a.chain!.openPauses().length, 1);
  assert.deepEqual(
    runner.pausesOf("m-1")[0]!.ending.held.map((h) => [h.host, h.id, h.who, h.why]),
    [["m-1", "m-3", "m-3", "cannotRenew"], ["m-3", "m-2", "m-3", "cannotRenew"]],
    "each held edge, and why: m-3 can never renew here, not merely exposed",
  );
  runner.stop();
});

test("§4.1 overlay: purpose-5 seals follow the chain's effective accept — a device a pause holds out is sealed no host key until it ends", async () => {
  const { a, x } = await account({ chain: false });
  assert.deepEqual((await a.issueMachineKey("m-1", new FakeCourier())).hostKeysTo, [x.deviceId], "before: to X, which m-1 lets in");
  const op = crypto.getRandomValues(new Uint8Array(16));
  assert.ok(await a.writePause(() => [{ type: "renewalPause", op, phase: 0, removals: [{ host: "m-1", ids: [x.deviceId] }] }]));
  assert.deepEqual(a.chain!.pausedEdges("m-1"), [x.deviceId]);
  assert.deepEqual((await a.issueMachineKey("m-1", new FakeCourier())).hostKeysTo, [], "paused: none to X");
  await a.endPause(op);
  assert.equal(a.chain!.pause(op)?.end?.phase, 2, "cancelled");
  assert.deepEqual((await a.issueMachineKey("m-1", new FakeCourier())).hostKeysTo, [x.deviceId], "after: to X again");
});

/** Patch the Ed25519 signing of the next batch: `then` runs as it is signed — after its build's checks, before it is published. */
function duringSigning(then: () => void): () => void {
  const subtle = crypto.subtle as unknown as { sign: (...args: unknown[]) => Promise<ArrayBuffer> };
  const sign = subtle.sign;
  let armed = true;
  subtle.sign = function (...args: unknown[]) {
    const alg = args[0] as string | { name?: string };
    if (armed && (alg === "Ed25519" || (typeof alg === "object" && alg?.name === "Ed25519"))) {
      armed = false;
      then();
    }
    return sign.apply(crypto.subtle, args);
  };
  return () => void delete (subtle as { sign?: unknown }).sign;
}

test("H1: a pause end is judged again right before publication — the hub's flag (or isolation) learned while it is signed stops it, by hand or on its own", async () => {
  const { a, x } = await account({ chain: false });
  a.noteRenewals({ renews: new Set(["m-1", "m-2", "m-3"]), pending: new Set(), isolated: new Set() });
  await a.setAccept("m-1", { type: "devices", ids: [x.deviceId, "m-2"] });
  for (const how of ["person", "runner"] as const) {
    const op = crypto.getRandomValues(new Uint8Array(16));
    assert.ok(await a.writePause(() => [{ type: "renewalPause", op, phase: 0, removals: [{ host: "m-1", ids: ["m-2"] }] }]));
    const restore = duringSigning(() => a.noteRenewals({ renews: new Set(["m-1", "m-2", "m-3"]), pending: new Set(), isolated: new Set(), flagged: new Set(["m-2"]) }));
    try {
      const ending = how === "person" ? a.endPause(op, [{ host: "m-1", id: "m-2" }]) : a.endPauseWith(op, 1, () => ({ keep: [] }));
      await assert.rejects(ending, (e: unknown) => e instanceof AccountError && e.kind === "exposed");
    } finally {
      restore();
    }
    assert.equal(a.chain!.pause(op)?.end ?? null, null, `${how}: not published`);
    assert.equal(admitsOn(a.chain!, "m-1", "m-2"), false);
    a.noteRenewals({ renews: new Set(["m-1", "m-2", "m-3"]), pending: new Set(), isolated: new Set() });
    await a.endPause(op, [{ host: "m-1", id: "m-2" }]);
  }
});

test("H1 (the runner's own plan): an end of a pause with two edges, one of whose ends is flagged while it is signed, gives back neither — what was signed is judged, not a plan made again", async () => {
  const { a, x, agents } = await account({ chain: false });
  const renews = new Set(["m-1", "m-2", "m-3"]);
  a.noteRenewals({ renews, pending: new Set(), isolated: new Set() });
  await a.setAccept("m-1", { type: "devices", ids: [x.deviceId, "m-2", "m-3"] });
  const op = crypto.getRandomValues(new Uint8Array(16));
  assert.ok(await a.writePause(() => [{ type: "renewalPause", op, phase: 0, removals: [{ host: "m-1", ids: ["m-2", "m-3"] }] }]));
  const runner = new RenewalRunner({ account: a, link: new FakeLink() as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }), renewOptions: quick });
  // The runner's own end of its pauses (`restoreGroups`): nothing must stay held when it is built, so the batch it signs returns both edges.
  const restore = (runner as unknown as { restoreGroups: () => Promise<boolean> }).restoreGroups.bind(runner);
  const flagM2 = duringSigning(() => a.noteRenewals({ renews, pending: new Set(), isolated: new Set(), flagged: new Set(["m-2"]) }));
  let wrote: boolean;
  try {
    wrote = await restore();
  } finally {
    flagM2();
  }
  assert.equal(a.flagged("m-2"), true);
  assert.equal(wrote, false, "the signed batch is not published");
  assert.equal(a.chain!.pause(op)?.end ?? null, null, "the pause stays open");
  assert.equal(admitsOn(a.chain!, "m-1", "m-2"), false, "m-2, flagged while the end was signed, is not let control m-1 again");
  assert.equal(admitsOn(a.chain!, "m-1", "m-3"), false);
  // The next look builds on what is known now: m-1←m-2 kept held in a new pause, m-1←m-3 given back, in one batch.
  assert.equal(await restore(), true);
  assert.notEqual(a.chain!.pause(op)?.end ?? null, null);
  assert.equal(admitsOn(a.chain!, "m-1", "m-2"), false, "still held");
  assert.equal(admitsOn(a.chain!, "m-1", "m-3"), true, "given back");
});

for (const mode of ["404", "off"] as const) {
  test(`switch off (${mode}): a removal that exposes hosts that control each other writes nothing new, asks the hub nothing, and keeps the re-enroll warning`, async () => {
    const { hub, a, agents, x } = await account({ policy: false });
    if (mode === "404") hub.rotationsGone = true;
    else hub.rotationEnabled = false;
    await a.setAccept("m-1", { type: "devices", ids: [x.deviceId, "m-2"] });
    const before = a.chain!.seq;
    await a.removeDevice(x.deviceId, new FakeCourier());
    const link = new FakeLink();
    for (const h of ["m-1", "m-2", "m-3"]) link.set(h, "online");
    const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }), renewOptions: quick });
    runner.start();
    await runner.idle();
    await new Promise((resolve) => setTimeout(resolve, 50));
    await runner.idle();
    const kinds = a.chain!.allEntries().filter((e) => e.draft.seq > before).map((e) => e.draft.action.type);
    assert.ok(!kinds.some((k) => k === "rotateHostKey" || k === "setRenewalPolicy" || k === "renewalPause"), kinds.join(","));
    assert.deepEqual(hub.rotationPuts, []);
    assert.deepEqual(agents.rotateCalls, []);
    assert.ok(a.reEnrollWarnings().has("m-1"), "the re-enroll warning stands");
    for (const h of ["m-1", "m-2"]) {
      assert.equal(runner.status(h).kind, "none");
      assert.equal(a.keysWithheldFrom(h), false);
      assert.equal(a.mustRenewFirst(h), false);
    }
    // And a list naming an exposed host is written as before.
    await a.setAccept("m-3", { type: "devices", ids: ["m-1"] });
    runner.stop();
  });
}

test("M7: 「结束暂停」 is bound to the exact control it showed — a 409 that reveals an overlapping pause ended meanwhile asks again", async () => {
  const { a, x } = await account({ chain: false });
  const p1 = crypto.getRandomValues(new Uint8Array(16));
  const p2 = crypto.getRandomValues(new Uint8Array(16));
  assert.ok(await a.writePause(() => [{ type: "renewalPause", op: p1, phase: 0, removals: [{ host: "m-1", ids: [x.deviceId] }] }]));
  assert.ok(await a.writePause(() => [{ type: "renewalPause", op: p2, phase: 0, removals: [{ host: "m-1", ids: [x.deviceId] }] }]));
  // Shown now: ending p1 gives back nothing (p2 still holds X out).
  const shown = a.pauseEnding(p1)!;
  assert.deepEqual(shown.edges, []);
  // Another page ends p2; this one's view lags (its append meets a 409, then the newer head).
  const other = await tabOf(a, null);
  // (Ending p2 alone gives nothing back: p1 still holds X out.)
  await other.endPause(p2, []);
  await assert.rejects(a.endPause(p1, shown.edges), (e: unknown) => e instanceof AccountError && e.kind === "pauseChanged");
  assert.equal(a.chain!.pause(p1)?.end ?? null, null, "not ended on what was not shown");
  const again = a.pauseEnding(p1)!;
  assert.deepEqual(again.edges, [{ host: "m-1", id: x.deviceId }], "the new set, to confirm again");
  await a.endPause(p1, again.edges);
  assert.equal(admitsOn(a.chain!, "m-1", x.deviceId), true);
});

test("M7: a pause whose starter no longer manages the account is never ended on its own — only by a person, and its row says so", async () => {
  const { hub, a, agents, x } = await account({ chain: false });
  const b = await pairAs(hub, a, { admin: true });
  await b.sync();
  const op = crypto.getRandomValues(new Uint8Array(16));
  assert.ok(await b.writePause(() => [{ type: "renewalPause", op, phase: 0, removals: [{ host: "m-1", ids: [x.deviceId] }] }]));
  await a.sync();
  await a.removeDevice(b.deviceId, new FakeCourier());
  // Removing b exposed every host; once they renewed, nothing held would stop the pause from ending — were its starter still an admin.
  const link = new FakeLink();
  for (const h of ["m-1", "m-2", "m-3"]) link.set(h, "online");
  const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf({ "m-1": true, "m-2": true, "m-3": true }), renewOptions: quick });
  runner.start();
  await until(() => exposedIds(a.chain!).length === 0);
  await runner.idle();
  await new Promise((resolve) => setTimeout(resolve, 20));
  await runner.idle();
  assert.equal(a.chain!.pause(op)?.end ?? null, null, "still open");
  const of = runner.pausesOf("m-1");
  assert.equal(of[0]!.ending.orphan, true);
  await a.endPause(op, of[0]!.ending.edges);
  assert.equal(a.chain!.pause(op)?.end?.phase, 2);
  runner.stop();
});

test("D2: when a pause ends, a device only paused is sealed the host's committed key epochs it missed — none while paused", async () => {
  const { hub, a, x } = await account({ chain: false });
  const op = crypto.getRandomValues(new Uint8Array(16));
  assert.ok(await a.writePause(() => [{ type: "renewalPause", op, phase: 0, removals: [{ host: "m-1", ids: [x.deviceId] }] }]));
  const issued = await a.issueMachineKey("m-1", new FakeCourier());
  assert.deepEqual(issued.hostKeysTo, [], "paused: not sealed the new epoch");
  const before = hub.mailLog.filter((m) => m.recipient === x.deviceId && m.purpose === MAIL_HOST_KEY).length;
  await a.endPause(op, [{ host: "m-1", id: x.deviceId }]);
  const after = hub.mailLog.filter((m) => m.recipient === x.deviceId && m.purpose === MAIL_HOST_KEY);
  assert.ok(after.length > before, "sealed once it is let in again");
  assert.ok(after.some((m) => m.epoch === issued.mkEpoch), "the epoch issued while it was paused among them");
});

test("M6: removals too large for one entry are cut into canonical pauses, a host's ids split only when they alone do not fit", () => {
  const removals = [
    { host: "m-1", ids: ["m-2", "m-3", "m-4"] },
    { host: "m-2", ids: ["m-1"] },
    { host: "m-3", ids: ["m-1", "m-2"] },
  ];
  const parts = splitPauseRemovals(removals, (r) => r.reduce((n, x) => n + x.ids.length, 0) <= 2);
  assert.deepEqual(parts, [
    [{ host: "m-1", ids: ["m-2", "m-3"] }],
    [{ host: "m-1", ids: ["m-4"] }, { host: "m-2", ids: ["m-1"] }],
    [{ host: "m-3", ids: ["m-1", "m-2"] }],
  ]);
  const edges = (list: { host: string; ids: string[] }[]) => list.flatMap((r) => r.ids.map((id) => `${r.host}<${id}`));
  assert.deepEqual(parts.flatMap(edges), edges(removals), "every edge, once, in order");
});

test("M6: a cycle too large for one RenewalPause is paused by several, each within MAX_PAUSE_ENTRY_LEN, in ONE batch before any host of it renews", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  const agents = new FakeAgents(hub, () => a.chain);
  // Eight hosts with long ids, each letting every other one control it: 56 edges, more than one entry holds.
  const ids = Array.from({ length: 8 }, (_, i) => `m-${i}-${"x".repeat(58)}`);
  const first = await hub.enroll(a, ids[0]!);
  await a.upgradeToV3({ writable: true });
  const hosts = [first];
  for (const id of ids.slice(1)) hosts.push(await hub.enroll(a, id, { writable: true }));
  for (const h of hosts) {
    agents.hold(h.id, { sign: h.signPrivate, signPublic: h.sign, dhPublic: h.dh.publicKey });
    hub.rotates.add(h.id);
  }
  await a.setRenewalPolicy(false);
  const x = await pairAs(hub, a, { admin: false });
  for (const id of ids) await a.setAccept(id, { type: "devices", ids: [...(id === ids[0] ? [x.deviceId] : []), ...ids.filter((o) => o !== id)] });
  await a.removeDevice(x.deviceId, new FakeCourier());
  const removedAt = a.chain!.seq;
  const link = new FakeLink();
  for (const id of ids) link.set(id, "online");
  const runner = new RenewalRunner({ account: a, link: link as never, courier: () => agents, me: meOf(Object.fromEntries(ids.map((id) => [id, true]))), renewOptions: quick });
  runner.start();
  await until(() => rotated(a).length > 0, 20_000);
  runner.stop();
  const after = a.chain!.allEntries().slice(removedAt + 1);
  const firstRotate = after.findIndex((e) => e.draft.action.type === "rotateHostKey");
  const starts = after.slice(0, firstRotate).filter((e) => e.draft.action.type === "renewalPause");
  assert.ok(starts.length >= 2, `split into ${starts.length} pauses`);
  // One batch, before any renewal: consecutive entries.
  assert.deepEqual(after.slice(0, starts.length).map((e) => e.draft.action.type), starts.map(() => "renewalPause"));
  for (const e of starts) assert.ok(encodeEntry(e).length <= MAX_PAUSE_ENTRY_LEN, `${encodeEntry(e).length} bytes`);
  const edges = new Set(starts.flatMap((e) => (e.draft.action.type === "renewalPause" ? e.draft.action.removals.flatMap((r) => r.ids.map((id) => `${r.host}<${id}`)) : [])));
  assert.equal(edges.size, 56, "every edge of the cycle paused");
});
