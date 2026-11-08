// The Instant Chat panel on a phone: the 「+」 opens the new-chat form, the
// form has a way back to the list, and an empty list's hint leads to the form.
// ChatLayout keeps one column below `md` by hiding the list when the address
// is not `/chat`, so the form needs an address of its own (`/chat/new`) and
// the hint needs to lead there. Rendered for real (jsdom) under a router laid
// out as the app's chat routes are; the hidden column is checked by the class
// that hides it, since jsdom has no layout.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { domTree, installLiveDom, until } from "./live-dom.js";

const container = installLiveDom({ act: true });
// The typed client builds relative URLs (`/api/chats`): give Node's Request an
// origin to resolve them against, as the setup tests do.
(globalThis as Record<symbol, unknown>)[Symbol.for("undici.globalOrigin.1")] = new URL("http://device.test");
const { all, first } = domTree(container);

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
const { ChatLayout } = await import("../src/features/chat/chat-layout.js");
const { ChatHome } = await import("../src/features/chat/chat-home.js");
const { router: appRouter } = await import("../src/app/router.js");
type Transport = import("../src/lib/transport/index.js").Transport;

/** A device that never answers (views wait) and whose socket never opens. */
const silent: Transport = {
  fetch: () => new Promise(() => undefined),
  socketUrl: async (path) => path,
  openSocket: () => ({ readyState: 0, binaryType: "blob", send() {}, close() {}, onopen: null, onmessage: null, onclose: null }),
};

/** A device answering the chat list with `chats`, and nothing else. */
function chatting(chats: unknown[]): Transport {
  return {
    fetch: async (request: Request) => {
      const url = new URL(request.url);
      if (request.method === "GET" && url.pathname === "/api/chats") return Response.json({ chats });
      return Response.json({ error: "not here" }, { status: 404 });
    },
    socketUrl: async (path) => path,
    openSocket: () => ({ readyState: 0, binaryType: "blob", send() {}, close() {}, onopen: null, onmessage: null, onclose: null }),
  };
}

function reset(transport: Transport) {
  registry.setShell("local", LOCAL);
  registry.openMachine(LOCAL, transport, {
    info: constantStore({ key: LOCAL, name: "studio", state: "online" as const, since: 0, canManage: true, live: "live" as const }),
    ensureLive: () => undefined,
    hold: () => () => undefined,
    dispose: () => undefined,
  });
  localStorage.clear();
}

/** The app's chat routes around the real layout and form, at `path`. */
async function mount(path: string, transport: Transport) {
  reset(transport);
  const root = router.createRootRoute({ component: () => <router.Outlet /> });
  const chat = router.createRoute({ getParentRoute: () => root, path: "chat", component: ChatLayout });
  const index = router.createRoute({ getParentRoute: () => chat, path: "/", component: ChatHome });
  const fresh = router.createRoute({ getParentRoute: () => chat, path: "new", component: ChatHome });
  const routes = router.createRouter({
    routeTree: root.addChildren([chat.addChildren([index, fresh])]),
    history: router.createMemoryHistory({ initialEntries: [path] }),
  });
  await routes.load();
  const errors: unknown[] = [];
  const page = createRoot(container as never, {
    onUncaughtError: (error) => void errors.push(error),
    onCaughtError: (error) => void errors.push(error),
  });
  const queries = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  await React.act(async () => {
    page.render(
      <QueryClientProvider client={queries}>
        <MachineProvider machine={LOCAL}>
          <GlobalFeedProvider>
            <router.RouterProvider router={routes} />
          </GlobalFeedProvider>
        </MachineProvider>
      </QueryClientProvider>,
    );
    await new Promise((resolve) => setTimeout(resolve, 30));
  });
  return { page, routes, errors };
}

test("the new-chat form is routed on the home and on another machine", () => {
  // The phone gets to the form by its own address; the route has to exist on
  // both trees (`/chat/new`, `/m/<machine>/chat/new`), beside — not instead
  // of — the chat detail routes.
  assert.ok(appRouter.routesById["/chat/new"], "the home's new-chat route");
  assert.ok(appRouter.routesById["/m/$machine/chat/new"], "another machine's new-chat route");
  assert.ok(appRouter.routesById["/chat/claude/$id"], "the chat detail routes stay");
  assert.ok(appRouter.routesById["/m/$machine/chat/api/$id"]);
});

test("the panel's + targets /chat/new, not the page it is already on", async (t) => {
  const { page, errors } = await mount("/chat", silent);
  t.after(() => page.unmount());
  await until("the + in the panel header", () =>
    !!first((n) => n.nodeName === "A" && n.getAttribute("title") === "new chat"),
  );
  const plus = first((n) => n.nodeName === "A" && n.getAttribute("title") === "new chat");
  assert.equal(plus?.getAttribute("href"), "/chat/new");
  assert.deepEqual(errors, []);
});

test("at /chat/new the form is the open column and the list is the hidden one", async (t) => {
  const { page, errors } = await mount("/chat/new", silent);
  t.after(() => page.unmount());
  await until("the new-chat form", () => !!first((n) => n.nodeName === "TEXTAREA"));
  const panel = all().find((n) => n.nodeName === "ASIDE" && (n.textContent ?? "").includes("Instant Chat"));
  assert.ok(panel, "the list panel is drawn");
  assert.match(panel.className, /max-md:hidden/, "the list gives way to the form below md");
  const main = all().find((n) => n.nodeName === "MAIN");
  assert.ok(main, "the content column is drawn");
  assert.doesNotMatch(main.className, /max-md:hidden/);
  assert.ok(main.querySelector("textarea"), "the form is in the open column");
  const back = first((n) => n.nodeName === "A" && n.getAttribute("title") === "back");
  assert.ok(back, "a phone-only way back to the list");
  assert.equal(back.getAttribute("href"), "/chat");
  assert.match(back.className, /md:hidden/, "hidden on desktop, where both columns show");
  assert.deepEqual(errors, []);
});

test("an empty list's hint links to /chat/new", async (t) => {
  const { page } = await mount("/chat", chatting([]));
  t.after(() => page.unmount());
  await until("the empty-chat hint", () => (container.textContent ?? "").includes("no chats yet"));
  const hint = first((n) => n.nodeName === "A" && (n.textContent ?? "").includes("no chats yet"));
  assert.ok(hint, "the hint is a link, so a phone can reach the form");
  assert.equal(hint.getAttribute("href"), "/chat/new");
});

test("on desktop the list and the form both show, at /chat and /chat/new alike", async (t) => {
  // jsdom has no layout: what is pinned is that the only hiding either column
  // can do is `max-md:` — below the breakpoint — so both stay on desktop.
  for (const path of ["/chat", "/chat/new"]) {
    const { page } = await mount(path, silent);
    await until("both columns", () => all().some((n) => n.nodeName === "ASIDE") && all().some((n) => n.nodeName === "MAIN"));
    const panel = all().find((n) => n.nodeName === "ASIDE" && (n.textContent ?? "").includes("Instant Chat"));
    const main = all().find((n) => n.nodeName === "MAIN");
    for (const [column, node] of [["list", panel], ["content", main]] as const) {
      const classes = (node?.className ?? "").split(/\s+/);
      assert.ok(!classes.includes("hidden"), `${path}: the ${column} is not hidden outright`);
      assert.ok(!classes.includes("md:hidden"), `${path}: the ${column} is not hidden on desktop`);
    }
    await React.act(async () => page.unmount());
  }
});
