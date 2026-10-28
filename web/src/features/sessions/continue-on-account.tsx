/*
 * "Continue on another account": the way out for a Claude session stuck on the
 * local login.
 *
 * A session started on the local login keeps its conversation in the user's
 * own `~/.claude`, and an account cannot run it there — the server refuses that
 * bind (`local_session_cannot_use_account`). When the local login stops
 * working, the session used to be a dead end: its badge was a plain "local"
 * with nothing behind it, and its messages failed. This offers the one thing
 * that does work: the server copies the conversation into a *new* session on a
 * stored account (`POST …/continue`), and the view moves to it. The session it
 * came from stays exactly as it was.
 *
 * Drawn in two places, from the same panel: the header badge's popover, and the
 * failure line under the composer when a message failed on the local login.
 */
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ArrowRight, Loader2 } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { errorText } from "@/api/errors";
import type { components } from "@/api/schema";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { AccountRows } from "@/features/accounts/account-picker";
import {
  accountsForProvider,
  continueOnAccount,
  quotaKey,
  useAccounts,
  type ContinuedSession,
} from "@/features/accounts/use-accounts";
import { useMachine } from "@/lib/machines/context";

/**
 * Whether a run's failure text reads as the login being refused — the CLI's
 * "Failed to authenticate", "OAuth session expired", "Invalid API key", a 401 —
 * for the moment before the server's record of the login has caught up.
 */
export function looksLikeLoginFailure(error: string | null | undefined): boolean {
  return !!error && /authenticat|oauth|\/login|invalid api key|not logged in|\b401\b/i.test(error);
}

/** Where the view goes once the new session exists. */
export type OpenSession = (source: components["schemas"]["Source"], id: string) => void;

/**
 * The stored Claude accounts to continue on, then a confirmation saying what
 * happens to which session, then the new session.
 */
export function ContinueOnAccountPanel({
  id,
  onOpenSession,
  onDone,
}: {
  /** The local-login Claude session to carry over. */
  id: string;
  onOpenSession: OpenSession;
  /** The panel's work is over (opened, or cancelled from the confirmation). */
  onDone?: () => void;
}) {
  const { t } = useTranslation();
  const machine = useMachine();
  const queryClient = useQueryClient();
  const { data: accountList } = useAccounts();
  const accounts = accountsForProvider(accountList?.accounts ?? [], "claude");
  const [pending, setPending] = useState<{ id: string; label: string } | null>(null);

  const start = useMutation({
    mutationFn: (accountId: string) => continueOnAccount({ machine, source: "claude", id }, accountId),
    onSuccess: (continued: ContinuedSession) => {
      // The new session bills the account: its quota reading is the one to show.
      void queryClient.invalidateQueries({ queryKey: quotaKey(machine) });
      setPending(null);
      onDone?.();
      onOpenSession(continued.source, continued.id);
    },
  });

  if (pending) {
    return (
      <div data-continue-confirm={pending.id} className="flex flex-col gap-2">
        <p className="text-2xs text-muted">
          {t(
            "A new session starts on {{label}} with a copy of this conversation. This session stays as it is, on your own login.",
            { label: pending.label },
          )}
        </p>
        {start.isError && (
          <p data-signal="failed" className="font-mono text-2xs text-danger">
            {errorText(start.error)}
          </p>
        )}
        <div className="flex items-center gap-1.5">
          <button
            type="button"
            data-variant="primary"
            disabled={start.isPending}
            onClick={() => start.mutate(pending.id)}
            className="flex h-6.5 items-center gap-1.5 bg-fg px-2.5 text-xs font-medium text-bg hover:bg-fg/85 disabled:opacity-40"
          >
            {start.isPending && <Loader2 className="size-3 animate-spin" />}
            {t("continue")}
          </button>
          <button
            type="button"
            onClick={() => {
              start.reset();
              setPending(null);
            }}
            className="flex h-6.5 items-center rounded border border-border px-2.5 font-mono text-2xs text-muted hover:bg-hover hover:text-fg"
          >
            {t("cancel")}
          </button>
        </div>
      </div>
    );
  }

  return (
    <div data-continue-on-account="" className="flex flex-col gap-0.5">
      <div className="px-1 pb-1 font-mono text-2xs text-faint">{t("continue this conversation on")}</div>
      <AccountRows
        accounts={accounts}
        provider="claude"
        // Nothing is "in force" here: every row is a new session.
        value={null}
        onSelect={(accountId) => {
          if (!accountId) return;
          const picked = accounts.find((a) => a.id === accountId);
          setPending({ id: accountId, label: picked?.label ?? accountId });
        }}
      />
    </div>
  );
}

/**
 * The same panel behind a button, for the failure line under the composer.
 */
export function ContinueOnAccountButton({ id, onOpenSession }: { id: string; onOpenSession: OpenSession }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger
        data-continue-trigger=""
        className="ml-2 inline-flex items-center gap-1 rounded bg-danger/15 px-1.5 py-px align-baseline font-mono text-2xs text-danger hover:bg-danger/25"
      >
        {t("continue on another account")}
        <ArrowRight className="size-3 shrink-0" />
      </PopoverTrigger>
      <PopoverContent side="top" className="max-h-[50vh] w-72 max-w-[calc(100vw-2rem)] overflow-y-auto">
        <ContinueOnAccountPanel id={id} onOpenSession={onOpenSession} onDone={() => setOpen(false)} />
      </PopoverContent>
    </Popover>
  );
}
