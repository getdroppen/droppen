import assert from "node:assert/strict";
import test from "node:test";
import {
  applyTranscriptDelta,
  historyQueryFor,
  LIVE_WINDOW_CAP,
  LIVE_WINDOW_SLACK,
  prependHistoryPage,
  resumeQuery,
  resyncMessage,
  scrollTopAfterPrepend,
  transcriptDeltaOf,
  transcriptWindowOf,
  trimLiveWindow,
  type TranscriptDelta,
  type TranscriptHistoryPage,
  type TranscriptItem,
  type TranscriptWindow,
} from "../src/features/sessions/session-feed-state.js";
import {
  NO_UNREAD,
  shouldTopUpHistory,
  trackUnread,
} from "../src/features/sessions/transcript-scroll.js";
import { transcriptItem } from "./session-fixtures.js";

const item = (text: string) => transcriptItem({ text });

/** Items `from..to`, each named for its absolute index. */
function itemsFrom(from: number, to: number, suffix = ""): TranscriptItem[] {
  return Array.from({ length: to - from }, (_, i) => item(String(from + i) + suffix));
}

function texts(w: { items: TranscriptItem[] }): string[] {
  return w.items.map((entry) => entry.text);
}

/** A window over `[startIndex, endIndex)` as a snapshot would hand it over. */
function held(startIndex: number, endIndex: number, sig = "v1"): TranscriptWindow {
  return {
    sig,
    items: itemsFrom(startIndex, endIndex),
    startIndex,
    endIndex,
    totalItems: endIndex,
    hasMoreBefore: startIndex > 0,
    prefixFp: `pfx@${endIndex}`,
    startFp: `start@${startIndex}`,
  };
}

/** The server's delta: everything from `fromIndex` to `endIndex`. */
function delta(fromIndex: number, endIndex: number, suffix = "", sig = "v2"): TranscriptDelta {
  return {
    sig,
    items: itemsFrom(fromIndex, endIndex, suffix),
    fromIndex,
    endIndex,
    totalItems: endIndex,
    prefixFp: `pfx@${endIndex}`,
  };
}

test("an adjacent history page is prepended to the transcript window", () => {
  const current = held(2, 4);
  const history: TranscriptHistoryPage = {
    sig: "v1",
    items: itemsFrom(0, 2),
    startIndex: 0,
    endIndex: 2,
    totalItems: 4,
    hasMoreBefore: false,
    startFp: "start@0",
  };

  const merged = prependHistoryPage(current, history);

  assert.deepEqual(texts(merged), ["0", "1", "2", "3"]);
  assert.deepEqual(
    {
      startIndex: merged.startIndex,
      endIndex: merged.endIndex,
      totalItems: merged.totalItems,
      hasMoreBefore: merged.hasMoreBefore,
      startFp: merged.startFp,
      prefixFp: merged.prefixFp,
    },
    {
      startIndex: 0,
      endIndex: 4,
      totalItems: 4,
      hasMoreBefore: false,
      startFp: "start@0",
      prefixFp: "pfx@4",
    },
  );
});

// What used to fail with "session changed while loading history": the session
// grew between the request and the answer. The server proved the page against
// the window's start fingerprint, so a newer whole-file signature is no reason
// to refuse it any more.
test("a fingerprinted history page still meets a window that grew meanwhile", () => {
  const grown = applyTranscriptDelta(held(2, 4), delta(4, 6));
  assert.ok(grown);
  assert.equal(grown.sig, "v2");
  const page: TranscriptHistoryPage = {
    sig: "v1",
    items: itemsFrom(0, 2),
    startIndex: 0,
    endIndex: 2,
    totalItems: 4,
    hasMoreBefore: false,
    startFp: "start@0",
  };

  const merged = prependHistoryPage(grown, page);
  assert.notStrictEqual(merged, grown);
  assert.deepEqual(texts(merged), ["0", "1", "2", "3", "4", "5"]);
  assert.equal(merged.endIndex, 6);
});

test("a history page from a server without fingerprints still needs the same signature", () => {
  const grown = applyTranscriptDelta(held(2, 4), delta(4, 5));
  assert.ok(grown);
  const legacy: TranscriptHistoryPage = {
    sig: "v1",
    items: itemsFrom(0, 2),
    startIndex: 0,
    endIndex: 2,
    totalItems: 4,
    hasMoreBefore: false,
  };
  assert.strictEqual(prependHistoryPage(grown, legacy), grown);
});

test("prepend compensation uses the scroll position at response time", () => {
  const requestStartScrollTop = 40;
  const responseScrollTop = 180;

  assert.equal(scrollTopAfterPrepend(responseScrollTop, 1_000, 1_400), 580);
  assert.notEqual(
    scrollTopAfterPrepend(responseScrollTop, 1_000, 1_400),
    requestStartScrollTop + 400,
  );
});

test("stale and duplicate history pages are ignored", () => {
  const current = held(2, 4);
  const acceptedPage: TranscriptHistoryPage = {
    sig: "v1",
    items: itemsFrom(0, 2),
    startIndex: 0,
    endIndex: 2,
    totalItems: 4,
    hasMoreBefore: false,
    startFp: "start@0",
  };
  const stalePage: TranscriptHistoryPage = {
    sig: "v1",
    items: [item("stale")],
    startIndex: 0,
    endIndex: 1,
    totalItems: 4,
    hasMoreBefore: false,
    startFp: "start@0",
  };

  assert.strictEqual(prependHistoryPage(current, stalePage), current);

  const merged = prependHistoryPage(current, acceptedPage);
  assert.strictEqual(prependHistoryPage(merged, acceptedPage), merged);
});

test("load earlier proves its prefix with the start fingerprint, or the resume marker once trimmed", () => {
  assert.deepEqual(historyQueryFor(held(200, 400)), { before: 200, fp: "start@200" });
  const trimmed = { ...held(200, 400), startFp: null };
  assert.deepEqual(historyQueryFor(trimmed), { before: 200, fp: "pfx@400", since: 400 });
  const unknown = { ...trimmed, prefixFp: null };
  assert.deepEqual(historyQueryFor(unknown), { before: 200 });
});

// ---------------------------------------------------------------- deltas

test("a pure append extends the window and keeps what it held by reference", () => {
  const current = held(2, 4);
  const next = applyTranscriptDelta(current, delta(4, 6));
  assert.ok(next);
  assert.deepEqual(texts(next), ["2", "3", "4", "5"]);
  assert.strictEqual(next.items[0], current.items[0]);
  assert.equal(next.endIndex, 6);
  assert.equal(next.totalItems, 6);
  assert.equal(next.sig, "v2");
  assert.equal(next.prefixFp, "pfx@6", "the resume marker moves with the window");
  assert.equal(next.startFp, "start@2", "the start did not move");
});

test("an overlapping delta replaces the items it re-sends instead of duplicating them", () => {
  const current = held(2, 10);
  // A resume (or an in-place modification) re-sends 7..10 and adds 10..12.
  const next = applyTranscriptDelta(current, delta(7, 12, "'"));
  assert.ok(next);
  assert.deepEqual(texts(next), ["2", "3", "4", "5", "6", "7'", "8'", "9'", "10'", "11'"]);
  assert.equal(next.items.length, next.endIndex - next.startIndex);
  assert.strictEqual(next.items[4], current.items[4], "items before the overlap are untouched");
});

test("a delta that ends before the window did truncates it", () => {
  // The file shrank by less than the overlap: the server's version is shorter.
  const next = applyTranscriptDelta(held(2, 10), delta(7, 8, "'"));
  assert.ok(next);
  assert.deepEqual(texts(next), ["2", "3", "4", "5", "6", "7'"]);
  assert.equal(next.endIndex, 8);
  assert.equal(next.totalItems, 8);
});

test("a delta that starts before the window replaces the whole of it", () => {
  const next = applyTranscriptDelta(held(200, 400), delta(150, 402, "'"));
  assert.ok(next);
  assert.equal(next.startIndex, 150);
  assert.equal(next.endIndex, 402);
  assert.equal(next.items.length, 252);
  assert.ok(next.hasMoreBefore);
  assert.equal(next.startFp, null, "nobody said what the prefix before 150 fingerprints to");
});

test("a delta with a gap is never applied, and asks for a resync instead", () => {
  const current = held(2, 4);
  // Starts past the end: the client cannot know what goes in between.
  assert.equal(applyTranscriptDelta(current, delta(5, 6)), null);
  // The request carries what the client does hold, so the answer can fit it.
  assert.deepEqual(JSON.parse(resyncMessage(current)), {
    type: "resync",
    since: 4,
    fp: "pfx@4",
  });
  // Without a marker it can only ask for a fresh window.
  assert.deepEqual(JSON.parse(resyncMessage(null)), { type: "resync" });
  assert.deepEqual(JSON.parse(resyncMessage({ endIndex: 4, prefixFp: null })), {
    type: "resync",
  });
});

test("a malformed delta is never applied either", () => {
  const current = held(2, 4);
  assert.equal(applyTranscriptDelta(current, { ...delta(4, 6), totalItems: 5 }), null);
  assert.equal(applyTranscriptDelta(current, { ...delta(4, 6), items: [item("x")] }), null);
  assert.equal(applyTranscriptDelta(current, { ...delta(4, 6), fromIndex: -1 }), null);
  assert.equal(applyTranscriptDelta(current, transcriptDeltaOf({ type: "delta" })), null);
});

test("an empty delta advances the transcript signature", () => {
  const current = held(2, 4);
  const next = applyTranscriptDelta(current, delta(4, 4));
  assert.ok(next);
  assert.notStrictEqual(next, current);
  assert.equal(next.sig, "v2");
  assert.strictEqual(next.items, current.items);
});

test("a delta from a server that predates endIndex runs to its total", () => {
  const frame = { type: "delta", sig: "v2", previousSig: "v1", fromIndex: 4, items: [item("4")], totalItems: 5 };
  const parsed = transcriptDeltaOf(frame);
  assert.equal(parsed.endIndex, 5);
  assert.equal(parsed.prefixFp, null);
  const next = applyTranscriptDelta(held(2, 4), parsed);
  assert.ok(next);
  assert.equal(next.endIndex, 5);
  assert.equal(next.prefixFp, null, "a window with no marker reconnects without one");
  assert.equal(resumeQuery(next), "");
});

// ---------------------------------------------------------------- resume

// The reconnect a phone on a bad network makes all day: the window, with its
// earlier pages, survives, and only the overlap and what is new arrives.
test("a window and its loaded history are kept across a resume", () => {
  // A snapshot of the last page, then two earlier pages loaded by scrolling up.
  let window = held(400, 600);
  for (const start of [200, 0]) {
    window = prependHistoryPage(window, {
      sig: "v1",
      items: itemsFrom(start, start + 200),
      startIndex: start,
      endIndex: start + 200,
      totalItems: 600,
      hasMoreBefore: start > 0,
      startFp: `start@${start}`,
    });
  }
  assert.equal(window.startIndex, 0);
  const oldest = window.items[0];

  // Reconnect: the marker goes in the URL...
  assert.equal(resumeQuery(window), "?since=600&fp=pfx%40600");
  // ...and the server answers with the overlap (K = 3) plus five new items.
  const resumed = applyTranscriptDelta(window, delta(597, 605, "'", "v3"));
  assert.ok(resumed);
  assert.equal(resumed.startIndex, 0, "the earlier pages are still there");
  assert.strictEqual(resumed.items[0], oldest, "and are the very same objects");
  assert.equal(resumed.endIndex, 605);
  assert.equal(resumed.items.length, 605);
  assert.deepEqual(texts(resumed).slice(595), ["595", "596", "597'", "598'", "599'", "600'", "601'", "602'", "603'", "604'"]);
  assert.equal(resumed.startFp, "start@0");
  assert.equal(resumeQuery(resumed), "?since=605&fp=pfx%40605");
});

test("a reset replaces the window, earlier pages and all", () => {
  let window = held(400, 600);
  window = prependHistoryPage(window, {
    sig: "v1",
    items: itemsFrom(200, 400),
    startIndex: 200,
    endIndex: 400,
    totalItems: 600,
    hasMoreBefore: true,
    startFp: "start@200",
  });
  assert.equal(window.startIndex, 200);

  const reset = transcriptWindowOf({
    type: "reset",
    sig: "v9",
    items: itemsFrom(410, 610, "*"),
    startIndex: 410,
    endIndex: 610,
    totalItems: 610,
    hasMoreBefore: true,
    omittedItems: 410,
    prefixFp: "pfx@610*",
    startFp: "start@410*",
  });
  assert.ok(reset);
  assert.equal(reset.startIndex, 410);
  assert.equal(reset.items.length, 200);
  assert.equal(reset.items[0]?.text, "410*");
  assert.equal(reset.prefixFp, "pfx@610*");
  assert.equal(reset.startFp, "start@410*");
  assert.equal(reset.sig, "v9");

  assert.equal(transcriptWindowOf({ type: "reset" }), null, "a frame without a signature is not one");
});

/** A window holding `count` items, ending at `endIndex` of a longer session. */
function window(count: number, endIndex: number, totalItems = endIndex): TranscriptWindow {
  const startIndex = endIndex - count;
  return {
    sig: "v1",
    items: Array.from({ length: count }, (_, i) => item(String(startIndex + i))),
    startIndex,
    endIndex,
    totalItems,
    hasMoreBefore: startIndex > 0,
    prefixFp: `pfx@${endIndex}`,
    startFp: `start@${startIndex}`,
  };
}

/** The invariant `isValidRange` enforces on anything the server sends. */
function assertConsistent(w: TranscriptWindow): void {
  assert.ok(w.startIndex >= 0, "startIndex is not negative");
  assert.equal(w.items.length, w.endIndex - w.startIndex, "length matches the range");
  assert.ok(w.totalItems >= w.endIndex, "the total covers the loaded range");
  assert.equal(w.hasMoreBefore, w.startIndex > 0, "hasMoreBefore is derived from the range");
}

test("a window under the trim point is left exactly as it is", () => {
  // Identity, not equality: an untouched window must cost no render.
  const small = window(LIVE_WINDOW_CAP, 900);
  assert.strictEqual(trimLiveWindow(small), small);
  const atThreshold = window(LIVE_WINDOW_CAP + LIVE_WINDOW_SLACK, 2000);
  assert.strictEqual(trimLiveWindow(atThreshold), atThreshold);
});

test("a trimmed window keeps its indices consistent", () => {
  const grown = window(LIVE_WINDOW_CAP + LIVE_WINDOW_SLACK + 1, 5000, 5000);
  const trimmed = trimLiveWindow(grown);

  assert.equal(trimmed.items.length, LIVE_WINDOW_CAP);
  // The cut is at the front, so the newest item is untouched — this is what
  // makes a trim invisible to someone following the tail.
  assert.strictEqual(trimmed.items.at(-1), grown.items.at(-1));
  assert.equal(trimmed.startIndex, grown.startIndex + (grown.items.length - LIVE_WINDOW_CAP));
  assert.equal(trimmed.endIndex, grown.endIndex);
  assert.equal(trimmed.totalItems, grown.totalItems);
  assert.equal(trimmed.sig, grown.sig);
  assert.ok(trimmed.hasMoreBefore);
  assertConsistent(trimmed);
  // The first kept item is the one the new startIndex names.
  assert.equal(trimmed.items[0]?.text, String(trimmed.startIndex));
  // Nobody said what the prefix up to the cut fingerprints to, so "load
  // earlier" falls back on the resume marker, which a trim does not move.
  assert.equal(trimmed.startFp, null);
  assert.equal(trimmed.prefixFp, grown.prefixFp);
  assert.deepEqual(historyQueryFor(trimmed), {
    before: trimmed.startIndex,
    fp: grown.prefixFp,
    since: grown.endIndex,
  });
});

test("earlier history still meets a trimmed window", () => {
  const trimmed = trimLiveWindow(window(LIVE_WINDOW_CAP + LIVE_WINDOW_SLACK + 1, 5000, 5000));
  // What `loadEarlier` asks for: the page ending at the window's first index.
  const before = trimmed.startIndex;
  const page: TranscriptHistoryPage = {
    sig: "v1",
    items: Array.from({ length: 200 }, (_, i) => item(String(before - 200 + i))),
    startIndex: before - 200,
    endIndex: before,
    totalItems: 5000,
    hasMoreBefore: true,
    startFp: `start@${before - 200}`,
  };

  const restored = prependHistoryPage(trimmed, page);
  assert.notStrictEqual(restored, trimmed, "the page was accepted, not refused");
  assert.equal(restored.startIndex, before - 200);
  assert.equal(restored.endIndex, trimmed.endIndex);
  assert.equal(restored.startFp, `start@${before - 200}`, "and the start fingerprint is known again");
  assertConsistent(restored);
});

// The one that matters most: trimming and topping up are opposites, so a trim
// that leaves the top-up guard willing to fetch is an infinite fetch/trim loop.
test("a trim cannot make the transcript fetch back what it dropped", () => {
  const trimmed = trimLiveWindow(window(LIVE_WINDOW_CAP + LIVE_WINDOW_SLACK + 1, 5000, 5000));
  const topUp = {
    hasMoreBefore: trimmed.hasMoreBefore,
    loading: false,
    failed: false,
    startIndex: trimmed.startIndex,
    // A `startIndex` never fired for, so the repeat guard cannot be what saves
    // this — the trim moved it to somewhere nothing was ever requested from.
    alreadyLoadedFrom: -1,
    loadedItems: trimmed.items.length,
    // A box claiming the content is shorter than the viewport, which is the
    // only thing the top-up ever looked at.
    box: { scrollTop: 0, scrollHeight: 300, clientHeight: 800 },
  };
  assert.ok(!shouldTopUpHistory(topUp));

  // And it stays no as the tail grows and is trimmed again, page after page.
  let live = trimmed;
  for (let round = 0; round < 5; round++) {
    const grown = applyTranscriptDelta(live, {
      sig: live.sig,
      items: Array.from({ length: LIVE_WINDOW_SLACK + 1 }, (_, i) =>
        item(String(live.endIndex + i)),
      ),
      fromIndex: live.endIndex,
      endIndex: live.endIndex + LIVE_WINDOW_SLACK + 1,
      totalItems: live.endIndex + LIVE_WINDOW_SLACK + 1,
      prefixFp: live.prefixFp,
    });
    assert.ok(grown);
    live = trimLiveWindow(grown);
    assert.equal(live.items.length, LIVE_WINDOW_CAP);
    assertConsistent(live);
    assert.ok(
      !shouldTopUpHistory({ ...topUp, startIndex: live.startIndex, loadedItems: live.items.length }),
    );
  }
});

// The unread badge counts what landed below the viewport, by absolute index.
// A trim moves `startIndex` and nothing else, so there is nothing for it to
// miscount — and it is only ever run while pinned, where the count is zero.
test("a trim cannot disturb the unread cursor", () => {
  const grown = window(LIVE_WINDOW_CAP + LIVE_WINDOW_SLACK + 1, 5000, 5000);
  const trimmed = trimLiveWindow(grown);
  assert.equal(trimmed.endIndex, grown.endIndex);

  const away = trackUnread(NO_UNREAD, grown.endIndex, false);
  assert.equal(trackUnread(away, trimmed.endIndex, false).count, 0);
  // And the next real arrival still counts as one, not as a thousand.
  assert.equal(trackUnread(away, trimmed.endIndex + 1, false).count, 1);
});
