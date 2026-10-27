import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { SourceIcon } from "@/components/source-icon";
import { useParams } from "@tanstack/react-router";
import {
  ArrowDown,
  Check,
  ChevronLeft,
  ChevronRight,
  GitBranch,
  Copy,
  Info,
  Loader2,
  Square,
  X,
} from "lucide-react";
import {
  createContext,
  memo,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type RefObject,
  type ReactNode,
} from "react";
import { useTranslation } from "react-i18next";
import { ApiError, errorText, EXTERNAL_SESSION } from "@/api/errors";
import type { components } from "@/api/schema";
import { FileChip } from "@/components/file-chip";
import { LightboxImage } from "@/components/image-lightbox";
import { LiveDot } from "@/components/live-dot";
import { Markdown, MathRendering } from "@/components/markdown";
import { Badge } from "@/components/ui/badge";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Spinner } from "@/components/ui/spinner";
import { SessionHeader } from "@/features/sessions/session-header";
import { useShowingCopy } from "@/features/cloud/context";
import { CopyWindowNotice } from "@/features/cloud/plan";
import { SessionActionsMenu } from "@/features/sessions/session-actions-menu";
import {
  ChecklistChip,
  ComposerChoiceChips,
  ComposerQuota,
  ContextRing,
  ModelDriftBadge,
  useComposerChoice,
} from "@/features/sessions/composer-controls";
import { attachmentPaths, ComposerBox, useComposerAttachments, useSendHint } from "@/features/sessions/composer-box";
import {
  claimSessionDraft,
  readSessionDraftSnapshot,
  sessionDraftKey,
  subscribeSessionDraft,
  writeSessionDraft,
} from "@/features/sessions/session-draft";
import {
  scrollTopAfterPrepend,
  type TranscriptHistoryPage,
} from "@/features/sessions/session-feed-state";
import {
  createSessionFeedPool,
  type FeedConnection,
  type FeedState,
} from "@/features/sessions/session-feed-pool";
import {
  NO_UNREAD,
  pinAfterScroll,
  pinOnSettle,
  shouldTopUpHistory,
  showJumpToBottom,
  trackUnread,
  unreadLabel,
  type ScrollBox,
  type UnreadCursor,
} from "@/features/sessions/transcript-scroll";
import {
  activityBanner,
  asyncLaunchAckOf,
  backgroundPanel,
  bannerCanStop,
  latchOutcome,
  liveSubtasks,
  normalizeTaskStatus,
  residentAbsenceIsFinal,
  residentOf,
  runningTaskIds,
  subagentOutcome,
  turnProgress,
  willQueueSend,
  type ActivityBanner,
  type AgentOutcome,
  type LiveSubtask,
  type ResidentInfo,
  type TurnProgress,
} from "@/features/sessions/session-activity";
import { BackgroundTaskPanel } from "@/features/sessions/background-panel";
import {
  injectionFailurePausesQueue,
  localQueueHeadAction,
  shouldDrainQueue,
} from "@/features/sessions/queue-drain";
import { ServerQueue } from "@/features/sessions/server-queue";
import { RotationNotice } from "@/features/sessions/rotation-notice";
import { AnswerNotice, QuestionCard } from "@/features/sessions/question-card";
import {
  composerAnswersQuestion,
  questionsByToolUse,
  sentOutcome,
  type SessionQuestion,
} from "@/features/sessions/session-questions";
import { DeliveryPreviews, useDeliveryPreviews, usePendingDeliveries } from "@/features/sessions/delivery-preview-view";
import { nextDeliveryId } from "@/features/sessions/delivery-preview";
import { OutboxList } from "@/features/sessions/outbox-list";
import {
  afterSend,
  classifySendFailure,
  headSendable,
  mergeStoredOutboxes,
  newClientMessageId,
  prepareResend,
  readOutbox,
  SendUnknown,
  thrownFailure,
  timeoutSignal,
  withoutMessage,
  type OutgoingMessage,
} from "@/features/sessions/send-receipts";
import { useOutboxResolution } from "@/features/sessions/use-outbox-resolution";
import { externalSendBlock } from "@/features/sessions/send-block";
import { SessionAccountBadge } from "@/features/sessions/session-account-badge";
import { ContinueOnAccountButton, looksLikeLoginFailure, type OpenSession } from "@/features/sessions/continue-on-account";
import { activeRests } from "@/features/accounts/account-rest";
import { localLoginOf, localLoginSignedOut, useAccounts } from "@/features/accounts/use-accounts";
import { StatusBadge } from "@/features/sessions/sessions-layout";
import {
  CommandChip,
  CompactDivider,
  ModelFallbackNotice,
  modelFallbackOf,
  SyntheticNotice,
} from "@/features/sessions/transcript-notices";
import { RenameSessionDialog } from "@/features/sessions/rename-dialog";
import {
  SearchButton,
  SessionSearchBar,
  useFindShortcut,
  useSessionSearch,
} from "@/features/sessions/session-search-view";
import { SessionPinButton } from "@/features/sessions/session-pin";
import { useReportSeen } from "@/features/sessions/use-seen";
import { FirstUnreadButton, useFirstUnread } from "@/features/sessions/first-unread-view";
import { type DraftStore } from "@/features/sessions/stream-draft";
import { TurnStatus } from "@/features/sessions/turn-status";
import { PaneUnreadPill, usePaneSeat } from "@/app/pane-seat";
import {
  bubbleRuns,
  chunkItems,
  countSteps,
  EMPTY_PAIRING,
  firstTimestamp,
  foldClassOf,
  modelOf,
  pairToolResults,
  type Chunk,
  type ToolPairing,
} from "@/features/sessions/transcript-groups";
import { feedHubFor, useFeedSessionTitle } from "@/app/global-feed";
import { MachineLink, useMachineNavigate } from "@/app/machine-route";
import { devicePersistFor } from "@/features/sessions/feed-persist";
import { cn } from "@/lib/cn";
import { diffRowsFor, diffStats, DIFF_MAX_ROWS, type DiffRow } from "@/lib/diff";
import {
  durationBetween,
  formatBytes,
  formatCompact,
  formatDuration,
  formatExactTime,
  formatInt,
  formatTime,
} from "@/lib/format";
import { useApi, useEnsureLive, useInfoOf, useMachine, useMachineRuntime, useSessionRef } from "@/lib/machines/context";
import { DeviceChip } from "@/features/machines/device-picker";
import { sessionKey, type MachineKey, type SessionRef } from "@/lib/machines/key";
import { machineStorageKey, readMachineItem } from "@/lib/machines/storage";
import { mq } from "@/lib/machines/query";
import { apiFor, perMachine, transportFor } from "@/lib/machines/registry";
import { useTicker } from "@/lib/ticker";
import { noteWsFailure } from "@/lib/ws-url";

type TranscriptItem = components["schemas"]["TranscriptItem"];

function detailLineClass(line: string): string | undefined {
  if (line.startsWith("+")) return "text-ok";
  if (line.startsWith("-")) return "text-danger";
  if (line.startsWith("***") || line.startsWith("@@")) return "text-faint";
  return undefined;
}

function DetailPre({ item }: { item: TranscriptItem }) {
  const { t } = useTranslation();
  const asDiff =
    item.kind === "tool_use" && (item.toolName === "Edit" || item.toolName === "apply_patch");
  return (
    <pre data-diff={asDiff ? "" : undefined} className="mt-1 mb-1.5 max-h-96 overflow-auto border border-border bg-raised px-2.5 py-1.5 font-mono text-2xs leading-[18px] whitespace-pre-wrap text-fg/85">
      {asDiff
        ? item.detail!.split("\n").map((line, i) => (
            <span key={i} className={detailLineClass(line)}>
              {line}
              {"\n"}
            </span>
          ))
        : item.detail}
      {item.truncated && <span data-signal="truncated" className="text-warn-fg"> {t("…[truncated]")}</span>}
    </pre>
  );
}

/** The rows of a diff card: real line numbers when the CLI recorded hunks. */
function DiffRows({ rows }: { rows: DiffRow[] }) {
  const { t } = useTranslation();
  const shown = rows.slice(0, DIFF_MAX_ROWS);
  const hidden = rows.length - shown.length;
  // A recomputed diff has no file line numbers to show, so the gutter goes.
  const hasLineNumbers = shown.some((r) => r.oldLn !== undefined || r.newLn !== undefined);
  return (
    <div data-diff="" className="mt-1 mb-1.5 max-h-96 overflow-auto border border-border font-mono text-2xs leading-[18px]">
      {shown.map((row, i) => (
        <div
          key={i}
          className={cn(
            "flex",
            row.type === "add" && "bg-ok/9 text-ok",
            row.type === "del" && "bg-danger/8 text-danger",
            row.type === "hunk" && "text-faint",
            row.type === "ctx" && "text-fg/85",
          )}
        >
          {hasLineNumbers && (
            <span className="w-9 shrink-0 border-r border-border pr-2 text-right tabular-nums opacity-80 select-none">
              {row.newLn ?? row.oldLn ?? ""}
            </span>
          )}
          <span className="w-5 shrink-0 text-center select-none">
            {row.type === "add" ? "+" : row.type === "del" ? "−" : " "}
          </span>
          <span className="min-w-0 pr-2 break-all whitespace-pre-wrap">{row.text}</span>
        </div>
      ))}
      {hidden > 0 && (
        <div className="border-t border-border px-2 py-0.5 text-faint">{t("…{{n}} more lines", { n: hidden })}</div>
      )}
    </div>
  );
}

/** Thumbnails for images a result carried, rather than the literal `[1 image]`. */
function ResultImages({ paths }: { paths: string[] }) {
  return (
    <div className="mt-1 mb-1 flex flex-wrap gap-1.5">
      {paths.map((p) => (
        <LightboxImage
          key={p}
          source={{ kind: "host", path: p }}
          alt={p}
          thumbClassName="max-h-40 max-w-[min(15rem,100%)] rounded border border-border object-contain"
        />
      ))}
    </div>
  );
}

/**
 * One tool call, with the result it produced folded into the same card when
 * that result is inside the loaded window. Edits render as a diff instead of
 * the raw old/new blob, and their success output — "the file has been
 * updated" — is dropped, because the diff above it already said so.
 *
 * Memoized on its own two facts. The pairing map is rebuilt on every frame the
 * socket delivers, so reading it from context here would re-render every row in
 * the window twice a second; [`ItemBlock`] reads it instead and hands this row
 * only the one result that answers it.
 */
export const ToolLine = memo(function ToolLine({
  item,
  result,
  live,
}: {
  item: TranscriptItem;
  result?: TranscriptItem;
  /** The session is working, so an unanswered call is running rather than lost. */
  live?: boolean;
}) {
  const { t } = useTranslation();
  const inBubble = useContext(InBubbleContext);
  const rowClass = inBubble ? "py-[3px]" : "border-b border-border px-4 py-[3px]";
  const isUse = item.kind === "tool_use";
  // Inside a folded run the steps are a real sequence, so they carry numbers.
  const stepNo = useContext(StepNumberContext);
  // Codex carries the code in metadata; Claude can report it as the result's
  // entire headline. Never mistake a number somewhere in stdout for a code.
  const exitCode = result?.meta?.match(/(?:^| · )exit (-?\d+)(?:$| · )/i)?.[1]
    ?? result?.text.match(/^Exit code:?\s+(-?\d+)\s*$/i)?.[1];
  const isError = item.isError || result?.isError === true || (exitCode !== undefined && Number(exitCode) !== 0);
  const rows = isUse ? diffRowsFor(item.toolName, item.detail, result?.structuredPatch) : null;
  const stats = rows ? diffStats(rows) : null;
  // A diff is its own report; everything else still wants its output shown.
  const showResultBody = result !== undefined && (rows === null || result.isError);
  // "Async agent launched successfully. (This tool result is internal
  // metadata…)" is a fifth of all results in real history, and every one of them
  // spent its line saying nothing. The receipt still matters — it is the CLI
  // confirming the child is out — so it keeps a line, in words.
  const ack = result ? asyncLaunchAckOf(result) : null;
  const resultText = ack
    ? ack === "agent"
      ? t("async agent started")
      : t("running in the background")
    : result?.text;
  // Both ends of the call, when both are in the window. Under two seconds is
  // every ordinary call, and a column of `0s` would bury the one that is not.
  const elapsed = durationBetween(item.timestamp, result?.timestamp ?? null);
  const duration = elapsed !== null && elapsed >= 2000 ? formatDuration(elapsed) : null;
  // Without a description, use the tool name and keep the command in the fold.
  const commandOnly = isUse && item.text.trimStart().startsWith("$ ");
  const description = (commandOnly ? item.toolName : item.text) || item.toolName || t("tool");
  const callDetail = item.detail ?? (commandOnly ? item.text : item.meta?.startsWith("$ ") ? item.meta : null);
  const status = exitCode !== undefined
    ? `exit ${exitCode}`
    : isError
      ? t("failed")
      : ack || (item.background && !result)
        ? t("bg")
        : result
          ? t("completed")
          : null;
  // Flat mode draws no per-turn header, so a run of fifty steps carries no time
  // anchor anywhere on screen; the row's own hover is the cheapest place to put
  // one back, and it goes on the row rather than on one span inside it so that
  // pointing anywhere along the line answers "when was this".
  const exact = [
    formatExactTime(item.timestamp),
    duration && `${t("how long the call took")}: ${duration}`,
  ].filter(Boolean).join(" · ") || undefined;
  const row = (
    <span data-signal={isError ? "failed" : undefined} className="flex min-w-0 items-baseline gap-1.5 font-mono text-2xs leading-4">
      {stepNo !== null && (
        <span aria-hidden="true" className="w-5 shrink-0 tabular-nums text-faint">
          {String(stepNo).padStart(2, "0")}
        </span>
      )}
      <span
        title={item.toolName ?? undefined}
        role={isUse ? "img" : undefined}
        aria-label={isUse ? item.toolName ?? t("tool") : undefined}
        className={cn(
          "shrink-0",
          isError ? "text-danger" : isUse ? "text-muted" : "text-faint",
        )}
      >
        {isUse ? "⚙" : "↳"}
      </span>
      <span
        className={cn("min-w-0 flex-1 truncate", isError ? "text-danger/90" : isUse ? "text-fg/80" : "text-faint")}
        title={description}
      >
        {description}
      </span>
      {status && (
        <span
          className={cn("shrink-0 pl-2 tabular-nums", isError ? "text-danger/90" : "text-faint/70")}
          title={resultText ?? undefined}
        >
          {status}
        </span>
      )}
      {/* No result and the session is still working: this is the step in flight,
          which in a long sequence is the only line anybody is looking for.
          Green rather than the old faint spinner's grey: what made that one
          findable in a wall of finished rows was that it moved, and a dot that
          blinks once a second has to earn the same glance on colour instead. */}
      {isUse && !result && live && !status && (
        <LiveDot
          title={t("still running")}
          className="bg-ok"
        />
      )}
    </span>
  );

  const resultImages = result?.images ?? null;
  const hasBody =
    rows !== null ||
    !!callDetail ||
    !!item.meta ||
    !!result?.meta ||
    (showResultBody && !!(result.detail || resultText)) ||
    (resultImages !== null && resultImages.length > 0);
  if (!hasBody) {
    return (
      <div className={rowClass} title={exact}>
        {row}
      </div>
    );
  }
  return (
    <details className={rowClass}>
      <summary title={exact} className="-mx-1 cursor-pointer list-none rounded px-1 hover:bg-hover">
        {row}
      </summary>
      {item.meta && !item.meta.startsWith("$ ") && (
        <div className="mt-1 font-mono text-2xs text-faint">{item.meta}</div>
      )}
      {stats && (
        <div data-diff="" className="mt-1 font-mono text-2xs tabular-nums">
          <span data-signal="ok" className="text-ok">+{stats.adds}</span> <span data-signal="failed" className="text-danger">−{stats.dels}</span>
        </div>
      )}
      {rows ? <DiffRows rows={rows} /> : callDetail && <DetailPre item={{ ...item, detail: callDetail }} />}
      {resultImages && resultImages.length > 0 && <ResultImages paths={resultImages} />}
      {result?.meta && <div className="mt-1 font-mono text-2xs text-faint">{result.meta}</div>}
      {showResultBody && (result.detail || resultText) && (
        <>
          {rows !== null && (
            <div data-signal="failed" className="mt-1 font-mono text-2xs text-danger">{t("the edit failed")}</div>
          )}
          <DetailPre item={{ ...result, detail: result.detail || resultText || null }} />
        </>
      )}
    </details>
  );
});

/** The step's number inside a folded run (null outside one). */
const StepNumberContext = createContext<number | null>(null);

/**
 * A folded run of steps. `base` is the absolute index of its first item: every
 * row is tagged with its own (`data-ti`), so a search can find the row an item
 * landed in even when it is folded away in here.
 */
const StepGroup = memo(function StepGroup({ items, base }: { items: TranscriptItem[]; base: number }) {
  const { t } = useTranslation();
  const inBubble = useContext(InBubbleContext);
  const counts = new Map<string, number>();
  // The same rule the fold threshold used, so a group can no longer collapse on
  // one count and then announce another.
  const steps = countSteps(items);
  for (const item of items) {
    if (foldClassOf(item) !== "step") continue;
    const key = item.kind === "thinking" ? t("thinking") : (item.toolName ?? t("tool"));
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  const summary = [...counts.entries()]
    .sort((a, b) => b[1] - a[1])
    .slice(0, 3)
    .map(([name, n]) => `${name} ×${n}`)
    .join(" · ");
  const hasError = items.some((i) => i.isError);
  // 01, 02, … for the rows a reader sees: calls and thinking, not the results
  // that fold into their call.
  let counter = 0;
  const stepNumbers = items.map((item) => (item.kind === "tool_use" || item.kind === "thinking" ? ++counter : null));

  return (
    <details className={cn("group/steps", !inBubble && "border-b border-border")}>
      <summary
        className={cn(
          "flex h-6.5 cursor-pointer list-none items-center gap-2 font-mono text-2xs hover:bg-hover",
          inBubble ? "-mx-1 px-1" : "px-4",
        )}
      >
        <ChevronRight className="size-3 shrink-0 text-faint group-open/steps:rotate-90" />
        <span data-label="" className="label-caps shrink-0 text-[10px] text-fg">{t("{{n}} steps", { n: steps })}</span>
        <span className="truncate text-muted">{summary}</span>
        {hasError && (
          <span data-signal="failed" className="shrink-0 text-danger" title={t("contains a failed tool call")}>
            ×
          </span>
        )}
      </summary>
      <div className={cn("mb-2 border-l border-border pl-2.5", inBubble ? "ml-1.5" : "mr-4 ml-[22px]")}>
        {items.map((item, i) => (
          <div key={i} data-ti={base + i}>
            <StepNumberContext.Provider value={stepNumbers[i] ?? null}>
              <ItemBlock item={item} />
            </StepNumberContext.Provider>
          </div>
        ))}
      </div>
    </details>
  );
});

// Roles are ink caps labels: green belongs to "working", and the user's turns
// are told apart by their raised band, not by a colour.
const ROLE_STYLE: Record<string, { label: string; className: string }> = {
  user: { label: "USER", className: "text-fg" },
  assistant: { label: "AGENT", className: "text-fg" },
  system: { label: "SYS", className: "text-faint" },
};

type TranscriptStyle = "flat" | "bubble";

/**
 * True while rendering inside a bubble, which already draws the border, padding
 * and one role header for the whole turn — so rows render flush and headerless
 * instead of as standalone full-width lines.
 */
const InBubbleContext = createContext(false);

/**
 * The model the enclosing bubble's header is already showing, if any.
 *
 * Rows inside a bubble draw no header — the turn's one header speaks for them,
 * and a bubble holds one model — so this is how a row knows whether the model
 * behind it has been said out loud already.
 * A subagent's transcript is the case where it has not: it ran on whatever model
 * *it* was handed, which the parent turn's header knows nothing about, so the
 * drill-down clears this (see [`AgentTranscriptBody`]) and its messages name
 * their own model.
 */
const BubbleModelContext = createContext<string | null>(null);

/**
 * A message's header: who spoke, on which model, when.
 *
 * The model is here because both CLIs downgrade in silence — a turn asked of
 * `claude-opus-5` can come back from something smaller with nothing on screen
 * saying so, and the reader concludes the agent got worse rather than smaller.
 * So the id is printed verbatim and *in full*, however long: truncating it to
 * `GLM-5.3-CYBERSECURITY-…` would hide the exact half that answers the question.
 * The header wraps instead of eliding, which costs a second line only on a
 * narrow screen showing a long id, and the `title` repeats it for good measure.
 */
function MessageHead({
  role,
  model,
  timestamp,
}: {
  role: { label: string; className: string };
  model: string | null;
  timestamp: string | null;
}) {
  return (
    <div className="mb-1 flex flex-wrap items-baseline gap-x-2 gap-y-0.5 font-mono text-[10px] leading-[14px]">
      <span data-label="" className={cn("label-caps shrink-0 font-semibold", role.className)}>{role.label}</span>
      {model && (
        <span className="min-w-0 break-words text-muted" title={model}>
          {model}
        </span>
      )}
      {timestamp && (
        <span className="ml-auto whitespace-nowrap tabular-nums text-faint">
          {formatTime(timestamp)}
        </span>
      )}
    </div>
  );
}

/**
 * Which result answers which call in the window being rendered. Held in context
 * because tool rows are reached from three places (the transcript itself, a
 * folded step group, and a subagent drill-down), each with its own window.
 *
 * Read in exactly one place — [`ItemBlock`], which is deliberately not
 * memoized — and never inside a memoized row. A context value that changes
 * re-renders every consumer under it regardless of `memo`, and this one is
 * rebuilt whenever the window is, so a thousand-row transcript would reconcile
 * in full on every append. The rows take their one answer as a prop instead.
 */
const PairingContext = createContext<ToolPairing>(EMPTY_PAIRING);

/**
 * Whether the session is working, for rows that mean different things when it
 * is: a tool call with no result is "running" during a turn and "never came
 * back" outside one. A plain boolean, so it re-renders the rows only when the
 * session actually changes state.
 */
const LiveContext = createContext(false);

/**
 * The launching `tool_use` ids the resident is minding right now.
 *
 * Read in [`ItemBlock`] only, and rebuilt only when the ids themselves change
 * rather than on every state frame — see `runningTaskIds` at the call site. A
 * card asks one question of it ("is mine in here"), and gets a boolean prop, so
 * the memoized rows below are never handed a set that is a new object.
 */
const NO_RUNNING_TASKS: ReadonlySet<string> = new Set<string>();
const RunningTasksContext = createContext<ReadonlySet<string>>(NO_RUNNING_TASKS);

/**
 * The questions the agent asked, by the `tool_use` each one is, and the session
 * to answer them in.
 *
 * Read in [`ItemBlock`] only, like the two above, and for the same reason: a
 * question card asks one thing of it ("what became of mine") and is handed the
 * one record as a prop, so the memoized rows never see the map. The value is
 * rebuilt only when the feed's `questions` list is a new object — which
 * `applyStateFrame` arranges to be only when a question opened or closed, not
 * on every state frame.
 */
interface QuestionIndex {
  session: SubagentCtx | undefined;
  byToolUse: ReadonlyMap<string, SessionQuestion>;
}
const NO_QUESTIONS: QuestionIndex = { session: undefined, byToolUse: questionsByToolUse([]) };
const QuestionsContext = createContext<QuestionIndex>(NO_QUESTIONS);

/**
 * One turn's bubble: draws the side, the chrome and a single role header.
 *
 * Exported for `web/test`, which holds the header against what it must say.
 */
export function BubbleRun({
  role,
  model,
  timestamp,
  children,
}: {
  role: "assistant" | "user";
  /** The one model the turn was written by — see [`bubbleRuns`]. */
  model: string | null;
  timestamp: string | null;
  children: React.ReactNode;
}) {
  const isUser = role === "user";
  const style = ROLE_STYLE[role]!;
  return (
    <div className={cn("flex px-4 py-1", isUser ? "justify-end" : "justify-start")}>
      <div
        className={cn(
          "min-w-0 max-w-[85%] border px-3 py-2",
          isUser ? "border-border bg-raised" : "border-border bg-bg",
        )}
      >
        <MessageHead role={style} model={model} timestamp={timestamp} />
        <InBubbleContext.Provider value={true}>
          <BubbleModelContext.Provider value={model}>{children}</BubbleModelContext.Provider>
        </InBubbleContext.Provider>
      </div>
    </div>
  );
}

/**
 * One agent's own transcript, fetched on demand.
 *
 * Shared by the card a `Task` call draws in the stream and by the list of
 * agents that hang off nothing in it — the second kind is reached only through
 * that list, and giving it a different reader would have been two answers to
 * "what did this agent do".
 */
function useAgentTranscript(session: SubagentCtx, agentId: string | null, open: boolean) {
  const api = useApi();
  const machine = useMachine();
  return useQuery({
    enabled: open && !!agentId,
    queryKey: mq(machine, "subagent", session.source, session.id, agentId),
    queryFn: async () => {
      const res = await api.GET("/api/sessions/{source}/{id}/agents/{agentId}", {
        params: { path: { source: session.source, id: session.id, agentId: agentId! } },
      });
      if (res.error) throw new ApiError(res.error);
      return res.data;
    },
    // A running subagent keeps appending; follow it while expanded.
    refetchInterval: open ? 4000 : false,
  });
}

type AgentTranscriptQuery = ReturnType<typeof useAgentTranscript>;

function AgentTranscriptBody({
  query,
  session,
}: {
  query: AgentTranscriptQuery;
  session: SubagentCtx;
}) {
  const { t } = useTranslation();
  const { data, error, isFetching, refetch } = query;
  // The subagent's transcript is its own window, so it pairs on its own.
  const subPairing = useMemo(() => pairToolResults(data?.items ?? []), [data?.items]);
  return (
    <div className="ml-3 border-l border-accent/30">
      {/* A refused drill-down used to spin here for ever, which reads as a
          subagent that is still loading rather than as one that cannot be
          read at all — and the retry is one click, so offer it. */}
      {!data && error && (
        <div data-signal="failed" className="flex flex-wrap items-center gap-2 px-3 py-1.5 font-mono text-2xs text-danger">
          <span className="min-w-0 break-words">
            {t("could not load this subagent:")} {errorText(error)}
          </span>
          <button
            type="button"
            disabled={isFetching}
            onClick={() => void refetch()} data-signal="failed" className="shrink-0 rounded bg-danger/15 px-1.5 py-0.5 text-danger hover:bg-danger/25 disabled:opacity-50"
          >
            {t("retry")}
          </button>
        </div>
      )}
      {!data && !error && <Spinner className="py-2" />}
      {data && data.omittedItems > 0 && (
        <div className="px-3 py-1 font-mono text-2xs text-faint">
          {t("…{{n}} earlier steps omitted", { n: data.omittedItems })}
        </div>
      )}
      <PairingContext.Provider value={subPairing}>
        {/* The card's own header names the *parent's* model; these messages are
            the child's, so nothing above them has said which model wrote them. */}
        <BubbleModelContext.Provider value={null}>
          {data?.items.map((sub: TranscriptItem, i: number) => (
            <ItemBlock key={i} item={sub} subagentSession={session} />
          ))}
        </BubbleModelContext.Provider>
      </PairingContext.Provider>
    </div>
  );
}

/**
 * The glyph and colour each terminal state wears.
 *
 * One character, in the transcript's own monospace column, rather than a badge:
 * a fan-out of eight cards is read down the left edge, and eight badges is a
 * wall. `unknown` deliberately borrows `stopped`'s glyph in a fainter tone —
 * both mean "no more work is coming from this one", and the difference between
 * them is the tooltip, not a shape somebody has to learn.
 */
const OUTCOME_MARK: Record<Exclude<AgentOutcome, "running">, { glyph: string; className: string }> = {
  completed: { glyph: "✓", className: "text-ok/80" },
  failed: { glyph: "✗", className: "text-danger" },
  stopped: { glyph: "⊘", className: "text-muted" },
  unknown: { glyph: "⊘", className: "text-faint/60" },
};

/**
 * The receipt a subagent call came back with, folded onto its card.
 *
 * The server now stamps this result with the launching call's `agent`, so
 * without this it renders as a top-level row of its own directly under the
 * card — a second line saying the thing the card's own ✓ already says. The
 * terminal receipt belongs to the card.
 */
function AgentResultLine({ result }: { result: TranscriptItem }) {
  const { t } = useTranslation();
  const ack = asyncLaunchAckOf(result);
  const text = ack
    ? ack === "agent"
      ? t("async agent started")
      : t("running in the background")
    : result.text;
  const row = (
    <span
      className={cn(
        "flex min-w-0 items-baseline gap-1.5 font-mono text-2xs",
        result.isError ? "text-danger/90" : "text-faint",
      )}
    >
      <span className="shrink-0">↳</span>
      <span className="min-w-0 truncate" title={result.text}>
        {text}
      </span>
    </span>
  );
  if (!result.detail) return <div className="px-3 py-[3px] pl-9">{row}</div>;
  return (
    <details className="px-3 py-[3px] pl-9">
      <summary className="-mx-1 cursor-pointer list-none rounded px-1 hover:bg-hover">{row}</summary>
      <DetailPre item={result} />
    </details>
  );
}

/**
 * Expandable view of a subagent's own transcript, with how it ended on the head.
 *
 * The mark on the right is the whole point of the card being a card. Before it,
 * a fan-out of eight `Task` calls was eight identical rows whatever had become
 * of them — three finished, one failed, one stopped and three still out looked
 * exactly alike, and the only way to tell was to open each one. The state is
 * derived by [`subagentOutcome`] and *latched*: every source that could say
 * "this finished" goes quiet eventually (the resident drops the task, the
 * notification pages out of the window), and a card that recomputed freely
 * would go from ✓ back to ⊘ for work that is demonstrably over.
 */
export function SubagentTool({
  item,
  session,
  result,
  taskStatus,
  residentRunning = false,
  live = false,
}: {
  item: TranscriptItem;
  session: SubagentCtx;
  /** The launching call's own result: a real answer, or an async receipt. */
  result?: TranscriptItem;
  /** `taskStatus` from the `⏱` notification that named this call, when in window. */
  taskStatus?: string;
  /** The resident is minding this `tool_use` id right now. */
  residentRunning?: boolean;
  /** The session has a turn in flight. */
  live?: boolean;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const query = useAgentTranscript(session, item.agent ?? null, open);
  const data = query.data;

  // A ref rather than state: the latch changes nothing about what would be
  // rendered on its own, it only refuses to un-render a verdict already shown,
  // and a `setState` here would be a second render per frame for every card.
  const latch = useRef<AgentOutcome | null>(null);
  latch.current = latchOutcome(
    latch.current,
    subagentOutcome({ result, taskStatus, residentRunning, live }),
  );
  const outcome = latch.current;
  const mark = outcome === "running" ? null : OUTCOME_MARK[outcome];
  const markTitle =
    outcome === "completed"
      ? t("this subagent finished")
      : outcome === "failed"
        ? t("this subagent failed")
        : outcome === "stopped"
          ? t("this subagent was stopped")
          : t("no result and nothing is minding it any more — how this subagent ended is unknown");

  return (
    <div className="border-b border-border">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex w-full cursor-pointer items-center gap-1.5 px-3 py-1 text-left font-mono text-2xs hover:bg-hover"
      >
        <ChevronRight className={cn("size-3 shrink-0 text-faint transition-transform", open && "rotate-90")} />
        <span className="shrink-0 text-accent">⚡ {t("subagent")}</span>
        <span className="min-w-0 flex-1 truncate text-muted">{data?.description ?? item.text}</span>
        {data && (
          <span className="shrink-0 text-faint">
            {data.agentType ?? ""} · {t("{{n}} items", { n: data.totalItems })}
          </span>
        )}
        {mark ? (
          <span className={cn("shrink-0", mark.className)} title={markTitle}>
            {mark.glyph}
          </span>
        ) : (
          <LiveDot className="bg-ok" title={t("this subagent is still running")} />
        )}
      </button>
      {result && <AgentResultLine result={result} />}
      {open && <AgentTranscriptBody query={query} session={session} />}
    </div>
  );
}

interface SubagentCtx {
  source: "claude" | "codex";
  id: string;
}


interface ItemBlockProps {
  item: TranscriptItem;
  /** Present when subagent drill-down applies (claude sessions). */
  subagentSession?: SubagentCtx;
}

/**
 * The pairing lookup, done once per row and outside the memo.
 *
 * Everything the transcript renders goes through here, and this is the only
 * component that reads [`PairingContext`]. Keeping the read here — in a function
 * that renders nothing itself — is what lets [`ItemRow`] below skip its whole
 * subtree when the window grows: it receives the one result that answers it
 * instead of a map that is a new object on every frame.
 */
function ItemBlock(props: ItemBlockProps) {
  const pairing = useContext(PairingContext);
  const running = useContext(RunningTasksContext);
  const questions = useContext(QuestionsContext);
  const item = props.item;
  // Its call renders it; a second standalone row would repeat the same output.
  if (item.kind === "tool_result" && pairing.consumed.has(item)) return null;
  const id = item.kind === "tool_use" ? (item.toolUseId ?? null) : null;
  const result = id ? pairing.resultFor.get(id) : undefined;
  const question = id && item.toolName === "AskUserQuestion" ? questions.byToolUse.get(id) : undefined;
  return (
    <ItemRow
      {...props}
      result={result}
      // Both of these are plain values, so a memoized row still bails out on a
      // frame that did not change *its* answer — which is every frame, for
      // every card but the one that just finished.
      taskStatus={id ? pairing.taskStatusFor.get(id) : undefined}
      residentRunning={id ? running.has(id) : false}
      noticeJoined={pairing.noticeJoined.has(item)}
      // The record itself, held by identity while its list is: only the card
      // whose question moved re-renders when one does.
      question={question}
      questionSession={question ? questions.session : undefined}
    />
  );
}

const ItemRow = memo(function ItemRow({
  item,
  result,
  taskStatus,
  residentRunning,
  noticeJoined,
  question,
  questionSession,
  subagentSession,
}: ItemBlockProps & {
  result?: TranscriptItem;
  taskStatus?: string;
  residentRunning?: boolean;
  noticeJoined?: boolean;
  /** The server's record of this `AskUserQuestion`, when it keeps one. */
  question?: SessionQuestion;
  /** Where that question is answered; stable for the life of the view. */
  questionSession?: SubagentCtx;
}) {
  const { t } = useTranslation();
  const inBubble = useContext(InBubbleContext);
  const live = useContext(LiveContext);
  // Which model wrote this row, and whether anything above it has said so: a
  // bubble's header speaks for its rows, but only about its own model — a
  // subagent ran on whatever model *it* was handed, which the card knows
  // nothing about. Read before the test, not inside it: a hook behind a `||` is
  // a hook the next row skips.
  const bubbleModel = useContext(BubbleModelContext);
  const model = modelOf(item);
  const modelSaidAbove = model === null || model === bubbleModel;
  const rowClass = inBubble ? "py-[3px]" : "border-b border-border px-4 py-[3px]";
  if (item.kind === "tool_use" && item.agent && subagentSession) {
    return (
      <SubagentTool
        item={item}
        session={subagentSession}
        result={result}
        taskStatus={taskStatus}
        residentRunning={residentRunning}
        live={live}
      />
    );
  }
  if (item.kind === "tool_use" && item.toolName === "AskUserQuestion") {
    return <QuestionCard item={item} record={question} session={questionSession} />;
  }
  if (item.kind === "tool_use" || item.kind === "tool_result") {
    return <ToolLine item={item} result={result} live={live} />;
  }

  if (item.kind === "thinking") {
    return (
      <details className={rowClass}>
        <summary className="-mx-1 cursor-pointer list-none px-1 font-mono text-2xs text-faint italic hover:bg-hover">
          ∴ {t("thinking")}
          <span className="ml-2 not-italic text-faint/70">{item.text.slice(0, 90).replace(/\s+/g, " ")}…</span>
        </summary>
        <div className="mt-1 mb-1 text-2xs leading-4 whitespace-pre-wrap text-faint italic">{item.text}</div>
      </details>
    );
  }

  // Neither is a message: one is the seam where the context was dropped, the
  // other an instruction to the CLI. Both draw themselves.
  if (item.kind === "compact") return <CompactDivider item={item} />;
  if (item.kind === "command") return <CommandChip item={item} />;

  if (item.role === "system") {
    // A model swap is the one system line nobody may scroll past: every answer
    // after it comes from a different model, and it used to be one grey
    // monospace line among all the other grey monospace lines.
    const fallback = item.kind === "text" ? modelFallbackOf(item.text) : null;
    if (fallback) return <ModelFallbackNotice fallback={fallback} rowClass={rowClass} />;
    // Everything else — notifications, interrupts — stays one quiet line.
    return <SystemLine item={item} rowClass={rowClass} joined={noticeJoined} />;
  }

  // The CLI apologising in the model's voice: a rate-limit wall, a "Not logged
  // in", an API error phrased as a turn. Drawn as chrome rather than as speech.
  if (item.synthetic) {
    return (
      <SyntheticNotice
        item={item}
        rowClass={inBubble ? "py-1" : "border-b border-border px-4 py-1.5"}
      />
    );
  }

  const role = ROLE_STYLE[item.role] ?? ROLE_STYLE.system!;
  return (
    <div
      className={cn(
        inBubble ? "py-1" : "border-b border-border px-4 pt-2 pb-2.5",
        !inBubble && item.role === "user" && "bg-raised",
      )}
    >
      {/* In a bubble the wrapper already labelled the turn — unless it cannot
          have named this model, which is a subagent's message under a card. */}
      {inBubble ? (
        model &&
        !modelSaidAbove && (
          <div className="mb-0.5 min-w-0 font-mono text-[10px] break-words text-muted" title={model}>
            {model}
          </div>
        )
      ) : (
        <MessageHead role={role} model={model} timestamp={item.timestamp} />
      )}
      <Markdown text={item.text} className={inBubble ? undefined : "max-w-[48em]"} />
      {item.images && item.images.length > 0 && (
        <div className="mt-1.5 flex flex-wrap gap-1.5">
          {item.images.map((p) => (
            <LightboxImage
              key={p}
              source={{ kind: "host", path: p }}
              alt={p}
              thumbClassName="max-h-40 max-w-[min(15rem,100%)] border border-border object-contain"
            />
          ))}
        </div>
      )}
      {item.files && item.files.length > 0 && (
        <div className="mt-1.5 flex flex-wrap gap-1.5">
          {item.files.map((f) => (
            <FileChip key={f.path} name={f.name} path={f.path} />
          ))}
        </div>
      )}
      {item.truncated && <span data-signal="truncated" className="font-mono text-2xs text-warn-fg"> {t("…[truncated]")}</span>}
    </div>
  );
});

/**
 * Words a background task's own report uses when it did not go well.
 *
 * The fallback for a server too old to send `taskStatus`, which folded the
 * outcome into the headline (`{summary} · {status}`) and left the sentence as
 * the only thing to key on. Deliberately a plain substring test on the first
 * line — a report *body* that discusses a failure is not itself a failure —
 * and deliberately consulted last: where the field is present it is the answer,
 * because it is the CLI's own word rather than this client reading English.
 */
const FAILED_WORDS = /\b(failed|failure|stopped|cancell?ed|error|timed out)\b/i;

/**
 * One quiet line for system chatter — and, for a finished background task, its
 * whole report.
 *
 * The report is the reason this is not a `<pre>` any more. A `⏱` notification
 * carries whatever the child wrote back, which in real history has been
 * twenty-two kilobytes of Markdown with tables in it, and rendering that as
 * pre-wrapped plain text was the same as not delivering it. The server now
 * splits the two — one headline in `text`, the whole report in `detail` — so
 * the summary line is a line and the fold is the document.
 */
export function SystemLine({
  item,
  rowClass,
  joined = false,
}: {
  item: TranscriptItem;
  rowClass: string;
  /**
   * The card that launched this task is in the same window and is already
   * wearing the outcome. See `ToolPairing.noticeJoined`.
   */
  joined?: boolean;
}) {
  const { t } = useTranslation();
  const headline = item.text.split("\n", 1)[0] ?? "";
  // The detail field where the server sets one; otherwise everything the
  // headline left behind, which is where the notification body used to live.
  const body = item.detail ?? (item.text.length > headline.length ? item.text.slice(headline.length).trim() : "");
  const status = normalizeTaskStatus(item.taskStatus);
  // A finished background task is real news, not routine chatter — and a task
  // that failed is a different kind of news from one that worked.
  //
  // `joined` quiets a *success* and nothing else. The card above already shows
  // the ✓, so a second amber line for it is the same news twice; a failure or a
  // stop keeps its colour wherever it lands, because a red line that turned
  // grey because the transcript happened to still hold its launch would be the
  // interface hiding the one thing it is for.
  const tone = !item.background
    ? "text-faint"
    : status === "failed"
      ? "text-danger"
      : status === "stopped"
        ? "text-muted"
        : joined
          ? "text-faint/80"
          : status === "completed"
            ? "text-warn-fg"
            : FAILED_WORDS.test(headline)
              ? "text-danger"
              : "text-warn-fg";
  return (
    <details className={rowClass}>
      <summary
        title={joined ? t("reported on the card that launched it, above") : undefined}
        className={cn(
          "-mx-1 cursor-pointer list-none truncate rounded px-1 font-mono text-2xs hover:bg-hover",
          tone,
        )}
      >
        {headline}
      </summary>
      {body &&
        (item.background ? (
          <Markdown text={body} className="mt-1 mb-1 text-muted" />
        ) : (
          <div className="mt-1 mb-1 text-2xs leading-4 whitespace-pre-wrap text-faint">{item.text}</div>
        ))}
    </details>
  );
}

type Source = components["schemas"]["Source"];

interface SessionFeedResult {
  data: FeedState | null;
  /**
   * The count of the think in flight, which the transcript cannot say — see
   * `stream-draft.ts`. A store rather than a value, and beside `data` rather
   * than in it: nothing that reads the window (the rows memo, the unread
   * cursor, the search, the first-unread jump) has any business with it, and
   * a draft frame — a dozen a second during a long think — must not re-render
   * the view this hook serves. Only the activity bar's words subscribe to it;
   * the transcript draws nothing of it. The same object for the life of the
   * feed.
   */
  drafts: DraftStore;
  isError: boolean;
  /** Live, retrying, or the server has kept saying the session is not there. */
  connection: FeedConnection;
  loadEarlier: () => Promise<void>;
  isLoadingEarlier: boolean;
  historyError: string | null;
  /** Re-attach the socket, which re-snapshots state the server just rewrote. */
  refresh: () => void;
  /**
   * Re-read the state fields alone, leaving the transcript window untouched.
   *
   * The socket already pushes state, but on a two-second tick, and there are
   * actions — removing a queued message, steering one — whose whole point is
   * that the list changes when you click. Two seconds of a chip sitting there
   * unchanged reads as a click that did nothing. [`refresh`] would also work
   * and costs far too much: it re-attaches the socket, which replaces the
   * window and throws away every earlier page the user scrolled back to load.
   */
  refreshState: () => void;
}

/**
 * A machine's session feeds; see `session-feed-pool.ts` for why a feed
 * outlives the view showing it. One set per machine, kept at module level,
 * because every view of a session — a split pane beside the main one, the chat
 * presentation — should be looking at the same window. Closing the machine
 * stops them all (`closeMachine`), sockets and timers with them.
 */
const sessionFeedsFor = perMachine((machine: MachineKey) =>
  createSessionFeedPool({
    // The last few windows read, kept on the device for the next cold start.
    persist: devicePersistFor(machine),
    // On the machine's one socket where its server can carry it there, which
    // saves every session switch a connection of its own; see `app/feed-hub.ts`.
    openFeed: (target) => feedHubFor(machine).open(target),
    // A socket of the feed's own, through the machine's transport
    // (`lib/machines`) as it is at each attempt.
    socketUrl: (path) => transportFor(machine).socketUrl(path),
    openSocket: (url) => transportFor(machine).openSocket(url),
    noteFailure: (streak) => noteWsFailure(streak, machine),
    async fetchHistory(source, id, query) {
      const res = await apiFor(machine).GET("/api/sessions/{source}/{id}/history", {
        params: { path: { source: source as Source, id }, query },
      });
      if (res.error) return { status: res.response.status, error: res.error.error };
      return { status: res.response.status, page: res.data as TranscriptHistoryPage | undefined };
    },
    async fetchState(source, id, held) {
      const res = await apiFor(machine).GET("/api/sessions/{source}/{id}", {
        params: { path: { source: source as Source, id }, query: held },
      });
      return res.error || !res.data ? null : res.data;
    },
  }),
  (pool) => pool.dispose(),
);

/**
 * @param pinnedRef Whether the view is following the live tail. Read, never
 *   written, and read only at the moment a delta lands — it is the permission
 *   to trim the window's oldest items ([`trimLiveWindow`]). A ref rather than a
 *   value because the pin is owned by the scroll handler in the component
 *   below, changes on a wheel event rather than on a render, and must not put
 *   this hook's socket through a re-subscribe every time the reader moves.
 */
function useSessionFeed(
  source: string,
  id: string,
  pinnedRef: RefObject<boolean>,
): SessionFeedResult {
  const sessionFeeds = sessionFeedsFor(useMachine());
  // Looked up during the render so that a session read a moment ago is drawn
  // from its kept window on the very first frame, with no spinner between.
  const feed = useMemo(() => sessionFeeds.feed(source, id), [sessionFeeds, source, id]);
  useEffect(() => sessionFeeds.acquire(feed, pinnedRef), [sessionFeeds, feed, pinnedRef]);
  const { data, connection, isLoadingEarlier, historyError } = useSyncExternalStore(
    feed.subscribe,
    feed.getView,
    feed.getView,
  );
  const loadEarlier = useCallback(() => feed.loadEarlier(), [feed]);
  const refresh = useCallback(() => feed.reattach(), [feed]);
  const refreshState = useCallback(() => feed.refreshState(), [feed]);
  return {
    data,
    drafts: feed.drafts,
    // Only a verdict when there is nothing to show it over: a session that went
    // missing while its transcript is on screen keeps the transcript, and says
    // so in a banner instead of replacing the page with "session not found".
    isError: connection === "gone" && data === null,
    connection,
    loadEarlier,
    isLoadingEarlier,
    historyError,
    refresh,
    refreshState,
  };
}

/**
 * The strip above the composer that says what the session is doing.
 *
 * Its own component, and a presentational one: every branch here is a case of
 * [`ActivityBanner`], which is decided in `session-activity.ts` and tested
 * there. Nothing in this file gets to have a second opinion about whether a
 * session is busy — that split is the whole reason the composer stopped
 * announcing "task finished" over a turn that was still running.
 *
 * It is about the user's message and nothing else. Background work used to get
 * a count here as well as the list floating over the transcript, which is one
 * screen saying the same thing twice in two shapes; the list won, and
 * [`activityBanner`] now answers null for a session that is only minding
 * children.
 *
 * The words under the spinner are the three facts a spinner cannot carry: how
 * much this turn has written, when it last wrote anything, and how long it has
 * been going. Explicitly *not* a progress bar — nothing on either side of the
 * wire knows how many steps a turn will take, so a bar could only be a guess
 * with a denominator invented for it. While the agent thinks, the second of
 * them is the think's running count instead, and a think counts as activity
 * once it is over ([`TurnStatus`]): the one piece of live state the transcript
 * does not show, said here so that the transcript only ever grows by what has
 * been written.
 */
export function ActivityBar({
  banner,
  subtask,
  progress,
  drafts,
  stopping,
  onStop,
}: {
  banner: ActivityBanner | null;
  /** The child work to name under the headline, when any is known. */
  subtask: LiveSubtask | null;
  /** How the running turn is going; null when nothing can date it. */
  progress?: TurnProgress | null;
  /** The session's draft store, for the count of a think in flight (claude only). */
  drafts?: DraftStore;
  stopping: boolean;
  /** Absent when there is nothing a Stop could interrupt; see `bannerCanStop`. */
  onStop?: () => void;
}) {
  const { t } = useTranslation();
  const working = banner?.kind === "working";
  // One second, and only while a turn is actually in flight — an idle
  // transcript must not be re-rendered on a timer for a line it is not showing.
  const now = useTicker(1000, working);
  if (!banner) return null;
  return (
    // A container, so the thinking count can drop its unit in a narrow bar
    // (a phone, a split pane) instead of the line losing its elapsed time.
    <div data-activity-bar="" className="@container/activity border-t border-border px-4 py-1 font-mono text-2xs leading-5 text-muted">
      <div className="flex items-center gap-2">
        {/* Inherits the banner's own green through `bg-current`. This is the
            single longest-lived indicator in the app — it is up for the whole
            of every turn — so it is the one that most had to stop being a
            spinning SVG repainting the main thread at the refresh rate. */}
        <span data-signal="working" className="flex shrink-0 text-ok">
          <LiveDot />
        </span>
        <span className="min-w-0 flex-1 truncate">
          {banner.kind === "working" ? (
            <TurnStatus drafts={drafts} progress={progress ?? null} now={now} />
          ) : (
            t("{{n}} messages queued", { n: banner.queued })
          )}
          {/* A turn in flight is the headline; a message stacked up behind it
              still has to appear somewhere, and this is the only place it can. */}
          {banner.kind === "working" && banner.queued > 0 && (
            <span className="text-faint">
              {" · "}
              {t("{{n}} messages queued", { n: banner.queued })}
            </span>
          )}
          {/* Not a reason to stop anything, but it is the difference between
              "thinking hard" and "wedged", and the Stop button beside it is the
              answer to the second one. */}
          {banner.kind === "working" && banner.stalled && (
            <span data-signal="stalled" className="text-warn-fg">
              {" · "}
              {t("no output for a while")}
            </span>
          )}
        </span>
        {onStop && (
          <button
            type="button"
            title={t("interrupt the running message")}
            disabled={stopping}
            onClick={onStop}
            className="ml-auto flex h-6 shrink-0 items-center gap-1.5 border border-border-strong bg-bg px-2 font-sans text-2xs font-medium text-fg hover:bg-hover disabled:opacity-50"
          >
            {stopping ? <Loader2 className="size-3 animate-spin" /> : <span data-signal="stop" aria-hidden="true" className="size-2 bg-danger" />}
            {t("stop")}
          </button>
        )}
      </div>
      {/* "Something is running out of sight" is only half an answer; this is the
          other half, and it costs one line. */}
      {subtask && (
        <div className="mt-0.5 flex items-center gap-1.5 text-faint" title={subtask.meta ?? undefined}>
          <span className="shrink-0">↳ {subtask.background ? t("bg") : t("subagent")}</span>
          <span className="min-w-0 flex-1 truncate">{subtask.text}</span>
        </div>
      )}
    </div>
  );
}

/**
 * Why the message at the head of the queue has not gone in.
 *
 * A settings change needs a new process, and starting one would kill the
 * background work the current process is minding — so the message parks. It is
 * the one wait with no visible cause, and silence here is indistinguishable
 * from a message that was simply dropped.
 */
export function AwaitingBackgroundNotice({ resident }: { resident: ResidentInfo | null }) {
  const { t } = useTranslation();
  if (!resident?.awaitingBackground) return null;
  return (
    <div data-signal="waiting" className="border-t border-border px-4 py-1 font-mono text-2xs leading-4 text-warn-fg shadow-[inset_2px_0_0_var(--color-warn)]">
      {t(
        "this message needs a fresh session process (the settings changed), so it is waiting for {{n}} background tasks to finish — it goes in automatically, and after a timeout regardless",
        { n: resident.background },
      )}
    </div>
  );
}

interface SendRequest {
  message: OutgoingMessage;
  /**
   * Typed into the composer and sent at once, rather than taken off the queue:
   * a refusal puts the text back where it was typed, when that is still free.
   */
  typed?: boolean;
}

/**
 * Composer messages this *tab* is holding back, surviving reloads.
 *
 * Not the session's queue — that one lives in the resident, is reported as
 * `summary.resident.queuedMessages`, and is drawn by `ServerQueue`. The two
 * are named apart in the UI ("waiting to send" here, "queued in the session"
 * there) because they promise different things: a message here exists only in
 * this browser's localStorage and goes nowhere if the tab is closed, while one
 * there has been accepted and will run regardless.
 *
 * Since the server grew a real queue this one only covers what it cannot:
 *
 * - an attachment is still uploading, so there is nothing to POST yet;
 * - a one-shot Codex process is still running, because Codex has no resident
 *   or server-side queue;
 * - a send failed, and the rest is paused so the user sees the failure before
 *   the next message lands on top of it.
 *
 * Everything else is POSTed straight out and, for resident drivers, queued
 * server-side.
 */
function usePersistedQueue(
  session: SessionRef,
): [OutgoingMessage[], (q: OutgoingMessage[]) => void, React.RefObject<OutgoingMessage[]>] {
  // `rh.queue.<source>.<id>` on the local machine, as it always was.
  const name = `queue.${session.source}.${session.id}`;
  const key = machineStorageKey(session.machine, name);
  const read = (): OutgoingMessage[] => {
    try {
      // Every older shape is read, every entry gets a message id, and one that
      // was in flight when the page went away comes back `unknown`.
      // An outbox an older tab wrote under the old key since is merged in.
      return readOutbox(JSON.parse(readMachineItem(localStorage, session.machine, name, mergeStoredOutboxes) ?? "[]"));
    } catch {
      return [];
    }
  };
  const [state, setState] = useState(() => ({ key, value: read() }));
  const ref = useRef(state.value);
  if (state.key !== key) {
    const value = read();
    ref.current = value;
    setState({ key, value });
  }
  const update = (q: OutgoingMessage[]) => {
    ref.current = q;
    setState({ key, value: q });
    try {
      localStorage.setItem(key, JSON.stringify(q));
    } catch {
      // storage unavailable: the queue still works for this page
    }
  };
  return [state.value, update, ref];
}

function Composer({
  source,
  id,
  detail,
  drafts,
  subtasks,
  chat = false,
  onQueueChanged,
  onOpenSession,
}: {
  source: "claude" | "codex";
  id: string;
  detail: FeedState;
  /** The feed's draft store: what the agent is doing that the transcript does not show. */
  drafts: DraftStore;
  /** Computed once by the caller, which also draws the panel that lists them. */
  subtasks: LiveSubtask[];
  /** Instant chat: tools are disabled server-side, so hide the permission pick. */
  chat?: boolean;
  /** Re-read session state after a queue route changed it. */
  onQueueChanged: () => void;
  /** Show another session in this one's place (a continuation onto an account). */
  onOpenSession?: OpenSession;
}) {
  const { t } = useTranslation();
  const runtime = useMachineRuntime();
  const api = runtime.api;
  const session = useSessionRef(source, id);
  const draftKey = sessionDraftKey(session);
  const [initialDraft] = useState(() => {
    try {
      claimSessionDraft(localStorage, session);
      return readSessionDraftSnapshot(localStorage, draftKey);
    } catch {
      return { text: "", revision: null, pending: false };
    }
  });
  const [text, setTextState] = useState(
    initialDraft.pending ? "" : initialDraft.text,
  );
  const [draftPending, setDraftPending] = useState(initialDraft.pending);
  // The machine's offline copy is read-only: nothing is queued for later
  // (architecture doc §4: no offline queue), so the composer is shut, not deferred.
  const readOnlyCopy = useShowingCopy();
  // Another device's session in the local app (its gateway, S1-7): nothing
  // here can go to it — not a message, a model or an attachment — until S2.
  const otherDevice = useInfoOf(useMachine()).readOnly === true;
  const draftRevisionRef = useRef(initialDraft.revision);
  useEffect(() => {
    return subscribeSessionDraft(draftKey, (snapshot) => {
      draftRevisionRef.current = snapshot.revision;
      setDraftPending(snapshot.pending);
      setTextState(snapshot.pending ? "" : snapshot.text);
    });
  }, [draftKey]);
  const setText = (value: string) => {
    setTextState(value);
    try {
      const snapshot = writeSessionDraft(localStorage, draftKey, value);
      draftRevisionRef.current = snapshot.revision;
    } catch {
      // Accessing localStorage itself can be blocked; the in-memory draft still works.
    }
  };
  const sendHint = useSendHint();

  // The models offered are the ones the account this session is *bound to* may
  // run — the same value `ComposerQuota` below is given, for the same reason.
  const choice = useComposerChoice(source, id, detail.cliChoice, detail.account?.id ?? null);
  const { activeModel, activeEffort, activePermission } = choice;

  // The checklist sidecar is only ever rewritten by a tool call, so the number
  // of calls in the window is the cheapest "it may have changed" signal there
  // is. Loading earlier history moves it too; the chip debounces, so the extra
  // refetch costs one stat of a small file.
  const toolTick = useMemo(
    () => detail.items.reduce((n, i) => n + (i.kind === "tool_use" ? 1 : 0), 0),
    [detail.items],
  );

  const [queue, setQueue, queueRef] = usePersistedQueue(session);
  // claude instant chats run without tools, so they cannot read attachments.
  const allowAttach = !(chat && source === "claude");
  const attachmentState = useComposerAttachments();
  const attachments = attachmentState.attachments;
  // Enter pressed while an upload is still in flight: send as soon as it lands.
  const [sendQueued, setSendQueued] = useState(false);

  const deliveryQueue = useMemo(() => detail.summary.resident?.queuedMessages ?? [], [detail.summary.resident]);
  const delivery = useDeliveryPreviews(sessionKey(session), {
    items: detail.items, startIndex: detail.startIndex, totalItems: detail.totalItems, queue: deliveryQueue,
    receipts: detail.receipts, adoptQueue: source === "codex",
  }, true);
  // Every message carries the id it was created with (`clientMessageId`), so a
  // retry of one the server already has is answered, not run again — see
  // `send-receipts.ts`. It stays in the outbox until the server has said yes.
  const send = useMutation({
    onMutate: ({ message }: SendRequest) => {
      const receiptId = nextDeliveryId();
      delivery.change({ type: "start", entry: {
        id: receiptId, text: message.text, after: detail.totalItems, sending: true,
        attachments: (message.images?.length ?? 0) + (message.files?.length ?? 0),
        clientMessageId: message.clientMessageId,
        resend: { text: message.text, images: message.images, files: message.files },
      } });
      return receiptId;
    },
    mutationFn: async ({ message }: SendRequest) => {
      let res;
      try {
        res = await api.POST("/api/sessions/{source}/{id}/message", {
          params: { path: { source, id } },
          body: {
            text: message.text,
            ...(message.images?.length ? { images: message.images } : {}),
            ...(message.files?.length ? { files: message.files } : {}),
            model: activeModel.id,
            // Omitted, not null: a model with no effort ladder refuses `--effort`
            // outright, and the flag is only built when a value is present.
            ...(activeEffort ? { effort: activeEffort } : {}),
            permission: activePermission,
            clientMessageId: message.clientMessageId,
          },
          signal: timeoutSignal(),
        });
      } catch (error) {
        // No answer at all. It may well have arrived; it is not a failure.
        throw new SendUnknown(thrownFailure(error));
      }
      if (!res.response.ok) {
        const failure = { kind: "http" as const, status: res.response.status, body: res.error };
        if (classifySendFailure(failure) === "unknown") throw new SendUnknown(failure);
        // ApiError, not Error: an `external_session` refusal carries the sentence
        // the block below shows.
        throw new ApiError(res.error);
      }
      return res.data;
    },
    onSuccess: (data, request, receiptId) => {
      const outcome = sentOutcome(data);
      if (receiptId) {
        // Taken as the answer to the question the agent was blocked on. The
        // question card is where it now shows — "answered", with these words
        // — and the transcript will never echo it as a message of its own, so
        // a preview here would sit waiting for an echo that is not coming.
        // Anything else is an ordinary accepted message, queued or not —
        // including one the notice promised as an answer and the server could
        // not take as one.
        if (outcome === "answered") delivery.change({ type: "dismiss", id: receiptId });
        else delivery.change({ type: "accepted", id: receiptId, queueId: data?.qid });
      }
      // The question this screen thought was open is not: answered from
      // another screen, stopped, or still being reconnected. Ask for the state
      // now rather than at the next frame, so the card settles, the notice goes,
      // and the message shows in the queue it actually joined.
      if (outcome === "queued instead") onQueueChanged();
      setQueue(afterSend(queueRef.current, request.message.clientMessageId, "accepted"));
      // Transcript and injection state arrive over the session websocket.
    },
    onError: (error, request, receiptId) => {
      if (receiptId) delivery.change({ type: "failed", id: receiptId });
      const outcome = error instanceof SendUnknown ? "unknown" : "refused";
      // Refused means nothing was enqueued, so a typed message goes back where
      // it was typed. Unknown means it may have been, so it stays in the outbox
      // — never in the composer, where sending it again would be a new message.
      const restored = outcome === "refused" && request.typed === true && restoreTyped(request.message);
      setQueue(afterSend(queueRef.current, request.message.clientMessageId, outcome, restored));
    },
  });

  // A refused typed message, back into a composer nobody has typed into since.
  // Text only: attachments live in the outbox entry, which then stays.
  const restoreTyped = (message: OutgoingMessage): boolean => {
    if (message.images?.length || message.files?.length) return false;
    try {
      if (readSessionDraftSnapshot(localStorage, draftKey).text !== "") return false;
    } catch {
      return false;
    }
    setText(message.text);
    return true;
  };

  const { check: checkReceipts, checking: checkingReceipts } = useOutboxResolution({
    source,
    id,
    outbox: queue,
    outboxRef: queueRef,
    setOutbox: setQueue,
    receipts: detail.receipts,
    // The server had it after all. One still waiting in its queue is shown by
    // that queue and then by the preview; one already handed over is in the
    // transcript, where a preview would only wait for an echo that has passed.
    onAdopted: ({ message, receipt }) => {
      if (receipt.status !== "queued") return;
      delivery.change({ type: "start", entry: {
        id: nextDeliveryId(), text: message.text, after: detail.totalItems, sending: false,
        attachments: (message.images?.length ?? 0) + (message.files?.length ?? 0),
        clientMessageId: message.clientMessageId, receipt: receipt.status,
        ...(receipt.qid ? { queueId: receipt.qid } : {}),
        resend: { text: message.text, images: message.images, files: message.files },
      } });
    },
  });

  // An `external_session` refusal describes the moment it was made, not the
  // session, and a spent one would keep the read-only block over a composer that
  // can send again. Dropping it when the frame stops saying external covers the
  // session released by whoever owned it.
  const scope = detail.summary.scope;
  const resetSend = send.reset;
  useEffect(() => {
    if (scope !== "external") resetSend();
  }, [scope, resetSend]);

  const interrupt = useMutation({
    mutationFn: async () => {
      const res = await api.POST("/api/sessions/{source}/{id}/interrupt", {
        params: { path: { source, id } },
      });
      if (res.error) throw new Error(res.error.error);
    },
  });
  const [codexSteering, setCodexSteering] = useState(false);

  const injectionRunning = detail.injection?.state === "running";
  // Legacy Codex exec runs inherited during an upgrade still need a
  // two-step handoff: end that process, then let the ordinary queue
  // drain start a continuation against the same thread. Keep the requested
  // state until the running frame actually clears; the interrupt route returns
  // before the child has necessarily exited. App Server sessions accept sends
  // while busy and use the native steer action in ServerQueue instead.
  useEffect(() => {
    if (!injectionRunning) setCodexSteering(false);
  }, [injectionRunning]);
  // The resident is asked first and the injection record second. For a claude
  // session they are the same fact seen twice, but only the resident sees the
  // half a turn spends `starting`, and only the injection record covers codex,
  // which has no resident at all. Either one saying "running" is running — which
  // is the whole of the "it said the task finished while it was thinking" bug.
  const banner = activityBanner(detail.summary, injectionRunning);
  const interrupted = detail.injection?.state === "failed" && detail.injection.interrupted;
  const failed = injectionFailurePausesQueue(detail.injection);
  // A message that failed on the local login, while that login is signed out or
  // resting (or the failure says it was the credential): the session cannot go
  // on as it is, so the failure line offers to carry it onto an account.
  const { data: accountList } = useAccounts();
  const localLogin = localLoginOf(accountList, "claude");
  const continueOffered =
    failed &&
    !!onOpenSession &&
    source === "claude" &&
    detail.claudeRoot === "local" &&
    detail.summary.scope === "owned" &&
    (localLoginSignedOut(localLogin) || activeRests(localLogin ?? undefined).length > 0 || looksLikeLoginFailure(detail.injection?.error));
  // An actual failure pauses the queue so it can be reviewed. A user interrupt
  // does not: abandoning the current turn is often done specifically so the
  // correction already queued behind it can run. A POST refusal still pauses,
  // because the level-triggered drain would otherwise retry it forever.
  // A send with no answer is not a failure: it holds the queue as `unknown`
  // (see `headBlocked` below) without calling the queue paused.
  const sendUnknown = send.error instanceof SendUnknown;
  const sendRefused = send.isError && !sendUnknown;
  const queuePaused = (failed || sendRefused) && queue.length > 0;
  const queueHeadAction = localQueueHeadAction({
    source,
    canSend: detail.canSend,
    injectionRunning,
    sending: send.isPending,
    draftPending,
    steering: codexSteering,
  });
  // The agent is blocked on a question, so what goes in next is its answer:
  // the server takes a message sent now as the question's free-text answer
  // rather than queueing it behind the turn that is waiting for it. The notice
  // says so whenever that is true; the button and the placeholder only when
  // *this* message is the one that will go — one typed behind messages this tab
  // is still holding goes after them, and the first of those is the answer.
  //
  // Only the words (and attachments) go in as the answer. The model, effort and
  // permission pickers below stay as they are and are still sent, but the
  // server sets them aside for an answer: they describe a process, and the one
  // answering is blocked mid-turn and cannot change. They take effect with the
  // next message that starts a turn, which is why nothing here claims more.
  const answering = composerAnswersQuestion({ questions: detail.questions, row: detail.summary });
  const sendsAnswer = answering && detail.canSend && queue.length === 0;
  const willQueue =
    !sendsAnswer &&
    willQueueSend({
      canSend: detail.canSend,
      localQueue: queue.length,
      row: detail.summary,
      injectionRunning,
    });
  // The floating panel over the transcript names every live child, so the
  // strip's own one-liner would be the same sentence twice on one screen. It
  // keeps the line only for the case the panel stays away from: a turn in
  // flight whose children the loaded window cannot see.
  const panel = backgroundPanel({ row: detail.summary, subtasks, injectionRunning });
  const subtask = banner && !panel ? (subtasks.at(-1) ?? null) : null;
  // `injection.startedAt` has been on the wire since injections existed and
  // nothing had ever read it; it is the only clock that knows when the *user's
  // message* went in rather than when the transcript happened to gain a line.
  const injectionStartedAt = detail.injection?.startedAt;
  const progress = useMemo(
    () => turnProgress(detail.items, injectionStartedAt),
    [detail.items, injectionStartedAt],
  );

  // Guards the window between `mutate` being called and `isPending` becoming
  // observable. The drain below is level-triggered, so that window is a render
  // it could fire in twice — and because `dispatchNext` marks the head as
  // sending synchronously, the second shot would not be a duplicate but
  // something worse: message #2 overtaking #1 on the wire.
  const dispatching = useRef(false);
  // The entry stays in the outbox, marked `sending`, until the server answers:
  // a page closed mid-send then knows to ask about it, rather than forgetting it.
  const dispatchEntry = (entry: OutgoingMessage | undefined) => {
    if (dispatching.current || entry === undefined || !detail.canSend || send.isPending || draftPending) {
      return;
    }
    // `unsent` keeps its id, `failed` gets a new one; see `prepareResend`.
    const message = prepareResend(entry);
    dispatching.current = true;
    setQueue(queueRef.current.map((queued) =>
      queued.clientMessageId === entry.clientMessageId ? { ...message, state: "sending" as const } : queued));
    send.mutate(
      { message },
      {
        onSettled: () => {
          dispatching.current = false;
        },
      },
    );
  };
  const dispatchNext = () => dispatchEntry(queueRef.current[0]);

  // Drain the local queue whenever it *can* be drained, rather than on the
  // moment the session becomes sendable. See `queue-drain.ts`: the edge this
  // used to wait for stopped happening the day a busy resident started
  // accepting messages, and a queued message would then sit there until
  // somebody pressed ▶.
  useEffect(() => {
    if (
      shouldDrainQueue({
        queued: queue.length,
        canSend: detail.canSend,
        failed,
        sendFailed: sendRefused,
        sending: send.isPending,
        draftPending,
        // Never past a message whose fate is unknown, or one waiting on a decision.
        headBlocked: !headSendable(queue),
      })
    ) {
      dispatchNext();
    }
  });

  const { anyUploading, anyFailed } = attachmentState;

  const submit = () => {
    const submittedText = text;
    const message = submittedText.trim();
    if (
      (!message && attachments.length === 0) ||
      send.isPending ||
      draftPending ||
      readOnlyCopy ||
      anyFailed
    ) {
      return;
    }
    if (anyUploading) {
      // Uploads are already racing; fire the message the moment they land.
      setSendQueued(true);
      return;
    }
    setSendQueued(false);

    const { images, files } = attachmentPaths(attachments);
    attachmentState.clear();
    // Named here, once: every retry of this message is the same message.
    const out: OutgoingMessage = {
      clientMessageId: newClientMessageId(),
      text: message || t("Please look at the attached file(s)."),
      images: images.length > 0 ? images : undefined,
      files: files.length > 0 ? files : undefined,
    };
    // The outbox holds it from here on, so the draft goes: a reload mid-send
    // must find it in one place, not in the outbox *and* the composer.
    setText("");
    // Busy, or already queued messages waiting: preserve order by appending.
    if (!detail.canSend || queue.length > 0) {
      setQueue([...queueRef.current, out]);
      return;
    }
    setQueue([...queueRef.current, { ...out, state: "sending" }]);
    send.mutate({ message: out, typed: true });
  };

  useEffect(() => {
    if (!sendQueued) return;
    if (anyFailed) {
      setSendQueued(false);
      return;
    }
    if (!anyUploading) submit();
  }); // runs each render; guarded by sendQueued

  if (otherDevice) {
    return (
      <div data-testid="composer-read-only" className="pb-safe shrink-0 border-t border-border bg-panel p-3">
        <p className="font-mono text-2xs leading-4 text-muted">{t("read-only here: this session is on another device")}</p>
      </div>
    );
  }
  const block = externalSendBlock({
    scope: detail.summary.scope,
    sendBlockReason: detail.sendBlockReason,
    refusal: send.error instanceof ApiError && send.error.code === EXTERNAL_SESSION ? send.error : null,
  });
  if (block) {
    return (
      <div className="pb-safe shrink-0 border-t border-border bg-panel p-3">
        <p data-signal="warn" className="font-mono text-2xs leading-4 text-warn-fg">
          {block.reason ?? t("this session belongs to another instance")}
        </p>
      </div>
    );
  }

  return (
    <div className="pb-safe shrink-0 border-t border-border bg-panel">
      <ActivityBar
        banner={banner}
        subtask={subtask}
        progress={progress}
        drafts={drafts}
        stopping={interrupt.isPending}
        onStop={bannerCanStop(banner) ? () => interrupt.mutate() : undefined}
      />
      <AnswerNotice show={answering} />
      <AwaitingBackgroundNotice resident={residentOf(detail.summary)} />
      <RotationNotice source={source} id={id} rotation={detail.rotation} />
      {interrupted && (
        <div className="border-t border-border px-4 py-1 font-mono text-2xs text-muted">
          {t("message cancelled")}
        </div>
      )}
      {failed && detail.injection?.error && (
        <div
          data-signal="failed"
          className="border-t border-border px-4 py-1 font-mono text-2xs whitespace-pre-wrap text-danger shadow-[inset_2px_0_0_var(--color-danger)]"
          title="last message failed"
        >
          {t("send failed:")} {detail.injection.error}
          {continueOffered && <ContinueOnAccountButton id={id} onOpenSession={onOpenSession} />}
        </div>
      )}
      {sendRefused && (
        // `errorText`, not `.message`: an `ApiError`'s message is the bare
        // machine token, so a refused send used to read `external_session` at a
        // user while the server had written a whole sentence for them.
        <div data-signal="failed" className="border-t border-border px-4 py-1 font-mono text-2xs whitespace-pre-wrap text-danger shadow-[inset_2px_0_0_var(--color-danger)]">
          {t("send failed:")} {errorText(send.error)}
        </div>
      )}
      {sendUnknown && queue.some((message) => message.state === "unknown") && (
        <div data-signal="unknown" className="border-t border-border px-4 py-1 font-mono text-2xs text-warn-fg shadow-[inset_2px_0_0_var(--color-warn)]">
          {t("no answer from the server — the message is kept below until it is clear whether it arrived")}
        </div>
      )}
      {/* The server's queue, then this tab's. They are drawn apart and named
          differently on purpose: everything below has been accepted and will
          run whether or not this browser is open, and everything above is
          still only a note to self. The order between them is also the order
          they run in — the local queue drains into the server one. */}
      <DeliveryPreviews
        messages={delivery.previews}
        // Dropped by the server: back into the queue as a new message, since
        // the old id already has an answer on file.
        onRequeue={(preview) => {
          if (!preview.resend) return;
          setQueue([...queueRef.current, { ...preview.resend, clientMessageId: newClientMessageId() }]);
          delivery.change({ type: "dismiss", id: preview.id });
        }}
        onDismiss={(previewId) => delivery.change({ type: "dismiss", id: previewId })}
      />
      <ServerQueue
        source={source}
        id={id}
        resident={residentOf(detail.summary)}
        receipts={detail.receipts}
        onChanged={onQueueChanged}
        onDropped={(queueId) => delivery.change({ type: "drop", queueId })}
      />
      <OutboxList
        queue={queue}
        paused={queuePaused}
        steering={codexSteering}
        headAction={queueHeadAction}
        checking={checkingReceipts}
        onSendHead={dispatchNext}
        onSteerHead={() => {
          setCodexSteering(true);
          interrupt.mutate(undefined, { onError: () => setCodexSteering(false) });
        }}
        onResend={(message) => {
          // The head goes now, past a refusal's pause, as ▶ always did; one
          // further back waits its turn as an ordinary queued message.
          if (message === queueRef.current[0]) dispatchEntry(message);
          else setQueue(queueRef.current.map((queued) => (queued === message ? prepareResend(message) : queued)));
        }}
        onCheck={checkReceipts}
        onRemove={(message) => setQueue(withoutMessage(queueRef.current, message.clientMessageId))}
      />
      {anyFailed && (
        <div data-signal="failed" className="border-t border-border px-4 py-1 font-mono text-2xs text-danger shadow-[inset_2px_0_0_var(--color-danger)]">
          {attachments.find((a) => a.error)?.error ?? t("upload failed")} — {t("remove the failed attachment to send")}
        </div>
      )}
      <div className="px-3 pt-0 pb-3 max-md:px-2 max-md:pb-2">
        <ComposerBox
          attachments={attachmentState}
          allowAttach={allowAttach}
          text={text}
          onText={setText}
          onTextLive={setTextState}
          disabled={draftPending || readOnlyCopy}
          onSubmit={submit}
          placeholder={
            readOnlyCopy
              ? (detail.sendBlockReason ?? t("the device is offline — this is a read-only copy"))
              : detail.canSend
                ? `${sendsAnswer ? t("type your answer…") : t("send a message to this session…")} (${sendHint})`
                : `${detail.sendBlockReason ?? t("session is busy")} — ${t("messages are queued")}`
          }
          chips={<>
            <ComposerChoiceChips source={source} choice={choice} permission={!chat} />
            <ComposerQuota source={source} accountId={detail.account?.id ?? null} />
            <ContextRing used={detail.contextTokens} contextWindow={detail.contextWindow} />
            <ChecklistChip source={source} id={id} toolTick={toolTick} />
          </>}
          send={{
            disabled: send.isPending || draftPending || readOnlyCopy || anyFailed || (!text.trim() && attachments.length === 0),
            label: sendsAnswer ? t("send as your answer") : willQueue ? t("add to queue") : t("send"),
            busy: send.isPending || draftPending || sendQueued,
            queues: willQueue,
          }}
        />
      </div>
    </div>
  );
}

export function SessionDetail() {
  // The home's `/sessions/$source/$id` or another machine's (`/m/$machine/…`): the same page.
  const { source, id } = useParams({ strict: false }) as { source: string; id: string };
  const navigate = useMachineNavigate();
  const machine = useMachine();
  return (
    <SessionDetailInner
      key={sessionKey({ machine, source, id })}
      source={source}
      id={id}
      onArchived={() => void navigate({ to: "/sessions" })}
      onOpenSession={(next, nextId) => void navigate({ to: "/sessions/$source/$id", params: { source: next, id: nextId } })}
    />
  );
}

/**
 * Separator for the running-task key below.
 *
 * A `tool_use` id is `toolu_` and base62, so any punctuation would do; a
 * character that cannot occur in one at all is the version that stays correct
 * if claude ever changes the shape.
 */
const TASK_ID_SEP = "\u0000";

/** The empty list handed out when a departed process took its children with it. */
const NO_SUBTASKS: LiveSubtask[] = [];

/** The session facts the header keeps behind a chip, already formatted. */
export type SessionMetaFacts = {
  /** Absolute working directory, or null when the session reports none. */
  cwd: string | null;
  /** When it last moved, empty when the server sent no usable stamp. */
  updated: string;
  /** Transcript size on disk, empty when unknown. */
  size: string;
  /** Everything the transcript holds, not just the loaded window. */
  items: number;
  /** How much of that the loaded window is missing; 0 means all of it is here. */
  omitted: number;
};

/**
 * Read the header's secondary facts off a session, in one place that can be
 * tested without a DOM.
 *
 * Formatting is the whole job — every field here used to be interpolated into
 * one line of the header. The counts arrive from a paging window that can
 * briefly disagree with itself (a delta can land before the page that explains
 * it), so a negative "omitted" is a state this has to survive rather than
 * print.
 */
export function sessionMetaFacts(input: {
  cwd?: string | null;
  updatedAt: string | null;
  sizeBytes: number | null;
  totalItems: number;
  omittedItems: number;
}): SessionMetaFacts {
  return {
    cwd: input.cwd ? input.cwd : null,
    updated: formatTime(input.updatedAt),
    size: formatBytes(input.sizeBytes),
    items: Math.max(0, input.totalItems),
    omitted: Math.max(0, input.omittedItems),
  };
}

/** One `label   value` line of the session-details popover. */
function MetaRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline gap-2 font-mono text-2xs">
      <span className="shrink-0 text-faint">{label}</span>
      <span className="min-w-0 flex-1 text-right break-all text-muted">{value}</span>
    </div>
  );
}

/**
 * The session's reference facts, one tap away instead of two lines down.
 *
 * The header used to spend two of its lines on the working directory and a
 * time·size·count run-on — on a phone that is half the chrome above a
 * transcript, for facts nobody reads twice. The count stays on the chip itself
 * because it is the one number that says how much history is above the fold;
 * the rest opens on demand, with the path copyable, since a truncated path is
 * useless and this one was always truncated.
 *
 * Deliberately not the same popover as the drift mark beside it: a downgrade is
 * a warning about what is running now, this is reference about what the session
 * is, and folding both into one chip would bury the warning.
 */
function SessionMetaChip({ facts }: { facts: SessionMetaFacts }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const cwd = facts.cwd;

  const copy = async (path: string) => {
    try {
      await navigator.clipboard.writeText(path);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // No clipboard permission (or no secure context): the path in the popover
      // is selectable text, which is the fallback that always works.
    }
  };

  return (
    <Popover>
      <PopoverTrigger
        title={t("session details")}
        aria-label={t("session details")}
        className="flex h-5 shrink-0 items-center gap-1 px-1.5 font-mono text-2xs text-faint hover:bg-hover hover:text-fg"
      >
        <Info className="size-3 shrink-0" />
        <span className="tabular-nums">{formatCompact(facts.items)}</span>
      </PopoverTrigger>
      <PopoverContent side="bottom" className="w-72 max-w-[calc(100vw-2rem)]">
        <div className="flex flex-col gap-2">
          {cwd && (
            <div className="flex flex-col gap-1">
              <span className="font-mono text-2xs text-faint">{t("working directory")}</span>
              <div className="flex items-start gap-1.5">
                <span className="min-w-0 flex-1 font-mono text-2xs break-all text-fg select-all">
                  {cwd}
                </span>
                <button
                  type="button"
                  title={copied ? t("copied") : t("copy")}
                  aria-label={copied ? t("copied") : t("copy")}
                  onClick={() => void copy(cwd)}
                  className="flex size-5 shrink-0 items-center justify-center rounded text-muted hover:bg-hover hover:text-fg"
                >
                  {copied ? <Check className="size-3" /> : <Copy className="size-3" />}
                </button>
              </div>
            </div>
          )}
          <div className="flex flex-col gap-0.5">
            {facts.updated && <MetaRow label={t("updated")} value={facts.updated} />}
            {facts.size && <MetaRow label={t("size")} value={facts.size} />}
            <MetaRow
              label={t("items")}
              value={
                facts.omitted > 0
                  ? `${formatInt(facts.items)} (${t("{{n}} omitted", { n: facts.omitted })})`
                  : formatInt(facts.items)
              }
            />
          </div>
        </div>
      </PopoverContent>
    </Popover>
  );
}

/** The three numbers the scroll decisions are made from, off a live element. */
function boxOf(el: HTMLElement): ScrollBox {
  return {
    scrollTop: el.scrollTop,
    scrollHeight: el.scrollHeight,
    clientHeight: el.clientHeight,
  };
}

/**
 * What the transcript's row list is rebuilt for.
 *
 * The dependency array of the `rows` memo below, as a value rather than as a
 * literal, because this is the half of that memo which can be wrong: a memo
 * over a list of elements is only worth having if the things it is keyed on
 * hold still between frames, and an input the builder reads but this does not
 * list is a stale row on screen. Written out here so `web/test` can hold both
 * ends of a `state` frame against it and assert that every entry is the same
 * object afterwards — there is no DOM renderer in this project, so a
 * component's *second* render is not otherwise observable from a test.
 *
 * Everything listed is something a frame changes and nothing is something a
 * render creates. `chunks` is memoized on the same `items` array this reads,
 * and `subagentSession` on the route.
 *
 * What is *not* listed matters as much. `canSend` and `sendBlockReason` used to
 * be, for a question card that answered through the composer; they flip at the
 * start and end of every turn, and each flip rebuilt every row in the window.
 * A question card now answers into the turn itself and finds its record
 * through [`QuestionsContext`], which reaches the one card it concerns without
 * touching this memo at all.
 */
export function transcriptRowDeps(
  chunks: Chunk[],
  transcriptStyle: TranscriptStyle,
  data: Pick<FeedState, "items" | "startIndex"> | null,
  subagentSession: SubagentCtx | undefined,
): readonly unknown[] {
  return [chunks, transcriptStyle, data?.items, data?.startIndex, subagentSession];
}

export function SessionDetailInner({
  source,
  id,
  chat = false,
  onArchived,
  paneActions,
  onOpenSession,
}: {
  source: string;
  id: string;
  /** Instant-chat presentation: back goes to /chat, cwd and permission hidden. */
  chat?: boolean;
  /** Called after a successful archive; omitting it hides the archive action. */
  onArchived?: () => void;
  /** Split-pane controls share the title row, including during loading/errors. */
  paneActions?: ReactNode;
  /**
   * Show another session in this one's place — where a session stuck on the
   * local login continues on an account. Omitting it hides that action.
   */
  onOpenSession?: OpenSession;
}) {
  const { t } = useTranslation();
  const api = useApi();
  const machine = useMachine();
  const session = useSessionRef(source, id);
  // Another device's session in the local app, from its copy (S1-7): read-only here.
  const readOnly = useInfoOf(machine).readOnly === true;
  // A transcript open is the machine itself in use: live, not only its copy.
  useEnsureLive();
  const scrollRef = useRef<HTMLDivElement>(null);
  // The one box that holds every row, and the only thing whose height changing
  // means "the transcript got taller" — the panel and the load-earlier control
  // are siblings of it inside the same scroller.
  const rowsRef = useRef<HTMLDivElement>(null);
  const prependAnchor = useRef<{
    startIndex: number;
    scrollHeight: number;
  } | null>(null);
  // Stick to the newest messages until the user scrolls up to read history.
  const stickToBottom = useRef(true);
  // Where the last scroll event left the view. `pinAfterScroll` compares
  // against it to tell a user scrolling up from the bottom growing downwards.
  const lastScrollTop = useRef(0);
  const unreadCursor = useRef<UnreadCursor>(NO_UNREAD);
  // Which window the automatic top-up below has already been fired for, so a
  // page the reducer refuses cannot be asked for again on the next render.
  // See [`shouldTopUpHistory`], which owns the reasoning.
  const autoLoadedFrom = useRef(-1);
  // Whether a page of history was loading as of the last layout pass, so the
  // pass where it stops can settle the pin.
  const loadingWas = useRef(false);
  // Mirrors of the two refs above for rendering the floating button. Kept as
  // refs *and* state so a socket frame that changes neither costs no render.
  const jumpShownRef = useRef(false);
  const [jumpShown, setJumpShown] = useState(false);
  const [unread, setUnread] = useState(0);

  const { data, drafts, isError, connection, loadEarlier, isLoadingEarlier, historyError, refreshState } =
    useSessionFeed(source, id, stickToBottom);
  // Messages this tab sent that the transcript has not shown yet.
  const pendingSends = usePendingDeliveries(sessionKey(session));
  const isPending = !data && !isError;
  // The global feed carries the override-applied title, so the header updates
  // reactively after a rename without re-fetching the per-session summary.
  //
  // One string, through a selector, rather than the whole session array. This
  // component is the reason the feed is an external store at all: every mounted
  // transcript used to re-render on every `sessions:delta` from any session on
  // the machine — panes on screens the user had switched away from, sessions
  // idle for hours — because a changed context value re-renders every consumer
  // whatever `memo` says. Selecting a string means the delta wakes this hook,
  // compares two equal strings, and stops there.
  const feedTitle = useFeedSessionTitle(session);
  // Read means *being read*: the pane has the reader's focus (always true
  // outside a split) and the transcript is at its newest message. Scrolled up
  // into history, or in a pane beside the one being typed in, new answers stay
  // unread — and the pane's frame says so — until the reader gets to them.
  const seat = usePaneSeat();
  const [atBottom, setAtBottom] = useState(true);
  const atBottomRef = useRef(true);
  useEffect(() => {
    seat?.reportAtBottom(atBottom);
  }, [seat, atBottom]);
  // A failed archive used to raise `window.alert`, which is a modal the user has
  // to dismiss before they can even read the transcript behind it — and on iOS
  // standalone it is the one piece of chrome that says "web page". The page has
  // notice strips already; this is one of them.
  const [actionError, setActionError] = useState<string | null>(null);
  // Which rows are waiting on a stop, keyed by the CLI's `task_id`.
  //
  // A set rather than the mutation's own `isPending` because several children
  // can be stopped in a row and only the ones asked about should spin. Cleared
  // by the task leaving the resident's list, which is also what removes the row
  // — so an entry that outlives its task is harmless.
  const [stoppingTasks, setStoppingTasks] = useState<ReadonlySet<string>>(new Set());
  const stopTask = useMutation({
    // The summary's own `source`, for the same reason `archive` takes one: this
    // component's prop is a bare string off the route.
    mutationFn: async (target: { source: Source; taskId: string }) => {
      const res = await api.POST("/api/sessions/{source}/{id}/tasks/{taskId}/stop", {
        params: { path: { source: target.source, id, taskId: target.taskId } },
      });
      if (res.error) throw new ApiError(res.error);
    },
    onSettled: (_data, _error, target) => {
      // Off on failure as well as success: a spinner that never stops is a
      // worse report of "that did not work" than the row simply staying put.
      setStoppingTasks((prev) => {
        const next = new Set(prev);
        next.delete(target.taskId);
        return next;
      });
    },
  });

  const archive = useMutation({
    mutationFn: async (archiveSource: Source) => {
      const res = await api.POST("/api/sessions/{source}/{id}/archive", {
        params: { path: { source: archiveSource, id } },
      });
      if (res.error) throw new ApiError(res.error);
    },
  });
  const [renameOpen, setRenameOpen] = useState(false);
  const { data: prefs } = useQuery({
    queryKey: mq(machine, "prefs"),
    queryFn: async () => (await api.GET("/api/prefs")).data,
    // The settings page writes through this cache; a refetch per mounted view
    // was a request on every session switch for a value that had not moved.
    staleTime: 5 * 60_000,
  });
  const transcriptStyle: TranscriptStyle = prefs?.transcriptStyle ?? "flat";
  // Typeset unless asked not to: a server from before the pref sends none.
  const renderMath = prefs?.math !== "source";
  // Recomputed only when the window itself changes; item identity is what the
  // renderer consults, so it must come from this exact array.
  const pairing = useMemo(() => pairToolResults(data?.items ?? []), [data?.items]);
  // One scan for the floating panel and the composer strip both, so the two
  // cannot end up with different opinions about which children are still out.
  const scanned = useMemo(() => liveSubtasks(data?.items ?? []), [data?.items]);
  // And neither of them believes a transcript over a process that has gone: a
  // background child belongs to the CLI that launched it, so where this server
  // would be holding that CLI and is not, every child the transcript still
  // shows as unfinished died with it. See [`residentAbsenceIsFinal`], which is
  // careful about when that is knowable at all.
  //
  // The same constant every time rather than a fresh `[]`, so the composer's
  // props do not change identity twice a second for a session with nothing
  // running.
  const subtasks =
    data?.summary && residentAbsenceIsFinal(data.summary) ? NO_SUBTASKS : scanned;
  // Which children the resident is minding, for the ✓/⊘ on each subagent card.
  //
  // Two steps because of what a context costs: a state frame arrives twice a
  // second and rebuilding the set on each one would hand every consumer a new
  // object, which re-renders them all regardless of `memo`. The first memo
  // recomputes freely and yields a *string*; the second only runs when that
  // string changes, which is when a child actually starts or finishes.
  const residentTaskKey = useMemo(
    () => [...runningTaskIds(residentOf(data?.summary ?? { status: "idle" }))].join(TASK_ID_SEP),
    [data?.summary],
  );
  const residentTasks = useMemo<ReadonlySet<string>>(
    () => new Set(residentTaskKey ? residentTaskKey.split(TASK_ID_SEP) : []),
    [residentTaskKey],
  );
  // `status` is a guess derived from file mtimes; `activity` (through the same
  // banner decision the composer draws) is the driver's own answer. The old
  // reading meant that during the exact 37 minutes a session spent working
  // without its status flipping, the newest tool call — "what is it doing right
  // now" — was folded away with the rest.
  //
  // `working` and not merely "there is a banner": the other banner is `queued`,
  // which is reached with the resident at `phase: "idle"` — see `bannerCanStop`,
  // which refuses a Stop for exactly that reason. Everything downstream of this
  // takes it as "a turn is in flight", so counting a queue as one put a spinner
  // on tool calls of a session that had stopped working.
  const live =
    activityBanner(data?.summary ?? { status: "idle" }, data?.injection?.state === "running")?.kind === "working";
  // Stable identity, so the memoized rows below are not handed a fresh object
  // on every frame the socket delivers.
  const subagentSession = useMemo(
    () => (source === "claude" ? ({ source, id } as SubagentCtx) : undefined),
    [source, id],
  );
  // What became of each question the agent asked, for the cards in the
  // transcript. Keyed on the list itself, which `applyStateFrame` holds by
  // identity until a question opens or closes — so a state frame twice a second
  // hands the context the same object, and no card hears about it.
  const questionIndex = useMemo<QuestionIndex>(
    () => ({ session: subagentSession, byToolUse: questionsByToolUse(data?.questions) }),
    [subagentSession, data?.questions],
  );
  // Chunking is pure and depends only on the window, so a state frame — which
  // arrives twice a second and leaves `items` untouched — reuses these objects
  // and every memoized row below bails out instead of reconciling.
  const chunks = useMemo(() => chunkItems(data?.items ?? [], live), [data?.items, live]);

  /**
   * The rows, built once per window instead of once per render.
   *
   * The elements themselves, not just the list: React reconciles a subtree it
   * is handed the *same element objects* for by skipping it entirely, so this
   * memo is what makes a render that did not touch the transcript cost nothing
   * below this point. Without it, every render — a state frame, a resize, a
   * keystroke in the composer — walked the whole window creating an element per
   * chunk and re-running [`ItemBlock`] for each one. `ItemBlock` is
   * deliberately not memoized (it is the context read that lets [`ItemRow`]
   * below it be), so "re-running it" is the real cost, and the window is capped
   * at `LIVE_WINDOW_CAP` rather than at anything a viewport implies, because
   * none of this is virtualized.
   *
   * What it is keyed on is [`transcriptRowDeps`], and that every entry of it is
   * something a *frame* changes rather than something a render creates is the
   * difference between a memo that holds and a memo that is decorative: one
   * callback rebuilt inline in this list would invalidate it on every render
   * and the elements would all be new objects again.
   *
   * The context providers around this in the JSX still reach the rows when
   * their values change, which is what makes bailing out safe: a subtree React
   * skipped is still re-rendered for the consumers of a context that moved, so
   * a new pairing or a task that started reaches the row it belongs to even
   * though the elements did not change.
   */
  const rows = useMemo(() => {
    const startIndex = data?.startIndex ?? 0;
    // Every row carries its absolute index (`data-ti`): the one thing the
    // in-session search needs to find the row a match is drawn in. A wrapper
    // rather than a prop, because a row is a dozen different components.
    const renderChunk = (chunk: Chunk) =>
      chunk.kind === "group" ? (
        <StepGroup
          key={`group:${startIndex + chunk.index}`}
          items={chunk.items}
          base={startIndex + chunk.index}
        />
      ) : (
        <div key={`item:${startIndex + chunk.index}`} data-ti={startIndex + chunk.index}>
          <ItemBlock item={chunk.item} subagentSession={subagentSession} />
        </div>
      );

    if (transcriptStyle !== "bubble") return chunks.map(renderChunk);
    return bubbleRuns(chunks).map((run) => {
      const key = `run:${startIndex + run.chunks[0]!.index}`;
      if (run.role === "system") return run.chunks.map(renderChunk);
      return (
        <BubbleRun key={key} role={run.role} model={run.model} timestamp={firstTimestamp(run.chunks)}>
          {run.chunks.map(renderChunk)}
        </BubbleRun>
      );
    });
  }, transcriptRowDeps(chunks, transcriptStyle, data, subagentSession));

  /** Publish the pin and what arrived while it was off, without idle renders. */
  const publishPin = useCallback((pinned: boolean, box: ScrollBox) => {
    stickToBottom.current = pinned;
    // Mirrored into state only on a change, so the steady stream of pinned
    // publishes while a turn streams in costs no render.
    if (pinned !== atBottomRef.current) {
      atBottomRef.current = pinned;
      setAtBottom(pinned);
    }
    const next = showJumpToBottom(pinned, box);
    if (next !== jumpShownRef.current) {
      jumpShownRef.current = next;
      setJumpShown(next);
    }
    if (pinned && unreadCursor.current.count !== 0) {
      unreadCursor.current = { ...unreadCursor.current, count: 0 };
      setUnread(0);
    }
  }, []);

  /** Put the view back on the newest row and follow it again. */
  const glueToBottom = useCallback(
    (smooth: boolean) => {
      const el = scrollRef.current;
      if (!el) return;
      if (smooth) el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
      else el.scrollTop = el.scrollHeight;
      lastScrollTop.current = el.scrollTop;
      // Pinned from this instant, not when the animation lands: the height can
      // change while it runs, and the observer below has to keep chasing it.
      publishPin(true, { ...boxOf(el), scrollTop: el.scrollHeight });
    },
    [publishPin],
  );

  const beginLoadEarlier = useCallback(
    // `keepPinned` is the automatic top-up below, which runs while the view is
    // still glued to the newest row: a page arriving above must not read as the
    // user walking away from the bottom.
    //
    // Answers the request, or `null` when none went out — which the search's
    // page-in needs to tell "a page is coming" from "nothing will happen".
    (keepPinned = false): Promise<void> | null => {
      const el = scrollRef.current;
      if (!el || !data?.hasMoreBefore || isLoadingEarlier) return null;
      if (!keepPinned) publishPin(false, boxOf(el));
      prependAnchor.current = {
        startIndex: data.startIndex,
        scrollHeight: el.scrollHeight,
      };
      return loadEarlier();
    },
    [data?.hasMoreBefore, data?.startIndex, isLoadingEarlier, loadEarlier, publishPin],
  );

  /**
   * In-session search, over the whole transcript rather than this window. A
   * jump to an older match pages history in through `beginLoadEarlier` — the
   * same anchored request scrolling up makes — so the window stays contiguous
   * and the resume marker is untouched; see `session-search-view.tsx`.
   */
  const unpinForSearch = useCallback(() => {
    const el = scrollRef.current;
    if (el) publishPin(false, boxOf(el));
  }, [publishPin]);
  const search = useSessionSearch({
    source,
    id,
    window: data,
    isLoadingEarlier,
    historyError,
    loadEarlier: beginLoadEarlier,
    scrollRef,
    rowsRef,
    unpin: unpinForSearch,
  });
  const rootRef = useRef<HTMLDivElement>(null);
  useFindShortcut(rootRef, search.openBar, !!data);
  // The way back to where the unread messages begin for a reader arriving at
  // the newest of them. The report their arrival sends is what marks those
  // messages read, so it is also what says where they began (`onArrival`);
  // the jump is the search's.
  const firstUnread = useFirstUnread({
    source,
    id,
    scrollRef,
    rowsRef,
    jumpToItem: search.jumpToItem,
    cancelJump: search.cancelJump,
    itemPaging: search.itemPaging,
  });
  useReportSeen(session, { attentive: seat?.attentive ?? true, atBottom, onArrival: firstUnread.onArrival });

  /** Take the pin back if what took it away left the view at the newest row. */
  const settlePin = useCallback(() => {
    const el = scrollRef.current;
    if (!el || stickToBottom.current) return;
    const box = boxOf(el);
    if (pinOnSettle(false, box)) publishPin(true, box);
  }, [publishPin]);
  // A search that closes: the jump to a match unpinned, and nothing re-pins a
  // view too short to scroll. See `pinOnSettle`.
  const searchWasOpen = useRef(false);
  useEffect(() => {
    if (searchWasOpen.current && !search.open) settlePin();
    searchWasOpen.current = search.open;
  }, [search.open, settlePin]);

  useLayoutEffect(() => {
    stickToBottom.current = true;
    prependAnchor.current = null;
    lastScrollTop.current = 0;
    autoLoadedFrom.current = -1;
    unreadCursor.current = NO_UNREAD;
    jumpShownRef.current = false;
    setJumpShown(false);
    setUnread(0);
  }, [source, id]);

  /**
   * Chase the bottom while the height under it is still settling.
   *
   * A transcript is pinned by assigning `scrollHeight` to `scrollTop`, and the
   * height that lands there is only as good as the moment it was read: an image
   * decodes, a fold opens, a web font swaps, the phone keyboard shortens the
   * box. Each one leaves the view halfway up — and an idle session gets no
   * further frames to re-stick from, because the server only pushes a state
   * frame when the state changed.
   *
   * It never moves a view the user took off the bottom, so prepend anchoring —
   * which only runs while unpinned — cannot collide with it. `pinAfterScroll`
   * is what keeps that line honest: any upward move that is not the browser
   * clamping against shrinking content surrenders the pin on the same frame,
   * before this observer can pull the reader back down.
   */
  // `hasFeed` is the dependency that matters: the transcript is only in the DOM
  // once the first frame has arrived, because the pending branch below renders
  // a spinner and no scroller at all.
  const hasFeed = !!data;
  useEffect(() => {
    const el = scrollRef.current;
    const rows = rowsRef.current;
    if (!hasFeed || !el || !rows) return;
    const observer = new ResizeObserver(() => {
      if (stickToBottom.current) {
        el.scrollTop = el.scrollHeight;
        lastScrollTop.current = el.scrollTop;
      }
      publishPin(stickToBottom.current, boxOf(el));
    });
    // The viewport as well as the content: a phone keyboard opening, or the
    // composer growing a line, shortens the box without touching the rows.
    observer.observe(el);
    observer.observe(rows);
    return () => observer.disconnect();
  }, [hasFeed, publishPin]);

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el || !data) return;

    const cursor = trackUnread(unreadCursor.current, data.endIndex, stickToBottom.current);
    if (cursor.count !== unreadCursor.current.count) setUnread(cursor.count);
    unreadCursor.current = cursor;

    const anchor = prependAnchor.current;
    if (stickToBottom.current) {
      prependAnchor.current = null;
      el.scrollTop = el.scrollHeight;
      lastScrollTop.current = el.scrollTop;
    } else if (anchor) {
      if (data.startIndex < anchor.startIndex) {
        el.scrollTop = scrollTopAfterPrepend(
          el.scrollTop,
          anchor.scrollHeight,
          el.scrollHeight,
        );
        prependAnchor.current = null;
      } else {
        // Ignore live tail growth while an earlier-page request is in flight.
        anchor.scrollHeight = el.scrollHeight;
      }
      lastScrollTop.current = el.scrollTop;
    }
    publishPin(stickToBottom.current, boxOf(el));
    // A page of history that has settled in — or a request that came back with
    // nothing — leaves the view where the anchor put it; if that is the bottom
    // (a short transcript), it is pinned again. See `pinOnSettle`.
    if (loadingWas.current && !isLoadingEarlier) settlePin();
    loadingWas.current = isLoadingEarlier;

    if (
      shouldTopUpHistory({
        hasMoreBefore: data.hasMoreBefore,
        loading: isLoadingEarlier,
        failed: historyError !== null,
        startIndex: data.startIndex,
        alreadyLoadedFrom: autoLoadedFrom.current,
        loadedItems: data.items.length,
        box: boxOf(el),
      })
    ) {
      autoLoadedFrom.current = data.startIndex;
      beginLoadEarlier(true);
    }
    // A transcript too short to scroll never fires a scroll event, and its
    // first unread message may be on screen from the start.
    firstUnread.check();
  }, [source, id, data, isLoadingEarlier, historyError, beginLoadEarlier, publishPin, settlePin, firstUnread.check]);

  if (isPending || isError || !data) {
    return (
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        {paneActions && (
          <SessionHeader
            titleRow={<span className="min-w-0 flex-1 truncate text-xs font-medium">{feedTitle ?? id.slice(0, 8)}</span>}
            paneActions={paneActions}
          />
        )}
        {isPending ? <Spinner className="flex-1" /> : (
          <div data-signal="failed" className="flex flex-1 items-center justify-center font-mono text-xs text-danger">
            {t("session not found")}
          </div>
        )}
      </div>
    );
  }

  const { summary, items } = data;
  const displayTitle = feedTitle ?? summary.title;
  return (
    <div
      ref={rootRef}
      // Esc anywhere in the view closes an open search, except in a text field,
      // where it belongs to the field (the search box handles its own).
      onKeyDown={(e) => {
        if (!search.open || e.key !== "Escape" || e.defaultPrevented || e.nativeEvent.isComposing) return;
        const target = e.target as HTMLElement;
        if (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable) return;
        search.close();
      }}
      className="flex min-h-0 flex-1 flex-col"
    >
      <SessionHeader
        paneActions={paneActions}
        titleRow={<>
          {!paneActions && <MachineLink
            to={chat ? "/chat" : "/sessions"}
            title={t("back")}
            className="flex size-9 shrink-0 items-center justify-center text-muted hover:bg-hover md:hidden"
          >
            <ChevronLeft className="size-4" />
          </MachineLink>}
          <span className="min-w-0 flex-1 truncate text-[13px] font-semibold max-md:text-[15px]" title={displayTitle}>{displayTitle}</span>
          <PaneUnreadPill />
          <StatusBadge session={summary} />
          {!chat && !readOnly && summary.scope === "owned" && <SessionPinButton session={summary} onError={setActionError} />}
          <SearchButton onClick={search.openBar} />
          {!chat && !readOnly && (
            <SessionActionsMenu
              onRename={() => setRenameOpen(true)}
              archiving={archive.isPending}
              onArchive={onArchived ? () => archive.mutate(summary.source, {
                onSuccess: () => onArchived(),
                onError: (err) => setActionError(t("archive failed: {{msg}}", { msg: errorText(err) })),
              }) : undefined}
            />
          )}
        </>}
        metadata={<>
          {readOnly && !paneActions && <DeviceChip machine={machine} />}
          {chat ? (
            <Badge variant={summary.source} className="shrink-0">
              {t("chat")}
            </Badge>
          ) : (
            <SourceIcon source={summary.source} className="text-muted" />
          )}
          {!readOnly && (
            <SessionAccountBadge
              source={summary.source}
              id={id}
              claudeRoot={data.claudeRoot}
              account={data.account}
              scope={summary.scope}
              model={data.cliChoice?.model ?? null}
              rotation={data.rotation}
              onOpenSession={onOpenSession}
            />
          )}
          {summary.gitBranch && (
            <span className="flex min-w-0 items-center gap-1 font-mono text-2xs text-muted" title={summary.gitBranch}>
              <GitBranch aria-hidden="true" className="size-3 shrink-0" />
              <span className="truncate">{summary.gitBranch}</span>
            </span>
          )}
          <ModelDriftBadge source={summary.source} id={id} cliChoice={data.cliChoice} />
          <SessionMetaChip
            facts={sessionMetaFacts({
              // Chat sessions never showed their working directory and still
              // do not: the instant-chat presentation has no project to speak of.
              cwd: chat ? null : summary.cwd,
              updatedAt: summary.updatedAt,
              sizeBytes: summary.sizeBytes,
              totalItems: data.totalItems,
              omittedItems: data.omittedItems,
            })}
          />
        </>}
      />
      <RenameSessionDialog
        target={renameOpen ? { source: summary.source, id, title: displayTitle } : null}
        onOpenChange={(o) => !o && setRenameOpen(false)}
      />
      {/* The transcript below stays exactly as it was while this is up. The
          server broadcasts `Closed` to every feed when it restarts to install
          its own update, and blanking the page on it meant a release wiped
          every open session in every open tab. */}
      {connection !== "live" && (
        <div
          className={cn(
            "flex shrink-0 items-center gap-1.5 border-b border-border px-4 py-1 font-mono text-2xs",
            connection === "gone"
              ? "text-danger shadow-[inset_2px_0_0_var(--color-danger)]"
              : "text-warn-fg shadow-[inset_2px_0_0_var(--color-warn)]",
          )}
          data-signal={connection === "gone" ? "offline" : "reconnecting"}
        >
          {connection === "reconnecting" ? (
            <>
              {/* Amber from the wrapper. A backoff that has given up on a
                  restarting server can sit here for minutes, so this is a
                  long-lived indicator too, not a submit spinner. */}
              <LiveDot />
              {t("connection lost — reconnecting…")}
            </>
          ) : connection === "cached" ? (
            // Drawn from the copy this device kept, before the server has
            // answered: honest about it, and gone with the first frame.
            <>
              <LiveDot />
              {t("showing the copy saved on this device — updating…")}
            </>
          ) : (
            // No retry button: the socket keeps going back on its own, and the
            // only "reconnect now" this component has is `refresh`, which
            // re-attaches by clearing the window first — which would throw away
            // the copy this banner exists to preserve.
            <span className="min-w-0 flex-1">
              {t("this session is no longer available on the server — what is below is the last copy loaded")}
            </span>
          )}
        </div>
      )}
      {/* Not in the floating panel, which is about work that is running *now*
          and is gone by the time you go looking for what a session actually
          did. This is history, so it lives with the rest of the session's
          chrome and is one folded line until it is asked for. */}
      {actionError && (
        <div data-signal="failed" className="flex shrink-0 items-start gap-1.5 border-b border-border px-4 py-1 font-mono text-2xs text-danger shadow-[inset_2px_0_0_var(--color-danger)]">
          <span className="min-w-0 flex-1 break-words">{actionError}</span>
          <button
            type="button"
            aria-label={t("dismiss")}
            onClick={() => setActionError(null)} data-signal="failed" className="flex size-4 shrink-0 items-center justify-center rounded hover:bg-danger/15"
          >
            <X className="size-3" />
          </button>
        </div>
      )}
      <SessionSearchBar search={search} />
      {/* The positioning context for the jump-to-bottom button: it floats over
          the transcript and must stay clear of the composer, so it hangs off
          the scroller's box rather than scrolling with the rows inside it. */}
      <div className="relative flex min-h-0 flex-1 flex-col">
      <div
        ref={scrollRef}
        onScroll={() => {
          const el = scrollRef.current;
          if (!el) return;
          const box = boxOf(el);
          const pinned = pinAfterScroll(stickToBottom.current, box, lastScrollTop.current);
          lastScrollTop.current = box.scrollTop;
          publishPin(pinned, box);
          // Only while the view is off the bottom, which is the only way the
          // top can be what the user is heading for. A pinned view passing
          // through the same coordinates — the jump button animating down from
          // the top of a short window, a stick that landed under 120px — is
          // going the other way, and used to load a page and unpin for it.
          if (!pinned && el.scrollTop < 120) beginLoadEarlier();
          // Scrolling up to the first unread message by hand retires its button.
          firstUnread.check();
        }}
        // `transcript-scroll` turns the browser's own scroll anchoring off:
        // the prepend anchor below is the only thing allowed to compensate for
        // history arriving above the viewport, and two of them double-count.
        // `overflow-x-hidden` is load-bearing, not tidiness. CSS has no state
        // for "scrolls on one axis, visible on the other": the `visible` side
        // computes to `auto`, so `overflow-y-auto` on its own also made this a
        // horizontal scroller, and any one row too wide out of a thousand — a
        // forty-six character MCP tool name, a thumbnail in a narrow split —
        // set the whole stream sliding sideways under a vertical drag. Every
        // row below is built to give instead (truncate, wrap, or scroll inside
        // its own box), so nothing should reach this clip. It is here so the
        // next row that forgets loses its own right edge rather than the
        // transcript's vertical scroll.
        className="transcript-scroll min-h-0 flex-1 overflow-x-hidden overflow-y-auto"
      >
        {/* First child of the scroll container, so `sticky top-0` pins it to
            the top of the transcript wherever the history is scrolled to —
            which is the point: a session minding background work writes
            nothing, so scrolling up to read while you wait would otherwise
            take away the only evidence that anything is still happening. */}
        <BackgroundTaskPanel
          panel={backgroundPanel({
            row: summary,
            subtasks,
            injectionRunning: data.injection?.state === "running",
          })}
          stopping={stoppingTasks}
          onStop={(subtask) => {
            const taskId = subtask.taskId;
            if (taskId === undefined) return;
            // Asked before it is done, because it cannot be undone: a stopped
            // subagent's work is gone, and a stopped shell leaves whatever it
            // had half-written on disk. The question names the child rather
            // than the count, so a mis-tap on a phone is caught by reading it.
            if (!window.confirm(t("Stop the background task \"{{name}}\"? It cannot be resumed.", { name: subtask.text }))) {
              return;
            }
            setStoppingTasks((prev) => new Set(prev).add(taskId));
            stopTask.mutate({ source: summary.source, taskId });
          }}
        />
        {/* The top of an offline copy the plan's retention trimmed: the rest is on the computer (cloud mode only). */}
        {!data.hasMoreBefore && <CopyWindowNotice source={summary.source} id={id} />}
        {data.hasMoreBefore && (
          <>
            <button
              type="button"
              onClick={() => beginLoadEarlier()}
              disabled={isLoadingEarlier}
              className="flex h-7 w-full items-center justify-center gap-1.5 border-b border-border font-mono text-2xs text-faint hover:bg-hover hover:text-muted disabled:pointer-events-none"
            >
              {isLoadingEarlier && <Loader2 className="size-3 animate-spin" />}
              {t(
                isLoadingEarlier
                  ? "loading earlier history…"
                  : historyError
                    ? "retry loading earlier history"
                    : "load earlier history",
              )}
            </button>
            {/* Used to be a `title` on the button, which is a hover — so on the
                phones this app is mostly used from, the reason a page would not
                load was unreachable. */}
            {historyError && (
              <div data-signal="failed" className="border-b border-border px-3 py-1 font-mono text-2xs break-words text-danger">
                {historyError}
              </div>
            )}
          </>
        )}
        <PairingContext.Provider value={pairing}>
        <RunningTasksContext.Provider value={residentTasks}>
        <QuestionsContext.Provider value={questionIndex}>
        <LiveContext.Provider value={live}>
        <MathRendering.Provider value={renderMath}>
        {items.length === 0 && (pendingSends > 0 ? (
          // A first message is on its way in (a session just started, its
          // message pending below): it is starting, not empty.
          <div data-starting="" className="flex items-center justify-center gap-1.5 px-3 py-10 text-center font-mono text-2xs text-faint">
            <span data-signal="working" className="flex shrink-0 text-ok">
              <LiveDot />
            </span>
            <span>{t("starting the session…")}</span>
          </div>
        ) : (
          <div className="flex flex-col items-center gap-1 px-3 py-10 text-center font-mono text-2xs text-faint">
            <span>{t("nothing in this transcript yet")}</span>
            <span className="text-faint/70">{t("send a message below to start the session")}</span>
          </div>
        ))}
        {/* One box around every row, so the observer below can tell "the
            transcript grew" from "the panel above it did". Deliberately not a
            virtual list — dynamic heights, stick-to-bottom and prepend
            anchoring all fight it, and it would take the rows out of the DOM,
            so Ctrl+F would stop finding them. `content-visibility` was the
            cheap half of that trade and it is gone too: see `index.css`.
            What keeps the bill for that finite is `trimLiveWindow`: every row
            here stays laid out, so the number of them has to be bounded by
            something, and it is the window rather than the renderer.
            And the last thing in the scroller: nothing provisional goes below
            it. What the agent is doing and has not written yet — a think in
            flight — is said in the activity bar above the composer, so the
            transcript only grows by written entries and nothing in it changes
            height under the reader while a turn runs (`turn-status.tsx`). */}
        <div ref={rowsRef} className="transcript-rows">{rows}</div>
        </MathRendering.Provider>
        </LiveContext.Provider>
        </QuestionsContext.Provider>
        </RunningTasksContext.Provider>
        </PairingContext.Provider>
      </div>
      {/* Always rendered so it can fade both ways, and `inert` while hidden so
          a button nobody can see is not in the tab order or read out. It only
          appears once the newest row is properly off screen, so it never sits
          over the message it would take you to. */}
      <button
        type="button"
        inert={!jumpShown}
        aria-label={t("jump to the newest messages")}
        title={t("jump to the newest messages")}
        onClick={() => glueToBottom(true)}
        className={cn(
          "absolute right-3 bottom-3 z-30 flex h-8 min-w-8 items-center justify-center gap-1 border border-border-strong bg-bg px-2 text-muted transition-opacity hover:bg-hover hover:text-fg",
          jumpShown ? "opacity-100" : "pointer-events-none opacity-0",
        )}
      >
        <ArrowDown className="size-4 shrink-0" />
        {/* What arrived while you were reading history — the reason to press it. */}
        {unread > 0 && (
          <span className="pr-0.5 font-mono text-2xs text-accent">{unreadLabel(unread)}</span>
        )}
      </button>
      {/* Where this visit's unread messages begin, for a reader who arrived at
          the newest of them. Centred, and narrow enough never to reach the
          jump-to-bottom button in the corner. */}
      <FirstUnreadButton
        view={firstUnread.view}
        paging={firstUnread.paging}
        onJump={firstUnread.jump}
        onCancel={firstUnread.cancel}
        className="absolute bottom-3 left-1/2 z-30 max-w-[calc(100%-10rem)] -translate-x-1/2"
      />
      </div>
      <Composer
        key={`${source}/${id}`}
        source={source as "claude" | "codex"}
        id={id}
        detail={data}
        drafts={drafts}
        subtasks={subtasks}
        chat={chat}
        onQueueChanged={refreshState}
        // Another device shown read-only here takes no action, this one included.
        onOpenSession={readOnly ? undefined : onOpenSession}
      />
    </div>
  );
}
