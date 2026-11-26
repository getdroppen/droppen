// Chain v3's attack regressions (`crates/droppen-proto/src/v3_regressions.rs`),
// a few of them replayed against the browser's verifier with entries signed
// here in WebCrypto: no self-grants and the last admin stays (item 4), a
// passkey adds only a plain pending device (item 17), no v2 entry after the
// first v3 one (item 2), and a host accepts control from another host only
// once an admin says so (S4). The fixture is Rust's: the v2 sample chain of
// `rust.json` (dev-0 an admin, dev-2 a plain device, m-1 a host, dev-1
// removed) and the extra devices and passkey slots of its `chainV3` section.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  type Action,
  CHAIN_VERSION,
  CHAIN_VERSION_3,
  ChainClock,
  ChainError,
  ChainState,
  createAttestation,
  decodeEntry,
  encodeEntry,
  type Entry,
  fromHex,
  HandshakeError,
  importEd25519Private,
  keyPairFromSecret,
  machineKeyCommit,
  MANAGE_FRESHNESS_MS,
  operationMaxAgeMs,
  type PasskeySlot,
  type Signer,
  signDraft,
  startHandshake,
} from "../src/lib/e2e/index.js";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Json = any;
const V: Json = JSON.parse(readFileSync(new URL("../../crates/droppen-proto/vectors/rust.json", import.meta.url), "utf8"));
const h = (s: string) => fromHex(s);
const hist = () => ChainClock.history();
const kind = (want: string) => (e: unknown) => {
  assert.ok(e instanceof ChainError, String(e));
  assert.equal(e.kind, want, e.message);
  return true;
};

const extra = (id: string) => V.chainV3.extraDevices.find((d: Json) => d.id === id);
const by = (id: string): Signer => ({ type: "device", id });
const sign = (seed: string) => importEd25519Private(h(seed));
const k0 = () => sign(V.chain.devices[0].signSeed);
const k2 = () => sign(V.chain.devices[2].signSeed);
const dev3 = (id: string, host: boolean, admin: boolean) => ({ id, signKey: h(extra(id).signKey), dhKey: h(extra(id).dhKey), flags: { host, admin } });
const slotOf = (i: number): { slot: PasskeySlot; key: Promise<CryptoKey> } => {
  const p = V.chainV3.passkeys[i];
  return {
    slot: { slot: p.slot, key: h(p.key), proof: h(p.proof), credHash: h(p.credHash), saltNonce: h(p.saltNonce), grants: p.grants, selfActivate: p.selfActivate },
    key: sign(p.seed),
  };
};

async function sampleChain(): Promise<ChainState> {
  return ChainState.verify(
    V.chain.entries.map((e: string) => decodeEntry(h(e))),
    undefined,
    hist(),
  );
}

async function entryFor(state: ChainState, signer: Signer, key: CryptoKey | Promise<CryptoKey>, action: Action): Promise<Entry> {
  return signDraft(state.draft(signer, action, 10_000 + state.seq), await key);
}

/** Sign and apply. */
async function push(state: ChainState, signer: Signer, key: CryptoKey | Promise<CryptoKey>, action: Action): Promise<Entry> {
  const entry = await entryFor(state, signer, key, action);
  await state.apply(entry, hist());
  return entry;
}

/** Sign and apply to a copy, leaving `state` as it was. */
async function tryPush(state: ChainState, signer: Signer, key: CryptoKey | Promise<CryptoKey>, action: Action): Promise<void> {
  const copy = await ChainState.verify(state.allEntries(), undefined, hist());
  await copy.apply(await entryFor(copy, signer, key, action), hist());
}

const mkCommit = async (host: string, epoch: number, key: Uint8Array): Promise<Action> => ({
  type: "keyEpochs",
  user: null,
  machines: [{ machine: host, epoch, hash: await machineKeyCommit("acct-1", host, epoch, key) }],
});

/** The sample chain plus, from dev-0, a second host `h-2` (not an admin) with its first machine key: the first v3 entries. */
async function withSecondHost(): Promise<ChainState> {
  const state = await sampleChain();
  await push(state, by("dev-0"), k0(), { type: "addDevice3", device: dev3("h-2", true, false) });
  assert.equal(state.hostKeyDue, "h-2");
  await push(state, by("dev-0"), k0(), await mkCommit("h-2", 0, new Uint8Array(32).fill(0x22)));
  assert.equal(state.hostKeyDue, null);
  return state;
}

test("a v2 chain maps onto devices with flags, and its views say what they said", async () => {
  const state = await sampleChain();
  const flags = (id: string) => {
    const d = state.deviceRecord(id)!;
    return [d.host, d.admin, d.status.type];
  };
  assert.deepEqual(flags("dev-0"), [false, true, "active"]);
  assert.deepEqual(flags("dev-1"), [false, true, "removed"]);
  assert.deepEqual(flags("dev-2"), [false, false, "active"]);
  assert.deepEqual(flags("m-1"), [true, false, "active"]);
  assert.ok(state.machine("m-1") && !state.machine("dev-0"));
  assert.ok(state.device("m-1"), "a host is a device too");
  assert.throws(() => state.fullDeviceKey("m-1"), kind("unknownDevice"));
  assert.equal(state.isV3, false);
  assert.equal(state.v3From, null);
  assert.deepEqual(
    state.hosts().map((d) => d.info.id),
    ["m-1"],
  );
  assert.deepEqual(
    state.admins().map((d) => d.info.id),
    ["dev-0"],
  );
  // A v2 chain drafts the shared kinds as v2.
  assert.equal(state.draft(by("dev-0"), { type: "approveDevice", id: "x" }, 1).version, CHAIN_VERSION);
  assert.equal(state.draft(by("dev-0"), { type: "setAdmin", id: "x", admin: true }, 1).version, CHAIN_VERSION_3);
});

test("item 4: no device grants itself admin or host, the last admin stays, and a batch applies in order", async () => {
  const state = await withSecondHost();
  await assert.rejects(tryPush(state, by("dev-0"), k0(), { type: "setHost", id: "dev-0", host: true }), kind("notAllowed"));
  await assert.rejects(tryPush(state, by("dev-0"), k0(), { type: "setAdmin", id: "dev-0", admin: true }), kind("notAllowed"));
  // A plain device cannot make itself (or anyone) an admin: it signs nothing.
  await assert.rejects(tryPush(state, by("dev-2"), k2(), { type: "setAdmin", id: "dev-2", admin: true }), kind("notAllowed"));
  // The sole admin can neither step down nor be removed.
  await assert.rejects(tryPush(state, by("dev-0"), k0(), { type: "setAdmin", id: "dev-0", admin: false }), kind("lastAdmin"));
  await assert.rejects(tryPush(state, by("dev-0"), k0(), { type: "removeDevice3", id: "dev-0" }), kind("lastAdmin"));
  // With a second admin, dev-0 may step down — and in the same batch signs nothing more.
  await push(state, by("dev-0"), k0(), { type: "setAdmin", id: "dev-2", admin: true });
  const stepDown = await entryFor(state, by("dev-0"), k0(), { type: "setAdmin", id: "dev-0", admin: false });
  const ahead = await ChainState.verify([...state.allEntries(), stepDown], undefined, hist());
  const after = await signDraft(ahead.draft(by("dev-0"), { type: "setAdmin", id: "dev-2", admin: false }, 2), await k0());
  const { applied, error } = await state.mergePrefix([stepDown, after], hist());
  assert.equal(applied, 1);
  assert.ok(error && kind("notAllowed")(error));
  assert.ok(state.admin("dev-2") && !state.admin("dev-0"));
  // And dev-2, now the last admin, cannot be demoted.
  await assert.rejects(tryPush(state, by("dev-2"), k2(), { type: "setAdmin", id: "dev-2", admin: false }), kind("lastAdmin"));
  // A no-op is refused.
  await assert.rejects(tryPush(state, by("dev-2"), k2(), { type: "setHost", id: "m-1", host: true }), kind("notAllowed"));
});

test("item 5: a new host's machine key comes next, and only hosts have machine keys", async () => {
  const state = await withSecondHost();
  await assert.rejects(tryPush(state, by("dev-0"), k0(), await mkCommit("dev-2", 0, new Uint8Array(32))), kind("unknownId"));
  await push(state, by("dev-0"), k0(), { type: "addDevice3", device: dev3("d-x", true, false) });
  await assert.rejects(tryPush(state, by("dev-0"), k0(), { type: "setAdmin", id: "dev-2", admin: true }), kind("notAllowed"));
  await assert.rejects(tryPush(state, by("dev-0"), k0(), await mkCommit("m-1", 0, new Uint8Array(32))), kind("notAllowed"));
  await push(state, by("dev-0"), k0(), await mkCommit("d-x", 0, new Uint8Array(32).fill(3)));
  // Stop hosting: no current machine, no control; restart: a new epoch next.
  await push(state, by("dev-0"), k0(), { type: "setHost", id: "h-2", host: false });
  assert.ok(!state.machine("h-2") && state.device("h-2"));
  assert.throws(() => state.mayControl("dev-0", "h-2"), kind("notAccepted"));
  await push(state, by("dev-0"), k0(), { type: "setHost", id: "h-2", host: true });
  await assert.rejects(tryPush(state, by("dev-0"), k0(), await mkCommit("h-2", 0, new Uint8Array(32).fill(0x22))), kind("notAllowed"));
  await push(state, by("dev-0"), k0(), await mkCommit("h-2", 1, new Uint8Array(32).fill(5)));
  assert.ok(state.machine("h-2"));
});

test("item 17: a passkey adds only a plain, pending device, and signs nothing else", async () => {
  const state = await withSecondHost();
  const p1 = slotOf(0);
  const byPk: Signer = { type: "passkey", slot: "p-1" };
  // Only an admin registers a slot, and it grants nothing.
  await assert.rejects(tryPush(state, by("h-2"), sign(extra("h-2").signSeed), { type: "passkeySlotSet", slot: p1.slot }), kind("notAllowed"));
  await assert.rejects(tryPush(state, by("dev-0"), k0(), { type: "passkeySlotSet", slot: { ...p1.slot, grants: 2 } }), kind("notAllowed"));
  await assert.rejects(tryPush(state, by("dev-0"), k0(), { type: "passkeySlotSet", slot: { ...p1.slot, slot: "p-3" } }), kind("notAllowed"), "the proof names its slot");
  await push(state, by("dev-0"), k0(), { type: "passkeySlotSet", slot: p1.slot });
  assert.equal(state.slot("p-1")?.setBy, "dev-0");
  // A host, an admin, or both: refused.
  for (const [host, admin] of [
    [true, false],
    [false, true],
    [true, true],
  ] as const) {
    await assert.rejects(tryPush(state, byPk, p1.key, { type: "addDevice3", device: dev3("d-p", host, admin) }), kind("notAllowed"));
  }
  // Anything but its own join and activation: refused.
  for (const action of [
    { type: "setAdmin", id: "dev-2", admin: true },
    { type: "removeDevice3", id: "dev-0" },
    { type: "approveDevice", id: "dev-2" },
    { type: "passkeySlotRemove", slot: "p-1" },
  ] as Action[]) {
    await assert.rejects(tryPush(state, byPk, p1.key, action), kind("notAllowed"));
  }
  await assert.rejects(tryPush(state, { type: "passkey", slot: "p-9" }, p1.key, { type: "addDevice3", device: dev3("d-p", false, false) }), kind("unknownSlot"));
  await assert.rejects(tryPush(state, byPk, k0(), { type: "addDevice3", device: dev3("d-p", false, false) }), kind("badSignature"));
  // The plain join: pending, via p-1; no handshakes yet; an admin approves it.
  await push(state, byPk, p1.key, { type: "addDevice3", device: dev3("d-p", false, false) });
  const r = state.deviceRecord("d-p")!;
  assert.ok(!r.host && !r.admin && r.viaPasskey === "p-1" && r.status.type === "pending");
  assert.throws(() => state.authorizeDevice("d-p", h(extra("d-p").dhKey)), kind("unknownDevice"));
  assert.deepEqual(state.pendingPasskeyJoins(), [{ id: "d-p", joinSeq: state.seq }]);
  await assert.rejects(tryPush(state, by("dev-0"), k0(), { type: "passkeyActivate", id: "d-p" }), kind("notAllowed"));
  await push(state, by("dev-0"), k0(), { type: "approveDevice", id: "d-p" });
  state.authorizeDevice("d-p", h(extra("d-p").dhKey));
  assert.equal(state.admin("d-p"), undefined, "approved, still not an admin");
  // Removing the admin that registered the slot voids it.
  await push(state, by("dev-0"), k0(), { type: "setAdmin", id: "dev-2", admin: true });
  await push(state, by("dev-2"), k2(), { type: "removeDevice3", id: "dev-0" });
  assert.equal(state.slot("p-1"), undefined);
});

test("item 2: no v2 entry follows the first v3 one, and the version byte is signed", async () => {
  const v2 = await sampleChain();
  const state = await withSecondHost();
  assert.equal(state.isV3, true);
  assert.equal(state.v3From, 5);
  // A v2-only action, or a shared one written as v2, after the move: refused.
  await assert.rejects(tryPush(state, by("dev-0"), k0(), { type: "removeMachine", id: "m-1" }), kind("notAllowed"));
  const shared = state.draft(by("dev-0"), await mkCommit("h-2", 1, new Uint8Array(32)), 1);
  assert.equal(shared.version, CHAIN_VERSION_3, "a v3 chain drafts the shared kinds as v3");
  const asV2 = await signDraft({ ...shared, version: CHAIN_VERSION }, await k0());
  const copy = await ChainState.verify(state.allEntries(), undefined, hist());
  await assert.rejects(copy.apply(asV2, hist()), kind("notAllowed"));
  // A v3 action under a v2 version byte: it does not decode; a draft saying so is refused.
  const wrong = await signDraft({ ...state.draft(by("dev-0"), { type: "setAdmin", id: "dev-2", admin: true }, 1), version: CHAIN_VERSION }, await k0());
  assert.throws(() => decodeEntry(encodeEntry(wrong)), kind("decode"));
  await assert.rejects(copy.apply(wrong, hist()), kind("version"));
  // A v2 entry relabelled v3: the signature covered the 2.
  const keyed = await entryFor(v2, by("dev-0"), k0(), await mkCommit("m-1", 0, new Uint8Array(32).fill(7)));
  assert.equal(keyed.draft.version, CHAIN_VERSION);
  const bytes = encodeEntry(keyed);
  bytes[0] = CHAIN_VERSION_3;
  await assert.rejects(v2.apply(decodeEntry(bytes), hist()), kind("badSignature"));
  await v2.apply(keyed, hist());
  // Unknown versions do not decode at all.
  for (const version of [0, 1, 4, 255]) {
    const b = encodeEntry(keyed);
    b[0] = version;
    assert.throws(() => decodeEntry(b), kind("version"));
  }
});

test("S4: a host accepts control only from admins until an admin allows more, and never from itself", async () => {
  const state = await withSecondHost();
  state.mayControl("dev-0", "m-1");
  state.mayControl("dev-0", "h-2");
  for (const [from, to] of [
    ["h-2", "m-1"],
    ["m-1", "h-2"],
    ["dev-2", "m-1"],
  ]) {
    assert.throws(() => state.mayControl(from!, to!), kind("notAccepted"), `${from} → ${to}`);
  }
  assert.throws(() => state.mayControl("m-1", "m-1"), kind("selfControl"));
  assert.throws(() => state.mayControl("dev-0", "dev-2"), kind("notAccepted"), "not a host");
  assert.throws(() => state.mayControl("dev-1", "m-1"), kind("unknownDevice"), "removed");
  // Only an admin sets a policy: not the host for itself, nor the device that wants in.
  for (const [id, seed] of [
    ["m-1", V.chain.machine.signSeed],
    ["h-2", extra("h-2").signSeed],
    ["dev-2", V.chain.devices[2].signSeed],
  ]) {
    await assert.rejects(tryPush(state, by(id), sign(seed), { type: "setAccept", host: "m-1", from: { type: "all" } }), kind("notAllowed"));
  }
  const list = (ids: string[]): Action => ({ type: "setAccept", host: "m-1", from: { type: "devices", ids } });
  await assert.rejects(tryPush(state, by("dev-0"), k0(), list(["m-1"])), kind("notAllowed"));
  await assert.rejects(tryPush(state, by("dev-0"), k0(), list(["dev-1"])), kind("unknownId"));
  await assert.rejects(tryPush(state, by("dev-0"), k0(), { type: "setAccept", host: "m-1", from: { type: "admins" } }), kind("notAllowed"), "no change");
  await assert.rejects(tryPush(state, by("dev-0"), k0(), { type: "setAccept", host: "dev-2", from: { type: "all" } }), kind("notAllowed"), "not a host");
  // m-1 lets h-2 in: h-2 → m-1 only.
  await push(state, by("dev-0"), k0(), list(["h-2"]));
  state.mayControl("h-2", "m-1");
  state.authorizeControl("h-2", h(extra("h-2").dhKey), "m-1");
  assert.throws(() => state.mayControl("m-1", "h-2"), kind("notAccepted"));
  assert.throws(() => state.mayControl("dev-2", "m-1"), kind("notAccepted"));
  await push(state, by("dev-0"), k0(), { type: "setAccept", host: "m-1", from: { type: "all" } });
  state.mayControl("dev-2", "m-1");
  await push(state, by("dev-0"), k0(), { type: "setAccept", host: "m-1", from: { type: "admins" } });
  assert.throws(() => state.mayControl("h-2", "m-1"), kind("notAccepted"));
  // A device never starts a channel to itself.
  const h2 = await keyPairFromSecret(h(extra("h-2").dhSecret));
  await assert.rejects(startHandshake(state, h2, "h-2", "h-2", 1, new Uint8Array(0)), (e: unknown) => e instanceof HandshakeError && e.kind === "notAccepted");
  await startHandshake(state, h2, "h-2", "m-1", 1, new Uint8Array(0));
});

test("item 8: only admins vouch, and management needs a fresher word than control", async () => {
  const state = await withSecondHost();
  const t = 1_000_000;
  const att = await createAttestation(state, "dev-0", await k0(), t);
  const day = 24 * 60 * 60 * 1000;
  assert.deepEqual(await state.freshness("h-2", [att], t + 6 * day, operationMaxAgeMs("control")), { type: "fresh" });
  assert.deepEqual(await state.freshness("h-2", [att], t + 8 * day, operationMaxAgeMs("control")), { type: "stale", newestMs: t });
  assert.deepEqual(await state.freshness("h-2", [att], t + 11 * 3_600_000, MANAGE_FRESHNESS_MS), { type: "fresh" });
  assert.deepEqual(await state.freshness("h-2", [att], t + 13 * 3_600_000, operationMaxAgeMs("manage")), { type: "stale", newestMs: t });
  for (const [id, seed] of [
    ["h-2", extra("h-2").signSeed],
    ["dev-2", V.chain.devices[2].signSeed],
    ["m-1", V.chain.machine.signSeed],
  ]) {
    const by = await createAttestation(state, id!, await sign(seed!), t);
    await assert.rejects(state.checkAttestation(by), kind("badAttestation"));
    assert.deepEqual(await state.freshness("dev-0", [by], t, operationMaxAgeMs("control")), { type: "stale", newestMs: null });
  }
});
