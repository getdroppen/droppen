// The questions an agent stops to ask, from the answer a card builds to the
// card it draws.
//
// Two halves. The pure one is what an answer *is*: which keys, which shape, and
// when a tap is the whole answer — the server checks every key against the
// exact question text, so a body that is wrong here is a `400` there. The
// rendered one is what each state of a question looks like, drawn with
// `react-dom/server` and no DOM, like `transcript-rows.test.tsx`: an open
// question offers its options, an "Other" field and a way to decline; a
// settled one says who settled it and when, and offers nothing.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { components } from "../src/api/schema.js";
import { ApiError } from "../src/api/errors.js";
import { formatTime } from "../src/lib/format.js";
import { AnswerNotice, QuestionCard, QuestionCardView } from "../src/features/sessions/question-card.js";
import {
  answerBody,
  answersOnTap,
  composerAnswersQuestion,
  EMPTY_DRAFT,
  effectiveQuestion,
  questionIsOpen,
  questionsByToolUse,
  sentOutcome,
  settledQuestionOf,
  togglePick,
  typeOther,
  type QuestionEntry,
  type SessionQuestion,
} from "../src/features/sessions/session-questions.js";
import { makeI18n, pageDraw } from "./render.js";

type TranscriptItem = components["schemas"]["TranscriptItem"];

// English keys are the source strings, so an empty resource bundle renders the
// same text the untranslated UI does — and asserting on it asserts on the key.
const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} } }
});

// The card holds a mutation it never fires here; `useMutation` needs a client.
const queryClient = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
const session = { source: "claude" as const, id: "s1" };

const draw = pageDraw(i18n, (node) => <QueryClientProvider client={queryClient}>{node}</QueryClientProvider>);

function visibleText(html: string): string {
  return html.replace(/<[^>]*>/g, "");
}

const colour: QuestionEntry = {
  question: "Which colour do you prefer?",
  header: "Colour",
  multiSelect: false,
  options: [
    { label: "Red", description: "Warm" },
    { label: "Blue", description: "Cool" },
  ],
};

const toppings: QuestionEntry = {
  question: "Which toppings?",
  header: "Toppings",
  multiSelect: true,
  options: [
    { label: "Cheese", description: null },
    { label: "Olives", description: null },
    { label: "Basil", description: null },
  ],
};

function record(over: Partial<SessionQuestion> = {}): SessionQuestion {
  return {
    requestId: "req-1",
    toolUseId: "toolu_1",
    questions: [colour],
    state: "pending",
    answers: null,
    response: null,
    answeredBy: null,
    answeredAt: null,
    createdAt: "2026-10-10T09:00:00Z",
    ...over,
  };
}

function call(over: Partial<TranscriptItem> = {}): TranscriptItem {
  return {
    kind: "tool_use",
    role: "assistant",
    toolName: "AskUserQuestion",
    toolUseId: "toolu_1",
    text: "Which colour do you prefer?",
    detail: "Which colour do you prefer?\n- Red\n- Blue",
    meta: null,
    isError: false,
    question: [colour],
    timestamp: null,
    truncated: false,
    ...over,
  };
}

/* ── the answer body ────────────────────────────────────────────────────── */

test("a tapped option is the answer, keyed by the exact question text", () => {
  const draft = togglePick(EMPTY_DRAFT, 0, 1, false);
  assert.deepEqual(answerBody([colour], draft), { answers: { "Which colour do you prefer?": "Blue" } });
});

test("nothing picked and nothing typed is no answer at all", () => {
  assert.equal(answerBody([colour], EMPTY_DRAFT), null);
  // Whitespace is not an answer either: the server would refuse it.
  assert.equal(answerBody([colour], typeOther(EMPTY_DRAFT, 0, "   ", false)), null);
});

test("a multi-select answer is the list of labels, typed words last", () => {
  let draft = togglePick(EMPTY_DRAFT, 0, 2, true);
  draft = togglePick(draft, 0, 0, true);
  draft = typeOther(draft, 0, " Anchovies ", true);
  // In the order offered rather than the order tapped, and an array: the
  // server joins it the way the CLI wants, so the client never does.
  assert.deepEqual(answerBody([toppings], draft), {
    answers: { "Which toppings?": ["Cheese", "Basil", "Anchovies"] },
  });
  // Tapping a ticked one again unticks it.
  assert.deepEqual(answerBody([toppings], togglePick(draft, 0, 0, true)), {
    answers: { "Which toppings?": ["Basil", "Anchovies"] },
  });
});

test("for a single choice, typed words are the answer instead of an option", () => {
  const picked = togglePick(EMPTY_DRAFT, 0, 0, false);
  const typed = typeOther(picked, 0, "Green", false);
  assert.deepEqual(typed.picked[0], [], "typing un-picks, so the two cannot disagree");
  assert.deepEqual(answerBody([colour], typed), { answers: { "Which colour do you prefer?": "Green" } });
  // And picking again clears what was typed.
  const repicked = togglePick(typed, 0, 1, false);
  assert.equal(repicked.other[0], "");
  assert.deepEqual(answerBody([colour], repicked), { answers: { "Which colour do you prefer?": "Blue" } });
});

test("a card with several questions may be answered in part", () => {
  const draft = togglePick(EMPTY_DRAFT, 1, 1, true);
  assert.deepEqual(answerBody([colour, toppings], draft), { answers: { "Which toppings?": ["Olives"] } });
});

test("only one question with one answer is answered by the tap itself", () => {
  assert.equal(answersOnTap([colour]), true);
  assert.equal(answersOnTap([toppings]), false, "a multi-select question gathers its ticks first");
  assert.equal(answersOnTap([colour, colour]), false, "several questions are submitted together");
});

/* ── which record a card draws ──────────────────────────────────────────── */

test("pending and expired are open; everything else is settled", () => {
  assert.deepEqual(
    (["pending", "expired", "answered", "dismissed", "cancelled"] as const).map(questionIsOpen),
    [true, true, false, false, false],
  );
});

test("cards find their record by the tool_use id, the newer of two winning", () => {
  const older = record({ requestId: "a" });
  const newer = record({ requestId: "b" });
  const loose = record({ requestId: "c", toolUseId: null });
  const index = questionsByToolUse([older, loose, newer]);
  assert.equal(index.get("toolu_1"), newer);
  assert.equal(index.size, 1, "a record that names no call cannot be placed on a card");
  // The empty answer is one shared object, so a memo over it holds.
  assert.equal(questionsByToolUse([]), questionsByToolUse(undefined));
});

test("what the route answered beats a frame that has not caught up, and no longer", () => {
  const pending = record();
  const answered = record({ state: "answered", answers: { "Which colour do you prefer?": "Red" }, answeredBy: "bob" });
  assert.equal(effectiveQuestion(pending, null), pending);
  assert.equal(effectiveQuestion(pending, answered), answered, "settled at once, not a frame later");
  // The frame caught up — whatever it says is the authority again.
  const framed = record({ state: "dismissed" });
  assert.equal(effectiveQuestion(framed, answered), framed);
  // Another question's answer says nothing about this one.
  assert.equal(effectiveQuestion(pending, record({ requestId: "other", state: "answered" })), pending);
  // A record aged out of the list still has the last word this screen heard.
  assert.equal(effectiveQuestion(undefined, answered), answered);
});

test("a 409 question_settled hands back the question as the winner left it", () => {
  const winner = record({ state: "answered", answeredBy: "alice", answers: { "Which colour do you prefer?": "Red" } });
  const conflict = new ApiError({ error: "question_settled", hint: "alice already answered it", question: winner });
  assert.deepEqual(settledQuestionOf(conflict), winner);
  assert.equal(conflict.hint, "alice already answered it", "the ordinary fields are unchanged");
  // Any other refusal is an error to show, not a state to adopt.
  assert.equal(settledQuestionOf(new ApiError({ error: "no such question" })), null);
  assert.equal(settledQuestionOf(new ApiError({ error: "question_settled" })), null, "no question, nothing to adopt");
  assert.equal(settledQuestionOf(new Error("question_settled")), null);
  // An ApiError over a body that is not an object still constructs.
  assert.deepEqual(new ApiError("boom").body, {});
});

/* ── the composer's notice ──────────────────────────────────────────────── */

test("the composer answers while a question is pending, on either signal", () => {
  const row = { status: "running" as const, activity: "working" as const, resident: null };
  assert.equal(composerAnswersQuestion({ questions: [record()], row }), true);
  // The resident's own word can arrive a frame before the list does.
  const asking = {
    ...row,
    resident: {
      phase: "working",
      queued: 0,
      background: 0,
      awaitingBackground: false,
      asking: true,
    } as unknown as components["schemas"]["ResidentInfo"],
  };
  assert.equal(composerAnswersQuestion({ questions: [], row: asking }), true);
});

// The notice promised an answer, and the server could not take the message as
// one — another screen answered a moment before, or the session was still
// reconnecting. It went in as an ordinary message, and the screen has to say
// so rather than drop a preview that will now appear in the queue.
test("a send says whether it answered, was queued instead, or was ordinary", () => {
  assert.equal(sentOutcome({ answered: true }), "answered");
  assert.equal(sentOutcome({ answered: false }), "queued instead");
  assert.equal(sentOutcome({}), "ordinary", "no question was open");
  assert.equal(sentOutcome(undefined), "ordinary", "a server too old to say");
});

test("the composer is an ordinary composer once nothing is pending", () => {
  const row = { status: "waiting" as const, activity: "idle" as const, resident: null };
  assert.equal(composerAnswersQuestion({ questions: [], row }), false);
  assert.equal(composerAnswersQuestion({ questions: undefined, row }), false, "a server that sends no list");
  // Expired: nothing is waiting, so a message is a message.
  assert.equal(composerAnswersQuestion({ questions: [record({ state: "expired" })], row }), false);
  assert.equal(composerAnswersQuestion({ questions: [record({ state: "answered" })], row }), false);
  // The transcript's guess about a session this server does not drive is not
  // the server's word that a message will be taken as an answer.
  assert.equal(composerAnswersQuestion({ questions: [], row: { ...row, asking: true } }), false);
});

test("the notice is drawn only when it is true", () => {
  assert.equal(draw(<AnswerNotice show={false} />), "");
  assert.match(
    visibleText(draw(<AnswerNotice show />)),
    /the agent is waiting for your answer — what you send now goes in as the answer/,
  );
});

/* ── the card ───────────────────────────────────────────────────────────── */

test("a pending question offers its options, an Other field and a way to decline", () => {
  const html = draw(<QuestionCard item={call()} record={record()} session={session} />);
  assert.match(html, /data-question="pending"/);
  assert.match(visibleText(html), /waiting for your answer/);
  assert.match(html, /Red/);
  assert.match(html, /Blue/);
  assert.match(html, /Warm/, "an option's description is kept");
  assert.match(html, /placeholder="other answer…"/);
  assert.match(visibleText(html), /let the agent decide/);
  // Enabled: nothing is in flight, so the fieldset is not disabled — and a tap
  // is the answer, so there is no separate submit button and no tick boxes.
  assert.doesNotMatch(html, /<fieldset[^>]*disabled/);
  assert.doesNotMatch(visibleText(html), /send answers/);
  assert.doesNotMatch(html, /role="radio"|role="checkbox"/);
  // The Other field's own send waits for something to be typed.
  assert.match(html, /<button[^>]*disabled=""[^>]*aria-label="send answer"|<button[^>]*aria-label="send answer"[^>]*disabled=""/);
});

test("a multi-select question ticks boxes and is sent with a submit button", () => {
  const item = call({ question: [toppings] });
  const open = record({ questions: [toppings] });
  const blank = draw(<QuestionCardView item={item} question={open} onSubmit={() => {}} onDismiss={() => {}} />);
  assert.equal((blank.match(/role="checkbox"/g) ?? []).length, 3);
  assert.doesNotMatch(blank, /aria-checked="true"/);
  assert.match(blank, /<button[^>]*disabled=""[^>]*>send answers<\/button>/, "nothing ticked, nothing to send");

  const draft = togglePick(togglePick(EMPTY_DRAFT, 0, 0, true), 0, 2, true);
  const ticked = draw(
    <QuestionCardView item={item} question={open} draft={draft} onSubmit={() => {}} onDismiss={() => {}} />,
  );
  assert.equal((ticked.match(/aria-checked="true"/g) ?? []).length, 2);
  assert.match(ticked, /<button type="button" data-variant="primary" class="[^"]*">send answers<\/button>/, "and now it can go");
});

test("several single-select questions are radio groups sent together", () => {
  const both = record({ questions: [colour, toppings] });
  const html = draw(<QuestionCardView item={call({ question: [colour, toppings] })} question={both} onSubmit={() => {}} />);
  assert.match(html, /role="radiogroup"/);
  assert.equal((html.match(/role="radio"/g) ?? []).length, 2);
  assert.match(visibleText(html), /send answers/);
  // Both question texts and both Other fields.
  assert.match(html, /Which colour do you prefer\?/);
  assert.match(html, /Which toppings\?/);
  assert.equal((html.match(/placeholder="other answer…"/g) ?? []).length, 2);
});

test("an answer in flight disables the whole card", () => {
  const html = draw(<QuestionCardView item={call()} question={record()} busy onSubmit={() => {}} onDismiss={() => {}} />);
  assert.match(html, /<fieldset disabled=""/);
});

test("a refusal that is not a lost race is shown under the card", () => {
  const html = draw(
    <QuestionCardView
      item={call()}
      question={record()}
      error="the session is still reconnecting after a server restart; try again in a moment"
      onSubmit={() => {}}
    />,
  );
  assert.match(visibleText(html), /send failed: the session is still reconnecting/);
});

test("an expired question says the answer goes in as a new message, and still takes one", () => {
  const html = draw(<QuestionCard item={call()} record={record({ state: "expired" })} session={session} />);
  assert.match(html, /data-question="expired"/);
  assert.match(
    visibleText(html),
    /the session restarted after asking, so nothing is blocked on this any more — your answer will be sent as a new message/,
  );
  assert.match(html, /placeholder="other answer…"/, "still answerable");
  assert.match(visibleText(html), /let the agent decide/);
});

test("an answered question is a read-only record of who chose what, and when", () => {
  const answeredAt = "2026-10-10T09:05:00Z";
  const html = draw(
    <QuestionCard
      item={call({ question: null })}
      record={record({
        state: "answered",
        answers: { "Which colour do you prefer?": "Red" },
        response: "and make it bright",
        answeredBy: "alice",
        answeredAt,
      })}
      session={session}
    />,
  );
  const text = visibleText(html);
  assert.match(html, /data-question="answered"/);
  // The question comes from the record: the transcript stops carrying the
  // structured options once the call has an answer.
  assert.match(text, /Which colour do you prefer\?/);
  assert.match(text, /→ Red/);
  assert.match(text, /and make it bright/);
  assert.match(text, new RegExp(`answered · alice · ${formatTime(answeredAt).replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`));
  assert.doesNotMatch(html, /<button|<input/, "nothing to press");
});

test("dismissed and cancelled questions say so, and offer nothing", () => {
  const dismissed = draw(<QuestionCard item={call()} record={record({ state: "dismissed", answeredBy: "bob" })} session={session} />);
  assert.match(visibleText(dismissed), /dismissed — left to the agent · bob/);
  assert.doesNotMatch(dismissed, /<button|<input/);

  const cancelled = draw(<QuestionCard item={call()} record={record({ state: "cancelled" })} session={session} />);
  assert.match(visibleText(cancelled), /cancelled — the turn was stopped/);
  assert.doesNotMatch(cancelled, /<button|<input/);
});

test("a question with no record is drawn as the transcript wrote it", () => {
  // An external session, a transcript older than the feature, a server that
  // sends no questions: nobody is holding it open, so nothing is offered.
  const html = draw(<QuestionCard item={call()} />);
  assert.match(html, /data-question="record"/);
  assert.match(visibleText(html), /Which colour do you prefer\?\n- Red\n- Blue/);
  assert.doesNotMatch(html, /<button|<input/);
});

test("a lost race draws the winner's answer straight away", () => {
  // What the card does with a 409: adopt the question it carries, ahead of
  // the frame that will say the same thing.
  const winner = record({ state: "answered", answers: { "Which colour do you prefer?": "Blue" }, answeredBy: "alice" });
  const conflict = new ApiError({ error: "question_settled", hint: "alice already answered it", question: winner });
  const shown = effectiveQuestion(record(), settledQuestionOf(conflict));
  const html = draw(<QuestionCardView item={call()} question={shown} onSubmit={() => {}} onDismiss={() => {}} />);
  assert.match(visibleText(html), /→ Blue/);
  assert.match(visibleText(html), /answered · alice/);
  assert.doesNotMatch(html, /<button|<input/, "no buttons that can only fail again");
});
