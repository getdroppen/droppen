// A steered message, drawn at each step of the three ways it can go.
//
// The server decides where the message is: a resident takes a steered message
// off its queue the moment claude's transcript shows it folded into the turn in
// flight (`Driver::absorb` in server-rs/src/inject/resident.rs), and keeps it
// queued — delivered, waiting to start — when claude holds it for a turn of its
// own. These tests pin that every surface reads that one answer: the queue
// panel, its Stop hint, the strip above the composer and the session list chip.
// And that a transcript arriving before the state frame that says so cannot put
// the message back anywhere.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { I18nextProvider } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { zhCN } from "../src/i18n/zh-CN.js";
import { StatusBadge } from "../src/features/sessions/sessions-layout.js";
import { ActivityBar } from "../src/features/sessions/session-detail.js";
import { ServerQueueList, noticeInForce, type QueueNotice } from "../src/features/sessions/server-queue.js";
import { DeliveryPreviews } from "../src/features/sessions/delivery-preview-view.js";
import {
  changeDelivery,
  emptyDeliveryState,
  reconcileDeliveries,
  visibleDeliveries,
  type DeliveryFrame,
} from "../src/features/sessions/delivery-preview.js";
import {
  activityBanner,
  queuedMessagesOf,
  type ActivityRow,
  type QueuedMessage,
  type ResidentInfo,
} from "../src/features/sessions/session-activity.js";
import type { MessageReceipt } from "../src/features/sessions/send-receipts.js";
import { makeI18n } from "./render.js";

// The user's own language, so the assertions read like the screenshot did.
const zh = makeI18n({
  lng: "zh-CN",
  resources: { "zh-CN": { translation: zhCN } },
  keySeparator: false,
  nsSeparator: false
});

const noop = () => {};
const draw = (node: React.ReactElement) => renderToStaticMarkup(<I18nextProvider i18n={zh}>{node}</I18nextProvider>);
const AT = "2026-10-19T05:00:00.000Z";

const WAITING: QueuedMessage = { id: "q-wait", text: "把测试也修一下", steered: false };
const STEERED: QueuedMessage = { id: "q-steer", text: "先别动 main 分支", steered: true };
const receipt = (clientMessageId: string, qid: string, status: MessageReceipt["status"]): MessageReceipt =>
  ({ clientMessageId, qid, status, at: AT }) as MessageReceipt;

/** The resident as the server publishes it: `queued` always the list's length. */
function resident(messages: QueuedMessage[], phase: ResidentInfo["phase"] = "working"): ResidentInfo {
  return {
    phase,
    queued: messages.length,
    queuedMessages: messages,
    background: 0,
    tasks: [],
    awaitingBackground: false,
    stalled: false,
  } as unknown as ResidentInfo;
}

/** The notice the ⚡ leaves behind, about the message it steered. */
const steerNotice: QueueNotice = {
  tone: "info",
  qid: STEERED.id,
  text: zh.t("delivered — it runs next; Stop ends the current turn but will not hold it back"),
};

/** Every surface that counts the queue, drawn from one state frame. */
function surfaces(state: ResidentInfo, receipts: MessageReceipt[], notice: QueueNotice | null) {
  const messages = queuedMessagesOf(state);
  const row: ActivityRow = {
    status: state.phase === "idle" ? "idle" : "running",
    activity: state.phase === "idle" ? (state.queued > 0 ? "queued" : "idle") : "working",
    resident: state,
  };
  return {
    panel: draw(
      <ServerQueueList
        source="claude"
        messages={messages}
        receipts={receipts}
        pendingId={null}
        restartId={null}
        notice={noticeInForce(notice, messages)}
        onSteer={noop}
        onDrop={noop}
      />,
    ),
    strip: draw(<ActivityBar banner={activityBanner(row, false)} subtask={null} stopping={false} onStop={noop} />),
    list: draw(<StatusBadge session={row} />),
  };
}

test("not yet delivered: the message waits in the server's queue and can still be steered or withdrawn", () => {
  const { panel, strip, list } = surfaces(resident([WAITING]), [receipt("c-wait", WAITING.id, "queued")], null);
  assert.match(panel, /排队中（1）/);
  assert.match(panel, /已送达/);
  assert.match(panel, /title="插队递送：/, "⚡ is offered");
  assert.match(panel, /title="从队列移除"/, "and so is ✕");
  assert.doesNotMatch(panel, /等待开跑/);
  assert.match(strip, /1 条消息排队中/);
  assert.match(list, /工作中 · \+1/);
});

test("steered and held for the next turn: delivered, waiting to start, and still counted", () => {
  const { panel, strip, list } = surfaces(resident([STEERED]), [receipt("c-steer", STEERED.id, "sent")], steerNotice);
  assert.match(panel, /排队中（1）/);
  assert.match(panel, /已执行/);
  assert.match(panel, /已递送，等待开跑/);
  assert.match(panel, /拦不住它/, "the Stop hint is still true: it runs next");
  assert.doesNotMatch(panel, /<button/, "past the point of being taken back");
  assert.match(strip, /1 条消息排队中/);
  assert.match(list, /工作中 · \+1/);
});

test("steered and folded into the running turn: it leaves the panel, the hint and every count at once", () => {
  // The frame the server sends once the transcript holds the fold: the turn it
  // was folded into is still running, and the message is in neither list.
  const receipts = [receipt("c-wait", WAITING.id, "queued"), receipt("c-steer", STEERED.id, "sent")];
  const folded = surfaces(resident([WAITING]), receipts, steerNotice);
  assert.match(folded.panel, /排队中（1）/, "only the message that really is waiting");
  assert.match(folded.panel, /把测试也修一下/);
  assert.doesNotMatch(folded.panel, /先别动 main 分支/);
  assert.doesNotMatch(folded.panel, /等待开跑/);
  assert.doesNotMatch(folded.panel, /拦不住它/, "the hint was about the folded message, and went with it");
  assert.match(folded.strip, /1 条消息排队中/);

  const alone = surfaces(resident([]), [receipt("c-steer", STEERED.id, "sent")], steerNotice);
  assert.equal(alone.panel, "", "nothing queued, nothing to say");
  assert.doesNotMatch(alone.strip, /条消息排队中/);
  assert.match(alone.strip, /agent 正在处理你的消息/, "the turn is still running");
  assert.match(alone.list, /工作中/);
  assert.doesNotMatch(alone.list, /\+1/);
  assert.equal(noticeInForce(steerNotice, []), null);

  // And once that turn ends, nothing is left over to run: no queued chip.
  const after = surfaces(resident([], "idle"), [receipt("c-steer", STEERED.id, "sent")], steerNotice);
  assert.equal(after.panel, "");
  assert.equal(after.strip, "");
  assert.doesNotMatch(after.list, /排队中/);
});

test("the transcript showing the fold before the state frame does: nothing flickers back", () => {
  const text = STEERED.text;
  const frame = (over: Partial<DeliveryFrame>): DeliveryFrame => ({ items: [], startIndex: 10, totalItems: 10, queue: [], ...over });
  const user = { role: "user" as const, kind: "text" as const, text };
  // This tab sent it, the server queued it, and the ⚡ handed it to claude.
  let state = changeDelivery(emptyDeliveryState(10), {
    type: "start",
    entry: { id: "local:1", text, sending: true, after: 10, clientMessageId: "c-steer" },
  });
  state = changeDelivery(state, { type: "accepted", id: "local:1", queueId: STEERED.id });
  const steered = frame({ queue: [STEERED], receipts: [receipt("c-steer", STEERED.id, "sent")] });
  state = reconcileDeliveries(state, steered);
  assert.deepEqual(visibleDeliveries(state, steered.queue), [], "the queue panel is where it shows");

  // The transcript frame lands first: the fold is on screen as the user
  // speaking, while the last state frame still lists the message.
  const echoed = { ...steered, items: [user], totalItems: 11 };
  state = reconcileDeliveries(state, echoed);
  assert.deepEqual(visibleDeliveries(state, echoed.queue), []);

  // Then the state frame without it: no preview appears in its place — the
  // transcript already has it — and the panel has nothing to draw.
  const caughtUp = { ...echoed, queue: [] };
  for (let replay = 0; replay < 2; replay += 1) {
    state = reconcileDeliveries(state, caughtUp);
    const previews = visibleDeliveries(state, caughtUp.queue);
    assert.deepEqual(previews, [], "a replayed frame changes nothing either");
    assert.equal(draw(<DeliveryPreviews messages={previews} onRequeue={noop} onDismiss={noop} />), "");
  }
  assert.equal(state.entries.length, 0, "and the store has let go of it");
  assert.equal(surfaces(resident([]), [receipt("c-steer", STEERED.id, "sent")], steerNotice).panel, "");
});
