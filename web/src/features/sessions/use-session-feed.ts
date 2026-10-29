/*
 * The detail view's session feed: the per-machine pool of feeds, and the hook
 * a view reads its window, drafts and connection from.
 */
import { useCallback, useEffect, useMemo, useSyncExternalStore, type RefObject } from "react";
import type { components } from "@/api/schema";
import type { TranscriptHistoryPage } from "@/features/sessions/session-feed-state";
import { createSessionFeedPool, type FeedConnection, type FeedState } from "@/features/sessions/session-feed-pool";
import type { DraftStore } from "@/features/sessions/stream-draft";
import { feedHubFor } from "@/app/global-feed";
import { devicePersistFor } from "@/features/sessions/feed-persist";
import { useMachine } from "@/lib/machines/context";
import type { MachineKey } from "@/lib/machines/key";
import { apiFor, perMachine, transportFor } from "@/lib/machines/registry";
import { noteWsFailure } from "@/lib/ws-url";

export type Source = components["schemas"]["Source"];

interface SessionFeedResult {
  data: FeedState | null;
  /**
   * The count of the think in flight, which the transcript cannot say — see
   * `stream-draft.ts`. A store rather than a value, and beside `data` rather
   * than in it: nothing that reads the window (the rows memo, the unread
   * cursor, the search, the first-unread jump) has any business with it, and
   * a draft frame — a dozen a second during a long think — must not re-render
   * the view this hook serves. Only the activity bar's words subscribe to it;
   * the transcript draws nothing of it. The same object for the life of the
   * feed.
   */
  drafts: DraftStore;
  isError: boolean;
  /** Live, retrying, or the server has kept saying the session is not there. */
  connection: FeedConnection;
  loadEarlier: () => Promise<void>;
  isLoadingEarlier: boolean;
  historyError: string | null;
  /** Re-attach the socket, which re-snapshots state the server just rewrote. */
  refresh: () => void;
  /**
   * Re-read the state fields alone, leaving the transcript window untouched.
   *
   * The socket already pushes state, but on a two-second tick, and there are
   * actions — removing a queued message, steering one — whose whole point is
   * that the list changes when you click. Two seconds of a chip sitting there
   * unchanged reads as a click that did nothing. [`refresh`] would also work
   * and costs far too much: it re-attaches the socket, which replaces the
   * window and throws away every earlier page the user scrolled back to load.
   */
  refreshState: () => void;
}

/**
 * A machine's session feeds; see `session-feed-pool.ts` for why a feed
 * outlives the view showing it. One set per machine, kept at module level,
 * because every view of a session — a split pane beside the main one, the chat
 * presentation — should be looking at the same window. Closing the machine
 * stops them all (`closeMachine`), sockets and timers with them.
 */
const sessionFeedsFor = perMachine((machine: MachineKey) =>
  createSessionFeedPool({
    // The last few windows read, kept on the device for the next cold start.
    persist: devicePersistFor(machine),
    // On the machine's one socket where its server can carry it there, which
    // saves every session switch a connection of its own; see `app/feed-hub.ts`.
    openFeed: (target) => feedHubFor(machine).open(target),
    // A socket of the feed's own, through the machine's transport
    // (`lib/machines`) as it is at each attempt.
    socketUrl: (path) => transportFor(machine).socketUrl(path),
    openSocket: (url) => transportFor(machine).openSocket(url),
    noteFailure: (streak) => noteWsFailure(streak, machine),
    async fetchHistory(source, id, query) {
      const res = await apiFor(machine).GET("/api/sessions/{source}/{id}/history", {
        params: { path: { source: source as Source, id }, query },
      });
      if (res.error) return { status: res.response.status, error: res.error.error };
      return { status: res.response.status, page: res.data as TranscriptHistoryPage | undefined };
    },
    async fetchState(source, id, held) {
      const res = await apiFor(machine).GET("/api/sessions/{source}/{id}", {
        params: { path: { source: source as Source, id }, query: held },
      });
      return res.error || !res.data ? null : res.data;
    },
  }),
  (pool) => pool.dispose(),
);

/**
 * @param pinnedRef Whether the view is following the live tail. Read, never
 *   written, and read only at the moment a delta lands — it is the permission
 *   to trim the window's oldest items ([`trimLiveWindow`]). A ref rather than a
 *   value because the pin is owned by the scroll handler in the component
 *   below, changes on a wheel event rather than on a render, and must not put
 *   this hook's socket through a re-subscribe every time the reader moves.
 */
export function useSessionFeed(
  source: string,
  id: string,
  pinnedRef: RefObject<boolean>,
): SessionFeedResult {
  const sessionFeeds = sessionFeedsFor(useMachine());
  // Looked up during the render so that a session read a moment ago is drawn
  // from its kept window on the very first frame, with no spinner between.
  const feed = useMemo(() => sessionFeeds.feed(source, id), [sessionFeeds, source, id]);
  useEffect(() => sessionFeeds.acquire(feed, pinnedRef), [sessionFeeds, feed, pinnedRef]);
  const { data, connection, isLoadingEarlier, historyError } = useSyncExternalStore(
    feed.subscribe,
    feed.getView,
    feed.getView,
  );
  const loadEarlier = useCallback(() => feed.loadEarlier(), [feed]);
  const refresh = useCallback(() => feed.reattach(), [feed]);
  const refreshState = useCallback(() => feed.refreshState(), [feed]);
  return {
    data,
    drafts: feed.drafts,
    // Only a verdict when there is nothing to show it over: a session that went
    // missing while its transcript is on screen keeps the transcript, and says
    // so in a banner instead of replacing the page with "session not found".
    isError: connection === "gone" && data === null,
    connection,
    loadEarlier,
    isLoadingEarlier,
    historyError,
    refresh,
    refreshState,
  };
}
