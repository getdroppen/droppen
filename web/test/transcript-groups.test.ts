import assert from "node:assert/strict";
import test from "node:test";
import type { components } from "../src/api/schema.js";
import {
  bubbleRuns,
  chunkItems,
  countSteps,
  firstTimestamp,
  foldClassOf,
  modelOf,
  modelRunBreaks,
  pairToolResults,
  runModelOf,
} from "../src/features/sessions/transcript-groups.js";
import { transcriptItem as item } from "./session-fixtures.js";

type TranscriptItem = components["schemas"]["TranscriptItem"];

const text = (role: TranscriptItem["role"], t: string, timestamp: string | null = null) =>
  item({ role, kind: "text", text: t, timestamp });
const toolUse = (name: string) => item({ role: "assistant", kind: "tool_use", toolName: name });
const toolResult = () => item({ role: "user", kind: "tool_result" });

test("a turn of text → tool calls → text stays one bubble", () => {
  // The reported bug: the tool calls between two paragraphs split the agent's
  // turn into two separate bubbles.
  const runs = bubbleRuns(
    chunkItems([
      text("assistant", "本地已全绿，现在修 coverage:", "2026-08-01T20:45:00Z"),
      toolUse("Read"),
      toolResult(),
      toolUse("Edit"),
      toolResult(),
      toolUse("Bash"),
      text("assistant", "我把 workflow 改动误并进了 lint 提交，拆开:"),
    ]),
  );

  assert.equal(runs.length, 1, "the whole agent turn should be a single bubble");
  assert.equal(runs[0]!.role, "assistant");
  assert.equal(firstTimestamp(runs[0]!.chunks), "2026-08-01T20:45:00Z");
});

test("user and agent turns still get their own bubbles", () => {
  const runs = bubbleRuns(
    chunkItems([
      text("user", "fix CI"),
      text("assistant", "on it"),
      toolUse("Bash"),
      text("assistant", "done"),
      text("user", "thanks"),
    ]),
  );

  assert.deepEqual(
    runs.map((r) => r.role),
    ["user", "assistant", "user"],
  );
});

test("tool results group with the agent despite carrying the user role", () => {
  const runs = bubbleRuns(chunkItems([text("assistant", "checking"), toolResult()]));
  assert.equal(runs.length, 1);
  assert.equal(runs[0]!.role, "assistant");
});

test("system lines stay standalone between bubbles", () => {
  const runs = bubbleRuns(
    chunkItems([text("assistant", "a"), text("system", "❯ /compact"), text("assistant", "b")]),
  );
  assert.deepEqual(
    runs.map((r) => r.role),
    ["assistant", "system", "assistant"],
  );
});

test("a folded step group belongs to the agent's bubble", () => {
  // 5+ foldable items collapse into one group chunk (GROUP_MIN).
  const runs = bubbleRuns(
    chunkItems([
      text("assistant", "working"),
      toolUse("Read"),
      toolResult(),
      toolUse("Read"),
      toolResult(),
      toolUse("Bash"),
      text("assistant", "finished"),
    ]),
  );

  assert.equal(runs.length, 1);
  const kinds = runs[0]!.chunks.map((c) => c.kind);
  assert.ok(kinds.includes("group"), `expected a folded group, got ${kinds.join(",")}`);
});

test("liveTail keeps the newest tool call out of the fold while running", () => {
  const items = [
    text("assistant", "working on it"),
    toolUse("Read"),
    toolResult(),
    toolUse("Edit"),
    toolResult(),
    toolUse("Bash"),
    toolResult(),
    toolUse("Bash"),
  ];

  const idle = chunkItems(items);
  assert.deepEqual(
    idle.map((c) => c.kind),
    ["item", "group"],
    "idle: the whole tool run folds",
  );

  const live = chunkItems(items, true);
  assert.deepEqual(
    live.map((c) => c.kind),
    ["item", "group", "item"],
    "running: the newest tool call stays visible",
  );
  const tail = live[live.length - 1]!;
  assert.equal(tail.kind === "item" && tail.item.toolName, "Bash");
  assert.equal(tail.index, 7, "exposed chunk keeps its absolute index");
});

test("liveTail exposes the call and its result once it lands", () => {
  const items = [
    toolUse("Read"),
    toolResult(),
    toolUse("Edit"),
    toolResult(),
    toolUse("Bash"),
    toolResult(),
  ];
  const live = chunkItems(items, true);
  // Prefix shrinks below GROUP_MIN, so it renders as plain items — same rule
  // any short run follows; only the newest call is guaranteed visible.
  assert.deepEqual(
    live.map((c) => (c.kind === "item" ? (c.item.kind === "tool_use" ? c.item.toolName : "result") : "group")),
    ["Read", "result", "Edit", "result", "Bash", "result"],
  );
});

test("liveTail leaves a tail without tool calls folded", () => {
  const items = [
    toolUse("Read"),
    toolResult(),
    toolUse("Edit"),
    toolResult(),
    item({ role: "assistant", kind: "thinking", text: "hmm" }),
  ];
  // The newest tool call is Edit; thinking after it stays visible with it.
  const live = chunkItems(items, true);
  assert.equal(live[live.length - 1]!.kind, "item");
  const chunkKinds = live.map((c) => c.kind);
  assert.ok(!chunkKinds.includes("group"), "5 items minus exposed tail folds nothing");
});

/** The `⏱ background task finished` line the CLI drops into a running turn. */
const notice = (label = "background task finished") =>
  item({ role: "system", kind: "text", text: `⏱ ${label}`, background: true });

test("a background notice mid-run does not split the run", () => {
  // The reported bug: one invisible bookkeeping line between step 6 and step 7
  // cut a twelve-step run into a five and a six, both under the threshold, so
  // the whole thing rendered expanded.
  const half = () => [toolUse("Read"), toolResult(), toolUse("Bash"), toolResult(), toolUse("Grep"), toolResult()];
  const chunks = chunkItems([...half(), notice(), ...half()]);

  assert.deepEqual(chunks.map((c) => c.kind), ["group"], "the notice must not be a boundary");
  const group = chunks[0]!;
  assert.equal(group.kind === "group" && group.items.length, 13, "the notice rides inside the fold");
  assert.equal(
    group.kind === "group" && group.items[6]!.text,
    "⏱ background task finished",
    "and keeps its position, so opening the fold still shows it where it happened",
  );
});

test("a background notice does not count towards the fold threshold", () => {
  // Two calls and three notices is two steps of work, not five.
  const chunks = chunkItems([
    toolUse("Read"),
    toolResult(),
    notice("a"),
    notice("b"),
    notice("c"),
    toolUse("Bash"),
    toolResult(),
  ]);
  assert.ok(
    chunks.every((c) => c.kind === "item"),
    "nothing may fold on bookkeeping alone",
  );
});

test("the fold threshold and the summary count the same thing", () => {
  const items = [toolUse("Read"), toolResult(), toolUse("Bash"), toolResult(), toolUse("Grep"), notice()];
  const chunks = chunkItems(items);
  assert.equal(chunks.length, 1);
  const group = chunks[0]!;
  assert.equal(group.kind, "group");
  assert.equal(
    countSteps(group.kind === "group" ? group.items : []),
    3,
    "a group that folded on 3 steps must also print 3",
  );
});

test("an agent call still never folds", () => {
  // Guarded because the drill-down card is the one row that must stay clickable.
  const agent = item({ role: "assistant", kind: "tool_use", toolName: "Task", agent: "agent-1" });
  assert.equal(foldClassOf(agent), "break");
  const chunks = chunkItems([toolUse("Read"), toolResult(), agent, toolUse("Bash"), toolResult()]);
  const exposed = chunks.filter((c) => c.kind === "item" && c.item.agent === "agent-1");
  assert.equal(exposed.length, 1, "the subagent row stays its own chunk");
});

test("a background tool launch is still a boundary", () => {
  // Its receipt lands out of order among the steps, so it is not transparent.
  const launch = item({ role: "assistant", kind: "tool_use", toolName: "Bash", background: true });
  assert.equal(foldClassOf(launch), "break");
});

const call = (id: string, name: string, extra: Partial<TranscriptItem> = {}) =>
  item({ role: "assistant", kind: "tool_use", toolName: name, toolUseId: id, ...extra });
const answer = (id: string, extra: Partial<TranscriptItem> = {}) =>
  item({ role: "user", kind: "tool_result", resultOf: id, ...extra });

test("a result is paired with its call and marked for merging", () => {
  const bash = call("t1", "Bash");
  const output = answer("t1", { text: "no output" });
  const pairing = pairToolResults([bash, output]);
  assert.equal(pairing.resultFor.get("t1"), output);
  assert.ok(pairing.consumed.has(output), "the call renders it, so the row must not");
});

test("a result whose call is outside the window keeps rendering on its own", () => {
  // The window was paged back: only the result survived into it.
  const orphan = answer("gone", { text: "…" });
  const pairing = pairToolResults([orphan]);
  assert.equal(pairing.resultFor.get("gone"), orphan);
  assert.ok(!pairing.consumed.has(orphan), "nothing else would show this output");
});

test("a question's result is left where it is, a subagent's is absorbed", () => {
  // A question card draws its own body, so absorbing its result would hide it.
  //
  // The subagent half is a deliberate reversal of the rule this test used to
  // guard. The server now stamps a subagent's *result* with the launching
  // call's `agent`, so it arrives as a top-level row of its own — a second line
  // directly under the card, saying the thing the card's own ✓ already says.
  // The card folds it instead (`AgentResultLine`), which is where a terminal
  // receipt belongs.
  const task = call("t1", "Task", { agent: "agent-1" });
  const taskResult = answer("t1", { agent: "agent-1" });
  const asked = call("t2", "AskUserQuestion");
  const askedResult = answer("t2");
  const pairing = pairToolResults([task, taskResult, asked, askedResult]);
  assert.equal(pairing.resultFor.get("t1"), taskResult, "the card needs it to draw its receipt");
  assert.ok(pairing.consumed.has(taskResult), "so the stream must not draw it a second time");
  assert.ok(!pairing.consumed.has(askedResult));
});

test("an absorbed subagent result does not cut the run it sits in", () => {
  // It renders nothing at all once the card has it, so treating it as a
  // boundary would split a run around a row that is not there — the same
  // failure the `transparent` class exists for.
  const agentResult = item({ role: "user", kind: "tool_result", agent: "agent-1", resultOf: "t9" });
  assert.equal(foldClassOf(agentResult), "transparent");
  const chunks = chunkItems([
    toolUse("Read"),
    toolResult(),
    agentResult,
    toolUse("Bash"),
    toolResult(),
    toolUse("Grep"),
  ]);
  assert.deepEqual(chunks.map((c) => c.kind), ["group"]);
});

/* ── the `⏱` notification, joined back to the call that started it ─────── */

test("a notification's status is filed under the call that launched the task", () => {
  const launch = call("t1", "Task", { agent: "agent-1" });
  const notice = item({
    role: "system",
    kind: "text",
    background: true,
    text: "⏱ review the diff",
    taskToolUseId: "t1",
    taskStatus: "completed",
  });
  const pairing = pairToolResults([launch, notice]);
  assert.equal(pairing.taskStatusFor.get("t1"), "completed");
  assert.ok(pairing.noticeJoined.has(notice), "the card is on screen, so the notice stops shouting");
});

test("a background command's notice keeps its voice — nothing else would report it", () => {
  // A `run_in_background` Bash is an ordinary tool row with nowhere to put a ✓,
  // so quieting its notice would take the news off the screen and put it
  // nowhere. Only a call that draws a card can absorb one.
  const launch = call("t1", "Bash", { background: true });
  const notice = item({
    role: "system",
    kind: "text",
    background: true,
    text: "⏱ cargo test",
    taskToolUseId: "t1",
    taskStatus: "completed",
  });
  const pairing = pairToolResults([launch, notice]);
  assert.equal(pairing.taskStatusFor.get("t1"), "completed");
  assert.ok(!pairing.noticeJoined.has(notice));
});

test("a notification whose call is out of the window keeps its own voice", () => {
  // Paged history, or a notice the CLI wrote with no tool-use id at all: then
  // this line is the only place the news appears.
  const orphan = item({
    role: "system",
    kind: "text",
    background: true,
    text: "⏱ build the docs · failed",
    taskToolUseId: "gone",
    taskStatus: "failed",
  });
  const anonymous = item({ role: "system", kind: "text", background: true, text: "⏱ something" });
  const pairing = pairToolResults([orphan, anonymous]);
  assert.equal(pairing.taskStatusFor.get("gone"), "failed", "still worth filing, in case the call loads");
  assert.ok(!pairing.noticeJoined.has(orphan));
  assert.ok(!pairing.noticeJoined.has(anonymous));
});

test("the newest word on a task is the one that describes how it ended", () => {
  const launch = call("t1", "Task", { agent: "agent-1" });
  const running = item({ role: "system", kind: "text", background: true, text: "⏱ a", taskToolUseId: "t1", taskStatus: "running" });
  const done = item({ role: "system", kind: "text", background: true, text: "⏱ a", taskToolUseId: "t1", taskStatus: "stopped" });
  assert.equal(pairToolResults([launch, running, done]).taskStatusFor.get("t1"), "stopped");
});

test("pairing ignores items the server sent no ids for", () => {
  // Codex transcripts carry no tool_use ids, so nothing may be merged.
  const pairing = pairToolResults([toolUse("Read"), toolResult()]);
  assert.equal(pairing.resultFor.size, 0);
  assert.equal(pairing.consumed.size, 0);
});

test("a repeated result id keeps the first answer", () => {
  const one = call("t1", "Bash");
  const first = answer("t1", { text: "first" });
  const second = answer("t1", { text: "second" });
  const pairing = pairToolResults([one, first, second]);
  assert.equal(pairing.resultFor.get("t1"), first);
  assert.ok(pairing.consumed.has(second), "both belong to a call that is on screen");
});

test("merged results stay foldable, so a run of steps still groups", () => {
  // Regression guard: pairing must not change how runs are chunked, or the
  // paging indices the transcript keys on would shift under it.
  const items = [
    call("t1", "Read"),
    answer("t1"),
    call("t2", "Edit"),
    answer("t2"),
    call("t3", "Bash"),
    answer("t3"),
  ];
  const chunks = chunkItems(items);
  assert.equal(chunks.length, 1);
  assert.equal(chunks[0]!.kind, "group");
});

/* ── which model wrote it ───────────────────────────────────────────────── */

const said = (model: string | null, over: Partial<TranscriptItem> = {}) =>
  item({ role: "assistant", kind: "text", text: "…", model, ...over });

test("a message names the model it came from, whole", () => {
  // Never truncated: `GLM-5.3-CYBERSECURITY-…` hides the half that answers the
  // question the id is on screen to answer.
  assert.equal(modelOf(said("GLM-5.3-CYBERSECURITY-FP8")), "GLM-5.3-CYBERSECURITY-FP8");
  assert.equal(modelOf(said("  claude-opus-5  ")), "claude-opus-5");
});

test("a message the transcript says nothing about stays unlabelled", () => {
  assert.equal(modelOf(text("assistant", "done")), null);
  assert.equal(modelOf(said(null)), null);
  assert.equal(runModelOf([]), null);
});

test("a model on a synthetic message is not the model speaking", () => {
  // The CLI wrote that sentence in the model's voice; crediting a model with it
  // is exactly the confusion `synthetic` exists to undo.
  assert.equal(modelOf(said("claude-opus-5", { synthetic: true })), null);
});

test("a run is on the first model any of its items names", () => {
  assert.equal(runModelOf([text("assistant", "…"), said("claude-opus-5"), said("claude-sonnet-5")]), "claude-opus-5");
});

/* ── one group, one model ───────────────────────────────────────────────── */

test("the split rule: only a different known model ends a run", () => {
  assert.equal(modelRunBreaks("claude-opus-5", "claude-opus-5"), false, "same model runs on");
  assert.equal(modelRunBreaks("claude-opus-5", "claude-sonnet-5"), true, "a downgrade ends the run");
  assert.equal(modelRunBreaks("claude-opus-5", null), false, "an item nobody named rides along");
  assert.equal(modelRunBreaks(null, "claude-opus-5"), true, "a name after none is news");
  assert.equal(modelRunBreaks(null, null), false, "nothing said either side");
});

test("a fold holds steps from one model only", () => {
  const step = (model: string | null) => item({ role: "assistant", kind: "tool_use", toolName: "Bash", model });
  const chunks = chunkItems([step("claude-opus-5"), step("claude-opus-5"), step("claude-opus-5"), step("claude-sonnet-5"), step("claude-sonnet-5"), step("claude-sonnet-5")]);
  assert.equal(chunks.length, 2, "the model change cuts the run in two");
  assert.deepEqual(chunks.map((c) => c.kind), ["group", "group"]);
  assert.equal(runModelOf((chunks[0] as { items: TranscriptItem[] }).items), "claude-opus-5");
  assert.equal(runModelOf((chunks[1] as { items: TranscriptItem[] }).items), "claude-sonnet-5");
});

test("a turn on one model is one bubble, headed by it", () => {
  const runs = bubbleRuns(
    chunkItems([
      said("claude-opus-5", { text: "on it", timestamp: "2026-09-23T12:28:00Z" }),
      item({ role: "assistant", kind: "tool_use", toolName: "Read", model: "claude-opus-5" }),
      toolResult(),
      said("claude-opus-5", { text: "done" }),
    ]),
  );
  assert.equal(runs.length, 1);
  assert.equal(runs[0]!.model, "claude-opus-5");
  assert.equal(firstTimestamp(runs[0]!.chunks), "2026-09-23T12:28:00Z");
});

test("a turn that was downgraded halfway becomes two bubbles, in order", () => {
  // The reason the field is on screen at all: the second half reads worse and
  // the header is the only thing that can say why.
  const runs = bubbleRuns(
    chunkItems([
      said("claude-opus-5", { text: "first" }),
      said("claude-sonnet-5", { text: "second", timestamp: "2026-09-23T12:31:00Z" }),
    ]),
  );
  assert.deepEqual(runs.map((r) => r.model), ["claude-opus-5", "claude-sonnet-5"]);
  assert.equal(runs[0]!.chunks.length, 1);
  assert.equal(firstTimestamp(runs[1]!.chunks), "2026-09-23T12:31:00Z", "the second bubble wears its own time");
});

test("an item nobody named between two on the same model keeps one bubble", () => {
  const runs = bubbleRuns(
    chunkItems([
      said("claude-opus-5", { text: "first" }),
      text("assistant", "a line the CLI stamped with nothing"),
      said("claude-opus-5", { text: "second" }),
    ]),
  );
  assert.equal(runs.length, 1, "an unknown model splits nothing by itself");
  assert.equal(runs[0]!.chunks.length, 3);
  assert.equal(runs[0]!.model, "claude-opus-5");
});

test("a named model after an unnamed run starts its own bubble", () => {
  const runs = bubbleRuns(chunkItems([text("assistant", "from before the field existed"), said("claude-opus-5", { text: "now" })]));
  assert.deepEqual(runs.map((r) => r.model), [null, "claude-opus-5"]);
});

test("a user turn is untouched by any of this", () => {
  const runs = bubbleRuns(chunkItems([text("user", "fix CI"), said("claude-opus-5", { text: "on it" })]));
  assert.deepEqual(runs.map((r) => [r.role, r.model]), [["user", null], ["assistant", "claude-opus-5"]]);
});
