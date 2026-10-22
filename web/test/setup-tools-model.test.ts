// Step 01's rules (`features/setup/tools-model.ts`): each card's state from
// the report and what the
// user opened, which card holds the page's one primary, which command a
// confirm box shows, a failed install in words, when to poll, where F6 goes,
// and how a running task or a fresh sign-in reaches the cached report.
import assert from "node:assert/strict";
import { test } from "node:test";
import { withLogin, withTask } from "../src/features/setup/api.js";
import {
  cardState,
  cardsOwnPrimary,
  chosenCommand,
  elapsed,
  f6Next,
  failedLogin,
  failureWords,
  IDLE_CARD,
  IDLE_TOOLS_UI,
  LOGIN_POLL_LIMIT_MS,
  methodChoices,
  offeredMethods,
  pollsLogin,
  primaryOwner,
  readyWindows,
  toolsPoll,
  unavailableReason,
} from "../src/features/setup/tools-model.js";
import { commands, quota, report, STARTED, task, tool } from "./setup-fixtures.js";

const ok = { pathRead: "ok" as const };

test("a card's state: checking, then what the report says, with a running task and an opened form on top", () => {
  assert.equal(cardState(undefined, undefined, IDLE_CARD), "checking");
  assert.equal(cardState(tool("claude", "missing"), ok, IDLE_CARD), "notInstalled");
  assert.equal(cardState(tool("claude", "missing"), { pathRead: "timeout" }, IDLE_CARD), "notFound", "the shell read timed out: maybe hiding");
  assert.equal(cardState(tool("claude", "missing"), { pathRead: "failed" }, IDLE_CARD), "notFound");
  assert.equal(cardState(tool("claude", "signedOut"), ok, IDLE_CARD), "signedOut");
  assert.equal(cardState(tool("claude", "ready"), ok, IDLE_CARD), "ready");
  assert.equal(cardState(tool("claude", "viaAccount"), ok, IDLE_CARD), "ready", "a Droppen account makes it ready");
  assert.equal(cardState(tool("claude", "tooOld"), ok, IDLE_CARD), "tooOld");
  assert.equal(cardState(tool("claude", "plan"), ok, IDLE_CARD), "plan");
  assert.equal(cardState(tool("claude", "fallback"), ok, IDLE_CARD), "fallback", "found outside the sessions' PATH: adopted first, ready or not");
  // What the user opened.
  assert.equal(cardState(tool("claude", "missing"), ok, { ...IDLE_CARD, open: "confirm" }), "confirm");
  assert.equal(cardState(tool("codex", "signedOut"), ok, { ...IDLE_CARD, open: "apiKey" }), "apiKey");
  assert.equal(cardState(tool("codex", "missing"), { pathRead: "timeout" }, { ...IDLE_CARD, open: "choosePath" }), "choosePath");
  // A running task wins over everything.
  assert.equal(cardState(tool("claude", "missing", { task: task("claude", "install") }), ok, { ...IDLE_CARD, open: "confirm" }), "installing");
  assert.equal(cardState(tool("claude", "signedOut", { task: task("claude", "login") }), ok, IDLE_CARD), "signingIn");
  assert.equal(cardState(tool("claude", "tooOld", { task: task("claude", "update") }), ok, IDLE_CARD), "installing");
});

test("a finished task: failed until put away, a success just shows what it changed", () => {
  const failed = task("claude", "install", { running: false, exitCode: 1, failure: "network" });
  assert.equal(cardState(tool("claude", "missing", { task: failed }), ok, IDLE_CARD), "failed");
  assert.equal(cardState(tool("claude", "missing", { task: failed }), ok, { ...IDLE_CARD, dismissed: STARTED }), "notInstalled", "Retry / Stop put it away");
  // It failed, but the tool is there now (installed some other way): the tool's state.
  assert.equal(cardState(tool("claude", "signedOut", { task: failed }), ok, IDLE_CARD), "signedOut");
  // Exit 0: the install worked, the card says what it is now.
  assert.equal(cardState(tool("claude", "signedOut", { task: task("claude", "install", { running: false, exitCode: 0 }) }), ok, IDLE_CARD), "signedOut");
  // A task the server lost (restarted, terminal gone) is not a failure.
  assert.equal(cardState(tool("claude", "missing", { task: task("claude", "install", { running: false, exitCode: null }) }), ok, IDLE_CARD), "notInstalled");
  // A stopped running task is gone from the card at once.
  assert.equal(cardState(tool("claude", "missing", { task: task("claude", "install") }), ok, { ...IDLE_CARD, dismissed: STARTED }), "notInstalled");
  // A failed sign-in is said on the signed-out card.
  const login = tool("codex", "signedOut", { task: task("codex", "login", { running: false, exitCode: 1 }) });
  assert.equal(cardState(login, ok, IDLE_CARD), "signedOut");
  assert.ok(failedLogin(login, login.task));
});

test("an installer that exited 0 without installing anything failed, and says so", () => {
  const nothing = task("claude", "install", { running: false, exitCode: 0, failed: true, tail: ["Installation complete!"] });
  assert.equal(cardState(tool("claude", "missing", { task: nothing }), ok, IDLE_CARD), "failed", "the server's `failed`, not the exit code");
  assert.equal(failureWords(nothing).title, "The installer finished, but the tool still isn't on this device.");
  assert.match(failureWords(nothing).why, /last lines are below/);
  const stale = task("codex", "update", { running: false, exitCode: 0, failed: true });
  assert.equal(cardState(tool("codex", "tooOld", { task: stale }), ok, IDLE_CARD), "failed");
  assert.equal(failureWords(stale).title, "The update finished, but the version is still too old.");
});

test("one primary: the card the user is in, else the first that needs the user, else Continue once one is ready", () => {
  const cards = (claude: ReturnType<typeof tool>, codex: ReturnType<typeof tool>, ui = IDLE_TOOLS_UI) =>
    [claude, codex].map((t) => ({ id: t.id, state: cardState(t, ok, ui[t.id]), ui: ui[t.id] }));
  assert.equal(primaryOwner(cards(tool("claude", "missing"), tool("codex", "missing"))), "claude");
  assert.equal(primaryOwner(cards(tool("claude", "ready"), tool("codex", "missing"))), "codex", "Codex still needs the user");
  assert.equal(primaryOwner(cards(tool("claude", "ready"), tool("codex", "ready"))), null, "both ready: Continue");
  assert.equal(primaryOwner(cards(tool("claude", "signedOut", { task: task("claude", "login") }), tool("codex", "ready"))), null, "a sign-in in progress needs nobody");
  // Opened on Codex: Codex's button outranks Claude's waiting Install….
  const ui = { ...IDLE_TOOLS_UI, codex: { ...IDLE_CARD, open: "confirm" as const } };
  assert.equal(primaryOwner(cards(tool("claude", "missing"), tool("codex", "missing"), ui)), "codex");
  // Codex's "Sign in with a code" during a sign-in.
  const noBrowser = { ...IDLE_TOOLS_UI, codex: { ...IDLE_CARD, noBrowser: true } };
  assert.equal(primaryOwner(cards(tool("claude", "ready"), tool("codex", "signedOut", { task: task("codex", "login") }), noBrowser)), "codex");
  // Claude's no-browser help has nothing to press.
  const claudeNoBrowser = { ...IDLE_TOOLS_UI, claude: { ...IDLE_CARD, noBrowser: true } };
  assert.equal(primaryOwner(cards(tool("claude", "signedOut", { task: task("claude", "login") }), tool("codex", "ready"), claudeNoBrowser)), null);
  // The action bar's question.
  assert.equal(cardsOwnPrimary(report(tool("claude", "ready"), tool("codex", "ready")), IDLE_TOOLS_UI), false);
  assert.equal(cardsOwnPrimary(report(tool("claude", "ready"), tool("codex", "missing")), IDLE_TOOLS_UI), true);
  assert.equal(cardsOwnPrimary(undefined, IDLE_TOOLS_UI), false, "checking: nobody");
});

test("the confirm box offers only usable methods, the official installer first; an update keeps the way it was installed", () => {
  const all = commands({ brew: false });
  const { available, unavailable } = methodChoices(all, "claude", "install");
  assert.deepEqual(available.map((c) => c.method), ["native", "npm"]);
  assert.deepEqual(unavailable.map((c) => c.method), ["brew"]);
  assert.equal(unavailableReason(unavailable[0]!), "Homebrew isn't on this Mac.");
  assert.equal(unavailableReason({ method: "native", reason: "macos_too_old" }), "The official installer needs macOS 13 or later.");
  const missing = tool("claude", "missing");
  assert.equal(chosenCommand(all, missing, { ...IDLE_CARD, open: "confirm" })?.command, `/bin/sh -c 't="$(mktemp)" && curl --proto =https --tlsv1.2 -fsSLo "$t" https://claude.ai/install.sh && bash "$t"; s=$?; rm -f "$t"; exit $s'`, "the recommended one");
  assert.equal(chosenCommand(all, missing, { ...IDLE_CARD, open: "confirm", method: "npm" })?.command, "npm install -g @anthropic-ai/claude-code");
  assert.equal(chosenCommand(all, missing, { ...IDLE_CARD, open: "confirm", method: "brew" })?.method, "native", "an unusable pick falls back");
  // macOS 12: no official Claude installer.
  assert.deepEqual(offeredMethods(commands({ oldMac: true }), missing, "install").map((c) => c.method), ["brew", "npm"]);
  // An npm copy is updated with npm.
  const old = tool("claude", "tooOld");
  assert.deepEqual(offeredMethods(all, old, "update").map((c) => c.command), ["npm install -g @anthropic-ai/claude-code@latest"]);
  assert.equal(chosenCommand(all, old, { ...IDLE_CARD, open: "confirm", kind: "update" })?.command, "npm install -g @anthropic-ai/claude-code@latest");
  // Unknown method: every usable update.
  const unknown = tool("claude", "tooOld", { found: { path: "/x/claude", via: "path", method: "unknown" } });
  assert.deepEqual(offeredMethods(all, unknown, "update").map((c) => c.method), ["native", "npm"]);
});

test("a failed install in plain words: the network, a permission, a full disk, anything else", () => {
  const words = (over: Parameters<typeof task>[2]) => failureWords(task("claude", "install", { running: false, exitCode: 1, ...over }));
  assert.equal(words({ failure: "network" }).title, "The install didn't finish (exit code {{code}}).");
  assert.match(words({ failure: "network" }).why, /Couldn't reach the download server/);
  assert.match(words({ failure: "permission", method: "brew" }).why, /Homebrew on this Mac may belong to another user/, "a Homebrew owned by another user");
  assert.match(words({ failure: "permission" }).why, /Permission denied: it couldn't write/);
  assert.match(words({ failure: "disk" }).why, /disk is full/);
  assert.match(words({ failure: null }).why, /last lines are below/);
  assert.equal(failureWords(task("claude", "update", { running: false, exitCode: 2 })).title, "The update didn't finish (exit code {{code}}).");
  assert.equal(words({ exitCode: 7 }).code, 7);
});

test("polling: the report every 2 s while a task runs; a sign-in every 2 s only while it runs, the page is visible, and for ten minutes", () => {
  assert.equal(toolsPoll(report(tool("claude", "ready"), tool("codex", "missing"))), false);
  assert.equal(toolsPoll(report(tool("claude", "missing", { task: task("claude", "install") }), tool("codex", "missing"))), 2000);
  assert.equal(toolsPoll(undefined), false);
  const signingIn = tool("claude", "signedOut", { task: task("claude", "login") });
  const at = Date.parse(STARTED);
  assert.equal(pollsLogin(signingIn, IDLE_CARD, true, at + 1000), true);
  assert.equal(pollsLogin(signingIn, IDLE_CARD, false, at + 1000), false, "hidden: held");
  assert.equal(pollsLogin(signingIn, IDLE_CARD, true, at + LOGIN_POLL_LIMIT_MS + 1), false, "ten minutes at most");
  assert.equal(pollsLogin(signingIn, { ...IDLE_CARD, dismissed: STARTED }, true, at + 1000), false, "cancelled");
  assert.equal(pollsLogin(tool("claude", "missing", { task: task("claude", "install") }), IDLE_CARD, true, at), false, "an install is not a sign-in");
  assert.equal(pollsLogin(tool("claude", "signedOut", { task: task("claude", "login", { running: false, exitCode: 0 }) }), IDLE_CARD, true, at), false);
});

test("a task just started shows at once; a sign-in the cheap check read turns the card ready", () => {
  const before = report(tool("claude", "signedOut"), tool("codex", "missing"));
  const started = withTask(before, task("claude", "login"));
  assert.equal(cardState(started.tools[0], started, IDLE_CARD), "signingIn");
  assert.equal(started.tools[1], before.tools[1]);
  const signedIn = withLogin(started, "claude", { state: "signed-in", method: "claude.ai", plan: "Pro", email: "lin@example.com", usable: true });
  assert.equal(signedIn.tools[0]!.ready, true);
  assert.equal(withLogin(started, "claude", { state: "signed-in", method: "claude.ai", plan: "Free", email: null, usable: false }).tools[0]!.ready, false, "a plan without it");
  const tooOld = report(tool("claude", "tooOld", { login: { state: "signed-out", method: null, plan: null, email: null, usable: null } }), tool("codex", "missing"));
  assert.equal(withLogin(tooOld, "claude", { state: "signed-in", method: "claude.ai", plan: "Pro", email: null, usable: true }).tools[0]!.ready, false, "still too old");
});

test("the ready card's two windows, a task's time, and F6's order", () => {
  const { fiveHour, week } = readyWindows(quota().claude!.windows);
  assert.equal(fiveHour?.usedPercent, 12);
  assert.equal(week?.usedPercent, 30);
  assert.deepEqual(readyWindows([]), { fiveHour: null, week: null });
  assert.equal(readyWindows([{ label: "weekly (opus)", windowMinutes: 10_080, usedPercent: 1, remainingPercent: 99, resetsAt: null }, { label: "weekly", windowMinutes: 10_080, usedPercent: 5, remainingPercent: 95, resetsAt: null }]).week?.usedPercent, 5);
  const at = Date.parse(STARTED);
  assert.equal(elapsed(STARTED, at + 24_000), "00:24");
  assert.equal(elapsed(STARTED, at + 61_000), "01:01");
  assert.equal(elapsed(STARTED, at + 3_725_000), "1:02:05");
  // Regions: [terminal, bar]. From the page into the terminal, then the bar, then back.
  assert.equal(f6Next(2, -1, false), 0);
  assert.equal(f6Next(2, 0, false), 1);
  assert.equal(f6Next(2, 1, false), 0);
  assert.equal(f6Next(2, 0, true), 1, "Shift+F6 goes the other way");
  assert.equal(f6Next(3, 1, false), 2, "two terminals, then the bar");
  assert.equal(f6Next(0, -1, false), -1);
});
