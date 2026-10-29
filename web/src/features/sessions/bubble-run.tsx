/*
 * A turn drawn as a bubble: its role header, and the run of rows under it.
 */
import { cn } from "@/lib/cn";
import { formatTime } from "@/lib/format";
import { BubbleModelContext, InBubbleContext } from "@/features/sessions/transcript-context";

// Roles are ink caps labels: green belongs to "working", and the user's turns
// are told apart by their raised band, not by a colour.
export const ROLE_STYLE: Record<string, { label: string; className: string }> = {
  user: { label: "USER", className: "text-fg" },
  assistant: { label: "AGENT", className: "text-fg" },
  system: { label: "SYS", className: "text-faint" },
};

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
export function MessageHead({
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
