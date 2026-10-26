// Pairing a second device (`lib/hub/pairing.ts`), both ends against a fake hub.
//
// What these pin: the QR / pasted text parses back into exactly the offered
// keys (and is refused for another account, or a device already in the
// chain); the existing device signs "add device" for those keys and mails a
// delivery the new device accepts only with its secret, against a chain that
// verifies and lists it — "paired" only then, with the account's user keys;
// a numeric code runs CPace once (a second hello is ignored), a wrong code
// aborts on both ends, and a code is never reused; only the new device closes
// its pairing; a device already in the chain that lacks its keys pairs again
// (a new secret, no second add), and one that never pinned the account takes
// no mailed keys at all.
import assert from "node:assert/strict";
import test from "node:test";
import { createDelivery, encodeDelivery, fromHex, hex, offerFromText, offerText, PairingError } from "../src/lib/e2e/index.js";
import type { CloudAccount } from "../src/lib/hub/account.js";
import { MAIL_PAIRING } from "../src/lib/hub/api.js";
import {
  abandonCandidate,
  candidateFromOffer,
  completePairing,
  type CompleteDeps,
  PairingResponder,
  type ResponderDeps,
  type ResponderState,
  runCodeInitiator,
} from "../src/lib/hub/pairing.js";
import { createV2Genesis, FakeKeysHub, RecordingKeyStore } from "./fake-keys-hub.js";

const made: PairingResponder[] = [];
test.afterEach(() => {
  for (const r of made.splice(0)) r.stop();
});

function responderFor(b: CloudAccount, extra: Partial<ResponderDeps> = {}): PairingResponder {
  return remember(new PairingResponder({
    ...extra,
    api: b.api,
    token: b.session.token,
    account: b.account,
    device: b.deviceId,
    signPublic: b.keys.signPublic,
    noise: b.noiseKeys,
    chain: async () => {
      await b.sync();
      return b.chain;
    },
    storeUserKeys: (keys) => b.storeUserKeys(keys),
    pollMs: 1,
    chainRetries: 1,
  }));
}

function remember(r: PairingResponder): PairingResponder {
  made.push(r);
  return r;
}

function completeDeps(a: CloudAccount): CompleteDeps {
  return {
    api: a.api,
    token: a.session.token,
    device: a.deviceId,
    signPublic: a.keys.signPublic,
    noise: a.noiseKeys,
    chain: () => a.chain!,
    append: (action) => a.append(action),
    withUserKeys: (use) => a.withUserKeys(use),
  };
}

/** Run the responder's rounds by hand until `done` (no timers). */
async function until(r: PairingResponder, done: (s: ResponderState) => boolean, rounds = 20): Promise<ResponderState> {
  for (let i = 0; i < rounds && !done(r.state); i++) await r.tick();
  return r.state;
}

/** Wait for the responder's fire-and-forget hub write (a close) to land. */
async function settles(check: () => boolean, ms = 2_000): Promise<void> {
  const end = Date.now() + ms;
  while (!check() && Date.now() < end) await new Promise((resolve) => setTimeout(resolve, 2));
}

async function twoDevices() {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  await hub.enroll(a); // MK epoch 0, committed, its blob in the keyring
  const b = await hub.device("tok");
  assert.equal(b.standing, "absent", "signed in and registered, not in the chain");
  return { hub, a, b };
}

test("QR: the offer's text parses back to the same keys, and only for this account", async () => {
  const { a, b } = await twoDevices();
  const r = responderFor(b);
  const text = await r.startOffer();
  r.stop();
  assert.match(text, /^RHP1-[A-Z2-7]+$/);
  const offer = offerFromText(text.toLowerCase().replace(/(.{8})/g, "$1 "));
  assert.equal(offer.device, b.deviceId);
  assert.equal(hex(offer.signKey), hex(b.keys.signPublic));
  assert.equal(hex(offer.dhKey), hex(b.keys.dhPublic));
  const c = await candidateFromOffer(text, a.chain!);
  assert.equal(c.device, b.deviceId);
  assert.match(c.fingerprint, /^[A-Z2-7]{4}(-[A-Z2-7]{4}){3}$/);
  assert.equal(r.state.kind, "offer");
  assert.equal((r.state as { fingerprint: string }).fingerprint, c.fingerprint, "both screens show the same fingerprint");
  // The same offer with the newer DRP1- prefix, any case, pasted with spaces: the same candidate.
  const dr = text.replace(/^RHP1-/, "DRP1-");
  assert.deepEqual(offerFromText(dr.toLowerCase().replace(/(.{8})/g, "$1 ")), offer);
  assert.deepEqual(await candidateFromOffer(dr, a.chain!), c);
  for (const other of ["RH1-", "DR1-", "RHR1-", "DRR1-", "DRP2-"]) {
    await assert.rejects(candidateFromOffer(text.replace(/^RHP1-/, other), a.chain!), { name: "PairingError" }, other);
  }

  await assert.rejects(candidateFromOffer("RHP1-AAAA", a.chain!), { name: "PairingError" });
  await assert.rejects(candidateFromOffer("DRP1-AAAA", a.chain!), { name: "PairingError" });
  await assert.rejects(candidateFromOffer("hello", a.chain!), { name: "PairingError" });
  const other = offerText({ ...offer, account: "a-other" });
  await assert.rejects(candidateFromOffer(other, a.chain!), (e: unknown) => e instanceof PairingError && e.kind === "wrongAccount");
  const self = offerText({ ...offer, device: a.deviceId });
  await assert.rejects(candidateFromOffer(self, a.chain!), (e: unknown) => e instanceof PairingError && e.kind === "badOffer");
});

test("QR: add device, deliver every user key, and the new device is paired only once the chain lists it", async () => {
  const { hub, a, b } = await twoDevices();
  const r = responderFor(b);
  const text = await r.startOffer();
  r.stop(); // drive it by hand
  const c = await candidateFromOffer(text, a.chain!);
  await completePairing(completeDeps(a), c);
  const last = a.chain!.suffix(1)[0]!.draft.action;
  assert.equal(last.type, "addDevice");
  assert.ok(last.type === "addDevice" && last.device.id === b.deviceId && last.device.full && hex(last.device.dhKey) === hex(b.keys.dhPublic));
  const mail = hub.mail.filter((m) => m.recipient === b.deviceId);
  assert.deepEqual(mail.map((m) => [m.purpose, m.epoch, m.sender]), [[MAIL_PAIRING, 0, a.deviceId]]);

  const fresh = responderFor(b);
  (fresh as unknown as { secrets: Uint8Array[] }).secrets = (r as unknown as { secrets: Uint8Array[] }).secrets;
  // `stop()` zeroed the first responder's secret: a fresh run must not accept it.
  const state = await until(fresh, (s) => s.kind === "paired" || s.kind === "failed", 3);
  assert.equal(state.kind, "idle", "a delivery its secret does not check is dropped, not trusted");
  assert.equal(hub.mail.filter((m) => m.recipient === b.deviceId).length, 0, "and acked");
});

test("QR end to end: paired, the user keys open the machine's keyring blobs, the mail is acked", async () => {
  const { hub, a, b } = await twoDevices();
  const r = responderFor(b);
  const text = await r.startOffer();
  const running = r as unknown as { stopped: boolean };
  running.stopped = true; // no timer loop; ticks by hand
  running.stopped = false;
  const c = await candidateFromOffer(text, a.chain!);
  await completePairing(completeDeps(a), c);
  const state = await until(r, (s) => s.kind === "paired" || s.kind === "failed");
  r.stop();
  assert.deepEqual(state, { kind: "paired", epochs: [0] });
  assert.equal(b.standing, "active");
  assert.deepEqual(await b.vault.epochs(), [0]);
  await b.keyring.refresh();
  const keys = await b.machineKeys("m-1");
  const mine = await a.machineKeys("m-1");
  assert.deepEqual([...keys.keys()], [0]);
  const { indexStream } = await import("../src/lib/e2e/index.js");
  assert.equal(hex(await indexStream(keys.get(0)!)), hex(await indexStream(mine.get(0)!)), "the same MK on both devices");
  assert.equal(hub.mail.filter((m) => m.recipient === b.deviceId).length, 0);
});

test("a delivery MAC'd correctly but for a chain that does not list the new device is refused", async () => {
  const { hub, a, b } = await twoDevices();
  const r = responderFor(b);
  const text = await r.startOffer();
  const offer = offerFromText(text);
  // Someone who saw the code mails a delivery but never signs "add device".
  const d = await a.vault.withAllRaw((keys) => createDelivery(a.chain!, a.deviceId, a.keys.signPublic, a.noiseKeys, b.deviceId, b.keys.dhPublic, keys, offer.secret));
  await a.api.mail(a.session.token, { recipient: b.deviceId, sender: a.deviceId, purpose: MAIL_PAIRING, epoch: 0, sealed: encodeDelivery(d) });
  const state = await until(r, (s) => s.kind === "paired" || s.kind === "failed");
  r.stop();
  assert.equal(state.kind, "failed");
  assert.equal((state as { reason: string }).reason, "badDelivery");
  assert.deepEqual(await b.vault.epochs(), [], "no key kept");
  assert.equal(hub.mail.filter((m) => m.recipient === b.deviceId).length, 0);
});

test("numeric code: one CPace run, the answer checked, paired", async () => {
  const { hub, a, b } = await twoDevices();
  const r = responderFor(b);
  const code = await r.startCode();
  assert.match(code, /^\d{8}$/);
  // One code per screen: asking again (back from the QR tab) shows the same one, and opens nothing.
  assert.equal(await r.startCode(), code, "one code per screen");
  assert.equal(hub.pairings.size, 1);
  const [pairing] = await a.api.pairings(a.session.token);
  assert.equal(pairing!.device, b.deviceId);
  // The initiator posts its hello and polls for the answer; the responder answers in between.
  const answering = (async () => {
    await until(r, (s) => s.kind === "answered" || s.kind === "failed");
  })();
  const c = await runCodeInitiator({ api: a.api, token: a.session.token, account: a.account, pollMs: 1 }, pairing!, code);
  await answering;
  assert.equal(c.device, b.deviceId);
  assert.equal(hex(c.dhKey), hex(b.keys.dhPublic));
  assert.equal(hub.pairings.size, 1, "the initiator leaves the pairing to the device that opened it");
  await completePairing(completeDeps(a), c);
  const state = await until(r, (s) => s.kind === "paired" || s.kind === "failed");
  r.stop();
  assert.deepEqual(state, { kind: "paired", epochs: [0] });
  assert.equal(b.standing, "active");
  assert.ok(b.pinned, "the delivery vouched for the account");
  assert.equal(hub.pairings.size, 0, "the new device closed its pairing once its delivery arrived");
});

test("numeric code: a wrong code aborts on both ends, and the code is spent", async () => {
  const { hub, a, b } = await twoDevices();
  let clock = Date.now();
  const r = responderFor(b, { now: () => clock });
  const code = await r.startCode();
  const wrong = String((Number(code) + 1) % 100_000_000).padStart(8, "0");
  const [pairing] = await a.api.pairings(a.session.token);
  const answering = until(r, (s) => s.kind === "answered" || s.kind === "failed");
  await assert.rejects(
    runCodeInitiator({ api: a.api, token: a.session.token, account: a.account, pollMs: 1 }, pairing!, wrong),
    (e: unknown) => e instanceof PairingError && e.kind === "badAnswer",
  );
  await answering;
  assert.equal(hub.pairings.size, 1, "the initiator does not close it");
  assert.equal((await until(r, (s) => s.kind === "failed", 3)).kind, "answered", "the new device waits for a delivery that never comes…");
  clock += 10 * 60 * 1000 + 1;
  const state = await until(r, (s) => s.kind === "failed");
  assert.equal(state.kind, "failed");
  assert.equal((state as { reason: string }).reason, "codeFailed");
  await settles(() => hub.pairings.size === 0);
  assert.equal(hub.pairings.size, 0, "…then gives up and closes it");
  assert.equal(a.chain!.deviceRecord(b.deviceId), undefined, "nothing signed");
  assert.equal(hub.mail.filter((m) => m.recipient === b.deviceId).length, 0);
  // Start again: a new responder, a new code, a new pairing.
  const again = responderFor(b);
  const next = await again.startCode();
  again.stop();
  assert.notEqual(next, code);
});

test("numeric code: the new device answers only the first hello (one run per code)", async () => {
  const { hub, a, b } = await twoDevices();
  const r = responderFor(b);
  const code = await r.startCode();
  const [pairing] = await a.api.pairings(a.session.token);
  const { Cpace, channelId, encodePakeHello, newSid, utf8 } = await import("../src/lib/e2e/index.js");
  for (const guess of ["00000000", code]) {
    const run = await Cpace.start("initiator", utf8(guess), channelId(a.account, b.deviceId), newSid());
    await a.api.postPairingMessage(a.session.token, pairing!.id, "initiator", encodePakeHello({ sid: newSid(), share: run.share }));
  }
  await until(r, (s) => s.kind === "answered" || s.kind === "failed");
  r.stop();
  const answers = hub.pairings.get(pairing!.id)?.messages.filter((m) => m.from === "responder") ?? [];
  assert.ok(answers.length <= 1, "at most one answer for one code");
});

test("a code answered but never confirmed: the new device gives up and closes the pairing", async () => {
  const { hub, a, b } = await twoDevices();
  let clock = Date.now();
  const r = responderFor(b, { now: () => clock });
  const code = await r.startCode();
  const [pairing] = await a.api.pairings(a.session.token);
  const answering = until(r, (s) => s.kind === "answered");
  const c = await runCodeInitiator({ api: a.api, token: a.session.token, account: a.account, pollMs: 1 }, pairing!, code);
  await answering;
  abandonCandidate(c);
  assert.ok(c.secret.every((x) => x === 0), "its secret forgotten");
  // Another device outside the chain cannot close it (its opener, or any
  // current full device, can: M2 re-review).
  const other = await hub.device("tok", new RecordingKeyStore());
  await other.api.closePairing(other.session.token, pairing!.id);
  assert.equal(hub.pairings.size, 1, "still open");
  clock += 10 * 60 * 1000 + 1;
  const state = await until(r, (s) => s.kind === "failed");
  assert.equal((state as { reason: string }).reason, "codeFailed");
  await settles(() => hub.pairings.size === 0);
  assert.equal(hub.pairings.size, 0);
});

test("the hub's push of the new entry landing before the append's answer does not break adding", async () => {
  const { hub, a, b } = await twoDevices();
  const r = responderFor(b);
  const text = await r.startOffer();
  // As the follower does: the hub pushes the entry to the page's chain subscription before it answers the POST.
  hub.afterAppend = async (entries) => {
    await a.absorb(entries.map((e) => fromHex(e)));
  };
  const c = await candidateFromOffer(text, a.chain!);
  await completePairing(completeDeps(a), c);
  hub.afterAppend = null;
  assert.equal(a.chain!.seq, hub.chain.length - 1);
  assert.equal(a.chain!.device(b.deviceId)?.admin, true);
  const state = await until(r, (s) => s.kind === "paired" || s.kind === "failed");
  assert.equal(state.kind, "paired");
});

test("an add whose delivery failed is retried by pairing again: the same keys, no second add, the new secret", async () => {
  const { hub, a, b } = await twoDevices();
  const r = responderFor(b);
  const text = await r.startOffer();
  const c = await candidateFromOffer(text, a.chain!);
  hub.failMail = 1;
  await assert.rejects(completePairing(completeDeps(a), c), { status: 503 }, "a delivery that did not go out is said so");
  const seq = hub.chain.length;
  assert.equal(a.chain!.device(b.deviceId)?.admin, true, "added all the same");
  assert.equal(hub.mail.filter((m) => m.recipient === b.deviceId).length, 0);

  // Its keys sent as ordinary mail are not taken: this browser never pinned the account.
  await a.resendUserKeys(b.deviceId);
  await b.sync();
  assert.equal(b.standing, "active");
  assert.equal(b.pinned, false);
  assert.equal(await b.receiveMail(), 0);
  assert.deepEqual(await b.vault.epochs(), [], "no mailed key taken by a device that never pinned the account");
  assert.equal(await b.lacksKeys(), true);

  // It pairs again: a new screen, a new secret. The existing device sees it
  // listed with these very keys, adds nothing, and delivers again.
  r.stop();
  const again = responderFor(b);
  const next = await candidateFromOffer(await again.startOffer(), a.chain!);
  assert.equal(await completePairing(completeDeps(a), next), "redelivered");
  assert.equal(hub.chain.length, seq, "no second add");
  assert.deepEqual(await until(again, (s) => s.kind === "paired" || s.kind === "failed"), { kind: "paired", epochs: [0] });
  assert.ok(b.pinned);
  assert.equal(await b.receiveMail(), 0, "pinned now, it takes the mail that waited (nothing new) and clears it");
  // The old screen's text is spent: its secret is gone with that screen.
  const stale = await candidateFromOffer(text, a.chain!);
  assert.equal(await completePairing(completeDeps(a), stale), "redelivered");
  assert.deepEqual(hub.mail.filter((m) => m.recipient === b.deviceId).map((m) => m.purpose), [MAIL_PAIRING], "left for a screen that is not there");

  // Another device's keys under that id do not pass.
  const offer = offerFromText(text);
  await assert.rejects(candidateFromOffer(offerText({ ...offer, dhKey: a.keys.dhPublic }), a.chain!), (e: unknown) => e instanceof PairingError && e.kind === "badOffer");
});
