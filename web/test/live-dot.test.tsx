// Nothing on a screen someone is only *watching* may run a CSS animation.
//
// The bug behind this file was not a slow render. Three rounds of React work
// (feed selector split, live-window cap, render memoisation) moved nothing,
// because the cost never reached JavaScript: a running CSS animation makes
// Chromium schedule a frame every vsync for as long as it runs. A trace of the
// real site with two open session panes, no mouse, no scroll, no rAF and a
// renderer main thread at 0.2–0.8% still produced 165 frames a second, and
// pausing every animation in the page took total CPU from ~40% of a core to
// ~11%. `document.getAnimations()` held five: two `pulse` dots and three `spin`
// spinners, none of which anybody had asked for.
//
// So the property is about the *class attribute*, not about pixels, and that is
// exactly what a string of markup can answer. Each test below renders a
// component in the steady state of "a session is working and I am reading it"
// and asserts two things at once: no `animate-` anywhere in the output, and the
// liveness affordance still there — because deleting the animation and deleting
// the indicator look identical to a naive assertion, and only one of them is
// the fix.
//
// `react-dom/server` with no DOM, like `session-activity-render.test.tsx`.
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import test from "node:test";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { LiveDot } from "../src/components/live-dot.js";
import type { components } from "../src/api/schema.js";
import { ActivityBar, SubagentTool, ToolLine } from "../src/features/sessions/session-detail.js";
import { StatusBadge } from "../src/features/sessions/sessions-layout.js";
import { BackgroundTaskPanel } from "../src/features/sessions/background-panel.js";
import type {
  ActivityRow,
  LiveSubtask,
  ResidentInfo,
} from "../src/features/sessions/session-activity.js";
import { resident, transcriptItem as item } from "./session-fixtures.js";
import { makeI18n, pageDraw } from "./render.js";

type TranscriptItem = components["schemas"]["TranscriptItem"];

// English keys are the source strings, so an empty resource bundle renders the
// same text the untranslated UI does — and asserting on it asserts on the key.
const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} } }
});

// A subagent card holds a query it does not run until it is opened, so the
// client here never fetches — it exists because `useQuery` needs one.
const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });

const draw = pageDraw(i18n, (node) => <QueryClientProvider client={queryClient}>{node}</QueryClientProvider>);

/** The whole point, in one assertion: alive to a reader, still to the compositor. */
function assertLiveButStill(html: string, what: string) {
  assert.doesNotMatch(html, /animate-/, `${what} must not run a CSS animation`);
  assert.match(html, /data-live/, `${what} must still say it is running`);
}

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

/* ── the dot itself ─────────────────────────────────────────────────────── */

test("the dot is a plain span with no animation and no transition", () => {
  // A `transition-*` would be just as bad as an `animate-*`: a transition that
  // restarts on every beat is a running animation by another name, and it is
  // the obvious thing for the next person to reach for to "smooth out" the
  // blink. `getAnimations()` counts both.
  const html = draw(<LiveDot />);
  assert.doesNotMatch(html, /animate-|transition-/);
  assert.match(html, /data-live/);
  assert.match(html, /rounded-full/, "still round, still a dot");
});

test("the dot takes the colour of the line it sits in, and lets a caller override it", () => {
  // `bg-current` is what lets one component serve the green working banner and
  // the amber reconnect notice without either of them passing a colour.
  assert.match(draw(<LiveDot />), /bg-current/);
  const green = draw(<LiveDot className="bg-ok" />);
  assert.match(green, /bg-ok/);
  assert.doesNotMatch(green, /bg-current/, "tailwind-merge drops the default rather than stacking it");
});

test("the dot carries a caller's tooltip, which is the only text some of them have", () => {
  assert.match(draw(<LiveDot title="a session inside is running" />), /title="a session inside is running"/);
});

/* ── the sidebar, which is on screen for every session, always ───────────── */

test("a working row's green dot is still green, and no longer pulses", () => {
  const row: ActivityRow = { status: "running", activity: "working", resident: resident({ phase: "working" }) };
  const html = draw(<StatusBadge session={row} />);
  assertLiveButStill(html, "the sidebar's working row");
  assert.match(html, /bg-ok/);
  assert.match(html, />working</, "and the word beside it still says so");
});

test("a background-only sidebar row is labelled, and holds still", () => {
  const row: ActivityRow = {
    status: "running",
    activity: "background",
    resident: resident({ background: 2 }),
  };
  const html = draw(<StatusBadge session={row} />);
  assert.match(html, /background 2/);
  assert.doesNotMatch(html, /animate-|data-live/, "background work needs nobody, so it does not blink");
});

test("an unread count never moves", () => {
  const html = draw(<StatusBadge session={{ status: "idle", activity: "idle" }} unread={4} />);
  assert.match(html, /data-unread="4"/);
  assert.doesNotMatch(html, /animate-|transition-|data-live/);
});

test("a question holds still: it is waiting on the reader, not working", () => {
  const html = draw(<StatusBadge session={{ status: "waiting", activity: "idle", asking: true }} />);
  assert.match(html, /data-activity="asking"/);
  assert.doesNotMatch(html, /animate-|data-live/);
});

test("an idle row draws nothing that could animate in the first place", () => {
  const row: ActivityRow = { status: "idle", activity: "idle", resident: resident() };
  const html = draw(<StatusBadge session={row} />);
  assert.doesNotMatch(html, /animate-/);
  assert.doesNotMatch(html, /data-live/, "nothing is running, so nothing claims to be");
});

/* ── the transcript, which is the screen the user is actually reading ────── */

test("the working banner is alive without a spinner", () => {
  // The longest-lived indicator in the app: up for the whole of every turn, and
  // an SVG that never made it onto the compositor, so it repainted the same
  // thread the transcript needs, 165 times a second, for minutes at a time.
  const html = draw(
    <ActivityBar
      banner={{ kind: "working", queued: 0, stalled: false }}
      subtask={null}
      progress={null}
      stopping={false}
      onStop={() => {}}
    />,
  );
  assertLiveButStill(html, "the working banner");
  assert.match(html, /agent is working on your message/, "the sentence is still the sentence");
});

test("an unanswered call in a working turn is marked, and the mark does not spin", () => {
  const html = draw(
    <ToolLine item={item({ kind: "tool_use", toolName: "Bash", text: "cargo test", toolUseId: "t1" })} live />,
  );
  assertLiveButStill(html, "the in-flight tool row");
  assert.match(html, /bg-ok/, "green, because a still grey dot is not findable in a wall of finished rows");
});

test("a subagent the resident is still minding says so without spinning", () => {
  const html = draw(
    <SubagentTool
      item={item({ kind: "tool_use", toolName: "Task", text: "review the diff", toolUseId: "t1", agent: "a1" })}
      session={{ source: "claude", id: "s1" }}
      residentRunning
    />,
  );
  assertLiveButStill(html, "a running subagent card");
});

test("the background panel's summary line no longer pulses at the refresh rate", () => {
  // This is the dot that carried a gradient, which is why it never composited:
  // it ran style and paint on the main thread every frame it was on screen.
  const html = draw(
    <BackgroundTaskPanel
      panel={{ kind: "background", count: 2, subtasks: [] }}
      now={Date.parse("2026-08-30T04:00:00Z")}
    />,
  );
  assertLiveButStill(html, "the background panel summary");
  assert.match(html, /2 background tasks running/);
});

/* ── what must survive: the spinners the user is waiting on ─────────────── */

test("a stop the user just pressed still spins", () => {
  // The distinction the whole change rests on. This one runs because the user
  // did something a moment ago and ends when the server answers; it is not on
  // screen while somebody reads, so it costs nothing in the steady state and
  // its motion is the feedback for the press.
  const html = draw(
    <ActivityBar
      banner={{ kind: "working", queued: 0, stalled: false }}
      subtask={null}
      progress={null}
      stopping
      onStop={() => {}}
    />,
  );
  assert.match(html, /animate-spin/, "a pressed Stop is short-lived and user-initiated");
});

test("a background task being stopped still spins on its own row", () => {
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
      now={Date.parse("2026-08-30T04:00:00Z")}
      onStop={() => {}}
      stopping={new Set(["asked"])}
    />,
  );
  assert.equal(html.match(/animate-spin/g)?.length, 1, "one pressed button, one spinner");
});

/* ── the fence ──────────────────────────────────────────────────────────── */

test("no source file anywhere reintroduces `animate-pulse`", () => {
  // A source assertion, because the invariant is about a class that must not be
  // written at all rather than about any one component: `animate-pulse` has no
  // short-lived use in this app — it exists only to say "this is still going",
  // which is precisely the always-on case, and every one of them is a frame
  // every vsync for as long as the thing it decorates is running. `LiveDot` is
  // the replacement and names the class in its own prose, so it is the one file
  // allowed to contain the string.
  const offenders: string[] = [];
  const walk = (dir: URL) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const child = new URL(entry.name + (entry.isDirectory() ? "/" : ""), dir);
      if (entry.isDirectory()) walk(child);
      else if (/\.tsx?$/.test(entry.name) && entry.name !== "live-dot.tsx") {
        if (readFileSync(child, "utf8").includes("animate-pulse")) offenders.push(entry.name);
      }
    }
  };
  walk(new URL("../src/", import.meta.url));
  assert.deepEqual(offenders, [], "use `LiveDot` instead — see the comment at the top of live-dot.tsx");
});
