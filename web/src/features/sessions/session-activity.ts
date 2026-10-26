import type { components } from "@/api/schema";

type SessionSummary = components["schemas"]["SessionSummary"];
type TranscriptItem = components["schemas"]["TranscriptItem"];

/** What a session is doing, as the server's driver reports it. */
export type SessionActivity = components["schemas"]["SessionActivity"];
/** This server's live process for a session, when it holds one. */
export type ResidentInfo = NonNullable<components["schemas"]["ResidentInfo"]>;
/** One message the resident is holding, addressable by the two queue routes. */
export type QueuedMessage = components["schemas"]["QueuedMessage"];
/** One live child the resident is minding, joinable to a transcript call. */
export type BackgroundTask = components["schemas"]["BackgroundTask"];

/**
 * The shape everything here actually reads.
 *
 * Narrower than a whole `SessionSummary` so the badge, the banner and the sort
 * can be exercised from a four-field literal instead of a twenty-field one, and
 * so a caller that only has a state frame can still ask.
 */
export type ActivityRow = Pick<SessionSummary, "status"> &
  Partial<Pick<SessionSummary, "activity" | "resident" | "asking">>;

/**
 * A row's activity, falling back for a server that predates the field.
 *
 * The fallback is deliberately the same rule the server uses with no driver to
 * ask (`SessionActivity::from_status`): `running` is work and everything else,
 * `waiting` included, is not. Without it an older server would report every
 * session as idle and the sidebar would go quiet for a running turn — worse
 * than the bug this whole field exists to fix.
 */
export function activityOf(row: ActivityRow): SessionActivity {
  // Typed as required by the schema, but a server one version behind simply
  // does not send it, so the runtime answer really can be undefined.
  const activity = row.activity as SessionActivity | undefined;
  if (activity) return activity;
  return row.status === "running" ? "working" : "idle";
}

/** This server's resident for the row, or null — including on older servers. */
export function residentOf(row: ActivityRow): ResidentInfo | null {
  return (row.resident as ResidentInfo | null | undefined) ?? null;
}

/**
 * The queue as a list, for a server old enough to have only sent the count.
 *
 * `queued` and `queuedMessages` are the same fact at two resolutions and the
 * server keeps them equal, but only the count existed first — so a client that
 * indexed straight into the array would render an empty queue over a session
 * that has three messages waiting, which is the exact lie the count was added
 * to stop telling. The count still drives the headline; this drives the chips.
 */
export function queuedMessagesOf(resident: ResidentInfo | null): QueuedMessage[] {
  return (resident?.queuedMessages as QueuedMessage[] | undefined) ?? [];
}

/**
 * The background work the resident names, as a list — empty on older servers.
 *
 * Same shape of caution as `queuedMessagesOf`, and the same reason: `background`
 * is the count and this is the list behind it, but only the count existed first.
 * A client that read the missing array as "no tasks" would conclude that a
 * server one version behind is minding nothing — and [`panelRows`] treats this
 * list as live truth about what has finished, so believing an absent one would
 * retire every child on screen.
 */
export function backgroundTasksOf(resident: ResidentInfo | null): BackgroundTask[] {
  return (resident?.tasks as BackgroundTask[] | undefined) ?? [];
}

/**
 * Whether the resident has a turn in flight.
 *
 * `starting` counts: the window between a send being accepted and the CLI
 * saying it is listening is still the user's message being worked on, and
 * calling it idle is exactly the "task finished" lie the composer used to tell.
 */
export function residentBusy(resident: ResidentInfo | null): boolean {
  return resident?.phase === "working" || resident?.phase === "starting";
}

/**
 * A row seen from the process side: whose session it is, and whose CLI drives it.
 *
 * The three fields that decide whether *this server not holding a process* is
 * news about the session or news about this server — see
 * [`residentAbsenceIsFinal`], which is the only thing that reads them. All
 * optional, because a server one version behind may send none of them and the
 * answer then has to be "no idea", not "nothing is running".
 */
export type DrivenRow = ActivityRow & Partial<Pick<SessionSummary, "source" | "scope" | "claudeRoot">>;

/**
 * Whether having no resident *proves* this session has no live CLI behind it.
 *
 * A background child belongs to the process that launched it. claude's task
 * registry lives inside the CLI, and when that process goes its children go
 * with it — so where this server would be the one holding that process, a null
 * resident is not the absence of news. It is the end of every background task
 * the transcript ever recorded, however recently one was launched.
 *
 * The bug it exists for: two five-minute sentinels died with their CLI at
 * 13:42 and the panel was still drawing them as running an hour later, with no
 * Stop beside either — a row the transcript named alone carries no `task_id`,
 * so there was not even a button to disprove it with. Both the user and the
 * next agent read the screen and believed something was still out there.
 *
 * The proof holds *only* where the process would be ours, which is why the
 * resident being null is the smallest part of the test:
 *
 * * **claude.** codex has no resident at all — one child process per message,
 *   nothing to hold between them — so a null one says nothing about it. Its
 *   own liveness signal is the injection record, and this rule must not be
 *   read onto it.
 * * **owned.** An external session is another instance's, or another machine's.
 *   Its residents are invisible from here, and their absence from *this*
 *   server means only that this server did not start it.
 * * **managed.** The managed root belongs to this instance and nothing else can
 *   be in it, so a process for a session in it could only be one of ours. The
 *   local root is `~/.claude`, where the user's own terminal runs — and an
 *   adopted session is exactly that: ours to resume, but quite possibly being
 *   driven right now by a CLI nobody here spawned.
 *
 * Every test is an equality against a value the server sends today, so a
 * server too old for `scope` or `claudeRoot` answers `false` and keeps the old
 * behaviour. That asymmetry is the point: being wrong here costs a row that
 * lingers, and being wrong the other way hides a child that is still running,
 * which is the failure the panel exists to prevent.
 */
export function residentAbsenceIsFinal(row: DrivenRow): boolean {
  if (residentOf(row) !== null) return false;
  return row.source === "claude" && row.scope === "owned" && row.claudeRoot === "managed";
}

/** What the sidebar badge draws. One case per thing a row can be doing. */
export type ActivityBadge =
  /** A turn is in flight; `queued` is what is stacked up behind it. */
  | { kind: "working"; queued: number }
  /** No turn in flight, but messages are waiting to go in. */
  | { kind: "queued"; queued: number }
  /** Nothing in flight, nothing queued, but background tasks are still going. */
  | { kind: "background"; background: number }
  /** The agent stopped on a question it cannot go on without an answer to. */
  | { kind: "asking" }
  | { kind: "idle" };

/**
 * What one session's badge should say.
 *
 * `activity` decides, not `status`: the two agree about work being in flight
 * and disagree about everything else, and this is the one that can tell a
 * queued message from an idle session.
 *
 * `asking` comes first, because it is the one state that is waiting on the
 * reader. It is the server's call, not `status === "waiting"`: a resident
 * keeps every finished session's process alive, so `waiting` is true of nearly
 * all of them and says only that a turn ended — which is idle, and draws
 * nothing.
 *
 * And it has to come before `working`, not merely beside it: a resident claude
 * asks *mid-turn* — the CLI blocks the turn on `AskUserQuestion` and the
 * server holds the question open — so the row says `activity: "working"` and
 * `asking: true` at once for as long as the question is waiting. Asked the
 * other way round, every question this server holds would read as an
 * ordinary busy session, which is exactly the one that needs nobody.
 */
export function activityBadge(row: ActivityRow): ActivityBadge {
  if (row.asking === true) return { kind: "asking" };
  const resident = residentOf(row);
  switch (activityOf(row)) {
    case "working":
      return { kind: "working", queued: resident?.queued ?? 0 };
    case "queued":
      return { kind: "queued", queued: resident?.queued ?? 0 };
    case "background":
      return { kind: "background", background: resident?.background ?? 0 };
    default:
      return { kind: "idle" };
  }
}

/**
 * Whether a row belongs at the top of a feed sorted by "is this moving".
 *
 * Lower sorts first. Background work is deliberately *not* promoted: it needs
 * no answer from the user and floating it over unread output would bury the
 * thing that does.
 */
export function activitySortRank(row: ActivityRow): number {
  const activity = activityOf(row);
  return activity === "working" || activity === "queued" ? 0 : 1;
}

/**
 * What the composer draws above itself, or null when there is nothing to say.
 *
 * Only the two states that are about *the user's message*: it is being answered,
 * or it is waiting its turn. Background work is deliberately not one of them —
 * it is listed child by child in the panel floating over the transcript, and a
 * count above the composer was that same fact a second time on the same screen.
 */
export type ActivityBanner =
  | { kind: "working"; queued: number; stalled: boolean }
  | {
      kind: "queued";
      queued: number;
      /** The queue head needs a new process and is parked behind background work. */
      awaitingBackground: boolean;
      background: number;
    };

/**
 * What is happening to the session the composer is attached to.
 *
 * Takes the injection state as well as the row because the two cover different
 * CLIs: claude answers through a resident, and codex is still one child process
 * per message with no resident to ask. Either saying "running" is running.
 */
export function activityBanner(row: ActivityRow, injectionRunning: boolean): ActivityBanner | null {
  const resident = residentOf(row);
  const activity =
    residentBusy(resident) || injectionRunning ? "working" : activityOf(row);
  switch (activity) {
    case "working":
      return { kind: "working", queued: resident?.queued ?? 0, stalled: resident?.stalled ?? false };
    case "queued":
      return {
        kind: "queued",
        queued: resident?.queued ?? 0,
        awaitingBackground: resident?.awaitingBackground ?? false,
        background: resident?.background ?? 0,
      };
    // `background` falls through to null on purpose: the transcript panel names
    // every live child, so a strip here could only be an abbreviation of the
    // list already on screen.
    default:
      return null;
  }
}

/**
 * Whether a Stop button belongs beside a banner.
 *
 * Only `working` has a turn to stop. `queued` is reached with the resident at
 * `phase: "idle"`, and `InjectionRegistry::interrupt_resident` refuses an idle
 * resident — the route answers `409`. A button whose only outcome is an error
 * is worse than no button.
 */
export function bannerCanStop(banner: ActivityBanner | null): boolean {
  return banner?.kind === "working";
}

/**
 * Whether pressing send will append to a queue rather than start a turn.
 *
 * The local queue is only half of it: a message the server accepted with
 * `{"queued":true}` is waiting inside the resident where this client cannot see
 * it, and a composer that says "send" for it promises something it cannot keep.
 */
export function willQueueSend(input: {
  /** The feed's own verdict, which already refuses a busy session. */
  canSend: boolean;
  /** How many messages this tab is holding back. */
  localQueue: number;
  row: ActivityRow;
  injectionRunning: boolean;
}): boolean {
  if (!input.canSend || input.localQueue > 0) return true;
  const resident = residentOf(input.row);
  return (
    residentBusy(resident) || input.injectionRunning || (resident?.queued ?? 0) > 0
  );
}

/**
 * What a turn in flight has to show for itself so far.
 *
 * The one thing a spinner cannot say is whether anything is *happening*. These
 * three numbers are the ones a person actually asks for — how long has this been
 * going, how much has it written, and when did it last write anything — and they
 * are deliberately not a fraction: nothing here or on the server knows how many
 * steps a turn will take, so a progress bar could only be a guess dressed up as
 * a measurement.
 */
export interface TurnProgress {
  /** Items the transcript gained since the turn began, the opening one included. */
  messages: number;
  /** When the turn began: the injection's own clock, else its first item's. */
  startedAt: string | null;
  /**
   * The newest timestamp *inside this turn*, which is when it last did anything.
   *
   * Null when the turn has written nothing yet, and pointedly not the newest
   * timestamp in the transcript: that one belongs to whatever ran before, and
   * reporting it here is how the bar came to say "0 messages · last activity
   * 83m ago · 52m" — a turn that started half an hour *after* the activity it
   * claimed as its own. Three numbers that cannot all be true are worse than
   * two that are, so the one that is not this turn's is left out.
   */
  lastActivityAt: string | null;
}

/**
 * Measure the turn that is currently running.
 *
 * `startedAt` comes from the injection record, which is the only source that
 * knows when *the user's message* went in rather than when the transcript
 * happened to gain a line. It has been on the wire the whole time and nothing
 * read it. When it is missing — an older server, or a turn this instance did
 * not start — the fallback is the first item of the current turn, found by
 * walking back to the last thing the user said, which is the same boundary a
 * reader would use.
 *
 * Returns `null` when neither source can place the turn, so the caller can fall
 * back to plain wording instead of printing a duration measured from nothing.
 */
export function turnProgress(
  items: readonly TranscriptItem[],
  startedAt: string | null | undefined,
): TurnProgress | null {
  const started = startedAt ? Date.parse(startedAt) : Number.NaN;
  let from = -1;
  if (!Number.isNaN(started)) {
    for (let i = 0; i < items.length; i++) {
      const at = items[i]!.timestamp;
      if (at && Date.parse(at) >= started) {
        from = i;
        break;
      }
    }
    // Nothing has been written since the message went in: still a turn, and
    // "0 messages · 3s" is exactly the honest thing to say about it.
    if (from < 0) from = items.length;
  } else {
    for (let i = items.length - 1; i >= 0; i--) {
      const item = items[i]!;
      if (item.role === "user" && item.kind === "text") {
        from = i;
        break;
      }
    }
    if (from < 0) return null;
  }

  // Only as far back as the turn's own first item. Walking the whole transcript
  // finds a timestamp for a turn that has produced nothing, and that timestamp
  // is the *previous* turn's last line — which is how a re-attached turn came to
  // report "last activity" from before it started.
  let lastActivityAt: string | null = null;
  for (let i = items.length - 1; i >= from; i--) {
    const at = items[i]!.timestamp;
    if (at && !Number.isNaN(Date.parse(at))) {
      lastActivityAt = at;
      break;
    }
  }
  const fallbackStart = items[from]?.timestamp ?? null;
  const resolvedStart = Number.isNaN(started) ? fallbackStart : (startedAt ?? fallbackStart);
  if (!resolvedStart && lastActivityAt === null) return null;
  return { messages: items.length - from, startedAt: resolvedStart, lastActivityAt };
}

/** A child of this session that has not reported back yet. */
export interface LiveSubtask {
  /**
   * The launching `tool_use`'s id, which is what makes one row addressable.
   *
   * Also the join key onto the resident's own list: `BackgroundTask.toolUseId`
   * is this same id, seen from the process rather than from the file it writes.
   *
   * Nullable because a transcript entry can reach us without one (an older
   * writer, a truncated line), and a resident task first heard of in a snapshot
   * carries none either. A child with no id is still a child that is running —
   * it just cannot be paired with anything, so it is listed by position instead.
   */
  id: string | null;
  /**
   * The CLI's own `task_id`, when a resident named this row.
   *
   * A second id, and a different one from [`LiveSubtask.id`] on purpose: `id`
   * is the `tool_use` that *launched* the child, which is what the transcript
   * knows it by, and this is what the CLI's own task registry calls it — which
   * is the only name a stop request can be addressed to.
   *
   * Absent for a row the transcript named alone: a foreground subagent, or a
   * launch the resident has not published a task for yet. Those rows are not
   * stoppable, and drawing them without a Stop is the honest report of that.
   */
  taskId?: string;
  /** The tool call's one-line summary — the Task description, usually. */
  text: string;
  /** Extra caption the CLI recorded, when it left one. */
  meta: string | null;
  /**
   * The subagent's type, pulled off the front of `meta`.
   *
   * The CLI writes `subagent_type · model` into one caption, and the type is
   * the half worth a badge: "Explore" says what the child is *for*, where the
   * model is a detail that would crowd out the description beside it.
   */
  agentType: string | null;
  /** A `run_in_background` launch rather than a subagent. */
  background: boolean;
  /**
   * Whether this child reports back through its own channel rather than here.
   *
   * True when the launching call was answered by a *receipt* — "Async agent
   * launched…", "Command running in background…" — which is the CLI saying the
   * child is out and this `tool_use` will never hear from it again. True by
   * construction for a row that came from the resident's task list, because
   * `task_started` fires for exactly that kind of launch.
   *
   * False means the launching call has simply not been answered yet, which is
   * what a *foreground* subagent looks like for the whole time it runs. The
   * distinction is what [`panelRows`] needs: the resident lists async children
   * and does not list foreground ones, so its silence means "finished" for the
   * first and nothing at all for the second.
   */
  acked: boolean;
  /**
   * When the child started, for "running 12m".
   *
   * The launching call's transcript timestamp where there is one, and otherwise
   * the resident's own first-seen clock (`BackgroundTask.startedAt`) — the two
   * are the same moment described by whichever source could see it.
   */
  startedAt: string | null;
  /**
   * What the child was actually handed: the Agent prompt, or the shell command.
   *
   * The one-line `text` is a *description* — "explore the repo" — written by
   * the same turn that wrote the prompt, and a wrong or vague one is exactly
   * when you want to know what really went out. Null where the transcript
   * recorded no expanded content, which the panel draws as a row with no fold
   * rather than an empty one.
   */
  detail: string | null;
  /**
   * What the CLI's own progress stream says about the child, when it says
   * anything.
   *
   * All five are optional rather than nullable, and that is the distinction
   * that matters: absent means the field never arrived — an older server, or a
   * task the CLI has not published progress for yet — and a row must then look
   * exactly as it did before these existed. Only `subagentType` overlaps with
   * anything the transcript knows ([`LiveSubtask.agentType`], parsed out of a
   * caption); it is the better answer where both are present, because it is the
   * CLI naming its own child rather than this client splitting a string.
   */
  subagentType?: string;
  /** The tool the child was last seen running — `Bash`, `Read`, `Edit`. */
  lastToolName?: string;
  /** Tokens spent so far, as the CLI counts them. */
  totalTokens?: number;
  /** How many tools it has run. */
  toolUses?: number;
  /**
   * The CLI's own duration, which beats `now - startedAt`.
   *
   * `startedAt` is when *this server* first heard of the task, so after a
   * restart it is the restart, and after attaching late it is the attach. The
   * CLI has been counting since the child actually began.
   */
  durationMs?: number;
}

/**
 * The progress fields, copied off a resident task onto the row that draws it.
 *
 * Absent keys are left absent rather than written as `null`, because the server
 * went out of its way to make that distinction: a task the CLI has published no
 * `task_progress` for yet omits these entirely, and a row built from one must
 * come out identical to a row built before the fields existed.
 */
function taskProgress(task: BackgroundTask): Partial<LiveSubtask> {
  const progress: Partial<LiveSubtask> = {};
  if (task.subagentType !== undefined) progress.subagentType = task.subagentType;
  if (task.lastToolName !== undefined) progress.lastToolName = task.lastToolName;
  if (task.totalTokens !== undefined) progress.totalTokens = task.totalTokens;
  if (task.toolUses !== undefined) progress.toolUses = task.toolUses;
  if (task.durationMs !== undefined) progress.durationMs = task.durationMs;
  return progress;
}

/**
 * How far back a still-running child could plausibly have been launched.
 *
 * Not a performance bound — a bound on being wrong. A subagent that was
 * interrupted, or whose parent turn died, never gets a `tool_result`, so it
 * looks unfinished for the rest of the session; without a limit the banner
 * would confidently name something that stopped hours ago. Roughly a turn's
 * worth of steps, which is as far as "still running" can be believed.
 */
const SUBTASK_LOOKBACK = 80;

/** `Explore · opus` → `Explore`; the model is a detail, the type is the label. */
function agentTypeOf(meta: string | null): string | null {
  const first = meta?.split("·", 1)[0]?.trim();
  return first ? first : null;
}

/**
 * How the CLI opens a `tool_result` that only *acknowledges* an async launch.
 *
 * An async launch is answered twice over: immediately, with a receipt saying
 * the child is out — and then never again on that id, because the child
 * reports through its own channel. Matched as a lower-cased prefix rather than
 * an equality because both receipts carry a tail (an agent id, a shell id) and
 * the server clips a result's `text` to its first 180 characters.
 */
const ASYNC_LAUNCH_ACKS = [
  { prefix: "async agent launched", kind: "agent" as const },
  { prefix: "command running in background", kind: "command" as const },
];

/** Which kind of child a launch receipt acknowledges, for the row's caption. */
export type AsyncLaunchAck = "agent" | "command";

/**
 * Whether a result is a launch receipt rather than a child reporting back.
 *
 * This is the whole of the bug it exists for. A background `Agent` and a
 * `Bash` with `run_in_background` both get a `tool_result` within about two
 * seconds of being launched, carrying the same `resultOf` as any real answer —
 * so a rule that reads "has a result" as "has finished" retires every
 * long-running child roughly the moment it starts, which is precisely the
 * child the panel exists to show. Measured against real transcripts before
 * this: a seven-agent fan-out listed none of them.
 *
 * Both fields are checked because the server derives a result's `text` from
 * the first line of its `detail` (`tool_display.rs`), and a transcript that
 * reaches us with only one of the two populated is still the same receipt.
 * An errored result is never a receipt: a launch that failed *is* the end of
 * that child, and leaving it listed would be a ghost row for something that
 * never ran.
 */
export function asyncLaunchAckOf(item: TranscriptItem): AsyncLaunchAck | null {
  if (item.isError) return null;
  for (const head of [item.text, item.detail]) {
    if (!head) continue;
    const start = head.trimStart().toLowerCase();
    const hit = ASYNC_LAUNCH_ACKS.find((ack) => start.startsWith(ack.prefix));
    if (hit) return hit.kind;
  }
  return null;
}

function isAsyncLaunchAck(item: TranscriptItem): boolean {
  return asyncLaunchAckOf(item) !== null;
}

/** How a task ended, once the CLI's own word for it is normalised. */
export type TaskStatus = "completed" | "failed" | "stopped";

/**
 * The CLI's `task_status`, reduced to the three outcomes a card can draw.
 *
 * Passed through the wire as a bare string on purpose — the server refuses to
 * guess at an enum for a value claude may add to tomorrow — so a word this
 * client has never seen answers `null` rather than being forced into one of the
 * three. `null` means "no verdict here", never "it worked".
 */
export function normalizeTaskStatus(status: string | null | undefined): TaskStatus | null {
  switch (status?.trim().toLowerCase()) {
    case "completed":
    case "complete":
    case "succeeded":
    case "success":
      return "completed";
    case "failed":
    case "failure":
    case "error":
      return "failed";
    case "stopped":
    case "cancelled":
    case "canceled":
    case "interrupted":
    case "killed":
      return "stopped";
    default:
      return null;
  }
}

/**
 * What a subagent card says about itself.
 *
 * `unknown` is the case this type exists for. A card has four sources and each
 * of them can be silent: an async launch is answered by a receipt that says
 * nothing about the work, the `⏱` notification that carries the real verdict
 * may be outside the loaded window (or may never have been written — a session
 * that died mid-task leaves none), and the resident's list only knows about
 * children *this* server is minding right now. With all four silent the honest
 * answer is that nobody knows, and the two dishonest ones are both worse: a
 * spinner promises an answer that is not coming, and a ✓ claims work that may
 * have crashed.
 */
export type AgentOutcome = "running" | "completed" | "failed" | "stopped" | "unknown";

/** The three outcomes that are final — see [`latchOutcome`]. */
const TERMINAL: ReadonlySet<AgentOutcome> = new Set<AgentOutcome>(["completed", "failed", "stopped"]);

/**
 * What state a subagent card is in, from every source that has an opinion.
 *
 * In precedence order, because the sources disagree by design:
 *
 * 1. **The notification's `taskStatus`.** The only source that saw the child
 *    finish. It outranks everything, including a resident that has not noticed
 *    yet — the task list is republished on the CLI's own schedule, so it can
 *    still name a child whose report has already landed.
 * 2. **The paired `tool_result`,** unless it is a launch receipt. For a
 *    *foreground* subagent this is the whole story: the call is answered when
 *    the child is done, and `isError` says how it went. For an async one the
 *    receipt arrives about two seconds in and means only "it is out there", so
 *    it is stepped over rather than read as an answer — reading it as one is
 *    what used to retire a seven-agent fan-out the moment it launched.
 * 3. **The resident's task list**, which is live truth about what is running
 *    now, and the only source that can say so for an async child.
 * 4. **A foreground call with no answer while the session works** — the shape
 *    of every subagent for the whole time it runs.
 *
 * Anything else is `unknown`.
 */
export function subagentOutcome(input: {
  /** The `tool_result` answering the launching call, when the window has it. */
  result?: TranscriptItem | null;
  /** `taskStatus` from the `⏱` notification that named this call. */
  taskStatus?: string | null;
  /** The resident lists this `tool_use` id among the children it is minding. */
  residentRunning?: boolean;
  /** The session has a turn in flight. */
  live?: boolean;
}): AgentOutcome {
  const status = normalizeTaskStatus(input.taskStatus);
  if (status) return status;
  const result = input.result ?? null;
  // An errored receipt is not a receipt: a launch that failed *is* the end of
  // that child, and `asyncLaunchAckOf` already refuses one.
  const receipt = result !== null && asyncLaunchAckOf(result) !== null;
  if (result && !receipt) return result.isError ? "failed" : "completed";
  if (input.residentRunning) return "running";
  if (!result && input.live) return "running";
  return "unknown";
}

/**
 * Hold the first terminal answer for ever.
 *
 * The sources a card reads go quiet in a fixed order — the resident drops a
 * finished task within seconds, the notification may be paged out of the window
 * as history loads around it — so a card that recomputed freely would show ✓
 * and then fall back to ⊘, or worse to a spinner, for work that is demonstrably
 * over. A verdict is a fact about the past and does not expire; `running` and
 * `unknown` are readings of the present and are replaced freely.
 */
export function latchOutcome(previous: AgentOutcome | null, next: AgentOutcome): AgentOutcome {
  if (previous !== null && TERMINAL.has(previous)) return previous;
  return next;
}

/**
 * The launching `tool_use` ids the resident is minding right now.
 *
 * The join key a card asks about, in the shape a card asks it in. Built once
 * per window rather than per row: `backgroundTasksOf` allocates, and a
 * thousand-row transcript would otherwise walk the task list a thousand times
 * on every state frame.
 */
export function runningTaskIds(resident: ResidentInfo | null): Set<string> {
  const ids = new Set<string>();
  for (const task of backgroundTasksOf(resident)) {
    if (task.toolUseId) ids.add(task.toolUseId);
  }
  return ids;
}

/**
 * Every child near the tail that has produced no result, oldest launch first.
 *
 * The cheapest possible answer to "the session says background, but doing
 * *what*": a `Task` call carries its own description, and one that has no
 * `tool_result` pointing back at it has not finished. Only the loaded window is
 * searched, so a subtask launched before the first loaded item reads as absent
 * rather than as finished — the panel it feeds is an extra line, never the
 * thing that decides whether the panel shows at all.
 *
 * All of them rather than the last one, because a session that fanned out to
 * four agents is exactly the session where "something is running" is useless:
 * the whole question is which four, and how long each has been out there.
 * Launch order, because that is the order they will report back in often
 * enough to be worth reading down.
 */
export function liveSubtasks(items: readonly TranscriptItem[]): LiveSubtask[] {
  const answered = new Set<string>();
  const acked = new Set<string>();
  for (const item of items) {
    if (item.kind !== "tool_result" || !item.resultOf) continue;
    // A launch receipt is not a report back, so it does not close the row.
    //
    // Which leaves acked launches with no closing signal at all, and that is a
    // deliberate choice rather than an oversight. The CLI *does* write
    // something when a background child finishes — a `<task-notification>`
    // entry, which the server renders as a visible `⏱` system item — but the
    // parser strips `<task-id>`, `<tool-use-id>` and `<agent-id>` out of it on
    // the way through, so it arrives naming no one. Pairing those to launches
    // by position instead would mean a child that finished second retiring the
    // row of the child that is still out: a live row vanishing, which is the
    // exact failure being fixed here, traded for a cosmetic one.
    //
    // Which leaves a receipt as the only signal that this child is one of the
    // ones the resident is minding — so it is recorded rather than dropped, and
    // [`panelRows`] uses the resident's list to retire the row exactly. Where
    // there is no resident to ask (an external session, a CLI nobody here is
    // driving) the bound on being wrong stays SUBTASK_LOOKBACK, as it already
    // is for an interrupted subagent that never reports at all: an acked launch
    // ages out of the window and stops being listed. A row that says "running"
    // for one turn too long is a smaller lie than a running child the panel
    // never mentions.
    if (isAsyncLaunchAck(item)) {
      acked.add(item.resultOf);
      continue;
    }
    answered.add(item.resultOf);
  }
  const floor = Math.max(0, items.length - SUBTASK_LOOKBACK);
  const live: LiveSubtask[] = [];
  for (let i = floor; i < items.length; i++) {
    const item = items[i]!;
    if (item.kind !== "tool_use") continue;
    // `agent` is a subagent drill-down; `background` is a `run_in_background`
    // launch. Both outlive the step that started them, and both are what the
    // resident is counting when it reports background work.
    if (!item.agent && !item.background) continue;
    if (item.toolUseId && answered.has(item.toolUseId)) continue;
    live.push({
      id: item.toolUseId ?? null,
      text: item.text,
      meta: item.meta,
      agentType: item.agent ? agentTypeOf(item.meta) : null,
      background: !!item.background,
      acked: !!item.toolUseId && acked.has(item.toolUseId),
      startedAt: item.timestamp,
      detail: item.detail,
    });
  }
  return live;
}

/**
 * The floating panel over the transcript, or null when it should not be there.
 *
 * Two states, and they are two different sentences. `background` is the one
 * with no other home: nothing is being written to the transcript, so a user
 * scrolling it sees a finished session, and the only thing that says otherwise
 * is this. `working` already has a strip above the composer saying the turn is
 * in flight, so the panel earns its space only by naming children the strip
 * cannot fit — which is why it stays away when a working turn has none.
 */
export interface BackgroundPanel {
  kind: "background" | "working";
  /**
   * How many children there are, which is only drawn when none can be listed.
   *
   * The resident's own count, which is uncapped where its task list is not, so
   * `count` can still exceed `subtasks.length` — and when nothing can be named
   * at all, this number standing alone is all that says the session is not done.
   */
  count: number;
  /** The children, named by [`panelRows`]; may be shorter than `count`. */
  subtasks: LiveSubtask[];
}

/**
 * The rows the panel draws: the resident's task list, named by the transcript.
 *
 * Two sources describing the same children, each holding half of what a row
 * needs. The resident is the *process*, so it alone knows what is still
 * running — a child that reports through its own channel leaves nothing in the
 * transcript when it finishes. The transcript is the *record*, so it alone
 * knows what the child was handed: the prompt, the command, the subagent type.
 * Reading either one on its own is a choice of which way to be wrong — a list
 * of ids with no names, or names that keep claiming to be running.
 *
 * So they are joined on the launching `tool_use` id, and three things fall out:
 *
 * 1. **Both** — the transcript's `detail` and `agentType` on the resident's
 *    verdict about liveness, with the resident's description and clock filling
 *    whatever the transcript entry left empty.
 * 2. **Resident only** — a child launched before the loaded window, or in a
 *    turn nobody has scrolled to. Previously invisible: it was counted and not
 *    named. Now it is a row called whatever claude called it.
 * 3. **Transcript only** — kept when there is no resident, because then the
 *    transcript is the only source there is. With a resident it depends on
 *    `acked`: a foreground subagent is *expected* to be missing from the task
 *    list (no `task_started` is ever emitted for one) and stays listed until
 *    its call is answered, while an acked async launch the resident does not
 *    name has finished, and is dropped.
 *
 * Case 3 is what retires the stale rows the lookback window used to carry: the
 * receipt for an async launch is not a report back, so nothing in the
 * transcript ever closes those rows, and before the resident's list arrived
 * they could only age out.
 */
export function panelRows(input: {
  resident: ResidentInfo | null;
  subtasks: readonly LiveSubtask[];
  /**
   * The row that resident came off, where the caller has one.
   *
   * Optional because the join itself does not need it: it is read only to tell
   * an absent resident that proves nothing from one that proves the work is
   * over — see [`residentAbsenceIsFinal`].
   */
  row?: DrivenRow;
}): LiveSubtask[] {
  const { resident } = input;
  // A process that is gone took its children with it. The transcript records
  // launches and never deaths, so once the resident that could retire these
  // rows exactly is gone, its account is all that is left and all of it is
  // stale. Dropped rather than drawn as finished: this list is what is running
  // *now*, and the launches are still in the transcript they were read out of.
  if (!resident && input.row && residentAbsenceIsFinal(input.row)) return [];
  const tasks = backgroundTasksOf(resident);
  // No resident, or a resident too old to send the list: the transcript is the
  // only account of these children, so every one of it is kept, bounded by
  // SUBTASK_LOOKBACK as it always was. A resident that claims background work
  // and names none of it is the older-server case specifically — the driver
  // publishes exactly as many tasks as it counts — and reading its silence as
  // "everything finished" there would empty a panel over a live fan-out.
  if (!resident || (tasks.length === 0 && resident.background > 0)) {
    return byStart([...input.subtasks]);
  }
  const byToolUse = new Map<string, LiveSubtask>();
  for (const subtask of input.subtasks) {
    if (subtask.id && !byToolUse.has(subtask.id)) byToolUse.set(subtask.id, subtask);
  }
  // By identity rather than by id: the rows put on the list are copies with the
  // resident's fields merged in, and what the second pass needs to skip is the
  // original it was copied from.
  const joined = new Set<LiveSubtask>();
  const rows: LiveSubtask[] = [];
  for (const task of tasks) {
    const match = task.toolUseId ? byToolUse.get(task.toolUseId) : undefined;
    if (match) {
      joined.add(match);
      rows.push({
        ...match,
        taskId: task.id,
        // The transcript's one-liner is the better label where it has one: it
        // is the description *and* the caption and the prompt behind it. The
        // resident's is the fallback, not the override.
        text: match.text || task.description || task.id,
        startedAt: match.startedAt ?? task.startedAt,
        ...taskProgress(task),
      });
      continue;
    }
    // Named by the resident alone. Deliberately plain: no `detail` to expand
    // onto, because the launch that would have carried the prompt is outside
    // the window, and a fold that opens onto nothing is worse than no fold.
    rows.push({
      id: task.toolUseId,
      taskId: task.id,
      text: task.description ?? task.id,
      meta: null,
      // `local_bash` is a shell command the CLI is minding, which the panel
      // chips as background; anything else is a subagent whose type this list
      // does not carry, so it gets the generic label rather than a guess.
      agentType: null,
      background: task.taskType === "local_bash",
      acked: true,
      startedAt: task.startedAt,
      detail: null,
      ...taskProgress(task),
    });
  }
  for (const subtask of input.subtasks) {
    if (joined.has(subtask)) continue;
    // The resident is live truth about async children and says nothing about
    // foreground ones, so only the first kind is retired by its silence.
    if (subtask.acked) continue;
    rows.push(subtask);
  }
  return byStart(rows);
}

/**
 * Oldest first, which is the order the panel reads down.
 *
 * The two sources are each already in their own launch order and interleaving
 * them by position would mean nothing, so the clock decides. Rows with no time
 * at all go last rather than first: an unknown age is not evidence of being the
 * oldest thing here, and they keep their relative order among themselves.
 */
function byStart(rows: LiveSubtask[]): LiveSubtask[] {
  return rows
    .map((row, at) => ({ row, at, started: row.startedAt ? Date.parse(row.startedAt) : Number.NaN }))
    .sort((a, b) => {
      const dated = Number(Number.isNaN(a.started)) - Number(Number.isNaN(b.started));
      if (dated !== 0) return dated;
      if (!Number.isNaN(a.started) && a.started !== b.started) return a.started - b.started;
      return a.at - b.at;
    })
    .map((entry) => entry.row);
}

export function backgroundPanel(input: {
  row: DrivenRow;
  subtasks: readonly LiveSubtask[];
  /** Codex has no resident, so its run record is the only "working" signal. */
  injectionRunning?: boolean;
}): BackgroundPanel | null {
  const resident = residentOf(input.row);
  // The same derivation `activityBanner` uses, on purpose: a panel and a strip
  // that disagree about whether a turn is in flight is a worse failure than
  // either being wrong on its own.
  const activity =
    residentBusy(resident) || input.injectionRunning ? "working" : activityOf(input.row);
  const subtasks = panelRows({ resident, subtasks: input.subtasks, row: input.row });
  // The resident's count is authoritative and the row list is a floor: the list
  // it publishes is capped and this number is not, and a session with no
  // resident has no number at all. The larger of the two is the one that is not
  // a lie. True of a working turn as much as an idle one — a turn that fanned
  // out and is now waiting on its children has the same children as a session
  // that has stopped, and reading only the transcript there drew no panel.
  const count = Math.max(resident?.background ?? 0, subtasks.length);
  // Nothing to name and nothing to count is not a panel. `background` cannot
  // be reached without a resident that says so, so this is only ever the
  // retirement above having emptied the list — and an empty card saying "0
  // background tasks running" would be the same lie in smaller type.
  if (count === 0) return null;
  if (activity === "background") {
    return { kind: "background", count, subtasks };
  }
  // A working turn with no children has nothing the strip above the composer
  // does not already say, which is what the guard above now covers for both
  // kinds. The condition that matters is "are there children", not "can this
  // window name them": with none nameable the panel falls back to the bare
  // count, which is the only thing on screen saying the turn is minding
  // anything.
  if (activity === "working") {
    return { kind: "working", count, subtasks };
  }
  return null;
}
