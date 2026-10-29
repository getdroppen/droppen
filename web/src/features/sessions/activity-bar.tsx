/*
 * The live lines above the composer: what the session is doing now, and why
 * the message at the head of the queue is waiting.
 */
import { Loader2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { LiveDot } from "@/components/live-dot";
import type { ActivityBanner, LiveSubtask, ResidentInfo, TurnProgress } from "@/features/sessions/session-activity";
import type { DraftStore } from "@/features/sessions/stream-draft";
import { TurnStatus } from "@/features/sessions/turn-status";
import { useTicker } from "@/lib/ticker";

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
