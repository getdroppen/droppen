// The rendering half of `live-work.test.ts`.
//
// That file pins the decisions; this one pins that the components draw them.
// The bug all three of these components exist to fix is a UI that stayed quiet
// about work that was really happening, and a correct model feeding a
// component that ignores it reproduces exactly that — so the model tests alone
// would not be evidence of anything.
//
// `react-dom/server` rather than a DOM, like `session-activity-render.test.tsx`:
// the folds are native `<details>`, so both the collapsed summary and the
// expanded body are in the markup and the browser owns the toggle. That is why
// there is no event to simulate and no runner to add.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { BackgroundTaskPanel } from "../src/features/sessions/background-panel.js";
import {
  ServerQueueList,
  parkedHead,
  queueFailureNotice,
  QueueRefusal,
} from "../src/features/sessions/server-queue.js";
import type { LiveSubtask, ResidentInfo } from "../src/features/sessions/session-activity.js";
import { resident } from "./session-fixtures.js";
import { makeI18n, pageDraw } from "./render.js";

test("the sidebar has no extra working/background summary strip", () => {
  const source = readFileSync(new URL("../src/features/sessions/sessions-layout.tsx", import.meta.url), "utf8");
  assert.doesNotMatch(source, /ActiveSessionsBar|activeWork\(|sessions working/);
  assert.match(source, /<StatusBadge/, "each session keeps its own status");
});

// English keys are the source strings, so an empty resource bundle renders the
// same text the untranslated UI does — and asserting on it asserts on the key.
const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} } }
});

const draw = pageDraw(i18n);

/** A fixed clock, so "running 12m" is a fact rather than a race. */
const NOW = Date.parse("2026-08-30T04:00:00Z");

function subtask(over: Partial<LiveSubtask> = {}): LiveSubtask {
  return {
    id: "u1",
    text: "explore the repo",
    meta: null,
    agentType: null,
    background: false,
    startedAt: null,
    detail: null,
    ...over,
  };
}

/* ── the panel floating over the transcript ─────────────────────────────── */

test("nothing running draws no panel at all", () => {
  assert.equal(draw(<BackgroundTaskPanel panel={null} />), "");
});

test("a count is drawn only when there is no list to draw instead", () => {
  // The children were launched before the loaded window, so there is nothing to
  // name — and with the composer's strip out of the background business, this
  // line is the only thing left saying the session has not finished.
  const html = draw(<BackgroundTaskPanel panel={{ kind: "background", count: 2, subtasks: [] }} />);
  assert.match(html, /2 background tasks running/);
  assert.doesNotMatch(html, /<details/, "nothing to expand into");
  assert.match(html, /sticky/, "it still rides the top of the scroll area");
});

test("every child is a row, with no count and no fold in front of them", () => {
  // "2 background tasks running" over a fold is the question, asked instead of
  // answered, and one click away from the answer. The rows are the answer.
  const html = draw(
    <BackgroundTaskPanel
      now={NOW}
      panel={{
        kind: "background",
        count: 2,
        subtasks: [
          subtask({ id: "u1", text: "explore the repo", agentType: "Explore", startedAt: "2026-08-30T03:48:00Z" }),
          subtask({ id: "u2", text: "write the tests", agentType: "claude", startedAt: "2026-08-30T02:40:00Z" }),
        ],
      }}
    />,
  );
  assert.doesNotMatch(html, /background tasks running/, "no count over the list it summarises");
  assert.match(html, /explore the repo/);
  assert.match(html, /write the tests/);
  assert.match(html, /Explore/, "the agent type, which says what the child is for");
  assert.match(html, /running 12m/, "and how long it has been out");
  assert.match(html, /running 1h 20m/);
  assert.doesNotMatch(html, /<details/, "neither child recorded a prompt to open onto");
});

test("a child expands onto what it was actually handed", () => {
  const html = draw(
    <BackgroundTaskPanel
      now={NOW}
      panel={{
        kind: "background",
        count: 1,
        subtasks: [
          subtask({ text: "explore the repo", detail: "Read every route file and report the auth checks." }),
        ],
      }}
    />,
  );
  assert.match(html, /<details/, "the row is its own fold");
  assert.doesNotMatch(html, /<details[^>]*\bopen\b/, "a prompt is many lines; it opens on request");
  assert.match(html, /explore the repo/, "the description stays on the summary line");
  assert.match(html, /Read every route file and report the auth checks\./);
  assert.match(html, /<pre/, "quoted like every other tool input in this UI");
});

test("a child with nothing recorded is a plain row, promising nothing", () => {
  const html = draw(
    <BackgroundTaskPanel panel={{ kind: "background", count: 1, subtasks: [subtask()] }} now={NOW} />,
  );
  assert.match(html, /explore the repo/);
  assert.doesNotMatch(html, /<details/, "no triangle over an empty body");
  assert.doesNotMatch(html, /cursor-pointer/, "and no affordance to click either");
});

test("a working turn's children are listed the same way, with no headline", () => {
  // The turn in flight already has a strip above the composer saying so; this
  // panel exists for the part the strip cannot fit, which is the children.
  const html = draw(
    <BackgroundTaskPanel panel={{ kind: "working", count: 1, subtasks: [subtask()] }} now={NOW} />,
  );
  assert.match(html, /explore the repo/);
  assert.doesNotMatch(html, /main turn in flight/, "a count of what is on screen is not information");
  assert.doesNotMatch(html, /background tasks running/);
});

test("a background launch is labelled as one, not as a subagent", () => {
  const html = draw(
    <BackgroundTaskPanel
      now={NOW}
      panel={{
        kind: "background",
        count: 1,
        subtasks: [subtask({ text: "pnpm build", background: true, meta: "watch" })],
      }}
    />,
  );
  assert.match(html, /pnpm build/);
  assert.match(html, /text-warn/, "a minded shell command is not a subagent");
  assert.doesNotMatch(html, /subagent/);
});

test("a subagent with no recorded type still gets a label", () => {
  const html = draw(
    <BackgroundTaskPanel panel={{ kind: "background", count: 1, subtasks: [subtask()] }} now={NOW} />,
  );
  assert.match(html, /subagent/);
});

/* ── the session's own queue ────────────────────────────────────────────── */

test("an empty queue with nothing to report draws nothing", () => {
  assert.equal(
    draw(<ServerQueueList
        messages={[]}
        pendingId={null}
        restartId={null}
        notice={null}
        onSteer={() => {}}
        onDrop={() => {}}
      />),
    "",
  );
});

test("a waiting message offers both a steer and a withdrawal", () => {
  const html = draw(
    <ServerQueueList
      messages={[{ id: "q1", text: "actually, use the other parser", steered: false }]}
      pendingId={null}
      restartId={null}
      notice={null}
      onSteer={() => {}}
      onDrop={() => {}}
    />,
  );
  assert.match(html, /queued in the session \(1\)/);
  assert.match(html, /actually, use the other parser/);
  assert.match(html, /deliver now, ahead of the turn in flight/);
  assert.match(html, /remove from queue/);
  assert.doesNotMatch(html, /disabled=""/, "nothing is in flight");
});

test("Codex distinguishes native steering from waiting for an acknowledgement", () => {
  const render = (steered: boolean) => draw(
    <ServerQueueList source="codex" messages={[{ id: "q", text: "correction", steered }]}
      pendingId={null} restartId={null} notice={null} onSteer={() => {}} onDrop={() => {}} />,
  );
  assert.match(render(false), /steer into the active Codex turn/);
  assert.match(render(true), /awaiting Codex acknowledgement/);
  assert.doesNotMatch(render(true), /runs as the next turn/);
});

test("the message a park is waiting on says the button restarts the session", () => {
  // The same ▶, and a different act: this message needs a process the running
  // one is not, so pressing it replaces the process and kills the background
  // work that parked it. A button that says "deliver now" for that is a trap.
  const html = draw(
    <ServerQueueList
      messages={[
        { id: "q1", text: "switch to the other account", steered: false },
        { id: "q2", text: "and then this", steered: false },
      ]}
      pendingId={null}
      restartId="q1"
      notice={null}
      onSteer={() => {}}
      onDrop={() => {}}
    />,
  );
  assert.match(html, /restart the session now and send this message/);
  assert.match(html, /deliver now, ahead of the turn in flight/, "the one behind it is an ordinary steer");
  assert.match(html, /text-warn/, "and it is not drawn as the harmless one");
});

test("the parked head is the first message not already handed over", () => {
  const messages = [
    { id: "q1", text: "already in claude's stdin", steered: true },
    { id: "q2", text: "wants another account", steered: false },
    { id: "q3", text: "waiting behind it", steered: false },
  ];
  const parked = resident({ awaitingBackground: true, background: 2 });
  assert.equal(parkedHead(parked, messages), "q2", "steered rows are listed first; the queue starts after them");
  assert.equal(
    parkedHead(resident({ background: 2 }), messages),
    null,
    "nothing is parked, so every ▶ is an ordinary steer",
  );
  assert.equal(parkedHead(null, messages), null);
  assert.equal(parkedHead(parked, []), null, "and a park with nothing in the queue is a state, not a button");
});

test("a steered message offers neither, because both routes would refuse", () => {
  // It is in claude's stdin. Delete and steer both answer 409 for it, and a
  // button whose only outcome is an error is worse than no button.
  const html = draw(
    <ServerQueueList
      messages={[{ id: "q1", text: "actually, use the other parser", steered: true }]}
      pendingId={null}
      restartId={null}
      notice={null}
      onSteer={() => {}}
      onDrop={() => {}}
    />,
  );
  assert.match(html, /delivered — waiting to start/);
  assert.doesNotMatch(html, /<button/);
});

test("the message being acted on has its buttons taken away", () => {
  const html = draw(
    <ServerQueueList
      messages={[
        { id: "q1", text: "first", steered: false },
        { id: "q2", text: "second", steered: false },
      ]}
      pendingId="q1"
      restartId={null}
      notice={null}
      onSteer={() => {}}
      onDrop={() => {}}
    />,
  );
  // Two disabled buttons on the pending row, and two live ones on the other.
  assert.equal(html.match(/disabled=""/g)?.length, 2);
  assert.match(html, /animate-spin/, "the row says which one it is waiting on");
});

test("the queue keeps its order, numbered from the front", () => {
  const html = draw(
    <ServerQueueList
      messages={[
        { id: "q1", text: "runs first", steered: true },
        { id: "q2", text: "runs second", steered: false },
      ]}
      pendingId={null}
      restartId={null}
      notice={null}
      onSteer={() => {}}
      onDrop={() => {}}
    />,
  );
  assert.match(html, /#1.*runs first.*#2.*runs second/s);
});

test("a notice survives the queue emptying under it", () => {
  // The common case: withdraw the only message, and the sentence explaining
  // what happened would vanish with the row it was about.
  const html = draw(
    <ServerQueueList
      messages={[]}
      pendingId={null}
      restartId={null}
      notice={{ tone: "info", text: "delivered — it runs next" }}
      onSteer={() => {}}
      onDrop={() => {}}
    />,
  );
  assert.match(html, /delivered — it runs next/);
  assert.doesNotMatch(html, /queued in the session/);
});

test("a refusal is drawn in the colour of a refusal", () => {
  const html = draw(
    <ServerQueueList
      messages={[]}
      pendingId={null}
      restartId={null}
      notice={{ tone: "danger", text: "already steered" }}
      onSteer={() => {}}
      onDrop={() => {}}
    />,
  );
  assert.match(html, /text-danger/);
});

/* ── what a refusal means ───────────────────────────────────────────────── */

const echo = (key: string) => key;

test("a 404 is a race the user lost, not an error", () => {
  // The message left the queue because it *started*. Reporting that as a
  // failure would be reporting the thing the user asked for.
  const notice = queueFailureNotice(new QueueRefusal(404, { error: "unknown_queued_message" }), echo);
  assert.equal(notice.tone, "info");
  assert.match(notice.text, /already started/);
});

test("a 409 is shown in the server's own words", () => {
  // The server writes a whole sentence for these — already steered, or wants
  // a process this one is not. A second wording invented here is how the UI
  // ends up disagreeing with the server about why nothing happened.
  const notice = queueFailureNotice(
    new QueueRefusal(409, {
      error: "already_steered",
      hint: "that message is already with the CLI and cannot be taken back",
    }),
    echo,
  );
  assert.equal(notice.tone, "danger");
  assert.equal(notice.text, "that message is already with the CLI and cannot be taken back");
});

test("a refusal with no sentence still says something", () => {
  const notice = queueFailureNotice(new QueueRefusal(409, { error: "needs_restart" }), echo);
  assert.equal(notice.text, "needs_restart");
});

test("a dropped connection is a refusal too", () => {
  const notice = queueFailureNotice(new Error("Failed to fetch"), echo);
  assert.equal(notice.tone, "danger");
  assert.equal(notice.text, "Failed to fetch");
});
