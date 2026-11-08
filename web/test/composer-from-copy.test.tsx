// The session composer over a window drawn from the machine's offline copy
// (`features/sessions/feed-from-copy.ts`), before the machine's own state has
// come. The copy cannot say what the session last ran with (`cliChoice`), so
// the choice the composer would send stands on the defaults: a message sent
// then, typed or taken off the outbox, would run under the default permission
// rather than the session's own. Nothing leaves until the machine's state is
// in; then everything goes as it always did, under the session's own choice.
// A window the local app kept, which carries its state, sends at once as
// before.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { domTree, installLiveDom, propsOf, until } from "./live-dom.js";

const container = installLiveDom({ act: true });
// Relative requests (`/api/…`) resolve as on a page.
(globalThis as Record<symbol, unknown>)[Symbol.for("undici.globalOrigin.1")] = new URL("http://device.test");
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
const { MachineProvider } = await import("../src/lib/machines/context.js");
const { LOCAL } = await import("../src/lib/machines/key.js");
const registry = await import("../src/lib/machines/registry.js");
const { machineStorageKey } = await import("../src/lib/machines/storage.js");
const { constantStore } = await import("../src/lib/machines/store.js");
const { Composer } = await import("../src/features/sessions/composer.js");
const { createDraftStore } = await import("../src/features/sessions/stream-draft.js");
const { copyFeedState } = await import("../src/features/sessions/feed-from-copy.js");
const { liveState, persistable, snapshotOf } = await import("../src/features/sessions/session-feed-pool.js");
type Transport = import("../src/lib/transport/types.js").Transport;
type FeedState = import("../src/features/sessions/session-feed-pool.js").FeedState;
type SessionSummary = import("../src/api/schema.js").components["schemas"]["SessionSummary"];

const SESSION = "s-1";
/** What the session last ran with, as the machine reads it off the transcript: not the defaults. */
const CHOICE = { model: "sonnet", effort: "low", permission: "plan", at: "2026-11-08T09:00:00Z" };

/** The device: the composer's reads, and every message posted to the session, kept. */
function device(posted: Record<string, unknown>[]): Transport {
  const answers: Record<string, unknown> = {
    "/api/models": {
      models: [
        { id: "fable", label: null, efforts: ["low", "high"], defaultEffort: "high" },
        { id: "sonnet", label: null, efforts: ["low", "high"], defaultEffort: "high" },
      ],
      source: "cache",
    },
    "/api/composer-prefs": { defaults: {}, sessions: {} },
    "/api/prefs": { composerSend: "enter", sessionGrouping: "flat", transcriptStyle: "flat", fontSize: "default", language: "en" },
    "/api/accounts": { accounts: [], defaultAccountId: null, defaultCodexAccountId: null },
    "/api/stats/quota": { accounts: [], claude: null, codex: null },
  };
  return {
    fetch: async (request: Request) => {
      const url = new URL(request.url);
      if (url.pathname === `/api/sessions/claude/${SESSION}/message` && request.method === "POST") {
        posted.push((await request.json()) as Record<string, unknown>);
        return Response.json({});
      }
      const answer = answers[url.pathname];
      return answer === undefined ? Response.json({ error: "not here" }, { status: 404 }) : Response.json(answer);
    },
    socketUrl: async (path) => path,
    openSocket: () => ({ readyState: 0, binaryType: "blob", send() {}, close() {}, onopen: null, onmessage: null, onclose: null }),
  };
}

const row = {
  id: SESSION, source: "claude", title: "a session", activity: "idle", status: "idle", asking: false, attentionAt: null,
  createdAt: "2026-11-08T00:00:00Z", updatedAt: "2026-11-08T09:00:00Z", cwd: "/home/demo/work",
  filePath: "/x.jsonl", gitBranch: null, repoKey: null, resident: null, scope: "owned", server: "test",
  sizeBytes: 1, unreadCount: 0, unread: false,
} as SessionSummary;
const item = (n: number) => ({ role: "user", kind: "text", text: `item ${n}`, detail: null, isError: false, meta: null, question: null, timestamp: null, toolName: null, truncated: false });

/** The window the copy gives the session, as the feed pool draws it. */
const fromCopy = (): FeedState =>
  copyFeedState({ state: { started: true, startIndex: 0, endIndex: 3, items: [item(0), item(1), item(2)] as never, fp: "fp3", prefixFp: "fp0" }, seq: 4 }, row)!;
/** The machine's own state frame, as it follows the resume delta. */
const stateFrame = { summary: row, canSend: true, sendBlockReason: null, cliChoice: CHOICE, injection: null, contextTokens: null, contextWindow: 200_000, claudeRoot: null, account: null, rotation: null, receipts: [], questions: [] };

let setDetail: (next: FeedState) => void = () => undefined;

function reset(posted: Record<string, unknown>[], outbox: unknown[] = []) {
  registry.setShell("local", LOCAL);
  registry.openMachine(LOCAL, device(posted), {
    info: constantStore({ key: LOCAL, name: "studio", state: "online" as const, since: 0, canManage: true, live: "live" as const }),
    ensureLive: () => undefined,
    hold: () => () => undefined,
    dispose: () => undefined,
  });
  localStorage.clear();
  if (outbox.length) localStorage.setItem(machineStorageKey(LOCAL, `queue.claude.${SESSION}`), JSON.stringify(outbox));
}

/** The real composer under the app's providers, showing `initial` until the test hands it another state. */
async function mount(initial: FeedState) {
  const drafts = createDraftStore();
  function Session() {
    const [detail, set] = React.useState(initial);
    setDetail = set;
    return <Composer source="claude" id={SESSION} detail={detail} drafts={drafts} subtasks={[]} onQueueChanged={() => undefined} />;
  }
  const root = router.createRootRoute({ component: Session });
  const routes = router.createRouter({ routeTree: root, history: router.createMemoryHistory({ initialEntries: ["/"] }) });
  await routes.load();
  const errors: unknown[] = [];
  const page = createRoot(container as never, {
    onUncaughtError: (error) => void errors.push(error),
    onCaughtError: (error) => void errors.push(error),
  });
  await React.act(async () => {
    page.render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <MachineProvider machine={LOCAL}>
          <router.RouterProvider router={routes} />
        </MachineProvider>
      </QueryClientProvider>,
    );
    await new Promise((resolve) => setTimeout(resolve, 50));
  });
  return { page, errors };
}

const textarea = () => first((n) => n.nodeName === "TEXTAREA") as HTMLTextAreaElement | undefined;
const sendButton = () => first((n) => n.getAttribute("data-variant") === "primary");
const settle = (ms = 80) => React.act(async () => void (await new Promise((resolve) => setTimeout(resolve, ms))));
const type = async (text: string) => {
  await React.act(async () => {
    propsOf(textarea()).onChange!({ target: { value: text } });
    await new Promise((resolve) => setTimeout(resolve, 10));
  });
};
/** Enter in the box, as the keyboard sends it. */
const enter = async () => {
  await React.act(async () => {
    propsOf(textarea()).onKeyDown!({ key: "Enter", shiftKey: false, metaKey: false, ctrlKey: false, keyCode: 13, nativeEvent: { isComposing: false }, preventDefault() {} });
    await new Promise((resolve) => setTimeout(resolve, 10));
  });
};
const outbox = () => JSON.parse(localStorage.getItem(machineStorageKey(LOCAL, `queue.claude.${SESSION}`)) ?? "[]") as { text: string }[];

test("drawn from the copy, nothing leaves — typed or queued — until the machine's state is in, then all of it under the session's own choice", async (t) => {
  const posted: Record<string, unknown>[] = [];
  // A message this tab was holding back for the session (an earlier visit).
  reset(posted, [{ clientMessageId: "m-queued", text: "queued earlier" }]);
  const { page, errors } = await mount(fromCopy());
  t.after(() => page.unmount());
  await until("the composer", () => !!textarea());

  assert.equal(textarea()!.getAttribute("placeholder"), "loading session…", "what it waits for, not that the session is busy");
  assert.equal(textarea()!.disabled, false, "the text stays editable");
  await type("hello");
  assert.equal(textarea()!.value, "hello");
  assert.equal(sendButton()!.hasAttribute("disabled"), true, "nothing to send with yet");
  await enter();
  propsOf(sendButton()).onClick!();
  await settle(150);
  assert.deepEqual(posted, [], "neither the typed message nor the queued one went out");
  assert.equal(textarea()!.value, "hello", "and the typed one is still where it was typed");
  assert.deepEqual(outbox().map((m) => m.text), ["queued earlier"], "nor was anything queued in its place");

  // The machine's state: the queue drains, under the session's own choice.
  await React.act(async () => setDetail(liveState(fromCopy(), stateFrame)));
  await until("the queued message", () => posted.length === 1);
  assert.equal(posted[0]!.text, "queued earlier");
  assert.deepEqual([posted[0]!.permission, posted[0]!.model, posted[0]!.effort], ["plan", "sonnet", "low"], "the session's own permission, model and effort");
  await until("an empty outbox", () => outbox().length === 0);

  await until("a sendable composer", () => !sendButton()!.hasAttribute("disabled"));
  assert.match(textarea()!.getAttribute("placeholder") ?? "", /^send a message to this session…/);
  propsOf(sendButton()).onClick!();
  await until("the typed message", () => posted.length === 2);
  assert.equal(posted[1]!.text, "hello");
  assert.deepEqual([posted[1]!.permission, posted[1]!.model, posted[1]!.effort], ["plan", "sonnet", "low"]);
  assert.deepEqual(errors, []);
});

test("a window the local app kept carries its state, and sends at once as it always did", async (t) => {
  const posted: Record<string, unknown>[] = [];
  reset(posted);
  // What `persistable` keeps of a live window: the whole state, `cliChoice` with it, and no mark of the copy.
  const kept = persistable(snapshotOf({ type: "snapshot", sig: "s-1:9", ...stateFrame, items: [item(0), item(1), item(2)], startIndex: 0, endIndex: 3, totalItems: 3, prefixFp: "fp0", startFp: "s0" })!);
  assert.equal(kept.fromCopy, undefined);
  assert.deepEqual(kept.cliChoice, CHOICE);
  const { page, errors } = await mount(kept);
  t.after(() => page.unmount());
  await until("the composer", () => !!textarea());
  await type("straight away");
  await until("a sendable composer", () => !sendButton()!.hasAttribute("disabled"));
  propsOf(sendButton()).onClick!();
  await until("the message", () => posted.length === 1);
  assert.deepEqual([posted[0]!.text, posted[0]!.permission, posted[0]!.model], ["straight away", "plan", "sonnet"]);
  assert.deepEqual(errors, []);
});
