// The device a browser takes the place of (`features/cloud/devices.tsx`
// `retireOffer`, `lib/hub/account.ts` registration): a browser that lost its
// keys registers as a new device naming the old one (`replaces`, its claim),
// and the device that adds the new one is offered the old one's removal —
// only a browser or phone the chain still lists, by the same browser name as
// the new one, whose removal leaves the recovery words alone; never this
// device or the new one, never one signed in now or within the last minutes.
// Only ever an offer: the removal is the person's to confirm
// (`RetireReplacedPrompt`, rendered in `retire-replaced-render.test.tsx`).
import assert from "node:assert/strict";
import test from "node:test";
import { forgetDeviceKeys, MemoryKeyStore } from "../src/lib/e2e/index.js";
import { CloudAccount } from "../src/lib/hub/account.js";
import type { RegisteredDevice } from "../src/lib/hub/api.js";
import { type Candidate, candidateFromOffer, PairingResponder } from "../src/lib/hub/pairing.js";
import { IN_USE_MS, retireOffer } from "../src/features/cloud/devices.js";
import { browserOf, FakeCourier, FakeKeysHub, HUB_ORIGIN } from "./fake-keys-hub.js";

const PHONE = "iPhone · Chrome";

/** `account` adds `other` to the chain as a full device (its pairing delivery mailed, as a real pairing would). */
async function add(account: CloudAccount, other: CloudAccount): Promise<void> {
  const candidate: Candidate = {
    device: other.deviceId,
    signKey: other.keys.signPublic,
    dhKey: other.keys.dhPublic,
    secret: crypto.getRandomValues(new Uint8Array(32)),
    fingerprint: "",
  };
  await account.pairDevice(candidate, { admin: true });
}

/** `b` paired from `a` by its QR code, both ends run: it holds the user keys afterwards. */
async function pair(a: CloudAccount, b: CloudAccount): Promise<void> {
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
    await a.pairDevice(await candidateFromOffer(await r.startOffer(), a.chain!), { admin: true });
    for (let i = 0; i < 10 && r.state.kind !== "paired"; i++) await r.tick();
    assert.equal(r.state.kind, "paired", JSON.stringify(r.state));
  } finally {
    r.stop();
  }
  await b.sync();
}

/**
 * A computer's account with a phone in it, whose browser then loses the
 * phone's keys and signs in again: `phone` was replaced by `fresh`, the same
 * browser (`ua_label`). `before`: what the phone does while it still can.
 */
async function lostPhone(before?: (phone: CloudAccount) => Promise<unknown>) {
  const hub = new FakeKeysHub();
  const computer = await hub.device("tok");
  await computer.createGenesis(true);
  const store = new MemoryKeyStore();
  const phone = await hub.device("tok", store);
  await pair(computer, phone);
  if (before) await before(phone);
  await computer.sync();
  // The phone's browser loses its keys; its next sign-in registers a new device.
  await forgetDeviceKeys(store);
  const login = await hub.api().login("someone@example.test", "a password");
  const fresh = await CloudAccount.open(hub.api(), login, store, { origin: HUB_ORIGIN, ...browserOf(store) });
  hub.uaLabels.set(phone.deviceId, PHONE);
  hub.uaLabels.set(fresh.deviceId, PHONE);
  await add(computer, fresh);
  await computer.sync();
  return { hub, computer, phone: phone.deviceId, fresh };
}

const devicesOf = async (hub: FakeKeysHub, account: CloudAccount): Promise<RegisteredDevice[]> => (await hub.api().me(account.session.token)).devices;

test("a browser that lost a device's keys registers its new device as taking that one's place", async () => {
  const { hub, phone, fresh } = await lostPhone();
  assert.notEqual(fresh.deviceId, phone);
  assert.equal(fresh.lostDevice, phone);
  assert.equal(hub.replaces.get(fresh.deviceId), phone, "registered with `replaces`");
  // A browser that lost nothing names nothing.
  const other = await hub.device("tok");
  assert.equal(hub.replaces.has(other.deviceId), false);
});

test("the device that adds the new one is offered the old one's removal: only while the old one is not in use", async () => {
  const { hub, computer, phone, fresh } = await lostPhone();
  const now = Date.now();
  const offer = (devices: RegisteredDevice[], self = computer.deviceId) => retireOffer({ chain: computer.chain, devices, added: fresh.deviceId, self, nowMs: now })?.id ?? null;
  const devices = await devicesOf(hub, computer);
  assert.equal(devices.find((d) => d.id === fresh.deviceId)?.replaces, phone, "/v1/me says which device it replaces");
  assert.equal(offer(devices), phone, "offered");
  assert.deepEqual(retireOffer({ chain: computer.chain, devices, added: fresh.deviceId, self: computer.deviceId, nowMs: now }), { id: phone, label: PHONE, addedMs: devices.find((d) => d.id === phone)!.createdMs, lastSeenMs: null }, "with what tells it apart");

  // Signed in now: never.
  hub.online.add(phone);
  assert.equal(offer(await devicesOf(hub, computer)), null, "online");
  hub.online.delete(phone);
  // Signed in within the last minutes: never.
  const recent = devices.map((d) => (d.id === phone ? { ...d, lastSeenMs: now - IN_USE_MS / 2 } : d));
  assert.equal(offer(recent), null, "seen moments ago");
  const long = devices.map((d) => (d.id === phone ? { ...d, lastSeenMs: now - IN_USE_MS * 4 } : d));
  assert.equal(offer(long), phone, "seen long ago");
  // Never this device (the one asking), never the new one, never one the chain does not list.
  assert.equal(offer(devices.map((d) => (d.id === fresh.deviceId ? { ...d, replaces: computer.deviceId } : d))), null, "this device");
  assert.equal(offer(devices.map((d) => (d.id === fresh.deviceId ? { ...d, replaces: fresh.deviceId } : d))), null, "the new one itself");
  assert.equal(offer(devices.map((d) => (d.id === fresh.deviceId ? { ...d, replaces: "d-unlisted" } : d))), null, "not in the chain");
  assert.equal(offer(devices.map((d) => (d.id === fresh.deviceId ? { ...d, replaces: null } : d))), null, "replaces nothing");
  assert.equal(retireOffer({ chain: null, devices, added: fresh.deviceId, self: computer.deviceId, nowMs: now }), null, "no chain");
});

test("only the same browser: a new device whose browser differs from the old one's (or is unknown) offers nothing", async () => {
  const { hub, computer, fresh } = await lostPhone();
  const offer = async () => retireOffer({ chain: computer.chain, devices: await devicesOf(hub, computer), added: fresh.deviceId, self: computer.deviceId, nowMs: Date.now() });
  assert.ok(await offer(), "the same browser: offered");
  hub.uaLabels.set(fresh.deviceId, "Windows · Edge");
  assert.equal(await offer(), null, "another browser");
  hub.uaLabels.delete(fresh.deviceId);
  assert.equal(await offer(), null, "the new one's browser unknown");
});

test("a device whose removal would void the recovery words is never offered: the list's own removal says so", async () => {
  const { hub, computer, phone, fresh } = await lostPhone((p) => p.setRecoveryWords());
  assert.equal(computer.chain?.voidsRecoveryWords(phone, "removal"), true, "the phone set the recovery words");
  assert.equal(retireOffer({ chain: computer.chain, devices: await devicesOf(hub, computer), added: fresh.deviceId, self: computer.deviceId, nowMs: Date.now() }), null);
});

test("once the old device is removed it is offered no more", async () => {
  const { hub, computer, phone, fresh } = await lostPhone();
  assert.equal(retireOffer({ chain: computer.chain, devices: await devicesOf(hub, computer), added: fresh.deviceId, self: computer.deviceId, nowMs: Date.now() })?.id, phone);
  await computer.removeDevice(phone, new FakeCourier());
  assert.equal(computer.chain?.device(phone), undefined, "removed by the ordinary removal");
  assert.equal(retireOffer({ chain: computer.chain, devices: await devicesOf(hub, computer), added: fresh.deviceId, self: computer.deviceId, nowMs: Date.now() }), null);
});
