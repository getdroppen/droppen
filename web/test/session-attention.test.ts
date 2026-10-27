// The decisions behind "which sessions are working, and how much is unread":
// one state per row where only one fits, the chip's words, the folder and
// screen roll-ups, a pane's frame, and the tab title.
import assert from "node:assert/strict";
import test from "node:test";
import type { PaneNode } from "../src/app/pane-tree.js";
import type { ResidentInfo } from "../src/features/sessions/session-activity.js";
import {
  activityChip,
  attentionState,
  chipText,
  formatUnread,
  groupAttention,
  paneAttention,
  readLocally,
  screenAttention,
  sessionsInLayout,
  strongestState,
  stripTabCount,
  tabTitle,
  totalUnread,
  unreadCountOf,
  type AttentionRow,
} from "../src/features/sessions/session-attention.js";
import { LOCAL } from "../src/lib/machines/key.js";
import { resident } from "./session-fixtures.js";

const idle: AttentionRow = { status: "idle", activity: "idle", unreadCount: 0 };
const working: AttentionRow = { status: "running", activity: "working", resident: resident({ phase: "working" }), unreadCount: 0 };
const queued: AttentionRow = { status: "running", activity: "queued", resident: resident({ queued: 2 }), unreadCount: 0 };
const background: AttentionRow = { status: "idle", activity: "background", resident: resident({ background: 3 }), unreadCount: 0 };
// A turn that finished: a live process with nothing in flight. The server
// calls this `waiting`, and it is true of nearly every session a resident keeps
// alive — so it is idle here, and draws nothing.
const finished: AttentionRow = { status: "waiting", activity: "idle", unreadCount: 0 };
const asking: AttentionRow = { status: "waiting", activity: "idle", asking: true, unreadCount: 0 };
const withUnread = (row: AttentionRow, unreadCount: number): AttentionRow => ({ ...row, unreadCount, unread: unreadCount > 0 });

/* ── one state per row ─────────────────────────────────────────────────── */

test("the strongest state wins: asking > working > unread > background > idle", () => {
  assert.equal(attentionState(withUnread(asking, 4)), "asking", "a question for you outranks everything");
  assert.equal(attentionState({ ...withUnread(working, 4), asking: true }), "asking", "even a row that also says it works");
  assert.equal(attentionState(withUnread(working, 4)), "working", "a turn in flight outranks what it already said");
  assert.equal(attentionState(withUnread(queued, 1)), "working", "a queue is work that has not finished");
  assert.equal(attentionState(withUnread(background, 1)), "unread", "background work must not hide an answer");
  assert.equal(attentionState(background), "background");
  assert.equal(attentionState(withUnread(idle, 2)), "unread");
  assert.equal(attentionState(idle), "idle");
  assert.equal(strongestState(["idle", "background", "unread", "working"]), "working");
  assert.equal(strongestState(["unread", "asking", "working"]), "asking");
  assert.equal(strongestState([]), "idle", "an empty folder is calm");
});

test("a turn that simply finished looks idle everywhere", () => {
  // The reported bug: every stopped session wore the amber "waiting for you".
  assert.equal(attentionState(finished), "idle");
  assert.equal(activityChip(finished), null, "no chip");
  assert.equal(attentionState(withUnread(finished, 2)), "unread", "its new answers are still unread — in violet");
  assert.equal(attentionState({ status: "waiting" }), "idle", "an older server, which sends neither field");
});

test("an older server's bare flag reads as one unread message", () => {
  assert.equal(unreadCountOf({ status: "idle", unread: true }), 1);
  assert.equal(unreadCountOf({ status: "idle", unread: false }), 0);
  assert.equal(unreadCountOf({ status: "idle", unread: false, unreadCount: 7 }), 7, "the count, where there is one");
});

/* ── words ─────────────────────────────────────────────────────────────── */

test("a count says 99+ from the cap up, and nothing for none", () => {
  assert.equal(formatUnread(0), "");
  assert.equal(formatUnread(1), "1");
  assert.equal(formatUnread(98), "98");
  assert.equal(formatUnread(99), "99+", "the server caps at 99, so 99 means at least that");
  assert.equal(formatUnread(312), "99+", "and a folder's sum can run past it");
});

test("the chip says what the row is doing, with its number", () => {
  const now = Date.parse("2026-10-10T10:00:00.000Z");
  const longTurn: AttentionRow = {
    ...working,
    resident: resident({ phase: "working", turnStartedAt: "2026-10-10T09:56:30.000Z", queued: 1 }),
  };
  assert.equal(chipText(activityChip(longTurn, now)!), "working · 3m · +1");
  const freshTurn: AttentionRow = { ...working, resident: resident({ phase: "working", turnStartedAt: "2026-10-10T09:59:40.000Z" }) };
  assert.equal(chipText(activityChip(freshTurn, now)!), "working", "under a minute: the dot says it has just started");
  assert.equal(chipText(activityChip(working, now)!), "working", "no start time from a codex driver: no duration");
  assert.equal(chipText(activityChip(queued, now)!), "queued 2");
  assert.equal(chipText(activityChip(background, now)!), "background 3");
  assert.equal(chipText(activityChip(asking, now)!), "asking you");
  assert.equal(activityChip(idle, now), null, "an idle row says nothing");
  assert.equal(
    chipText(activityChip(queued, now)!, (key) => ({ queued: "排队" })[key] ?? key),
    "排队 2",
    "the label is translated, the count is not",
  );
});

/* ── roll-ups ──────────────────────────────────────────────────────────── */

test("a folder counts its working and asking sessions and sums their unread", () => {
  const inside = groupAttention([withUnread(working, 3), queued, withUnread(idle, 120), withUnread(asking, 1), finished, background]);
  assert.equal(inside.working, 2, "a queue counts as working");
  assert.equal(inside.asking, 1, "only the question; the finished turn is not counted");
  assert.equal(inside.unread, 124, "summed, and formatted as 99+ by whoever draws it");
  assert.equal(inside.strongest, "asking");
  assert.deepEqual(groupAttention([idle, finished]), { working: 0, asking: 0, unread: 0, strongest: "idle" });
});

test("a screen sums the sessions on its panes, and says whether any works", () => {
  const layout: PaneNode = {
    type: "split",
    id: "s1",
    direction: "row",
    ratio: 0.5,
    children: [
      { type: "leaf", id: "a", content: { kind: "session", source: "claude", id: "one" } },
      {
        type: "split",
        id: "s2",
        direction: "column",
        ratio: 0.5,
        children: [
          { type: "leaf", id: "b", content: { kind: "terminal", id: "t" } },
          { type: "leaf", id: "c", content: { kind: "session", source: "codex", id: "two" } },
        ],
      },
    ],
  };
  assert.deepEqual(sessionsInLayout(layout, LOCAL), [
    { machine: LOCAL, source: "claude", id: "one" },
    { machine: LOCAL, source: "codex", id: "two" },
  ]);
  assert.deepEqual(screenAttention([withUnread(working, 2), withUnread(idle, 5), undefined]), { working: true, asking: 0, unread: 7 });
  assert.deepEqual(screenAttention([asking, asking, finished]), { working: false, asking: 2, unread: 0 }, "a count, and only of questions");
  assert.deepEqual(sessionsInLayout(null, LOCAL), []);
});

test("a session open in two panes of one screen is counted once", () => {
  const twice: PaneNode = {
    type: "split",
    id: "s",
    direction: "row",
    ratio: 0.5,
    children: [
      { type: "leaf", id: "a", content: { kind: "session", source: "claude", id: "one" } },
      { type: "leaf", id: "b", content: { kind: "session", source: "claude", id: "one" } },
    ],
  };
  assert.deepEqual(sessionsInLayout(twice, LOCAL), [{ machine: LOCAL, source: "claude", id: "one" }]);
});

/* ── a pane's frame ────────────────────────────────────────────────────── */

test("a pane that is focused and scrolled to the bottom is being read", () => {
  const row = withUnread(idle, 3);
  assert.deepEqual(paneAttention({ row, focused: true, atBottom: true }), { state: "idle", unread: 0 }, "read: no frame, no count");
  assert.deepEqual(paneAttention({ row, focused: false, atBottom: true }), { state: "unread", unread: 3 }, "beside the one being read");
  assert.deepEqual(paneAttention({ row, focused: true, atBottom: false }), { state: "unread", unread: 3 }, "scrolled up into history");
});

test("a pane frames a question and work over unread, and keeps the count", () => {
  assert.deepEqual(paneAttention({ row: withUnread(working, 2), focused: false, atBottom: true }), { state: "working", unread: 2 });
  assert.deepEqual(paneAttention({ row: withUnread(working, 2), focused: true, atBottom: true }), { state: "working", unread: 0 });
  assert.deepEqual(paneAttention({ row: asking, focused: true, atBottom: true }), { state: "asking", unread: 0 });
  assert.deepEqual(paneAttention({ row: withUnread(asking, 2), focused: false, atBottom: true }), { state: "asking", unread: 2 });
  assert.deepEqual(paneAttention({ row: finished, focused: false, atBottom: true }), { state: "idle", unread: 0 }, "a finished turn: no frame");
  assert.deepEqual(paneAttention({ row: background, focused: false, atBottom: true }), { state: "idle", unread: 0 }, "background work stays calm");
  assert.deepEqual(paneAttention({ row: undefined, focused: false, atBottom: false }), { state: "idle", unread: 0 }, "not a session");
});

/* ── the tab ───────────────────────────────────────────────────────────── */

test("the tab title carries the unread total and drops it once all is read", () => {
  assert.equal(tabTitle("Droppen", 3), "(3) Droppen");
  assert.equal(tabTitle("(3) Droppen", 5), "(5) Droppen", "replaced, never stacked");
  assert.equal(tabTitle("(5) Droppen", 0), "Droppen");
  assert.equal(tabTitle("Droppen", 140), "(99+) Droppen");
  assert.equal(stripTabCount("(99+) Droppen"), "Droppen");
  assert.equal(stripTabCount("(draft) notes"), "(draft) notes", "only a count this module wrote is removed");
});

test("the total counts the sessions the sidebar lists, not every transcript on the machine", () => {
  assert.equal(
    totalUnread([
      { ...withUnread(idle, 2), scope: "owned" },
      { ...withUnread(working, 3), scope: "owned" },
      { ...withUnread(idle, 99), scope: "external" },
    ]),
    5,
  );
});

/* ── reading, optimistically ───────────────────────────────────────────── */

test("a session being read shows nothing unread until the server catches up", () => {
  type Row = Parameters<typeof readLocally>[0][number];
  const row = (id: string, unreadCount: number) =>
    ({ id, source: "claude", status: "idle", activity: "idle", scope: "owned", unread: unreadCount > 0, unreadCount }) as unknown as Row;
  const rows = [row("a", 2), row("b", 3), row("c", 0)];
  assert.equal(readLocally(rows, new Set(), LOCAL), rows, "nobody reading: the very same list");
  assert.equal(readLocally(rows, new Set(["local|claude/c"]), LOCAL), rows, "reading a row with nothing unread changes nothing");

  const read = readLocally(rows, new Set(["local|claude/a"]), LOCAL);
  assert.notEqual(read, rows);
  assert.equal(read[0]!.unreadCount, 0);
  assert.equal(read[0]!.unread, false);
  assert.equal(read[1], rows[1], "the others are the same objects");
  assert.equal(readLocally(rows, new Set(["local|claude/a"]), LOCAL)[0], read[0], "and the read copy is stable while its row is");
  assert.equal(totalUnread(read), 3, "so the tab title, folders and rail stop counting it at once");
  assert.equal(attentionState(read[0]!), "idle");
});
