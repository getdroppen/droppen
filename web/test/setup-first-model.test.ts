// Step 03's rules (`features/setup/first-model.ts`, `first-session.ts`): only
// ready tools run the
// first session, the first messages offered for a project and for the
// practice folder, the macOS privacy note, what Continue stores, and which
// first session Finish opens.
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  agentOf,
  ago,
  EMPTY_FIRST_UI,
  firstOf,
  firstUiFrom,
  isPracticeFolder,
  noToolReady,
  promptChoices,
  promptOf,
  promptText,
  protectedFolder,
  PROMPTS,
  readyAgents,
} from "../src/features/setup/first-model.js";
import { firstSessionOf } from "../src/features/setup/first-session.js";
import { complete, emptyProgress, start } from "../src/features/setup/model.js";
import { settle, stepAction } from "../src/features/setup/welcome-view.js";
import { report, tool } from "./setup-fixtures.js";

const t = (key: string) => `«${key}»`;
const both = report(tool("claude", "ready"), tool("codex", "ready"));
const codexOnly = report(tool("claude", "missing"), tool("codex", "ready"));
const none = report(tool("claude", "signedOut"), tool("codex", "missing"));

test("only the ready tools run the first session; none ready is step 03's own state", () => {
  assert.deepEqual(readyAgents(both), ["claude", "codex"]);
  assert.deepEqual(readyAgents(codexOnly), ["codex"]);
  assert.equal(agentOf(EMPTY_FIRST_UI, codexOnly), "codex");
  assert.equal(agentOf({ ...EMPTY_FIRST_UI, agent: "claude" }, codexOnly), "codex", "a pick that is not ready is not used");
  assert.equal(agentOf({ ...EMPTY_FIRST_UI, agent: "codex" }, both), "codex");
  assert.equal(noToolReady(none), true);
  assert.equal(noToolReady(both), false);
  assert.equal(noToolReady(undefined), false, "still checking is not “none”");
  assert.equal(noToolReady(null), true, "a server that cannot say has nothing to start with");
});

test("first messages: two that only look in a project, one that builds in the practice folder, or the user's own", () => {
  assert.deepEqual(promptChoices(false), ["repo1", "repo2", "own"]);
  assert.deepEqual(promptChoices(true), ["practice", "own"]);
  assert.match(PROMPTS.repo1, /Don't change any files yet\./);
  assert.match(PROMPTS.repo2, /don't change anything\./);
  assert.equal(promptOf(EMPTY_FIRST_UI), "repo1");
  assert.equal(promptOf({ ...EMPTY_FIRST_UI, practice: true }), "practice");
  assert.equal(promptOf({ ...EMPTY_FIRST_UI, practice: true, prompt: "repo2" }), "practice", "a project's suggestion is not offered in the practice folder");
  assert.equal(promptText({ ...EMPTY_FIRST_UI, prompt: "repo2" }, t), `«${PROMPTS.repo2}»`, "in the reader's language");
  assert.equal(promptText({ ...EMPTY_FIRST_UI, prompt: "own", own: "  look at the tests  " }, t), "look at the tests");
});

test("Documents, Desktop and Downloads: macOS will ask", () => {
  const home = "/Users/lin";
  for (const path of ["/Users/lin/Documents", "/Users/lin/Documents/notes", "/Users/lin/Desktop/site", "/Users/lin/Downloads/x"]) assert.ok(protectedFolder(path, home), path);
  for (const path of ["/Users/lin/Projects/app", "/Users/lin/DocumentsArchive", "/Users/lin", "/Volumes/Documents/a", null]) assert.ok(!protectedFolder(path, home), String(path));
  assert.ok(!protectedFolder("/Users/lin/Documents", ""), "no home known: no guess");
});

test("Continue stores the folder, the tool and the message; nothing until all three are there", () => {
  assert.equal(firstOf(EMPTY_FIRST_UI, both, t), null, "no folder");
  assert.equal(firstOf({ ...EMPTY_FIRST_UI, cwd: "/srv/app" }, none, t), null, "no tool");
  assert.equal(firstOf({ ...EMPTY_FIRST_UI, cwd: "/srv/app", prompt: "own", own: " " }, both, t), null, "an empty message of one's own");
  assert.deepEqual(firstOf({ ...EMPTY_FIRST_UI, cwd: "/srv/app", agent: "codex" }, both, t), { cwd: "/srv/app", agent: "codex", prompt: `«${PROMPTS.repo1}»` });
  const progress = start(emptyProgress(), "2026-10-19T09:00:00Z");
  const ui = { ...EMPTY_FIRST_UI, cwd: "/Users/lin/Droppen/first-session", practice: true };
  const settled = settle("first", progress, { online: true, tools: both, firstUi: ui, t });
  assert.deepEqual(settled.first, { cwd: "/Users/lin/Droppen/first-session", agent: "claude", prompt: `«${PROMPTS.practice}»` });
  assert.deepEqual(settle("first", progress, { online: true, tools: both, firstUi: EMPTY_FIRST_UI, t }).first, progress.first, "incomplete: kept as it was");
  // The action bar: Continue waits for a folder; with no tool there is none (the step's own button leads back).
  const noop = () => undefined;
  assert.equal(stepAction("first", progress, both, t, { next: noop, finish: noop }, undefined, EMPTY_FIRST_UI)?.disabled, true);
  assert.equal(stepAction("first", progress, both, t, { next: noop, finish: noop }, undefined, { ...EMPTY_FIRST_UI, cwd: "/srv/app" })?.disabled, false);
  assert.equal(stepAction("first", progress, none, t, { next: noop, finish: noop }, undefined, { ...EMPTY_FIRST_UI, cwd: "/srv/app" }), null);
});

test("coming back to step 03 shows what it stored; the practice folder is known by its place", () => {
  const progress = { ...emptyProgress(), first: { cwd: "/Users/lin/Droppen/first-session-2", agent: "codex" as const, prompt: "Make a page" } };
  assert.deepEqual(firstUiFrom(progress), { ...EMPTY_FIRST_UI, cwd: "/Users/lin/Droppen/first-session-2", practice: true, agent: "codex", prompt: "own", own: "Make a page" });
  assert.deepEqual(firstUiFrom(emptyProgress()), EMPTY_FIRST_UI);
  assert.ok(isPracticeFolder("/Users/lin/Droppen/first-session"));
  assert.ok(!isPracticeFolder("/Users/lin/Droppen/first-session-x"));
});

test("Finish opens the first session step 03 saved — done, with a folder and a tool — and nothing else", () => {
  const saved = { ...complete(emptyProgress(), "first"), first: { cwd: "/srv/app", agent: "claude" as const, prompt: "Explain" } };
  assert.deepEqual(firstSessionOf(saved), { folder: "/srv/app", agent: "claude", text: "Explain" });
  assert.equal(firstSessionOf({ ...saved, steps: { ...saved.steps, first: "skipped" } }), null, "skipped: the session list");
  assert.equal(firstSessionOf({ ...saved, first: { ...saved.first, agent: null } }), null);
  assert.deepEqual(firstSessionOf({ ...saved, first: { ...saved.first, prompt: null } }), { folder: "/srv/app", agent: "claude", text: "" });
});

test("how long ago, in the reader's language", () => {
  const now = Date.parse("2026-10-19T12:00:00Z");
  assert.equal(ago("2026-10-16T12:00:00Z", "en", now), "3 days ago");
  assert.equal(ago("2026-10-05T12:00:00Z", "en", now), "2 weeks ago");
  assert.equal(ago("2026-10-16T12:00:00Z", "zh-CN", now), "3天前");
  assert.equal(ago("2026-10-19T11:30:00Z", "en", now), "30 minutes ago");
  assert.equal(ago(null, "en", now), "");
});
