// The M2 security review's account-level findings, as regressions against a
// fake hub (`lib/hub/account.ts`, `keyring.ts`, `deliver.ts`, `pairing.ts`).
// The review's proofs of concept (`poc-m2.test.ts`) each attacked successfully
// before; ported here, each attack must now fail:
//
// - PoC 1 (C1): a removed device — with the hub, since it can no longer write
//   the keyring itself — plants `mk/<m>/1/0`, and the pending re-encryption
//   would hand ITS key to the agent.
// - PoC 2 (C1): one garbage keyring name (`rk/4294967295/x`) blocked every
//   rotation after the removal was already signed.
// - PoC 3 (H3): a removed device plants a recovery package, and a later
//   recovery adopted its user key.
//
// Then the fixes themselves: recovery never pins a hub-made chain (H2); a
// browser that never pinned the account takes no keys; a join and its
// activation delivered together at a page load are refused (M2); alerts for
// every device change this device did not make (M1); "rotate keys again";
// the agent's key epoch checked after a channel opens; device sessions minted
// by the device key and renewed once; the mailbox and keyring caps surfaced.
import assert from "node:assert/strict";
import test from "node:test";
import {
  ChainClock,
  ChainError,
  ChainState,
  type DeviceInfo,
  decodeEntry,
  encodeEntry,
  entryHash,
  generateDeviceKeys,
  fromHex,
  genesisDraft,
  hex,
  machineKeyCommit,
  MemoryKeyStore,
  recoveryKeyFor,
  recoveryKeys,
  sealForRecovery,
  sealTo,
  signDraft,
  type Draft,
  type Entry,
} from "../src/lib/e2e/index.js";
import { AccountError, type AccountLocks, type AccountStorage, CloudAccount } from "../src/lib/hub/account.js";
import { hubErrorKind, MAIL_USER_KEY } from "../src/lib/hub/api.js";
import { checkAgentKey, flushPendingReencrypts, notePendingReencrypt, pendingReencrypts } from "../src/lib/hub/deliver.js";
import { keyName, wrapMachineKey } from "../src/lib/hub/keyring.js";
import { candidateFromOffer, completePairing, PairingResponder } from "../src/lib/hub/pairing.js";
import { recoverySecret } from "../src/lib/hub/recovery-words.js";
import { localStorageSeenStore, type MemorySeenStore, type SeenStore, SeenStoreError } from "../src/lib/hub/seen-store.js";
import { browserSeen, browserStorage, createV2Genesis, FakeCourier, FakeKeysHub, HUB_ORIGIN, RecordingKeyStore } from "./fake-keys-hub.js";

const DAY = 24 * 60 * 60 * 1000;

async function pair(hub: FakeKeysHub, a: CloudAccount): Promise<CloudAccount> {
  const b = await hub.device("tok", new RecordingKeyStore());
  const r = new PairingResponder({
    api: b.api,
    token: b.session.token,
    account: b.account,
    device: b.deviceId,
    signPublic: b.keys.signPublic,
    noise: b.noiseKeys,
    chain: async () => (await b.sync(), b.chain),
    storeUserKeys: (keys) => b.storeUserKeys(keys),
    pollMs: 1,
  });
  try {
    const c = await candidateFromOffer(await r.startOffer(), a.chain!);
    await completePairing(
      {
        api: a.api,
        token: a.session.token,
        device: a.deviceId,
        signPublic: a.keys.signPublic,
        noise: a.noiseKeys,
        chain: () => a.chain!,
        append: (action) => a.append(action),
        withUserKeys: (use) => a.withUserKeys(use),
      },
      c,
    );
    for (let i = 0; i < 10 && r.state.kind !== "paired"; i++) await r.tick();
  } finally {
    r.stop();
  }
  assert.equal(r.state.kind, "paired");
  return b;
}

/** Postgres lists the keyring `ORDER BY name` (rh-hub db.rs key_wraps). */
function sortByName(hub: FakeKeysHub) {
  hub.keys = new Map([...hub.keys.entries()].sort(([x], [y]) => (x < y ? -1 : x > y ? 1 : 0)));
}

async function setup() {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  const words = (await createV2Genesis(a, true))!;
  const machine = await hub.enroll(a);
  await a.handOverMachineKey(machine.id, new FakeCourier());
  const b = await pair(hub, a);
  return { hub, a, b, words, machine };
}

test("PoC 1 now fails: a key planted under a pending re-encryption's epoch never reaches the agent", async () => {
  const { hub, a, b, machine } = await setup();
  // Stolen b is removed with re-encryption while the machine is offline: MK_1 goes by mailbox, re-encrypt noted.
  const courier = new FakeCourier();
  courier.online = false;
  const report = await a.removeDevice(b.deviceId, courier, true);
  assert.equal(report.machines[0]!.mkEpoch, 1);
  const committed = courier.mailed[0]!.key;
  assert.deepEqual(pendingReencrypts(a.account), [
    { machine: machine.id, mkEpoch: 1, ukEpoch: 1, mkHash: hex(await machineKeyCommit(a.account, machine.id, 1, Buffer.from(committed, "hex"))) },
  ]);
  // b (removed) still holds UK_0. It can no longer write the keyring itself…
  const attackerMk = new Uint8Array(32).fill(0x66);
  const blob = await wrapMachineKey(await b.vault.key(0), b.account, machine.id, 1, 0, attackerMk);
  await assert.rejects(b.api.putKey(b.session.token, `mk/${machine.id}/1/0`, blob), { status: 401 });
  // …so the hub plants it for it, sorted to come first.
  hub.keys.set(`mk/${machine.id}/1/0`, { blob: hex(blob), updated_ms: hub.now() });
  sortByName(hub);
  // a's page reloads (settle refreshes the keyring), the machine comes back, the channel opens: flush.
  await a.keyring.refresh();
  const posted: { epoch: number; key: string; reencrypt: boolean }[] = [];
  await flushPendingReencrypts(a.account, machine.id, (item) => a.pinnedMachineKey(item), async (body) => (posted.push(body), { status: 200, body: { changed: true } }));
  assert.equal(posted.length, 1);
  assert.equal(posted[0]!.reencrypt, true);
  assert.equal(posted[0]!.key, committed, "the agent is told to re-encrypt under the committed key only");
  assert.notEqual(posted[0]!.key, hex(attackerMk));
  // Nor does the page read streams with it: epoch 1 opens to the committed key, from the blob that verifies.
  assert.equal(hex((await a.machineKeyBytes(machine.id, 1))!), committed);
  // And a note pinned to a key that is not the committed one is never posted.
  notePendingReencrypt(a.account, { machine: machine.id, mkEpoch: 1, ukEpoch: 0, mkHash: hex(new Uint8Array(32)) });
  const more: unknown[] = [];
  await flushPendingReencrypts(a.account, machine.id, (item) => a.pinnedMachineKey(item), async (body) => (more.push(body), { status: 200, body: null }));
  assert.deepEqual(more, []);
});

test("PoC 2 now fails: a garbage keyring name numbers nothing, and the removal still rotates the keys", async () => {
  const { hub, a, b } = await setup();
  const c = b; // the device to remove
  // A full device no longer can plant a name with the largest epoch (M2
  // re-review #27: names end with their writer and name committed epochs)…
  await assert.rejects(c.api.putKey(c.session.token, "rk/4294967295/x", new Uint8Array(16)), { status: 403 });
  await assert.rejects(c.api.putKey(c.session.token, `mk/m-1/4294967295/4294967295/${c.deviceId}`, new Uint8Array(16)), { status: 403 });
  // …but the hub can.
  hub.keys.set("rk/4294967295/x", { blob: hex(new Uint8Array(16)), updated_ms: 0 });
  hub.keys.set("mk/m-1/4294967295/4294967295", { blob: hex(new Uint8Array(16)), updated_ms: 0 });
  const report = await a.removeDevice(c.deviceId, new FakeCourier(), true);
  assert.equal(a.chain!.deviceRecord(c.deviceId)?.status.type, "removed", "removed in the chain…");
  assert.equal(report.ukEpoch, 1);
  assert.deepEqual(await a.vault.epochs(), [0, 1], "…and a new UK epoch: c's keys are retired");
  assert.equal(a.chain!.latestUserEpoch(), 1);
  assert.equal(report.machines[0]!.mkEpoch, 1);
  assert.equal(a.rotationDue, false);
});

test("PoC 3 now fails: a recovery package planted by a removed device is not adopted", async () => {
  const { hub, a, b, words } = await setup();
  await a.removeDevice(b.deviceId, new FakeCourier(), false); // UK epoch 1 made, sealed to RK by a
  const fakeUk = new Uint8Array(32).fill(0x77);
  const rsk = a.chain!.currentRecoveryKey!;
  assert.ok(rsk, "RSK survives: b neither set it nor recovered with it");
  const sealed = await sealForRecovery(b.noiseKeys, b.deviceId, b.account, rsk, 1, fakeUk);
  await assert.rejects(b.api.putKey(b.session.token, `rk/1/${b.deviceId}`, sealed), { status: 401 });
  // The hub plants it, and a second one sealed by a current device with a key the chain never committed.
  hub.keys.set(`rk/1/${b.deviceId}`, { blob: hex(sealed), updated_ms: hub.now() + 10 });
  hub.keys.set(`rk/1/${b.deviceId}/${hex(rsk.slice(0, 8))}`, { blob: hex(sealed), updated_ms: hub.now() + 10 });
  // The user lost a; recovers with the words on a new browser.
  const z = await hub.device("tok", new RecordingKeyStore());
  await z.recover(words.join(" "));
  const got = await z.vault.withRaw(1, async (raw) => hex(raw));
  const real = await a.vault.withRaw(1, async (raw) => hex(raw));
  assert.equal(got, real, "the recovered device's UK epoch 1 is the account's, not the removed device's choice");
  assert.notEqual(got, hex(fakeUk));
});

/** A hub of its own, serving `entries` (made up) as the account's chain. */
function evilHub(entries: Entry[]): FakeKeysHub {
  const evil = new FakeKeysHub();
  evil.chain = entries.map((e) => hex(encodeEntry(e)));
  return evil;
}

async function madeUpGenesis(account: string, device: DeviceInfo, recoveryKey: { key: Uint8Array; proof: Uint8Array } | null): Promise<Entry> {
  const ak = (await crypto.subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"])) as CryptoKeyPair;
  const akPublic = new Uint8Array(await crypto.subtle.exportKey("raw", ak.publicKey));
  return signDraft(genesisDraft(account, akPublic, device, recoveryKey, Date.now()), ak.privateKey);
}

test("H2: recovery against a hub-made chain is refused, and nothing is pinned", async () => {
  const { hub, words } = await setup();
  const real = (await import("../src/lib/e2e/index.js")).decodeEntry(Buffer.from(hub.chain[0]!, "hex"));
  assert.ok(real.draft.action.type === "genesis" && real.draft.action.recoveryKey);
  const realRecovery = real.draft.action.recoveryKey;
  const hubDevice = await generateDeviceKeys();
  const own: DeviceInfo = { id: "d-hub", signKey: hubDevice.signPublic, dhKey: hubDevice.dhPublic, full: true };
  const rk = await recoverySecret(words.join(" "));
  const theirs = await recoveryKeys(crypto.getRandomValues(new Uint8Array(16)));

  const cases: [string, () => Promise<Entry>, (e: unknown) => boolean][] = [
    // The account's real RSK and its proof, copied around the hub's own root key: the proof does not hold.
    ["the real RSK, its proof copied", () => madeUpGenesis(hub.account, own, realRecovery), (e) => e instanceof ChainError && e.kind === "notAllowed"],
    // The real RSK with a "proof" the hub made with its own key.
    [
      "the real RSK, a forged proof",
      async () => madeUpGenesis(hub.account, own, { key: realRecovery.key, proof: (await recoveryKeyFor(theirs, hub.account, own.signKey)).proof }),
      (e) => e instanceof ChainError && e.kind === "notAllowed",
    ],
    // A recovery key of the hub's own, properly proved: the words are not its.
    [
      "the hub's own RSK",
      async () => {
        const ak = (await crypto.subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"])) as CryptoKeyPair;
        const akPublic = new Uint8Array(await crypto.subtle.exportKey("raw", ak.publicKey));
        return signDraft(genesisDraft(hub.account, akPublic, own, await recoveryKeyFor(theirs, hub.account, akPublic), Date.now()), ak.privateKey);
      },
      (e) => e instanceof AccountError && e.kind === "wrongWords",
    ],
    // No recovery key at all.
    ["no recovery key", () => madeUpGenesis(hub.account, own, null), (e) => e instanceof AccountError && e.kind === "noRecoveryKey"],
  ];
  rk.fill(0);
  for (const [what, make, expected] of cases) {
    const evil = evilHub([await make()]);
    const store = new RecordingKeyStore();
    // The made-up chain may not even verify when the page opens: refused there, or when the words are typed.
    let z: CloudAccount | null = null;
    try {
      z = await evil.device("tok", store);
    } catch (error) {
      assert.ok(expected(error), `${what}: ${String(error)}`);
    }
    if (z) await assert.rejects(z.recover(words.join(" ")), expected, what);
    assert.equal(z?.pinned ?? false, false, what);
    assert.ok(![...browserStorage(store).values.keys()].some((k) => k.startsWith("rh.hub.genesis.")), `${what}: no genesis pinned`);
    assert.ok(![...browserSeen(store).values.values()].some((v) => typeof (v as { genesis?: unknown }).genesis === "string"), `${what}: no genesis pinned in the store`);
    assert.equal(await store.get(`uk-epochs/${evil.account}`), undefined, `${what}: no key adopted`);
  }
});

test("H2: a hub-made chain that lists this browser as active gives it no keys", async () => {
  const fresh = new FakeKeysHub();
  const store = new RecordingKeyStore();
  const z = await fresh.device("tok", store);
  // The hub knows z's public keys: it makes a chain with its own device, which "adds" z and mails it a key.
  const hubDevice = await generateDeviceKeys();
  const own: DeviceInfo = { id: "d-hub", signKey: hubDevice.signPublic, dhKey: hubDevice.dhPublic, full: true };
  const genesis = await madeUpGenesis(fresh.account, own, null);
  const add: Draft = {
    version: 2,
    account: fresh.account,
    seq: 1,
    prev: await entryHash(genesis),
    createdMs: Date.now(),
    signer: { type: "device", id: own.id },
    action: { type: "addDevice", device: { id: z.deviceId, signKey: z.keys.signPublic, dhKey: z.keys.dhPublic, full: true } },
  };
  fresh.chain = [genesis, await signDraft(add, hubDevice.sign.privateKey)].map((e) => hex(encodeEntry(e)));
  const planted = new Uint8Array(32).fill(0x55);
  const sealed = await sealTo({ privateKey: hubDevice.dh.privateKey, publicKey: hubDevice.dhPublic }, z.keys.dhPublic, { purpose: "userKey", account: fresh.account, sender: own.id, recipient: z.deviceId, epoch: 0 }, planted);
  fresh.mail.push({ id: 1, recipient: z.deviceId, sender: own.id, purpose: MAIL_USER_KEY, epoch: 0, sealed: hex(sealed) });
  const again = await fresh.reload(z);
  assert.equal(again.standing, "active", "the made-up chain lists it");
  assert.equal(again.pinned, false);
  assert.equal(await again.receiveMail(), 0);
  await again.settle();
  assert.deepEqual(await again.vault.epochs(), [], "no key taken from a chain it never had reason to trust");
  assert.equal(await again.lacksKeys(), true, "it pairs again instead");
});

test("M2: a join and its activation delivered together at a page load are refused", async () => {
  const { hub, a, words } = await setup();
  await a.sync();
  const seen = a.chain!.seq;
  // While a's page is closed: someone with the words joins, and a day later (by the hub's clock) activates.
  const z = await hub.device("tok", new RecordingKeyStore());
  await z.recover(words.join(" "));
  hub.chainNow = () => Date.now() + DAY + 60_000;
  await z.activateRecovered(words.join(" "), Date.now() + DAY + 60_000);
  assert.equal(hub.state!.device(z.deviceId)?.status.type, "active");
  hub.chainNow = () => Date.now();
  assert.equal(hub.chain.length - 1, seen + 2);
  // a's page loads: both arrive at once. Its clock resumes where it left off,
  // so the join is new to it: the activation is held back (M2 re-review: no
  // longer a thrown error that locked the page out — see cloud-m2-verify).
  const back = await hub.reload(a);
  assert.equal(back.chain!.seq, seen + 1, "the join, not its activation");
  assert.equal(back.chain!.deviceRecord(z.deviceId)?.status.type, "pending");
  assert.equal(back.heldBack?.device, z.deviceId);
});

test("M2: what a device verified is kept, and a hub serving less than that (or another head) is a fork", async () => {
  const { hub, a } = await setup();
  await a.sync();
  const kept = hub.chain.slice();
  hub.chain = kept.slice(0, -1);
  await assert.rejects(hub.reload(a), (e: unknown) => e instanceof AccountError && e.kind === "fork");
  hub.chain = kept;
  const back = await hub.reload(a);
  assert.equal(back.chain!.seq, kept.length - 1);
});

const isFork = (e: unknown) => e instanceof AccountError && e.kind === "fork";

test("E2E review: a page left behind does not roll back what another page of this browser verified (a removal stays seen)", async () => {
  const { hub, a, b } = await setup();
  await a.sync();
  // A second tab of a's browser, at the same head; a's tab goes on without it.
  const stale = await hub.reload(a);
  await a.append({ type: "setRecoveryKey", key: null });
  const n = a.chain!.seq;
  await a.removeDevice(b.deviceId, new FakeCourier());
  assert.equal(a.chain!.deviceRecord(b.deviceId)?.status.type, "removed");
  const full = hub.chain.slice();
  // The hub serves the stale tab entry N (genuine), and nothing past it.
  hub.chain = full.slice(0, n + 1);
  await stale.sync();
  assert.equal(stale.chain!.seq, n);
  // a's tab reloads, and the hub withholds the removal: the head that tab saw is kept, so this is a fork.
  await assert.rejects(hub.reload(a), isFork);
  hub.chain = full;
  const back = await hub.reload(a);
  assert.equal(back.chain!.seq, full.length - 1);
  assert.equal(back.chain!.deviceRecord(b.deviceId)?.status.type, "removed", "the removed device stays removed");
});

test("E2E review: two pages of one browser whose chains differ at the same height are a fork, and the record keeps the first", async () => {
  const { hub, a, b } = await setup();
  await a.sync();
  await b.sync();
  const other = await hub.reload(a);
  const chain0 = hub.chain.slice();
  const state0 = hub.state!.clone();
  await a.append({ type: "setRecoveryKey", key: null });
  const mine = hub.chain.slice();
  // The hub forgets a's entry and takes b's at that seq instead, which it serves the other tab.
  hub.chain = chain0;
  hub.state = state0;
  await b.append({ type: "setRecoveryKey", key: null });
  assert.equal(hub.chain.length - 1, a.chain!.seq, "the same height");
  assert.notEqual(hub.chain.at(-1), mine.at(-1), "another entry");
  await assert.rejects(other.sync(), isFork);
  // What a's tab verified is still the record: b's chain is a fork to it, a's own is not.
  await assert.rejects(hub.reload(a), isFork);
  hub.chain = mine;
  assert.equal((await hub.reload(a)).chain!.seq, a.chain!.seq);
});

test("E2E review: a page behind the record whose chain differs at its own height is a fork", async () => {
  const { hub, a, b } = await setup();
  await a.sync();
  await b.sync();
  const stale = await hub.reload(a);
  const chain0 = hub.chain.slice();
  const state0 = hub.state!.clone();
  await a.append({ type: "setRecoveryKey", key: null });
  await a.append({ type: "setRecoveryKey", key: null });
  const mine = hub.chain.slice();
  // The stale tab is served another entry at the height after its own, one short of a's head.
  hub.chain = chain0;
  hub.state = state0;
  await b.append({ type: "setRecoveryKey", key: null });
  await assert.rejects(stale.sync(), isFork);
  assert.equal(stale.chain!.seq, a.chain!.seq - 2, "the forked entry was not taken");
  assert.ok(stale.forked, "and the page is latched forked");
  hub.chain = mine;
  assert.equal((await hub.reload(a)).chain!.seq, a.chain!.seq, "the record still holds a's head");
});

test("E2E review: first sightings from every page are merged, the earliest of each kept", async (t) => {
  const t0 = Date.now();
  t.mock.timers.enable({ apis: ["Date"], now: t0 });
  const { hub, a, b } = await setup();
  await a.sync();
  await b.sync();
  const stale = await hub.reload(a);
  // b adds entry x; a's tab sees it first, the stale tab a minute later.
  await b.append({ type: "setRecoveryKey", key: null });
  const x = hub.chain.length - 1;
  t.mock.timers.setTime(t0 + 60_000);
  await a.sync();
  t.mock.timers.setTime(t0 + 120_000);
  await stale.sync();
  // b adds y; this time the stale tab sees it first.
  await b.append({ type: "setRecoveryKey", key: null });
  const y = hub.chain.length - 1;
  t.mock.timers.setTime(t0 + 180_000);
  await stale.sync();
  t.mock.timers.setTime(t0 + 240_000);
  await a.sync();
  const back = await hub.reload(a);
  assert.equal(back.chainClock.firstSeen.get(x), t0 + 60_000, "a's sighting of x, not the stale tab's later one");
  assert.equal(back.chainClock.firstSeen.get(y), t0 + 180_000, "the stale tab's sighting of y, not a's later one");
});

// ---- review of #172 ----------------------------------------------------

/**
 * `account`'s browser opened again on `storage` (another page's copy of its
 * `localStorage`), with `locks` — and the browser's seen records (its
 * IndexedDB, which every page reads as it is), or `seen`.
 */
function openPage(hub: FakeKeysHub, account: CloudAccount, storage: AccountStorage, locks: AccountLocks | null = null, seen: SeenStore = browserSeen(account.store)): Promise<CloudAccount> {
  return CloudAccount.open(hub.api(), { ...account.session }, account.store, { origin: HUB_ORIGIN, storage, locks, seen });
}

type StoredSeen = { v: 1; seenThrough: number; head: string; heads?: [number, string][]; firstSeen: [number, number][] };

/** The browser's seen record for the account, as stored. */
function seenRecord(seen: MemorySeenStore, account: string): StoredSeen {
  return structuredClone((seen.values.get(account) as { seen: StoredSeen }).seen);
}

/** The genesis the browser pinned for the account, as stored. */
function storedGenesis(seen: MemorySeenStore, account: string): string | null {
  return (seen.values.get(account) as { genesis?: string } | undefined)?.genesis ?? null;
}

/** Put `record` in the browser's store as the account's seen record (its pin as it is) — what no page writes, for the tests. */
function setSeenRecord(seen: MemorySeenStore, account: string, record: StoredSeen): void {
  seen.values.set(account, { v: 1, genesis: storedGenesis(seen, account), seen: record });
}

/** Let what is under way run (the store's turns, the pages' answers). */
async function settle(): Promise<void> {
  for (let i = 0; i < 20; i++) await Promise.resolve();
}

/** A gate for `MemorySeenStore.gate`: holds the next update back until `release`; `reached` once it is held. */
function holdNextUpdate(seen: MemorySeenStore) {
  let reach!: () => void;
  const reached = new Promise<void>((r) => (reach = r));
  let release!: () => void;
  const released = new Promise<void>((r) => (release = r));
  seen.gate = async () => {
    seen.gate = null;
    reach();
    await released;
  };
  return { reached, release };
}

/** Two equal-height views: a's entry at the next seq, then the hub forgetting it and taking b's there instead. */
async function equalHeightFork(hub: FakeKeysHub, a: CloudAccount, b: CloudAccount): Promise<{ mine: string[]; theirs: string[] }> {
  const chain0 = hub.chain.slice();
  const state0 = hub.state!.clone();
  await a.append({ type: "setRecoveryKey", key: null });
  const mine = hub.chain.slice();
  hub.chain = chain0;
  hub.state = state0;
  await b.append({ type: "setRecoveryKey", key: null });
  return { mine, theirs: hub.chain.slice() };
}

test("review of #172: a fork caught is not installed — the page keeps its view, latches the alarm, and attests, signs and seals nothing", async () => {
  const { hub, a, b } = await setup();
  await a.sync();
  await b.sync();
  const viaSync = await hub.reload(a);
  const viaPush = await hub.reload(a);
  const before = viaSync.chain!.seq;
  const { theirs } = await equalHeightFork(hub, a, b);
  await assert.rejects(viaSync.sync(), isFork);
  await assert.rejects(viaPush.absorb([fromHex(theirs.at(-1)!)]), isFork);
  for (const page of [viaSync, viaPush]) {
    assert.equal(page.chain!.seq, before, "the forked entry is not this page's view");
    assert.ok(page.forked, "the alarm is latched");
    await assert.rejects(page.attestation(), isFork, "it vouches for nothing");
    await assert.rejects(page.append({ type: "setRecoveryKey", key: null }), isFork, "it signs nothing");
    await assert.rejects(page.withUserKeys(async () => undefined), isFork, "it hands out no key");
    await assert.rejects(page.sync(), isFork, "and stays latched");
  }
});

test("review of #172: a page behind what this browser verified vouches for nothing until it catches up, and a hub that will not let it is a fork", async () => {
  const { hub, a, b } = await setup();
  await a.sync();
  const behind = await hub.reload(a);
  const caughtUp = await hub.reload(a);
  await a.append({ type: "setRecoveryKey", key: null });
  await a.removeDevice(b.deviceId, new FakeCourier());
  const full = hub.chain.slice();
  const seq = behind.chain!.seq;
  // The hub keeps the newer entries from this page: its fresh word for the old head is what it would want.
  hub.chainServed = async () => full.slice(0, seq + 1);
  await assert.rejects(behind.attestation(), isFork);
  assert.ok(behind.forked);
  hub.chainServed = null;
  const vouched = await caughtUp.attestation();
  assert.equal(vouched.seq, full.length - 1, "a page that could catch up vouches for the head");
  assert.equal(caughtUp.chain!.deviceRecord(b.deviceId)?.status.type, "removed");
});

/** `backing` (a browser's localStorage) as one page's own copy of it, brought into step only by `settle` (as browsers do, after the fact). */
function pageStorage(backing: Map<string, string>) {
  const copy = new Map(backing);
  return {
    getItem: (k: string) => copy.get(k) ?? null,
    setItem: (k: string, v: string) => void (copy.set(k, v), backing.set(k, v)),
    removeItem: (k: string) => void (copy.delete(k), backing.delete(k)),
    settle: () => {
      copy.clear();
      for (const [k, v] of backing) copy.set(k, v);
    },
  };
}

test("review of #172, #183: a page on a stale copy of localStorage no longer lowers the record — it holds its view against the record as stored", async () => {
  const { hub, a } = await setup();
  await a.sync();
  const backing = browserStorage(a.store).values;
  const seen = browserSeen(a.store);
  const pageA = pageStorage(backing);
  const pageB = pageStorage(backing);
  const tabA = await openPage(hub, a, pageA);
  const tabB = await openPage(hub, a, pageB);
  pageA.settle();
  pageB.settle();
  const seq = tabA.chain!.seq;
  await tabB.append({ type: "setRecoveryKey", key: null });
  await tabB.append({ type: "setRecoveryKey", key: null });
  const full = hub.chain.slice();
  assert.equal(seenRecord(seen, a.account).seenThrough, seq + 2);
  // Page A, on its stale copy, takes only the next entry: what it writes is
  // made of the record as stored, B's, in the same step.
  hub.chain = full.slice(0, seq + 2);
  await tabA.sync();
  assert.equal(tabA.chain!.seq, seq + 1);
  assert.equal(seenRecord(seen, a.account).seenThrough, seq + 2, "not lowered, with no page having to put it back");
  // So a later load of the browser, served the chain without B's entries, is a fork.
  await assert.rejects(openPage(hub, a, pageStorage(backing)), isFork);
  hub.chain = full;
});

test("review of #172: the pin is read, compared and written under one Web Lock per account, and the seen record with it", async () => {
  const { hub, a } = await setup();
  await a.sync();
  const storage = browserStorage(a.store);
  const seen = browserSeen(a.store);
  const asked: string[] = [];
  let release!: () => void;
  const released = new Promise<void>((r) => (release = r));
  let heard!: () => void;
  const askedOnce = new Promise<void>((r) => (heard = r));
  const locks: AccountLocks = {
    request: async (name, options, callback) => {
      asked.push(`${name} ${options.mode}`);
      heard();
      await released;
      return callback();
    },
  };
  const seq = a.chain!.seq;
  const page = openPage(hub, a, storage, locks);
  // Waited for, however busy the machine: the request itself, or the page ending without one (which fails here).
  await Promise.race([
    askedOnce,
    page.then(() => {
      throw new Error("the page finished without asking for the lock");
    }),
  ]);
  assert.deepEqual(asked, [`rh.hub.seen.${a.account} exclusive`], "the page asks for the account's lock");
  // While it waits, another page records more.
  await a.append({ type: "setRecoveryKey", key: null });
  assert.equal(seenRecord(seen, a.account).seenThrough, seq + 1);
  release();
  await page;
  assert.equal(seenRecord(seen, a.account).seenThrough, seq + 1, "compared with the record as it was once the lock was its own: not lowered");
});

test("review of #172: first sightings are not merged into a record this page could not hold its view against", async () => {
  const { hub, a } = await setup();
  await a.sync();
  const seen = browserSeen(a.store);
  const behind = await hub.reload(a);
  await a.append({ type: "setRecoveryKey", key: null });
  await a.append({ type: "setRecoveryKey", key: null });
  const full = hub.chain.slice();
  // The record as a page from before `heads` wrote it, with no sightings.
  const legacy = seenRecord(seen, a.account);
  delete legacy.heads;
  legacy.firstSeen = [];
  setSeenRecord(seen, a.account, legacy);
  // And where older pages read it (pages of this version keep it there too).
  browserStorage(a.store).setItem(`rh.hub.seen.${a.account}`, JSON.stringify(legacy));
  hub.chain = full.slice(0, full.length - 1);
  await behind.sync();
  hub.chain = full;
  assert.equal(behind.chain!.seq, full.length - 2, "a page behind the record");
  assert.deepEqual(seenRecord(seen, a.account).firstSeen, [], "its sightings stay out of a record it could not check");
});

test("review of #172: a hub serving no chain to a browser that pinned one is a fork, and no new genesis is made over the pin", async () => {
  const { hub, a } = await setup();
  await a.sync();
  const posts = hub.chainPosts;
  hub.chainServed = async () => [];
  await assert.rejects(hub.reload(a), isFork, "an empty chain is no fresh account here");
  await assert.rejects(a.createGenesis(false), isFork, "never a second genesis on a pinned browser");
  assert.equal(hub.chainPosts, posts, "nothing was sent");
  hub.chainServed = null;
  assert.equal((await hub.reload(a)).chain!.seq, hub.chain.length - 1);
});

test("review of #172: a page that fetched the chain just before another page recorded more looks once more before calling it a fork", async () => {
  const { hub, a } = await setup();
  await a.sync();
  const before = hub.chain.slice();
  await a.append({ type: "setRecoveryKey", key: null });
  let served = 0;
  // The first answer predates a's append (as if fetched a moment before it landed).
  hub.chainServed = async () => (served++ === 0 ? before : undefined);
  const page = await hub.reload(a);
  hub.chainServed = null;
  assert.ok(served >= 2, "fetched once more");
  assert.equal(page.chain!.seq, hub.chain.length - 1);
  assert.equal(page.forked, null);
});

// ---- re-review of #172 ---------------------------------------------------

/** A gate for `hub.gate`: holds back the first request `matches` says, until `release`; `reached` once it is held. */
function holdFirst(hub: FakeKeysHub, matches: (request: string, body: unknown) => boolean, then?: () => void) {
  let reach!: () => void;
  const reached = new Promise<void>((r) => (reach = r));
  let release!: () => void;
  const released = new Promise<void>((r) => (release = r));
  hub.gate = async (request, body) => {
    if (!matches(request, body)) return;
    hub.gate = null;
    reach();
    await released;
    then?.();
  };
  return { reached, release };
}

/** Latch `a`'s fork alarm on a real path: a new genesis asked for on its pinned browser while the hub serves no chain. */
async function latchAFork(hub: FakeKeysHub, a: CloudAccount): Promise<void> {
  hub.chainServed = async () => [];
  await assert.rejects(a.createGenesis(false), isFork);
  hub.chainServed = null;
  assert.ok(a.forked);
}

test("re-review of #172 (#179's reviewer): two pages on the set-up screen — the second click says the account was just set up in another tab, not a fork", async () => {
  const hub = new FakeKeysHub();
  const tabA = await hub.device("tok");
  const tabB = await hub.reload(tabA);
  await tabA.createGenesis(false);
  const posts = hub.chainPosts;
  await assert.rejects(tabB.createGenesis(false), (e: unknown) => e instanceof AccountError && e.kind === "setUpElsewhere");
  assert.equal(tabB.forked, null, "no alarm");
  assert.equal(hub.chainPosts, posts, "and no second genesis");
  assert.equal((await hub.reload(tabB)).chain!.seq, hub.chain.length - 1, "a reload opens the account");
});

test("re-review of #172: a handshake's copy that differs from this view at the same height is a fork, even when it adds nothing", async () => {
  const { hub, a, b } = await setup();
  await a.sync();
  await b.sync();
  const page = await hub.reload(a);
  const chain0 = hub.chain.slice();
  const state0 = hub.state!.clone();
  await b.append({ type: "setRecoveryKey", key: null });
  const theirs = decodeEntry(fromHex(hub.chain.at(-1)!));
  hub.chain = chain0;
  hub.state = state0;
  await a.append({ type: "setRecoveryKey", key: null });
  // The agent's reply teaches B at the next seq while a sync of this page takes A there.
  await assert.rejects(
    page.completeHandshake(async (merged) => {
      await merged.chain.merge([theirs], merged.clock);
      await page.sync();
      return "open";
    }),
    isFork,
  );
  assert.ok(page.forked);
  assert.equal(page.chain!.seq, a.chain!.seq, "this page keeps A");
});

test("re-review of #172: user keys are not handed out by a call that began before the fork latched", async () => {
  const { hub, a } = await setup();
  await a.sync();
  const held = holdFirst(hub, (r) => r === "GET /v1/mailbox");
  let used = false;
  const handing = a.withUserKeys(async () => {
    used = true;
  });
  await held.reached;
  // Meanwhile the page latches a fork (a new genesis asked for while the hub serves no chain at all).
  await latchAFork(hub, a);
  held.release();
  await assert.rejects(handing, isFork);
  assert.equal(used, false, "the keys never reached the callback");
});

test("re-review of #172: a loop mailing user key epochs stops at the fork, between one epoch and the next", async () => {
  const { hub, a, b, words } = await setup();
  await a.removeDevice(b.deviceId, new FakeCourier());
  assert.deepEqual(await a.vault.epochs(), [0, 1]);
  const z = await hub.device("tok", new RecordingKeyStore());
  await z.recover(words.join(" "));
  await a.sync();
  const toZ = () => hub.mailAttempts.filter((m) => m === `${a.deviceId} ${z.deviceId} ${MAIL_USER_KEY}`).length;
  const held = holdFirst(hub, (r, body) => r === "POST /v1/mailbox" && (body as { recipient?: string; purpose?: number }).recipient === z.deviceId && (body as { purpose?: number }).purpose === MAIL_USER_KEY);
  const approving = a.approveDevice(z.deviceId);
  await held.reached;
  await latchAFork(hub, a);
  held.release();
  await assert.rejects(approving, isFork);
  assert.equal(toZ(), 1, "the epoch held at the hub went; the next was neither sealed nor sent");
});

test("re-review of #172: two first set-ups at once: the genesis pinned first stands, with its record", async () => {
  const hub = new FakeKeysHub();
  const tabA = await hub.device("tok");
  const tabB = await hub.reload(tabA);
  const storage = browserStorage(tabA.store);
  const seen = browserSeen(tabA.store);
  // A's genesis is held at the hub until B has made and pinned its own; then the hub takes A's too.
  const held = holdFirst(
    hub,
    (r) => r === "POST /v1/chain",
    () => {
      hub.chain = [];
      hub.state = null;
    },
  );
  const settingUp = tabA.createGenesis(false);
  await held.reached;
  await tabB.createGenesis(false);
  const pin = storage.getItem(`rh.hub.genesis.${tabB.account}`);
  const record = seenRecord(seen, tabB.account);
  assert.ok(pin && record);
  held.release();
  await assert.rejects(settingUp, isFork);
  assert.equal(storage.getItem(`rh.hub.genesis.${tabB.account}`), pin, "B's pin stands");
  assert.deepEqual(seenRecord(seen, tabB.account), record, "and B's record");
});

test("re-review of #172, #183: an open page puts a lowered record back when another page writes it (the store says so), when it comes back into view, and as it starts watching", async () => {
  const { hub, a } = await setup();
  await a.sync();
  await a.append({ type: "setRecoveryKey", key: null });
  const seen = browserSeen(a.store);
  const page = await hub.reload(a);
  const seq = page.chain!.seq;
  // Lowered as no page's write lowers it any more (code in this origin, say).
  const lowered: StoredSeen = { v: 1, seenThrough: seq - 1, head: hex(page.chain!.hashAt(seq - 1)!), heads: [], firstSeen: [] };
  const documentLike = Object.assign(new EventTarget(), { visibilityState: "visible" });
  setSeenRecord(seen, a.account, lowered);
  page.watchSeenRecord({ visibility: documentLike });
  await settle();
  assert.equal(seenRecord(seen, a.account).seenThrough, seq, "as it starts watching (a load)");
  // Written through the store, which tells every page (IndexedDB's writers say so on a BroadcastChannel).
  await seen.update(a.account, (stored) => ({ ...(stored as object), seen: lowered }));
  await settle();
  assert.equal(seenRecord(seen, a.account).seenThrough, seq, "as soon as another page wrote it");
  setSeenRecord(seen, a.account, lowered);
  documentLike.visibilityState = "hidden";
  documentLike.dispatchEvent(new Event("visibilitychange"));
  await settle();
  assert.equal(seenRecord(seen, a.account).seenThrough, seq - 1, "not while hidden");
  documentLike.visibilityState = "visible";
  documentLike.dispatchEvent(new Event("visibilitychange"));
  await settle();
  assert.equal(seenRecord(seen, a.account).seenThrough, seq, "once back in view");
});

test("re-review of #172: a page loading while other pages record twice more looks again until the chain holds the record's head", async () => {
  const { hub, a } = await setup();
  await a.sync();
  const before = hub.chain.slice();
  await a.append({ type: "setRecoveryKey", key: null });
  const once = hub.chain.slice();
  await a.append({ type: "setRecoveryKey", key: null });
  let served = 0;
  // Each answer one advance behind the record, twice.
  hub.chainServed = async () => [before, once][served++];
  const page = await hub.reload(a);
  hub.chainServed = null;
  assert.ok(served >= 3, "fetched until it held the record's head");
  assert.equal(page.chain!.seq, hub.chain.length - 1);
  assert.equal(page.forked, null);
});

test("M1: every device change another device made is an alert until this device says it was it", async () => {
  const { hub, a, b } = await setup();
  assert.deepEqual(a.alerts(), [], "its own changes are not alerts");
  assert.deepEqual(b.alerts(), [], "what happened before it joined is history to it");
  // b adds c, sets new words, and removes c.
  const c = await pair(hub, b);
  await b.setRecoveryWords();
  await b.removeDevice(c.deviceId, new FakeCourier());
  await a.sync();
  assert.deepEqual(
    a.alerts().map((x) => [x.kind, x.by, x.subject, x.remove]),
    [
      ["addDevice", b.deviceId, c.deviceId, null],
      ["setRecoveryKey", b.deviceId, null, b.deviceId],
      ["removeDevice", b.deviceId, c.deviceId, null],
    ],
  );
  assert.ok(a.alerts().every((x) => x.atMs > 0));
  assert.deepEqual(b.alerts(), []);
  // "That was me" — kept on this device across a reload, and only for that alert.
  a.acknowledge(a.alerts()[1]!.seq);
  const again = await hub.reload(a);
  assert.deepEqual(
    again.alerts().map((x) => x.kind),
    ["addDevice", "removeDevice"],
  );
  for (const x of again.alerts()) again.acknowledge(x.seq);
  assert.deepEqual(again.alerts(), []);
  assert.deepEqual((await hub.reload(a)).alerts(), []);
  // A recovery join is the pending banner's while it waits; once activated, it is an alert.
  const z = await hub.device("tok", new RecordingKeyStore());
  const phrase = await b.setRecoveryWords();
  await z.recover(phrase.join(" "));
  await again.sync();
  assert.deepEqual(
    again.alerts().map((x) => x.kind),
    ["setRecoveryKey"],
  );
  await again.approveDevice(z.deviceId);
  assert.deepEqual(
    again.alerts().map((x) => x.kind),
    ["setRecoveryKey"],
    "a join this device approved itself needs no alert",
  );
});

test("rotate keys again: due after a removal no new user key followed, and it completes one that died half-way", async () => {
  const { hub, a, b, machine } = await setup();
  // A removal alone (an older page, or one that died before rotating).
  await a.append({ type: "removeDevice", id: b.deviceId });
  assert.equal(a.rotationDue, true);
  const report = await a.rotate(new FakeCourier());
  assert.equal(a.rotationDue, false);
  assert.equal(report.ukEpoch, 1);
  // A rotation whose blobs never reached the keyring: the keys are committed but nobody can read them.
  hub.noKeyring = true;
  const broken = await a.rotate(new FakeCourier());
  // The report says so (handed over all the same: M2 re-review N2).
  assert.ok(broken.machines[0]!.keyring, `its machine key could not be put: ${JSON.stringify(broken)}`);
  hub.noKeyring = false;
  await a.keyring.refresh();
  assert.equal(await a.machineKeyBytes(machine.id, 2), null);
  // Rotating again makes new committed epochs, and their blobs are there.
  const fixed = await a.rotate(new FakeCourier());
  assert.equal(fixed.ukEpoch, 3);
  assert.equal(fixed.machines[0]!.mkEpoch, 3);
  assert.ok(await a.machineKeyBytes(machine.id, 3));
  assert.deepEqual(fixed.machines[0]!.result, { via: "channel", changed: true });
});

test("an agent behind the chain's key epoch is handed the committed key in the channel, else flagged", async () => {
  const { a, machine } = await setup();
  await a.issueMachineKey(machine.id, Object.assign(new FakeCourier(), { online: false }));
  const latest = a.chain!.latestMachineEpoch(machine.id);
  assert.equal(latest, 1);
  const posted: { epoch: number; key: string; reencrypt: boolean }[] = [];
  const post = async (body: { epoch: number; key: string; reencrypt: boolean }) => (posted.push(body), { status: 200, body: { changed: true } });
  const keyBytes = (e: number) => a.machineKeyBytes(machine.id, e);
  assert.equal(await checkAgentKey({ latest, agentEpoch: async () => 1, keyBytes, post }), "current");
  assert.equal(await checkAgentKey({ latest, agentEpoch: async () => undefined, keyBytes, post }), "unknown");
  assert.equal(await checkAgentKey({ latest, agentEpoch: async () => 0, keyBytes, post }), "updated");
  assert.deepEqual(posted.map((p) => [p.epoch, p.reencrypt]), [[1, false]]);
  await a.chain!.checkMachineKey(machine.id, 1, Buffer.from(posted[0]!.key, "hex"));
  assert.equal(await checkAgentKey({ latest, agentEpoch: async () => null, keyBytes, post: async () => ({ status: 409, body: null }) }), "behind");
  assert.equal(await checkAgentKey({ latest, agentEpoch: async () => 0, keyBytes: async () => null, post }), "behind");
});

test("sessions: minted by the device key, renewed once on a 401, and a second refusal stops the device", async () => {
  const { hub, a } = await setup();
  const first = a.session.token;
  assert.match(first, new RegExp(`^ds-${a.deviceId}-`));
  // The session went away (expired, revoked): one renewal, and the request goes through.
  hub.sessions.delete(first);
  await a.sync();
  assert.notEqual(a.session.token, first);
  assert.equal(a.refusal, null);
  // A hub that refuses every session it hands out: renewed once, refused again, and the device stops asking.
  hub.refuseAllSessions = true;
  await assert.rejects(a.api.keys(a.session.token), { status: 401 });
  const minted = hub.sessions.size;
  await assert.rejects(a.api.keys(a.session.token), { status: 401 });
  assert.equal(a.refusal, "denied");
  await assert.rejects(a.api.keys(a.session.token), { status: 401 });
  assert.equal(hub.sessions.size, minted, "no session minted after the second refusal");
});

test("the hub's caps are surfaced: mail rate, a full mailbox, and the write-once keyring", async () => {
  const { hub, a, b } = await setup();
  hub.mailRatePerMin = 0;
  await assert.rejects(a.resendUserKeys(b.deviceId), (e: unknown) => hubErrorKind(e) === "mailRate");
  hub.mailRatePerMin = 120;
  for (let i = 0; i < 256; i++) hub.mail.push({ id: 10_000 + i, recipient: b.deviceId, sender: a.deviceId, purpose: 9, epoch: 0, sealed: "00" });
  await assert.rejects(a.resendUserKeys(b.deviceId), (e: unknown) => hubErrorKind(e) === "mailboxFull");
  const report = await a.rotate(new FakeCourier());
  assert.deepEqual(report.failedTo, [b.deviceId]);
  assert.equal(report.failedWhy[b.deviceId], "mailbox_full");
  // The same bytes again are fine; other bytes under a written name are refused —
  // and the keyring, which writes only names of its own, takes that as tampering
  // and puts the blob under a new name (M2 re-review #27).
  const base = keyName.machine("m-1", 0, 0);
  const name = keyName.written(base, a.deviceId);
  const blob = Buffer.from(hub.keys.get(name)!.blob, "hex");
  assert.equal(await a.keyring.put(base, blob), name);
  await assert.rejects(a.api.putKey(a.session.token, name, new Uint8Array(blob.length)), (e: unknown) => hubErrorKind(e) === "exists");
  const renamed = await a.keyring.put(base, new Uint8Array(blob.length));
  assert.match(renamed, new RegExp(`^${name}\\.[0-9a-f]{8}$`));
  // A device that is not a current full device may not write it at all.
  const z = await hub.device("tok", new RecordingKeyStore());
  await assert.rejects(z.api.putKey(z.session.token, `mk/m-1/0/0/${z.deviceId}`, new Uint8Array(16)), (e: unknown) => hubErrorKind(e) === "notFullDevice");
});

// ---- third look at #172 --------------------------------------------------

test("third look at #172: a handshake's copy that adds nothing is still held against the record", async () => {
  const { hub, a } = await setup();
  await a.sync();
  const seen = browserSeen(a.store);
  const page = await hub.reload(a);
  const seq = page.chain!.seq;
  // The record as another page verified it a moment ago: another head at this very height.
  setSeenRecord(seen, a.account, { v: 1, seenThrough: seq, head: "ff".repeat(32), heads: [[seq, "ff".repeat(32)]], firstSeen: [] });
  await assert.rejects(
    page.completeHandshake(async () => "open"),
    isFork,
  );
  assert.ok(page.forked, "no channel goes on from it");
});

test("third look at #172: \"set up in another tab\" only for the very chain this browser recorded; a chain that cannot be read is tried again, not a fork", async () => {
  const hub = new FakeKeysHub();
  const tabA = await hub.device("tok");
  await tabA.createGenesis(false);
  const full = hub.chain.slice();
  assert.ok(full.length >= 2);
  // Not readable: no verdict either way.
  const tabB = await hub.reload(tabA);
  hub.gate = async (request) => {
    if (request !== "GET /v1/chain") return;
    hub.gate = null;
    throw new TypeError("the network is down");
  };
  await assert.rejects(tabB.createGenesis(false), (e: unknown) => !(e instanceof AccountError && e.kind === "fork"));
  assert.equal(tabB.forked, null, "nothing latched");
  // The genesis alone: not the chain this browser recorded.
  hub.chainServed = async () => full.slice(0, 1);
  await assert.rejects(tabB.createGenesis(false), isFork);
  hub.chainServed = null;
  // The genesis, then another history.
  const tabC = await hub.reload(tabA);
  const genesisOnly = await ChainState.verify([decodeEntry(fromHex(full[0]!))], undefined, ChainClock.history());
  const other = await signDraft(genesisOnly.draft({ type: "device", id: tabA.deviceId }, { type: "setRecoveryKey", key: null }, Date.now()), tabA.keys.sign.privateKey);
  hub.chainServed = async () => [full[0]!, hex(encodeEntry(other))];
  await assert.rejects(tabC.createGenesis(false), isFork);
  hub.chainServed = null;
  // The recorded chain itself: set up in another tab.
  const tabD = await hub.reload(tabA);
  await assert.rejects(tabD.createGenesis(false), (e: unknown) => e instanceof AccountError && e.kind === "setUpElsewhere");
  assert.equal(tabD.forked, null);
});

test("third look at #172: no machine key leaves after a fork latched while the hand-over waited for the machine", async () => {
  const { hub, a, machine } = await setup();
  await a.sync();
  let reach!: () => void;
  const reached = new Promise<void>((r) => (reach = r));
  let release!: () => void;
  const released = new Promise<void>((r) => (release = r));
  class WaitingCourier extends FakeCourier {
    override async waitOnline(machine: string, timeoutMs: number): Promise<boolean> {
      reach();
      await released;
      return super.waitOnline(machine, timeoutMs);
    }
  }
  const courier = new WaitingCourier();
  const handing = a.handOverMachineKey(machine.id, courier);
  await reached;
  await latchAFork(hub, a);
  release();
  await assert.rejects(handing, isFork);
  assert.deepEqual(courier.posted, [], "not in the channel");
  assert.deepEqual(courier.mailed, [], "nor by mail");
});

test("third look at #172: user keys read before a fork latched are not handed over after it", async () => {
  const hub = new FakeKeysHub();
  let reach!: () => void;
  const reached = new Promise<void>((r) => (reach = r));
  let release!: () => void;
  const released = new Promise<void>((r) => (release = r));
  let armed = false;
  class GatedStore extends MemoryKeyStore {
    override async get(name: string): Promise<unknown> {
      if (armed && /^uk\/[^/]+\/\d+$/.test(name)) {
        armed = false;
        reach();
        await released;
      }
      return super.get(name);
    }
  }
  const a = await hub.device("tok", new GatedStore());
  await a.createGenesis(false);
  let used = false;
  armed = true;
  const handing = a.withUserKeys(async () => {
    used = true;
  });
  await reached;
  await latchAFork(hub, a);
  release();
  await assert.rejects(handing, isFork);
  assert.equal(used, false, "the keys never reached the callback");
});

test("third look at #172: a first load retries only a checkpoint still moving; a contradicting chain is a fork at once", async () => {
  const { hub, a, b } = await setup();
  await a.sync();
  await b.sync();
  const { mine, theirs } = await equalHeightFork(hub, a, b);
  let served = 0;
  hub.chainServed = async () => (served++ === 0 ? theirs : mine);
  await assert.rejects(hub.reload(a), isFork);
  hub.chainServed = null;
  assert.equal(served, 1, "not fetched again");
});

// ---- #183: the seen record in IndexedDB ---------------------------------

/** One page's handle on the browser's seen records, which counts (and says) the updates the page asks for. */
function pageSeen(seen: MemorySeenStore) {
  const page = { updates: 0, onUpdate: null as null | (() => void), store: null as unknown as SeenStore };
  page.store = {
    get atomic() {
      return seen.atomic;
    },
    get: (account) => seen.get(account),
    update: (account, change) => {
      page.updates++;
      page.onUpdate?.();
      return seen.update(account, change);
    },
    watch: (heard) => seen.watch(heard),
  };
  return page;
}

/** Wait (a few ms at a time) until `done`, or fail. */
async function until(done: () => boolean, what: string): Promise<void> {
  for (let i = 0; i < 200 && !done(); i++) await new Promise((r) => setTimeout(r, 5));
  assert.ok(done(), what);
}

test("#183: two pages writing the seen record at once never leave it lower, whichever goes first", async () => {
  for (const higherFirst of [true, false]) {
    const { hub, a, b } = await setup();
    await a.sync();
    const seen = browserSeen(a.store);
    const backing = browserStorage(a.store).values;
    const lowSeen = pageSeen(seen);
    const highSeen = pageSeen(seen);
    // Two pages of a's browser, each on its own copy of localStorage.
    const low = await openPage(hub, a, pageStorage(backing), null, lowSeen.store);
    const high = await openPage(hub, a, pageStorage(backing), null, highSeen.store);
    const seq = low.chain!.seq;
    // Another browser adds two entries: one page is served the first, the other both.
    await b.append({ type: "setRecoveryKey", key: null });
    await b.append({ type: "setRecoveryKey", key: null });
    const full = hub.chain.slice();
    const [first, second] = higherFirst ? [high, low] : [low, high];
    const served = (page: CloudAccount) => (page === low ? full.slice(0, seq + 2) : full);
    // The first page's write is held in the store until the second's is asked for (behind it, as IndexedDB queues it).
    const held = holdNextUpdate(seen);
    hub.chainServed = async () => served(first);
    const one = first.sync();
    await held.reached;
    const asked = new Promise<void>((r) => ((first === low ? highSeen : lowSeen).onUpdate = r));
    hub.chainServed = async () => served(second);
    const two = second.sync();
    await asked;
    held.release();
    await Promise.all([one, two]);
    hub.chainServed = null;
    assert.equal(low.chain!.seq, seq + 1);
    assert.equal(high.chain!.seq, seq + 2);
    assert.equal(seenRecord(seen, a.account).seenThrough, seq + 2, `${higherFirst ? "the higher" : "the lower"} first: the record keeps the higher`);
  }
});

test("#183: an older page's record in localStorage counts where it is proven one chain with the store's and goes past it, never lower, is a fork where it disagrees, and is left there", async () => {
  const { hub, a } = await setup();
  await a.sync();
  const storage = browserStorage(a.store);
  const seen = browserSeen(a.store);
  const key = `rh.hub.seen.${a.account}`;
  const before = seenRecord(seen, a.account);
  await a.append({ type: "setRecoveryKey", key: null });
  await a.append({ type: "setRecoveryKey", key: null });
  const after = seenRecord(seen, a.account);
  assert.equal(after.seenThrough, before.seenThrough + 2);
  assert.deepEqual(JSON.parse(storage.getItem(key)!), after, "the record, for older pages");
  assert.equal(storage.getItem(`rh.hub.genesis.${a.account}`), storedGenesis(seen, a.account), "and the pin");
  // Higher, and proven one chain with the store's: an older page verified two entries more (what it wrote is the same JSON).
  setSeenRecord(seen, a.account, before);
  storage.setItem(key, JSON.stringify(after));
  const full = hub.chain.slice();
  hub.chain = full.slice(0, before.seenThrough + 1);
  await assert.rejects(hub.reload(a), isFork, "a load served less than the older page verified is a fork");
  hub.chain = full;
  await hub.reload(a);
  assert.equal(seenRecord(seen, a.account).seenThrough, after.seenThrough, "the store takes the older page's higher record");
  assert.equal(storage.getItem(key), JSON.stringify(after), "and leaves it where it is: an older page may still be writing there");
  // Lower: an older page on a stale view writes what it saw.
  storage.setItem(key, JSON.stringify(before));
  await hub.reload(a);
  assert.equal(seenRecord(seen, a.account).seenThrough, after.seenThrough, "never lowered by it");
  // Disagreeing: another head at the store's height.
  storage.setItem(key, JSON.stringify({ ...after, head: "ff".repeat(32), heads: [[after.seenThrough, "ff".repeat(32)]] }));
  await assert.rejects(hub.reload(a), isFork);
  assert.equal(seenRecord(seen, a.account).head, after.head, "the store's record stays as it was");
});

test("#183 review: an older page's higher record that does not show it holds the store's checkpoint is not taken for it — both are held, and a chain missing either is a fork", async () => {
  const { hub, a, b } = await setup();
  await a.sync();
  await b.sync();
  const seen = browserSeen(a.store);
  const storage = browserStorage(a.store);
  const chain0 = hub.chain.slice();
  const state0 = hub.state!.clone();
  const s = chain0.length - 1;
  // a's browser verifies A at s+1.
  await a.append({ type: "setRecoveryKey", key: null });
  const mine = hub.chain.slice();
  const ours = hex(a.chain!.hashAt(s + 1)!);
  assert.equal(seenRecord(seen, a.account).head, ours);
  // The hub forgets it and takes another history, B, three entries from
  // there — which an older page of a's browser verified, keeping only its
  // last two hashes (none at s+1).
  hub.chain = chain0;
  hub.state = state0;
  for (let i = 0; i < 3; i++) await b.append({ type: "setRecoveryKey", key: null });
  const theirs = hub.chain.slice();
  const forked = b.chain!;
  const older: StoredSeen = {
    v: 1,
    seenThrough: s + 3,
    head: hex(forked.hashAt(s + 3)!),
    heads: [
      [s + 2, hex(forked.hashAt(s + 2)!)],
      [s + 3, hex(forked.hashAt(s + 3)!)],
    ],
    firstSeen: [],
  };
  storage.setItem(`rh.hub.seen.${a.account}`, JSON.stringify(older));
  // Served B, which holds the older page's checkpoint and not the store's: a fork, and the store keeps A.
  hub.chain = theirs;
  await assert.rejects(hub.reload(a), isFork);
  assert.equal(seenRecord(seen, a.account).head, ours, "the store's checkpoint stays");
  // Served A, which holds the store's checkpoint and not the older page's: never proven, so a fork too.
  hub.chain = mine;
  await assert.rejects(hub.reload(a), isFork);
  assert.equal(seenRecord(seen, a.account).head, ours);
});

test("#183 review: an older page's newer record, written after this page's copy of localStorage was taken, is not deleted", async () => {
  const { hub, a } = await setup();
  await a.sync();
  const seen = browserSeen(a.store);
  const backing = browserStorage(a.store).values;
  const key = `rh.hub.seen.${a.account}`;
  const r0 = seenRecord(seen, a.account);
  await a.append({ type: "setRecoveryKey", key: null });
  const r1 = seenRecord(seen, a.account);
  await a.append({ type: "setRecoveryKey", key: null });
  const r2 = seenRecord(seen, a.account);
  // The store holds r0; an older page wrote r1, and this page's copy of localStorage is taken now.
  setSeenRecord(seen, a.account, r0);
  backing.set(key, JSON.stringify(r1));
  const copy = pageStorage(backing);
  // Then the older page writes r2, which this page's copy does not show.
  backing.set(key, JSON.stringify(r2));
  // This page is served the chain as far as r1.
  const full = hub.chain.slice();
  hub.chain = full.slice(0, r1.seenThrough + 1);
  await openPage(hub, a, copy);
  assert.equal(seenRecord(seen, a.account).seenThrough, r1.seenThrough, "the page took what its copy showed");
  assert.equal(backing.get(key), JSON.stringify(r2), "and did not delete the newer record it never read");
  // Which still counts: a load served no more than that is behind what the older page verified.
  await assert.rejects(hub.reload(a), isFork);
  hub.chain = full;
  await hub.reload(a);
  assert.equal(seenRecord(seen, a.account).seenThrough, r2.seenThrough);
});

test("#183 review: the pin is decided in the store — a page whose copy of localStorage shows no pin cannot pin another genesis over another page's", async () => {
  const hub = new FakeKeysHub();
  const first = await hub.device("tok");
  const seen = browserSeen(first.store);
  const backing = browserStorage(first.store).values;
  // Two pages on the set-up screen, each on its own copy of localStorage.
  const tabA = await openPage(hub, first, pageStorage(backing));
  const tabB = await openPage(hub, first, pageStorage(backing));
  // A's genesis is held at the hub until B has made and pinned its own; then the hub takes A's too.
  const held = holdFirst(
    hub,
    (r) => r === "POST /v1/chain",
    () => {
      hub.chain = [];
      hub.state = null;
    },
  );
  const settingUp = tabA.createGenesis(false);
  await held.reached;
  await tabB.createGenesis(false);
  const pin = storedGenesis(seen, tabB.account);
  const record = seenRecord(seen, tabB.account);
  assert.ok(pin && record);
  held.release();
  await assert.rejects(settingUp, isFork, "A's page, whose copy shows no pin, is told of B's");
  assert.equal(storedGenesis(seen, tabB.account), pin, "B's pin stands");
  assert.deepEqual(seenRecord(seen, tabB.account), record, "with B's record");
});

test("#183 review: a pin only older pages kept (localStorage) is taken into the store; another one beside the store's is a fork", async () => {
  const { hub, a } = await setup();
  await a.sync();
  const seen = browserSeen(a.store);
  const storage = browserStorage(a.store);
  const genesis = storedGenesis(seen, a.account)!;
  // As a browser that only ever ran older pages: nothing in the store.
  seen.values.delete(a.account);
  const page = await hub.reload(a);
  assert.equal(page.pinned, true, "the older pages' pin is this browser's");
  assert.equal(storedGenesis(seen, a.account), genesis, "and the store holds it now");
  storage.setItem(`rh.hub.genesis.${a.account}`, "ff".repeat(32));
  await assert.rejects(hub.reload(a), isFork);
});

test("#183 review: a view whose record could not be written is not installed, and is tried again", async () => {
  const { hub, a, b } = await setup();
  await a.sync();
  const seen = browserSeen(a.store);
  let failing = false;
  const flaky: SeenStore = {
    get atomic() {
      return seen.atomic;
    },
    get: (account) => seen.get(account),
    // The commit fails after the compare (a full disk, say).
    update: (account, change) =>
      seen.update(account, (stored) => {
        const next = change(stored);
        if (failing && next !== undefined) throw new DOMException("the disk is full", "QuotaExceededError");
        return next;
      }),
    watch: (heard) => seen.watch(heard),
  };
  const page = await openPage(hub, a, browserStorage(a.store), null, flaky);
  const seq = page.chain!.seq;
  await b.append({ type: "setRecoveryKey", key: null });
  failing = true;
  await assert.rejects(page.sync(), (e: unknown) => e instanceof SeenStoreError);
  assert.equal(page.chain!.seq, seq, "not installed");
  assert.equal(page.forked, null, "and no fork");
  failing = false;
  await page.sync();
  assert.equal(page.chain!.seq, seq + 1, "taken once the store takes its record");
  assert.equal(seenRecord(seen, a.account).seenThrough, seq + 1);
});

test("#183 review: a store that fails on the set-up screen is tried again, not taken for another chain", async () => {
  const hub = new FakeKeysHub();
  const tabA = await hub.device("tok");
  const seen = browserSeen(tabA.store);
  let failNext = false;
  const flaky: SeenStore = {
    get atomic() {
      return seen.atomic;
    },
    get: (account) => {
      if (!failNext) return seen.get(account);
      failNext = false;
      return Promise.reject(new DOMException("Connection to Indexed Database server lost. Refresh the page to try again", "UnknownError"));
    },
    update: (account, change) => seen.update(account, change),
    watch: (heard) => seen.watch(heard),
  };
  const tabB = await openPage(hub, tabA, browserStorage(tabA.store), null, flaky);
  await tabA.createGenesis(false);
  failNext = true;
  await assert.rejects(tabB.createGenesis(false), (e: unknown) => e instanceof SeenStoreError);
  assert.equal(tabB.forked, null, "no alarm");
  await assert.rejects(tabB.createGenesis(false), (e: unknown) => e instanceof AccountError && e.kind === "setUpElsewhere");
  assert.equal(tabB.forked, null);
});

test("#183: a page pinning on a stale copy of localStorage (no pin in it yet) keeps another page's record of the same chain", async () => {
  const { hub, a } = await setup();
  await a.sync();
  const backing = browserStorage(a.store).values;
  const seen = browserSeen(a.store);
  // A page whose copy of localStorage does not show the pin yet.
  const copy = pageStorage(backing);
  copy.removeItem(`rh.hub.genesis.${a.account}`);
  backing.set(`rh.hub.genesis.${a.account}`, hex(a.chain!.hashAt(0)!));
  const stale = await openPage(hub, a, copy);
  const seq = stale.chain!.seq;
  // Another page goes on; then the stale page pins (as a pairing's delivery does).
  await a.append({ type: "setRecoveryKey", key: null });
  await a.append({ type: "setRecoveryKey", key: null });
  await stale.storeUserKeys([]);
  assert.equal(stale.chain!.seq, seq);
  assert.equal(seenRecord(seen, a.account).seenThrough, seq + 2, "not replaced by the stale page's lower view");
});

test("#183: a page hears another page's write of the record through the store, and holds it against its own view", async () => {
  const { hub, a } = await setup();
  await a.sync();
  const listening = pageSeen(browserSeen(a.store));
  const page = await openPage(hub, a, browserStorage(a.store), null, listening.store);
  page.watchSeenRecord({});
  await settle();
  const looked = listening.updates;
  await a.append({ type: "setRecoveryKey", key: null });
  await until(() => listening.updates > looked, "it looked at the record again");
  page.stopSession();
  const stopped = listening.updates;
  await a.append({ type: "setRecoveryKey", key: null });
  await settle();
  assert.equal(listening.updates, stopped, "and no more once it stopped watching");
});

test("#183: where IndexedDB is not to be had, the record stays in localStorage, as before", async () => {
  const hub = new FakeKeysHub();
  const store = new MemoryKeyStore();
  const storage = browserStorage(store);
  const seen = localStorageSeenStore(storage, null);
  const login = await hub.api().login("someone@example.test", "a password");
  const a = await CloudAccount.open(hub.api(), login, store, { origin: HUB_ORIGIN, storage, seen });
  await a.createGenesis(false);
  await a.append({ type: "setRecoveryKey", key: null });
  assert.equal((JSON.parse(storage.getItem(`rh.hub.seen.${a.account}`)!) as StoredSeen).seenThrough, a.chain!.seq);
  const full = hub.chain.slice();
  hub.chain = full.slice(0, -1);
  await assert.rejects(CloudAccount.open(hub.api(), { ...a.session }, store, { origin: HUB_ORIGIN, storage, seen }), isFork, "and it is held against as before");
  hub.chain = full;
});

// ---- #183 re-review: a pin another page made ------------------------------

/** `seen` as a page that hears nothing of other pages' writes has it (a message lost; an older page writing only localStorage). */
function deafSeen(seen: MemorySeenStore): SeenStore {
  return {
    get atomic() {
      return seen.atomic;
    },
    get: (account) => seen.get(account),
    update: (account, change) => seen.update(account, change),
    watch: () => () => undefined,
  };
}

/**
 * A new device's browser with two pages on the pairing screen: the device is
 * paired through `first`, and `second` takes in the chain that names the
 * device just before `first` pins the account (as it takes the keys) — so
 * `second` last read the store before there was a pin in it. `second` opens
 * on `seen` (the browser's store as it is, by default), and `opened` runs as
 * it has opened (its watch, say).
 */
async function pairTwoPages(
  hub: FakeKeysHub,
  a: CloudAccount,
  opts: { seen?: (backing: MemorySeenStore) => SeenStore; opened?: (second: CloudAccount) => void } = {},
): Promise<{ first: CloudAccount; second: CloudAccount }> {
  const store = new RecordingKeyStore();
  const first = await hub.device("tok", store);
  const backing = browserSeen(store);
  const second = await openPage(hub, first, browserStorage(store), null, opts.seen?.(backing) ?? backing);
  opts.opened?.(second);
  const r = new PairingResponder({
    api: first.api,
    token: first.session.token,
    account: first.account,
    device: first.deviceId,
    signPublic: first.keys.signPublic,
    noise: first.noiseKeys,
    chain: async () => (await first.sync(), first.chain),
    storeUserKeys: async (keys) => {
      await second.sync();
      return first.storeUserKeys(keys);
    },
    pollMs: 1,
  });
  try {
    const c = await candidateFromOffer(await r.startOffer(), a.chain!);
    await completePairing(
      {
        api: a.api,
        token: a.session.token,
        device: a.deviceId,
        signPublic: a.keys.signPublic,
        noise: a.noiseKeys,
        chain: () => a.chain!,
        append: (action) => a.append(action),
        withUserKeys: (use) => a.withUserKeys(use),
      },
      c,
    );
    for (let i = 0; i < 10 && r.state.kind !== "paired"; i++) await r.tick();
  } finally {
    r.stop();
  }
  assert.equal(r.state.kind, "paired");
  assert.ok(first.pinned);
  assert.equal(second.standing, "active", "the second page sees its device in the account");
  return { first, second };
}

test("#183 re-review: a page that took the chain naming its device just before another page pinned the account still vouches for nothing behind the record", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await a.createGenesis(false);
  const { first, second } = await pairTwoPages(hub, a);
  // The account goes on; the first page verifies it, and the hub keeps the second where it was.
  const seq = second.chain!.seq;
  await a.append({ type: "setRecoveryKey", key: null });
  await a.append({ type: "setRecoveryKey", key: null });
  await first.sync();
  const full = hub.chain.slice();
  hub.chainServed = async () => full.slice(0, seq + 1);
  await assert.rejects(second.attestation(), isFork, "its fresh word for the old head is what a hub keeping the newer entries from it would want");
  assert.match(second.forked ?? "", new RegExp(`through entry ${seq}, and another page of this browser verified it through ${full.length - 1}$`));
  hub.chainServed = null;
});

test("#183 re-review: an open page takes in a pin another page made — as the store says it was written, or as the page comes back into view — and is not sent to pair again", async (t) => {
  for (const heard of ["store", "visibility", "unwatched"] as const) {
    await t.test(heard, async () => {
      const hub = new FakeKeysHub();
      const a = await hub.device("tok");
      await a.createGenesis(false);
      const documentLike = Object.assign(new EventTarget(), { visibilityState: "visible" });
      const { second } = await pairTwoPages(hub, a, {
        seen: heard === "store" ? undefined : deafSeen,
        opened: heard === "unwatched" ? undefined : (page) => void page.watchSeenRecord({ visibility: documentLike }),
      });
      await settle();
      if (heard === "visibility") {
        documentLike.visibilityState = "hidden";
        documentLike.dispatchEvent(new Event("visibilitychange"));
        documentLike.visibilityState = "visible";
        documentLike.dispatchEvent(new Event("visibilitychange"));
        await settle();
      }
      if (heard !== "unwatched") assert.ok(second.pinned, `the pin taken in (${heard})`);
      // Asked of the store, not of what this page saw last: an honest page is not sent to pair again.
      assert.equal(await second.lacksKeys(), false, `the keys are there (${heard})`);
      assert.ok(second.pinned, `and the pin with them (${heard})`);
      second.stopSession();
    });
  }
});

test("#183 re-review: a store that twice will not take the pin of the genesis this device just made leaves it in localStorage — the first user key is kept — and the next view takes it into the store", async (t) => {
  t.mock.method(console, "warn", () => undefined);
  const hub = new FakeKeysHub();
  const store = new MemoryKeyStore();
  const storage = browserStorage(store);
  const seen = browserSeen(store);
  let failing = 2;
  const flaky: SeenStore = {
    get atomic() {
      return seen.atomic;
    },
    get: (account) => seen.get(account),
    // Each write of a pin fails as long as `failing` lasts (the connection lost, say).
    update: (account, change) =>
      seen.update(account, (stored) => {
        const next = change(stored) as { genesis?: string | null } | undefined;
        if (next?.genesis && failing > 0) {
          failing--;
          throw new DOMException("Connection to Indexed Database server lost. Refresh the page to try again", "UnknownError");
        }
        return next;
      }),
    watch: (heard) => seen.watch(heard),
  };
  const login = await hub.api().login("someone@example.test", "a password");
  const a = await CloudAccount.open(hub.api(), login, store, { origin: HUB_ORIGIN, storage, seen: flaky });
  await a.createGenesis(false);
  const genesis = hex(a.chain!.hashAt(0)!);
  assert.equal(failing, 0, "tried twice");
  assert.ok(a.pinned);
  assert.equal(storage.getItem(`rh.hub.genesis.${a.account}`), genesis, "pinned where older pages keep it");
  assert.equal(storedGenesis(seen, a.account), null, "and not in the store yet");
  assert.equal(await a.lacksKeys(), false, "the account's first user key is kept");
  await a.append({ type: "setRecoveryKey", key: null });
  assert.equal(storedGenesis(seen, a.account), genesis, "the next view takes the pin into the store");
  assert.equal(seenRecord(seen, a.account).seenThrough, a.chain!.seq, "with its record");
  const again = await CloudAccount.open(hub.api(), { ...a.session }, store, { origin: HUB_ORIGIN, storage, seen });
  assert.ok(again.pinned, "a reload trusts it");
  assert.equal(again.chain!.seq, a.chain!.seq);
});

// ---- #183 third review ------------------------------------------------------

test("#183 third review: what a page records is written where older pages read it too, so one of them that a hub keeps behind vouches for nothing", async () => {
  const { hub, a } = await setup();
  await a.sync();
  const storage = browserStorage(a.store);
  // A page of this browser that keeps the record in localStorage (an older version of the app; one where IndexedDB would not open).
  const older = await openPage(hub, a, storage, null, localStorageSeenStore(storage, null));
  const seq = older.chain!.seq;
  await a.append({ type: "setRecoveryKey", key: null });
  await a.append({ type: "setRecoveryKey", key: null });
  // The hub keeps the newer entries from the older page.
  const full = hub.chain.slice();
  hub.chainServed = async () => full.slice(0, seq + 1);
  await assert.rejects(older.attestation(), isFork, "not its word for the old head");
  hub.chainServed = null;
  assert.equal((JSON.parse(storage.getItem(`rh.hub.seen.${a.account}`)!) as StoredSeen).seenThrough, a.chain!.seq, "the record raised where older pages read it");
});

test("#183 third review: what a page records where older pages read it never lowers theirs, nor replaces one it cannot prove is the same chain", async (t) => {
  await t.test("higher, not proven", async () => {
    const { a } = await setup();
    await a.sync();
    const storage = browserStorage(a.store);
    const key = `rh.hub.seen.${a.account}`;
    // An older page went further than this page (no hash at this page's height to prove it the same chain): kept apart.
    const higher: StoredSeen = { v: 1, seenThrough: a.chain!.seq + 5, head: "ee".repeat(32), heads: [], firstSeen: [] };
    storage.setItem(key, JSON.stringify(higher));
    a.watchSeenRecord({});
    await settle();
    assert.equal(a.forked, null);
    assert.equal(storage.getItem(key), JSON.stringify(higher), "left as it is");
  });
  await t.test("lower, not proven", async () => {
    const { a } = await setup();
    await a.sync();
    const seen = browserSeen(a.store);
    const storage = browserStorage(a.store);
    const key = `rh.hub.seen.${a.account}`;
    const seq = a.chain!.seq;
    // The store's record from before `heads`, and an older page's lower one: nothing proves them one chain, so that one is kept apart.
    setSeenRecord(seen, a.account, { v: 1, seenThrough: seq, head: hex(a.chain!.head), firstSeen: [] });
    const lower: StoredSeen = { v: 1, seenThrough: seq - 1, head: hex(a.chain!.hashAt(seq - 1)!), firstSeen: [] };
    storage.setItem(key, JSON.stringify(lower));
    a.watchSeenRecord({});
    await settle();
    assert.equal(a.forked, null);
    assert.equal(storage.getItem(key), JSON.stringify(lower), "not replaced: it is a checkpoint of its own");
  });
});

test("#183 third review: a page whose newer view could not be recorded vouches for nothing — not for the older head it still shows — until it takes that view", async () => {
  const { hub, a, b } = await setup();
  await a.sync();
  const seen = browserSeen(a.store);
  let failing = false;
  const flaky: SeenStore = {
    get atomic() {
      return seen.atomic;
    },
    get: (account) => seen.get(account),
    // Every commit fails after the compare (a full disk, say).
    update: (account, change) =>
      seen.update(account, (stored) => {
        const next = change(stored);
        if (failing && next !== undefined) throw new DOMException("the disk is full", "QuotaExceededError");
        return next;
      }),
    watch: (heard) => seen.watch(heard),
  };
  const page = await openPage(hub, a, browserStorage(a.store), null, flaky);
  const seq = page.chain!.seq;
  await b.append({ type: "setRecoveryKey", key: null });
  failing = true;
  await assert.rejects(page.sync(), (e: unknown) => e instanceof SeenStoreError);
  assert.equal(page.chain!.seq, seq, "the newer view verified, and not taken");
  await assert.rejects(page.attestation(), (e: unknown) => e instanceof SeenStoreError, "no fresh word for the older head");
  assert.equal(page.forked, null, "and no fork: it is tried again");
  failing = false;
  const vouched = await page.attestation();
  assert.equal(vouched.seq, seq + 1, "once the store takes the record, it takes the view and vouches for that");
});

test("#183 third review: a page whose chain does not start with the genesis this browser pinned, or contradicts the record at its own head, vouches for nothing: a fork", async (t) => {
  await t.test("another genesis pinned", async () => {
    const { a } = await setup();
    await a.sync();
    // What no page of this browser writes over a pin (code in this origin, say).
    const other = "ab".repeat(32);
    browserSeen(a.store).values.set(a.account, { v: 1, genesis: other, seen: null });
    browserStorage(a.store).setItem(`rh.hub.genesis.${a.account}`, other);
    browserStorage(a.store).removeItem(`rh.hub.seen.${a.account}`);
    await assert.rejects(a.attestation(), isFork);
  });
  await t.test("another head at this page's height", async () => {
    const { a } = await setup();
    await a.sync();
    const seq = a.chain!.seq;
    setSeenRecord(browserSeen(a.store), a.account, { v: 1, seenThrough: seq, head: "cd".repeat(32), heads: [], firstSeen: [] });
    browserStorage(a.store).removeItem(`rh.hub.seen.${a.account}`);
    await assert.rejects(a.attestation(), isFork);
  });
});

test("#183 third review: a sync that takes in more entries than the record keeps hashes of still raises the record where older pages read it", async () => {
  const { hub, a, b } = await setup();
  await a.sync();
  const storage = browserStorage(a.store);
  const key = `rh.hub.seen.${a.account}`;
  // A page of this browser that keeps the record in localStorage (an older version of the app).
  const older = await openPage(hub, a, storage, null, localStorageSeenStore(storage, null));
  const seq = older.chain!.seq;
  assert.equal((JSON.parse(storage.getItem(key)!) as StoredSeen).seenThrough, seq);
  // Another device goes on by more than the record keeps hashes of; this page takes it all in one view.
  for (let i = 0; i < 300; i++) await b.append({ type: "setRecoveryKey", key: null });
  await a.sync();
  assert.equal(a.chain!.seq, seq + 300);
  // The hub keeps the newer entries from the older page.
  const full = hub.chain.slice();
  hub.chainServed = async () => full.slice(0, seq + 1);
  await assert.rejects(older.attestation(), isFork, "not its word for the old head");
  hub.chainServed = null;
  assert.equal((JSON.parse(storage.getItem(key)!) as StoredSeen).seenThrough, a.chain!.seq, "the view that holds both proved them one chain");
});
