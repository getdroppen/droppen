import assert from "node:assert/strict";
import test from "node:test";
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { parseSvgGlyph } from "../src/components/glyph.js";
import type { AcpAgentList } from "../src/features/sessions/acp-agent.js";
import { ACP_GLYPH, acpAgentGlyph, acpAgentLook, glyphsById } from "../src/features/sessions/acp-agent-icons.js";
import { applyPreset, CUSTOM_PRESET, newAgentId, type AcpAgentDraft } from "../src/features/sessions/acp-agent-presets.js";
import { agentKey, canCreateConversation, creationRequest, parseAgentKey, type NewConversation } from "../src/features/sessions/new-session-flow.js";
import { prefillFromSearch, readNewSessionDraft, writeNewSessionDraft, EMPTY_NEW_SESSION_DRAFT } from "../src/features/sessions/new-session.js";
import {
  accountsSignal,
  acpSignal,
  acpStatus,
  agentsSearch,
  agoKey,
  openingTab,
} from "../src/features/settings/agents/agents-tabs.js";
import { LOCAL } from "../src/lib/machines/key.js";

const ICONS = fileURLToPath(new URL("../src/features/sessions/presets/icons/", import.meta.url));
/** The server's built-in agent descriptions, which `GET /api/acp/agents` serves as `presets`. */
const BUILT_IN = fileURLToPath(new URL("../../server-rs/src/acp-agents.json", import.meta.url));
const SLUG = /^[a-z0-9][a-z0-9-]*$/;

class MemoryStorage {
  values = new Map<string, string>();
  getItem(key: string) {
    return this.values.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    this.values.set(key, value);
  }
  removeItem(key: string) {
    this.values.delete(key);
  }
}

test("every built-in agent is well-formed: a unique slug id, a name, one executable and its arguments", () => {
  const presets = JSON.parse(readFileSync(BUILT_IN, "utf8")) as unknown;
  assert.ok(Array.isArray(presets) && presets.length > 0, "at least one built-in");
  const ids = new Set<string>();
  for (const entry of presets as unknown[]) {
    assert.equal(typeof entry, "object");
    const preset = entry as Record<string, unknown>;
    const known = ["id", "name", "command", "args", "homepage", "note"];
    assert.deepEqual(Object.keys(preset).filter((key) => !known.includes(key)), [], `no unknown fields: ${JSON.stringify(preset)}`);
    assert.ok(typeof preset.id === "string" && SLUG.test(preset.id), `id: ${String(preset.id)}`);
    assert.notEqual(preset.id, CUSTOM_PRESET, "the custom choice is not a built-in");
    assert.ok(!ids.has(preset.id), `duplicate id ${preset.id}`);
    ids.add(preset.id);
    assert.ok(typeof preset.name === "string" && preset.name.trim() === preset.name && preset.name.length > 0, `name of ${preset.id}`);
    // One executable, looked up on the PATH: no arguments folded into it.
    assert.ok(typeof preset.command === "string" && /^[^\s/]+$/.test(preset.command), `command of ${preset.id}`);
    // Arguments are split on whitespace when typed, so none may hold any.
    assert.ok(Array.isArray(preset.args), `args of ${preset.id}`);
    for (const arg of preset.args as unknown[]) {
      assert.ok(typeof arg === "string" && arg.length > 0 && !/\s/.test(arg), `an argument of ${preset.id}: ${String(arg)}`);
    }
    if ("homepage" in preset) {
      assert.ok(typeof preset.homepage === "string" && new URL(preset.homepage).protocol === "https:", `homepage of ${preset.id}`);
    }
    if ("note" in preset) {
      assert.ok(typeof preset.note === "string" && preset.note.length > 0 && !preset.note.includes("\n"), `note of ${preset.id}`);
    }
  }
});

test("every agent mark is path data on a view box, named for an agent id, and nothing else", () => {
  const files = readdirSync(ICONS).filter((file) => file.endsWith(".svg"));
  assert.ok(files.length > 0);
  const sources: Record<string, string> = {};
  for (const file of files) {
    assert.ok(SLUG.test(file.slice(0, -".svg".length)), `${file} is named <agent id>.svg`);
    const svg = readFileSync(ICONS + file, "utf8");
    const glyph = parseSvgGlyph(file, svg);
    assert.ok(glyph, `${file} parses as a glyph`);
    assert.equal(glyph.viewBox, "0 0 24 24", `${file} draws on the 24-unit box the other marks use`);
    sources[`./presets/icons/${file}`] = svg;
  }
  // Found by the id of the agent a session names, whatever it is called.
  const glyphs = glyphsById(sources);
  assert.equal(acpAgentGlyph("bub", glyphs).id, "acp-agent:bub");
  assert.equal(acpAgentGlyph("my-own-agent", glyphs), ACP_GLYPH, "an agent without a mark draws the generic one");
  assert.equal(acpAgentGlyph(null, glyphs), ACP_GLYPH);
  // Markup beyond paths is refused rather than drawn.
  assert.equal(parseSvgGlyph("x", '<svg viewBox="0 0 24 24"><script>alert(1)</script><path d="M0 0h1Z"/></svg>'), null);
  assert.equal(parseSvgGlyph("x", '<svg viewBox="0 0 24 24"><path d="M0 0h1Z" onclick="x"/></svg>'), null);
  assert.deepEqual(parseSvgGlyph("x", '<svg viewBox="0 0 24 24">\n<!-- source -->\n<path d="M0 0h1Z"/></svg>\n')?.paths, ["M0 0h1Z"]);
  assert.deepEqual([...glyphsById({ "./presets/icons/Bad Name.svg": '<svg viewBox="0 0 24 24"><path d="M0 0h1Z"/></svg>' }).keys()], []);
});

const PRESETS = [
  { id: "one", name: "One", command: "one", args: ["acp"], onPath: false },
  { id: "two", name: "Two", command: "two", args: ["--acp", "--stdio"], homepage: "https://two.example/", onPath: true },
];

test("choosing a preset fills in the name, executable and arguments, and the custom choice leaves them", () => {
  const draft: AcpAgentDraft = { name: null, command: "/typed/before", args: null };
  assert.deepEqual(applyPreset(draft, "two", PRESETS), { name: "Two", command: "two", args: "--acp --stdio" });
  const typed: AcpAgentDraft = { name: "Mine", command: "/opt/mine", args: "serve --acp" };
  assert.equal(applyPreset(typed, CUSTOM_PRESET, PRESETS), typed);
  assert.equal(applyPreset(typed, "nobody", PRESETS), typed);
});

test("a new agent's id is its preset's, else its name as a slug, never one already taken", () => {
  const none = new Set<string>();
  assert.equal(newAgentId("Anything", none, "two"), "two");
  assert.equal(newAgentId("Two", new Set(["two"]), "two"), "two-2", "a preset already added is added again under another id");
  assert.equal(newAgentId("My Agent (dev)", none), "my-agent-dev");
  assert.equal(newAgentId("My Agent", new Set(["my-agent", "my-agent-2"])), "my-agent-3");
  assert.equal(newAgentId("Café Agent", none), "cafe-agent");
  assert.equal(newAgentId("智能体", none), "agent", "a name with no slug in it");
  assert.equal(newAgentId("", none, CUSTOM_PRESET), "agent");
  const long = newAgentId("x".repeat(200), none);
  assert.ok(long.length <= 64 && SLUG.test(long), long);
});

const LIST: AcpAgentList = {
  agents: [{ id: "two", name: "My Two", command: "/opt/two", args: [], envNames: [] }],
  presets: PRESETS,
};

test("a session's agent is shown by its own id: configured, removed, or not known yet", () => {
  assert.deepEqual(acpAgentLook("two", LIST, new Map()), { name: "My Two", glyph: ACP_GLYPH, removed: false });
  assert.deepEqual(acpAgentLook("one", LIST, new Map()), { name: "One", glyph: ACP_GLYPH, removed: true }, "a built-in that is not configured");
  assert.deepEqual(acpAgentLook("gone", LIST, new Map()), { name: "gone", glyph: ACP_GLYPH, removed: true });
  assert.deepEqual(acpAgentLook("gone", undefined, new Map()), { name: "gone", glyph: ACP_GLYPH, removed: false }, "nothing is called removed before the list is in");
  assert.deepEqual(acpAgentLook(null, LIST, new Map()), { name: null, glyph: ACP_GLYPH, removed: false });
});

test("the agent picker names a CLI or one ACP agent, and nothing else", () => {
  for (const agent of [
    { source: "claude", acpAgent: null },
    { source: "codex", acpAgent: null },
    { source: "acp", acpAgent: "bub" },
    { source: "acp", acpAgent: "open-code-2" },
  ] as const) {
    assert.deepEqual(parseAgentKey(agentKey(agent)), agent);
  }
  assert.equal(agentKey({ source: "acp", acpAgent: "bub" }), "acp:bub");
  assert.deepEqual(parseAgentKey("acp"), { source: "acp", acpAgent: null });
  for (const bad of ["acp:", "acp:Bub", "acp:../x", "acp:-x", "gemini", 7, null, undefined]) {
    assert.equal(parseAgentKey(bad), null, String(bad));
  }
});

test("a start on an ACP agent names it, and cannot be sent without one", () => {
  const start: NewConversation = { kind: "session", source: "acp", acpAgent: "bub", cwd: "/w", text: " hi ", model: "", effort: null, permission: "" };
  assert.deepEqual(creationRequest(start, "6f1c2a52-3b7e-4d1f-9a0c-2b8f1e4d5c6a"), {
    path: "/api/sessions/new",
    body: { text: "hi", source: "acp", acpAgent: "bub", cwd: "/w", clientMessageId: "6f1c2a52-3b7e-4d1f-9a0c-2b8f1e4d5c6a" },
  });
  assert.equal(canCreateConversation(start, false), true);
  assert.equal(canCreateConversation({ ...start, acpAgent: null }, false), false);
  const claude = creationRequest({ ...start, source: "claude", model: "fable", permission: "auto" });
  assert.ok(!("acpAgent" in claude.body), "only an ACP start names an agent");
});

test("the new-session draft keeps the ACP agent picked, and an address can name one", () => {
  const storage = new MemoryStorage();
  writeNewSessionDraft(storage, LOCAL, { ...EMPTY_NEW_SESSION_DRAFT, source: "acp", acpAgent: "bub" });
  assert.deepEqual(readNewSessionDraft(storage, LOCAL), { ...EMPTY_NEW_SESSION_DRAFT, source: "acp", acpAgent: "bub" });
  storage.setItem("rh.draft.new-session", JSON.stringify({ ...EMPTY_NEW_SESSION_DRAFT, source: "acp", acpAgent: "../x" }));
  assert.deepEqual(readNewSessionDraft(storage, LOCAL), { ...EMPTY_NEW_SESSION_DRAFT, source: "acp" }, "an id the server could not have is no pick");
  storage.setItem("rh.draft.new-session", JSON.stringify({ ...EMPTY_NEW_SESSION_DRAFT, source: "codex", acpAgent: "bub" }));
  assert.deepEqual(readNewSessionDraft(storage, LOCAL), { ...EMPTY_NEW_SESSION_DRAFT, source: "codex" });
  assert.deepEqual(prefillFromSearch({ agent: "acp:bub" }), { source: "acp", acpAgent: "bub" });
  assert.deepEqual(prefillFromSearch({ agent: "acp" }), { source: "acp" });
  assert.equal(prefillFromSearch({ agent: "acp:Not An Id" }), null);
});

test("the agents page opens on the address's tab, else the one this browser used last, else Claude Code", () => {
  assert.equal(openingTab("acp", "codex"), "acp");
  assert.equal(openingTab(undefined, "codex"), "codex");
  assert.equal(openingTab("gemini", "nonsense"), "claude", "neither names a tab");
  assert.equal(openingTab(undefined, null), "claude");
  assert.deepEqual(agentsSearch("acp", "bub"), { section: "accounts", tab: "acp", agent: "bub" });
  assert.deepEqual(agentsSearch("codex"), { section: "accounts", tab: "codex" });
});

const AT = Date.parse("2026-10-28T12:00:00Z");
const check = (state: string, minutes = 5) => ({ state, checkedAt: new Date(AT - minutes * 60_000).toISOString() }) as never;

test("the ACP tab counts in red every agent whose last check failed; one never checked is not a failure", () => {
  const agents = [
    { ...LIST.agents[0]!, id: "a", lastCheck: check("ready") },
    { ...LIST.agents[0]!, id: "b", lastCheck: check("not_found") },
    { ...LIST.agents[0]!, id: "c", lastCheck: check("failed") },
    { ...LIST.agents[0]!, id: "d" },
  ];
  assert.deepEqual(acpSignal(agents), { warn: 0, danger: 2 });
  assert.deepEqual(acpSignal(undefined), { warn: 0, danger: 0 });
  assert.equal(acpStatus(undefined).kind, "unchecked");
  assert.equal(acpStatus(check("ready"), true).kind, "checking", "a check under way wins");
  assert.equal(acpStatus(check("not_found")).kind, "not_found");
  assert.deepEqual(agoKey(new Date(AT - 30_000).toISOString(), AT), { key: "just now" });
  assert.deepEqual(agoKey(new Date(AT - 5 * 60_000).toISOString(), AT), { key: "{{n}} min ago", n: 5 });
  assert.deepEqual(agoKey(new Date(AT - 3 * 3_600_000).toISOString(), AT), { key: "{{n}} h ago", n: 3 });
  assert.deepEqual(agoKey(new Date(AT - 50 * 3_600_000).toISOString(), AT), { key: "{{n}} d ago", n: 2 });
});

test("a CLI tab counts a signed-out local login and an expired usage login in amber, a refused credential in red, and not a limit rest", () => {
  const rest = (klass: string) => ({ scope: "all", class: klass, untilAt: null, reason: "", observedAt: "2026-10-28T11:00:00Z", sessionId: null, source: "claude:result" });
  const oauth = (id: string, extra: Record<string, unknown> = {}) => ({
    id, label: id, kind: "oauth_token", provider: "claude", usageEnabled: true, rotation: true, rest: [],
    createdAt: "2026-10-01T00:00:00Z", updatedAt: "2026-10-01T00:00:00Z", ...extra,
  });
  const list = {
    accounts: [
      oauth("refused", { rest: [rest("auth")] }),
      oauth("resting", { rest: [{ ...rest("session"), untilAt: "2099-01-01T00:00:00Z" }] }),
      oauth("expired"),
      oauth("not-enabled", { usageEnabled: false }),
      { ...oauth("codex-key"), provider: "codex", kind: "env" },
    ],
    defaultAccountId: null,
    defaultCodexAccountId: null,
    local: { claude: { state: "signed_out", rest: [], isDefault: false }, codex: { state: "unknown", rest: [], isDefault: false } },
  } as never;
  const quota = { accounts: [{ id: "expired", windows: [], error: "not_logged_in", updatedAt: null }], claude: null, codex: null } as never;
  assert.deepEqual(accountsSignal(list, quota, "claude", AT), { warn: 2, danger: 1 });
  assert.deepEqual(accountsSignal(list, quota, "codex", AT), { warn: 0, danger: 0 });
  assert.deepEqual(accountsSignal(undefined, quota, "claude", AT), { warn: 0, danger: 0 });
});
