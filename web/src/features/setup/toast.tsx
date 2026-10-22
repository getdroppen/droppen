/*
 * The guide's one passing line: "Claude Code is signed in. You can go back to
 * Droppen." when a sign-in in step 01 lands,
 * and the note over the first session the guide opens. The app has no toasts
 * of its own; this is the smallest that does: one line at a time, kept in a
 * module store so it outlives the guide's page when it navigates away, read
 * out politely, gone after a few seconds or on its ×.
 */
import { X } from "lucide-react";
import { useEffect, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";

export interface SetupToast {
  id: number;
  /** An i18n key, with its placeholders. */
  text: string;
  vars?: Record<string, string | number>;
  /** How long it stays. */
  ms: number;
}

/** How long a line stays. */
export const TOAST_MS = 8_000;

let current: SetupToast | null = null;
let seq = 0;
const listeners = new Set<() => void>();
const notify = () => {
  for (const listener of listeners) listener();
};

/** Show `text` (an i18n key) in place of whatever line was there, for `ms`. */
export function showSetupToast(text: string, vars?: SetupToast["vars"], ms = TOAST_MS): SetupToast {
  current = { id: ++seq, text, vars, ms };
  notify();
  return current;
}

/** Take the line away, if it is still `id`'s. */
export function dismissSetupToast(id?: number): void {
  if (!current || (id !== undefined && current.id !== id)) return;
  current = null;
  notify();
}

/** The line on screen now, if any (tests read it). */
export function setupToastNow(): SetupToast | null {
  return current;
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => void listeners.delete(listener);
}

/** Where the line is drawn: once in the guide's window, once in the app's. */
export function SetupToasts() {
  const { t } = useTranslation();
  const toast = useSyncExternalStore(subscribe, setupToastNow, () => null);
  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => dismissSetupToast(toast.id), toast.ms);
    return () => clearTimeout(timer);
  }, [toast]);
  return (
    <div aria-live="polite" className="pointer-events-none fixed inset-x-0 bottom-16 z-50 flex justify-center px-4">
      {toast && (
        <div
          role="status"
          data-testid="setup-toast"
          className="pointer-events-auto flex max-w-[34rem] items-center gap-3 border border-fg bg-fg py-2 pr-2 pl-3.5 text-xs leading-[18px] text-bg"
        >
          <span className="min-w-0">{t(toast.text, toast.vars)}</span>
          <button
            type="button"
            aria-label={t("Close")}
            title={t("Close")}
            onClick={() => dismissSetupToast(toast.id)}
            className="flex size-5 shrink-0 items-center justify-center text-bg/70 hover:text-bg"
          >
            <X className="size-3" />
          </button>
        </div>
      )}
    </div>
  );
}
