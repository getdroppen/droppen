/*
 * The connection log, on screen (`lib/connection-log.ts` holds it): what this
 * browser's way to its devices did — for the owner to read, and to copy or
 * download and send for analysis.
 *
 * One dialog per page (`ConnectionLogHost`, in the shell's view), opened by
 * {@link openConnectionLog}: from the bar's strip (`ConnectionLogButton`, from
 * 640 px up, where the strip has room), from the path indicator's card while
 * the page is live, and from 「这台设备」 on 「我的设备」, at any width.
 *
 * The list is live: what is logged while it is open shows within half a
 * second, and it stays at the newest line while it is scrolled to the end.
 * Three views: everything, everything but the samples and probes (where it
 * opens), and the problems alone (warnings and errors: closes, timeouts,
 * failures). Rows are keyed by their event's seq, so a refresh draws only
 * what is new. Copy puts the view on the clipboard as compact text with a
 * header (the web build, the device's agent, the browser, the network, how
 * the page is connected now and what the export leaves out), the newest of
 * it up to `COPY_MAX_CHARS`, under what an issue or a chat takes; Download
 * saves the whole log; Clear forgets it. Nothing in it names a device, a
 * session or a file (`connection-log.ts`, PRIVACY).
 */
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { PulseIcon } from "@phosphor-icons/react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Tabs, TabsList, TabsTab } from "@/components/ui/tabs";
import { cn } from "@/lib/cn";
import { useCopy } from "@/lib/clipboard";
import { buildId, type ConnectionLog, type ConnEvent, type ConnValue, connLog, maskId, networkFields } from "@/lib/connection-log";
import { clockOf, type ConnFilter, COPY_MAX_CHARS, eventLine, fieldsText, filterEvents, type LogHeader, logFileName, logText, sinceLoad } from "./connection-log-format";
import { routeName } from "@/lib/hub/machine";
import { createStore, useStore } from "@/lib/machines/store";
import { hubOnly } from "@/lib/transport/p2p";
import { hubShell } from "@/features/machines/shells";

/** Whether the page's connection log is open. */
const logOpen = createStore(false);

/** Open the connection log (the dialog `ConnectionLogHost` draws). */
export function openConnectionLog(): void {
  logOpen.set(true);
}

/** The page's one connection log dialog, opened by {@link openConnectionLog}. */
export function ConnectionLogHost() {
  const open = useStore(logOpen);
  return <ConnectionLogDialog open={open} onOpenChange={(next) => logOpen.set(next)} />;
}

/** How the page is connected now, for the export's header: the hub link, the device in view's channel and paths, the relays. */
export function connectionNow(): Record<string, ConnValue> | null {
  const shell = hubShell();
  if (!shell) return null;
  const focus = shell.focus.get();
  const machine = shell.machine(focus);
  const channel = machine?.channel ?? null;
  const paths = channel?.paths ?? null;
  const relays = shell.relays;
  const rtt = (id: string) => {
    const ms = relays.rtt(id);
    return ms === null ? "-" : String(Math.round(ms));
  };
  return {
    hub: shell.link.state,
    hubRetryInMs: shell.link.retryAt === null ? null : Math.max(0, shell.link.retryAt - Date.now()),
    device: maskId(focus),
    channel: channel?.status ?? null,
    mode: machine?.transport.current ?? null,
    via: channel?.route ? routeName(channel.route) : null,
    relayRttMs: paths?.relayRttMs ?? null,
    direct: channel?.directPaths?.state ?? null,
    directRttMs: paths?.direct ? paths.directRttMs : null,
    retryInMs: machine?.controller.retryAt == null ? null : Math.max(0, machine.controller.retryAt - Date.now()),
    hubOnly: hubOnly(),
    relays: (relays.directory?.relays ?? []).map((r) => `${r.id}=${rtt(r.id)}`).join(" ") || null,
  };
}

/** The release the device in view's agent says it runs (its status, as the copy or the device reports it), if it said one. */
export function agentVersionNow(): string | null {
  const shell = hubShell();
  const version = shell?.machine(shell.focus.get())?.info.get().agentVersion ?? null;
  return version && /^[\w.+-]{1,40}$/.test(version) ? version : null;
}

/** The export's header as things stand now: every field of it held to the log's rules (`ConnectionLog.clean`). */
export function headerNow(
  log: ConnectionLog,
  filter: ConnFilter,
  connection: () => Record<string, ConnValue> | null = connectionNow,
  agentVersion: () => string | null = agentVersionNow,
): LogHeader {
  const now = connection();
  return {
    build: buildId(),
    agentVersion: agentVersion(),
    userAgent: typeof navigator === "undefined" ? "" : navigator.userAgent,
    loadedAt: log.loadedAt,
    exportedAt: Date.now(),
    network: log.clean(networkFields()),
    connection: now ? log.clean(now) : null,
    filter,
    total: log.size,
  };
}

/** How often an open list takes in what was logged meanwhile. */
const REFRESH_MS = 500;
/** Rows drawn at first, and more each time "earlier" is asked for (Copy and Download hold everything). */
const PAGE_ROWS = 500;

/** The log's events, taken again at most every {@link REFRESH_MS} while something is logged. */
function useLogEvents(log: ConnectionLog): readonly ConnEvent[] {
  const [events, setEvents] = useState<readonly ConnEvent[]>(() => log.events());
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    setEvents(log.events());
    const off = log.subscribe(() => {
      if (timer !== undefined) return;
      timer = setTimeout(() => {
        timer = undefined;
        setEvents(log.events());
      }, REFRESH_MS);
    });
    return () => {
      off();
      clearTimeout(timer);
    };
  }, [log]);
  return events;
}

/** `<a download>`: the browser's way to hand over a file. */
function saveText(text: string, name: string): void {
  const url = URL.createObjectURL(new Blob([text], { type: "text/plain;charset=utf-8" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}

export function ConnectionLogDialog({
  open,
  onOpenChange,
  log = connLog,
  connection = connectionNow,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  log?: ConnectionLog;
  connection?: () => Record<string, ConnValue> | null;
}) {
  const { t } = useTranslation();
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {open && (
        <DialogContent
          title={t("Connection log")}
          closeLabel={t("close")}
          className="flex h-[min(40rem,calc(100dvh-2rem))] w-[min(56rem,calc(100vw-2rem))] flex-col overflow-hidden"
        >
          <ConnectionLogView log={log} connection={connection} />
        </DialogContent>
      )}
    </Dialog>
  );
}

const FILTERS: readonly ConnFilter[] = ["all", "events", "problems"];

/** The log's toolbar, note and list (the dialog's body). */
export function ConnectionLogView({ log = connLog, connection = connectionNow }: { log?: ConnectionLog; connection?: () => Record<string, ConnValue> | null }) {
  const { t } = useTranslation();
  const events = useLogEvents(log);
  // Events, not the every-second samples and probes: what a copy is for, and what fits in an issue.
  const [filter, setFilter] = useState<ConnFilter>("events");
  const [rows, setRows] = useState(PAGE_ROWS);
  const shown = useMemo(() => filterEvents(events, filter), [events, filter]);
  const drawn = shown.length > rows ? shown.slice(shown.length - rows) : shown;
  const withPrevious = drawn.some((e) => e.prev !== undefined);
  const { copied, copy } = useCopy();
  const list = useRef<HTMLOListElement>(null);
  // At the newest line (scrolled to the end): stays there as lines come.
  const atEnd = useRef(true);
  useLayoutEffect(() => {
    const el = list.current;
    if (el && atEnd.current) el.scrollTop = el.scrollHeight;
  }, [drawn]);
  const filterName: Record<ConnFilter, string> = { all: t("all"), events: t("events"), problems: t("problems") };
  return (
    <div data-testid="connection-log" className="flex min-h-0 flex-1 flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <Tabs value={filter} onValueChange={(value) => setFilter(value as ConnFilter)}>
          <TabsList aria-label={t("which events")}>
            {FILTERS.map((f) => (
              <TabsTab key={f} value={f} data-testid={`connection-log-filter-${f}`}>
                {filterName[f]}
              </TabsTab>
            ))}
          </TabsList>
        </Tabs>
        <span data-testid="connection-log-count" className="font-mono text-2xs text-muted tabular-nums">
          {t("{{shown}} of {{total}} events", { shown: shown.length, total: events.length })}
        </span>
        <span className="flex-1" />
        <Button data-testid="connection-log-copy" onClick={() => copy(logText(shown, headerNow(log, filter, connection), COPY_MAX_CHARS))}>
          {copied ? t("copied") : t("copy")}
        </Button>
        <Button data-testid="connection-log-download" onClick={() => saveText(logText(log.events(), headerNow(log, "all", connection)), logFileName(Date.now()))}>
          {t("download")}
        </Button>
        <Button data-testid="connection-log-clear" variant="ghost" onClick={() => log.clear()}>
          {t("clear")}
        </Button>
      </div>
      <p className="text-xs leading-5 text-muted">
        {t(
          "What this browser's connections to your devices did, kept in this tab for the last 30 minutes. Device ids are hashed, and text that is not Droppen's own is withheld: no tokens, keys, messages, names, paths or addresses are kept. Copy the newest (up to 60,000 characters) or download all of it to send for analysis.",
        )}
      </p>
      <ol
        ref={list}
        data-testid="connection-log-list"
        onScroll={(e) => {
          const el = e.currentTarget;
          atEnd.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
        }}
        className="min-h-0 flex-1 overflow-y-auto border-t border-border pt-1 font-mono text-2xs leading-4"
      >
        {shown.length > drawn.length && (
          <li>
            <button type="button" className="py-1 text-muted underline-offset-2 hover:text-fg hover:underline" onClick={() => setRows((n) => n + PAGE_ROWS)}>
              {t("show {{n}} earlier", { n: Math.min(PAGE_ROWS, shown.length - drawn.length) })}
            </button>
          </li>
        )}
        {drawn.length === 0 && <li className="py-2 font-sans text-xs text-muted">{t("Nothing logged yet.")}</li>}
        {drawn.map((e, i) => {
          // With the page before a reload in the log, each page's lines start under a line of their own.
          const page = e.prev ?? null;
          const starts = withPrevious && (i === 0 || (drawn[i - 1]!.prev ?? null) !== page);
          return <LogRow key={e.seq} event={e} pageStart={starts ? page : undefined} />;
        })}
      </ol>
    </div>
  );
}

/** One event: its time (UTC) and seconds since its page loaded, its name, its fields; a page before a reload under a line of its own. */
function LogRow({ event, pageStart }: { event: ConnEvent; pageStart: number | null | undefined }) {
  const { t } = useTranslation();
  const tone = event.level === "error" ? "text-danger" : event.level === "warn" ? "text-warn-fg" : "text-fg";
  return (
    <>
      {pageStart !== undefined && (
        <li data-testid="connection-log-page" className="mt-1 border-t border-border pt-1 text-faint">
          {pageStart === null ? t("this page") : t("the page before a reload")}
        </li>
      )}
      <li data-event={event.name} data-level={event.level} title={eventLine(event)} className="break-words py-px hover:bg-hover">
        <span className="text-faint tabular-nums">{clockOf(event.wall)}</span> <span className="text-faint tabular-nums max-sm:hidden">{sinceLoad(event.t)}</span>{" "}
        <span data-signal={event.level === "error" ? "failed" : event.level === "warn" ? "warn" : undefined} className={cn(tone, event.level !== "info" && "font-semibold")}>
          {event.name}
        </span>{" "}
        <span className="text-muted">{fieldsText(event.fields)}</span>
      </li>
    </>
  );
}

/** The bar's way to the log: a 20px icon, from 640 px up (below, the strip has no room: the path card and 「这台设备」 have it). */
export function ConnectionLogButton() {
  const { t } = useTranslation();
  return (
    <button
      type="button"
      data-testid="connection-log-open"
      onClick={openConnectionLog}
      className="flex h-5 items-center px-1.5 text-muted hover:bg-hover hover:text-fg max-sm:hidden"
      title={t("connection log")}
      aria-label={t("connection log")}
    >
      <PulseIcon className="size-3" />
    </button>
  );
}

/** What the bar says, in the log (`bar`): each change of its words, its step, or when it tries again. */
export function useBarLog(device: string, state: { label: string; retryAt: number | null; notice: boolean; cover: boolean }, step: { kind: string; city?: string | null } | null): void {
  const city = step?.kind === "relay" ? (step.city ?? null) : null;
  useEffect(() => {
    connLog.emit("bar", {
      device: maskId(device),
      label: state.label,
      step: step?.kind ?? null,
      city,
      retryInMs: state.retryAt === null ? null : Math.max(0, state.retryAt - Date.now()),
      notice: state.notice,
      cover: state.cover,
    });
  }, [device, state.label, state.retryAt, state.notice, state.cover, step?.kind, city]);
}
