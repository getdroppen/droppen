// What a transcript row says about time, about boilerplate, and about a
// background task's report.
//
// `react-dom/server` with no DOM, like `session-activity-render.test.tsx`: the
// folds are native `<details>`, so both the collapsed summary and the expanded
// body are in the markup and the browser owns the toggle.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { components } from "../src/api/schema.js";
import { BubbleRun, SubagentTool, SystemLine, ToolLine } from "../src/features/sessions/session-detail.js";
import {
  ModelFallbackNotice,
  modelFallbackOf,
  SyntheticNotice,
} from "../src/features/sessions/transcript-notices.js";
import { transcriptItem as item } from "./session-fixtures.js";
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
const session = { source: "claude" as const, id: "s1" };

const draw = pageDraw(i18n, (node) => <QueryClientProvider client={queryClient}>{node}</QueryClientProvider>);

const call = (over: Partial<TranscriptItem> = {}) =>
  item({ kind: "tool_use", toolName: "Bash", text: "cargo test", toolUseId: "t1", ...over });
const answer = (over: Partial<TranscriptItem> = {}) =>
  item({ kind: "tool_result", role: "user", resultOf: "t1", text: "ok", ...over });

function summaryOf(html: string): string {
  const summary = /<summary\b[^>]*>([\s\S]*?)<\/summary>/.exec(html);
  assert.ok(summary, "the command and result remain expandable");
  return summary[1]!;
}

function visibleText(html: string): string {
  return html.replace(/<[^>]*>/g, "");
}

/* ── compact tool summaries ────────────────────────────────────────────── */

test("a tool summary is only icon, description and exit code", () => {
  const description = "Prepare key file, check available keys, confirm upstream clone and free space";
  const command = "umask 077; mkdir -p ~/.config/example";
  const html = draw(
    <ToolLine
      item={call({ text: description, meta: `$ ${command}`, detail: command })}
      result={answer({ text: "Exit code 1", detail: "Permission denied", isError: true })}
    />,
  );
  const summary = summaryOf(html);
  assert.equal(visibleText(summary), `⚙${description}exit 1`);
  assert.doesNotMatch(summary, /umask|mkdir|Permission denied/);
  assert.match(summary, /min-w-0 flex-1 truncate/, "the description owns the remaining width");
  assert.match(summary, /shrink-0 pl-2 tabular-nums[^>]*>exit 1</, "the exit code cannot be truncated");
  assert.match(html, /<pre[^>]*>umask 077; mkdir/);
  assert.match(html, /Permission denied/);
});

test("Codex's exit metadata is used instead of its stdout preview", () => {
  const html = draw(
    <ToolLine
      item={call({ toolName: "exec_command", text: "Run the checks", meta: "/workspace", detail: "cargo test" })}
      result={answer({ text: "42 tests passed", meta: "1.25s · exit 0" })}
    />,
  );
  assert.equal(visibleText(summaryOf(html)), "⚙Run the checksexit 0");
  assert.match(html, /<pre[^>]*>42 tests passed<\/pre>/, "even a result without detail stays readable");
  assert.match(html, /\/workspace/, "working directory metadata moves into the fold");
});

test("a result without an exit code shows completion without inventing zero", () => {
  const html = draw(<ToolLine item={call({ text: "Run checks" })} result={answer({ text: "42 tests passed" })} />);
  assert.equal(visibleText(summaryOf(html)), "⚙Run checkscompleted");
  assert.doesNotMatch(summaryOf(html), /exit 0/);
});

test("a codex command that yielded reads as running in the background, not completed", () => {
  // The server's shape for an `exec_command` answered with a session id: no
  // exit code, not an error, and the partial output in the fold.
  const html = draw(
    <ToolLine
      item={call({ toolName: "shell", text: "uv run long.py", meta: "/w", detail: "uv run long.py" })}
      result={answer({
        text: "Command running in background (session 87142)",
        meta: "running · session 87142 · 1.0s",
        detail: "Downloading torch",
      })}
    />,
  );
  assert.equal(visibleText(summaryOf(html)), "⚙uv run long.pybg");
  assert.doesNotMatch(summaryOf(html), /completed/);
  assert.match(html, /<pre[^>]*>Downloading torch<\/pre>/, "what it printed so far stays readable");
});

test("an exit-code phrase inside output is not promoted to a status code", () => {
  const html = draw(
    <ToolLine item={call({ text: "Review a failure" })} result={answer({ text: "The old build had Exit code 1" })} />,
  );
  assert.equal(visibleText(summaryOf(html)), "⚙Review a failurecompleted");
});

test("a failed call without an exit code keeps a compact failure status", () => {
  const html = draw(
    <ToolLine item={call({ text: "Read config" })} result={answer({ text: "Access denied to a long file path", isError: true })} />,
  );
  assert.equal(visibleText(summaryOf(html)), "⚙Read configfailed");
  assert.match(summaryOf(html), /text-danger/);
  assert.match(html, /<pre[^>]*>Access denied/);
});

test("without a description the tool name is the fallback and the command stays in details", () => {
  const html = draw(<ToolLine item={call({ text: "$ cargo test" })} />);
  assert.equal(visibleText(summaryOf(html)), "⚙Bash");
  assert.match(html, /<pre[^>]*>\$ cargo test<\/pre>/);
});

test("a command caption remains accessible when an older item has no detail", () => {
  const html = draw(<ToolLine item={call({ text: "Run checks", meta: "$ cargo test" })} />);
  assert.equal(visibleText(summaryOf(html)), "⚙Run checks");
  assert.match(html, /<pre[^>]*>\$ cargo test<\/pre>/);
});

/* ── how long it took ───────────────────────────────────────────────────── */

test("a long call keeps its duration on hover instead of taking summary width", () => {
  // Real history holds a single `Agent` call that ran for ninety-two minutes
  // and rendered as an ordinary line with nothing to distinguish it.
  const html = draw(
    <ToolLine
      item={call({ timestamp: "2026-08-30T04:00:00Z" })}
      result={answer({ timestamp: "2026-08-30T05:32:00Z" })}
    />,
  );
  assert.match(html, /92m/);
  assert.doesNotMatch(visibleText(summaryOf(html)), /92m/);
});

test("seconds and minutes are both readable in the row tooltip", () => {
  const html = draw(
    <ToolLine
      item={call({ timestamp: "2026-08-30T04:00:00Z" })}
      result={answer({ timestamp: "2026-08-30T04:01:32Z" })}
    />,
  );
  assert.match(html, /1m32s/);
});

test("an ordinary quick call says nothing about time", () => {
  // A column of `0s` and `1s` would bury the one row that is not ordinary.
  const html = draw(
    <ToolLine
      item={call({ timestamp: "2026-08-30T04:00:00Z" })}
      result={answer({ timestamp: "2026-08-30T04:00:01Z" })}
    />,
  );
  assert.doesNotMatch(html, /how long the call took/);
});

test("a row carries its absolute time on hover, which is flat mode's only anchor", () => {
  const html = draw(<ToolLine item={call({ timestamp: "2026-08-30T04:00:00Z" })} />);
  assert.match(html, /title="[^"]*2026[^"]*"/);
});

test("an unanswered call is marked in flight only while the session is working", () => {
  const running = draw(<ToolLine item={call({ timestamp: "2026-08-30T04:00:00Z" })} live />);
  assert.match(running, /data-live/);
  // Outside a turn the same row is a call that never came back, not one in
  // flight, and an in-flight mark would promise an answer that is not coming.
  const idle = draw(<ToolLine item={call({ timestamp: "2026-08-30T04:00:00Z" })} />);
  assert.doesNotMatch(idle, /data-live/);
});

/* ── boilerplate results, in words ──────────────────────────────────────── */

test("a launch receipt reads as a sentence, not as internal metadata", () => {
  const html = draw(
    <ToolLine
      item={call({ toolName: "Agent", text: "explore the repo" })}
      result={answer({
        text: "Async agent launched successfully. (This tool result is internal metadata and should be ignored.)",
      })}
    />,
  );
  assert.match(html, /async agent started/);
  assert.doesNotMatch(html, /↳ Async agent launched successfully/);
});

test("a background shell launch says so in the same breath", () => {
  const html = draw(
    <ToolLine item={call()} result={answer({ text: "Command running in background (id: 7)" })} />,
  );
  assert.match(html, /running in the background/);
});

test("a real result is left exactly as the server wrote it", () => {
  const html = draw(<ToolLine item={call()} result={answer({ text: "42 tests passed" })} />);
  assert.match(html, /42 tests passed/);
});

/* ── images a result carried ────────────────────────────────────────────── */

test("images on a result render as thumbnails rather than as a count", () => {
  const html = draw(
    <ToolLine item={call({ toolName: "Read", text: "shot.png" })} result={answer({ images: ["/tmp/shot.png"] })} />,
  );
  assert.match(html, /<img/);
  assert.match(html, /shot\.png/);
});

/* ── what a background task reported ────────────────────────────────────── */

test("a background report is rendered as Markdown, not as a wall of text", () => {
  // Real history holds a twenty-two kilobyte report with tables in it that was
  // delivered as pre-wrapped plain text, which is the same as not delivering it.
  const html = draw(
    <SystemLine
      item={item({
        kind: "text",
        role: "system",
        background: true,
        text: "⏱ review finished\n\n| file | verdict |\n| --- | --- |\n| a.ts | ok |",
      })}
      rowClass=""
    />,
  );
  assert.match(html, /<table/);
  assert.match(html, /verdict/);
});

test("a task that failed is drawn as a failure, not as ordinary news", () => {
  const failed = draw(
    <SystemLine
      item={item({ kind: "text", role: "system", background: true, text: "⏱ build the docs · failed" })}
      rowClass=""
    />,
  );
  assert.match(failed, /text-danger/);
  const done = draw(
    <SystemLine
      item={item({ kind: "text", role: "system", background: true, text: "⏱ build the docs" })}
      rowClass=""
    />,
  );
  assert.match(done, /text-warn/);
  assert.doesNotMatch(done, /text-danger/);
});

test("ordinary system chatter stays quiet and stays literal", () => {
  // Not a report: an interrupt marker is one line of machine text and running
  // it through Markdown could only mangle it.
  const html = draw(
    <SystemLine
      item={item({ kind: "text", role: "system", text: "Request interrupted by user\nfor tool use" })}
      rowClass=""
    />,
  );
  assert.match(html, /text-faint/);
  assert.match(html, /whitespace-pre-wrap/);
});

test("the CLI's own word for the outcome decides the colour, not the sentence", () => {
  // What this replaces: a substring test on the headline, which had to read
  // English to guess. `taskStatus` is the CLI saying it.
  const failed = draw(
    <SystemLine
      item={item({
        kind: "text",
        role: "system",
        background: true,
        text: "⏱ ship the release · failed",
        taskStatus: "failed",
      })}
      rowClass=""
    />,
  );
  assert.match(failed, /text-danger/);
  const stopped = draw(
    <SystemLine
      item={item({
        kind: "text",
        role: "system",
        background: true,
        text: "⏱ ship the release · stopped",
        taskStatus: "stopped",
      })}
      rowClass=""
    />,
  );
  assert.match(stopped, /text-muted/);
  assert.doesNotMatch(stopped, /text-danger/, "a stop is not a failure");
});

test("a headline that merely says the word 'failed' is not a failure", () => {
  // The report of a task that *investigated* a failure and finished fine. The
  // old rule read the sentence; this one reads the field.
  const html = draw(
    <SystemLine
      item={item({
        kind: "text",
        role: "system",
        background: true,
        text: "⏱ find out why the build failed",
        taskStatus: "completed",
      })}
      rowClass=""
    />,
  );
  assert.match(html, /text-warn/);
  assert.doesNotMatch(html, /text-danger/);
});

test("the report body arrives in `detail` now, and is still Markdown", () => {
  // The server split the notice in two: one headline in `text`, the whole
  // report in `detail`. The summary must be the line, not the document.
  const html = draw(
    <SystemLine
      item={item({
        kind: "text",
        role: "system",
        background: true,
        text: "⏱ review the diff",
        detail: "## verdict\n\n- one\n- two",
        taskStatus: "completed",
      })}
      rowClass=""
    />,
  );
  assert.match(html, /<summary[^>]*>⏱ review the diff<\/summary>/);
  assert.match(html, /<h2/);
  assert.match(html, /<li[^>]*>one<\/li>/);
});

test("a notice whose card is on screen stops shouting; a failure never does", () => {
  const notice = (over: Partial<TranscriptItem>) =>
    item({ kind: "text", role: "system", background: true, text: "⏱ review the diff", ...over });
  const joined = draw(<SystemLine item={notice({ taskStatus: "completed" })} rowClass="" joined />);
  assert.match(joined, /text-faint\/80/);
  assert.doesNotMatch(joined, /text-warn/, "the card above is already wearing the ✓");
  // A red line that turned grey because the transcript happened to still hold
  // its launch would be the interface hiding the one thing it is for.
  const bad = draw(<SystemLine item={notice({ taskStatus: "failed" })} rowClass="" joined />);
  assert.match(bad, /text-danger/);
});

test("an older server with no status field keeps the sentence-reading fallback", () => {
  const html = draw(
    <SystemLine
      item={item({ kind: "text", role: "system", background: true, text: "⏱ build the docs · failed" })}
      rowClass=""
    />,
  );
  assert.match(html, /text-danger/);
});

/* ── how a subagent card ended ──────────────────────────────────────────── */

const agentCall = (over: Partial<TranscriptItem> = {}) =>
  item({ kind: "tool_use", toolName: "Task", text: "review the diff", toolUseId: "t1", agent: "a1", ...over });

test("a finished subagent wears its outcome, so a fan-out can be read down", () => {
  // Before this, eight `Task` calls were eight identical rows whatever had
  // become of them, and the only way to tell was to open each one.
  const done = draw(<SubagentTool item={agentCall()} session={session} taskStatus="completed" />);
  assert.match(done, /✓/);
  assert.match(done, /text-ok/);
  assert.doesNotMatch(done, /data-live/);

  const failed = draw(<SubagentTool item={agentCall()} session={session} taskStatus="failed" />);
  assert.match(failed, /✗/);
  assert.match(failed, /text-danger/);

  const stopped = draw(<SubagentTool item={agentCall()} session={session} taskStatus="stopped" />);
  assert.match(stopped, /⊘/);
});

test("a subagent the resident is minding is marked as still running", () => {
  const html = draw(<SubagentTool item={agentCall()} session={session} residentRunning />);
  assert.match(html, /data-live/);
});

test("a subagent nobody can account for says so instead of guessing", () => {
  // Acked (so its own result is only a receipt), gone from the resident's list,
  // and no notification in the window. Neither a live mark nor a ✓ would be true.
  const html = draw(
    <SubagentTool
      item={agentCall()}
      session={session}
      result={answer({ text: "Async agent launched successfully. (internal metadata)" })}
    />,
  );
  assert.match(html, /⊘/);
  assert.match(html, /is unknown/, "and the tooltip says which kind of ⊘ this is");
  assert.doesNotMatch(html, /data-live/);
});

test("the subagent's own receipt is folded onto its card, not left in the stream", () => {
  const html = draw(
    <SubagentTool
      item={agentCall()}
      session={session}
      taskStatus="completed"
      result={answer({ text: "reviewed 4 files", detail: "a.ts: ok\nb.ts: ok" })}
    />,
  );
  assert.match(html, /reviewed 4 files/);
  assert.match(html, /a\.ts: ok/);
});

/* ── the CLI talking, and the model being swapped ───────────────────────── */

test("a model swap is drawn as news rather than as another grey line", () => {
  assert.deepEqual(modelFallbackOf("model fallback: claude-fable-5 → claude-opus-4-8"), {
    from: "claude-fable-5",
    to: "claude-opus-4-8",
  });
  assert.equal(modelFallbackOf("⏱ background task finished"), null);
  const html = draw(
    <ModelFallbackNotice fallback={{ from: "claude-fable-5", to: "claude-opus-4-8" }} rowClass="" />,
  );
  assert.match(html, /model fallback/);
  assert.match(html, /claude-fable-5/);
  assert.match(html, /claude-opus-4-8/);
  assert.match(html, /text-warn/, "a silent downgrade stops being silent");
});

/// The server writes `model fallback: → {to}` when the block names no model to
/// come from, and a pattern that demanded one sent that line — a real, silent
/// swap — back to being one more grey row.
test("a swap that names only where it went is still a swap", () => {
  assert.deepEqual(modelFallbackOf("model fallback: → claude-opus-4-8"), {
    from: null,
    to: "claude-opus-4-8",
  });
  assert.deepEqual(
    modelFallbackOf("model fallback: claude-fable-5 -> claude-opus-4-8"),
    { from: "claude-fable-5", to: "claude-opus-4-8" },
    "the ascii arrow still reads",
  );
  assert.deepEqual(
    modelFallbackOf("model fallback: claude-fable-5 → ?"),
    { from: "claude-fable-5", to: "?" },
    "the server's own word for a destination it could not read",
  );
  const html = draw(<ModelFallbackNotice fallback={{ from: null, to: "claude-opus-4-8" }} rowClass="" />);
  assert.match(html, /claude-opus-4-8/);
  assert.match(html, /text-warn/);
  assert.doesNotMatch(html, /line-through/, "no struck-through blank where a name would have been");
});

test("a message the CLI wrote itself is not drawn as the model speaking", () => {
  // A rate-limit wall reads as a decision the model made — the reader argues
  // with it instead of topping up a quota.
  const html = draw(
    <SyntheticNotice
      item={item({ kind: "text", role: "assistant", synthetic: true, text: "You've hit your **usage limit**." })}
      rowClass=""
    />,
  );
  assert.match(html, /from the CLI, not from the model/);
  assert.match(html, /<strong>usage limit<\/strong>/, "the sentence is still worth reading");
});

/* ── nothing on a row may take the transcript sideways ──────────────────── */

test("a long tool name stays on the icon tooltip instead of competing with the description", () => {
  const name = "mcp__claude-in-chrome__list_connected_browsers";
  const html = draw(<ToolLine item={call({ toolName: name, text: "List connected browsers" })} />);
  assert.match(html, new RegExp(`title="${name}" role="img" aria-label="${name}"`));
  assert.equal(visibleText(html), "⚙List connected browsers");
});

test("short tool names also stay off the visible description line", () => {
  const html = draw(<ToolLine item={call({ toolName: "Bash", text: "Run tests" })} />);
  assert.equal(visibleText(html), "⚙Run tests");
  assert.match(html, /aria-label="Bash"/);
});

test("the transcript scroller clips sideways instead of scrolling", () => {
  // A source assertion, because the invariant is a CSS one and this harness
  // has no layout: `renderToStaticMarkup` would happily report a row four
  // hundred pixels wider than its parent. The rows above are all built to
  // give, so nothing should reach this clip — it is the backstop for the next
  // row that forgets, and it is the only thing that makes "the stream never
  // scrolls left and right" true rather than merely true of today's rows.
  const src = readFileSync(new URL("../src/features/sessions/session-detail.tsx", import.meta.url), "utf8");
  const scroller = /className="transcript-scroll[^"]*"/.exec(src);
  assert.ok(scroller, "the transcript scroller is still found by its marker class");
  assert.match(scroller[0], /overflow-x-hidden/);
});

/* ── which model wrote the turn ─────────────────────────────────────────── */

test("an agent bubble names its model between the role and the time", () => {
  const html = draw(
    <BubbleRun role="assistant" model="claude-opus-5" timestamp="2026-09-23T12:28:00Z">
      <span>done</span>
    </BubbleRun>,
  );
  // The spacing between the three is the flex gap, which is CSS and not text,
  // so stripped markup reads them back to back: `AGENTclaude-opus-512:28`.
  assert.match(visibleText(html), /^AGENTclaude-opus-5.+done$/, "the header order the reader scans");
  assert.match(html, /title="claude-opus-5"/, "the full id is on hover as well");
  assert.match(html, /flex-wrap/, "a long id wraps instead of pushing the time off the row");
  assert.doesNotMatch(html, /truncate/, "and is never elided — the id is the point");
});

test("a long model id is shown whole, not elided", () => {
  // A header that says `GLM-5.3-CYBERSECURITY-…` has hidden the exact half the
  // id is on screen for. One bubble only ever names one model — a turn that was
  // downgraded halfway is two bubbles — so there is nothing to make room for.
  const long = "GLM-5.3-CYBERSECURITY-FP8";
  const html = draw(
    <BubbleRun role="assistant" model={long} timestamp={null}>
      <span>done</span>
    </BubbleRun>,
  );
  assert.equal(visibleText(html), `AGENT${long}done`);
  assert.match(html, /title="GLM-5.3-CYBERSECURITY-FP8"/);
});

test("a bubble the transcript named no model for reads as it always has", () => {
  const html = draw(
    <BubbleRun role="assistant" model={null} timestamp={null}>
      <span>done</span>
    </BubbleRun>,
  );
  assert.equal(visibleText(html), "AGENTdone");
  assert.doesNotMatch(html, /·/, "no separator with nothing to separate");
});
