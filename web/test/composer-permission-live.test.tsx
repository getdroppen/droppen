// #563, the second message: a session started with a dangerous mode keeps it.
//
// The server records the mode a session was started with in its own composer
// row, but this page's prefs cache predates that row — the new-session screen
// has just written the defaults, and a scheduled task's session was never on
// this page at all. Without asking, the composer would find no row, drop the
// dangerous transcript reading (correctly, see `composer-permission.test.tsx`)
// and send the safe CLI default on the next message. So a session composer
// missing its row refetches the document once; these tests drive that against a
// live DOM, with the server's answer carrying the row the cache lacks.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { domTree, installLiveDom, until } from "./live-dom.js";
import { MemoryStorage } from "./storage.js";

const container = installLiveDom();
Object.defineProperty(globalThis, "localStorage", { value: new MemoryStorage(), configurable: true });
(globalThis as Record<symbol, unknown>)[Symbol.for("undici.globalOrigin.1")] = new URL("http://device.test");

const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
const { MachineProvider } = await import("../src/lib/machines/context.js");
const { LOCAL } = await import("../src/lib/machines/key.js");
const { mq } = await import("../src/lib/machines/query.js");
const registry = await import("../src/lib/machines/registry.js");
const { constantStore } = await import("../src/lib/machines/store.js");
const { useComposerChoice } = await import("../src/features/sessions/composer-controls.js");

type Source = "claude" | "codex";
type Doc = { defaults: Record<string, Record<string, string>>; sessions: Record<string, Record<string, string>> };
type Transport = import("../src/lib/transport/index.js").Transport;

/** What the composer shows (and so sends), read off the hook the composer uses. */
function Probe({ source, scope }: { source: Source; scope: string }) {
  const choice = useComposerChoice(source, scope, null, null);
  return <output data-permission={choice.activePermission} />;
}

/** A device whose composer-prefs answer is `server`, counting the reads. */
function device(server: Doc, reads: string[]): Transport {
  return {
    fetch: async (request: Request) => {
      const url = new URL(request.url);
      if (url.pathname === "/api/composer-prefs") {
        reads.push(url.pathname);
        return Response.json(server);
      }
      if (url.pathname === "/api/models") return Response.json({ models: [], source: "fallback" });
      return Response.json({ error: "not here" }, { status: 404 });
    },
    socketUrl: async (path) => path,
    openSocket: () => ({ readyState: 0, binaryType: "blob", send() {}, close() {}, onopen: null, onmessage: null, onclose: null }),
  };
}

/** Mount a probe over `cached` (what this page last knew) and the server's answer. */
async function draw(source: Source, scope: string, cached: Doc, server: Doc) {
  const reads: string[] = [];
  registry.setShell("local", LOCAL);
  registry.openMachine(LOCAL, device(server, reads), {
    info: constantStore({ key: LOCAL, name: "studio", state: "online" as const, since: 0, canManage: true, live: "live" as const }),
    ensureLive: () => undefined,
    hold: () => () => undefined,
    dispose: () => undefined,
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } } });
  client.setQueryData(mq(LOCAL, "composer-prefs"), cached);
  const root = createRoot(container as never);
  root.render(
    <QueryClientProvider client={client}>
      <MachineProvider machine={LOCAL}>
        <Probe source={source} scope={scope} />
      </MachineProvider>
    </QueryClientProvider>,
  );
  await until("the composer's permission", () => {
    const shown = domTree(container).first((node) => node.hasAttribute("data-permission"));
    return shown !== undefined;
  });
  return {
    shown: () => domTree(container).first((node) => node.hasAttribute("data-permission"))?.getAttribute("data-permission"),
    reads,
    root,
  };
}

test("a session started in bypass asks for the row it has not seen, so the next message keeps bypass", async () => {
  // Exactly the state after a new-session pick and start: the pick refreshed
  // `defaults` (so the cache is not stale), the server recorded the session's
  // row, and this page has never read it.
  const cached: Doc = { defaults: { claude: { permission: "bypassPermissions" } }, sessions: {} };
  const server: Doc = {
    defaults: { claude: { permission: "bypassPermissions" } },
    sessions: { "claude/started-1": { permission: "bypassPermissions" } },
  };
  const page = await draw("claude", "started-1", cached, server);
  await until("the fetched row to replace the safe default", () => page.shown() === "bypassPermissions");
  assert.equal(page.shown(), "bypassPermissions");
  assert.ok(page.reads.length >= 1, "the missing row was asked for");
  page.root.unmount();
});

test("opening a session a scheduled task started fetches the mode it recorded", async () => {
  // The schedule runner creates the session and records its task's mode; this
  // page never saw it, and its cached document is old enough to hold a
  // different default.
  const cached: Doc = { defaults: { codex: { permission: "auto" } }, sessions: {} };
  const server: Doc = {
    defaults: { codex: { permission: "auto" } },
    sessions: { "codex/scheduled-1": { permission: "bypass-all" } },
  };
  const page = await draw("codex", "scheduled-1", cached, server);
  await until("the scheduled session's row", () => page.shown() === "bypass-all");
  assert.equal(page.shown(), "bypass-all");
  assert.ok(page.reads.length >= 1, "the scheduled session's row was asked for");
  page.root.unmount();
});
