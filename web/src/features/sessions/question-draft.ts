/**
 * What a question card has been typed into and ticked so far, kept in the
 * browser.
 *
 * The card's draft is not a composer's: it belongs to one `AskUserQuestion`
 * call, not a session, and the card is unmounted and rebuilt whenever the
 * session is switched away from (the detail is keyed by session), and trimmed
 * out of a window that follows its tail. Held only in React state, as it was,
 * everything typed into "Other" and every tick was gone on the way back.
 *
 * So it is stored here, by the machine, the session and the call's `tool_use`
 * id — the same id the question's record is joined on. An answer through the
 * card, a dismissal, another screen winning the race or the turn stopping all
 * settle the record; the card clears the key the moment it draws a settled
 * question. An `expired` one stays answerable, so its draft stays too.
 *
 * Plain text and tick indexes, and nothing a question's text says: a settled
 * question is never answered again, so a stale key only has to be readable
 * until it is cleared.
 */
import type { MachineKey } from "@/lib/machines/key";
import { machineStorageKey } from "@/lib/machines/storage";
import { EMPTY_DRAFT, type QuestionDraft } from "./session-questions";
import type { DraftStorage } from "./session-draft";

interface StoredQuestionDraft {
  version: 1;
  picked: Record<string, readonly number[]>;
  other: Record<string, string>;
}

/** The draft's name on its machine: one per call, so two cards never meet. */
const draftName = (session: { source: string; id: string }, toolUseId: string) =>
  `question.${session.source}.${session.id}.${toolUseId}`;

/** The storage key of a card's draft: `rh.question.<source>.<id>.<tool use id>` on the local machine. */
export function questionDraftKey(
  machine: MachineKey,
  session: { source: string; id: string },
  toolUseId: string,
): string {
  return machineStorageKey(machine, draftName(session, toolUseId));
}

/** Indexes are question positions: a model may repeat a label, and a tick has to name the button. */
function isIndex(key: string): boolean {
  return /^\d+$/.test(key);
}

function decodeQuestionDraft(raw: string | null): QuestionDraft {
  if (raw === null) return EMPTY_DRAFT;
  try {
    const parsed = JSON.parse(raw) as Partial<StoredQuestionDraft>;
    if (parsed.version !== 1 || !parsed.picked || !parsed.other) return EMPTY_DRAFT;
    const picked: Record<number, readonly number[]> = {};
    for (const [index, ticks] of Object.entries(parsed.picked)) {
      if (!isIndex(index) || !Array.isArray(ticks)) continue;
      if (ticks.every((tick) => Number.isInteger(tick) && tick >= 0)) picked[Number(index)] = ticks;
    }
    const other: Record<number, string> = {};
    for (const [index, text] of Object.entries(parsed.other)) {
      if (isIndex(index) && typeof text === "string") other[Number(index)] = text;
    }
    return { picked, other };
  } catch {
    // A key written by another build, or by hand: an empty draft is still answerable.
    return EMPTY_DRAFT;
  }
}

export function readQuestionDraft(storage: DraftStorage, key: string): QuestionDraft {
  try {
    return decodeQuestionDraft(storage.getItem(key));
  } catch {
    return EMPTY_DRAFT;
  }
}

/** Nothing picked and nothing typed: the key is removed rather than holding an empty draft. */
function isEmpty(draft: QuestionDraft): boolean {
  return (
    Object.values(draft.picked).every((ticks) => ticks.length === 0) &&
    Object.values(draft.other).every((text) => text === "")
  );
}

export function writeQuestionDraft(storage: DraftStorage, key: string, draft: QuestionDraft): void {
  try {
    if (isEmpty(draft)) {
      storage.removeItem(key);
      return;
    }
    const stored: StoredQuestionDraft = {
      version: 1,
      picked: Object.fromEntries(Object.entries(draft.picked).filter(([, ticks]) => ticks.length > 0)),
      other: Object.fromEntries(Object.entries(draft.other).filter(([, text]) => text !== "")),
    };
    storage.setItem(key, JSON.stringify(stored));
  } catch {
    // storage unavailable: the card's in-memory draft still works
  }
}

export function clearQuestionDraft(storage: DraftStorage, key: string): void {
  try {
    storage.removeItem(key);
  } catch {
    // storage unavailable: nothing to clear
  }
}
