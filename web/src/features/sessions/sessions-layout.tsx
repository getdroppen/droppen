import { useMutation, useQuery } from "@tanstack/react-query";
import { SourceIcon } from "@/components/source-icon";
import { Outlet, useParams } from "@tanstack/react-router";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ChevronRight, Folder, Pin, Plus, X } from "lucide-react";
import { memo, useCallback, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ApiError, errorText } from "@/api/errors";
import type { components } from "@/api/schema";
import { CapsLabel } from "@/components/ui/caps-label";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { LiveDot } from "@/components/live-dot";
import { Content, Panel, PanelHeader } from "@/components/panel";
import { useFeedConnected, useFeedSessions, useFeedSettings, useMachineFeedSelector, type GlobalFeedState } from "@/app/global-feed";
import { LinkFirstRun } from "@/features/link/first-run";
import { useNewSessionShortcut, useOpenNewSession } from "@/features/sessions/new-session-open";
import { RenameSessionDialog, type RenameTarget } from "@/features/sessions/rename-dialog";
import { useSessionPin } from "@/features/sessions/session-pin";
import { ActivityChipView, EdgeBar, EDGE_CLASS, UnreadPill } from "@/features/sessions/attention-badges";
import {
  attentionState,
  groupAttention,
  unreadCountOf,
  type AttentionRow,
} from "@/features/sessions/session-attention";
import {
  buildDeviceList,
  buildSessionList,
  matchesQuery,
  NO_PROJECT,
  rowKey,
  type SessionListRow,
} from "@/features/sessions/session-list-model";
import { copyNote, type DeviceChoice, DeviceFilterBar, deviceLabel, loadDeviceChoice, saveDeviceChoice } from "@/features/sessions/device-filter";
import { gatewayShell, gatewayShellStore } from "@/features/machines/shells";
import { useDeviceSessions, useShellDevices } from "@/features/machines/device-sessions";
import { type DeviceSessions, needsYou } from "@/features/machines/needs-you";
import { NeedsYouLedger } from "@/features/machines/needs-you-ledger";
import { basename, formatRelative } from "@/lib/format";
import { useTicker } from "@/lib/ticker";
import { cn } from "@/lib/cn";
import { useApi, useHomeMachine, useMachine } from "@/lib/machines/context";
import { LOCAL, type MachineKey } from "@/lib/machines/key";
import type { MachineInfo } from "@/lib/machines/runtime";
import { useStore } from "@/lib/machines/store";
import { mq } from "@/lib/machines/query";
import { readMachineItem, writeMachineItem } from "@/lib/machines/storage";
import { MachineLink, MachineView, useMachineNavigate, useSectionPath } from "@/app/machine-route";
import { SetupBars } from "@/features/setup/bars";

type Source = components["schemas"]["Source"];
type SessionSummary = components["schemas"]["SessionSummary"];

/**
 * A relative timestamp that keeps up with the clock.
 *
 * Its own component, and on its own timer, because the row around it is
 * memoized on purpose: the feed only replaces the summaries that changed, and
 * re-rendering every row (each with a context-menu root) twice a minute to move
 * one word would give that back. This span is the only thing that repaints.
 *
 * Without it the column froze at whatever it said when the feed last pushed, so
 * a quiet instance showed a list of `2m` an hour after the fact.
 */
function RelativeTime({ iso }: { iso: string | null }) {
  useTicker(30_000);
  return <>{formatRelative(iso)}</>;
}

/**
 * What a session is doing and how much of it you have not read, side by side.
 *
 * Two independent facts, so both can show at once: a working session can be
 * holding three answers you have not seen, and an idle one can be holding
 * some. `unread` is the count to draw; the detail header passes none, because
 * the session it sits on is the one being read.
 */
export function StatusBadge({ session, unread = 0 }: { session: AttentionRow; unread?: number }) {
  return (
    <>
      <ActivityChipView row={session} />
      <UnreadPill count={unread} />
    </>
  );
}

// Memoized: the feed only replaces the summaries that actually changed, so
// unchanged rows (with their context-menu roots) skip re-rendering entirely.
// Every row here is one this server owns, so nothing in it has to hedge about
// scope: external sessions are not listed at all (see `buildSessionList`).
const SessionRow = memo(function SessionRow({
  session,
  dirLabel,
  nested,
  onArchive,
  onRename,
  onPin,
  pinPending,
  machine,
  device = null,
  readOnly = false,
}: {
  session: SessionSummary;
  /** Basename when flat, the row's own cwd under a repository heading. */
  dirLabel: string | null;
  /** Only ordinary rows inside a directory; pins stay at the top level. */
  nested: boolean;
  onArchive: (session: SessionSummary) => void;
  onRename: (session: SessionSummary) => void;
  onPin: (session: SessionSummary) => void;
  pinPending: boolean;
  /** The device it is on (absent: the list's own). */
  machine?: MachineKey;
  /** Its device's name, in a list of several devices' (absent: this device's, or a list of one). */
  device?: string | null;
  /** Another device's, shown from its copy: nothing can be changed from here. */
  readOnly?: boolean;
}) {
  const { t } = useTranslation();
  return (
    <ContextMenu>
      <ContextMenuTrigger
        render={
          <MachineLink
            to="/sessions/$source/$id"
            machine={machine}
            params={{ source: session.source, id: session.id }}
            className={cn(
              "relative block border-b border-border py-2 pr-2.5 pl-3 hover:bg-hover data-[popup-open]:bg-hover data-[popup-open]:ring-1 data-[popup-open]:ring-fg/40 data-[popup-open]:ring-inset",
              nested && "pl-8 before:pointer-events-none before:absolute before:inset-y-0 before:left-4 before:w-px before:bg-border",
            )}
            activeProps={{ className: "bg-hover opacity-100" }}
          >
            {/* The strongest state, on the edge: a long list can be scanned for
                colour before a single word of it is read. */}
            <EdgeBar state={attentionState(session)} />
            <div className="flex items-center gap-1.5">
              {session.pinned && <span title={t("pinned session")} aria-label={t("pinned session")}><Pin className="size-3 shrink-0 text-fg" /></span>}
              <span className={cn("min-w-0 flex-1 truncate text-[13px] leading-4.5 text-fg max-md:text-sm max-md:leading-5", unreadCountOf(session) > 0 && "font-semibold")}>{session.title}</span>
              <StatusBadge session={session} unread={unreadCountOf(session)} />
            </div>
            <div className="mt-0.5 flex items-center gap-1.5">
              {device && (
                <span data-row-device={machine} className="max-w-[45%] shrink-0 truncate font-mono text-2xs text-muted">
                  {device}
                </span>
              )}
              <SourceIcon source={session.source} className="text-muted" />
              {dirLabel && <span className="truncate font-mono text-2xs text-faint">{dirLabel}</span>}
              <span className="ml-auto shrink-0 font-mono text-2xs text-faint">
                <RelativeTime iso={session.updatedAt} />
              </span>
            </div>
          </MachineLink>
        }
      />
      <ContextMenuContent>
        {!readOnly && <ContextMenuItem disabled={pinPending} onClick={() => onPin(session)}>{t(session.pinned ? "unpin session" : "pin session")}</ContextMenuItem>}
        {!readOnly && <ContextMenuItem onClick={() => onRename(session)}>{t("rename")}</ContextMenuItem>}
        <ContextMenuItem onClick={() => void navigator.clipboard.writeText(session.id)}>
          {t("copy session id")}
        </ContextMenuItem>
        {!readOnly && (
          <ContextMenuItem danger onClick={() => onArchive(session)}>
            {t("archive session")}
          </ContextMenuItem>
        )}
      </ContextMenuContent>
    </ContextMenu>
  );
});

/** The folded directories of a machine's list (`rh.sessions.collapsedDirs` on the local machine, as it always was). */
const COLLAPSED_DIRS = "sessions.collapsedDirs";

/**
 * Other devices' folded projects, in a list of several devices' (S1-7): for
 * this page only — their paths are not written down on this device.
 */
const remoteCollapsed = new Set<string>();

function loadCollapsedDirs(machine: MachineKey): Set<string> {
  const out = new Set(remoteCollapsed);
  try {
    const raw = readMachineItem(localStorage, machine, COLLAPSED_DIRS);
    if (raw) for (const key of JSON.parse(raw) as string[]) out.add(key);
  } catch {
    // corrupted storage: start expanded
  }
  return out;
}

const GroupHeader = memo(function GroupHeader({
  dir,
  collapseKey,
  list,
  isOpen,
  onToggle,
  onArchiveFolder,
  onNewSessionHere,
  device = null,
}: {
  dir: string;
  /** Still `owned:`-prefixed, so folds saved before the split went away survive. */
  collapseKey: string;
  list: SessionSummary[];
  isOpen: boolean;
  /** `remote`: another device's project (its fold is not written down). */
  onToggle: (collapseKey: string, remote: boolean) => void;
  onArchiveFolder: (dir: string, sessions: SessionSummary[]) => void;
  onNewSessionHere: (dir: string) => void;
  /** Another device's project, in a list of several devices': its name (nothing can be done to it from here). */
  device?: string | null;
}) {
  const { t } = useTranslation();
  const realDir = dir !== NO_PROJECT;
  // Always shown, open or folded: the heading is where a long list is read
  // first, and a folded one is otherwise the only place this survives at all.
  const inside = groupAttention(list);
  return (
    <ContextMenu>
      <ContextMenuTrigger
        render={
          <button
            type="button"
            onClick={() => onToggle(collapseKey, !!device)}
            aria-expanded={isOpen}
            className="relative flex h-6.5 w-full cursor-pointer items-center gap-1.5 border-b border-border bg-bg pr-2 pl-3 text-left hover:bg-hover data-[popup-open]:bg-hover data-[popup-open]:ring-1 data-[popup-open]:ring-fg/40 data-[popup-open]:ring-inset"
          >
            {/* The heading's own edge: the strongest state inside while folded;
                open, its rows speak for themselves and the heading stays quiet. */}
            {!isOpen && EDGE_CLASS[inside.strongest] && (
              <span
                aria-hidden="true"
                data-edge={inside.strongest}
                className={cn("pointer-events-none absolute inset-y-0 left-0 w-[2px]", EDGE_CLASS[inside.strongest])}
              />
            )}
            <ChevronRight
              className={`size-3 shrink-0 text-muted transition-transform ${isOpen ? "rotate-90" : ""}`}
            />
            <Folder className="size-3.5 shrink-0 text-muted" />
            <span data-label="" className="label-caps truncate text-2xs text-fg" title={dir}>
              {realDir ? basename(dir) : dir}
            </span>
            {device && <span data-group-device="" className="max-w-[40%] shrink-0 truncate font-mono text-2xs text-muted">{device}</span>}
            <span className="ml-auto flex shrink-0 items-center gap-1.5">
              {inside.asking > 0 && (
                <span
                  data-activity="asking" data-signal="warn" className="inline-flex h-4 items-center bg-warn px-1 font-mono text-[10px] leading-none font-semibold tabular-nums text-on-signal"
                  title={t("{{n}} asking you", { n: inside.asking })}
                >
                  {inside.asking}
                </span>
              )}
              {inside.working > 0 && (
                <span
                  data-activity="working" data-signal="ok" className="inline-flex h-4 items-center gap-1 px-1 font-mono text-[10px] leading-none tabular-nums text-ok shadow-[inset_0_0_0_1px_color-mix(in_srgb,var(--color-ok)_60%,transparent)]"
                  title={t("{{n}} working", { n: inside.working })}
                >
                  <LiveDot className="bg-ok" />
                  {inside.working}
                </span>
              )}
              <UnreadPill count={inside.unread} />
              <span className="min-w-4 px-1 text-center font-mono text-2xs tabular-nums text-faint">{list.length}</span>
            </span>
          </button>
        }
      />
      <ContextMenuContent>
        {realDir && !device && <ContextMenuItem onClick={() => onNewSessionHere(dir)}>{t("new session here")}</ContextMenuItem>}
        {!device && (
          <ContextMenuItem danger onClick={() => onArchiveFolder(dir, list)}>
            {t("archive all ({{n}})", { n: list.length })}
          </ContextMenuItem>
        )}
        {device && <ContextMenuItem disabled>{t("read-only here: this session is on another device")}</ContextMenuItem>}
      </ContextMenuContent>
    </ContextMenu>
  );
});

const selectHomeRows = (state: GlobalFeedState) => state.sessions;

/**
 * Whether the list is several devices' as one (the local app whose gateway
 * shows other devices, S1-7), and those devices — this one first. On
 * droppen.app every device keeps its own list (its pages, `/m/<device>/…`).
 */
export function useUnifiedDevices(): { unified: boolean; devices: readonly MachineInfo[] } {
  const devices = useShellDevices();
  const gateway = useStore(gatewayShellStore());
  return { unified: gateway !== null && devices.length > 1, devices };
}

export function SessionsLayout() {
  const machine = useMachine();
  const home = useHomeMachine();
  const { unified, devices } = useUnifiedDevices();
  // On phones only one column fits: list, or the open session (the home's or another machine's route).
  const { source: openSource, id: openId } = useParams({ strict: false }) as { source?: string; id?: string };
  // A session about to be started is a session view too.
  const starting = useSectionPath() === "/sessions/new";
  const detailOpen = (!!openSource && !!openId) || starting;
  // Several devices' list is the home's — its prefs, its folds, its feed —
  // whichever device's session is open beside it.
  return (
    <>
      <MachineView machine={unified ? home : machine}>
        <SessionsPanel detailOpen={detailOpen} unified={unified} devices={devices} />
      </MachineView>
      <Content className={detailOpen ? "" : "max-md:hidden"}>
        <Outlet />
      </Content>
    </>
  );
}

function SessionsPanel({ detailOpen, unified, devices }: { detailOpen: boolean; unified: boolean; devices: readonly MachineInfo[] }) {
  const api = useApi();
  const machine = useMachine();
  const { t, i18n } = useTranslation();
  const [q, setQ] = useState("");
  const navigate = useMachineNavigate();

  // Failures used to go to `window.alert`, a modal the user has to dismiss
  // before they can look at anything — including the row that failed. A strip
  // under the header says the same thing without taking the app away.
  const [notice, setNotice] = useState<string | null>(null);
  const pin = useSessionPin((err) => setNotice(t("pin failed: {{msg}}", { msg: errorText(err) })));

  const archive = useMutation({
    mutationFn: async (s: SessionSummary) => {
      const res = await api.POST("/api/sessions/{source}/{id}/archive", {
        params: { path: { source: s.source, id: s.id } },
      });
      if (res.error) throw new ApiError(res.error);
      return s;
    },
    onSuccess: (s) => {
      // The global feed pushes the list change; just leave a dead detail view.
      if (location.pathname.includes(s.id)) void navigate({ to: "/sessions" });
    },
    onError: (err: Error) => setNotice(t("archive failed: {{msg}}", { msg: errorText(err) })),
  });

  const [renameTarget, setRenameTarget] = useState<RenameTarget | null>(null);
  // Stable so memoized rows don't re-render on every keystroke in the filter.
  const openRename = useCallback(
    (session: SessionSummary) =>
      setRenameTarget({ source: session.source, id: session.id, title: session.title }),
    [],
  );


  const archiveFolder = async (dir: string, list: SessionSummary[]) => {
    if (!window.confirm(`Archive all ${list.length} sessions under "${basename(dir)}"?`)) return;
    const results = await Promise.allSettled(
      list.map((s) =>
        api
          .POST("/api/sessions/{source}/{id}/archive", {
            params: { path: { source: s.source, id: s.id } },
          })
          .then((res) => {
            if (res.error) throw new Error(res.error.error);
          }),
      ),
    );
    const failed = results.filter((r) => r.status === "rejected").length;
    if (failed) {
      setNotice(t("{{failed}}/{{total}} sessions failed to archive", { failed, total: list.length }));
    }
  };

  // On this list's device: the empty session view, in the content column.
  const startNewSession = useOpenNewSession();
  const newSessionKeys = useNewSessionShortcut();
  const openNewSession = (cwd: string | null) => startNewSession({ machine, ...(cwd ? { cwd } : {}) });

  const { data: prefs } = useQuery({
    queryKey: mq(machine, "prefs"),
    queryFn: async () => (await api.GET("/api/prefs")).data,
    // The settings page writes through this cache; a refetch per mounted view
    // was a request on every session switch for a value that had not moved.
    staleTime: 5 * 60_000,
  });
  const grouped = prefs?.sessionGrouping === "directory";

  // Folding state lives here so the flattened row model can honor it. The keys
  // are the model's to shape; this only stores what it is handed.
  const [collapsedDirs, setCollapsedDirs] = useState<Set<string>>(() => loadCollapsedDirs(machine));
  const toggleDir = useCallback((key: string, remote: boolean) => {
    setCollapsedDirs((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      if (remote) {
        // Another device's: kept for this page only.
        if (next.has(key)) remoteCollapsed.add(key);
        else remoteCollapsed.delete(key);
        return next;
      }
      try {
        writeMachineItem(localStorage, machine, COLLAPSED_DIRS, JSON.stringify([...next].filter((k) => !remoteCollapsed.has(k))));
      } catch {
        // storage full/blocked: state still works for this session
      }
      return next;
    });
  }, [machine]);

  // The global feed streams both scopes — that is the server's one-stream
  // design, not something this view asks for — and the model drops the external
  // half. The detail route still wants those rows.
  const allSessions = useFeedSessions();
  const connected = useFeedConnected();
  const settings = useFeedSettings();
  const homeDir = settings?.homeDir ?? "";

  // Several devices (S1-7): this one's rows from its feed, the others' from
  // their copies; what needs you across all of them; which to show.
  const homeRows = useMachineFeedSelector(machine, selectHomeRows);
  const live = useMemo<DeviceSessions>(() => ({ machine, rows: homeRows, from: "live", asOf: null }), [machine, homeRows]);
  const lists = useDeviceSessions(unified ? live : null, unified);
  const needs = useMemo(() => needsYou(unified ? lists : []), [unified, lists]);
  const [choice, setChoiceState] = useState<DeviceChoice>(loadDeviceChoice);
  const setChoice = useCallback((next: DeviceChoice) => {
    setChoiceState(next);
    saveDeviceChoice(next);
  }, []);
  // A choice that is not there (any more) shows every device.
  const shown: DeviceChoice =
    choice === "needs" ? (needs.entries.length > 0 ? "needs" : "all") : choice === "all" || devices.some((d) => d.key === choice) ? choice : "all";
  const infoOf = useMemo(() => new Map(devices.map((d) => [d.key, d])), [devices]);
  const nameOf = useCallback((key: MachineKey) => (infoOf.has(key) ? deviceLabel(infoOf.get(key)!, t) : key), [infoOf, t]);
  const singleModel = useMemo(
    () =>
      unified || allSessions === null
        ? null
        : buildSessionList({
            sessions: allSessions,
            source: "all",
            query: q,
            grouped,
            collapsedDirs,
            homeDir,
          }),
    [unified, allSessions, q, grouped, collapsedDirs, homeDir],
  );
  const deviceModel = useMemo(() => {
    if (!unified) return null;
    if (shown === "needs") {
      const needle = q.trim().toLowerCase();
      const rows: SessionListRow[] = needs.entries
        .filter((e) => matchesQuery(e.row, needle))
        .map((e) => ({ type: "session", session: e.row, dirLabel: e.row.cwd ? basename(e.row.cwd) : null, machine: e.ref.machine }));
      return { rows, ownedShown: rows.length, ownedTotal: rows.length };
    }
    const homeDirOf = (key: MachineKey) =>
      key === machine ? homeDir : (gatewayShell()?.machine(key)?.mirror.status?.state?.settings?.homeDir as string | undefined) ?? "";
    return buildDeviceList({
      lists: lists.map((l) => ({ machine: l.machine, rows: l.rows, homeDir: homeDirOf(l.machine) })),
      home: machine,
      device: shown,
      query: q,
      grouped,
      collapsedDirs,
    });
  }, [unified, shown, needs, lists, q, grouped, collapsedDirs, machine, homeDir]);
  const model = unified ? deviceModel : singleModel;
  const shownInfo = shown !== "all" && shown !== "needs" && shown !== LOCAL ? infoOf.get(shown) : undefined;
  const isPending = model === null && connected;
  const isError = model === null && !connected;
  const rows: SessionListRow[] = model?.rows ?? [];

  // Virtualized: only ~25 rows (each carrying a context-menu root) are ever
  // mounted, so tab switches and large lists stay instant.
  const listRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => listRef.current,
    estimateSize: (i) => (rows[i]?.type === "session" ? 45 : 30),
    overscan: 12,
    getItemKey: (i) => rowKey(rows[i]!, machine),
  });

  return (
    <>
      <Panel collapseKey="sessions" className={detailOpen ? "max-md:hidden" : ""}>
        <PanelHeader
          title={t("Sessions")}
          extra={
            <span className="flex items-center gap-1.5">
              <span className="font-mono text-2xs text-faint">
                {model ? `${model.ownedShown}/${model.ownedTotal}` : "…"}
              </span>
              <button
                type="button"
                title={`${t("new session")} (${newSessionKeys})`}
                aria-label={t("new session")}
                onClick={() => openNewSession(null)}
                className="flex size-5 items-center justify-center text-muted hover:bg-hover hover:text-fg"
              >
                <Plus className="size-3.5" />
              </button>
            </span>
          }
        />
        {notice && (
          <div data-signal="failed" className="flex shrink-0 items-start gap-1.5 border-b border-border px-2.5 py-1 font-mono text-2xs text-danger shadow-[inset_2px_0_0_var(--color-danger)]">
            <span className="min-w-0 flex-1 break-words">{notice}</span>
            <button
              type="button"
              aria-label={t("dismiss")}
              onClick={() => setNotice(null)} data-signal="failed" className="flex size-4 shrink-0 items-center justify-center hover:bg-danger/15"
            >
              <X className="size-3" />
            </button>
          </div>
        )}
        {/* The Droppen app's first-run guide: unfinished, or never started with no AI tool ready. */}
        <SetupBars />
        {/* On a phone the list is the page: the first-run card leads it (local page only). */}
        <LinkFirstRun className="m-2 mb-0 md:hidden" />
        <div className="flex shrink-0 flex-col gap-1.5 border-b border-border p-2">
          <Input placeholder={t("filter title / path…")} value={q} onChange={(e) => setQ(e.target.value)} />
          {unified && <DeviceFilterBar value={shown} onChange={setChoice} devices={devices} needs={needs.entries.length} />}
          {shownInfo && (
            <p data-testid="device-filter-note" className="flex min-w-0 items-center gap-1.5 font-mono text-2xs text-faint">
              {copyNote(shownInfo, t, i18n.language) && (
                <span data-signal={shownInfo.state === "offline" ? "offline" : undefined} className={shownInfo.state === "offline" ? "shrink-0 text-danger" : "shrink-0"}>
                  {copyNote(shownInfo, t, i18n.language)}
                </span>
              )}
              <span className="truncate">{t("read-only here: these sessions are on another device")}</span>
            </p>
          )}
        </div>
        <div ref={listRef} className="min-h-0 flex-1 overflow-y-auto">
          {isPending && <Spinner />}
          {isError && <div data-signal="failed" className="p-3 text-2xs text-danger">{t("failed to load sessions")}</div>}
          {model && rows.length > 0 && (
            <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
              {virtualizer.getVirtualItems().map((vi) => {
                const row = rows[vi.index]!;
                return (
                  <div
                    key={vi.key}
                    data-index={vi.index}
                    ref={virtualizer.measureElement}
                    style={{
                      position: "absolute",
                      top: 0,
                      left: 0,
                      width: "100%",
                      transform: `translateY(${vi.start}px)`,
                    }}
                  >
                    {row.type === "header" ? (
                      <GroupHeader
                        dir={row.dir}
                        collapseKey={row.collapseKey}
                        list={row.list}
                        isOpen={row.isOpen}
                        onToggle={toggleDir}
                        onArchiveFolder={(dir, list) => void archiveFolder(dir, list)}
                        onNewSessionHere={openNewSession}
                        device={row.machine && row.machine !== machine ? nameOf(row.machine) : null}
                      />
                    ) : (
                      <SessionRow
                        session={row.session}
                        dirLabel={row.dirLabel}
                        nested={grouped && shown !== "needs" && !row.session.pinned}
                        onArchive={archive.mutate}
                        onRename={openRename}
                        onPin={pin.mutate}
                        pinPending={pin.isPending}
                        machine={row.machine}
                        device={unified && row.machine && row.machine !== machine ? nameOf(row.machine) : null}
                        // Only this list's own device's rows can be changed from here
                        // (pin, rename, archive go to its server, `api`): any other
                        // device's row is read-only, whatever its runtime says.
                        readOnly={!!row.machine && row.machine !== machine}
                      />
                    )}
                  </div>
                );
              })}
            </div>
          )}
          {model && rows.length === 0 && (
            <div className="p-3 text-2xs text-faint">{t("no sessions match")}</div>
          )}
        </div>
      </Panel>
      <RenameSessionDialog target={renameTarget} onOpenChange={(o) => !o && setRenameTarget(null)} />
    </>
  );
}

/**
 * 「需要你 · 所有设备」 where no session is open yet, when the list holds
 * several devices (S1-7): asking first, then unread, then working — one row
 * opens that session on its device.
 */
function DevicesLedger({ devices }: { devices: readonly MachineInfo[] }) {
  const { t } = useTranslation();
  const home = useHomeMachine();
  const navigate = useMachineNavigate();
  const rows = useMachineFeedSelector(home, selectHomeRows);
  const live = useMemo<DeviceSessions>(() => ({ machine: home, rows, from: "live", asOf: null }), [home, rows]);
  const lists = useDeviceSessions(live);
  const needs = useMemo(() => needsYou(lists), [lists]);
  const names = useMemo(() => new Map(devices.map((d) => [d.key, deviceLabel(d, t)])), [devices, t]);
  return (
    <NeedsYouLedger
      needs={needs}
      nameOf={(key) => names.get(key) ?? key}
      readOnly={(key) => key !== home}
      onOpen={(entry) => void navigate({ to: "/sessions/$source/$id", machine: entry.ref.machine, params: { source: entry.ref.source, id: entry.ref.id } })}
      className="w-full max-w-4xl"
    />
  );
}

export function SessionsIndex() {
  const { t } = useTranslation();
  const { unified, devices } = useUnifiedDevices();
  if (unified) {
    return (
      <div data-testid="sessions-index-ledger" className="flex min-h-0 flex-1 flex-col items-center overflow-y-auto">
        <DevicesLedger devices={devices} />
        <div className="flex max-w-xs flex-col gap-2 p-6">
          <CapsLabel>{t("select a session")}</CapsLabel>
          <p className="text-[28px] leading-[1.05] font-semibold tracking-[-0.03em] text-fg">{t("pick one on the left, or start a new one.")}</p>
        </div>
      </div>
    );
  }
  return (
    <div className="flex flex-1 items-center justify-center p-6">
      <div className="flex max-w-xs flex-col gap-2">
        <CapsLabel>{t("select a session")}</CapsLabel>
        <p className="text-[28px] leading-[1.05] font-semibold tracking-[-0.03em] text-fg">{t("pick one on the left, or start a new one.")}</p>
        {/* A device never signed in to Droppen: 「登录 Droppen」 front and centre (local page only). */}
        <LinkFirstRun className="mt-6" />
      </div>
    </div>
  );
}
