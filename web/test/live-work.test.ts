// What "still running" means, decided away from any component.
//
// Two separate questions live here, and each of them used to be answered
// somewhere it could not be checked: which children of a session are still
// out (`liveSubtasks`), and whether the floating panel has anything to say
// (`backgroundPanel`). The drain condition joins them because it has the same
// shape — a rule that is easy to get subtly wrong and impossible to see go
// wrong from the outside.
import assert from "node:assert/strict";
import test from "node:test";
import type { components } from "../src/api/schema.js";
import {
  activityBadge,
  backgroundPanel,
  liveSubtasks,
  panelRows,
  queuedMessagesOf,
  residentAbsenceIsFinal,
  type ActivityRow,
  type BackgroundTask,
  type DrivenRow,
  type ResidentInfo,
} from "../src/features/sessions/session-activity.js";
import {
  injectionFailurePausesQueue,
  localQueueHeadAction,
  shouldDrainQueue,
  type QueueDrainState,
} from "../src/features/sessions/queue-drain.js";
import { formatElapsed } from "../src/lib/format.js";

type TranscriptItem = components["schemas"]["TranscriptItem"];

function resident(over: Partial<ResidentInfo> = {}): ResidentInfo {
  return {
    phase: "idle",
    queued: 0,
    queuedMessages: [],
    background: 0,
    awaitingBackground: false,
    stalled: false,
    ...over,
  };
}

function item(over: Partial<TranscriptItem> & Pick<TranscriptItem, "kind">): TranscriptItem {
  return {
    detail: null,
    isError: false,
    meta: null,
    question: null,
    role: "assistant",
    text: "",
    timestamp: null,
    toolName: null,
    truncated: false,
    ...over,
  };
}

function task(id: string, text: string, meta: string | null = null): TranscriptItem {
  return item({ kind: "tool_use", toolName: "Task", agent: `a-${id}`, toolUseId: id, text, meta });
}

/* ── every child, not just the last one ─────────────────────────────────── */

test("a fan-out is listed in full, in the order it was launched", () => {
  // The session the one-line banner was useless for: four agents out at once,
  // and "something is running" as the entire answer.
  const items = [
    task("u1", "explore the repo"),
    task("u2", "write the tests"),
    task("u3", "review the diff"),
  ];
  assert.deepEqual(
    liveSubtasks(items).map((s) => s.text),
    ["explore the repo", "write the tests", "review the diff"],
  );
  // The single-child form still means "the newest one", so the composer strip
  // is unchanged where it still draws it.
  assert.equal((liveSubtasks(items).at(-1) ?? null)?.text, "review the diff");
});

test("children that have reported back drop out, the rest keep their order", () => {
  const items = [
    task("u1", "explore the repo"),
    task("u2", "write the tests"),
    item({ kind: "tool_result", resultOf: "u1", text: "done" }),
    task("u3", "review the diff"),
  ];
  assert.deepEqual(
    liveSubtasks(items).map((s) => s.id),
    ["u2", "u3"],
  );
});

/* ── launch receipts are not reports back ───────────────────────────────── */

/** The `Agent` receipt, as the CLI writes it, with its trailing agent id. */
const AGENT_ACK = "Async agent launched successfully with ID: agent_01H9";
/** The `Bash` one, same shape and same two-second latency. */
const BASH_ACK = "Command running in background with ID: bash_7";

test("a launch receipt does not retire the child it launched", () => {
  // The shipped bug, in the shape that produced it: an async agent is answered
  // within about two seconds by a receipt saying it is *out*, and reading that
  // as "reported back" dropped every long-running child from the panel whose
  // entire job is to show them. Measured on a real seven-agent fan-out: none
  // of the seven were listed.
  const items = [
    task("u1", "audit the replay"),
    item({ kind: "tool_result", resultOf: "u1", text: AGENT_ACK }),
  ];
  assert.deepEqual(
    liveSubtasks(items).map((s) => s.text),
    ["audit the replay"],
  );
  assert.equal((liveSubtasks(items).at(-1) ?? null)?.id, "u1", "the composer strip agrees with the panel");
});

test("a background shell stays listed while the CLI is still minding it", () => {
  const items = [
    item({ kind: "tool_use", toolName: "Bash", background: true, toolUseId: "u1", text: "pnpm build" }),
    item({ kind: "tool_result", resultOf: "u1", text: BASH_ACK }),
  ];
  assert.deepEqual(
    liveSubtasks(items).map((s) => s.id),
    ["u1"],
  );
});

test("a receipt recorded only as expanded content is still a receipt", () => {
  // The server derives a result's one-line `text` from the first line of its
  // `detail`, so a transcript that reaches us carrying only one of the two is
  // the same receipt and must not close the row either.
  const items = [
    task("u1", "explore the repo"),
    item({ kind: "tool_result", resultOf: "u1", text: "", detail: `${AGENT_ACK}\n\nUse the id to check on it.` }),
  ];
  assert.equal(liveSubtasks(items).length, 1);
});

test("a fan-out survives its receipts and keeps its launch order", () => {
  // Half the point of listing them: four children out, four receipts back, and
  // the question is still which four and for how long.
  const items = [
    task("u1", "explore the repo"),
    item({ kind: "tool_result", resultOf: "u1", text: AGENT_ACK }),
    task("u2", "write the tests"),
    item({ kind: "tool_result", resultOf: "u2", text: AGENT_ACK }),
    item({ kind: "tool_use", toolName: "Bash", background: true, toolUseId: "u3", text: "pnpm build" }),
    item({ kind: "tool_result", resultOf: "u3", text: BASH_ACK }),
  ];
  assert.deepEqual(
    liveSubtasks(items).map((s) => s.id),
    ["u1", "u2", "u3"],
  );
});

test("a foreground agent's real answer still closes its row", () => {
  // The half of the pairing that was never broken, and the half a fix for the
  // other half is most likely to break: a synchronous `Task` does report back
  // through its own `tool_result`, and that one means finished.
  const items = [
    task("u1", "review the diff"),
    item({
      kind: "tool_result",
      resultOf: "u1",
      text: "The diff is fine; two nits below.",
      detail: "The diff is fine; two nits below.\n\n1. …",
    }),
    task("u2", "write the tests"),
  ];
  assert.deepEqual(
    liveSubtasks(items).map((s) => s.id),
    ["u2"],
  );
});

test("a launch that failed outright is not left running", () => {
  // An error on the launch call *is* the end of that child, and the receipt
  // rule must not turn a failed launch into a permanent row.
  const items = [
    task("u1", "explore the repo"),
    item({
      kind: "tool_result",
      resultOf: "u1",
      isError: true,
      text: "Async agent launched successfully — then the process died",
    }),
  ];
  assert.deepEqual(liveSubtasks(items), []);
});

test("an acked launch still ages out of the lookback window", () => {
  // With no id-bearing completion signal in the transcript, this window is the
  // only bound on being wrong about an acked launch — so it has to hold. The
  // trade-off it buys: a stale row for at most a turn's worth of steps, rather
  // than a running child the panel never names.
  const launched = [task("u1", "long gone"), item({ kind: "tool_result", resultOf: "u1", text: AGENT_ACK })];
  const filler = Array.from({ length: 90 }, () => item({ kind: "text", text: "…" }));
  assert.deepEqual(liveSubtasks([...launched, ...filler]), []);
  assert.equal(liveSubtasks([...launched, ...filler.slice(0, 60)]).length, 1);
});

test("the subagent type is the label, not the whole caption", () => {
  // The CLI writes `subagent_type · model` into one string. The type says what
  // the child is *for* and fits beside the description; the model does not.
  const [only] = liveSubtasks([task("u1", "explore the repo", "Explore · opus")]);
  assert.equal(only?.agentType, "Explore");
  assert.equal(only?.meta, "Explore · opus", "the full caption survives for the tooltip");
});

test("a background launch has no agent type, because it is not an agent", () => {
  const [only] = liveSubtasks([
    item({ kind: "tool_use", toolName: "Bash", background: true, toolUseId: "u1", text: "pnpm build" }),
  ]);
  assert.equal(only?.background, true);
  assert.equal(only?.agentType, null);
});

test("the launching call's expanded content comes along, prompt or command", () => {
  // The row's one-line text is a *description* the same turn wrote; the prompt
  // is what the child was actually handed. A vague description is exactly when
  // the difference matters, so the panel can open onto the real thing.
  const [agent] = liveSubtasks([
    item({
      kind: "tool_use",
      toolName: "Task",
      agent: "a1",
      toolUseId: "u1",
      text: "explore the repo",
      detail: "Read every route file and report the auth checks.",
    }),
  ]);
  assert.equal(agent?.detail, "Read every route file and report the auth checks.");
  const [shell] = liveSubtasks([
    item({ kind: "tool_use", toolName: "Bash", background: true, toolUseId: "u2", text: "pnpm build", detail: "pnpm -r build --watch" }),
  ]);
  assert.equal(shell?.detail, "pnpm -r build --watch");
  // Nothing recorded stays nothing: the panel draws a plain row for it rather
  // than a fold onto an empty box.
  assert.equal(liveSubtasks([task("u3", "write the tests")])[0]?.detail, null);
});

test("the launch timestamp comes along, because a duration needs one", () => {
  const items = [
    item({
      kind: "tool_use",
      toolName: "Task",
      agent: "a1",
      toolUseId: "u1",
      text: "explore",
      timestamp: "2026-08-30T04:00:00Z",
    }),
  ];
  assert.equal(liveSubtasks(items)[0]?.startedAt, "2026-08-30T04:00:00Z");
});

test("children older than the lookback are dropped from the list too", () => {
  // An interrupted subagent never gets a result and looks unfinished forever;
  // the list must not grow a permanent ghost row for it.
  const stale = task("u1", "long dead");
  const filler = Array.from({ length: 90 }, () => item({ kind: "text", text: "…" }));
  assert.deepEqual(liveSubtasks([stale, ...filler]), []);
  assert.equal(liveSubtasks([stale, ...filler.slice(0, 70)]).length, 1);
});

/* ── whether the floating panel appears at all ──────────────────────────── */

test("background work always earns the panel, even with nothing nameable", () => {
  // The transcript can be loaded from after the launch, so the resident's
  // count is all there is. Saying "2 running" with an empty list still beats
  // a session that looks finished.
  const row: ActivityRow = { status: "idle", activity: "background", resident: resident({ background: 2 }) };
  const panel = backgroundPanel({ row, subtasks: [] });
  assert.equal(panel?.kind, "background");
  assert.equal(panel?.count, 2);
  assert.deepEqual(panel?.subtasks, []);
});

test("the count is the larger of what the resident says and what is visible", () => {
  // A server too old to report `background`, or one whose count lags the
  // transcript: naming fewer children than are listed underneath is the one
  // combination that reads as a bug.
  const row: ActivityRow = { status: "idle", activity: "background", resident: resident({ background: 0 }) };
  const panel = backgroundPanel({ row, subtasks: liveSubtasks([task("u1", "explore"), task("u2", "test")]) });
  assert.equal(panel?.count, 2);
});

test("a working turn earns the panel only when it has children at all", () => {
  const row: ActivityRow = {
    status: "running",
    activity: "working",
    resident: resident({ phase: "working" }),
  };
  // Nothing listed and nothing counted: the strip above the composer already
  // says the turn is in flight, so the panel would be the same sentence twice.
  assert.equal(backgroundPanel({ row, subtasks: [] }), null);

  const panel = backgroundPanel({ row, subtasks: liveSubtasks([task("u1", "explore")]) });
  assert.equal(panel?.kind, "working");
  assert.equal(panel?.count, 1);
});

test("a working turn whose children cannot be named still gets the count", () => {
  // The gap this closes: `background` took the larger of the resident's count
  // and the visible list, `working` took the list alone — so a turn in flight
  // whose children were all launched before the loaded window drew no panel at
  // all, and the resident's count appeared nowhere on the screen. The bare
  // count is the only thing there saying the turn is minding anything.
  const row: ActivityRow = {
    status: "running",
    activity: "working",
    resident: resident({ phase: "working", background: 2 }),
  };
  const panel = backgroundPanel({ row, subtasks: [] });
  assert.equal(panel?.kind, "working");
  assert.equal(panel?.count, 2);
  assert.deepEqual(panel?.subtasks, [], "the panel component draws the count-only row for this");
});

test("a working turn names more children than the resident counts, if it can", () => {
  // The same floor rule the background branch has always used, now on both:
  // listing three rows under the headline "2 running" is the one combination
  // that reads as a bug.
  const row: ActivityRow = {
    status: "running",
    activity: "working",
    resident: resident({ phase: "working", background: 1 }),
  };
  const panel = backgroundPanel({
    row,
    subtasks: liveSubtasks([task("u1", "explore"), task("u2", "test")]),
  });
  assert.equal(panel?.count, 2);
});

test("an idle session gets no panel, however much unfinished history it holds", () => {
  const row: ActivityRow = { status: "idle", activity: "idle", resident: resident() };
  assert.equal(backgroundPanel({ row, subtasks: liveSubtasks([task("u1", "explore")]) }), null);
});

test("a codex run with no resident is still working, so its children show", () => {
  // Codex has no resident at all; the injection record is the only signal.
  const row: ActivityRow = { status: "running", activity: "idle" };
  const panel = backgroundPanel({
    row,
    subtasks: liveSubtasks([task("u1", "explore")]),
    injectionRunning: true,
  });
  assert.equal(panel?.kind, "working");
});

/* ── a process that is gone takes its children with it ──────────────────── */

/**
 * The session this section exists for.
 *
 * Two five-minute sentinels launched asynchronously, each answered by a
 * receipt and never again — the shape that has no closing signal in the
 * transcript at all. Their CLI exited at 13:42, taking both with it, and an
 * hour later the panel was still drawing them as running with no Stop beside
 * either, because a row the transcript named alone carries no `task_id`.
 */
function sentinels(): TranscriptItem[] {
  return [
    task("u1", "watch the deploy"),
    item({ kind: "tool_result", resultOf: "u1", text: AGENT_ACK }),
    task("u2", "watch the queue"),
    item({ kind: "tool_result", resultOf: "u2", text: AGENT_ACK }),
  ];
}

/** A claude session in this instance's own root, with no process behind it. */
function orphaned(over: Partial<DrivenRow> = {}): DrivenRow {
  return {
    // What the row said an hour after the CLI died: the transcript's last
    // decisive entry is a call nobody answered, so the heuristic still reads
    // it as a turn in flight.
    status: "running",
    activity: "working",
    source: "claude",
    scope: "owned",
    claudeRoot: "managed",
    resident: null,
    ...over,
  };
}

function backgroundTask(over: Partial<BackgroundTask> & Pick<BackgroundTask, "id">): BackgroundTask {
  return { toolUseId: null, description: null, taskType: null, startedAt: null, ...over };
}

test("a managed session this server holds no process for is running nothing", () => {
  // The whole fix in one assertion: background children belong to the CLI that
  // launched them, so a session this server would be driving and is not has
  // none, whatever the transcript's unanswered launches suggest.
  const row = orphaned();
  assert.equal(residentAbsenceIsFinal(row), true);
  assert.equal(liveSubtasks(sentinels()).length, 2, "the transcript alone still believes them");
  assert.deepEqual(panelRows({ resident: null, subtasks: liveSubtasks(sentinels()), row }), []);
  assert.equal(
    backgroundPanel({ row, subtasks: liveSubtasks(sentinels()) }),
    null,
    "no rows, no count, and no empty card floating over the transcript either",
  );
});

test("an injection record left saying running does not resurrect them", () => {
  // How the dead rows reached the screen at all: with no resident, `working`
  // can only come from the row's own status or from a run record nobody
  // closed — and neither is a process.
  const row = orphaned();
  assert.equal(backgroundPanel({ row, subtasks: liveSubtasks(sentinels()), injectionRunning: true }), null);
});

test("another instance's session is not judged by our missing process", () => {
  // An external row is somebody else's to drive. Its resident is invisible
  // from here, so a null one is the absence of news, not the end of the work.
  const row = orphaned({ scope: "external" });
  assert.equal(residentAbsenceIsFinal(row), false);
  assert.equal(backgroundPanel({ row, subtasks: liveSubtasks(sentinels()) })?.count, 2);
});

test("a session in the user's own root is not judged by it either", () => {
  // `~/.claude` is where a terminal claude writes. The session may be ours to
  // resume — adopted, or started on the local login — and still be driven
  // right now by a process nobody here spawned.
  const row = orphaned({ claudeRoot: "local" });
  assert.equal(residentAbsenceIsFinal(row), false);
  assert.equal(backgroundPanel({ row, subtasks: liveSubtasks(sentinels()) })?.count, 2);
});

test("codex is never read this way: it has no resident to be missing", () => {
  // One child process per message and nothing held between them, so a null
  // resident is codex's *normal* state and says nothing about what is out.
  const row = orphaned({ source: "codex", claudeRoot: undefined, activity: "idle", status: "running" });
  assert.equal(residentAbsenceIsFinal(row), false);
  const panel = backgroundPanel({ row, subtasks: liveSubtasks(sentinels()), injectionRunning: true });
  assert.equal(panel?.kind, "working");
  assert.equal(panel?.count, 2);
});

test("a server too old to say whose session it is keeps the old behaviour", () => {
  // `scope` and `claudeRoot` are equalities against values today's server
  // sends. One version behind sends neither, and the honest answer there is
  // "no idea", which has to read as the old behaviour rather than as proof.
  for (const row of [orphaned({ scope: undefined }), orphaned({ claudeRoot: undefined }), orphaned({ source: undefined })]) {
    assert.equal(residentAbsenceIsFinal(row), false);
    assert.equal(backgroundPanel({ row, subtasks: liveSubtasks(sentinels()) })?.count, 2);
  }
});

test("a resident is still the authority, and its rows still carry a Stop", () => {
  // The discipline this must not break: where there *is* a process, its task
  // list decides. It names the first sentinel and not the second, which is the
  // one case the transcript cannot settle on its own.
  const row = orphaned({
    status: "idle",
    activity: "background",
    resident: resident({
      background: 1,
      tasks: [backgroundTask({ id: "t1", toolUseId: "u1", startedAt: "2026-09-03T13:40:00.000Z" })],
    }),
  });
  assert.equal(residentAbsenceIsFinal(row), false);
  const panel = backgroundPanel({ row, subtasks: liveSubtasks(sentinels()) });
  assert.equal(panel?.kind, "background");
  assert.deepEqual(panel?.subtasks.map((s) => s.text), ["watch the deploy"]);
  assert.equal(panel?.subtasks[0]?.taskId, "t1", "a row the resident named can still be stopped");
});

test("a resident that counts background work and names none of it is believed", () => {
  // The older-server path `panelRows` already had: `background` shipped before
  // `tasks` did, so a count with an empty list is a server one version behind
  // rather than a fan-out that finished. The new rule sits in front of it and
  // must not swallow it — there is a resident here, so it never applies.
  const row = orphaned({ status: "idle", activity: "background", resident: resident({ background: 3 }) });
  const panel = backgroundPanel({ row, subtasks: liveSubtasks(sentinels()) });
  assert.equal(panel?.count, 3);
  assert.deepEqual(panel?.subtasks.map((s) => s.id), ["u1", "u2"]);
});

test("the badge goes quiet with the process, for the same reason", () => {
  // The badge reads the resident's own count, which is the number that dies
  // with the process — nothing a transcript says can inflate it.
  const dead: ActivityRow = { status: "idle", activity: "idle", resident: null };
  assert.deepEqual(activityBadge(dead), { kind: "idle" }, "no bg ×2 in the sidebar");
});

/* ── the queue, as a list rather than a number ──────────────────────────── */

test("a server too old to send the list leaves the chips empty, not undefined", () => {
  assert.deepEqual(queuedMessagesOf(null), []);
  assert.deepEqual(queuedMessagesOf({ ...resident(), queuedMessages: undefined as never }), []);
  const held = [{ id: "q1", text: "hi", steered: false }];
  assert.deepEqual(queuedMessagesOf(resident({ queued: 1, queuedMessages: held })), held);
});

/* ── draining the local queue ───────────────────────────────────────────── */

function drain(over: Partial<QueueDrainState> = {}): QueueDrainState {
  return {
    queued: 1,
    canSend: true,
    failed: false,
    sendFailed: false,
    sending: false,
    ...over,
  };
}

test("a queue that can go, goes — with no edge to wait for", () => {
  // The regression this replaces: the old effect fired only on `canSend`
  // going false→true, and a resident-backed session reports `canSend: true`
  // throughout, so the transition never arrived and the queue never moved.
  assert.equal(shouldDrainQueue(drain()), true);
});

test("an empty queue has nothing to drain", () => {
  assert.equal(shouldDrainQueue(drain({ queued: 0 })), false);
});

test("a send already in flight blocks the next one", () => {
  // Not politeness: `dispatchNext` shortens the queue synchronously, so a
  // second dispatch would put message #2 on the wire ahead of #1.
  assert.equal(shouldDrainQueue(drain({ sending: true })), false);
});

test("a failure pauses the queue until it is looked at", () => {
  assert.equal(shouldDrainQueue(drain({ failed: true })), false);
});

test("an intentional interrupt does not pause the correction queued behind it", () => {
  assert.equal(injectionFailurePausesQueue({ state: "failed", interrupted: true }), false);
  assert.equal(injectionFailurePausesQueue({ state: "failed", interrupted: false }), true);
  assert.equal(injectionFailurePausesQueue({ state: "running", interrupted: true }), false);
});

test("the local Codex queue can steer its head into a running session", () => {
  const head = {
    source: "codex" as const,
    canSend: false,
    injectionRunning: true,
    sending: false,
    steering: false,
  };
  assert.equal(localQueueHeadAction(head), "steer");
  assert.equal(localQueueHeadAction({ ...head, steering: true }), null, "one click stays in flight until the turn stops");
  assert.equal(localQueueHeadAction({ ...head, injectionRunning: false }), null, "a stale busy row cannot stop nothing");
});

test("an idle queue sends normally and Claude never uses the local steer", () => {
  const head = {
    source: "codex" as const,
    canSend: true,
    injectionRunning: false,
    sending: false,
    steering: false,
  };
  assert.equal(localQueueHeadAction(head), "send");
  assert.equal(localQueueHeadAction({ ...head, source: "claude", canSend: false, injectionRunning: true }), null);
  assert.equal(localQueueHeadAction({ ...head, sending: true }), null);
});

test("a POST that failed does not get retried in a loop", () => {
  // The hazard level-triggering introduces, and the reason `sendFailed` is a
  // field: a failed send puts its message back at the head of the queue, so a
  // drain that only asked "can it go?" would send it again one render later,
  // and again, as fast as the network allows. ▶ is what resumes it.
  assert.equal(shouldDrainQueue(drain({ sendFailed: true })), false);
});

test("a session that will not take a message is not sent one", () => {
  assert.equal(shouldDrainQueue(drain({ canSend: false })), false);
});

/* ── how long a child has been out ──────────────────────────────────────── */

test("elapsed time reads as a duration, never as `now`", () => {
  const at = Date.parse("2026-08-30T04:00:00Z");
  const after = (ms: number) => formatElapsed("2026-08-30T04:00:00Z", at + ms);
  assert.equal(after(20_000), "<1m", "`now` would read as a status, not a duration");
  assert.equal(after(12 * 60_000), "12m");
  assert.equal(after(60 * 60_000), "1h");
  assert.equal(after(80 * 60_000), "1h 20m");
  assert.equal(after(51 * 60 * 60_000), "2d 3h");
  assert.equal(formatElapsed(null), "");
  assert.equal(formatElapsed("not a date"), "");
});
