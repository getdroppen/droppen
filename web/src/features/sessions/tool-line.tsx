/*
 * One tool call in the transcript, with what it changed or printed.
 */
import { memo, useContext } from "react";
import { useTranslation } from "react-i18next";
import { LightboxImage } from "@/components/image-lightbox";
import { LiveDot } from "@/components/live-dot";
import { asyncLaunchAckOf } from "@/features/sessions/session-activity";
import { cn } from "@/lib/cn";
import { diffRowsFor, diffStats, DIFF_MAX_ROWS, type DiffRow } from "@/lib/diff";
import { durationBetween, formatDuration, formatExactTime } from "@/lib/format";
import { InBubbleContext, StepNumberContext, type TranscriptItem } from "@/features/sessions/transcript-context";

function detailLineClass(line: string): string | undefined {
  if (line.startsWith("+")) return "text-ok";
  if (line.startsWith("-")) return "text-danger";
  if (line.startsWith("***") || line.startsWith("@@")) return "text-faint";
  return undefined;
}

export function DetailPre({ item }: { item: TranscriptItem }) {
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
