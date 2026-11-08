// Instant chats take the attachments sessions take.
//
// The new-chat form (`features/chat/chat-home.tsx`) is the shared session
// composer, so a picture pasted into it is uploaded at once and the start
// request carries `images`/`files` exactly as `/api/sessions/new` does. The
// api backend's provider call is text-only: it offers no attach control, and
// switching to it takes what is attached out with a visible note. A CLI
// chat's follow-up composer (`features/sessions/composer.tsx`) offers the
// same controls for both CLIs, because both can read what is attached.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { domTree, installLiveDom, propsOf, until } from "./live-dom.js";

const container = installLiveDom({ act: true });
// Relative requests (`/api/uploads`) resolve as on a page.
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
const { ChatHome } = await import("../src/features/chat/chat-home.js");
const { Composer } = await import("../src/features/sessions/composer.js");
const { createDraftStore } = await import("../src/features/sessions/stream-draft.js");
type Transport = import("../src/lib/transport/types.js").Transport;
type FeedState = import("../src/features/sessions/session-feed-pool.js").FeedState;
type SessionSummary = import("../src/api/schema.js").components["schemas"]["SessionSummary"];

const ACCOUNT = "11111111-1111-4111-8111-111111111111";
const UPLOADS = "/home/demo/.remote-hand/uploads/u1";

/** What the device was asked to do. */
interface Sent {
  method: string;
  path: string;
  body: unknown;
}

/** The device answers the chat form's routes, uploads anything, and starts a chat as `chat-1`. */
function device(sent: Sent[]): Transport {
  const answers: Record<string, unknown> = {
    "/api/accounts": {
      accounts: [{
        id: ACCOUNT, label: "work", kind: "oauth_token", provider: "claude", usageEnabled: false,
        rotation: true, rest: [], createdAt: "2026-10-01T00:00:00Z", updatedAt: "2026-10-01T00:00:00Z",
      }],
      defaultAccountId: null,
      defaultCodexAccountId: null,
      local: {
        claude: { state: "unknown", rest: [], isDefault: false },
        codex: { state: "unknown", rest: [], isDefault: false },
      },
    },
    "/api/accounts/start": { accountId: ACCOUNT, reason: "default", model: "fable", placed: true },
    "/api/models": { models: [{ id: "fable", label: null, efforts: ["low", "high"], defaultEffort: "high" }], source: "cache" },
    "/api/composer-prefs": { defaults: {}, sessions: {} },
    "/api/prefs": { composerSend: "enter", sessionGrouping: "flat", transcriptStyle: "flat", fontSize: "default", language: "en" },
    "/api/stats/quota": { accounts: [], claude: null, codex: null },
    "/api/chat-provider": { baseUrl: "", hasKey: false, model: "" },
    "/api/chats": { chats: [] },
  };
  return {
    fetch: async (request: Request) => {
      const url = new URL(request.url);
      if (url.pathname === "/api/uploads" && request.method === "POST") {
        const file = (await request.formData()).get("file") as File;
        sent.push({ method: "POST", path: url.pathname, body: file.name });
        return Response.json({ path: `${UPLOADS}/${file.name}`, name: file.name, isImage: file.type.startsWith("image/") });
      }
      if (url.pathname === "/api/chats/new" && request.method === "POST") {
        sent.push({ method: "POST", path: url.pathname, body: await request.json() });
        return Response.json({ kind: "claude", id: "chat-1" });
      }
      const answer = answers[url.pathname];
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
}

/** The app's chat routes around the real form, at `/chat/new`. */
async function mount(path: string, extra?: (props: Record<string, never>) => React.ReactNode) {
  const root = router.createRootRoute({ component: () => <router.Outlet /> });
  const chat = router.createRoute({ getParentRoute: () => root, path: "chat", component: () => <router.Outlet /> });
  const index = router.createRoute({ getParentRoute: () => chat, path: "/", component: ChatHome });
  const fresh = router.createRoute({ getParentRoute: () => chat, path: "new", component: ChatHome });
  const detail = (name: string) =>
    router.createRoute({ getParentRoute: () => chat, path: name, component: extra ?? (() => null) });
  const routes = router.createRouter({
    routeTree: root.addChildren([chat.addChildren([index, fresh, detail("claude/$id"), detail("codex/$id"), detail("api/$id")])]),
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

const textarea = () => first((n) => n.nodeName === "TEXTAREA");
const sendButton = () => first((n) => (n as Element).getAttribute("data-variant") === "primary");
const attachButton = () => first((n) => n.getAttribute("title") === "attach files (or paste into the message box)");
const chips = () => all().filter((n) => n.hasAttribute("data-attachment"));
const type = async (text: string) => {
  await React.act(async () => {
    propsOf(textarea()).onChange!({ target: { value: text } });
    await new Promise((resolve) => setTimeout(resolve, 10));
  });
};
const paste = async (file: File) => {
  await React.act(async () => {
    propsOf(textarea()).onPaste!({ clipboardData: { files: [file] }, preventDefault() {} });
    await new Promise((resolve) => setTimeout(resolve, 10));
  });
};
const picture = () => new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], "shot.png", { type: "image/png" });
const notes = () => new File(["a line"], "notes.txt", { type: "text/plain" });

/** The tab whose label is `name`, from the backend segmented control. */
const tab = (name: string) =>
  first((n) => n.getAttribute("role") === "tab" && (n.textContent ?? "").trim() === name);

const click = async (element: Element | null) => {
  assert.ok(element, "the control is on the page");
  await React.act(async () => {
    (element as HTMLElement).click();
    await new Promise((resolve) => setTimeout(resolve, 30));
  });
};

test("the new-chat form uploads a pasted picture and file, and starts the chat with both", async (t) => {
  const sent: Sent[] = [];
  reset(sent);
  const { page, errors } = await mount("/chat/new");
  t.after(() => page.unmount());
  await until("the composer", () => !!textarea());
  assert.ok(attachButton(), "a CLI chat offers the session composer's attach control");

  await paste(picture());
  await paste(notes());
  await until("both uploads", () => sent.filter((s) => s.path === "/api/uploads").length === 2);
  await until("both chips", () => chips().length === 2);
  assert.ok(chips().some((n) => n.getAttribute("data-attachment") === "image"), "the picture is a thumbnail");
  assert.ok(chips().some((n) => n.getAttribute("data-attachment") === "file"), "the text file is a file chip");

  await type("what is in these?");
  await until("a sendable form", () => propsOf(sendButton()).disabled === false);
  await click(sendButton());

  await until("the chat request", () => sent.some((s) => s.path === "/api/chats/new"));
  const start = sent.find((s) => s.path === "/api/chats/new")!;
  const body = start.body as Record<string, unknown>;
  assert.equal(body.backend, "claude");
  assert.equal(body.text, "what is in these?");
  assert.deepEqual(body.images, [`${UPLOADS}/shot.png`]);
  assert.deepEqual(body.files, [`${UPLOADS}/notes.txt`], "the same fields and paths a session's start carries");
  assert.deepEqual(errors, []);
});

test("the api backend has no attach control, ignores a paste, and says why a switch cleared the box", async (t) => {
  const sent: Sent[] = [];
  reset(sent);
  const { page, errors } = await mount("/chat/new");
  t.after(() => page.unmount());
  await until("the composer", () => !!textarea());
  await until("the backend tabs", () => !!tab("api"));

  // Attach one file, then switch: it is taken out with a note rather than
  // sent to a backend that cannot carry it.
  await paste(notes());
  await until("the upload", () => sent.some((s) => s.path === "/api/uploads"));
  await until("the chip", () => chips().length === 1);
  await click(tab("api"));
  await until("the api backend", () => !attachButton());
  assert.equal(chips().length, 0, "the attachment left with the switch");
  await until("the note", () => (container.textContent ?? "").includes("api chats take text only"));

  // A paste is ignored entirely: no upload, and nothing to send.
  const uploads = sent.filter((s) => s.path === "/api/uploads").length;
  await paste(picture());
  await React.act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 60));
  });
  assert.equal(sent.filter((s) => s.path === "/api/uploads").length, uploads, "no upload was started");
  assert.equal(chips().length, 0);
  assert.deepEqual(errors, []);
});

/** A session row the composer reads. */
function summary(source: "claude" | "codex"): SessionSummary {
  return {
    id: "chat-1", source, title: "a chat", activity: "idle", status: "idle", asking: false, attentionAt: null,
    createdAt: "2026-10-18T00:00:00Z", updatedAt: "2026-10-18T01:00:00Z", cwd: "/home/demo/.remote-hand/chats",
    filePath: "/x.jsonl", gitBranch: null, repoKey: null, resident: null, scope: "owned", server: "test",
    sizeBytes: 1, unreadCount: 0, unread: false,
  } as SessionSummary;
}

/** Everything a feed's state has, with nothing running. */
function feed(source: "claude" | "codex"): FeedState {
  return {
    account: null,
    canSend: true,
    claudeRoot: null,
    cliChoice: null,
    contextTokens: null,
    contextWindow: 200_000,
    endIndex: 0,
    hasMoreBefore: false,
    injection: null,
    items: [],
    itemsChanged: false,
    omittedItems: 0,
    questions: [],
    receipts: [],
    reset: true,
    sendBlockReason: null,
    sig: "sig",
    startIndex: 0,
    summary: summary(source),
    totalItems: 0,
    prefixFp: "",
    startFp: "",
  } as FeedState;
}

for (const source of ["claude", "codex"] as const) {
  test(`a ${source} chat's follow-up composer offers attachments`, async (t) => {
    const sent: Sent[] = [];
    reset(sent);
    const drafts = createDraftStore();
    const { page, errors } = await mount(`/chat/${source}/chat-1`, () => (
      <Composer
        source={source}
        id="chat-1"
        detail={feed(source)}
        drafts={drafts}
        subtasks={[]}
        chat
        onQueueChanged={() => undefined}
      />
    ));
    t.after(() => page.unmount());
    await until(`the ${source} chat composer`, () => !!textarea());
    assert.ok(attachButton(), "the session composer's attach control is offered in a chat");
    assert.deepEqual(errors, []);
  });
}
