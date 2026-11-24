// Settings › A2A (`features/settings/a2a-section.tsx`), driven in a real DOM
// against a recording device: a new token's secret is shown once — with a
// copy button and "you won't see it again" — and is gone from the page, the
// list and the query client once dismissed; revoking asks first and says the
// unfinished tasks are cancelled; a new entry starts off, in a worktree, with
// edits accepted and nothing bypassed, one task at a time, questions
// answerable over A2A; an entry other devices may call refuses a bypass
// mode; and every entry's Agent Card URL is this page's origin plus
// `/a2a/agents/<id>`.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after, afterEach } from "node:test";
import { installJsdom } from "./jsdom.js";

const { window, document } = installJsdom();
(globalThis as Record<symbol, unknown>)[Symbol.for("undici.globalOrigin.1")] = new URL("http://device.test");
after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});

const { act } = await import("react");
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
const { A2aSection } = await import("../src/features/settings/a2a-section.js");
type Transport = import("../src/lib/transport/index.js").Transport;

interface Sent {
  method: string;
  path: string;
  body: unknown;
}

const SECRET = "dpa_n4x8w2_c3ludGhldGljLW5vdC1hLXJlYWwtc2VjcmV0LWF0LWFsbA";
const NOW = new Date().toISOString();

const entry = (over: Record<string, unknown>) => ({
  description: "",
  enabled: true,
  source: "claude",
  acpAgent: null,
  account: { kind: "default" },
  model: null,
  effort: null,
  permission: "acceptEdits",
  isolation: "worktree",
  maxConcurrent: 1,
  template: null,
  allowRemoteAnswers: true,
  pushOrigins: [],
  allowedDevices: [],
  createdAt: NOW,
  updatedAt: NOW,
  ...over,
});

type DeviceState = { entries: unknown[]; tokens: unknown[] };

/** The device: the A2A management API, and what the entry form's pickers ask, recording each request. */
function device(sent: Sent[], state: DeviceState): Transport {
  return {
    fetch: async (request: Request) => {
      const url = new URL(request.url);
      const body = request.method === "GET" || request.method === "DELETE" ? null : await request.json().catch(() => null);
      sent.push({ method: request.method, path: url.pathname, body });
      if (url.pathname === "/api/a2a/entries" && request.method === "GET") return Response.json({ entries: state.entries, defaultTemplate: "{{request}}" });
      if (url.pathname === "/api/a2a/entries" && request.method === "POST") {
        const made = entry(body as Record<string, unknown>);
        state.entries = [...state.entries, made];
        return Response.json(made);
      }
      if (url.pathname === "/api/a2a/tokens" && request.method === "GET") return Response.json({ tokens: state.tokens });
      if (url.pathname === "/api/a2a/tokens" && request.method === "POST") {
        const { name, entries } = body as { name: string; entries: string[] };
        const token = { id: "n4x8w2", name, entries, createdAt: NOW, lastUsedAt: null, revokedAt: null };
        state.tokens = [token, ...state.tokens];
        return Response.json({ token, secret: SECRET });
      }
      const revoke = /^\/api\/a2a\/tokens\/([^/]+)$/.exec(url.pathname);
      if (revoke && request.method === "DELETE") {
        state.tokens = state.tokens.map((t) => ((t as { id: string }).id === revoke[1] ? { ...(t as object), revokedAt: NOW } : t));
        return Response.json({ accepted: true });
      }
      if (url.pathname === "/api/a2a/tasks") return Response.json({ tasks: [] });
      if (url.pathname === "/api/accounts") return Response.json({ accounts: [], defaultAccountId: null, defaultCodexAccountId: null });
      if (url.pathname === "/api/acp/agents") return Response.json({ agents: [], presets: [] });
      if (url.pathname === "/api/models") return Response.json({ models: [{ id: "fable", label: null, efforts: ["low", "medium", "high"], defaultEffort: "medium" }], source: "cache" });
      return Response.json({ error: "not here" }, { status: 404 });
    },
    socketUrl: async (path) => path,
    openSocket: () => ({ readyState: 0, binaryType: "blob", send() {}, close() {}, onopen: null, onmessage: null, onclose: null }),
  };
}

let root: ReturnType<typeof createRoot> | null = null;
const container = document.body.appendChild(document.createElement("div"));
afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  document.body.querySelectorAll("[data-base-ui-portal], [data-base-ui-floating-portal]").forEach((n) => n.remove());
});

async function mount(sent: Sent[], state: DeviceState) {
  registry.setShell("local", LOCAL);
  registry.openMachine(LOCAL, device(sent, state), {
    info: constantStore({ key: LOCAL, name: "studio", state: "online" as const, since: 0, canManage: true, live: "live" as const }),
    ensureLive: () => undefined,
    hold: () => () => undefined,
    dispose: () => undefined,
  });
  const rootRoute = router.createRootRoute({ component: () => <router.Outlet /> });
  const settings = router.createRoute({ getParentRoute: () => rootRoute, path: "settings", component: () => <A2aSection /> });
  const sessions = router.createRoute({ getParentRoute: () => rootRoute, path: "sessions/$source/$id", component: () => null });
  const routes = router.createRouter({
    routeTree: rootRoute.addChildren([settings, sessions]),
    history: router.createMemoryHistory({ initialEntries: ["/settings"] }),
  });
  await routes.load();
  const queries = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  root = createRoot(container);
  await act(async () =>
    root!.render(
      <QueryClientProvider client={queries}>
        <MachineProvider machine={LOCAL}>
          <router.RouterProvider router={routes} />
        </MachineProvider>
      </QueryClientProvider>,
    ),
  );
  await settle();
  return queries;
}

const settle = (ms = 30) => act(async () => new Promise((resolve) => setTimeout(resolve, ms)));
async function until(what: string, check: () => boolean) {
  for (let i = 0; i < 100; i++) {
    if (check()) return;
    await settle(20);
  }
  assert.fail(`timed out waiting for ${what}`);
}
const $ = (selector: string) => document.querySelector<HTMLElement>(selector);
const byText = (selector: string, text: string) => [...document.querySelectorAll<HTMLElement>(selector)].find((el) => el.textContent?.trim() === text) ?? null;
const click = (el: HTMLElement | null) =>
  act(async () => {
    assert.ok(el, "the control is on the page");
    el.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
  });
/** Type into a controlled field the way a browser does: its value, then an `input` event. */
const type = (el: HTMLElement | null, value: string) =>
  act(async () => {
    assert.ok(el, "the field is on the page");
    const proto = el instanceof window.HTMLTextAreaElement ? window.HTMLTextAreaElement.prototype : window.HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, "value")!.set!.call(el, value);
    el.dispatchEvent(new window.Event("input", { bubbles: true }));
  });

const DONT_DO = entry({ id: "dont-do", name: "dont-do tasks", cwd: "/home/studio/code" });
const LIVE_TOKEN = { id: "k3f9q2", name: "planner", entries: ["dont-do"], createdAt: NOW, lastUsedAt: null, revokedAt: null };

test("a new token's secret is shown once, with a copy button, and is gone from the page and the cache once dismissed", async () => {
  const sent: Sent[] = [];
  const queries = await mount(sent, { entries: [DONT_DO], tokens: [] });
  await until("the entries", () => !!$('[data-a2a-entry="dont-do"]'));
  await click(byText("[data-a2a-tokens] button", "new token"));
  await type($("[data-a2a-token-form] input"), "dont-do");
  // The only entry is ticked already; a second click would untick it.
  assert.equal($("[data-a2a-token-form] [role=checkbox]")?.getAttribute("aria-checked"), "true");
  await click(byText("[data-a2a-token-form] button", "create token"));
  await until("the secret", () => !!$("[data-a2a-secret]"));

  assert.deepEqual(sent.find((s) => s.method === "POST" && s.path === "/api/a2a/tokens")?.body, { name: "dont-do", entries: ["dont-do"] });
  assert.equal(($("[data-a2a-secret] input") as HTMLInputElement | null)?.value, SECRET);
  assert.match($("[data-a2a-secret]")!.textContent!, /Copy the token for dont-do now: you won't see it again\./);
  assert.ok(byText("[data-a2a-secret] button", "copy"), "a copy button beside it");
  assert.equal($("[data-a2a-token-form]"), null, "the form is closed");
  await until("the new token listed", () => !!$('[data-a2a-token="n4x8w2"]'));
  assert.ok(!$('[data-a2a-token="n4x8w2"]')!.textContent!.includes(SECRET), "the list never shows it");

  // Nothing but this one component holds it: not a query, not a mutation.
  const cached = JSON.stringify([
    queries.getQueryCache().getAll().map((q) => q.state.data),
    queries.getMutationCache().getAll().map((m) => m.state.data),
  ]);
  assert.ok(!cached.includes(SECRET), "the query client never had it");

  await click(byText("[data-a2a-secret] button", "done"));
  assert.equal($("[data-a2a-secret]"), null);
  assert.ok(!document.body.innerHTML.includes(SECRET), "gone from the page");
  assert.ok(!document.body.innerHTML.includes("dpa_"), "no part of it left");
});

test("revoking asks first, and says the token's unfinished tasks are cancelled", async () => {
  const sent: Sent[] = [];
  await mount(sent, { entries: [DONT_DO], tokens: [LIVE_TOKEN] });
  await until("the token", () => !!$('[data-a2a-token="k3f9q2"]'));
  await click(byText('[data-a2a-token="k3f9q2"] button', "revoke…"));
  assert.equal(sent.filter((s) => s.method === "DELETE").length, 0, "the first click revokes nothing");
  assert.match($("[data-a2a-revoke-confirm]")!.textContent!, /Revoke planner\? Its unfinished tasks are cancelled/);
  await click(byText("[data-a2a-revoke-confirm] button", "revoke"));
  await until("the revoke", () => sent.some((s) => s.method === "DELETE" && s.path === "/api/a2a/tokens/k3f9q2"));
  await until("it moves under the revoked ones", () => !!$('[data-a2a-token="k3f9q2"][data-revoked]'));
});

test("a new entry starts in a worktree, accepting edits, one task at a time, answerable over A2A — and is saved switched off", async () => {
  const sent: Sent[] = [];
  await mount(sent, { entries: [], tokens: [] });
  await until("the empty list", () => /no entries yet/.test($("[data-a2a-entries]")?.textContent ?? ""));
  assert.equal(byText("[data-a2a-tokens] button", "new token")?.hasAttribute("disabled"), true, "no token before an entry");
  await click(byText("[data-a2a-entries] button", "add entry"));
  const form = () => $('[data-a2a-entry-form=""]')!;
  await until("the form", () => !!form());

  assert.equal(form().querySelector('[role="tab"][aria-selected="true"]')?.textContent, "worktree");
  const selects = [...form().querySelectorAll<HTMLElement>('[role="combobox"]')].map((s) => s.getAttribute("aria-label") + "=" + s.textContent?.trim());
  assert.ok(selects.includes("permission=acceptEdits"), selects.join(", "));
  assert.ok(selects.includes("tasks at once=1"), selects.join(", "));
  assert.ok(selects.includes("agent=Claude Code"), selects.join(", "));
  assert.equal(form().querySelector('[role="checkbox"]')?.getAttribute("aria-checked"), "true", "questions answerable over A2A");
  const save = () => byText('[data-a2a-entry-form=""] button', "add entry")!;
  assert.ok(save().hasAttribute("disabled"), "nothing to save yet");

  await type(form().querySelector<HTMLElement>("input"), "Dont Do");
  assert.equal(form().querySelectorAll<HTMLInputElement>("input")[1]!.value, "dont-do", "the id follows the name");
  await type(form().querySelector<HTMLElement>('input[placeholder="/path/to/project"]'), "/home/studio/code");
  await until("a complete form", () => !save().hasAttribute("disabled"));
  await click(save());
  await until("the create", () => sent.some((s) => s.method === "POST" && s.path === "/api/a2a/entries"));
  const body = sent.find((s) => s.method === "POST" && s.path === "/api/a2a/entries")!.body as Record<string, unknown>;
  assert.deepEqual(
    { enabled: body.enabled, isolation: body.isolation, permission: body.permission, maxConcurrent: body.maxConcurrent, allowRemoteAnswers: body.allowRemoteAnswers, id: body.id, cwd: body.cwd },
    { enabled: false, isolation: "worktree", permission: "acceptEdits", maxConcurrent: 1, allowRemoteAnswers: true, id: "dont-do", cwd: "/home/studio/code" },
  );
});

/** Open the select named `label` in `scope`, and read its options. */
async function optionsOf(scope: HTMLElement, label: string): Promise<(string | null)[]> {
  const trigger = [...scope.querySelectorAll<HTMLElement>('[role="combobox"]')].find((s) => s.getAttribute("aria-label") === label);
  assert.ok(trigger, `a ${label} select`);
  await act(async () => {
    trigger.dispatchEvent(new window.PointerEvent("pointerdown", { bubbles: true, cancelable: true, pointerType: "mouse", button: 0 }));
    trigger.dispatchEvent(new window.MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 }));
    trigger.dispatchEvent(new window.PointerEvent("pointerup", { bubbles: true, cancelable: true, pointerType: "mouse", button: 0 }));
    trigger.dispatchEvent(new window.MouseEvent("mouseup", { bubbles: true, cancelable: true, button: 0 }));
    trigger.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
  });
  const open = () => [...document.querySelectorAll<HTMLElement>('[role="listbox"]')].find((l) => !l.closest("[hidden]"));
  await until("the list", () => !!open());
  return [...open()!.querySelectorAll('[role="option"]')].map((o) => o.textContent);
}

test("an entry other devices may call is offered no bypass mode", async () => {
  const sent: Sent[] = [];
  const called = entry({ id: "remote", name: "Remote", cwd: "/home/studio/code", allowedDevices: ["d-7Kq2"] });
  await mount(sent, { entries: [called], tokens: [] });
  await until("the entry", () => !!$('[data-a2a-entry="remote"]'));
  await click($('[data-a2a-entry="remote"] button[aria-label="edit entry"]'));
  const form = () => $('[data-a2a-entry-form="remote"]')!;
  await until("the form", () => !!form());
  assert.match(form().querySelector("[data-a2a-allowed-devices]")!.textContent!, /d-7Kq2/, "the devices, read-only");
  assert.deepEqual(await optionsOf(form(), "permission"), ["CLI default", "acceptEdits", "auto", "plan", "dontAsk"]);
});

test("an entry other devices may call that holds a bypass mode shows it, and cannot be saved with it", async () => {
  const sent: Sent[] = [];
  const called = entry({ id: "remote", name: "Remote", cwd: "/home/studio/code", permission: "bypassPermissions", allowedDevices: ["d-7Kq2"] });
  await mount(sent, { entries: [called], tokens: [] });
  await until("the entry", () => !!$('[data-a2a-entry="remote"]'));
  await click($('[data-a2a-entry="remote"] button[aria-label="edit entry"]'));
  const form = () => $('[data-a2a-entry-form="remote"]')!;
  await until("the form", () => !!form());
  assert.match(form().querySelector("[data-a2a-problems]")!.textContent!, /a mode that bypasses permissions cannot be used while other devices may call this entry/);
  assert.ok(byText('[data-a2a-entry-form="remote"] button', "save")!.hasAttribute("disabled"));
  // What is saved is shown as such; no other bypass mode is offered beside it.
  assert.deepEqual(await optionsOf(form(), "permission"), ["CLI default", "acceptEdits", "auto", "plan", "dontAsk", "bypassPermissions"]);
});

test("an entry no other device calls is offered every mode, the bypass included", async () => {
  const sent: Sent[] = [];
  await mount(sent, { entries: [DONT_DO], tokens: [] });
  await until("the entry", () => !!$('[data-a2a-entry="dont-do"]'));
  await click($('[data-a2a-entry="dont-do"] button[aria-label="edit entry"]'));
  await until("the form", () => !!$('[data-a2a-entry-form="dont-do"]'));
  assert.equal($('[data-a2a-entry-form="dont-do"] [data-a2a-allowed-devices]'), null, "no devices, no line for them");
  assert.deepEqual(await optionsOf($('[data-a2a-entry-form="dont-do"]')!, "permission"), ["CLI default", "acceptEdits", "auto", "plan", "dontAsk", "bypassPermissions"]);
});

test("each entry's Agent Card URL is this page's origin and /a2a/agents/<id>, to copy", async () => {
  const sent: Sent[] = [];
  await mount(sent, { entries: [DONT_DO, entry({ id: "lab", name: "Lab", source: "acp", acpAgent: "bub", cwd: "/home/studio/lab", permission: null })], tokens: [] });
  await until("the entries", () => document.querySelectorAll("[data-a2a-entry]").length === 2);
  const urls = [...document.querySelectorAll("[data-a2a-card-url]")].map((e) => e.getAttribute("data-a2a-card-url"));
  assert.deepEqual(urls, [`${window.location.origin}/a2a/agents/dont-do`, `${window.location.origin}/a2a/agents/lab`]);
  assert.ok($('[data-a2a-entry="dont-do"] button[aria-label="copy the Agent Card URL"]'));
});
