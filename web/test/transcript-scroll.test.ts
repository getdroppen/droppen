// The transcript's pin: what keeps it on the newest row, what takes it off,
// and what the floating jump control says while it is off.
import assert from "node:assert/strict";
import test from "node:test";
import {
  BOTTOM_SLACK_PX,
  bottomGap,
  holdBottomEdge,
  isAtBottom,
  lastContentBottom,
  NO_UNREAD,
  pinAfterScroll,
  pinOnSettle,
  scrollTopForGap,
  shouldTopUpHistory,
  showJumpToBottom,
  trackUnread,
  unreadLabel,
  type ScrollBox,
} from "../src/features/sessions/transcript-scroll.js";
import {
  LIVE_WINDOW_CAP,
  LIVE_WINDOW_SLACK,
} from "../src/features/sessions/session-feed-state.js";

function box(scrollTop: number, scrollHeight: number, clientHeight = 800): ScrollBox {
  return { scrollTop, scrollHeight, clientHeight };
}

test("the gap is the distance from the last row to the bottom edge", () => {
  assert.equal(bottomGap(box(1000, 2000)), 200);
  assert.equal(bottomGap(box(1200, 2000)), 0);
  // Overscroll bounce on iOS reports a scrollTop past the end.
  assert.equal(bottomGap(box(1300, 2000)), 0);
  assert.ok(isAtBottom(box(1200 - (BOTTOM_SLACK_PX - 1), 2000)));
  assert.ok(!isAtBottom(box(1200 - (BOTTOM_SLACK_PX + 1), 2000)));
});

// The regression this whole module exists for. A transcript is pinned against
// whatever `scrollHeight` said at the time, and that height keeps moving — an
// image decodes, a fold opens, the socket delivers. Reading the growth
// underneath a still view as "the user scrolled up" left every session parked
// in the middle.
test("the keyboard resizing the box keeps its bottom edge where it was", () => {
  // Pinned (gap 0): the keyboard takes 336px off an 800px box, and the view
  // is still on the newest row.
  assert.equal(scrollTopForGap(0, { scrollHeight: 5000, clientHeight: 464 }), 4536);
  // A reader 300px up keeps reading the same lines above the composer, with
  // the keyboard opening and with it closing again.
  const reading = box(3900, 5000);
  const gap = bottomGap(reading);
  assert.equal(gap, 300);
  const up = scrollTopForGap(gap, { scrollHeight: 5000, clientHeight: 464 });
  assert.equal(up, 4236);
  assert.equal(bottomGap(box(up, 5000, 464)), 300);
  assert.equal(scrollTopForGap(gap, { scrollHeight: 5000, clientHeight: 800 }), 3900);
  // A transcript shorter than the box stays at the top.
  assert.equal(scrollTopForGap(0, { scrollHeight: 300, clientHeight: 464 }), 0);
  assert.equal(scrollTopForGap(900, { scrollHeight: 1200, clientHeight: 464 }), 0);
});

/** A scroll box that clamps `scrollTop` the way a browser does, including when it is resized. */
class ClampingScroller implements ScrollBox {
  private top = 0;
  constructor(
    public scrollHeight: number,
    public clientHeight: number,
  ) {}
  get scrollTop(): number {
    return this.top;
  }
  set scrollTop(v: number) {
    this.top = Math.min(Math.max(0, v), Math.max(0, this.scrollHeight - this.clientHeight));
  }
  resize(clientHeight: number) {
    this.clientHeight = clientHeight;
    this.scrollTop = this.top;
  }
}

test("holding the bottom edge across the keyboard: the scroll event it causes is not the reader moving", () => {
  // The order session-detail.tsx keeps: measure, resize, settle, record the
  // returned position as the last seen one — then the queued scroll event.
  const keyboard = (el: ClampingScroller, pinned: boolean, clientHeight: number) => {
    const settle = holdBottomEdge(el, pinned);
    el.resize(clientHeight);
    const lastTop = settle();
    assert.equal(lastTop, el.scrollTop);
    return pinAfterScroll(pinned, el, lastTop);
  };

  // Pinned: on the newest row with the keyboard up, and again once it is down.
  const pinnedView = new ClampingScroller(5000, 800);
  pinnedView.scrollTop = 4200;
  assert.equal(keyboard(pinnedView, true, 464), true);
  assert.equal(bottomGap(pinnedView), 0);
  assert.equal(keyboard(pinnedView, true, 800), true);
  assert.equal(bottomGap(pinnedView), 0);

  // A reader 300px up stays 300px up and unpinned, both ways — including the
  // keyboard going down, where the browser first clamps the grown box onto
  // the bottom and the hold then takes it back up.
  const reader = new ClampingScroller(5000, 800);
  reader.scrollTop = 3900;
  assert.equal(keyboard(reader, false, 464), false);
  assert.equal(bottomGap(reader), 300);
  assert.equal(keyboard(reader, false, 800), false);
  assert.equal(bottomGap(reader), 300);
  assert.equal(reader.scrollTop, 3900);
  assert.equal(showJumpToBottom(false, reader), true);
});

test("a bottom that runs away downwards keeps the pin", () => {
  const pinnedAt = box(1200, 2000);
  assert.ok(pinAfterScroll(true, pinnedAt, 1200));
  // Same scrollTop, height corrected upwards by the real row heights.
  const grown = box(1200, 5000);
  assert.ok(!isAtBottom(grown));
  assert.ok(pinAfterScroll(true, grown, 1200));
});

test("scrolling up surrenders the pin, scrolling back to the bottom takes it again", () => {
  assert.ok(!pinAfterScroll(true, box(400, 5000), 1200));
  // Still unpinned wherever the user stops, however the height moves.
  assert.ok(!pinAfterScroll(false, box(400, 9000), 400));
  assert.ok(!pinAfterScroll(false, box(3000, 5000), 400));
  // Back within the slack of the bottom: following the newest rows resumes.
  assert.ok(pinAfterScroll(false, box(4150, 5000), 3000));
});

test("sub-pixel drift is not a user scrolling up", () => {
  assert.ok(pinAfterScroll(true, box(1199.5, 5000), 1200));
  assert.ok(!pinAfterScroll(true, box(1150, 5000), 1200));
});

test("content shrinking under a pinned view keeps it pinned", () => {
  // A fold closing shortens the transcript and the browser clamps scrollTop,
  // which moves the view up without anyone having touched it — but it lands
  // exactly on the bottom, which is what clamping means.
  assert.ok(pinAfterScroll(true, box(700, 1500), 1200));
});

// The jitter fix: the observer re-sticks a pinned view on every resize, so a
// drag that has not yet cleared the bottom slack must not still count as
// pinned, or the first tenth of a second of every upward swipe is a tug of war
// between the finger and `scrollTop = scrollHeight`.
test("an upward drag inside the bottom slack surrenders the pin", () => {
  // 30px off the bottom, which `isAtBottom` would happily call "at the newest".
  const dragged = box(4170, 5000);
  assert.ok(isAtBottom(dragged));
  assert.ok(!pinAfterScroll(true, dragged, 4200));
  // Carrying on up keeps it off, still inside the slack.
  assert.ok(!pinAfterScroll(false, box(4150, 5000), 4170));
});

test("a drag back down onto the bottom re-pins", () => {
  assert.ok(pinAfterScroll(false, box(4200, 5000), 4170));
});

test("the jump button appears only while the newest row is off screen", () => {
  assert.ok(showJumpToBottom(false, box(400, 5000)));
  // A pinned view chasing a height that just changed must not blink it on.
  assert.ok(!showJumpToBottom(true, box(400, 5000)));
  // Unpinned but the bottom is right there: nothing to jump to.
  assert.ok(!showJumpToBottom(false, box(4180, 5000)));
});

/** A scroller with a fixed top edge and rows at given bottoms. */
function scroller(top: number, rows: { text: string; bottom: number }[]) {
  return {
    getBoundingClientRect: () => ({ top }),
    querySelectorAll: () =>
      rows.map((row) => ({ textContent: row.text, getBoundingClientRect: () => ({ bottom: row.bottom }) })),
  };
}

test("the empty rows a consumed result leaves are not something to jump to", () => {
  // The viewport (scrollTop 1000, 800 tall) ends at 1800; 200px of empty rows
  // sit below it, so the box alone would read a gap of 200 and park the button
  // over a line that is already fully in view. The last row with text ends
  // exactly on the bottom edge, which is a gap of none.
  const trailing = box(1000, 2000);
  assert.equal(bottomGap(trailing), 200);
  const flush = lastContentBottom(scroller(0, [{ text: "a line", bottom: 800 }, { text: "  ", bottom: 900 }]), 1000);
  assert.equal(flush, 1800);
  assert.ok(!showJumpToBottom(false, trailing, flush));

  // And a newest line properly below the edge still offers the button.
  const below = lastContentBottom(scroller(0, [{ text: "a line", bottom: 900 }]), 1000);
  assert.equal(below, 1900);
  assert.ok(showJumpToBottom(false, trailing, below));
});

test("a window with nothing readable in its last rows falls back to the box", () => {
  const empty = scroller(0, Array.from({ length: 20 }, () => ({ text: "", bottom: 500 })));
  assert.equal(lastContentBottom(empty, 0), null);
  assert.ok(showJumpToBottom(false, box(400, 5000), null));
});

test("unread counts what arrived below the viewport while the user was away", () => {
  // The first window seen is adopted, never counted.
  let cursor = trackUnread(NO_UNREAD, 200, false);
  assert.deepEqual(cursor, { endIndex: 200, count: 0 });

  cursor = trackUnread(cursor, 203, false);
  assert.equal(cursor.count, 3);
  cursor = trackUnread(cursor, 205, false);
  assert.equal(cursor.count, 5);

  // Earlier history prepends: startIndex moves, the end does not, nothing is new.
  cursor = trackUnread(cursor, 205, false);
  assert.equal(cursor.count, 5);

  // Back at the bottom, the count is spent.
  cursor = trackUnread(cursor, 205, true);
  assert.deepEqual(cursor, { endIndex: 205, count: 0 });
});

test("a window replaced by a reconnect does not carry a stale count", () => {
  const cursor = trackUnread({ endIndex: 400, count: 7 }, 210, false);
  assert.deepEqual(cursor, { endIndex: 210, count: 0 });
});

test("the badge stays three characters wide", () => {
  assert.equal(unreadLabel(1), "1");
  assert.equal(unreadLabel(99), "99");
  assert.equal(unreadLabel(100), "99+");
});

// The automatic top-up: a loaded window shorter than its own viewport has
// nothing to scroll, so the reader cannot ask for the rest and the transcript
// asks for them itself.
const TOP_UP = {
  hasMoreBefore: true,
  loading: false,
  failed: false,
  startIndex: 400,
  alreadyLoadedFrom: -1,
  loadedItems: 200,
  box: box(0, 300, 800),
};

test("a window that does not fill its viewport tops itself up", () => {
  assert.ok(shouldTopUpHistory(TOP_UP));
  // Taller than the box: there is something to scroll, so the reader asks.
  assert.ok(!shouldTopUpHistory({ ...TOP_UP, box: box(0, 4000, 800) }));
  // Nothing older to ask for, a request already out, or a failure on screen.
  assert.ok(!shouldTopUpHistory({ ...TOP_UP, hasMoreBefore: false }));
  assert.ok(!shouldTopUpHistory({ ...TOP_UP, loading: true }));
  assert.ok(!shouldTopUpHistory({ ...TOP_UP, failed: true }));
});

// The incident this guard exists for. A session pane inside a screen the user
// has switched away from stays mounted and is `display: none`, and a box with
// no layout reports every scroll metric as 0 — which read as "shorter than the
// viewport" and stayed true however many pages landed, so the pane walked the
// whole transcript into memory and into the DOM behind a hidden div.
test("a scroller with no layout box is not a short transcript", () => {
  assert.ok(!shouldTopUpHistory({ ...TOP_UP, box: box(0, 0, 0) }));
  // And it stays no, page after page: a hidden box never gets taller.
  assert.ok(!shouldTopUpHistory({ ...TOP_UP, startIndex: 200, box: box(0, 0, 0) }));
  assert.ok(!shouldTopUpHistory({ ...TOP_UP, startIndex: 0, box: box(0, 0, 0) }));
});

// `prependHistoryPage` refuses a page whose range no longer meets the window,
// which a live session produces routinely — a delta lands while the request is
// out. The refusal changes nothing and raises no error, so without a memory of
// what was already asked for the same page is requested for ever.
test("a window that did not move is not asked for twice", () => {
  assert.ok(!shouldTopUpHistory({ ...TOP_UP, alreadyLoadedFrom: 400 }));
  // A page that really landed moved `startIndex`, so the next one is allowed.
  assert.ok(shouldTopUpHistory({ ...TOP_UP, startIndex: 200, alreadyLoadedFrom: 400 }));
});

// The loop the cap would otherwise create. `trimLiveWindow` drops the oldest
// items to keep a live pane bounded; the top-up fetches the page *before* the
// oldest item. Left to themselves they undo each other for ever — trim, shrink,
// top up, re-grow, trim — at whatever rate the server serves history, and none
// of the guards above notice, because every one of those fetches is a genuinely
// new `startIndex` that was genuinely accepted.
test("a window that has been trimmed is never topped up again", () => {
  // Exactly what a trim leaves behind, in a box reporting nothing (the worst
  // case: a hidden pane, or a height that has not settled).
  const trimmed = { ...TOP_UP, loadedItems: LIVE_WINDOW_CAP, box: box(0, 0, 0) };
  assert.ok(!shouldTopUpHistory(trimmed));
  // And with a box that really does say "shorter than the viewport", which is
  // the only condition the top-up was ever looking for.
  assert.ok(!shouldTopUpHistory({ ...trimmed, box: box(0, 300, 800) }));
  // A window sitting between the cap and the trim point is the same answer.
  assert.ok(
    !shouldTopUpHistory({ ...TOP_UP, loadedItems: LIVE_WINDOW_CAP + LIVE_WINDOW_SLACK }),
  );
  // A user who asked for earlier history by hand has a longer window still, and
  // must not have the automatic fetch start up underneath them either.
  assert.ok(
    !shouldTopUpHistory({ ...TOP_UP, startIndex: 0, loadedItems: LIVE_WINDOW_CAP + 400 }),
  );
  // One item under the cap is a window no trim has ever touched — a fresh
  // attach, or a short session — and it tops up exactly as it always did.
  assert.ok(shouldTopUpHistory({ ...TOP_UP, loadedItems: LIVE_WINDOW_CAP - 1 }));
});

test("a search or a history page that settles at the bottom takes the pin back", () => {
  // The bug: on a transcript too short to scroll, a search jump unpinned the
  // view and nothing could re-pin it — no scroll event, no jump button — so the
  // session could never be marked read again until it was reopened.
  assert.equal(pinOnSettle(false, box(0, 600)), true, "nothing to scroll: it is at the newest row");
  assert.equal(pinOnSettle(false, box(1200, 2000)), true, "settled right at the bottom");
  assert.equal(pinOnSettle(false, box(400, 4000)), false, "a reader left in the middle stays there");
  assert.equal(pinOnSettle(true, box(400, 4000)), true, "and a pinned view is never unpinned by it");
});
