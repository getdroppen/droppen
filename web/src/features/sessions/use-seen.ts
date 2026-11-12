import { useEffect, useRef, useSyncExternalStore } from "react";
import { useDebounceCallback, useEventCallback } from "usehooks-ts";
import type { components } from "@/api/schema";
import { feedRow, useGlobalFeedStore } from "@/app/global-feed";
import { browserPresence } from "@/lib/presence";
import { ARRIVE_EVENT, arrivesAt } from "@/lib/sw-nav";
import type { SessionRef } from "@/lib/machines/key";
import { apiFor } from "@/lib/machines/registry";

type Source = components["schemas"]["Source"];

/**
 * Long enough that a burst of streaming deltas collapses into one POST, short
 * enough that the badge clears while you are still looking at the session.
 */
const DEBOUNCE_MS = 300;

// No document to ask outside a browser; nothing there reports anything anyway.
const nobodyOnServer = () => false;

/** What the seen report needs to know about the view, and who hears of arrivals. */
export interface SeenReporting {
  /**
   * The view has the reader's focus: always, outside a split; a focused pane
   * on a screen that is showing, inside one. Defaults to true.
   */
  attentive?: boolean;
  /** Its transcript is at the newest message. Defaults to true. */
  atBottom?: boolean;
  /**
   * Called when the report an arrival sent comes back, with the mark that
   * report replaced — when the reader had last looked before they got here,
   * or `null` if never. Where this arrival's unread messages begin.
   */
  onArrival?: (previousReadAt: string | null) => void;
}

/**
 * Report that this session has been looked at.
 *
 * Fires when the session is opened, again whenever new content lands while it
 * stays on screen, again when the window regains focus, and again the moment
 * it is being read again — each time gated on somebody actually being at the
 * page (`browserPresence`: visible, and focused or recently used) *and* on the
 * view being read: attentive and at its newest message. That gate is the whole
 * point: a detail view mounted in a background tab, a hidden screen, a pane
 * beside the one being read, a transcript scrolled up into history, or a
 * window left showing on a locked machine must never clear an unread count on
 * the reader's behalf, so both are checked when the report is scheduled and
 * again when it fires, since either can change in between.
 *
 * Some of those moments are the reader *arriving*: the session opened, the
 * tab or window come back to, its pane given the focus, the reader back at a
 * page they had left idle, a notification about it tapped while it was
 * already on screen. The report an arrival sends is the one that moves the
 * mark past whatever piled up while they were away, so its answer — the mark
 * it replaced — goes to `onArrival`. An arrival into a view that is not being
 * read (scrolled up, hidden) sends nothing and is not one: the reader is
 * already somewhere.
 *
 * Nothing is invalidated afterwards. The server pushes the updated row down the
 * global feed, and that feed is what the sidebar renders from.
 *
 * Under the same conditions the session is also registered with the feed as
 * *being read* (`markReading`), which shows its row with nothing unread
 * everywhere at once — list, folder, rail, tab title, pane frame — instead of
 * flashing a count on every answer for the length of the seen round trip.
 */
export function useReportSeen(session: SessionRef, reporting: SeenReporting = {}): void {
  const { machine, source, id } = session;
  const { attentive = true, atBottom = true } = reporting;
  // The view's half of being read. The other half — somebody at the page — is
  // `present`, read live at report time and as state for the effects below.
  const attending = attentive && atBottom;
  const presence = browserPresence();
  const present = useSyncExternalStore(presence.subscribe, presence.snapshot, nobodyOnServer);
  const reading = attending && present;
  // The feed row's timestamp doubles as the "new content arrived" edge: the
  // server bumps it for exactly the changes that are worth re-reporting against.
  //
  // Watched through the store rather than selected into a render, because this
  // hook renders nothing and its caller is the entire session transcript. A
  // `updatedAt` read as state would re-render that transcript on every streamed
  // message of the session being worked on — one full re-derivation of the
  // window's grouping and pairing — to arrive at an effect that only wants to
  // reset a 300ms timer. Nothing here is displayed, so nothing here needs a
  // render.
  const store = useGlobalFeedStore();

  // Read at schedule time and again at fire time, like visibility.
  const attendingRef = useRef(attending);
  attendingRef.current = attending;
  const onArrivalRef = useRef(reporting.onArrival);
  onArrivalRef.current = reporting.onArrival;

  // The session this hook is for, read again when a response comes back: an
  // answer about the last one must not reach the new one's `onArrival`.
  const targetRef = useRef({ machine, source, id });
  targetRef.current = { machine, source, id };
  // Unmounted: a response still in flight belongs to nothing.
  const aliveRef = useRef(true);
  // The next report is an arrival's. Cleared by the report that goes, and by
  // one that does not: an arrival only counts if the reader is still there.
  const arrivingRef = useRef(false);

  const post = useEventCallback(() => {
    const arrival = arrivingRef.current;
    const at = targetRef.current;
    arrivingRef.current = false;
    if (!presence.present() || !attendingRef.current) return;
    void apiFor(at.machine)
      .POST("/api/sessions/{source}/{id}/seen", {
        params: { path: { source: at.source as Source, id: at.id } },
      })
      .then(
        (res) => {
          const now = targetRef.current;
          if (!aliveRef.current || now.machine !== at.machine || now.source !== at.source || now.id !== at.id) return;
          if (arrival && res.data) onArrivalRef.current?.(res.data.previousReadAt ?? null);
        },
        // Offline: nothing was marked, so nothing moved — the next report tries again.
        () => undefined,
      );
  });
  const report = useDebounceCallback(post, DEBOUNCE_MS);

  const schedule = useEventCallback(() => {
    if (!presence.present() || !attendingRef.current) return;
    report();
  });

  const arrive = useEventCallback(() => {
    if (!presence.present() || !attendingRef.current) return;
    arrivingRef.current = true;
    schedule();
  });

  useEffect(() => {
    aliveRef.current = true;
    arrivingRef.current = false;
    // Back at a page that had been left: the reader returning to it, which is
    // an arrival like the window coming back into focus. Leaving needs nothing
    // here — every report checks presence before it goes.
    let wasPresent = presence.snapshot();
    const unsubscribePresence = presence.subscribe(() => {
      const now = presence.snapshot();
      if (now && !wasPresent) arrive();
      wasPresent = now;
    });

    const onVisibility = () => {
      if (document.visibilityState === "visible") arrive();
    };
    // A notification about this session, tapped while it is already on screen:
    // the route does not change, so nothing else would notice.
    const onNav = (event: Event) => {
      if (arrivesAt((event as CustomEvent<unknown>).detail, source, id)) arrive();
    };

    document.addEventListener("visibilitychange", onVisibility);
    window.addEventListener("focus", arrive);
    window.addEventListener(ARRIVE_EVENT, onNav);
    // Opening the session is itself a look at it, and an arrival.
    arrive();

    return () => {
      aliveRef.current = false;
      report.cancel();
      unsubscribePresence();
      document.removeEventListener("visibilitychange", onVisibility);
      window.removeEventListener("focus", arrive);
      window.removeEventListener(ARRIVE_EVENT, onNav);
    };
  }, [machine, source, id, presence, arrive, report]);

  // Being read again — the pane focused, the transcript scrolled down to the
  // newest message — is itself a look at whatever arrived meanwhile. Only the
  // focus is an arrival: scrolling down is reading, not coming back.
  const wasAttentive = useRef(attentive);
  useEffect(() => {
    const focused = attentive && !wasAttentive.current;
    wasAttentive.current = attentive;
    if (!reading) return;
    if (focused) arrive();
    else schedule();
  }, [reading, attentive, arrive, schedule]);

  // The optimistic half: while it is being read, it reads as read.
  useEffect(() => {
    if (!reading) return;
    return store.markReading({ machine, source, id });
  }, [store, machine, source, id, reading]);

  useEffect(() => {
    // The starting value is read, not reported on: opening the session is
    // already reported by the effect above, and firing again here would make
    // every mount of a pane cost two POSTs.
    const ref = { machine, source, id };
    let last = feedRow(store.getSnapshot(), ref)?.updatedAt;
    return store.subscribe(() => {
      const next = feedRow(store.getSnapshot(), ref)?.updatedAt;
      if (next === last) return;
      last = next;
      schedule();
    });
  }, [store, machine, source, id, schedule]);
}
