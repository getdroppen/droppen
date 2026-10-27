// Step 01 mounted for real (`features/setup/tools-step.tsx`): React renders
// into a stand-in DOM (`fake-dom.ts`), the device answers through a recording
// transport, and what is pinned is what a person sees and what the device is
// sent — a sign-in that lands turns the card ready with its toast, a finished
// install turns into the next state, a task still running when the guide
// reopens shows its terminal again, F6 moves the focus between the terminal
// and the action bar (never on its own), and an API key goes in one request
// body and is kept nowhere.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after } from "node:test";
import { register } from "node:module";
import { installFakeDom, type FakeNode } from "./fake-dom.js";
import { commands, quota, report, task, tool } from "./setup-fixtures.js";

// The terminal view imports xterm, which needs a browser: the same stand-in
// this file has always used.
register(
  `data:text/javascript,${encodeURIComponent(
    `export async function resolve(specifier, context, next) { if (["xterm-pane", "xterm-pane.tsx", "xterm-pane.js"].includes(specifier.split("/").pop())) return { url: "data:text/javascript,export function XtermPane() { return null; }", shortCircuit: true }; return next(specifier, context); }`,
  )}`,
);

const container = installFakeDom();
(globalThis as Record<symbol, unknown>)[Symbol.for("undici.globalOrigin.1")] = new URL("http://device.test");
// The page's own listeners, kept so a test can press a key on the page.
const windowListeners: { type: string; fn: (event: unknown) => void; capture: boolean }[] = [];
Object.assign(window as object, {
  addEventListener: (type: string, fn: (event: unknown) => void, options?: boolean | { capture?: boolean }) =>
    void windowListeners.push({ type, fn, capture: options === true || (typeof options === "object" && options?.capture === true) }),
  removeEventListener: (type: string, fn: (event: unknown) => void) => {
    const at = windowListeners.findIndex((l) => l.type === type && l.fn === fn);
    if (at >= 0) windowListeners.splice(at, 1);
  },
});
// The terminal itself needs a browser (xterm): a pane that draws nothing stands in, inside the real frame around it.
after(() => void setTimeout(() => process.exit(), 50));
const flush = (ms = 30) => new Promise((resolve) => setTimeout(resolve, ms));

const i18next = (await import("i18next")).default;
const { initReactI18next } = await import("react-i18next");
await i18next.use(initReactI18next).init({ lng: "en", resources: {}, keySeparator: false, nsSeparator: false, interpolation: { escapeValue: false } });
const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
const { MachineProvider } = await import("../src/lib/machines/context.js");
const { LOCAL } = await import("../src/lib/machines/key.js");
const registry = await import("../src/lib/machines/registry.js");
const { constantStore } = await import("../src/lib/machines/store.js");
const { useSetupTools } = await import("../src/features/setup/api.js");
const { ToolsStep, SIGNED_IN_TOAST } = await import("../src/features/setup/tools-step.js");
const { IDLE_TOOLS_UI } = await import("../src/features/setup/tools-model.js");
const { dismissSetupToast, setupToastNow } = await import("../src/features/setup/toast.js");
const { emptyProgress } = await import("../src/features/setup/model.js");
type ToolsReport = import("../src/features/setup/types.js").ToolsReport;
type ToolsUi = import("../src/features/setup/tools-model.js").ToolsUi;
type Transport = import("../src/lib/transport/index.js").Transport;

interface Sent {
  method: string;
  path: string;
  body: unknown;
}

/** A device whose `/api/setup/tools` is whatever `now()` says, and that records every request. */
function device(sent: Sent[], now: () => ToolsReport, extra: Record<string, () => unknown> = {}): Transport {
  return {
    fetch: async (request: Request) => {
      const url = new URL(request.url);
      const body = request.method === "GET" || request.method === "DELETE" ? null : await request.json().catch(() => null);
      sent.push({ method: request.method, path: url.pathname, body });
      if (url.pathname === "/api/setup/tools") return Response.json(now());
      if (url.pathname === "/api/setup/commands") return Response.json({ commands: commands() });
      if (url.pathname === "/api/stats/quota") return Response.json(quota());
      const answer = extra[`${request.method} ${url.pathname}`];
      return answer ? Response.json(answer()) : Response.json({ error: "not here" }, { status: 404 });
    },
    socketUrl: async (path) => path,
    openSocket: () => ({ readyState: 0, binaryType: "blob", send() {}, close() {}, onopen: null, onmessage: null, onclose: null }),
  };
}

function open(transport: Transport) {
  registry.setShell("local", LOCAL);
  registry.openMachine(LOCAL, transport, {
    info: constantStore({ key: LOCAL, name: "studio", state: "online" as const, since: 0, canManage: true, live: "live" as const }),
    ensureLive: () => undefined,
    hold: () => () => undefined,
    dispose: () => undefined,
  });
}

/** Step 01 as the guide holds it: the report from the cache, the opened forms in the guide's state. */
function Guide({ onUi }: { onUi?: (ui: ToolsUi) => void }) {
  const tools = useSetupTools();
  const [ui, setUi] = React.useState<ToolsUi>(IDLE_TOOLS_UI);
  return (
    <ToolsStep
      progress={emptyProgress()}
      tools={tools.isPending ? undefined : (tools.data ?? null)}
      online
      onChange={() => undefined}
      toolsUi={ui}
      onToolsUi={(next) => {
        onUi?.(next);
        setUi(next);
      }}
    />
  );
}

async function mount(onUi?: (ui: ToolsUi) => void) {
  const queries = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const root = createRoot(container as never);
  root.render(
    <QueryClientProvider client={queries}>
      <MachineProvider machine={LOCAL}>
        <Guide onUi={onUi} />
      </MachineProvider>
    </QueryClientProvider>,
  );
  await flush(60);
  return { root, queries };
}

function walk(node: FakeNode, out: FakeNode[] = []): FakeNode[] {
  for (const child of node.childNodes) {
    out.push(child);
    walk(child, out);
  }
  return out;
}
const all = () => walk(container as FakeNode);
const find = (match: (node: FakeNode) => boolean) => all().find(match);
const byTestId = (id: string, within?: FakeNode) => (within ? walk(within) : all()).find((n) => n.getAttribute("data-testid") === id);
const cardOf = (id: string) => find((n) => n.getAttribute("data-tool") === id);
const propsOf = (node: FakeNode | undefined): Record<string, (...args: unknown[]) => unknown> => {
  assert.ok(node, "the element is on the page");
  const key = Object.keys(node).find((k) => k.startsWith("__reactProps$"));
  assert.ok(key, "rendered by React");
  return (node as unknown as Record<string, Record<string, (...args: unknown[]) => unknown>>)[key];
};
async function until(check: () => boolean, ms = 5000) {
  const end = Date.now() + ms;
  while (!check()) {
    if (Date.now() > end) return false;
    await flush(50);
  }
  return true;
}

test("a sign-in that lands: the card turns ready, and the toast says so", async (t) => {
  const sent: Sent[] = [];
  let signedIn = false;
  const signingIn = () => report(tool("claude", "signedOut", { task: task("claude", "login", { startedAt: new Date().toISOString() }) }), tool("codex", "missing"));
  open(
    device(sent, () => (signedIn ? report(tool("claude", "ready"), tool("codex", "missing")) : signingIn()), {
      "GET /api/setup/tools/claude/login": () => {
        signedIn = true;
        return { state: "signed-in", method: "claude.ai", plan: "Max 20x", email: "lin@example.com", usable: true };
      },
    }),
  );
  dismissSetupToast();
  const { root } = await mount();
  t.after(() => root.unmount());
  assert.ok(await until(() => cardOf("claude")?.getAttribute("data-state") === "signingIn"));
  assert.ok(await until(() => cardOf("claude")?.getAttribute("data-state") === "ready"), "ready without a reload");
  assert.ok(sent.some((s) => s.path === "/api/setup/tools/claude/login"), "the cheap sign-in check was asked");
  assert.equal(setupToastNow()?.text, SIGNED_IN_TOAST.claude);
  assert.equal(sent.filter((s) => s.path === "/api/setup/tools/codex/login").length, 0, "only the tool signing in is checked");
});

test("an install that finishes: polled while it runs, the card moves on to signing in", async (t) => {
  const sent: Sent[] = [];
  let polls = 0;
  open(
    device(sent, () =>
      ++polls >= 2
        ? report(tool("claude", "signedOut", { task: task("claude", "install", { running: false, exitCode: 0 }) }), tool("codex", "missing"))
        : report(tool("claude", "missing", { task: task("claude", "install") }), tool("codex", "missing")),
    ),
  );
  const { root } = await mount();
  t.after(() => root.unmount());
  assert.ok(await until(() => cardOf("claude")?.getAttribute("data-state") === "installing"));
  assert.ok(await until(() => cardOf("claude")?.getAttribute("data-state") === "signedOut"), "the next state, from the next detection");
  assert.ok(polls >= 2);
  const before = polls;
  await flush(2300);
  assert.equal(polls, before, "nothing runs any more: no more polling");
});

test("a task still running when the guide opens again shows its terminal, without taking the focus", async (t) => {
  open(device([], () => report(tool("claude", "missing", { task: task("claude", "install", { terminalId: "t-live" }) }), tool("codex", "ready"))));
  const focused: string[] = [];
  const realFocus = (container as FakeNode).focus;
  Object.getPrototypeOf(container).focus = function (this: FakeNode) {
    focused.push(this.nodeName);
  };
  t.after(() => void (Object.getPrototypeOf(container).focus = realFocus));
  const { root } = await mount();
  t.after(() => root.unmount());
  await until(() => find((n) => n.getAttribute("data-setup-terminal") === "t-live") !== undefined);
  const region = find((n) => n.getAttribute("data-setup-terminal") === "t-live");
  assert.ok(region, "the running task's terminal, attached again");
  assert.equal(region.getAttribute("role"), "region");
  assert.equal(region.getAttribute("aria-label"), "Terminal · on this device");
  assert.deepEqual(focused, [], "nothing took the focus");
});

test("F6 moves the focus between the terminal and the action bar, and the CLI never gets the key", async (t) => {
  open(device([], () => report(tool("claude", "missing", { task: task("claude", "install", { terminalId: "t-f6" }) }), tool("codex", "ready"))));
  const { root } = await mount();
  t.after(() => root.unmount());
  await until(() => windowListeners.some((l) => l.type === "keydown" && l.capture));
  const listener = windowListeners.find((l) => l.type === "keydown" && l.capture);
  assert.ok(listener, "caught before xterm sees it");
  // The page as a browser would query it: the terminal region with xterm's input, and the action bar with its primary.
  const doc = document as unknown as FakeNode & Record<string, unknown>;
  const region = find((n) => n.getAttribute("data-setup-terminal") === "t-f6")!;
  const input = doc.createElement("textarea" as never) as unknown as FakeNode;
  region.appendChild(input);
  const bar = doc.createElement("footer" as never) as unknown as FakeNode;
  const primary = doc.createElement("button" as never) as unknown as FakeNode;
  bar.appendChild(primary);
  const focused: FakeNode[] = [];
  for (const node of [input, primary]) node.focus = () => void (focused.push(node), (doc.activeElement = node));
  Object.assign(region, { querySelector: () => input });
  Object.assign(bar, { querySelector: () => primary });
  const restore = { all: doc.querySelectorAll, one: doc.querySelector };
  Object.assign(doc, {
    querySelectorAll: (sel: string) => (sel === "[data-setup-terminal]" ? [region] : []),
    querySelector: (sel: string) => (sel.includes("action-bar") ? bar : null),
    activeElement: doc.body,
  });
  t.after(() => Object.assign(doc, { querySelectorAll: restore.all, querySelector: restore.one }));
  const press = (shiftKey = false) => {
    const event = { key: "F6", shiftKey, altKey: false, metaKey: false, ctrlKey: false, prevented: false, stopped: false, preventDefault() { this.prevented = true; }, stopPropagation() { this.stopped = true; } };
    listener.fn(event);
    return event;
  };
  const first = press();
  assert.deepEqual(focused, [input], "into the terminal");
  assert.ok(first.prevented && first.stopped, "the key is the page's, not the CLI's");
  press();
  assert.deepEqual(focused, [input, primary], "out to the action bar");
  press();
  assert.deepEqual(focused, [input, primary, input], "and back");
  press(true);
  assert.equal(focused.at(-1), primary, "Shift+F6 the other way");
  // Other keys are left alone.
  const other = { key: "Tab", preventDefault: () => assert.fail("not ours"), stopPropagation: () => assert.fail("not ours") };
  listener.fn(other);
});

test("an OpenAI API key goes in the sign-in request's body and nowhere else: not the field, not the guide's state, not the cache", async (t) => {
  const sent: Sent[] = [];
  const KEY = "sk-test-0123456789abcdef";
  let signedIn = false;
  open(
    device(sent, () => (signedIn ? report(tool("claude", "ready"), tool("codex", "ready", { login: { state: "signed-in", method: "apiKey", plan: null, email: null, usable: true } })) : report(tool("claude", "ready"), tool("codex", "signedOut"))), {
      "POST /api/setup/tools/codex/login": () => {
        signedIn = true;
        return task("codex", "login", { method: "apikey", terminalId: null, running: false, exitCode: 0 });
      },
    }),
  );
  const uis: ToolsUi[] = [];
  const { root, queries } = await mount((ui) => uis.push(ui));
  t.after(() => root.unmount());
  await until(() => byTestId("use-api-key", cardOf("codex")) !== undefined);
  propsOf(byTestId("use-api-key", cardOf("codex"))).onClick!();
  await flush();
  const input = () => byTestId("apikey-input");
  assert.equal(propsOf(input()).type as unknown, "password");
  propsOf(input()).onChange!({ target: { value: KEY } });
  await flush();
  assert.equal(propsOf(input()).value as unknown, KEY);
  propsOf(byTestId("apikey-submit")).onClick!();
  await flush(80);
  const login = sent.find((s) => s.method === "POST" && s.path === "/api/setup/tools/codex/login");
  assert.deepEqual(login?.body, { mode: "apikey", apiKey: KEY }, "in the body, once");
  assert.equal(sent.filter((s) => JSON.stringify(s).includes(KEY)).length, 1, "in no other request");
  assert.ok(!input() || propsOf(input()).value === "", "the field is empty");
  assert.ok(!JSON.stringify(uis).includes(KEY), "never in the guide's state");
  assert.ok(!JSON.stringify(queries.getQueryCache().getAll().map((q) => q.state.data)).includes(KEY), "never in the cache");
  assert.ok(!all().some((n) => n.getAttribute("value") === KEY || n.textContent.includes(KEY)), "never on the page");
  assert.ok(await until(() => cardOf("codex")?.getAttribute("data-state") === "ready"), "and the card turns ready");
});
