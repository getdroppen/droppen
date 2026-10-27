// The `draft` frame: the count of a think in flight, which is all a page
// takes from it — what it is read as (from this agent and from one before,
// which also sent finished blocks), the store it lands in, and the words in
// the activity bar it becomes. Nothing of it reaches the transcript.
//
// `react-dom/server` with no DOM, like `transcript-rows.test.tsx`, and
// `import * as React` for the same reason: the test directory is outside the
// tsconfig's `include`, so tsx compiles JSX here with the classic runtime.
// The count moving under a mounted bar is `turn-status.test.tsx`'s (jsdom).
import * as React from "react";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { approxTokens, createDraftStore, thinkingOf } from "../src/features/sessions/stream-draft.js";
import { TurnStatus } from "../src/features/sessions/turn-status.js";
import type { TurnProgress } from "../src/features/sessions/session-activity.js";
import {
  applyStateFrame,
  transcriptWindowOf,
  type SessionStateFields,
} from "../src/features/sessions/session-feed-state.js";
import { makeI18n, pageDraw } from "./render.js";

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} } }
});

const draw = pageDraw(i18n);

/** The text a reader sees: no tags, and not the invisible copy that holds the count's width. */
function visibleText(html: string): string {
  return html.replace(/<span aria-hidden="true" data-ghost=""[^>]*>[^<]*<\/span>/g, "").replace(/<[^>]*>/g, "");
}

/** A frame as this server writes one. */
const counting = (tokens: number) => ({ type: "draft", turn: 3, v: 17, blocks: [{ index: 0, kind: "thinking", thinkingTokens: tokens }] });

/** A frame as an agent from before writes one: finished blocks beside the counter. */
const before = {
  type: "draft",
  turn: 3,
  v: 17,
  blocks: [
    { uuid: "u-think", msgId: "msg_1", index: 0, kind: "thinking", thinkingTokens: 1240, done: true },
    { uuid: "u-text", msgId: "msg_1", index: 0, kind: "text", text: "I'll write the **new** module now." },
    { uuid: "u-tool", msgId: "msg_1", index: 0, kind: "tool_use", toolName: "Write", toolSummary: "/w/repo/src/drafts.rs", toolUseId: "toolu_1" },
    { index: 0, kind: "thinking", thinkingTokens: 42 },
  ],
};

/* ── reading the frame ─────────────────────────────────────────────────── */

test("a draft frame is read for its count, from this agent and from one before", () => {
  assert.equal(thinkingOf(counting(42)), 42);
  assert.equal(thinkingOf(before), 42, "the counter, not the finished think or the words beside it");
  assert.equal(thinkingOf({ ...before, blocks: before.blocks.slice(0, 3) }), null, "finished blocks alone: nothing is being thought");
  assert.equal(thinkingOf({ type: "draft", blocks: [{ index: 0, kind: "thinking" }] }), 0, "a counter with no count yet is a think at nought");
});

test("an empty or unreadable draft is no think, never an error", () => {
  assert.equal(thinkingOf({ type: "draft", turn: 3, v: 18, blocks: [] }), null, "empty: the think is over");
  assert.equal(thinkingOf({ type: "draft" }), null);
  assert.equal(thinkingOf({ type: "draft", blocks: "nope" }), null);
  assert.equal(thinkingOf({ type: "state", blocks: counting(5).blocks }), null, "only a draft frame is a draft");
  assert.equal(thinkingOf({ type: "draft", blocks: [null, 7, { kind: "image" }, { kind: "thinking", thinkingTokens: -3 }] }), 0);
});

/** An older client routes frames by `type` and ignores the rest; this one's
 * other readers must take nothing from a draft frame if one ever reached them. */
test("a draft frame carries nothing the window or the state readers would take", () => {
  assert.equal(transcriptWindowOf(before), null, "no `sig`, so it is no window");
  const state = {
    summary: { source: "claude", id: "s1" },
    injection: null,
    canSend: true,
    sendBlockReason: null,
    cliChoice: null,
    contextTokens: null,
    contextWindow: 200_000,
    claudeRoot: "managed",
    account: null,
    rotation: null,
    receipts: [],
    questions: [],
  } as unknown as SessionStateFields;
  assert.equal(applyStateFrame(state, before as Partial<SessionStateFields>), state, "and changes no state field");
});

/* ── the store ─────────────────────────────────────────────────────────── */

test("the store wakes its readers when the count moves, and only then", () => {
  const store = createDraftStore();
  let woken = 0;
  const stop = store.subscribe(() => woken++);
  store.receive(40, 1_000);
  assert.deepEqual(store.get(), { tokens: 40, movedAt: 1_000 });
  const first = store.get();
  store.receive(40, 2_000);
  assert.equal(woken, 1, "the same count again is nothing to redraw");
  assert.equal(store.get(), first, "and the very same object");
  store.receive(80, 3_000);
  assert.equal(woken, 2);
  store.receive(null, 4_000);
  assert.deepEqual(store.get(), { tokens: null, movedAt: 4_000 }, "the think ending is its last move");
  store.receive(null, 5_000);
  assert.equal(woken, 3);

  // The socket going: whatever was in flight is gone, when it last moved is not.
  store.receive(12, 6_000);
  store.clear();
  assert.deepEqual(store.get(), { tokens: null, movedAt: 6_000 });
  const heard = woken;
  store.clear();
  stop();
  store.receive(99, 7_000);
  assert.equal(woken, heard, "nothing to clear wakes nobody, and an unsubscribed reader hears nothing");
});

test("a long think reads in thousands, never wider than its five-character slot", () => {
  assert.equal(approxTokens(0), "~0");
  assert.equal(approxTokens(850), "~850");
  assert.equal(approxTokens(1000), "~1k");
  assert.equal(approxTokens(1240), "~1.2k");
  assert.equal(approxTokens(12_400), "~12k");
  for (const tokens of [0, 9, 99, 999, 1049, 9_949, 9_999, 99_499, 999_499]) {
    assert.ok(approxTokens(tokens).length <= 5, `${tokens} → ${approxTokens(tokens)}`);
  }
});

/* ── the words in the bar ─────────────────────────────────────────────── */

const NOW = Date.parse("2026-10-21T12:00:00Z");
const turn: TurnProgress = {
  messages: 28,
  startedAt: new Date(NOW - (7 * 60 + 14) * 1000).toISOString(),
  lastActivityAt: new Date(NOW - 45_000).toISOString(),
};

test("while the agent thinks, the bar says so in place of its last activity", () => {
  const store = createDraftStore();
  store.receive(thinkingOf(before), NOW - 1_000);
  const html = draw(<TurnStatus drafts={store} progress={turn} now={NOW} />);
  assert.equal(visibleText(html), "28 messages · thinking… ~42 tokens · 7m14s");
  assert.doesNotMatch(html, /last activity/, "the count moving is the activity");
  // The count right-aligned in a cell held at the widest count by an
  // invisible copy nothing reads out, so nothing after it moves as it counts.
  assert.match(
    html,
    /<span class="inline-grid tabular-nums"><span data-thinking-tokens="" class="col-start-1 row-start-1 justify-self-end">~42<\/span><span aria-hidden="true" data-ghost="" class="invisible col-start-1 row-start-1">~0\.0k<\/span><\/span>/,
  );
  assert.equal(approxTokens(1240).length, "~0.0k".length);
  // The unit only where the bar is wide enough for it.
  assert.match(html, /class="hidden @min-\[26rem\]\/activity:inline"> tokens</);
  assert.doesNotMatch(html, /title=/, "no tooltip: a phone could not reach it");
});

test("the bar shows nothing of a draft but the count", () => {
  const store = createDraftStore();
  store.receive(thinkingOf(before), NOW - 1_000);
  const text = visibleText(draw(<TurnStatus drafts={store} progress={turn} now={NOW} />));
  assert.doesNotMatch(text, /write the|module|Write|drafts\.rs|1\.2k|not in the transcript yet/);
});

test("once the think is over, last activity counts it — within this turn only", () => {
  const store = createDraftStore();
  // The transcript last moved 45s ago; the think ended 3s ago.
  store.receive(500, NOW - 20_000);
  store.receive(null, NOW - 3_000);
  assert.equal(visibleText(draw(<TurnStatus drafts={store} progress={turn} now={NOW} />)), "28 messages · last activity 3s ago · 7m14s");
  // A think that ended before this turn began is not this turn's activity.
  const later: TurnProgress = { ...turn, startedAt: new Date(NOW - 2_000).toISOString(), lastActivityAt: null };
  assert.equal(visibleText(draw(<TurnStatus drafts={store} progress={later} now={NOW} />)), "28 messages · 2s");
  // And a turn the transcript wrote to since keeps its own, newer, time.
  const wrote: TurnProgress = { ...turn, lastActivityAt: new Date(NOW - 1_000).toISOString() };
  assert.equal(visibleText(draw(<TurnStatus drafts={store} progress={wrote} now={NOW} />)), "28 messages · last activity 1s ago · 7m14s");
  // No store: the transcript's time, as it always was.
  assert.equal(visibleText(draw(<TurnStatus progress={turn} now={NOW} />)), "28 messages · last activity 45s ago · 7m14s");
});

test("a think with nothing to date the turn by is still said, and the old sentence is the fallback", () => {
  assert.equal(visibleText(draw(<TurnStatus progress={null} now={NOW} />)), "agent is working on your message…");
  const store = createDraftStore();
  store.receive(12_400, NOW);
  assert.equal(visibleText(draw(<TurnStatus drafts={store} progress={null} now={NOW} />)), "thinking… ~12k tokens");
});

/**
 * The transcript is append-only: it grows by written entries and by nothing
 * else. The provisional bubble that used to sit below the last row came and
 * went and changed height while the agent worked, so the draft store must not
 * reach the scroller at all — only the composer, for the activity bar.
 */
test("the transcript draws nothing provisional: the draft store reaches only the activity bar", () => {
  const source = readFileSync(new URL("../src/features/sessions/session-detail.tsx", import.meta.url), "utf8");
  const handedTo = [...source.matchAll(/drafts=\{/g)].map((match) => {
    const before = source.slice(0, match.index);
    return /<([A-Z]\w*)\b[^<]*$/.exec(before)?.[1];
  });
  assert.deepEqual(
    handedTo.sort(),
    ["ActivityBar", "Composer", "TurnStatus"],
    "the composer, which hands it to the bar, which hands it to its words",
  );
  assert.doesNotMatch(source, /data-draft|stream-draft-view|useSyncExternalStore\(\s*drafts/);
});
