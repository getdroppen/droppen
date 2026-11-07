import { CircleNotchIcon, PlayIcon, ArrowClockwiseIcon, XIcon, LightningIcon } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import { ReceiptBadge } from "./receipt-badge";
import { outboxState, type OutgoingMessage } from "./send-receipts";

/**
 * This tab's queue: the messages it is still holding, each with where it is.
 *
 * A message in flight is not drawn here — the preview over the composer shows
 * it — so what is left is what needs this device: `saved` ones waiting their
 * turn, an `unknown` one whose send got no answer (and holds everything behind
 * it until the server says whether it arrived), and the two the user decides
 * about: `unsent`, which the server never saw and can be sent again under the
 * same id, and `failed`.
 *
 * Presentational, so it renders without a query client; the composer owns the
 * actions.
 */
export function OutboxList({
  queue,
  paused,
  steering,
  headAction,
  checking,
  onSendHead,
  onSteerHead,
  onResend,
  onCheck,
  onRemove,
}: {
  queue: readonly OutgoingMessage[];
  /** A failure is waiting to be looked at; the drain is stopped. */
  paused: boolean;
  /** A legacy codex run is being stopped so the head can go in. */
  steering: boolean;
  /** What the head of a `saved` queue can do right now. */
  headAction: "send" | "steer" | null;
  /** A receipt lookup is in flight. */
  checking: boolean;
  onSendHead: () => void;
  onSteerHead: () => void;
  /** Send an `unsent` or `failed` message again. */
  onResend: (message: OutgoingMessage) => void;
  /** Ask the server what became of the `unknown` ones. */
  onCheck: () => void;
  onRemove: (message: OutgoingMessage) => void;
}) {
  const { t } = useTranslation();
  const visible = queue.filter((message) => outboxState(message) !== "sending");
  if (visible.length === 0) return null;
  const head = queue[0];
  const blockedByUnknown = head !== undefined && outboxState(head) === "unknown";
  return (
    <div className="border-b border-border">
      <div className="px-3 pt-1 font-mono text-2xs text-faint">
        {t("waiting to send ({{n}})", { n: visible.length })}
      </div>
      {paused && (
        <div data-signal="warn" className="px-3 pt-1 font-mono text-2xs text-warn-fg">
          {t("queue paused after the failure above — resume with ▶")}
        </div>
      )}
      {blockedByUnknown && (
        <div data-signal="warn" className="px-3 pt-1 font-mono text-2xs text-warn-fg">
          {t("checking with the server whether the unknown message arrived — nothing after it is sent until then")}
        </div>
      )}
      {steering && (
        <div className="px-3 pt-1 font-mono text-2xs text-accent">
          {t("steering — stopping the current turn, then this message will run")}
        </div>
      )}
      {visible.map((message, i) => {
        const state = outboxState(message);
        const isHead = message === head;
        const attached = (message.images?.length ?? 0) + (message.files?.length ?? 0);
        return (
          <div key={message.clientMessageId} className="flex items-center gap-1.5 px-3 py-1">
            <span className="shrink-0 font-mono text-2xs text-faint">#{i + 1}</span>
            <span className="min-w-0 flex-1 truncate font-mono text-2xs text-muted" title={message.text}>
              {message.text}
              {attached > 0 ? ` 📎${attached}` : ""}
            </span>
            <ReceiptBadge state={state} />
            {state === "unknown" ? (
              <button
                type="button"
                disabled={checking}
                title={t("check whether it arrived")}
                onClick={onCheck} data-signal="warn" className="flex size-5 shrink-0 items-center justify-center rounded text-warn-fg hover:bg-hover disabled:opacity-40"
              >
                {checking ? <CircleNotchIcon className="size-3 animate-spin" /> : <ArrowClockwiseIcon className="size-3" />}
              </button>
            ) : state === "unsent" || state === "failed" ? (
              <button
                type="button"
                title={
                  state === "unsent"
                    ? t("resend under the same message id — it cannot run twice")
                    : t("resend as a new message")
                }
                onClick={() => onResend(message)} data-signal="ok" className="flex size-5 shrink-0 items-center justify-center rounded text-ok hover:bg-hover"
              >
                <PlayIcon className="size-3" />
              </button>
            ) : isHead && steering ? (
              <CircleNotchIcon className="size-3 shrink-0 animate-spin text-accent" />
            ) : isHead && headAction === "send" ? (
              <button
                type="button"
                title={t("send this queued message now")}
                onClick={onSendHead} data-signal="ok" className="flex size-5 shrink-0 items-center justify-center rounded text-ok hover:bg-hover"
              >
                <PlayIcon className="size-3" />
              </button>
            ) : isHead && headAction === "steer" ? (
              <button
                type="button"
                title={t("interrupt the current Codex turn and continue with this message")}
                onClick={onSteerHead}
                className="flex size-5 shrink-0 items-center justify-center rounded text-accent hover:bg-hover"
              >
                <LightningIcon className="size-3" />
              </button>
            ) : null}
            <button
              type="button"
              title={t("remove from queue")}
              onClick={() => onRemove(message)} data-signal="failed" className="flex size-5 shrink-0 items-center justify-center rounded text-muted hover:bg-hover hover:text-danger"
            >
              <XIcon className="size-3" />
            </button>
          </div>
        );
      })}
    </div>
  );
}
