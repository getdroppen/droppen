// Whether the floating background panel lists its children or folds to one line.
//
// The bug being fixed was a phone: a lead session minding a dozen agents drew a
// dozen rows of monospace over the transcript, with no way to put them away. So
// what is pinned here is the fold *and* the two things it must not break — a
// short list still opens with no extra tap, and one global preference must not
// leak a busy session's choice into a quiet one.
import assert from "node:assert/strict";
import test from "node:test";
import {
  BACKGROUND_FOLD_THRESHOLD,
  BACKGROUND_PANEL_OPEN_KEY,
  backgroundFold,
  oldestElapsedMs,
  readBackgroundPanelOpen,
  writeBackgroundPanelOpen,
  type FoldStorage,
} from "../src/features/sessions/background-prefs.js";
import { MemoryStorage } from "./storage.js";

const blocked: FoldStorage = {
  getItem: () => {
    throw new Error("blocked");
  },
  setItem: () => {
    throw new Error("blocked");
  },
};

/* ── the fold decision ──────────────────────────────────────────────────── */

test("a short list is open, with no fold control to reach past", () => {
  for (let rows = 0; rows <= BACKGROUND_FOLD_THRESHOLD; rows++) {
    assert.deepEqual(
      backgroundFold(rows, null),
      { foldable: false, open: true },
      `${rows} rows fit; folding them would cost a tap to read what already fits`,
    );
  }
});

test("past the threshold the list folds itself shut", () => {
  assert.deepEqual(backgroundFold(BACKGROUND_FOLD_THRESHOLD + 1, null), {
    foldable: true,
    open: false,
  });
  assert.deepEqual(backgroundFold(20, null), { foldable: true, open: false });
});

test("a stored preference overrides the count in both directions", () => {
  assert.deepEqual(backgroundFold(12, true), { foldable: true, open: true });
  assert.deepEqual(backgroundFold(12, false), { foldable: true, open: false });
});

test("a stored fold does not follow you into a session that never needed one", () => {
  // The cost of one global key, declined: somebody who folded a twelve-agent
  // fan-out has said nothing about the two-row panel in the next session, and
  // making them tap to read two rows would be the fold breaking its own reason
  // for existing.
  assert.deepEqual(backgroundFold(2, false), { foldable: false, open: true });
  assert.deepEqual(backgroundFold(2, true), { foldable: false, open: true });
});

/* ── the stored preference ──────────────────────────────────────────────── */

test("nothing stored is no preference, not a preference for closed", () => {
  assert.equal(readBackgroundPanelOpen(new MemoryStorage()), null);
});

test("the fold survives a reload, either way round", () => {
  const storage = new MemoryStorage();
  writeBackgroundPanelOpen(storage, false);
  assert.equal(storage.getItem(BACKGROUND_PANEL_OPEN_KEY), "0");
  assert.equal(readBackgroundPanelOpen(storage), false);
  writeBackgroundPanelOpen(storage, true);
  assert.equal(storage.getItem(BACKGROUND_PANEL_OPEN_KEY), "1");
  assert.equal(readBackgroundPanelOpen(storage), true);
});

test("a value this build does not recognise is no preference at all", () => {
  // An older or newer build, or a hand-edited key: guessing what it meant would
  // be worse than letting the row count decide, which is always defensible.
  const storage = new MemoryStorage();
  storage.setItem(BACKGROUND_PANEL_OPEN_KEY, "yes");
  assert.equal(readBackgroundPanelOpen(storage), null);
});

test("storage that throws is storage that has no opinion", () => {
  assert.equal(readBackgroundPanelOpen(blocked), null);
  assert.doesNotThrow(() => writeBackgroundPanelOpen(blocked, true));
});

/* ── the number the folded line carries ─────────────────────────────────── */

const NOW = Date.parse("2026-08-31T09:30:00.000Z");

test("the folded line reports the oldest child, not the newest", () => {
  const oldest = oldestElapsedMs(
    [
      { startedAt: "2026-08-31T09:29:00.000Z" },
      { startedAt: "2026-08-31T08:30:00.000Z" },
      { startedAt: "2026-08-31T09:00:00.000Z" },
    ],
    NOW,
  );
  assert.equal(oldest, 60 * 60_000, "an hour: the one that might be wedged");
});

test("the CLI's own duration beats now-minus-startedAt", () => {
  // `startedAt` is when this *server* first heard of the task, so after a
  // restart every child claims to have begun at the restart.
  const oldest = oldestElapsedMs(
    [{ startedAt: "2026-08-31T09:29:00.000Z", durationMs: 90 * 60_000 }],
    NOW,
  );
  assert.equal(oldest, 90 * 60_000);
});

test("children nobody can date are skipped rather than counted as ancient", () => {
  assert.equal(oldestElapsedMs([{ startedAt: null }, { startedAt: "nonsense" }], NOW), null);
  assert.equal(
    oldestElapsedMs([{ startedAt: null }, { startedAt: "2026-08-31T09:20:00.000Z" }], NOW),
    10 * 60_000,
    "one datable child is still an answer",
  );
});

test("no children at all is no number, not a zero", () => {
  // The panel drawn for a count with no rows has nothing to time, and "oldest
  // <1m" there would be an invention.
  assert.equal(oldestElapsedMs([], NOW), null);
});
