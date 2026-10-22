// Step 03 · First session as drawn (`features/setup/first-step.tsx`): the
// recent folders, the practice
// folder, another folder through the empty-session view's own browser (and
// no search of the disk for projects), the macOS privacy note, only ready tools to run with,
// the first messages, and the step with no tool ready.
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
import { EMPTY_FIRST_UI, type FirstUi } from "../src/features/setup/first-model.js";
import { FirstStepView } from "../src/features/setup/first-step.js";
import { complete, emptyProgress, start } from "../src/features/setup/model.js";
import type { RecentFolder, ToolsReport } from "../src/features/setup/types.js";
import { GuideScreen, STEP_HEADERS, stepAction } from "../src/features/setup/welcome-view.js";
import { LOCAL } from "../src/lib/machines/key.js";
import { mq } from "../src/lib/machines/query.js";
import { HOME, report, tool } from "./setup-fixtures.js";

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
const primaries = (html: string) => (html.match(/data-variant="primary"/g) ?? []).length;
const both = report(tool("claude", "ready"), tool("codex", "ready"));
const RECENT: RecentFolder[] = [
  { path: `${HOME}/Projects/droppen-site`, name: "droppen-site", tools: ["claude"], lastUsed: new Date(Date.now() - 3 * 86_400_000).toISOString(), sessions: 4 },
  { path: `${HOME}/Documents/notes`, name: "notes", tools: ["codex", "droppen"], lastUsed: null, sessions: 1 },
];

function step(
  r: ToolsReport | null | undefined,
  { ui = EMPTY_FIRST_UI, recent = RECENT, practicePath = null as string | null, lang = "en" }: { ui?: FirstUi; recent?: RecentFolder[]; practicePath?: string | null; lang?: string } = {},
): string {
  return draw(
    <FirstStepView
      report={r}
      ui={ui}
      onUi={noop}
      recent={recent}
      onPractice={noop}
      practice={{ state: "idle", path: practicePath }}
      homeDir={HOME}
      onGoto={noop}
    />,
    lang,
  );
}

test("the folders: recent ones (name, place, which tool, how long ago), the practice folder, another one; no search of the disk", () => {
  const html = step(both);
  assert.match(html, /role="radiogroup" aria-label="Pick a folder for the AI to work in"/);
  assert.match(html, /Used recently with Claude Code or Codex/);
  assert.match(html, /data-folder="\/Users\/lin\/Projects\/droppen-site"[\s\S]*droppen-site[\s\S]*~\/Projects\/droppen-site[\s\S]*Claude Code · 3 days ago/);
  assert.match(html, /~\/Documents\/notes[\s\S]*Codex · Droppen/);
  assert.doesNotMatch(html, /Find projects|scan-/, "no search for projects on the disk");
  assert.match(html, /data-testid="practice-row"[\s\S]*Make a practice folder[\s\S]*~\/Droppen\/first-session: empty, safe to experiment in\./);
  assert.match(html, /data-testid="other-row"[\s\S]*Choose another folder…/);
  assert.equal((html.match(/role="radio" aria-checked="true" [^>]*data-folder/g) ?? []).length, 0, "nothing picked for the user");
  assert.doesNotMatch(step(both, { recent: [] }), /Used recently/, "no recent folders: no section");
});

test("a folder in Documents, Desktop or Downloads: macOS will ask once — choose Allow", () => {
  const html = step(both, { ui: { ...EMPTY_FIRST_UI, cwd: `${HOME}/Documents/notes` } });
  assert.match(html, /data-testid="first-tcc"[^>]*>Folders in Documents, Desktop or Downloads: macOS asks once whether Droppen may open them\. Choose Allow\./);
  assert.doesNotMatch(step(both, { ui: { ...EMPTY_FIRST_UI, cwd: `${HOME}/Projects/droppen-site` } }), /first-tcc/);
});

test("another folder: the empty-session view's own browser; the folder picked there is a row of its own", () => {
  assert.match(step(both, { ui: { ...EMPTY_FIRST_UI, browsing: true } }), /use this folder/);
  const picked = step(both, { ui: { ...EMPTY_FIRST_UI, cwd: "/srv/elsewhere" } });
  assert.match(picked, /data-testid="other-row"[^>]*>[\s\S]*?\/srv\/elsewhere/);
  assert.match(picked, /role="radio" aria-checked="true"[^>]*data-folder="\/srv\/elsewhere"/);
});

test("run with: only the ready tools", () => {
  const one = step(report(tool("claude", "missing"), tool("codex", "ready")));
  assert.match(one, /role="radiogroup" aria-label="Run with"/);
  assert.deepEqual([...one.matchAll(/data-value="(claude|codex)"/g)].map((m) => m[1]), ["codex"]);
  assert.deepEqual([...step(both).matchAll(/data-value="(claude|codex)"/g)].map((m) => m[1]), ["claude", "codex"]);
  assert.match(step(both, { ui: { ...EMPTY_FIRST_UI, agent: "codex" } }), /aria-checked="true"[^>]*data-value="codex"/);
});

test("first messages: two that only look in a project, one that builds in the practice folder, or my own", () => {
  const repo = step(both, { ui: { ...EMPTY_FIRST_UI, cwd: `${HOME}/Projects/droppen-site` } });
  assert.deepEqual([...repo.matchAll(/data-prompt="(\w+)"/g)].map((m) => m[1]), ["repo1", "repo2", "own"]);
  assert.match(repo, /aria-checked="true" data-prompt="repo1"[\s\S]*Explain this project: what it does and how to run it\. Don&#x27;t change any files yet\./);
  const practice = step(both, { ui: { ...EMPTY_FIRST_UI, cwd: `${HOME}/Droppen/first-session`, practice: true }, practicePath: `${HOME}/Droppen/first-session` });
  assert.deepEqual([...practice.matchAll(/data-prompt="(\w+)"/g)].map((m) => m[1]), ["practice", "own"]);
  assert.match(practice, /Make a one-page website that shows today&#x27;s date/);
  assert.match(practice, /role="radio" aria-checked="true"[^>]*data-folder="\/Users\/lin\/Droppen\/first-session" data-testid="practice-row"/);
  const own = step(both, { ui: { ...EMPTY_FIRST_UI, prompt: "own", own: "Read the README" } });
  assert.match(own, /<textarea data-testid="first-own"[^>]*>Read the README<\/textarea>/);
  assert.match(repo, /You send it yourself on the next screen; nothing runs until you press Send\./);
  assert.match(step(both, { lang: "zh-CN" }), /介绍一下这个项目[\s\S]*自己写/);
});

test("the step's lead says what is true: the permission is the view's own, nothing runs before Send", () => {
  assert.doesNotMatch(STEP_HEADERS.first.body ?? "", /asks before changing anything/);
  const progress = complete(start(emptyProgress(), "2026-10-19T09:00:00Z"), "tools");
  const html = draw(<GuideScreen screen="first" progress={progress} tools={both} online onLater={noop} action={null} />);
  assert.match(html, /nothing runs until you press Send on the next screen, where the permission it runs with is shown and can be changed\./);
  assert.match(draw(<GuideScreen screen="first" progress={progress} tools={both} online onLater={noop} action={null} />, "zh-CN"), /它用什么权限运行也显示在那里/);
});

test("no tool ready: say so, the way back to step 1 is the one primary, no Continue; Skip stays", () => {
  const none = report(tool("claude", "signedOut"), tool("codex", "missing"));
  const progress = complete(start(emptyProgress(), "2026-10-19T09:00:00Z"), "where");
  void i18n.changeLanguage("en");
  const t = (key: string) => i18n.t(key);
  const action = stepAction("first", progress, none, t, { next: noop, finish: noop });
  assert.equal(action, null);
  const html = draw(<GuideScreen screen="first" progress={progress} tools={none} online onLater={noop} action={action} />);
  assert.match(html, /data-testid="first-no-tool"[\s\S]*No AI tool is ready yet, so there is nothing to start\. Get one ready in step 1, or skip this step\./);
  assert.match(html, /data-variant="primary"[^>]*data-guide-primary=""[^>]*data-testid="first-back"[^>]*>Back to step 1/);
  assert.doesNotMatch(html, /data-testid="step-action"/);
  assert.match(html, /data-testid="skip-step"/);
  assert.doesNotMatch(html, /data-folder/, "no folders to pick");
  assert.equal(primaries(html), 1);
  assert.match(draw(<GuideScreen screen="first" progress={progress} tools={none} online onLater={noop} action={action} />, "zh-CN"), /回到第 1 步/);
});

test("Continue is the page's one primary, grey until a folder is picked", () => {
  void i18n.changeLanguage("en");
  const progress = complete(start(emptyProgress(), "2026-10-19T09:00:00Z"), "where");
  const t = (key: string) => i18n.t(key);
  for (const ui of [EMPTY_FIRST_UI, { ...EMPTY_FIRST_UI, cwd: `${HOME}/Projects/droppen-site` }]) {
    const action = stepAction("first", progress, both, t, { next: noop, finish: noop }, undefined, ui);
    const html = draw(<GuideScreen screen="first" progress={progress} tools={both} online onLater={noop} action={action} firstUi={ui} />);
    assert.equal(primaries(html), 1);
    assert.match(html, ui.cwd ? /data-testid="step-action" data-guide-primary="">Continue/ : /data-testid="step-action" data-guide-primary="" disabled="">Continue/);
  }
});
