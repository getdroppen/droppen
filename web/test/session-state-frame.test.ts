// What a `state` frame is allowed to cost.
//
// A session pane holds up to `LIVE_WINDOW_CAP` rows and none of them are
// virtualized, so the price of a render is set by the size of the window rather
// than by the size of the screen. That makes the interesting question about the
// state half of the feed not "is the merge correct" — `session-feed-state.ts`
// has always been that — but "does a frame that says nothing new produce a new
// object", because a new object is a render of every row in the window. The
// server pushes a state frame roughly every other second while a session works,
// and it re-serializes the whole state each time: the summary in it is a fresh
// object on every frame whether or not a single field in it moved.
//
// So these tests are about identity as much as about values. They also pin the
// `undefined`-versus-`null` distinction the merge has always had, which is not
// pedantry: collapsing the two is how the composer used to lock itself against
// a server too old to send a field.
import assert from "node:assert/strict";
import test from "node:test";
import {
  applyStateFrame,
  sameJson,
  type SessionStateFields,
  type TranscriptItem,
} from "../src/features/sessions/session-feed-state.js";
import { chunkItems } from "../src/features/sessions/transcript-groups.js";
import { transcriptRowDeps } from "../src/features/sessions/session-detail.js";
import type { components } from "../src/api/schema.js";
import { transcriptItem } from "./session-fixtures.js";

type SessionSummary = components["schemas"]["SessionSummary"];

/** The client's whole feed state: the transcript window plus the state fields. */
type FeedState = SessionStateFields & {
  sig: string;
  items: TranscriptItem[];
  startIndex: number;
  endIndex: number;
  totalItems: number;
  hasMoreBefore: boolean;
  omittedItems: number;
};

function summary(over: Partial<SessionSummary> = {}): SessionSummary {
  return {
    activity: "working",
    attentionAt: null,
    createdAt: "2026-09-10T10:00:00Z",
    cwd: "/Users/ada/Projects/remote-hand",
    filePath: "/tmp/s1.jsonl",
    gitBranch: "main",
    id: "s1",
    repoKey: "/Users/ada/Projects/remote-hand",
    resident: null,
    scope: "owned",
    sizeBytes: 4096,
    source: "claude",
    status: "running",
    title: "a session",
    unread: false,
    updatedAt: "2026-09-10T10:00:00Z",
    ...over,
  };
}

const item = (text: string) => transcriptItem({ text });

function state(over: Partial<FeedState> = {}): FeedState {
  const items = [item("0"), item("1")];
  return {
    sig: "v1",
    items,
    startIndex: 0,
    endIndex: items.length,
    totalItems: items.length,
    hasMoreBefore: false,
    omittedItems: 0,
    summary: summary(),
    injection: null,
    canSend: true,
    sendBlockReason: null,
    cliChoice: { at: null, effort: "high", model: "claude-opus-5" },
    contextTokens: 1234,
    contextWindow: 200_000,
    claudeRoot: "managed",
    account: { id: "acct-1", label: "work" },
    ...over,
  };
}

/**
 * The state fields as they arrive: through the wire, so nothing is shared.
 *
 * The round trip is the whole point of these tests. A frame built by spreading
 * the state under test would hand the merge the very objects it already holds,
 * and every identity assertion below would pass for the wrong reason.
 */
function frameOf(fields: Partial<SessionStateFields>): Partial<SessionStateFields> {
  return JSON.parse(JSON.stringify(fields)) as Partial<SessionStateFields>;
}

test("a state frame that says nothing new is answered with the same object", () => {
  const prev = state();
  const heartbeat = frameOf({
    summary: prev.summary,
    injection: prev.injection,
    canSend: prev.canSend,
    sendBlockReason: prev.sendBlockReason,
    cliChoice: prev.cliChoice,
    contextTokens: prev.contextTokens,
    contextWindow: prev.contextWindow,
    claudeRoot: prev.claudeRoot,
    account: prev.account,
  });

  // Not `deepEqual`: `equal` on two objects is `Object.is`, and identity is the
  // assertion — a new object here is a render of the whole transcript.
  assert.equal(applyStateFrame(prev, heartbeat), prev);
});

test("a state frame that changes one field keeps every other by identity", () => {
  const prev = state();
  const next = applyStateFrame(
    prev,
    frameOf({ summary: prev.summary, canSend: false, sendBlockReason: "a turn is running" }),
  );

  assert.notEqual(next, prev);
  assert.equal(next.canSend, false);
  assert.equal(next.sendBlockReason, "a turn is running");
  // The summary arrived as a fresh object saying the same thing. Holding the
  // one already there is what lets the memos keyed on it — the resident task
  // set, the activity banner, the background panel — skip.
  assert.equal(next.summary, prev.summary);
  assert.equal(next.cliChoice, prev.cliChoice);
  assert.equal(next.account, prev.account);
  // A state frame carries no transcript, so the window must come through
  // untouched, right down to the array the row memo is keyed on.
  assert.equal(next.items, prev.items);
  assert.equal(next.sig, prev.sig);
  assert.equal(next.endIndex, prev.endIndex);
});

test("a summary that really moved replaces the one held", () => {
  const prev = state();
  const next = applyStateFrame(prev, frameOf({ summary: summary({ activity: "idle" }) }));

  assert.notEqual(next, prev);
  assert.notEqual(next.summary, prev.summary);
  assert.equal(next.summary.activity, "idle");
  assert.equal(prev.summary.activity, "working");
});

test("`null` clears a field that a run ending has to clear", () => {
  const prev = state({
    injection: {
      error: null,
      finishedAt: null,
      interrupted: false,
      startedAt: "2026-09-10T10:00:00Z",
      state: "running",
    },
    sendBlockReason: "a turn is running",
    contextTokens: 1234,
    account: { id: "acct-1", label: "work" },
  });

  const next = applyStateFrame(
    prev,
    frameOf({
      summary: prev.summary,
      injection: null,
      sendBlockReason: null,
      contextTokens: null,
      account: null,
      claudeRoot: null,
    }),
  );

  assert.equal(next.injection, null);
  assert.equal(next.sendBlockReason, null);
  assert.equal(next.contextTokens, null);
  assert.equal(next.account, null);
  assert.equal(next.claudeRoot, null);
});

test("a field a server is too old to send leaves what the snapshot established", () => {
  const prev = state({
    injection: {
      error: null,
      finishedAt: null,
      interrupted: false,
      startedAt: "2026-09-10T10:00:00Z",
      state: "running",
    },
  });

  // Every field of the frame absent: an older server, or a shape this client
  // has grown past. Nothing may be wiped, and nothing may be allocated either.
  assert.equal(applyStateFrame(prev, {}), prev);

  // And a frame that omits only some of them keeps exactly those.
  const next = applyStateFrame(prev, frameOf({ summary: summary({ activity: "idle" }) }));
  assert.equal(next.injection, prev.injection);
  assert.equal(next.contextTokens, prev.contextTokens);
  assert.equal(next.claudeRoot, prev.claudeRoot);
  assert.equal(next.account, prev.account);
});

test("a rotation arrives, stays when a frame leaves it out, and clears on null", () => {
  const prev = state();
  const rotation = {
    from: { id: "acct-1", label: "work" },
    to: { id: "acct-2", label: "spare" },
    at: "2026-09-10T10:05:00Z",
    class: "session" as const,
    scope: "all",
    untilAt: "2026-09-10T14:30:00Z",
    interruptedTasks: 0,
    outcome: "switched" as const,
    trigger: "limit" as const,
  };
  const moved = applyStateFrame(prev, frameOf({ account: rotation.to, rotation }));
  assert.deepEqual(moved.rotation, rotation);

  // A server that reports it only on the snapshot sends frames without it;
  // those keep what the snapshot said, by identity.
  const later = applyStateFrame(moved, frameOf({ summary: summary({ activity: "idle" }) }));
  assert.equal(later.rotation, moved.rotation);
  assert.equal(applyStateFrame(moved, frameOf({ rotation })), moved);

  assert.equal(applyStateFrame(moved, frameOf({ rotation: null })).rotation, null);

  // A state that never had one does not grow an `undefined` member.
  const plain = applyStateFrame(prev, frameOf({ canSend: false }));
  assert.equal(Object.hasOwn(plain, "rotation"), false);
});

test("a sticky field is not cleared by a null, because null is not a value it takes", () => {
  const prev = state();
  // `cliChoice` and `contextWindow` are read with `??` rather than by presence:
  // the server sends `null` for a session whose transcript has not said what it
  // ran with yet, and the last reading is better than blanking the chip.
  const next = applyStateFrame(prev, frameOf({ cliChoice: null, contextWindow: undefined }));
  assert.equal(next, prev);
  assert.equal(next.cliChoice, prev.cliChoice);
  assert.equal(next.contextWindow, 200_000);
});

// The frame the fix is really aimed at. A working session rewrites its file
// constantly, so `sizeBytes` and `updatedAt` move on nearly every frame while
// everything the transcript is drawn from stands still. The state legitimately
// changes — there is no bailing out of the render — so what has to hold is the
// layer below: every input the row list is memoized on must be the same object
// afterwards, or the memo is decorative and the window is rebuilt anyway.
test("a frame that only moved the byte count leaves the transcript's memo inputs alone", () => {
  const prev = state();
  const next = applyStateFrame(
    prev,
    frameOf({
      summary: summary({ sizeBytes: 8192, updatedAt: "2026-09-10T10:00:02Z" }),
      canSend: prev.canSend,
      contextTokens: prev.contextTokens,
    }),
  );
  assert.notEqual(next, prev, "the byte count really did move");

  // `chunks` is itself a `useMemo` keyed on `items`, so this is what the
  // component would hold: the same array, recomputed only if `items` moved.
  const chunks = chunkItems(prev.items, true);
  const chunksAfter = next.items === prev.items ? chunks : chunkItems(next.items, true);
  const session = { source: "claude", id: "s1" } as const;

  const before = transcriptRowDeps(chunks, "flat", prev, session);
  const after = transcriptRowDeps(chunksAfter, "flat", next, session);
  assert.equal(before.length, after.length);
  before.forEach((dep, index) => {
    assert.ok(Object.is(dep, after[index]), `row dependency ${index} was replaced`);
  });
});

// And the other half of that: an input that really changed must invalidate it,
// or the transcript would go stale under a delta.
test("a window that grew replaces the inputs the row list is memoized on", () => {
  const prev = state();
  const grown: FeedState = {
    ...prev,
    items: [...prev.items, item("2")],
    endIndex: prev.endIndex + 1,
    totalItems: prev.totalItems + 1,
  };
  const session = { source: "claude", id: "s1" } as const;

  const before = transcriptRowDeps(chunkItems(prev.items, true), "flat", prev, session);
  const after = transcriptRowDeps(chunkItems(grown.items, true), "flat", grown, session);
  assert.ok(
    before.some((dep, index) => !Object.is(dep, after[index])),
    "a delta has to reach the rows",
  );
});

/* ── questions ──────────────────────────────────────────────────────────── */

type SessionQuestion = components["schemas"]["SessionQuestion"];

function question(over: Partial<SessionQuestion> = {}): SessionQuestion {
  return {
    requestId: "req-1",
    toolUseId: "toolu_1",
    questions: [
      {
        question: "Which colour do you prefer?",
        header: "Colour",
        multiSelect: false,
        options: [
          { label: "Red", description: null },
          { label: "Blue", description: null },
        ],
      },
    ],
    state: "pending",
    answers: null,
    response: null,
    answeredBy: null,
    answeredAt: null,
    createdAt: "2026-10-10T09:00:00Z",
    ...over,
  };
}

// The question cards look their records up in this list through a context, so
// a fresh array on every frame would wake every card in the window twice a
// second — the list has to be held exactly the way the summary is.
test("a question list that says the same thing is held by identity", () => {
  const prev = state({ questions: [question()] });
  const same = applyStateFrame(prev, frameOf({ questions: prev.questions }));
  assert.equal(same, prev, "a frame that only repeats the questions changes nothing");

  const moved = applyStateFrame(
    prev,
    frameOf({ questions: prev.questions, summary: summary({ sizeBytes: 8192 }) }),
  );
  assert.notEqual(moved, prev);
  assert.equal(moved.questions, prev.questions, "the byte count moved; the questions did not");
});

test("a question that opened or closed replaces the list", () => {
  const prev = state({ questions: [question()] });
  const answered = question({ state: "answered", answers: { "Which colour do you prefer?": "Red" }, answeredBy: "alice" });
  const next = applyStateFrame(prev, frameOf({ questions: [answered] }));
  assert.notEqual(next, prev);
  assert.notEqual(next.questions, prev.questions);
  assert.equal(next.questions[0]!.state, "answered");
  assert.equal(next.summary, prev.summary, "and nothing else moved");

  // Emptied is a real answer, not an absence.
  assert.deepEqual(applyStateFrame(next, frameOf({ questions: [] })).questions, []);
});

test("a frame from a server too old to send questions keeps the ones held", () => {
  const prev = state({ questions: [question()] });
  // Absent altogether: nothing to merge, so nothing may be allocated.
  assert.equal(applyStateFrame(prev, frameOf({ canSend: prev.canSend })), prev);
  const next = applyStateFrame(prev, frameOf({ canSend: false }));
  assert.equal(next.questions, prev.questions);
  // And a state that never had the field does not grow an `undefined` one.
  const bare = state();
  assert.equal(Object.hasOwn(applyStateFrame(bare, frameOf({ canSend: false })), "questions"), false);
});

// The other half of what the cards cost: a question moving reaches its card
// through the context, and must not rebuild the row list on the way — nor must
// a turn starting or ending, which is what `canSend` flipping is. Both used to
// be in the row memo's keys, for a card that answered through the composer.
test("a question or a turn changing leaves the row list's memo inputs alone", () => {
  const prev = state({ questions: [question()] });
  const next = applyStateFrame(
    prev,
    frameOf({
      questions: [question({ state: "answered", answeredBy: "alice" })],
      canSend: false,
      sendBlockReason: "a turn is running",
    }),
  );
  assert.notEqual(next.questions, prev.questions, "the question really did move");

  const chunks = chunkItems(prev.items, true);
  const session = { source: "claude", id: "s1" } as const;
  const before = transcriptRowDeps(chunks, "flat", prev, session);
  const after = transcriptRowDeps(next.items === prev.items ? chunks : chunkItems(next.items, true), "flat", next, session);
  before.forEach((dep, index) => {
    assert.ok(Object.is(dep, after[index]), `row dependency ${index} was replaced`);
  });
});

test("sameJson walks structure rather than serialization", () => {
  assert.ok(sameJson({ a: [1, { b: null }] }, { a: [1, { b: null }] }));
  // Key order is a property of how a value was built, not of what it says.
  assert.ok(sameJson({ a: 1, b: 2 }, { b: 2, a: 1 }));
  assert.ok(!sameJson({ a: 1 }, { a: 1, b: undefined }), "an extra key is a difference");
  assert.ok(!sameJson(null, {}));
  assert.ok(!sameJson([1, 2], [1, 2, 3]));
  assert.ok(!sameJson({ a: [1] }, { a: [2] }));
});
