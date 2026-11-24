// The A2A settings' rules as data (`features/a2a/model.ts`, `docs/a2a.md`
// §4, §6): what a new entry starts with, the bypass rule for an entry other
// devices may call, the bodies the management API takes, the Agent Card URL,
// the task states' words and colours, and the session list's mark. Every
// string the A2A screens put on screen, the model's keys included, is in
// every shipped language.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import langJa from "../src/i18n/lang-ja.js";
import langZhCN from "../src/i18n/lang-zh-CN.js";
import langZhTW from "../src/i18n/lang-zh-TW.js";
import {
  a2aMarks,
  callerLabel,
  cardBase,
  cardUrl,
  createBody,
  ENTRY_PROBLEM_WORD,
  entryFormOf,
  entryProblems,
  idFromName,
  isBypass,
  newEntryForm,
  parseOrigins,
  permissionOptions,
  sessionMarkKey,
  TASK_STATE_WORD,
  TASK_STATES,
  taskSignal,
  updateBody,
  validTokenName,
  withName,
  withSource,
  type A2aEntry,
  type A2aTask,
} from "../src/features/a2a/model.js";

const saved: A2aEntry = {
  id: "dont-do",
  name: "dont-do tasks",
  description: "",
  enabled: true,
  source: "claude",
  acpAgent: null,
  cwd: "/home/studio/code",
  account: { kind: "id", id: "acct-1" },
  model: "fable",
  effort: "high",
  permission: "acceptEdits",
  isolation: "worktree",
  maxConcurrent: 2,
  template: null,
  allowRemoteAnswers: true,
  pushOrigins: ["https://dont-do.example.com"],
  allowedDevices: [],
  createdAt: "2026-11-25T00:00:00Z",
  updatedAt: "2026-11-25T00:00:00Z",
};

test("a new entry: a worktree, edits accepted, questions answerable over A2A, one task at a time, the default account", () => {
  const form = newEntryForm();
  assert.equal(form.source, "claude");
  assert.equal(form.isolation, "worktree");
  assert.equal(form.permission, "acceptEdits");
  assert.equal(form.allowRemoteAnswers, true);
  assert.equal(form.maxConcurrent, 1);
  assert.deepEqual(form.account, { kind: "default" });
  assert.deepEqual(form.allowedDevices, []);
});

test("on Codex the sandbox starts at workspace-write; an ACP agent takes no permission of its own", () => {
  const codex = withSource({ ...newEntryForm(), model: "fable", effort: "high", account: { kind: "local" } }, "codex");
  assert.equal(codex.permission, "workspace-write");
  assert.deepEqual([codex.model, codex.effort, codex.account], ["", "", { kind: "default" }], "nothing the other CLI chose carries over");
  assert.equal(withSource(codex, "claude").permission, "acceptEdits");
  const acp = withSource(codex, "acp", "bub");
  assert.deepEqual([acp.permission, acp.acpAgent], ["", "bub"]);
  assert.ok(permissionOptions("codex", []).includes("workspace-write"));
  assert.ok(!permissionOptions("codex", []).includes("auto"), "Codex's auto is offered under its sandbox name");
});

test("a new entry is created switched off, with the defaults it was shown", () => {
  const form = { ...withName(newEntryForm(), "Dont Do"), cwd: "/home/studio/code" };
  assert.deepEqual(entryProblems(form), []);
  assert.deepEqual(createBody(form), {
    id: "dont-do",
    name: "Dont Do",
    source: "claude",
    enabled: false,
    cwd: "/home/studio/code",
    isolation: "worktree",
    maxConcurrent: 1,
    allowRemoteAnswers: true,
    account: { kind: "default" },
    permission: "acceptEdits",
  });
  assert.equal(createBody({ ...form, source: "acp", acpAgent: "bub", permission: "" }).enabled, false);
  assert.equal("account" in createBody({ ...form, source: "acp", acpAgent: "bub" }), false, "an ACP agent runs on its own configuration");
});

test("an incomplete form says what is missing, and is not saved", () => {
  assert.deepEqual(entryProblems(newEntryForm()), ["id", "name", "cwd"]);
  assert.deepEqual(entryProblems({ ...withName(newEntryForm("acp"), "lab"), cwd: "/lab" }), ["acpAgent"]);
  assert.deepEqual(entryProblems({ ...withName(newEntryForm(), "x"), cwd: "relative/path" }), ["cwd"]);
  assert.deepEqual(entryProblems({ ...withName(newEntryForm(), "x"), cwd: "/x", pushOrigins: "http://plain.example" }), ["pushOrigins"]);
});

test("an entry other devices may call is offered no bypass mode, and cannot be saved holding one", () => {
  for (const cli of ["claude", "codex"] as const) {
    const all = permissionOptions(cli, []);
    const called = permissionOptions(cli, ["d-7Kq2"]);
    assert.ok(all.some(isBypass), `${cli}: offered while nobody else calls it`);
    assert.deepEqual(called.filter(isBypass), [], `${cli}: none while a device may call it`);
    assert.deepEqual(called, all.filter((mode) => !isBypass(mode)));
  }
  assert.ok(isBypass("bypassPermissions") && isBypass("danger-full-access") && isBypass("full-access") && isBypass("bypass-all"));
  assert.ok(!isBypass("acceptEdits") && !isBypass("workspace-write") && !isBypass(null));
  // The mode it holds is still shown, as what is saved — and named as the problem.
  assert.ok(permissionOptions("claude", ["d-7Kq2"], "bypassPermissions").includes("bypassPermissions"));
  const held = entryFormOf({ ...saved, permission: "bypassPermissions", allowedDevices: ["d-7Kq2"] });
  assert.deepEqual(entryProblems(held), ["bypassWithDevices"]);
  assert.deepEqual(entryProblems({ ...held, allowedDevices: [] }), [], "fine while nobody else calls it");
});

test("an edit sends every field it shows, empty where it goes back to the default, and never enabled or the devices", () => {
  const form = entryFormOf(saved);
  assert.equal(form.idTyped, true, "the id stays: it is in the URL");
  const body = updateBody({ ...form, model: "", template: "  " });
  assert.equal(body.model, "", "back to the CLI's default");
  assert.equal(body.template, "", "back to the default template");
  assert.equal(body.acpAgent, "", "a CLI entry has no ACP agent");
  assert.deepEqual(body.account, { kind: "id", id: "acct-1" });
  assert.deepEqual(body.pushOrigins, ["https://dont-do.example.com"]);
  assert.equal("enabled" in body, false);
  assert.equal("allowedDevices" in body, false);
  const acp = updateBody(withSource(form, "acp", "bub"));
  assert.deepEqual([acp.acpAgent, acp.model, acp.effort, acp.permission, acp.account], ["bub", "", "", "", { kind: "default" }]);
});

test("the id is made from the name until it is typed", () => {
  assert.equal(idFromName("Dont Do — nightly!"), "dont-do-nightly");
  assert.equal(idFromName("  --Lab__agent--  "), "lab-agent");
  assert.equal(idFromName("代办"), "", "nothing of it fits: the id is typed");
  assert.equal(idFromName("x".repeat(50)).length, 40);
  const typed = { ...newEntryForm(), id: "mine", idTyped: true };
  assert.equal(withName(typed, "Something else").id, "mine");
});

test("push origins are https origins, one per line", () => {
  assert.deepEqual(parseOrigins("https://a.example\nhttps://b.example:8443/\n\nhttps://a.example"), { origins: ["https://a.example", "https://b.example:8443"], bad: [] });
  assert.deepEqual(parseOrigins("http://a.example https://b.example/path").bad, ["http://a.example", "https://b.example/path"]);
});

test("the Agent Card URL: the public base, else this page's origin, then /a2a/agents/<id>", () => {
  assert.equal(cardUrl(cardBase(null, "http://127.0.0.1:8700"), "dont-do"), "http://127.0.0.1:8700/a2a/agents/dont-do");
  assert.equal(cardUrl(cardBase("https://agents.example.com/", "http://127.0.0.1:8700"), "lab"), "https://agents.example.com/a2a/agents/lab");
  assert.equal(cardUrl(cardBase("  ", "http://127.0.0.1:8700/"), "lab"), "http://127.0.0.1:8700/a2a/agents/lab", "a blank base is none");
  assert.equal(cardBase(null, null), null, "no page origin to name: another device's");
  assert.equal(cardUrl(null, "lab"), "/a2a/agents/lab");
});

test("a token's name is letters, digits, '.', '_' and '-', at most 64", () => {
  assert.ok(validTokenName("dont-do.prod_1"));
  assert.ok(!validTokenName(""));
  assert.ok(!validTokenName("has space"));
  assert.ok(!validTokenName("x".repeat(65)));
});

test("task states: a word for each, colour only where the state is a signal", () => {
  assert.equal(new Set(TASK_STATES).size, 8);
  for (const state of TASK_STATES) assert.ok(TASK_STATE_WORD[state]);
  assert.equal(taskSignal("TASK_STATE_WORKING"), "working");
  assert.equal(taskSignal("TASK_STATE_INPUT_REQUIRED"), "warn");
  assert.equal(taskSignal("TASK_STATE_FAILED"), "failed");
  assert.equal(taskSignal("TASK_STATE_REJECTED"), "failed");
  assert.equal(taskSignal("TASK_STATE_COMPLETED"), null);
  assert.equal(taskSignal("TASK_STATE_CANCELED"), null);
  assert.equal(taskSignal("TASK_STATE_SUBMITTED"), null);
  assert.deepEqual(callerLabel("a2a:token:dont-do"), { kind: "token", name: "dont-do" });
  assert.deepEqual(callerLabel("a2a:device:d-7Kq2"), { kind: "device", name: "d-7Kq2" });
});

test("the session list's mark: each session a task ran in, by its newest task; none before the session is known", () => {
  const task = (id: string, sessionId: string | null, entry: string, caller: string, source: A2aTask["source"] = "claude"): A2aTask => ({
    id, contextId: `c-${id}`, entry, caller, source, sessionId, cwd: "/x", branch: null, statusText: null, createdAt: "", updatedAt: "", state: "TASK_STATE_COMPLETED",
  });
  const marks = a2aMarks([
    task("3", "s1", "dont-do", "a2a:token:new"),
    task("2", "s1", "dont-do", "a2a:token:old"),
    task("1", null, "lab", "a2a:token:x"),
    task("0", "s1", "lab", "a2a:token:y", "codex"),
  ]);
  assert.equal(marks.size, 2);
  assert.deepEqual(marks.get(sessionMarkKey("claude", "s1")), { caller: "a2a:token:new", entry: "dont-do" });
  assert.deepEqual(marks.get(sessionMarkKey("codex", "s1")), { caller: "a2a:token:y", entry: "lab" });
});

/** The `t("…")` keys a source file holds, read from its syntax tree. */
function keysOf(file: string): string[] {
  const text = readFileSync(fileURLToPath(new URL(file, import.meta.url)), "utf8");
  const sf = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const keys: string[] = [];
  const visit = (n: ts.Node) => {
    if (ts.isCallExpression(n) && n.expression.getText(sf) === "t" && n.arguments[0] && ts.isStringLiteral(n.arguments[0])) keys.push(n.arguments[0].text);
    ts.forEachChild(n, visit);
  };
  visit(sf);
  return keys;
}

test("every string the A2A screens say is in every shipped language", () => {
  const files = [
    "../src/features/settings/a2a-section.tsx",
    "../src/features/a2a/entries.tsx",
    "../src/features/a2a/entry-form.tsx",
    "../src/features/a2a/tokens.tsx",
    "../src/features/a2a/tasks.tsx",
  ];
  const sessionMark = keysOf("../src/features/sessions/sessions-layout.tsx").filter((key) => /A2A/.test(key));
  assert.equal(sessionMark.length, 2, "the session row's mark and its title");
  // The words the model hands to `t()`: the task states and the form's problems.
  const model = Object.values(TASK_STATE_WORD);
  const problems = Object.values(ENTRY_PROBLEM_WORD);
  const keys = [...new Set([...files.flatMap(keysOf), ...sessionMark, ...model, ...problems])];
  assert.ok(keys.length > 60);
  for (const [lang, table] of Object.entries({ "zh-CN": langZhCN, "zh-TW": langZhTW, ja: langJa })) {
    assert.deepEqual(keys.filter((key) => !table[key]), [], lang);
  }
});
