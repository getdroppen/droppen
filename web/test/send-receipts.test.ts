import assert from "node:assert/strict";
import test from "node:test";
import { shouldDrainQueue, type QueueDrainState } from "../src/features/sessions/queue-drain.js";
import {
  afterSend,
  classifySendFailure,
  headSendable,
  newClientMessageId,
  outboxState,
  prepareResend,
  readOutbox,
  receiptState,
  resolveUnknowns,
  thrownFailure,
  unknownIds,
  withState,
  type MessageReceipt,
  type OutgoingMessage,
} from "../src/features/sessions/send-receipts.js";
import {
  changeDelivery,
  emptyDeliveryState,
  reconcileDeliveries,
  visibleDeliveries,
  ECHO_GRACE_ITEMS,
  type DeliveryFrame,
} from "../src/features/sessions/delivery-preview.js";
import { creationAttempt, creationRequest, type NewConversation } from "../src/features/sessions/new-session-flow.js";
import { applyStateFrame, type SessionStateFields } from "../src/features/sessions/session-feed-state.js";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

let minted = 0;
const mint = () => `id-${++minted}`;
const message = (clientMessageId: string, over: Partial<OutgoingMessage> = {}): OutgoingMessage => ({
  clientMessageId,
  text: `text of ${clientMessageId}`,
  ...over,
});
const receipt = (clientMessageId: string, status: MessageReceipt["status"], qid = `q-${clientMessageId}`): MessageReceipt => ({
  clientMessageId,
  status,
  qid,
  at: "2026-10-01T00:00:00.000Z",
});
const drain = (over: Partial<QueueDrainState> = {}): QueueDrainState => ({
  queued: 1,
  canSend: true,
  failed: false,
  sendFailed: false,
  sending: false,
  ...over,
});

// ------------------------------------------------------------- the id

test("a client message id is a v4 UUID, with or without a secure context", () => {
  assert.match(newClientMessageId(), UUID);
  // Plain-HTTP LAN origins have `getRandomValues` but no `randomUUID`; an
  // environment with neither still gets a well-formed id.
  assert.match(newClientMessageId({ getRandomValues: <T extends ArrayBufferView | null>(array: T) => array }), UUID);
  assert.match(newClientMessageId(undefined), UUID);
  const ids = new Set(Array.from({ length: 200 }, () => newClientMessageId()));
  assert.equal(ids.size, 200);
});

// ------------------------------------------------------------- the outbox on disk

test("every older outbox shape is read, and every entry gets an id it keeps", () => {
  const read = readOutbox(["plain", { text: "with files", files: ["/u/a.txt"] }, { text: "named", clientMessageId: "keep-me" }, 7, null], mint);
  assert.equal(read.length, 3);
  assert.equal(read[0]!.text, "plain");
  assert.ok(read[0]!.clientMessageId.startsWith("id-"), "a legacy entry is named on the way in");
  assert.deepEqual(read[1]!.files, ["/u/a.txt"]);
  assert.equal(read[2]!.clientMessageId, "keep-me", "an id already given is never replaced");
  assert.equal(outboxState(read[2]!), "saved");
});

test("a send in flight when the page went away comes back unknown", () => {
  const read = readOutbox([{ text: "mid-send", clientMessageId: "a", state: "sending" }, { text: "u", clientMessageId: "b", state: "unsent" }], mint);
  assert.equal(outboxState(read[0]!), "unknown", "nobody will ever hear its answer, so it must be asked about");
  assert.equal(outboxState(read[1]!), "unsent");
});

// ------------------------------------------------------------- the state machine

test("saved → sending → accepted leaves the outbox; the server's receipts say delivered, then executed", () => {
  let outbox = [message("a")];
  assert.equal(outboxState(outbox[0]!), "saved");
  outbox = withState(outbox, "a", "sending");
  assert.equal(outboxState(outbox[0]!), "sending");
  outbox = afterSend(outbox, "a", "accepted");
  assert.deepEqual(outbox, [], "accepted is the server's now");
  assert.equal(receiptState("queued"), "delivered");
  assert.equal(receiptState("sent"), "executed");
  assert.equal(receiptState("failed"), "failed");
});

test("a network error, a timeout and an unrecognisable 5xx are unknown — not failures", () => {
  assert.equal(classifySendFailure({ kind: "network" }), "unknown");
  assert.equal(classifySendFailure({ kind: "timeout" }), "unknown");
  assert.equal(classifySendFailure({ kind: "http", status: 502, body: "<html>Bad Gateway</html>" }), "unknown");
  assert.equal(classifySendFailure({ kind: "http", status: 504, body: "" }), "unknown");
  assert.equal(classifySendFailure({ kind: "http", status: 409, body: { error: "in_progress" } }), "unknown", "the server itself says the first attempt is still going");
  assert.equal(thrownFailure(new TypeError("Failed to fetch")).kind, "network");
  assert.equal(thrownFailure(Object.assign(new Error("t"), { name: "TimeoutError" })).kind, "timeout");

  const outbox = afterSend([message("a"), message("b")], "a", "unknown");
  assert.equal(outboxState(outbox[0]!), "unknown", "kept, as unknown, at the head");
  assert.equal(outbox.length, 2);
});

test("a clean 4xx is a refusal: back to saved where it was, or out if its draft was restored", () => {
  assert.equal(classifySendFailure({ kind: "http", status: 409, body: { error: "a message is already running" } }), "refused");
  assert.equal(classifySendFailure({ kind: "http", status: 400, body: { error: "text is empty" } }), "refused");
  assert.equal(classifySendFailure({ kind: "http", status: 500, body: { error: "cannot record the message" } }), "refused", "a 5xx this server wrote is still its verdict");

  const sending = withState([message("a"), message("b")], "a", "sending");
  const back = afterSend(sending, "a", "refused");
  assert.equal(outboxState(back[0]!), "saved");
  assert.equal(back[0]!.clientMessageId, "a", "same message, same id");
  assert.deepEqual(afterSend(sending, "a", "refused", true).map((m) => m.clientMessageId), ["b"], "its text is in the composer instead");
});

test("the drain never takes an unknown head, nor anything behind it", () => {
  const unknown = afterSend([message("a"), message("b")], "a", "unknown");
  assert.equal(headSendable(unknown), false);
  assert.equal(shouldDrainQueue(drain({ queued: 2, headBlocked: !headSendable(unknown) })), false);
  assert.equal(headSendable(withState(unknown, "a", "unsent")), false, "unsent waits for the user's one click");
  assert.equal(headSendable(withState(unknown, "a", "failed")), false);
  assert.equal(headSendable(withState(unknown, "a", "sending")), false);
  assert.equal(headSendable(withState(unknown, "a", "saved")), true);
  assert.equal(shouldDrainQueue(drain({ queued: 2, headBlocked: false })), true);
  assert.equal(headSendable([]), false);
});

test("reconnecting resolves an unknown send the server has into delivered or executed", () => {
  const outbox = [message("q", { state: "unknown" }), message("s", { state: "unknown" }), message("later")];
  const resolved = resolveUnknowns(outbox, { receipts: [receipt("q", "queued"), receipt("s", "sent")] });
  assert.equal(resolved.changed, true);
  assert.deepEqual(resolved.outbox.map((m) => m.clientMessageId), ["later"], "both are the server's, not this device's");
  assert.deepEqual(
    resolved.adopted.map(({ message, receipt }) => [message.clientMessageId, receiptState(receipt.status), message.state]),
    [["q", "delivered", undefined], ["s", "executed", undefined]],
  );
  assert.equal(headSendable(resolved.outbox), true, "and the queue behind them moves again");
});

test("reconnecting resolves an unknown send the server never saw into resendable, under the same id", () => {
  const outbox = [message("lost", { state: "unknown" }), message("pending", { state: "unknown" })];
  const resolved = resolveUnknowns(outbox, { receipts: [], unknown: ["lost"] });
  assert.equal(outboxState(resolved.outbox[0]!), "unsent");
  assert.equal(outboxState(resolved.outbox[1]!), "unknown", "in neither list: still being accepted, ask again");

  const resend = prepareResend(resolved.outbox[0]!, mint);
  assert.equal(resend.clientMessageId, "lost", "the server dedupes by it, so reusing it is what makes resending safe");
  assert.equal(outboxState(resend), "saved");
});

test("an unknown send the server dropped is failed, and is resent as a new message", () => {
  const resolved = resolveUnknowns([message("dropped", { state: "unknown" })], { receipts: [receipt("dropped", "failed")] });
  assert.equal(outboxState(resolved.outbox[0]!), "failed");
  const resend = prepareResend(resolved.outbox[0]!, () => "fresh");
  assert.equal(resend.clientMessageId, "fresh", "the old id has an answer on file and would only be replayed");
});

test("receipts never touch a message this device has not sent, and nothing changes when nothing matches", () => {
  const outbox = [message("saved"), message("other", { state: "unknown" })];
  const resolved = resolveUnknowns(outbox, { receipts: [receipt("saved", "sent")] });
  assert.equal(resolved.changed, false);
  assert.deepEqual(resolved.outbox, outbox);
  assert.deepEqual(unknownIds(outbox), ["other"]);
});

// ------------------------------------------------------------- the preview

const frame = (over: Partial<DeliveryFrame> = {}): DeliveryFrame => ({ items: [], startIndex: 10, totalItems: 10, queue: [], ...over });
const user = (text: string) => ({ role: "user" as const, kind: "text" as const, text });

test("a preview follows its receipt: delivered on acceptance, executed when a turn takes it", () => {
  let state = changeDelivery(emptyDeliveryState(10), {
    type: "start",
    entry: { id: "p", text: "hello", after: 10, sending: true, clientMessageId: "c" },
  });
  state = changeDelivery(state, { type: "accepted", id: "p", queueId: "q-c" });
  assert.equal(state.entries[0]!.receipt, "queued");
  // While the server queue lists it, the queue row shows it.
  const queue = [{ id: "q-c", text: "hello", steered: false }];
  state = reconcileDeliveries(state, frame({ queue, receipts: [receipt("c", "queued")] }));
  assert.deepEqual(visibleDeliveries(state, queue), []);
  state = reconcileDeliveries(state, frame({ receipts: [receipt("c", "sent")] }));
  assert.equal(visibleDeliveries(state, [])[0]!.receipt, "sent");
  state = reconcileDeliveries(state, frame({ items: [user("hello")], totalItems: 11, receipts: [receipt("c", "sent")] }));
  assert.deepEqual(visibleDeliveries(state, []), [], "echoed: the transcript shows it now");
});

test("an executed message folded into a running turn stops waiting for an echo that never comes", () => {
  let state = changeDelivery(emptyDeliveryState(10), {
    type: "start",
    entry: { id: "p", text: "fold me", after: 10, sending: false, clientMessageId: "c" },
  });
  state = reconcileDeliveries(state, frame({ receipts: [receipt("c", "sent")] }));
  const other = Array.from({ length: ECHO_GRACE_ITEMS }, () => ({ ...user("…"), role: "assistant" as const }));
  state = reconcileDeliveries(state, frame({ items: other, totalItems: 10 + ECHO_GRACE_ITEMS, receipts: [receipt("c", "sent")] }));
  assert.deepEqual(visibleDeliveries(state, []), []);
});

test("a claude queue row this tab did not send never becomes a preview", () => {
  const queue = [{ id: "elsewhere", text: "from another device", steered: false }];
  const state = reconcileDeliveries(emptyDeliveryState(10), frame({ queue, adoptQueue: false }));
  assert.equal(state.entries.length, 0);
  assert.equal(reconcileDeliveries(emptyDeliveryState(10), frame({ queue })).entries.length, 1, "codex still adopts them");
});

test("a message adopted after an unknown send is one preview, not two", () => {
  let state = changeDelivery(emptyDeliveryState(10), {
    type: "start",
    entry: { id: "first", text: "x", after: 10, sending: true, clientMessageId: "c", queueId: "q-c" },
  });
  state = changeDelivery(state, {
    type: "start",
    entry: { id: "second", text: "x", after: 12, sending: false, clientMessageId: "c", receipt: "queued" },
  });
  assert.equal(state.entries.length, 1);
  assert.equal(state.entries[0]!.id, "first");
  assert.equal(state.entries[0]!.receipt, "queued");
});

// ------------------------------------------------------------- the state frame

test("receipts follow state-frame semantics: replaced when sent, kept when a frame omits them", () => {
  const first = [receipt("a", "queued")];
  const previous = { receipts: first } as unknown as SessionStateFields;
  assert.equal(applyStateFrame(previous, { receipts: [receipt("a", "queued")] }), previous, "the same receipts are no change");
  assert.equal(applyStateFrame(previous, {}), previous, "a server too old to send them leaves the snapshot's");
  const next = applyStateFrame(previous, { receipts: [receipt("a", "sent")] });
  assert.equal(next.receipts[0]!.status, "sent");
});

// ------------------------------------------------------------- a new session

test("a new session's retries reuse one client message id until the request changes", () => {
  const input: NewConversation = { kind: "session", source: "claude", cwd: "~/w", text: "hi", model: "opus", permission: "default" };
  const first = creationAttempt(null, input, () => "one");
  const retry = creationAttempt(first, { ...input }, () => "two");
  assert.equal(retry.clientMessageId, "one", "pressing start again after a lost answer is the same start");
  assert.equal(creationAttempt(first, { ...input, text: "hi there" }, () => "three").clientMessageId, "three");
  const request = creationRequest(input, "one");
  assert.equal(request.path, "/api/sessions/new");
  assert.equal((request.body as { clientMessageId?: string }).clientMessageId, "one");
});
