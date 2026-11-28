/**
 * The questions an agent stops to ask (`AskUserQuestion`), as the transcript's
 * question card and the composer see them.
 *
 * A resident claude runs with `--permission-prompt-tool stdio`, so when the
 * model calls `AskUserQuestion` the CLI blocks the turn *mid-flight* and the
 * server holds the question open until somebody answers it through the API.
 * The session's state carries every recent question with what became of it
 * (`SessionQuestion`), and the card a question draws in the transcript is
 * joined to its record by the `tool_use` id both of them name.
 *
 * The card used to pre-fill the composer and send an ordinary message, which
 * was the only thing a client could do while the CLI simply refused the tool.
 * Now the answer goes *into the turn that asked*, so the agent carries on from
 * the question with the answer in hand — and the decision about what a set of
 * ticks and typed words amounts to as an answer lives here, where it can be
 * tested without a DOM.
 *
 * Pure, and free of React.
 */
import type { components } from "@/api/schema";
import { ApiError } from "@/api/errors";
import { residentOf, type ActivityRow } from "./session-activity";

export type SessionQuestion = components["schemas"]["SessionQuestion"];
export type QuestionEntry = components["schemas"]["QuestionEntry"];
export type QuestionState = components["schemas"]["QuestionState"];
export type AnswerQuestion = components["schemas"]["AnswerQuestion"];

/** The token a `409` carries when another screen (or a double tap) got there first. */
export const QUESTION_SETTLED = "question_settled";

/**
 * Whether a question can still be answered.
 *
 * `expired` is answerable too, which is the non-obvious half: the process that
 * asked is gone (a restart, a crash, a rotation), so nothing is blocked on it
 * any more — but the server still takes the answer and sends it to the session
 * as an ordinary message saying what it answers. Everything else is settled.
 */
export function questionIsOpen(state: QuestionState): boolean {
  return state === "pending" || state === "expired";
}

const NO_QUESTIONS_BY_TOOL_USE: ReadonlyMap<string, SessionQuestion> = new Map();

/**
 * The questions by the `tool_use` each one is, for the cards to look theirs up.
 *
 * The list is oldest first, so where two records name the same call — which
 * should not happen, but a replay after a restart is the kind of thing that
 * could make it — the newer one wins. A record with no `tool_use` id cannot be
 * placed on a card at all; the composer's notice still covers it.
 *
 * Called on the list the feed holds, which `applyStateFrame` keeps by identity
 * while nothing in it changes, so a memo over this rebuilds only when a
 * question actually opens or closes.
 */
export function questionsByToolUse(
  questions: readonly SessionQuestion[] | null | undefined,
): ReadonlyMap<string, SessionQuestion> {
  if (!questions?.length) return NO_QUESTIONS_BY_TOOL_USE;
  const out = new Map<string, SessionQuestion>();
  for (const question of questions) {
    if (question.toolUseId) out.set(question.toolUseId, question);
  }
  return out;
}

/**
 * The record a card should draw: the feed's, unless this screen has heard
 * something newer straight from the server.
 *
 * `local` is the question as an answer, a dismissal or a `409` left it — the
 * route answers with the record as it now stands. The state frame that says
 * the same thing follows promptly, but "promptly" is still a beat in which the
 * card would sit there looking unanswered, or a lost race in which it would
 * still offer buttons that can only fail. So a settled `local` beats a record
 * that is still open; once the frame has caught up it is the authority again,
 * whatever it says.
 */
export function effectiveQuestion(
  record: SessionQuestion | null | undefined,
  local: SessionQuestion | null | undefined,
): SessionQuestion | null {
  if (!local) return record ?? null;
  // The list ages out after a week: what the server told this screen is still
  // the last word on a card it is drawing.
  if (!record) return local;
  if (local.requestId !== record.requestId) return record;
  return questionIsOpen(record.state) && !questionIsOpen(local.state) ? local : record;
}

/**
 * The question a `409 question_settled` came back with, or null for any other
 * failure.
 *
 * The refusal carries the question as whoever got there first left it — who
 * answered, when, and with what — which is exactly what the screen that lost
 * the race should show instead of an error.
 */
export function settledQuestionOf(error: unknown): SessionQuestion | null {
  if (!(error instanceof ApiError) || error.code !== QUESTION_SETTLED) return null;
  const question = error.body.question as Partial<SessionQuestion> | null | undefined;
  if (!question || typeof question !== "object" || typeof question.requestId !== "string") return null;
  if (typeof question.state !== "string" || !Array.isArray(question.questions)) return null;
  return question as SessionQuestion;
}

/**
 * What the user has done to a card so far, by question index.
 *
 * Indexes rather than labels or question texts: nothing stops a model from
 * offering two options with the same label, or asking the same thing twice,
 * and a selection has to say which *button* was pressed.
 */
export interface QuestionDraft {
  /** The options picked for each question, in the order they were offered. */
  picked: Readonly<Record<number, readonly number[]>>;
  /** What was typed into each question's "Other" field. */
  other: Readonly<Record<number, string>>;
}

export const EMPTY_DRAFT: QuestionDraft = { picked: {}, other: {} };

/**
 * Tap an option.
 *
 * Multi-select toggles it. Single-select is a radio group with "Other" as its
 * last member: picking an option clears what was typed, and tapping the picked
 * one again clears the pick — so the two can never disagree about which one is
 * the answer.
 */
export function togglePick(draft: QuestionDraft, qi: number, oi: number, multiSelect: boolean): QuestionDraft {
  const current = draft.picked[qi] ?? [];
  const on = current.includes(oi);
  if (multiSelect) {
    const next = on ? current.filter((index) => index !== oi) : [...current, oi].sort((a, b) => a - b);
    return { ...draft, picked: { ...draft.picked, [qi]: next } };
  }
  return {
    picked: { ...draft.picked, [qi]: on ? [] : [oi] },
    other: { ...draft.other, [qi]: "" },
  };
}

/**
 * Type into a question's "Other" field.
 *
 * For a multi-select question the words are one more choice beside the ticked
 * ones. For a single-select question they are the answer *instead* of an
 * option, so typing anything un-picks the option that was picked.
 */
export function typeOther(draft: QuestionDraft, qi: number, text: string, multiSelect: boolean): QuestionDraft {
  const other = { ...draft.other, [qi]: text };
  if (multiSelect || text.trim() === "") return { ...draft, other };
  return { picked: { ...draft.picked, [qi]: [] }, other };
}

/**
 * One question's answer as the route takes it, or null for a question left blank.
 *
 * A multi-select answer is the list of labels — typed words last — and the
 * server joins it the way the CLI wants (`", "`), so no client has to know
 * that. A single-select answer is the typed words when there are any, else the
 * label picked.
 */
export function answerFor(
  entry: QuestionEntry,
  picked: readonly number[],
  other: string,
): string | string[] | null {
  const labels = picked
    .map((index) => entry.options[index]?.label)
    .filter((label): label is string => typeof label === "string" && label.trim() !== "");
  const typed = other.trim();
  if (entry.multiSelect) {
    const all = typed ? [...labels, typed] : labels;
    return all.length > 0 ? all : null;
  }
  if (typed) return typed;
  return labels[0] ?? null;
}

/**
 * The body of `POST …/questions/{requestId}/answer` for a card's draft, or null
 * when nothing has been answered yet.
 *
 * Keyed by the *exact* question text, because that is what the CLI matches its
 * answers on and what the server checks every key against. A question left
 * blank is left out rather than sent empty: the server would drop it anyway,
 * and a card with several questions may fairly be answered in part.
 */
export function answerBody(entries: readonly QuestionEntry[], draft: QuestionDraft): AnswerQuestion | null {
  const answers: Record<string, string | string[]> = {};
  entries.forEach((entry, qi) => {
    const value = answerFor(entry, draft.picked[qi] ?? [], draft.other[qi] ?? "");
    if (value !== null) answers[entry.question] = value;
  });
  return Object.keys(answers).length > 0 ? { answers } : null;
}

/**
 * Whether tapping an option should answer at once.
 *
 * Only for the card that is one question with one answer: the tap *is* the
 * whole answer, and a second "submit" tap would be ceremony. A multi-select
 * question, or several questions, needs the selections gathered first.
 */
export function answersOnTap(entries: readonly QuestionEntry[]): boolean {
  return entries.length === 1 && !entries[0]!.multiSelect;
}

/**
 * What a send's answer says became of a message typed while a question was open.
 *
 * - `answered`: it went in as the question's answer (`answered: true`). The
 *   transcript will never echo it as a message of its own; the card shows it.
 * - `queued instead`: a question was open, but could not take it — another
 *   screen answered a moment before, the turn was stopped, or the session was
 *   still reconnecting after a restart (`answered: false`). It went in as an
 *   ordinary message, queued behind the turn like any other, and what this
 *   screen believes about the question is stale: the composer's "goes in as
 *   your answer" was not true for it.
 * - `ordinary`: no question was open (the field is absent).
 */
export type SentOutcome = "answered" | "queued instead" | "ordinary";

export function sentOutcome(result: { answered?: boolean } | null | undefined): SentOutcome {
  if (result?.answered === true) return "answered";
  if (result?.answered === false) return "queued instead";
  return "ordinary";
}

/**
 * Whether what the composer sends right now goes in as an answer.
 *
 * The server turns a message sent while the agent is blocked on a question
 * into that question's free-text answer, instead of queueing it behind the
 * very turn that is waiting for it. Two sources say so, and either is enough:
 * the question list (a `pending` record) and the resident's own word
 * (`resident.asking`) — the one can arrive a frame before the other, and
 * a composer that says "queued" over a message that is about to be an answer
 * is the confusion this exists to prevent.
 *
 * Deliberately not `summary.asking` alone: for a session this server does not
 * drive that is read off the transcript, and a message sent there is just a
 * message. And not `expired` either — nothing is waiting, so a message is an
 * ordinary one, and the card says how to answer it.
 */
export function composerAnswersQuestion(input: {
  questions: readonly SessionQuestion[] | null | undefined;
  permissions?: readonly { state: string }[] | null | undefined;
  row: ActivityRow;
}): boolean {
  if (input.questions?.some((question) => question.state === "pending")) return true;
  // A pending tool permission also sets `resident.asking`, but a typed message
  // never answers a permission prompt (it must be approved or denied explicitly).
  if (input.permissions?.some((permission) => permission.state === "pending")) return false;
  return residentOf(input.row)?.asking === true;
}
