/*
 * A session being opened with nothing to show yet: the spinner, and once the
 * wait has gone on for a moment, one short line under it saying what it is
 * waiting for, with a thin bar only where there is a real total to measure
 * against. The device's name, the connection's countdown and "retry now" are
 * the cloud bar's, above the view, and are not said again here.
 */
import { Loader2 } from "lucide-react";
import { useRef } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import type { FeedOpening } from "@/features/sessions/session-feed-pool";
import type { MachineInfo } from "@/lib/machines/runtime";
import { useShownAfter } from "@/lib/use-shown-after";

/** The line waits this long (ms): an open that is over by then shows the spinner alone, as it always has. */
export const OPENING_DELAY_MS = 500;
/** Once said, it stays at least this long (ms), so a wait that ends just after does not blink. */
export const OPENING_MIN_MS = 600;

/**
 * What the line says the open waits for, given what the page shows of the
 * device (`MachineInfo.live`). On droppen.app the page draws a device from
 * its offline copy until the end-to-end channel to it is open (`connecting`),
 * and then asks the device itself, so a feed that has asked the copy and not
 * heard back is waiting on that channel as much as on the copy: it says it is
 * connecting. Numbers from the copy say more than that, and are kept.
 */
export function openingShown(opening: FeedOpening, live: MachineInfo["live"]): FeedOpening {
  if (opening.stage === "loading" && opening.total === undefined && live === "connecting") return { stage: "connecting" };
  return opening;
}

/**
 * The line for `opening`. A read's numbers are bytes, which the bar says
 * better than any count would, so only the copy's pages are written out.
 */
export function openingText(opening: FeedOpening, t: TFunction): string {
  switch (opening.stage) {
    case "connecting":
      return t("connecting…");
    case "loading":
      return t("loading session…");
    case "copy":
      return opening.total === undefined
        ? t("from hub…")
        : t("from hub · {{done}}/{{total}}", { done: opening.done ?? 0, total: opening.total });
    case "retrying":
      return t("cannot open · retrying");
  }
}

export function SessionOpening({ opening, live }: { opening: FeedOpening | null; live: MachineInfo["live"] }) {
  const { t } = useTranslation();
  const shown = useShownAfter(opening !== null, OPENING_DELAY_MS, OPENING_MIN_MS);
  // What was last said, for the moment the line outlasts it (`OPENING_MIN_MS`).
  const last = useRef(opening);
  if (opening) last.current = opening;
  const said = shown && last.current ? openingShown(last.current, live) : null;
  const text = said ? openingText(said, t) : "";
  return (
    <div data-session-opening={said?.stage ?? ""} className="flex flex-1 items-center justify-center p-6 text-faint">
      {/* The words hang under the spinner rather than beside it in the flow,
          so the spinner stays exactly where it is when they come and go. */}
      <div className="relative flex">
        <Loader2 className="size-4 animate-spin" />
        {said && (
          <div className="absolute top-full left-1/2 mt-2 flex w-max -translate-x-1/2 flex-col items-center gap-1.5">
            <span data-opening-line="" className="whitespace-nowrap font-mono text-2xs text-muted tabular-nums">
              {text}
            </span>
            {said.total !== undefined && (
              <div
                data-opening-bar=""
                role="progressbar"
                aria-label={text}
                aria-valuemin={0}
                aria-valuemax={said.total}
                aria-valuenow={said.done ?? 0}
                className="h-0.5 w-32 bg-border"
              >
                <div className="h-full bg-fg" style={{ width: `${(100 * (said.done ?? 0)) / said.total}%` }} />
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
