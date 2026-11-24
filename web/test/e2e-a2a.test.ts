// A2A proxies on the device chain (`docs/a2a.md` §7.1), browser side: the
// flag (bit 2), `setA2aAccept` (kind 23), `mayCallA2a`, and the rule this
// whole lane rests on — an A2A proxy never controls a host, so no machine key
// is ever sealed to it. Each rule is tried here against chains signed in
// WebCrypto, and both halves' interop vectors are replayed: Rust's `a2a`
// section of `rust.json` (its state, matrices and refusals must be ours, rule
// for rule) and ours of `ts.json` (which Rust replays the same way).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { a2aReachJson, a2aStateJson, relationJson } from "../scripts/e2e-vectors.js";
import {
  type Action,
  CHAIN_VERSION,
  ChainClock,
  ChainError,
  ChainState,
  createAttestation,
  decodeEntry,
  encodeEntry,
  type Entry,
  entryHash,
  FLAG_A2A,
  flagsFromByte,
  flagsToByte,
  fromHex,
  genesis3Draft,
  hex,
  importEd25519Private,
  isCurrentA2a,
  machineKeyCommit,
  MAX_ACCEPT_IDS,
  PAUSE_CANCELLED,
  PAUSE_COMPLETED,
  PAUSE_START,
  publicFromSecret,
  RULES,
  type Signer,
  sha256,
  signDraft,
  utf8,
} from "../src/lib/e2e/index.js";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Json = any;
const read = (name: string): Json => JSON.parse(readFileSync(new URL(`../../crates/droppen-proto/vectors/${name}`, import.meta.url), "utf8"));
const hist = () => ChainClock.history();
const kind = (want: string, reason?: string) => (e: unknown) => {
  assert.ok(e instanceof ChainError, String(e));
  assert.equal(e.kind, want, e.message);
  if (reason !== undefined) assert.equal(e.detail.reason, reason, e.message);
  return true;
};

/** Replay one half's `a2a` section: bytes, hashes, every step's state, the matrices and every refusal by its rule. */
async function replay(v: Json, half: string): Promise<void> {
  const entries = v.entries.map((e: string) => decodeEntry(fromHex(e)));
  entries.forEach((e: Entry, i: number) => assert.equal(hex(encodeEntry(e)), v.entries[i], `${half}: entry ${i} re-encodes to its bytes`));
  for (const [i, e] of entries.entries()) assert.equal(hex(await entryHash(e)), v.hashes[i], `${half}: hash ${i}`);
  if (v.versions) assert.deepEqual(entries.map((e: Entry) => e.draft.version), v.versions);
  const hosts: string[] = v.state.a2aAccept.map((a: Json) => a.host);
  for (const step of v.steps) {
    const prefix = await ChainState.verify(entries.slice(0, step.seq + 1), undefined, hist());
    if (step.state) assert.deepEqual(a2aStateJson(prefix, hosts), step.state, `${half}: ${step.label}`);
  }
  const state = await ChainState.verify(entries, undefined, hist());
  assert.deepEqual(a2aStateJson(state, hosts), v.state, `${half}: the final state`);
  for (const c of v.control) assert.deepEqual(relationJson((d, h) => state.mayControl(d, h), c.device, c.host), c, `${half}: control ${c.device} → ${c.host}`);
  for (const c of v.a2aCalls) assert.deepEqual(relationJson((d, h) => state.mayCallA2a(d, h), c.device, c.host), c, `${half}: A2A ${c.device} → ${c.host}`);
  const reach = a2aReachJson(state, "nobody-at-all");
  const byDevice = (list: Json[]) => new Map(list.map((r: Json) => [r.device, r.hosts]));
  const admitting = byDevice(reach.hostsAdmitting);
  const accepting = byDevice(reach.hostsAcceptingA2a);
  for (const r of v.hostsAdmitting) assert.deepEqual(admitting.get(r.device) ?? [], r.hosts, `${half}: hosts admitting ${r.device}`);
  for (const r of v.hostsAcceptingA2a) assert.deepEqual(accepting.get(r.device) ?? [], r.hosts, `${half}: hosts taking A2A calls from ${r.device}`);
  // Whose machine keys an A2A proxy may hold: none, on either half's chain.
  for (const d of state.allDevices().filter((d) => d.a2a)) assert.deepEqual(admitting.get(d.info.id), [], `${half}: ${d.info.id} is sealed no machine key`);
  for (const f of v.flagBytes ?? []) {
    if (f.ok) assert.deepEqual(flagsFromByte(f.byte), f.flags, `flag byte ${f.byte}`);
    else assert.throws(() => flagsFromByte(f.byte), `flag byte ${f.byte}`);
  }
  let ruled = 0;
  for (const c of v.invalid) {
    let error: unknown = null;
    try {
      const es = c.entries.map((e: string) => decodeEntry(fromHex(e)));
      await ChainState.verify(es, undefined, hist());
    } catch (e) {
      error = e;
    }
    assert.ok(error instanceof ChainError, `${half}: ${c.name} must be refused (${String(error)})`);
    assert.equal(error.kind, c.error, `${half}: ${c.name}: ${error.message}`);
    if (typeof c.rule === "string") {
      assert.equal(error.detail.reason, c.rule, `${half}: ${c.name}`);
      ruled++;
    }
  }
  assert.ok(ruled >= 12, `${half} names the rule of its refusals (${ruled})`);
}

test("Rust's A2A proxy vectors replay here: the same bytes, state, matrices and refusals", async (t) => {
  const v = read("rust.json");
  if (!v.a2a) {
    t.skip("rust.json has no a2a section yet: regenerate it (DROPPEN_PROTO_WRITE_VECTORS=1 cargo test -p droppen-proto --lib vectors)");
    return;
  }
  await replay(v.a2a, "rust.json");
});

test("our own A2A proxy vectors replay as Rust replays them", async () => {
  const v = read("ts.json");
  assert.ok(v.a2a, "ts.json has an a2a section: regenerate it with npx tsx web/scripts/e2e-vectors.ts");
  await replay(v.a2a, "ts.json");
});

// ------------------------------------------------------------ a chain built here

const seed = (n: number) => sha256(utf8(`a2a test ${n}`));
interface Member {
  id: string;
  key: CryptoKey;
  signKey: Uint8Array;
  dhKey: Uint8Array;
}
async function member(id: string, n: number): Promise<Member> {
  const s = await seed(n);
  return { id, key: await importEd25519Private(s), signKey: await publicFromSecret("Ed25519", s), dhKey: await publicFromSecret("X25519", await seed(n + 1000)) };
}
const v3 = (m: Member, f: { host?: boolean; admin?: boolean; a2a?: boolean }) => ({ id: m.id, signKey: m.signKey, dhKey: m.dhKey, flags: { host: !!f.host, admin: !!f.admin, a2a: !!f.a2a } });
const by = (m: Member): Signer => ({ type: "device", id: m.id });

/** genesis(admin) → host h + its key → plain p → A2A proxies x, y: the fixture every test below starts from. */
async function fixture() {
  const ak = await member("ak", 1);
  const [admin, host, plain, x, y, other] = [await member("d-admin", 2), await member("m-host", 3), await member("d-plain", 4), await member("a-x", 5), await member("a-y", 6), await member("d-other", 7)];
  const genesis = await signDraft(genesis3Draft("acct-a2a", ak.signKey, v3(admin, { admin: true }), null, 1_000), ak.key);
  const state = await ChainState.verify([genesis], undefined, hist());
  const push = async (action: Action, signer: Member = admin, st = state) => {
    const entry = await signDraft(st.draft(by(signer), action, 2_000 + st.seq), signer.key);
    await st.apply(entry, hist());
    return entry;
  };
  const tryPush = async (action: Action, signer: Member = admin, st = state) => push(action, signer, st.clone());
  await push({ type: "addDevice3", device: v3(host, { host: true }) });
  await push({ type: "keyEpochs", user: null, machines: [{ machine: host.id, epoch: 0, hash: await machineKeyCommit("acct-a2a", host.id, 0, new Uint8Array(32).fill(1)) }] });
  await push({ type: "addDevice3", device: v3(plain, {}) });
  await push({ type: "addDevice3", device: v3(x, { a2a: true }) });
  await push({ type: "addDevice3", device: v3(y, { a2a: true }) });
  return { ak, admin, host, plain, x, y, other, state, push, tryPush };
}

test("flag bit 2 is a2a; bit 3 and beyond do not decode", () => {
  for (let b = 0; b < 8; b++) {
    const f = flagsFromByte(b);
    assert.equal(flagsToByte(f), b);
    assert.equal(f.a2a, (b & FLAG_A2A) !== 0);
  }
  assert.equal(FLAG_A2A, 0b100);
  for (const b of [0b1000, 0b1100, 0xff]) assert.throws(() => flagsFromByte(b), /invalid device flags/);
});

test("setA2aAccept is kind 23, v3 only, one canonical list of at most 64 ids, possibly none", async () => {
  const { state, admin, host, x, y } = await fixture();
  const entry = await signDraft(state.draft(by(admin), { type: "setA2aAccept", host: host.id, devices: [x.id, y.id] }, 9), admin.key);
  const bytes = encodeEntry(entry);
  assert.equal(entry.draft.version, 3);
  assert.deepEqual(decodeEntry(bytes), entry);
  const kindAt = bytes.length - 64 - (2 + host.id.length) - 2 - (2 + x.id.length) - (2 + y.id.length) - 1;
  assert.equal(bytes[kindAt], 23);
  const empty = await signDraft(state.draft(by(admin), { type: "setA2aAccept", host: host.id, devices: [] }, 9), admin.key);
  assert.deepEqual(decodeEntry(encodeEntry(empty)), empty);
  const decodes = async (devices: string[], version?: number) => {
    const draft = state.draft(by(admin), { type: "setA2aAccept", host: host.id, devices }, 9);
    return decodeEntry(encodeEntry(await signDraft(version ? { ...draft, version } : draft, admin.key)));
  };
  await assert.rejects(decodes([y.id, x.id]), kind("decode"));
  await assert.rejects(decodes([x.id, x.id]), kind("decode"));
  await assert.rejects(decodes(Array.from({ length: MAX_ACCEPT_IDS + 1 }, (_, i) => `a-${String(i).padStart(3, "0")}`)), kind("decode"));
  await decodes(Array.from({ length: MAX_ACCEPT_IDS }, (_, i) => `a-${String(i).padStart(3, "0")}`));
  await assert.rejects(decodes([x.id], CHAIN_VERSION), kind("decode"));
});

test("an A2A proxy is never a host nor an admin, and never joins with a passkey or the recovery words", async () => {
  const { state, admin, ak, other, x, tryPush } = await fixture();
  const genesis = await signDraft(genesis3Draft("acct-a2a", ak.signKey, v3(admin, { admin: true, a2a: true }), null, 1_000), ak.key);
  await assert.rejects(ChainState.verify([genesis], undefined, hist()), kind("notAllowed", RULES.a2aPlain));
  await assert.rejects(tryPush({ type: "addDevice3", device: v3(other, { a2a: true, host: true }) }), kind("notAllowed", RULES.a2aPlain));
  await assert.rejects(tryPush({ type: "addDevice3", device: v3(other, { a2a: true, admin: true }) }), kind("notAllowed", RULES.a2aPlain));
  await assert.rejects(tryPush({ type: "addDevice3", device: v3(other, { a2a: true, host: true, admin: true }) }), kind("notAllowed", RULES.hostNotAdmin));
  await assert.rejects(tryPush({ type: "setAdmin", id: x.id, admin: true }), kind("notAllowed", RULES.a2aPlain));
  await assert.rejects(tryPush({ type: "setHost", id: x.id, host: true }), kind("notAllowed", RULES.a2aPlain));
  // Nothing takes the flag away either: there is no entry that sets it, and setting what it is not is no change.
  await assert.rejects(tryPush({ type: "setAdmin", id: x.id, admin: false }), kind("notAllowed", RULES.noChange));
  await assert.rejects(tryPush({ type: "setHost", id: x.id, host: false }), kind("notAllowed", RULES.noChange));
  assert.ok(state.a2aDevice(x.id) && isCurrentA2a(state.device(x.id)!));
  assert.equal(state.a2aDevice(admin.id), undefined);
});

test("a passkey adds no A2A proxy, and an A2A proxy coming back with the recovery words is refused", async () => {
  // The rule's own chains: the vectors' generator (scripts/e2e-vectors.ts) builds and checks both, with a slot and words.
  const v = read("ts.json").a2a;
  const names = new Map(v.invalid.map((c: Json) => [c.name, c]));
  assert.equal((names.get("a passkey adds an A2A proxy") as Json).rule, RULES.passkeyPlain);
  assert.equal((names.get("an A2A proxy coming back with the recovery words") as Json).rule, RULES.a2aPlain);
});

test("an A2A proxy signs nothing and attests nothing", async () => {
  const { state, x, host, tryPush } = await fixture();
  await assert.rejects(tryPush({ type: "setA2aAccept", host: host.id, devices: [x.id] }, x), kind("notAllowed", RULES.onlyAdminSigns));
  await assert.rejects(tryPush({ type: "removeDevice3", id: x.id }, x), kind("notAllowed", RULES.onlyAdminSigns));
  const att = await createAttestation(state, x.id, x.key, 5_000);
  await assert.rejects(state.checkAttestation(att), kind("badAttestation"));
  assert.throws(() => state.fullDeviceKey(x.id), kind("unknownDevice"));
});

test("setA2aAccept: an admin's, for a current host, naming current A2A proxies, and it changes something", async () => {
  const { state, admin, host, plain, x, y, tryPush, push } = await fixture();
  const set = (h: string, devices: string[]): Action => ({ type: "setA2aAccept", host: h, devices });
  await assert.rejects(tryPush(set(host.id, [x.id]), plain), kind("notAllowed", RULES.onlyAdminSigns));
  await assert.rejects(tryPush(set(plain.id, [x.id])), kind("notAllowed", RULES.a2aAcceptNotHost));
  await assert.rejects(tryPush(set(x.id, [y.id])), kind("notAllowed", RULES.a2aAcceptNotHost));
  await assert.rejects(tryPush(set("nobody", [x.id])), kind("unknownId"));
  await assert.rejects(tryPush(set(host.id, [plain.id])), kind("notAllowed", RULES.a2aAcceptNotA2a));
  await assert.rejects(tryPush(set(host.id, [admin.id])), kind("notAllowed", RULES.a2aAcceptNotA2a));
  await assert.rejects(tryPush(set(host.id, [host.id])), kind("notAllowed", RULES.a2aAcceptNotA2a));
  await assert.rejects(tryPush(set(host.id, ["a-nobody"])), kind("unknownId"));
  await assert.rejects(tryPush(set(host.id, [])), kind("notAllowed", RULES.a2aAcceptSame));
  await push(set(host.id, [x.id, y.id]));
  assert.equal(state.hostKeyDue, null, "adding proxies owes no machine key");
  assert.deepEqual(state.a2aAccept(host.id), [x.id, y.id]);
  await assert.rejects(tryPush(set(host.id, [x.id, y.id])), kind("notAllowed", RULES.a2aAcceptSame));
  // Dropping one takes the host's next machine key epoch in the very next entry: a session it started could read the
  // host's key files (as a setAccept narrowing, D2). Anything else next is refused.
  const key = async (epoch: number): Promise<Action> => ({
    type: "keyEpochs",
    user: null,
    machines: [{ machine: host.id, epoch, hash: await machineKeyCommit("acct-a2a", host.id, epoch, new Uint8Array(32).fill(epoch + 10)) }],
  });
  await push(set(host.id, [y.id]));
  assert.equal(state.hostKeyDue, host.id);
  await assert.rejects(tryPush(set(host.id, [x.id, y.id])), kind("notAllowed", RULES.hostKeyNext));
  await push(await key(1));
  assert.equal(state.hostKeyDue, null);
  await push(set(host.id, []));
  assert.equal(state.hostKeyDue, host.id);
  await push(await key(2));
  assert.deepEqual(state.a2aAccept(host.id), []);
  assert.deepEqual(state.a2aAccept(plain.id), [], "none by default");
});

test("mayCallA2a: only a current A2A proxy, only to a current host whose list names it", async () => {
  const { state, admin, host, plain, x, y, push } = await fixture();
  assert.throws(() => state.mayCallA2a(x.id, host.id), kind("notAccepted"), "none by default");
  await push({ type: "setA2aAccept", host: host.id, devices: [x.id] });
  state.mayCallA2a(x.id, host.id);
  assert.throws(() => state.mayCallA2a(y.id, host.id), kind("notAccepted"), "not on the list");
  assert.throws(() => state.mayCallA2a(admin.id, host.id), kind("notAccepted"), "an admin is no A2A proxy");
  assert.throws(() => state.mayCallA2a(plain.id, host.id), kind("notAccepted"));
  assert.throws(() => state.mayCallA2a(x.id, plain.id), kind("notAccepted"), "not a host");
  assert.throws(() => state.mayCallA2a(x.id, x.id), kind("selfControl"));
  assert.throws(() => state.mayCallA2a("nobody", host.id), kind("unknownDevice"));
  assert.deepEqual(
    state.hostsAcceptingA2a(x.id).map((d) => d.info.id),
    [host.id],
  );
  assert.deepEqual(state.hostsAcceptingA2a(y.id), []);
  assert.deepEqual(state.hostsAcceptingA2a(admin.id), []);
});

test("an A2A proxy never controls a host — every device does not take it in, no list may name it — so it is never sealed a machine key", async () => {
  const { state, host, plain, x, y, push, tryPush } = await fixture();
  const admitting = (id: string) =>
    state
      .hosts()
      .filter((h) => state.machine(h.info.id) && relationJson((d, hh) => state.mayControl(d, hh), id, h.info.id).ok)
      .map((h) => h.info.id);
  await push({ type: "setA2aAccept", host: host.id, devices: [x.id, y.id] });
  assert.throws(() => state.mayControl(x.id, host.id), kind("notAccepted"), "admins only");
  // A list never names it (a name written but ignored would mislead), alone or beside a device that does control.
  for (const ids of [[x.id], [x.id, plain.id], [x.id, y.id, plain.id]]) {
    await assert.rejects(tryPush({ type: "setAccept", host: host.id, from: { type: "devices", ids } }), kind("notAllowed", RULES.a2aNotController), ids.join(","));
  }
  // The older checks, each id in turn: an unknown one, the host itself.
  await assert.rejects(tryPush({ type: "setAccept", host: host.id, from: { type: "devices", ids: ["a-nobody"] } }), kind("unknownId"));
  await assert.rejects(tryPush({ type: "setAccept", host: host.id, from: { type: "devices", ids: [host.id] } }), kind("notAllowed", RULES.acceptSelf));
  await push({ type: "setAccept", host: host.id, from: { type: "devices", ids: [plain.id] } });
  state.mayControl(plain.id, host.id);
  await push({ type: "setAccept", host: host.id, from: { type: "all" } });
  assert.throws(() => state.mayControl(x.id, host.id), kind("notAccepted"), "nor does every device");
  assert.throws(() => state.authorizeControl(x.id, x.dhKey, host.id), kind("notAccepted"));
  assert.deepEqual(admitting(x.id), []);
  assert.deepEqual(admitting(y.id), []);
  assert.deepEqual(admitting(plain.id), [host.id]);
  // From everyone to admins only: plain is shut out (a new key), the proxies were never in.
  const narrowed = state.clone();
  await push({ type: "setAccept", host: host.id, from: { type: "admins" } }, undefined, narrowed);
  assert.equal(narrowed.hostKeyDue, host.id);
  // With only proxies besides the host and admins, the same narrowing owes nothing.
  const onlyProxies = state.clone();
  await push({ type: "removeDevice3", id: plain.id }, undefined, onlyProxies);
  await push({ type: "setAccept", host: host.id, from: { type: "admins" } }, undefined, onlyProxies);
  assert.equal(onlyProxies.hostKeyDue, null);
  await tryPush({ type: "setA2aAccept", host: host.id, devices: [x.id] });
});

test("a renewal pause holds an A2A proxy's calls as it holds control; its end gives them back, the list untouched", async () => {
  const { state, host, x, y, push } = await fixture();
  await push({ type: "setA2aAccept", host: host.id, devices: [x.id, y.id] });
  const op = new Uint8Array(16).fill(0xa2);
  await push({ type: "renewalPause", op, phase: PAUSE_START, removals: [{ host: host.id, ids: [x.id] }] });
  assert.deepEqual(state.pausedEdges(host.id), [x.id]);
  assert.throws(() => state.mayCallA2a(x.id, host.id), kind("notAccepted"));
  state.mayCallA2a(y.id, host.id);
  assert.deepEqual(state.a2aAccept(host.id), [x.id, y.id], "the list stays as it was");
  assert.deepEqual(state.hostsAcceptingA2a(x.id), []);
  for (const phase of [PAUSE_COMPLETED, PAUSE_CANCELLED] as const) {
    const ended = state.clone();
    await push({ type: "renewalPause", op, phase, removals: [] }, undefined, ended);
    ended.mayCallA2a(x.id, host.id);
    assert.deepEqual(ended.hostsAcceptingA2a(x.id).map((d) => d.info.id), [host.id]);
  }
});

test("a removal drops the proxy from every A2A list; a host that stops running AI drops its own", async () => {
  const { state, host, x, y, push, tryPush } = await fixture();
  await push({ type: "setA2aAccept", host: host.id, devices: [x.id, y.id] });
  const copy = state.clone();
  await push({ type: "removeDevice3", id: x.id });
  assert.deepEqual(state.a2aAccept(host.id), [y.id]);
  assert.deepEqual(copy.a2aAccept(host.id), [x.id, y.id], "a clone keeps its own lists");
  assert.throws(() => state.mayCallA2a(x.id, host.id), kind("unknownDevice"));
  assert.equal(state.a2aDevice(x.id), undefined);
  await assert.rejects(tryPush({ type: "setA2aAccept", host: host.id, devices: [y.id] }), kind("notAllowed", RULES.a2aAcceptSame));
  await assert.rejects(tryPush({ type: "setA2aAccept", host: host.id, devices: [x.id, y.id] }), kind("unknownId"));
  await push({ type: "removeDevice3", id: y.id });
  assert.deepEqual(state.a2aAccept(host.id), [], "a list left empty is none");
  const f = await fixture();
  await f.push({ type: "setA2aAccept", host: f.host.id, devices: [f.x.id] });
  await f.push({ type: "setHost", id: f.host.id, host: false });
  assert.deepEqual(f.state.a2aAccept(f.host.id), []);
  await f.push({ type: "setHost", id: f.host.id, host: true });
  await f.push({ type: "keyEpochs", user: null, machines: [{ machine: f.host.id, epoch: 1, hash: await machineKeyCommit("acct-a2a", f.host.id, 1, new Uint8Array(32).fill(2)) }] });
  assert.deepEqual(f.state.a2aAccept(f.host.id), [], "restarting starts from none, never silently from before");
  assert.throws(() => f.state.mayCallA2a(f.x.id, f.host.id), kind("notAccepted"));
});
