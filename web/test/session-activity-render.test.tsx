// The rendering half of `session-activity.test.ts`.
//
// That file pins the *decisions* — which of the four states a row is in, and
// whether a Stop belongs beside it. This one pins that the components actually
// draw them: the bug being fixed was a banner that never appeared, and a pure
// function returning the right descriptor into a component that ignores it
// would reproduce it exactly.
//
// `react-dom/server` rather than a DOM: these three components are static
// markup with no effects and no events beyond one `onClick`, so a string of
// HTML answers every question worth asking and the repo needs no new runner.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { StatusBadge } from "../src/features/sessions/sessions-layout.js";
import { ActivityBar, AwaitingBackgroundNotice } from "../src/features/sessions/session-detail.js";
import { BackgroundTaskPanel } from "../src/features/sessions/background-panel.js";
import { createDraftStore, thinkingOf } from "../src/features/sessions/stream-draft.js";
import type {
  ActivityRow,
  LiveSubtask,
  ResidentInfo,
} from "../src/features/sessions/session-activity.js";
import { resident } from "./session-fixtures.js";
import { makeI18n, pageDraw } from "./render.js";

// English keys are the source strings, so an empty resource bundle renders the
// same text the untranslated UI does — and asserting on it asserts on the key.
const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} } }
});

const draw = pageDraw(i18n);

/* ── the sidebar badge ──────────────────────────────────────────────────── */

test("a working row draws the live dot and says so in words", () => {
  const row: ActivityRow = { status: "running", activity: "working", resident: resident({ phase: "working" }) };
  const html = draw(<StatusBadge session={row} />);
  assert.match(html, />working</);
  assert.match(html, /data-live/, "the one state that blinks");
  assert.match(html, /bg-ok/, "green: a turn is in flight");
  assert.doesNotMatch(html, /bg-accent|bg-warn/);
});

test("a working row says how long the turn has run, once there is something to say", () => {
  const started = new Date(Date.now() - 3 * 60_000 - 5_000).toISOString();
  const row: ActivityRow = {
    status: "running",
    activity: "working",
    resident: resident({ phase: "working", turnStartedAt: started }),
  };
  assert.match(draw(<StatusBadge session={row} />), /working · 3m/);
  const fresh: ActivityRow = {
    status: "running",
    activity: "working",
    resident: resident({ phase: "working", turnStartedAt: new Date().toISOString() }),
  };
  assert.match(draw(<StatusBadge session={fresh} />), />working</, "under a minute, the word alone");
});

test("a row with a queued message says how many, without blinking", () => {
  // The state the sidebar used to render as plain idle, which is what made a
  // sent message look lost.
  const row: ActivityRow = { status: "running", activity: "queued", resident: resident({ queued: 2 }) };
  const html = draw(<StatusBadge session={row} />);
  assert.match(html, /queued 2/, "the count is the part that says a message of yours is in there");
  assert.doesNotMatch(html, /data-live/, "only a turn in flight moves");
});

test("a working row shows a queue stacked up behind the turn", () => {
  const row: ActivityRow = {
    status: "running",
    activity: "working",
    resident: resident({ phase: "working", queued: 1 }),
  };
  assert.match(draw(<StatusBadge session={row} />), /working · \+1/);
});

test("background work is listed now, in its own colour and with its count", () => {
  const row: ActivityRow = { status: "idle", activity: "background", resident: resident({ background: 3 }) };
  const html = draw(<StatusBadge session={row} />);
  assert.match(html, /background 3/);
  assert.match(html, /text-muted/, "busy, but needs nothing from you: a grey outline, not a signal colour");
  assert.doesNotMatch(html, /bg-ok|data-live/);
  assert.match(html, /3 background tasks still running/, "the count is in the tooltip too");
  const unread = draw(<StatusBadge session={row} unread={2} />);
  assert.match(unread, /background 3/);
  assert.match(unread, /data-unread="2"/, "and unread sits beside it");
});

test("a question for you is the loudest chip: filled amber", () => {
  const html = draw(<StatusBadge session={{ status: "waiting", activity: "idle", asking: true }} />);
  assert.match(html, />asking you</);
  assert.match(html, /bg-warn text-on-signal/, "the one coloured fill, with ink on it in both themes");
  assert.match(html, /the agent is asking you a question/, "and the tooltip says what it wants");
  // It wins over a turn in flight and over unread, which still sits beside it.
  const busy: ActivityRow = { status: "running", activity: "working", resident: resident({ phase: "working" }), asking: true };
  const both = draw(<StatusBadge session={busy} unread={3} />);
  assert.match(both, />asking you</);
  assert.doesNotMatch(both, />working</);
  assert.match(both, /data-unread="3"/);
});

test("a turn that simply finished draws no chip: it is idle", () => {
  // The reported bug: every stopped session wore an amber "waiting for you".
  assert.equal(draw(<StatusBadge session={{ status: "waiting", activity: "idle" }} />), "");
  const html = draw(<StatusBadge session={{ status: "waiting", activity: "idle", asking: false }} unread={2} />);
  assert.doesNotMatch(html, /bg-warn|waiting for you|asking you/);
  assert.match(html, /data-unread="2"/, "its new answers are unread, in ink");
});

test("an idle row with nothing unread draws nothing at all", () => {
  assert.equal(draw(<StatusBadge session={{ status: "idle", activity: "idle" }} />), "");
});

test("unread is a count, drawn beside activity, never instead of it", () => {
  // Two independent facts: a running session can gain unread output, and an
  // idle one can be holding some.
  const row: ActivityRow = { status: "running", activity: "working", resident: resident({ phase: "working" }) };
  const html = draw(<StatusBadge session={row} unread={3} />);
  assert.match(html, /bg-unread/, "the unread pill");
  assert.match(html, />3</, "a number, not a dot");
  assert.match(html, /3 unread messages/);
  assert.match(html, />working</);
  assert.match(draw(<StatusBadge session={{ status: "idle", activity: "idle" }} unread={250} />), />99\+</);
});

/* ── the strip above the composer ───────────────────────────────────────── */

test("a working banner offers Stop, because there is a turn to stop", () => {
  const html = draw(
    <ActivityBar
      banner={{ kind: "working", queued: 0, stalled: false }}
      subtask={null}
      stopping={false}
      onStop={() => {}}
    />,
  );
  assert.match(html, /agent is working on your message/);
  assert.match(html, /stop<\/button>/);
});

test("a queued banner does not offer Stop", () => {
  // `queued` is reached with the resident idle, and the interrupt route answers
  // 409 for an idle resident: the button could only ever produce an error.
  const html = draw(
    <ActivityBar
      banner={{ kind: "queued", queued: 1, awaitingBackground: false, background: 0 }}
      subtask={null}
      stopping={false}
    />,
  );
  assert.match(html, /1 messages queued/);
  assert.doesNotMatch(html, /<button/);
});

test("a stalled turn says so beside the Stop it justifies", () => {
  const html = draw(
    <ActivityBar
      banner={{ kind: "working", queued: 0, stalled: true }}
      subtask={null}
      stopping={false}
      onStop={() => {}}
    />,
  );
  assert.match(html, /no output for a while/);
  assert.match(html, /stop<\/button>/);
});

test("a working turn says how much it has written, when, and for how long", () => {
  // Deliberately not a progress bar: nothing on either
  // side of the wire knows how many steps a turn will take, so a bar could only
  // carry a denominator somebody invented for it.
  const html = draw(
    <ActivityBar
      banner={{ kind: "working", queued: 0, stalled: false }}
      subtask={null}
      progress={{
        messages: 12,
        startedAt: new Date(Date.now() - 37 * 60_000).toISOString(),
        lastActivityAt: new Date(Date.now() - 4_000).toISOString(),
      }}
      stopping={false}
      onStop={() => {}}
    />,
  );
  assert.match(html, /12 messages/);
  assert.match(html, /last activity \ds ago/);
  assert.match(html, /37m/);
  assert.doesNotMatch(html, /agent is working on your message/);
  assert.doesNotMatch(html, /<progress|role="progressbar"/);
});

test("a think in flight is said in the bar, in place of the last activity, on the one line", () => {
  // It used to be a dashed box below the transcript that appeared, grew and
  // went away while the agent worked, and moved everything above it. The bar
  // says it instead, so the transcript only grows by what has been written.
  const drafts = createDraftStore();
  // A frame from an agent from before, with words beside the count.
  drafts.receive(
    thinkingOf({
      type: "draft",
      turn: 1,
      v: 1,
      blocks: [
        { uuid: "u-said", index: 0, kind: "text", text: "Let me look at the pool first." },
        { index: 0, kind: "thinking", thinkingTokens: 50 },
      ],
    }),
  );
  const html = draw(
    <ActivityBar
      banner={{ kind: "working", queued: 1, stalled: false }}
      subtask={null}
      progress={{
        messages: 28,
        startedAt: new Date(Date.now() - (7 * 60 + 14) * 1000).toISOString(),
        lastActivityAt: new Date(Date.now() - 3_000).toISOString(),
      }}
      drafts={drafts}
      stopping={false}
      onStop={() => {}}
    />,
  );
  // Less the invisible copy that holds the count's width.
  const text = html.replace(/<span aria-hidden="true" data-ghost=""[^>]*>[^<]*<\/span>/g, "").replace(/<[^>]*>/g, "");
  assert.match(text, /^28 messages · thinking… ~50 tokens · 7m1\ds · 1 messages queued/);
  assert.doesNotMatch(text, /last activity|Let me look/, "nothing of the draft but the count");
  assert.match(html, /stop<\/button>/, "Stop stays beside it");
  // One truncating line, in a container the count's unit is sized against.
  assert.match(html, /data-activity-bar=""[^>]*class="@container\/activity /);
  assert.match(html, /class="min-w-0 flex-1 truncate">28 messages/);
  assert.doesNotMatch(html, /border-dashed|data-draft/);
});

test("a turn nothing can date keeps the sentence it always had", () => {
  // An older server sends no `startedAt`, and a window with no timestamps in it
  // cannot stand in for one. A duration measured from nothing is worse than a
  // sentence, so the sentence stays.
  const html = draw(
    <ActivityBar
      banner={{ kind: "working", queued: 0, stalled: false }}
      subtask={null}
      progress={null}
      stopping={false}
      onStop={() => {}}
    />,
  );
  assert.match(html, /agent is working on your message/);
});

test("a queued banner is about the queue, not about a turn's clock", () => {
  const html = draw(
    <ActivityBar
      banner={{ kind: "queued", queued: 2, awaitingBackground: false, background: 0 }}
      subtask={null}
      progress={{ messages: 9, startedAt: new Date().toISOString(), lastActivityAt: null }}
      stopping={false}
    />,
  );
  assert.match(html, /2 messages queued/);
  assert.doesNotMatch(html, /last activity/);
});

test("nothing to say about the user's message draws no strip", () => {
  // An idle session is one way to get here. A session only minding background
  // children is the other: `activityBanner` answers null for it too now (pinned
  // in `session-activity.ts`'s own test), because the panel floating over the
  // transcript lists every one of those children by name, and a count under it
  // was the same screen saying the same thing twice.
  assert.equal(draw(<ActivityBar banner={null} subtask={null} stopping={false} />), "");
});

test("the strip names the child work that is still out there", () => {
  // "Something is running" is half an answer; this is the other half. Only for
  // the case the floating panel stays away from — a turn whose children the
  // loaded transcript window cannot see.
  const html = draw(
    <ActivityBar
      banner={{ kind: "working", queued: 0, stalled: false }}
      subtask={{
        id: "u1",
        text: "review the diff",
        meta: "code-review",
        agentType: "code-review",
        background: false,
        acked: false,
        startedAt: null,
        detail: null,
      }}
      stopping={false}
    />,
  );
  assert.match(html, /subagent/);
  assert.match(html, /review the diff/);
  assert.match(html, /code-review/, "the caption the CLI recorded, as a tooltip");
});

/* ── the parked-settings notice ─────────────────────────────────────────── */

test("a message parked behind background work says so", () => {
  const html = draw(<AwaitingBackgroundNotice resident={resident({ queued: 1, background: 2, awaitingBackground: true })} />);
  assert.match(html, /needs a fresh session process/);
  assert.match(html, /2 background tasks/);
  assert.match(html, /text-warn/, "it is a wait the user did not ask for: amber, not green");
});

test("nothing is parked, so nothing is said", () => {
  assert.equal(draw(<AwaitingBackgroundNotice resident={resident({ queued: 1 })} />), "");
  assert.equal(draw(<AwaitingBackgroundNotice resident={null} />), "");
});

/* ── the floating list of live children ─────────────────────────────────── */

function subtask(over: Partial<LiveSubtask> = {}): LiveSubtask {
  return {
    id: "u1",
    text: "scout the codebase",
    meta: null,
    agentType: null,
    background: false,
    acked: true,
    startedAt: "2026-08-31T09:00:00.000Z",
    detail: null,
    ...over,
  };
}

test("a row says what the child is doing now, not only how long it has been", () => {
  // The stopwatch alone answered "how long"; these three are the difference
  // between a child that is working and one that is wedged. The CLI had been
  // publishing them the whole time and the panel stepped over them.
  const html = draw(
    <BackgroundTaskPanel
      panel={{
        kind: "background",
        count: 1,
        subtasks: [
          subtask({
            subagentType: "Explore",
            lastToolName: "Bash",
            toolUses: 17,
            totalTokens: 10_240,
            durationMs: 12 * 60_000,
          }),
        ],
      }}
      now={Date.parse("2026-08-31T09:01:00.000Z")}
    />,
  );
  assert.match(html, /Explore/, "the CLI's own name for the child, as its chip");
  assert.match(html, /↳ Bash/);
  assert.match(html, /×17/);
  assert.match(html, /10K/, "tokens, compactly");
  // The CLI's own duration wins over `now - startedAt`, which after a restart
  // measures the restart rather than the child.
  assert.match(html, /12m/);
  assert.doesNotMatch(html, /&lt;1m/);
});

test("a row from a server that publishes no progress is exactly what it was", () => {
  const html = draw(
    <BackgroundTaskPanel
      panel={{ kind: "background", count: 1, subtasks: [subtask({ agentType: "code-review" })] }}
      now={Date.parse("2026-08-31T09:12:00.000Z")}
    />,
  );
  assert.match(html, /code-review/, "the caption-parsed type still labels the row");
  assert.match(html, /12m/, "and the age still comes from the clock");
  assert.doesNotMatch(html, /↳ [A-Z]/);
  assert.doesNotMatch(html, /×\d/);
});

test("a background command is chipped as one, whatever the CLI called its type", () => {
  const html = draw(
    <BackgroundTaskPanel
      panel={{ kind: "background", count: 1, subtasks: [subtask({ background: true, subagentType: "Explore" })] }}
      now={Date.parse("2026-08-31T09:01:00.000Z")}
    />,
  );
  assert.match(html, /bg<\/span>/);
});

/* ── folding the list away ──────────────────────────────────────────────── */

/** `n` distinct children, so the panel's keys and the fold both see real rows. */
function fanOut(n: number): LiveSubtask[] {
  return Array.from({ length: n }, (_, i) =>
    subtask({ id: `u${i}`, text: `child ${i}`, startedAt: "2026-08-31T09:00:00.000Z" }),
  );
}

/** The component reads localStorage at mount; node has none until it is given one. */
function withStoredFold<T>(value: string | null, body: () => T): T {
  const before = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  const values = new Map<string, string>();
  if (value !== null) values.set("rh.sessions.backgroundPanelOpen", value);
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, next: string) => void values.set(key, next),
    },
  });
  try {
    return body();
  } finally {
    if (before) Object.defineProperty(globalThis, "localStorage", before);
    else delete (globalThis as { localStorage?: unknown }).localStorage;
  }
}

test("a handful of children is drawn exactly as it was: rows, no headline", () => {
  // The common session, and the one the fold must not tax. "3 background tasks"
  // is the question, not the answer, and three rows already answer it.
  const html = draw(
    <BackgroundTaskPanel
      panel={{ kind: "background", count: 3, subtasks: fanOut(3) }}
      now={Date.parse("2026-08-31T09:05:00.000Z")}
    />,
  );
  assert.match(html, /child 0.*child 1.*child 2/s);
  assert.doesNotMatch(html, /<button/, "nothing to fold, so nothing to tap");
  assert.doesNotMatch(html, /background tasks running/);
});

test("a fan-out folds to one line, and says how long the oldest has been out", () => {
  // The phone bug: fifteen rows of monospace over the transcript they annotate.
  const html = draw(
    <BackgroundTaskPanel
      panel={{ kind: "background", count: 15, subtasks: fanOut(15) }}
      now={Date.parse("2026-08-31T10:00:00.000Z")}
    />,
  );
  assert.match(html, /15 background tasks running/);
  assert.match(html, /oldest 1h/, "the count alone cannot tell a fresh fan-out from a wedged one");
  assert.match(html, /aria-expanded="false"/);
  assert.doesNotMatch(html, /child 0/, "the rows are what folding puts away");
});

test("the count is the resident's, which can outrun the rows it could name", () => {
  const html = draw(
    <BackgroundTaskPanel
      panel={{ kind: "background", count: 40, subtasks: fanOut(6) }}
      now={Date.parse("2026-08-31T09:05:00.000Z")}
    />,
  );
  assert.match(html, /40 background tasks running/);
});

test("an opened fold keeps its summary above the rows", () => {
  const html = withStoredFold("1", () =>
    draw(
      <BackgroundTaskPanel
        panel={{ kind: "background", count: 15, subtasks: fanOut(15) }}
        now={Date.parse("2026-08-31T10:00:00.000Z")}
      />,
    ),
  );
  assert.match(html, /aria-expanded="true"/);
  assert.match(html, /15 background tasks running.*child 0/s, "the way back is above the list");
});

test("a stored fold is not applied to a panel that never needed folding", () => {
  const html = withStoredFold("0", () =>
    draw(
      <BackgroundTaskPanel
        panel={{ kind: "background", count: 2, subtasks: fanOut(2) }}
        now={Date.parse("2026-08-31T09:05:00.000Z")}
      />,
    ),
  );
  assert.match(html, /child 0/, "one global key, and this is the session it must not follow into");
  assert.doesNotMatch(html, /<button/);
});

test("a panel that can name no children still draws its count, and no fold", () => {
  // Children launched before the loaded window: the count is drawn *instead of*
  // rows, and there is nothing to put away.
  const html = draw(<BackgroundTaskPanel panel={{ kind: "background", count: 7, subtasks: [] }} />);
  assert.match(html, /7 background tasks running/);
  assert.doesNotMatch(html, /<button/);
  assert.doesNotMatch(html, /oldest/, "nothing datable to be the oldest of");
});

/* ── stopping one child ─────────────────────────────────────────────────── */

test("a row the resident named carries a Stop; one the transcript named alone does not", () => {
  // The honesty rule: `stop_task` is addressed by the CLI's own `task_id`, so a
  // row that has none could only draw a button that fails.
  const html = draw(
    <BackgroundTaskPanel
      panel={{
        kind: "background",
        count: 2,
        subtasks: [
          subtask({ id: "u1", taskId: "b0tghhqgu", text: "the resident named this one" }),
          subtask({ id: "u2", text: "a foreground child the resident cannot see" }),
        ],
      }}
      now={Date.parse("2026-08-31T09:05:00.000Z")}
      onStop={() => {}}
    />,
  );
  assert.equal(
    html.match(/aria-label="stop this background task"/g)?.length,
    1,
    "exactly one row is stoppable",
  );
});

test("no way to ask means no button, however the row was named", () => {
  // A codex session, or a feed with no resident behind it: the panel still
  // lists what it knows and offers nothing it cannot do.
  const html = draw(
    <BackgroundTaskPanel
      panel={{ kind: "background", count: 1, subtasks: [subtask({ taskId: "b0tghhqgu" })] }}
      now={Date.parse("2026-08-31T09:05:00.000Z")}
    />,
  );
  assert.doesNotMatch(html, /stop this background task/);
});

test("a stop already asked for spins on its own row and disables only that one", () => {
  const html = draw(
    <BackgroundTaskPanel
      panel={{
        kind: "background",
        count: 2,
        subtasks: [
          subtask({ id: "u1", taskId: "asked", text: "already asked to stop" }),
          subtask({ id: "u2", taskId: "untouched", text: "still going" }),
        ],
      }}
      now={Date.parse("2026-08-31T09:05:00.000Z")}
      onStop={() => {}}
      stopping={new Set(["asked"])}
    />,
  );
  assert.equal(html.match(/animate-spin/g)?.length, 1, "one spinner, not one per row");
  assert.equal(html.match(/disabled=""/g)?.length, 1);
});
