import type { components } from "@/api/schema";
import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from "react";
import { ReceiptChecker } from "./receipt-check";
import {
  resolveUnknowns,
  unknownIds,
  type AdoptedMessage,
  type MessageReceipt,
  type OutgoingMessage,
  type ReceiptLookup,
} from "./send-receipts";
import { useMachineRuntime } from "@/lib/machines/context";

/** How often unanswered sends are asked about while any are left. */
const RECHECK_EVERY_MS = 15_000;

/**
 * Settle this tab's `unknown` sends against what the server knows.
 *
 * Two sources, because each answers half the question. The receipts riding on
 * the session's state frames say what the server *has* — they arrive on every
 * reconnect for free, and settle a message that did arrive without a request.
 * Only `GET …/receipts?ids=` can say what it *never saw*, which is the answer
 * that makes a message safe to send again; that is asked on mount, when the
 * browser comes back online or back into view, when a new connection to the
 * machine is up (cloud mode: a send lost with its channel is settled as soon
 * as there is a channel to ask on), every fifteen seconds while anything is
 * still unknown, and whenever the user asks. Each check gives up after a
 * bounded wait and is followed by another after a backoff while anything is
 * still unknown (`receipt-check.ts`), so a lost answer never stops the asking.
 *
 * `onAdopted` is told about every message the server turned out to have, which
 * leaves the outbox here: the caller shows it where an accepted send is shown.
 */
export function useOutboxResolution({
  source,
  id,
  outbox,
  outboxRef,
  setOutbox,
  receipts,
  onAdopted,
}: {
  source: components["schemas"]["Source"];
  id: string;
  outbox: readonly OutgoingMessage[];
  outboxRef: RefObject<OutgoingMessage[]>;
  setOutbox: (next: OutgoingMessage[]) => void;
  /** The receipts on the session's latest state. */
  receipts: readonly MessageReceipt[] | undefined;
  onAdopted: (adopted: AdoptedMessage) => void;
}): { check: () => void; checking: boolean } {
  const runtime = useMachineRuntime();
  const api = runtime.api;
  const pending = unknownIds(outbox).join(",");
  const adoptedRef = useRef(onAdopted);
  adoptedRef.current = onAdopted;
  const setRef = useRef(setOutbox);
  setRef.current = setOutbox;

  const apply = useCallback(
    (lookup: { receipts: readonly MessageReceipt[]; unknown?: readonly string[] }) => {
      const resolved = resolveUnknowns(outboxRef.current ?? [], lookup);
      if (!resolved.changed) return;
      setRef.current(resolved.outbox);
      resolved.adopted.forEach((adopted) => adoptedRef.current(adopted));
    },
    [outboxRef],
  );
  const applyRef = useRef(apply);
  applyRef.current = apply;

  const [checking, setChecking] = useState(false);
  const checker = useMemo(
    () =>
      new ReceiptChecker({
        ids: () => unknownIds(outboxRef.current ?? []),
        ask: async (ids, signal) => {
          const res = await api.GET("/api/sessions/{source}/{id}/receipts", {
            params: { path: { source, id }, query: { ids: ids.join(",") } },
            signal,
          });
          if (!res.response.ok) throw new Error(`receipts: HTTP ${res.response.status}`);
          return (res.data as ReceiptLookup | undefined) ?? null;
        },
        apply: (lookup) => applyRef.current(lookup),
        onChecking: setChecking,
      }),
    [api, id, outboxRef, source],
  );
  useEffect(() => () => checker.stop(), [checker]);
  const check = useCallback(() => checker.check(), [checker]);

  // The frames first: free, and enough for everything that did arrive. A new
  // set of receipts is also the sign of a reconnect, which is the moment to
  // ask about whatever they did not settle.
  useEffect(() => {
    if (!pending || !receipts) return;
    apply({ receipts });
    check();
  }, [apply, check, pending, receipts]);

  useEffect(() => {
    if (!pending) return;
    check();
    const again = () => {
      if (document.visibilityState !== "hidden") check();
    };
    // A new connection to the machine (a new channel in cloud mode), or the
    // page switching between the machine and its offline copy: whatever the
    // old one lost can be asked about on this one, at once.
    const live = runtime.transport;
    const offs = [live.onReconnect?.(() => checker.reconnected()), live.onChange?.(() => checker.reconnected())];
    window.addEventListener("online", again);
    document.addEventListener("visibilitychange", again);
    const timer = setInterval(again, RECHECK_EVERY_MS);
    return () => {
      for (const off of offs) off?.();
      window.removeEventListener("online", again);
      document.removeEventListener("visibilitychange", again);
      clearInterval(timer);
    };
  }, [check, checker, pending, runtime]);

  return { check, checking };
}
