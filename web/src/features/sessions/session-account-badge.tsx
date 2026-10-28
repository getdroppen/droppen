import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ChevronDown, Loader2 } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "@/api/schema";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { AccountRows, localLoginHint } from "@/features/accounts/account-picker";
import { restTitle, sessionRest } from "@/features/accounts/account-rest";
import { RestChip } from "@/features/accounts/rest-chip";
import { rotationMarkerShown, rotationMarkerTitle, type SessionRotation } from "@/features/accounts/session-rotation";
import {
  accountsForProvider,
  bindSessionAccount,
  localLoginOf,
  localLoginSignedOut,
  quotaKey,
  useAccounts,
  type LocalLogin,
  type SessionSource,
} from "@/features/accounts/use-accounts";
import { ContinueOnAccountPanel, type OpenSession } from "@/features/sessions/continue-on-account";
import { cn } from "@/lib/cn";
import { useMachine } from "@/lib/machines/context";

type ClaudeRoot = components["schemas"]["ClaudeRoot"];
type AccountRef = components["schemas"]["AccountRef"];
type SessionScope = components["schemas"]["SessionScope"];

/**
 * Which login a session runs under, in the session header.
 *
 * For Claude, only a session in the managed root can be repointed at another
 * stored account. A session whose transcript is in the user's own `~/.claude`
 * keeps the user's own login: an account spawn runs against this instance's
 * root, and the server refuses the bind with `local_session_cannot_use_account`
 * rather than fork the conversation into it. Such a session's badge explains
 * that instead of being a dead end, and — for a session this server started —
 * offers the one move that works: continue the conversation on a stored
 * account, as a new session (`continue-on-account.tsx`). A session this server
 * does not own only reports the login it runs on.
 *
 * Codex has no managed transcript root: an account is a set of environment
 * variables the next turn is spawned with, so any session this instance owns
 * can be repointed, and one with nothing bound simply runs on `~/.codex`.
 */
export function SessionAccountBadge({
  source,
  id,
  claudeRoot,
  account,
  scope,
  model,
  rotation,
  onOpenSession,
}: {
  /** Which CLI this session belongs to; it picks the accounts and the route. An ACP agent has no accounts. */
  source: Exclude<SessionSource, "acp">;
  id: string;
  claudeRoot: ClaudeRoot | undefined;
  account: AccountRef | undefined;
  scope: SessionScope;
  /**
   * The model the session last ran on (`cliChoice.model`), which decides
   * whether a model-scoped rest on its account concerns it.
   */
  model?: string | null;
  /**
   * The session's latest automatic account rotation; a recent one that moved
   * it onto the bound account earns that account a marker.
   */
  rotation?: SessionRotation | null;
  /**
   * Show another session in this one's place. With it, a local-login Claude
   * session this server started offers to continue on an account.
   */
  onOpenSession?: OpenSession;
}) {
  const { t } = useTranslation();
  const machine = useMachine();
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState<{ id: string; label: string } | null>(null);
  const { data: accountList } = useAccounts();

  const settle = () => {
    // The session list and this session's own state both arrive over
    // websockets; the quota poll is the only cache that would keep showing
    // the old binding's windows.
    void queryClient.invalidateQueries({ queryKey: quotaKey(machine) });
    setPending(null);
    setOpen(false);
  };

  const bind = useMutation({
    mutationFn: (accountId: string) => bindSessionAccount({ machine, source, id }, accountId),
    onSuccess: settle,
  });

  const codex = source === "codex";
  // `claudeRoot` is a Claude notion; a Codex session runs on the user's own
  // `~/.codex` until an account is bound, and binding is what moves it off.
  const local = codex ? !account : claudeRoot === "local";
  // A Claude session on the user's own login stays on it, owned or not.
  const bindable = codex ? scope === "owned" : claudeRoot === "managed";

  // Each CLI runs under its own accounts; a Claude login is not on offer here
  // for a Codex session.
  const accounts = accountsForProvider(accountList?.accounts ?? [], source);
  // A Codex header said nothing about accounts before there were any, and still
  // says nothing: with none stored and none bound there is no choice to offer.
  if (codex && accounts.length === 0 && !account) return null;

  if (!bindable) {
    // For Codex the header's own `codex` chip already says that much.
    if (codex || !local) return null;
    return (
      <LocalLoginBadge
        id={id}
        owned={scope === "owned"}
        login={localLoginOf(accountList, "claude")}
        onOpenSession={onOpenSession}
      />
    );
  }

  // A managed session with nothing bound refuses every message; a local one
  // simply runs on the user's own login, which is not a warning.
  const unbound = !local && !account;
  const label = account ? account.label : local ? t("local") : t("no account");
  // A bound account resting on this session's model (or on everything). The
  // local login's rests are not shown here in phase 1.
  const rest = account ? sessionRest(accountList?.accounts.find((a) => a.id === account.id), model) : null;
  // The server moved the session onto this account after the last one hit a
  // limit: a quiet mark on the name, with the where-from and why on hover.
  const rotatedTitle = rotationMarkerShown(rotation, account?.id) ? rotationMarkerTitle(rotation, t) : null;

  return (
    <>
      <Popover
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) setPending(null);
        }}
      >
        <PopoverTrigger
          className={cn(
            // `min-w-0` so the label is one of the things that gives when the
            // header row runs out of width, rather than pushing a phone's
            // second line into a third.
            "inline-flex min-w-0 items-center gap-0.5 rounded px-1.5 py-px font-mono text-2xs leading-4 whitespace-nowrap",
            unbound ? "bg-warn/15 text-warn-fg" : "bg-raised text-muted hover:bg-hover hover:text-fg",
          )}
          title={
            unbound
              ? t("no account bound — messages will be refused")
              : local
                ? t("runs on your own codex login")
                : t("switch account")
          }
        >
          <span className="max-w-40 truncate">{label}</span>
          {rotatedTitle && (
            <span role="img" aria-label={rotatedTitle} title={rotatedTitle} className="shrink-0 text-accent">
              ↻
            </span>
          )}
          <ChevronDown className="size-2.5 shrink-0 opacity-70" />
        </PopoverTrigger>
        <PopoverContent
          side="bottom"
          // Wider than a bare name list: every row now carries a fuel gauge.
          className="max-h-[50vh] w-72 max-w-[calc(100vw-2rem)] overflow-y-auto"
        >
          {pending ? (
            <div className="flex flex-col gap-2">
              <p className="text-2xs text-muted">
                {t("Subsequent messages will run under {{label}}.", { label: pending.label })}
              </p>
              {bind.isError && <p data-signal="failed" className="font-mono text-2xs text-danger">{bind.error.message}</p>}
              <div className="flex items-center gap-1.5">
                <button
                  type="button"
                  disabled={bind.isPending}
                  onClick={() => bind.mutate(pending.id)}
                  data-variant="primary" className="flex h-6.5 items-center gap-1.5 bg-fg px-2.5 text-xs font-medium text-bg hover:bg-fg/85 disabled:opacity-40"
                >
                  {bind.isPending && <Loader2 className="size-3 animate-spin" />}
                  {t("switch")}
                </button>
                <button
                  type="button"
                  onClick={() => setPending(null)}
                  className="flex h-6.5 items-center rounded border border-border px-2.5 font-mono text-2xs text-muted hover:bg-hover hover:text-fg"
                >
                  {t("cancel")}
                </button>
              </div>
            </div>
          ) : (
            <div className="flex flex-col gap-0.5">
              <div className="px-1 pb-1 font-mono text-2xs text-faint">{t("run this session under")}</div>
              <AccountRows
                accounts={accounts}
                provider={source}
                value={account?.id ?? null}
                onSelect={(accountId) => {
                  // The rows never yield null here: the local login is not one
                  // of them, because leaving an account is not a bind.
                  if (!accountId) return;
                  const picked = accounts.find((a) => a.id === accountId);
                  setPending({ id: accountId, label: picked?.label ?? accountId });
                }}
              />
            </div>
          )}
        </PopoverContent>
      </Popover>
      {rest && (
        <RestChip
          rest={rest}
          warning
          className="max-w-56 shrink"
          title={`${t("This account hit a limit and is resting; turns on it may fail until it recovers.")}\n${restTitle(rest, t)}`}
        />
      )}
    </>
  );
}

/**
 * A Claude session on the user's own login: why it cannot switch accounts, the
 * login's state, and — for a session this server started — the way to carry
 * the conversation onto an account.
 */
function LocalLoginBadge({
  id,
  owned,
  login,
  onOpenSession,
}: {
  id: string;
  owned: boolean;
  login: LocalLogin | null;
  onOpenSession?: OpenSession;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const signedOut = localLoginSignedOut(login);
  // The login's own rest — a refused credential, or a window it ran out of —
  // concerns every session on it, so it shows beside the badge like an
  // account's does.
  const rest = sessionRest(login ?? undefined, null);
  const why = t(
    "This session runs on your own claude login on this device, and its conversation is kept there, so it cannot switch to an account.",
  );
  return (
    <>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger
          data-local-badge={login?.state ?? "unknown"}
          title={signedOut ? `${why}\n${localLoginHint("claude", t)}` : why}
          className={cn(
            "inline-flex min-w-0 items-center gap-0.5 rounded px-1.5 py-px font-mono text-2xs leading-4 whitespace-nowrap",
            signedOut ? "bg-danger/15 text-danger hover:bg-danger/25" : "bg-raised text-muted hover:bg-hover hover:text-fg",
          )}
        >
          <span className="max-w-40 truncate">{signedOut ? t("local (not signed in)") : t("local")}</span>
          <ChevronDown className="size-2.5 shrink-0 opacity-70" />
        </PopoverTrigger>
        <PopoverContent side="bottom" className="max-h-[60vh] w-72 max-w-[calc(100vw-2rem)] overflow-y-auto">
          <LocalLoginExplainer id={id} owned={owned} login={login} onOpenSession={onOpenSession} onDone={() => setOpen(false)} />
        </PopoverContent>
      </Popover>
      {rest && (
        <RestChip
          rest={rest}
          warning
          className="max-w-56 shrink"
          title={`${t("Your own login hit a limit; turns on it may fail until it recovers.")}\n${restTitle(rest, t)}`}
        />
      )}
    </>
  );
}

/**
 * What the local badge's popover says; exported on its own so it can be drawn
 * without a popover around it.
 */
export function LocalLoginExplainer({
  id,
  owned,
  login,
  onOpenSession,
  onDone,
}: {
  id: string;
  owned: boolean;
  login: LocalLogin | null;
  onOpenSession?: OpenSession;
  onDone?: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div data-local-explainer="" className="flex flex-col gap-2">
      <p className="text-2xs text-muted">
        {t(
          "This session runs on your own claude login on this device, and its conversation is kept there, so it cannot switch to an account.",
        )}
      </p>
      {localLoginSignedOut(login) && (
        <p data-signal="warn" className="text-2xs text-warn-fg">
          {localLoginHint("claude", t)}
        </p>
      )}
      {owned && onOpenSession ? (
        <div className="border-t border-border pt-2">
          <ContinueOnAccountPanel id={id} onOpenSession={onOpenSession} onDone={onDone} />
        </div>
      ) : (
        !owned && (
          <p className="text-2xs text-faint">
            {t("It was not started by Droppen on this device, so it can only be read here.")}
          </p>
        )
      )}
    </div>
  );
}
