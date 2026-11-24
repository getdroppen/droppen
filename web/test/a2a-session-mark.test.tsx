// The session list's 「来自 A2A」 mark (`sessions-layout.tsx`, `useA2aMarks`):
// mounted for real, this device's feed played from a copy and its A2A task
// list answered by the test. A session an A2A task runs in carries the mark,
// with who sent it and through which entry in its title; a session no task
// knows, and a device whose server has no A2A task list, carry none.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { makeI18n } from "./render.js";
import { findAll, installLiveDom, until } from "./live-dom.js";

const container = installLiveDom();
// The client asks for `/api/…`: a relative URL needs an origin to be a Request.
(globalThis as Record<symbol, unknown>)[Symbol.for("undici.globalOrigin.1")] = new URL("http://device.test");
// No layout here: the list is virtualized, and draws rows only into a scroll area of some size.
for (const [name, value] of [["offsetHeight", 800], ["offsetWidth", 400]] as const) {
  Object.defineProperty(window.HTMLElement.prototype, name, { configurable: true, get: () => value });
}
const flush = (ms = 30) => new Promise((resolve) => setTimeout(resolve, ms));

const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
const router = await import("@tanstack/react-router");
const { I18nextProvider } = await import("react-i18next");
const { SessionsLayout, SessionsIndex } = await import("../src/features/sessions/sessions-layout.js");
const { GlobalFeedProvider } = await import("../src/app/global-feed.js");
const { MachineProvider } = await import("../src/lib/machines/context.js");
const { LOCAL } = await import("../src/lib/machines/key.js");
const registry = await import("../src/lib/machines/registry.js");
const { offlineTransport } = await import("../src/lib/transport/offline.js");
const { a2aZhCN } = await import("../src/i18n/a2a.js");

const i18n = makeI18n({ lng: "en", resources: { en: { translation: {} }, "zh-CN": { translation: a2aZhCN } }, keySeparator: false, nsSeparator: false });

const now = Date.now();
const ago = (m: number) => new Date(now - m * 60_000).toISOString();
const row = (id: string, minutes: number) => ({
  source: "claude",
  id,
  title: `title ${id}`,
  cwd: `/work/${id}`,
  updatedAt: ago(minutes),
  createdAt: ago(minutes + 10),
  attentionAt: ago(minutes),
  activity: "idle",
  status: "idle",
  asking: false,
  scope: "owned",
  unreadCount: 0,
  unread: false,
  resident: null,
});

const TASK = {
  id: "t1",
  contextId: "c1",
  entry: "dont-do",
  caller: "a2a:token:dont-do",
  source: "claude",
  sessionId: "a",
  cwd: "/work/a",
  branch: null,
  statusText: null,
  createdAt: ago(5),
  updatedAt: ago(1),
  state: "TASK_STATE_WORKING",
};

/** This device: its feed from a copy listing `rows`, and its A2A task list (`null`: a server without one). */
function device(rows: ReturnType<typeof row>[], tasks: unknown[] | null) {
  const source = {
    whenLoaded: async () => undefined,
    rows: () => rows as never,
    row: (s: string, id: string) => rows.find((r) => r.source === s && r.id === id) as never,
    status: { state: { asOf: 1, settings: { hostname: "this-one", homeDir: "/work", serverStartedAt: "2026-10-19T00:00:00Z" }, accounts: null, quota: null, cli: null, mkEpoch: null } },
    session: async () => null,
    sessionNow: () => null,
    touch: () => undefined,
    onChange: () => () => undefined,
  };
  const copy = offlineTransport(source as never, { user: "alice", messages: { readOnly: "ro", noCopy: "nc", unavailable: "na" }, machineOnline: () => true });
  registry.openMachine(LOCAL, {
    ...copy,
    fetch: async (request: Request) =>
      new URL(request.url).pathname === "/api/a2a/tasks"
        ? tasks === null
          ? Response.json({ error: "not here" }, { status: 404 })
          : Response.json({ tasks })
        : copy.fetch(request),
  });
  registry.setShell("local", LOCAL);
}

function mount(lng = "en") {
  void i18n.changeLanguage(lng);
  const root = createRoot(container as never);
  const rootRoute = router.createRootRoute({ component: () => <router.Outlet /> });
  const sessions = router.createRoute({ getParentRoute: () => rootRoute, path: "/sessions", component: SessionsLayout });
  const index = router.createRoute({ getParentRoute: () => sessions, path: "/", component: SessionsIndex });
  const app = router.createRouter({
    routeTree: rootRoute.addChildren([sessions.addChildren([index])]),
    history: router.createMemoryHistory({ initialEntries: ["/sessions"] }),
  });
  root.render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <MachineProvider machine={LOCAL}>
          <GlobalFeedProvider>
            <router.RouterProvider router={app} />
          </GlobalFeedProvider>
        </MachineProvider>
      </QueryClientProvider>
    </I18nextProvider>,
  );
  return { root, loaded: app.load() };
}

/** The row of session `id`: the link to it. */
const rowOf = (id: string) => [...container.querySelectorAll<HTMLAnchorElement>("a[href]")].find((a) => a.getAttribute("href")?.endsWith(`/claude/${id}`)) ?? null;

test("a session an A2A task runs in is marked 「来自 A2A」, with who sent it; the others are not", async (t) => {
  localStorage.clear();
  device([row("a", 1), row("b", 5)], [TASK]);
  const { root, loaded } = mount("zh-CN");
  t.after(() => root.unmount());
  await loaded;
  await until("the marked row", () => !!rowOf("a")?.querySelector("[data-a2a-mark]"));
  const mark = rowOf("a")!.querySelector("[data-a2a-mark]")!;
  assert.equal(mark.textContent, "来自 A2A");
  assert.equal(mark.getAttribute("data-a2a-mark"), "dont-do");
  assert.equal(mark.getAttribute("title"), "由 dont-do 经 A2A 启动，入口 dont-do");
  assert.ok(rowOf("b"), "the other row is listed");
  assert.equal(rowOf("b")!.querySelector("[data-a2a-mark]"), null);
});

test("a server without an A2A task list marks nothing, and the list is as it was", async (t) => {
  localStorage.clear();
  device([row("a", 1)], null);
  const { root, loaded } = mount();
  t.after(() => root.unmount());
  await loaded;
  await until("the row", () => !!rowOf("a"));
  await flush(100);
  assert.equal(findAll(container, "data-a2a-mark").length, 0);
});
