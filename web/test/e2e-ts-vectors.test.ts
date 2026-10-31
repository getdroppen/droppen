// TypeScript → Rust interop: the committed `server-rs/crates/droppen-proto/vectors/ts.json`
// is what `web/scripts/e2e-vectors.ts` produces today.
//
// The Rust test `vectors::ts_vectors_verify_in_rust` replays that file (a
// TS-signed chain, a TS handshake and transport transcript, TS-sealed records
// and TS-wrapped keys) against the agent's implementation; this one makes sure
// the file it replays has not drifted from the code that would produce it. The
// record ciphertexts are the one part checked by opening rather than by bytes:
// they contain raw-deflate output, which may differ between zlib builds.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { buildTsVectors, serialize, TS_VECTORS_PATH } from "../scripts/e2e-vectors.js";
import { decodeSealedRecordBytes, decodeSignedHead, fromHex, hex, importMachineKey, LogReader } from "../src/lib/e2e/index.js";

test("ts.json is what the TypeScript implementation produces now", async () => {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const committed: any = JSON.parse(readFileSync(TS_VECTORS_PATH, "utf8"));
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const fresh: any = JSON.parse(serialize(await buildTsVectors()));
  // Record ciphertexts hold raw-deflate output, which may differ between zlib
  // builds, and the head signs their links; both are checked by opening below.
  // The pairing delivery and the recovery package are sealed with fresh
  // ephemeral keys and nonces: checked by opening (in Rust), not by bytes.
  const withoutWire = (v: typeof committed) => ({
    ...v,
    records: { ...v.records, head: undefined, records: v.records.records.map(({ wire: _, ...rest }: { wire: string }) => rest) },
    pairing: { ...v.pairing, delivery: { ...v.pairing.delivery, hex: undefined }, recovery: { ...v.pairing.recovery, sealed: undefined } },
  });
  assert.deepEqual(withoutWire(fresh), withoutWire(committed), "regenerate with: npx tsx web/scripts/e2e-vectors.ts (then run the droppen-proto tests)");

  const r = committed.records;
  const mk = await importMachineKey(fromHex(r.mk));
  const reader = LogReader.fromStart(fromHex(r.streamId), fromHex(r.machineKey));
  await reader.addEpoch(r.epoch, mk);
  for (const rec of r.records) await reader.push(decodeSealedRecordBytes(fromHex(rec.wire)));
  const released = await reader.confirm(decodeSignedHead(fromHex(r.head)));
  assert.deepEqual(released.map((o) => hex(o.body)), r.records.map((rec: { body: string }) => rec.body));
});
