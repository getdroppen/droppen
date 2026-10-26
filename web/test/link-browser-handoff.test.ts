// Signing a device in to Droppen through the system browser (the browser
// hand-off, docs/local-aggregator.md §4.9): the sealed answer agrees with
// Rust byte for byte, the two addresses carry only what they should, each page
// takes its fragment out of the address, the return page hands the answer
// over without a login, and the local page follows the attempt at its server.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { fromHex, hex, keyPairFromSecret, publicFromSecret, sealLinkReturn, utf8 } from "../src/lib/e2e/index.js";
import {
  base64Url,
  fromBase64Url,
  handoffOf,
  handoffPayload,
  isReturnPath,
  parseHandoff,
  parseReturn,
  plainOrigin,
  returnAddress,
} from "../src/features/link/handoff.js";
import { captureLinkFragments, forgetHandoff, HANDOFF_STORE_KEY, pendingHandoff, restoreReturn, takeReturn, type FragmentEnv } from "../src/features/link/fragment-boot.js";
import { giveBack, RETURN_API } from "../src/features/link/link-return.js";
import { type Attempt, type HandoffStatus, LinkOpener, type OpenerWindow } from "../src/features/link/opener.js";
import { formatLeft } from "../src/features/link/browser-sign-in.js";

const fill = (n: number, b: number) => new Uint8Array(n).fill(b);

test("the sealed answer is the bytes Rust seals (rh-proto vectors/link-return.hex)", async () => {
  const expected = readFileSync(new URL("../../server-rs/crates/rh-proto/vectors/link-return.hex", import.meta.url), "utf8").trim();
  const recipient = await publicFromSecret("X25519", fill(32, 0x11));
  const ephemeral = await keyPairFromSecret(fill(32, 0x22));
  const sealed = await sealLinkReturn(
    recipient,
    fill(16, 0x44),
    { attempt: "0123456789abcdef0123456789abcdef", origin: "http://127.0.0.1:51234", signKey: fill(32, 0x55), dhKey: fill(32, 0x66) },
    utf8('{"code":"DR1-AAAA-BBBB","email":"me@example.com"}'),
    { ephemeral, nonce: fill(12, 0x33) },
  );
  assert.equal(hex(sealed), expected);
  // A fresh one-off key each time otherwise.
  const a = await sealLinkReturn(recipient, fill(16, 1), { attempt: "a", origin: "http://x", signKey: fill(32, 1), dhKey: fill(32, 2) }, utf8("x"));
  const b = await sealLinkReturn(recipient, fill(16, 1), { attempt: "a", origin: "http://x", signKey: fill(32, 1), dhKey: fill(32, 2) }, utf8("x"));
  assert.notEqual(hex(a.subarray(0, 32)), hex(b.subarray(0, 32)));
  await assert.rejects(sealLinkReturn(recipient, fill(15, 1), { attempt: "a", origin: "http://x", signKey: fill(32, 1), dhKey: fill(32, 2) }, utf8("x")));
});

const ATTEMPT = "0123456789abcdef0123456789abcdef";
const PAYLOAD = {
  v: 1,
  a: ATTEMPT,
  k: "ab".repeat(32),
  s: "cd".repeat(16),
  o: "http://127.0.0.1:51234",
  x: 1_700_000_000_000,
  m: { h: "laptop", s: "11".repeat(32), d: "22".repeat(32) },
};
const encode = (value: unknown) => base64Url(utf8(JSON.stringify(value)));

test("droppen.app reads a well-formed hand-off from its address, and nothing else", () => {
  const handoff = parseHandoff(`#droppen-link=${encode(PAYLOAD)}`);
  assert.ok(handoff);
  assert.deepEqual(
    { attempt: handoff.attempt, origin: handoff.origin, expiresMs: handoff.expiresMs, host: handoff.machine.hostname, sign: handoff.machine.signKey },
    { attempt: ATTEMPT, origin: "http://127.0.0.1:51234", expiresMs: 1_700_000_000_000, host: "laptop", sign: "11".repeat(32) },
  );
  assert.deepEqual(handoffOf(handoffPayload(handoff)), handoff, "kept for the tab and read back the same");
  const enrolled = parseHandoff(`#droppen-link=${encode({ ...PAYLOAD, m: { ...PAYLOAD.m, e: { f: "ef".repeat(16), i: "m-1" } } })}`);
  assert.deepEqual(enrolled?.machine.enrolled, { accountFp: "ef".repeat(16), id: "m-1" });
  for (const bad of [
    { ...PAYLOAD, v: 2 },
    { ...PAYLOAD, a: "short" },
    { ...PAYLOAD, k: "zz".repeat(32) },
    { ...PAYLOAD, s: "cd".repeat(15) },
    { ...PAYLOAD, o: "http://127.0.0.1:51234/path" },
    { ...PAYLOAD, o: "javascript:alert(1)" },
    { ...PAYLOAD, o: "file:///etc" },
    { ...PAYLOAD, x: "soon" },
    { ...PAYLOAD, m: { ...PAYLOAD.m, s: "11" } },
    { ...PAYLOAD, m: undefined },
  ]) {
    assert.equal(parseHandoff(`#droppen-link=${encode(bad)}`), null, JSON.stringify(bad));
  }
  assert.equal(parseHandoff("#droppen-link=***"), null);
  assert.equal(parseHandoff("#other=1"), null);
  assert.equal(parseHandoff(""), null);
  // The host name is shown as plain, short text.
  assert.equal(parseHandoff(`#droppen-link=${encode({ ...PAYLOAD, m: { ...PAYLOAD.m, h: "<b>box‮</b>" } })}`)?.machine.hostname, "bbox/b");
  assert.equal(plainOrigin("https://box.example:8443"), "https://box.example:8443");
  assert.equal(plainOrigin("https://box.example/"), null);
});

test("the return address carries the attempt and ciphertext in its fragment only", () => {
  const sealed = fill(80, 7);
  const address = returnAddress("http://127.0.0.1:51234", ATTEMPT, sealed);
  const url = new URL(address);
  assert.equal(url.origin, "http://127.0.0.1:51234");
  assert.equal(url.pathname, "/link/return");
  assert.equal(url.search, "");
  const back = parseReturn(url.hash);
  assert.ok(back);
  assert.equal(back.attempt, ATTEMPT);
  assert.deepEqual(fromBase64Url(back.sealed), sealed);
  assert.equal(parseReturn(`#droppen-return=nothex.${base64Url(sealed)}`), null);
  assert.equal(parseReturn(`#droppen-return=${ATTEMPT}.${base64Url(sealed)}.x`), null);
  assert.equal(parseReturn(`#droppen-return=${ATTEMPT}.${"A".repeat(2049)}`), null);
  assert.equal(parseReturn(`#droppen-return=${ATTEMPT}.`), null);
  assert.ok(isReturnPath("/link/return") && isReturnPath("/link/return/") && !isReturnPath("/link") && !isReturnPath("/link/returns"));
});

function fakeEnv(path: string, hash: string): FragmentEnv & { replaced: string[]; kept: Map<string, string> } {
  const kept = new Map<string, string>();
  const replaced: string[] = [];
  return {
    location: { pathname: path, search: "", hash },
    history: { state: null, replaceState: (_s, _u, url) => void replaced.push(url) },
    storage: { getItem: (k) => kept.get(k) ?? null, setItem: (k, v) => void kept.set(k, v), removeItem: (k) => void kept.delete(k) },
    replaced,
    kept,
  };
}

test("each page takes its fragment out of the address before anything reads it", () => {
  const link = fakeEnv("/link", `#droppen-link=${encode(PAYLOAD)}`);
  captureLinkFragments(link);
  assert.deepEqual(link.replaced, ["/link"], "gone from the address");
  assert.ok(link.kept.has(HANDOFF_STORE_KEY), "kept for this tab");
  assert.equal(pendingHandoff(link)?.attempt, ATTEMPT);
  forgetHandoff(link);
  assert.equal(pendingHandoff(link), null);
  assert.ok(!link.kept.has(HANDOFF_STORE_KEY));

  const bad = fakeEnv("/link", "#droppen-link=garbage");
  captureLinkFragments(bad);
  assert.deepEqual(bad.replaced, ["/link"], "a malformed one leaves the address too");
  assert.equal(pendingHandoff(bad), null);

  const back = fakeEnv("/link/return", `#droppen-return=${ATTEMPT}.${base64Url(fill(80, 7))}`);
  captureLinkFragments(back);
  assert.deepEqual(back.replaced, ["/link/return"]);
  assert.equal(back.kept.size, 0, "the answer is never stored");
  assert.equal(takeReturn()?.attempt, ATTEMPT);
  assert.equal(takeReturn(), null, "handed out once");

  const other = fakeEnv("/sessions", "#section-2");
  captureLinkFragments(other);
  assert.deepEqual(other.replaced, [], "anything else is left be");
});

test("an answer put back on the address for a reload is the one the reloaded page takes, and is never stored", () => {
  const sealed = base64Url(fill(80, 7));
  const page = fakeEnv("/link/return", `#droppen-return=${ATTEMPT}.${sealed}`);
  captureLinkFragments(page);
  const answer = takeReturn()!;
  restoreReturn(page, answer);
  assert.deepEqual(page.replaced, ["/link/return", `/link/return#droppen-return=${ATTEMPT}.${sealed}`]);
  // The page as the reload loads it, at that address.
  const reloaded = fakeEnv("/link/return", page.replaced[1]!.slice("/link/return".length));
  captureLinkFragments(reloaded);
  assert.deepEqual(reloaded.replaced, ["/link/return"], "off the address again at once");
  assert.deepEqual(takeReturn(), answer);
  assert.equal(page.kept.size + reloaded.kept.size, 0);
});

test("the return page hands the answer over without a login, and says how it went", async () => {
  const calls: { url: string; init: RequestInit }[] = [];
  const answering = (status: number, body: unknown) =>
    (async (url: string, init: RequestInit) => {
      calls.push({ url, init });
      return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
    }) as unknown as typeof fetch;
  const answer = { attempt: ATTEMPT, sealed: "AAAA" };
  assert.deepEqual(await giveBack(answer, answering(202, { machine: "m-1", account: "a-1" })), { kind: "done" });
  assert.equal(calls[0]!.url, RETURN_API);
  assert.equal(calls[0]!.init.method, "POST");
  assert.equal(calls[0]!.init.credentials, "omit");
  assert.equal(new Headers(calls[0]!.init.headers).get("Authorization"), null, "no login");
  assert.deepEqual(JSON.parse(String(calls[0]!.init.body)), answer);
  assert.deepEqual(await giveBack(answer, answering(410, { error: "expired", message: "the sign-in attempt expired (start again)" })), {
    kind: "error",
    code: "expired",
    message: "the sign-in attempt expired (start again)",
  });
  assert.deepEqual(await giveBack(answer, answering(500, "oops")), { kind: "error", code: "http_500", message: "" });
  const down = (async () => {
    throw new TypeError("Failed to fetch");
  }) as unknown as typeof fetch;
  assert.deepEqual(await giveBack(answer, down), { kind: "error", code: "unreachable", message: "Failed to fetch" });
});

const SIGN_IN: Attempt = {
  attempt: ATTEMPT,
  nonce: "f".repeat(32),
  hubWeb: "https://droppen.app",
  expiresMs: Date.now() + 60_000,
  machine: { hostname: "box", signKey: "11".repeat(32), dhKey: "22".repeat(32), fingerprint: "0000 0000 0000 0000 0000 0000 0000 0000", enrolled: null },
  handoffUrl: `https://droppen.app/link#droppen-link=${encode(PAYLOAD)}`,
};

function browserWin() {
  const opened: { url: string; name: string; features: string }[] = [];
  const win: OpenerWindow = {
    open(url, name, features) {
      opened.push({ url, name, features });
      return null;
    },
    addEventListener: () => {},
    removeEventListener: () => {},
  };
  return { win, opened };
}

const tick = (ms = 0) => new Promise((r) => setTimeout(r, ms));

test("through the browser: the address opens in a tab, and the page follows the attempt at its server to the end", async () => {
  const { win, opened } = browserWin();
  let status: HandoffStatus = { state: "none", attempt: { id: ATTEMPT } };
  const cancelled: string[] = [];
  const opener = new LinkOpener({
    win,
    start: async () => SIGN_IN,
    finish: async () => {
      throw new Error("never: no popup");
    },
    cancel: async (a) => void cancelled.push(a),
    status: async () => status,
    onState: () => {},
    pollMs: 5,
  });
  opener.openInBrowser();
  await tick();
  assert.equal(opener.current.kind, "browser");
  assert.deepEqual(opened, [{ url: SIGN_IN.handoffUrl!, name: "_blank", features: "noopener" }], "a tab, with no way back to this page");
  assert.equal(opener.handoffUrl, SIGN_IN.handoffUrl, "to copy");
  opener.reopen();
  assert.equal(opened.length, 2, "open again, from a click");
  await tick(20);
  assert.equal(opener.current.kind, "browser", "still waiting");
  // droppen.app answered through /link/return: taken, the attempt gone while the server enrolls — not "ran out".
  status = { state: "none", attempt: null, handoff: { attempt: ATTEMPT, outcome: "receiving" } };
  await tick(20);
  assert.deepEqual(opener.current, { kind: "finishing", attempt: SIGN_IN });
  // The hub took it.
  status = { state: "enrolling", attempt: null, handoff: { attempt: ATTEMPT, outcome: "handed_over", machine: "m-9" } };
  await tick(20);
  assert.deepEqual(opener.current, { kind: "enrolling", attempt: SIGN_IN, machine: "m-9", browser: true });
  // A device of the account added it.
  status = { state: "linked", account: { id: "a-1", email: "me@example.com" }, machine: { id: "m-9" }, handoff: status.handoff };
  await tick(20);
  assert.deepEqual(opener.current, { kind: "done", email: "me@example.com", machine: "m-9" });
  assert.deepEqual(cancelled, []);
  opener.close();
});

test("through the browser: a failed answer, an attempt gone without one, and an old server are said", async () => {
  const run = async (status: HandoffStatus, attempt: Attempt = SIGN_IN) => {
    const { win } = browserWin();
    const opener = new LinkOpener({ win, start: async () => attempt, finish: async () => ({ machine: "", account: "" }), cancel: async () => {}, status: async () => status, onState: () => {}, pollMs: 5 });
    opener.openInBrowser();
    await tick(30);
    const state = opener.current;
    opener.close();
    return state;
  };
  // The answer's code goes with it: the first-run guide words an unreachable hub apart from a refusal.
  assert.deepEqual(await run({ state: "none", handoff: { attempt: ATTEMPT, outcome: "failed", error: "hub_unreachable", message: "connection refused" } }), {
    kind: "error",
    message: "connection refused",
    attempt: SIGN_IN,
    code: "hub_unreachable",
  });
  assert.equal(((await run({ state: "none", handoff: { attempt: ATTEMPT, outcome: "failed", message: "no" } })) as { code?: string }).code, undefined, "no code, none made up");
  assert.equal((await run({ state: "none", attempt: null })).kind, "expired", "cancelled or replaced elsewhere");
  assert.equal((await run({ state: "none", attempt: { id: ATTEMPT } }, { ...SIGN_IN, expiresMs: Date.now() - 1 })).kind, "expired");
  assert.equal((await run({ state: "none", handoff: { attempt: "another", outcome: "failed", message: "x" }, attempt: { id: ATTEMPT } })).kind, "browser", "another attempt's answer is not this one's");
  const old = await run({ state: "none" }, { ...SIGN_IN, handoffUrl: null });
  assert.equal(old.kind, "error");
});

test("a blocked popup can go on in a browser tab with the same attempt", async () => {
  const opened: string[] = [];
  const win: OpenerWindow = {
    open(url) {
      opened.push(url);
      return null;
    },
    addEventListener: () => {},
    removeEventListener: () => {},
  };
  const opener = new LinkOpener({ win, start: async () => SIGN_IN, finish: async () => ({ machine: "", account: "" }), cancel: async () => {}, status: async () => ({ state: "none", attempt: { id: ATTEMPT } }), onState: () => {}, pollMs: 10_000 });
  opener.open("https://droppen.app");
  await tick();
  assert.equal(opener.current.kind, "blocked");
  opener.continueInBrowser();
  assert.equal(opener.current.kind, "browser");
  assert.deepEqual(opened, ["https://droppen.app/link?v=1", SIGN_IN.handoffUrl]);
  opener.close();
});

test("how long the link still works reads as minutes and seconds", () => {
  assert.equal(formatLeft(15 * 60_000), "15:00");
  assert.equal(formatLeft(61_001), "1:02");
  assert.equal(formatLeft(-5), "0:00");
  assert.equal(hex(fromHex("0a")), "0a");
});
