/*
 * A line the session itself wrote into the transcript.
 */
import { useTranslation } from "react-i18next";
import { Markdown } from "@/components/markdown";
import { normalizeTaskStatus } from "@/features/sessions/session-activity";
import { cn } from "@/lib/cn";
import type { TranscriptItem } from "@/features/sessions/transcript-context";

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
