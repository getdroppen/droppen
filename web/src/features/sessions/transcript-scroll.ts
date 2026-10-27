/**
 * The transcript's scroll discipline: when the view is pinned to the newest
 * rows, when the user has taken it off the bottom, and what the jump-to-bottom
 * control says while they are away.
 *
 * Pure, and deliberately not inside `session-detail.tsx`: the browser half of
 * this — a `ResizeObserver` chasing a height that is still settling — is
 * exactly the part no test can drive, so the decisions must not be tangled up
 * in it.
 */

import { LIVE_WINDOW_CAP } from "./session-feed-state.js";

/** Distance from the bottom, in px, that still counts as "at the newest". */
export const BOTTOM_SLACK_PX = 80;

/**
 * Slack, in px, for a `scrollTop` comparison.
 *
 * Fractional device pixels and a clamp against a height that changed in the
 * same frame both move `scrollTop` by less than a pixel without anyone having
 * touched the wheel, and reading that as "the user scrolled up" is what this
 * avoids.
 */
const DRIFT_PX = 1;

/** The three numbers a scroll container answers about where it is. */
export interface ScrollBox {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
}

/** How far the last row is below the bottom edge of the viewport. */
export function bottomGap(box: ScrollBox): number {
  return Math.max(0, box.scrollHeight - box.scrollTop - box.clientHeight);
}

export function isAtBottom(box: ScrollBox, slack = BOTTOM_SLACK_PX): boolean {
  return bottomGap(box) < slack;
}

/**
 * Where `scrollTop` goes so the view's bottom edge is `gap` px above the end of
 * the rows again, after the box changed height under it — the phone keyboard
 * opening or closing. The bottom edge is the one that
 * holds, as in a phone's own messaging apps: `0` keeps a pinned view on the
 * newest row, and a reader partway up keeps the lines just above the composer.
 */
export function scrollTopForGap(gap: number, box: Pick<ScrollBox, "scrollHeight" | "clientHeight">): number {
  return Math.max(0, box.scrollHeight - box.clientHeight - gap);
}

/**
 * Hold a scroller's bottom edge across a resize from below. Call it before the
 * resize, with whether the view is pinned (a pinned view's gap is none); call
 * what it returns after, and it puts the edge back and returns the new
 * `scrollTop`.
 *
 * The caller records that as its last-seen position straight away, before the
 * scroll event the assignment queues comes in: that event is the shell moving,
 * not the reader, and measured against the old position `pinAfterScroll`
 * would read a keyboard going down as an upward drag.
 */
export function holdBottomEdge(el: ScrollBox, pinned: boolean): () => number {
  const gap = pinned ? 0 : bottomGap(el);
  return () => {
    el.scrollTop = scrollTopForGap(gap, el);
    return el.scrollTop;
  };
}

/**
 * Whether the view is still pinned after a scroll event.
 *
 * The rule is *not* "is it at the bottom": a pinned transcript is regularly not
 * at the bottom for a frame or two, because the height it was pinned against
 * moved afterwards — an image decoded, a fold opened, the socket delivered.
 * Reading that as "the user has scrolled up" is what left sessions parked in
 * the middle: one stick against a stale height, an immediate self-inflicted
 * unpin, and no state frame afterwards for an idle session to re-stick from.
 * So a bottom running away downwards keeps the pin, and the `ResizeObserver`
 * that noticed the growth chases it.
 *
 * Moving *up* is the other half, and it is decided before the bottom slack is
 * consulted. Only two things move a view up: the user, and the browser clamping
 * `scrollTop` when the content under it shrank. The clamp lands exactly on the
 * bottom — that is what clamping means — so it keeps the pin, and everything
 * else surrenders it. Asking "is it at the bottom" first instead made the first
 * {@link BOTTOM_SLACK_PX} of every upward drag re-pin the view, and any resize
 * landing inside that window — a streamed token, a row settling — sent the
 * observer's `scrollTop = scrollHeight` to yank the reader back down mid-drag.
 * On a phone, where a drag starts slowly and the finger is still on the glass,
 * that reads as the transcript fighting back.
 */
export function pinAfterScroll(pinned: boolean, box: ScrollBox, previousTop: number): boolean {
  if (box.scrollTop < previousTop - DRIFT_PX) return bottomGap(box) <= DRIFT_PX;
  // Anyone who arrives at the bottom is pinned, however they got there — this
  // is how scrolling back down resumes following the newest rows.
  if (isAtBottom(box)) return true;
  return pinned;
}

/**
 * The pin once something that took it away has finished: a search closed, or a
 * page of history settled in.
 *
 * Both unpin on purpose and neither scrolls back, and on a transcript too short
 * to scroll nothing else ever will either — no scroll event, no jump button
 * (the gap is under the slack) — so the view would stay "away from the newest"
 * for good, and the session could never again count as read. So at those two
 * moments, and only those, the geometry decides. Not on every frame: a reader
 * who has just dragged a few pixels up must keep the pin they gave away (see
 * [`pinAfterScroll`]).
 */
export function pinOnSettle(pinned: boolean, box: ScrollBox): boolean {
  return pinned || isAtBottom(box);
}

/**
 * Whether the floating "back to the newest" button belongs on screen.
 *
 * Pinned views never show it, even while they are catching up with a height
 * that just changed, or it would blink on every streamed token.
 */
export function showJumpToBottom(pinned: boolean, box: ScrollBox): boolean {
  return !pinned && bottomGap(box) > BOTTOM_SLACK_PX;
}

/**
 * Whether the transcript should go and fetch an earlier page by itself.
 *
 * The rule being encoded is small — "the loaded window does not fill its
 * viewport, so there is nothing to scroll and no way for the reader to ask for
 * more" — and the two guards around it are the whole reason this is a named
 * function instead of an `if` in the layout effect. Both of them cost a real
 * incident.
 *
 * `clientHeight > 0` is the load-bearing one. A session pane inside a screen
 * the user has switched away from stays mounted and goes `display: none` (see
 * `features/screens/screen-frames.tsx`, which is keeping the README's promise
 * that a screen you come back to is exactly as you left it). An element with no
 * layout box reports every scroll metric as `0`, so "is the content shorter
 * than the viewport" reads `0 <= 0 + 80` — true, and true again however many
 * pages arrive, because nothing that lands can ever make a hidden box taller.
 * The pane then walks the *entire* transcript into the item array and into the
 * DOM, 200 items at a time (`SESSION_PAGE_SIZE`), in a tab nobody is looking at. One hidden screen on the machine this was found on held a 77 MB
 * session; the tab reached 1.6 GB of renderer memory. A zero-height scroller is
 * not a short transcript, it is an undisplayed one, and the answer is no.
 *
 * `alreadyLoadedFrom` is the second. `prependHistoryPage` rejects a page whose
 * range no longer meets the window, which a *live* session produces routinely:
 * a delta lands between the request going out and the answer coming back. A
 * rejected page leaves the window exactly where it was and raises no error, so
 * the caller's effect re-runs on the loading edge, finds this condition still
 * true, and asks for the same page again — as fast as the server can answer,
 * for ever. Keyed on `startIndex` because a page that was really accepted
 * always moves it, so a legitimate second top-up is never blocked.
 *
 * `loadedItems` is the third, and it exists because of the trim that keeps a
 * live window bounded ([`trimLiveWindow`]). Trimming and topping up are exact
 * opposites — one drops the oldest items, the other fetches the page before the
 * oldest item — so a window that can do both is a window that can do them to
 * each other, for ever: the trim shortens the box, the box no longer fills the
 * viewport, this fires, the server hands back precisely what was just dropped,
 * the window is over the cap again, and round it goes at whatever rate the
 * server can serve a history page. Neither guard above catches it, because
 * every one of those fetches is legitimately a *different* `startIndex` that
 * really was accepted.
 *
 * The rule that breaks it is a structural one rather than a guess about
 * heights. A trim always leaves exactly `LIVE_WINDOW_CAP` items, and nothing
 * else in the client ever removes items — deltas append, history pages prepend,
 * a reset replaces the window with a fresh page at most `SESSION_PAGE_SIZE`
 * long. So a window holding fewer than the cap has provably never been trimmed,
 * and a window holding at least the cap is provably not short of content: the
 * cap is many viewports tall by construction. Refusing to auto-fetch at or
 * above the cap therefore cannot suppress a real top-up, and makes the loop
 * impossible rather than merely unlikely — including in the case where the box
 * is reporting a height that has not settled yet.
 */
export function shouldTopUpHistory(input: {
  hasMoreBefore: boolean;
  /** A request is already out; one at a time or the pages interleave. */
  loading: boolean;
  /** The last attempt failed and is showing a retry; do not loop on it. */
  failed: boolean;
  /** The window's current first absolute index. */
  startIndex: number;
  /** The `startIndex` a top-up was last fired for; -1 before the first. */
  alreadyLoadedFrom: number;
  /** How many items the window holds; at the cap it is never topped up. */
  loadedItems: number;
  box: ScrollBox;
  slack?: number;
}): boolean {
  if (!input.hasMoreBefore || input.loading || input.failed) return false;
  if (input.alreadyLoadedFrom === input.startIndex) return false;
  // A window this long has either been trimmed or is about to be. Either way it
  // is not short of content, and fetching more would only feed the trim.
  if (input.loadedItems >= LIVE_WINDOW_CAP) return false;
  // Not "short": not displayed. See the note above.
  if (input.box.clientHeight <= 0) return false;
  return input.box.scrollHeight <= input.box.clientHeight + (input.slack ?? BOTTOM_SLACK_PX);
}

/** What has arrived below the viewport since the user scrolled away from it. */
export interface UnreadCursor {
  /** Exclusive end of the window when it was last seen; -1 before the first. */
  endIndex: number;
  count: number;
}

/** No window seen yet — the next one is adopted without counting as unread. */
export const NO_UNREAD: UnreadCursor = { endIndex: -1, count: 0 };

/**
 * Fold the newest window end into the unread count.
 *
 * Only growth at the end counts: loading earlier history moves `startIndex` and
 * leaves `endIndex` alone, so reading back through a session never invents
 * unread messages.
 */
export function trackUnread(prev: UnreadCursor, endIndex: number, pinned: boolean): UnreadCursor {
  if (pinned) return { endIndex, count: 0 };
  // A reconnect replaces the window rather than extending it, and a count
  // carried across that has nothing left to refer to.
  if (prev.endIndex < 0 || endIndex < prev.endIndex) return { endIndex, count: 0 };
  return { endIndex, count: prev.count + (endIndex - prev.endIndex) };
}

/** The badge text, kept to three characters so the button stays a button. */
export function unreadLabel(count: number): string {
  return count > 99 ? "99+" : String(count);
}
