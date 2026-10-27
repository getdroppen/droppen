// The new-session view mounted for real (`features/sessions/new-session-view.tsx`):
// React renders into a stand-in DOM (`fake-dom.ts`), the device answers
// through a recording transport, and what is pinned is what a person sees and
// what the device is sent — a pasted picture in the composer, the start
// request carrying the first message's attachments, the view turning into the
// session it started (the URL replaced, the message pending in the session's
// own composer), the draft kept across a reload, and Cmd/Ctrl+N opening it.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test, { after } from "node:test";
import { register } from "node:module";
import { installFakeDom, type FakeNode } from "./fake-dom.js";

// The build's plugin list (`virtual:droppen/bundled-plugins`, made by Vite):
// the open distribution's, none, for the pane code to import.
const bundled = `data:text/javascript,${encodeURIComponent('export const distribution = "public"; export const bundledPlugins = [];')}`;
register(
  `data:text/javascript,${encodeURIComponent(
    `export async function resolve(specifier, context, next) { return specifier === "virtual:droppen/bundled-plugins" ? { url: ${JSON.stringify(bundled)}, shortCircuit: true } : next(specifier, context); }`,
  )}`,
);

const container = installFakeDom();
// Relative requests (`new Request("/api/uploads")`) resolve as on a page.
(globalThis as Record<symbol, unknown>)[Symbol.for("undici.globalOrigin.1")] = new URL("http://device.test");
// The page's own listeners, kept so a test can press a key on the page.
const windowListeners: { type: string; fn: (event: unknown) => void }[] = [];
Object.assign(window as object, {
  addEventListener: (type: string, fn: (event: unknown) => void) => void windowListeners.push({ type, fn }),
  removeEventListener: (type: string, fn: (event: unknown) => void) => {
    const at = windowListeners.findIndex((l) => l.type === type && l.fn === fn);
    if (at >= 0) windowListeners.splice(at, 1);
  },
});
// Query caches, feeds and React's scheduler keep timers this file does not own: once every test has run, go.
after(() => void setTimeout(() => process.exit(), 50));
const flush = (ms = 30) => new Promise((resolve) => setTimeout(resolve, ms));

// After the stand-in DOM: React and the app's modules read it as they load.
const i18next = (await import("i18next")).default;
const { initReactI18next } = await import("react-i18next");
await i18next.use(initReactI18next).init({ lng: "en", resources: {}, keySeparator: false, nsSeparator: false, interpolation: { escapeValue: false } });
const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
const router = await import("@tanstack/react-router");
const { GlobalFeedProvider, machineFeedFor } = await import("../src/app/global-feed.js");
const { MachineProvider } = await import("../src/lib/machines/context.js");
const { LOCAL, sessionKey } = await import("../src/lib/machines/key.js");
const registry = await import("../src/lib/machines/registry.js");
const { constantStore } = await import("../src/lib/machines/store.js");
const { NewSessionRoute } = await import("../src/features/sessions/new-session-view.js");
const { NewSessionHotkey, useOpenNewSession, validateNewSessionSearch } = await import("../src/features/sessions/new-session-open.js");
const { DeliveryPreviews, seedDeliveryPreview, useDeliveryPreviews, usePendingDeliveries } = await import("../src/features/sessions/delivery-preview-view.js");
const { readNewSessionDraft, writeNewSessionDraft } = await import("../src/features/sessions/new-session.js");
const { PaneLeafContent } = await import("../src/app/secondary-pane.js");
type PaneContent = import("../src/app/pane-tree.js").PaneContent;
type Transport = import("../src/lib/transport/index.js").Transport;
type SessionSummary = import("../src/api/schema.js").components["schemas"]["SessionSummary"];

const UPLOADS = "/home/demo/.remote-hand/uploads/u1";

/** What the device was asked to do. */
interface Sent {
  method: string;
  path: string;
  body: unknown;
}

/** A device that answers what the view asks, uploads anything, and starts a session as `new-1`. */
/** How the device answers a start: it starts one, refuses the directory, or finds an attachment gone. */
type Answer = "start" | "refuse" | "missing";

/** The device's account list: none stored, no defaults, nothing known about the local login. */
const NO_ACCOUNTS = { accounts: [], defaultAccountId: null, defaultCodexAccountId: null };

/**
 * The device. `start` is its answer to `GET /api/accounts/start`; without one
 * the route is missing and the view reads the rule off the list.
 */
function device(sent: Sent[], mode: Answer = "start", accounts: unknown = NO_ACCOUNTS, start?: unknown, extra: Record<string, unknown> = {}): Transport {
  const answers: Record<string, unknown> = {
    ...(start === undefined ? {} : { "/api/accounts/start": start }),
    "/api/models": { models: [{ id: "fable", label: null, efforts: ["low", "high"], defaultEffort: "high" }], source: "cache" },
    "/api/composer-prefs": { defaults: {}, sessions: {} },
    "/api/accounts": accounts,
    "/api/prefs": { composerSend: "enter" },
    "/api/quota": { accounts: [], claude: null, codex: null },
    ...extra,
  };
  return {
    fetch: async (request: Request) => {
      const url = new URL(request.url);
      if (url.pathname === "/api/uploads" && request.method === "POST") {
        const file = (await request.formData()).get("file") as File;
        sent.push({ method: "POST", path: url.pathname, body: file.name });
        // A test may hold an upload, or fail it.
        const upload = extra["/api/uploads"];
        if (typeof upload === "function") {
          const answered = await (upload as (name: string) => Promise<Response | undefined>)(file.name);
          if (answered) return answered;
        }
        return Response.json({ path: `${UPLOADS}/${file.name}`, name: file.name, isImage: file.type.startsWith("image/") });
      }
      if (url.pathname === "/api/sessions/new" && request.method === "POST") {
        sent.push({ method: "POST", path: url.pathname, body: await request.json() });
        // A test may hold the device's answer to a start.
        const gate = extra["/api/sessions/new"];
        if (typeof gate === "function") await (gate as () => Promise<void>)();
        if (mode === "refuse") return Response.json({ error: "not a directory: /home/demo/app" }, { status: 400 });
        if (mode === "missing") return Response.json({ error: "attachment_missing", hint: "an attachment is no longer on this device; attach it again" }, { status: 409 });
        return Response.json({ id: "new-1" });
      }
      const listed = answers[url.pathname];
      // An answer may depend on what is asked, and may take its time (a promise).
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

function summary(over: Partial<SessionSummary>): SessionSummary {
  return {
    id: "s-1", source: "claude", title: "earlier", activity: "idle", status: "idle", asking: false, attentionAt: null,
    createdAt: "2026-10-18T00:00:00Z", updatedAt: "2026-10-18T01:00:00Z", cwd: "/home/demo/app", filePath: "/x.jsonl",
    gitBranch: null, repoKey: null, resident: null, scope: "owned", server: "test", sizeBytes: 1, unreadCount: 0, unread: false,
    ...over,
  } as SessionSummary;
}

/** The session page the new-session view gives way to: its composer's pending messages. */
function SessionStub() {
  const { source, id } = router.useParams({ strict: false }) as { source: string; id: string };
  const frame = React.useMemo(() => ({ items: [], startIndex: 0, totalItems: 0, queue: [] }), []);
  const { previews } = useDeliveryPreviews(sessionKey({ machine: LOCAL, source, id }), frame, true);
  return (
    <div data-session={`${source}/${id}`}>
      <DeliveryPreviews messages={previews} />
    </div>
  );
}

/** The app's session routes, around the real new-session route, at `path`. */
function app(path: string) {
  const root = router.createRootRoute({ component: () => <router.Outlet /> });
  const sessions = router.createRoute({ getParentRoute: () => root, path: "sessions", component: () => <router.Outlet /> });
  const index = router.createRoute({ getParentRoute: () => sessions, path: "/", component: () => <div data-sessions-index="" /> });
  const fresh = router.createRoute({ getParentRoute: () => sessions, path: "new", component: NewSessionRoute, validateSearch: validateNewSessionSearch });
  const detail = router.createRoute({ getParentRoute: () => sessions, path: "$source/$id", component: SessionStub });
  const history = router.createMemoryHistory({ initialEntries: [path] });
  return { history, router: router.createRouter({ routeTree: root.addChildren([sessions.addChildren([index, fresh, detail])]), history }) };
}

/** The query options production runs with (`main.tsx`), for a test about retries; `retry: false` otherwise. */
const PRODUCTION_QUERIES = { retry: 1, retryDelay: 5 };

async function mount(path: string, extra: React.ReactNode = null, queryOptions: Record<string, unknown> = { retry: false }) {
  const errors: unknown[] = [];
  const { router: routes, history } = app(path);
  // Matched before the first render, as the app's own router has by then.
  await routes.load();
  const root = createRoot(container as never, {
    onUncaughtError: (error) => void errors.push(error),
    onCaughtError: (error) => void errors.push(error),
  });
  const queries = new QueryClient({ defaultOptions: { queries: queryOptions } });
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
  root.render(<React.Fragment key={generation}>{tree()}</React.Fragment>);
  // The router commits a navigation (its stores say so: the new matches, not
  // loading) in a transition the stand-in page never draws; a fresh tree over
  // the same router, caches and device draws what it committed.
  const settle = async () => {
    await flush();
    await routes.load();
    root.render(<React.Fragment key={++generation}>{tree()}</React.Fragment>);
    await flush();
  };
  return { root, errors, routes, history, settle };
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
/** The props React gave an element, so a test can do what a browser's event would. */
const propsOf = (node: FakeNode | undefined): Record<string, (...args: unknown[]) => unknown> => {
  assert.ok(node, "the element is on the page");
  const key = Object.keys(node).find((k) => k.startsWith("__reactProps$"));
  assert.ok(key, "rendered by React");
  return (node as unknown as Record<string, Record<string, (...args: unknown[]) => unknown>>)[key];
};
const textarea = () => first((n) => n.nodeName === "TEXTAREA");
const sendButton = () => first((n) => n.getAttribute("data-variant") === "primary");
const type = (text: string) => propsOf(textarea()).onChange!({ target: { value: text } });
const paste = (file: File) => propsOf(textarea()).onPaste!({ clipboardData: { files: [file] }, preventDefault() {} });
const picture = () => new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], "shot.png", { type: "image/png" });

function reset(sent: Sent[], mode: Answer = "start", accounts: unknown = NO_ACCOUNTS, start?: unknown, extra: Record<string, unknown> = {}) {
  registry.setShell("local", LOCAL);
  registry.openMachine(LOCAL, device(sent, mode, accounts, start, extra), {
    info: constantStore({ key: LOCAL, name: "studio", state: "online" as const, since: 0, canManage: true, live: "live" as const }),
    ensureLive: () => undefined,
    hold: () => () => undefined,
    dispose: () => undefined,
  });
  localStorage.clear();
  machineFeedFor(LOCAL).store.publish({ sessions: [summary({})] });
}

test("a picture pasted into the new session's composer shows as a thumbnail and is uploaded at once", async (t) => {
  const sent: Sent[] = [];
  reset(sent);
  const { root, errors } = await mount("/sessions/new");
  t.after(() => root.unmount());
  await until("the view", () => !!textarea());
  assert.ok(first((n) => n.getAttribute("data-new-session") === LOCAL), "the empty session view");
  paste(picture());
  await until("the thumbnail", () => !!first((n) => n.getAttribute("data-attachment") === "image"));
  const thumb = first((n) => n.getAttribute("data-attachment") === "image");
  assert.ok(thumb, "a thumbnail in the composer");
  const img = walk(thumb).find((n) => n.nodeName === "IMG");
  assert.match(img?.getAttribute("src") ?? "", /^blob:/, "drawn from the pasted file itself");
  assert.deepEqual(sent.map((s) => `${s.method} ${s.path}`), ["POST /api/uploads"], "uploaded before anything is sent");
  assert.deepEqual(errors, []);
});

test("sending starts the session with the first message's attachments, and the view becomes that session in place", async (t) => {
  const sent: Sent[] = [];
  reset(sent);
  const { root, errors, routes, history, settle } = await mount("/sessions/new");
  t.after(() => root.unmount());
  await flush();
  // The directory last used is already the pick: the device's one recent directory.
  await until("the directory list", () => !!first((n) => n.getAttribute("data-directory") === "/home/demo/app"));
  assert.equal(first((n) => n.getAttribute("data-directory") === "/home/demo/app")?.getAttribute("aria-checked"), "true");
  type("what is wrong with this page?");
  paste(picture());
  paste(new File(["log line"], "trace.txt", { type: "text/plain" }));
  await until("both uploads", () => sent.filter((s) => s.path === "/api/uploads").length === 2);
  await flush();
  propsOf(sendButton()).onClick!();
  await until("the start", () => !!first((n) => n.hasAttribute("data-preview-images")));

  const start = sent.find((s) => s.path === "/api/sessions/new");
  assert.ok(start, "a start was sent");
  const body = start.body as Record<string, unknown>;
  assert.equal(body.text, "what is wrong with this page?");
  assert.equal(body.cwd, "/home/demo/app");
  assert.equal(body.source, "claude");
  assert.deepEqual(body.images, [`${UPLOADS}/shot.png`]);
  assert.deepEqual(body.files, [`${UPLOADS}/trace.txt`]);
  assert.match(String(body.clientMessageId), /^[0-9a-f-]{36}$/, "named, so a retry is the same start");

  // Accepted, not yet listed: the message waits in the view, its picture with it.
  assert.ok(first((n) => n.hasAttribute("data-starting")), "says the session is starting");
  assert.ok(first((n) => n.hasAttribute("data-preview-images")), "the pending message carries its thumbnail");
  assert.equal(routes.state.location.pathname, "/sessions/new");

  // Listed: the same place is the session now — the address replaced, not pushed.
  machineFeedFor(LOCAL).store.publish({ sessions: [summary({}), summary({ id: "new-1", title: "what is wrong" })] });
  await until("the session's address", () => routes.state.location.pathname === "/sessions/claude/new-1");
  assert.equal(routes.state.location.pathname, "/sessions/claude/new-1");
  assert.equal(history.length, 1, "Back does not return to a form that was already sent");
  await settle();
  const session = first((n) => n.getAttribute("data-session") === "claude/new-1");
  assert.ok(session, "the session's page");
  assert.match(session.textContent, /what is wrong with this page\?/, "its composer has the first message as pending");
  assert.ok(walk(session).some((n) => n.hasAttribute("data-preview-images")), "thumbnail and all");
  assert.equal(readNewSessionDraft(localStorage, LOCAL).text, "", "and the draft is spent");
  assert.deepEqual(errors, []);
});

test("a start the device refuses puts the message and its attachments back where they were typed", async (t) => {
  const sent: Sent[] = [];
  reset(sent, "refuse");
  const { root, routes } = await mount("/sessions/new");
  t.after(() => root.unmount());
  await flush();
  type("try this");
  paste(picture());
  await until("the upload", () => sent.some((s) => s.path === "/api/uploads"));
  await flush();
  propsOf(sendButton()).onClick!();
  await until("the refusal", () => /could not start the session/.test((container as FakeNode).textContent));
  assert.equal(sent.filter((s) => s.path === "/api/sessions/new").length, 1);
  assert.equal(propsOf(textarea()).value, "try this", "the words are back");
  assert.ok(first((n) => n.getAttribute("data-attachment") === "image"), "and the picture");
  assert.match((container as FakeNode).textContent, /could not start the session: not a directory/);
  assert.equal(routes.state.location.pathname, "/sessions/new", "nothing to give way to");
  assert.equal(readNewSessionDraft(localStorage, LOCAL).text, "try this", "the draft still holds it");
  assert.ok(readNewSessionDraft(localStorage, LOCAL).attempt, "with the id it went under, for a retry after a reload");
});

test("an attachment no longer on the device is refused in words, and the draft keeps the message", async (t) => {
  const sent: Sent[] = [];
  reset(sent, "missing");
  const { root } = await mount("/sessions/new");
  t.after(() => root.unmount());
  await flush();
  type("look again");
  paste(picture());
  await until("the upload", () => sent.some((s) => s.path === "/api/uploads"));
  await flush();
  propsOf(sendButton()).onClick!();
  await until("the refusal", () => /could not start the session/.test((container as FakeNode).textContent));
  assert.match((container as FakeNode).textContent, /could not start the session: an attachment is no longer on this device; attach it again/);
  assert.doesNotMatch((container as FakeNode).textContent, /attachment_missing/, "the reason, not its token");
  assert.equal(propsOf(textarea()).value, "look again");
  const kept = readNewSessionDraft(localStorage, LOCAL);
  assert.equal(kept.text, "look again");
  assert.equal(kept.attachments.length, 1, "the attachment stays, to be removed or attached again");
});

test("a session's pending first message is counted until its transcript has it — the view says it is starting, not empty", async (t) => {
  const key = sessionKey({ machine: LOCAL, source: "claude", id: "fresh-1" });
  function Count() {
    return <span data-pending={usePendingDeliveries(key)} />;
  }
  function Echo({ items }: { items: { role: string; kind: string; text: string }[] }) {
    const frame = React.useMemo(() => ({ items, startIndex: 0, totalItems: items.length, queue: [] }), [items]);
    useDeliveryPreviews(key, frame as never, true);
    return null;
  }
  const errors: unknown[] = [];
  const root = createRoot(container as never, { onUncaughtError: (e) => void errors.push(e), onCaughtError: (e) => void errors.push(e) });
  t.after(() => root.unmount());
  const pending = () => first((n) => n.hasAttribute("data-pending"))?.getAttribute("data-pending");
  // The transcript asks before anything made the session's previews.
  root.render(<Count />);
  await flush();
  assert.equal(pending(), "0");
  seedDeliveryPreview(key, { id: "p-1", text: "first words", after: 0, sending: false });
  await flush();
  assert.equal(pending(), "1", "the started session's first message");
  root.render(<><Count /><Echo items={[]} /></>);
  await flush();
  assert.equal(pending(), "1", "still waiting while the transcript is empty");
  root.render(<><Count /><Echo items={[{ role: "user", kind: "text", text: "first words" }]} /></>);
  await flush();
  assert.equal(pending(), "0", "the transcript has it");
  assert.deepEqual(errors, []);
  // And the transcript's empty state reads it.
  const detail = readFileSync(fileURLToPath(new URL("../src/features/sessions/session-detail.tsx", import.meta.url)), "utf8");
  assert.match(detail, /items\.length === 0 && \(pendingSends > 0 \?/);
});

test("in a split pane the view is the pane's content, and the pane becomes the session it starts", async (t) => {
  const sent: Sent[] = [];
  reset(sent);
  const changes: PaneContent[] = [];
  const { root, errors } = await mount(
    "/sessions",
    <PaneLeafContent content={{ kind: "new-session" }} onChange={(next) => void changes.push(next)} onClose={() => undefined} />,
  );
  t.after(() => root.unmount());
  // The pane's view is loaded the first time one is shown (`app/lazy-views.ts`).
  await until("the empty session view, in the pane", () => !!first((n) => n.getAttribute("data-new-session") === LOCAL));
  const view = first((n) => n.getAttribute("data-new-session") === LOCAL)!;
  assert.ok(walk(view).some((n) => n.getAttribute("aria-label") === "close pane"), "with the pane's own controls in its title row");
  type("start here");
  await flush();
  propsOf(sendButton()).onClick!();
  await until("the start", () => sent.some((s) => s.path === "/api/sessions/new"));
  assert.equal((sent.find((s) => s.path === "/api/sessions/new")?.body as Record<string, unknown>)?.text, "start here");
  machineFeedFor(LOCAL).store.publish({ sessions: [summary({}), summary({ id: "new-1" })] });
  await flush();
  assert.deepEqual(changes, [{ kind: "session", source: "claude", id: "new-1" }]);
  assert.deepEqual(errors, []);
});

test("the draft — text, uploaded attachments and choices — survives a reload, per device", async (t) => {
  const sent: Sent[] = [];
  reset(sent);
  const first_ = await mount("/sessions/new?cwd=%2Fhome%2Fdemo%2Fother");
  await until("the view", () => !!textarea());
  type("keep this for later");
  paste(picture());
  await until("the upload in the draft", () => readNewSessionDraft(localStorage, LOCAL).attachments.length === 1);
  first_.root.unmount();

  const kept = readNewSessionDraft(localStorage, LOCAL);
  assert.equal(kept.text, "keep this for later");
  assert.deepEqual(kept.attachments, [{ path: `${UPLOADS}/shot.png`, name: "shot.png", isImage: true }]);
  assert.equal(kept.cwd, "/home/demo/other", "the directory it was opened in");

  const second = await mount("/sessions/new");
  t.after(() => second.root.unmount());
  await until("the restored picture", () => !!first((n) => n.getAttribute("data-attachment") === "image"));
  assert.equal(textarea()?.getAttribute("value") ?? propsOf(textarea()).value, "keep this for later");
  assert.ok(first((n) => n.getAttribute("data-attachment") === "image"), "the picture is back, from the device");
  assert.ok(first((n) => n.getAttribute("data-directory") === "/home/demo/other"), "and so is the directory");
  assert.equal(sent.filter((s) => s.path === "/api/uploads").length, 1, "without uploading it again");
  assert.deepEqual(second.errors, []);
});

// ---------------------------------------------------------------- the local login

/** A stored claude account, as `GET /api/accounts` lists it. */
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
const WEEKLY_REST = { ...AUTH_REST, class: "weekly", untilAt: "2099-01-01T00:00:00Z", reason: "weekly limit" };
/** The user's own login, signed out: a turn on it was refused for its credential. */
const SIGNED_OUT = { claude: { state: "signed_out", rest: [AUTH_REST], isDefault: false }, codex: { state: "unknown", rest: [], isDefault: false } };
const ACCOUNT_A = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const ACCOUNT_B = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

const accountChip = () => first((n) => n.nodeName === "BUTTON" && n.getAttribute("aria-label") === "account");
const notice = () => first((n) => n.hasAttribute("data-local-signed-out"));
/** Wait for the page to show something, rather than for a fixed time: the device answers when it answers.
 * Up to 10 s: it returns as soon as the page shows it, and a busy CI runner can take more than 2 s. */
async function until(what: string, check: () => boolean) {
  for (let i = 0; i < 500; i++) {
    if (check()) return;
    await flush(20);
  }
  assert.fail(`timed out waiting for ${what}`);
}

const startBody = (sent: Sent[]) => sent.find((s) => s.path === "/api/sessions/new")?.body as Record<string, unknown>;

test("a local login known to be signed out is never the default: an untouched pick shows the first usable account and leaves the choice to the server", async (t) => {
  const sent: Sent[] = [];
  reset(sent, "start", {
    // A is resting on everything, so B is the one a session can run on.
    accounts: [account(ACCOUNT_A, "work", [WEEKLY_REST]), account(ACCOUNT_B, "spare")],
    defaultAccountId: null, defaultCodexAccountId: null, local: SIGNED_OUT,
  });
  const { root, errors } = await mount("/sessions/new");
  t.after(() => root.unmount());
  await until("the account list", () => /spare/.test(accountChip()?.textContent ?? ""));
  assert.match(accountChip()?.textContent ?? "", /spare/, "the chip names the account it will start on");
  assert.equal(notice(), undefined, "nothing to warn about: it is not starting on the local login");
  type("go on");
  await flush();
  propsOf(sendButton()).onClick!();
  await until("the start", () => !!startBody(sent));
  assert.equal("accountId" in startBody(sent), false, "untouched: the server's start rule decides, and logs the stand-in");
  assert.deepEqual(errors, []);
});

test("the new session shows where the server says it will start, and names no account unless one is picked", async (t) => {
  const sent: Sent[] = [];
  // The list alone would say `work` (the first free pool account); the server
  // ranks by headroom and says `spare`.
  reset(
    sent,
    "start",
    { accounts: [account(ACCOUNT_A, "work"), account(ACCOUNT_B, "spare")], defaultAccountId: null, defaultCodexAccountId: null, local: SIGNED_OUT },
    { accountId: ACCOUNT_B, reason: "fallback" },
  );
  const { root, errors } = await mount("/sessions/new");
  t.after(() => root.unmount());
  await until("the server's answer", () => /spare/.test(accountChip()?.textContent ?? ""));
  type("go on");
  await flush();
  propsOf(sendButton()).onClick!();
  await until("the start", () => !!startBody(sent));
  assert.equal("accountId" in startBody(sent), false, "shown, not sent: the start is the server's to place");
  assert.deepEqual(errors, []);
});

test("nothing stored: the new session sends the model the server paired with the login it shows, and names no account", async (t) => {
  const sent: Sent[] = [];
  const list = (...ids: string[]) => ({ models: ids.map((id) => ({ id, label: null, efforts: null, defaultEffort: null })), source: "cache" });
  reset(
    sent,
    "start",
    { accounts: [account(ACCOUNT_A, "work"), account(ACCOUNT_B, "spare")], defaultAccountId: ACCOUNT_A, defaultCodexAccountId: null, local: SIGNED_OUT },
    // `work` rests on opus, which its probed list leads with; the server pairs it with fable.
    (query: URLSearchParams) =>
      query.get("chooseModel") === "true"
        ? { accountId: ACCOUNT_A, reason: "default", model: "fable", placed: true }
        : { accountId: ACCOUNT_B, reason: "rotated", model: null, placed: true },
    { "/api/models": (query: URLSearchParams) => (query.get("accountId") === ACCOUNT_A ? list("opus[1m]", "fable") : list("fable", "opus")) },
  );
  const { root, errors } = await mount("/sessions/new");
  t.after(() => root.unmount());
  await until("the pair", () => /work/.test(accountChip()?.textContent ?? ""));
  type("go on");
  await flush();
  propsOf(sendButton()).onClick!();
  await until("the start", () => !!startBody(sent));
  assert.equal(startBody(sent).model, "fable", "the pair's model: opus on `work` would land on `spare`");
  assert.equal("accountId" in startBody(sent), false);
  assert.deepEqual(errors, []);
});

test("a new session sent before the server's answer is held, then started on the answer", async (t) => {
  const sent: Sent[] = [];
  let answer: (value: unknown) => void = () => undefined;
  const late = new Promise((resolve) => (answer = resolve));
  reset(
    sent,
    "start",
    { accounts: [account(ACCOUNT_A, "work"), account(ACCOUNT_B, "spare")], defaultAccountId: ACCOUNT_A, defaultCodexAccountId: null, local: SIGNED_OUT },
    () => late,
  );
  const { root, errors } = await mount("/sessions/new");
  t.after(() => {
    answer({ accountId: ACCOUNT_A, reason: "default", model: null, placed: true });
    root.unmount();
  });
  await until("the view", () => !!textarea());
  type("restored draft, sent at once");
  await flush();
  propsOf(sendButton()).onClick!();
  await flush(60);
  assert.equal(startBody(sent), undefined, "held: nothing shown yet is what it would start on");

  answer({ accountId: ACCOUNT_B, reason: "rotated", model: "fable", placed: true });
  await until("the held start", () => !!startBody(sent));
  assert.equal(startBody(sent).model, "fable", "started on the answer's pair");
  assert.equal("accountId" in startBody(sent), false);
  assert.deepEqual(errors, []);
});

test("a pair no model-less start reaches is sent with its login named as the start rule's, not as a pick", async (t) => {
  const sent: Sent[] = [];
  reset(
    sent,
    "start",
    { accounts: [account(ACCOUNT_A, "work"), account(ACCOUNT_B, "spare")], defaultAccountId: ACCOUNT_A, defaultCodexAccountId: null, local: SIGNED_OUT },
    { accountId: ACCOUNT_B, reason: "rotated", model: "fable", placed: false },
  );
  const { root, errors } = await mount("/sessions/new");
  t.after(() => root.unmount());
  await until("the pair", () => /spare/.test(accountChip()?.textContent ?? ""));
  type("go on");
  await flush();
  propsOf(sendButton()).onClick!();
  await until("the start", () => !!startBody(sent));
  assert.equal(startBody(sent).accountId, ACCOUNT_B);
  assert.equal(startBody(sent).accountFrom, "start", "the rule's choice: the server logs it so and checks it again");
  assert.equal(startBody(sent).model, "fable");
  assert.deepEqual(errors, []);
});

test("an answer the rule has moved on from by the press is asked again: the new one is shown, and the start goes out on it", async (t) => {
  const sent: Sent[] = [];
  let moved = false;
  let askedAfter = 0;
  reset(
    sent,
    "start",
    { accounts: [account(ACCOUNT_A, "work"), account(ACCOUNT_B, "spare")], defaultAccountId: ACCOUNT_A, defaultCodexAccountId: null, local: SIGNED_OUT },
    // On screen since the page opened: `work`. By the press, the rule says `spare`.
    () => {
      if (!moved) return { accountId: ACCOUNT_A, reason: "default", model: null, placed: true };
      askedAfter++;
      return { accountId: ACCOUNT_B, reason: "rotated", model: "sonnet", placed: true };
    },
  );
  const { root, errors } = await mount("/sessions/new");
  t.after(() => root.unmount());
  await until("the first answer", () => /work/.test(accountChip()?.textContent ?? ""));
  type("go on");
  await flush();
  moved = true;
  propsOf(sendButton()).onClick!();
  await until("the start", () => !!startBody(sent));
  assert.match(accountChip()?.textContent ?? "", /spare/, "the fresh answer is what the page shows");
  assert.equal(startBody(sent).model, "sonnet", "and what the start carries");
  assert.equal("accountId" in startBody(sent), false);
  assert.ok(askedAfter >= 1, "asked again at the press");
  assert.deepEqual(errors, []);
});

test("a start answer that fails (after the query's retry) releases a held new session to the server's own rule", async (t) => {
  const sent: Sent[] = [];
  let asked = 0;
  reset(
    sent,
    "start",
    { accounts: [account(ACCOUNT_A, "work")], defaultAccountId: ACCOUNT_A, defaultCodexAccountId: null, local: SIGNED_OUT },
    () => {
      asked++;
      throw new Error("the device could not answer");
    },
  );
  const { root, errors } = await mount("/sessions/new", null, PRODUCTION_QUERIES);
  t.after(() => root.unmount());
  await until("the view", () => !!textarea());
  type("go on anyway");
  await flush();
  propsOf(sendButton()).onClick!();
  await until("the released start", () => !!startBody(sent));
  assert.equal(asked >= 2, true, "asked, and asked again, before giving up");
  assert.equal("accountId" in startBody(sent), false, "no answer to name a login from: the server places it at the spawn");
  assert.deepEqual(errors, []);
});

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

test("an account picked here is sent as the session's own, whatever the server would have chosen", async (t) => {
  const sent: Sent[] = [];
  reset(
    sent,
    "start",
    { accounts: [account(ACCOUNT_A, "work"), account(ACCOUNT_B, "spare")], defaultAccountId: null, defaultCodexAccountId: null, local: SIGNED_OUT },
    { accountId: ACCOUNT_B, reason: "fallback" },
  );
  const { root, errors } = await mount("/sessions/new");
  t.after(() => root.unmount());
  await until("the server's answer", () => /spare/.test(accountChip()?.textContent ?? ""));
  componentProps(accountChip(), "AccountPicker").onSelect!(ACCOUNT_A);
  await until("the pick", () => /work/.test(accountChip()?.textContent ?? ""));
  type("on work");
  await flush();
  propsOf(sendButton()).onClick!();
  await until("the start", () => !!startBody(sent));
  assert.equal(startBody(sent).accountId, ACCOUNT_A);
  assert.deepEqual(errors, []);
});

test("a local login picked by an earlier start is not carried into a session once the login is signed out", async (t) => {
  const sent: Sent[] = [];
  reset(sent, "start", {
    accounts: [account(ACCOUNT_A, "work")], defaultAccountId: null, defaultCodexAccountId: null, local: SIGNED_OUT,
  });
  // The draft remembers the local login, picked while it still worked.
  writeNewSessionDraft(localStorage, LOCAL, { ...readNewSessionDraft(localStorage, LOCAL), text: "next", account: "" });
  const { root } = await mount("/sessions/new");
  t.after(() => root.unmount());
  await until("the account list", () => /work/.test(accountChip()?.textContent ?? ""));
  propsOf(sendButton()).onClick!();
  await until("the start", () => !!startBody(sent));
  assert.equal("accountId" in startBody(sent), false, "the stale pick is dropped: untouched, the server decides");
});

test("with no account to fall back on, starting on a signed-out local login is still possible — and says how to fix it", async (t) => {
  const sent: Sent[] = [];
  reset(sent, "start", { ...NO_ACCOUNTS, local: SIGNED_OUT });
  const { root, errors } = await mount("/sessions/new");
  t.after(() => root.unmount());
  await until("the warning", () => !!notice());
  const warned = notice();
  assert.ok(warned, "a warning above the composer");
  assert.match(warned.textContent, /claude login on this device is signed out: run `claude` in a terminal on this device, then `\/login`/);
  assert.match(warned.textContent, /or add an account · Settings/);
  type("try anyway");
  await flush();
  propsOf(sendButton()).onClick!();
  await until("the start", () => !!startBody(sent));
  assert.equal("accountId" in startBody(sent), false, "untouched: the server starts it on the local login, its only login");
  assert.deepEqual(errors, []);
});

test("a local login nobody knows anything about is offered exactly as before", async (t) => {
  const sent: Sent[] = [];
  reset(sent, "start", {
    accounts: [account(ACCOUNT_A, "work")], defaultAccountId: null, defaultCodexAccountId: null,
    local: { claude: { state: "unknown", rest: [] }, codex: { state: "unknown", rest: [] } },
  });
  const { root } = await mount("/sessions/new");
  t.after(() => root.unmount());
  await until("the account chip", () => !!accountChip());
  assert.match(accountChip()?.textContent ?? "", /^local login$/);
  assert.equal(accountChip()?.getAttribute("data-signal"), null);
  assert.equal(notice(), undefined);
});

// ---------------------------------------------------------------- the layout

/**
 * Sessions in `n` directories, the most recently started first: an hour apart,
 * the first an hour ago. Counted back from now rather than from a fixed date,
 * since a row says how long ago it was used — a fixed date reads "2d" two days
 * after the test was written, where the row is asserted to say hours.
 */
function inDirectories(n: number): SessionSummary[] {
  return Array.from({ length: n }, (_, i) => {
    const at = new Date(Date.now() - (i + 1) * 3_600_000).toISOString();
    return summary({ id: `d-${i}`, cwd: `/home/demo/work/dir-${i}`, createdAt: at, updatedAt: at });
  });
}
const view = () => first((n) => n.hasAttribute("data-new-session"));
const rows = () => all().filter((n) => n.hasAttribute("data-directory"));
const picked = () => rows().find((n) => n.getAttribute("aria-checked") === "true")?.getAttribute("data-directory");
const button = (text: RegExp) => first((n) => n.nodeName === "BUTTON" && text.test(n.textContent));
/** A key pressed inside the view, as React hands it to the view's handler. */
function key(target: FakeNode | undefined, over: Record<string, unknown>): { prevented: boolean } {
  const seen = { prevented: false };
  propsOf(view()).onKeyDown!({
    key: "ArrowDown", altKey: false, metaKey: false, ctrlKey: false, shiftKey: false, keyCode: 40,
    nativeEvent: { isComposing: false }, target, preventDefault: () => void (seen.prevented = true), ...over,
  });
  return seen;
}

test("one column: the question, the composer under it, the directories under that — and the header names no directory", async (t) => {
  const sent: Sent[] = [];
  reset(sent);
  const { root, errors } = await mount("/sessions/new");
  t.after(() => root.unmount());
  await until("the directory list", () => rows().length === 1);
  const parts = view()!.childNodes;
  assert.equal(parts[0]?.nodeName, "HEADER");
  assert.doesNotMatch(parts[0]!.textContent, /home\/demo/, "no path strip: the folder chip and the list say where");
  assert.equal(parts[1]?.getAttribute("aria-hidden"), "true", "the room above the question");
  // Sized by the view, never by the composer: typing or pasting moves nothing above the composer.
  assert.equal(parts[1]?.style.height, "max(1rem, calc(50% - 12rem))");
  assert.ok(parts[2]?.hasAttribute("data-new-session-title"));
  assert.match(parts[2]!.textContent, /Where do you want to start\?/);
  assert.ok(parts[3]?.hasAttribute("data-new-session-composer"));
  assert.ok(walk(parts[3]!).some((n) => n.nodeName === "TEXTAREA"), "the composer");
  assert.match(parts[3]!.getAttribute("class") ?? "", /max-md:order-last/, "a phone pins it to the bottom, under the directories");
  assert.ok(walk(parts[4]!).some((n) => n.hasAttribute("data-recent-directories")), "the directories, under it");
  for (const part of [parts[2]!, parts[3]!, parts[4]!]) {
    assert.match(part.childNodes[0]?.getAttribute("class") ?? "", /mx-auto w-full max-w-\[48rem\]/, "the one column, the transcript's measure");
  }
  assert.deepEqual(errors, []);
});

test("the directories: one line each, six at first and then all of them, the pick marked as a radio group marks it", async (t) => {
  const sent: Sent[] = [];
  reset(sent);
  machineFeedFor(LOCAL).store.publish({ sessions: inDirectories(9) });
  const { root, errors } = await mount("/sessions/new");
  t.after(() => root.unmount());
  await until("the directory list", () => rows().length > 0);
  assert.equal(rows().length, 6);
  assert.ok(rows().every((row) => row.getAttribute("role") === "radio"));
  assert.equal(first((n) => n.getAttribute("role") === "radiogroup")?.getAttribute("aria-label"), "working directory");
  assert.equal(picked(), "/home/demo/work/dir-0", "the one last started in");
  assert.deepEqual(rows().map((row) => row.getAttribute("tabindex")), ["0", "-1", "-1", "-1", "-1", "-1"], "Tab lands on the pick");
  const row = rows()[2]!;
  assert.doesNotMatch(row.textContent, /session/, "no session count on the row");
  assert.match(row.getAttribute("title") ?? "", /^\/home\/demo\/work\/dir-2 · 1 session$/, "it is in the row's title");
  assert.match(row.textContent, /^dir-2\/home\/demo\/work\d+h$/, "name, where it is (the parent), when it was last used");

  propsOf(button(/^show all \(9\)$/)).onClick!();
  await until("every directory", () => rows().length === 9);
  assert.ok(button(/^show fewer$/));
  propsOf(rows()[7]).onClick!();
  await until("the pick", () => picked() === "/home/demo/work/dir-7");
  propsOf(button(/^show fewer$/)).onClick!();
  await until("the shorter list", () => rows().length === 7);
  assert.equal(rows()[6]?.getAttribute("data-directory"), "/home/demo/work/dir-7", "the pick stays on screen");
  assert.equal(readNewSessionDraft(localStorage, LOCAL).cwd, "/home/demo/work/dir-7");
  assert.deepEqual(errors, []);
});

test("↑/↓ move the pick: in the list, and from the composer while nothing is typed or with Alt — Enter stays send", async (t) => {
  const sent: Sent[] = [];
  reset(sent);
  machineFeedFor(LOCAL).store.publish({ sessions: inDirectories(4) });
  const { root, errors } = await mount("/sessions/new");
  t.after(() => root.unmount());
  await until("the directory list", () => rows().length === 4);
  assert.equal(key(textarea(), {}).prevented, true, "the caret has nowhere to go in an empty message");
  await until("the next directory", () => picked() === "/home/demo/work/dir-1");
  type("look at the logs");
  await flush();
  assert.equal(key(textarea(), {}).prevented, false, "with words typed, ↓ is the caret's");
  await flush();
  assert.equal(picked(), "/home/demo/work/dir-1");
  assert.equal(key(textarea(), { altKey: true }).prevented, true, "Alt+↓ is always the list's");
  await until("the one after", () => picked() === "/home/demo/work/dir-2");
  assert.equal(key(textarea(), { key: "Enter", keyCode: 13 }).prevented, false, "Enter is the composer's: send");
  // In the list, as in any radio group: the pick moves and the focus with it.
  const focused: (string | null)[] = [];
  for (const row of rows()) row.focus = () => void focused.push(row.getAttribute("data-directory"));
  assert.equal(key(rows().find((row) => row.getAttribute("aria-checked") === "true"), { key: "ArrowUp", keyCode: 38 }).prevented, true);
  await until("the one before", () => picked() === "/home/demo/work/dir-1");
  assert.deepEqual(focused, ["/home/demo/work/dir-1"]);
  assert.equal(key(textarea(), { key: "ArrowUp", keyCode: 38, altKey: true, metaKey: true }).prevented, false, "other chords are left alone");
  assert.equal(sent.filter((s) => s.path === "/api/sessions/new").length, 0);
  assert.deepEqual(errors, []);
});

test("the folder chip is where the directory shows by the composer; it takes you to the list, on the pick", async (t) => {
  const sent: Sent[] = [];
  reset(sent);
  machineFeedFor(LOCAL).store.publish({ sessions: inDirectories(3) });
  const { root } = await mount("/sessions/new");
  t.after(() => root.unmount());
  await until("the directory list", () => rows().length === 3);
  const chip = first((n) => n.getAttribute("data-chip") === "directory");
  assert.match(chip?.textContent ?? "", /dir-0/);
  const focused: (string | null)[] = [];
  for (const row of rows()) row.focus = () => void focused.push(row.getAttribute("data-directory"));
  propsOf(chip).onClick!();
  assert.deepEqual(focused, ["/home/demo/work/dir-0"]);
});

test("any other directory: a path typed in is the pick, a row of its own", async (t) => {
  const sent: Sent[] = [];
  reset(sent);
  const { root } = await mount("/sessions/new");
  t.after(() => root.unmount());
  await until("the directory list", () => rows().length === 1);
  propsOf(button(/^choose another directory…$/)).onClick!();
  await until("the path field", () => !!first((n) => n.nodeName === "INPUT" && n.getAttribute("aria-label") === "working directory"));
  propsOf(first((n) => n.nodeName === "INPUT" && n.getAttribute("aria-label") === "working directory")).onChange!({ target: { value: "/srv/elsewhere" } });
  await flush();
  propsOf(first((n) => n.nodeName === "FORM")).onSubmit!({ preventDefault() {} });
  await until("the typed pick", () => picked() === "/srv/elsewhere");
  assert.equal(rows().length, 2, "the recent one, and the one typed");
  assert.match(first((n) => n.getAttribute("data-chip") === "directory")?.textContent ?? "", /elsewhere/);
});

test("opened with a folder, a message and an agent (`?cwd=…&draft=…&agent=…`): all filled in, nothing sent, the address left clean", async (t) => {
  const sent: Sent[] = [];
  reset(sent);
  writeNewSessionDraft(localStorage, LOCAL, { ...readNewSessionDraft(localStorage, LOCAL), text: "an older draft" });
  const { root, routes, errors, settle } = await mount("/sessions/new?cwd=%2Fsrv%2Fpractice&draft=Explain%20this%20project&agent=codex");
  t.after(() => root.unmount());
  await until("the message", () => !!textarea() && propsOf(textarea()).value === "Explain this project");
  await until("the address without it", () => routes.state.location.searchStr === "");
  await settle();
  await until("the view again", () => !!textarea());
  assert.equal(propsOf(textarea()).value, "Explain this project", "kept once the address forgets it");
  assert.equal(picked(), "/srv/practice");
  const draft = readNewSessionDraft(localStorage, LOCAL);
  assert.deepEqual([draft.text, draft.cwd, draft.source], ["Explain this project", "/srv/practice", "codex"]);
  assert.equal(sent.filter((s) => s.path === "/api/sessions/new").length, 0, "the person sends it");
  assert.deepEqual(errors, []);
});

test("useOpenNewSession({ cwd, draft, source }) opens the view filled in", async (t) => {
  const sent: Sent[] = [];
  reset(sent);
  let open: ((options?: { cwd?: string; draft?: string; source?: "claude" | "codex" }) => void) | null = null;
  function Opener() {
    open = useOpenNewSession(LOCAL);
    return null;
  }
  const { root, routes, settle } = await mount("/sessions", <Opener />);
  t.after(() => root.unmount());
  await until("the opener", () => open !== null);
  open!({ cwd: "/srv/first", draft: "Say hello", source: "claude" });
  await until("the new-session address", () => routes.state.location.pathname === "/sessions/new");
  await settle();
  await until("the message", () => !!textarea() && propsOf(textarea()).value === "Say hello");
  await until("the draft", () => readNewSessionDraft(localStorage, LOCAL).cwd === "/srv/first");
  assert.equal(sent.filter((s) => s.path === "/api/sessions/new").length, 0);
});

test("while it starts, the message waits where the directories were; the composer does not move", async (t) => {
  const sent: Sent[] = [];
  reset(sent);
  const { root } = await mount("/sessions/new");
  t.after(() => root.unmount());
  await until("the view", () => !!textarea());
  type("start here");
  await flush();
  propsOf(sendButton()).onClick!();
  await until("the starting line", () => !!first((n) => n.hasAttribute("data-starting")));
  const parts = view()!.childNodes;
  assert.ok(!walk(parts[3]!).some((n) => n.hasAttribute("data-starting") || /start here/.test(n.textContent) && n.nodeName !== "TEXTAREA" && n.hasAttribute("aria-live")), "nothing pushed in over the composer");
  assert.ok(walk(parts[4]!).some((n) => n.getAttribute("aria-live") === "polite" && /start here/.test(n.textContent)), "the pending message, under it");
});

const MAC = /Mac|iPhone|iPad|iPod/.test(navigator.platform || navigator.userAgent);
const MOD = MAC ? "metaKey" : "ctrlKey";

/** Press a key on the page, as the browser would hand it to the window's listeners. */
function press(over: Record<string, unknown>): { prevented: boolean } {
  const seen = { prevented: false };
  for (const { type, fn } of [...windowListeners]) {
    if (type === "keydown") fn({ key: "n", code: "KeyN", metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, defaultPrevented: false, target: null, preventDefault: () => void (seen.prevented = true), ...over });
  }
  return seen;
}

test("in a browser, Cmd+Shift+O (Ctrl+Shift+O elsewhere) opens the new-session view, and Cmd+N is left to the browser", async (t) => {
  const sent: Sent[] = [];
  reset(sent);
  windowListeners.length = 0;
  // The hotkey lives in the shell, beside the routed view; here, beside the router.
  const { root, routes, settle } = await mount("/sessions", <NewSessionHotkey here={LOCAL} desktop={false} />);
  t.after(() => root.unmount());
  await until("the hotkey", () => windowListeners.some((l) => l.type === "keydown"));
  assert.equal(press({ [MOD]: true }).prevented, false, "Cmd/Ctrl+N is the browser's new window");
  assert.equal(press({ key: "N", shiftKey: true, [MOD]: true }).prevented, false, "Cmd/Ctrl+Shift+N is its private window");
  await flush();
  assert.equal(routes.state.location.pathname, "/sessions");
  assert.equal(press({ key: "O", code: "KeyO", shiftKey: true, [MOD]: true }).prevented, true, "taken, so the browser does nothing else with it");
  await until("the new-session address", () => routes.state.location.pathname === "/sessions/new");
  await settle();
  await until("the view", () => !!first((n) => n.getAttribute("data-new-session") === LOCAL));
  assert.equal(routes.state.location.pathname, "/sessions/new");
  assert.ok(first((n) => n.getAttribute("data-new-session") === LOCAL));
});

test("in the desktop app's window, Cmd+N (Ctrl+N elsewhere) opens it too", async (t) => {
  const sent: Sent[] = [];
  reset(sent);
  windowListeners.length = 0;
  const { root, routes, settle } = await mount("/sessions", <NewSessionHotkey here={LOCAL} desktop />);
  t.after(() => root.unmount());
  await until("the hotkey", () => windowListeners.some((l) => l.type === "keydown"));
  assert.equal(press({ shiftKey: true, [MOD]: true }).prevented, false, "Shift with it is not the chord");
  assert.equal(press({ [MOD]: true }).prevented, true);
  await until("the new-session address", () => routes.state.location.pathname === "/sessions/new");
  await settle();
  await until("the view", () => !!first((n) => n.getAttribute("data-new-session") === LOCAL));
  assert.equal(routes.state.location.pathname, "/sessions/new");
  assert.ok(first((n) => n.getAttribute("data-new-session") === LOCAL));
});

test("the new-session dialog is gone: nothing in the app opens one", () => {
  const src = fileURLToPath(new URL("../src", import.meta.url));
  assert.equal(existsSync(join(src, "features/sessions/new-session-dialog.tsx")), false);
  const files = (dir: string): string[] =>
    readdirSync(dir).flatMap((name) => {
      const path = join(dir, name);
      return statSync(path).isDirectory() ? files(path) : /\.tsx?$/.test(name) ? [path] : [];
    });
  const mentions = files(src).filter((path) => /NewSessionDialog|new-session-dialog/.test(readFileSync(path, "utf8")));
  assert.deepEqual(mentions, []);
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

test("switching the CLI while a start waits for its answer drops the start: it never goes to the other CLI", async (t) => {
  const sent: Sent[] = [];
  let answer: (value: unknown) => void = () => undefined;
  const late = new Promise((resolve) => (answer = resolve));
  reset(
    sent,
    "start",
    { accounts: [account(ACCOUNT_A, "work")], defaultAccountId: ACCOUNT_A, defaultCodexAccountId: null, local: SIGNED_OUT },
    () => late,
  );
  const { root, errors } = await mount("/sessions/new");
  t.after(() => {
    answer({ accountId: ACCOUNT_A, reason: "default", model: null, placed: true });
    root.unmount();
  });
  await until("the view", () => !!textarea());
  type("for claude");
  await flush();
  propsOf(sendButton()).onClick!();
  await until("the hold", () => !!first((n) => n.hasAttribute("data-choosing-account")));

  const cli = first((n) => n.nodeName === "BUTTON" && n.getAttribute("aria-label") === "cli");
  propsWith(cli, "onChange").onChange!("codex");
  await until("the hold dropped", () => !first((n) => n.hasAttribute("data-choosing-account")));
  answer({ accountId: ACCOUNT_A, reason: "default", model: null, placed: true });
  await flush(120);
  assert.equal(startBody(sent), undefined, "pressed for claude, never sent as codex");
  assert.deepEqual(errors, []);
});

test("an answer that never comes holds a start a few seconds at most: then it goes out for the server to place", async (t) => {
  const sent: Sent[] = [];
  reset(
    sent,
    "start",
    { accounts: [account(ACCOUNT_A, "work"), account(ACCOUNT_B, "spare")], defaultAccountId: ACCOUNT_A, defaultCodexAccountId: null, local: SIGNED_OUT },
    () => new Promise(() => undefined),
  );
  const { root, errors } = await mount("/sessions/new");
  t.after(() => root.unmount());
  await until("the view", () => !!textarea());
  type("go on");
  await flush();
  const pressed = Date.now();
  propsOf(sendButton()).onClick!();
  for (let i = 0; i < 300 && !startBody(sent); i++) await flush(50);
  assert.ok(startBody(sent), "sent once the hold ran out");
  assert.ok(Date.now() - pressed >= 7_500, "after the hold, not before");
  assert.equal("accountId" in startBody(sent), false, "with no account: the server places it");
  assert.deepEqual(errors, []);
});

/** A start answer that waits until released, and the list it is asked against. */
function heldStart(answer: unknown = { accountId: ACCOUNT_A, reason: "default", model: null, placed: true }) {
  let release: () => void = () => undefined;
  const gate = new Promise<void>((resolve) => (release = resolve));
  let asks = 0;
  return {
    route: async () => {
      // The first answer comes at once; the ones asked again at the press wait.
      if (++asks > 1) await gate;
      return answer;
    },
    release: () => release(),
    asks: () => asks,
  };
}
const HELD_ACCOUNTS = {
  accounts: [account(ACCOUNT_A, "work"), account(ACCOUNT_B, "spare")], defaultAccountId: ACCOUNT_A, defaultCodexAccountId: null, local: SIGNED_OUT,
};
const holding = () => !!first((n) => n.hasAttribute("data-choosing-account"));

test("emptying the composer while a start is held calls it off: the next keystroke sends nothing", async (t) => {
  const sent: Sent[] = [];
  const held = heldStart();
  reset(sent, "start", HELD_ACCOUNTS, held.route);
  const { root, errors } = await mount("/sessions/new");
  t.after(() => {
    held.release();
    root.unmount();
  });
  await until("the answer", () => /work/.test(accountChip()?.textContent ?? ""));
  type("start this");
  await flush();
  propsOf(sendButton()).onClick!();
  await until("the hold", holding);

  type("");
  await until("the hold dropped", () => !holding());
  type("x");
  held.release();
  await flush(150);
  assert.equal(startBody(sent), undefined, "called off: no session from what was typed after");
  assert.equal(textarea() && propsOf(textarea()).value as unknown, "x", "and the composer keeps what is being typed");
  assert.deepEqual(errors, []);
});

test("what was pressed is what goes out: edits while the start is held do not change it", async (t) => {
  const sent: Sent[] = [];
  const held = heldStart();
  reset(sent, "start", HELD_ACCOUNTS, held.route);
  const { root, errors } = await mount("/sessions/new");
  t.after(() => {
    held.release();
    root.unmount();
  });
  await until("the answer", () => /work/.test(accountChip()?.textContent ?? ""));
  type("the message as pressed");
  await flush();
  propsOf(sendButton()).onClick!();
  await until("the hold", holding);
  type("the message as pressed, and then some more typed while it waited");
  await flush();
  held.release();
  await until("the start", () => !!startBody(sent));
  assert.equal(startBody(sent).text, "the message as pressed");
  assert.deepEqual(errors, []);
});

test("what is typed and added after the press stays in the composer when the held start goes out", async (t) => {
  const sent: Sent[] = [];
  const held = heldStart();
  reset(sent, "start", HELD_ACCOUNTS, held.route);
  const { root, errors } = await mount("/sessions/new");
  t.after(() => {
    held.release();
    root.unmount();
  });
  await until("the answer", () => /work/.test(accountChip()?.textContent ?? ""));
  type("first");
  await flush();
  propsOf(sendButton()).onClick!();
  await until("the hold", holding);
  // After the press: more text, and a picture.
  type("first, and the next thing to ask");
  paste(picture());
  await until("the picture uploaded", () => sent.some((s) => s.path === "/api/uploads"));
  await flush();
  held.release();
  await until("the start", () => !!startBody(sent));
  assert.equal(startBody(sent).text, "first");
  assert.equal(startBody(sent).images, undefined, "the picture came after the press: not sent");
  await flush();
  assert.equal(propsOf(textarea()).value as unknown, "first, and the next thing to ask", "the later text stays");
  assert.equal(all().filter((n) => n.getAttribute("data-attachment") === "image").length, 1, "and so does the later picture");
  const kept = readNewSessionDraft(localStorage, LOCAL);
  assert.equal(kept.text, "first, and the next thing to ask", "kept in the draft, for when the view goes");
  assert.equal(kept.attachments.length, 1);
  assert.deepEqual(errors, []);
});

test("the text the press carried cleared while it is held calls it off, even with its picture still there", async (t) => {
  const sent: Sent[] = [];
  const held = heldStart();
  reset(sent, "start", HELD_ACCOUNTS, held.route);
  const { root, errors } = await mount("/sessions/new");
  t.after(() => {
    held.release();
    root.unmount();
  });
  await until("the answer", () => /work/.test(accountChip()?.textContent ?? ""));
  type("about this picture");
  paste(picture());
  await until("the picture uploaded", () => sent.some((s) => s.path === "/api/uploads"));
  await flush();
  propsOf(sendButton()).onClick!();
  await until("the hold", holding);
  type("");
  await until("the hold dropped", () => !holding());
  held.release();
  await flush(150);
  assert.equal(startBody(sent), undefined, "the message was taken back: nothing goes out");
  assert.deepEqual(errors, []);
});

test("an attachment that fails while its start is held calls the start off; the next press asks afresh", async (t) => {
  const sent: Sent[] = [];
  let failUpload: () => void = () => undefined;
  const upload = new Promise<void>((resolve) => (failUpload = resolve));
  let asks = 0;
  reset(
    sent,
    "start",
    HELD_ACCOUNTS,
    () => {
      asks++;
      return { accountId: ACCOUNT_B, reason: "rotated", model: null, placed: false };
    },
    { "/api/uploads": async () => (await upload, Response.json({ error: "disk full" }, { status: 500 })) },
  );
  const { root, errors } = await mount("/sessions/new");
  t.after(() => {
    failUpload();
    root.unmount();
  });
  await until("the answer", () => /spare/.test(accountChip()?.textContent ?? ""));
  type("with a picture");
  paste(picture());
  await flush();
  propsOf(sendButton()).onClick!();
  await flush();
  failUpload();
  const remove = () => first((n) => n.getAttribute("title") === "remove attachment");
  await until("the failed upload", () => /disk full|upload failed/.test((container as FakeNode).textContent) || !!remove());
  await flush(60);
  assert.equal(startBody(sent), undefined, "called off with its attachment");

  propsOf(remove()).onClick!();
  await flush();
  const before = asks;
  propsOf(sendButton()).onClick!();
  await until("the start", () => !!startBody(sent));
  assert.ok(asks > before, "asked afresh at the new press");
  assert.equal(startBody(sent).accountId, ACCOUNT_B, "on the answer, not unplaced by a hold left over");
  assert.equal(startBody(sent).accountFrom, "start");
  assert.deepEqual(errors, []);
});

test("switching the device while a start is held drops it", async (t) => {
  const sent: Sent[] = [];
  const held = heldStart();
  reset(sent, "start", HELD_ACCOUNTS, held.route);
  // A second device, so the composer offers the device picker at all.
  const other: Sent[] = [];
  registry.openMachine("studio-2", device(other, "start", HELD_ACCOUNTS), {
    info: constantStore({ key: "studio-2", name: "studio 2", state: "online" as const, since: 0, canManage: true, live: "live" as const }),
    ensureLive: () => undefined,
    hold: () => () => undefined,
    dispose: () => undefined,
  });
  const { root, errors } = await mount("/sessions/new");
  t.after(() => {
    held.release();
    root.unmount();
    registry.closeMachine("studio-2");
  });
  await until("the answer", () => /work/.test(accountChip()?.textContent ?? ""));
  type("for this device");
  await flush();
  propsOf(sendButton()).onClick!();
  await until("the hold", holding);

  componentProps(first((n) => n.nodeName === "BUTTON" && n.getAttribute("aria-label") === "device"), "DevicePicker").onSelect!(LOCAL);
  await until("the hold dropped", () => !holding());
  held.release();
  await flush(150);
  assert.equal(startBody(sent), undefined, "pressed for one device, never sent once it moved");
  assert.equal(startBody(other), undefined, "nor from the other one");
  assert.deepEqual(errors, []);
});

const { mergeBack, MAX_ATTACHMENTS, useComposerAttachments } = await import("../src/features/sessions/composer-box.js");

test("a refused message's attachments go back first, then as many later ones as fit; none twice", () => {
  const att = (key: string) => ({ key, file: null, name: `${key}.png`, isImage: true, preview: "", status: "ready" as const, path: `/u/${key}` });
  const back = [att("a"), att("b")];
  assert.deepEqual(
    mergeBack(back, [att("c"), att("a"), att("d")]).kept.map((a) => a.key),
    ["a", "b", "c", "d"],
    "the refused ones first, then the later ones in their order, and no attachment twice",
  );
  const later = Array.from({ length: MAX_ATTACHMENTS }, (_, i) => att(`later-${i}`));
  const { kept, dropped } = mergeBack(back, later);
  assert.equal(kept.length, MAX_ATTACHMENTS, "never over the cap");
  assert.deepEqual(kept.slice(0, 2).map((a) => a.key), ["a", "b"], "the message keeps all of its own");
  assert.deepEqual(dropped.map((a) => a.key), ["later-18", "later-19"], "the last later ones are what does not fit");
});

test("a refused held start puts its message back in front: attachments first, text ahead of what was written since, and the draft holds it all", async (t) => {
  const sent: Sent[] = [];
  const held = heldStart();
  reset(sent, "refuse", HELD_ACCOUNTS, held.route);
  const { root, errors } = await mount("/sessions/new");
  t.after(() => {
    held.release();
    root.unmount();
  });
  await until("the answer", () => /work/.test(accountChip()?.textContent ?? ""));
  type("the first question");
  paste(new File(["a"], "pressed.txt", { type: "text/plain" }));
  await until("the pressed upload", () => sent.filter((s) => s.path === "/api/uploads").length === 1);
  await flush();
  propsOf(sendButton()).onClick!();
  await until("the hold", holding);
  // Written and added after the press.
  type("a second thought");
  paste(new File(["b"], "later.txt", { type: "text/plain" }));
  await until("the later upload", () => sent.filter((s) => s.path === "/api/uploads").length === 2);
  await flush();
  held.release();
  await until("the refusal", () => /could not start the session/.test((container as FakeNode).textContent));
  await flush();

  assert.equal(startBody(sent).text, "the first question");
  assert.equal(propsOf(textarea()).value as unknown, "the first question\na second thought", "the message back in front of what was written since");
  const names = all().filter((n) => n.getAttribute("data-attachment")).map((n) => n.textContent);
  assert.equal(names.length, 2, "both attachments, once each");
  assert.match(names[0] ?? "", /pressed\.txt/, "the refused message's own first");
  assert.match(names[1] ?? "", /later\.txt/, "then the one added since");
  const kept = readNewSessionDraft(localStorage, LOCAL);
  assert.equal(kept.text, "the first question\na second thought", "and the draft holds the merged text");
  assert.deepEqual(kept.attachments.map((a) => a.name), ["pressed.txt", "later.txt"], "and both attachments, in that order");
  assert.deepEqual(errors, []);
});

test("a refused start's attachments always go back; later ones that no longer fit are taken out, and named", async (t) => {
  const sent: Sent[] = [];
  let answerStart: () => void = () => undefined;
  const startGate = new Promise<void>((resolve) => (answerStart = resolve));
  reset(sent, "refuse", HELD_ACCOUNTS, { accountId: ACCOUNT_A, reason: "default", model: null, placed: true }, { "/api/sessions/new": () => startGate });
  const { root, errors } = await mount("/sessions/new");
  t.after(() => {
    answerStart();
    root.unmount();
  });
  await until("the answer", () => /work/.test(accountChip()?.textContent ?? ""));
  type("with one file");
  paste(new File(["a"], "pressed.txt", { type: "text/plain" }));
  await until("the pressed upload", () => sent.filter((s) => s.path === "/api/uploads").length === 1);
  await flush();
  propsOf(sendButton()).onClick!();
  await until("the start sent", () => !!startBody(sent));
  // While the device decides, the composer has room for a full set again.
  const files = Array.from({ length: MAX_ATTACHMENTS }, (_, i) => new File([String(i)], `later-${i}.txt`, { type: "text/plain" }));
  propsOf(textarea()).onPaste!({ clipboardData: { files }, preventDefault() {} });
  await until("the later uploads", () => sent.filter((s) => s.path === "/api/uploads").length === 1 + MAX_ATTACHMENTS);
  await flush();
  answerStart();
  await until("the refusal", () => /could not start the session/.test((container as FakeNode).textContent));
  await flush();

  const names = all().filter((n) => n.getAttribute("data-attachment")).map((n) => n.textContent);
  assert.equal(names.length, MAX_ATTACHMENTS, "never over the cap");
  assert.match(names[0] ?? "", /pressed\.txt/, "the refused message keeps its own attachment, first");
  const notice = first((n) => n.hasAttribute("data-attachments-dropped"));
  assert.ok(notice, "the composer says what it took out");
  assert.match(notice.textContent, /later-19\.txt/);
  assert.doesNotMatch(notice.textContent, /later-18\.txt/, "only what did not fit");
  assert.deepEqual(errors, []);
});

test("a change to the attachments counts the ones before it that React has not drawn yet: a paste right after a press has the room the press made", async (t) => {
  const sent: Sent[] = [];
  reset(sent);
  const view: { attachments?: ReturnType<typeof useComposerAttachments>; elsewhere?: () => void } = {};
  function Composer() {
    view.attachments = useComposerAttachments();
    const [, setOther] = React.useState(0);
    view.elsewhere = () => setOther((n) => n + 1);
    return null;
  }
  /** The attachments as last drawn, and what changes them. */
  const drawn = () => view.attachments!;
  const root = createRoot(container as never);
  t.after(() => root.unmount());
  root.render(
    <MachineProvider machine={LOCAL}>
      <Composer />
    </MachineProvider>,
  );
  await until("the composer", () => !!view.attachments);
  drawn().addFiles([new File(["a"], "pressed.txt", { type: "text/plain" })]);
  await until("the pressed upload", () => drawn().attachments[0]?.status === "ready");
  const pressed = drawn().attachments.map((a) => a.key);
  // In one tick, as a press's start does: another change to the view first —
  // React keeps every change after it for its next render — then the press
  // takes what it carries, and a full set is pasted before that render.
  view.elsewhere!();
  drawn().take(pressed);
  const files = Array.from({ length: MAX_ATTACHMENTS }, (_, i) => new File([String(i)], `later-${i}.txt`, { type: "text/plain" }));
  drawn().addFiles(files);
  const later = () => drawn().attachments.filter((a) => a.name.startsWith("later-"));
  await until("the paste drawn and uploaded", () => later().length > 0 && drawn().attachments.every((a) => a.status === "ready"));
  assert.deepEqual(
    drawn().attachments.map((a) => a.name),
    files.map((f) => f.name),
    "the whole set: the room the press made counts before React draws it",
  );
  assert.equal(sent.filter((s) => s.path === "/api/uploads").length, 1 + MAX_ATTACHMENTS);
});
