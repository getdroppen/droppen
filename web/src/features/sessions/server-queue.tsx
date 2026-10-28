import type { components } from "@/api/schema";
import { useMutation } from "@tanstack/react-query";
import { Loader2, RotateCw, X, Zap } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ApiError, errorText } from "@/api/errors";
import {
  queuedMessagesOf,
  type QueuedMessage,
  type ResidentInfo,
} from "@/features/sessions/session-activity";
import { cn } from "@/lib/cn";
import { ReceiptBadge } from "@/features/sessions/receipt-badge";
import { receiptState, type DeliveryState, type MessageReceipt } from "@/features/sessions/send-receipts";
import { useApi, useMachine } from "@/lib/machines/context";
import { sessionKey } from "@/lib/machines/key";

/** A sentence about the last queue action, kept until the next one. */
export interface QueueNotice {
  /** `info` is a consequence worth knowing; `danger` is a refusal. */
  tone: "info" | "danger";
  text: string;
  /**
   * The queued message the sentence is about. Once that message has left the
   * queue — it started, so "it runs next" is no longer news — the sentence goes
   * with it.
   */
  qid?: string;
}

/** `notice`, unless the message it is about has already left the queue. */
export function noticeInForce(notice: QueueNotice | null, messages: readonly { id: string }[]): QueueNotice | null {
  if (!notice?.qid) return notice;
  return messages.some((message) => message.id === notice.qid) ? notice : null;
}

/**
 * Where a queued row is, by the receipt its queue id carries.
 *
 * A row with no receipt was sent by a client that did not name it, and being
 * in the server's queue is all that can be said of it: delivered. A claude row
 * that was steered is in the CLI's stdin, which is handed over.
 */
export function queuedRowState(
  message: QueuedMessage,
  receipts: readonly MessageReceipt[] | undefined,
  source: components["schemas"]["Source"],
): DeliveryState {
  const receipt = receipts?.find((candidate) => candidate.qid === message.id);
  if (receipt) return receiptState(receipt.status);
  return message.steered && source === "claude" ? "executed" : "delivered";
}

/**
 * The messages the *server* is holding for this session.
 *
 * Distinct from the composer's local queue in every way that matters: these
 * have been accepted, they survive this tab closing, and they will run whether
 * or not anyone is watching. Until the queue routes existed they were also
 * completely opaque — `resident.queued` said "3" and nothing said which three,
 * so a user who queued the wrong message behind a half-hour turn could neither
 * read it back nor take it out.
 *
 * Presentational, so it can be rendered without a query client; the container
 * below owns the two routes.
 */
export function ServerQueueList({
  source = "claude",
  messages,
  pendingId,
  restartId,
  notice,
  receipts,
  onSteer,
  onDrop,
}: {
  source?: components["schemas"]["Source"];
  messages: readonly QueuedMessage[];
  /** The session's receipts, which say where each named row is. */
  receipts?: readonly MessageReceipt[];
  /** The message whose route call is in flight; its buttons go dead. */
  pendingId: string | null;
  /**
   * The message whose ▶ would *restart* the session rather than deliver it.
   *
   * The parked head of a settings change: it needs a process this one is not,
   * so the server answers the press by replacing the process — killing whatever
   * background work held it up. Different act, different button; see
   * [`parkedHead`] for how it is found.
   */
  restartId: string | null;
  notice: QueueNotice | null;
  onSteer: (qid: string) => void;
  onDrop: (qid: string) => void;
}) {
  const { t } = useTranslation();
  if (messages.length === 0 && !notice) return null;
  return (
    <div className="border-b border-border">
      {messages.length > 0 && (
        <div className="px-3 pt-1 font-mono text-2xs text-faint">
          {t("queued in the session ({{n}})", { n: messages.length })}
        </div>
      )}
      {messages.map((message, i) => {
        const busy = pendingId === message.id;
        const restarts = restartId === message.id;
        return (
          <div key={message.id} className="flex items-center gap-1.5 px-3 py-1">
            <span className="shrink-0 font-mono text-2xs text-faint">#{i + 1}</span>
            <span
              className="min-w-0 flex-1 truncate font-mono text-2xs text-muted"
              title={message.text}
            >
              {message.text}
            </span>
            <ReceiptBadge state={queuedRowState(message, receipts, source)} />
            {message.steered ? (
              // Past the point of being taken back: it is in claude's stdin,
              // and both routes answer 409 for it. Saying so beats offering
              // two buttons whose only outcome is an error.
              <span
                className="shrink-0 px-1.5 py-0.5 font-mono text-2xs text-fg shadow-[inset_0_0_0_1px_var(--color-border-strong)]"
                title={source === "codex" ? t("awaiting Codex acknowledgement") : t("already handed to the CLI; it runs as the next turn")}
              >
                {source === "codex" ? t("awaiting Codex acknowledgement") : t("delivered — waiting to start")}
              </span>
            ) : (
              <>
                {/* An ACP agent takes the next message only when the turn in flight ends. */}
                {source !== "acp" && <button
                  type="button"
                  disabled={busy}
                  title={
                    restarts
                      ? t("restart the session now and send this message")
                      : source === "codex" ? t("steer into the active Codex turn") : t("deliver now, ahead of the turn in flight")
                  }
                  onClick={() => onSteer(message.id)}
                  className={cn(
                    "flex size-5 shrink-0 items-center justify-center rounded hover:bg-hover disabled:opacity-40",
                    // A restart ends work; a steer does not. The button that
                    // does the first should not look like the one that does the
                    // second, because the press is the same press.
                    restarts ? "text-warn-fg" : "text-accent",
                  )}
                >
                  {busy ? (
                    <Loader2 className="size-3 animate-spin" />
                  ) : restarts ? (
                    <RotateCw className="size-3" />
                  ) : (
                    <Zap className="size-3" />
                  )}
                </button>}
                <button
                  type="button"
                  disabled={busy}
                  title={t("remove from queue")}
                  onClick={() => onDrop(message.id)} data-signal="failed" className="flex size-5 shrink-0 items-center justify-center rounded text-muted hover:bg-hover hover:text-danger disabled:opacity-40"
                >
                  <X className="size-3" />
                </button>
              </>
            )}
          </div>
        );
      })}
      {notice && (
        <div
          className={cn(
            "px-3 pb-1 font-mono text-2xs leading-4",
            notice.tone === "danger" ? "text-danger" : "text-warn-fg",
          )}
        >
          {notice.text}
        </div>
      )}
    </div>
  );
}

/**
 * What a refusal from the two queue routes means to the user.
 *
 * The status is the whole distinction. `404` is a race the user lost by two
 * seconds — the message left the queue because it *started* — and the honest
 * response is to say so and re-read the state rather than to report an error.
 * `409` is the server declining for a reason it wrote out in full (already
 * steered, or wants a process the running one is not), so its sentence is
 * shown as-is; inventing a second wording for it here is how the UI ends up
 * disagreeing with the server about why something did not happen.
 */
export function queueFailureNotice(error: unknown, t: (key: string) => string): QueueNotice {
  if (error instanceof QueueRefusal && error.status === 404) {
    return { tone: "info", text: t("that message has already started — it is out of the queue") };
  }
  return { tone: "danger", text: errorText(error) };
}

/**
 * The queued message a park is waiting on, if this session is parked.
 *
 * `awaitingBackground` says a settings change is at the head of the *server's*
 * queue waiting for claude's background work to end — up to half an hour — and
 * the row lists steered messages first, then the queue in order. So the parked
 * message is the first one not yet handed over, and it is the only one whose ▶
 * costs anything: every other queued message fits the running process and is
 * delivered into it exactly as before.
 */
export function parkedHead(
  resident: ResidentInfo | null,
  messages: readonly QueuedMessage[],
): string | null {
  if (!resident?.awaitingBackground) return null;
  return messages.find((message) => !message.steered)?.id ?? null;
}

/** An [`ApiError`] that remembers which refusal it was. */
export class QueueRefusal extends ApiError {
  readonly status: number;
  constructor(status: number, body: unknown) {
    super(body);
    this.name = "QueueRefusal";
    this.status = status;
  }
}

/** The queue routes, wired to the list above. */
export function ServerQueue({
  source,
  id,
  resident,
  receipts,
  onChanged,
  onDropped,
}: {
  source: components["schemas"]["Source"];
  id: string;
  resident: ResidentInfo | null;
  /** The session's message receipts, from its state. */
  receipts?: readonly MessageReceipt[];
  /**
   * Re-read the session's state after a route said yes.
   *
   * The resident publishes its new queue immediately, but the socket's state
   * frame is on a two-second tick — long enough that a chip you just removed
   * sits there looking like a click that did nothing.
   */
  onChanged: () => void;
  /** A removed message must not become an awaiting-transcript receipt. */
  onDropped?: (qid: string) => void;
}) {
  const api = useApi();
  const { t } = useTranslation();
  const messages = queuedMessagesOf(resident);
  const restartId = parkedHead(resident, messages);
  // The panel stays mounted when the view moves to another session, so a
  // notice remembers whose it is — on which device too — and is not shown
  // anywhere else.
  const session = sessionKey({ machine: useMachine(), source, id });
  const [held, setHeld] = useState<{ session: string; notice: QueueNotice } | null>(null);
  const notice = held?.session === session ? noticeInForce(held.notice, messages) : null;
  const setNotice = (next: QueueNotice | null, at: string = session) => setHeld(next ? { session: at, notice: next } : null);

  const act = useMutation({
    mutationFn: async ({ qid, steer }: { qid: string; steer: boolean; restart?: boolean; session: string }) => {
      const params = { path: { source, id, qid } };
      const res = steer
        ? await api.POST("/api/sessions/{source}/{id}/queue/{qid}/steer", { params })
        : await api.DELETE("/api/sessions/{source}/{id}/queue/{qid}", { params });
      if (res.error) throw new QueueRefusal(res.response.status, res.error);
    },
    onSuccess: (_data, { qid, steer, restart, session: at }) => {
      if (!steer) {
        onDropped?.(qid);
        setNotice(null);
        return;
      }
      setNotice({
        tone: "info",
        // Until the message starts: then "it runs next" has happened.
        qid,
        text: restart
          ? // The other half of the confirmation: what was traded is said out
            // loud once it has actually happened.
            t("restarting — the session is coming back under the settings this message asked for")
          : source === "codex"
            ? t("delivered into the active Codex turn")
          : // Stop ends the turn in flight and nothing else, so a steered
            // message survives it. Said once, at the moment it becomes true.
            t("delivered — it runs next; Stop ends the current turn but will not hold it back"),
      }, at);
    },
    onError: (error, { session: at }) => setNotice(queueFailureNotice(error, t), at),
    // Whichever way it went, the queue this client is drawing is now at least
    // one event out of date — a 404 says so outright.
    onSettled: () => onChanged(),
  });

  if (messages.length === 0 && !notice) return null;
  return (
    <ServerQueueList
      source={source}
      messages={messages}
      pendingId={act.isPending && act.variables?.session === session ? act.variables.qid : null}
      restartId={restartId}
      notice={notice}
      receipts={receipts}
      onSteer={(qid) => {
        const restart = qid === restartId;
        // The one press in this panel that destroys something. The server will
        // do it without arguing — that is the fix for a park with no way out —
        // so the question has to be asked here, and it has to name the cost.
        if (restart) {
          const running = resident?.background ?? 0;
          const ask =
            running > 0
              ? t(
                  "This message needs different settings, so the session has to restart. That kills the {{n}} background tasks it is still minding. Restart now?",
                  { n: running },
                )
              : t(
                  "This message needs different settings, so the session has to restart. That kills whatever work it is still minding. Restart now?",
                );
          if (!window.confirm(ask)) return;
        }
        setNotice(null);
        act.mutate({ qid, steer: true, restart, session });
      }}
      onDrop={(qid) => {
        setNotice(null);
        act.mutate({ qid, steer: false, session });
      }}
    />
  );
}
