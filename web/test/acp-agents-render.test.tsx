// Settings › Agents & accounts › ACP, driven in a real DOM against a recording
// device: agents as cards, a failing one in red and counted on its tab; the
// details dialog the address opens (`&agent=<id>`), with the check's error read
// from the check's own route — or, for a device that does not manage the
// computer, a line saying so — and a removal that a running turn refuses; and,
// with no agent yet, the presets as one-click starts whose dialog stays open on
// a failed check.
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
const { AgentsSection } = await import("../src/features/settings/agents/agents-section.js");
type Transport = import("../src/lib/transport/index.js").Transport;

interface Sent {
  method: string;
  path: string;
  body: unknown;
}

const READY = { checkedAt: new Date(Date.now() - 2 * 60_000).toISOString(), state: "ready", resolvedPath: "/usr/bin/bub", protocolVersion: 1 };
const NOT_FOUND = {
  checkedAt: new Date(Date.now() - 5 * 60_000).toISOString(),
  state: "not_found",
  error: "lab-agent: no such executable\nPATH=/usr/local/bin:/usr/bin",
};
/** A check as the list carries it: how it ended, not why. */
const listed = ({ error: _error, ...check }: { error?: string } & Record<string, unknown>) => check;
const PRESETS = [
  { id: "bub", name: "Bub", command: "bub", args: ["acp"], onPath: false },
  { id: "opencode", name: "OpenCode", command: "opencode", args: ["acp"], onPath: true, homepage: "https://opencode.ai/docs/acp/" },
];
const agent = (id: string, name: string, lastCheck?: unknown) => ({
  id, name, command: id, args: ["--acp"], envNames: ["API_TOKEN"], ...(lastCheck ? { lastCheck } : {}),
});

type DeviceState = {
  agents: unknown[];
  remove?: number;
  /** What a check run now answers, or that it is refused. */
  check?: unknown;
  checkRefused?: boolean;
  /** Each agent's last check in full, or that this device may not read it. */
  full?: Record<string, unknown>;
  fullRefused?: boolean;
};

/** The device: its agent list, and how it answers a removal and a check. */
function device(sent: Sent[], state: DeviceState): Transport {
  return {
    fetch: async (request: Request) => {
      const url = new URL(request.url);
      const body = request.method === "GET" || request.method === "DELETE" ? null : await request.json().catch(() => null);
      sent.push({ method: request.method, path: url.pathname, body });
      if (url.pathname === "/api/acp/agents" && request.method === "GET") return Response.json({ agents: state.agents, presets: PRESETS });
      const id = /^\/api\/acp\/agents\/([^/]+)/.exec(url.pathname)?.[1];
      if (id && url.pathname.endsWith("/check") && request.method === "GET") {
        if (state.fullRefused) return Response.json({ error: "limited_device", message: "this device cannot manage the account" }, { status: 403 });
        const full = state.full?.[id];
        return full ? Response.json(full) : Response.json({ error: "this ACP agent has not been checked" }, { status: 404 });
      }
      if (id && url.pathname.endsWith("/check")) {
        return state.checkRefused
          ? Response.json({ error: "limited_device", message: "this device cannot manage the account" }, { status: 403 })
          : Response.json(state.check ?? READY);
      }
      if (id && request.method === "PUT") {
        const saved = { ...agent(id, (body as { name?: string }).name ?? id), command: (body as { command?: string }).command ?? id };
        state.agents = [...state.agents, saved];
        return Response.json(saved);
      }
      if (id && request.method === "DELETE") {
        return state.remove === 409
          ? Response.json({ error: "Lab agent is running a turn; stop it, or let it finish, then remove the agent" }, { status: 409 })
          : Response.json({ accepted: true });
      }
      if (url.pathname === "/api/accounts") return Response.json({ accounts: [], defaultAccountId: null, defaultCodexAccountId: null });
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
});

async function mount(sent: Sent[], state: DeviceState, address: string) {
  registry.setShell("local", LOCAL);
  registry.openMachine(LOCAL, device(sent, state), {
    info: constantStore({ key: LOCAL, name: "studio", state: "online" as const, since: 0, canManage: true, live: "live" as const }),
    ensureLive: () => undefined,
    hold: () => () => undefined,
    dispose: () => undefined,
  });
  const rootRoute = router.createRootRoute({ component: () => <router.Outlet /> });
  const settings = router.createRoute({ getParentRoute: () => rootRoute, path: "settings", component: () => <AgentsSection /> });
  const routes = router.createRouter({
    routeTree: rootRoute.addChildren([settings]),
    history: router.createMemoryHistory({ initialEntries: [address] }),
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
  return routes;
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

test("the ACP tab shows each agent as a card, a failing one in red and counted on the tab", async () => {
  const sent: Sent[] = [];
  await mount(sent, { agents: [agent("bub", "Bub", READY), agent("lab-agent", "Lab agent", listed(NOT_FOUND)), agent("new", "New one")] }, "/settings?section=accounts&tab=acp");
  await until("the cards", () => document.querySelectorAll("[data-acp-card]").length === 3);
  assert.match($('[data-acp-card="bub"]')!.textContent!, /Bub.*bub --acp.*API_TOKEN.*ready to run.*checked 2 min ago/);
  const failing = $('[data-acp-card="lab-agent"] [data-signal="failed"]');
  assert.equal(failing?.textContent, "command not found");
  assert.match($('[data-acp-card="new"]')!.textContent!, /not checked yet/);
  assert.ok($("[data-add-card]"), "the grid ends with the add card");
  const tab = $('[data-tab="acp"]');
  assert.equal(tab?.getAttribute("aria-selected"), "true", "the address names the tab");
  assert.equal(tab?.querySelector('[data-signal="failed"]')?.textContent, "1", "one agent cannot run");
  assert.equal(localStorage.getItem("rh.settings.agents-tab"), "acp", "the tab this browser used last");
});

test("the details the address names show the check's error, and a running turn refuses the removal", async () => {
  const sent: Sent[] = [];
  const state = { agents: [agent("lab-agent", "Lab agent", listed(NOT_FOUND))], full: { "lab-agent": NOT_FOUND }, remove: 409 };
  const routes = await mount(sent, state, "/settings?section=accounts&tab=acp&agent=lab-agent");
  await until("the details", () => !!$('[data-acp-details="lab-agent"]'));
  await until("the error", () => !!$('[data-acp-details] pre[data-signal="failed"]'));
  assert.match($('[data-acp-details] pre[data-signal="failed"]')!.textContent!, /no such executable[\s\S]*PATH=/);
  assert.ok(sent.some((s) => s.method === "GET" && s.path === "/api/acp/agents/lab-agent/check"), "read from the check's own route");

  await click(byText("[data-acp-details] button", "delete…"));
  assert.match($("[data-acp-remove-confirm]")!.textContent!, /Delete Lab agent\? Its 0 sessions stay readable/);
  await click(byText("[data-acp-remove-confirm] button", "delete"));
  await until("the refusal", () => /running a turn/.test($("[data-acp-details]")?.textContent ?? ""));
  assert.ok(sent.some((s) => s.method === "DELETE" && s.path === "/api/acp/agents/lab-agent"));
  assert.ok($("[data-acp-details]"), "the dialog stays, saying why");

  await click(byText("[data-acp-remove-confirm] button", "cancel"));
  await click(byText("[data-acp-details] button", "check again"));
  await until("the check", () => sent.some((s) => s.method === "POST" && s.path === "/api/acp/agents/lab-agent/check"));
  assert.equal(new URLSearchParams(routes.state.location.searchStr).get("agent"), "lab-agent");
});

test("a check the device refuses to run says why in the details", async () => {
  const sent: Sent[] = [];
  await mount(sent, { agents: [agent("lab-agent", "Lab agent", listed(NOT_FOUND))], full: { "lab-agent": NOT_FOUND }, checkRefused: true }, "/settings?section=accounts&tab=acp&agent=lab-agent");
  await until("the details", () => !!$('[data-acp-details="lab-agent"]'));
  await click(byText("[data-acp-details] button", "check again"));
  await until("the refusal", () => /a limited device cannot do this/.test($('[data-acp-details] p[data-signal="failed"]')?.textContent ?? ""));
});

test("a device that does not manage the computer is told why the error is not shown", async () => {
  const sent: Sent[] = [];
  await mount(sent, { agents: [agent("lab-agent", "Lab agent", listed(NOT_FOUND))], fullRefused: true }, "/settings?section=accounts&tab=acp&agent=lab-agent");
  await until("the details", () => !!$('[data-acp-details="lab-agent"]'));
  await until("the line", () => !!$("[data-acp-withheld]"));
  assert.equal($("[data-acp-withheld]")!.textContent, "only a full device can see why it failed");
  assert.match($("[data-acp-details]")!.textContent!, /command not found/, "how it ended is still there");
  assert.equal($('[data-acp-details] pre[data-signal="failed"]'), null, "no error well");
  assert.equal($('[data-acp-details] p[data-signal="failed"]'), null, "and no error: the refusal is expected");
});

test("a model list that leaves an agent's models out says how many, and why", async () => {
  const { ModelSourceHint } = await import("../src/features/sessions/composer-controls.js");
  root = createRoot(container);
  await act(async () => root!.render(<ModelSourceHint fallback={false} hidden={2} />));
  assert.equal($('[data-signal="warn"]')?.getAttribute("title"), "2 of the agent's models are not shown: their ids look like credentials");
  await act(async () => root!.render(<ModelSourceHint fallback={false} />));
  assert.equal($('[data-signal="warn"]'), null, "nothing left out, nothing said");
});

test("with no agent, a preset starts the add dialog filled in, which stays open on a failed check", async () => {
  const sent: Sent[] = [];
  const state = { agents: [] as unknown[], check: NOT_FOUND };
  await mount(sent, state, "/settings?section=accounts&tab=acp");
  await until("the presets", () => document.querySelectorAll("[data-acp-preset]").length === 2);
  assert.match($('[data-acp-preset="opencode"]')!.textContent!, /OpenCode.*opencode acp.*on PATH/);
  assert.match($('[data-acp-preset="bub"]')!.textContent!, /not on PATH/);

  await click($('[data-acp-preset="opencode"] button'));
  await until("the form", () => !!$('[data-acp-form="new"]'));
  const command = $('[data-acp-form] input.font-mono') as HTMLInputElement;
  assert.equal(command.value, "opencode", "the preset's command, editable");
  await click(byText('[data-acp-form] button', "add and check"));
  await until("the check", () => sent.some((s) => s.path === "/api/acp/agents/opencode/check"));
  const put = sent.find((s) => s.method === "PUT");
  assert.equal(put?.path, "/api/acp/agents/opencode", "a preset's agent keeps its id");
  assert.deepEqual(put?.body, { name: "OpenCode", command: "opencode", args: ["acp"] });
  await until("the error", () => !!$('[data-acp-check] pre[data-signal="failed"]'));
  assert.ok($("[data-acp-form]"), "a failed check keeps the dialog open, with the error");
});
