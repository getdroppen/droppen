/*
 * The session sidebar grouped by directory, for the ordering audit: a loud
 * old project beside quiet recent ones. Headings must keep the start-time
 * order (`buildSessionList`) while their rows stream in and out; only the rows
 * inside a heading follow the feed's loudness order.
 */
import "@/index.css";
import { createRoot } from "react-dom/client";
import { useMemo } from "react";
import { MachineProvider } from "@/lib/machines/context";
import { LOCAL } from "@/lib/machines/key";
import { openMachine, setShell } from "@/lib/machines/registry";
import { constantStore } from "@/lib/machines/store";
import type { MachineInfo } from "@/lib/machines/runtime";
import { applyLanguage } from "@/i18n";
import { compareSessions, buildSessionList, type SessionListRow } from "@/features/sessions/session-list-model";
import { GroupHeader, SessionRow } from "@/features/sessions/sessions-layout";

type Session = Parameters<typeof buildSessionList>[0]["sessions"][number];

const now = Date.now();
const ago = (ms: number) => new Date(now - ms).toISOString();
const HOUR = 3_600_000;
const MINUTE = 60_000;
const DAY = 24 * HOUR;

function session(over: Partial<Session> & Pick<Session, "id" | "cwd" | "createdAt" | "updatedAt">): Session {
  return {
    accountId: null,
    attentionAt: null,
    activity: { state: "idle" } as Session["activity"],
    filePath: `/root/${over.id}.jsonl`,
    gitBranch: null,
    repoKey: `${over.cwd}/.git`,
    scope: "owned",
    source: "claude",
    status: "idle",
    title: "a session",
    unread: false,
    unreadCount: 0,
    ...over,
  } as Session;
}

// alpha started two days ago and is loud right now; beta started two hours
// ago and is quiet; gamma started five hours ago and is second loudest. The
// old order (activity) is alpha, gamma, beta; the start-time order is beta,
// gamma, alpha.
const SESSIONS: Session[] = [
  session({ id: "alpha-1", cwd: "/Users/k/Projects/alpha", createdAt: ago(2 * DAY), updatedAt: ago(MINUTE), unread: true, unreadCount: 4, title: "Fix the flaky feed retry", activity: { state: "working" } as Session["activity"], status: "working" }),
  session({ id: "alpha-2", cwd: "/Users/k/Projects/alpha", createdAt: ago(2 * DAY - HOUR), updatedAt: ago(4 * HOUR), title: "Rename the relay ticket" }),
  session({ id: "beta-1", cwd: "/Users/k/Projects/beta", createdAt: ago(2 * HOUR), updatedAt: ago(2 * HOUR), title: "Draft the migration note" }),
  session({ id: "beta-2", cwd: "/Users/k/Projects/beta", createdAt: ago(2 * HOUR - MINUTE), updatedAt: ago(6 * HOUR), title: "Check the quota edges" }),
  session({ id: "gamma-1", cwd: "/Users/k/Projects/gamma", createdAt: ago(5 * HOUR), updatedAt: ago(20 * MINUTE), unread: true, unreadCount: 2, title: "Why the sockets idle out" }),
  session({ id: "gamma-2", cwd: "/Users/k/Projects/gamma", createdAt: ago(5 * HOUR - MINUTE), updatedAt: ago(5 * HOUR), title: "Retry the relay probe" }),
];

const noop = () => undefined;

function SessionListFixture() {
  const rows = useMemo(
    () =>
      buildSessionList({
        sessions: [...SESSIONS].sort(compareSessions),
        source: "all",
        query: "",
        grouped: true,
        collapsedDirs: new Set<string>(),
        homeDir: "/Users/k",
      }).rows,
    [],
  );
  const draw = (row: SessionListRow) =>
    row.type === "header" ? (
      <GroupHeader
        dir={row.dir}
        collapseKey={row.collapseKey}
        list={row.list}
        isOpen={row.isOpen}
        onToggle={noop}
        onArchiveFolder={noop}
        onNewSessionHere={noop}
      />
    ) : row.type === "session" ? (
      <SessionRow
        session={row.session}
        dirLabel={row.dirLabel}
        nested={!row.session.pinned}
        onArchive={noop}
        onRename={noop}
        onPin={noop}
        pinPending={false}
        expandKey={row.expandKey}
        isOpen={row.isOpen}
        onToggleChildren={noop}
      />
    ) : null;
  return (
    <div data-fixture-root="" className="flex h-dvh w-[380px] flex-col border-r border-border bg-bg">
      <div className="flex h-8 shrink-0 items-center border-b border-border px-3">
        <span className="label-caps text-2xs text-muted">Sessions</span>
      </div>
      <div data-audit-row="sidebar" className="min-h-0 flex-1 overflow-y-auto">
        {rows.map((row) => (
          <div key={row.type === "header" ? row.collapseKey : row.type === "session" ? row.session.id : ""}>{draw(row)}</div>
        ))}
      </div>
    </div>
  );
}

/** Mount the grouped sidebar alone in the audit page. */
export async function mountSessionList(): Promise<void> {
  setShell("local", LOCAL);
  openMachine(LOCAL, { fetch: async () => Response.json({}), socketUrl: async (p: string) => p, openSocket: () => ({ readyState: 0, binaryType: "blob", send() {}, close() {}, onopen: null, onmessage: null, onclose: null }) } as never, {
    info: constantStore<readonly MachineInfo[]>([{ key: LOCAL, name: "studio", state: "online", since: 0, canManage: true, live: "live" }]),
    ensureLive: noop,
    hold: () => noop,
    dispose: noop,
  });
  try {
    localStorage.clear();
  } catch {
    // storage unavailable: the fixture still draws
  }
  await applyLanguage("en");

  const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
  const router = await import("@tanstack/react-router");
  const rootRoute = router.createRootRoute({ component: () => <router.Outlet /> });
  const listRoute = router.createRoute({ getParentRoute: () => rootRoute, path: "sessions", component: SessionListFixture });
  const detailRoute = router.createRoute({ getParentRoute: () => rootRoute, path: "sessions/$source/$id", component: () => null });
  const routes = router.createRouter({
    routeTree: rootRoute.addChildren([listRoute, detailRoute]),
    history: router.createMemoryHistory({ initialEntries: ["/sessions"] }),
  });
  await routes.load();

  createRoot(document.getElementById("root")!).render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MachineProvider machine={LOCAL}>
        <router.RouterProvider router={routes} />
      </MachineProvider>
    </QueryClientProvider>,
  );
}
