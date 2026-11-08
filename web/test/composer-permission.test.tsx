// #563: a permission picked in one session is that session's, not every
// session's.
//
// The composer both shows `activePermission` and sends it with every message
// (`composer.tsx`, the `permission` field of the send body), so this pins the
// one value. A session with no pick of its own falls back to what its own
// transcript says the last turn ran under, then to the CLI's default — never to
// the source default, which is just the latest pick from somewhere else.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { MemoryStorage } from "./storage.js";

Object.defineProperty(globalThis, "localStorage", { value: new MemoryStorage(), configurable: true });

const { renderToStaticMarkup } = await import("react-dom/server");
const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
const { MachineProvider } = await import("../src/lib/machines/context.js");
const { LOCAL } = await import("../src/lib/machines/key.js");
const { mq } = await import("../src/lib/machines/query.js");
const { useComposerChoice } = await import("../src/features/sessions/composer-controls.js");

type Source = "claude" | "codex";
type Choice = {
  model: string | null;
  effort: string | null;
  permission?: string | null;
  at: string | null;
};
type Prefs = { defaults: Record<string, Record<string, string>>; sessions: Record<string, Record<string, string>> };

/** The composer's answer, read off the hook the composer itself uses. */
function Probe({ source, scope, cliChoice }: { source: Source; scope?: string; cliChoice?: Choice | null }) {
  const choice = useComposerChoice(source, scope, cliChoice, null);
  return <output data-permission={choice.activePermission} />;
}

const answer = (source: Source, scope: string | undefined, doc: Prefs, cliChoice?: Choice | null): string => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } } });
  client.setQueryData(mq(LOCAL, "composer-prefs"), doc);
  const html = renderToStaticMarkup(
    <QueryClientProvider client={client}>
      <MachineProvider machine={LOCAL}>
        <Probe source={source} scope={scope} cliChoice={cliChoice} />
      </MachineProvider>
    </QueryClientProvider>,
  );
  client.clear();
  return /data-permission="([^"]*)"/.exec(html)?.[1] ?? "";
};

const at = "2026-10-20T00:00:00.000Z";

/** The document after a pick in session A: the pick is A's and the default's. */
const afterPickInA = (permission: string): Prefs => ({
  defaults: { claude: { permission, updatedAt: at } },
  sessions: { "claude/A": { permission, updatedAt: at } },
});

test("a bypass pick in one session is not another session's permission", () => {
  const doc = afterPickInA("bypassPermissions");
  assert.equal(answer("claude", "A", doc), "bypassPermissions", "the session that picked keeps it");
  assert.equal(answer("claude", "B", doc), "acceptEdits", "a session with no row gets the CLI default, not A's pick");
});

test("a row that picked only a model has no permission to inherit", () => {
  const doc: Prefs = {
    defaults: { claude: { permission: "bypassPermissions", updatedAt: at } },
    sessions: { "claude/B": { model: "opus", updatedAt: at } },
  };
  assert.equal(answer("claude", "B", doc), "acceptEdits");
});

test("changing the default in A never changes what B shows or sends", () => {
  const before: Prefs = { defaults: { claude: { permission: "acceptEdits" } }, sessions: {} };
  const after: Prefs = { defaults: { claude: { permission: "bypassPermissions" } }, sessions: {} };
  assert.equal(answer("claude", "B", before), "acceptEdits");
  assert.equal(answer("claude", "B", after), "acceptEdits", "not even when the default is the dangerous one");
});

test("a session with no stored permission uses what its last turn ran under", () => {
  const doc = afterPickInA("bypassPermissions");
  const observed: Choice = { model: "claude-opus-5", effort: "high", permission: "plan", at };
  assert.equal(answer("claude", "B", doc, observed), "plan", "the session's own last run, not A's bypass");
});

test("a session's own stored permission outranks its transcript and the default", () => {
  const doc: Prefs = {
    defaults: { claude: { permission: "bypassPermissions", updatedAt: at } },
    sessions: { "claude/B": { permission: "dontAsk", updatedAt: at } },
  };
  const observed: Choice = { model: null, effort: null, permission: "plan", at };
  assert.equal(answer("claude", "B", doc, observed), "dontAsk");
});

test("a new session still starts from the latest pick", () => {
  assert.equal(answer("claude", undefined, afterPickInA("bypassPermissions")), "bypassPermissions");
});

test("codex gets the same treatment, with its own default", () => {
  const doc: Prefs = {
    defaults: { codex: { permission: "bypass-all" } },
    sessions: { "codex/A": { permission: "bypass-all" } },
  };
  assert.equal(answer("codex", "B", doc), "auto");
  assert.equal(answer("codex", "B", doc, { model: null, effort: null, permission: "read-only", at }), "read-only");
  assert.equal(answer("codex", undefined, doc), "bypass-all");
});

test("a permission the composer cannot offer falls back to the CLI default", () => {
  const doc: Prefs = { defaults: {}, sessions: { "claude/B": { permission: "some-old-mode" } } };
  assert.equal(answer("claude", "B", doc), "acceptEdits");
  // The transcript's `default` is not one of the composer's modes either; it
  // must not be sent as an unlisted value.
  assert.equal(
    answer("claude", "B", { defaults: {}, sessions: {} }, { model: null, effort: null, permission: "default", at }),
    "acceptEdits",
  );
});
