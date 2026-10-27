// Relays on the page (`lib/hub/relays.ts`, `lib/hub/relay-link.ts`,
// `features/cloud/relays.tsx`; relay-and-plans §2.6, §2.14).
//
// What these pin: the automatic choice scores this device's round trip plus
// the machine's, adds half for a loaded relay, keeps the current relay unless
// another is 20% and 15 ms better, and keeps to the account's set when it is
// full (an emergency only when none of the set works) — but not to members
// the Hub filled in itself, which no device used; a manual choice holds
// until the relay leaves the list or fails three times, then the page goes
// back to automatic and says why; the median of a ping drops the first
// sample; a relay connection signs in with a relayDevice signature over the
// relay's own origin, routes frames by ch, measures its round trip with
// ping/pong, tells channels it is going away, and closes them when it drops;
// and the picker greys a third relay on a full free account.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { chooseAuto, medianAfterFirst, RelayPool, type RelayDirectory } from "../src/lib/hub/relays.js";
import { RelayLink } from "../src/lib/hub/relay-link.js";
import { RelaySection } from "../src/features/cloud/relays.js";
import { decodeFrame, encodeFrame, fromHex, generateDeviceKeys, hex, verifyHubLogin } from "../src/lib/e2e/index.js";
import { makeI18n } from "./render.js";

const NOW = 1_760_000_000_000;
const DAY = 24 * 60 * 60 * 1000;

/** Wait for `cond`; `ms` only keeps a broken test from hanging (it is no claim about how fast). */
async function until(cond: () => boolean, what: string, ms = 10_000): Promise<void> {
  const deadline = Date.now() + ms;
  while (!cond()) {
    if (Date.now() > deadline) throw new Error(`timed out: ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 2));
  }
}

function dir(over: Partial<RelayDirectory> = {}): RelayDirectory {
  return {
    relays: [
      { id: "sin-1", city: "singapore", region: "sin", origin: "wss://sin-1.example", state: "up", load: 0.1 },
      { id: "nrt-1", city: "tokyo", region: "nrt", origin: "wss://nrt-1.example", state: "up", load: 0.1 },
      { id: "fra-1", city: "frankfurt", region: "fra", origin: "wss://fra-1.example", state: "up", load: 0.1 },
    ],
    limit: 2,
    accountSet: [],
    emergency: [],
    agents: {},
    throttled: false,
    ...over,
  };
}

const rtts = (m: Record<string, number>) => (id: string) => m[id] ?? null;

test("automatic: the lowest device-plus-machine round trip, half again for a loaded relay", () => {
  const d = dir({ agents: { "m-1": { "sin-1": 70, "nrt-1": 9, "fra-1": 240 } } });
  const pick = (deviceRtt: (id: string) => number | null, directory = d) =>
    chooseAuto({ directory, machine: "m-1", deviceRtt, usable: () => true, current: null, now: NOW })?.relay.id;
  assert.equal(pick(rtts({ "sin-1": 80, "nrt-1": 20, "fra-1": 250 })), "nrt-1");
  // Tokyo loaded: 29 × 1.5 = 43.5 still beats Singapore's 150.
  const loaded = dir({ ...d, relays: d.relays.map((r) => (r.id === "nrt-1" ? { ...r, load: 0.9 } : r)) });
  assert.equal(pick(rtts({ "sin-1": 80, "nrt-1": 20, "fra-1": 250 }), loaded), "nrt-1");
  // Without the machine's report, only this device's.
  assert.equal(
    chooseAuto({ directory: dir(), machine: "m-9", deviceRtt: rtts({ "sin-1": 30, "nrt-1": 50, "fra-1": 200 }), usable: () => true, current: null, now: NOW })?.relay.id,
    "sin-1",
  );
  // Down and unusable ones are never chosen; nothing usable: none.
  const down = dir({ relays: dir().relays.map((r) => (r.id === "nrt-1" ? { ...r, state: "down" } : r)) });
  assert.equal(pick(rtts({ "sin-1": 80, "nrt-1": 20, "fra-1": 250 }), down), "sin-1");
  assert.equal(chooseAuto({ directory: d, machine: null, deviceRtt: () => 1, usable: () => false, current: null, now: NOW }), null);
});

test("automatic: the current relay stays unless another is 20% and 15 ms better", () => {
  const d = dir();
  const choose = (m: Record<string, number>, current: string) =>
    chooseAuto({ directory: d, machine: null, deviceRtt: rtts(m), usable: () => true, current, now: NOW })?.relay.id;
  assert.equal(choose({ "sin-1": 50, "nrt-1": 41 }, "sin-1"), "sin-1", "18% better: stays");
  assert.equal(choose({ "sin-1": 50, "nrt-1": 39 }, "sin-1"), "sin-1", "22% but 11 ms: stays");
  assert.equal(choose({ "sin-1": 100, "nrt-1": 70 }, "sin-1"), "nrt-1", "30% and 30 ms: moves");
});

test("automatic: a relay the machine is on wins unless another is clearly better", () => {
  const d = dir();
  const pick = (m: Record<string, number>, on: string[]) =>
    chooseAuto({ directory: d, machine: null, deviceRtt: rtts(m), usable: () => true, current: null, now: NOW, on: new Set(on) })?.relay.id;
  assert.equal(pick({ "sin-1": 50, "nrt-1": 45, "fra-1": 200 }, ["sin-1"]), "sin-1", "10% nearer: it stays where the machine is");
  assert.equal(pick({ "sin-1": 100, "nrt-1": 60, "fra-1": 200 }, ["sin-1"]), "nrt-1", "40% and 40 ms nearer: worth the machine moving");
  assert.equal(pick({ "nrt-1": 20 }, ["sin-1"]), "sin-1", "not measured yet: where the machine is");
  assert.equal(pick({ "sin-1": 50, "nrt-1": 45, "fra-1": 200 }, []), "nrt-1", "on none: the best");
  // The current relay's hysteresis gives way to where the machine moved (its set moved: a drain, a failure).
  assert.equal(
    chooseAuto({ directory: d, machine: null, deviceRtt: rtts({ "sin-1": 50, "nrt-1": 52 }), usable: () => true, current: "sin-1", now: NOW, on: new Set(["nrt-1"]) })?.relay.id,
    "nrt-1",
  );
});

test("a full set keeps to its members, unless one is idle a week or none of them works", () => {
  const set = [
    { id: "sin-1", lastUsedMs: NOW - DAY },
    { id: "nrt-1", lastUsedMs: NOW - DAY },
  ];
  const near = rtts({ "sin-1": 80, "nrt-1": 60, "fra-1": 5 });
  const choose = (d: RelayDirectory) => chooseAuto({ directory: d, machine: null, deviceRtt: near, usable: () => true, current: null, now: NOW });
  assert.equal(choose(dir({ accountSet: set }))?.relay.id, "nrt-1", "Frankfurt is nearer but the set is full");
  const idle = [set[0]!, { id: "nrt-1", lastUsedMs: NOW - 8 * DAY }];
  assert.equal(choose(dir({ accountSet: idle }))?.relay.id, "fra-1", "a member idle for a week may be replaced");
  const down = dir({ accountSet: set, relays: dir().relays.map((r) => (r.id === "fra-1" ? r : { ...r, state: "down" })) });
  const urgent = choose(down);
  assert.equal(urgent?.relay.id, "fra-1");
  assert.equal(urgent?.emergency, true);
});

test("a set the Hub filled itself (never used: last used 0) keeps no relay from a page, automatic, manual or in the picker", async () => {
  // relay-and-plans §2.7: the Hub keeps two relays in the set for the agents; no device has used either.
  const filled = [
    { id: "sin-1", lastUsedMs: 0 },
    { id: "nrt-1", lastUsedMs: 0 },
  ];
  const near = rtts({ "sin-1": 80, "nrt-1": 60, "fra-1": 5 });
  const auto = chooseAuto({ directory: dir({ accountSet: filled }), machine: null, deviceRtt: near, usable: () => true, current: null, now: NOW });
  assert.equal(auto?.relay.id, "fra-1", "the nearest, though the set is full");
  assert.equal(auto?.emergency, false, "its ticket makes it a member, not an emergency");
  const { pool, store } = fakePool(dir({ accountSet: filled }));
  await pool.refresh();
  store.set("rh.relays.manual", "fra-1");
  const manual = pool.choose(null);
  assert.equal(manual?.relay.id, "fra-1");
  assert.equal(manual?.mode, "manual");
  const i18n = makeI18n({ lng: "en", resources: { en: { translation: {} } }, keySeparator: false, nsSeparator: false });
  const html = renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <RelaySection pool={pool} machine="m-1" />
    </I18nextProvider>,
  );
  assert.doesNotMatch(html, /data-disabled/, "no relay is greyed");
  assert.ok(!html.includes("full, not selectable"));
});

test("a ping's median drops the first sample", () => {
  assert.equal(medianAfterFirst([900, 12, 10, 11]), 11);
  assert.equal(medianAfterFirst([900]), null);
});

function fakePool(directory: RelayDirectory) {
  const store = new Map<string, string>();
  const storage = { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => void store.set(k, v), removeItem: (k: string) => void store.delete(k) };
  const api = { relays: async () => directory, relayTickets: async () => ({ tickets: [], errors: [] }) };
  const pool = new RelayPool({
    api: api as never,
    account: { account: "a-1", deviceId: "d-1", session: { token: "t" }, keys: { sign: { privateKey: {} as CryptoKey } } },
    storage,
    now: () => NOW,
  });
  return { pool, store };
}

test("a manual choice holds until its relay fails three times or leaves the list, then the page says why", async () => {
  const d = dir();
  const { pool, store } = fakePool(d);
  await pool.refresh();
  pool.setManual("fra-1");
  assert.equal(store.get("rh.relays.manual"), "fra-1", "kept on this device");
  assert.equal(pool.choose(null)?.relay.id, "fra-1");
  assert.equal(pool.choose(null)?.mode, "manual");
  pool.noteFailure("fra-1", "refused");
  pool.noteFailure("fra-1", "refused");
  assert.equal(pool.choose(null)?.mode, "manual", "two failures: still manual");
  pool.noteFailure("fra-1", "refused");
  assert.equal(pool.choose(null)?.mode, "auto", "three failures: back to automatic");
  assert.equal(store.get("rh.relays.manual"), undefined);
  assert.deepEqual(pool.notice, { relay: "fra-1", city: "frankfurt", why: "failed" });

  pool.setManual("nrt-1");
  assert.equal(pool.notice, null);
  d.relays = d.relays.filter((r) => r.id !== "nrt-1");
  await pool.refresh();
  assert.equal(pool.manual, null, "the relay left the list");
  assert.equal(pool.notice?.why, "gone");
  // A manual choice is never a relay the caller excludes (one going away).
  pool.setManual("sin-1");
  assert.notEqual(pool.choose(null, new Set(["sin-1"]))?.relay.id, "sin-1");
});

test("the picker offers automatic first and greys a third relay on a full free account", async () => {
  const d = dir({
    accountSet: [
      { id: "sin-1", lastUsedMs: Date.now() - DAY },
      { id: "nrt-1", lastUsedMs: Date.now() - DAY },
    ],
  });
  const { pool } = fakePool(d);
  await pool.refresh();
  const i18n = i18next.createInstance();
  await i18n.use(initReactI18next).init({ lng: "en", resources: { en: { translation: {} } }, interpolation: { escapeValue: false }, keySeparator: false, nsSeparator: false });
  const html = renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <RelaySection pool={pool} machine="m-1" />
    </I18nextProvider>,
  );
  assert.match(html, /data-relay="auto"[\s\S]*checked=""/, "automatic by default");
  assert.match(html, /data-relay="fra-1" data-disabled=""/);
  assert.ok(html.includes("full, not selectable"));
  assert.ok(html.includes("At most 2 relays at once · in use: Singapore, Tokyo"));
  assert.doesNotMatch(html, /data-relay="sin-1" data-disabled/);
});

/** A socket the test plays the relay's side of. */
class FakeSocket {
  binaryType = "arraybuffer";
  readyState = 1;
  sent: (string | Uint8Array)[] = [];
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: { code?: number; reason?: string }) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  send(data: string | ArrayBufferLike | ArrayBufferView): void {
    this.sent.push(typeof data === "string" ? data : new Uint8Array(data as ArrayBuffer));
  }
  close(): void {
    this.readyState = 3;
  }
  text(v: unknown) {
    this.onmessage?.({ data: JSON.stringify(v) });
  }
  frame(bytes: Uint8Array) {
    this.onmessage?.({ data: bytes.slice().buffer });
  }
}

test("a relay connection signs in with relayDevice over the relay's origin, routes by ch, times pings and closes channels when it drops", async () => {
  const keys = await generateDeviceKeys();
  const socket = new FakeSocket();
  const link = new RelayLink({
    relay: "nrt-1",
    origin: "wss://nrt-1.example",
    account: "a-1",
    deviceId: "d-1",
    signKey: keys.sign.privateKey,
    ticket: async () => "TICKET",
    makeSocket: () => socket as never,
  });
  const opened = link.open();
  const nonce = new Uint8Array(32).fill(7);
  socket.text({ t: "challenge", nonce: hex(nonce), relay: "nrt-1", origin: "wss://nrt-1.example", version: 1 });
  // Signed with WebCrypto, which takes as long as the machine makes it.
  await until(() => socket.sent.length > 0, "the login");
  assert.equal(socket.sent.length, 1, "the login, and nothing before it");
  const login = JSON.parse(socket.sent[0] as string) as { t: string; ticket: string; time_ms: number; sig: string };
  assert.equal(login.t, "login");
  assert.equal(login.ticket, "TICKET");
  const signed = { role: "relayDevice" as const, account: "a-1", id: "d-1", hub: "wss://nrt-1.example", nonce, timeMs: login.time_ms };
  assert.ok(await verifyHubLogin(signed, keys.signPublic, fromHex(login.sig)), "a relayDevice signature over the relay's own origin");
  assert.equal(await verifyHubLogin({ ...signed, role: "device" }, keys.signPublic, fromHex(login.sig)), false, "never a hub sign-in");
  socket.text({ t: "welcome", relay: "nrt-1", id: "d-1", expires_ms: NOW });
  await opened;
  assert.equal(link.state, "open");
  socket.text({ t: "presence", machines: [{ id: "m-1", state: "online" }] });
  assert.equal(link.presence.get("m-1"), "online");

  // A ping went out at sign-in; its pong is the round trip.
  const ping = socket.sent.map((s) => (typeof s === "string" ? null : decodeFrame(s))).find((f) => f?.type === "ping");
  assert.ok(ping && ping.type === "ping");
  socket.frame(encodeFrame({ type: "pong", nonce: ping.nonce }));
  assert.ok(link.rttMs !== null && link.rttMs >= 0);

  const got: string[] = [];
  const ch = link.reserve((f) => got.push(f.type));
  assert.ok(link.send({ type: "open", ch, peer: "m-1" }));
  socket.frame(encodeFrame({ type: "data", ch, payload: new Uint8Array([1]) }));
  assert.deepEqual(got, ["data"]);
  let away = "";
  link.onGoaway((g) => (away = g.reason));
  socket.text({ t: "goaway", retry_ms: 1000, reason: "restart" });
  assert.equal(away, "restart");
  assert.ok(link.goingAway);
  socket.onclose?.({ code: 1012, reason: "restart" });
  assert.deepEqual(got, ["data", "close"], "its channels are told the connection went");
  assert.equal(link.state, "closed");
});

test("a manual relay that does not answer for 30 s gives way to automatic", async () => {
  let now = NOW;
  const store = new Map<string, string>([["rh.relays.manual", "nrt-1"]]);
  const storage = { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => void store.set(k, v), removeItem: (k: string) => void store.delete(k) };
  const pool = new RelayPool({
    api: { relays: async () => dir(), relayTickets: async () => ({ tickets: [], errors: [] }) } as never,
    account: { account: "a-1", deviceId: "d-1", session: { token: "t" }, keys: { sign: { privateKey: {} as CryptoKey } } },
    storage,
    now: () => now,
    fetchImpl: async () => {
      throw new Error("unreachable");
    },
  });
  await pool.refresh();
  await pool.checkManualReachable();
  assert.equal(pool.manual, "nrt-1", "not yet");
  now += 20_000;
  await pool.checkManualReachable();
  assert.equal(pool.manual, "nrt-1");
  now += 11_000;
  await pool.checkManualReachable();
  assert.equal(pool.manual, null, "30 s without an answer");
  assert.deepEqual(pool.notice, { relay: "nrt-1", city: "tokyo", why: "failed" });
});
