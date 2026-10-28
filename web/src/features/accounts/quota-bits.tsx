import { useTranslation } from "react-i18next";
import { Badge } from "@/components/ui/badge";
import {
  quotaAge,
  usageNotice,
  type AccountUsageState,
  type CodexQuotaSource,
  type QuotaReadingSource,
  type QuotaWindow,
} from "@/features/accounts/use-accounts";
import { cn } from "@/lib/cn";
import { formatRelative, formatResetAt, formatUntil } from "@/lib/format";
import { MachineLink } from "@/app/machine-route";
import { agentsSearch } from "@/features/settings/agents/agents-tabs";

/**
 * The quota readouts shared by the composer gauges, the account picker and the
 * popover overview. They all draw the same window twice over — a thin "how much
 * is left" bar and a fuller "how much is gone" row — so the colour thresholds
 * live here once rather than drifting per call site.
 */

/** Bars that read as "left" go red as the window empties. */
export function remainingTone(remaining: number): string {
  return remaining <= 10 ? "bg-danger" : remaining <= 30 ? "bg-warn" : "bg-fg/70";
}

/** Overview bars read as "used", so they warn as the window fills up. */
export function usedTone(used: number): string {
  return used >= 90 ? "bg-danger" : used >= 80 ? "bg-warn" : "bg-fg/70";
}

/**
 * What is true of a reading *besides* its numbers: how old it is, where it came
 * from, and whether the account is out.
 *
 * Every place that shows a percentage shows this too. The bug it exists for was
 * a Codex panel reporting "30% of the week left" for an account that was at
 * 100% and blocked: the number was three days old, scraped out of a session
 * log, and nothing on screen said either thing. A reading that cannot be dated
 * (`unknown`) is drawn as differently from a fresh one as a stale reading is —
 * "no timestamp" is not "just now".
 */
export function QuotaReadingNote({
  updatedAt,
  source,
  blocked,
  className,
}: {
  updatedAt: string | null | undefined;
  /**
   * Codex: `session_log` is the fallback, and says so. Claude: `session` is a
   * live session's rate-limit events, `probe` the idle-time `/usage` fallback.
   */
  source?: CodexQuotaSource | QuotaReadingSource;
  /** Codex only; `null`/undefined is unknown, which says nothing either way. */
  blocked?: boolean | null;
  className?: string;
}) {
  const { t } = useTranslation();
  const age = quotaAge(updatedAt);
  return (
    <div className={cn("flex flex-wrap items-center gap-x-1.5 font-mono text-[9px] leading-none", className)}>
      {blocked === true && (
        <span data-signal="failed" className="rounded bg-danger/15 px-1 py-0.5 text-danger" title={t("Codex is refusing ordinary usage on this account until the window resets.")}>
          {t("blocked")}
        </span>
      )}
      {age === "fresh" && (
        <span className="text-faint" title={updatedAt ? new Date(updatedAt).toLocaleString() : undefined}>
          {t("read {{ago}} ago", { ago: formatRelative(updatedAt ?? null) })}
        </span>
      )}
      {age === "stale" && (
        <span data-signal="warn" className="text-warn-fg" title={updatedAt ? new Date(updatedAt).toLocaleString() : undefined}>
          {t("stale")} · {t("read {{ago}} ago", { ago: formatRelative(updatedAt ?? null) })}
        </span>
      )}
      {age === "unknown" && (
        <span data-signal="warn" className="text-warn-fg" title={t("nothing says when this reading was taken, so it may be any age")}>
          {t("age unknown")}
        </span>
      )}
      {source === "session_log" && (
        <span className="text-faint" title={t("the codex app server could not be reached; these numbers come out of session logs")}>
          {t("from session logs")}
        </span>
      )}
      {source === "session" && <span className="text-faint">{t("from sessions")}</span>}
      {source === "probe" && <span className="text-faint">{t("from /usage")}</span>}
    </div>
  );
}

/** The composer's inline fuel gauges: one tiny bar per window, % left. */
export function QuotaGauges({ windows }: { windows: QuotaWindow[] }) {
  return (
    <>
      {windows.map((w) => {
        const remaining = Math.max(0, Math.min(100, w.remainingPercent));
        return (
          <span key={`${w.label} ${w.windowMinutes}`} className="flex items-center gap-0.5">
            <span data-signal="quota" className="h-[3px] w-5 overflow-hidden bg-border">
              <span
                className={cn("block h-full", remainingTone(remaining))}
                style={{ width: `${remaining}%` }}
              />
            </span>
            <span className="font-mono text-[9px] leading-none tabular-nums text-faint">
              {Math.round(remaining)}%
            </span>
          </span>
        );
      })}
    </>
  );
}

/** One "5h" / "weekly" / "weekly (opus)" row of the popover overview. */
export function QuotaWindowRow({ w }: { w: QuotaWindow }) {
  const { t } = useTranslation();
  // The bar fills as the window is consumed (as on the stats page), while the
  // number stays the "left" reading the rest of the UI shows.
  const used = Math.max(0, Math.min(100, w.usedPercent));
  const resetAt = formatResetAt(w.resetsAt);
  // Windows of one reading can differ in age — a session reports the ones it
  // was told about, and the rest are whatever was read last — while the note
  // under them dates only the newest. A window that carries its own time and
  // is no longer fresh is dimmed, with its age on hover, rather than passing
  // for as current as its siblings.
  const age = w.updatedAt ? quotaAge(w.updatedAt) : "fresh";
  return (
    <div
      className={age !== "fresh" ? "opacity-50" : undefined}
      title={
        age === "stale"
          ? t("read {{ago}} ago", { ago: formatRelative(w.updatedAt ?? null) })
          : age === "unknown"
            ? t("age unknown")
            : undefined
      }
    >
      {/*
        Keep all three readings on one dense row. The label and reset cells may
        truncate independently, with the exact reset timestamp still available
        in the title, so a long localized date cannot double every row's height.
      */}
      <div className="grid grid-cols-[minmax(0,1.15fr)_auto_minmax(0,1.35fr)] items-baseline gap-x-1.5 font-mono text-2xs leading-4">
        <span className="min-w-0 truncate text-muted">{w.label}</span>
        <span className={cn("shrink-0 tabular-nums", used >= 80 ? "text-warn-fg" : "text-fg")}>
          {Math.round(w.remainingPercent)}% {t("left")}
        </span>
        {w.resetsAt && (
          <span
            className="min-w-0 truncate text-right text-[10px] leading-none whitespace-nowrap tabular-nums text-faint"
            title={resetAt ? new Date(w.resetsAt).toLocaleString() : undefined}
          >
            ↻ {formatUntil(w.resetsAt)}
            {resetAt && ` · ${resetAt}`}
          </span>
        )}
      </div>
      <div data-signal="quota" className="mt-0.5 h-1 overflow-hidden bg-border">
        <div className={cn("h-full rounded", usedTone(used))} style={{ width: `${used}%` }} />
      </div>
    </div>
  );
}

/** One account (or the local login) inside the popover overview. */
export function QuotaBlock({
  title,
  chip,
  email,
  windows,
  current,
  usage = "ready",
  updatedAt,
  source,
  blocked,
  notice,
}: {
  title: string;
  chip?: string;
  email?: string;
  windows: QuotaWindow[];
  current: boolean;
  /**
   * When these numbers were read. Passed wherever the caller has it — a block
   * that draws percentages without saying how old they are is the whole defect
   * this note exists for — and omitted only where there is nothing to date.
   */
  updatedAt?: string | null;
  /** Which reading this is: Codex's app server or log, Claude's session or probe. */
  source?: CodexQuotaSource | QuotaReadingSource;
  /** Codex only: whether ordinary usage is blocked right now. */
  blocked?: boolean | null;
  /**
   * Account blocks only, and only for the line about the /usage fallback: a
   * missing login points at the wizard, and a failed probe is transient — it
   * says nothing about the login, so it must not read as one that needs
   * redoing. Windows are drawn whatever this is.
   */
  usage?: AccountUsageState;
  /** A line above the readings, such as the account resting after a limit. */
  notice?: React.ReactNode;
}) {
  const { t } = useTranslation();
  // Sessions report any account's windows, so the usage state no longer decides
  // whether there are numbers — only what to say about the /usage fallback, and
  // that faintly while the numbers are there and current (see `usageNotice`).
  const usageLine = usageNotice(usage, updatedAt, windows.length > 0);
  return (
    <div className={cn("px-1.5 py-0.5", current && "bg-hover ring-1 ring-fg/40")}>
      {notice}
      <div className="mb-0.5 flex min-w-0 items-center gap-1.5">
        <span className={cn("min-w-0 truncate font-mono text-2xs text-fg", current && "font-semibold")}>
          {title}
        </span>
        {chip && <Badge variant="outline">{chip}</Badge>}
        {email && (
          <span className="ml-auto min-w-0 truncate font-mono text-[9px] leading-none text-faint">{email}</span>
        )}
      </div>
      {usageLine === "unavailable" && (
        <div className="font-mono text-[10px] text-faint" title={t("the last quota probe failed; it will be retried")}>
          {t("usage unavailable")}
        </div>
      )}
      {(usageLine === "login_needed" || usageLine === "login_needed_idle") && (
        <div className="font-mono text-[10px] text-faint">
          {usageLine === "login_needed"
            ? t("usage not enabled")
            : t("usage not enabled · only affects the idle-time fallback")}
          {" · "}
          <MachineLink to="/settings" search={agentsSearch("claude")} className="text-accent hover:underline">
            {t("Settings")}
          </MachineLink>
        </div>
      )}
      {(usageLine === "login_expired" || usageLine === "login_expired_idle") && (
        <div className={cn("font-mono text-[10px]", usageLine === "login_expired" ? "text-warn-fg" : "text-faint")}>
          {usageLine === "login_expired"
            ? t("usage login expired")
            : t("usage login expired · only affects the idle-time fallback")}
          {" · "}
          <MachineLink to="/settings" search={agentsSearch("claude")} className="text-accent hover:underline">
            {t("Settings")}
          </MachineLink>
        </div>
      )}
      {/* Whatever the usage state: sessions report windows for any account, and
          the server keeps the last good ones when a probe fails or a login
          expires. The toolbar gauges draw them, so this shows the same numbers,
          dated, rather than a line that says there are none. */}
      {windows.length > 0 ? (
        <div className="flex flex-col gap-0.5">
          {windows.map((w) => (
            <QuotaWindowRow key={`${w.label} ${w.windowMinutes}`} w={w} />
          ))}
          {updatedAt !== undefined && (
            <QuotaReadingNote updatedAt={updatedAt} source={source} blocked={blocked} className="mt-0.5" />
          )}
        </div>
      ) : (
        usage === "ready" && <div className="font-mono text-[10px] text-faint">{t("no window data")}</div>
      )}
    </div>
  );
}
