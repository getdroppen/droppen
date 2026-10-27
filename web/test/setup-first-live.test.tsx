// Step 03 and the hand-off mounted for real (`features/setup/first-step.tsx`,
// `first-session.ts`): React renders into a real DOM (jsdom), the
// device answers through a recording transport, and what is pinned is what
// the device is asked — the recent folders, the practice folder made when
// it is picked, and never a search of the disk — and that the
// guide's Finish lands in the real empty-session view with the folder, the
// tool and the first message filled in, the landing note over it, and no
// session started until the person presses Send.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { report, tool } from "./setup-fixtures.js";
import { domTree, type FakeNode, installLiveDom, propsOf, until } from "./live-dom.js";

const container = installLiveDom();
(globalThis as Record<symbol, unknown>)[Symbol.for("undici.globalOrigin.1")] = new URL("http://device.test");
Object.assign(window as object, { addEventListener: () => undefined, removeEventListener: () => undefined });
const flush = (ms = 30) => new Promise((resolve) => setTimeout(resolve, ms));
const { first } = domTree(container);

const i18next = (await import("i18next")).default;
const { initReactI18next } = await import("react-i18next");
await i18next.use(initReactI18next).init({ lng: "en", resources: {}, keySeparator: false, nsSeparator: false, interpolation: { escapeValue: false } });
const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
const router = await import("@tanstack/react-router");
const { GlobalFeedProvider, machineFeedFor } = await import("../src/app/global-feed.js");
const { MachineProvider } = await import("../src/lib/machines/context.js");
const { LOCAL } = await import("../src/lib/machines/key.js");
const registry = await import("../src/lib/machines/registry.js");
const { constantStore } = await import("../src/lib/machines/store.js");
const { NewSessionRoute } = await import("../src/features/sessions/new-session-view.js");
const { validateNewSessionSearch } = await import("../src/features/sessions/new-session-open.js");
const { readNewSessionDraft } = await import("../src/features/sessions/new-session.js");
const { FirstStep } = await import("../src/features/setup/first-step.js");
const { EMPTY_FIRST_UI } = await import("../src/features/setup/first-model.js");
const { LANDED, useOpenFirstSession } = await import("../src/features/setup/first-session.js");
const { dismissSetupToast, setupToastNow } = await import("../src/features/setup/toast.js");
const { emptyProgress } = await import("../src/features/setup/model.js");
type Transport = import("../src/lib/transport/index.js").Transport;
type FirstUi = import("../src/features/setup/first-model.js").FirstUi;
type SessionSummary = import("../src/api/schema.js").components["schemas"]["SessionSummary"];

interface Sent {
  method: string;
  path: string;
  body: unknown;
}

const HOME = "/home/demo";

/** A device that answers the guide's step 03 and the new-session view, and records every request. */
function device(sent: Sent[]): Transport {
  const answers: Record<string, unknown> = {
    "GET /api/models": { models: [{ id: "fable", label: null, efforts: ["low", "high"], defaultEffort: "high" }], source: "cache" },
    "GET /api/composer-prefs": { defaults: {}, sessions: {} },
    "GET /api/accounts": { accounts: [], defaultAccountId: null, defaultCodexAccountId: null },
    "GET /api/prefs": { composerSend: "enter" },
    "GET /api/quota": { accounts: [], claude: null, codex: null },
    "GET /api/setup/projects/recent": { folders: [{ path: `${HOME}/Projects/site`, name: "site", tools: ["claude"], lastUsed: null, sessions: 2 }] },
    "POST /api/setup/practice-folder": { path: `${HOME}/Droppen/first-session` },
    "POST /api/sessions/new": { id: "new-1" },
  };
  return {
    fetch: async (request: Request) => {
      const url = new URL(request.url);
      const body = request.method === "GET" ? null : await request.json().catch(() => null);
      sent.push({ method: request.method, path: url.pathname, body });
      const answer = answers[`${request.method} ${url.pathname}`];
      return answer === undefined ? Response.json({ error: "not here" }, { status: 404 }) : Response.json(answer);
    },
    socketUrl: async (path) => path,
    openSocket: () => ({ readyState: 0, binaryType: "blob", send() {}, close() {}, onopen: null, onmessage: null, onclose: null }),
  };
}

function reset(sent: Sent[]) {
  registry.setShell("local", LOCAL);
  registry.openMachine(LOCAL, device(sent), {
    info: constantStore({ key: LOCAL, name: "studio", state: "online" as const, since: 0, canManage: true, live: "live" as const }),
    ensureLive: () => undefined,
    hold: () => () => undefined,
    dispose: () => undefined,
  });
  localStorage.clear();
  machineFeedFor(LOCAL).store.publish({
    sessions: [
      {
        id: "s-1", source: "claude", title: "earlier", activity: "idle", status: "idle", asking: false, attentionAt: null,
        createdAt: "2026-10-18T00:00:00Z", updatedAt: "2026-10-18T01:00:00Z", cwd: `${HOME}/app`, filePath: "/x.jsonl",
        gitBranch: null, repoKey: null, resident: null, scope: "owned", server: "test", sizeBytes: 1, unreadCount: 0, unread: false,
      } as SessionSummary,
    ],
  });
}

/** The app's session routes around the real new-session route, and `extra` beside them. */
async function mount(path: string, extra: React.ReactNode = null) {
  const root = router.createRootRoute({ component: () => <router.Outlet /> });
  const sessions = router.createRoute({ getParentRoute: () => root, path: "sessions", component: () => <router.Outlet /> });
  const index = router.createRoute({ getParentRoute: () => sessions, path: "/", component: () => <div data-sessions-index="" /> });
  const fresh = router.createRoute({ getParentRoute: () => sessions, path: "new", component: NewSessionRoute, validateSearch: validateNewSessionSearch });
  const routes = router.createRouter({ routeTree: root.addChildren([sessions.addChildren([index, fresh])]), history: router.createMemoryHistory({ initialEntries: [path] }) });
  await routes.load();
  const errors: unknown[] = [];
  const reactRoot = createRoot(container as never, { onUncaughtError: (e) => void errors.push(e), onCaughtError: (e) => void errors.push(e) });
  const queries = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const tree = () => (
    <QueryClientProvider client={queries}>
      <MachineProvider machine={LOCAL}>
        <GlobalFeedProvider>
          <router.RouterProvider router={routes} />
          <router.RouterContextProvider router={routes}>{extra}</router.RouterContextProvider>
        </GlobalFeedProvider>
      </MachineProvider>
    </QueryClientProvider>
  );
  let generation = 0;
  reactRoot.render(<React.Fragment key={generation}>{tree()}</React.Fragment>);
  const settle = async () => {
    await flush();
    await routes.load();
    reactRoot.render(<React.Fragment key={++generation}>{tree()}</React.Fragment>);
    await flush();
  };
  return { root: reactRoot, routes, errors, settle };
}

const byTestId = (id: string) => first((n) => n.getAttribute("data-testid") === id);

/** Step 03 as the guide holds it. */
function Step({ onUi }: { onUi: (ui: FirstUi) => void }) {
  const [ui, setUi] = React.useState<FirstUi>(EMPTY_FIRST_UI);
  return (
    <FirstStep
      progress={emptyProgress()}
      tools={report(tool("claude", "ready"), tool("codex", "ready"))}
      online
      onChange={() => undefined}
      firstUi={ui}
      onFirstUi={(next) => {
        onUi(next);
        setUi(next);
      }}
    />
  );
}

test("the practice folder is made when it is picked, and picked with the first message for it", async (t) => {
  const sent: Sent[] = [];
  reset(sent);
  const uis: FirstUi[] = [];
  const { root } = await mount("/sessions", <Step onUi={(ui) => uis.push(ui)} />);
  t.after(() => root.unmount());
  await until("the practice row", () => !!byTestId("practice-row"));
  // The step has its recent folders before anything is picked in it.
  await until("the recent folders", () => !!first((n) => n.getAttribute("data-folder") === `${HOME}/Projects/site`));
  assert.equal(sent.filter((s) => s.path === "/api/setup/practice-folder").length, 0, "not made by opening the step");
  propsOf(byTestId("practice-row")).onClick!();
  await until("the practice folder picked", () => uis.at(-1)?.cwd === `${HOME}/Droppen/first-session`);
  assert.equal(uis.at(-1)?.practice, true);
  assert.equal(sent.filter((s) => s.method === "POST" && s.path === "/api/setup/practice-folder").length, 1);
  await until("its first message", () => !!first((n) => n.getAttribute("data-prompt") === "practice"));
  assert.equal(first((n) => n.getAttribute("data-prompt") === "practice")?.getAttribute("aria-checked"), "true");
  assert.deepEqual(
    sent.filter((s) => s.path.startsWith("/api/setup/projects/") && s.path !== "/api/setup/projects/recent"),
    [],
    "the step asks for the recent folders and nothing else about the disk"
  );
});

test("Finish lands in the empty-session view with the folder, the tool and the message filled in, the note over it — and nothing starts until Send", async (t) => {
  const sent: Sent[] = [];
  reset(sent);
  dismissSetupToast();
  let open: ((first: { folder: string; agent: "claude" | "codex"; text: string }) => void) | null = null;
  function Finish() {
    open = useOpenFirstSession();
    return null;
  }
  const { root, routes, settle, errors } = await mount("/sessions", <Finish />);
  t.after(() => root.unmount());
  await until("the guide's hand-off", () => open !== null);
  open!({ folder: `${HOME}/Projects/recipes-app`, agent: "codex", text: "Explain this project: what it does and how to run it. Don't change any files yet." });
  await until("the new-session view", () => routes.state.location.pathname === "/sessions/new");
  await settle();
  const textarea = () => first((n) => n.nodeName === "TEXTAREA");
  await until("the message in the composer", () => !!textarea() && propsOf(textarea()).value === "Explain this project: what it does and how to run it. Don't change any files yet.");
  await until("the address left clean", () => routes.state.location.searchStr === "");
  const draft = readNewSessionDraft(localStorage, LOCAL);
  assert.deepEqual([draft.text, draft.cwd, draft.source], ["Explain this project: what it does and how to run it. Don't change any files yet.", `${HOME}/Projects/recipes-app`, "codex"], "what the view reads");
  assert.equal(setupToastNow()?.text, LANDED, "the landing note, once");
  await flush(200);
  assert.equal(sent.filter((s) => s.method === "POST" && s.path.startsWith("/api/sessions")).length, 0, "nothing sent on landing");
  // The person presses Send: only now does a session start, with what the guide filled in.
  await settle();
  await until("the send button", () => !!first((n) => n.getAttribute("data-variant") === "primary" && n.nodeName === "BUTTON"));
  propsOf(first((n) => n.getAttribute("data-variant") === "primary" && n.nodeName === "BUTTON")).onClick!();
  await until("the start", () => sent.some((s) => s.method === "POST" && s.path === "/api/sessions/new"));
  const body = sent.find((s) => s.path === "/api/sessions/new")!.body as Record<string, unknown>;
  assert.equal(body.text, "Explain this project: what it does and how to run it. Don't change any files yet.");
  assert.equal(body.cwd, `${HOME}/Projects/recipes-app`);
  assert.equal(body.source, "codex");
  assert.deepEqual(errors, []);
});
