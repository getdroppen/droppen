// The session composer over a window drawn from the machine's offline copy
// (`features/sessions/feed-from-copy.ts`), before the machine's own state has
// come. The copy cannot say what the session last ran with (`cliChoice`), so
// the choice the composer would send stands on the defaults: a message sent
// then, typed or taken off the outbox, would run under the default permission
// rather than the session's own. Nothing leaves until the machine's state is
// in; then everything goes as it always did, under the session's own choice.
// Nor does the composer show any of what only that state can say: no model,
// effort or permission to see or to pick, no quota (the local login's, over a
// session that runs on an account) and no context, until the state names the
// session's own. A window the local app kept, which carries its state, sends
// and shows them at once as before.
//
// The same holds on the copy itself, the machine away (cloud mode's offline
// transport, `lib/transport/offline.ts`): the composer is shut, and its row
// shows none of that either — not the static model list, not the local
// login's quota — and offers nothing to pick; nor from the moment the page
// goes to the copy, over the machine's state still held. Back on the machine
// the copy's state waits for the machine's like a window drawn from it, and
// then the session's own shows. Another device in the local app's gateway,
// read from its copy, keeps the read-only line it always had.
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
const { foldState, persistable, snapshotOf } = await import("../src/features/sessions/session-feed-pool.js");
const { offlineTransport } = await import("../src/lib/transport/offline.js");
const { SwitchingTransport } = await import("../src/lib/transport/switching.js");
const { invalidateMachine } = await import("../src/lib/machines/query.js");
type Transport = import("../src/lib/transport/types.js").Transport;
type OfflineSource = import("../src/lib/transport/offline.js").OfflineSource;
type MachineInfo = import("../src/lib/machines/runtime.js").MachineInfo;
type FeedState = import("../src/features/sessions/session-feed-pool.js").FeedState;
type SessionSummary = import("../src/api/schema.js").components["schemas"]["SessionSummary"];

const SESSION = "s-1";
/** What the session last ran with, as the machine reads it off the transcript: not the defaults. */
const CHOICE = { model: "sonnet", effort: "low", permission: "plan", at: "2026-11-08T09:00:00Z" };
/** The account the session runs on, which only the machine's state names. */
const ACCOUNT = { id: "acct-work", label: "work" };
/** A login's two quota windows, by what is left of each. */
const quota = (fiveHours: number, week: number) => [
  { label: "5h", windowMinutes: 300, usedPercent: 100 - fiveHours, remainingPercent: fiveHours, resetsAt: null },
  { label: "weekly", windowMinutes: 10_080, usedPercent: 100 - week, remainingPercent: week, resetsAt: null },
];
/** What the composer would show without the session's state: the list's first model, its default effort, the CLI's default permission, the local login's quota. */
const DEFAULTS = ["fable", "high", "acceptEdits", "88%", "64%"];
/** And what the session's own state says: its model, effort and permission, its account's quota, its context. */
const OWN = ["sonnet", "low", "plan", "37%", "21%", "42%"];
/** The quota of the account the session runs on, and the local login's. */
const QUOTA = () => {
  const now = new Date().toISOString();
  return {
    accounts: [{ ...ACCOUNT, kind: "oauth_token", usageEnabled: true, updatedAt: now, windows: quota(37, 21) }],
    claude: { plan: null, tier: null, updatedAt: now, windows: quota(88, 64) },
    codex: null,
  };
};

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
    "/api/stats/quota": QUOTA(),
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
const stateFrame = { summary: row, canSend: true, sendBlockReason: null, cliChoice: CHOICE, injection: null, contextTokens: 84_000, contextWindow: 200_000, claudeRoot: "managed" as const, account: ACCOUNT, rotation: null, receipts: [], questions: [] };

/** Why the session cannot send, as the copy says it while the machine is away. */
const READ_ONLY = "the device is offline — this is a read-only copy";
/**
 * The machine's offline copy as the offline transport reads it: the session's
 * row and copy, and the status the machine last wrote — the account the
 * session runs on, and both its quota and the local login's.
 */
function copySource(): OfflineSource {
  const copy = { source: "claude", id: SESSION, seq: 4, freshestMs: 1, state: { started: true, startIndex: 0, endIndex: 3, items: [item(0), item(1), item(2)], fp: "fp3", prefixFp: "fp0" } } as never;
  const accounts = { accounts: [{ ...ACCOUNT, kind: "oauth_token", provider: "claude" }], defaultAccountId: null };
  return {
    whenLoaded: async () => undefined,
    rows: () => [row],
    row: (source, id) => (source === "claude" && id === SESSION ? row : undefined),
    status: { state: { asOf: 1, settings: null, accounts, quota: QUOTA(), cli: null, mkEpoch: null } },
    session: async () => copy,
    sessionNow: () => copy,
    sessionProgress: () => null,
    onSessionProgress: () => () => undefined,
    touch: () => undefined,
    onChange: () => () => undefined,
  };
}
const offlineOptions = { user: "cloud:browser", messages: { readOnly: READ_ONLY, noCopy: "no copy here", unavailable: "the device is offline" }, machineOnline: () => false };
/** Cloud mode's transport (`lib/transport/switching.ts`): the machine, or its offline copy while it is away. */
const cloud = (posted: Record<string, unknown>[], mode: "live" | "offline") =>
  new SwitchingTransport({ live: device(posted), offline: offlineTransport(copySource(), offlineOptions) }, mode);
/** The session as the copy serves it: its detail, read as the feed reads a snapshot. */
async function servedByCopy(transport: Transport): Promise<FeedState> {
  const res = await transport.fetch(new Request(`http://device.test/api/sessions/claude/${SESSION}`));
  return snapshotOf({ type: "snapshot", ...((await res.json()) as Record<string, unknown>) })!;
}

let setDetail: (next: FeedState) => void = () => undefined;

function reset(posted: Record<string, unknown>[], outbox: unknown[] = [], transport: Transport = device(posted), info: Partial<MachineInfo> = {}) {
  registry.setShell("local", LOCAL);
  registry.openMachine(LOCAL, transport, {
    info: constantStore({ key: LOCAL, name: "studio", state: "online" as const, since: 0, canManage: true, live: "live" as const, ...info }),
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
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  await React.act(async () => {
    page.render(
      <QueryClientProvider client={client}>
        <MachineProvider machine={LOCAL}>
          <router.RouterProvider router={routes} />
        </MachineProvider>
      </QueryClientProvider>,
    );
    await new Promise((resolve) => setTimeout(resolve, 50));
  });
  return { page, errors, client };
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
/** The composer's one toolbar row: attach, the chips, send. */
const toolbar = () => first((n) => n.hasAttribute("data-composer"))?.lastElementChild ?? null;
/** What the row says: attach and send are icons, so only the chips have words. */
const chipWords = () => (toolbar()?.textContent ?? "").trim();
/** Every control in the row that opens something: a picker, the quota's overview. */
const openers = () => [...(toolbar()?.querySelectorAll('[role="combobox"], [aria-haspopup], [aria-expanded]') ?? [])];
/** The row's pickers, by the value each shows: model, effort, permission. */
const pickers = () => [...(toolbar()?.querySelectorAll('[role="combobox"]') ?? [])].map((p) => p.textContent);
/** Whatever in the row can still be pressed. */
const pressable = () => [...(toolbar()?.querySelectorAll("button") ?? [])].filter((b) => !b.disabled);
const showsOwn = (what: string) => {
  for (const word of OWN) assert.ok(chipWords().includes(word), `${what}: ${word} (${chipWords()})`);
  for (const word of DEFAULTS) assert.ok(!chipWords().includes(word), `${what}, and not ${word} (${chipWords()})`);
};

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
  await React.act(async () => setDetail(foldState(fromCopy(), stateFrame)));
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

test("drawn from the copy, the composer shows no model, effort, permission, quota or context, and offers none to pick, until the machine's state names the session's own", async (t) => {
  const posted: Record<string, unknown>[] = [];
  reset(posted);
  const { page, errors } = await mount(fromCopy());
  t.after(() => page.unmount());
  await until("the composer", () => !!textarea());
  // Long enough for the model list, the stored choices and the quota to be in:
  // whatever the row would show without the session's state, it shows by now.
  await settle(150);
  assert.equal(chipWords(), "", "nothing but attach and send: neither the defaults nor the local login's quota");
  assert.deepEqual(openers(), [], "and nothing to pick a model, effort or permission from, or to open");

  // The machine's state: the session's own, at once.
  await React.act(async () => setDetail(foldState(fromCopy(), stateFrame)));
  // The models are listed per account, and the state names one: its list is read now.
  await until("the session's own choice", () => pickers()[0] === "sonnet");
  assert.deepEqual(pickers(), ["sonnet", "low", "plan"], "its model, effort and permission, to see and to pick");
  showsOwn("its choice, its account's quota and its context");
  assert.deepEqual(posted, [], "and showing them sends nothing");
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
  // Its choice, quota and context shown as soon as the reads they need are in, as before.
  await until("the session's own choice, quota and context", () => pickers()[0] === "sonnet" && OWN.every((word) => chipWords().includes(word)));
  assert.deepEqual(pickers(), ["sonnet", "low", "plan"]);
  showsOwn("a kept window's own choice, quota and context");
  await type("straight away");
  await until("a sendable composer", () => !sendButton()!.hasAttribute("disabled"));
  propsOf(sendButton()).onClick!();
  await until("the message", () => posted.length === 1);
  assert.deepEqual([posted[0]!.text, posted[0]!.permission, posted[0]!.model], ["straight away", "plan", "sonnet"]);
  assert.deepEqual(errors, []);
});

test("on the copy, the machine away, the shut composer shows no model, effort, permission, quota or context, and nothing in its row can be pressed or opened", async (t) => {
  const posted: Record<string, unknown>[] = [];
  const transport = cloud(posted, "offline");
  reset(posted, [], transport, { state: "offline", live: "copy" });
  const served = await servedByCopy(transport);
  const { page, errors } = await mount(served);
  t.after(() => page.unmount());
  await until("the composer", () => !!textarea());
  // Long enough for the model list (refused, so the static one), the stored
  // choices (refused) and the copy's quota to be in.
  await settle(150);
  assert.equal(textarea()!.getAttribute("placeholder"), READ_ONLY, "the read-only box, as it was");
  assert.equal(textarea()!.disabled, true);
  assert.equal(chipWords(), "", "neither the static list's model and effort, nor the CLI's default permission, nor the local login's quota");
  assert.deepEqual(openers(), [], "nothing to pick a model, effort or permission from, or to open");
  assert.deepEqual(pressable(), [], "attach and send there, disabled with the box as before");
  assert.equal(served.fromCopy, true, "the state the copy serves is marked as the copy's");
  assert.deepEqual(posted, []);
  assert.deepEqual(errors, []);
});

test("the page goes to the copy while the machine's state is held: the row empties at once, before the copy has said anything", async (t) => {
  const posted: Record<string, unknown>[] = [];
  const transport = cloud(posted, "live");
  reset(posted, [], transport);
  const { page, errors } = await mount(foldState(fromCopy(), stateFrame));
  t.after(() => page.unmount());
  await until("the session's own choice, quota and context", () => pickers()[0] === "sonnet" && OWN.every((word) => chipWords().includes(word)));

  // The machine went: nothing picked now would reach it, and what is shown is no longer its word.
  await React.act(async () => transport.setMode("offline"));
  assert.equal(chipWords(), "", "the session's last choice, quota and context gone with the machine");
  assert.deepEqual(openers(), [], "and nothing left to pick from");
  assert.equal(textarea()!.disabled, true, "the box shut, as it always was on the copy");
  assert.deepEqual(errors, []);
});

test("back on the machine after the copy, the row stays empty and nothing leaves until the machine's state is in, then the session's own", async (t) => {
  const posted: Record<string, unknown>[] = [];
  const transport = cloud(posted, "offline");
  reset(posted, [], transport);
  const served = await servedByCopy(transport);
  const { page, errors, client } = await mount(served);
  t.after(() => page.unmount());
  await until("the composer", () => !!textarea());
  await settle(150);
  assert.equal(chipWords(), "");

  // The machine is back: the page goes to it and asks again, as the shell
  // does on every switch. What the view holds is still the copy's state.
  await React.act(async () => {
    transport.setMode("live");
    await invalidateMachine(client, LOCAL);
  });
  await settle(150);
  assert.equal(textarea()!.disabled, false, "the box free again");
  assert.equal(textarea()!.getAttribute("placeholder"), "loading session…", "and waiting for the session, as over a window drawn from the copy");
  assert.equal(chipWords(), "", "still neither the defaults nor the local login's quota");
  assert.deepEqual(openers(), []);
  await type("hello");
  await enter();
  await settle(100);
  assert.deepEqual(posted, [], "nothing sent under the defaults");

  // The machine's own state: the session's choice, quota and context, and sending under them.
  await React.act(async () => setDetail(foldState(served, stateFrame)));
  await until("the session's own choice", () => pickers()[0] === "sonnet");
  assert.deepEqual(pickers(), ["sonnet", "low", "plan"]);
  showsOwn("back on the machine");
  await until("a sendable composer", () => !sendButton()!.hasAttribute("disabled"));
  propsOf(sendButton()).onClick!();
  await until("the typed message", () => posted.length === 1);
  assert.deepEqual([posted[0]!.text, posted[0]!.permission, posted[0]!.model, posted[0]!.effort], ["hello", "plan", "sonnet", "low"]);
  assert.deepEqual(errors, []);
});

test("another device in the local app's gateway, read from its copy, keeps the read-only line it had: no row at all", async (t) => {
  const posted: Record<string, unknown>[] = [];
  // The gateway shows another device through the same offline transport over its copy.
  const transport = offlineTransport(copySource(), offlineOptions);
  reset(posted, [], transport, { readOnly: true, live: "copy" });
  const served = await servedByCopy(transport);
  assert.equal(served.fromCopy, true, "its state is the copy's too: no model, account or context of the device's own");
  const { page, errors } = await mount(served);
  t.after(() => page.unmount());
  await until("the read-only line", () => !!first((n) => n.getAttribute("data-testid") === "composer-read-only"));
  assert.equal(textarea(), undefined);
  assert.equal(toolbar(), null, "no row: nothing of the device's to show or to pick");
  assert.deepEqual(errors, []);
});
