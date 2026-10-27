// A Claude session on the user's own login, when that login has stopped
// working — mounted for real into a real DOM (jsdom), the device
// answering through a recording transport:
//
// - the account rows label a signed-out local login and say how to fix it,
//   while keeping it a choice;
// - the header badge of a local-login session is a control, not a dead "local":
//   it says why the account cannot be switched and offers to continue the
//   conversation on an account instead;
// - continuing asks the device for a new session holding a copy of the
//   conversation, then shows that session in this one's place;
// - the failure line under the composer offers the same, for a failure that
//   reads as the login being refused.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { domTree, type FakeNode, installLiveDom, propsOf, until } from "./live-dom.js";

const container = installLiveDom();
(globalThis as Record<symbol, unknown>)[Symbol.for("undici.globalOrigin.1")] = new URL("http://device.test");
const flush = (ms = 30) => new Promise((resolve) => setTimeout(resolve, ms));
const { first } = domTree(container);
/** Wait for the page to show something, rather than for a fixed time: the device answers when it answers. */

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
const { AccountRows } = await import("../src/features/accounts/account-picker.js");
const { SessionAccountBadge, LocalLoginExplainer } = await import("../src/features/sessions/session-account-badge.js");
const { ContinueOnAccountButton, looksLikeLoginFailure } = await import("../src/features/sessions/continue-on-account.js");
const { localLoginLabel, startAccountFor } = await import("../src/features/accounts/use-accounts.js");
type Transport = import("../src/lib/transport/index.js").Transport;
type AccountList = import("../src/features/accounts/use-accounts.js").AccountList;

interface Sent {
  method: string;
  path: string;
  body: unknown;
}

const SESSION = "11111111-1111-4111-8111-111111111111";
const CONTINUED = "22222222-2222-4222-8222-222222222222";
const ACCOUNT_A = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const ACCOUNT_B = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

function account(id: string, label: string, rest: unknown[] = []) {
  return {
    id, label, kind: "oauth_token", provider: "claude", usageEnabled: false, rotation: true, rest,
    createdAt: "2026-10-01T00:00:00Z", updatedAt: "2026-10-01T00:00:00Z",
  };
}
const AUTH_REST = {
  scope: "all", class: "auth", untilAt: null, reason: "Failed to authenticate: OAuth session expired and could not be refreshed",
  observedAt: "2026-10-19T05:11:15Z", sessionId: SESSION, source: "claude:result",
};
const signedOut = (accounts: unknown[]) => ({
  accounts, defaultAccountId: null, defaultCodexAccountId: null,
  local: { claude: { state: "signed_out", rest: [AUTH_REST], isDefault: false }, codex: { state: "unknown", rest: [], isDefault: false } },
});

/** A device that lists `accounts` and continues any session as `CONTINUED`. */
function device(sent: Sent[], accounts: unknown, refuse = false): Transport {
  return {
    fetch: async (request: Request) => {
      const url = new URL(request.url);
      if (url.pathname === "/api/accounts") return Response.json(accounts);
      if (url.pathname === "/api/stats/quota") return Response.json({ accounts: [], claude: null, codex: null });
      if (request.method === "POST" && url.pathname.endsWith("/continue")) {
        sent.push({ method: "POST", path: url.pathname, body: await request.json() });
        if (refuse) return Response.json({ error: "external_session", hint: "this session belongs to another server" }, { status: 409 });
        return Response.json({ source: "claude", id: CONTINUED });
      }
      return Response.json({ error: "not here" }, { status: 404 });
    },
    socketUrl: async (path) => path,
    openSocket: () => ({ readyState: 0, binaryType: "blob", send() {}, close() {}, onopen: null, onmessage: null, onclose: null }),
  };
}

function reset(sent: Sent[], accounts: unknown, refuse = false) {
  registry.setShell("local", LOCAL);
  registry.openMachine(LOCAL, device(sent, accounts, refuse), {
    info: constantStore({ key: LOCAL, name: "studio", state: "online" as const, since: 0, canManage: true, live: "live" as const }),
    ensureLive: () => undefined,
    hold: () => () => undefined,
    dispose: () => undefined,
  });
}

async function mount(node: React.ReactNode) {
  const errors: unknown[] = [];
  const root = createRoot(container as never, {
    onUncaughtError: (error) => void errors.push(error),
    onCaughtError: (error) => void errors.push(error),
  });
  const queries = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  // A link to Settings is part of the account rows, so the tree sits in a router.
  const routes = router.createRouter({
    routeTree: router.createRootRoute({ component: () => <>{node}</> }),
    history: router.createMemoryHistory({ initialEntries: ["/sessions/claude/" + SESSION] }),
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

const text = () => (container as FakeNode).textContent;
const button = (label: RegExp) => first((n) => n.nodeName === "BUTTON" && label.test(n.textContent));

test("the account rows name a signed-out local login, say how to fix it, and keep it a choice", async (t) => {
  reset([], signedOut([account(ACCOUNT_A, "work")]));
  const picks: (string | null)[] = [];
  const { root, errors } = await mount(
    <AccountRows accounts={[account(ACCOUNT_A, "work")] as never} value={ACCOUNT_A} allowLocal onSelect={(id) => void picks.push(id)} />,
  );
  t.after(() => root.unmount());
  await until("the account list", () => !!first((n) => n.getAttribute("data-local-login") === "signed_out"));
  const row = first((n) => n.getAttribute("data-local-login") === "signed_out");
  assert.ok(row, "the local login's row says what state it is in");
  assert.match(row.textContent, /^local login \(not signed in\)/);
  assert.match(row.textContent, /run `claude` in a terminal on this device, then `\/login`/, "and how to sign it back in");
  propsOf(row).onClick!();
  assert.deepEqual(picks, [null], "picking it is still allowed");
  assert.deepEqual(errors, []);
});

test("a local-login session's badge is a control that explains itself, not a dead label", async (t) => {
  reset([], signedOut([account(ACCOUNT_A, "work")]));
  const { root, errors } = await mount(
    <SessionAccountBadge source="claude" id={SESSION} claudeRoot="local" account={null} scope="owned" onOpenSession={() => undefined} />,
  );
  t.after(() => root.unmount());
  await until("the account list", () => first((n) => n.hasAttribute("data-local-badge"))?.getAttribute("data-local-badge") === "signed_out");
  const trigger = first((n) => n.hasAttribute("data-local-badge"));
  assert.ok(trigger, "the badge");
  assert.equal(trigger.nodeName, "BUTTON", "something to press");
  assert.equal(trigger.getAttribute("data-local-badge"), "signed_out");
  assert.match(trigger.textContent, /local \(not signed in\)/);
  assert.match(trigger.getAttribute("title") ?? "", /cannot switch to an account/, "why, on hover");
  assert.match(text(), /credential invalid · re-authorize needed/, "the login's own rest beside it");
  assert.deepEqual(errors, []);
});

test("the badge's panel offers to continue on an account: a new session with a copy, opened in this one's place", async (t) => {
  const sent: Sent[] = [];
  reset(sent, signedOut([account(ACCOUNT_A, "work"), account(ACCOUNT_B, "spare")]));
  const opened: [string, string][] = [];
  let done = 0;
  const { root, errors } = await mount(
    <LocalLoginExplainer
      id={SESSION}
      owned
      login={signedOut([]).local.claude as never}
      onOpenSession={(source, id) => void opened.push([source, id])}
      onDone={() => void done++}
    />,
  );
  t.after(() => root.unmount());
  await until("the accounts to continue on", () => !!button(/^spare/));
  assert.match(text(), /runs on your own claude login on this device, and its conversation is kept there, so it cannot switch to an account/);
  assert.match(text(), /signed out: run `claude` in a terminal on this device, then `\/login`/);
  assert.ok(first((n) => n.hasAttribute("data-continue-on-account")), "the way out");
  assert.equal(first((n) => n.hasAttribute("data-local-login")), undefined, "the local login is not offered: it is the one that is stuck");

  propsOf(button(/^spare/)).onClick!();
  await flush();
  assert.ok(first((n) => n.getAttribute("data-continue-confirm") === ACCOUNT_B));
  assert.match(text(), /A new session starts on spare with a copy of this conversation\. This session stays as it is, on your own login\./);
  assert.deepEqual(sent, [], "nothing is asked of the device before the confirmation");

  propsOf(first((n) => n.getAttribute("data-variant") === "primary")).onClick!();
  await until("the new session", () => opened.length > 0);
  assert.deepEqual(sent, [{ method: "POST", path: `/api/sessions/claude/${SESSION}/continue`, body: { accountId: ACCOUNT_B } }]);
  assert.deepEqual(opened, [["claude", CONTINUED]], "the new session takes this one's place");
  assert.equal(done, 1, "and the panel closes");
  assert.deepEqual(errors, []);
});

test("a refused continuation says why and stays on the confirmation", async (t) => {
  const sent: Sent[] = [];
  reset(sent, signedOut([account(ACCOUNT_A, "work")]), true);
  const opened: string[] = [];
  const { root } = await mount(
    <LocalLoginExplainer id={SESSION} owned login={null} onOpenSession={(_, id) => void opened.push(id)} />,
  );
  t.after(() => root.unmount());
  await until("the accounts to continue on", () => !!button(/^work/));
  propsOf(button(/^work/)).onClick!();
  await flush();
  propsOf(first((n) => n.getAttribute("data-variant") === "primary")).onClick!();
  await until("the refusal", () => /belongs to another server/.test(text()));
  assert.equal(sent.length, 1);
  assert.match(text(), /this session belongs to another server/, "the server's own words, not its token");
  assert.deepEqual(opened, []);
});

test("a session this server did not start only explains; there is nothing to continue from here", async (t) => {
  reset([], signedOut([account(ACCOUNT_A, "work")]));
  const { root } = await mount(
    <LocalLoginExplainer id={SESSION} owned={false} login={null} onOpenSession={() => undefined} />,
  );
  t.after(() => root.unmount());
  assert.match(text(), /not started by Droppen on this device, so it can only be read here/);
  assert.equal(first((n) => n.hasAttribute("data-continue-on-account")), undefined);
});

test("the failure line's action is the same panel behind a button", async (t) => {
  reset([], signedOut([account(ACCOUNT_A, "work")]));
  const { root, errors } = await mount(<ContinueOnAccountButton id={SESSION} onOpenSession={() => undefined} />);
  t.after(() => root.unmount());
  const trigger = first((n) => n.hasAttribute("data-continue-trigger"));
  assert.ok(trigger);
  assert.equal(trigger.nodeName, "BUTTON");
  assert.match(trigger.textContent, /continue on another account/);
  assert.deepEqual(errors, []);
});

test("which failures read as the login being refused", () => {
  for (const error of [
    "Failed to authenticate: OAuth session expired and could not be refreshed",
    "API Error: 401 {\"type\":\"error\"}",
    "Invalid API key · Please run /login",
    "Not logged in",
  ]) {
    assert.equal(looksLikeLoginFailure(error), true, error);
  }
  for (const error of ["claude exited with code 1", "You've hit your session limit", "", null, undefined]) {
    assert.equal(looksLikeLoginFailure(error), false, String(error));
  }
});

test("the start account and the local label follow the local login's state", () => {
  const t = (key: string) => key;
  const list = (local: unknown, accounts: unknown[] = [account(ACCOUNT_A, "work"), account(ACCOUNT_B, "spare")], defaultAccountId: string | null = null) =>
    ({ accounts, defaultAccountId, defaultCodexAccountId: null, local }) as unknown as AccountList;
  const out = signedOut([]).local;
  const unknown = { claude: { state: "unknown", rest: [] }, codex: { state: "unknown", rest: [] } };
  assert.equal(startAccountFor(list(out), "claude").accountId, ACCOUNT_A, "signed out: the first account");
  assert.equal(startAccountFor(list(out, [], null), "claude").accountId, null, "with no account, the local login is all there is");
  assert.equal(startAccountFor(list(out, undefined, ACCOUNT_B), "claude").accountId, ACCOUNT_B, "a configured default always wins");
  assert.equal(startAccountFor(list(unknown), "claude").accountId, null, "not known to be broken: the local login, as before");
  assert.equal(startAccountFor(list(undefined), "claude").accountId, null, "an older server that says nothing");
  assert.equal(
    startAccountFor(list({ claude: { state: "signed_out", rest: [AUTH_REST] }, codex: { state: "unknown", rest: [] } }), "claude").accountId,
    null,
    "a server from before the start rule (no isDefault) starts on the local login whatever its state, and is shown doing so",
  );
  assert.equal(startAccountFor(list(out), "codex").accountId, null, "the codex login is its own");
  assert.equal(
    startAccountFor(list(out, [account(ACCOUNT_A, "work", [{ ...AUTH_REST, class: "weekly", untilAt: "2099-01-01T00:00:00Z" }]), account(ACCOUNT_B, "spare")]), "claude").accountId,
    ACCOUNT_B,
    "an account resting on everything is skipped",
  );
  assert.equal(localLoginLabel(out.claude as never, t), "local login (not signed in)");
  assert.equal(localLoginLabel(unknown.claude as never, t), "local login");
  assert.equal(localLoginLabel(null, t), "local login");
});
