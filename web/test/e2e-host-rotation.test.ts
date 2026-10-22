// A host renewing its identity key (`docs/host-identity-rotation.md` §3.4, §5.7), browser side.
//
// Rust → TypeScript: the `hostRotation` section of `rust.json` replays here —
// the chain verifies and re-signs (entries and both proof signatures) to the
// same bytes, maps to the same state with the same retired keys, each renewal
// proves the same body, code and fingerprint, the confirmation MAC and the ack
// are Rust's bytes, `keysAt` and `everNamed` agree, and every chain Rust
// refuses is refused here for the same rule. Then the attack regressions of
// `v3_regressions.rs` (§8 items 1–3) against the browser's verifier, with
// entries signed here: one admin rebinding a host alone, a proof replayed, a
// host's identity collided with another device's key.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { pauseStateJson, renewalJson, rotationStateJson } from "../scripts/e2e-vectors.js";
import {
  type Accept,
  type Action,
  ASSURANCE_AUTO,
  ASSURANCE_ON_DEVICE,
  type Assurance,
  CHAIN_VERSION,
  ChainClock,
  ChainError,
  ChainState,
  decodeEntry,
  decodeFrame,
  decodeHelloBody,
  decodeReplyBody,
  encodeDraft,
  encodeEntry,
  encodeFrame,
  encodeHelloBody,
  encodeReplyBody,
  type Entry,
  entryHash,
  fromHex,
  hex,
  HOST_ROTATE,
  type HostRotation,
  hostRotationBody,
  importEd25519Private,
  importX25519Private,
  LABELS,
  machineKeyCommit,
  MAX_HELLO_ENTRIES,
  MAX_HELLO_PAYLOAD,
  MAX_INFO,
  MAX_PAUSE_ENTRY_LEN,
  type OpenPause,
  PAUSE_CANCELLED,
  PAUSE_COMPLETED,
  PAUSE_START,
  type PausePhase,
  type PauseRemoval,
  publicFromSecret,
  recoveryKeys,
  RENEWAL_NONCE_LEN,
  ROTATE_ACK,
  ROTATE_CONFIRM_LABEL,
  ROTATE_DISPUTE,
  ROTATE_SAS_LABEL,
  rotateAckBody,
  rotateConfirmFor,
  rotateDisputeBody,
  rotateSas,
  RULES,
  sha256,
  signDraft,
  type Signer,
  signHostRotate,
  signRotateAck,
  signRotateDispute,
  utf8,
  verifyHostRotation,
  verifyRotateAck,
  verifyRotateConfirm,
  Writer,
} from "../src/lib/e2e/index.js";
import { deviceFingerprint } from "../src/lib/hub/account.js";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Json = any;
const V: Json = JSON.parse(readFileSync(new URL("../../server-rs/crates/rh-proto/vectors/rust.json", import.meta.url), "utf8"));
const R: Json = V.hostRotation;
const h = (s: string) => fromHex(s);
const hist = () => ChainClock.history();
const entriesOf = (list: string[]): Entry[] => list.map((e) => decodeEntry(h(e)));
const seqOf = (label: string): number => R.steps.find((s: Json) => s.label === label).seq;
const upto = (label: string) => ChainState.verify(entriesOf(R.entries).slice(0, seqOf(label) + 1), undefined, hist());
const rule = (reason: string) => (e: unknown) => {
  assert.ok(e instanceof ChainError, String(e));
  assert.equal(e.kind, "notAllowed", e.message);
  assert.equal(e.detail.reason, reason);
  return true;
};
const kind = (want: string) => (e: unknown) => {
  assert.ok(e instanceof ChainError, String(e));
  assert.equal(e.kind, want, e.message);
  return true;
};

/** A key pair: Rust's fixture keys by their seeds, or fresh ones from a label. */
interface Keys {
  key: CryptoKey;
  signKey: Uint8Array;
  dhKey: Uint8Array;
}
const fixtureKeys = async (k: { signSeed: string; dhKey: string }): Promise<Keys> => ({
  key: await importEd25519Private(h(k.signSeed)),
  signKey: await publicFromSecret("Ed25519", h(k.signSeed)),
  dhKey: h(k.dhKey),
});
const freshKeys = async (label: string): Promise<Keys> => {
  const seed = await sha256(utf8(`renewal test ${label} sign`));
  return { key: await importEd25519Private(seed), signKey: await publicFromSecret("Ed25519", seed), dhKey: await publicFromSecret("X25519", await sha256(utf8(`renewal test ${label} dh`))) };
};
const dev0 = () => fixtureKeys(V.chain.devices[0]);
const byDev0: Signer = { type: "device", id: "dev-0" };

/** A renewal of `host` to `next`, its proof signed by `oldKey` (honestly the host's current one) and `next`, against `st`'s head. */
async function renewal(st: ChainState, host: string, oldKey: CryptoKey, next: Keys, assurance: Assurance = ASSURANCE_AUTO): Promise<HostRotation> {
  const cur = st.deviceRecord(host)!.info;
  const r = { host, signKey: next.signKey, dhKey: next.dhKey, stagedSeq: st.seq, stagedHead: st.head, nonce: new Uint8Array(16).fill(9), assurance };
  const body = hostRotationBody(st.account, r, cur.signKey, cur.dhKey);
  return { ...r, oldSig: await signHostRotate(oldKey, body), newSig: await signHostRotate(next.key, body) };
}

/** Sign `action` with dev-0 on top of `st` and apply it to a copy. */
async function tryPush(st: ChainState, action: Action, signer: Signer = byDev0, key?: CryptoKey): Promise<ChainState> {
  const copy = st.clone();
  await copy.apply(await signDraft(copy.draft(signer, action, 50_000 + copy.seq), key ?? (await dev0()).key), hist());
  return copy;
}

test("renewals: Rust's chain verifies here, re-signs to the same bytes, and maps to the same state and retired keys", async () => {
  const entries = entriesOf(R.entries);
  entries.forEach((e, i) => assert.equal(hex(encodeEntry(e)), R.entries[i], `entry ${i} round-trips`));
  for (let i = 0; i < entries.length; i++) assert.equal(hex(await entryHash(entries[i]!)), R.hashes[i]);
  // Ed25519 is deterministic: dev-0 signed every entry, and h-2's keys the proofs.
  const k0 = (await dev0()).key;
  for (const e of entries) {
    if (e.draft.signer.type !== "device" || e.draft.signer.id !== "dev-0") continue;
    assert.equal(hex(encodeEntry(await signDraft(e.draft, k0))), hex(encodeEntry(e)), `entry ${e.draft.seq} re-signed`);
  }
  for (const step of R.steps) assert.equal((await ChainState.verify(entries.slice(0, step.seq + 1), undefined, hist())).seq, step.seq, step.label);
  const state = await ChainState.verify(entries, undefined, hist());
  assert.deepEqual(rotationStateJson(state, ["m-1", "h-2"]), R.state);
  assert.equal(state.hostKeyDue, null);
  // What each renewal proves, from its fields and the keys before it — body, code and fingerprint as Rust's.
  const hostKeys = await Promise.all(R.hostKeys.map(fixtureKeys));
  for (const [i, want] of R.renewals.entries()) {
    const before = await ChainState.verify(entries.slice(0, want.seq), undefined, hist());
    assert.deepEqual(await renewalJson(before, entries[want.seq]!), want);
    const a = entries[want.seq]!.draft.action;
    assert.ok(a.type === "rotateHostKey");
    assert.equal(a.rotation.assurance, i === 0 ? ASSURANCE_AUTO : ASSURANCE_ON_DEVICE);
    assert.equal(hex(await signHostRotate(hostKeys[i].key, h(want.body))), want.oldSig, "the old key's signature");
    assert.equal(hex(await signHostRotate(hostKeys[i + 1].key, h(want.body))), want.newSig, "the new key's");
    assert.ok(await verifyHostRotation(state.account, a.rotation, h(want.oldSign), h(want.oldDh)));
    // The fingerprint the alert says is the one the device list shows.
    assert.equal(await deviceFingerprint(h(want.newSign), h(want.newDh)), want.newFingerprint);
    assert.match(want.sas, /^[A-Z2-7]{4}-[A-Z2-7]{4}$/);
  }
  // The id and what the chain says about it stay; only the new keys open a channel.
  const r = state.machine("h-2")!;
  assert.ok(r.host && !r.admin && r.ranAi);
  assert.deepEqual(state.acceptPolicy("h-2"), { type: "devices", ids: ["d-c"] });
  assert.equal(state.machineKeyEpochs("h-2"), 4);
  state.authorizeDevice("h-2", h(R.hostKeys[2].dhKey));
  for (const old of R.hostKeys.slice(0, 2)) assert.throws(() => state.authorizeDevice("h-2", h(old.dhKey)), kind("unknownDevice"));
});

test("renewals: the confirmation MAC, the code and the ack are Rust's, from either side", async () => {
  const c = R.confirm;
  const body = h(c.body);
  assert.equal(hex(await publicFromSecret("X25519", h(c.newDhSecret))), c.newDhPublic);
  assert.equal(hex(await publicFromSecret("X25519", h(c.ephSecret))), c.ephPublic);
  assert.equal(hex(await rotateConfirmFor(await importX25519Private(h(c.newDhSecret)), h(c.ephPublic), body)), c.mac, "the host's side");
  const eph = await importX25519Private(h(c.ephSecret));
  assert.ok(await verifyRotateConfirm(eph, h(c.newDhPublic), body, h(c.mac)), "the admin's side");
  assert.equal(await verifyRotateConfirm(eph, h(R.hostKeys[0].dhKey), body, h(c.mac)), false, "the old DH key proves nothing");
  const other = Uint8Array.from(body, (b, i) => (i === body.length - 1 ? b ^ 1 : b));
  assert.equal(await verifyRotateConfirm(eph, h(c.newDhPublic), other, h(c.mac)), false);
  assert.equal(await verifyRotateConfirm(eph, new Uint8Array(32), body, h(c.mac)), false, "a low-order key");
  assert.notEqual(await rotateSas(other), await rotateSas(body));
  const a = R.ack;
  assert.equal(hex(await rotateAckBody(a.account, a.host, h(a.body), h(a.loginNonce))), a.ackBody);
  const key = await importEd25519Private(h(a.newSignSeed));
  assert.equal(hex(await signRotateAck(key, a.account, a.host, h(a.body), h(a.loginNonce))), a.signature);
  assert.ok(await verifyRotateAck(h(a.newSign), a.account, a.host, h(a.body), h(a.loginNonce), h(a.signature)));
  assert.equal(await verifyRotateAck(h(a.newSign), a.account, a.host, h(a.body), new Uint8Array(32).fill(1), h(a.signature)), false, "another login");
  assert.equal(await verifyRotateAck(h(a.newSign), a.account, "m-1", h(a.body), h(a.loginNonce), h(a.signature)), false);
  await assert.rejects(rotateAckBody(a.account, a.host, h(a.body), new Uint8Array(16)), /32 bytes/);
  // The labels and the rules are Rust's, word for word; every signature label stays prefix-free.
  assert.deepEqual(
    [HOST_ROTATE, ROTATE_ACK, ROTATE_CONFIRM_LABEL, ROTATE_SAS_LABEL, ROTATE_DISPUTE].map((l) => new TextDecoder().decode(l)),
    [R.labels.hostRotate, R.labels.rotateAck, R.labels.rotateConfirm, R.labels.rotateSas, R.labels.rotateDispute],
  );
  assert.deepEqual(
    {
      rotateNotHost: RULES.rotateNotHost,
      rotateKeyReused: RULES.rotateKeyReused,
      rotateStaged: RULES.rotateStaged,
      rotateProof: RULES.rotateProof,
      keyRetired: RULES.keyRetired,
      rotateNeedsAssurance: RULES.rotateNeedsAssurance,
      pauseOpReused: RULES.pauseOpReused,
      pauseHosts: RULES.pauseHosts,
      pauseEdges: RULES.pauseEdges,
      pauseIds: RULES.pauseIds,
      pauseTooLarge: RULES.pauseTooLarge,
      pauseNotOpen: RULES.pauseNotOpen,
      pauseEndRemovals: RULES.pauseEndRemovals,
    },
    R.rules,
  );
  for (const [i, x] of LABELS.entries()) {
    assert.equal(x[x.length - 1], 0);
    assert.equal(x.filter((b) => b === 0).length, 1);
    for (const [j, y] of LABELS.entries()) if (i !== j) assert.ok(!(y.length >= x.length && x.every((b, k) => y[k] === b)), `${i} is a prefix of ${j}`);
  }
});

test("renewals: keysAt and everNamed are Rust's", async () => {
  const state = await ChainState.verify(entriesOf(R.entries), undefined, hist());
  for (const k of R.keysAt) {
    const at = state.keysAt(k.id, k.seq);
    assert.deepEqual({ signKey: at ? hex(at.signKey) : null, dhKey: at ? hex(at.dhKey) : null }, { signKey: k.signKey, dhKey: k.dhKey }, `${k.id} at ${k.seq}`);
  }
  for (const k of R.everNamed) assert.equal(state.everNamed(h(k.key)), k.named, k.key);
  assert.deepEqual(
    state.retiredKeys("h-2").map((r) => [hex(r.signKey), r.untilSeq, r.byAssured]),
    [
      [R.hostKeys[0].signKey, seqOf("rotate"), false],
      [R.hostKeys[1].signKey, seqOf("rotateAgain"), true],
    ],
  );
  assert.deepEqual(state.retiredKeys("nobody"), []);
});

test("renewals: every chain Rust refuses is refused here, for the same rule", async () => {
  assert.ok(R.invalid.length >= 18);
  let ruled = 0;
  for (const bad of R.invalid) {
    let error: string | null = null;
    let reason: string | null = null;
    try {
      await ChainState.verify(entriesOf(bad.entries), undefined, hist());
    } catch (e) {
      assert.ok(e instanceof ChainError, `${bad.name}: ${e}`);
      error = e.kind;
      reason = e.detail.reason ?? null;
    }
    assert.equal(error, bad.error, bad.name);
    assert.equal(reason, bad.rule, bad.name);
    if (bad.rule) ruled += 1;
  }
  assert.ok(ruled >= 15, `${ruled} refusals named by their rule`);
});

test("§8 items 2–3: one admin cannot rebind a host alone, a used proof does not apply again, and no host takes another device's key", async () => {
  const base = await upto("removalKeys");
  const k0 = await dev0();
  const oldHost = await fixtureKeys(R.hostKeys[0]);
  const thief = await freshKeys("thief");
  // The honest renewal goes through, and owes the host's next machine key at once.
  const honest = await tryPush(base, { type: "rotateHostKey", rotation: await renewal(base, "h-2", oldHost.key, thief) });
  assert.equal(honest.hostKeyDue, "h-2");
  assert.deepEqual(honest.keysAt("h-2", base.seq), { id: "h-2", signKey: oldHost.signKey, dhKey: oldHost.dhKey });
  await assert.rejects(tryPush(honest, { type: "removeDevice3", id: "dev-2" }), rule(RULES.hostKeyNext));
  const keyed = await tryPush(honest, { type: "keyEpochs", user: null, machines: [{ machine: "h-2", epoch: 2, hash: await machineKeyCommit("acct-1", "h-2", 2, new Uint8Array(32)) }] });
  assert.equal(keyed.hostKeyDue, null);
  assert.equal(base.retiredKeys("h-2").length, 0, "the view it was tried on is untouched");

  // 1. A stolen admin device: it signs the entry and the new key's half, never the host's current key's.
  await assert.rejects(tryPush(base, { type: "rotateHostKey", rotation: await renewal(base, "h-2", thief.key, thief) }), rule(RULES.rotateProof));
  await assert.rejects(tryPush(base, { type: "rotateHostKey", rotation: await renewal(base, "h-2", k0.key, thief) }), rule(RULES.rotateProof));
  const bare = await renewal(base, "h-2", oldHost.key, thief);
  await assert.rejects(tryPush(base, { type: "rotateHostKey", rotation: { ...bare, oldSig: new Uint8Array(64) } }), rule(RULES.rotateProof));
  await assert.rejects(tryPush(base, { type: "rotateHostKey", rotation: { ...bare, assurance: ASSURANCE_ON_DEVICE } }), rule(RULES.rotateProof), "the assurance is under both signatures");
  // Nor may anyone but an active admin write it.
  await assert.rejects(tryPush(base, { type: "rotateHostKey", rotation: bare }, { type: "device", id: "dev-2" }, (await fixtureKeys(V.chain.devices[2])).key), rule(RULES.onlyAdminSigns));

  // 2. A proof already used, replayed: at the next renewal, and after the one after it.
  const used = entriesOf(R.entries)[seqOf("rotate")]!.draft.action;
  assert.equal(used.type, "rotateHostKey");
  await assert.rejects(tryPush(await upto("rotateKey"), used), rule(RULES.rotateKeyReused));
  await assert.rejects(tryPush(await upto("rotateAgainKey"), used), rule(RULES.rotateKeyReused));

  // 3. Another device's keys: dev-0's (with its key signing), m-1's DH key, a removed device's key, a retired one.
  const m1 = { ...(await freshKeys("m-1 dh")), dhKey: h(V.chain.machine.dhKey) };
  const dev1 = await fixtureKeys(V.chain.devices[1]);
  for (const [what, keys] of [
    ["dev-0", k0],
    ["m-1's DH key", m1],
    ["dev-1 (removed)", dev1],
    ["h-2's own", oldHost],
  ] as [string, Keys][]) {
    await assert.rejects(tryPush(base, { type: "rotateHostKey", rotation: await renewal(base, "h-2", oldHost.key, keys) }), rule(RULES.rotateKeyReused), what);
  }
  const renewed = await upto("rotateKey");
  const current = await fixtureKeys(R.hostKeys[1]);
  await assert.rejects(tryPush(renewed, { type: "rotateHostKey", rotation: await renewal(renewed, "h-2", current.key, oldHost) }), rule(RULES.rotateKeyReused), "back to a retired key");
  await assert.rejects(tryPush(renewed, { type: "rotateHostKey", rotation: await renewal(renewed, "h-2", oldHost.key, thief) }), rule(RULES.rotateProof), "signed by the retired key");
  // A browser or phone, an admin, a host that never was: no renewal.
  for (const id of ["dev-2", "dev-0", "h-9"]) {
    const r = await renewal(base, "h-2", oldHost.key, thief);
    await assert.rejects(tryPush(base, { type: "rotateHostKey", rotation: { ...r, host: id } }), rule(RULES.rotateNotHost), id);
  }
  // The view it was proved against must be this chain's, before the entry.
  await assert.rejects(tryPush(base, { type: "rotateHostKey", rotation: { ...bare, stagedSeq: base.seq + 1 } }), rule(RULES.rotateStaged));
  await assert.rejects(tryPush(base, { type: "rotateHostKey", rotation: { ...bare, stagedHead: new Uint8Array(32) } }), rule(RULES.rotateStaged));
});

test("a retired key never names a device again — added, enrolled, back with the words or by a passkey — while a current device's still may", async () => {
  const state = await ChainState.verify(entriesOf(R.entries), undefined, hist());
  const [old, first, current] = await Promise.all(R.hostKeys.map(fixtureKeys));
  for (const k of [old.signKey, old.dhKey, first.signKey, first.dhKey]) assert.ok(state.isRetiredKey(k));
  assert.equal(state.isRetiredKey(current.signKey), false, "h-2's current key is not retired");
  assert.equal(state.isRetiredKey(h(V.chain.devices[1].dhKey)), false, "nor a removed device's");
  const fresh = await freshKeys("retired");
  const add = (id: string, signKey: Uint8Array, dhKey: Uint8Array, host = false): Action => ({ type: "addDevice3", device: { id, signKey, dhKey, flags: { host, admin: false } } });
  for (const [what, action] of [
    ["the first retired sign key", add("d-k", old.signKey, fresh.dhKey)],
    ["the first retired DH key", add("d-k", fresh.signKey, old.dhKey)],
    ["the second retired sign key", add("d-k", first.signKey, fresh.dhKey)],
    ["the second retired DH key", add("d-k", fresh.signKey, first.dhKey)],
    ["a host enrolled with them", add("h-k", first.signKey, first.dhKey, true)],
  ] as [string, Action][]) {
    await assert.rejects(tryPush(state, action), rule(RULES.keyRetired), what);
  }
  await tryPush(state, add("d-k", fresh.signKey, fresh.dhKey));
  // Back with the recovery words: the same refusal (the fixture's recovery key is still the account's).
  const rk = await recoveryKeys(h(V.chain.recoverySecret));
  await assert.rejects(
    tryPush(state, { type: "recoveryJoin3", device: { id: "d-r", signKey: fresh.signKey, dhKey: old.dhKey, flags: { host: false, admin: true } } }, { type: "recovery" }, rk.signing),
    rule(RULES.keyRetired),
  );
  // Before the renewal that retired it, the same key was only a current device's: a second device naming it is what chains always allowed.
  await tryPush(await upto("removalKeys"), add("d-k", fresh.signKey, old.dhKey));
});

test("R1b: a pending join made with a host's key before its renewal is neither approved nor activated after it", async () => {
  const base = await upto("removalKeys");
  const k0 = await dev0();
  const oldHost = await fixtureKeys(R.hostKeys[0]);
  const rk = await recoveryKeys(h(V.chain.recoverySecret));
  const fresh = await freshKeys("r1b");
  const push = async (st: ChainState, action: Action, signer: Signer = byDev0, key?: CryptoKey) =>
    st.apply(await signDraft(st.draft(signer, action, 50_000 + st.seq), key ?? k0.key), hist());
  const renew = async (st: ChainState) => {
    await push(st, { type: "rotateHostKey", rotation: await renewal(st, "h-2", oldHost.key, fresh) });
    await push(st, { type: "keyEpochs", user: null, machines: [{ machine: "h-2", epoch: st.machineKeyEpochs("h-2"), hash: await machineKeyCommit("acct-1", "h-2", st.machineKeyEpochs("h-2"), new Uint8Array(32)) }] });
  };
  const early = { id: "d-k0", signKey: oldHost.signKey, dhKey: oldHost.dhKey };
  const recovery: Signer = { type: "recovery" };
  // Back with the recovery words, with h-2's keys while they were current: approved or activated then, fine.
  const joined = base.clone();
  await push(joined, { type: "recoveryJoin3", device: { ...early, flags: { host: false, admin: true } } }, recovery, rk.signing);
  await tryPush(joined, { type: "approveDevice", id: "d-k0" });
  await tryPush(joined, { type: "activateRecovered", id: "d-k0" }, recovery, rk.signing);
  // After the renewal: neither.
  await renew(joined);
  await assert.rejects(tryPush(joined, { type: "approveDevice", id: "d-k0" }), rule(RULES.keyRetired));
  await assert.rejects(tryPush(joined, { type: "activateRecovered", id: "d-k0" }, recovery, rk.signing), rule(RULES.keyRetired));
  assert.equal(joined.deviceRecord("d-k0")!.status.type, "pending");
  // Joined with a passkey: the same.
  const p = V.chainV3.passkeys[0];
  const slot = { slot: p.slot, key: h(p.key), proof: h(p.proof), credHash: h(p.credHash), saltNonce: h(p.saltNonce), grants: p.grants, selfActivate: p.selfActivate };
  const pak = await importEd25519Private(h(p.seed));
  const byPasskey: Signer = { type: "passkey", slot: p.slot };
  const slotted = base.clone();
  await push(slotted, { type: "passkeySlotSet", slot });
  await push(slotted, { type: "addDevice3", device: { ...early, flags: { host: false, admin: false } } }, byPasskey, pak);
  await tryPush(slotted, { type: "passkeyActivate", id: "d-k0" }, byPasskey, pak);
  await renew(slotted);
  await assert.rejects(tryPush(slotted, { type: "passkeyActivate", id: "d-k0" }, byPasskey, pak), rule(RULES.keyRetired));
  await assert.rejects(tryPush(slotted, { type: "approveDevice", id: "d-k0" }), rule(RULES.keyRetired), "nor an admin's approval");
  // A pending device whose keys were never retired is approved as before.
  const other = joined.clone();
  const ok = await freshKeys("r1b ok");
  await push(other, { type: "recoveryJoin3", device: { id: "d-ok", signKey: ok.signKey, dhKey: ok.dhKey, flags: { host: false, admin: true } } }, recovery, rk.signing);
  await tryPush(other, { type: "approveDevice", id: "d-ok" });
});

test("R1c: Rust's disputes verify here and re-sign to the same bytes; the ones that must not, do not", async () => {
  const state = await ChainState.verify(entriesOf(R.entries), undefined, hist());
  const d = R.disputes;
  assert.ok(d.valid.length >= 2 && d.invalid.length >= 7);
  for (const c of d.valid) {
    assert.equal(hex(state.hashAt(c.seq)!), c.entryHash, "the entry's hash");
    assert.equal(hex(rotateDisputeBody(state.account, c.host, c.seq, h(c.entryHash), h(c.newSign))), c.body);
    const sig = await signRotateDispute(await importEd25519Private(h(c.signerSeed)), state.account, c.host, c.seq, h(c.entryHash), h(c.newSign));
    assert.equal(hex(sig), c.signature, "Ed25519 is deterministic");
    assert.ok(await state.verifyRotateDispute(c.host, c.seq, h(c.signature)), `seq ${c.seq}`);
  }
  for (const c of d.invalid) assert.equal(await state.verifyRotateDispute(c.host, c.seq, h(c.signature)), false, c.name);
});

test("R1c: a dispute names the renewal by its hash, signed by the key that renewal retired", async () => {
  const entries = entriesOf(R.entries);
  const state = await ChainState.verify(entries, undefined, hist());
  const [old, first] = await Promise.all(R.hostKeys.slice(0, 2).map(fixtureKeys));
  const rotate = seqOf("rotate");
  const hashOf = (seq: number) => state.hashAt(seq)!;
  const sign = (key: CryptoKey, host: string, seq: number, entryHash: Uint8Array, newSign: Uint8Array) => signRotateDispute(key, "acct-1", host, seq, entryHash, newSign);
  const honest = await sign(old.key, "h-2", rotate, hashOf(rotate), first.signKey);
  assert.ok(await state.verifyRotateDispute("h-2", rotate, honest));
  assert.equal(rotateDisputeBody("acct-1", "h-2", rotate, hashOf(rotate), first.signKey).length, 2 + 6 + 2 + 3 + 8 + 32 + 32);
  const k0 = (await dev0()).key;
  for (const [what, sig] of [
    ["the new key", await sign(first.key, "h-2", rotate, hashOf(rotate), first.signKey)],
    ["another device", await sign(k0, "h-2", rotate, hashOf(rotate), first.signKey)],
    ["another hash", await sign(old.key, "h-2", rotate, new Uint8Array(32).fill(7), first.signKey)],
    ["another new key", await sign(old.key, "h-2", rotate, hashOf(rotate), new Uint8Array(32).fill(7))],
    ["another account", await signRotateDispute(old.key, "acct-2", "h-2", rotate, hashOf(rotate), first.signKey)],
  ] as [string, Uint8Array][]) {
    assert.equal(await state.verifyRotateDispute("h-2", rotate, sig), false, what);
  }
  assert.equal(await state.verifyRotateDispute("m-1", rotate, await sign(old.key, "m-1", rotate, hashOf(rotate), first.signKey)), false, "another host");
  for (const seq of [0, -1, 1.5, state.seq + 1, rotate + 1]) assert.equal(await state.verifyRotateDispute("h-2", seq, honest), false, `seq ${seq}`);
  // A view without the renewal cannot check it; the renewal's own proof signature is no statement.
  assert.equal(await (await upto("removalKeys")).verifyRotateDispute("h-2", rotate, honest), false);
  const a = entries[rotate]!.draft.action;
  assert.ok(a.type === "rotateHostKey");
  assert.equal(await state.verifyRotateDispute("h-2", rotate, a.rotation.oldSig), false);
  assert.throws(() => rotateDisputeBody("acct-1", "h-2", rotate, new Uint8Array(31), first.signKey), /32-byte/);
  assert.throws(() => rotateDisputeBody("acct-1", "h-2", rotate, hashOf(rotate), new Uint8Array(33)), /32-byte/);
});

test("R1d: Rust's renewal policy chain verifies here step by step, and says what the policy is", async () => {
  const p = R.policy;
  const entries = entriesOf(p.entries);
  entries.forEach((e, i) => assert.equal(hex(encodeEntry(e)), p.entries[i], `entry ${i} round-trips`));
  const k0 = (await dev0()).key;
  for (const e of entries) {
    if (e.draft.signer.type === "device" && e.draft.signer.id === "dev-0") assert.equal(hex(encodeEntry(await signDraft(e.draft, k0))), hex(encodeEntry(e)), `entry ${e.draft.seq} re-signed`);
  }
  for (const step of p.steps) {
    const st = await ChainState.verify(entries.slice(0, step.seq + 1), undefined, hist());
    assert.deepEqual({ alwaysAssured: st.alwaysAssured, renewalPolicySeq: st.renewalPolicySeq }, { alwaysAssured: step.alwaysAssured, renewalPolicySeq: step.renewalPolicySeq }, step.label);
  }
  for (const which of ["on", "off"]) assert.deepEqual(decodeEntry(h(p.encoding[which])).draft.action, { type: "setRenewalPolicy", alwaysAssured: which === "on" });
});

test("R1d: while the account asks for it, only renewals confirmed on the device go through; only an admin sets it", async () => {
  const st = await upto("removalKeys");
  assert.equal(st.alwaysAssured, false, "off until an admin turns it on");
  assert.equal(st.renewalPolicySeq, null);
  const oldHost = await fixtureKeys(R.hostKeys[0]);
  const [first, second] = await Promise.all(R.hostKeys.slice(1, 3).map(fixtureKeys));
  const policy = (on: boolean): Action => ({ type: "setRenewalPolicy", alwaysAssured: on });
  await assert.rejects(tryPush(st, policy(true), { type: "device", id: "dev-2" }, (await fixtureKeys(V.chain.devices[2])).key), rule(RULES.onlyAdminSigns));
  await assert.rejects(tryPush(st, policy(true), { type: "device", id: "h-2" }, oldHost.key), rule(RULES.onlyAdminSigns), "the host itself");
  const on = await tryPush(st, policy(true));
  assert.equal(on.alwaysAssured, true);
  assert.equal(on.renewalPolicySeq, st.seq + 1);
  assert.equal(st.alwaysAssured, false, "the view it was tried on is untouched");
  const again = await tryPush(on, policy(true));
  assert.equal(again.renewalPolicySeq, on.seq + 1, "setting it again is allowed");
  // assurance 0: refused for that reason; assurance 1: taken.
  await assert.rejects(tryPush(on, { type: "rotateHostKey", rotation: await renewal(on, "h-2", oldHost.key, first) }), rule(RULES.rotateNeedsAssurance));
  const assured = await tryPush(on, { type: "rotateHostKey", rotation: await renewal(on, "h-2", oldHost.key, first, ASSURANCE_ON_DEVICE) });
  assert.equal(assured.retiredKeys("h-2")[0]!.byAssured, true);
  // A bad proof is still refused as one.
  const bare = await renewal(on, "h-2", oldHost.key, first);
  await assert.rejects(tryPush(on, { type: "rotateHostKey", rotation: { ...bare, oldSig: new Uint8Array(64) } }), rule(RULES.rotateProof));
  // Off: automatic renewals again.
  const keyed = await tryPush(assured, { type: "keyEpochs", user: null, machines: [{ machine: "h-2", epoch: assured.machineKeyEpochs("h-2"), hash: await machineKeyCommit("acct-1", "h-2", assured.machineKeyEpochs("h-2"), new Uint8Array(32)) }] });
  const off = await tryPush(keyed, policy(false));
  assert.equal(off.alwaysAssured, false);
  await tryPush(off, { type: "rotateHostKey", rotation: await renewal(off, "h-2", first.key, second) });
  // Encoding: kind 21, then one byte 0 or 1; nothing else decodes, nor kind 21 under v2.
  for (const [view, byte] of [
    [on, 1],
    [off, 0],
  ] as [ChainState, number][]) {
    const e = view.entryAt(view.renewalPolicySeq!)!;
    const bytes = encodeEntry(e);
    assert.deepEqual([...bytes.slice(bytes.length - 66, bytes.length - 64)], [21, byte]);
    for (const bad of [2, 0x80, 0xff]) {
      const b = Uint8Array.from(bytes);
      b[b.length - 65] = bad;
      assert.throws(() => decodeEntry(b), kind("decode"), `byte ${bad}`);
    }
    const v2 = Uint8Array.from(bytes);
    v2[0] = CHAIN_VERSION;
    assert.throws(() => decodeEntry(v2), kind("decode"), "kind 21 under version 2");
  }
  // clone() keeps the policy.
  assert.equal(on.clone().alwaysAssured, true);
});

test("R1e: Rust's renewal pause chain verifies here step by step — the same open pauses, bases, paused edges and control", async () => {
  const p = R.pause;
  const entries = entriesOf(p.entries);
  entries.forEach((e, i) => assert.equal(hex(encodeEntry(e)), p.entries[i], `entry ${i} round-trips`));
  const k0 = (await dev0()).key;
  for (const e of entries) {
    if (e.draft.signer.type === "device" && e.draft.signer.id === "dev-0") assert.equal(hex(encodeEntry(await signDraft(e.draft, k0))), hex(encodeEntry(e)), `entry ${e.draft.seq} re-signed`);
  }
  for (const step of p.steps) {
    const st = await ChainState.verify(entries.slice(0, step.seq + 1), undefined, hist());
    assert.deepEqual(pauseStateJson(st, p.hosts, p.pairs), step.state, step.label);
  }
  for (const [which, phase] of [
    ["start", PAUSE_START],
    ["completed", PAUSE_COMPLETED],
    ["cancelled", PAUSE_CANCELLED],
  ] as [string, number][]) {
    const a = decodeEntry(h(p.encoding[which])).draft.action;
    assert.ok(a.type === "renewalPause" && a.phase === phase, which);
  }
  const labels = p.steps.map((s: Json) => s.label);
  for (const label of ["pauseStart", "pauseCompleted", "pause2Cancelled", "overlapCompleted", "pausedHostStops", "pausedDeviceRemoved", "overlap2Completed"]) {
    assert.ok(labels.includes(label), `Rust's pause chain has ${label}`);
  }
  assert.equal(p.maxEntryLen, MAX_PAUSE_ENTRY_LEN, "one size bound on both sides");
});

test("R1e: a renewal pause overlays the accept policy — never an admin — and leaves the base alone; any admin ends it", async () => {
  const st = (await upto("removalKeys")).clone();
  const k0 = (await dev0()).key;
  const da = await freshKeys("d-a");
  const push = async (s: ChainState, action: Action, signer: Signer = byDev0, key: CryptoKey = k0) => s.apply(await signDraft(s.draft(signer, action, 80_000 + s.seq), key), hist());
  const marker = (op: Uint8Array, phase: PausePhase, removals: PauseRemoval[]): Action => ({ type: "renewalPause", op, phase, removals });
  const accept = (hostId: string, from: Accept): Action => ({ type: "setAccept", host: hostId, from });
  const ok = (s: ChainState, d: string, hostId: string) => {
    try {
      s.mayControl(d, hostId);
      return true;
    } catch (e) {
      assert.ok(e instanceof ChainError);
      return false;
    }
  };
  const [a, b] = [new Uint8Array(16).fill(0xa1), new Uint8Array(16).fill(0xb2)];
  const byA: Signer = { type: "device", id: "d-a" };
  await push(st, accept("m-1", { type: "all" }));
  await push(st, accept("h-2", { type: "devices", ids: ["dev-2", "m-1"] }));
  await push(st, { type: "addDevice3", device: { id: "d-a", signKey: da.signKey, dhKey: da.dhKey, flags: { host: false, admin: true } } });
  assert.ok(ok(st, "h-2", "m-1") && ok(st, "m-1", "h-2") && ok(st, "dev-0", "m-1"));
  const removals = [
    { host: "h-2", ids: ["m-1"] },
    { host: "m-1", ids: ["dev-2", "h-2"] },
  ];
  await push(st, marker(a, PAUSE_START, removals));
  assert.ok(!ok(st, "h-2", "m-1") && !ok(st, "m-1", "h-2") && !ok(st, "dev-2", "m-1"), "the paused edges are refused");
  assert.throws(() => st.mayControl("dev-2", "m-1"), /control is paused while identity keys are renewed/);
  assert.ok(ok(st, "dev-0", "m-1") && ok(st, "d-a", "m-1") && ok(st, "dev-2", "h-2"), "the rest stands, admins always");
  assert.throws(() => st.authorizeControl("h-2", h(R.hostKeys[0].dhKey), "m-1"), kind("notAccepted"));
  assert.deepEqual(st.baseAccept("m-1"), { type: "all" });
  assert.deepEqual(st.pausedEdges("m-1"), ["dev-2", "h-2"]);
  assert.deepEqual(
    st.openPauses().map((x: OpenPause) => [hex(x.op), x.signer, x.removals]),
    [[hex(a), "dev-0", removals]],
  );
  // A hand setAccept during the pause changes only the base.
  await push(st, accept("m-1", { type: "devices", ids: ["dev-2", "h-2"] }));
  assert.deepEqual(st.baseAccept("m-1"), { type: "devices", ids: ["dev-2", "h-2"] });
  assert.ok(!ok(st, "h-2", "m-1"), "listed in the base, paused all the same");
  // Another admin completes it: the overlay lifts.
  await push(st, marker(a, PAUSE_COMPLETED, []), byA, da.key);
  assert.deepEqual(st.openPauses(), []);
  assert.ok(ok(st, "h-2", "m-1") && ok(st, "m-1", "h-2") && ok(st, "dev-2", "m-1") && ok(st, "dev-0", "m-1"));
  // A pause whose starter is demoted stays in force; a demoted admin ends nothing; another admin does.
  await push(st, marker(b, PAUSE_START, [{ host: "m-1", ids: ["dev-2"] }]), byA, da.key);
  await push(st, { type: "setAdmin", id: "d-a", admin: false });
  assert.deepEqual(st.pausedEdges("m-1"), ["dev-2"]);
  await assert.rejects(tryPush(st, marker(b, PAUSE_CANCELLED, []), byA, da.key), rule(RULES.onlyAdminSigns));
  await push(st, marker(b, PAUSE_CANCELLED, []));
  assert.ok(ok(st, "dev-2", "m-1"));
  // A device made admin while paused stays paused until the pause ends: a pause only ever narrows.
  const d3 = new Uint8Array(16).fill(0xd3);
  const promoted = await tryPush(st, marker(d3, PAUSE_START, [{ host: "m-1", ids: ["dev-2"] }]));
  await push(promoted, { type: "setAdmin", id: "dev-2", admin: true });
  assert.ok(!ok(promoted, "dev-2", "m-1"), "an admin now, paused all the same");
  await push(promoted, marker(d3, PAUSE_COMPLETED, []));
  assert.ok(ok(promoted, "dev-2", "m-1"));
  // The rules.
  const c = new Uint8Array(16).fill(0xc1);
  await assert.rejects(tryPush(st, marker(a, PAUSE_START, [{ host: "m-1", ids: ["dev-2"] }])), rule(RULES.pauseOpReused));
  for (const bad of [
    [],
    [{ host: "dev-2", ids: ["m-1"] }],
    [{ host: "dev-1", ids: ["m-1"] }],
    [{ host: "m-1", ids: ["dev-2"] }, { host: "m-1", ids: ["h-2"] }],
    [{ host: "m-1", ids: ["dev-2"] }, { host: "h-2", ids: ["dev-2"] }],
    Array.from({ length: 257 }, (_, i) => ({ host: `h-${i}`, ids: ["dev-2"] })),
  ]) {
    await assert.rejects(tryPush(st, marker(c, PAUSE_START, bad)), rule(RULES.pauseHosts), JSON.stringify(bad).slice(0, 80));
  }
  const many = Array.from({ length: 513 }, (_, i) => `d-${i}`);
  await assert.rejects(tryPush(st, marker(c, PAUSE_START, [{ host: "h-2", ids: many }, { host: "m-1", ids: many }])), rule(RULES.pauseEdges));
  for (const ids of [[], ["m-1"], ["nobody"], ["dev-2", "dev-2"], ["h-2", "dev-2"], ["dev-0"]]) {
    await assert.rejects(tryPush(st, marker(c, PAUSE_START, [{ host: "m-1", ids }])), rule(RULES.pauseIds), ids.join(","));
  }
  // The whole entry fits every transport: one byte past MAX_PAUSE_ENTRY_LEN is refused for that alone; at it, the ids'
  // own rule speaks.
  const sized = (len: number): Action => {
    const length = (ids: string[]) => encodeDraft(st.draft(byDev0, marker(c, PAUSE_START, [{ host: "m-1", ids }]), 0)).length + 64;
    const ids: string[] = [];
    while (length(ids) + 2 * 66 <= len) ids.push(`x${String(ids.length).padStart(63, "0")}`);
    const rest = len - length(ids) - 4;
    ids.push("y".repeat(Math.floor(rest / 2)), "z".repeat(rest - Math.floor(rest / 2)));
    assert.equal(length(ids), len);
    return marker(c, PAUSE_START, [{ host: "m-1", ids }]);
  };
  await assert.rejects(tryPush(st, sized(MAX_PAUSE_ENTRY_LEN + 1)), rule(RULES.pauseTooLarge));
  await assert.rejects(tryPush(st, sized(MAX_PAUSE_ENTRY_LEN)), rule(RULES.pauseIds));
  await tryPush(st, marker(c, PAUSE_START, [{ host: "m-1", ids: ["dev-1"] }]));
  await assert.rejects(tryPush(st, marker(a, PAUSE_COMPLETED, [])), rule(RULES.pauseNotOpen), "twice");
  await assert.rejects(tryPush(st, marker(new Uint8Array(16).fill(0xcc), PAUSE_CANCELLED, [])), rule(RULES.pauseNotOpen), "unknown");
  const open = await tryPush(st, marker(c, PAUSE_START, [{ host: "m-1", ids: ["dev-2"] }]));
  await assert.rejects(tryPush(open, marker(c, PAUSE_COMPLETED, [{ host: "m-1", ids: ["dev-2"] }])), rule(RULES.pauseEndRemovals));
  await assert.rejects(tryPush(st, marker(c, PAUSE_START, [{ host: "m-1", ids: ["dev-2"] }]), { type: "device", id: "dev-2" }, (await fixtureKeys(V.chain.devices[2])).key), rule(RULES.onlyAdminSigns));
  const rk = await recoveryKeys(h(V.chain.recoverySecret));
  await assert.rejects(tryPush(st, marker(c, PAUSE_START, [{ host: "m-1", ids: ["dev-2"] }]), { type: "recovery" }, rk.signing), kind("notAllowed"));
  // Encoding: phase 0/1/2 only, kind 22 only under v3; too many hosts or edges do not decode.
  const cancelled = encodeEntry(st.entryAt(st.seq)!);
  const at = cancelled.length - 64 - 2 - 1;
  assert.equal(cancelled[at], PAUSE_CANCELLED);
  for (const x of [3, 0x80, 0xff]) {
    const y = Uint8Array.from(cancelled);
    y[at] = x;
    assert.throws(() => decodeEntry(y), kind("decode"), `phase ${x}`);
  }
  const v2 = Uint8Array.from(cancelled);
  v2[0] = CHAIN_VERSION;
  assert.throws(() => decodeEntry(v2), kind("decode"));
  for (const removalsTooBig of [Array.from({ length: 257 }, (_, i) => ({ host: `h-${i}`, ids: ["d"] })), [{ host: "m-1", ids: Array.from({ length: 1025 }, (_, i) => `d-${i}`) }]]) {
    const e = await signDraft(st.draft(byDev0, marker(c, PAUSE_START, removalsTooBig), 1), k0);
    assert.throws(() => decodeEntry(encodeEntry(e)), kind("decode"));
  }
  // clone() keeps pauses apart.
  const copyState = open.clone();
  await push(copyState, marker(c, PAUSE_CANCELLED, []));
  assert.deepEqual(open.pausedEdges("m-1"), ["dev-2"]);
});

test("R1e: the largest renewal pauses fit every transport; a longer entry or count is refused, never cut", async () => {
  // As in Rust (handshake.rs): MAX_HELLO_ENTRIES pauses of MAX_PAUSE_ENTRY_LEN bytes fill a hello and a reply beside the
  // most info, within what a handshake message carries; one fills a `chain` frame.
  const st = await upto("removalKeys");
  const k0 = (await dev0()).key;
  const pauseOf = (ids: string[]) => st.draft(byDev0, { type: "renewalPause", op: new Uint8Array(16).fill(1), phase: PAUSE_START, removals: [{ host: "m-1", ids }] }, 1);
  const ids: string[] = [];
  while (encodeDraft(pauseOf(ids)).length + 64 + 2 * 66 <= MAX_PAUSE_ENTRY_LEN) ids.push(`x${String(ids.length).padStart(63, "0")}`);
  const rest = MAX_PAUSE_ENTRY_LEN - (encodeDraft(pauseOf(ids)).length + 64) - 4;
  ids.push("y".repeat(Math.floor(rest / 2)), "z".repeat(rest - Math.floor(rest / 2)));
  const largest = await signDraft(pauseOf(ids), k0);
  assert.equal(encodeEntry(largest).length, MAX_PAUSE_ENTRY_LEN);
  const hello = { chainSeq: 9, chainHead: new Uint8Array(32).fill(2), entries: Array(MAX_HELLO_ENTRIES).fill(largest), client: new Uint8Array(MAX_INFO).fill(7) };
  const helloBytes = encodeHelloBody(hello);
  assert.ok(helloBytes.length <= MAX_HELLO_PAYLOAD, `${helloBytes.length} bytes`);
  assert.equal(decodeHelloBody(helloBytes).entries.length, MAX_HELLO_ENTRIES);
  const reply = { status: "accepted" as const, chainSeq: 9, chainHead: new Uint8Array(32).fill(2), needFrom: 0, server: new Uint8Array(MAX_INFO).fill(7), entries: Array(MAX_HELLO_ENTRIES).fill(largest) };
  const replyBytes = encodeReplyBody(reply);
  assert.ok(replyBytes.length <= MAX_HELLO_PAYLOAD, `${replyBytes.length} bytes`);
  assert.equal(decodeReplyBody(replyBytes).entries.length, MAX_HELLO_ENTRIES);
  const frame = decodeFrame(encodeFrame({ type: "chain", ch: 0, entries: [encodeEntry(largest)] }));
  assert.ok(frame.type === "chain" && hex(frame.entries[0]!) === hex(encodeEntry(largest)));
  // Past a u16 length: refused, never cut (the encoder used to wrap the length).
  const huge = await signDraft(pauseOf(Array.from({ length: 1100 }, (_, i) => String(i).padStart(64, "0"))), k0);
  assert.ok(encodeEntry(huge).length > 0xffff);
  assert.throws(() => encodeHelloBody({ ...hello, entries: [huge] }), /too long/);
  assert.throws(() => encodeReplyBody({ ...reply, entries: [huge] }), /too long/);
  assert.throws(() => encodeFrame({ type: "chain", ch: 0, entries: [encodeEntry(huge)] }));
  // No field clamps or wraps: a count past 0xffff throws.
  assert.throws(() => new Writer().u16(0x10000), /does not fit a u16/);
  assert.throws(() => new Writer().u8(256), /does not fit a u8/);
  assert.throws(() => new Writer().u32(-1), /does not fit a u32/);
  const wide = Array.from({ length: 0x10000 }, () => ({ host: "m-1", ids: ["dev-2"] }));
  assert.throws(() => encodeDraft(st.draft(byDev0, { type: "renewalPause", op: new Uint8Array(16), phase: PAUSE_START, removals: wide }, 1)), /does not fit a u16/);
  const longList = Array.from({ length: 0x10000 }, (_, i) => `d-${i}`);
  assert.throws(() => encodeDraft(st.draft(byDev0, { type: "setAccept", host: "m-1", from: { type: "devices", ids: longList } }, 1)), /does not fit a u16/);
});

test("R1b: a renewal's nonce is 16 bytes, in the proof and in the entry", async () => {
  const base = await upto("removalKeys");
  const oldHost = await fixtureKeys(R.hostKeys[0]);
  const good = await renewal(base, "h-2", oldHost.key, await freshKeys("nonce"));
  for (const n of [15, 17, 0]) {
    const nonce = new Uint8Array(n);
    assert.throws(() => hostRotationBody(base.account, { ...good, nonce }, oldHost.signKey, oldHost.dhKey), /16 bytes/, `${n} in the proof`);
    assert.throws(() => encodeDraft(base.draft(byDev0, { type: "rotateHostKey", rotation: { ...good, nonce } }, 1)), /16 bytes/, `${n} in the entry`);
  }
  assert.equal(RENEWAL_NONCE_LEN, 16);
  encodeDraft(base.draft(byDev0, { type: "rotateHostKey", rotation: good }, 1));
});

test("kind 20 exists in v3 only, and its assurance is 0 or 1", async () => {
  const entries = entriesOf(R.entries);
  const rotate = entries[seqOf("rotate")]!;
  const bytes = encodeEntry(rotate);
  const at = bytes.length - 64 * 3 - 1;
  assert.equal(bytes[at], ASSURANCE_AUTO);
  assert.equal(encodeEntry(entries[seqOf("rotateAgain")]!)[at], ASSURANCE_ON_DEVICE);
  for (const v of [2, 3, 0x80, 0xff]) {
    const b = Uint8Array.from(bytes);
    b[at] = v;
    assert.throws(() => decodeEntry(b), kind("decode"), `assurance ${v}`);
  }
  const v2 = Uint8Array.from(bytes);
  v2[0] = CHAIN_VERSION;
  assert.throws(() => decodeEntry(v2), kind("decode"), "kind 20 under version 2");
  // A draft that says v2 is refused for its version.
  const base = await upto("removalKeys");
  const draft = base.draft(byDev0, { type: "rotateHostKey", rotation: await renewal(base, "h-2", (await fixtureKeys(R.hostKeys[0])).key, await freshKeys("v2")) }, 1);
  assert.equal(draft.version, 3);
  await assert.rejects(base.clone().apply(await signDraft({ ...draft, version: CHAIN_VERSION }, (await dev0()).key), hist()), kind("version"));
});
