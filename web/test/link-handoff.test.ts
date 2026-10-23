// The local sign-in's popup handshake (docs/local-aggregator.md §3.3–3.4,
// docs/hub-api/link.md): what each side accepts, the `cb` tiers, the
// fingerprint both sides show, the opener's state machine against a fake
// popup, and every string of the two screens in all four languages.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { callbackTier, confirmsHost, hostToType, hubWebOrigin, machineFingerprint, parseLinkMessage, privateHost } from "../src/features/link/handoff.js";
import { LinkOpener, type OpenerState, type OpenerWindow } from "../src/features/link/opener.js";
import { linkJa, linkZhCN, linkZhTW } from "../src/i18n/link.js";
import { onboardingJa, onboardingZhCN, onboardingZhTW } from "../src/i18n/onboarding.js";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "../src/i18n/cloud-devices.js";
import { ja } from "../src/i18n/ja.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";

const NONCE = "0123456789abcdef0123456789abcdef";
const KEY = "ab".repeat(32);

test("only well-formed messages of this version are taken", () => {
  assert.deepEqual(parseLinkMessage({ t: "rh-link/ready", v: 1 }), { t: "rh-link/ready", v: 1 });
  assert.equal(parseLinkMessage({ t: "rh-link/ready", v: 2 }), null, "another version");
  assert.equal(parseLinkMessage({ t: "rh-link/whatever", v: 1 }), null, "an unknown type");
  assert.equal(parseLinkMessage("rh-link/ready"), null);
  const request = { t: "rh-link/request", v: 1, nonce: NONCE, machine: { hostname: "box", signKey: KEY, dhKey: KEY }, aggregator: null, aggregate: true };
  const parsed = parseLinkMessage(request);
  assert.ok(parsed?.t === "rh-link/request" && parsed.aggregate === false, "no aggregator keys, no aggregation");
  assert.equal(parseLinkMessage({ ...request, nonce: "short" }), null);
  assert.equal(parseLinkMessage({ ...request, machine: { ...request.machine, signKey: "zz" } }), null);
  assert.equal(parseLinkMessage({ ...request, machine: { ...request.machine, enrolled: { accountFp: "nothex", id: "m-1" } } }), null);
  assert.ok(parseLinkMessage({ t: "rh-link/code", v: 1, nonce: NONCE, code: "RH1-AAAA-BBBB" }));
  for (const code of ["DR1-AAAA-BBBB", "dr1-aaaa-bbbb", "rh1-aaaa-bbbb"]) {
    assert.deepEqual(parseLinkMessage({ t: "rh-link/code", v: 1, nonce: NONCE, code }), { t: "rh-link/code", v: 1, nonce: NONCE, code }, code);
  }
  assert.equal(parseLinkMessage({ t: "rh-link/code", v: 1, nonce: NONCE, code: "not a code" }), null);
  for (const code of ["DRP1-AAAA", "RHP1-AAAA", "DRR1-AAAA", "RHR1-AAAA", "DR2-AAAA-BBBB", "XDR1-AAAA-BBBB"]) {
    assert.equal(parseLinkMessage({ t: "rh-link/code", v: 1, nonce: NONCE, code }), null, code);
  }
});

test("only this computer itself is trusted without a warning; a public address must be typed out; the hub's own host is refused", () => {
  const hub = "https://droppen.app";
  for (const local of ["http://127.0.0.1:8700", "http://127.8.9.10", "http://localhost:5180", "http://box.localhost", "http://[::1]:8700"]) {
    assert.equal(callbackTier(local, hub), "local", local);
  }
  for (const other of ["https://evil.example", "http://8.8.8.8", "http://172.32.0.1", "http://100.128.0.1", "http://[2001:db8::1]", "https://droppen.app.evil.example", "https://my-box.example.com:8443"]) {
    assert.equal(callbackTier(other, hub), "public", other);
  }
  assert.equal(callbackTier("https://droppen.app", hub), "refuse", "the hub's own origin");
  assert.equal(callbackTier("http://droppen.app", hub), "refuse", "the hub's host on another scheme");
  assert.equal(callbackTier("https://droppen.app:8443", hub), "refuse", "the hub's host on another port");
  assert.equal(callbackTier("http://127.0.0.1:59701", "http://127.0.0.1:54823"), "local", "a test hub on this computer's loopback");
  assert.equal(callbackTier("http://127.0.0.1:54823", "http://127.0.0.1:54823"), "refuse", "but never the hub's own origin");
  assert.equal(callbackTier("null", hub), "refuse", "an opaque origin");
  assert.equal(callbackTier("file://", hub), "refuse");
  assert.equal(callbackTier("https://droppen.app/path", hub), "refuse", "not an origin");
});

// Review of the local sign-in (its proof of concept): Tailscale Funnel hands
// anyone a public https://x.<tailnet>.ts.net; a LAN neighbour can answer for
// *.local, a private address or 100.64/10. A fake settings page on any of
// them could open the real /link, and one click on "Add this computer" sent it
// the code. They are all "warn" now: the warning, the explicit confirmation
// and the fingerprint check.
test("addresses an attacker can obtain are never trusted silently", () => {
  const hub = "https://droppen.app";
  for (const address of [
    "https://evil-box.tail1234.ts.net",
    "http://printer.local",
    "http://studio.local:8700",
    "http://192.168.1.66",
    "http://10.0.0.2",
    "http://172.20.1.1:8700",
    "http://100.100.1.1",
    "http://100.64.0.1:8700",
    "http://[fd7a:115c:a1e0::1]",
    "http://[fd12:3456::1]:8700",
    "http://[fc00::1]",
    "http://0.0.0.0:8700",
    "http://[::ffff:7f00:1]",
    "http://localhost.:8700",
  ]) {
    assert.equal(callbackTier(address, hub), "warn", address);
  }
});

test("the host name a request carries is shown as plain, short text", () => {
  const request = (hostname: string) =>
    parseLinkMessage({ t: "rh-link/request", v: 1, nonce: NONCE, machine: { hostname, signKey: KEY, dhKey: KEY }, aggregator: null, aggregate: false });
  const odd = request("ada-mbp\u202e\u0000\n<b>x</b>'\"");
  assert.ok(odd?.t === "rh-link/request");
  assert.equal(odd.machine.hostname, "ada-mbpbx/b");
  const long = request("h".repeat(200));
  assert.ok(long?.t === "rh-link/request" && long.machine.hostname.length === 64);
});

test("the machine fingerprint matches the local server's", async () => {
  // `machine_fingerprint_of(&[1; 32], &[2; 32])` in server-rs/src/agent/control.rs.
  assert.equal(await machineFingerprint(new Uint8Array(32).fill(1), new Uint8Array(32).fill(2)), "d4c7 841d 009c f90b 2395 fcd4 4a5a 52fd");
});

test("a typed hub address becomes the popup's origin", () => {
  assert.equal(hubWebOrigin("wss://hub.example/v1/agent"), "https://hub.example");
  assert.equal(hubWebOrigin("http://127.0.0.1:9000/"), "http://127.0.0.1:9000");
  assert.equal(hubWebOrigin("ftp://x"), null);
  assert.equal(hubWebOrigin(""), null);
});

/** A window that records what it opens, and a popup that records what it is sent. */
function fakes(openResult: "popup" | "blocked" = "popup") {
  const listeners = new Set<(e: MessageEvent) => void>();
  const sent: { message: unknown; origin: string }[] = [];
  const popup = {
    closed: false,
    postMessage(message: unknown, origin: string) {
      sent.push({ message, origin });
    },
    close() {
      this.closed = true;
    },
    focus() {},
  };
  const opened: string[] = [];
  const win: OpenerWindow = {
    open(url) {
      opened.push(url);
      return openResult === "popup" ? (popup as unknown as Window) : null;
    },
    addEventListener: (_t, l) => void listeners.add(l),
    removeEventListener: (_t, l) => void listeners.delete(l),
  };
  const deliver = (data: unknown, origin: string, source: unknown = popup) => {
    for (const l of listeners) l({ data, origin, source } as MessageEvent);
  };
  return { win, popup, sent, opened, deliver };
}

const ATTEMPT = {
  attempt: "att-1",
  nonce: NONCE,
  hubWeb: "https://droppen.app",
  expiresMs: Date.now() + 60_000,
  machine: { hostname: "box", signKey: KEY, dhKey: KEY, fingerprint: "0000 0000 0000 0000 0000 0000 0000 0000", enrolled: null },
  aggregator: { signKey: KEY, dhKey: KEY },
};

const tick = () => new Promise((r) => setTimeout(r, 0));

test("the opener answers only its own popup from the hub's origin, and hands the code to its server", async () => {
  const f = fakes();
  const states: OpenerState[] = [];
  const finished: unknown[] = [];
  const opener = new LinkOpener({
    win: f.win,
    start: async () => ATTEMPT,
    finish: async (body) => {
      finished.push(body);
      return { machine: "m-9", account: "a-1" };
    },
    cancel: async () => {},
    onState: (s) => states.push(s),
    pollMs: 10_000,
  });
  opener.open("https://droppen.app");
  assert.deepEqual(f.opened, ["https://droppen.app/link?v=1"], "opened at once, nothing in the URL");
  await tick();
  assert.equal(opener.current.kind, "waiting");
  f.deliver({ t: "rh-link/ready", v: 1 }, "https://evil.example");
  f.deliver({ t: "rh-link/ready", v: 1 }, "https://droppen.app", {});
  assert.equal(f.sent.length, 0, "not from the hub's origin, or not from its own popup: ignored");
  f.deliver({ t: "rh-link/ready", v: 1 }, "https://droppen.app");
  assert.equal(f.sent.length, 1);
  assert.equal(f.sent[0]!.origin, "https://droppen.app", "posted to the hub's origin only");
  assert.deepEqual((f.sent[0]!.message as { t: string; nonce: string }).nonce, NONCE);
  f.deliver({ t: "rh-link/code", v: 1, nonce: "f".repeat(32), code: "RH1-AAAA-BBBB" }, "https://droppen.app");
  await tick();
  assert.equal(finished.length, 0, "another nonce: ignored");
  f.deliver({ t: "rh-link/code", v: 1, nonce: NONCE, code: "RH1-AAAA-BBBB", email: "a@example.com" }, "https://droppen.app");
  await tick();
  assert.deepEqual(finished, [{ attempt: "att-1", nonce: NONCE, code: "RH1-AAAA-BBBB", email: "a@example.com" }]);
  assert.equal(opener.current.kind, "enrolling");
  f.deliver({ t: "rh-link/done", v: 1, nonce: NONCE, account: "a-1", email: "a@example.com", machine: "m-9" }, "https://droppen.app");
  assert.equal(opener.current.kind, "done");
  opener.close();
});

test("a blocked popup leaves a link to open it, and a failed finish tells the popup", async () => {
  const f = fakes("blocked");
  const opener = new LinkOpener({
    win: f.win,
    start: async () => ATTEMPT,
    finish: async () => {
      throw new Error("hub_unreachable");
    },
    cancel: async () => {},
    onState: () => {},
    pollMs: 10_000,
  });
  opener.open("https://droppen.app");
  await tick();
  assert.equal(opener.current.kind, "blocked");
  opener.close();

  const g = fakes();
  const failing = new LinkOpener({
    win: g.win,
    start: async () => ATTEMPT,
    finish: async () => {
      throw new Error("the hub could not be reached");
    },
    cancel: async () => {},
    onState: () => {},
    pollMs: 10_000,
  });
  failing.open("https://droppen.app");
  await tick();
  g.deliver({ t: "rh-link/code", v: 1, nonce: NONCE, code: "RH1-AAAA-BBBB" }, "https://droppen.app");
  await tick();
  await tick();
  assert.equal(failing.current.kind, "error");
  const told = g.sent.map((s) => s.message as { t: string }).find((m) => m.t === "rh-link/error");
  assert.ok(told, "the popup hears it");
  failing.close();
});

test("every string of the Droppen settings, the /link popup and the browser hand-off is translated into zh-CN, zh-TW and ja", () => {
  const files = ["link-settings.tsx", "link-popup.tsx", "first-run.tsx", "browser-sign-in.tsx", "link-return.tsx"].map((f) => readFileSync(new URL(`../src/features/link/${f}`, import.meta.url), "utf8"));
  const keys = new Set<string>();
  for (const src of files) for (const m of src.matchAll(/\bt\(\s*"((?:[^"\\]|\\.)*)"/g)) keys.add(m[1]!);
  assert.ok(keys.size > 40);
  for (const [name, table] of [
    ["zh-CN", { ...onboardingZhCN, ...linkZhCN, ...cloudDevicesZhCN, ...zhCN }],
    ["zh-TW", { ...onboardingZhTW, ...linkZhTW, ...cloudDevicesZhTW, ...zhTW }],
    ["ja", { ...onboardingJa, ...linkJa, ...cloudDevicesJa, ...ja }],
  ] as const) {
    const missing = [...keys].filter((k) => !(k in table));
    assert.deepEqual(missing, [], `${name} lacks these`);
  }
});

test("an attempt that runs out while the popup waits (a recovered browser awaiting approval) says so", async () => {
  const f = fakes();
  let now = 1_000;
  const opener = new LinkOpener({
    win: f.win,
    start: async () => ({ ...ATTEMPT, expiresMs: 5_000 }),
    finish: async () => ({ machine: "m-1", account: "a-1" }),
    cancel: async () => {},
    onState: () => {},
    pollMs: 5,
    now: () => now,
  });
  opener.open("https://droppen.app");
  await tick();
  assert.equal(opener.current.kind, "waiting");
  now = 5_000;
  await new Promise((r) => setTimeout(r, 30));
  assert.equal(opener.current.kind, "expired");
  assert.ok(f.popup.closed, "the popup is closed with it");
  opener.close();
});

test("the warning with a tick is kept for the networks a computer of one's own sits on; any other host is public", () => {
  for (const host of ["192.168.0.2", "10.1.2.3", "172.16.0.1", "172.31.255.255", "169.254.1.1", "100.64.0.1", "100.127.255.255", "0.0.0.0", "nas", "box.local", "box.local.", "pi.home.arpa", "srv.internal", "x.tail1234.ts.net", "[fd00::1]", "[fe80::1]", "[::ffff:c0a8:1]"]) {
    assert.equal(privateHost(host), true, host);
  }
  for (const host of ["8.8.8.8", "172.15.0.1", "172.32.0.1", "100.63.255.255", "100.128.0.1", "example.com", "local.example.com", "x.ts.net.evil.example", "[2001:db8::1]", "[::ffff:808:808]"]) {
    assert.equal(privateHost(host), false, host);
  }
  // A name with no dot is the local network's own (its search domain resolves it).
  assert.equal(privateHost("studio-mac"), true);
});

test("a public address is confirmed by typing its host name, not by a tick", () => {
  const cb = "https://My-Box.Example.com:8443";
  assert.equal(hostToType(cb), "my-box.example.com", "the name, no port");
  assert.equal(confirmsHost("my-box.example.com", cb), true);
  assert.equal(confirmsHost("  MY-BOX.example.COM ", cb), true, "case and blanks do not matter");
  assert.equal(confirmsHost("my-box.example.com.", cb), true, "nor a trailing dot");
  for (const wrong of ["", "example.com", "my-box.example.co", "my-box.example.com:8443", "https://my-box.example.com", "y"]) {
    assert.equal(confirmsHost(wrong, cb), false, wrong);
  }
  assert.equal(hostToType("http://[2001:db8::1]:8700"), "2001:db8::1");
  assert.equal(confirmsHost("[2001:db8::1]", "http://[2001:db8::1]:8700"), true);
  assert.equal(confirmsHost("anything", "not a url"), false);
});
