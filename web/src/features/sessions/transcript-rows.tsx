/*
 * The transcript's rows: one item, a folded group of steps, and a subagent's
 * tool call with its own transcript under it.
 */
import { useQuery } from "@tanstack/react-query";
import { ChevronRight } from "lucide-react";
import { memo, useContext, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ApiError, errorText } from "@/api/errors";
import { FileChip } from "@/components/file-chip";
import { LightboxImage } from "@/components/image-lightbox";
import { LiveDot } from "@/components/live-dot";
import { Markdown } from "@/components/markdown";
import { Spinner } from "@/components/ui/spinner";
import { asyncLaunchAckOf, latchOutcome, subagentOutcome, type AgentOutcome } from "@/features/sessions/session-activity";
import { QuestionCard } from "@/features/sessions/question-card";
import type { SessionQuestion } from "@/features/sessions/session-questions";
import { CommandChip, CompactDivider, ModelFallbackNotice, modelFallbackOf, SyntheticNotice } from "@/features/sessions/transcript-notices";
import { countSteps, foldClassOf, modelOf, pairToolResults } from "@/features/sessions/transcript-groups";
import { cn } from "@/lib/cn";
import { useApi, useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";
import { MessageHead, ROLE_STYLE } from "@/features/sessions/bubble-run";
import { SystemLine } from "@/features/sessions/system-line";
import { DetailPre, ToolLine } from "@/features/sessions/tool-line";
import {
  BubbleModelContext,
  InBubbleContext,
  LiveContext,
  PairingContext,
  QuestionsContext,
  RunningTasksContext,
  StepNumberContext,
  type SubagentCtx,
  type TranscriptItem,
} from "@/features/sessions/transcript-context";

/**
 * A folded run of steps. `base` is the absolute index of its first item: every
 * row is tagged with its own (`data-ti`), so a search can find the row an item
 * landed in even when it is folded away in here.
 */
export const StepGroup = memo(function StepGroup({ items, base }: { items: TranscriptItem[]; base: number }) {
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

/**
 * One agent's own transcript, fetched on demand.
 *
 * Shared by the card a `Task` call draws in the stream and by the list of
 * agents that hang off nothing in it — the second kind is reached only through
 * that list, and giving it a different reader would have been two answers to
 * "what did this agent do".
 */
export function useAgentTranscript(session: SubagentCtx, agentId: string | null, open: boolean) {
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

export function AgentTranscriptBody({
  query,
  session,
  flush = false,
}: {
  query: AgentTranscriptQuery;
  session: SubagentCtx;
  /** The agent's own page rather than a card's drill-down: no indent, no rule. */
  flush?: boolean;
}) {
  const { t } = useTranslation();
  const { data, error, isFetching, refetch } = query;
  // The subagent's transcript is its own window, so it pairs on its own.
  const subPairing = useMemo(() => pairToolResults(data?.items ?? []), [data?.items]);
  return (
    <div className={flush ? undefined : "ml-3 border-l border-accent/30"}>
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
export function ItemBlock(props: ItemBlockProps) {
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
