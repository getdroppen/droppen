// The real session view on droppen.app, on the machine itself, drawn from the
// device's offline copy of the session (`features/sessions/feed-from-copy.ts`)
// before the machine's own state has come. The header says nothing only that
// state can: the account the session runs on (a session bound to one would
// read "no account", in the warning colour), and whether the model it ran is
// the one asked for. They come in with the machine's state, as the composer's
// choice, quota and context do (`composer-from-copy.test.tsx`).
//
// So on the copy itself, the machine away: the real offline transport serves
// the session (`lib/transport/offline.ts`), and the header names no account
// and no drift; back on the machine, they come in with its state and not
// before. Another device in the local app's gateway, read from its copy the
// same way, names neither either.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { domTree, installLiveDom, until } from "./live-dom.js";

const container = installLiveDom({ act: true });
// Relative requests (`/api/…`) resolve as on a page.
(globalThis as Record<symbol, unknown>)[Symbol.for("undici.globalOrigin.1")] = new URL("http://device.test");
// jsdom lays nothing out, so the view's observers have nothing to report.
(globalThis as Record<string, unknown>).ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
const { first } = domTree(container);

const i18next = (await import("i18next")).default;
const { initReactI18next } = await import("react-i18next");
await i18next.use(initReactI18next).init({
  lng: "en",
  resources: {},
  keySeparator: false,
  nsSeparator: false,
  interpolation: { escapeValue: false },
});

const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
const router = await import("@tanstack/react-router");
const { GlobalFeedProvider } = await import("../src/app/global-feed.js");
const { MachineProvider } = await import("../src/lib/machines/context.js");
const { LOCAL } = await import("../src/lib/machines/key.js");
const registry = await import("../src/lib/machines/registry.js");
const { constantStore } = await import("../src/lib/machines/store.js");
const { SessionDetail } = await import("../src/features/sessions/session-detail.js");
const { feedHubFor } = await import("../src/app/global-feed.js");
const { offlineTransport } = await import("../src/lib/transport/offline.js");
const { SwitchingTransport } = await import("../src/lib/transport/switching.js");
const { openGatewayMachine } = await import("../src/lib/machines/shells/gateway.js");
type Transport = import("../src/lib/transport/types.js").Transport;
type OfflineSource = import("../src/lib/transport/offline.js").OfflineSource;
type AppShell = import("../src/lib/machines/runtime.js").AppShell;
type MachineInfo = import("../src/lib/machines/runtime.js").MachineInfo;

const MACHINE = "studio-mac";
const SESSION = "7c1e2a90-5b3d-4f6e-9a21-0d8c4b7e3f15";
/** The account the session runs on, which only the machine's state names. */
const ACCOUNT = { id: "acct-work", label: "work" };

const row = {
  source: "claude", id: SESSION, title: "a session on an account", cwd: "/home/demo/work", filePath: "/x.jsonl",
  gitBranch: "main", repoKey: null, createdAt: "2026-11-08T00:00:00Z", updatedAt: "2026-11-08T09:00:00Z", sizeBytes: 1, scope: "owned",
  status: "idle", activity: "idle", attentionAt: null, unread: false, unreadCount: 0, resident: null,
  // Started on an account: the root is the managed one, and the list says so.
  claudeRoot: "managed",
};
const item = (n: number) => ({ role: "user", kind: "text", text: `item ${n}`, detail: null, isError: false, meta: null, question: null, timestamp: null, toolName: null, truncated: false });
const copy = { started: true, startIndex: 0, endIndex: 3, items: [item(0), item(1), item(2)], fp: "fp3", prefixFp: "fp0" };
/**
 * The machine's own state, as it follows the resume delta: the session ran
 * sonnet on the account, where it had asked for opus (below), at a time after
 * that pick.
 */
const stateFrame = {
  type: "state", summary: row, canSend: true, sendBlockReason: null,
  cliChoice: { model: "claude-sonnet-5", effort: "low", permission: "plan", at: "2026-11-08T09:00:00Z" },
  injection: null, contextTokens: 84_000, contextWindow: 200_000, claudeRoot: "managed", account: ACCOUNT, rotation: null, receipts: [], questions: [],
};

/** A socket the test answers by hand. */
class FakeSocket {
  readyState = 0;
  binaryType = "blob";
  sent: Record<string, unknown>[] = [];
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: unknown) => void) | null = null;
  constructor(readonly url: string) {}
  send(data: string) {
    this.sent.push(JSON.parse(data) as Record<string, unknown>);
  }
  close() {
    this.readyState = 3;
    this.onclose?.({});
  }
  open() {
    this.readyState = 1;
    this.onopen?.({});
  }
  frame(value: unknown) {
    this.onmessage?.({ data: JSON.stringify(value) });
  }
}

/** The machine behind the view: its reads answered, its sockets kept for the test to open. */
function device(sockets: FakeSocket[]): Transport {
  const answers: Record<string, unknown> = {
    "/api/prefs": { composerSend: "enter", sessionGrouping: "flat", transcriptStyle: "flat", fontSize: "default", language: "en" },
    // The session asked for opus before its last turn: a model drift, once the machine says what ran.
    "/api/composer-prefs": { defaults: {}, sessions: { [`claude/${SESSION}`]: { model: "opus", updatedAt: "2026-11-08T08:00:00Z" } } },
    "/api/models": { models: [{ id: "fable", label: null, efforts: ["low", "high"], defaultEffort: "high" }], source: "cache" },
    "/api/accounts": { accounts: [], defaultAccountId: null, defaultCodexAccountId: null },
    "/api/stats/quota": { accounts: [], claude: null, codex: null },
  };
  return {
    fetch: async (request: Request) => {
      const answer = answers[new URL(request.url).pathname];
      return answer === undefined ? Response.json({ error: "not here" }, { status: 404 }) : Response.json(answer);
    },
    socketUrl: async (path) => path,
    openSocket: (url) => {
      const socket = new FakeSocket(url);
      sockets.push(socket);
      return socket;
    },
  };
}

const INFO: MachineInfo = { key: MACHINE, name: MACHINE, state: "online", since: 0, canManage: true, live: "live" };
/** What the feed reads of the machine's offline copy (`MachineMirror`): the list's row and the session's copy. */
const mirror = {
  row: (source: string, id: string) => (source === "claude" && id === SESSION ? row : undefined),
  sessionNow: (source: string, id: string) => (source === "claude" && id === SESSION ? { source, id, state: copy, seq: 4, freshestMs: 1 } : null),
  whenLoaded: async () => undefined,
  sessionHeld: async (source: string, id: string) => mirror.sessionNow(source, id),
};
/** droppen.app's shell as the feed reads it: the machine, its copy, and that the page is on the machine itself. */
const shell = {
  kind: "hub" as const,
  home: MACHINE,
  machines: () => constantStore<readonly MachineInfo[]>([INFO]),
  machine: (key: string | null | undefined) => (key === MACHINE ? { mirror, transport: { current: "live" } } : null),
  setMain: () => undefined,
};

/**
 * The machine's offline copy as the offline transport reads it: the list's
 * row, the session's copy, and the status it last wrote, with the account
 * the session runs on among its accounts.
 */
const copySource: OfflineSource = {
  whenLoaded: async () => undefined,
  rows: () => [row as never],
  row: (source, id) => mirror.row(source, id) as never,
  status: {
    state: {
      asOf: 1,
      settings: null,
      accounts: { accounts: [{ ...ACCOUNT, kind: "oauth_token", provider: "claude" }], defaultAccountId: null },
      quota: { accounts: [], claude: null, codex: null },
      cli: null,
      mkEpoch: null,
    },
  },
  session: async (source, id) => mirror.sessionNow(source, id) as never,
  sessionNow: (source, id) => mirror.sessionNow(source, id) as never,
  sessionProgress: () => null,
  onSessionProgress: () => () => undefined,
  touch: () => undefined,
  onChange: () => () => undefined,
};
const offlineOptions = { user: "cloud:browser", messages: { readOnly: "read-only copy", noCopy: "no copy here", unavailable: "unavailable" }, machineOnline: () => false };
const driver = (info: MachineInfo) => ({ info: constantStore(info), ensureLive: () => undefined, hold: () => () => undefined, dispose: () => undefined });

/** The session view, routed as the app routes it, on the machine's own page. */
async function mount(sockets: FakeSocket[]) {
  registry.openMachine(MACHINE, device(sockets), driver(INFO));
  registry.setShell("hub", MACHINE);
  registry.setCurrentShell(shell as unknown as AppShell);
  localStorage.clear();
  return render(MACHINE);
}

/** The session view, routed as the app routes it, on `machine`'s page. */
async function render(machine: string) {
  const rootRoute = router.createRootRoute({ component: () => <router.Outlet /> });
  const detailRoute = router.createRoute({ getParentRoute: () => rootRoute, path: "sessions/$source/$id", component: SessionDetail });
  const listRoute = router.createRoute({ getParentRoute: () => rootRoute, path: "sessions", component: () => null });
  const routes = router.createRouter({
    routeTree: rootRoute.addChildren([detailRoute, listRoute]),
    history: router.createMemoryHistory({ initialEntries: [`/sessions/claude/${SESSION}`] }),
  });
  await routes.load();
  const errors: unknown[] = [];
  const page = createRoot(container as never, {
    onUncaughtError: (error) => void errors.push(error),
    onCaughtError: (error) => void errors.push(error),
  });
  await React.act(async () => {
    page.render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <MachineProvider machine={machine}>
          <GlobalFeedProvider>
            <router.RouterProvider router={routes} />
          </GlobalFeedProvider>
        </MachineProvider>
      </QueryClientProvider>,
    );
    await new Promise((resolve) => setTimeout(resolve, 50));
  });
  return { page, errors };
}

const settle = (ms = 80) => React.act(async () => void (await new Promise((resolve) => setTimeout(resolve, ms))));
/** The header's second row: the session's source, account, branch, model drift and size. */
const metadata = () => first((n) => n.nodeName === "HEADER")?.children[1] ?? null;
const metadataWords = () => (metadata()?.textContent ?? "").trim();
const drift = () => metadata()?.querySelector('[aria-label^="requested "]') ?? null;
const banner = () => first((n) => n.getAttribute("data-signal") === "reconnecting");

test("drawn from the copy, the header names no account and no model drift until the machine's state is in, then the session's own", async (t) => {
  const sockets: FakeSocket[] = [];
  const { page, errors } = await mount(sockets);
  t.after(() => page.unmount());
  await until("the copy's transcript", () => (container.textContent ?? "").includes("item 2"));
  assert.ok(banner(), "under the banner that says it is the copy");
  // Long enough for the accounts, the stored choices and the models to be in:
  // whatever the header would show without the session's state, it shows by now.
  await settle(150);
  assert.match(metadataWords(), /main/, "the list's own facts are there");
  assert.doesNotMatch(metadataWords(), /no account|local|work/, "but no account: not \"no account\" over a session bound to one");
  assert.equal(drift(), null, "and no word on the model it ran");

  // The machine answers on its one socket: the session resumed from the copy's
  // marker with nothing new, then its state.
  const hub = sockets.find((s) => s.url.startsWith("/api/events"));
  assert.ok(hub, "the machine's socket");
  await React.act(async () => hub.open());
  await until("the session's subscription", () => hub.sent.some((m) => m.type === "sub"));
  const sub = (hub.sent.find((m) => m.type === "sub")!.feeds as { sub: number; since?: number; fp?: string }[])[0]!;
  assert.deepEqual([sub.since, sub.fp], [3, "fp0"], "resumed from the copy's own marker");
  await React.act(async () => {
    hub.frame({ type: "hello" });
    hub.frame({ sub: sub.sub, type: "delta", sig: "live:1", fromIndex: 3, items: [], endIndex: 3, totalItems: 3, prefixFp: "fp0" });
    hub.frame({ sub: sub.sub, ...stateFrame });
  });
  await until("the account the session runs on", () => metadataWords().includes(ACCOUNT.label));
  assert.equal(banner(), undefined, "the copy's banner gone with the machine's answer");
  assert.doesNotMatch(metadataWords(), /no account/);
  await until("the model drift the machine's state shows", () => drift() !== null);
  assert.equal(drift()!.getAttribute("aria-label"), "requested opus, actually running claude-sonnet-5");
  assert.deepEqual(errors, []);
});

test("on the copy, the machine away, the header names no account and no model drift; back on the machine, they come with its state and not before", async (t) => {
  const AWAY = "travel-air";
  const sockets: FakeSocket[] = [];
  const info: MachineInfo = { key: AWAY, name: AWAY, state: "offline", since: 0, canManage: true, live: "copy" };
  // Cloud mode's transport while the machine is away: every read and the
  // session's feed answered by the offline transport over its copy.
  const transport = new SwitchingTransport({ live: device(sockets), offline: offlineTransport(copySource, offlineOptions) }, "offline");
  registry.openMachine(AWAY, transport, driver(info));
  registry.setShell("hub", AWAY);
  registry.setCurrentShell({
    ...shell,
    home: AWAY,
    machines: () => constantStore<readonly MachineInfo[]>([info]),
    machine: (key: string | null | undefined) => (key === AWAY ? { mirror, transport } : null),
  } as unknown as AppShell);
  localStorage.clear();
  const { page, errors } = await render(AWAY);
  t.after(() => page.unmount());
  await until("the transcript the copy serves", () => (container.textContent ?? "").includes("item 2"));
  // Long enough for the accounts (the copy's) and the stored choices (refused) to be in.
  await settle(150);
  assert.match(metadataWords(), /main/, "the list's own facts are there");
  assert.doesNotMatch(metadataWords(), /no account|local|work/, "but no account: not \"no account\" over a session bound to one");
  assert.equal(drift(), null, "and no word on the model it ran");

  // The machine is back: the page goes to it, and its socket reconnects at once, as the shell has it on a switch.
  await React.act(async () => {
    transport.setMode("live");
    await new Promise((resolve) => setTimeout(resolve, 0));
    feedHubFor(AWAY).kick();
  });
  await until("the machine's socket", () => sockets.some((s) => s.url.startsWith("/api/events")));
  const hub = sockets.find((s) => s.url.startsWith("/api/events"))!;
  await React.act(async () => hub.open());
  await until("the session's subscription", () => hub.sent.some((m) => m.type === "sub"));
  const sub = (hub.sent.find((m) => m.type === "sub")!.feeds as { sub: number; since?: number; fp?: string }[])[0]!;
  assert.deepEqual([sub.since, sub.fp], [3, "fp0"], "resumed from the copy's own marker");
  await React.act(async () => {
    hub.frame({ type: "hello" });
    hub.frame({ sub: sub.sub, type: "delta", sig: "live:1", fromIndex: 3, items: [], endIndex: 3, totalItems: 3, prefixFp: "fp0" });
  });
  await settle(150);
  assert.doesNotMatch(metadataWords(), /no account|local|work/, "the copy's state still held after the delta: still no account");
  assert.equal(drift(), null);
  await React.act(async () => hub.frame({ sub: sub.sub, ...stateFrame }));
  await until("the account the session runs on", () => metadataWords().includes(ACCOUNT.label));
  await until("the model drift the machine's state shows", () => drift() !== null);
  assert.deepEqual(errors, []);
});

test("another device in the local app's gateway, read from its copy, names no account and no model drift in the header", async (t) => {
  const OTHER = "m-studio";
  const info: MachineInfo = { key: OTHER, name: "studio", state: "online", since: 0, canManage: false, live: "copy", readOnly: true };
  registry.setCurrentShell(null);
  registry.setShell("local", LOCAL);
  // The gateway reads another device through the same offline transport, over the copy the install holds.
  openGatewayMachine(OTHER, offlineTransport(copySource, { ...offlineOptions, machineOnline: () => true }), driver(info));
  localStorage.clear();
  const { page, errors } = await render(OTHER);
  t.after(() => page.unmount());
  await until("the transcript its copy serves", () => (container.textContent ?? "").includes("item 2"));
  await settle(150);
  assert.match(metadataWords(), /studio/, "the device it is on");
  assert.match(metadataWords(), /main/);
  assert.doesNotMatch(metadataWords(), /no account|local|work/, "no account: the copy does not say the device's");
  assert.equal(drift(), null);
  assert.deepEqual(errors, []);
});
