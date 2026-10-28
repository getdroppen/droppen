import { ChevronDown } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Badge } from "@/components/ui/badge";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { QuotaGauges } from "@/features/accounts/quota-bits";
import {
  accountKindLabel,
  accountUsageState,
  localLoginLabel,
  localLoginOf,
  localLoginSignedOut,
  quotaAge,
  quotaDoubt,
  tightestWindow,
  useAccounts,
  useQuota,
  type Account,
  type AccountProvider,
  type AccountUsageState,
  type Quota,
  type QuotaWindow,
} from "@/features/accounts/use-accounts";
import { cn } from "@/lib/cn";
import { formatRelative, formatResetAt } from "@/lib/format";
import { MachineLink } from "@/app/machine-route";
import { agentsSearch } from "@/features/settings/agents/agents-tabs";

/**
 * Which login to run under, with how much fuel each one has left.
 *
 * The choice used to be a bare `<select>` of names, which made the one thing
 * that decides it — remaining quota — invisible until after the session was
 * started. Every row therefore carries the tightest of that account's windows:
 * a login with 80% of its week but 3% of its five hours is empty *now*, so the
 * summary shows the window closest to running out rather than an average.
 *
 * Readings come from the shared quota cache only. Opening the list never
 * triggers a probe: the server spends a small hourly budget of `claude /usage`
 * calls per account, and a picker that refetched on open would burn it.
 */

/** The one-line usage summary at the right of a row, or its excuse for having none. */
function RowQuota({
  usage,
  windows,
  updatedAt,
}: {
  usage: AccountUsageState;
  windows: QuotaWindow[];
  updatedAt: string | null;
}) {
  const { t } = useTranslation();

  // Sessions report windows for any account — one that never had a /usage
  // login, an env block on a relay that passes rate limits through — so the
  // tightest window is shown whenever there is one, in every usage state. The
  // usage state is only the excuse for having no numbers, and then reads
  // exactly as it always has.
  const w = tightestWindow(windows);
  if (!w) {
    // An API key has no /usage endpoint to read, so it gets no summary at all —
    // the "API key" chip beside the label is the whole explanation.
    if (usage === "not_applicable") return null;
    if (usage === "unavailable") {
      return (
        <span
          className="ml-auto shrink-0 font-mono text-[9px] leading-none text-faint"
          title={t("the last quota probe failed; it will be retried")}
        >
          {t("usage unavailable")}
        </span>
      );
    }
    if (usage === "login_needed") {
      return (
        <span className="ml-auto shrink-0 font-mono text-[9px] leading-none text-faint">{t("usage not enabled")}</span>
      );
    }
    if (usage === "login_expired") {
      return (
        <span data-signal="warn" className="ml-auto shrink-0 font-mono text-[9px] leading-none text-warn-fg">{t("usage login expired")}</span>
      );
    }
    return <span className="ml-auto shrink-0 font-mono text-[9px] leading-none text-faint">{t("no window data")}</span>;
  }

  // A number the server has not refreshed in a while is a number about the
  // past. Say so rather than letting it pass for the current reading — and say
  // it for a reading that carries no timestamp at all, which by this point in
  // the row means windows nobody can date rather than an account still warming
  // up (that one returned above with no windows to show).
  const age = quotaAge(updatedAt);
  // This row prints the reset *time* rather than the countdown the overview
  // shows: there is no width here for both, "5h · 2h" read as two durations
  // with no hint which was which, and the clock reading is the one a person
  // plans around.
  const resetAt = formatResetAt(w.resetsAt);
  return (
    <span className="ml-auto flex shrink-0 items-center gap-1">
      <QuotaGauges windows={[w]} />
      <span
        className="font-mono text-[9px] leading-none text-faint"
        title={resetAt && w.resetsAt ? new Date(w.resetsAt).toLocaleString() : undefined}
      >
        {w.label}
        {resetAt ? ` · ${resetAt}` : ""}
      </span>
      {age !== "fresh" && (
        <span data-signal="warn" className="font-mono text-[9px] leading-none text-warn-fg"
          title={
            age === "stale"
              ? t("last read {{ago}} ago", { ago: formatRelative(updatedAt) })
              : t("nothing says when this reading was taken, so it may be any age")
          }
        >
          {age === "stale" ? t("stale") : t("age unknown")}
        </span>
      )}
    </span>
  );
}

function AccountRow({
  label,
  chip,
  email,
  note,
  local,
  selected,
  disabled,
  onSelect,
  usage,
  windows,
  updatedAt,
}: {
  label: string;
  chip?: string;
  email?: string;
  /** A warning under the name: what is wrong with this login and how to fix it. */
  note?: string;
  /** The local login's state, for the row that is the local login. */
  local?: string;
  selected: boolean;
  disabled: boolean;
  onSelect: () => void;
  usage: AccountUsageState;
  windows: QuotaWindow[];
  updatedAt: string | null;
}) {
  return (
    <button
      type="button"
      data-local-login={local}
      // Re-picking what is already in force is a no-op everywhere this is used,
      // and in the session badge it would open a pointless confirmation.
      disabled={disabled || selected}
      onClick={onSelect}
      className={cn(
        "flex w-full flex-col gap-0.5 rounded px-1.5 py-1 text-left font-mono text-2xs",
        selected ? "bg-hover text-accent" : "text-fg hover:bg-hover",
        disabled && !selected && "opacity-40",
      )}
    >
      {/*
        The name is what the picker exists to show, so it never pays for the
        quota summary beside it: once a reset time is spelled out in full the
        pair no longer fits a phone-width row, and the quota drops to a second
        line of its own (`ml-auto` keeps it right) rather than truncating the
        name. `flex-auto` rather than `flex-1` so the wrap is decided against
        the name's real width; a row that fits still renders as one line.
      */}
      <span className="flex w-full flex-wrap items-center gap-x-1.5 gap-y-0.5">
        <span className="min-w-0 flex-auto truncate">{label}</span>
        {chip && <Badge variant="outline">{chip}</Badge>}
        <RowQuota usage={usage} windows={windows} updatedAt={updatedAt} />
      </span>
      {email && <span className="w-full truncate text-[10px] text-faint">{email}</span>}
      {note && <span data-signal="warn" className="w-full text-[10px] leading-snug whitespace-normal text-warn-fg">{note}</span>}
    </button>
  );
}

export type AccountRowsProps = {
  /** The stored accounts, in the order the server lists them. */
  accounts: Account[];
  /** The account in force; `null` is the local login. */
  value: string | null;
  /** A row was picked. `null` means the local login row. */
  onSelect: (id: string | null) => void;
  /** Offer the local login as the first row (new sessions only). */
  allowLocal?: boolean;
  /**
   * Which CLI these accounts and the local login belong to. It decides whose
   * quota the local row repeats: a Codex picker showing Claude's windows would
   * be describing a login it cannot start anything under.
   */
  provider?: AccountProvider;
  disabled?: boolean;
  /** A borderless trigger, for a composer's chips row (`ComposerSelect` ghost). */
  ghost?: boolean;
};

/** How to sign the local login back in, said where the login is offered. */
export function localLoginHint(provider: AccountProvider, t: (key: string) => string): string {
  return provider === "codex"
    ? t("your own codex login on this device is signed out: run `codex login` in a terminal on this device")
    : t("your own claude login on this device is signed out: run `claude` in a terminal on this device, then `/login`");
}

/** The local login's own reading, for whichever CLI the picker is about. */
function localQuota(
  quota: Quota | undefined,
  provider: AccountProvider,
): { plan?: string | null; windows: QuotaWindow[]; updatedAt: string | null } | null {
  return (provider === "codex" ? quota?.codex : quota?.claude) ?? null;
}

/**
 * The rows on their own, for callers that already own a popover — the session
 * badge keeps its two-step confirmation in the same panel, so it supplies the
 * container and this only renders the choices and reports the pick.
 */
export function AccountRows({
  accounts,
  value,
  onSelect,
  allowLocal = false,
  provider = "claude",
  disabled = false,
}: AccountRowsProps) {
  const { t } = useTranslation();
  // Cache read only: never a fetch of its own (see the note at the top).
  const { data: quota } = useQuota();
  const quotaById = new Map((quota?.accounts ?? []).map((a) => [a.id, a]));
  const local = localQuota(quota, provider);
  // The same cached list the caller drew its accounts from.
  const { data: list } = useAccounts();
  const login = localLoginOf(list, provider);
  const localLabel = localLoginLabel(login, t);

  return (
    <div className="flex flex-col gap-0.5">
      {allowLocal && (
        <AccountRow
          label={local?.plan ? `${localLabel} · ${local.plan}` : localLabel}
          local={login?.state ?? "unknown"}
          // Still a choice — the user may be about to sign in — but never one
          // made without saying what it costs.
          note={localLoginSignedOut(login) ? localLoginHint(provider, t) : undefined}
          selected={value === null}
          disabled={disabled}
          onSelect={() => onSelect(null)}
          usage="ready"
          windows={local?.windows ?? []}
          updatedAt={local?.updatedAt ?? null}
        />
      )}
      {accounts.map((a) => {
        const q = quotaById.get(a.id);
        return (
          <AccountRow
            key={a.id}
            label={a.label}
            chip={accountKindLabel(a.kind)}
            email={q?.email ?? a.email}
            selected={a.id === value}
            disabled={disabled}
            onSelect={() => onSelect(a.id)}
            usage={accountUsageState(a.kind, a.usageEnabled, q?.error)}
            windows={q?.windows ?? []}
            updatedAt={q?.updatedAt ?? null}
          />
        );
      })}
      {accounts.length === 0 && (
        <div className="px-1 py-1 text-2xs text-faint">
          {t("no accounts yet")}
          {" · "}
          <MachineLink to="/settings" search={agentsSearch(provider)} className="text-accent hover:underline">
            {t("Settings")}
          </MachineLink>
        </div>
      )}
    </div>
  );
}

/**
 * The rows behind a select-shaped trigger, for forms. The trigger repeats the
 * chosen login's tightest window, so the quota is legible without opening it.
 */
export function AccountPicker({
  accounts,
  value,
  onSelect,
  allowLocal = false,
  provider = "claude",
  disabled = false,
  ghost = false,
}: AccountRowsProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const { data: quota } = useQuota();
  const { data: list } = useAccounts();
  const login = localLoginOf(list, provider);

  const selected = value === null ? null : accounts.find((a) => a.id === value);
  const label = value === null ? localLoginLabel(login, t) : (selected?.label ?? t("no account"));
  // The local login picked while it is known to be signed out: the trigger says
  // so too, not only the row inside it.
  const signedOutPick = value === null && localLoginSignedOut(login);
  const activeQuota = value === null ? undefined : quota?.accounts.find((a) => a.id === value);
  const windows =
    value === null ? (localQuota(quota, provider)?.windows ?? []) : (activeQuota?.windows ?? []);
  // The trigger repeats the tightest window whenever there is one, whatever
  // the usage state (sessions report windows for any account), and marks it
  // with the composer gauges' `~` once it has aged; the rows inside say why.
  const summary = tightestWindow(windows);
  const doubt = quotaDoubt(
    selected ? accountUsageState(selected.kind, selected.usageEnabled, activeQuota?.error) : "ready",
    value === null ? (localQuota(quota, provider)?.updatedAt ?? null) : (activeQuota?.updatedAt ?? null),
  );

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger
        disabled={disabled}
        title={signedOutPick ? localLoginHint(provider, t) : t("account")}
        aria-label={t("account")}
        data-signal={signedOutPick ? "warn" : undefined}
        className={cn(
          ghost
            ? "flex h-6 max-w-full shrink-0 items-center gap-1 rounded pr-1 pl-1.5 font-mono text-2xs data-[popup-open]:bg-hover"
            : "flex h-5.5 max-w-full items-center gap-1 rounded border border-border bg-raised px-1.5 font-mono text-2xs focus:border-accent/60",
          signedOutPick ? "text-warn-fg" : "text-muted hover:text-fg",
          "focus:outline-none disabled:opacity-40",
          ghost && "hover:bg-hover",
        )}
      >
        <span className="max-w-40 truncate">{label}</span>
        {summary && <QuotaGauges windows={[summary]} />}
        {summary && doubt && (
          <span data-signal="warn" className="font-mono text-[9px] leading-none text-warn-fg"
            title={
              doubt === "login_expired"
                ? t("usage login expired")
                : doubt === "unavailable"
                  ? t("usage unavailable")
                  : doubt === "stale"
                    ? t("stale")
                    : t("age unknown")
            }
          >
            ~
          </span>
        )}
        <ChevronDown className="size-2.5 shrink-0 text-faint" />
      </PopoverTrigger>
      <PopoverContent side="bottom" className="max-h-[50vh] w-72 max-w-[calc(100vw-2rem)] overflow-y-auto">
        <AccountRows
          accounts={accounts}
          value={value}
          allowLocal={allowLocal}
          provider={provider}
          disabled={disabled}
          onSelect={(id) => {
            onSelect(id);
            setOpen(false);
          }}
        />
      </PopoverContent>
    </Popover>
  );
}
