// Step 01 · AI tools as drawn (`features/setup/tools-step.tsx`): one test per
// state a card can be in
// — what the card says, its status chip, its buttons — plus the page's one
// primary across states, a page that is not the app's (`canInstall: false`)
// and offline.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { welcomeJa, welcomeZhCN, welcomeZhTW } from "../src/i18n/welcome.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { emptyProgress, start } from "../src/features/setup/model.js";
import { type CardUi, IDLE_CARD, IDLE_TOOLS_UI, type ToolsUi } from "../src/features/setup/tools-model.js";
import { IDLE_STATUS, type ToolActs, ToolsStepView, type ToolsStatus } from "../src/features/setup/tools-step.js";
import type { SetupCommand, ToolsReport } from "../src/features/setup/types.js";
import { GuideScreen, stepAction } from "../src/features/setup/welcome-view.js";
import type { Quota } from "../src/features/accounts/use-accounts.js";
import { LOCAL } from "../src/lib/machines/key.js";
import { mq } from "../src/lib/machines/query.js";
import { commands, HOME, quota, report, task, tool } from "./setup-fixtures.js";

const i18n = i18next.createInstance();
await i18n.use(initReactI18next).init({
  lng: "en",
  resources: { "zh-CN": { translation: { ...welcomeZhCN, ...zhCN } }, "zh-TW": { translation: welcomeZhTW }, ja: { translation: welcomeJa } },
  fallbackLng: false,
  keySeparator: false,
  nsSeparator: false,
  interpolation: { escapeValue: false },
});

function draw(node: React.ReactNode, lang = "en"): string {
  void i18n.changeLanguage(lang);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } } });
  client.setQueryData(mq(LOCAL, "prefs"), { sessionGrouping: "flat", transcriptStyle: "flat", fontSize: "default", language: lang, composerSend: "enter" });
  const html = renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={client}>{node}</QueryClientProvider>
    </I18nextProvider>,
  );
  client.clear();
  return html;
}

const noop = () => undefined;
const acts: ToolActs = { ui: noop, run: noop, login: noop, loginWithKey: noop, stop: noop, dismiss: noop, rescan: noop, use: noop, installGit: noop, openAdvanced: noop };
const primaries = (html: string) => (html.match(/data-variant="primary"/g) ?? []).length;

function step(
  r: ToolsReport | null | undefined,
  {
    ui = IDLE_TOOLS_UI,
    cmds = commands(),
    q = null,
    online = true,
    status = IDLE_STATUS,
    advanced = { open: false, kind: "token" as const },
    lang = "en",
  }: { ui?: ToolsUi; cmds?: SetupCommand[]; q?: Quota | null; online?: boolean; status?: ToolsStatus; advanced?: { open: boolean; kind: "token" | "env" | "codex" }; lang?: string } = {},
): string {
  return draw(
    <ToolsStepView report={r} commands={cmds} quota={q} homeDir={HOME} online={online} ui={ui} status={status} acts={acts} advanced={{ ...advanced, onOpen: noop, onKind: noop }} />,
    lang,
  );
}

/** One card's markup. */
function cardOf(html: string, id: "claude" | "codex"): string {
  const at = html.indexOf(`<section data-tool="${id}"`);
  assert.ok(at >= 0, `${id}'s card is drawn`);
  return html.slice(at, html.indexOf("</section>", at));
}
const opened = (id: "claude" | "codex", ui: Partial<CardUi>): ToolsUi => ({ ...IDLE_TOOLS_UI, [id]: { ...IDLE_CARD, ...ui } });
const missing = report(tool("claude", "missing"), tool("codex", "missing"));

test("checking: both cards, still looking", () => {
  const html = step(undefined);
  for (const id of ["claude", "codex"] as const) {
    const c = cardOf(html, id);
    assert.match(c, /data-state="checking"/);
    assert.match(c, /role="status"[^>]*>[^<]*checking/);
    assert.match(c, /Already looking for Claude Code and Codex on this device…/);
  }
  assert.doesNotMatch(html, /tools-git/, "the git row waits for the report");
  assert.equal(primaries(html), 0);
});

test("not installed: what it needs, Install… (the primary) and the official guide", () => {
  const html = step(missing);
  const claude = cardOf(html, "claude");
  assert.match(claude, /data-state="notInstalled"/);
  assert.match(claude, /role="status" data-tone="neutral"[^>]*>not installed/);
  assert.match(claude, /Claude Code isn&#x27;t on this device yet\./);
  assert.match(claude, /Needs a Claude Pro, Max, Team or Enterprise plan, or an Anthropic Console account\./);
  assert.match(claude, /data-variant="primary"[^>]*data-guide-primary=""[^>]*data-testid="install"[^>]*>Install…/);
  assert.match(claude, /href="https:\/\/code\.claude\.com\/docs\/en\/setup" target="_blank"[^>]*>Official guide/);
  // Codex waits its turn: one primary on the page.
  assert.match(cardOf(html, "codex"), /data-variant="secondary"[^>]*data-testid="install"/);
  assert.equal(primaries(html), 1);
  assert.match(html, /Without Claude Code or Codex, Droppen can show files and open terminals but can&#x27;t run AI\./);
  assert.match(step(missing, { lang: "zh-CN" }), /这台设备上还没有 Claude Code。[\s\S]*安装…/);
});

test("confirm: only the usable methods, the server's command verbatim, three facts, Run it here — and why Homebrew isn't offered", () => {
  const html = step(missing, { ui: opened("claude", { open: "confirm" }), cmds: commands({ brew: false }) });
  const c = cardOf(html, "claude");
  assert.match(c, /data-state="confirm"/);
  assert.match(c, /role="radiogroup" aria-label="How to install"/);
  assert.deepEqual([...c.matchAll(/role="radio" aria-checked="(true|false)"[^>]*data-value="(\w+)"/g)].map((m) => `${m[2]}:${m[1]}`), ["native:true", "npm:false"]);
  assert.match(c, /Official installer<span[^>]*> · recommended/);
  assert.match(c, /Droppen will run this command/);
  assert.match(c, /<code data-command=""[^>]*>\/bin\/sh -c &#x27;t=&quot;\$\(mktemp\)&quot; &amp;&amp; curl --proto =https --tlsv1\.2 -fsSLo &quot;\$t&quot; https:\/\/claude\.ai\/install\.sh &amp;&amp; bash &quot;\$t&quot;; s=\$\?; rm -f &quot;\$t&quot;; exit \$s&#x27;<\/code>/);
  assert.match(c, /From Anthropic \(claude\.ai\)[\s\S]*No administrator password, no Node\.js\.[\s\S]*Takes about a minute\./);
  assert.match(c, /data-testid="confirm-unavailable"[\s\S]*Homebrew: Homebrew isn&#x27;t on this Mac\./);
  assert.match(c, /data-variant="primary"[^>]*data-guide-primary=""[^>]*data-testid="run-here"[^>]*>Run it here/);
  assert.match(c, />Copy command</);
  assert.match(c, />Cancel</);
  assert.match(c, />Official guide/);
  assert.equal(primaries(html), 1, "Codex's Install… gives way to the box the user opened");
  // npm picked: its own fact, with the Node version.
  const npm = cardOf(step(missing, { ui: opened("claude", { open: "confirm", method: "npm" }) }), "claude");
  assert.match(npm, /npm install -g @anthropic-ai\/claude-code/);
  assert.match(npm, /Uses your Node\.js \(v22\.12\.0\)\. Only if you already manage tools with npm\./);
  // Codex's official installer says it edits ~/.zprofile (fact 3).
  assert.match(cardOf(step(missing, { ui: opened("codex", { open: "confirm" }) }), "codex"), /Adds ~\/\.local\/bin to PATH in ~\/\.zprofile/);
  // Nothing usable (an old Mac with neither Homebrew nor Node): the reasons and the guide, nothing to run.
  const none = commands({ oldMac: true, brew: false }).map((c) => (c.method === "npm" ? { ...c, available: false, reason: "no_node" } : c));
  const stuck = cardOf(step(missing, { ui: opened("claude", { open: "confirm" }), cmds: none }), "claude");
  assert.match(stuck, /data-testid="confirm-none"/);
  assert.match(stuck, /The official installer needs macOS 13 or later\.[\s\S]*Homebrew isn&#x27;t on this Mac\.[\s\S]*npm needs Node\.js/);
  assert.doesNotMatch(stuck, /run-here/);
});

test("installing: the embedded terminal (labelled, with time and Stop), the F6 hint, and the chip that blinks", () => {
  const html = step(report(tool("claude", "missing", { task: task("claude", "install") }), tool("codex", "missing")));
  const c = cardOf(html, "claude");
  assert.match(c, /data-state="installing"/);
  assert.match(c, /role="status" data-tone="running"[^>]*>[\s\S]*installing/);
  assert.match(c, /Installing Claude Code\. It keeps going if you leave this step\./);
  assert.match(c, /role="region" aria-label="Terminal · on this device" data-setup-terminal="t-claude-install"/);
  assert.match(c, /data-testid="task-stop"[^>]*>Stop/);
  assert.match(c, /F6 moves between the terminal and the buttons/);
  assert.doesNotMatch(c, /autofocus/i, "the terminal never takes the focus by itself");
  // An update says so.
  const update = cardOf(step(report(tool("claude", "tooOld", { task: task("claude", "update") }), tool("codex", "missing"))), "claude");
  assert.match(update, /Updating Claude Code\./);
  assert.match(update, /data-tone="running"[^>]*>[\s\S]*updating/);
});

test("failed: the exit code, the reason in plain words, the last lines, Retry, Copy command and the guide", () => {
  const failed = task("claude", "install", { running: false, exitCode: 6, failure: "network", tail: ["curl: (6) Could not resolve host: claude.ai"] });
  const html = step(report(tool("claude", "missing", { task: failed }), tool("codex", "missing")));
  const c = cardOf(html, "claude");
  assert.match(c, /data-state="failed"/);
  assert.match(c, /data-tone="failed"[^>]*>failed/);
  assert.match(c, /The install didn&#x27;t finish \(exit code 6\)\. Couldn&#x27;t reach the download server\./);
  assert.match(c, /data-testid="task-tail"[^>]*>curl: \(6\) Could not resolve host: claude\.ai/);
  assert.match(c, /data-variant="primary"[^>]*data-testid="retry"[^>]*>Try again/);
  assert.match(c, />Copy command</);
  assert.match(c, />Official guide/);
  assert.match(cardOf(step(report(tool("claude", "missing", { task: { ...failed, failure: "disk" } }), tool("codex", "missing"))), "claude"), /The disk is full\./);
});

test("signed out: Sign in (primary); Claude offers a Console account and SSO under More, Codex an API key", () => {
  const html = step(report(tool("claude", "signedOut"), tool("codex", "signedOut")));
  const claude = cardOf(html, "claude");
  assert.match(claude, /data-tone="neutral"[^>]*>not signed in/);
  assert.match(claude, /Installed\. Sign in once and Droppen can start Claude Code sessions\./);
  assert.match(claude, /data-variant="primary"[^>]*data-testid="sign-in"[^>]*>Sign in/);
  assert.match(claude, /Use an Anthropic Console account instead/);
  assert.match(claude, /aria-label="More"/);
  assert.match(claude, /2\.1\.292 · ~\/\.local\/bin\/claude/, "version and path, home written ~");
  const codex = cardOf(html, "codex");
  assert.match(codex, /sign in with ChatGPT\./);
  assert.match(codex, /data-testid="use-api-key"[^>]*>Use an OpenAI API key instead/);
  assert.match(codex, /data-variant="secondary"[^>]*data-testid="sign-in"/);
  assert.equal(primaries(html), 1);
  // A sign-in that failed says so.
  const failed = cardOf(step(report(tool("claude", "signedOut", { task: task("claude", "login", { running: false, exitCode: 1 }) }), tool("codex", "ready"))), "claude");
  assert.match(failed, /Signing in didn&#x27;t finish \(exit code 1\)\. Try again\./);
});

test("the API key form: a password field, Sign in, and what happens to the key", () => {
  const c = cardOf(step(report(tool("claude", "ready"), tool("codex", "signedOut")), { ui: opened("codex", { open: "apiKey" }) }), "codex");
  assert.match(c, /data-state="apiKey"/);
  assert.match(c, /<input[^>]*type="password" autoComplete="off"[^>]*data-testid="apikey-input"[^>]*value=""/);
  assert.match(c, /data-variant="primary"[^>]*data-testid="apikey-submit" disabled=""/);
  assert.match(c, /codex login --with-api-key once, through its input\. Codex keeps it; Droppen doesn&#x27;t\./);
});

test("signing in: the terminal, waiting, “Browser didn't open?” and Cancel; Continue stays primary while Codex is ready", () => {
  const r = report(tool("claude", "signedOut", { task: task("claude", "login") }), tool("codex", "ready"));
  const html = step(r);
  const c = cardOf(html, "claude");
  assert.match(c, /data-state="signingIn"/);
  assert.match(c, /data-tone="running"[^>]*>[\s\S]*signing in/);
  assert.match(c, /Finish signing in in your browser, then come back\. Droppen notices by itself\./);
  assert.match(c, /data-setup-terminal="t-claude-login"/);
  assert.match(c, /Waiting for sign-in…/);
  assert.match(c, /data-testid="no-browser-link"[^>]*>Browser didn&#x27;t open\?/);
  assert.match(c, />Cancel</);
  assert.equal(primaries(html), 0, "nobody's card needs the user: Continue is the primary");
  assert.equal(stepAction("tools", start(emptyProgress(), "x"), r, (k) => k, { next: noop, finish: noop }).primary, true);
  // The API key sign-in runs without a terminal.
  const key = cardOf(step(report(tool("claude", "ready"), tool("codex", "signedOut", { task: task("codex", "login", { method: "apikey", terminalId: null }) }))), "codex");
  assert.match(key, /Signing in with the API key…/);
  assert.doesNotMatch(key, /data-setup-terminal/);
});

test("no browser: Claude copies the link and pastes the code back; Codex signs in with a code (the primary)", () => {
  const claude = cardOf(step(report(tool("claude", "signedOut", { task: task("claude", "login") }), tool("codex", "ready")), { ui: opened("claude", { noBrowser: true }) }), "claude");
  assert.match(claude, /data-testid="no-browser"[\s\S]*Press c in the terminal to copy the sign-in link/);
  assert.doesNotMatch(claude, /use-code/);
  const r = report(tool("claude", "ready"), tool("codex", "signedOut", { task: task("codex", "login") }));
  const html = step(r, { ui: opened("codex", { noBrowser: true }) });
  const codex = cardOf(html, "codex");
  assert.match(codex, /Use a one-time code instead: Droppen runs codex login --device-auth/);
  assert.match(codex, /data-variant="primary"[^>]*data-testid="use-code"[^>]*>Sign in with a code/);
  assert.equal(stepAction("tools", start(emptyProgress(), "x"), r, (k) => k, { next: noop, finish: noop }, opened("codex", { noBrowser: true })).primary, false, "Continue gives way");
});

test("ready: the plan, the email and the 5-hour and weekly bars — or that quota comes later; a different account", () => {
  const r = report(tool("claude", "ready"), tool("codex", "ready"));
  const html = step(r, { q: quota() });
  const c = cardOf(html, "claude");
  assert.match(c, /role="status" data-tone="ready"[^>]*><svg[\s\S]*ready/);
  assert.match(c, /Max 20x[\s\S]*lin@example\.com/);
  assert.match(c, /data-quota="5 hours"[\s\S]*12% used[\s\S]*data-quota="this week"[\s\S]*30% used/);
  assert.match(c, /resets /);
  assert.match(c, /data-testid="switch-account"[^>]*>Use a different account/);
  assert.match(cardOf(html, "codex"), /8% used[\s\S]*21% used/);
  assert.equal(primaries(html), 0);
  const later = cardOf(step(r, { q: null }), "claude");
  assert.match(later, /data-testid="quota-later"[^>]*>Quota shows up after the first session\./);
  assert.match(step(r, { q: quota(), lang: "zh-CN" }), /已用 12%/);
});

test("ready through a Droppen account, not a red “not signed in”", () => {
  const c = cardOf(step(report(tool("claude", "viaAccount"), tool("codex", "missing"))), "claude");
  assert.match(c, /data-tone="ready"/);
  assert.match(c, /data-testid="via-account"[^>]*>Ready through the Droppen account “Company gateway”/);
  assert.doesNotMatch(c, /not signed in/);
});

test("update needed: the version, the minimum, Update (into the same confirm flow) and the update command of the way it was installed", () => {
  const r = report(tool("claude", "tooOld"), tool("codex", "ready"));
  const c = cardOf(step(r), "claude");
  assert.match(c, /data-tone="warn"[^>]*>update needed/);
  assert.match(c, /Claude Code 1\.0\.88 is too old for Droppen; it needs 2\.0\.0 or later\./);
  assert.match(c, /data-variant="primary"[^>]*data-testid="update"[^>]*>Update/);
  const confirm = cardOf(step(r, { ui: opened("claude", { open: "confirm", kind: "update" }) }), "claude");
  assert.match(confirm, /npm install -g @anthropic-ai\/claude-code@latest/);
  assert.doesNotMatch(confirm, /role="radiogroup" aria-label="How to install"/, "the way it was installed, no choice");
  assert.match(confirm, /data-testid="run-here"/);
  assert.match(cardOf(step(report(tool("claude", "ready"), tool("codex", "tooOld"))), "codex"), /Codex 1\.0\.88 is too old for Droppen/);
});

test("not found (the shell read timed out): Look again (wait longer), Show where it is…, and a ghost Install a fresh copy", () => {
  const r = report(tool("claude", "ready"), tool("codex", "missing"), { pathRead: "timeout" });
  const c = cardOf(step(r), "codex");
  assert.match(c, /data-state="notFound"/);
  assert.match(c, />not found</);
  assert.match(c, /couldn&#x27;t read your Terminal&#x27;s settings in time \(over 5 s\)/);
  assert.match(c, /data-variant="primary"[^>]*data-testid="look-again"[^>]*>Look again \(wait longer\)/);
  assert.match(c, /data-testid="choose-path-open"[^>]*>Show where it is…/);
  assert.match(c, /data-variant="ghost"[^>]*>Install a fresh copy/);
  assert.match(cardOf(step(r, { status: { ...IDLE_STATUS, rescanning: true } }), "codex"), /disabled=""[^>]*>Looking… \(up to 20 s\)/);
  const path = cardOf(step(r, { ui: opened("codex", { open: "choosePath" }) }), "codex");
  assert.match(path, /data-testid="choose-path"[\s\S]*Where is it\?[\s\S]*which codex[\s\S]*Use this one/);
});

test("found outside the sessions' PATH: “Found it, use it”", () => {
  const c = cardOf(step(report(tool("claude", "fallback"), tool("codex", "ready"))), "claude");
  assert.match(c, /data-state="fallback"/);
  assert.match(c, /Found Claude Code at \/Users\/lin\/\.nvm\/versions\/node\/v22\.12\.0\/bin\/claude/);
  assert.match(c, /data-variant="primary"[^>]*data-testid="use-found"[^>]*>Found it, use it/);
});

test("the plan doesn't include it: claude.ai's plans (primary) and a Console account; a key in the shell opens the advanced form", () => {
  const html = step(report(tool("claude", "plan", { shellKey: true }), tool("codex", "missing")));
  const c = cardOf(html, "claude");
  assert.match(c, /data-tone="failed"[^>]*>plan doesn&#x27;t include it/);
  assert.match(c, /Signed in as lin@example\.com, but the free Claude plan doesn&#x27;t include Claude Code\./);
  assert.match(c, /href="https:\/\/claude\.ai\/upgrade"[^>]*data-variant="primary"[^>]*data-guide-primary=""[^>]*>Plans at claude\.ai/);
  assert.match(c, />Use an Anthropic Console account instead</);
  assert.match(c, /data-testid="shell-key"[\s\S]*Your Terminal sets ANTHROPIC_API_KEY\.[\s\S]*Add it to Droppen/);
  assert.equal(primaries(html), 1);
  assert.match(cardOf(step(report(tool("claude", "ready"), tool("codex", "signedOut", { shellKey: true }))), "codex"), /Your Terminal sets OPENAI_API_KEY\./);
  assert.doesNotMatch(cardOf(step(report(tool("claude", "ready", { shellKey: true }), tool("codex", "missing"))), "claude"), /shell-key/, "ready: nothing to add");
});

test("advanced, folded, then three ways: the setup-token wizard, the env block, a Codex API key; and the link to Settings → Accounts", () => {
  const r = report(tool("claude", "ready"), tool("codex", "ready"));
  const closed = step(r);
  assert.match(closed, /data-testid="advanced-toggle" aria-expanded="false"[\s\S]*Add accounts to Droppen \(advanced\)/);
  assert.doesNotMatch(closed, /role="radio" aria-checked="true" data-kind/);
  const token = step(r, { advanced: { open: true, kind: "token" } });
  assert.match(token, /aria-expanded="true"/);
  assert.match(token, /Most people don&#x27;t need this\./);
  assert.deepEqual([...token.matchAll(/data-kind="(\w+)"/g)].map((m) => m[1]), ["token", "env", "codex"]);
  assert.match(token, /aria-checked="true" data-kind="token"/);
  assert.match(token, /data-testid="adv-token-start"[^>]*>Start/);
  assert.match(token, /href="\/settings\?section=accounts"[^>]*>Rotation and the full list are in Settings → Accounts\./);
  const env = step(r, { advanced: { open: true, kind: "env" } });
  assert.match(env, /<textarea/);
  assert.match(env, /environment variables \(one KEY=VALUE per line\)/);
  assert.match(env, /data-testid="adv-add" disabled=""/);
  const codex = step(r, { advanced: { open: true, kind: "codex" } });
  assert.match(codex, /OPENAI_API_KEY[\s\S]*<input[^>]*type="password"/);
  assert.equal(primaries(env), 0, "adding an account is never the page's primary");
});

test("the git row: ✓ when there, else why and Apple's installer; Check again once asked", () => {
  assert.match(step(report(tool("claude", "ready"), tool("codex", "ready"))), /data-testid="tools-git"[\s\S]*Developer tools \(git\)[\s\S]*optional[\s\S]*data-tone="ready"[\s\S]*installed/);
  const r = report(tool("claude", "ready"), tool("codex", "ready"), { helpers: { brew: true, node: null, git: "missing" } });
  const missingGit = step(r);
  assert.match(missingGit, /Without it, macOS interrupts the first session/);
  assert.match(missingGit, /data-testid="git-install"[^>]*>Install \(Apple\)/);
  assert.match(step(r, { status: { ...IDLE_STATUS, git: "started" } }), /macOS opened Apple&#x27;s installer\.[\s\S]*data-testid="git-check"[^>]*>Check again/);
  assert.doesNotMatch(step(report(tool("claude", "ready"), tool("codex", "ready"), { helpers: { brew: true, node: null, git: "unknown" } })), /tools-git/);
});

test("a page that is not the app's (canInstall false): nothing runs here — copy the command instead", () => {
  const r = report(tool("claude", "signedOut"), tool("codex", "missing"), { canInstall: false, helpers: { brew: true, node: null, git: "missing" } });
  const html = step(r);
  assert.doesNotMatch(html, /data-testid="sign-in"/);
  assert.doesNotMatch(html, /git-install/);
  const claude = cardOf(html, "claude");
  assert.match(claude, /<code data-command=""[^>]*>claude auth login<\/code>/);
  assert.match(claude, /data-variant="primary"[^>]*data-guide-primary=""[^>]*>Copy command/);
  assert.match(claude, /These run from the Droppen app on this Mac\. Copy the command and run it in your own Terminal\./);
  assert.match(html, /data-testid="tools-git"[\s\S]*Copy command/);
  const confirm = cardOf(step(r, { ui: opened("codex", { open: "confirm" }) }), "codex");
  assert.doesNotMatch(confirm, /run-here/);
  assert.match(confirm, /data-variant="primary"[^>]*data-guide-primary=""[^>]*>Copy command/);
  assert.equal(primaries(step(r, { ui: opened("codex", { open: "confirm" }) })), 1);
});

test("offline: install, update and sign in are grey and say why", () => {
  const html = step(report(tool("claude", "signedOut"), tool("codex", "missing")), { online: false });
  assert.match(cardOf(html, "claude"), /data-testid="sign-in" disabled=""/);
  assert.match(cardOf(html, "claude"), /Offline\. Installing, signing in and Droppen need the internet; everything else works\./);
  const confirm = cardOf(step(missing, { online: false, ui: opened("claude", { open: "confirm" }) }), "claude");
  assert.match(confirm, /data-testid="run-here" disabled=""/);
  assert.match(confirm, /Offline\./);
});

test("at most one primary on step 01 in every state, the action bar included", () => {
  const states = [
    tool("claude", "missing"),
    tool("claude", "signedOut"),
    tool("claude", "ready"),
    tool("claude", "tooOld"),
    tool("claude", "plan"),
    tool("claude", "fallback"),
    tool("claude", "viaAccount"),
    tool("claude", "missing", { task: task("claude", "install") }),
    tool("claude", "signedOut", { task: task("claude", "login") }),
    tool("claude", "missing", { task: task("claude", "install", { running: false, exitCode: 1 }) }),
  ];
  const uis: Partial<CardUi>[] = [{}, { open: "confirm" }, { noBrowser: true }];
  const progress = start(emptyProgress(), "2026-10-19T09:00:00Z");
  let checked = 0;
  for (const claude of states) {
    for (const codexBase of states) {
      const codex = { ...codexBase, id: "codex" as const, task: codexBase.task ? { ...codexBase.task, tool: "codex" as const } : null };
      for (const u of uis) {
        for (const canInstall of [true, false]) {
          const r = report(claude, codex, { canInstall, pathRead: checked % 3 === 0 ? "timeout" : "ok" });
          const ui = opened(checked % 2 ? "codex" : "claude", u);
          const action = stepAction("tools", progress, r, (k) => k, { next: noop, finish: noop }, ui);
          const html = draw(<GuideScreen screen="tools" progress={progress} tools={r} online onLater={noop} action={action} toolsUi={ui} />);
          assert.ok(primaries(html) <= 1, `${primaries(html)} primaries: ${claude.id}/${JSON.stringify(u)}/${canInstall}`);
          checked++;
        }
      }
    }
  }
  assert.ok(checked > 500);
});
