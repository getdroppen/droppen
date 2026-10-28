/**
 * The "jump to the first unread message" button: the half that touches the
 * DOM and the network. The rules are in `first-unread.ts`, where they are
 * tested.
 */
import { ArrowUp, Loader2, X } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useReducer, useRef, type RefObject } from "react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/cn";
import { formatInt } from "@/lib/format";
import {
  FIRST_UNREAD_IDLE,
  firstUnreadQuery,
  firstUnreadReducer,
  firstUnreadView,
  reachedFirstUnread,
  targetOf,
  type FirstUnreadView,
} from "./first-unread";
import type { JumpOutcome } from "./session-search-view";
import { useApi, useMachine } from "@/lib/machines/context";
import { sessionKey } from "@/lib/machines/key";
import { isSessionSource } from "./new-session-flow";

/** How long "could not get there" stays up. */
const FAILED_MS = 3000;

export interface FirstUnreadJump {
  /** What the button draws; `null` while there is none. */
  view: FirstUnreadView | null;
  /** Pages paged in so far while the jump runs, of about `total`. */
  paging: { loaded: number; total: number } | null;
  /** For `useReportSeen`: the mark an arrival's report replaced. */
  onArrival: (previousReadAt: string | null) => void;
  /** Take it. */
  jump: () => void;
  /** Stop the jump in flight. */
  cancel: () => void;
  /**
   * Re-check whether the reader has got there by themselves. Called on every
   * scroll of the transcript and after every layout; cheap when no button is up.
   */
  check: () => void;
}

/** The button's state for one session view. */
export function useFirstUnread(input: {
  source: string;
  id: string;
  scrollRef: RefObject<HTMLDivElement | null>;
  rowsRef: RefObject<HTMLDivElement | null>;
  /** The search's jump: page the item in, scroll to it, flash it. */
  jumpToItem: (index: number, onSettled?: (outcome: JumpOutcome) => void) => void;
  cancelJump: () => void;
  /** The search's progress for that jump. */
  itemPaging: { loaded: number; total: number } | null;
}): FirstUnreadJump {
  const api = useApi();
  const machine = useMachine();
  const { source, id, scrollRef, rowsRef, jumpToItem, cancelJump } = input;
  const [state, dispatch] = useReducer(firstUnreadReducer, FIRST_UNREAD_IDLE);
  const seq = useRef(0);
  const request = useRef<AbortController | null>(null);

  // Another session in the same view: nothing up is about it, and an answer
  // still on its way is about the last one.
  const lastKey = useRef(sessionKey({ machine, source, id }));
  useLayoutEffect(() => {
    const key = sessionKey({ machine, source, id });
    if (key === lastKey.current) return;
    lastKey.current = key;
    request.current?.abort();
    request.current = null;
    seq.current += 1;
    dispatch({ type: "reset" });
  }, [machine, source, id]);
  useEffect(() => () => request.current?.abort(), []);

  const onArrival = useCallback(
    (previousReadAt: string | null) => {
      if (!isSessionSource(source)) return;
      request.current?.abort();
      const controller = new AbortController();
      request.current = controller;
      const at = ++seq.current;
      dispatch({ type: "arrived", seq: at });
      const settle = (answer?: { index: number | null; count: number }) => {
        if (controller.signal.aborted) return;
        request.current = null;
        dispatch({ type: "located", seq: at, target: targetOf(answer) });
      };
      api
        .GET("/api/sessions/{source}/{id}/first-unread", {
          params: { path: { source, id }, query: firstUnreadQuery(previousReadAt) },
          signal: controller.signal,
        })
        .then((res) => settle(res.data), () => settle());
    },
    [source, id],
  );

  const view = firstUnreadView(state);
  const offered = view?.kind === "offer" ? view.target : null;
  const offeredRef = useRef(offered);
  useLayoutEffect(() => {
    offeredRef.current = offered;
  });

  const check = useCallback(() => {
    const target = offeredRef.current;
    const rows = rowsRef.current;
    const scroller = scrollRef.current;
    if (!target || !rows || !scroller) return;
    const row = rows.querySelector<HTMLElement>(`[data-ti="${target.index}"]`);
    // A row with no box is not laid out (a hidden screen): not reached.
    const top = row && row.getClientRects().length > 0 ? row.getBoundingClientRect().top : null;
    if (reachedFirstUnread(top, scroller.getBoundingClientRect().top)) dispatch({ type: "dismiss" });
  }, [rowsRef, scrollRef]);

  // Once it is known where the first unread is, it may already be on screen —
  // a few short answers under a tall viewport — and then there is nowhere to
  // go. A frame later, so the transcript's own pin to the bottom has landed.
  const offeredAt = offered?.index ?? null;
  useEffect(() => {
    if (offeredAt === null) return;
    const frame = requestAnimationFrame(check);
    return () => cancelAnimationFrame(frame);
  }, [offeredAt, check]);

  const failed = state.phase === "failed";
  useEffect(() => {
    if (!failed) return;
    const timer = setTimeout(() => dispatch({ type: "dismiss" }), FAILED_MS);
    return () => clearTimeout(timer);
  }, [failed]);

  const jump = useCallback(() => {
    const target = offeredRef.current;
    if (!target) return;
    dispatch({ type: "jump" });
    jumpToItem(target.index, (outcome) => dispatch({ type: outcome }));
  }, [jumpToItem]);

  return {
    view,
    paging: view?.kind === "jumping" ? input.itemPaging : null,
    onArrival,
    jump,
    cancel: cancelJump,
    check,
  };
}

const PILL =
  "flex h-7 max-w-full items-center gap-1.5 px-2.5 text-xs font-medium";

/**
 * `↑ 12 unread · jump to the first`, floating over the bottom of the
 * transcript in the unread colour. While the jump runs it says so, with a way
 * to stop it; if the jump cannot get there it says that, briefly.
 */
export function FirstUnreadButton({
  view,
  paging = null,
  onJump,
  onCancel,
  className,
}: {
  view: FirstUnreadView | null;
  paging?: { loaded: number; total: number } | null;
  onJump: () => void;
  onCancel: () => void;
  className?: string;
}) {
  const { t } = useTranslation();
  if (!view) return null;
  if (view.kind === "failed") {
    return (
      <div role="status" data-first-unread="failed" data-signal="failed" className={cn(PILL, "border border-danger/55 bg-bg text-danger", className)}>
        <span className="truncate">{t("could not reach the first unread message")}</span>
      </div>
    );
  }
  if (view.kind === "jumping") {
    return (
      <div role="status" data-first-unread="jumping" className={cn(PILL, "bg-unread pr-1 text-bg", className)}>
        <Loader2 aria-hidden="true" className="size-3.5 shrink-0 animate-spin" />
        <span className="truncate">{t("going to the first unread message…")}</span>
        {paging && paging.total > 1 && (
          <span className="shrink-0 font-mono text-2xs tabular-nums opacity-80">
            {paging.loaded}/{paging.total}
          </span>
        )}
        <button
          type="button"
          title={t("stop")}
          aria-label={t("stop")}
          onClick={onCancel}
          className="flex size-5 shrink-0 items-center justify-center hover:bg-bg/20"
        >
          <X className="size-3.5" />
        </button>
      </div>
    );
  }
  const label = t("jump to the first unread message");
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      data-first-unread={view.target.count}
      onClick={onJump}
      className={cn(PILL, "bg-unread text-bg hover:brightness-110", className)}
    >
      <ArrowUp aria-hidden="true" className="size-3.5 shrink-0" />
      <span className="truncate tabular-nums">{t("{{n}} unread · jump to the first", { n: formatInt(view.target.count) })}</span>
    </button>
  );
}
