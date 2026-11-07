import { ArrowRightIcon, WarningIcon } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import type { components } from "@/api/schema";
import { Markdown } from "@/components/markdown";
import { cn } from "@/lib/cn";

type TranscriptItem = components["schemas"]["TranscriptItem"];

/**
 * The seam where the CLI threw the conversation away and replaced it with a
 * summary.
 *
 * It is drawn as a rule across the column rather than as a message, because it
 * is not one: nothing above it is in the agent's context any more, and that is
 * the single fact a reader scrolling back needs. Before this existed the
 * boundary was not rendered at all and its summary arrived as an ordinary user
 * bubble — fifteen thousand words of English the user never typed, with nothing
 * saying where they came from.
 *
 * The summary itself is behind a fold. It is worth being able to read (it is
 * the only record of what was dropped) and worth never having to scroll past.
 */
export function CompactDivider({ item }: { item: TranscriptItem }) {
  const { t } = useTranslation();
  const rule = <span className="h-px flex-1 bg-border" aria-hidden="true" />;
  const label = (
    <span className="flex shrink-0 items-baseline gap-2 px-2">
      <span className="font-mono text-2xs tracking-wide text-faint uppercase">{t("context compacted")}</span>
      {item.meta && <span className="font-mono text-2xs text-faint/70">{item.meta}</span>}
    </span>
  );

  if (!item.detail) {
    return <div className="flex items-center px-3 py-2">{rule}{label}{rule}</div>;
  }
  return (
    <details className="px-3 py-2">
      <summary className="flex cursor-pointer list-none items-center hover:opacity-75">
        {rule}
        {label}
        <span className="shrink-0 pr-2 font-mono text-2xs text-faint/70">{t("read the summary")}</span>
        {rule}
      </summary>
      <div className="mt-2 max-h-96 overflow-auto rounded border border-border bg-raised/40 p-2 text-2xs leading-4 whitespace-pre-wrap text-muted">
        {item.detail}
      </div>
      {item.truncated && <span data-signal="warn" className="font-mono text-2xs text-warn-fg">{t("…[truncated]")}</span>}
    </details>
  );
}

/**
 * A slash command the user ran.
 *
 * Styled as a chip rather than a user bubble: `/compact` is an instruction to
 * the CLI, not something said to the agent, and the reply to it is the fold
 * below rather than the message after it. The command keeps its own colour so
 * it can be found while scrolling — the previous rendering was one faint grey
 * monospace line among all the other faint grey monospace lines, which is the
 * complaint this answers.
 */
export function CommandChip({ item }: { item: TranscriptItem }) {
  const { t } = useTranslation();
  const head = (
    <>
      <span className="shrink-0 px-1.5 py-px font-mono text-2xs font-bold text-fg shadow-[inset_0_0_0_1px_var(--color-border-strong)]">
        {item.toolName ?? "/"}
      </span>
      {item.text && <span className="min-w-0 truncate font-mono text-2xs text-muted">{item.text}</span>}
    </>
  );

  if (!item.detail) {
    return <div className="flex items-baseline gap-2 px-3 py-1">{head}</div>;
  }
  return (
    <details className="px-3 py-1">
      <summary className="-mx-1 flex cursor-pointer list-none items-baseline gap-2 rounded px-1 hover:bg-hover">
        {head}
        <span className="shrink-0 font-mono text-2xs text-faint/70">{t("command output")}</span>
      </summary>
      <div className="mt-1 ml-1 border-l border-border pl-2 text-2xs leading-4 whitespace-pre-wrap text-faint">
        {item.detail}
      </div>
    </details>
  );
}

/**
 * `model fallback: claude-fable-5 → claude-opus-4-8`, parsed off a system line.
 *
 * Matched on the first line only, and both arrow spellings, because the server
 * writes the sentence and a body underneath it may quote anything. Returns null
 * for every other system line, which is most of them.
 *
 * The model swapped *from* is optional, because the server's own sentence makes
 * it optional: a `fallback` block that names only its destination is written as
 * `model fallback: → claude-opus-4-8`, and a pattern that insisted on a name
 * before the arrow sent exactly that line — a real, silent model swap — back to
 * being one more grey row, which is the failure this whole notice exists for.
 */
export function modelFallbackOf(text: string): { from: string | null; to: string } | null {
  const line = text.split("\n", 1)[0] ?? "";
  const match = /^\s*model fallback:\s*(?:(\S.*?)\s*)?(?:→|->)\s*(\S.*?)\s*$/i.exec(line);
  if (!match) return null;
  return { from: match[1] ?? null, to: match[2]! };
}

/**
 * The model changed under the session, said out loud.
 *
 * A silent downgrade is the failure this exists for: the answer that comes back
 * is worse and nothing on screen says why, so the reader concludes the agent
 * got worse rather than that it got smaller. Drawn as a notice with the two
 * names side by side, because "which model am I talking to now" is the only
 * question the line has to answer.
 *
 * When the line names no model to swap *from*, the struck-through half is
 * dropped and the notice says where the turn went instead — the destination is
 * the half that answers the question, and an empty box before an arrow would
 * only look like something failed to load.
 */
export function ModelFallbackNotice({
  fallback,
  rowClass,
}: {
  fallback: { from: string | null; to: string };
  rowClass: string;
}) {
  const { t } = useTranslation();
  return (
    <div className={cn(rowClass, "flex flex-wrap items-center gap-1.5 font-mono text-2xs text-warn-fg")}>
      <WarningIcon className="size-3 shrink-0" />
      <span className="shrink-0">{fallback.from === null ? t("switched to") : t("model fallback")}</span>
      {fallback.from !== null && (
        <>
          <span className="min-w-0 truncate text-faint line-through">{fallback.from}</span>
          <ArrowRightIcon className="size-3 shrink-0 text-faint" />
        </>
      )}
      <span className="min-w-0 truncate font-bold">{fallback.to}</span>
    </div>
  );
}

/**
 * A message the CLI wrote in the model's voice.
 *
 * `synthetic` is set when the entry's model was `<synthetic>`: a rate-limit
 * wall, a "Not logged in", an API error phrased as an assistant turn. Rendered
 * as an ordinary bubble these are indistinguishable from the agent talking —
 * "I'm unable to continue" reads as a decision the model made rather than as a
 * wall it hit, and the reader argues with it instead of topping up a quota. So
 * it keeps the Markdown body (the CLI's sentence is worth reading, and some of
 * them carry links) inside notice chrome that says who is really speaking.
 */
export function SyntheticNotice({ item, rowClass }: { item: TranscriptItem; rowClass: string }) {
  const { t } = useTranslation();
  return (
    <div className={cn(rowClass, "border-l-2 border-l-warn/60 bg-warn/5")}>
      <div data-signal="warn" className="mb-0.5 flex items-center gap-1.5 font-mono text-[10px] text-warn-fg">
        <WarningIcon className="size-3 shrink-0" />
        <span>{t("from the CLI, not from the model")}</span>
      </div>
      <Markdown text={item.text} className="text-muted" />
    </div>
  );
}
