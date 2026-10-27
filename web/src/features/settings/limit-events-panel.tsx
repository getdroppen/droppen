import { useQuery } from "@tanstack/react-query";
import { Check, ChevronRight, Copy, RefreshCw } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Badge } from "@/components/ui/badge";
import { Spinner } from "@/components/ui/spinner";
import {
  formatRestUntil,
  limitEventsKey,
  restLabel,
  restTone,
  type AccountLimitEvent,
} from "@/features/accounts/account-rest";
import type { Account } from "@/features/accounts/use-accounts";
import { useCopy } from "@/lib/clipboard";
import { cn } from "@/lib/cn";
import { formatExactTime, formatTime } from "@/lib/format";
import { useApi, useMachine } from "@/lib/machines/context";

/** How many events the panel asks for; the server keeps them newest first. */
const LIMIT_EVENTS_SHOWN = 100;

/** How often an open panel re-reads the log. */
const LIMIT_EVENTS_REFRESH_MS = 60_000;

function CopyRaw({ text }: { text: string }) {
  const { t } = useTranslation();
  const { copied, copy } = useCopy();
  return (
    <button
      type="button"
      title={copied ? t("copied") : t("copy")}
      aria-label={t("copy")}
      onClick={() => copy(text)}
      className="ml-auto flex size-5 shrink-0 items-center justify-center rounded text-muted hover:bg-hover hover:text-fg"
    >
      {copied ? <Check data-signal="ok" className="size-3 text-ok" /> : <Copy className="size-3" />}
    </button>
  );
}

function LimitEventRow({ event, accountName }: { event: AccountLimitEvent; accountName: string }) {
  const { t } = useTranslation();
  const exact = formatExactTime(event.observedAt) || event.observedAt;
  return (
    <li className="border-b border-border/40 py-1.5">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <span
          className="shrink-0 font-mono text-2xs tabular-nums text-faint"
          title={event.sessionId ? `${exact}\n${t("session: {{id}}", { id: event.sessionId })}` : exact}
        >
          {formatTime(event.observedAt) || event.observedAt}
        </span>
        <span
          className="max-w-40 truncate font-mono text-2xs text-fg"
          title={event.accountId ?? t("local login")}
        >
          {accountName}
        </span>
        <Badge variant={event.source.startsWith("codex") ? "codex" : "claude"}>{event.source}</Badge>
        {event.model && (
          <span className="max-w-48 truncate font-mono text-2xs text-muted" title={event.model}>
            {event.model}
          </span>
        )}
        <span
          className={cn(
            "inline-flex items-center rounded px-1.5 py-px font-mono text-2xs leading-4 whitespace-nowrap",
            restTone(event.class),
          )}
        >
          {restLabel(event, t)}
        </span>
        {event.untilAt && (
          <span
            className="font-mono text-2xs tabular-nums text-faint"
            title={formatExactTime(event.untilAt) || event.untilAt}
          >
            {t("rest until {{at}}", { at: formatRestUntil(event.untilAt) })}
          </span>
        )}
        <CopyRaw text={event.raw} />
      </div>
      {/*
        The whole message, wrapped rather than clipped: collecting the CLIs'
        real wording is what this log is for.
      */}
      <pre className="mt-1 rounded bg-raised px-2 py-1 font-mono text-2xs leading-4 break-words whitespace-pre-wrap text-fg select-text">
        {event.raw}
      </pre>
    </li>
  );
}

/**
 * The limit messages CLI turns produced, newest first, exactly as the CLI
 * printed them. Collapsed by default; the count is read once so the header
 * says whether there is anything to open, and an open panel refreshes itself.
 */
export function LimitEventsPanel({ accounts }: { accounts: Account[] }) {
  const machine = useMachine();
  const api = useApi();
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);

  const events = useQuery({
    queryKey: limitEventsKey(machine),
    queryFn: async () => {
      const res = await api.GET("/api/accounts/limit-events", {
        params: { query: { limit: LIMIT_EVENTS_SHOWN } },
      });
      // A server older than the log answers 404; say so instead of an empty list.
      if (!res.data) throw new Error(`HTTP ${res.response.status}`);
      return res.data;
    },
    staleTime: 30_000,
    refetchInterval: open ? LIMIT_EVENTS_REFRESH_MS : false,
  });

  const nameOf = (accountId: string | null): string => {
    if (accountId === null) return t("local login");
    return accounts.find((a) => a.id === accountId)?.label ?? accountId.slice(0, 8);
  };

  const list = events.data?.events ?? [];

  return (
    <div className="mt-4 w-full max-w-3xl">
      <div className="flex items-center gap-1.5 py-1">
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen((v) => !v)}
          className="flex h-7 items-center gap-1.5 rounded px-2 text-xs font-medium text-muted hover:bg-hover hover:text-fg"
        >
          <ChevronRight className={cn("size-3 transition-transform", open && "rotate-90")} />
          {t("limit log")}
          <span className="tabular-nums text-faint">
            {events.isPending ? "…" : events.isError ? "?" : list.length}
          </span>
        </button>
        {open && (
          <button
            type="button"
            title={t("refresh")}
            aria-label={t("refresh")}
            disabled={events.isFetching}
            onClick={() => void events.refetch()}
            className="flex size-6 items-center justify-center rounded text-muted hover:bg-hover hover:text-fg disabled:opacity-40"
          >
            <RefreshCw className={cn("size-3", events.isFetching && "animate-spin")} />
          </button>
        )}
      </div>
      {open && (
        <div>
          <p className="max-w-2xl text-2xs text-faint">
            {t("The last {{n}} limit messages CLI turns produced, newest first, as the CLI printed them.", {
              n: LIMIT_EVENTS_SHOWN,
            })}
          </p>
          {events.isPending && <Spinner />}
          {events.isError && (
            <p data-signal="failed" className="mt-1 font-mono text-2xs text-danger">
              {t("limit log unavailable: {{msg}}", { msg: events.error.message })}
            </p>
          )}
          {events.isSuccess && list.length === 0 && (
            <p className="py-2 font-mono text-2xs text-faint">{t("no limit messages recorded yet")}</p>
          )}
          {list.length > 0 && (
            <ul className="mt-1">
              {list.map((event) => (
                <LimitEventRow key={event.id} event={event} accountName={nameOf(event.accountId)} />
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}
