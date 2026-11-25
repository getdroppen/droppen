// The two transcript items that are not messages.
//
// A compact used to render as nothing plus a fifteen-thousand-word user bubble,
// and a slash command as one faint grey line indistinguishable from every other
// faint grey line. The parser now emits a `compact` and a `command` item for
// them; these pin that the components actually draw the difference, because a
// correct parser feeding a renderer that ignores it reproduces the bug exactly.
//
// `react-dom/server` rather than a DOM, like `live-work-render.test.tsx`: both
// folds are native `<details>`, so the collapsed summary and the expanded body
// are both in the markup and the browser owns the toggle.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { CommandChip, CompactDivider } from "../src/features/sessions/transcript-notices.js";
import { SystemLine } from "../src/features/sessions/system-line.js";
import type { components } from "../src/api/schema.js";
import { transcriptItem } from "./session-fixtures.js";
import { makeI18n, pageDraw } from "./render.js";

type TranscriptItem = components["schemas"]["TranscriptItem"];

// English keys are the source strings, so an empty resource bundle renders the
// same text the untranslated UI does — and asserting on it asserts on the key.
const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} } }
});

const draw = pageDraw(i18n);

const item = (over: Partial<TranscriptItem> = {}) => transcriptItem({ role: "system", ...over });

/* ── the compact boundary ───────────────────────────────────────────────── */

// The shape the parser emits for the real 2026-08-30T07:20 compact.
const COMPACT = item({
  kind: "compact",
  text: "Conversation compacted",
  meta: "manual · 540084 → 5180 tokens",
  detail: "This session is being continued from a previous conversation that ran out of context.",
});

test("a compact draws a labelled divider, not a message", () => {
  const html = draw(<CompactDivider item={COMPACT} />);
  assert.match(html, /context compacted/, "the localised label, not the transcript's English");
  assert.match(html, /manual · 540084 → 5180 tokens/, "what it cost is the reason to notice it");
});

test("the dropped summary is readable but folded away", () => {
  const html = draw(<CompactDivider item={COMPACT} />);
  assert.match(html, /<details/);
  assert.doesNotMatch(html, /<details[^>]*\bopen\b/, "nobody scrolls back to re-read a summary");
  assert.match(html, /read the summary/, "the fold says what is behind it");
  // In the markup either way — the browser hides it — so this asserts the
  // expanded state is not empty.
  assert.match(html, /This session is being continued/);
});

test("a boundary with no summary is a rule and nothing to open", () => {
  // Offering a disclosure triangle over an empty body is a promise the divider
  // cannot keep: the summary entry is outside the loaded window.
  const html = draw(<CompactDivider item={item({ kind: "compact", text: "Conversation compacted" })} />);
  assert.match(html, /context compacted/);
  assert.doesNotMatch(html, /<details/);
});

/* ── slash commands ─────────────────────────────────────────────────────── */

test("a command is a chip carrying its name, its arguments and its output", () => {
  const html = draw(
    <CommandChip item={item({ kind: "command", toolName: "/goal", text: "ship the PWA", detail: "Goal set: ship the PWA" })} />,
  );
  assert.match(html, /\/goal/);
  assert.match(html, /ship the PWA/);
  assert.match(html, /font-mono/, "a command reads as typed input, not as prose");
  assert.match(html, /command output/, "the stdout is behind a fold of its own");
  assert.match(html, /Goal set: ship the PWA/);
});

test("an argument-less command with no output is one line", () => {
  const html = draw(<CommandChip item={item({ kind: "command", toolName: "/compact", text: "" })} />);
  assert.match(html, /\/compact/);
  assert.doesNotMatch(html, /<details/, "nothing to expand into");
});

test("a command that failed says so instead of calling stderr its output", () => {
  // A `/compact` over the image limit: the reason is stderr, and calling it
  // "command output" made it read like an answer.
  const html = draw(
    <CommandChip
      item={item({
        kind: "command",
        toolName: "/compact",
        text: "",
        detail: "Error during compaction: API Error: 400 too many images",
        isError: true,
      })}
    />,
  );
  assert.match(html, /failed/, "the label names the failure");
  assert.match(html, /text-danger/, "and it is the failure colour");
  assert.match(html, /Error during compaction: API Error: 400 too many images/);
  assert.doesNotMatch(html, /command output/);
});

test("an orphaned failure line keeps the failure colour", () => {
  // The command is outside the loaded window, so the reason stands alone — and
  // must still not read as routine chatter.
  const html = draw(<SystemLine item={item({ text: "Error during compaction: no room", isError: true })} rowClass="px-4" />);
  assert.match(html, /Error during compaction: no room/);
  assert.match(html, /text-danger/);
});
