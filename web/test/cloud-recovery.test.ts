// Recovery words, recovery, removal with key rotation, and following the chain
// (`lib/hub/recovery-words.ts`, `account.ts`, `follow.ts`) against a fake hub.
//
// What these pin: the words screen asks back distinct positions and checks
// them case-blind; a typed phrase says what is wrong (an unknown word, too few
// words, a checksum); recovering opens the recovery packages, joins *pending*,
// and becomes active by another device's approval (which mails it every user
// key) or by itself only after the delay (refused before, with the time);
// removing a device commits a new user key epoch and a new machine key epoch
// in the same call, seals the user key to the remaining full devices only
// (and the recovery key), wraps the machine key under it and hands it to the
// agent; a device that finds itself removed on the chain wipes its keys, and
// one only the hub calls removed keeps them; the follower subscribes, merges,
// attests, takes mail, and reports a removal.
import assert from "node:assert/strict";
import test from "node:test";
import { encodeEntry, entryHash, fromHex, genesisDraft, hex, indexStream, MemoryKeyStore, MnemonicError, sealTo, signDraft, utf8 } from "../src/lib/e2e/index.js";
import { AccountError, type CloudAccount } from "../src/lib/hub/account.js";
import { MAIL_USER_KEY } from "../src/lib/hub/api.js";
import { ChainFollower, type FollowLink } from "../src/lib/hub/follow.js";
import { keyName } from "../src/lib/hub/keyring.js";
import { candidateFromOffer, completePairing, PairingResponder } from "../src/lib/hub/pairing.js";
import { challengeAnswers, challengePassed, checkPhrase, pickChallenge } from "../src/lib/hub/recovery-words.js";
import { browserStorage, createV2Genesis, FakeCourier, FakeKeysHub, RecordingKeyStore, pairBrowser } from "./fake-keys-hub.js";

const DAY = 24 * 60 * 60 * 1000;

/** Pair a new browser into `a`'s account by QR text, as the screens do. */
async function account(withWords = true) {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  const words = await createV2Genesis(a, withWords);
  const machine = await hub.enroll(a); // MK epoch 0 committed, its blob put
  const courier = new FakeCourier();
  await a.handOverMachineKey(machine.id, courier);
  return { hub, a, words, machine, courier };
}

/** The recovery package device `sender` sealed for UK epoch `epoch`, under the account's current recovery key. */
function packaged(hub: FakeKeysHub, a: CloudAccount, epoch: number, sender = a.deviceId): boolean {
  return hub.keys.has(keyName.written(keyName.recovery(epoch, sender, a.chain!.currentRecoveryKey!), sender));
}

/** A password sign-in's login token (it may read the chain, nothing else of a device's). */
async function loginToken(hub: FakeKeysHub): Promise<string> {
  return (await hub.api().login("someone@example.test", "a password")).token;
}

test("the words screen asks back distinct positions and checks them case-blind", () => {
  let seed = 7;
  const random = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
  for (let i = 0; i < 50; i++) {
    const p = pickChallenge(12, 3, random);
    assert.equal(p.length, 3);
    assert.equal(new Set(p).size, 3);
    assert.deepEqual(p, [...p].sort((x, y) => x - y));
    assert.ok(p.every((x) => x >= 0 && x < 12));
  }
  const words = "abandon ability able about above absent absorb abstract absurd abuse access accident".split(" ");
  assert.ok(challengePassed(words, [0, 5, 11], ["Abandon", " absent ", "ACCIDENT"]));
  assert.ok(!challengePassed(words, [0, 5, 11], ["abandon", "absorb", "accident"]));
  assert.deepEqual(challengeAnswers(words, [0, 5, 11], ["abandon", "absorb", ""]), [true, false, false]);
  assert.ok(!challengePassed(words, [], []));
});

test("a typed phrase says what is wrong with it", async () => {
  const { words } = await account();
  const phrase = words!.join(" ");
  assert.deepEqual(await checkPhrase(""), { kind: "empty" });
  assert.deepEqual(await checkPhrase(words!.slice(0, 4).join(" ")), { kind: "incomplete", count: 4 });
  assert.deepEqual(await checkPhrase(`${words!.slice(0, 4).join(" ")} aba`), { kind: "incomplete", count: 4 }, "the word being typed is not judged yet");
  assert.deepEqual(await checkPhrase(`${words!.slice(0, 4).join(" ")} xyzzy `), { kind: "unknownWord", word: "xyzzy", index: 4 });
  assert.deepEqual(await checkPhrase(phrase), { kind: "ok", legacy: false });
  assert.deepEqual(await checkPhrase(phrase.toUpperCase()), { kind: "ok", legacy: false });
  // Twelve real words whose checksum is wrong (sixteen zero bytes end in "about").
  assert.deepEqual(await checkPhrase("abandon ".repeat(12)), { kind: "checksum" });
  assert.deepEqual(await checkPhrase(`${"abandon ".repeat(11)}about`), { kind: "ok", legacy: false });
  assert.deepEqual(await checkPhrase(`${phrase} abandon`), { kind: "wordCount", count: 13 });
  const { base32 } = await import("../src/lib/e2e/index.js");
  const legacy = (base32(crypto.getRandomValues(new Uint8Array(16))).match(/.{1,4}/g) ?? []).join("-");
  assert.deepEqual(await checkPhrase(legacy), { kind: "ok", legacy: true }, "M1's base32 key");
  assert.deepEqual(await checkPhrase(legacy.toLowerCase()), { kind: "ok", legacy: true });
});

test("recover → pending → another device approves → active, with every user key", async () => {
  const { hub, a, words, machine, courier } = await account();
  const c = await hub.device("tok");
  assert.equal(c.standing, "absent");
  await assert.rejects(c.recover("abandon ".repeat(11) + "zoo"), (e: unknown) => e instanceof MnemonicError);
  const other = await (await import("../src/lib/hub/recovery-words.js")).newRecoverySecret();
  await assert.rejects(c.recover(other.words.join(" ")), (e: unknown) => e instanceof AccountError && e.kind === "wrongWords");
  assert.equal(c.standing, "absent", "nothing signed for wrong words");

  const { epochs } = await c.recover(words!.join("  "));
  assert.deepEqual(epochs, [0]);
  assert.equal(c.standing, "pending");
  const ready = c.recoveryReadyAt()!;
  assert.ok(Math.abs(ready - (Date.now() + DAY)) < 60_000);

  await a.sync();
  assert.deepEqual(a.chain!.pendingRecoveries().map((p) => p.id), [c.deviceId]);
  // Meanwhile a rotation (another removal) made epoch 1: approval mails it too.
  const b = await pairBrowser(hub, a, { rawComplete: true });
  await a.removeDevice(b.deviceId, courier);
  assert.deepEqual(await a.vault.epochs(), [0, 1]);
  await a.approveDevice(c.deviceId);
  assert.deepEqual(a.chain!.pendingRecoveries(), []);
  const toC = hub.mail.filter((m) => m.recipient === c.deviceId);
  assert.deepEqual(toC.map((m) => [m.purpose, m.epoch, m.sender]), [[MAIL_USER_KEY, 0, a.deviceId], [MAIL_USER_KEY, 1, a.deviceId]]);

  await c.sync();
  assert.equal(c.standing, "active");
  assert.ok(c.pinned, "the words' key was the chain's own: the recovered device trusts this account");
  assert.equal(await c.receiveMail(), 1, "epoch 1 is new; epoch 0 it had from the package");
  assert.deepEqual(await c.vault.epochs(), [0, 1]);
  await c.keyring.refresh();
  const keys = await c.machineKeys(machine.id);
  assert.deepEqual([...keys.keys()].sort(), [0, 1]);
});

test("a pending device activates itself only after the delay, and asks for the words again", async (t) => {
  const t0 = Date.now();
  t.mock.timers.enable({ apis: ["Date"], now: t0 });
  const { hub, a, words } = await account();
  const c = await hub.device("tok");
  await c.recover(words!.join(" "));
  await a.sync(); // the other device sees the join now: its own clock starts here
  const phrase = words!.join(" ");
  await assert.rejects(c.activateRecovered(phrase), (e: unknown) => e instanceof AccountError && e.kind === "tooEarly" && e.readyMs === c.recoveryReadyAt());
  // Its own clock says yes, the hub's does not yet: refused with the hub's time.
  await assert.rejects(c.activateRecovered(phrase, t0 + DAY + 60_000), (e: unknown) => e instanceof AccountError && e.kind === "tooEarly" && (e.readyMs ?? 0) >= t0 + DAY);
  assert.equal(c.standing, "pending");
  t.mock.timers.setTime(t0 + DAY + 60_000);
  await assert.rejects(c.activateRecovered("abandon ".repeat(11) + "about"), (e: unknown) => e instanceof AccountError && e.kind === "wrongWords");
  await c.activateRecovered(phrase);
  assert.equal(c.standing, "active");
  assert.equal(hub.state!.device(c.deviceId)?.status.type, "active");
  await a.sync();
  assert.equal(a.chain!.device(c.deviceId)?.admin, true, "and the device that watched for a day accepts it");
  // Every device is told, until it says "that was me".
  assert.deepEqual(a.alerts().map((x) => [x.kind, x.subject]), [["recoveryJoin", c.deviceId]]);
});

test("vetoing a pending recovery clears the recovery key; new words replace it and open every epoch", async () => {
  const { hub, a, words, courier } = await account();
  const c = await hub.device("tok");
  await c.recover(words!.join(" "));
  await a.sync();
  const report = await a.removeDevice(c.deviceId, courier);
  assert.equal(a.chain!.currentRecoveryKey, null, "a device that came back with RK took it with it");
  assert.equal(report.recovery, false);
  assert.equal(report.ukEpoch, 1, "it held the user keys: they are rotated");
  const fresh = await a.setRecoveryWords();
  assert.equal(fresh.length, 12);
  assert.ok(packaged(hub, a, 0) && packaged(hub, a, 1));
  const d = await hub.device("tok");
  await assert.rejects(d.recover(words!.join(" ")), (e: unknown) => e instanceof AccountError && e.kind === "wrongWords");
  const { epochs } = await d.recover(fresh.join(" "));
  assert.deepEqual(epochs, [0, 1]);
});

test("removing a device rotates: a new UK only to the remaining full devices, a new MK to the agent", async () => {
  const { hub, a, machine } = await account();
  const b = await pairBrowser(hub, a, { rawComplete: true });
  const c = await pairBrowser(hub, a, { rawComplete: true });
  await assert.rejects(a.removeDevice(a.deviceId, new FakeCourier()), (e: unknown) => e instanceof AccountError && e.kind === "self");
  const courier = new FakeCourier();
  const before = hub.mailLog.length;
  const report = await a.removeDevice(c.deviceId, courier, true);
  assert.equal(report.ukEpoch, 1);
  assert.deepEqual(report.sealedTo, [b.deviceId]);
  assert.equal(report.recovery, true);
  assert.deepEqual(report.machines, [{ machine: machine.id, mkEpoch: 1, result: { via: "channel", changed: true } }]);
  assert.deepEqual(courier.posted.map((p) => [p.machine, p.epoch, p.reencrypt]), [[machine.id, 1, true]]);
  // The removal and both new epochs landed together, in one entry after it.
  const last = a.chain!.suffix(2).map((e) => e.draft.action);
  assert.equal(last[0]!.type, "removeDevice");
  assert.ok(last[1]!.type === "keyEpochs" && last[1].user?.epoch === 1 && last[1].machines[0]?.epoch === 1);
  assert.equal(a.rotationDue, false);
  const sent = hub.mailLog.slice(before);
  assert.deepEqual(sent.map((m) => [m.recipient, m.purpose, m.epoch]), [[b.deviceId, MAIL_USER_KEY, 1]], "nothing to the removed device, nothing to itself");
  assert.ok(packaged(hub, a, 1) && packaged(hub, a, 0), "this device's own package for every epoch it holds");
  const mk11 = `mk/${machine.id}/1/1/${a.deviceId}`;
  assert.ok(hub.keys.has(mk11));
  // Everything wrapped under the new epoch is in the keyring before the epoch is mailed:
  // a device that refreshes its keyring when the mail lands finds it all.
  const mailedAt = hub.requests.indexOf(`mail ${b.deviceId} ${MAIL_USER_KEY} 1`);
  assert.ok(mailedAt > 0);
  const putAt = (name: string) => hub.requests.indexOf(`put ${name}`);
  assert.ok(putAt(mk11) >= 0 && putAt(mk11) < mailedAt, hub.requests.join("\n"));
  const rk1 = keyName.written(keyName.recovery(1, a.deviceId, a.chain!.currentRecoveryKey!), a.deviceId);
  assert.ok(putAt(rk1) >= 0 && putAt(rk1) < mailedAt, hub.requests.join("\n"));

  // The remaining device takes the new epoch and reads the new MK. (Had its
  // keyring been read before the blob was there, a stream stalling on the
  // new epoch makes it look again: `refreshKeys`, and listeners hear it.)
  const blob = hub.keys.get(mk11)!;
  hub.keys.delete(mk11);
  assert.equal(await b.receiveMail(), 1);
  assert.deepEqual([...(await b.machineKeys(machine.id)).keys()], [0]);
  hub.keys.set(mk11, blob);
  let told = 0;
  const off = b.onChange(() => told++);
  await b.refreshKeys();
  off();
  assert.ok(told > 0);
  const keys = await b.machineKeys(machine.id);
  assert.deepEqual([...keys.keys()].sort(), [0, 1]);
  const { importMachineKey } = await import("../src/lib/e2e/index.js");
  assert.equal(hex(await indexStream(keys.get(1)!)), hex(await indexStream(await importMachineKey(fromHex(courier.posted[0]!.key)))));

  // The removed device may no longer mail; a key the hub mails for it is dropped (and acked).
  const forged = await sealTo(c.noiseKeys, b.keys.dhPublic, { purpose: "userKey", account: b.account, sender: c.deviceId, recipient: b.deviceId, epoch: 1 }, utf8("x".repeat(32)));
  await assert.rejects(c.api.mail(c.session.token, { recipient: b.deviceId, sender: c.deviceId, purpose: MAIL_USER_KEY, epoch: 1, sealed: forged }), { status: 401 });
  hub.mail.push({ id: 9999, recipient: b.deviceId, sender: c.deviceId, purpose: MAIL_USER_KEY, epoch: 1, sealed: hex(forged) });
  assert.equal(await b.receiveMail(), 0);
  assert.equal(hub.mail.filter((m) => m.recipient === b.deviceId).length, 0);

  // The removed device: the hub no longer gives it a session, and the chain
  // (read with a password sign-in) shows the removal — then it wipes.
  await assert.rejects(c.sync(), { status: 401 });
  assert.equal(c.refusal, "removed");
  assert.equal(c.standing, "active", "the hub's word alone is not the chain's");
  assert.equal(await c.confirmRemoval(await loginToken(hub)), true);
  assert.equal(c.standing, "removed");
  assert.equal(await c.vault.has(1), false, "it never got the new epoch");
  await c.wipe();
  assert.deepEqual(await c.vault.epochs(), []);
  assert.equal(await c.store.get("device-keys/v1"), undefined);
  assert.equal(await c.store.get("local-wrap/v1"), undefined);
  assert.equal(await c.store.get(`keyring/${c.account}`), undefined);
  assert.equal(await c.store.get(`uk/${c.account}/0`), undefined);
});

test("an entry signed on a stale view is re-signed once on the hub's head", async () => {
  const { hub, a } = await account(false);
  const b = await pairBrowser(hub, a, { rawComplete: true });
  const c = await pairBrowser(hub, a, { rawComplete: true });
  await b.sync();
  await b.removeDevice(c.deviceId, new FakeCourier());
  assert.ok(a.chain!.seq < hub.chain.length - 1, "a has not seen the removal");
  const words = await a.setRecoveryWords();
  assert.equal(words.length, 12);
  assert.equal(a.chain!.seq, hub.chain.length - 1);
  assert.equal(a.chain!.deviceRecord(c.deviceId)?.status.type, "removed");
  assert.ok(a.chain!.currentRecoveryKey);
  // The epoch b made while a looked away was taken from the mail first, and sealed for the new words too.
  assert.deepEqual(await a.vault.epochs(), [0, 1]);
  assert.ok(packaged(hub, a, 0) && packaged(hub, a, 1));
});

test("a device that pinned the account takes the keys it missed when another device re-sends them; one that never pinned does not", async () => {
  const { hub, a, machine, courier } = await account(false);
  const b = await pairBrowser(hub, a, { rawComplete: true });
  const c = await pairBrowser(hub, a, { rawComplete: true });
  // A rotation whose mail to b is lost.
  await a.removeDevice(c.deviceId, courier);
  hub.mail = hub.mail.filter((m) => m.recipient !== b.deviceId);
  const reloaded = await hub.reload(b);
  assert.equal(reloaded.deviceId, b.deviceId);
  assert.ok(reloaded.pinned);
  assert.deepEqual(await reloaded.vault.epochs(), [0], "it missed epoch 1");
  await assert.rejects(a.resendUserKeys(a.deviceId), (e: unknown) => e instanceof AccountError && e.kind === "self");
  assert.deepEqual(await a.resendUserKeys(b.deviceId), [0, 1]);
  assert.equal(await reloaded.receiveMail(), 1);
  assert.deepEqual(await reloaded.vault.epochs(), [0, 1]);
  await reloaded.keyring.refresh();
  assert.deepEqual([...(await reloaded.machineKeys(machine.id)).keys()].sort(), [0, 1]);

  // A browser added without a pairing delivery never pinned the account: mailed keys are not for it.
  const store = new RecordingKeyStore();
  const d = await hub.device("tok", store);
  await a.append({ type: "addDevice", device: { id: d.deviceId, signKey: d.keys.signPublic, dhKey: d.keys.dhPublic, full: true } });
  const again = await hub.device("tok", store);
  assert.equal(again.deviceId, d.deviceId);
  assert.equal(again.standing, "active");
  assert.equal(again.pinned, false);
  assert.equal(await again.lacksKeys(), true, "in the account, holding no key: it pairs again");
  await a.resendUserKeys(d.deviceId);
  assert.equal(await again.receiveMail(), 0);
  assert.deepEqual(await again.vault.epochs(), [], "and it makes up no user key of its own");
});

test("the sole device commits user key 0 itself when the chain has none, and never makes up one it committed", async () => {
  const hub = new FakeKeysHub();
  const store = new MemoryKeyStore();
  const a = await hub.device("tok", store);
  // A genesis that committed no user key.
  const ak = (await crypto.subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"])) as CryptoKeyPair;
  const akPublic = new Uint8Array(await crypto.subtle.exportKey("raw", ak.publicKey));
  const genesis = await signDraft(genesisDraft(a.account, akPublic, { id: a.deviceId, signKey: a.keys.signPublic, dhKey: a.keys.dhPublic, full: true }, null, Date.now()), ak.privateKey);
  await a.api.appendChain(a.session.token, [encodeEntry(genesis)]);
  browserStorage(store).setItem(`rh.hub.genesis.${a.account}`, hex(await entryHash(genesis)));
  const again = await hub.reload(a);
  assert.ok(again.pinned);
  assert.equal(again.chain!.latestUserEpoch(), null);
  assert.equal(await again.lacksKeys(), false, "no other device could hold one: it makes it, committed first");
  assert.deepEqual(await again.vault.epochs(), [0]);
  assert.equal(again.chain!.latestUserEpoch(), 0);
  // Lost: the chain names epoch 0, so nothing new is made up under it.
  await again.vault.wipe();
  assert.equal(await again.lacksKeys(), true);
  assert.deepEqual(await again.vault.epochs(), []);
});

/** A hub link the test drives. */
class FakeLink implements FollowLink {
  state: "open" | "closed" | "connecting" = "open";
  lastClose: number | null = null;
  sent: Record<string, unknown>[] = [];
  attested: number[] = [];
  private stateFns = new Set<(s: "open" | "closed" | "connecting") => void>();
  private controlFns = new Set<(m: Record<string, unknown>) => void>();
  private chainFns = new Set<(e: Uint8Array[]) => void>();
  onState(fn: (s: "open" | "closed" | "connecting") => void) {
    this.stateFns.add(fn);
    return () => this.stateFns.delete(fn);
  }
  onControl(fn: (m: Record<string, unknown>) => void) {
    this.controlFns.add(fn);
    return () => this.controlFns.delete(fn);
  }
  onChain(fn: (e: Uint8Array[]) => void) {
    this.chainFns.add(fn);
    return () => this.chainFns.delete(fn);
  }
  sendControl(m: Record<string, unknown>) {
    this.sent.push(m);
    return true;
  }
  attest(att: { seq: number }) {
    this.attested.push(att.seq);
    return true;
  }
  set(state: "open" | "closed", close: number | null = null) {
    this.state = state;
    this.lastClose = close;
    for (const fn of this.stateFns) fn(state);
  }
  chain(entries: Uint8Array[]) {
    for (const fn of this.chainFns) fn(entries);
  }
  control(m: Record<string, unknown>) {
    for (const fn of this.controlFns) fn(m);
  }
}

/** Wait until `done` holds (the follower works asynchronously), or fail after `ms`. */
async function until(done: () => boolean | Promise<boolean>, ms = 5000): Promise<void> {
  const end = Date.now() + ms;
  while (!(await done())) {
    if (Date.now() > end) throw new Error("timed out waiting");
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}

function startFollower(t: { after: (fn: () => void) => void }, link: FakeLink, account: CloudAccount, removed: string[]): ChainFollower {
  const follower = new ChainFollower({ link: link as unknown as FollowLink, account, onRemoved: () => removed.push(account.deviceId) });
  t.after(() => follower.stop());
  follower.start();
  return follower;
}

test("the follower subscribes from its head, merges pushed entries, attests each change, and takes mail", async (t) => {
  const { hub, a, courier } = await account(false);
  const b = await pairBrowser(hub, a, { rawComplete: true });
  const link = new FakeLink();
  const removed: string[] = [];
  const follower = startFollower(t, link, b, removed);
  await until(() => link.attested.length === 1);
  assert.deepEqual(link.sent, [{ t: "chain_sub", from: b.chain!.seq + 1 }]);
  assert.deepEqual(link.attested, [b.chain!.seq]);

  // `a` removes a third device: the entry is pushed, merged, attested; the new key arrives by mail.
  const c = await pairBrowser(hub, a, { rawComplete: true });
  await a.removeDevice(c.deviceId, courier);
  link.chain(hub.chain.slice(b.chain!.seq + 1).map((e) => fromHex(e)));
  await until(async () => (await b.vault.epochs()).length === 2 && link.attested.at(-1) === hub.chain.length - 1);
  assert.equal(b.chain!.seq, hub.chain.length - 1);
  assert.deepEqual(await b.vault.epochs(), [0, 1], "settled: the mailed user key taken");

  // An entry this device appends itself is attested too (the hub's echo of it changes nothing here).
  await b.setRecoveryWords();
  await until(() => link.attested.at(-1) === b.chain!.seq);

  // A `mail` notice: taken at once.
  link.control({ t: "mail", id: 1 });
  await follower.idle();

  // Reconnecting asks only for what is new.
  link.set("closed", 1012);
  link.set("open");
  await until(() => link.sent.length === 2);
  assert.deepEqual(link.sent.at(-1), { t: "chain_sub", from: b.chain!.seq + 1 });
  await follower.idle();
  assert.deepEqual(removed, []);
});

test("a 4403 close is confirmed against the chain before the device counts as removed", async (t) => {
  const { hub, a, courier } = await account(false);
  const b = await pairBrowser(hub, a, { rawComplete: true });
  const link = new FakeLink();
  const removed: string[] = [];
  const refused: string[] = [];
  const follower = new ChainFollower({ link: link as unknown as FollowLink, account: b, onRemoved: () => removed.push(b.deviceId), onRefused: () => refused.push(b.deviceId) });
  t.after(() => follower.stop());
  follower.start();
  await follower.idle();
  // A 4403 the chain does not back up, from a hub that still serves this device: ignored.
  link.set("closed", 4403);
  await follower.idle();
  assert.deepEqual([removed, refused], [[], []]);
  // Removed for real: the hub revokes its sessions, so it cannot read the chain — the page stops, the keys stay.
  await a.removeDevice(b.deviceId, courier);
  link.set("open");
  link.set("closed", 4403);
  await until(() => refused.length === 1);
  assert.deepEqual(removed, []);
  assert.equal(b.refusal, "removed");
  assert.deepEqual(await b.vault.epochs(), [0], "the hub's word alone wipes nothing");
  // Signing in again reads the chain: the removal is real.
  const back = await hub.device("tok", b.store);
  assert.equal(back.deviceId, b.deviceId);
  assert.equal(back.refusal, "removed");
  assert.equal(back.standing, "removed");
});

test("a hub that calls a device removed when the chain does not is not believed", async () => {
  const { hub, a } = await account(false);
  const b = await pairBrowser(hub, a, { rawComplete: true });
  hub.claimRemoved.add(b.deviceId);
  hub.sessions.clear();
  const back = await hub.device("tok", b.store);
  assert.equal(back.refusal, "removed");
  assert.equal(back.standing, "active", "the chain read with the login token still lists it");
  assert.deepEqual(await back.vault.epochs(), [0], "nothing wiped");
});

test("a removal pushed over the link is noticed at once", async (t) => {
  const { hub, a, courier } = await account(false);
  const b = await pairBrowser(hub, a, { rawComplete: true });
  const link = new FakeLink();
  const removed: string[] = [];
  const follower = startFollower(t, link, b, removed);
  await follower.idle();
  await a.removeDevice(b.deviceId, courier);
  link.chain(hub.chain.slice(b.chain!.seq + 1).map((e) => fromHex(e)));
  await until(() => removed.length === 1);
  assert.deepEqual(removed, [b.deviceId]);
});
