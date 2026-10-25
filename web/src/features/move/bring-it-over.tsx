import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import { Note, secondary } from "@/features/cloud/ui";
import { localDevice, watchMoves } from "@/lib/move";
import { onlyUnfinishedHere } from "@/lib/move/receiver";
import { type MoveOutcome, startPull } from "./index";

/*
 * The app host's sign-in screen (its sign-in tab: a new account has nothing
 * to bring over), in a browser that holds no device here
 * (plan §12, user steps 4–5): "Used Droppen at droppen.app in this browser?
 * Bring it over" — the pull (`startPull`): a droppen.app window opens, the
 * person confirms there, and this browser opens here as the same device,
 * with no pairing. Whether droppen.app holds one cannot be seen from here
 * (its storage is its own): the window says so if it does not.
 */

const hostOf = (origin: string) => {
  try {
    return new URL(origin).host;
  } catch {
    return origin;
  }
};

export type BringState = { kind: "idle" } | { kind: "busy" } | { kind: "failed"; message: string };

/** What the offer shows (render-tested). */
export function BringItOverView({ legacy, state, onBring }: { legacy: string; state: BringState; onBring: () => void }) {
  const { t } = useTranslation();
  const host = hostOf(legacy);
  return (
    <Note>
      <div data-testid="bring-it-over-offer" className="flex flex-col gap-2">
        <p className="text-xs leading-5 font-medium text-fg">{t("Used Droppen at {{legacy}} in this browser?", { legacy: host })}</p>
        <p className="text-xs leading-5 text-muted">{t("Bring it over: a {{legacy}} window opens, you confirm there, and this browser opens here as the same device, without adding it again.", { legacy: host })}</p>
        <button type="button" data-testid="bring-it-over" className={secondary} disabled={state.kind === "busy"} onClick={onBring}>
          {state.kind === "busy" && <Loader2 className="size-3.5 animate-spin" />}
          {t("Bring it over")}
        </button>
        {state.kind === "failed" && (
          <p data-testid="bring-it-over-error" role="alert" className="text-xs leading-5 text-danger">
            {state.message}
          </p>
        )}
      </div>
    </Note>
  );
}

/** What a pull's outcome leaves the offer showing (`null`: moved, the page goes on). */
export function afterPull(outcome: MoveOutcome, t: (key: string) => string): BringState | null {
  switch (outcome.kind) {
    case "moved":
      return null;
    case "cancelled":
      return { kind: "idle" };
    case "blocked":
      return { kind: "failed", message: t("The browser blocked the window the move needs. Allow pop-ups for this site, then try again.") };
    case "unavailable":
      return { kind: "failed", message: t("This browser cannot move its keys here.") };
    case "failed":
      return { kind: "failed", message: outcome.message };
  }
}

/**
 * The offer, shown only while this origin holds no device (or only what a
 * move cut off left). `onMoved`: this origin holds the device now and a fresh
 * session — by default the page reloads into the app.
 */
export function BringItOver({ legacy, onMoved = () => location.reload(), pull = startPull }: { legacy: string; onMoved?: () => void; pull?: typeof startPull }) {
  const { t } = useTranslation();
  const [shown, setShown] = useState(false);
  const [state, setState] = useState<BringState>({ kind: "idle" });
  useEffect(() => {
    let live = true;
    void localDevice().then(
      // No device here, or only what a move of one cut off before it signed in: never over another device.
      (here) => live && setShown(here === null || onlyUnfinishedHere(localStorage, here)),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, []);
  if (!shown) return null;
  const bring = () => {
    setState({ kind: "busy" });
    // Straight from the click: the pull opens its window before it waits for anything.
    void pull({ legacy }).then((outcome) => {
      const next = afterPull(outcome, t);
      if (next) setState(next);
      else onMoved();
    });
  };
  return <BringItOverView legacy={legacy} state={state} onBring={bring} />;
}

/**
 * Re-open the account when another tab of this origin took a device over
 * (`arrived`): a tab with no account open reloads, and opens it. One with an
 * account open already keeps it (a move only comes into an origin holding no
 * device, or this very one). Returns a stop.
 */
export function reloadOnArrival(opts: { isOpen: () => boolean; reload?: () => void; watch?: typeof watchMoves }): () => void {
  const reload = opts.reload ?? (() => location.reload());
  return (opts.watch ?? watchMoves)((event) => {
    if (event.t === "arrived" && !opts.isOpen()) reload();
  });
}
