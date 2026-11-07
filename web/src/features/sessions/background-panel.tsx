import { CaretRightIcon, CircleNotchIcon, SquareIcon } from "@phosphor-icons/react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { LiveDot } from "@/components/live-dot";
import {
  backgroundFold,
  type FoldStorage,
  oldestElapsedMs,
  readBackgroundPanelOpen,
  writeBackgroundPanelOpen,
} from "@/features/sessions/background-prefs";
import type { BackgroundPanel, LiveSubtask } from "@/features/sessions/session-activity";
import { cn } from "@/lib/cn";
import { formatCompact, formatElapsed, formatElapsedMs, formatInt } from "@/lib/format";

/** How often the "running 12m" durations are re-rendered. */
const TICK = 30_000;

/** One row of the list, shared by a plain child and an expandable one. */
const ROW = "flex items-baseline gap-1.5 px-2.5 py-1 font-mono text-2xs";

/**
 * The list of live children that floats over the transcript.
 *
 * # Why it floats
 *
 * Background work has no transcript. A subagent writes into its own file and a
 * `run_in_background` Bash writes nowhere until it finishes, so from the main
 * feed's point of view a session that fanned out to four agents looks exactly
 * like a session that stopped — the last thing in the window is the tool call
 * that launched them, minutes or hours ago. Scrolling up through history, which
 * is what you do while waiting, moves the composer's own strip off screen and
 * takes the only remaining evidence with it.
 *
 * So this is `sticky`: it rides the top of the scroll area, over the messages
 * rather than above them, and stays there wherever the history is scrolled to.
 * It is also the *only* place background work is drawn — the strip above the
 * composer says nothing about it, because a count there and a list here were
 * the same fact twice on one screen.
 *
 * # What it says
 *
 * One row per child, always visible, with no headline over them: "3 background
 * tasks" is the question, not the answer, and the answer is *which* three and
 * how long each has been out — the difference between "still going" and
 * "stuck".
 *
 * Each row is its own fold onto what that child was actually handed: the
 * subagent's prompt, or the command being minded. The one-line description is
 * written by the same turn that wrote the prompt, so when it is vague or wrong
 * the prompt is the only thing that settles what is really running. Folded shut
 * by default — a prompt is many lines, and four of them expanded would bury the
 * transcript this thing floats over. A child whose launch recorded no expanded
 * content gets a plain row with no triangle, rather than one that opens onto
 * nothing.
 *
 * The single case with no list: children launched before the loaded transcript
 * window, where the resident's count is all that survives. Then the count is
 * drawn *instead of* rows, never above them.
 *
 * # When it folds
 *
 * The argument above holds for three children and breaks for a dozen. A lead
 * session that fanned out to fifteen agents draws fifteen rows of monospace
 * across the top of a phone, over the transcript it exists to annotate, and the
 * cap that stops it eating the whole screen only turns it into a scroll region
 * you have to scroll past. So past [`BACKGROUND_FOLD_THRESHOLD`] the list folds
 * to its own summary — the count that would be drawn anyway if none of the
 * children could be named, plus how long the oldest has been out, which is the
 * one number the hidden rows were carrying that a count cannot replace.
 *
 * Short lists never grow a fold control: they are already the size the fold is
 * for, and a headline over three rows would cost a line to say what the rows
 * underneath already say.
 *
 * # Stopping one of them
 *
 * The panel is also the only place a single child can be *ended*. The Stop
 * above the composer is a different verb — it ends the turn, and claude's
 * `interrupt` deliberately leaves a backgrounded shell running — so a
 * `run_in_background` command that will never finish had, until this button,
 * no way out short of restarting the session and losing everything else it was
 * minding. The CLI's own `stop_task` names one task, which is why the button
 * lives on the row rather than anywhere a whole session is described.
 *
 * Only rows the resident named carry one. A row the transcript named alone has
 * no `task_id` to address a request to, and a Stop that could only fail is the
 * UI promising something it cannot do.
 *
 * Purely presentational. Every decision — whether it appears at all, which
 * children count as live, what each one is called — is made by
 * [`backgroundPanel`] and [`liveSubtasks`] in `session-activity.ts` and tested
 * there.
 */
export function BackgroundTaskPanel({
  panel,
  /** Fixed clock for tests; real renders read the wall clock. */
  now,
  onStop,
  stopping,
}: {
  panel: BackgroundPanel | null;
  now?: number;
  /**
   * Ask claude to end one child, by its [`LiveSubtask.taskId`].
   *
   * Absent where no session can be asked — a codex row, a feed with no
   * resident — and then no row draws a Stop at all.
   */
  onStop?: (subtask: LiveSubtask) => void;
  /** The `taskId`s with a stop already in flight. */
  stopping?: ReadonlySet<string>;
}) {
  const { t } = useTranslation();
  const [, setTick] = useState(0);
  // The stored answer to "may this thing take my screen", read once at mount.
  // Held as state rather than read per render so the fold responds to the tap
  // that wrote it; null means the row count decides, which is why toggling is
  // the only thing that ever sets it.
  const [stored, setStored] = useState<boolean | null>(() => {
    const storage = foldStorage();
    return storage ? readBackgroundPanelOpen(storage) : null;
  });
  // Durations that never move read as a frozen UI, and a session minding
  // background work produces no other render for minutes at a time. Still true
  // folded: the summary carries the oldest child's stopwatch.
  const ticking = panel !== null && panel.subtasks.length > 0 && now === undefined;
  useEffect(() => {
    if (!ticking) return;
    const timer = setInterval(() => setTick((n) => n + 1), TICK);
    return () => clearInterval(timer);
  }, [ticking]);

  if (!panel) return null;

  const { foldable, open } = backgroundFold(panel.subtasks.length, stored);
  const toggle = () => {
    const next = !open;
    setStored(next);
    const storage = foldStorage();
    if (storage) writeBackgroundPanelOpen(storage, next);
  };

  return (
    // `sticky` keeps the card's own space in the flow and pins it once the
    // scroll passes it, so the transcript slides *under* it — which is the
    // whole effect. The padding is on the wrapper and made click-through, so
    // the gap either side of the card does not swallow clicks on the messages
    // scrolling behind it.
    <div className="pointer-events-none sticky top-0 z-20 px-2 py-2">
      <div className="pointer-events-auto overflow-hidden border border-border-strong bg-bg">
        {foldable && (
          // The summary is the fold control rather than sitting beside one: on
          // a phone the whole line is the target, and a 12px chevron is not.
          <button
            type="button"
            onClick={toggle}
            aria-expanded={open}
            title={open ? t("hide the background task list") : t("show the background task list")}
            className={cn(
              ROW,
              "w-full cursor-pointer text-left text-accent hover:bg-hover",
              open && "border-b border-border",
            )}
          >
            <PanelSummary count={panel.count} subtasks={panel.subtasks} now={now} />
            <CaretRightIcon
              className={cn(
                "size-3 shrink-0 self-center text-faint transition-transform",
                open && "rotate-90",
              )}
            />
          </button>
        )}
        {panel.subtasks.length === 0 ? (
          <div className={cn(ROW, "text-accent")}>
            <PanelSummary count={panel.count} subtasks={panel.subtasks} now={now} />
          </div>
        ) : (
          open && (
            // Capped and scrollable: a fan-out of ten, or one open prompt, must
            // not push the transcript off the screen it is floating over. The
            // cap is on the list rather than the card so that the summary above
            // it — the one line that says work is still out — cannot be scrolled
            // out of a folded-open panel.
            <div className="max-h-64 overflow-y-auto">
              {panel.subtasks.map((subtask, i) => (
                // Position is the fallback key because a transcript entry can
                // reach us with no tool_use id, and two of those must still be
                // two rows.
                <SubtaskRow
                  key={subtask.id ?? `at:${i}`}
                  subtask={subtask}
                  now={now}
                  onStop={onStop}
                  stopping={stopping}
                />
              ))}
            </div>
          )
        )}
      </div>
    </div>
  );
}

/**
 * The one line that stands for the whole list: the count, and the oldest child's
 * stopwatch.
 *
 * Drawn in two places that are the same sentence about the same fact — over a
 * folded list, and *instead of* a list that could not be named — so they are one
 * component. Deliberately the same wording in both: a panel that folds shut must
 * not look like a different feature from one that never had rows to show.
 */
function PanelSummary({
  count,
  subtasks,
  now,
}: {
  count: number;
  subtasks: readonly LiveSubtask[];
  now?: number;
}) {
  const { t } = useTranslation();
  const oldest = oldestElapsedMs(subtasks, now ?? Date.now());
  return (
    <>
      {/* The colour background work is drawn in everywhere else in this UI, so
          the panel and the sidebar dot are recognisably the same fact. */}
      <LiveDot className="bg-muted" />
      <span className="min-w-0 flex-1 truncate">
        {t("{{n}} background tasks running", { n: count })}
      </span>
      {/* Folding hides every per-row stopwatch, and the count that survives
          cannot tell a fan-out launched a minute ago from one wedged since
          lunch. This is the number that can. */}
      {oldest !== null && (
        <span className="shrink-0 tabular-nums text-faint/80" title={t("the longest-running task")}>
          {t("oldest {{d}}", { d: formatElapsedMs(oldest) })}
        </span>
      )}
    </>
  );
}

/**
 * localStorage, or null where there is none.
 *
 * This component is rendered by `react-dom/server` in the tests and could be
 * server-rendered in principle, and a missing global there must be a default
 * fold rather than a crash.
 */
function foldStorage(): FoldStorage | null {
  return typeof localStorage === "undefined" ? null : localStorage;
}

function SubtaskRow({
  subtask,
  now,
  onStop,
  stopping,
}: {
  subtask: LiveSubtask;
  now?: number;
  onStop?: (subtask: LiveSubtask) => void;
  stopping?: ReadonlySet<string>;
}) {
  const { t } = useTranslation();
  // The CLI's own clock first. `startedAt` is when *this server* first heard of
  // the task, so after a restart every child claims to have begun at the
  // restart; `durationMs` has been counting since the child actually began.
  const elapsed =
    typeof subtask.durationMs === "number"
      ? formatElapsedMs(subtask.durationMs)
      : formatElapsed(subtask.startedAt, now);
  // `subagentType` is the CLI naming its own child; `agentType` is this client
  // splitting a caption in half. Where both exist the first one is right.
  const type = subtask.subagentType ?? subtask.agentType;
  const line = (
    <>
      <span className="shrink-0 text-faint">↳</span>
      {/* A `run_in_background` launch has no agent type — it is a shell command
          the CLI is minding — so it is labelled by what it is instead. */}
      <span
        className={cn(
          "shrink-0 rounded px-1",
          subtask.background ? "bg-warn/15 text-warn-fg" : "text-fg shadow-[inset_0_0_0_1px_var(--color-border-strong)]",
        )}
      >
        {subtask.background ? t("bg") : (type ?? t("subagent"))}
      </span>
      <span className="min-w-0 flex-1 truncate text-muted">{subtask.text}</span>
      {/* What it is doing *now*, which a description written at launch cannot
          say. The stopwatch beside it only ever answered "how long" — this is
          the difference between a child that is working and one that is wedged. */}
      {subtask.lastToolName && (
        <span className="shrink-0 text-faint/80" title={t("the tool it is running now")}>
          ↳ {subtask.lastToolName}
        </span>
      )}
      {typeof subtask.toolUses === "number" && subtask.toolUses > 0 && (
        <span className="shrink-0 tabular-nums text-faint/70" title={t("tools run so far")}>
          ×{subtask.toolUses}
        </span>
      )}
      {typeof subtask.totalTokens === "number" && subtask.totalTokens > 0 && (
        <span
          className="shrink-0 tabular-nums text-faint/70"
          title={t("{{n}} tokens spent so far", { n: formatInt(subtask.totalTokens) })}
        >
          {formatCompact(subtask.totalTokens)}
        </span>
      )}
      {elapsed && (
        <span className="shrink-0 tabular-nums text-faint/80">{t("running {{d}}", { d: elapsed })}</span>
      )}
      {onStop && subtask.taskId !== undefined && (
        <button
          type="button"
          title={t("stop this background task")}
          aria-label={t("stop this background task")}
          disabled={stopping?.has(subtask.taskId)}
          onClick={(event) => {
            // The row is a `summary` in the expandable case, where a click is
            // the browser's own toggle: without this, stopping a task would
            // also unfold the prompt of the row that is about to disappear.
            event.preventDefault();
            event.stopPropagation();
            onStop(subtask);
          }}
          className="-my-0.5 shrink-0 self-center rounded p-1 text-faint hover:bg-danger/15 hover:text-danger disabled:opacity-50"
        >
          {stopping?.has(subtask.taskId) ? (
            <CircleNotchIcon className="size-3 animate-spin" />
          ) : (
            <SquareIcon className="size-2.5 fill-current" />
          )}
        </button>
      )}
    </>
  );
  // A hairline between children, not around the outside: the card's own border
  // already draws that, and the first row must not double it.
  const divider = "border-t border-border first:border-t-0";

  if (!subtask.detail) {
    return (
      <div className={cn(ROW, divider)} title={subtask.meta ?? undefined}>
        {line}
      </div>
    );
  }
  return (
    // `details` rather than a state hook, which is what every other fold in the
    // transcript uses: the browser owns the open/closed bit, so the panel
    // re-renders on the socket's schedule — every 30s at least, for the
    // durations — without ever losing what the user opened.
    <details className={cn("group/task", divider)}>
      <summary
        title={subtask.meta ?? t("show what this task is running")}
        className={cn(ROW, "cursor-pointer list-none hover:bg-hover")}
      >
        {line}
        <CaretRightIcon className="size-3 shrink-0 self-center text-faint transition-transform group-open/task:rotate-90" />
      </summary>
      {/* The transcript's own `DetailPre`, in the panel's smaller box: the
          prompt is the thing being quoted, so it is drawn where the rest of
          this UI draws quoted tool input. */}
      <pre className="mx-2.5 mt-0.5 mb-1.5 max-h-48 overflow-y-auto rounded bg-raised p-2 font-mono text-2xs leading-4 whitespace-pre-wrap text-muted">
        {subtask.detail}
      </pre>
    </details>
  );
}
