// A new instant chat, a new scheduled task and the default in Settings, when
// the local login is signed out — mounted for real into the stand-in DOM
// (`fake-dom.ts`), the device answering through a recording transport. Chats
// and tasks start with no account named, so they run on the server's start
// rule (`accounts::rotation::start_for`): the default; the local login when
// somebody chose it; else the local login unless it is signed out, when a free
// account in the auto-rotation pool takes its place — never one out of the
// pool. The pages show the server's own answer (`GET /api/accounts/start`), or
// the same rule read off the list when the server has none, so what these
// screens show — the account, its models — is the login the run will use.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after } from "node:test";
import { register } from "node:module";
import { FakeNode, installFakeDom } from "./fake-dom.js";

// The build's plugin list (`virtual:droppen/bundled-plugins`, made by Vite).
const bundled = `data:text/javascript,${encodeURIComponent('export const distribution = "public"; export const bundledPlugins = [];')}`;
register(
  `data:text/javascript,${encodeURIComponent(
    `export async function resolve(specifier, context, next) { return specifier === "virtual:droppen/bundled-plugins" ? { url: ${JSON.stringify(bundled)}, shortCircuit: true } : next(specifier, context); }`,
  )}`,
);

const container = installFakeDom();
// The schedule form's controls ask an element whether it matches a selector;
// the stand-in page has no selector engine, and nothing here matches.
(FakeNode.prototype as unknown as { matches?: () => boolean }).matches ??= () => false;
(globalThis as Record<symbol, unknown>)[Symbol.for("undici.globalOrigin.1")] = new URL("http://device.test");
after(() => void setTimeout(() => process.exit(), 50));
const flush = (ms = 30) => new Promise((resolve) => setTimeout(resolve, ms));

const i18next = (await import("i18next")).default;
const { initReactI18next } = await import("react-i18next");
await i18next.use(initReactI18next).init({ lng: "en", resources: {}, keySeparator: false, nsSeparator: false, interpolation: { escapeValue: false } });
const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
const router = await import("@tanstack/react-router");
const { MachineProvider } = await import("../src/lib/machines/context.js");
const { LOCAL } = await import("../src/lib/machines/key.js");
const registry = await import("../src/lib/machines/registry.js");
const { constantStore } = await import("../src/lib/machines/store.js");
const { ChatHome } = await import("../src/features/chat/chat-home.js");
const { ScheduleSection } = await import("../src/features/settings/schedule-section.js");
const { AccountsSection } = await import("../src/features/settings/accounts-section.js");
type Transport = import("../src/lib/transport/index.js").Transport;

interface Sent {
  method: string;
  path: string;
  query: string;
  body: unknown;
}

const WORK = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const SPARE = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

function account(id: string, label: string, rest: unknown[] = []) {
  return {
    id, label, kind: "oauth_token", provider: "claude", usageEnabled: false, rotation: true, rest,
    createdAt: "2026-10-01T00:00:00Z", updatedAt: "2026-10-01T00:00:00Z",
  };
}
const AUTH_REST = {
  scope: "all", class: "auth", untilAt: null, reason: "Failed to authenticate: OAuth session expired and could not be refreshed",
  observedAt: "2026-10-19T05:11:15Z", sessionId: "s-1", source: "claude:result",
};
const RESTING = { ...AUTH_REST, class: "session", untilAt: "2099-01-01T00:00:00Z", reason: "session limit" };
const local = (state: string) => ({
  claude: { state, rest: state === "signed_out" ? [AUTH_REST] : [], isDefault: false },
  codex: { state: "unknown", rest: [], isDefault: false },
});
const list = (accounts: unknown[], state: string, defaultAccountId: string | null = null) => ({
  accounts, defaultAccountId, defaultCodexAccountId: null, local: local(state),
});

/**
 * The device. `start` is its answer to `GET /api/accounts/start`; without one
 * the route is missing, as on a server too old to have it, and the page reads
 * the rule off the list instead.
 */
function device(sent: Sent[], accounts: unknown, start?: unknown, extra: Record<string, unknown> = {}): Transport {
  const answers: Record<string, unknown> = {
    "/api/accounts": accounts,
    ...(start === undefined ? {} : { "/api/accounts/start": start }),
    "/api/stats/quota": { accounts: [], claude: null, codex: null },
    "/api/models": { models: [{ id: "fable", label: null, efforts: ["low", "high"], defaultEffort: "high" }], source: "cache" },
    "/api/composer-prefs": { defaults: {}, sessions: {} },
    ...extra,
    "/api/chat-provider": { baseUrl: "", hasKey: false, model: "" },
    "/api/scheduled-tasks": { tasks: [] },
    "/api/sessions/recent-cwds": { cwds: ["/home/demo/app"] },
  };
  return {
    fetch: async (request: Request) => {
      const url = new URL(request.url);
      const body = request.method === "GET" ? null : await request.json().catch(() => null);
      sent.push({ method: request.method, path: url.pathname, query: url.search, body });
      if (url.pathname === "/api/chats/new" && request.method === "POST") return Response.json({ kind: "claude", id: "chat-1" });
      if (url.pathname === "/api/accounts/default" && request.method === "PUT") return Response.json(accounts);
      const listed = answers[url.pathname];
      // An answer may depend on what is asked, as the server's does — and may
      // take its time (a promise).
      let answer: unknown;
      try {
        answer = typeof listed === "function" ? await (listed as (query: URLSearchParams) => unknown)(url.searchParams) : listed;
      } catch (error) {
        return Response.json({ error: String(error) }, { status: 500 });
      }
      return answer === undefined ? Response.json({ error: "not here" }, { status: 404 }) : Response.json(answer);
    },
    socketUrl: async (path) => path,
    openSocket: () => ({ readyState: 0, binaryType: "blob", send() {}, close() {}, onopen: null, onmessage: null, onclose: null }),
  };
}

function reset(sent: Sent[], accounts: unknown, start?: unknown, extra: Record<string, unknown> = {}) {
  registry.setShell("local", LOCAL);
  registry.openMachine(LOCAL, device(sent, accounts, start, extra), {
    info: constantStore({ key: LOCAL, name: "studio", state: "online" as const, since: 0, canManage: true, live: "live" as const }),
    ensureLive: () => undefined,
    hold: () => () => undefined,
    dispose: () => undefined,
  });
}

async function mount(node: React.ReactNode, queryOptions: Record<string, unknown> = { retry: false }) {
  const errors: unknown[] = [];
  const root = createRoot(container as never, {
    onUncaughtError: (error) => void errors.push(error),
    onCaughtError: (error) => void errors.push(error),
  });
  const queries = new QueryClient({ defaultOptions: { queries: queryOptions } });
  const routes = router.createRouter({
    routeTree: router.createRootRoute({ component: () => <>{node}</> }),
    history: router.createMemoryHistory({ initialEntries: ["/chat"] }),
  });
  await routes.load();
  root.render(
    <QueryClientProvider client={queries}>
      <MachineProvider machine={LOCAL}>
        <router.RouterProvider router={routes} />
      </MachineProvider>
    </QueryClientProvider>,
  );
  await flush();
  return { root, errors };
}

function walk(node: FakeNode, out: FakeNode[] = []): FakeNode[] {
  for (const child of node.childNodes) {
    out.push(child);
    walk(child, out);
  }
  return out;
}
const all = () => walk(container as FakeNode);
const first = (match: (node: FakeNode) => boolean) => all().find(match);
const text = () => (container as FakeNode).textContent;
const propsOf = (node: FakeNode | undefined): Record<string, (...args: unknown[]) => unknown> => {
  assert.ok(node, "the element is on the page");
  const key = Object.keys(node).find((k) => k.startsWith("__reactProps$"));
  assert.ok(key, "rendered by React");
  return (node as unknown as Record<string, Record<string, (...args: unknown[]) => unknown>>)[key];
};
async function until(what: string, check: () => boolean) {
  for (let i = 0; i < 100; i++) {
    if (check()) return;
    await flush(20);
  }
  assert.fail(`timed out waiting for ${what}`);
}
const accountChip = () => first((n) => n.nodeName === "BUTTON" && n.getAttribute("aria-label") === "account");
const notice = () => first((n) => n.hasAttribute("data-local-signed-out"));
const modelQueries = (sent: Sent[]) => sent.filter((s) => s.path === "/api/models").map((s) => new URLSearchParams(s.query).get("accountId"));

test("an instant chat does not start on a signed-out local login: it shows, and lists models for, the first usable account", async (t) => {
  const sent: Sent[] = [];
  // `work` is resting on everything; `spare` is the one a chat can run on.
  reset(sent, list([account(WORK, "work", [RESTING]), account(SPARE, "spare")], "signed_out"));
  const { root, errors } = await mount(<ChatHome />);
  t.after(() => root.unmount());
  await until("the account list", () => /spare/.test(accountChip()?.textContent ?? ""));
  await until("the account's models", () => modelQueries(sent).includes(SPARE));
  assert.equal(notice(), undefined, "nothing to warn about: it does not run on the local login");

  propsOf(first((n) => n.nodeName === "TEXTAREA")).onChange!({ target: { value: "hello" } });
  await flush();
  propsOf(first((n) => n.getAttribute("data-variant") === "primary")).onClick!();
  await until("the start", () => sent.some((s) => s.path === "/api/chats/new"));
  const body = sent.find((s) => s.path === "/api/chats/new")?.body as Record<string, unknown>;
  assert.equal("accountId" in body, false, "untouched, the chat names no account: the server applies the same rule");
  assert.deepEqual(errors, []);
});

test("with no account to fall back on, an instant chat says the local login is signed out and how to fix it", async (t) => {
  const sent: Sent[] = [];
  reset(sent, list([], "signed_out"));
  const { root } = await mount(<ChatHome />);
  t.after(() => root.unmount());
  await until("the warning", () => !!notice());
  assert.match(notice()!.textContent, /claude login on this device is signed out: run `claude` in a terminal on this device, then `\/login`/);
  assert.match(notice()!.textContent, /or add an account · Settings/);
  assert.equal(accountChip(), undefined, "no picker with nothing to pick");
});

test("a local login nobody knows anything about stays the chat's default, as before", async (t) => {
  const sent: Sent[] = [];
  reset(sent, list([account(WORK, "work")], "unknown"));
  const { root } = await mount(<ChatHome />);
  t.after(() => root.unmount());
  await until("the account chip", () => !!accountChip());
  assert.match(accountChip()!.textContent, /^local login$/);
  assert.equal(notice(), undefined);
  await until("the models", () => modelQueries(sent).length > 0);
  assert.ok(modelQueries(sent).every((id) => id === null), "the local login's models");
});

test("a scheduled task's form says which login it will run on while the local login is signed out", async (t) => {
  const sent: Sent[] = [];
  reset(sent, list([account(WORK, "work", [RESTING]), account(SPARE, "spare")], "signed_out"));
  const { root, errors } = await mount(<ScheduleSection />);
  t.after(() => root.unmount());
  await until("the section", () => !!first((n) => n.nodeName === "BUTTON" && /add scheduled task/.test(n.textContent)));
  propsOf(first((n) => n.nodeName === "BUTTON" && /add scheduled task/.test(n.textContent))).onClick!();
  await until("the note", () => !!notice());
  assert.match(notice()!.textContent, /your own login on this device is signed out, so this task runs on spare until you sign in again/);
  await until("the account's models", () => modelQueries(sent).includes(SPARE));
  assert.deepEqual(errors, []);
});

test("a scheduled task with a configured default says nothing: the default is the user's own choice", async (t) => {
  const sent: Sent[] = [];
  reset(sent, list([account(WORK, "work"), account(SPARE, "spare")], "signed_out", WORK));
  const { root } = await mount(<ScheduleSection />);
  t.after(() => root.unmount());
  await until("the section", () => !!first((n) => n.nodeName === "BUTTON" && /add scheduled task/.test(n.textContent)));
  propsOf(first((n) => n.nodeName === "BUTTON" && /add scheduled task/.test(n.textContent))).onClick!();
  await until("the default's models", () => modelQueries(sent).includes(WORK));
  assert.equal(notice(), undefined);
  assert.doesNotMatch(text(), /signed out/);
});

/** An env block behind a gateway: out of the auto-rotation pool, as every env account is unless somebody puts it in. */
function gateway(id: string, label: string) {
  return { ...account(id, label), kind: "env", rotation: false, envKeys: ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL"] };
}
const GATEWAY = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";

/** A component's own props, found up the fiber tree from one of its DOM nodes. */
function componentProps(node: FakeNode | undefined, name: string): Record<string, (...args: unknown[]) => unknown> {
  assert.ok(node, "the element is on the page");
  const key = Object.keys(node).find((k) => k.startsWith("__reactFiber$"));
  assert.ok(key, "rendered by React");
  let fiber = (node as unknown as Record<string, { type?: { name?: string }; return?: unknown; memoizedProps?: unknown }>)[key];
  while (fiber && fiber.type?.name !== name) fiber = fiber.return as typeof fiber;
  assert.ok(fiber, `inside ${name}`);
  return fiber.memoizedProps as Record<string, (...args: unknown[]) => unknown>;
}

async function startChat(sent: Sent[]): Promise<Record<string, unknown>> {
  propsOf(first((n) => n.nodeName === "TEXTAREA")).onChange!({ target: { value: "hello" } });
  await flush();
  propsOf(first((n) => n.getAttribute("data-variant") === "primary")).onClick!();
  await until("the start", () => sent.some((s) => s.path === "/api/chats/new"));
  return sent.find((s) => s.path === "/api/chats/new")?.body as Record<string, unknown>;
}

test("an account out of the auto-rotation pool never stands in for a signed-out local login", async (t) => {
  const sent: Sent[] = [];
  // First in the list, an env block behind a gateway; the one pool account is resting.
  reset(sent, list([gateway(GATEWAY, "gateway"), account(WORK, "work", [RESTING])], "signed_out"));
  const { root, errors } = await mount(<ChatHome />);
  t.after(() => root.unmount());
  await until("the warning", () => !!notice());
  assert.match(accountChip()!.textContent, /^local login \(not signed in\)$/, "not the gateway, not the resting account");
  assert.match(notice()!.textContent, /claude login on this device is signed out: run `claude` in a terminal on this device, then `\/login`/);
  assert.match(notice()!.textContent, /and no account in auto-rotation is free to take its place/);
  await until("the models", () => modelQueries(sent).length > 0);
  assert.ok(modelQueries(sent).every((id) => id === null), "the local login's models");
  const body = await startChat(sent);
  assert.equal("accountId" in body, false, "untouched: the server's rule, which keeps the gateway out too");
  assert.deepEqual(errors, []);
});

/** A model list, as `GET /api/models` answers for one login. */
const models = (...ids: string[]) => ({
  models: ids.map((id) => ({ id, label: null, efforts: ["low", "high"], defaultEffort: "high" })),
  source: "cache",
});
/** D, the default, was probed: its list leads with opus. E was not: the static list, fable first. */
const D_MODELS = models("opus[1m]", "fable", "sonnet");
const E_MODELS = models("fable", "opus", "sonnet", "haiku");
const listFor = (query: URLSearchParams) => (query.get("accountId") === WORK ? D_MODELS : E_MODELS);
const startQueries = (sent: Sent[]) => sent.filter((s) => s.path === "/api/accounts/start").map((s) => new URLSearchParams(s.query));

test("nothing stored: the chat shows and sends the login and model the server chose together", async (t) => {
  const sent: Sent[] = [];
  // D (`work`) is the default and rests on opus, its list's head; E (`spare`)
  // leads with fable. Asked one at a time they would chase each other (opus →
  // E, fable → D); the server answers with a pair that lands on itself.
  reset(sent, list([account(WORK, "work"), account(SPARE, "spare")], "unknown", WORK), (query: URLSearchParams) =>
    query.get("chooseModel") === "true"
      ? { accountId: WORK, reason: "default", model: "fable", placed: true }
      : { accountId: SPARE, reason: "rotated", model: null, placed: true },
  { "/api/models": listFor });
  const { root, errors } = await mount(<ChatHome />);
  t.after(() => root.unmount());
  await until("the pair", () => /work/.test(accountChip()?.textContent ?? "") && modelQueries(sent).includes(WORK));
  assert.ok(startQueries(sent).every((q) => q.get("chooseModel") === "true" && !q.has("model")), "nothing stored: the server chooses");

  const body = await startChat(sent);
  assert.equal(body.model, "fable", "the model of the pair, not D's head (which would land on E)");
  assert.equal("accountId" in body, false, "and no account: sent with fable, the server places it on D");
  assert.deepEqual(errors, []);
});

test("a stored model is the model; the chat shows where the server places it", async (t) => {
  const sent: Sent[] = [];
  reset(
    sent,
    list([account(WORK, "work"), account(SPARE, "spare")], "unknown", WORK),
    (query: URLSearchParams) =>
      query.get("model") === "opus"
        ? { accountId: SPARE, reason: "rotated", model: null, placed: true }
        : { accountId: WORK, reason: "default", model: null, placed: true },
    { "/api/models": listFor, "/api/composer-prefs": { defaults: { claude: { model: "opus" } }, sessions: {} } },
  );
  const { root, errors } = await mount(<ChatHome />);
  t.after(() => root.unmount());
  await until("the placement", () => /spare/.test(accountChip()?.textContent ?? "") && modelQueries(sent).includes(SPARE));
  const asked = startQueries(sent).at(-1)!;
  assert.equal(asked.get("model"), "opus");
  assert.equal(asked.has("chooseModel"), false);
  const body = await startChat(sent);
  assert.equal(body.model, "opus", "the stored model, as the shown login lists it");
  assert.equal("accountId" in body, false);
  assert.deepEqual(errors, []);
});

test("the pair holds while the shown login's list is still loading", async (t) => {
  const sent: Sent[] = [];
  let release: () => void = () => undefined;
  const held = new Promise<void>((resolve) => (release = resolve));
  reset(
    sent,
    list([account(WORK, "work"), account(SPARE, "spare")], "unknown", WORK),
    { accountId: WORK, reason: "default", model: "fable", placed: true },
    // D's list does not come until released: the composer has only the static guess.
    { "/api/models": async (query: URLSearchParams) => (query.get("accountId") === WORK ? (await held, D_MODELS) : E_MODELS) },
  );
  const { root, errors } = await mount(<ChatHome />);
  t.after(() => {
    release();
    root.unmount();
  });
  await until("the pair", () => /work/.test(accountChip()?.textContent ?? ""));
  const body = await startChat(sent);
  assert.equal(body.model, "fable", "the server's model, whatever the list on screen leads with meanwhile");
  assert.equal("accountId" in body, false);
  assert.deepEqual(errors, []);
});

test("a chat left to the server waits for the server's answer before it can be sent", async (t) => {
  const sent: Sent[] = [];
  let answer: (value: unknown) => void = () => undefined;
  const late = new Promise((resolve) => (answer = resolve));
  reset(sent, list([account(WORK, "work"), account(SPARE, "spare")], "unknown", WORK), () => late, { "/api/models": listFor });
  const { root, errors } = await mount(<ChatHome />);
  t.after(() => {
    answer({ accountId: WORK, reason: "default", model: null, placed: true });
    root.unmount();
  });
  await until("the composer", () => !!first((n) => n.nodeName === "TEXTAREA"));
  propsOf(first((n) => n.nodeName === "TEXTAREA")).onChange!({ target: { value: "hello" } });
  await flush();
  const send = () => first((n) => n.getAttribute("data-variant") === "primary");
  assert.equal(propsOf(send()).disabled as unknown, true, "no answer yet: nothing to show, so nothing to send");

  answer({ accountId: SPARE, reason: "rotated", model: "sonnet", placed: true });
  await until("the answer", () => /spare/.test(accountChip()?.textContent ?? ""));
  await until("send", () => propsOf(send()).disabled !== true);
  propsOf(send()).onClick!();
  await until("the start", () => sent.some((s) => s.path === "/api/chats/new"));
  const body = sent.find((s) => s.path === "/api/chats/new")?.body as Record<string, unknown>;
  assert.equal(body.model, "sonnet");
  assert.deepEqual(errors, []);
});

test("a start answer that fails (after the query's retry) enables the chat's send, to the server's own rule", async (t) => {
  const sent: Sent[] = [];
  let asked = 0;
  reset(sent, list([account(WORK, "work"), account(SPARE, "spare")], "unknown", WORK), () => {
    asked++;
    throw new Error("the device could not answer");
  });
  const { root, errors } = await mount(<ChatHome />, { retry: 1, retryDelay: 5 });
  t.after(() => root.unmount());
  await until("the composer", () => !!first((n) => n.nodeName === "TEXTAREA"));
  propsOf(first((n) => n.nodeName === "TEXTAREA")).onChange!({ target: { value: "hello" } });
  const send = () => first((n) => n.getAttribute("data-variant") === "primary");
  await until("send enabled", () => propsOf(send()).disabled !== true);
  assert.ok(asked >= 2, "asked, and asked again, before giving up");
  const body = await startChat(sent);
  assert.equal("accountId" in body, false, "no answer to name a login from: the server places it at the spawn");
  assert.deepEqual(errors, []);
});

test("the chat goes out on the answer asked again at the press, shown first", async (t) => {
  const sent: Sent[] = [];
  let moved = false;
  reset(
    sent,
    list([account(WORK, "work"), account(SPARE, "spare")], "unknown", WORK),
    () => (moved ? { accountId: SPARE, reason: "rotated", model: "sonnet", placed: true } : { accountId: WORK, reason: "default", model: null, placed: true }),
    { "/api/models": listFor },
  );
  const { root, errors } = await mount(<ChatHome />);
  t.after(() => root.unmount());
  await until("the first answer", () => /work/.test(accountChip()?.textContent ?? ""));
  moved = true;
  const body = await startChat(sent);
  assert.match(accountChip()?.textContent ?? "", /spare/, "the fresh answer is on screen");
  assert.equal(body.model, "sonnet");
  assert.equal("accountId" in body, false);
  assert.deepEqual(errors, []);
});

test("a pair no model-less start reaches is sent with its login named", async (t) => {
  const sent: Sent[] = [];
  reset(sent, list([account(WORK, "work"), account(SPARE, "spare")], "unknown", WORK),
    { accountId: SPARE, reason: "rotated", model: "fable", placed: false }, { "/api/models": listFor });
  const { root, errors } = await mount(<ChatHome />);
  t.after(() => root.unmount());
  await until("the pair", () => /spare/.test(accountChip()?.textContent ?? ""));
  const body = await startChat(sent);
  assert.equal(body.model, "fable");
  assert.equal(body.accountId, SPARE, "named, so it lands where it is shown");
  assert.equal(body.accountFrom, "start", "as the start rule's choice, which the server checks again");
  assert.deepEqual(errors, []);
});

test("the local login picked by hand while it is signed out is sent as the chat's own, and said", async (t) => {
  const sent: Sent[] = [];
  reset(sent, list([account(WORK, "work"), account(SPARE, "spare")], "signed_out"));
  const { root, errors } = await mount(<ChatHome />);
  t.after(() => root.unmount());
  await until("the stand-in", () => /work/.test(accountChip()?.textContent ?? ""));
  assert.equal(notice(), undefined);

  componentProps(accountChip(), "AccountPicker").onSelect!(null);
  await until("the warning", () => !!notice());
  assert.match(accountChip()!.textContent, /^local login \(not signed in\)$/);
  assert.match(notice()!.textContent, /signed out: run `claude` in a terminal on this device, then `\/login`/);
  const body = await startChat(sent);
  assert.equal(body.accountId, null, "the local login, as picked — not the stand-in");
  assert.deepEqual(errors, []);
});

const defaultRadios = (provider = "claude") =>
  all().filter((n) => n.nodeName === "INPUT" && (propsOf(n).name as unknown) === `rh-default-account-${provider}`);
const checkedRadios = (provider = "claude") => defaultRadios(provider).map((n) => propsOf(n).checked as unknown);

test("settings: automatic, always the local login, or an account — and automatic names its stand-in while the local login is signed out", async (t) => {
  const sent: Sent[] = [];
  reset(sent, list([gateway(GATEWAY, "gateway"), account(WORK, "work")], "signed_out"));
  const { root, errors } = await mount(<AccountsSection />);
  t.after(() => root.unmount());
  await until("the radios", () => defaultRadios().length === 4);
  assert.deepEqual(
    defaultRadios().map((n) => n.getAttribute("data-default-choice")),
    ["automatic", "local", null, null],
    "automatic, the local login, then the accounts",
  );
  assert.deepEqual(checkedRadios(), [true, false, false, false], "nobody chose: automatic");
  assert.match(
    notice()!.textContent,
    /your own claude login on this device is signed out, so new sessions that name no account start on work until you sign in again/,
    "the stand-in is the pool account, never the gateway listed first",
  );

  propsOf(defaultRadios()[1]).onChange!();
  await until("the choice", () => sent.some((s) => s.path === "/api/accounts/default"));
  assert.deepEqual(sent.filter((s) => s.path === "/api/accounts/default").at(-1)?.body, { accountId: null, provider: "claude" });
  propsOf(defaultRadios()[0]).onChange!();
  await until("back to automatic", () => sent.filter((s) => s.path === "/api/accounts/default").length === 2);
  assert.deepEqual(sent.filter((s) => s.path === "/api/accounts/default").at(-1)?.body, { accountId: null, provider: "claude", automatic: true });
  assert.deepEqual(errors, []);
});

test("settings: an env account offers edit, an OAuth one re-auth", async (t) => {
  const sent: Sent[] = [];
  // A gateway (env) beside an OAuth account: only the first has fields to edit.
  reset(sent, list([gateway(GATEWAY, "gateway"), account(WORK, "work")], "unknown"));
  const { root, errors } = await mount(<AccountsSection />);
  t.after(() => root.unmount());
  const edit = () =>
    first((n) => n.nodeName === "BUTTON" && n.getAttribute("title") === "edit this account's environment variables");
  await until("the edit button", () => !!edit());
  assert.match(edit()!.textContent, /^edit$/, "an env account is edited, not re-authorized");
  const reauth = () => first((n) => n.nodeName === "BUTTON" && n.getAttribute("title") === "replace this account's credential, keeping its sessions");
  assert.match(reauth()!.textContent, /^re-auth$/, "an OAuth account has a credential to replace");
  assert.deepEqual(errors, []);
});

test("settings: a local login somebody chose stays chosen while it is signed out, and says so", async (t) => {
  const sent: Sent[] = [];
  const chosen = list([account(WORK, "work")], "signed_out");
  chosen.local.claude = { ...chosen.local.claude, isDefault: true };
  reset(sent, chosen);
  const { root } = await mount(<AccountsSection />);
  t.after(() => root.unmount());
  await until("the radios", () => defaultRadios().length === 3);
  assert.deepEqual(checkedRadios(), [false, true, false]);
  assert.match(notice()!.textContent, /run `claude` in a terminal on this device, then `\/login`/);
  assert.doesNotMatch(notice()!.textContent, /start on/, "a choice, not a lapse: nothing stands in for it");
});

test("settings: a codex stand-in says how codex's own login comes back", async (t) => {
  const sent: Sent[] = [];
  const codex = list([{ ...account(SPARE, "codex key"), provider: "codex", kind: "env", rotation: true }], "unknown");
  codex.local.codex = { state: "signed_out", rest: [{ ...AUTH_REST, source: "codex:turn" }], isDefault: false };
  reset(sent, codex);
  const { root } = await mount(<AccountsSection />);
  t.after(() => root.unmount());
  await until("the codex note", () => /codex key/.test(notice()?.textContent ?? ""));
  assert.match(
    notice()!.textContent,
    /codex login on this device was refused, so new sessions that name no account start on codex key; after `codex login`, run one chat or session on the local login to switch back/,
  );
});

test("an older server: no automatic choice in settings, the old rule in the chat, and no account picker its chats would ignore", async (t) => {
  const sent: Sent[] = [];
  const old = { accounts: [account(WORK, "work"), account(SPARE, "spare")], defaultAccountId: null, defaultCodexAccountId: null,
    local: { claude: { state: "signed_out", rest: [AUTH_REST] }, codex: { state: "unknown", rest: [] } } };
  reset(sent, old);
  const settings = await mount(<AccountsSection />);
  await until("the radios", () => defaultRadios().length === 3);
  assert.deepEqual(checkedRadios(), [true, false, false], "the local login: what an older server starts on");
  settings.root.unmount();

  const chat = await mount(<ChatHome />);
  t.after(() => chat.root.unmount());
  await until("the warning", () => !!notice());
  assert.equal(accountChip(), undefined);
  assert.match(notice()!.textContent, /signed out: run `claude` in a terminal on this device/);
  await until("the models", () => modelQueries(sent).length > 0);
  assert.ok(modelQueries(sent).every((id) => id === null), "the local login's models: where an older server starts the chat");
});

/** The props of the nearest component above `node` that takes `prop`. */
function propsWith(node: FakeNode | undefined, prop: string): Record<string, (...args: unknown[]) => unknown> {
  assert.ok(node, "the element is on the page");
  const key = Object.keys(node).find((k) => k.startsWith("__reactFiber$"));
  assert.ok(key, "rendered by React");
  let fiber = (node as unknown as Record<string, { return?: unknown; memoizedProps?: Record<string, unknown> }>)[key];
  while (fiber && !(fiber.memoizedProps && prop in fiber.memoizedProps)) fiber = fiber.return as typeof fiber;
  assert.ok(fiber, `something above takes ${prop}`);
  return fiber.memoizedProps as Record<string, (...args: unknown[]) => unknown>;
}

test("switching the chat's backend while it waits for its answer drops the chat: it never goes to the other CLI", async (t) => {
  const sent: Sent[] = [];
  let release: () => void = () => undefined;
  let asked = 0;
  const held = new Promise<void>((resolve) => (release = resolve));
  reset(sent, list([account(WORK, "work"), account(SPARE, "spare")], "unknown", WORK), async () => {
    // The first answer comes at once; the one asked again at the press waits.
    if (++asked > 1) await held;
    return { accountId: WORK, reason: "default", model: null, placed: true };
  });
  const { root, errors } = await mount(<ChatHome />);
  t.after(() => {
    release();
    root.unmount();
  });
  await until("the answer", () => /work/.test(accountChip()?.textContent ?? ""));
  propsOf(first((n) => n.nodeName === "TEXTAREA")).onChange!({ target: { value: "for claude" } });
  await flush();
  const send = () => first((n) => n.getAttribute("data-variant") === "primary");
  propsOf(send()).onClick!();
  await until("the hold", () => /choosing account/.test(send()?.textContent ?? ""));

  propsWith(first((n) => n.nodeName === "BUTTON" && n.textContent === "codex"), "onValueChange").onValueChange!("codex");
  await flush();
  release();
  await flush(150);
  assert.equal(sent.some((s) => s.path === "/api/chats/new"), false, "pressed for claude, never sent as codex");
  assert.deepEqual(errors, []);
});

test("an answer that will not settle is asked again a few times at most: then the chat goes out for the server to place", async (t) => {
  const sent: Sent[] = [];
  let asked = 0;
  reset(
    sent,
    list([account(WORK, "work"), account(SPARE, "spare")], "unknown", WORK),
    // Every time it is asked, the rule names the other account: it never settles.
    () => (++asked % 2 ? { accountId: WORK, reason: "default", model: null, placed: false } : { accountId: SPARE, reason: "rotated", model: null, placed: false }),
    { "/api/models": listFor },
  );
  const { root, errors } = await mount(<ChatHome />);
  t.after(() => root.unmount());
  await until("an answer", () => /work|spare/.test(accountChip()?.textContent ?? ""));
  const before = asked;
  const body = await startChat(sent);
  assert.ok(asked - before <= 3, `asked again at most three times (${asked - before})`);
  assert.equal("accountId" in body, false, "no account named: the server places it");
  assert.equal("accountFrom" in body, false);
  assert.deepEqual(errors, []);
});

test("the last re-ask is awaited before a chat that will not settle goes out unplaced", async (t) => {
  const sent: Sent[] = [];
  let asked = 0;
  let sentBeforeLastAnswer = false;
  reset(
    sent,
    list([account(WORK, "work"), account(SPARE, "spare")], "unknown", WORK),
    async () => {
      const n = ++asked;
      // Slow enough that the page renders while each is being asked.
      await flush(60);
      if (sent.some((s) => s.path === "/api/chats/new")) sentBeforeLastAnswer = true;
      return n % 2 ? { accountId: WORK, reason: "default", model: null, placed: false } : { accountId: SPARE, reason: "rotated", model: null, placed: false };
    },
    { "/api/models": listFor },
  );
  const { root, errors } = await mount(<ChatHome />);
  t.after(() => root.unmount());
  await until("the composer", () => !!first((n) => n.nodeName === "TEXTAREA"));
  propsOf(first((n) => n.nodeName === "TEXTAREA")).onChange!({ target: { value: "hello" } });
  const sendButton = () => first((n) => n.getAttribute("data-variant") === "primary");
  // Sendable once the server's first answer is in.
  await until("the first answer", () => propsOf(sendButton()).disabled !== true);
  const before = asked;
  propsOf(sendButton()).onClick!();
  for (let i = 0; i < 100 && !sent.some((s) => s.path === "/api/chats/new"); i++) await flush(30);
  const body = sent.find((s) => s.path === "/api/chats/new")?.body as Record<string, unknown>;
  assert.ok(body, "it went out");
  assert.equal(asked - before, 3, "asked again three times");
  assert.equal(sentBeforeLastAnswer, false, "and not before the third answer came back");
  assert.equal("accountId" in body, false, "unplaced: the server places it");
  assert.deepEqual(errors, []);
});

test("emptying the chat's box while it is held calls it off; what was pressed is what goes out", async (t) => {
  const sent: Sent[] = [];
  let release: () => void = () => undefined;
  let gate = new Promise<void>((resolve) => (release = resolve));
  let asked = 0;
  reset(sent, list([account(WORK, "work"), account(SPARE, "spare")], "unknown", WORK), async () => {
    if (++asked > 1) await gate;
    return { accountId: WORK, reason: "default", model: null, placed: true };
  });
  const { root, errors } = await mount(<ChatHome />);
  t.after(() => {
    release();
    root.unmount();
  });
  await until("the answer", () => /work/.test(accountChip()?.textContent ?? ""));
  const box = () => first((n) => n.nodeName === "TEXTAREA");
  const send = () => first((n) => n.getAttribute("data-variant") === "primary");
  propsOf(box()).onChange!({ target: { value: "first" } });
  await flush();
  propsOf(send()).onClick!();
  await until("the hold", () => /choosing account/.test(send()?.textContent ?? ""));
  propsOf(box()).onChange!({ target: { value: "" } });
  await flush();
  propsOf(box()).onChange!({ target: { value: "y" } });
  release();
  await flush(150);
  assert.equal(sent.some((s) => s.path === "/api/chats/new"), false, "emptied: called off");

  // Pressed again, then edited while held: what was pressed goes out.
  gate = new Promise<void>((resolve) => (release = resolve));
  propsOf(box()).onChange!({ target: { value: "as pressed" } });
  await flush();
  propsOf(send()).onClick!();
  await until("the hold", () => /choosing account/.test(send()?.textContent ?? ""));
  propsOf(box()).onChange!({ target: { value: "as pressed, then more" } });
  await flush();
  release();
  await until("the start", () => sent.some((s) => s.path === "/api/chats/new"));
  assert.equal((sent.find((s) => s.path === "/api/chats/new")?.body as Record<string, unknown>).text, "as pressed");
  await flush();
  assert.equal(propsOf(box()).value as unknown, "as pressed, then more", "what was typed after the press stays in the box");
  assert.deepEqual(errors, []);
});
