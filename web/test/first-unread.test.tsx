// The "jump to the first unread message" button: when it is up, what it asks
// the server, what it draws while the jump runs, and how it goes away.
//
// The mark it counts from is never the client's own: it is what the report an
// arrival sends replaced on the server (`previousReadAt`). A feed row can be
// stale or missing exactly when it matters — a phone waking with a dropped
// socket, a page opened from a notification — and the report is what moves
// the mark anyway, so nothing read before it can be trusted to say where the
// unread messages began.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import {
  FIRST_UNREAD_IDLE,
  firstUnreadQuery,
  firstUnreadReducer,
  firstUnreadView,
  reachedFirstUnread,
  targetOf,
  type FirstUnreadEvent,
  type FirstUnreadState,
} from "../src/features/sessions/first-unread.js";
import { FirstUnreadButton } from "../src/features/sessions/first-unread-view.js";
import { arrivesAt } from "../src/lib/sw-nav.js";
import { makeI18n, pageDraw } from "./render.js";

const READ_AT = "2026-10-10T09:00:00.000Z";

function run(...events: FirstUnreadEvent[]): FirstUnreadState {
  return events.reduce(firstUnreadReducer, FIRST_UNREAD_IDLE);
}

/** An arrival whose report came back, and the server's answer to it. */
function arrive(seq: number, target: { index: number; count: number } | null): FirstUnreadEvent[] {
  return [
    { type: "arrived", seq },
    { type: "located", seq, target },
  ];
}

/* ── when the button is up ─────────────────────────────────────────────── */

test("arriving at a session with unread messages offers the jump once the server says where", () => {
  const locating = run({ type: "arrived", seq: 1 });
  assert.equal(firstUnreadView(locating), null, "nothing to point at yet");
  assert.deepEqual(firstUnreadView(run(...arrive(1, { index: 412, count: 5 }))), {
    kind: "offer",
    target: { index: 412, count: 5 },
  });
});

test("arriving with nothing unread offers nothing", () => {
  assert.equal(firstUnreadView(run(...arrive(1, null))), null);
  assert.equal(targetOf({ index: null, count: 0 }), null);
  assert.equal(targetOf(undefined), null, "a request that failed");
  assert.deepEqual(targetOf({ index: 7, count: 2 }), { index: 7, count: 2 });
});

test("an answer for an arrival that has been replaced is ignored", () => {
  const second = run({ type: "arrived", seq: 1 }, { type: "arrived", seq: 2 });
  assert.equal(firstUnreadReducer(second, { type: "located", seq: 1, target: { index: 10, count: 9 } }), second, "stale");
  assert.deepEqual(firstUnreadView(firstUnreadReducer(second, { type: "located", seq: 2, target: { index: 30, count: 2 } })), {
    kind: "offer",
    target: { index: 30, count: 2 },
  });
});

test("coming back with the button still up keeps its target and adds what piled up since", () => {
  // The tab was left with the button up; the reader returns to more answers.
  const back = run(...arrive(1, { index: 412, count: 5 }), { type: "arrived", seq: 2 });
  assert.deepEqual(firstUnreadView(back), { kind: "offer", target: { index: 412, count: 5 } }, "up while asking");
  assert.deepEqual(firstUnreadView(firstUnreadReducer(back, { type: "located", seq: 2, target: { index: 500, count: 3 } })), {
    kind: "offer",
    target: { index: 412, count: 8 },
  });
  assert.deepEqual(firstUnreadView(firstUnreadReducer(back, { type: "located", seq: 2, target: null })), {
    kind: "offer",
    target: { index: 412, count: 5 },
  }, "nothing new: still there");
  assert.equal(firstUnreadView(firstUnreadReducer(back, { type: "reset" })), null, "but another session clears it");
});

test("reaching the message by hand retires the button", () => {
  assert.equal(firstUnreadView(run(...arrive(1, { index: 412, count: 5 }), { type: "dismiss" })), null);
});

/* ── the jump ──────────────────────────────────────────────────────────── */

test("taken, the button says the jump is running until it lands", () => {
  const jumping = run(...arrive(1, { index: 412, count: 5 }), { type: "jump" });
  assert.deepEqual(firstUnreadView(jumping), { kind: "jumping", target: { index: 412, count: 5 } });
  assert.equal(firstUnreadReducer(jumping, { type: "dismiss" }), jumping, "scrolling while it pages is not reaching it");
  assert.equal(firstUnreadReducer(jumping, { type: "arrived", seq: 2 }), jumping, "a focus mid-jump does not start over");
  assert.equal(firstUnreadView(firstUnreadReducer(jumping, { type: "landed" })), null, "landed: gone");
  assert.equal(firstUnreadView(firstUnreadReducer(jumping, { type: "cancelled" })), null, "stopped: gone");
});

test("a jump that cannot get there says so, then goes", () => {
  const failed = run(...arrive(1, { index: 412, count: 5 }), { type: "jump" }, { type: "failed" });
  assert.deepEqual(firstUnreadView(failed), { kind: "failed" });
  assert.equal(firstUnreadView(firstUnreadReducer(failed, { type: "dismiss" })), null);
});

/* ── the mark ──────────────────────────────────────────────────────────── */

test("the server is asked from the mark the arrival's report replaced", () => {
  assert.deepEqual(firstUnreadQuery(READ_AT), { after: READ_AT });
  assert.deepEqual(firstUnreadQuery(null), {}, "never read: from the start");
});

test("a notification tapped for the session on screen is an arrival at it", () => {
  assert.equal(arrivesAt("/sessions/claude/abc", "claude", "abc"), true);
  assert.equal(arrivesAt("/sessions/claude/abc/?from=push#x", "claude", "abc"), true);
  assert.equal(arrivesAt("/sessions/claude/abcd", "claude", "abc"), false);
  assert.equal(arrivesAt("/sessions/codex/abc", "claude", "abc"), false);
  assert.equal(arrivesAt(undefined, "claude", "abc"), false);
});

/* ── reaching it by hand ───────────────────────────────────────────────── */

test("the message is reached once its top is on screen, or below the top edge", () => {
  const viewportTop = 100;
  assert.equal(reachedFirstUnread(null, viewportTop), false, "not loaded: further up still");
  assert.equal(reachedFirstUnread(-2400, viewportTop), false, "above the viewport");
  assert.equal(reachedFirstUnread(97, viewportTop), true, "on the edge");
  assert.equal(reachedFirstUnread(350, viewportTop), true, "in view");
});

/* ── what it draws ─────────────────────────────────────────────────────── */

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} } }
});

const draw = pageDraw(i18n);

const noop = () => {};

test("the button says how many and where it goes, and is gone without a target", () => {
  const html = draw(<FirstUnreadButton view={{ kind: "offer", target: { index: 1, count: 5 } }} onJump={noop} onCancel={noop} />);
  assert.match(html, />5 unread · jump to the first</);
  assert.match(html, /aria-label="jump to the first unread message"/);
  assert.match(html, /bg-unread/, "in the unread colour");
  const many = draw(<FirstUnreadButton view={{ kind: "offer", target: { index: 1, count: 143 } }} onJump={noop} onCancel={noop} />);
  assert.match(many, />143 unread/, "the walk is uncapped, so the count is exact");
  assert.equal(draw(<FirstUnreadButton view={null} onJump={noop} onCancel={noop} />), "");
});

test("while the jump pages history in, it says how far and can be stopped", () => {
  const html = draw(
    <FirstUnreadButton
      view={{ kind: "jumping", target: { index: 1, count: 5 } }}
      paging={{ loaded: 2, total: 5 }}
      onJump={noop}
      onCancel={noop}
    />,
  );
  assert.match(html, /going to the first unread message…/);
  assert.match(html, />2\/5</);
  assert.match(html, /aria-label="stop"/);
});

test("a jump that failed is said, in the danger colour", () => {
  const html = draw(<FirstUnreadButton view={{ kind: "failed" }} onJump={noop} onCancel={noop} />);
  assert.match(html, /could not reach the first unread message/);
  assert.match(html, /text-danger/);
});
