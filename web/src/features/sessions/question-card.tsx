import { useMutation } from "@tanstack/react-query";
import { CheckIcon, CircleNotchIcon, PaperPlaneTiltIcon } from "@phosphor-icons/react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ApiError, errorText } from "@/api/errors";
import type { components } from "@/api/schema";
import { cn } from "@/lib/cn";
import { formatExactTime, formatTime } from "@/lib/format";
import { isImeKeydown } from "@/lib/ime";
import {
  answerBody,
  answersOnTap,
  EMPTY_DRAFT,
  effectiveQuestion,
  questionIsOpen,
  settledQuestionOf,
  togglePick,
  typeOther,
  type AnswerQuestion,
  type QuestionDraft,
  type QuestionEntry,
  type QuestionState,
  type SessionQuestion,
} from "./session-questions";
import { useApi } from "@/lib/machines/context";

type TranscriptItem = components["schemas"]["TranscriptItem"];

/** The session a card answers into. Questions are only ever claude's. */
export interface QuestionSession {
  source: "claude" | "codex";
  id: string;
}

type Settle = { kind: "answer"; body: AnswerQuestion } | { kind: "dismiss" };

/**
 * An `AskUserQuestion` call, drawn where the transcript has it.
 *
 * Three cards in one, decided by the record the server keeps for the question
 * (`record`, looked up by the call's `tool_use` id):
 *
 * - **open** (`pending`, or `expired` — see `questionIsOpen`): the options as
 *   buttons, an "Other" field per question, and a way to decline. The answer
 *   goes straight into the turn that is blocked on it, not through the
 *   composer — the card used to pre-fill the composer and send an ordinary
 *   message, which queued the answer behind the very turn waiting for it.
 * - **settled** (`answered`, `dismissed`, `cancelled`): what was chosen, who
 *   chose it and when, read-only.
 * - **no record** — an external session, a transcript older than the feature,
 *   codex, a server too old to send `questions`: the call as the transcript
 *   wrote it, read-only. There is nobody holding it open to answer.
 *
 * Holds its own draft and its own request, so a row that is not this one never
 * re-renders for a tap here; and keeps what the server answered (`local`) so
 * the card settles the moment the route says so rather than a frame later —
 * which matters most for a `409`, where the alternative is buttons that can
 * only fail again.
 */
export function QuestionCard({
  item,
  record,
  session,
}: {
  item: TranscriptItem;
  /** The server's record of this question, when it keeps one. */
  record?: SessionQuestion;
  /** Where to answer it; absent where nothing can be answered. */
  session?: QuestionSession;
}) {
  const api = useApi();
  const [draft, setDraft] = useState<QuestionDraft>(EMPTY_DRAFT);
  const [local, setLocal] = useState<SessionQuestion | null>(null);
  const settle = useMutation({
    mutationFn: async (action: Settle) => {
      if (!session || !record) throw new Error("no question to answer");
      const path = { source: session.source, id: session.id, requestId: record.requestId };
      if (action.kind === "answer") {
        const res = await api.POST("/api/sessions/{source}/{id}/questions/{requestId}/answer", {
          params: { path },
          body: action.body,
        });
        // ApiError, not Error: a `409` carries the question as the winner left
        // it, and `settledQuestionOf` reads it back out.
        if (res.error || !res.data) throw new ApiError(res.error);
        return res.data;
      }
      const res = await api.POST("/api/sessions/{source}/{id}/questions/{requestId}/dismiss", {
        params: { path },
      });
      if (res.error || !res.data) throw new ApiError(res.error);
      return res.data;
    },
    onSuccess: (result) => setLocal(result.question),
    onError: (error) => {
      const settled = settledQuestionOf(error);
      if (settled) setLocal(settled);
    },
  });
  // A lost race is not an error to show: the card simply turns into what the
  // winner left, who and when included.
  const refusal = settle.error && !settledQuestionOf(settle.error) ? errorText(settle.error) : null;
  const answerable = session !== undefined && record !== undefined;
  return (
    <QuestionCardView
      item={item}
      question={effectiveQuestion(record, local)}
      draft={draft}
      busy={settle.isPending}
      error={refusal}
      onDraft={setDraft}
      onSubmit={answerable ? (body) => settle.mutate({ kind: "answer", body }) : undefined}
      onDismiss={answerable ? () => settle.mutate({ kind: "dismiss" }) : undefined}
    />
  );
}

/**
 * The card as a function of its state, for `web/test` to draw every case of
 * without a DOM, a server or a click.
 *
 * Interactive only when the question is open *and* there is somewhere to send
 * the answer (`onSubmit`); otherwise the same record, read-only.
 */
export function QuestionCardView({
  item,
  question,
  draft = EMPTY_DRAFT,
  busy = false,
  error = null,
  onDraft = () => {},
  onSubmit,
  onDismiss,
}: {
  item: TranscriptItem;
  /** The record to draw, already reconciled with what this screen was told. */
  question: SessionQuestion | null;
  draft?: QuestionDraft;
  /** An answer or a dismissal is in flight: everything is disabled. */
  busy?: boolean;
  /** Why the last attempt failed, when it did not fail by losing a race. */
  error?: string | null;
  onDraft?: (next: QuestionDraft) => void;
  onSubmit?: (body: AnswerQuestion) => void;
  onDismiss?: () => void;
}) {
  const { t } = useTranslation();
  // The record's copy first: the transcript only carries the structured
  // options while the call is unanswered, and a settled card still has to say
  // what was asked.
  const entries: QuestionEntry[] =
    question && question.questions.length > 0 ? question.questions : (item.question ?? []);
  const open = question !== null && questionIsOpen(question.state) && onSubmit !== undefined && entries.length > 0;
  return (
    <div
      data-question={question?.state ?? "record"}
      data-signal="asking"
      className="border-b border-border bg-warn/7 px-4 pt-2 pb-3 shadow-[inset_2px_0_0_var(--color-warn)]"
    >
      <div className="mb-1.5 flex items-center gap-2 font-mono text-[10px] leading-[14px]">
        {open && question.state === "pending" ? (
          <span data-signal="warn" className="inline-flex h-4 items-center bg-warn px-1 font-semibold text-on-signal">{t("waiting for your answer")}</span>
        ) : null}
        <span data-label="" className="label-caps font-semibold text-fg">? QUESTION</span>
        {item.meta && <span className="text-faint">{item.meta}</span>}
        {open && question.state !== "pending" && <span data-signal="warn" className="text-warn-fg">{t("expired")}</span>}
        {item.timestamp && (
          <span className="ml-auto tabular-nums text-faint">{formatTime(item.timestamp)}</span>
        )}
      </div>
      {open ? (
        <OpenQuestion
          entries={entries}
          state={question.state}
          draft={draft}
          busy={busy}
          error={error}
          onDraft={onDraft}
          onSubmit={onSubmit!}
          onDismiss={onDismiss}
        />
      ) : question !== null && entries.length > 0 ? (
        <SettledQuestion question={question} entries={entries} />
      ) : (
        <div className="text-xs leading-5 whitespace-pre-wrap">{item.detail ?? item.text}</div>
      )}
    </div>
  );
}

/** A tick box or a radio dot, drawn rather than a native input: the button is the control. */
function Mark({ kind, on }: { kind: "checkbox" | "radio"; on: boolean }) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        "mt-0.5 flex size-3 shrink-0 items-center justify-center border",
        kind === "radio" ? "rounded-full" : "",
        on ? (kind === "radio" ? "border-fg text-fg" : "border-fg bg-fg text-bg") : "border-border-strong",
      )}
      data-shape={kind === "radio" ? "radio" : undefined}
    >
      {on && (kind === "radio" ? <span data-shape="dot" className="size-1.5 rounded-full bg-current" /> : <CheckIcon className="size-2.5" />)}
    </span>
  );
}

function OpenQuestion({
  entries,
  state,
  draft,
  busy,
  error,
  onDraft,
  onSubmit,
  onDismiss,
}: {
  entries: QuestionEntry[];
  state: QuestionState;
  draft: QuestionDraft;
  busy: boolean;
  error: string | null;
  onDraft: (next: QuestionDraft) => void;
  onSubmit: (body: AnswerQuestion) => void;
  onDismiss?: () => void;
}) {
  const { t } = useTranslation();
  const tap = answersOnTap(entries);
  const body = answerBody(entries, draft);
  const send = (from: QuestionDraft) => {
    const answer = answerBody(entries, from);
    if (answer) onSubmit(answer);
  };
  return (
    // A disabled fieldset disables every control inside it, which is the whole
    // of "nothing can be pressed twice while the answer is on its way".
    <fieldset disabled={busy} className="mt-1 flex min-w-0 flex-col gap-2">
      {state === "expired" && (
        <p className="font-mono text-2xs leading-4 text-muted">
          {t(
            "the session restarted after asking, so nothing is blocked on this any more — your answer will be sent as a new message",
          )}
        </p>
      )}
      {entries.map((entry, qi) => {
        const picked = draft.picked[qi] ?? [];
        const other = draft.other[qi] ?? "";
        const kind = entry.multiSelect ? "checkbox" : "radio";
        return (
          <div key={qi} className="flex min-w-0 flex-col gap-1">
            {entry.header && (
              <span className="font-mono text-[10px] tracking-wide text-faint uppercase">{entry.header}</span>
            )}
            <div className="text-xs leading-5 whitespace-pre-wrap">{entry.question}</div>
            <div
              role={tap ? undefined : entry.multiSelect ? "group" : "radiogroup"}
              aria-label={tap ? undefined : entry.question}
              className="flex flex-col"
            >
              {entry.options.map((opt, oi) => {
                const on = picked.includes(oi);
                return (
                  <button
                    key={oi}
                    type="button"
                    role={tap ? undefined : kind}
                    aria-checked={tap ? undefined : on}
                    onClick={() => {
                      const next = togglePick(tap ? EMPTY_DRAFT : draft, qi, oi, entry.multiSelect);
                      onDraft(next);
                      // One question, one answer: the tap is the answer.
                      if (tap) send(next);
                    }}
                    className={cn(
                      "-mt-px flex items-start gap-2 border bg-bg px-2.5 py-1.5 text-left text-xs first:mt-0 disabled:cursor-not-allowed disabled:opacity-50",
                      on ? "relative z-[1] border-fg" : "border-border hover:bg-hover",
                    )}
                  >
                    {!tap && <Mark kind={kind} on={on} />}
                    <span className="min-w-0 break-words">
                      <span className="font-medium">{opt.label}</span>
                      {opt.description && <span className="ml-1.5 text-2xs text-muted">{opt.description}</span>}
                    </span>
                  </button>
                );
              })}
              <div className="flex min-w-0 items-center gap-1">
                <input
                  type="text"
                  value={other}
                  placeholder={t("other answer…")}
                  aria-label={t("other answer…")}
                  onChange={(e) => onDraft(typeOther(draft, qi, e.target.value, entry.multiSelect))}
                  onKeyDown={(e) => {
                    // Only where the field is the whole answer: on a card with
                    // more to fill in, Enter in one field must not send the lot.
                    if (!tap || e.key !== "Enter" || isImeKeydown(e)) return;
                    e.preventDefault();
                    send(draft);
                  }}
                  className="h-7 min-w-0 flex-1 border border-border bg-bg px-2 text-xs text-fg placeholder:text-faint focus:border-fg focus:outline-none disabled:opacity-50"
                />
                {tap && (
                  <button
                    type="button"
                    disabled={other.trim() === ""}
                    title={t("send answer")}
                    aria-label={t("send answer")}
                    onClick={() => send(draft)}
                    data-variant="primary" className="flex size-7 shrink-0 items-center justify-center bg-fg text-bg hover:bg-fg/85 disabled:opacity-40"
                  >
                    <PaperPlaneTiltIcon className="size-3.5" />
                  </button>
                )}
              </div>
            </div>
          </div>
        );
      })}
      <div className="flex flex-wrap items-center gap-2">
        {!tap && (
          <button
            type="button"
            disabled={body === null}
            onClick={() => send(draft)}
            data-variant="primary" className="h-7 bg-fg px-3 text-xs font-medium text-bg hover:bg-fg/85 disabled:opacity-40"
          >
            {t("send answers")}
          </button>
        )}
        {onDismiss && (
          <button
            type="button"
            onClick={onDismiss}
            title={t("the agent is told you chose not to answer, and carries on with its own judgement")}
            className="h-7 px-2.5 text-xs text-muted hover:bg-hover hover:text-fg disabled:opacity-40"
          >
            {t("let the agent decide")}
          </button>
        )}
        {busy && <CircleNotchIcon className="size-3 animate-spin text-muted" />}
      </div>
      {error && (
        <p data-signal="failed" className="font-mono text-2xs leading-4 break-words text-danger">
          {t("send failed:")} {error}
        </p>
      )}
    </fieldset>
  );
}

/** What a settled question's last line says, before who and when. */
const SETTLED_LABEL: Record<QuestionState, { glyph: string; label: string; tone: string }> = {
  answered: { glyph: "✓", label: "answered", tone: "text-ok/80" },
  dismissed: { glyph: "⊘", label: "dismissed — left to the agent", tone: "text-muted" },
  cancelled: { glyph: "⊘", label: "cancelled — the turn was stopped", tone: "text-muted" },
  // Only reached for an open question with nowhere to send an answer.
  pending: { glyph: "?", label: "waiting for your answer", tone: "text-warn-fg" },
  expired: { glyph: "?", label: "expired", tone: "text-muted" },
};

/**
 * A question with its outcome: what was asked, what was chosen, and who chose
 * it when. Read-only — a settled question cannot be answered again, which is
 * the server's rule (`409`) as well as this card's.
 */
function SettledQuestion({ question, entries }: { question: SessionQuestion; entries: QuestionEntry[] }) {
  const { t } = useTranslation();
  const look = SETTLED_LABEL[question.state];
  const line = [t(look.label), question.answeredBy, formatTime(question.answeredAt)]
    .filter((part): part is string => !!part)
    .join(" · ");
  return (
    <div className="flex min-w-0 flex-col gap-1">
      {entries.map((entry, qi) => {
        const answer = question.answers?.[entry.question];
        return (
          <div key={qi} className="min-w-0">
            {entry.header && (
              <span className="font-mono text-[10px] tracking-wide text-faint uppercase">{entry.header}</span>
            )}
            <div className="text-xs leading-5 whitespace-pre-wrap">{entry.question}</div>
            {answer && (
              <div className="text-xs leading-5 font-medium break-words whitespace-pre-wrap" data-answer>
                <span className="text-faint">→ </span>
                {answer}
              </div>
            )}
          </div>
        );
      })}
      {question.response && (
        <div className="border-l border-border-strong pl-2.5 text-xs leading-5 break-words whitespace-pre-wrap" data-response>
          {question.response}
        </div>
      )}
      <div
        className={cn("font-mono text-[10px]", look.tone)}
        title={formatExactTime(question.answeredAt) || undefined}
        data-settled={question.state}
      >
        {look.glyph} {line}
      </div>
    </div>
  );
}

/**
 * The line above the composer while the agent is blocked on a question.
 *
 * Because the composer changes meaning then: the server turns what is sent
 * into the question's answer, in the user's own words, rather than queueing it
 * behind the turn that is waiting for it. A composer that looked the same as
 * ever would leave the user to guess which of the two their message became.
 */
export function AnswerNotice({ show }: { show: boolean }) {
  const { t } = useTranslation();
  if (!show) return null;
  return (
    <div
      role="status"
      data-answer-notice
      data-signal="asking" className="flex items-start gap-1.5 border-t border-border px-4 py-1 font-mono text-2xs leading-4 text-warn-fg shadow-[inset_2px_0_0_var(--color-warn)]"
    >
      <span aria-hidden="true" className="shrink-0 font-bold">
        ?
      </span>
      <span className="min-w-0 flex-1 break-words">
        {t("the agent is waiting for your answer — what you send now goes in as the answer")}
      </span>
    </div>
  );
}
