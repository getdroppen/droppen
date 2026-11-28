// #563: a permission picked in one session is that session's, not every
// session's.
//
// The composer both shows `activePermission` and sends it with every message
// (`composer.tsx`, the `permission` field of the send body), so this pins the
// one value. A session with no pick of its own falls back to what its own
// transcript says the last turn ran under, then to the CLI's default — never to
// the source default, which is just the latest pick from somewhere else. A
// dangerous mode from the transcript or this device's old keys is ignored: the
// sessions this bug escalated ran bypass without ever picking it. Only the
// session's own composer row may hold a dangerous mode.
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
const { machineStorageKey } = await import("../src/lib/machines/storage.js");
const { resolveAcpPermission, useComposerChoice } = await import("../src/features/sessions/composer-controls.js");
const { withStartedPermission } = await import("../src/features/sessions/new-session-flow.js");

type Source = "claude" | "codex" | "acp";
type Choice = {
  model: string | null;
  effort: string | null;
  permission?: string | null;
  at: string | null;
};
type Prefs = { defaults: Record<string, Record<string, string>>; sessions: Record<string, Record<string, string>> };
type AcpModels = {
  models: { id: string; label: string | null; efforts: string[] | null; defaultEffort: string | null }[];
  source: "cache";
  permissions?: { id: string; label?: string }[];
  defaultPermission?: string;
};

/** The composer's answer, read off the hook the composer itself uses. */
function Probe({
  source,
  scope,
  cliChoice,
  acpAgent,
}: {
  source: Source;
  scope?: string;
  cliChoice?: Choice | null;
  acpAgent?: string | null;
}) {
  const choice = useComposerChoice(source, scope, cliChoice, null, null, acpAgent);
  return (
    <output
      data-permission={choice.activePermission}
      data-options={JSON.stringify(choice.permissionOptions)}
    />
  );
}

const queryChoice = (
  source: Source,
  scope: string | undefined,
  doc: Prefs,
  cliChoice?: Choice | null,
  acp?: { agent: string; models?: AcpModels },
): { permission: string; options: { value: string; label: string }[] } => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } } });
  client.setQueryData(mq(LOCAL, "composer-prefs"), doc);
  if (source === "acp" && acp?.models) {
    client.setQueryData(mq(LOCAL, "models", "acp", acp.agent), acp.models);
  }
  const html = renderToStaticMarkup(
    <QueryClientProvider client={client}>
      <MachineProvider machine={LOCAL}>
        <Probe source={source} scope={scope} cliChoice={cliChoice} acpAgent={acp?.agent ?? null} />
      </MachineProvider>
    </QueryClientProvider>,
  );
  client.clear();
  const permission = /data-permission="([^"]*)"/.exec(html)?.[1] ?? "";
  const rawOptions = /data-options="([^"]*)"/.exec(html)?.[1] ?? "[]";
  const options = JSON.parse(rawOptions.replaceAll("&quot;", '"')) as { value: string; label: string }[];
  return { permission, options };
};

const answer = (
  source: Source,
  scope: string | undefined,
  doc: Prefs,
  cliChoice?: Choice | null,
  acp?: { agent: string; models?: AcpModels },
): string => queryChoice(source, scope, doc, cliChoice, acp).permission;

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

test("an observed bypassPermissions does not keep a claude session escalated", () => {
  // The transcripts this bug wrote: session B ran with bypass because A's pick
  // reached it. Reading that back and sending it would pin the escalation for
  // good, so it falls through to the CLI default instead.
  const observed: Choice = { model: "claude-opus-5", effort: "high", permission: "bypassPermissions", at };
  assert.equal(answer("claude", "B", { defaults: {}, sessions: {} }, observed), "acceptEdits");
});

test("an observed full-access or bypass-all does not keep a codex session escalated", () => {
  const observed: Choice = { model: "gpt-5.5", effort: "high", permission: "full-access", at };
  assert.equal(answer("codex", "B", { defaults: {}, sessions: {} }, observed), "auto");
  assert.equal(
    answer("codex", "B", { defaults: {}, sessions: {} }, { model: null, effort: null, permission: "bypass-all", at }),
    "auto",
  );
  // `danger-full-access` is codex's other spelling of the same sandbox, and the
  // server accepts it on a send.
  assert.equal(
    answer("codex", "B", { defaults: {}, sessions: {} }, { model: null, effort: null, permission: "danger-full-access", at }),
    "auto",
  );
});

test("a dangerous mode in the session's own row is still that session's pick", () => {
  const doc: Prefs = {
    defaults: {},
    sessions: { "claude/B": { permission: "bypassPermissions", updatedAt: at } },
  };
  const observed: Choice = { model: null, effort: null, permission: "plan", at };
  assert.equal(answer("claude", "B", doc, observed), "bypassPermissions", "only an explicit pick may be dangerous");
});

test("a dangerous mode from this device's old per-session key is ignored", () => {
  const key = machineStorageKey(LOCAL, "composer.permission.claude.B");
  localStorage.setItem(key, "bypassPermissions");
  try {
    assert.equal(answer("claude", "B", { defaults: {}, sessions: {} }), "acceptEdits");
  } finally {
    localStorage.removeItem(key);
  }
});

test("an existing session never reads the source-wide legacy key", () => {
  // `composer.permission.claude` is the source default from before the server
  // store, so it is another session's business, like `defaults`. Only a new
  // session may bridge from it; the session-scoped key above is the one an
  // existing session may use.
  const key = machineStorageKey(LOCAL, "composer.permission.claude");
  localStorage.setItem(key, "plan");
  try {
    assert.equal(answer("claude", "B", { defaults: {}, sessions: {} }), "acceptEdits");
    assert.equal(answer("claude", undefined, { defaults: {}, sessions: {} }), "plan");
  } finally {
    localStorage.removeItem(key);
  }
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

test("a successful start puts its permission in this page's composer cache at once", () => {
  // What `useCreateConversation` writes before the new session opens: the row
  // the server recorded, so the composer does not show the safe default while
  // a refetch is in flight.
  const doc = withStartedPermission(
    { defaults: { claude: { permission: "bypassPermissions", updatedAt: at } }, sessions: {} },
    "claude",
    "started-1",
    "bypassPermissions",
  );
  assert.equal(doc.sessions["claude/started-1"]?.permission, "bypassPermissions");
  assert.equal(doc.defaults.claude?.permission, "bypassPermissions", "the defaults are left alone");

  const existing = withStartedPermission(
    { defaults: {}, sessions: { "claude/started-1": { model: "opus", permission: "plan" } } },
    "claude",
    "started-1",
    "bypassPermissions",
  );
  assert.equal(existing.sessions["claude/started-1"]?.model, "opus", "the row's other fields survive");
  assert.equal(existing.sessions["claude/started-1"]?.permission, "bypassPermissions");

  const acpDoc = withStartedPermission(
    { defaults: { acp: { permission: "accept-edits", updatedAt: at } }, sessions: {} },
    "acp",
    "acp-1",
    "accept-edits",
  );
  assert.equal(acpDoc.sessions["acp/acp-1"]?.permission, "accept-edits");
  assert.equal(acpDoc.defaults.acp?.permission, "accept-edits");
});

const acpClaudeModes: AcpModels = {
  models: [],
  source: "cache",
  permissions: [
    { id: "default", label: "Default" },
    { id: "accept-edits", label: "Accept edits" },
    { id: "auto", label: "Auto" },
    { id: "plan", label: "Plan" },
    { id: "bypassPermissions" },
  ],
  defaultPermission: "accept-edits",
};

const acpOtherModes: AcpModels = {
  models: [],
  source: "cache",
  permissions: [
    { id: "ask", label: "Ask" },
    { id: "code", label: "Code" },
  ],
  defaultPermission: "code",
};

test("an ACP agent's published permission modes map labels and fall back to its defaultPermission or first option", () => {
  const empty: Prefs = { defaults: {}, sessions: {} };
  const result = queryChoice("acp", undefined, empty, null, { agent: "claude-acp", models: acpClaudeModes });
  assert.equal(result.permission, "accept-edits");
  assert.deepEqual(result.options, [
    { value: "default", label: "Default" },
    { value: "accept-edits", label: "Accept edits" },
    { value: "auto", label: "Auto" },
    { value: "plan", label: "Plan" },
    { value: "bypassPermissions", label: "bypassPermissions" },
  ]);

  // Without a valid defaultPermission, the first published mode leads.
  const noDefault: AcpModels = {
    models: [],
    source: "cache",
    permissions: [{ id: "ask", label: "Ask" }, { id: "code", label: "Code" }],
  };
  assert.equal(answer("acp", undefined, empty, null, { agent: "a", models: noDefault }), "ask");
  assert.equal(
    answer("acp", undefined, empty, null, { agent: "a", models: { ...noDefault, defaultPermission: "unlisted" } }),
    "ask",
  );
});

test("an ACP agent publishing no permission modes leaves activePermission empty and options empty", () => {
  const doc: Prefs = {
    defaults: { acp: { permission: "accept-edits" } },
    sessions: { "acp/s-1": { permission: "plan" } },
  };
  const none = queryChoice("acp", "s-1", doc, { model: null, effort: null, permission: "plan", at }, {
    agent: "plain-acp",
    models: { models: [], source: "cache" },
  });
  assert.equal(none.permission, "");
  assert.deepEqual(none.options, []);

  const emptyList = queryChoice("acp", undefined, doc, null, {
    agent: "plain-acp",
    models: { models: [], source: "cache", permissions: [] },
  });
  assert.equal(emptyList.permission, "");
  assert.deepEqual(emptyList.options, []);
});

test("ACP existing sessions obey #563: own stored pick wins, safe transcript reading bridges, source default and dangerous observations never escalate", () => {
  const doc: Prefs = {
    defaults: { acp: { permission: "bypassPermissions", updatedAt: at } },
    sessions: { "acp/A": { permission: "bypassPermissions", updatedAt: at } },
  };
  const acp = { agent: "claude-acp", models: acpClaudeModes };

  // Session A's own stored dangerous pick is respected.
  assert.equal(answer("acp", "A", doc, null, acp), "bypassPermissions");
  // Session B never inherits defaults.acp.permission.
  assert.equal(answer("acp", "B", doc, null, acp), "accept-edits");
  // Safe transcript cliChoice.permission is adopted when no session row exists.
  assert.equal(answer("acp", "B", doc, { model: null, effort: null, permission: "plan", at }, acp), "plan");
  // Session's own stored pick outranks transcript cliChoice.permission.
  const storedDoc: Prefs = {
    defaults: { acp: { permission: "bypassPermissions", updatedAt: at } },
    sessions: { "acp/B": { permission: "auto", updatedAt: at } },
  };
  assert.equal(answer("acp", "B", storedDoc, { model: null, effort: null, permission: "plan", at }, acp), "auto");
  // Dangerous transcript reading is ignored on an existing session (#563).
  assert.equal(
    answer("acp", "B", { defaults: {}, sessions: {} }, { model: null, effort: null, permission: "bypassPermissions", at }, acp),
    "accept-edits",
  );
  // Dangerous legacy per-session key is ignored too.
  const key = machineStorageKey(LOCAL, "composer.permission.acp.B");
  localStorage.setItem(key, "bypassPermissions");
  try {
    assert.equal(answer("acp", "B", { defaults: {}, sessions: {} }, null, acp), "accept-edits");
  } finally {
    localStorage.removeItem(key);
  }
});

test("switching between ACP agents on a new session falls back to the new agent's default when the stored default is not in its list", () => {
  const doc: Prefs = {
    defaults: { acp: { permission: "plan", updatedAt: at } },
    sessions: {},
  };
  // Agent A lists "plan", so the stored default applies.
  assert.equal(answer("acp", undefined, doc, null, { agent: "claude-acp", models: acpClaudeModes }), "plan");
  // Agent B does not list "plan", so it gracefully falls back to Agent B's defaultPermission ("code").
  assert.equal(answer("acp", undefined, doc, null, { agent: "other-acp", models: acpOtherModes }), "code");
  // Direct helper check for an unlisted defaultPermission falling back to options[0].
  assert.equal(
    resolveAcpPermission({
      existing: false,
      sourceDefault: "plan",
      options: [{ value: "ask", label: "Ask" }, { value: "code", label: "Code" }],
      defaultPermission: "missing",
    }),
    "ask",
  );
});
