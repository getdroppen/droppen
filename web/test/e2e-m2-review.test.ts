// The M2 security review's proofs-of-concept against the protocol, kept as
// lib-level regressions (Rust's twins are `attack_regressions::m2`): each runs
// the attack against `web/src/lib/e2e` and asserts that it now fails. The
// account-level PoCs (a whole CloudAccount on FakeKeysHub) live with the hub
// tests.
//
// Fixtures come from `rust.json`: the chain fixture's keys (dev-0 full, dev-1
// removed, dev-2 limited, machine m-1) and its recovery secret.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  type Action,
  createAttestation,
  createDelivery,
  ChainClock,
  ChainError,
  ChainState,
  context,
  decodeEntry,
  type DeviceInfo,
  encodeManifest,
  type Entry,
  fromHex,
  genesisDraft,
  hex,
  importEd25519Private,
  importMachineKey,
  keyPairFromSecret,
  machineKeyCommit,
  openDelivery,
  openEphemeral,
  openFromRecovery,
  openMachineKey,
  openUserKey,
  publicFromSecret,
  RECOVERY_DELAY_MS,
  RECOVERY_RECIPIENT,
  RecordError,
  type RecoveryKey,
  recoveryKeys,
  requireCommitted,
  type SealContext,
  sealEphemeral,
  sealForRecovery,
  sealTo,
  signDraft,
  signManifest,
  signStreamHead,
  streamId,
  streamKey,
  unwrapUnder,
  userKeyCommit,
  utf8,
  verifyRecoveryKey,
  WrapError,
  wrapUnder,
} from "../src/lib/e2e/index.js";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const V: any = JSON.parse(readFileSync(new URL("../../server-rs/crates/rh-proto/vectors/rust.json", import.meta.url), "utf8"));
const h = (s: string) => fromHex(s);
const fill = (b: number, n = 32) => new Uint8Array(n).fill(b);
const ACCOUNT = "acct-1";

/** The sample chain (seq 0–4): dev-0 full, dev-1 removed, dev-2 limited, m-1. */
const sampleEntries = (): Entry[] => V.chain.entries.map((e: string) => decodeEntry(h(e)));
const dev = async (i: number) => ({
  sign: await importEd25519Private(h(V.chain.devices[i].signSeed)),
  signPublic: h(V.chain.devices[i].signKey),
  dh: await keyPairFromSecret(h(V.chain.devices[i].dhSecret)),
});
const byDev0 = { type: "device", id: "dev-0" } as const;

/** Apply `actions` signed by dev-0 on top of `chain`, returning the new entries. */
async function extend(chain: ChainState, actions: Action[], clock = ChainClock.history()): Promise<Entry[]> {
  const d0 = await dev(0);
  const out = [];
  for (const action of actions) {
    const e = await signDraft(chain.draft(byDev0, action, 8_000), d0.sign);
    await chain.apply(e, clock);
    out.push(e);
  }
  return out;
}

/** The sample chain with user key epochs `uks` and m-1's key epochs `mks` committed (as Rust's `wrap::tests::committed`). */
async function committed(uks: Uint8Array[], mks: Uint8Array[]): Promise<ChainState> {
  const chain = await ChainState.verify(sampleEntries(), undefined, ChainClock.history());
  for (const [epoch, key] of uks.entries()) await extend(chain, [{ type: "keyEpochs", user: { epoch, hash: await userKeyCommit(ACCOUNT, epoch, key) }, machines: [] }]);
  for (const [epoch, key] of mks.entries()) {
    await extend(chain, [{ type: "keyEpochs", user: null, machines: [{ machine: "m-1", epoch, hash: await machineKeyCommit(ACCOUNT, "m-1", epoch, key) }] }]);
  }
  return chain;
}

const wrapKind = (kind: WrapError["kind"], cause?: ChainError["kind"]) => (e: unknown) => {
  if (!(e instanceof WrapError)) throw new Error(`not a WrapError: ${String(e)}`);
  assert.equal(e.kind, kind);
  if (cause) assert.equal((e.cause as ChainError).kind, cause);
  return true;
};
const chainKind = (kind: ChainError["kind"]) => (e: unknown) => e instanceof ChainError && e.kind === kind;
const rkCtx = (sender: string, epoch: number): SealContext => ({ purpose: "userKey", account: ACCOUNT, sender, recipient: RECOVERY_RECIPIENT, epoch });

// poc_removed_device_plants_recovery_package, and the TS PoC "a removed device
// plants a recovery package; a later recovery adopts its UK": the removed dev-1
// still holds the public RSK and seals a UK of its choosing for recovery.
test("a removed device's recovery package is refused, and so is any key the chain did not commit", async () => {
  const [uk0, uk1] = [fill(0x10), fill(0x11)];
  const chain = await committed([uk0, uk1], []);
  assert.equal(chain.device("dev-1"), undefined, "dev-1 is removed");
  const rsk = chain.currentRecoveryKey!;
  assert.ok(rsk, "RSK survives: dev-1 neither set it nor recovered with it");
  const rk = await recoveryKeys(h(V.chain.recoverySecret));
  const [d0, d1] = [await dev(0), await dev(1)];
  // The removed device plants a key of its own, for an existing epoch and for one minted after its removal:
  // whoever sealed a package, only the key the chain committed opens.
  for (const epoch of [1, 7]) {
    const sealed = await sealForRecovery(d1.dh, "dev-1", ACCOUNT, rsk, epoch, fill(0xaa));
    await assert.rejects(openFromRecovery(rk.seal, chain, rkCtx("dev-1", epoch), sealed), wrapKind("uncommitted"));
  }
  // What it sealed of a committed key before its removal still opens: it may be the only way back in for that epoch.
  const before = await sealForRecovery(d1.dh, "dev-1", ACCOUNT, rsk, 1, uk1);
  assert.equal(hex(await openFromRecovery(rk.seal, chain, rkCtx("dev-1", 1), before)), hex(uk1));
  // A current full device sealing a key the chain never committed, or an epoch it has not reached.
  const planted = await sealForRecovery(d0.dh, "dev-0", ACCOUNT, rsk, 1, fill(0xaa));
  await assert.rejects(openFromRecovery(rk.seal, chain, rkCtx("dev-0", 1), planted), wrapKind("uncommitted", "keyMismatch"));
  const ahead = await sealForRecovery(d0.dh, "dev-0", ACCOUNT, rsk, 2, fill(0xaa));
  await assert.rejects(openFromRecovery(rk.seal, chain, rkCtx("dev-0", 2), ahead), wrapKind("uncommitted", "uncommittedKey"));
  // Named as another device: its key does not open the seal. A sender the chain never named is refused as such.
  const genuine = await sealForRecovery(d0.dh, "dev-0", ACCOUNT, rsk, 1, uk1);
  await assert.rejects(openFromRecovery(rk.seal, chain, rkCtx("dev-2", 1), genuine));
  await assert.rejects(openFromRecovery(rk.seal, chain, rkCtx("dev-9", 1), genuine), wrapKind("unknownSender"));
  // The genuine package still opens.
  assert.equal(hex(await openFromRecovery(rk.seal, chain, rkCtx("dev-0", 1), genuine)), hex(uk1));
});

// poc_hub_fabricated_chain_fools_recovery: the hub builds a chain under its own
// AK around the account's real (public) RSK, for a fresh browser to recover into.
test("a hub-fabricated chain around the real RSK is refused: the copied proof is over another root", async () => {
  const realGenesis = sampleEntries()[0]!;
  assert.equal(realGenesis.draft.action.type, "genesis");
  const realRk = (realGenesis.draft.action as Extract<Action, { type: "genesis" }>).recoveryKey!;
  const real = await ChainState.verify(sampleEntries(), undefined, ChainClock.history());
  assert.equal(hex(realRk.key), hex(real.currentRecoveryKey!));
  const hubSeed = fill(0x90);
  const hubAk = await importEd25519Private(hubSeed);
  const hubAkPublic = await publicFromSecret("Ed25519", hubSeed);
  const hubDev: DeviceInfo = { id: "dev-h", signKey: await publicFromSecret("Ed25519", fill(0x91)), dhKey: await publicFromSecret("X25519", fill(0x92)), full: true };
  // Copying the real proof: it is over the real AK.
  assert.equal(await verifyRecoveryKey(realRk, ACCOUNT, hubAkPublic), false);
  const copied = await signDraft(genesisDraft(ACCOUNT, hubAkPublic, hubDev, realRk, 1_000), hubAk);
  await assert.rejects(ChainState.verify([copied], undefined, ChainClock.history()), (e: unknown) => chainKind("notAllowed")(e) && (e as ChainError).message.includes("proof"));
  // A proof of its own needs RSK's secret, which only RK gives.
  const forged: RecoveryKey = { key: realRk.key, proof: fill(0x42, 64) };
  const g = await signDraft(genesisDraft(ACCOUNT, hubAkPublic, hubDev, forged, 1_000), hubAk);
  await assert.rejects(ChainState.verify([g], undefined, ChainClock.history()), chainKind("notAllowed"));
  // Nor can a device of the real account set RSK without its proof over this account and root.
  const chain = await ChainState.verify(sampleEntries(), undefined, ChainClock.history());
  const d0 = await dev(0);
  const rk = await recoveryKeys(h(V.chain.recoverySecret));
  for (const key of [forged, { key: rk.signingPublic, proof: realRk.proof.slice().reverse() }]) {
    const e = await signDraft(chain.draft(byDev0, { type: "setRecoveryKey", key }, 6_000), d0.sign);
    await assert.rejects(chain.apply(e, ChainClock.history()), chainKind("notAllowed"));
  }
  assert.equal(chain.seq, 4, "nothing applied");
});

// poc_browser_page_load_accepts_instant_self_activation: a browser that
// re-verified with a history clock on every page load accepted a join and its
// activation delivered together.
test("a page load resumed live refuses an instant self-activation; a verifier that starts after the join waits a full delay", async () => {
  const entries = sampleEntries();
  const seenThrough = entries.length - 1; // what the browser had verified last time (seq 4)
  // What the browser saved then: a live clock's first sightings.
  const before = ChainClock.live(10_000, 0);
  await ChainState.verify(entries, undefined, before);
  const saved = [...before.firstSeen];
  const state = await ChainState.verify(entries, undefined, ChainClock.history());
  const rk = await recoveryKeys(h(V.chain.recoverySecret));
  const me: DeviceInfo = { id: "dev-r", signKey: h(V.chain.devices[3].signKey), dhKey: h(V.chain.devices[3].dhKey), full: true };
  const join = await signDraft(state.draft({ type: "recovery" }, { type: "recoveryJoin", device: me }, 6_000), rk.signing);
  await state.apply(join, ChainClock.history());
  const activate = await signDraft(state.draft({ type: "recovery" }, { type: "activateRecovered", id: "dev-r" }, 7_000), rk.signing);
  const all = [...entries, join, activate];
  // The next page load, a second later, resumes live from what it saved.
  const reload = ChainClock.resume(50_000, seenThrough, saved);
  assert.equal(reload.liveFromSeq, seenThrough + 1);
  await assert.rejects(ChainState.verify(all, undefined, reload), (e: unknown) => chainKind("tooEarly")(e) && (e as ChainError).detail.readyMs === 50_000 + RECOVERY_DELAY_MS);
  // What the browser used to do, and must not: history accepts it.
  assert.equal((await ChainState.verify(all, undefined, ChainClock.history())).device("dev-r")?.status.type, "active");
  // A verifier that starts after the join (an agent enrolled then, a fresh
  // browser on its first chain: live from head + 1) waits a full delay from then.
  const fresh = ChainClock.live(50_000, seenThrough + 2);
  const view = await ChainState.verify([...entries, join], undefined, fresh);
  fresh.nowMs = 50_000 + RECOVERY_DELAY_MS - 1;
  await assert.rejects(view.apply(activate, fresh), chainKind("tooEarly"));
  // Its saved clock carries the sighting across a reload.
  const again = ChainClock.resume(50_000 + RECOVERY_DELAY_MS - 1, seenThrough + 1, [...fresh.firstSeen]);
  const reloaded = await ChainState.verify([...entries, join], undefined, again);
  await assert.rejects(reloaded.apply(activate, again), chainKind("tooEarly"));
  fresh.nowMs = 50_000 + RECOVERY_DELAY_MS;
  await view.apply(activate, fresh);
  assert.equal(view.device("dev-r")?.status.type, "active");
  // A history clock records nothing.
  const hist = ChainClock.history();
  await ChainState.verify(all, undefined, hist);
  assert.equal(hist.firstSeen.size, 0);
});

// The TS PoC "a removed device plants mk/<m>/1/0 and the pending re-encryption
// hands ITS key to the agent", and the agent PoC (an MK epoch pushed out of
// reach): a key of anyone's choosing, however it is delivered, is not the
// account's unless the chain committed it.
test("a planted MK or UK that does not match the chain's commitment is refused, however it arrives", async () => {
  const [uk0, mk0] = [fill(0x20), fill(0x21)];
  const chain = await committed([uk0], [mk0]);
  const [d0, d1, d2] = [await dev(0), await dev(1), await dev(2)];
  const agent = await keyPairFromSecret(h(V.chain.machine.dhSecret));
  const mkCtx = (sender: string, epoch: number): SealContext => ({ purpose: "machineKey", account: ACCOUNT, sender, recipient: "m-1", epoch });
  // Mailed to the agent: the genuine MK opens; a key of dev-0's choosing, or an epoch out of reach, does not.
  assert.equal(hex(await openMachineKey(agent, "m-1", chain, mkCtx("dev-0", 0), await sealTo(d0.dh, agent.publicKey, mkCtx("dev-0", 0), mk0))), hex(mk0));
  await assert.rejects(openMachineKey(agent, "m-1", chain, mkCtx("dev-0", 0), await sealTo(d0.dh, agent.publicKey, mkCtx("dev-0", 0), fill(0x66))), wrapKind("uncommitted", "keyMismatch"));
  for (const epoch of [1, 4_294_967_295]) {
    const sealed = await sealTo(d0.dh, agent.publicKey, mkCtx("dev-0", epoch), fill(0x66));
    await assert.rejects(openMachineKey(agent, "m-1", chain, mkCtx("dev-0", epoch), sealed), wrapKind("uncommitted", "uncommittedKey"));
  }
  // The removed device is no sender at all.
  await assert.rejects(openMachineKey(agent, "m-1", chain, mkCtx("dev-1", 0), await sealTo(d1.dh, agent.publicKey, mkCtx("dev-1", 0), mk0)), wrapKind("unknownSender"));
  // The keyring's MK blob under UK: a removed device that kept UK_0 plants mk/m-1/1/0. Opened, it is not epoch 1's committed key.
  const blob = await wrapUnder(uk0, context.machineKeyUnderUserKey(ACCOUNT, "m-1", 1, 0), fill(0x66));
  const raw = await unwrapUnder(uk0, context.machineKeyUnderUserKey(ACCOUNT, "m-1", 1, 0), blob);
  await assert.rejects(requireCommitted(raw, (k) => chain.checkMachineKey("m-1", 1, k)), wrapKind("uncommitted", "uncommittedKey"));
  assert.ok(raw.every((b) => b === 0), "the refused key is wiped");
  const blob0 = await wrapUnder(uk0, context.machineKeyUnderUserKey(ACCOUNT, "m-1", 0, 0), fill(0x66));
  const raw0 = await unwrapUnder(uk0, context.machineKeyUnderUserKey(ACCOUNT, "m-1", 0, 0), blob0);
  await assert.rejects(requireCommitted(raw0, (k) => chain.checkMachineKey("m-1", 0, k)), wrapKind("uncommitted", "keyMismatch"));
  // And the chain itself refuses a jump, or a machine twice.
  const jump: Action = { type: "keyEpochs", user: null, machines: [{ machine: "m-1", epoch: 4_294_967_295, hash: fill(0) }] };
  await assert.rejects(extend(chain, [jump]), chainKind("notAllowed"));
  const twice: Action = { type: "keyEpochs", user: null, machines: [{ machine: "m-1", epoch: 1, hash: fill(1) }, { machine: "m-1", epoch: 2, hash: fill(2) }] };
  await assert.rejects(extend(chain, [twice]), chainKind("notAllowed"));
  await assert.rejects(extend(chain, [{ type: "keyEpochs", user: null, machines: [] }]), chainKind("notAllowed"));
  await assert.rejects(extend(chain, [{ type: "keyEpochs", user: null, machines: [{ machine: "m-9", epoch: 0, hash: fill(0) }] }]), chainKind("unknownId"));
  // A UK mailed to a device: only the committed one opens.
  const ukCtx: SealContext = { purpose: "userKey", account: ACCOUNT, sender: "dev-0", recipient: "dev-2", epoch: 0 };
  assert.equal(hex(await openUserKey(d2.dh, "dev-2", chain, ukCtx, await sealTo(d0.dh, d2.dh.publicKey, ukCtx, uk0))), hex(uk0));
  await assert.rejects(openUserKey(d2.dh, "dev-2", chain, ukCtx, await sealTo(d0.dh, d2.dh.publicKey, ukCtx, fill(0x77))), wrapKind("uncommitted", "keyMismatch"));
  // A pairing delivery from a current full device, MAC'd correctly, with a UK of its own choosing.
  const n = { signSeed: fill(0x30), dhSecret: fill(0x31) };
  const nInfo: DeviceInfo = { id: "dev-n", signKey: await publicFromSecret("Ed25519", n.signSeed), dhKey: await publicFromSecret("X25519", n.dhSecret), full: true };
  await extend(chain, [{ type: "addDevice", device: nInfo }]);
  const nDh = await keyPairFromSecret(n.dhSecret);
  const secret = fill(0x32);
  const rogue = await createDelivery(chain, "dev-0", d0.signPublic, d0.dh, "dev-n", nInfo.dhKey, [{ epoch: 0, key: fill(0xaa) }], secret);
  await assert.rejects(openDelivery(rogue, secret, chain, "dev-n", nInfo.signKey, nDh), wrapKind("uncommitted", "keyMismatch"));
  const ahead = await createDelivery(chain, "dev-0", d0.signPublic, d0.dh, "dev-n", nInfo.dhKey, [{ epoch: 0, key: uk0 }, { epoch: 1, key: fill(0xaa) }], secret);
  await assert.rejects(openDelivery(ahead, secret, chain, "dev-n", nInfo.signKey, nDh), wrapKind("uncommitted", "uncommittedKey"));
  const genuine = await createDelivery(chain, "dev-0", d0.signPublic, d0.dh, "dev-n", nInfo.dhKey, [{ epoch: 0, key: uk0 }], secret);
  assert.deepEqual((await openDelivery(genuine, secret, chain, "dev-n", nInfo.signKey, nDh)).map((k) => [k.epoch, hex(k.key)]), [[0, hex(uk0)]]);
});

// The agent PoC on drafts: anyone holding the stream key (a device removed
// since) could write a draft the reader showed as the machine's; an old
// epoch's key kept working after a rotation.
test("drafts from a key that is not the machine's, for another stream, or under an old epoch are refused", async () => {
  const mk = await importMachineKey(fill(0x40));
  const stream = await streamId(mk, "claude", "s-1");
  const other = await streamId(mk, "claude", "s-2");
  const key = await streamKey(mk, stream);
  const machineSeed = fill(0x41);
  const machine = await importEd25519Private(machineSeed);
  const machinePublic = await publicFromSecret("Ed25519", machineSeed);
  const thief = await importEd25519Private(fill(0x42));
  const body = utf8('{"turn":"t1","v":2,"blocks":[{"kind":"text","text":"typing"}]}');
  const refused = (kind: RecordError["kind"]) => (e: unknown) => e instanceof RecordError && e.kind === kind;
  const keyFor = () => key;
  // The machine's own draft in the current epoch opens.
  const good = await sealEphemeral(key, stream, 3, body, machine);
  assert.deepEqual(await openEphemeral(keyFor, stream, machinePublic, 3, good), { epoch: 3, body });
  // A stream-key holder signing for itself.
  await assert.rejects(openEphemeral(keyFor, stream, machinePublic, 3, await sealEphemeral(key, stream, 3, body, thief)), refused("badHead"));
  // An unsigned draft in the old wire shape.
  await assert.rejects(openEphemeral(keyFor, stream, machinePublic, 3, good.subarray(0, good.length - 64)), refused("badHead"));
  // The machine's draft moved to another stream.
  await assert.rejects(openEphemeral(keyFor, other, machinePublic, 3, good), refused("badHead"));
  // A genuine draft under an older epoch than the reader's newest: someone who kept that key, replaying.
  const old = await sealEphemeral(key, stream, 2, body, machine);
  await assert.rejects(openEphemeral(keyFor, stream, machinePublic, 3, old), (e: unknown) => refused("malformed")(e) && (e as Error).message.includes("old key"));
  // Too short to carry a signature at all.
  await assert.rejects(openEphemeral(keyFor, stream, machinePublic, 3, new Uint8Array(40)), refused("malformed"));
});

// poc_thief_mints_puppet_to_skip_24h: the day of distrust after a removal was
// skipped by a thief with a puppet device yet locked the honest owner out. It
// is gone; who counts is the same with or without a puppet.
test("the sole-device rule a puppet skipped is gone: a sole device vouches for itself at once, others always count", async () => {
  const chain = await ChainState.verify(sampleEntries(), undefined, ChainClock.live(1_000, 0));
  const d0 = await dev(0);
  const d3 = { sign: await importEd25519Private(h(V.chain.devices[3].signSeed)) };
  const info3: DeviceInfo = { id: "dev-3", signKey: h(V.chain.devices[3].signKey), dhKey: h(V.chain.devices[3].dhKey), full: true };
  const clock = ChainClock.live(10_000, 0);
  await extend(chain, [{ type: "addDevice", device: info3 }], clock);
  const day = 12 * 3_600_000;
  // Two full devices: one's own word does not count, the other's does.
  const own = await createAttestation(chain, "dev-0", d0.sign, 10_001);
  assert.equal((await chain.freshness("dev-0", [own], 10_002, day)).type, "stale");
  const other = await createAttestation(chain, "dev-3", d3.sign, 10_001);
  assert.deepEqual(await chain.freshness("dev-0", [other], 10_002, day), { type: "fresh" });
  // dev-0 removes dev-3 and is the sole full device: its own word counts at once.
  await extend(chain, [{ type: "removeDevice", id: "dev-3" }], clock);
  const sole = await createAttestation(chain, "dev-0", d0.sign, 10_003);
  assert.deepEqual(await chain.freshness("dev-0", [sole], 10_004, day), { type: "fresh" });
  // The removed device's word no longer counts for anyone.
  const removed = await createAttestation(chain, "dev-3", d3.sign, 10_003);
  assert.equal((await chain.freshness("dev-0", [removed], 10_004, day)).type, "stale");
});

// The agent PoC's third point: a manifest said in the clear which kind of stream it was.
test("a manifest no longer carries its format in the clear", async () => {
  const mk = await importMachineKey(fill(7));
  const machine = await importEd25519Private(fill(40));
  const encoded = [];
  for (const source of ["rh.index", "rh.status"]) {
    const stream = await streamId(mk, source, "");
    const head = await signStreamHead({ stream, nextSeq: 3, epoch: 0, link: fill(0), timeMs: 1 }, machine);
    encoded.push(encodeManifest(await signManifest(head, [], machine)));
  }
  const headLen = 32 + 8 + 4 + 32 + 8 + 64;
  assert.equal(encoded[0]!.length, encoded[1]!.length);
  assert.equal(encoded[0]!.length, headLen + 2 + 64, "head, page count, signature: nothing else");
  assert.deepEqual([...encoded[0]!.subarray(headLen, headLen + 2)], [0, 0], "the page count, and nothing about the kind");
});
