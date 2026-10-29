/*
 * What the transcript's rows share: the item type, the render style, and the
 * contexts a row reads from the view around it.
 */
import { createContext } from "react";
import type { components } from "@/api/schema";
import { questionsByToolUse, type SessionQuestion } from "@/features/sessions/session-questions";
import { EMPTY_PAIRING, type ToolPairing } from "@/features/sessions/transcript-groups";

export type TranscriptItem = components["schemas"]["TranscriptItem"];

/** The step's number inside a folded run (null outside one). */
export const StepNumberContext = createContext<number | null>(null);

export type TranscriptStyle = "flat" | "bubble";

/**
 * True while rendering inside a bubble, which already draws the border, padding
 * and one role header for the whole turn — so rows render flush and headerless
 * instead of as standalone full-width lines.
 */
export const InBubbleContext = createContext(false);

/**
 * The model the enclosing bubble's header is already showing, if any.
 *
 * Rows inside a bubble draw no header — the turn's one header speaks for them,
 * and a bubble holds one model — so this is how a row knows whether the model
 * behind it has been said out loud already.
 * A subagent's transcript is the case where it has not: it ran on whatever model
 * *it* was handed, which the parent turn's header knows nothing about, so the
 * drill-down clears this (see [`AgentTranscriptBody`]) and its messages name
 * their own model.
 */
export const BubbleModelContext = createContext<string | null>(null);

/**
 * Which result answers which call in the window being rendered. Held in context
 * because tool rows are reached from three places (the transcript itself, a
 * folded step group, and a subagent drill-down), each with its own window.
 *
 * Read in exactly one place — [`ItemBlock`], which is deliberately not
 * memoized — and never inside a memoized row. A context value that changes
 * re-renders every consumer under it regardless of `memo`, and this one is
 * rebuilt whenever the window is, so a thousand-row transcript would reconcile
 * in full on every append. The rows take their one answer as a prop instead.
 */
export const PairingContext = createContext<ToolPairing>(EMPTY_PAIRING);

/**
 * Whether the session is working, for rows that mean different things when it
 * is: a tool call with no result is "running" during a turn and "never came
 * back" outside one. A plain boolean, so it re-renders the rows only when the
 * session actually changes state.
 */
export const LiveContext = createContext(false);

/**
 * The launching `tool_use` ids the resident is minding right now.
 *
 * Read in [`ItemBlock`] only, and rebuilt only when the ids themselves change
 * rather than on every state frame — see `runningTaskIds` at the call site. A
 * card asks one question of it ("is mine in here"), and gets a boolean prop, so
 * the memoized rows below are never handed a set that is a new object.
 */
const NO_RUNNING_TASKS: ReadonlySet<string> = new Set<string>();

export const RunningTasksContext = createContext<ReadonlySet<string>>(NO_RUNNING_TASKS);

/**
 * The questions the agent asked, by the `tool_use` each one is, and the session
 * to answer them in.
 *
 * Read in [`ItemBlock`] only, like the two above, and for the same reason: a
 * question card asks one thing of it ("what became of mine") and is handed the
 * one record as a prop, so the memoized rows never see the map. The value is
 * rebuilt only when the feed's `questions` list is a new object — which
 * `applyStateFrame` arranges to be only when a question opened or closed, not
 * on every state frame.
 */
export interface QuestionIndex {
  session: SubagentCtx | undefined;
  byToolUse: ReadonlyMap<string, SessionQuestion>;
}

const NO_QUESTIONS: QuestionIndex = { session: undefined, byToolUse: questionsByToolUse([]) };

export const QuestionsContext = createContext<QuestionIndex>(NO_QUESTIONS);

export interface SubagentCtx {
  source: "claude" | "codex";
  id: string;
}
