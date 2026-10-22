// The Droppen app's first-run guide, as rules (`features/setup/model.ts`,
// `auto-open.ts`): when it opens by
// itself, what Continue, Skip, Back, Finish and a rerun do to the stored
// progress, where it picks up again, and which bar the session list shows.
import assert from "node:assert/strict";
import { test } from "node:test";
import { createFirstRun } from "../src/features/setup/auto-open.js";
import {
  complete,
  dismiss,
  emptyProgress,
  finish,
  nextStep,
  opensFirstSession,
  prevStep,
  rerun,
  resumeStep,
  shouldAutoOpen,
  skip,
  start,
  STEPS,
  stepsLeft,
  toolStatus,
  validateWelcomeSearch,
  whichBar,
} from "../src/features/setup/model.js";
import type { SetupProgress, SetupTool, ToolsReport } from "../src/features/setup/types.js";

const NOW = "2026-10-19T09:00:00Z";
const LATER = "2026-10-19T09:05:00Z";

function progress(over: Partial<SetupProgress> = {}): SetupProgress {
  return { ...emptyProgress(), ...over };
}

function tool(id: "claude" | "codex", over: Partial<SetupTool> = {}): SetupTool {
  return {
    id,
    found: { path: `/Users/demo/.local/bin/${id}`, via: "path", method: "native" },
    version: id === "claude" ? "2.1.211" : "0.163.0",
    minVersion: "0.0.1",
    supported: true,
    login: { state: "signed-in", method: null, plan: null, email: null, usable: null },
    shellKey: false,
    droppenAccounts: { total: 0, defaultLabel: null },
    ready: true,
    task: null,
    ...over,
  };
}

function report(claude: Partial<SetupTool>, codex: Partial<SetupTool>): ToolsReport {
  return {
    pathRead: "ok",
    os: { platform: "darwin", macos: "15.3", arch: "arm64" },
    helpers: { brew: false, node: null, git: "ready" },
    canInstall: false,
    tools: [tool("claude", claude), tool("codex", codex)],
  };
}

const NONE_READY = report({ found: null, ready: false, version: null }, { ready: false, login: { state: "signed-out", method: null, plan: null, email: null, usable: null } });
const ONE_READY = report({ ready: true }, { found: null, ready: false });

test("the guide opens by itself only in the app, on the page load's first route, at home, when the server says so", () => {
  const yes = { desktopApp: true, firstLoad: true, pathname: "/", progress: { autoOpen: true } };
  assert.equal(shouldAutoOpen(yes), true);
  assert.equal(shouldAutoOpen({ ...yes, pathname: "/sessions" }), true);
  assert.equal(shouldAutoOpen({ ...yes, pathname: "/sessions/claude/abc" }), false, "never from a deep link");
  assert.equal(shouldAutoOpen({ ...yes, pathname: "/settings" }), false);
  assert.equal(shouldAutoOpen({ ...yes, desktopApp: false }), false, "a browser never gets it");
  assert.equal(shouldAutoOpen({ ...yes, firstLoad: false }), false, "only once per page load");
  assert.equal(shouldAutoOpen({ ...yes, progress: { autoOpen: false } }), false, "an existing install never gets it");
  assert.equal(shouldAutoOpen({ ...yes, progress: null }), false, "no answer is no");
});

test("the router's question: asked once per page load, and not even asked outside the app or for a deep link", async () => {
  let asked = 0;
  const answer = (autoOpen: boolean) => async () => {
    asked++;
    return { ...emptyProgress(), autoOpen };
  };
  const fresh = createFirstRun({ desktopApp: () => true, progress: answer(true) });
  assert.equal(await fresh("/"), true, "a new install opens the guide");
  assert.equal(await fresh("/sessions"), false, "the redirect itself and every later navigation go where they go");
  assert.equal(asked, 1);

  const existing = createFirstRun({ desktopApp: () => true, progress: answer(false) });
  assert.equal(await existing("/sessions"), false, "an install with sessions, or a guide once started, never opens it");

  asked = 0;
  const deep = createFirstRun({ desktopApp: () => true, progress: answer(true) });
  assert.equal(await deep("/sessions/claude/abc"), false);
  assert.equal(await deep("/"), false, "a later navigation home is not the first load");
  assert.equal(asked, 0);

  const browser = createFirstRun({ desktopApp: () => false, progress: answer(true) });
  assert.equal(await browser("/"), false);
  assert.equal(asked, 0);

  const broken = createFirstRun({ desktopApp: () => true, progress: async () => Promise.reject(new Error("no route")) });
  assert.equal(await broken("/"), false, "a server without the route is read as no");
});

test("Continue, Skip and Back move through the four steps", () => {
  assert.deepEqual(STEPS, ["tools", "where", "first", "wrap"]);
  assert.equal(nextStep("tools"), "where");
  assert.equal(nextStep("wrap"), null);
  assert.equal(prevStep("where"), "tools");
  assert.equal(prevStep("tools"), null, "the welcome page is not a step to go back to");

  let p = start(progress(), NOW);
  assert.equal(p.startedAt, NOW);
  assert.equal(start(p, LATER).startedAt, NOW, "started once");
  p = complete(p, "tools");
  assert.equal(p.steps.tools, "done");
  assert.equal(p.stepFlow.tools, 1, "the flow version it was done in");
  p = skip(p, "where");
  assert.equal(p.steps.where, "skipped");
  assert.equal(skip(p, "tools").steps.tools, "done", "skipping a done step keeps it done");
  assert.equal(stepsLeft(p), 3, "skipped steps are still left to set up");
});

test("picking up again: the first pending step, else the first skipped one", () => {
  assert.equal(resumeStep(progress()), "tools");
  const p = { ...progress(), steps: { tools: "skipped", where: "done", first: "pending", wrap: "pending" } } as SetupProgress;
  assert.equal(resumeStep(p), "first");
  const allSet = { ...progress(), steps: { tools: "done", where: "skipped", first: "done", wrap: "skipped" } } as SetupProgress;
  assert.equal(resumeStep(allSet), "where");
});

test("Finish ends the guide, leaves start at login to the app, and opens the first session once step 03 saved one", () => {
  const p = finish(complete(complete(start(progress(), NOW), "tools"), "where"), LATER);
  assert.equal(p.finishedAt, LATER);
  assert.equal(p.steps.wrap, "done");
  // On by default, but the app's to turn on (at a new install's first
  // launch) and the menu bar's to change; the guide never chooses it.
  assert.equal(p.wrap.startAtLogin, null, "not chosen by the guide");
  assert.equal(finish({ ...progress(), wrap: { startAtLogin: false } }, LATER).wrap.startAtLogin, false, "nor overwritten");
  assert.equal(opensFirstSession(p), false);

  const withFolder = { ...complete(p, "first"), first: { cwd: "/Users/demo/app", agent: "claude" as const, prompt: null } };
  assert.equal(opensFirstSession(withFolder), true);
  assert.equal(opensFirstSession({ ...withFolder, steps: { ...withFolder.steps, first: "skipped" } }), false, "a skipped step 03 opens nothing");
  assert.equal(opensFirstSession({ ...withFolder, first: { ...withFolder.first, agent: null } }), false, "nor one without a tool");
});

test("a rerun clears the steps and the finish, keeps what is set, and the guide stays started", () => {
  const done = finish({ ...complete(start(progress(), NOW), "tools"), where: { choice: "local", lan: false }, first: { cwd: "/x", agent: "codex", prompt: "hi" }, dismissed: true }, LATER);
  const again = rerun(done, "2026-10-20T00:00:00Z");
  assert.deepEqual(again.steps, { tools: "pending", where: "pending", first: "pending", wrap: "pending" });
  assert.equal(again.finishedAt, null);
  assert.equal(again.dismissed, false);
  assert.equal(again.startedAt, NOW, "never auto-opens again");
  assert.deepEqual(again.where, { choice: "local", lan: false }, "the choice stays as the default");
  assert.deepEqual(again.wrap, done.wrap, "step 04's record is kept");
  assert.deepEqual(again.first, { cwd: null, agent: null, prompt: null }, "the first session is picked again");
  assert.deepEqual(validateWelcomeSearch({ rerun: "1" }), { rerun: 1 });
  assert.deepEqual(validateWelcomeSearch({ rerun: 1 }), { rerun: 1 });
  assert.deepEqual(validateWelcomeSearch({ rerun: "yes" }), {});
});

test("the resume bar: while a started guide is unfinished and not dismissed, with the steps left", () => {
  const started = complete(start(progress(), NOW), "tools");
  assert.deepEqual(whichBar({ desktopApp: true, progress: started, tools: ONE_READY }), { kind: "resume", left: 3 });
  assert.deepEqual(whichBar({ desktopApp: true, progress: start(progress(), NOW), tools: undefined }), { kind: "resume", left: 4 }, "skipping the whole guide leaves four");
  assert.equal(whichBar({ desktopApp: true, progress: dismiss(started), tools: NONE_READY }), null, "× is don't remind me");
  assert.equal(whichBar({ desktopApp: true, progress: finish(started, LATER), tools: NONE_READY }), null, "a finished guide says nothing, ready tools or not");
  assert.equal(whichBar({ desktopApp: false, progress: started, tools: NONE_READY }), null, "the app only");
  assert.equal(whichBar({ desktopApp: true, progress: undefined, tools: NONE_READY }), null, "nothing until the progress is known");
});

test("the no-tool bar: an install that never started the guide, only while no AI tool is ready", () => {
  const never = progress({ autoOpen: false });
  assert.deepEqual(whichBar({ desktopApp: true, progress: never, tools: NONE_READY }), { kind: "noTool" });
  assert.equal(whichBar({ desktopApp: true, progress: never, tools: ONE_READY }), null, "one ready tool is enough");
  assert.equal(whichBar({ desktopApp: true, progress: never, tools: undefined }), null, "nothing while detection runs");
  assert.deepEqual(whichBar({ desktopApp: true, progress: never, tools: report({ ready: false, droppenAccounts: { total: 1, defaultLabel: "work" } }, { found: null, ready: false }) }), { kind: "noTool" }, "the server's ready is the word: it already counts Droppen accounts");
});

test("a tool's status in step 01's words", () => {
  assert.equal(toolStatus(tool("claude")), "ready");
  assert.equal(toolStatus(tool("claude", { found: null, ready: false })), "notInstalled");
  assert.equal(toolStatus(tool("claude", { supported: false, ready: false })), "tooOld");
  assert.equal(toolStatus(tool("codex", { ready: false, login: { state: "signed-out", method: null, plan: null, email: null, usable: null } })), "signedOut");
  assert.equal(toolStatus(tool("claude", { ready: false, login: { state: "signed-in", method: "claude.ai", plan: "Free", email: null, usable: false } })), "plan");
});
