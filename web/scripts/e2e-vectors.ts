/*
 * The TypeScript → Rust interop vectors: `server-rs/crates/rh-proto/vectors/ts.json`.
 *
 * Everything here is produced by `web/src/lib/e2e` on WebCrypto, from fixed
 * seeds, and replayed by the Rust test `vectors::ts_vectors_verify_in_rust`:
 * a chain TS signed (v2: a recovery key with its proof, key epochs committed)
 * must verify in Rust, and so must a v3 chain TS built on a v2 prefix (a
 * host, a promoted admin, a passkey slot and its join, an accept policy) with
 * the same state, the same control matrix and the same refusals, and so must a chain where a host renews its identity
 * key (the proofs, the confirmation MAC, the code and the ack recomputed there), a handshake TS ran (both roles, fixed
 * ephemerals) must be exactly what `snow` writes, records TS sealed must open,
 * keys TS wrapped must unwrap. `web/test/e2e-ts-vectors.test.ts` regenerates
 * this in memory and fails when the committed file has drifted.
 *
 *   npx tsx web/scripts/e2e-vectors.ts          # rewrite ts.json
 */
import { writeFileSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import {
  type Action,
  ChainClock,
  encodeHelloBody,
  encodeReplyBody,
  ChainState,
  recoveryKeys,
  RULES,
  type Signer,
  context,
  type DeviceInfo,
  encodeEntry,
  encodeFrame,
  encodeMuxFrame,
  type Entry,
  entryHash,
  fingerprint,
  FMT_FEED_V1,
  type Frame,
  genesisDraft,
  hex,
  importEd25519Private,
  importMachineKey,
  Initiator,
  keyPairFromSecret,
  type MuxFrame,
  prologue,
  publicFromSecret,
  recordLink,
  signStreamHead,
  encodeSignedHead,
  type Compression,
  type RekeyPolicy,
  Responder,
  sealRecord,
  sealTo,
  sha256,
  signDraft,
  streamId,
  STREAMS_FEATURE,
  streamKey,
  utf8,
  wrapUnder,
  encodeSealContext,
  pairingTag,
  type SealContext,
  concat,
  type Bytes,
  encodeSealedRecord,
  encodeManifest,
  encodePage,
  indexStream,
  pageHash,
  sealEphemeral,
  sealPage,
  signManifest,
  statusStream,
  channelId,
  Cpace,
  cpaceGenerator,
  createDelivery,
  encodeDelivery,
  encodePakeAnswer,
  encodePakeHello,
  mnemonicFromEntropy,
  offerText,
  pakeAnswer,
  sealForRecovery,
  machineKeyCommit,
  recoveryKeyFor,
  userKeyCommit,
  type Accept,
  CHAIN_VERSION,
  ChainError,
  encodeHostKeyContext,
  encodeHostKeyMail,
  type HostKeyContext,
  type PasskeySlot,
  passkeySlotProof,
  sealHostKey,
  ASSURANCE_AUTO,
  ASSURANCE_ON_DEVICE,
  type Assurance,
  base32,
  copy,
  decodeEntry,
  genesis3Draft,
  type HostRotation,
  hostRotationBody,
  importX25519Private,
  rotateAckBody,
  rotateConfirmFor,
  encodeDraft,
  MAX_PAUSE_ENTRY_LEN,
  PAUSE_CANCELLED,
  PAUSE_COMPLETED,
  PAUSE_START,
  type PausePhase,
  type PauseRemoval,
  rotateDisputeBody,
  rotateSas,
  signHostRotate,
  signRotateDispute,
  signRotateAck,
  verifyRotateConfirm,
} from "../src/lib/e2e/index";

export const TS_VECTORS_PATH = fileURLToPath(new URL("../../server-rs/crates/rh-proto/vectors/ts.json", import.meta.url));

const seed = (n: number): Promise<Bytes> => sha256(utf8(`rh-proto ts vector ${n}`));

async function chainVectors() {
  const akSeed = await seed(1);
  const ak = await importEd25519Private(akSeed);
  const akPublic = await publicFromSecret("Ed25519", akSeed);
  const devices: { info: DeviceInfo; key: CryptoKey }[] = [];
  for (let i = 0; i < 4; i++) {
    const signSeed = await seed(10 + i);
    const dh = await seed(20 + i);
    devices.push({
      key: await importEd25519Private(signSeed),
      info: { id: `ts-dev-${i}`, signKey: await publicFromSecret("Ed25519", signSeed), dhKey: await publicFromSecret("X25519", dh), full: i !== 2 },
    });
  }
  const machine = { id: "ts-m-1", signKey: await publicFromSecret("Ed25519", await seed(30)), dhKey: await publicFromSecret("X25519", await seed(31)) };
  const recovery = await recoveryKeys((await seed(5)).slice(0, 16));
  const recoveryKey = await recoveryKeyFor(recovery, "ts-acct", akPublic);
  const entries: Entry[] = [await signDraft(genesisDraft("ts-acct", akPublic, devices[0]!.info, recoveryKey, 1_000), ak)];
  const clock = ChainClock.history();
  const state = await ChainState.verify(entries, undefined, clock);
  const byDev0 = { type: "device", id: "ts-dev-0" } as const;
  // Key epochs: user key 0 with ts-m-1's key 0, then ts-m-1's key 1.
  const [uk0, mk0, mk1] = [await seed(6), await seed(7), await seed(8)];
  const steps: [Action, number][] = [
    [{ type: "addDevice", device: devices[1]!.info }, 2_000],
    [{ type: "addMachine", machine }, 3_000],
    [{ type: "addDevice", device: devices[2]!.info }, 4_000],
    [{ type: "removeDevice", id: "ts-dev-1" }, 5_000],
    [
      {
        type: "keyEpochs",
        user: { epoch: 0, hash: await userKeyCommit("ts-acct", 0, uk0) },
        machines: [{ machine: "ts-m-1", epoch: 0, hash: await machineKeyCommit("ts-acct", "ts-m-1", 0, mk0) }],
      },
      5_500,
    ],
    [{ type: "keyEpochs", user: null, machines: [{ machine: "ts-m-1", epoch: 1, hash: await machineKeyCommit("ts-acct", "ts-m-1", 1, mk1) }] }, 5_600],
  ];
  for (const [action, ms] of steps) {
    const entry = await signDraft(state.draft(byDev0, action, ms), devices[0]!.key);
    await state.apply(entry, clock);
    entries.push(entry);
  }
  // A recovered device (signed by RSK), approved by the remaining full device.
  const recovered = { ...devices[3]!.info, id: "ts-dev-r" };
  for (const [signer, key, action] of [
    [{ type: "recovery" }, recovery.signing, { type: "recoveryJoin", device: recovered }],
    [byDev0, devices[0]!.key, { type: "approveDevice", id: "ts-dev-r" }],
  ] as [Signer, CryptoKey, Action][]) {
    const entry = await signDraft(state.draft(signer, action, 6_000), key);
    await state.apply(entry, clock);
    entries.push(entry);
  }
  const badSig = entries.map((e, i) => (i === 3 ? { ...e, signature: Uint8Array.from(e.signature, (b, j) => (j === 9 ? b ^ 1 : b)) } : e));
  const akRejoin = [...entries, await signDraft(state.draft({ type: "account" }, { type: "recoveryJoin", device: { ...devices[3]!.info, id: "ts-dev-evil" } }, 7_000), ak)];
  const byRemoved = [...entries, await signDraft(state.draft({ type: "device", id: "ts-dev-1" }, { type: "removeMachine", id: "ts-m-1" }, 6_000), devices[1]!.key)];
  // A hub's own genesis around the account's real RSK, with the real proof copied: it is over another root.
  const hubSeed = await seed(9);
  const hubGenesis = await signDraft(genesisDraft("ts-acct", await publicFromSecret("Ed25519", hubSeed), devices[0]!.info, recoveryKey, 1_000), await importEd25519Private(hubSeed));
  const skip = [...entries, await signDraft(state.draft(byDev0, { type: "keyEpochs", user: { epoch: 2, hash: await userKeyCommit("ts-acct", 2, uk0) }, machines: [] }, 7_000), devices[0]!.key)];
  const machineJump = [
    ...entries,
    await signDraft(state.draft(byDev0, { type: "keyEpochs", user: null, machines: [{ machine: "ts-m-1", epoch: 4_294_967_295, hash: new Uint8Array(32) }] }, 7_000), devices[0]!.key),
  ];
  const badProof = [
    ...entries,
    await signDraft(state.draft(byDev0, { type: "setRecoveryKey", key: { key: recoveryKey.key, proof: new Uint8Array(64).fill(1) } }, 7_000), devices[0]!.key),
  ];
  return {
    accountFingerprint: hex(await fingerprint(akPublic)),
    recoveryKey: { key: hex(recoveryKey.key), proof: hex(recoveryKey.proof) },
    keyEpochs: {
      userKey: hex(uk0),
      userCommit: hex(await userKeyCommit("ts-acct", 0, uk0)),
      machine: "ts-m-1",
      machineKeys: [hex(mk0), hex(mk1)],
      machineCommits: [hex(await machineKeyCommit("ts-acct", "ts-m-1", 0, mk0)), hex(await machineKeyCommit("ts-acct", "ts-m-1", 1, mk1))],
    },
    entries: entries.map((e) => hex(encodeEntry(e))),
    hashes: await Promise.all(entries.map(async (e) => hex(await entryHash(e)))),
    state: {
      seq: state.seq,
      head: hex(state.head),
      // The v2 view: the members that are not hosts (the machine is not a device here).
      devices: state
        .allDevices()
        .filter((d) => !d.host)
        .map((d) => ({ id: d.info.id, active: d.status.type === "active" })),
    },
    invalid: [
      { name: "a bad signature on entry 3", entries: badSig.map((e) => hex(encodeEntry(e))), error: "badSignature" },
      { name: "signed by a removed device", entries: byRemoved.map((e) => hex(encodeEntry(e))), error: "unknownId" },
      { name: "the account key signing a recovery-join", entries: akRejoin.map((e) => hex(encodeEntry(e))), error: "notAllowed" },
      { name: "a recovery key whose proof is over another root", entries: [hex(encodeEntry(hubGenesis))], error: "notAllowed" },
      { name: "a recovery key set without a valid proof", entries: badProof.map((e) => hex(encodeEntry(e))), error: "notAllowed" },
      { name: "a user key epoch that skips one", entries: skip.map((e) => hex(encodeEntry(e))), error: "notAllowed" },
      { name: "a machine key epoch out of reach", entries: machineJump.map((e) => hex(encodeEntry(e))), error: "notAllowed" },
    ],
  };
}

const acceptJson = (a: Accept) => (a.type === "devices" ? { kind: "devices", ids: a.ids } : { kind: a.type });

/** A v3 state as Rust's vectors write it (`hosts`: whose accept policy and machine key epochs to list). */
function v3StateJson(state: ChainState, hosts: string[]) {
  return {
    seq: state.seq,
    head: hex(state.head),
    v3From: state.v3From,
    recoveryKey: state.currentRecoveryKey ? hex(state.currentRecoveryKey) : null,
    devices: state.allDevices().map((d) => ({
      id: d.info.id,
      host: d.host,
      admin: d.admin,
      status: d.status.type,
      viaRecovery: d.viaRecovery,
      viaPasskey: d.viaPasskey,
      addedSeq: d.addedSeq,
      activatedSeq: d.activatedSeq,
    })),
    machines: state
      .allDevices()
      .filter((d) => state.machine(d.info.id))
      .map((d) => d.info.id),
    admins: state.admins().map((d) => d.info.id),
    slots: state.allSlots().map((r) => ({ slot: r.slot.slot, setBy: r.setBy, removed: r.removed, addedSeq: r.addedSeq })),
    accept: hosts.map((host) => ({ host, from: acceptJson(state.acceptPolicy(host)) })),
    userKeyEpochs: state.userKeyEpochs(),
    machineKeyEpochs: Object.fromEntries(hosts.map((host) => [host, state.machineKeyEpochs(host)])),
  };
}

/**
 * Chain v3 built and signed here: a v2 prefix (genesis, a limited device, a
 * machine, key epochs), then v3 entries — a host and its first key, the
 * limited device promoted to admin, a passkey slot it registers with a proof
 * computed here, a passkey-signed join, its approval, and an accept policy —
 * with the state, who may control whom, chains that must be refused (and
 * why), and a host key sealed for the passkey device.
 */
async function chainV3Vectors() {
  const account = "ts-acct-3";
  const akSeed = await seed(160);
  const ak = await importEd25519Private(akSeed);
  const akPublic = await publicFromSecret("Ed25519", akSeed);
  const member = async (id: string, n: number) => {
    const signSeed = await seed(n);
    const dhSecret = await seed(n + 1);
    return { id, dhSecret, key: await importEd25519Private(signSeed), signKey: await publicFromSecret("Ed25519", signSeed), dhKey: await publicFromSecret("X25519", dhSecret) };
  };
  const [dev0, dev1, m1, h2, devP, devX, devR] = [
    await member("ts-dev-0", 161),
    await member("ts-dev-1", 163),
    await member("ts-m-1", 165),
    await member("ts-h-2", 167),
    await member("ts-dev-p", 169),
    await member("ts-dev-x", 179),
    await member("ts-dev-r", 181),
  ];
  const devY = await member("ts-dev-y", 191);
  const [devW, devV] = [await member("ts-dev-w", 193), await member("ts-dev-v", 197)];
  const rkW = await recoveryKeys((await seed(195)).slice(0, 16));
  // Write gate: words a device came back with (rk4), and newer ones it never knew (rk5).
  const [rk4, rk5] = [await recoveryKeys((await seed(199)).slice(0, 16)), await recoveryKeys((await seed(200)).slice(0, 16))];
  const [devS, devT] = [await member("ts-dev-s", 201), await member("ts-dev-t", 203)];
  const rk = await recoveryKeys((await seed(183)).slice(0, 16));
  const pakSeed = await seed(171);
  const pak = await importEd25519Private(pakSeed);
  const pakPublic = await publicFromSecret("Ed25519", pakSeed);
  const credHash = await seed(172);
  const saltNonce = await seed(173);
  const slot: PasskeySlot = {
    slot: "p-ts1",
    key: pakPublic,
    proof: await passkeySlotProof(pak, pakPublic, account, akPublic, "p-ts1", credHash, saltNonce),
    credHash,
    saltNonce,
    grants: 0,
    selfActivate: true,
  };
  const [uk0, mkM1, mkH2, mkM1b, mkM1c] = [await seed(174), await seed(175), await seed(176), await seed(185), await seed(190)];
  // A second slot, registered by an admin that is then demoted (review T16).
  const pak2Seed = await seed(186);
  const pak2 = await importEd25519Private(pak2Seed);
  const pak2Public = await publicFromSecret("Ed25519", pak2Seed);
  const [credHash2, saltNonce2] = [await seed(187), await seed(188)];
  const slot2: PasskeySlot = {
    slot: "p-ts2",
    key: pak2Public,
    proof: await passkeySlotProof(pak2, pak2Public, account, akPublic, "p-ts2", credHash2, saltNonce2),
    credHash: credHash2,
    saltNonce: saltNonce2,
    grants: 0,
    selfActivate: false,
  };
  const byDev = (m: { id: string }) => ({ type: "device", id: m.id }) as const;
  const v3 = (m: typeof dev0, host: boolean, admin: boolean) => ({ id: m.id, signKey: m.signKey, dhKey: m.dhKey, flags: { host, admin } });
  const entries: Entry[] = [
    await signDraft(genesisDraft(account, akPublic, { id: dev0.id, signKey: dev0.signKey, dhKey: dev0.dhKey, full: true }, await recoveryKeyFor(rk, account, akPublic), 1_000), ak),
  ];
  const state = await ChainState.verify(entries, undefined, ChainClock.history());
  const steps: [string, Signer, CryptoKey, Action][] = [
    // v2
    ["addLimited", byDev(dev0), dev0.key, { type: "addDevice", device: { id: dev1.id, signKey: dev1.signKey, dhKey: dev1.dhKey, full: false } }],
    ["addMachine", byDev(dev0), dev0.key, { type: "addMachine", machine: { id: m1.id, signKey: m1.signKey, dhKey: m1.dhKey } }],
    [
      "keys",
      byDev(dev0),
      dev0.key,
      {
        type: "keyEpochs",
        user: { epoch: 0, hash: await userKeyCommit(account, 0, uk0) },
        machines: [{ machine: m1.id, epoch: 0, hash: await machineKeyCommit(account, m1.id, 0, mkM1) }],
      },
    ],
    // v3
    ["addHost", byDev(dev0), dev0.key, { type: "addDevice3", device: v3(h2, true, false) }],
    ["hostKey", byDev(dev0), dev0.key, { type: "keyEpochs", user: null, machines: [{ machine: h2.id, epoch: 0, hash: await machineKeyCommit(account, h2.id, 0, mkH2) }] }],
    ["promote", byDev(dev0), dev0.key, { type: "setAdmin", id: dev1.id, admin: true }],
    ["slot", byDev(dev1), dev1.key, { type: "passkeySlotSet", slot }],
    ["passkeyJoin", { type: "passkey", slot: slot.slot }, pak, { type: "addDevice3", device: v3(devP, false, false) }],
    ["approve", byDev(dev0), dev0.key, { type: "approveDevice", id: devP.id }],
    ["accept", byDev(dev1), dev1.key, { type: "setAccept", host: m1.id, from: { type: "devices", ids: [devP.id, h2.id] } }],
    // A narrowing (h-2 out): m-1's next machine key comes with it (review D2).
    ["narrow", byDev(dev1), dev1.key, { type: "setAccept", host: m1.id, from: { type: "devices", ids: [devP.id] } }],
    ["narrowKey", byDev(dev1), dev1.key, { type: "keyEpochs", user: null, machines: [{ machine: m1.id, epoch: 1, hash: await machineKeyCommit(account, m1.id, 1, mkM1b) }] }],
    // The recovery words on a v3 chain: a v3 join (review D4), approved.
    ["recoveryJoin3", { type: "recovery" }, rk.signing, { type: "recoveryJoin3", device: v3(devR, false, true) }],
    ["approveRecovered", byDev(dev0), dev0.key, { type: "approveDevice", id: devR.id }],
    // Re-review T16: a demoted admin's slots go with its standing; it stays a device.
    ["addAdminX", byDev(dev0), dev0.key, { type: "addDevice3", device: v3(devX, false, true) }],
    ["slot2", byDev(devX), devX.key, { type: "passkeySlotSet", slot: slot2 }],
    ["demoteX", byDev(dev0), dev0.key, { type: "setAdmin", id: devX.id, admin: false }],
    // Re-review T17: a host that stops running AI loses its accept policy; restarted, it admits admins only until set again.
    ["stopM1", byDev(dev0), dev0.key, { type: "setHost", id: m1.id, host: false }],
    ["restartM1", byDev(dev0), dev0.key, { type: "setHost", id: m1.id, host: true }],
    ["restartM1Key", byDev(dev0), dev0.key, { type: "keyEpochs", user: null, machines: [{ machine: m1.id, epoch: 2, hash: await machineKeyCommit(account, m1.id, 2, mkM1c) }] }],
    ["reaccept", byDev(dev0), dev0.key, { type: "setAccept", host: m1.id, from: { type: "devices", ids: [devP.id] } }],
    // Write gate: an admin that set the recovery words and is then demoted knows them, so the key stops counting.
    ["addAdminW", byDev(dev0), dev0.key, { type: "addDevice3", device: v3(devW, false, true) }],
    ["setRecoveryW", byDev(devW), devW.key, { type: "setRecoveryKey", key: await recoveryKeyFor(rkW, account, akPublic) }],
    ["demoteW", byDev(dev0), dev0.key, { type: "setAdmin", id: devW.id, admin: false }],
    // Write gate: a device that came back with the words takes only those along — newer words stand, the current ones go.
    ["setRecovery4", byDev(dev0), dev0.key, { type: "setRecoveryKey", key: await recoveryKeyFor(rk4, account, akPublic) }],
    ["recoveryJoinS", { type: "recovery" }, rk4.signing, { type: "recoveryJoin3", device: v3(devS, false, true) }],
    ["approveS", byDev(dev0), dev0.key, { type: "approveDevice", id: devS.id }],
    ["setRecovery5", byDev(dev0), dev0.key, { type: "setRecoveryKey", key: await recoveryKeyFor(rk5, account, akPublic) }],
    ["demoteS", byDev(dev0), dev0.key, { type: "setAdmin", id: devS.id, admin: false }],
    ["removeS", byDev(dev0), dev0.key, { type: "removeDevice3", id: devS.id }],
    ["recoveryJoinT", { type: "recovery" }, rk5.signing, { type: "recoveryJoin3", device: v3(devT, false, true) }],
    ["approveT", byDev(dev0), dev0.key, { type: "approveDevice", id: devT.id }],
    ["removeT", byDev(dev0), dev0.key, { type: "removeDevice3", id: devT.id }],
  ];
  const labels: { label: string; seq: number }[] = [];
  for (const [label, signer, key, action] of steps) {
    const entry = await signDraft(state.draft(signer, action, 2_000 + state.seq), key);
    await state.apply(entry, ChainClock.history());
    entries.push(entry);
    labels.push({ label, seq: entry.draft.seq });
  }
  const at = (label: string) => entries.slice(0, labels.find((l) => l.label === label)!.seq + 1);

  // Chains that must be refused: a valid prefix and one entry, labelled with the error.
  // Each `notAllowed` case names its rule (review finding 10), as Rust's do.
  const invalid: { name: string; entries: string[]; error: string; rule: string | null }[] = [];
  const bad = async (name: string, prefix: Entry[], signer: Signer, key: CryptoKey, action: Action, error: string, version?: number, rule?: string) => {
    const st = await ChainState.verify(prefix, undefined, ChainClock.history());
    const draft = st.draft(signer, action, 3_000);
    const entry = await signDraft(version === undefined ? draft : { ...draft, version }, key);
    let reason: string | null = null;
    try {
      await st.apply(entry, ChainClock.history());
      throw new Error(`${name}: applied`);
    } catch (e) {
      if (!(e instanceof ChainError) || e.kind !== error) throw new Error(`${name}: ${e}`);
      reason = e.detail.reason ?? null;
    }
    if (rule !== undefined && reason !== rule) throw new Error(`${name}: refused for ${reason}, not ${rule}`);
    invalid.push({ name, entries: [...prefix, entry].map((e) => hex(encodeEntry(e))), error, rule: reason });
  };
  const ruled = (name: string, prefix: Entry[], signer: Signer, key: CryptoKey, action: Action, rule: string) => bad(name, prefix, signer, key, action, "notAllowed", undefined, rule);
  await bad("a plain device signs a v3 entry", at("hostKey"), byDev(dev1), dev1.key, { type: "setAdmin", id: dev1.id, admin: true }, "notAllowed");
  await bad("demoting the last admin", at("hostKey"), byDev(dev0), dev0.key, { type: "setAdmin", id: dev0.id, admin: false }, "lastAdmin");
  await bad("an admin makes itself a host", at("promote"), byDev(dev1), dev1.key, { type: "setHost", id: dev1.id, host: true }, "notAllowed");
  await bad("a new host without its machine key next", at("addHost"), byDev(dev0), dev0.key, { type: "setAdmin", id: dev1.id, admin: true }, "notAllowed");
  await bad("a passkey adds an admin", at("slot"), { type: "passkey", slot: slot.slot }, pak, { type: "addDevice3", device: v3(devX, false, true) }, "notAllowed");
  await bad("an unknown passkey slot signs", at("slot"), { type: "passkey", slot: "p-ts9" }, pak, { type: "addDevice3", device: v3(devX, false, false) }, "unknownSlot");
  await bad("an accept list naming the host", at("hostKey"), byDev(dev0), dev0.key, { type: "setAccept", host: m1.id, from: { type: "devices", ids: [m1.id] } }, "notAllowed");
  // Parity with Rust's list, rule by rule.
  await ruled("a host made an admin", at("hostKey"), byDev(dev0), dev0.key, { type: "setAdmin", id: h2.id, admin: true }, RULES.hostNotAdmin);
  await ruled("a computer added as host and admin", at("hostKey"), byDev(dev0), dev0.key, { type: "addDevice3", device: v3(devX, true, true) }, RULES.hostNotAdmin);
  await ruled("an admin made a host", at("promote"), byDev(dev0), dev0.key, { type: "setHost", id: dev1.id, host: true }, RULES.hostNotAdmin);
  await ruled("a recovered host", at("hostKey"), { type: "recovery" }, rk.signing, { type: "recoveryJoin3", device: v3(devX, true, true) }, RULES.hostNotAdmin);
  await ruled("a recovered device that is not an admin", at("hostKey"), { type: "recovery" }, rk.signing, { type: "recoveryJoin3", device: v3(devX, false, false) }, RULES.recoveredAdmin);
  await ruled("an admin granting itself admin", at("hostKey"), byDev(dev0), dev0.key, { type: "setAdmin", id: dev0.id, admin: true }, RULES.selfAdmin);
  await ruled("an admin made an admin again", at("promote"), byDev(dev0), dev0.key, { type: "setAdmin", id: dev1.id, admin: true }, RULES.noChange);
  await ruled("a narrowing without the host's new key next", at("narrow"), byDev(dev0), dev0.key, { type: "setAdmin", id: devP.id, admin: true }, RULES.hostKeyNext);
  await ruled("a plain device signs", at("hostKey"), byDev(dev1), dev1.key, { type: "addDevice3", device: v3(devX, false, false) }, RULES.onlyAdminSigns);
  await ruled("a passkey adds a host", at("slot"), { type: "passkey", slot: slot.slot }, pak, { type: "addDevice3", device: v3(devX, true, false) }, RULES.passkeyPlain);
  await ruled("an accept policy for a device that is not a host", at("hostKey"), byDev(dev0), dev0.key, { type: "setAccept", host: dev1.id, from: { type: "all" } }, RULES.acceptNotHost);
  await ruled("an accept policy that changes nothing", at("hostKey"), byDev(dev0), dev0.key, { type: "setAccept", host: m1.id, from: { type: "admins" } }, RULES.acceptSame);
  await bad("an admin standing set on a pending device", at("passkeyJoin"), byDev(dev0), dev0.key, { type: "setAdmin", id: devP.id, admin: true }, "unknownId");
  await bad("the words a device came back with, once newer ones were set", at("demoteS"), { type: "recovery" }, rk4.signing, { type: "recoveryJoin3", device: v3(devV, false, true) }, "badSignature");
  await bad("the recovery words of a demoted setter sign a join", at("demoteW"), { type: "recovery" }, rkW.signing, { type: "recoveryJoin3", device: v3(devV, false, true) }, "notAllowed", undefined, "this account has no recovery key");
  await bad("a slot whose admin was demoted signs a join", at("demoteX"), { type: "passkey", slot: slot2.slot }, pak2, { type: "addDevice3", device: v3(devY, false, false) }, "unknownSlot");
  await ruled("a restarted host's policy set back to admins only", at("restartM1Key"), byDev(dev0), dev0.key, { type: "setAccept", host: m1.id, from: { type: "admins" } }, RULES.acceptSame);
  await ruled("a computer that stopped running AI made an admin", at("stopM1"), byDev(dev0), dev0.key, { type: "setAdmin", id: m1.id, admin: true }, RULES.ranAiNotAdmin);
  await bad(
    "a v2 recovery join after a v3 entry",
    at("hostKey"),
    { type: "recovery" },
    rk.signing,
    { type: "recoveryJoin", device: { id: devX.id, signKey: devX.signKey, dhKey: devX.dhKey, full: true } },
    "notAllowed",
    CHAIN_VERSION,
    RULES.v2AfterV3,
  );
  await bad(
    "a v2 entry after a v3 one",
    entries,
    byDev(dev0),
    dev0.key,
    { type: "keyEpochs", user: null, machines: [{ machine: m1.id, epoch: 1, hash: await machineKeyCommit(account, m1.id, 1, mkM1) }] },
    "notAllowed",
    CHAIN_VERSION,
  );

  // Who may control whom in the final state.
  const control = [
    [dev0.id, m1.id],
    [h2.id, m1.id],
    [devP.id, m1.id],
    [devP.id, h2.id],
    [m1.id, h2.id],
    [h2.id, h2.id],
    [dev1.id, h2.id],
    [dev0.id, devP.id],
    ["ts-nobody", m1.id],
  ].map(([device, host]) => {
    try {
      state.mayControl(device!, host!);
      return { device, host, ok: true, error: null };
    } catch (e) {
      if (!(e instanceof ChainError)) throw e;
      return { device, host, ok: false, error: e.kind };
    }
  });

  // ts-h-2's machine key epoch 0, sealed by ts-dev-0 for ts-dev-p (fixed ephemeral and nonce).
  const ctx: HostKeyContext = { account, sender: dev0.id, recipient: devP.id, host: h2.id, epoch: 0 };
  const sealed = await sealHostKey(await keyPairFromSecret(dev0.dhSecret), devP.dhKey, ctx, mkH2, {
    ephemeral: await keyPairFromSecret(await seed(177)),
    nonce: (await seed(178)).slice(0, 12),
  });
  // Write gate: the v2 removal rule, untouched — a device that came back with the words is removed the v2 way after
  // newer words were set, and the account's words go (v3 keeps the newer ones). Rust must replay it the same.
  const v2RecoveryRemoval = await (async () => {
    const acc = "ts-acct-v2r";
    const akSeed2 = await seed(205);
    const ak2 = await importEd25519Private(akSeed2);
    const akPub2 = await publicFromSecret("Ed25519", akSeed2);
    const [owner, back] = [await member("ts-v2-owner", 206), await member("ts-v2-back", 208)];
    const [rkA, rkB] = [await recoveryKeys((await seed(210)).slice(0, 16)), await recoveryKeys((await seed(211)).slice(0, 16))];
    const es: Entry[] = [
      await signDraft(genesisDraft(acc, akPub2, { id: owner.id, signKey: owner.signKey, dhKey: owner.dhKey, full: true }, await recoveryKeyFor(rkA, acc, akPub2), 1_000), ak2),
    ];
    const st = await ChainState.verify(es, undefined, ChainClock.history());
    const v2Steps: [Signer, CryptoKey, Action][] = [
      [{ type: "recovery" }, rkA.signing, { type: "recoveryJoin", device: { id: back.id, signKey: back.signKey, dhKey: back.dhKey, full: true } }],
      [byDev(owner), owner.key, { type: "approveDevice", id: back.id }],
      [byDev(owner), owner.key, { type: "setRecoveryKey", key: await recoveryKeyFor(rkB, acc, akPub2) }],
      [byDev(owner), owner.key, { type: "removeDevice", id: back.id }],
    ];
    for (const [signer, key, action] of v2Steps) {
      const entry = await signDraft(st.draft(signer, action, 2_000 + st.seq), key);
      await st.apply(entry, ChainClock.history());
      es.push(entry);
    }
    if (st.isV3 || st.currentRecoveryKey) throw new Error("v2: a v2 chain whose words went with the removal");
    return { entries: es.map((e) => hex(encodeEntry(e))), head: hex(st.head), recoveryKey: null };
  })();
  return {
    account,
    accountFingerprint: hex(await fingerprint(akPublic)),
    entries: entries.map((e) => hex(encodeEntry(e))),
    hashes: await Promise.all(entries.map(async (e) => hex(await entryHash(e)))),
    versions: entries.map((e) => e.draft.version),
    steps: labels,
    passkey: { slot: slot.slot, key: hex(slot.key), proof: hex(slot.proof), credHash: hex(credHash), saltNonce: hex(saltNonce) },
    keys: { userKey: hex(uk0), machines: { [m1.id]: [hex(mkM1), hex(mkM1b), hex(mkM1c)], [h2.id]: [hex(mkH2)] } },
    state: v3StateJson(state, [m1.id, h2.id]),
    control,
    invalid,
    v2RecoveryRemoval,
    hostKey: {
      context: ctx,
      contextBytes: hex(encodeHostKeyContext(ctx)),
      sealed: hex(sealed),
      mail: hex(encodeHostKeyMail(h2.id, sealed)),
      recipientDhSecret: hex(devP.dhSecret),
      key: hex(mkH2),
    },
  };
}

/** A device's two keys as the device list shows them (`lib/hub/account.ts` `deviceFingerprint`; Rust's `device_fingerprint_text`). */
async function deviceFingerprintText(signKey: Uint8Array, dhKey: Uint8Array): Promise<string> {
  return (base32((await fingerprint(concat(signKey, dhKey))).slice(0, 10)).match(/.{1,4}/g) ?? []).join("-");
}

/** A state as Rust's identity renewal vectors write it: `v3StateJson` and every device's keys, retired keys and last renewal. */
/** The renewal pauses of a state, as Rust's `pause_state_json` writes them (`hosts`: whose base and paused edges; `pairs`: who may control whom). */
export function pauseStateJson(state: ChainState, hosts: string[], pairs: [string, string][]) {
  return {
    openPauses: state.openPauses().map((p) => ({ op: hex(p.op), startSeq: p.startSeq, signer: p.signer, removals: p.removals.map((r) => ({ host: r.host, ids: r.ids })) })),
    hosts: hosts.map((h) => ({ host: h, base: acceptJson(state.baseAccept(h)), paused: state.pausedEdges(h) })),
    control: pairs.map(([device, h]) => {
      try {
        state.mayControl(device, h);
        return { device, host: h, ok: true, error: null };
      } catch (e) {
        if (!(e instanceof ChainError)) throw e;
        return { device, host: h, ok: false, error: e.kind };
      }
    }),
  };
}

export function rotationStateJson(state: ChainState, hosts: string[]) {
  return {
    ...v3StateJson(state, hosts),
    identities: state.allDevices().map((d) => ({
      id: d.info.id,
      signKey: hex(d.info.signKey),
      dhKey: hex(d.info.dhKey),
      rotatedSeq: d.rotatedSeq,
      retired: d.retired.map((r) => ({ signKey: hex(r.signKey), dhKey: hex(r.dhKey), untilSeq: r.untilSeq, byAssured: r.byAssured })),
    })),
  };
}

/** What a renewal entry proves, as Rust's `renewal_json` writes it (`before`: the chain up to the entry before it). */
export async function renewalJson(before: ChainState, entry: Entry) {
  const a = entry.draft.action;
  if (a.type !== "rotateHostKey") throw new Error("a renewal");
  const r = a.rotation;
  const old = before.machine(r.host)!.info;
  const body = hostRotationBody(before.account, r, old.signKey, old.dhKey);
  return {
    seq: entry.draft.seq,
    host: r.host,
    oldSign: hex(old.signKey),
    oldDh: hex(old.dhKey),
    newSign: hex(r.signKey),
    newDh: hex(r.dhKey),
    stagedSeq: r.stagedSeq,
    stagedHead: hex(r.stagedHead),
    nonce: hex(r.nonce),
    assurance: r.assurance,
    oldSig: hex(r.oldSig),
    newSig: hex(r.newSig),
    body: hex(body),
    sas: await rotateSas(body),
    newFingerprint: await deviceFingerprintText(r.signKey, r.dhKey),
  };
}

/**
 * Host identity renewals built and signed here (`docs/host-identity-rotation.md` §3.4, §5.7): a v3 account whose host
 * joins, a device it accepts control from is removed, and the host renews its key twice (automatic, then confirmed on
 * the device, proved against the entry before the head), each time with its next machine key — with the state and
 * its retired keys, what each renewal proves, the confirmation MAC, the code and the ack, `keysAt`, `everNamed` and
 * the chains that must be refused (and why).
 */
async function hostRotationVectors() {
  const account = "ts-acct-r";
  const akSeed = await seed(220);
  const ak = await importEd25519Private(akSeed);
  const akPublic = await publicFromSecret("Ed25519", akSeed);
  const member = async (id: string, n: number) => {
    const signSeed = await seed(n);
    const dhSecret = await seed(n + 1);
    return { id, signSeed, dhSecret, key: await importEd25519Private(signSeed), signKey: await publicFromSecret("Ed25519", signSeed), dhKey: await publicFromSecret("X25519", dhSecret) };
  };
  type Member = Awaited<ReturnType<typeof member>>;
  const admin = await member("ts-r-admin", 221);
  const plain = await member("ts-r-plain", 223);
  const host = await member("ts-r-host", 225);
  const other = await member("ts-r-other", 227);
  const ctl = await member("ts-r-ctl", 229);
  const renewed = [await member("ts-r-host", 231), await member("ts-r-host", 233)] as const;
  const third = await member("ts-r-third", 235);
  const back = await member("ts-r-back", 237);
  const thief = await member("ts-r-host", 239);
  const fresh = await member("ts-r-host", 241);
  const rk = await recoveryKeys((await seed(243)).slice(0, 16));
  const v3 = (m: Member, isHost: boolean, isAdmin: boolean) => ({ id: m.id, signKey: m.signKey, dhKey: m.dhKey, flags: { host: isHost, admin: isAdmin } });
  const byAdmin = { type: "device", id: admin.id } as const;
  const mk = async (id: string, epoch: number): Promise<Action> => ({
    type: "keyEpochs",
    user: null,
    machines: [{ machine: id, epoch, hash: await machineKeyCommit(account, id, epoch, await seed(250 + epoch)) }],
  });
  /** A renewal of `hostId` to `next`, its proof signed by `oldKey` (honestly the host's current key) and the new key, against entry `stagedSeq`. */
  const renewal = async (st: ChainState, hostId: string, oldKey: CryptoKey, next: Member, stagedSeq: number, nonce: Uint8Array, assurance: Assurance): Promise<HostRotation> => {
    const cur = st.deviceRecord(hostId)!.info;
    const r = { host: hostId, signKey: next.signKey, dhKey: next.dhKey, stagedSeq, stagedHead: st.hashAt(stagedSeq)!, nonce: copy(nonce), assurance };
    const body = hostRotationBody(account, r, cur.signKey, cur.dhKey);
    return { ...r, oldSig: await signHostRotate(oldKey, body), newSig: await signHostRotate(next.key, body) };
  };
  const nonce = (b: number) => new Uint8Array(16).fill(b);

  const entries: Entry[] = [await signDraft(genesis3Draft(account, akPublic, v3(admin, false, true), await recoveryKeyFor(rk, account, akPublic), 1_000), ak)];
  const state = await ChainState.verify(entries, undefined, ChainClock.history());
  const labels: { label: string; seq: number }[] = [];
  const step = async (label: string, action: Action) => {
    const entry = await signDraft(state.draft(byAdmin, action, 2_000 + state.seq), admin.key);
    await state.apply(entry, ChainClock.history());
    entries.push(entry);
    labels.push({ label, seq: entry.draft.seq });
  };
  await step("addPlain", { type: "addDevice3", device: v3(plain, false, false) });
  await step("addHost", { type: "addDevice3", device: v3(host, true, false) });
  await step("hostKey", await mk(host.id, 0));
  await step("addOther", { type: "addDevice3", device: v3(other, true, false) });
  await step("otherKey", await mk(other.id, 0));
  await step("addController", { type: "addDevice3", device: v3(ctl, false, false) });
  await step("accept", { type: "setAccept", host: host.id, from: { type: "devices", ids: [ctl.id] } });
  await step("removeController", { type: "removeDevice3", id: ctl.id });
  await step("removalKeys", await mk(host.id, 1));
  await step("rotate", { type: "rotateHostKey", rotation: await renewal(state, host.id, host.key, renewed[0], state.seq, nonce(0x41), ASSURANCE_AUTO) });
  await step("rotateKey", await mk(host.id, 2));
  await step("rotateAgain", { type: "rotateHostKey", rotation: await renewal(state, host.id, renewed[0].key, renewed[1], state.seq - 1, nonce(0x42), ASSURANCE_ON_DEVICE) });
  await step("rotateAgainKey", await mk(host.id, 3));
  const seqOf = (label: string) => labels.find((l) => l.label === label)!.seq;
  const prefix = (label: string) => entries.slice(0, seqOf(label) + 1);
  const before = (seq: number) => ChainState.verify(entries.slice(0, seq), undefined, ChainClock.history());
  const rotate = seqOf("rotate");
  const again = seqOf("rotateAgain");
  const renewals = [await renewalJson(await before(rotate), entries[rotate]!), await renewalJson(await before(again), entries[again]!)];

  // The confirmation, the code and the ack of the first renewal.
  const first = entries[rotate]!.draft.action;
  if (first.type !== "rotateHostKey") throw new Error("a renewal");
  const body = hostRotationBody(account, first.rotation, host.signKey, host.dhKey);
  const ephSecret = await seed(260);
  const ephPublic = await publicFromSecret("X25519", ephSecret);
  const mac = await rotateConfirmFor(await importX25519Private(renewed[0].dhSecret), ephPublic, body);
  if (!(await verifyRotateConfirm(await importX25519Private(ephSecret), renewed[0].dhKey, body, mac))) throw new Error("the admin's side checks the host's MAC");
  const loginNonce = await seed(261);

  // keysAt and everNamed.
  const keysAt = (
    [
      [host.id, 1],
      [host.id, 2],
      [host.id, rotate - 1],
      [host.id, rotate],
      [host.id, again - 1],
      [host.id, again],
      [host.id, state.seq],
      [host.id, state.seq + 1],
      [ctl.id, state.seq],
      ["ts-nobody", 3],
    ] as [string, number][]
  ).map(([id, seq]) => {
    const k = state.keysAt(id, seq);
    return { id, seq, signKey: k ? hex(k.signKey) : null, dhKey: k ? hex(k.dhKey) : null };
  });
  const everNamed = [host.signKey, host.dhKey, renewed[0].signKey, renewed[1].dhKey, ctl.signKey, other.dhKey, fresh.signKey, fresh.dhKey, new Uint8Array(32).fill(7)].map((k) => ({
    key: hex(k),
    named: state.everNamed(k),
  }));

  // Chains that must be refused: a valid prefix and one entry, labelled with the error and the rule.
  const invalid: { name: string; entries: string[]; error: string; rule: string | null }[] = [];
  const refuse = async (name: string, chain: Entry[], entry: Entry, error: string, rule: string | null) => {
    let reason: string | null = null;
    try {
      await ChainState.verify([...chain, entry], undefined, ChainClock.history());
      throw new Error(`${name}: applied`);
    } catch (e) {
      if (!(e instanceof ChainError) || e.kind !== error) throw new Error(`${name}: ${e}`);
      reason = e.detail.reason ?? null;
    }
    if (reason !== rule) throw new Error(`${name}: refused for ${reason}, not ${rule}`);
    invalid.push({ name, entries: [...chain, entry].map((e) => hex(encodeEntry(e))), error, rule });
  };
  const bad = async (name: string, chain: Entry[], action: (st: ChainState) => Promise<Action>, error: string, rule: string | null, signer: Member = admin) => {
    const st = await ChainState.verify(chain, undefined, ChainClock.history());
    const entry = await signDraft(st.draft({ type: "device", id: signer.id }, await action(st), 3_000 + st.seq), signer.key);
    await refuse(name, chain, entry, error, rule);
  };
  const renew = (hostId: string, oldKey: CryptoKey, next: Member = fresh, tweak: (r: HostRotation) => void = () => undefined) => async (st: ChainState): Promise<Action> => {
    const r = await renewal(st, hostId, oldKey, next, st.seq, nonce(9), ASSURANCE_AUTO);
    tweak(r);
    return { type: "rotateHostKey", rotation: r };
  };
  const base = prefix("removalKeys");
  /** `chain` and one more entry by the admin. */
  const plus = async (chain: Entry[], action: Action): Promise<Entry[]> => {
    const st = await ChainState.verify(chain, undefined, ChainClock.history());
    return [...chain, await signDraft(st.draft(byAdmin, action, 3_000 + st.seq), admin.key)];
  };
  await bad("a renewal of a browser or phone", base, renew(plain.id, plain.key), "notAllowed", RULES.rotateNotHost);
  {
    const st = await ChainState.verify(base, undefined, ChainClock.history());
    const join = await signDraft(st.draft({ type: "recovery" }, { type: "recoveryJoin3", device: v3(back, false, true) }, 3_000), rk.signing);
    await bad("a renewal of a pending device", [...base, join], renew(back.id, back.key), "notAllowed", RULES.rotateNotHost);
  }
  await bad("a renewal of a removed host", await plus(base, { type: "removeDevice3", id: other.id }), renew(other.id, other.key), "notAllowed", RULES.rotateNotHost);
  await bad("a renewal to the host's current keys", base, renew(host.id, host.key, host), "notAllowed", RULES.rotateKeyReused);
  await bad("a renewal back to a retired key", prefix("rotateKey"), renew(host.id, renewed[0].key, host), "notAllowed", RULES.rotateKeyReused);
  await bad("a renewal onto another device's key", base, renew(host.id, host.key, admin), "notAllowed", RULES.rotateKeyReused);
  await bad("a host's identity collided with another host's DH key", base, renew(host.id, host.key, { ...fresh, dhKey: other.dhKey }), "notAllowed", RULES.rotateKeyReused);
  await bad("a renewal replayed", prefix("rotateKey"), async () => first, "notAllowed", RULES.rotateKeyReused);
  await bad("a renewal proved against a view ahead of it", base, renew(host.id, host.key, fresh, (r) => void (r.stagedSeq += 1)), "notAllowed", RULES.rotateStaged);
  await bad("a renewal proved against another head", base, renew(host.id, host.key, fresh, (r) => void (r.stagedHead = new Uint8Array(32).fill(7))), "notAllowed", RULES.rotateStaged);
  await bad("a renewal without the old key's signature", base, renew(host.id, host.key, fresh, (r) => void (r.oldSig = new Uint8Array(64))), "notAllowed", RULES.rotateProof);
  await bad("a renewal signed by the retired key", prefix("rotateKey"), renew(host.id, host.key), "notAllowed", RULES.rotateProof);
  await bad("a renewal with a bad new signature", base, renew(host.id, host.key, fresh, (r) => void (r.newSig = Uint8Array.from(r.newSig, (b, i) => (i === 0 ? b ^ 1 : b)))), "notAllowed", RULES.rotateProof);
  await bad("an admin rebinding a host alone", base, renew(host.id, admin.key, thief), "notAllowed", RULES.rotateProof);
  await bad("a renewal signed by a device that is not an admin", base, renew(host.id, host.key), "notAllowed", RULES.onlyAdminSigns, plain);
  await bad("a renewal not followed by the host's machine key", prefix("rotate"), async () => ({ type: "setAccept", host: other.id, from: { type: "all" } }), "notAllowed", RULES.hostKeyNext);
  await bad("a renewal of a host that owes its machine key", await plus(base, { type: "addDevice3", device: v3(third, true, false) }), renew(third.id, third.key), "notAllowed", RULES.hostKeyNext);
  await bad("a renewal to a weak key", base, renew(host.id, host.key, fresh, (r) => void (r.dhKey = new Uint8Array(32))), "weakKey", null);
  // A retired key never names a device again: added (either key), enrolled as a host, back with the words.
  const addDevice = (m: Member, isHost: boolean) => async (): Promise<Action> => ({ type: "addDevice3", device: v3(m, isHost, false) });
  await bad("a device added with a retired sign key", prefix("rotateKey"), addDevice({ ...fresh, id: "ts-r-k1", signKey: host.signKey }, false), "notAllowed", RULES.keyRetired);
  await bad("a device added with a retired DH key", prefix("rotateKey"), addDevice({ ...fresh, id: "ts-r-k2", dhKey: host.dhKey }, false), "notAllowed", RULES.keyRetired);
  await bad("a host enrolled with a key its renewal retired", prefix("rotateAgainKey"), addDevice({ ...renewed[0], id: "ts-r-hk" }, true), "notAllowed", RULES.keyRetired);
  {
    const chain = prefix("rotateKey");
    const st = await ChainState.verify(chain, undefined, ChainClock.history());
    const join = await signDraft(st.draft({ type: "recovery" }, { type: "recoveryJoin3", device: v3({ ...host, id: "ts-r-k3" }, false, true) }, 3_000 + st.seq), rk.signing);
    await refuse("a device back with the recovery words on a retired key", chain, join, "notAllowed", RULES.keyRetired);
  }
  // A pending join made with the host's keys before its renewal does not come alive after it: approved by an admin,
  // activated with the recovery words, or by its passkey.
  {
    const extend = async (chain: Entry[], signer: Signer, key: CryptoKey, action: (st: ChainState) => Promise<Action> | Action): Promise<Entry[]> => {
      const st = await ChainState.verify(chain, undefined, ChainClock.history());
      const entry = await signDraft(st.draft(signer, await action(st), 3_000 + st.seq), key);
      await st.apply(entry, ChainClock.history());
      return [...chain, entry];
    };
    const renewedAfter = async (chain: Entry[]) => {
      const renewed = await extend(chain, byAdmin, admin.key, async (st) => ({ type: "rotateHostKey", rotation: await renewal(st, host.id, host.key, fresh, st.seq, nonce(9), ASSURANCE_AUTO) }));
      return extend(renewed, byAdmin, admin.key, (st) => mk(host.id, st.machineKeyEpochs(host.id)));
    };
    const early = { ...host, id: "ts-r-k0" };
    const joined = await renewedAfter(await extend(base, { type: "recovery" }, rk.signing, () => ({ type: "recoveryJoin3", device: v3(early, false, true) })));
    const lastOf = async (chain: Entry[], signer: Signer, key: CryptoKey, action: Action) => {
      const st = await ChainState.verify(chain, undefined, ChainClock.history());
      return signDraft(st.draft(signer, action, 3_000 + st.seq), key);
    };
    await refuse("a pending device with a key retired since approved", joined, await lastOf(joined, byAdmin, admin.key, { type: "approveDevice", id: early.id }), "notAllowed", RULES.keyRetired);
    await refuse(
      "a recovered device with a key retired since activating itself",
      joined,
      await lastOf(joined, { type: "recovery" }, rk.signing, { type: "activateRecovered", id: early.id }),
      "notAllowed",
      RULES.keyRetired,
    );
    const pakSeed = await seed(262);
    const pak = await importEd25519Private(pakSeed);
    const pakPublic = await publicFromSecret("Ed25519", pakSeed);
    const [credHash, saltNonce] = [await seed(263), await seed(264)];
    const slot: PasskeySlot = { slot: "p-tsr", key: pakPublic, proof: await passkeySlotProof(pak, pakPublic, account, akPublic, "p-tsr", credHash, saltNonce), credHash, saltNonce, grants: 0, selfActivate: true };
    const byPasskey: Signer = { type: "passkey", slot: slot.slot };
    const slotted = await extend(base, byAdmin, admin.key, () => ({ type: "passkeySlotSet", slot }));
    const pkJoined = await renewedAfter(await extend(slotted, byPasskey, pak, () => ({ type: "addDevice3", device: v3(early, false, false) })));
    await refuse(
      "a passkey device with a key retired since activating itself",
      pkJoined,
      await lastOf(pkJoined, byPasskey, pak, { type: "passkeyActivate", id: early.id }),
      "notAllowed",
      RULES.keyRetired,
    );
  }
  // The account's renewal policy (§3.4 rule 9): on (twice: setting it again is allowed), an assured renewal and its
  // key, off, an automatic renewal and its key.
  const policy = await (async () => {
    const st = await ChainState.verify(base, undefined, ChainClock.history());
    const es = [...base];
    const steps: { label: string; seq: number; alwaysAssured: boolean; renewalPolicySeq: number | null }[] = [];
    const step = async (label: string, action: Action) => {
      const entry = await signDraft(st.draft(byAdmin, action, 3_000 + st.seq), admin.key);
      await st.apply(entry, ChainClock.history());
      es.push(entry);
      steps.push({ label, seq: st.seq, alwaysAssured: st.alwaysAssured, renewalPolicySeq: st.renewalPolicySeq });
    };
    await step("policyOn", { type: "setRenewalPolicy", alwaysAssured: true });
    await step("policyOnAgain", { type: "setRenewalPolicy", alwaysAssured: true });
    await step("assuredRenewal", { type: "rotateHostKey", rotation: await renewal(st, host.id, host.key, renewed[0], st.seq, nonce(0x51), ASSURANCE_ON_DEVICE) });
    await step("assuredRenewalKey", await mk(host.id, st.machineKeyEpochs(host.id)));
    await step("policyOff", { type: "setRenewalPolicy", alwaysAssured: false });
    await step("autoRenewal", { type: "rotateHostKey", rotation: await renewal(st, host.id, renewed[0].key, renewed[1], st.seq, nonce(0x52), ASSURANCE_AUTO) });
    await step("autoRenewalKey", await mk(host.id, st.machineKeyEpochs(host.id)));
    if (st.alwaysAssured || st.retiredKeys(host.id).length !== 2) throw new Error("the policy chain ends off, renewed twice");
    return { entries: es, steps };
  })();
  const policyOn = policy.entries.slice(0, base.length + 1);
  await bad(
    "a renewal without assurance while the account asks for it",
    policyOn,
    async (st) => ({ type: "rotateHostKey", rotation: await renewal(st, host.id, host.key, fresh, st.seq, nonce(9), ASSURANCE_AUTO) }),
    "notAllowed",
    RULES.rotateNeedsAssurance,
  );
  await bad("a renewal policy set by a device that is not an admin", base, async () => ({ type: "setRenewalPolicy", alwaysAssured: true }), "notAllowed", RULES.onlyAdminSigns, plain);
  // The renewal pause, an overlay (§3.4 rule 10): the other host accepts every device and the host accepts the other and
  // the plain device (the bases); a second admin joins; a pause removes the other from the host, and the host and the
  // plain device from the other (never an admin: D1; hosts and ids in byte order); a hand setAccept narrows the other's
  // base during it; the host renews; the second admin completes the pause; it starts another, is demoted (the pause
  // stays in force), and the first admin cancels it. Then two pauses overlap on the plain device over the other (the edge
  // stays paused until both end); while the second is open, the other stops hosting and the plain device is removed;
  // ending it gives back only what the base still holds.
  const admin2 = await member("ts-r-admin2", 245);
  const [opA, opB] = [new Uint8Array(16).fill(0xa1), new Uint8Array(16).fill(0xb2)];
  const pauseHosts = [other.id, host.id];
  const pairs: [string, string][] = [
    [host.id, other.id],
    [other.id, host.id],
    [plain.id, other.id],
    [plain.id, host.id],
    [admin.id, other.id],
    [admin.id, host.id],
  ];
  const marker = (op: Uint8Array, phase: PausePhase, removals: PauseRemoval[]): Action => ({ type: "renewalPause", op: copy(op), phase, removals });
  const accept = (hostId: string, from: Accept): Action => ({ type: "setAccept", host: hostId, from });
  const pause = await (async () => {
    const st = await ChainState.verify(base, undefined, ChainClock.history());
    const es = [...base];
    const steps: { label: string; seq: number; state: ReturnType<typeof pauseStateJson> }[] = [];
    const step = async (label: string, by: Member, action: Action) => {
      const entry = await signDraft(st.draft({ type: "device", id: by.id }, action, 3_000 + st.seq), by.key);
      await st.apply(entry, ChainClock.history());
      es.push(entry);
      steps.push({ label, seq: st.seq, state: pauseStateJson(st, pauseHosts, pairs) });
    };
    await step("acceptAll", admin, accept(other.id, { type: "all" }));
    await step("acceptHost", admin, accept(host.id, { type: "devices", ids: [other.id, plain.id] }));
    await step("addAdmin", admin, { type: "addDevice3", device: v3(admin2, false, true) });
    await step("pauseStart", admin, marker(opA, PAUSE_START, [{ host: host.id, ids: [other.id] }, { host: other.id, ids: [host.id, plain.id] }]));
    await step("handOther", admin, accept(other.id, { type: "devices", ids: [plain.id] }));
    await step("handOtherKey", admin, await mk(other.id, st.machineKeyEpochs(other.id)));
    await step("renew", admin, { type: "rotateHostKey", rotation: await renewal(st, host.id, host.key, renewed[0], st.seq, nonce(0x61), ASSURANCE_AUTO) });
    await step("renewKey", admin, await mk(host.id, st.machineKeyEpochs(host.id)));
    await step("pauseCompleted", admin2, marker(opA, PAUSE_COMPLETED, []));
    await step("pause2Start", admin2, marker(opB, PAUSE_START, [{ host: other.id, ids: [plain.id] }]));
    await step("demoteStarter", admin, { type: "setAdmin", id: admin2.id, admin: false });
    if (st.pausedEdges(other.id).join() !== plain.id) throw new Error("a demoted starter's pause stays in force");
    await step("pause2Cancelled", admin, marker(opB, PAUSE_CANCELLED, []));
    if (st.openPauses().length !== 0 || st.pausedEdges(other.id).length !== 0) throw new Error("the pause chain ends with nothing open");
    const [opD, opE] = [new Uint8Array(16).fill(0xd1), new Uint8Array(16).fill(0xe2)];
    await step("overlapStart", admin, marker(opD, PAUSE_START, [{ host: other.id, ids: [plain.id] }]));
    await step("overlap2Start", admin, marker(opE, PAUSE_START, [{ host: host.id, ids: [other.id, plain.id] }, { host: other.id, ids: [plain.id] }]));
    await step("overlapCompleted", admin, marker(opD, PAUSE_COMPLETED, []));
    if (st.pausedEdges(other.id).join() !== plain.id) throw new Error("still paused by the other pause");
    await step("pausedHostStops", admin, { type: "setHost", id: other.id, host: false });
    await step("pausedDeviceRemoved", admin, { type: "removeDevice3", id: plain.id });
    await step("overlap2Completed", admin, marker(opE, PAUSE_COMPLETED, []));
    if (st.openPauses().length !== 0 || st.pausedEdges(host.id).length !== 0) throw new Error("nothing open at the end");
    st.mayControl(other.id, host.id);
    return { entries: es, steps };
  })();
  const pauseSeq = (label: string) => pause.steps.find((s) => s.label === label)!.seq;
  const pauseUpto = (label: string) => pause.entries.slice(0, pauseSeq(label) + 1);
  const withEntry = async (chain: Entry[], signer: Signer, key: CryptoKey, action: Action): Promise<Entry[]> => {
    const st = await ChainState.verify(chain, undefined, ChainClock.history());
    const entry = await signDraft(st.draft(signer, action, 3_000 + st.seq), key);
    await st.apply(entry, ChainClock.history());
    return [...chain, entry];
  };
  const pendingChain = await withEntry(base, { type: "recovery" }, rk.signing, { type: "recoveryJoin3", device: v3({ ...back, id: "ts-r-pending" }, false, true) });
  const removedChain = await withEntry(base, byAdmin, admin.key, { type: "removeDevice3", id: other.id });
  const opC = new Uint8Array(16).fill(0xc1);
  const one = (hostId: string, ids: string[]): PauseRemoval[] => [{ host: hostId, ids }];
  for (const [name, chain, action, rule] of [
    ["a renewal pause that starts under a used op", pauseUpto("pauseCompleted"), marker(opA, PAUSE_START, one(other.id, [plain.id])), RULES.pauseOpReused],
    ["a renewal pause that starts under an open op", pauseUpto("pauseStart"), marker(opA, PAUSE_START, one(other.id, [plain.id])), RULES.pauseOpReused],
    ["a renewal pause naming a pending device as a host", pendingChain, marker(opC, PAUSE_START, one("ts-r-pending", [other.id])), RULES.pauseHosts],
    ["a renewal pause naming a removed host", removedChain, marker(opC, PAUSE_START, one(other.id, [plain.id])), RULES.pauseHosts],
    ["a renewal pause naming a device that does not run AI as a host", base, marker(opC, PAUSE_START, one(plain.id, [other.id])), RULES.pauseHosts],
    ["a renewal pause naming a host twice", base, marker(opC, PAUSE_START, [...one(other.id, [plain.id]), ...one(other.id, [host.id])]), RULES.pauseHosts],
    ["a renewal pause with no removals", base, marker(opC, PAUSE_START, []), RULES.pauseHosts],
    ["a renewal pause removing nothing from a host", base, marker(opC, PAUSE_START, one(other.id, [])), RULES.pauseIds],
    ["a renewal pause removing a host from itself", base, marker(opC, PAUSE_START, one(other.id, [other.id])), RULES.pauseIds],
    ["a renewal pause removing an unknown device", base, marker(opC, PAUSE_START, one(other.id, ["ts-nobody"])), RULES.pauseIds],
    ["a renewal pause removing a device twice", base, marker(opC, PAUSE_START, one(other.id, [plain.id, plain.id])), RULES.pauseIds],
    ["a renewal pause removing an admin", base, marker(opC, PAUSE_START, one(other.id, [admin.id])), RULES.pauseIds],
    ["a renewal pause naming its hosts out of byte order", base, marker(opC, PAUSE_START, [...one(other.id, [plain.id]), ...one(host.id, [plain.id])]), RULES.pauseHosts],
    ["a renewal pause removing ids out of byte order", base, marker(opC, PAUSE_START, one(other.id, [plain.id, host.id])), RULES.pauseIds],
    ["a renewal pause that ends an unknown op", base, marker(new Uint8Array(16).fill(0xcc), PAUSE_CANCELLED, []), RULES.pauseNotOpen],
    ["a renewal pause that ends twice", pauseUpto("pauseCompleted"), marker(opA, PAUSE_COMPLETED, []), RULES.pauseNotOpen],
    ["a renewal pause that ends naming removals", pauseUpto("pauseStart"), marker(opA, PAUSE_COMPLETED, one(host.id, [other.id])), RULES.pauseEndRemovals],
  ] as [string, Entry[], Action, string][]) {
    await bad(name, chain, async () => action, "notAllowed", rule);
  }
  const startC = () => marker(opC, PAUSE_START, one(other.id, [plain.id]));
  await bad("a renewal pause by a device that is not an admin", base, async () => startC(), "notAllowed", RULES.onlyAdminSigns, plain);
  {
    const qSeed = await seed(266);
    const q = await importEd25519Private(qSeed);
    const qPublic = await publicFromSecret("Ed25519", qSeed);
    const [qCred, qSalt] = [await seed(267), await seed(268)];
    const qSlot: PasskeySlot = { slot: "p-tsq", key: qPublic, proof: await passkeySlotProof(q, qPublic, account, akPublic, "p-tsq", qCred, qSalt), credHash: qCred, saltNonce: qSalt, grants: 0, selfActivate: true };
    const slotted = await withEntry(base, byAdmin, admin.key, { type: "passkeySlotSet", slot: qSlot });
    const byQ = await signDraft((await ChainState.verify(slotted, undefined, ChainClock.history())).draft({ type: "passkey", slot: "p-tsq" }, startC(), 3_000 + slotted.length - 1), q);
    await refuse("a renewal pause signed by a passkey slot", slotted, byQ, "notAllowed", "a passkey slot signs only its own joins and activations");
    const byRecovery = await signDraft((await ChainState.verify(base, undefined, ChainClock.history())).draft({ type: "recovery" }, startC(), 3_000 + base.length - 1), rk.signing);
    await refuse("a renewal pause signed by the recovery key", base, byRecovery, "notAllowed", "the recovery key signs only recovery entries");
  }
  // The size bound (大小上限): one byte past it is refused for that alone; at it, the ids' own rule speaks (they name no
  // devices).
  const sized = (len: number) => async (st: ChainState): Promise<Action> => {
    const length = (ids: string[]) => encodeDraft(st.draft(byAdmin, marker(opC, PAUSE_START, one(other.id, ids)), 0)).length + 64;
    const ids: string[] = [];
    while (length(ids) + 2 * 66 <= len) ids.push(`x${String(ids.length).padStart(63, "0")}`);
    const rest = len - length(ids) - 4;
    ids.push("y".repeat(Math.floor(rest / 2)), "z".repeat(rest - Math.floor(rest / 2)));
    if (length(ids) !== len) throw new Error(`sized ${length(ids)}, not ${len}`);
    return marker(opC, PAUSE_START, one(other.id, ids));
  };
  await bad("a renewal pause one byte too large for every transport", base, sized(MAX_PAUSE_ENTRY_LEN + 1), "notAllowed", RULES.pauseTooLarge);
  await bad("a renewal pause of the largest size, removing no devices", base, sized(MAX_PAUSE_ENTRY_LEN), "notAllowed", RULES.pauseIds);
  // Bytes that do not decode: an assurance other than 0 or 1, kind 20 in a v2 entry.
  const rotateBytes = encodeEntry(entries[rotate]!);
  for (const [name, at, value] of [
    ["a renewal with an unknown assurance", rotateBytes.length - 64 * 3 - 1, 2],
    ["a renewal inside a v2 entry", 0, CHAIN_VERSION],
  ] as [string, number, number][]) {
    const b = copy(rotateBytes);
    b[at] = value;
    try {
      decodeEntry(b);
      throw new Error(`${name}: decoded`);
    } catch (e) {
      if (!(e instanceof ChainError) || e.kind !== "decode") throw new Error(`${name}: ${e}`);
    }
    invalid.push({ name, entries: [...entries.slice(0, rotate).map((e) => hex(encodeEntry(e))), hex(b)], error: "decode", rule: null });
  }
  // Kind 22 with a phase other than 0, 1 or 2, under version 2, naming more than 256 hosts or removing more than 1024
  // edges: none decodes.
  const pauseBytes: [string, Bytes, Entry[]][] = [];
  {
    const completedBytes = encodeEntry(pause.entries[pauseSeq("pauseCompleted")]!);
    const phaseAt = completedBytes.length - 64 - 2 - 1;
    if (completedBytes[phaseAt - 17] !== 22 || completedBytes[phaseAt] !== 1) throw new Error("kind 22, op, then the phase");
    const before = pause.entries.slice(0, pauseSeq("pauseCompleted"));
    for (const [name, at, value] of [
      ["a renewal pause with an unknown phase", phaseAt, 3],
      ["a renewal pause inside a v2 entry", 0, CHAIN_VERSION],
    ] as [string, number, number][]) {
      const b = copy(completedBytes);
      b[at] = value;
      pauseBytes.push([name, b, before]);
    }
    const st = await ChainState.verify(base, undefined, ChainClock.history());
    const tooMany = Array.from({ length: 257 }, (_, i) => ({ host: `h-${i}`, ids: [plain.id] }));
    const tooWide = [{ host: other.id, ids: Array.from({ length: 1025 }, (_, i) => `d-${i}`) }];
    for (const [name, removals] of [
      ["a renewal pause naming more than 256 hosts", tooMany],
      ["a renewal pause removing more than 1024 edges", tooWide],
    ] as [string, PauseRemoval[]][]) {
      pauseBytes.push([name, encodeEntry(await signDraft(st.draft(byAdmin, marker(opC, PAUSE_START, removals), 3_000 + st.seq), admin.key)), base]);
    }
  }
  for (const [name, b, before] of pauseBytes) {
    try {
      decodeEntry(b);
      throw new Error(`${name}: decoded`);
    } catch (e) {
      if (!(e instanceof ChainError) || e.kind !== "decode") throw new Error(`${name}: ${e}`);
    }
    invalid.push({ name, entries: [...before.map((e) => hex(encodeEntry(e))), hex(b)], error: "decode", rule: null });
  }
  // Kind 21 with a byte other than 0 or 1, and under version 2: neither decodes.
  const policyBytes = encodeEntry(policyOn[policyOn.length - 1]!);
  const policyAt = policyBytes.length - 64 - 1;
  if (policyBytes[policyAt - 1] !== 21 || policyBytes[policyAt] !== 1) throw new Error("kind 21, then its byte");
  for (const [name, at, value] of [
    ["a renewal policy with an unknown byte", policyAt, 2],
    ["a renewal policy inside a v2 entry", 0, CHAIN_VERSION],
  ] as [string, number, number][]) {
    const b = copy(policyBytes);
    b[at] = value;
    try {
      decodeEntry(b);
      throw new Error(`${name}: decoded`);
    } catch (e) {
      if (!(e instanceof ChainError) || e.kind !== "decode") throw new Error(`${name}: ${e}`);
    }
    invalid.push({ name, entries: [...base.map((e) => hex(encodeEntry(e))), hex(b)], error: "decode", rule: null });
  }

  // Disputes (§3.8): each renewal's statement by the key it retired, and statements that must not verify.
  const disputes = await (async () => {
    const hashOf = (seq: number) => state.hashAt(seq)!;
    const newSignOf = (seq: number) => {
      const a = entries[seq]!.draft.action;
      return a.type === "rotateHostKey" ? a.rotation.signKey : new Uint8Array(32);
    };
    const statement = (key: CryptoKey, hostId: string, seq: number, entryHash: Uint8Array, newSign: Uint8Array) => signRotateDispute(key, account, hostId, seq, entryHash, newSign);
    const valid: Record<string, unknown>[] = [];
    for (const [seq, key] of [
      [rotate, host],
      [again, renewed[0]],
    ] as [number, Member][]) {
      const signature = await statement(key.key, host.id, seq, hashOf(seq), newSignOf(seq));
      if (!(await state.verifyRotateDispute(host.id, seq, signature))) throw new Error("a renewal's dispute verifies");
      valid.push({
        host: host.id,
        seq,
        entryHash: hex(hashOf(seq)),
        newSign: hex(newSignOf(seq)),
        body: hex(rotateDisputeBody(account, host.id, seq, hashOf(seq), newSignOf(seq))),
        signerSeed: hex(key.signSeed),
        signature: hex(signature),
      });
    }
    const first = await statement(host.key, host.id, rotate, hashOf(rotate), newSignOf(rotate));
    const notRenewal = rotate + 1;
    const cases: [string, string, number, Uint8Array][] = [
      ["the statement of one renewal for another", host.id, again, first],
      ["an entry that is not a renewal", host.id, notRenewal, await statement(renewed[0].key, host.id, notRenewal, hashOf(notRenewal), newSignOf(rotate))],
      ["signed by the new key", host.id, rotate, await statement(renewed[0].key, host.id, rotate, hashOf(rotate), newSignOf(rotate))],
      ["signed by another device", host.id, rotate, await statement(admin.key, host.id, rotate, hashOf(rotate), newSignOf(rotate))],
      ["for another host", other.id, rotate, await statement(host.key, other.id, rotate, hashOf(rotate), newSignOf(rotate))],
      ["over a hash that is not the entry's", host.id, rotate, await statement(host.key, host.id, rotate, new Uint8Array(32).fill(7), newSignOf(rotate))],
      ["over another new key", host.id, rotate, await statement(host.key, host.id, rotate, hashOf(rotate), new Uint8Array(32).fill(7))],
      ["past this view", host.id, state.seq + 1, first],
      ["the genesis", host.id, 0, first],
    ];
    const invalid: Record<string, unknown>[] = [];
    for (const [name, hostId, seq, signature] of cases) {
      if (await state.verifyRotateDispute(hostId, seq, signature)) throw new Error(`${name}: verified`);
      invalid.push({ name, host: hostId, seq, signature: hex(signature) });
    }
    return { valid, invalid };
  })();

  return {
    account,
    accountFingerprint: hex(await fingerprint(akPublic)),
    entries: entries.map((e) => hex(encodeEntry(e))),
    hashes: await Promise.all(entries.map(async (e) => hex(await entryHash(e)))),
    steps: labels,
    state: rotationStateJson(state, [host.id, other.id]),
    renewals,
    confirm: { body: hex(body), ephSecret: hex(ephSecret), ephPublic: hex(ephPublic), newDhSecret: hex(renewed[0].dhSecret), newDhPublic: hex(renewed[0].dhKey), mac: hex(mac) },
    ack: {
      account,
      host: host.id,
      body: hex(body),
      loginNonce: hex(loginNonce),
      ackBody: hex(await rotateAckBody(account, host.id, body, loginNonce)),
      newSignSeed: hex(renewed[0].signSeed),
      newSign: hex(renewed[0].signKey),
      signature: hex(await signRotateAck(renewed[0].key, account, host.id, body, loginNonce)),
    },
    keysAt,
    everNamed,
    invalid,
    disputes,
    pause: {
      entries: pause.entries.map((e) => hex(encodeEntry(e))),
      steps: pause.steps,
      hosts: pauseHosts,
      pairs,
      maxEntryLen: MAX_PAUSE_ENTRY_LEN,
      encoding: {
        start: hex(encodeEntry(pause.entries[pauseSeq("pauseStart")]!)),
        completed: hex(encodeEntry(pause.entries[pauseSeq("pauseCompleted")]!)),
        cancelled: hex(encodeEntry(pause.entries[pauseSeq("pause2Cancelled")]!)),
      },
    },
    policy: {
      entries: policy.entries.map((e) => hex(encodeEntry(e))),
      steps: policy.steps,
      encoding: { on: hex(encodeEntry(policy.entries[base.length]!)), off: hex(encodeEntry(policy.entries[base.length + 4]!)) },
    },
  };
}

async function noiseVectors() {
  const device = await keyPairFromSecret(await seed(40));
  const agent = await keyPairFromSecret(await seed(41));
  const ie = await seed(42);
  const re = await seed(43);
  const binding = { account: "ts-acct", device: "ts-dev-0", machine: "ts-m-1" };
  const policy: RekeyPolicy = { maxMessages: 3, maxAgeMs: 600_000 };
  const start = 1_760_000_000_000;
  const initiator = await Initiator.create(device, agent.publicKey, binding, await keyPairFromSecret(ie));
  const responder = await Responder.create(agent, binding, await keyPairFromSecret(re));
  // Structured payloads: a chain view, never a request.
  const helloPayload = encodeHelloBody({ chainSeq: 7, chainHead: await seed(44), entries: [], client: utf8("web/ts") });
  const m1 = await initiator.writeHello(start, helloPayload);
  await responder.readHello(m1);
  const replyPayload = encodeReplyBody({ status: "accepted", chainSeq: 7, chainHead: await seed(44), needFrom: 0, server: utf8('{"features":["term"]}'), entries: [] });
  const { message: m2, transport: rt } = await responder.writeReply(replyPayload, policy, start);
  const { transport: it } = await initiator.readReply(m2, policy, start);
  const script: ["initiator" | "responder", number, string][] = [
    ["initiator", start + 10, "a"],
    ["responder", start + 20, "b"],
    ["initiator", start + 30, "c"],
    ["initiator", start + 40, "d: rekeys"],
    ["initiator", start + 50, "e: new key"],
    ["responder", start + 700_000, "f: rekeys by age"],
    ["responder", start + 700_010, ""],
    ["initiator", start + 700_020, "g: rekeys by age"],
  ];
  const transport = [];
  for (const [from, now, text] of script) {
    const [tx, rx] = from === "initiator" ? [it, rt] : [rt, it];
    const message = await tx.seal(now, utf8(text));
    await rx.open(message);
    transport.push({ from, nowMs: now, data: hex(utf8(text)), message: hex(message) });
  }
  return {
    binding,
    prologue: hex(prologue(binding)),
    policy,
    initiator: { static: hex(await seed(40)), ephemeral: hex(ie) },
    responder: { static: hex(await seed(41)), ephemeral: hex(re) },
    hello: { timestampMs: start, payload: hex(helloPayload), message: hex(m1) },
    reply: { payload: hex(replyPayload), message: hex(m2) },
    handshakeHash: hex(it.handshakeHash),
    transport,
  };
}

async function recordVectors() {
  const mkRaw = await seed(50);
  const mk = await importMachineKey(mkRaw);
  const machineSeed = await seed(52);
  const machine = await importEd25519Private(machineSeed);
  const source = "codex";
  const id = "019a2b3c-0000-7000-8000-00000000abcd";
  const epoch = 1;
  const stream = await streamId(mk, source, id);
  const key = await streamKey(mk, stream);
  const bodies: [Bytes, Compression][] = [
    [utf8('{"fromIndex":0,"items":[],"endIndex":0,"fp":"0"}'), "none"],
    [utf8("tool output line\n".repeat(300)), "deflate"],
    [await seed(51), "none"],
  ];
  let link: Bytes = new Uint8Array(32);
  const records = [];
  for (let seq = 0; seq < bodies.length; seq++) {
    const [body, compression] = bodies[seq]!;
    const nonce = (await seed(60 + seq)).slice(0, 12);
    const sealed = await sealRecord(key, stream, seq, epoch, FMT_FEED_V1, link, body, compression, nonce);
    records.push({ seq, wire: hex(encodeSealedRecord(sealed)), fmt: FMT_FEED_V1, body: hex(body) });
    link = await recordLink(stream, link, sealed);
  }
  const head = await signStreamHead({ stream, nextSeq: bodies.length, epoch, link, timeMs: 1_760_000_000_000 }, machine);
  return {
    mk: hex(mkRaw),
    epoch,
    source,
    id,
    machineKey: hex(await publicFromSecret("Ed25519", machineSeed)),
    streamId: hex(stream),
    records,
    head: hex(encodeSignedHead(head)),
  };
}

/** A page, a manifest naming it and a draft, sealed and signed here (no deflate: the bytes are fixed). */
async function streamVectors() {
  const mkRaw = await seed(110);
  const mk = await importMachineKey(mkRaw);
  const machineSeed = await seed(111);
  const machine = await importEd25519Private(machineSeed);
  const source = "claude";
  const id = "ts-session-0001";
  const epoch = 4;
  const session = await streamId(mk, source, id);
  const key = await streamKey(mk, session);
  const bodies = [
    utf8('{"fromIndex":0,"items":[{"kind":"user","text":"ts page"}],"endIndex":1,"fp":"t1"}'),
    utf8('{"fromIndex":1,"items":[{"kind":"assistant","text":"second page"}],"endIndex":2,"fp":"t2"}'),
  ];
  const pages = [];
  const hashes: Bytes[] = [];
  for (let i = 0; i < bodies.length; i++) {
    const wire = encodePage(await sealPage(key, session, epoch, FMT_FEED_V1, bodies[i]!, "none", (await seed(112 + i)).slice(0, 12)));
    const hash = await pageHash(wire);
    hashes.push(hash);
    pages.push({ wire: hex(wire), hash: hex(hash), fmt: FMT_FEED_V1, body: hex(bodies[i]!) });
  }
  const head = await signStreamHead({ stream: session, nextSeq: 7, epoch, link: await seed(115), timeMs: 1_760_000_200_000 }, machine);
  const manifest = await signManifest(head, hashes, machine);
  const draftBody = utf8('{"turn":"t","v":1,"blocks":[]}');
  const draft = await sealEphemeral(key, session, epoch, draftBody, machine, (await seed(116)).slice(0, 12));
  return {
    mk: hex(mkRaw),
    epoch,
    source,
    id,
    machineKey: hex(await publicFromSecret("Ed25519", machineSeed)),
    indexStream: hex(await indexStream(mk)),
    statusStream: hex(await statusStream(mk)),
    sessionStream: hex(session),
    pages,
    manifest: hex(encodeManifest(manifest)),
    draft: { wire: hex(draft), body: hex(draftBody) },
  };
}

/**
 * Stream names per key epoch (relay-and-plans §5): one machine's index,
 * status and a session under two epochs' keys, and index entries naming the
 * session's stream — with `se`, and without (the first epoch).
 */
async function streamNameVectors() {
  const source = "codex";
  const id = "ts-session-epochs";
  const epochs = [];
  const keys = new Map<number, Awaited<ReturnType<typeof importMachineKey>>>();
  for (const [epoch, n] of [
    [1, 117],
    [4, 118],
  ] as const) {
    const raw = await seed(n);
    const mk = await importMachineKey(raw);
    keys.set(epoch, mk);
    epochs.push({ epoch, mk: hex(raw), indexStream: hex(await indexStream(mk)), statusStream: hex(await statusStream(mk)), sessionStream: hex(await streamId(mk, source, id)) });
  }
  return {
    source,
    id,
    feature: STREAMS_FEATURE,
    epochs,
    entries: [
      { entry: { source, id, title: "moved", se: 4 }, sessionStream: hex(await streamId(keys.get(4)!, source, id)) },
      { entry: { source, id, title: "from before" }, sessionStream: hex(await streamId(keys.get(1)!, source, id)) },
    ],
  };
}

/**
 * Recovery words, CPace with fixed scalars, a pairing delivery and a recovery
 * package, made here — on a chain that commits user key epochs 0–2 (the
 * delivery carries 0, the recovery package 2).
 */
async function pairingVectors() {
  const account = "ts-acct-p";
  const rk = (await seed(130)).slice(0, 16);
  const rkeys = await recoveryKeys(rk);
  const akSeed = await seed(131);
  const akPublic = await publicFromSecret("Ed25519", akSeed);
  const dev0 = { sign: await importEd25519Private(await seed(132)), signPublic: await publicFromSecret("Ed25519", await seed(132)), dh: await keyPairFromSecret(await seed(133)) };
  const info0 = { id: "ts-dev-0", signKey: dev0.signPublic, dhKey: dev0.dh.publicKey, full: true };
  const genesis = await signDraft(genesisDraft(account, akPublic, info0, await recoveryKeyFor(rkeys, account, akPublic), 1_000), await importEd25519Private(akSeed));
  const chain = await ChainState.verify([genesis], undefined, ChainClock.history());
  const chainEntries: Entry[] = [genesis];
  const uk = await seed(137);
  const recoveryUk = await seed(141);
  for (const [epoch, key] of [uk, await seed(144), recoveryUk].entries()) {
    const commit = await signDraft(
      chain.draft({ type: "device", id: "ts-dev-0" }, { type: "keyEpochs", user: { epoch, hash: await userKeyCommit(account, epoch, key) }, machines: [] }, 1_500),
      dev0.sign,
    );
    await chain.apply(commit, ChainClock.history());
    chainEntries.push(commit);
  }
  const newSignSeed = await seed(134);
  const newDhSecret = await seed(135);
  const newInfo = { id: "ts-dev-n", signKey: await publicFromSecret("Ed25519", newSignSeed), dhKey: await publicFromSecret("X25519", newDhSecret), full: true };
  const add = await signDraft(chain.draft({ type: "device", id: "ts-dev-0" }, { type: "addDevice", device: newInfo }, 2_000), dev0.sign);
  await chain.apply(add, ChainClock.history());
  chainEntries.push(add);
  const secret = await seed(136);
  const delivery = await createDelivery(chain, "ts-dev-0", dev0.signPublic, dev0.dh, "ts-dev-n", newInfo.dhKey, [{ epoch: 0, key: uk }], secret);
  const code = utf8("13572468");
  const ci = channelId(account, "ts-dev-n");
  const sid = (await seed(138)).slice(0, 16);
  const [sa, sb] = [await seed(139), await seed(140)];
  const a = await Cpace.start("initiator", code, ci, sid, sa);
  const b = await Cpace.start("responder", code, ci, sid, sb);
  const keys = await b.finish(a.share);
  const hello = { sid, share: a.share };
  const answer = await pakeAnswer(hello, b.share, "ts-dev-n", newInfo.signKey, newInfo.dhKey, keys);
  const offer = offerText({ account, device: "ts-dev-n", signKey: newInfo.signKey, dhKey: newInfo.dhKey, secret });
  const sealed = await sealForRecovery(dev0.dh, "ts-dev-0", account, rkeys.signingPublic, 2, recoveryUk);
  const mnemonic = [];
  for (const e of [(await seed(142)).slice(0, 16), await seed(143)]) mnemonic.push({ entropy: hex(e), words: (await mnemonicFromEntropy(e)).join(" ") });
  return {
    mnemonic,
    pake: {
      code: "13572468",
      account,
      device: "ts-dev-n",
      sid: hex(sid),
      generator: hex(await cpaceGenerator(code, ci, sid)),
      initiatorScalar: hex(sa),
      responderScalar: hex(sb),
      ya: hex(a.share),
      yb: hex(b.share),
      pairSecret: hex(keys.pairSecret),
    },
    hello: hex(encodePakeHello(hello)),
    answer: hex(encodePakeAnswer(answer)),
    // `textDr`: the same offer with the newer prefix, which readers accept too (nothing writes it yet).
    offer: { text: offer, textDr: offer.replace(/^RHP1-/, "DRP1-") },
    delivery: { hex: hex(encodeDelivery(delivery)), recipient: "ts-dev-n", recipientDhSecret: hex(newDhSecret), uk: hex(uk), chain: chainEntries.map((e) => hex(encodeEntry(e))) },
    recovery: { rk: hex(rk), sealPublic: hex(rkeys.seal.publicKey), sender: "ts-dev-0", epoch: 2, sealed: hex(sealed), uk: hex(recoveryUk) },
  };
}

function ctxJson(c: SealContext) {
  return { purpose: c.purpose === "userKey" ? 1 : 2, account: c.account, sender: c.sender, recipient: c.recipient, epoch: c.epoch, hex: hex(encodeSealContext(c)) };
}

async function wrapVectors() {
  const senderSecret = await seed(69);
  const sender = await keyPairFromSecret(senderSecret);
  const cases: [SealContext, Bytes, Bytes][] = [
    [{ purpose: "userKey", account: "ts-acct", sender: "ts-dev-0", recipient: "ts-dev-3", epoch: 2 }, await seed(70), await seed(71)],
    [{ purpose: "machineKey", account: "ts-acct", sender: "ts-dev-0", recipient: "ts-m-1", epoch: 0 }, await seed(73), await seed(72)],
  ];
  const sealed = [];
  for (let i = 0; i < cases.length; i++) {
    const [ctx, recipientSecret, secret] = cases[i]!;
    const recipient = await publicFromSecret("X25519", recipientSecret);
    const blob = await sealTo(sender, recipient, ctx, secret, { ephemeral: await keyPairFromSecret(await seed(80 + i)), nonce: (await seed(90 + i)).slice(0, 12) });
    sealed.push({ context: ctxJson(ctx), recipientSecret: hex(recipientSecret), secret: hex(secret), sealed: hex(blob) });
  }
  const uk = await seed(71);
  const ctx = context.machineKeyUnderUserKey("ts-acct", "ts-m-1", 0, 2);
  const wrapped = await wrapUnder(uk, ctx, await seed(72), (await seed(95)).slice(0, 12));
  const s = await seed(96);
  const fields = [utf8("ts-acct"), utf8("ts-dev-0"), sender.publicKey];
  return {
    senderSecret: hex(senderSecret),
    sealed,
    symmetric: [{ key: hex(uk), context: hex(ctx), secret: hex(await seed(72)), wrapped: hex(wrapped) }],
    pairing: { secret: hex(s), fields: fields.map(hex), tag: hex(await pairingTag(s, fields)) },
  };
}

async function frameVectors() {
  const stream = await seed(100);
  const frames: Frame[] = [
    { type: "open", ch: 3, peer: "ts-m-1" },
    { type: "data", ch: 3, payload: utf8("opaque") },
    { type: "close", ch: 3, code: 4000, reason: "revoked" },
    { type: "credit", ch: 3, bytes: 65_536 },
    { type: "ping", nonce: 12_345 },
    { type: "append", ch: 5, stream, expectSeq: 9, records: [{ seq: 9, epoch: 1, nonce: new Uint8Array(12), ct: new Uint8Array(16).fill(7) }] },
    { type: "sub", ch: 6, stream, fromSeq: 0 },
    { type: "chain", ch: 7, entries: [utf8("e1"), utf8("e2")] },
    { type: "pushSend", ch: 8, subscription: "s", ttl: 30, payload: utf8("p") },
    { type: "page", ch: 9, stream, hash: await seed(101), data: utf8("sealed page bytes") },
  ];
  const mux: MuxFrame[] = [
    { type: "open", sid: 3, class: "rpc", window: 1000, meta: utf8("GET /api/stats") },
    { type: "data", sid: 3, data: utf8("{}") },
    { type: "credit", sid: 4, delta: 500 },
    { type: "close", sid: 3, code: 0 },
    { type: "ping", nonce: 1 },
  ];
  return {
    frames: frames.map((f) => ({ hex: hex(encodeFrame(f)) })),
    muxFrames: mux.map((f) => ({ hex: hex(encodeMuxFrame(f)) })),
  };
}

export async function buildTsVectors() {
  const { frames, muxFrames } = await frameVectors();
  return {
    version: 1,
    generator: "web/src/lib/e2e (TypeScript, WebCrypto): npx tsx web/scripts/e2e-vectors.ts",
    chain: await chainVectors(),
    chainV3: await chainV3Vectors(),
    hostRotation: await hostRotationVectors(),
    noise: await noiseVectors(),
    records: await recordVectors(),
    streams: await streamVectors(),
    streamNames: await streamNameVectors(),
    pairing: await pairingVectors(),
    wrap: await wrapVectors(),
    frames,
    muxFrames,
  };
}

export function serialize(vectors: unknown): string {
  return JSON.stringify(vectors, null, 2) + "\n";
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const vectors = await buildTsVectors();
  writeFileSync(TS_VECTORS_PATH, serialize(vectors));
  console.log(`wrote ${TS_VECTORS_PATH}`);
}
