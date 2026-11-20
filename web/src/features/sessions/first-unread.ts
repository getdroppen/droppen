/**
 * The way back to where a visit's unread messages begin: the decisions.
 *
 * Arriving at a session at its newest message marks it read straight away
 * (`useReportSeen`), which is right for the badge and leaves the reader who
 * came back to twelve answers looking at the last of them. So the report an
 * arrival sends brings back the mark it replaced — when the reader had last
 * looked — and the server is asked where the first message after that is
 * (`GET /api/sessions/{source}/{id}/first-unread?after=`). Nothing the client
 * holds is trusted for that mark: a feed row can be stale (a socket that
 * dropped while the phone slept keeps its old rows) or missing (a page opened
 * straight from a notification), and the report is what moves it anyway.
 *
 * Arrivals are the session opened, the tab or window come back to, its pane
 * given the focus, and a notification about it tapped while it is on screen.
 * The jump is the in-session search's own (`useSessionSearch().jumpToItem`),
 * which already knows how to page history in to an item that is not loaded
 * yet and flash it; the button stays up while it does, with a way to stop it,
 * and says so if it could not get there.
 *
 * Pure, so every rule is a test away; `first-unread-view.tsx` is the half
 * that touches the DOM and the network.
 */

import type { TranscriptWindow } from "./session-feed-state";

/** The button, while there is one: where it leads and what it says. */
export interface FirstUnreadTarget {
  /** The first unread message's item index, in the feed's numbering. */
  index: number;
  /** How many agent messages from there on were unread. */
  count: number;
}

/** Where the button is in its life. */
export type FirstUnreadState =
  | { phase: "idle" }
  /**
   * An arrival's report came back; the server is being asked where its
   * unread messages begin. A button still up from an earlier arrival stays
   * up meanwhile (`kept`).
   */
  | { phase: "locating"; seq: number; kept: FirstUnreadTarget | null }
  | { phase: "ready"; target: FirstUnreadTarget }
  /** Taken: the jump is paging history in. */
  | { phase: "jumping"; target: FirstUnreadTarget }
  /** The jump could not get there; said briefly, then gone. */
  | { phase: "failed" };

export const FIRST_UNREAD_IDLE: FirstUnreadState = { phase: "idle" };

export type FirstUnreadEvent =
  /** The session changed under the view: whatever was up is about another one. */
  | { type: "reset" }
  /** Arrival `seq`'s report came back; its unread messages are being located. */
  | { type: "arrived"; seq: number }
  /** The server's answer for arrival `seq`. */
  | { type: "located"; seq: number; target: FirstUnreadTarget | null }
  | { type: "jump" }
  | { type: "landed" }
  | { type: "failed" }
  | { type: "cancelled" }
  /** Reached or scrolled past by hand, or a failure that has been said. */
  | { type: "dismiss" };

/** The target a state shows a button for, if any. */
function shownTarget(state: FirstUnreadState): FirstUnreadTarget | null {
  switch (state.phase) {
    case "ready":
    case "jumping":
      return state.target;
    case "locating":
      return state.kept;
    default:
      return null;
  }
}

export function firstUnreadReducer(state: FirstUnreadState, event: FirstUnreadEvent): FirstUnreadState {
  switch (event.type) {
    case "reset":
      return state.phase === "idle" ? state : FIRST_UNREAD_IDLE;
    case "arrived":
      // The reader is already on their way there; a focus event mid-jump is
      // not a reason to start over.
      if (state.phase === "jumping") return state;
      return { phase: "locating", seq: event.seq, kept: shownTarget(state) };
    case "located": {
      if (state.phase !== "locating" || state.seq !== event.seq) return state;
      const { kept } = state;
      // A button still up leads to a message the reader has not scrolled to,
      // which is older than anything this arrival found: it keeps its place,
      // and counts what has piled up behind it since.
      if (kept) return { phase: "ready", target: event.target ? { ...kept, count: kept.count + event.target.count } : kept };
      return event.target ? { phase: "ready", target: event.target } : FIRST_UNREAD_IDLE;
    }
    case "jump": {
      const target = shownTarget(state);
      return target ? { phase: "jumping", target } : state;
    }
    case "landed":
    case "cancelled":
      return state.phase === "jumping" ? FIRST_UNREAD_IDLE : state;
    case "failed":
      return state.phase === "jumping" ? { phase: "failed" } : state;
    case "dismiss":
      return state.phase === "jumping" || state.phase === "idle" ? state : FIRST_UNREAD_IDLE;
  }
}

/** What the button draws, or `null` when there is none. */
export type FirstUnreadView =
  | { kind: "offer"; target: FirstUnreadTarget }
  | { kind: "jumping"; target: FirstUnreadTarget }
  | { kind: "failed" };

export function firstUnreadView(state: FirstUnreadState): FirstUnreadView | null {
  if (state.phase === "jumping") return { kind: "jumping", target: state.target };
  if (state.phase === "failed") return { kind: "failed" };
  const target = shownTarget(state);
  return target ? { kind: "offer", target } : null;
}

/**
 * The query that asks where an arrival's unread messages begin: from the mark
 * the arrival's report replaced, and with the resume marker of the window the
 * feed holds, when it holds one. The marker only spares the server a read: it
 * lets it answer off the transcript it last read when that read provably
 * holds the window, and it reads the file as it is now when it cannot.
 */
export function firstUnreadQuery(
  previousReadAt: string | null,
  window?: Pick<TranscriptWindow, "endIndex" | "prefixFp"> | null,
): { after?: string; since?: number; fp?: string } {
  return {
    ...(previousReadAt === null ? {} : { after: previousReadAt }),
    ...(window?.prefixFp && Number.isInteger(window.endIndex)
      ? { since: window.endIndex, fp: window.prefixFp }
      : {}),
  };
}

/** The server's answer as a button target: none for nothing unread. */
export function targetOf(answer: { index: number | null; count: number } | undefined): FirstUnreadTarget | null {
  if (!answer || answer.index === null || answer.count <= 0) return null;
  return { index: answer.index, count: answer.count };
}

/** Slack, in px, for a row whose top sits exactly on the viewport's edge. */
const EDGE_SLACK_PX = 4;

/**
 * Whether the reader has got to the first unread row by themselves.
 *
 * Reached once its top is on screen or below the viewport's top edge: they
 * scrolled up to it, or past it, or it was in view all along (a handful of
 * short answers under a tall screen). `rowTop` is `null` for a row that is not
 * laid out — not loaded yet, which is to say further up still.
 */
export function reachedFirstUnread(rowTop: number | null, viewportTop: number): boolean {
  return rowTop !== null && rowTop >= viewportTop - EDGE_SLACK_PX;
}
