import assert from "node:assert/strict";
import test from "node:test";
import type { components } from "../src/api/schema.js";
import {
  activityBadge,
  activityBanner,
  activityOf,
  activitySortRank,
  asyncLaunchAckOf,
  backgroundPanel,
  bannerCanStop,
  latchOutcome,
  liveSubtasks,
  normalizeTaskStatus,
  panelRows,
  residentBusy,
  residentOf,
  runningTaskIds,
  subagentOutcome,
  turnProgress,
  willQueueSend,
  type ActivityRow,
  type BackgroundTask,
  type ResidentInfo,
} from "../src/features/sessions/session-activity.js";

type TranscriptItem = components["schemas"]["TranscriptItem"];

function resident(over: Partial<ResidentInfo> = {}): ResidentInfo {
  return {
    phase: "idle",
    queued: 0,
    queuedMessages: [],
    background: 0,
    tasks: [],
    awaitingBackground: false,
    stalled: false,
    ...over,
  };
}

function task(over: Partial<BackgroundTask> & Pick<BackgroundTask, "id">): BackgroundTask {
  return { toolUseId: null, description: null, taskType: null, startedAt: null, ...over };
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

/* ── the field, and what stands in for it ───────────────────────────────── */

test("the server's own answer is taken over the transcript heuristic", () => {
  // The whole point of the field: a resident that is between turns with a
  // message waiting is not idle, however finished its transcript looks.
  const row: ActivityRow = { status: "idle", activity: "queued", resident: resident({ queued: 1 }) };
  assert.equal(activityOf(row), "queued");
});

test("a server too old to send activity still reports a running turn", () => {
  // A row from a server one version behind has no `activity` at all. Falling
  // back to `idle` would take the sidebar backwards — quiet for a live turn —
  // so the fallback is the same rule the server uses with no driver to ask.
  assert.equal(activityOf({ status: "running" } as ActivityRow), "working");
  assert.equal(activityOf({ status: "waiting" } as ActivityRow), "idle");
  assert.equal(activityOf({ status: "idle" } as ActivityRow), "idle");
});

test("a missing resident reads as no resident rather than as an exception", () => {
  assert.equal(residentOf({ status: "idle" } as ActivityRow), null);
  assert.equal(residentOf({ status: "idle", resident: null }), null);
  assert.equal(residentOf({ status: "idle", resident: resident() })?.phase, "idle");
});

test("a starting resident counts as busy, because that is where a cold CLI sits", () => {
  assert.equal(residentBusy(resident({ phase: "starting" })), true);
  assert.equal(residentBusy(resident({ phase: "working" })), true);
  assert.equal(residentBusy(resident({ phase: "idle", queued: 3 })), false);
  assert.equal(residentBusy(null), false);
});

/* ── the sidebar badge ──────────────────────────────────────────────────── */

test("the badge has one case per thing a session can be doing", () => {
  assert.deepEqual(
    activityBadge({ status: "running", activity: "working", resident: resident({ phase: "working" }) }),
    { kind: "working", queued: 0 },
  );
  assert.deepEqual(
    activityBadge({ status: "running", activity: "queued", resident: resident({ queued: 2 }) }),
    { kind: "queued", queued: 2 },
  );
  assert.deepEqual(
    activityBadge({ status: "idle", activity: "background", resident: resident({ background: 3 }) }),
    { kind: "background", background: 3 },
  );
  assert.deepEqual(activityBadge({ status: "idle", activity: "idle" }), { kind: "idle" });
});

test("a working session still shows what is stacked up behind the turn", () => {
  // The case the composer was silent about: a message accepted with
  // `{"queued":true}` while a turn ran had nowhere at all to appear.
  const badge = activityBadge({
    status: "running",
    activity: "working",
    resident: resident({ phase: "working", queued: 1 }),
  });
  assert.deepEqual(badge, { kind: "working", queued: 1 });
});

test("a finished turn is idle; only a question asks", () => {
  // `waiting` only says a live process ended its turn, and a resident keeps
  // every finished session's process alive — so it is no reason to draw
  // anything. `asking` is the server's narrower verdict: a question nobody has
  // answered.
  assert.deepEqual(activityBadge({ status: "waiting", activity: "idle" }), { kind: "idle" });
  assert.deepEqual(activityBadge({ status: "waiting", activity: "idle", asking: true }), { kind: "asking" });
  assert.deepEqual(activityBadge({ status: "waiting", activity: "idle", asking: false }), { kind: "idle" });
  assert.deepEqual(
    activityBadge({ status: "waiting", activity: "working", resident: resident({ phase: "working" }) }),
    { kind: "working", queued: 0 },
  );
});

/* ── the feed sort ──────────────────────────────────────────────────────── */

test("work and queued messages sort above everything else, and background does not", () => {
  assert.equal(activitySortRank({ status: "running", activity: "working" }), 0);
  assert.equal(activitySortRank({ status: "idle", activity: "queued" }), 0);
  // Background work needs no answer from the user; promoting it over unread
  // output would bury the row that does.
  assert.equal(activitySortRank({ status: "idle", activity: "background" }), 1);
  assert.equal(activitySortRank({ status: "idle", activity: "idle" }), 1);
});

/* ── the composer banner ────────────────────────────────────────────────── */

test("either the resident or the injection record saying running is running", () => {
  // Two CLIs, two mechanisms: claude has a resident that knows about
  // `starting`, and codex has only the injection record.
  assert.deepEqual(
    activityBanner({ status: "idle", activity: "idle", resident: resident({ phase: "starting" }) }, false),
    { kind: "working", queued: 0, stalled: false },
  );
  assert.deepEqual(activityBanner({ status: "idle", activity: "idle" }, true), {
    kind: "working",
    queued: 0,
    stalled: false,
  });
});

test("nothing happening draws no banner at all", () => {
  assert.equal(activityBanner({ status: "idle", activity: "idle" }, false), null);
  assert.equal(activityBanner({ status: "waiting", activity: "idle" }, false), null);
});

test("a parked settings change is carried onto the banner, not swallowed", () => {
  // Without this the session reads as idle with something queued, which is
  // indistinguishable from a message that was simply dropped.
  assert.deepEqual(
    activityBanner(
      {
        status: "running",
        activity: "queued",
        resident: resident({ queued: 1, background: 2, awaitingBackground: true }),
      },
      false,
    ),
    { kind: "queued", queued: 1, awaitingBackground: true, background: 2 },
  );
});

test("background work draws no banner: the panel over the transcript owns it", () => {
  // The strip used to say "2 background tasks still running" directly under a
  // floating panel listing those same two by name — one screen, one fact, two
  // shapes. The list is the one that says which two, so it is the one kept.
  assert.equal(
    activityBanner({ status: "idle", activity: "background", resident: resident({ background: 2 }) }, false),
    null,
  );
});

test("only a turn in flight is offered a stop button", () => {
  // `queued` is reached with `phase: "idle"`, and the interrupt route answers
  // 409 for an idle resident: a button whose only outcome is an error is worse
  // than no button.
  assert.equal(bannerCanStop({ kind: "working", queued: 0, stalled: false }), true);
  assert.equal(bannerCanStop({ kind: "queued", queued: 1, awaitingBackground: false, background: 0 }), false);
  assert.equal(bannerCanStop(null), false);
});

/* ── what the send button promises ──────────────────────────────────────── */

test("a send lands in a queue whenever anything is already ahead of it", () => {
  const idle: ActivityRow = { status: "idle", activity: "idle", resident: resident() };
  assert.equal(
    willQueueSend({ canSend: true, localQueue: 0, row: idle, injectionRunning: false }),
    false,
    "nothing in the way: the button may say send",
  );
  assert.equal(willQueueSend({ canSend: false, localQueue: 0, row: idle, injectionRunning: false }), true);
  assert.equal(willQueueSend({ canSend: true, localQueue: 1, row: idle, injectionRunning: false }), true);
  assert.equal(willQueueSend({ canSend: true, localQueue: 0, row: idle, injectionRunning: true }), true);
});

test("a queue the server is holding counts even though this tab cannot see it", () => {
  // The bug this closes: the server accepted a message with `{"queued":true}`
  // and the composer went on offering "send", promising an immediate turn it
  // was in no position to give.
  const row: ActivityRow = { status: "running", activity: "queued", resident: resident({ queued: 1 }) };
  assert.equal(willQueueSend({ canSend: true, localQueue: 0, row, injectionRunning: false }), true);

  const working: ActivityRow = {
    status: "running",
    activity: "working",
    resident: resident({ phase: "working" }),
  };
  assert.equal(willQueueSend({ canSend: true, localQueue: 0, row: working, injectionRunning: false }), true);
});

/* ── naming the child that is still running ─────────────────────────────── */

test("the newest unanswered subagent call is what the banner names", () => {
  const items = [
    item({ kind: "tool_use", toolName: "Task", agent: "a1", toolUseId: "u1", text: "explore the repo" }),
    item({ kind: "tool_result", resultOf: "u1", text: "done" }),
    item({ kind: "tool_use", toolName: "Task", agent: "a2", toolUseId: "u2", text: "write the tests" }),
  ];
  assert.deepEqual(liveSubtasks(items).at(-1) ?? null, {
    id: "u2",
    text: "write the tests",
    meta: null,
    agentType: null,
    background: false,
    acked: false,
    startedAt: null,
    detail: null,
  });
});

test("a subagent that has already reported back is not still running", () => {
  const items = [
    item({ kind: "tool_use", toolName: "Task", agent: "a1", toolUseId: "u1", text: "explore the repo" }),
    item({ kind: "tool_result", resultOf: "u1", text: "done" }),
  ];
  assert.equal(liveSubtasks(items).at(-1) ?? null, null);
});

test("a receipt for an async launch is not the child reporting back", () => {
  // The counterpart to the test above, and the distinction the strip got wrong:
  // "done" is an answer, "launched successfully" is a receipt handed over about
  // two seconds in, while the child is still out.
  const items = [
    item({ kind: "tool_use", toolName: "Task", agent: "a1", toolUseId: "u1", text: "explore the repo" }),
    item({ kind: "tool_result", resultOf: "u1", text: "Async agent launched successfully with ID: agent_01H9" }),
  ];
  assert.equal((liveSubtasks(items).at(-1) ?? null)?.id, "u1");
});

test("a run_in_background launch is child work too", () => {
  // It is exactly what the resident counts as background, so the line that
  // explains a background banner has to be able to name it.
  const items = [
    item({ kind: "tool_use", toolName: "Bash", background: true, toolUseId: "u1", text: "pnpm build", meta: "watch" }),
  ];
  assert.deepEqual(liveSubtasks(items).at(-1) ?? null, {
    id: "u1",
    text: "pnpm build",
    meta: "watch",
    // A background Bash is not a subagent, so its caption is not an agent type.
    agentType: null,
    background: true,
    acked: false,
    startedAt: null,
    detail: null,
  });
});

test("a subagent too far back to still be running is not claimed to be", () => {
  // An interrupted subagent never gets a result, so it looks unfinished for the
  // rest of the session. Naming it hours later would be a confident lie.
  const stale = item({ kind: "tool_use", toolName: "Task", agent: "a1", toolUseId: "u1", text: "long dead" });
  const filler = Array.from({ length: 90 }, () => item({ kind: "text", text: "…" }));
  assert.equal(liveSubtasks([stale, ...filler]).at(-1) ?? null, null);
  // Just inside the window it is still believable.
  assert.deepEqual(liveSubtasks([stale, ...filler.slice(0, 70)]).at(-1) ?? null, {
    id: "u1",
    text: "long dead",
    meta: null,
    agentType: null,
    background: false,
    acked: false,
    startedAt: null,
    detail: null,
  });
});

test("ordinary tool calls are never mistaken for child work", () => {
  const items = [
    item({ kind: "tool_use", toolName: "Read", toolUseId: "u1", text: "src/main.rs" }),
    item({ kind: "text", role: "assistant", text: "here you go" }),
  ];
  assert.equal(liveSubtasks(items).at(-1) ?? null, null);
  assert.equal(liveSubtasks([]).at(-1) ?? null, null);
});

/* ── the resident's list and the transcript's, as one list ──────────────── */

/** The launch as the transcript records it, and the receipt that follows it. */
function launched(over: Partial<TranscriptItem> & Pick<TranscriptItem, "toolUseId">): TranscriptItem[] {
  return [
    item({ kind: "tool_use", toolName: "Task", agent: "a1", ...over }),
    item({
      kind: "tool_result",
      resultOf: over.toolUseId ?? undefined,
      text: "Async agent launched successfully with ID: agent_01H9",
    }),
  ];
}

test("a task and the launch that started it are one row, not two", () => {
  // The join, and the reason for it: the resident knows the child is alive and
  // the transcript knows what it was handed, so a row built from one of them
  // alone is either anonymous or a guess about liveness.
  const items = launched({
    toolUseId: "u1",
    text: "survey the repository",
    meta: "general-purpose · opus",
    detail: "read every file under src/ and report what it does",
    timestamp: "2026-08-31T10:00:00.000Z",
  });
  const row: ActivityRow = {
    status: "idle",
    activity: "background",
    resident: resident({
      background: 1,
      tasks: [
        task({
          id: "a8725c02",
          toolUseId: "u1",
          description: "survey the repository",
          taskType: "local_agent",
          startedAt: "2026-08-31T10:00:02.000Z",
        }),
      ],
    }),
  };
  const panel = backgroundPanel({ row, subtasks: liveSubtasks(items) });
  assert.equal(panel?.count, 1, "one child, described twice, is still one child");
  assert.deepEqual(panel?.subtasks, [
    {
      id: "u1",
      // Two ids, and the row needs both: `id` is the call that launched it,
      // and `taskId` is the only name a stop request can be addressed to.
      taskId: "a8725c02",
      text: "survey the repository",
      meta: "general-purpose · opus",
      agentType: "general-purpose",
      background: false,
      acked: true,
      // The transcript saw the launch itself; the resident only saw the line
      // about it, a beat later.
      startedAt: "2026-08-31T10:00:00.000Z",
      detail: "read every file under src/ and report what it does",
    },
  ]);
});

test("the resident's description and clock fill what the transcript entry left empty", () => {
  const items = launched({ toolUseId: "u1", text: "", timestamp: null });
  const rows = panelRows({
    resident: resident({
      background: 1,
      tasks: [task({ id: "t1", toolUseId: "u1", description: "run the migration", startedAt: "2026-08-31T10:00:02.000Z" })],
    }),
    subtasks: liveSubtasks(items),
  });
  assert.equal(rows.length, 1);
  assert.equal(rows[0]?.text, "run the migration");
  assert.equal(rows[0]?.startedAt, "2026-08-31T10:00:02.000Z", "an approximate age beats none at all");
});

test("a child launched outside the loaded window is named rather than counted", () => {
  // The gap this whole change exists for: the count said "2 background tasks"
  // and the window could name neither, so the panel drew a number.
  const row: ActivityRow = {
    status: "idle",
    activity: "background",
    resident: resident({
      background: 2,
      tasks: [
        task({
          id: "buli8h4uy",
          toolUseId: "u9",
          description: "Fetch PR 6-9 titles and bodies",
          taskType: "local_bash",
          startedAt: "2026-08-31T09:00:00.000Z",
        }),
        // Only ever seen in a snapshot: no launching id, and nothing said about
        // what it is. Still a row — it is running.
        task({ id: "ghost", taskType: "local_agent", startedAt: "2026-08-31T09:30:00.000Z" }),
      ],
    }),
  };
  const panel = backgroundPanel({ row, subtasks: [] });
  assert.deepEqual(panel?.subtasks, [
    {
      id: "u9",
      taskId: "buli8h4uy",
      text: "Fetch PR 6-9 titles and bodies",
      meta: null,
      agentType: null,
      // A `local_bash` is the shell command the CLI is minding, which is what
      // the panel chips as background rather than as a subagent.
      background: true,
      acked: true,
      startedAt: "2026-08-31T09:00:00.000Z",
      detail: null,
    },
    {
      id: null,
      // No launching call was ever seen, but the CLI's own id was — so this
      // row cannot be joined to the transcript and *can* still be stopped.
      taskId: "ghost",
      // Nothing was ever said about it, so it is listed by the only name it
      // has. A row with an unhelpful name still says something is running.
      text: "ghost",
      meta: null,
      agentType: null,
      background: false,
      acked: true,
      startedAt: "2026-08-31T09:30:00.000Z",
      detail: null,
    },
  ]);
});

test("an acked launch the resident does not list has finished", () => {
  // The stale row the lookback window used to carry for eighty steps. The
  // receipt is not a report back, so nothing in the transcript ever closes it —
  // but a live resident that does not name it has settled the question.
  const items = launched({ toolUseId: "u1", text: "explore the repo" });
  assert.equal(liveSubtasks(items).length, 1, "the transcript alone still believes it");
  assert.deepEqual(panelRows({ resident: resident({ background: 0 }), subtasks: liveSubtasks(items) }), []);
});

test("with no resident to ask, an acked launch is still believed", () => {
  // An external session, or a CLI nobody here is driving: there is no live
  // truth to prefer, so the transcript's account stands — bounded, as it always
  // was, by how far back a launch can be believed at all.
  const items = launched({ toolUseId: "u1", text: "explore the repo" });
  assert.equal(panelRows({ resident: null, subtasks: liveSubtasks(items) }).length, 1);
  const filler = Array.from({ length: 90 }, () => item({ kind: "text", text: "…" }));
  assert.deepEqual(panelRows({ resident: null, subtasks: liveSubtasks([...items, ...filler]) }), []);
});

test("a foreground subagent stays listed, because the resident never names one", () => {
  // `task_started` is emitted for async launches only, so a foreground Agent
  // call is *expected* to be missing from the list — reading that absence as
  // "finished" would blank the panel over a turn that is waiting on a child.
  const items = [
    item({ kind: "tool_use", toolName: "Task", agent: "a1", toolUseId: "u1", text: "review the diff" }),
  ];
  const rows = panelRows({
    resident: resident({ phase: "working", background: 1, tasks: [task({ id: "t1", toolUseId: "u2" })] }),
    subtasks: liveSubtasks(items),
  });
  assert.equal(rows.length, 2, "the resident's task and the foreground child are both live");
  assert.ok(rows.some((subtask) => subtask.id === "u1" && !subtask.acked));
});

test("a resident too old to send its task list does not retire anything", () => {
  // `background` shipped before `tasks` did. A server one version behind sends
  // a count and no list, and treating that silence as live truth would empty
  // the panel over a real fan-out.
  const items = launched({ toolUseId: "u1", text: "explore the repo" });
  const rows = panelRows({ resident: resident({ background: 3, tasks: [] }), subtasks: liveSubtasks(items) });
  assert.equal(rows.length, 1, "a claim of three tasks with none named is an older server, not an empty list");
});

test("the panel reads oldest first, whichever source named each row", () => {
  const items = launched({ toolUseId: "u1", text: "the newer child", timestamp: "2026-08-31T11:00:00.000Z" });
  const rows = panelRows({
    resident: resident({
      background: 2,
      tasks: [
        task({ id: "t1", toolUseId: "u1", startedAt: "2026-08-31T11:00:01.000Z" }),
        task({ id: "t2", description: "the older child", startedAt: "2026-08-31T09:00:00.000Z" }),
      ],
    }),
    subtasks: liveSubtasks(items),
  });
  assert.deepEqual(
    rows.map((subtask) => subtask.text),
    ["the older child", "the newer child"],
    "the one that has been out longest is the one worth reading first",
  );
});

test("the count still outranks the rows, because the list is capped and it is not", () => {
  const row: ActivityRow = {
    status: "idle",
    activity: "background",
    resident: resident({ background: 120, tasks: [task({ id: "t1", description: "one of many" })] }),
  };
  const panel = backgroundPanel({ row, subtasks: [] });
  assert.equal(panel?.count, 120);
  assert.equal(panel?.subtasks.length, 1);
});

/* ── what a running turn has to show for itself ─────────────────────────── */

test("the turn is measured from the injection's own clock", () => {
  // `startedAt` says when the *user's message* went in, which is earlier than
  // anything in the transcript and is the number the user is asking for.
  const items = [
    item({ kind: "text", role: "user", text: "go", timestamp: "2026-08-30T04:00:00Z" }),
    item({ kind: "tool_use", toolName: "Read", timestamp: "2026-08-30T04:00:10Z" }),
    item({ kind: "tool_result", role: "user", timestamp: "2026-08-30T04:00:12Z" }),
  ];
  const progress = turnProgress(items, "2026-08-30T04:00:00Z");
  assert.deepEqual(progress, {
    messages: 3,
    startedAt: "2026-08-30T04:00:00Z",
    lastActivityAt: "2026-08-30T04:00:12Z",
  });
});

test("everything before the turn began is left out of the count", () => {
  const items = [
    item({ kind: "text", role: "user", text: "old", timestamp: "2026-08-30T03:00:00Z" }),
    item({ kind: "text", role: "assistant", text: "old answer", timestamp: "2026-08-30T03:00:05Z" }),
    item({ kind: "text", role: "user", text: "new", timestamp: "2026-08-30T04:00:00Z" }),
    item({ kind: "tool_use", toolName: "Bash", timestamp: "2026-08-30T04:00:03Z" }),
  ];
  assert.equal(turnProgress(items, "2026-08-30T04:00:00Z")?.messages, 2);
});

test("a turn that has written nothing has no activity of its own to report", () => {
  // The re-attach shape, and the row it used to print. `startedAt` is the
  // moment this server took the session over — session `83193b4a`, 2026-09-07,
  // where the previous turn's last line was half an hour older than the clock
  // the bar was counting from. Reporting that line as *this* turn's last
  // activity is what put "0 messages · last activity 83m ago · 52m" on one row,
  // three numbers that cannot all be true at once.
  const items = [
    item({ kind: "text", role: "user", text: "old", timestamp: "2026-09-07T06:17:28Z" }),
    item({ kind: "text", role: "assistant", text: "old answer", timestamp: "2026-09-07T06:21:45Z" }),
  ];
  const progress = turnProgress(items, "2026-09-07T06:52:16Z");
  assert.equal(progress?.messages, 0);
  assert.equal(progress?.startedAt, "2026-09-07T06:52:16Z");
  assert.equal(progress?.lastActivityAt, null, "the previous turn's last line is not this turn's");
});

test("a message that has produced nothing yet is still a turn", () => {
  // Zero written and three seconds elapsed is exactly the honest answer here;
  // returning null would put the wordless spinner back.
  const items = [item({ kind: "text", role: "user", text: "old", timestamp: "2026-08-30T03:00:00Z" })];
  const progress = turnProgress(items, "2026-08-30T04:00:00Z");
  assert.equal(progress?.messages, 0);
  assert.equal(progress?.startedAt, "2026-08-30T04:00:00Z");
});

test("with no injection clock the turn starts at the last thing the user said", () => {
  // A server too old to send `startedAt`, or a turn this instance did not
  // start: the boundary a reader would use is the boundary used here.
  const items = [
    item({ kind: "text", role: "user", text: "old", timestamp: "2026-08-30T03:00:00Z" }),
    item({ kind: "text", role: "user", text: "new", timestamp: "2026-08-30T04:00:00Z" }),
    item({ kind: "thinking", timestamp: "2026-08-30T04:00:02Z" }),
  ];
  const progress = turnProgress(items, null);
  assert.equal(progress?.messages, 2);
  assert.equal(progress?.startedAt, "2026-08-30T04:00:00Z");
});

test("nothing to date the turn by is answered with nothing, not with a guess", () => {
  assert.equal(turnProgress([], null), null);
  assert.equal(turnProgress([item({ kind: "tool_use", toolName: "Read" })], null), null);
});

test("an unparseable injection clock falls back rather than printing NaN", () => {
  const items = [item({ kind: "text", role: "user", text: "go", timestamp: "2026-08-30T04:00:00Z" })];
  assert.equal(turnProgress(items, "not-a-date")?.startedAt, "2026-08-30T04:00:00Z");
});

/* ── launch receipts, in words ──────────────────────────────────────────── */

test("a launch receipt is recognised by which kind of child it acknowledges", () => {
  // A fifth of all results in real history are this boilerplate, and every one
  // of them spent its line on internal metadata.
  assert.equal(
    asyncLaunchAckOf(
      item({
        kind: "tool_result",
        role: "user",
        text: "Async agent launched successfully. (This tool result is internal metadata…",
      }),
    ),
    "agent",
  );
  assert.equal(
    asyncLaunchAckOf(item({ kind: "tool_result", role: "user", text: "Command running in background (id: 3)" })),
    "command",
  );
  assert.equal(asyncLaunchAckOf(item({ kind: "tool_result", role: "user", text: "no output" })), null);
});

test("a launch that failed is not a receipt, whatever it says", () => {
  assert.equal(
    asyncLaunchAckOf(
      item({ kind: "tool_result", role: "user", text: "Async agent launched…", isError: true }),
    ),
    null,
  );
});

/* ── how a subagent card ends up ────────────────────────────────────────── */

const receipt = item({
  kind: "tool_result",
  role: "user",
  text: "Async agent launched successfully. (This tool result is internal metadata…",
});

test("the CLI's own word for the outcome outranks everything else", () => {
  // Including a resident that still lists the task: the list is republished on
  // the CLI's schedule, so it can name a child whose report has already landed.
  assert.equal(subagentOutcome({ taskStatus: "completed", residentRunning: true, live: true }), "completed");
  assert.equal(subagentOutcome({ taskStatus: "failed", result: receipt }), "failed");
  assert.equal(subagentOutcome({ taskStatus: "stopped", live: true }), "stopped");
});

test("a word this client has never seen is not read as success", () => {
  // Passed through the wire as a bare string on purpose. An unknown value falls
  // back to the other sources rather than being forced into one of the three.
  assert.equal(normalizeTaskStatus("throttled"), null);
  assert.equal(subagentOutcome({ taskStatus: "throttled", live: true }), "running");
  assert.equal(subagentOutcome({ taskStatus: "throttled" }), "unknown");
});

test("a foreground subagent is answered by its own call", () => {
  const done = item({ kind: "tool_result", role: "user", text: "reviewed 4 files" });
  const failed = item({ kind: "tool_result", role: "user", text: "agent crashed", isError: true });
  assert.equal(subagentOutcome({ result: done, live: true }), "completed");
  assert.equal(subagentOutcome({ result: failed, live: true }), "failed");
});

test("a launch receipt is not an answer, so the card keeps looking", () => {
  // The whole failure this exists for: an async child is answered about two
  // seconds after it goes out, and reading that as "finished" ticks off every
  // agent in a fan-out at the moment it starts.
  assert.equal(subagentOutcome({ result: receipt, residentRunning: true, live: true }), "running");
  assert.equal(subagentOutcome({ result: receipt, live: true }), "unknown");
});

test("an unanswered call is running only while the session is", () => {
  assert.equal(subagentOutcome({ live: true }), "running");
  // Outside a turn the same call is one that never came back. A spinner there
  // promises an answer that is not coming.
  assert.equal(subagentOutcome({ live: false }), "unknown");
});

test("a resident minding the child is enough on its own", () => {
  assert.equal(subagentOutcome({ result: receipt, residentRunning: true }), "running");
});

test("a verdict is a fact about the past and does not expire", () => {
  // Every source that could say "this finished" goes quiet: the resident drops
  // the task within seconds, and the notification pages out of the window as
  // history loads around it. A card that recomputed freely would go from ✓ back
  // to ⊘ for work that is demonstrably over.
  assert.equal(latchOutcome("completed", "unknown"), "completed");
  assert.equal(latchOutcome("failed", "running"), "failed");
  assert.equal(latchOutcome("stopped", "completed"), "stopped");
});

test("running and unknown are readings of the present, and are replaced", () => {
  assert.equal(latchOutcome("running", "completed"), "completed");
  assert.equal(latchOutcome("unknown", "completed"), "completed", "a late notification still counts");
  assert.equal(latchOutcome("unknown", "running"), "running");
  assert.equal(latchOutcome(null, "running"), "running");
});

test("the ids the resident is minding come out as a set a card can ask", () => {
  const info = resident({
    background: 2,
    tasks: [task({ id: "a", toolUseId: "u1" }), task({ id: "b" }), task({ id: "c", toolUseId: "u2" })],
  });
  assert.deepEqual([...runningTaskIds(info)].sort(), ["u1", "u2"]);
  assert.equal(runningTaskIds(null).size, 0, "no resident is not evidence of anything");
});

/* ── what the CLI's progress stream adds to a panel row ─────────────────── */

test("progress fields ride onto the row that names the child", () => {
  const row: ActivityRow = {
    status: "idle",
    activity: "background",
    resident: resident({
      background: 1,
      tasks: [
        task({
          id: "t1",
          toolUseId: "u1",
          description: "Scout the codebase",
          taskType: "local_agent",
          subagentType: "Explore",
          lastToolName: "Bash",
          totalTokens: 10_240,
          toolUses: 17,
          durationMs: 12 * 60_000,
        }),
      ],
    }),
  };
  const rows = panelRows({
    resident: residentOf(row),
    subtasks: [
      {
        id: "u1",
        text: "scout the codebase",
        meta: "Explore · opus",
        agentType: "Explore",
        background: false,
        acked: true,
        startedAt: "2026-08-31T09:00:00.000Z",
        detail: "read everything",
      },
    ],
  });
  assert.equal(rows.length, 1);
  assert.equal(rows[0]?.lastToolName, "Bash");
  assert.equal(rows[0]?.totalTokens, 10_240);
  assert.equal(rows[0]?.toolUses, 17);
  assert.equal(rows[0]?.durationMs, 720_000);
  assert.equal(rows[0]?.detail, "read everything", "and the transcript's half of the row survives");
});

test("a task the CLI has published no progress for is unchanged by the join", () => {
  // Absent is not `null`: the server distinguishes them, and a row built from a
  // task with no progress line yet must come out exactly as it did before these
  // fields existed.
  const rows = panelRows({
    resident: resident({ background: 1, tasks: [task({ id: "t1", toolUseId: "u1", description: "go" })] }),
    subtasks: [],
  });
  assert.deepEqual(Object.keys(rows[0] ?? {}).sort(), [
    "acked",
    "agentType",
    "background",
    "detail",
    "id",
    "meta",
    "startedAt",
    // Always present on a row the resident named: it is the task's identity,
    // not a progress field the CLI may not have published yet.
    "taskId",
    "text",
  ]);
});
