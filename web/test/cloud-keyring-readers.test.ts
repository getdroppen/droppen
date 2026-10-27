// The page's side: the hub serves every copy of a wrap and keeps one
// copy per epoch, and one recovery package per epoch, of a writer no longer
// current (removed, or demoted by chain v3) — so the readers must try them:
//
// - a recovery package sealed by a device removed or demoted since opens with
//   the words when it is the user key the chain committed for its epoch
//   (`openRecoveryPackage`), and is refused when it is not, when its epoch is
//   not committed, when its sender is no device of the chain, or under other
//   words — as `openFromRecovery` now does too (it took current senders only);
// - a machine-key copy a demoted admin wrote is tried after a current admin's
//   bad one, and taken only as the chain committed it.
import assert from "node:assert/strict";
import test from "node:test";
import {
  fromHex,
  generateDeviceKeys,
  hex,
  openFromRecovery,
  RECOVERY_RECIPIENT,
  recoveryKeys,
  type SealContext,
  sealForRecovery,
  WrapError,
} from "../src/lib/e2e/index.js";
import type { CloudAccount } from "../src/lib/hub/account.js";
import { keyName, openRecoveryPackage, parseKeyName, wrapMachineKey } from "../src/lib/hub/keyring.js";
import { candidateFromOffer, PairingResponder } from "../src/lib/hub/pairing.js";
import { recoverySecret } from "../src/lib/hub/recovery-words.js";
import { createV2Genesis, FakeCourier, FakeKeysHub, RecordingKeyStore, pairBrowser } from "./fake-keys-hub.js";

/** A new browser paired from `a` (QR), as an admin. */
const rawUk = (who: CloudAccount, epoch: number) => who.vault.withRaw(epoch, async (raw) => raw.slice());
const wrapKind = (kind: WrapError["kind"]) => (e: unknown) => e instanceof WrapError && e.kind === kind;

/** Leave only `keep` of the recovery packages for `epoch` (say the others' were never written). */
function onlyPackage(hub: FakeKeysHub, epoch: number, keep: string): void {
  for (const name of [...hub.keys.keys()]) if (name.startsWith(`rk/${epoch}/`) && name !== keep) hub.keys.delete(name);
}

test("v2: a removed device's recovery package is the way back in for its epoch — only as the chain committed it", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  const words = (await createV2Genesis(a, true))!;
  const b = await pairBrowser(hub, a, { admin: true });
  await b.ensureRecoveryPackages("own");
  const rsk = a.chain!.currentRecoveryKey!;
  const tag = hex(rsk.slice(0, 8));
  const bPackage = keyName.written(keyName.recovery(0, b.deviceId, rsk), b.deviceId);
  assert.ok(hub.keys.has(bPackage));
  await a.removeDevice(b.deviceId, new FakeCourier(), false);
  assert.equal(a.chain!.deviceRecord(b.deviceId)?.status.type, "removed");
  assert.ok(hub.keys.has(bPackage), "the hub keeps the removed sender's one package for the epoch");
  onlyPackage(hub, 0, bPackage);

  // Straight through the opener: b's package opens to the committed UK_0 — through the e2e opener too (one rule).
  const rk = await recoveryKeys(await recoverySecret(words.join(" ")));
  const chain = a.chain!;
  const ctx = (sender: string, epoch: number): SealContext => ({ purpose: "userKey", account: a.account, sender, recipient: RECOVERY_RECIPIENT, epoch });
  const genuine = fromHex(hub.keys.get(bPackage)!.blob);
  const uk0 = await rawUk(a, 0);
  assert.equal(hex(await openRecoveryPackage(rk.seal, chain, ctx(b.deviceId, 0), genuine)), hex(uk0));
  assert.equal(hex(await openFromRecovery(rk.seal, chain, ctx(b.deviceId, 0), genuine)), hex(uk0));
  // Refused: a key of b's own choosing, an epoch never committed, a device the chain never named, other words.
  const fake = await sealForRecovery(b.noiseKeys, b.deviceId, b.account, rsk, 0, new Uint8Array(32).fill(0x77));
  await assert.rejects(openRecoveryPackage(rk.seal, chain, ctx(b.deviceId, 0), fake), wrapKind("uncommitted"));
  const ahead = await sealForRecovery(b.noiseKeys, b.deviceId, b.account, rsk, 7, uk0);
  await assert.rejects(openRecoveryPackage(rk.seal, chain, ctx(b.deviceId, 7), ahead), wrapKind("uncommitted"));
  const stranger = await generateDeviceKeys();
  const byStranger = await sealForRecovery({ privateKey: stranger.dh.privateKey, publicKey: stranger.dhPublic }, "d-stranger", a.account, rsk, 0, uk0);
  await assert.rejects(openRecoveryPackage(rk.seal, chain, ctx("d-stranger", 0), byStranger), wrapKind("unknownSender"));
  const other = await recoveryKeys(crypto.getRandomValues(new Uint8Array(16)));
  await assert.rejects(openRecoveryPackage(other.seal, chain, ctx(b.deviceId, 0), genuine), wrapKind("open"));

  // And through recovery on a new browser, b's own bad package planted newest (tried first): epoch 0 from the genuine one.
  hub.keys.set(`rk/0/${b.deviceId}/${tag}/${b.deviceId}.0f0f`, { blob: hex(fake), updated_ms: hub.now() + 10, writer: b.deviceId });
  const z = await hub.device("tok", new RecordingKeyStore());
  const { epochs } = await z.recover(words.join(" "));
  assert.deepEqual(epochs, [0, 1], "epoch 0 from the removed device's package, epoch 1 from the rotation's");
  assert.equal(hex(await rawUk(z, 0)), hex(uk0));
});

test("v3: a demoted admin's package and machine-key copy are tried, and taken only as the chain committed them", async () => {
  const hub = new FakeKeysHub();
  hub.chainV3Mode = "write";
  const a = await hub.device("tok");
  const words = (await createV2Genesis(a, true))!;
  const m = await hub.enroll(a, "m-1");
  await a.handOverMachineKey(m.id, new FakeCourier());
  await a.upgradeToV3({ writable: true });
  const b = await pairBrowser(hub, a, { admin: true });
  assert.equal(b.isAdmin, true);
  assert.deepEqual(await b.vault.epochs(), [0, 1]);
  // b's own package for every epoch, and its own copy of MK_0 under UK_1.
  await b.ensureRecoveryPackages("own");
  const mk0 = (await a.machineKeyBytes(m.id, 0))!;
  const bCopy = keyName.written(keyName.machine(m.id, 0, 1), b.deviceId);
  await b.api.putKey(b.session.token, bCopy, await wrapMachineKey(await b.vault.key(1), b.account, m.id, 0, 1, mk0));
  await a.sync();
  await a.setAdmin(b.deviceId, false);
  assert.ok(a.chain!.device(b.deviceId) && !a.chain!.admin(b.deviceId), "demoted, still a device");
  const rsk = a.chain!.currentRecoveryKey!;
  assert.ok(rsk, "a set the words and stays an admin: they still count");

  // The words, with b's packages the only ones: both epochs open from a sender no longer an admin.
  for (const epoch of [0, 1]) onlyPackage(hub, epoch, keyName.written(keyName.recovery(epoch, b.deviceId, rsk), b.deviceId));
  const z = await hub.device("tok", new RecordingKeyStore());
  const { epochs } = await z.recover(words.join(" "));
  assert.deepEqual(epochs, [0, 1]);
  assert.equal(hex(await rawUk(z, 1)), hex(await rawUk(a, 1)));

  // a's own copies of MK_0 are bad now (a current admin's), b's is good: a reader tries b's after them, and takes it.
  for (const [name, k] of [...hub.keys.entries()]) {
    const p = parseKeyName(name);
    if (p?.kind !== "machine" || p.machine !== m.id || p.mkEpoch !== 0 || p.writer !== a.deviceId) continue;
    const bad = await wrapMachineKey(await a.vault.key(p.ukEpoch), a.account, m.id, 0, p.ukEpoch, crypto.getRandomValues(new Uint8Array(32)));
    hub.keys.set(name, { ...k, blob: hex(bad) });
  }
  await a.keyring.refresh();
  assert.equal(hex((await a.machineKeyBytes(m.id, 0))!), hex(mk0), "from the demoted admin's copy");
  // And a bad copy from it is refused like any other: nothing opens then.
  hub.keys.set(bCopy, { ...hub.keys.get(bCopy)!, blob: hex(await wrapMachineKey(await b.vault.key(1), b.account, m.id, 0, 1, crypto.getRandomValues(new Uint8Array(32)))) });
  await a.keyring.refresh();
  assert.equal(await a.machineKeyBytes(m.id, 0), null);
});
