// In-session search and the first unread message while the machine is away,
// worked out over the offline copy (`lib/transport/offline-search.ts`).
//
// The first half replays `server-rs/src/feed/search.rs`'s own tests against
// the port: the same items, the same queries, the same matches, fields,
// snippets and ranges — so the page cannot tell which side answered. The
// second half drives the offline transport's routes: absolute indices for a
// copy that starts past item 0, the server's refusals, an empty answer rather
// than a failure where the copy holds nothing, and the first unread message
// found from the read mark a seen report hands back.
import assert from "node:assert/strict";
import test from "node:test";
import type { components } from "../src/api/schema.js";
import { firstUnreadIn, readMarkOf, searchCopy, snippetOf, termsOf, validateQuery, DEFAULT_LIMIT, TOTAL_CAP, SNIPPET_CHARS } from "../src/lib/transport/offline-search.js";
import { type OfflineOptions, type OfflineSource, offlineTransport } from "../src/lib/transport/offline.js";
import type { MirrorChange, SessionCopy } from "../src/lib/hub/mirror.js";
import type { FeedCopy } from "../src/lib/hub/streams.js";

type TranscriptItem = components["schemas"]["TranscriptItem"];
type SessionSummary = components["schemas"]["SessionSummary"];
type Role = TranscriptItem["role"];

const item = (role: Role, kind: TranscriptItem["kind"], text: string, extra: Partial<TranscriptItem> = {}): TranscriptItem =>
  ({ role, kind, text, detail: null, meta: null, toolName: null, timestamp: null, isError: false, truncated: false, question: null, ...extra }) as TranscriptItem;
const text = (role: Role, body: string) => item(role, "text", body, { timestamp: "2026-10-01T10:00:00.000Z" });
const toolUse = (name: string, summary: string, detail: string | null = null) => item("assistant", "tool_use", summary, { toolName: name, detail });
const toolResult = (name: string, summary: string, detail: string) => item("user", "tool_result", summary, { toolName: name, detail });

const search = (items: TranscriptItem[], query: string, limit = DEFAULT_LIMIT, cap = TOTAL_CAP) => searchCopy({ items, startIndex: 0 }, query, termsOf(query), limit, cap);
const indices = (result: ReturnType<typeof search>) => result.matches.map((m) => m.index);
const slice16 = (snippet: string, range: number[]) => snippet.slice(range[0], range[1]);

/* ── the server's own cases ─────────────────────────────────────────────── */

test("every term has to be in the same item, but not the same field", () => {
  const items = [
    text("user", "please fix the parser"),
    text("assistant", "the parser is fixed"),
    toolUse("Bash", "$ cargo test parser", '{"command":"cargo test parser"}'),
    toolResult("Bash", "test result: ok", "running 3 tests\nparser::works ... ok"),
    text("assistant", "all green"),
  ];
  assert.deepEqual(indices(search(items, "parser")), [0, 1, 2, 3]);
  assert.deepEqual(indices(search(items, "parser fixed")), [1], "AND, not OR");
  assert.deepEqual(indices(search(items, "  fixed   parser ")), [1]);
  assert.equal(search(items, "cargo ok").matches.length, 0);
  assert.deepEqual(indices(search(items, "works result")), [3]);
  assert.equal(search(items, "nowhere").matches.length, 0);
});

test("matching ignores case both ways, and a final sigma folds like any other", () => {
  const items = [text("user", "Run The TESTS"), text("assistant", "ΟΔΟΣ, οδος and Straße")];
  assert.deepEqual(indices(search(items, "tests")), [0]);
  assert.deepEqual(indices(search(items, "rUn THE")), [0]);
  assert.deepEqual(indices(search(items, "οδοσ")), [1]);
  assert.deepEqual(indices(search(items, "ΟΔΟΣ")), [1]);
  assert.equal(search(items, "οδος").matches[0]!.ranges.length, 2);
  assert.deepEqual(indices(search(items, "STRASSE")), [], "folding is not expansion");
  assert.deepEqual(indices(search(items, "STRAßE")), [1]);
  const hit = search(items, "tests").matches[0]!;
  assert.equal(slice16(hit.snippet, hit.ranges[0]!), "TESTS");
});

test("CJK matches as a plain substring", () => {
  const items = [text("user", "请帮我修复登录页面的样式问题"), text("assistant", "我已经修改了登录页面，现在样式正常了"), text("user", "ログイン画面を直してください")];
  assert.deepEqual(indices(search(items, "登录页面")), [0, 1]);
  assert.deepEqual(indices(search(items, "登录 正常")), [1]);
  assert.deepEqual(indices(search(items, "画面")), [2]);
  const hit = search(items, "页面").matches[0]!;
  assert.equal(hit.snippet, "请帮我修复登录页面的样式问题");
  assert.deepEqual(hit.ranges, [[7, 9]]);
});

test("a match names the field its snippet came from", () => {
  const items = [
    text("user", "deploy the server"),
    toolUse("Bash", "$ ./deploy.sh staging", '{"command":"./deploy.sh staging","timeout":600000}'),
    toolResult("Bash", "done", "uploading artifacts\nrestarting the service on staging"),
    toolUse("Read", "src/main.rs", '{"file_path":"/w/src/main.rs","limit":40}'),
    item("assistant", "thinking", "maybe the staging box is down"),
  ];
  const result = search(items, "staging");
  assert.deepEqual(
    result.matches.map((m) => [m.index, m.field]),
    [
      [1, "tool"],
      [2, "detail"],
      [4, "text"],
    ],
  );
  assert.equal(result.matches[0]!.snippet, "Bash · $ ./deploy.sh staging");
  assert.equal(result.matches[0]!.kind, "tool_use");
  assert.equal(result.matches[1]!.role, "user");
  const input = search(items, "limit");
  assert.deepEqual([input.matches[0]!.index, input.matches[0]!.field], [3, "detail"]);
  assert.deepEqual(indices(search(items, "read")), [3]);
  assert.deepEqual(indices(search(items, "bash")), [1], "a result only inherits the tool's name");
  const both = search(items, "restarting staging");
  assert.deepEqual([both.matches[0]!.index, both.matches[0]!.field], [2, "detail"]);
});

test("a snippet is cut around the first hit and says where it was cut", () => {
  const before = "a".repeat(300);
  const after = "b".repeat(300);
  let hit = search([text("assistant", `${before} needle ${after}`)], "needle").matches[0]!;
  assert.equal([...hit.snippet].length, SNIPPET_CHARS);
  assert.ok(hit.snippet.startsWith("…") && hit.snippet.endsWith("…"));
  assert.equal(slice16(hit.snippet, hit.ranges[0]!), "needle");
  assert.equal(hit.ranges[0]![0], 1 + 48);

  hit = search([text("assistant", `needle ${after}`)], "needle").matches[0]!;
  assert.ok(!hit.snippet.startsWith("…") && hit.snippet.endsWith("…"));
  assert.equal([...hit.snippet].length, SNIPPET_CHARS);
  assert.deepEqual(hit.ranges, [[0, 6]]);

  hit = search([text("assistant", `${before} needle`)], "needle").matches[0]!;
  assert.ok(hit.snippet.startsWith("…") && !hit.snippet.endsWith("…"));
  assert.equal([...hit.snippet].length, SNIPPET_CHARS);
  assert.ok(hit.snippet.endsWith(" needle"));

  hit = search([text("user", "  look\n\n\tat   this  needle\n")], "needle look").matches[0]!;
  assert.equal(hit.snippet, "look at this needle");
  assert.deepEqual(hit.ranges, [
    [0, 4],
    [13, 19],
  ]);
});

test("ranges count UTF-16 units through multibyte text and emoji", () => {
  const hit = search([text("user", "🎉 发布 v2 🚀 réussi: déploiement OK 🎉")], "déploiement 发布 ok").matches[0]!;
  assert.deepEqual(
    hit.ranges.map((r) => slice16(hit.snippet, r)),
    ["发布", "déploiement", "OK"],
  );
  assert.deepEqual(hit.ranges[0], [3, 5]);
  const long = `${"x".repeat(200)}🚀🚀 target ${"y".repeat(200)}`;
  const cut = search([text("user", long)], "TARGET").matches[0]!;
  assert.equal(slice16(cut.snippet, cut.ranges[0]!), "target");
  assert.equal(cut.ranges[0]![0], 1 + (48 - 3) + 4 + 1);
});

test("overlapping hits of different terms are one range", () => {
  assert.deepEqual(search([text("user", "abcdef and cd")], "abc bcd").matches[0]!.ranges, [[0, 4]]);
  assert.deepEqual(search([text("user", "abcdef and cd")], "ab cd").matches[0]!.ranges, [
    [0, 4],
    [11, 13],
  ]);
  assert.deepEqual(snippetOf("abcdef", ["abc", "bcd"]), { snippet: "abcdef", ranges: [[0, 4]] });
});

test("the newest matches are returned in order and the rest only counted", () => {
  const items = Array.from({ length: 50 }, (_, n) => text("assistant", `step ${n}: ${n % 2 === 0 ? "even" : "odd"}`));
  const limited = search(items, "even", 5);
  assert.equal(limited.total, 25);
  assert.equal(limited.truncated, false);
  assert.deepEqual(indices(limited), [40, 42, 44, 46, 48]);
  const capped = search(items, "step", 5, 20);
  assert.deepEqual([capped.total, capped.truncated], [20, true]);
  assert.deepEqual(indices(capped), [45, 46, 47, 48, 49]);
  const exactly = search(items, "even", 5, 25);
  assert.deepEqual([exactly.total, exactly.truncated], [25, false]);
  const countOnly = search(items, "odd", 0);
  assert.deepEqual([countOnly.total, countOnly.matches.length], [25, 0]);
});

test("a query is validated as the server validates it", () => {
  assert.deepEqual(validateQuery(""), { error: "the query is empty" });
  assert.deepEqual(validateQuery(" \t\n "), { error: "the query is empty" });
  assert.deepEqual(validateQuery("x".repeat(257)), { error: "the query is longer than 256 characters" });
  assert.ok("terms" in validateQuery("界".repeat(256)), "characters, not bytes");
  assert.deepEqual(validateQuery(Array.from({ length: 17 }, (_, n) => `t${n}`).join(" ")), { error: "the query has more than 16 terms" });
  assert.deepEqual(validateQuery(" Foo foo  BAR "), { terms: ["foo", "bar"] });
  assert.equal(search([text("user", "anything")], "   ").matches.length, 0);
});

test("a timestamp and the item's shape come with a match", () => {
  const result = search([text("user", "hello"), item("system", "text", "hello from remote-hand")], "hello");
  assert.equal(result.query, "hello");
  assert.equal(result.matches[0]!.timestamp, "2026-10-01T10:00:00.000Z");
  assert.equal(result.matches[1]!.timestamp, null, "null, not absent");
  assert.equal(result.matches[1]!.role, "system");
  assert.deepEqual(result.matches[0]!.ranges, [[0, 5]]);
});

/* ── the first unread message ───────────────────────────────────────────── */

const stamp = (n: number) => `2026-01-01T10:0${n}:00.000Z`;
const answer = (n: number) => item("assistant", "text", `m${n}`, { timestamp: stamp(n) });

test("the first unread is the oldest answer newer than the mark, and the count every one of them", () => {
  const items = [item("user", "text", "q", { timestamp: stamp(0) }), answer(1), toolUse("Bash", "ls"), answer(2), answer(3), item("assistant", "thinking", "hm", { timestamp: stamp(4) })];
  const copy = { items, startIndex: 0 };
  assert.deepEqual(firstUnreadIn(copy, null, stamp(9)), { index: 1, count: 3 }, "never read: from the very first answer");
  assert.deepEqual(firstUnreadIn(copy, stamp(1), stamp(9)), { index: 3, count: 2 });
  assert.deepEqual(firstUnreadIn(copy, stamp(3), stamp(9)), { index: null, count: 0 });
  assert.deepEqual(firstUnreadIn({ items, startIndex: 500 }, stamp(2), stamp(9)), { index: 504, count: 1 }, "in the feed's numbering");
  const undated = [item("assistant", "text", "no clock")];
  assert.deepEqual(firstUnreadIn({ items: undated, startIndex: 0 }, stamp(5), stamp(9)), { index: 0, count: 1 }, "an answer with no time stands at the fallback");
});

test("the read mark from the unread ledger: the last answer read", () => {
  const items = [answer(1), answer(2), answer(3), answer(4)];
  const row = (unreadCount: number) => ({ unreadCount, attentionAt: stamp(4), updatedAt: stamp(9) });
  assert.equal(readMarkOf(row(2), { items }), stamp(2), "two unread: read up to the second");
  assert.equal(readMarkOf(row(0), { items }), stamp(4), "none unread: read at the last answer");
  assert.equal(readMarkOf(row(4), { items }), null, "all held are unread: from the start");
  assert.equal(readMarkOf(row(2), null), null);
  for (const n of [0, 1, 2, 3, 4]) {
    assert.equal(firstUnreadIn({ items, startIndex: 0 }, readMarkOf(row(n), { items }), stamp(9)).count, n, `the mark finds exactly ${n}`);
  }
});

/* ── over the offline transport ─────────────────────────────────────────── */

const summary = (id: string, extra: Partial<SessionSummary> = {}) =>
  ({ source: "claude", id, title: id, cwd: "/w", updatedAt: stamp(9), activity: "idle", status: "idle", scope: "owned", unreadCount: 0, attentionAt: null, ...extra }) as unknown as SessionSummary;

class Source implements OfflineSource {
  list = [summary("a", { unreadCount: 2, attentionAt: stamp(4) }), summary("bare")];
  copies = new Map<string, SessionCopy>();
  whenLoaded = async () => undefined;
  status = null;
  rows() {
    return this.list;
  }
  row(source: string, id: string) {
    return this.list.find((r) => r.source === source && r.id === id);
  }
  async session(source: string, id: string) {
    return this.copies.get(`${source}/${id}`) ?? null;
  }
  sessionNow(source: string, id: string) {
    return this.copies.get(`${source}/${id}`) ?? null;
  }
  touch() {}
  onChange(_fn: (c: MirrorChange) => void) {
    return () => undefined;
  }
}

const opts: OfflineOptions = { user: "cloud:d-1", messages: { readOnly: "ro", noCopy: "none", unavailable: "the machine is offline" }, machineOnline: () => false };

async function call(source: OfflineSource, path: string, init?: RequestInit) {
  const res = await offlineTransport(source, opts).fetch(new Request(`http://page${path}`, init));
  return { status: res.status, body: (await res.json()) as Record<string, any> };
}

function withCopy(source: Source, items: TranscriptItem[], startIndex = 0): void {
  const state: FeedCopy = { started: true, startIndex, endIndex: startIndex + items.length, items, fp: "f", prefixFp: "p" };
  source.copies.set("claude/a", { source: "claude", id: "a", seq: 3, freshestMs: 1, state });
}

test("offline: a search answers over the copy, in the feed's numbering, with the server's refusals", async () => {
  const source = new Source();
  withCopy(source, [text("user", "deploy it"), text("assistant", "deployed to staging"), text("assistant", "nothing here")], 1000);
  const found = await call(source, "/api/sessions/claude/a/search?q=DEPLOY&limit=200");
  assert.equal(found.status, 200);
  assert.deepEqual(found.body.matches.map((m: { index: number }) => m.index), [1000, 1001]);
  assert.equal(found.body.total, 2);
  assert.equal(found.body.query, "DEPLOY");
  const limited = await call(source, "/api/sessions/claude/a/search?q=deploy&limit=1");
  assert.deepEqual([limited.body.total, limited.body.matches.length, limited.body.matches[0].index], [2, 1, 1001], "the newest one described");

  assert.deepEqual(await call(source, "/api/sessions/claude/a/search?q=%20%20"), { status: 400, body: { error: "the query is empty" } });
  assert.deepEqual(await call(source, "/api/sessions/claude/nope/search?q=x"), { status: 404, body: { error: "session not found" } });
  assert.deepEqual(await call(source, "/api/sessions/claude/bare/search?q=x"), {
    status: 200,
    body: { query: "x", total: 0, truncated: false, matches: [] },
  }, "no copy: nothing matches, rather than a failure");
});

test("offline: the first unread from the mark a seen report hands back, which records nothing", async () => {
  const source = new Source();
  withCopy(source, [item("user", "text", "q", { timestamp: stamp(0) }), answer(1), answer(2), toolUse("Bash", "ls"), answer(3)], 40);
  const seen = await call(source, "/api/sessions/claude/a/seen", { method: "POST" });
  assert.deepEqual(seen, { status: 200, body: { ok: true, previousReadAt: stamp(1) } }, "two unread on the ledger: read up to m1");
  const located = await call(source, `/api/sessions/claude/a/first-unread?after=${encodeURIComponent(seen.body.previousReadAt)}`);
  assert.deepEqual(located, { status: 200, body: { index: 42, count: 2 } });
  assert.deepEqual((await call(source, "/api/sessions/claude/a/first-unread")).body, { index: 41, count: 3 }, "never read: every answer held");

  assert.deepEqual(await call(source, "/api/sessions/claude/bare/first-unread"), { status: 200, body: { index: null, count: 0 } }, "no copy: nothing to jump to");
  assert.deepEqual(await call(source, "/api/sessions/claude/bare/seen", { method: "POST" }), { status: 200, body: { ok: true, previousReadAt: null } });
  assert.equal((await call(source, "/api/sessions/claude/nope/first-unread")).status, 404);
  assert.equal((await call(source, "/api/sessions/claude/nope/seen", { method: "POST" })).status, 404);
  // Every other write is still the machine's.
  assert.equal((await call(source, "/api/sessions/claude/a/message", { method: "POST", body: "{}" })).status, 503);
});
