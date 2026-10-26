// The numeric code's window and the ways a pairing stops (`lib/hub/pairing.ts`,
// `CloudAccount.open`, `features/cloud/sign-out.ts`, `features/cloud/ui.tsx`),
// against a fake hub that keeps the window as rh-hub does (a device outside
// the chain opens a code only within 30 minutes of registering).
//
// Pinned: every refusal the hub can give a pairing comes out as a cause the
// screen has words for (never the hub's message); a device past its window
// is told so; "sign in again" forgets an unjoined device's id and the next
// sign-in registers this browser afresh, with the same keys and a window
// open again — also a plain sign-in once the window lapsed, or once the hub
// forgot the device — while a device the chain names is never registered
// afresh; and the hub's refusals elsewhere read in words, in the reader's
// language, with the code (not the message) for anything unknown.
import assert from "node:assert/strict";
import test from "node:test";
import type { CloudAccount } from "../src/lib/hub/account.js";
import { CODE_PAIRING_WINDOW_MS, HubApiError } from "../src/lib/hub/api.js";
import { pairingCause, PairingResponder, type ResponderState } from "../src/lib/hub/pairing.js";
import { clearPairingResume, PAIRING_RESUME_KEY, pairingResume, signInAgainToPair } from "../src/features/cloud/sign-out.js";
import { browserStorage, FakeKeysHub } from "./fake-keys-hub.js";

const made: PairingResponder[] = [];
test.afterEach(() => {
  for (const r of made.splice(0)) r.stop();
});

function responderFor(b: CloudAccount, now?: () => number): PairingResponder {
  const r = new PairingResponder({
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
    now,
  });
  made.push(r);
  return r;
}

async function until(r: PairingResponder, done: (s: ResponderState) => boolean, rounds = 20): Promise<ResponderState> {
  for (let i = 0; i < rounds && !done(r.state); i++) await r.tick();
  return r.state;
}

/** An account with its first device `a`, and a browser `b` signed in to it but not in its chain. */
async function twoDevices() {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await a.createGenesis(true);
  const b = await hub.device("tok");
  assert.equal(b.standing, "absent");
  return { hub, a, b };
}

/** Move a device's registration `ms` into the past. */
const age = (hub: FakeKeysHub, device: string, ms: number) => hub.registeredMs.set(device, hub.now() - ms);

class MemoryStorage {
  values = new Map<string, string>();
  get length() {
    return this.values.size;
  }
  key(i: number) {
    return [...this.values.keys()][i] ?? null;
  }
  getItem(k: string) {
    return this.values.get(k) ?? null;
  }
  setItem(k: string, v: string) {
    this.values.set(k, String(v));
  }
  removeItem(k: string) {
    this.values.delete(k);
  }
  clear() {
    this.values.clear();
  }
}

test("every refusal the hub gives a pairing is a cause the screen has words for", () => {
  const e = (status: number, code: string, message = "the hub's own words") => new HubApiError(status, code, message);
  assert.equal(pairingCause(e(403, "pairing_window_closed")), "windowClosed");
  assert.equal(pairingCause(e(429, "too_many_pairings")), "tooMany");
  assert.equal(pairingCause(e(404, "not_found")), "expired");
  assert.equal(pairingCause(e(429, "pairing_full")), "full");
  assert.equal(pairingCause(e(429, "rate")), "rate");
  assert.equal(pairingCause(e(503, "unavailable")), "unavailable");
  assert.equal(pairingCause(e(500, "internal")), "unavailable");
  assert.equal(pairingCause(e(502, "error")), "unavailable", "a proxy in front of the hub");
  assert.equal(pairingCause(new TypeError("Failed to fetch")), "offline", "Chrome");
  assert.equal(pairingCause(new TypeError("Load failed")), "offline", "Safari");
  assert.equal(pairingCause(new TypeError("NetworkError when attempting to fetch resource.")), "offline", "Firefox");
  assert.equal(pairingCause(e(400, "bad_request")), "other");
  assert.equal(pairingCause(new TypeError("x is not a function")), "other", "a bug is not the network");
});

test("a device past its window: the code is refused as `windowClosed`, the hub's words kept for the console only", async () => {
  const { hub, b } = await twoDevices();
  age(hub, b.deviceId, CODE_PAIRING_WINDOW_MS + 1000);
  const r = responderFor(b);
  await assert.rejects(r.startCode(), (e: unknown) => e instanceof HubApiError && e.code === "pairing_window_closed");
  assert.equal(r.state.kind, "failed");
  const failed = r.state as Extract<ResponderState, { kind: "failed" }>;
  assert.equal(failed.cause, "windowClosed");
  assert.equal(failed.reason, "error");
  assert.match(failed.message, /too long ago/, "the hub's message, for the console");
  assert.equal(hub.pairings.size, 0);
  // Within the window it opens.
  age(hub, b.deviceId, CODE_PAIRING_WINDOW_MS - 60_000);
  const fresh = responderFor(b);
  assert.match(await fresh.startCode(), /^\d{8}$/);
  const shown = fresh.state as Extract<ResponderState, { kind: "code" }>;
  assert.equal(shown.kind, "code");
  assert.ok(Math.abs(shown.expiresMs - (Date.now() + 10 * 60_000)) < 5_000, "it lapses ten minutes after it opened");
});

test("a code that lapses before anyone types it is `expired`; one too many open in the account is `tooMany`", async () => {
  const { hub, b } = await twoDevices();
  const r = responderFor(b);
  await r.startCode();
  hub.pairings.clear(); // lapsed at the hub
  const state = await until(r, (s) => s.kind === "failed");
  assert.equal((state as { cause: string }).cause, "expired");
  // Four codes open from other new devices: the account's few are taken.
  for (let i = 0; i < 4; i++) hub.pairings.set(`p-other-${i}`, { device: `d-other-${i}`, created_ms: hub.now(), messages: [] });
  const next = responderFor(b);
  await assert.rejects(next.startCode());
  assert.equal((next.state as { cause: string }).cause, "tooMany");
});

test("a moment offline is waited through; offline for longer stops the pairing as `offline`", async () => {
  const { b } = await twoDevices();
  let clock = Date.now();
  const r = responderFor(b, () => clock);
  await r.startOffer();
  const real = b.api.mailbox.bind(b.api);
  let down = true;
  b.api.mailbox = async (token: string) => {
    if (down) throw new TypeError("Failed to fetch");
    return real(token);
  };
  await new Promise((resolve) => setTimeout(resolve, 30));
  assert.equal(r.state.kind, "offer", "still waiting, its QR code still good");
  down = false;
  await new Promise((resolve) => setTimeout(resolve, 10));
  down = true;
  await new Promise((resolve) => setTimeout(resolve, 10));
  clock += 61_000;
  await new Promise((resolve) => setTimeout(resolve, 30));
  assert.equal(r.state.kind, "failed");
  assert.equal((r.state as { cause: string }).cause, "offline");
});

test("sign in again: the next sign-in registers this browser afresh — same keys, a window open again — and only then leaves the old id", async () => {
  const { hub, a, b } = await twoDevices();
  const old = b.deviceId;
  age(hub, old, CODE_PAIRING_WINDOW_MS + 60_000);
  const before = responderFor(b);
  await assert.rejects(before.startCode());
  assert.equal((before.state as { cause: string }).cause, "windowClosed");
  // The button: ask for a new device at the next sign-in, remember the tab, sign out.
  const storage = new MemoryStorage() as unknown as Storage;
  let signedOut: CloudAccount | null = null;
  await signInAgainToPair(b, "code", { storage, signOut: async (account) => void (signedOut = account) });
  assert.equal(signedOut, b);
  const kept = browserStorage(b.store);
  assert.equal(kept.getItem(`rh.hub.device.${b.account}`), old, "nothing forgotten yet");
  assert.equal(kept.getItem(`rh.hub.afresh.${b.account}`), old);
  assert.equal(pairingResume(storage), "code", "the sign-in screen says why, and the pairing screen opens on its code");
  assert.equal(storage.getItem(PAIRING_RESUME_KEY), "code");
  // Signed in again: a new device of the same browser.
  const registered = hub.registrations.length;
  const fresh = await hub.device("tok", b.store);
  assert.notEqual(fresh.deviceId, old, "registered afresh");
  assert.equal(hub.registrations.length, registered + 1);
  assert.deepEqual([...fresh.keys.signPublic], [...b.keys.signPublic], "with this browser's keys");
  assert.equal(fresh.standing, "absent", "back on the pairing screen");
  assert.equal(kept.getItem(`rh.hub.device.${b.account}`), fresh.deviceId);
  assert.equal(kept.getItem(`rh.hub.afresh.${b.account}`), null, "asked once");
  // …whose window is open: a code, typed on the first device, adds it.
  const r = responderFor(fresh);
  const code = await r.startCode();
  const { completePairing, runCodeInitiator } = await import("../src/lib/hub/pairing.js");
  const [pairing] = await a.api.pairings(a.session.token);
  const answering = until(r, (s) => s.kind === "answered" || s.kind === "failed");
  const c = await runCodeInitiator({ api: a.api, token: a.session.token, account: a.account, pollMs: 1 }, pairing!, code);
  await answering;
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
  const done = await until(r, (s) => s.kind === "paired" || s.kind === "failed");
  assert.equal(done.kind, "paired");
  assert.equal(a.chain!.deviceRecord(old), undefined, "the old one never joined");
  clearPairingResume(storage);
  assert.equal(pairingResume(storage), null);
});

test("a plain sign-in registers an unjoined device afresh only once its window has lapsed (or nearly)", async () => {
  const { hub, b } = await twoDevices();
  const old = b.deviceId;
  const registered = hub.registrations.length;
  // Within the window: the same device, nothing registered.
  const same = await hub.device("tok", b.store);
  assert.equal(same.deviceId, old);
  assert.equal(hub.registrations.length, registered);
  // A reload (its device session kept, no sign-in) never registers anything.
  age(hub, old, CODE_PAIRING_WINDOW_MS + 60_000);
  const reloaded = await hub.reload(same);
  assert.equal(reloaded.deviceId, old);
  assert.equal(hub.registrations.length, registered);
  // A sign-in past the window: a new device.
  const later = await hub.device("tok", b.store);
  assert.notEqual(later.deviceId, old);
  assert.equal(hub.registrations.length, registered + 1);
  // Seconds before it shuts counts as shut.
  age(hub, later.deviceId, CODE_PAIRING_WINDOW_MS - 30_000);
  assert.notEqual((await hub.device("tok", b.store)).deviceId, later.deviceId);
});

test("the hub forgot an unjoined device (a day): a sign-in registers it afresh instead of being refused", async () => {
  const { hub, b } = await twoDevices();
  const old = b.deviceId;
  hub.devices.delete(old);
  hub.registeredMs.delete(old);
  const back = await hub.device("tok", b.store);
  assert.equal(back.refusal, null);
  assert.notEqual(back.deviceId, old);
  assert.equal(back.standing, "absent");
});

test("past its window, a sign-in the hub will not register a new device for keeps the old one — its QR code still works; once it may, what the old one noted goes with it", async () => {
  const { hub, b } = await twoDevices();
  const old = b.deviceId;
  age(hub, old, CODE_PAIRING_WINDOW_MS + 60_000);
  const noted = browserStorage(b.store);
  noted.setItem(`rh.hub.ack.${b.account}.${old}`, "{}");
  noted.setItem(`rh.hub.keyreq.${b.account}.${old}`, "{}");
  hub.refuseRegistrations = "device_rate";
  const registered = hub.registrations.length;
  const kept = await hub.device("tok", b.store);
  assert.equal(kept.deviceId, old, "not signed out of it for want of a new one");
  assert.equal(kept.refusal, null);
  assert.equal(kept.standing, "absent", "on the pairing screen");
  assert.equal(hub.registrations.length, registered);
  assert.match(await responderFor(kept).startOffer(), /\S/, "its QR code");
  await assert.rejects(responderFor(kept).startCode(), (e: unknown) => e instanceof HubApiError && e.code === "pairing_window_closed", "no code: the screen says sign in again");
  // The next sign-in that may: a new device, and nothing kept under the old id.
  hub.refuseRegistrations = null;
  const fresh = await hub.device("tok", b.store);
  assert.notEqual(fresh.deviceId, old);
  assert.equal(hub.registrations.length, registered + 1);
  assert.deepEqual(
    [...noted.values.keys()].filter((k) => k.endsWith(`.${old}`) || noted.values.get(k) === old),
    [],
    "the old id, and what it noted, forgotten",
  );
});

test("the hub forgot an unjoined device and will not register a new one: the sign-in fails with the hub's reason, not as a refusal", async () => {
  const { hub, b } = await twoDevices();
  hub.devices.delete(b.deviceId);
  hub.registeredMs.delete(b.deviceId);
  hub.refuseRegistrations = "too_many_pending_devices";
  await assert.rejects(hub.device("tok", b.store), (e: unknown) => e instanceof HubApiError && e.code === "too_many_pending_devices");
});

test("a device the chain names is never registered afresh, nor its id forgotten", async () => {
  const { hub, a } = await twoDevices();
  age(hub, a.deviceId, 2 * 24 * 60 * 60_000);
  const registered = hub.registrations.length;
  const back = await hub.device("tok", a.store);
  assert.equal(back.deviceId, a.deviceId);
  assert.equal(back.standing, "active");
  assert.equal(hub.registrations.length, registered);
  assert.equal(back.registerAfreshOnSignIn(), false);
  assert.equal(browserStorage(a.store).getItem(`rh.hub.afresh.${a.account}`), null);
  assert.equal((await hub.device("tok", a.store)).deviceId, a.deviceId, "still the same device");
});

/** `a` reads `b`'s QR code and adds it (the existing device's side of the pairing). */
async function addFromQr(a: CloudAccount, text: string): Promise<void> {
  const { candidateFromOffer, completePairing } = await import("../src/lib/hub/pairing.js");
  const c = await candidateFromOffer(text, a.chain!);
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
}

const signOutHere = { signOut: async () => undefined };

test("sign in again within the window (the hub shut it by its own clock): the button alone has the next sign-in register afresh", async () => {
  const { hub, b } = await twoDevices();
  const old = b.deviceId;
  age(hub, old, 5 * 60_000);
  await signInAgainToPair(b, "code", { storage: new MemoryStorage() as unknown as Storage, ...signOutHere });
  const registered = hub.registrations.length;
  const fresh = await hub.device("tok", b.store);
  assert.notEqual(fresh.deviceId, old, "registered afresh, though its window was open");
  assert.equal(hub.registrations.length, registered + 1);
  assert.equal(fresh.standing, "absent");
  // Asked once: the sign-in after that is the same device.
  assert.equal((await hub.device("tok", b.store)).deviceId, fresh.deviceId);
  assert.equal(hub.registrations.length, registered + 1);
});

test("sign in again, then added from its QR code before signing in: the chain names it, so it keeps its id and is asked nothing", async () => {
  const { hub, a, b } = await twoDevices();
  const old = b.deviceId;
  age(hub, old, CODE_PAIRING_WINDOW_MS + 60_000);
  const shown = await responderFor(b).startOffer();
  await signInAgainToPair(b, "code", { storage: new MemoryStorage() as unknown as Storage, ...signOutHere });
  // The other device reads the QR code still on screen, meanwhile.
  await addFromQr(a, shown);
  assert.ok(a.chain!.deviceRecord(old), "in the chain now");
  const registered = hub.registrations.length;
  const back = await hub.device("tok", b.store);
  assert.equal(back.deviceId, old, "the device the chain names: its id kept");
  assert.notEqual(back.standing, "absent");
  assert.equal(hub.registrations.length, registered, "no new device");
  assert.equal(browserStorage(b.store).getItem(`rh.hub.afresh.${b.account}`), null, "and the ask forgotten");
});

test("sign in again, and the hub will not register a new device: the sign-in goes on as the old one (its QR code still works), and asks again next time", async () => {
  const { hub, b } = await twoDevices();
  const old = b.deviceId;
  age(hub, old, 5 * 60_000);
  await signInAgainToPair(b, "qr", { storage: new MemoryStorage() as unknown as Storage, ...signOutHere });
  hub.refuseRegistrations = "too_many_pending_devices";
  const registered = hub.registrations.length;
  const kept = await hub.device("tok", b.store);
  assert.equal(kept.deviceId, old, "its id was never forgotten");
  assert.equal(kept.refusal, null);
  assert.equal(kept.standing, "absent");
  assert.match(await responderFor(kept).startOffer(), /\S/, "its QR code");
  assert.equal(browserStorage(b.store).getItem(`rh.hub.afresh.${b.account}`), old, "still asked for");
  hub.refuseRegistrations = null;
  const fresh = await hub.device("tok", b.store);
  assert.notEqual(fresh.deviceId, old);
  assert.equal(hub.registrations.length, registered + 1);
});

test("the hub's refusals read in words, in the reader's language — never its message; an unknown one by its code", async () => {
  const { default: i18n } = await import("./i18n-all.js");
  const { errorText } = await import("../src/features/cloud/ui.js");
  const e = (status: number, code: string) => new HubApiError(status, code, `RAW HUB MESSAGE ${code}`);
  const cases: [HubApiError | TypeError, string, string][] = [
    [e(429, "device_rate"), "this account added as many new devices as it may today: try again tomorrow", "这个账户今天添加的新设备已达上限，请明天再试"],
    [
      e(429, "too_many_pending_devices"),
      "too many new devices are waiting to be added to this account: add them from a device already in it, or try again in 30 minutes",
      "等待加入这个账户的新设备太多了：请先在已加入的设备上添加它们，或 30 分钟后再试",
    ],
    [e(429, "too_many_enrollments"), "too many devices are being added at once: finish adding them, or let them lapse, then try again", "同时在添加的设备太多了：请先完成添加，或等它们失效后再试"],
    [e(403, "pairing_window_closed"), "this browser signed in too long ago to make a code: sign in again, or use the QR code", "这个浏览器登录太久，不能再获取数字码：请重新登录，或改用二维码"],
    [e(429, "too_many_pairings"), "too many codes are open in this account: try again in 10 minutes, or use the QR code", "这个账户打开的数字码太多了：请 10 分钟后再试，或改用二维码"],
    [e(429, "pairing_full"), "this code was tried too often: start again for a new one", "这个数字码尝试次数过多：请重新开始获取新的"],
    [e(503, "unavailable"), "Droppen is not answering properly right now: try again in a moment", "Droppen 暂时无法正常响应，请稍后再试"],
    [e(500, "internal"), "Droppen is not answering properly right now: try again in a moment", "Droppen 暂时无法正常响应，请稍后再试"],
    [new TypeError("Failed to fetch"), "Droppen could not be reached: check the connection and try again", "连不上 Droppen：请检查网络连接后再试"],
    [e(422, "invalid_entry"), "Droppen could not do that (invalid_entry)", "Droppen 没能完成这一步（invalid_entry）"],
  ];
  for (const [error, en, zh] of cases) {
    assert.equal(errorText(error), en);
    assert.doesNotMatch(errorText(error), /RAW HUB MESSAGE/);
  }
  await i18n.changeLanguage("zh-CN");
  try {
    for (const [error, , zh] of cases) assert.equal(errorText(error), zh);
  } finally {
    await i18n.changeLanguage("en");
  }
  // A limit the page has no words for is the exception: the hub's sentence names it (cloud-plan.test.tsx).
  const limit = new HubApiError(409, "limit", "free accounts use at most 2 relays", { limit: { key: "relays.max", limit: 2, used: 2 } });
  assert.equal(errorText(limit), "free accounts use at most 2 relays");
});
