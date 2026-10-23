import { useState } from "react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/cn";
import { enablePush, forgetPushMoved, pushMovedFrom } from "@/lib/push";
import { ErrorLine, errorText, quiet } from "@/features/cloud/ui";

/*
 * After this browser's device moved here from droppen.app (`lib/move`), with
 * its notifications on there: they did not come along — a push subscription
 * belongs to the old origin's service worker, and the permission to show
 * notifications is per origin — so one tap turns them back on here (Safari
 * and Firefox ask for permission only from a tap). "Not now" stops asking.
 */
export function PushMovedNotice({
  moved = pushMovedFrom,
  enable = enablePush,
  dismiss = forgetPushMoved,
}: {
  moved?: () => string | null;
  enable?: (machine: string) => Promise<void>;
  dismiss?: () => void;
}) {
  const { t } = useTranslation();
  const [machine, setMachine] = useState(moved);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  if (!machine) return null;
  return (
    <div data-testid="push-moved" data-signal="warn" className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border bg-warn/7 px-6 py-2.5 shadow-[inset_2px_0_0_var(--color-warn)] max-md:px-4">
      <span className="min-w-0 flex-1 text-xs leading-5 text-fg">{t("Notifications from {{id}} were on before this browser moved here. They have to be turned on again here.", { id: machine })}</span>
      <button
        type="button"
        data-testid="push-moved-on"
        className={cn(quiet, "h-7 px-2")}
        disabled={busy}
        onClick={async () => {
          setBusy(true);
          setError(null);
          try {
            // Subscribing clears the note (`enablePush`).
            await enable(machine);
            setMachine(null);
          } catch (err) {
            setError(errorText(err));
          } finally {
            setBusy(false);
          }
        }}
      >
        {t("turn notifications back on")}
      </button>
      <button
        type="button"
        data-testid="push-moved-dismiss"
        className={cn(quiet, "h-7 px-2")}
        disabled={busy}
        onClick={() => {
          dismiss();
          setMachine(null);
        }}
      >
        {t("not now")}
      </button>
      <ErrorLine message={error} />
    </div>
  );
}
