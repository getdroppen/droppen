import { ArrowUUpLeftIcon, XIcon } from "@phosphor-icons/react";
import { useCallback, useLayoutEffect, useMemo, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import { LightboxImage } from "@/components/image-lightbox";
import { ReceiptBadge } from "./receipt-badge";
import { receiptState } from "./send-receipts";
import {
  createDeliveryStore, reconcileDeliveries, visibleDeliveries,
  type DeliveryAction, type DeliveryFrame, type DeliveryPreview,
} from "./delivery-preview";

const stores = new Map<string, ReturnType<typeof createDeliveryStore>>();
/** Who waits for a session's store to exist ({@link usePendingDeliveries} mounted before the composer). */
const awaitingStores = new Set<() => void>();

function putStore(key: string, store: ReturnType<typeof createDeliveryStore>): void {
  stores.set(key, store);
  // After the render that made it: telling another component now would update
  // it while React is still rendering this one.
  queueMicrotask(() => {
    for (const notify of [...awaitingStores]) notify();
  });
}

/**
 * Hand a session a preview before any view of it is open: the first message
 * of a session the new-session view just started, which the session's own
 * composer then shows as pending — as if it had sent it — until the
 * transcript echoes it.
 */
export function seedDeliveryPreview(key: string, entry: DeliveryPreview): void {
  let store = stores.get(key);
  if (!store) {
    store = createDeliveryStore(0);
    putStore(key, store);
  }
  store.change({ type: "start", entry });
}

/**
 * Session identity, not component lifetime, owns an unconfirmed display
 * receipt: `key` is the session's `sessionKey` (machine included).
 */
export function useDeliveryPreviews(key: string, frame: DeliveryFrame, enabled: boolean) {
  const store = useMemo(() => {
    for (const [id, candidate] of stores) if (id !== key && candidate.idle()) stores.delete(id);
    let existing = stores.get(key);
    if (!existing) { existing = createDeliveryStore(frame.totalItems); putStore(key, existing); }
    return existing;
  }, [key]);
  const state = useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
  const current = useMemo(() => enabled ? reconcileDeliveries(state, frame) : state,
    [state, enabled, frame.items, frame.queue, frame.startIndex, frame.totalItems, frame.receipts, frame.adoptQueue]);
  // Derive before rendering, commit before paint. Publishing during render would
  // update another pane while React was still rendering this one.
  useLayoutEffect(() => { if (enabled) store.reconcile(frame); },
    [store, enabled, frame.items, frame.queue, frame.startIndex, frame.totalItems, frame.receipts, frame.adoptQueue]);
  const change = (action: DeliveryAction) => {
    if (enabled) store.change(action);
  };
  return { previews: enabled ? visibleDeliveries(current, frame.queue) : [], change };
}

/**
 * How many of the messages this tab sent into the session the transcript has
 * not shown yet — the first one of a session just started among them. Reads
 * the session's previews without making them: the composer that does is
 * mounted after the transcript that asks.
 */
export function usePendingDeliveries(key: string): number {
  const subscribe = useCallback((onChange: () => void) => {
    let release = stores.get(key)?.subscribe(onChange);
    const attach = () => {
      const store = stores.get(key);
      if (release || !store) return;
      release = store.subscribe(onChange);
      onChange();
    };
    awaitingStores.add(attach);
    return () => {
      awaitingStores.delete(attach);
      release?.();
    };
  }, [key]);
  const read = useCallback(() => {
    const state = stores.get(key)?.getSnapshot();
    return state ? state.entries.filter((entry) => !entry.echoed && !entry.dropped).length : 0;
  }, [key]);
  return useSyncExternalStore(subscribe, read, read);
}

/**
 * The messages this tab sent that nothing else on screen shows yet.
 *
 * Each says where it is — sending, delivered, executed — in the words the
 * queues use. One the server dropped says so, and offers the only two things
 * worth doing about it: put it back in this tab's queue (as a new message: the
 * server has an answer on file for the old id), or let it go.
 */
export function DeliveryPreviews({
  messages,
  onRequeue,
  onDismiss,
  className = "max-h-40 overflow-y-auto border-b border-border px-3 py-2",
}: {
  messages: readonly DeliveryPreview[];
  onRequeue?: (message: DeliveryPreview) => void;
  onDismiss?: (id: string) => void;
  /** The strip around them: over a session's composer by default; the new-session view lays them in its own column. */
  className?: string;
}) {
  const { t } = useTranslation();
  if (!messages.length) return null;
  return (
    <div className={className} aria-live="polite">
      {messages.map((message) => (
        <div key={message.id} className="border border-border bg-raised px-2 py-1.5 not-first:mt-1.5">
          <div className="flex items-center gap-1.5 font-mono text-2xs text-muted">
            {message.sending ? (
              <ReceiptBadge state="sending" />
            ) : message.receipt ? (
              <ReceiptBadge state={receiptState(message.receipt)} />
            ) : (
              t("sent — waiting for transcript")
            )}
            {!!message.attachments && <span>{`📎${message.attachments}`}</span>}
            {message.receipt === "failed" && (
              <span className="ml-auto flex items-center gap-1">
                {message.resend && onRequeue && (
                  <button
                    type="button"
                    title={t("put it back in the queue")}
                    onClick={() => onRequeue(message)}
                    className="flex size-5 items-center justify-center rounded text-accent hover:bg-hover"
                  >
                    <ArrowUUpLeftIcon className="size-3" />
                  </button>
                )}
                {onDismiss && (
                  <button
                    type="button"
                    title={t("dismiss")}
                    onClick={() => onDismiss(message.id)}
                    className="flex size-5 items-center justify-center rounded text-muted hover:bg-hover"
                  >
                    <XIcon className="size-3" />
                  </button>
                )}
              </span>
            )}
          </div>
          <div className="mt-0.5 text-xs whitespace-pre-wrap break-words text-fg">{message.text}{message.truncated ? "…" : ""}</div>
          {/* The pictures it carries, as the transcript will show them once it is echoed. */}
          {!!message.resend?.images?.length && (
            <div data-preview-images="" className="mt-1 flex flex-wrap gap-1.5">
              {message.resend.images.map((path) => (
                <LightboxImage key={path} source={{ kind: "host", path }} alt={path} thumbClassName="size-10 rounded border border-border object-cover" />
              ))}
            </div>
          )}
        </div>
      ))}
    </div>
  );
}
