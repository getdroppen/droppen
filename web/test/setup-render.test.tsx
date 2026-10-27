// The Droppen app's first-run guide as drawn (`features/setup/`): the frame's
// header, rail and action bar, at most one primary button on every screen
// (docs/ui-spec.md V10), step 02 with and without signing in to Droppen and
// how that sign-in stands, step 04's Finish, and the bars above the session
// list.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { welcomeJa, welcomeZhCN, welcomeZhTW } from "../src/i18n/welcome.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { SetupBarView } from "../src/features/setup/bars.js";
import { complete, emptyProgress, skip, start, STEPS, type Screen } from "../src/features/setup/model.js";
import { chooseWhere, WhereStep, WrapStep } from "../src/features/setup/screens.js";
import { onceClicked, pressesPrimary, repeatedClick } from "../src/features/setup/frame.js";
import { SetupSectionView } from "../src/features/setup/settings-section.js";
import type { SetupProgress, SetupTool, ToolsReport } from "../src/features/setup/types.js";
import { GuideScreen, LINKED_TOAST, linkedToast, stepAction } from "../src/features/setup/welcome-view.js";
import { type GuideSignIn, type WhereSignIn, whereSignIn } from "../src/features/setup/where-sign-in.js";
import type { Attempt } from "../src/features/link/opener.js";
import { LOCAL } from "../src/lib/machines/key.js";
import { mq } from "../src/lib/machines/query.js";
import { makeI18n } from "./render.js";

const i18n = makeI18n({
  lng: "en",
  resources: { "zh-CN": { translation: { ...welcomeZhCN, ...zhCN } }, "zh-TW": { translation: welcomeZhTW }, ja: { translation: welcomeJa } },
  fallbackLng: false,
  keySeparator: false,
  nsSeparator: false
});

function tool(id: "claude" | "codex", ready: boolean): SetupTool {
  return {
    id,
    found: ready ? { path: `/Users/demo/.local/bin/${id}`, via: "path", method: "native" } : null,
    version: ready ? (id === "claude" ? "2.1.211" : "0.163.0") : null,
    minVersion: "0.0.1",
    supported: ready,
    login: { state: ready ? "signed-in" : "signed-out", method: null, plan: ready && id === "claude" ? "Max 20x" : null, email: null, usable: null },
    shellKey: false,
    droppenAccounts: { total: 0, defaultLabel: null },
    ready,
    task: null,
  };
}
const tools = (claude: boolean, codex: boolean): ToolsReport => ({
  pathRead: "ok",
  os: { platform: "darwin", macos: "15.3", arch: "arm64" },
  helpers: { brew: false, node: null, git: "ready" },
  canInstall: false,
  tools: [tool("claude", claude), tool("codex", codex)],
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
const t = (key: string) => i18n.t(key);
const primaries = (html: string) => (html.match(/data-variant="primary"/g) ?? []).length;

function screen(at: Screen, progress: SetupProgress, report: ToolsReport | null | undefined, lang = "en", online = true): string {
  void i18n.changeLanguage(lang);
  const action = at === "welcome" ? null : stepAction(at, progress, report, t, { next: noop, finish: noop });
  return draw(
    <GuideScreen screen={at} progress={progress} tools={report} online={online} onLater={noop} onBack={at === "tools" || at === "welcome" ? null : noop} action={action} />,
    lang,
  );
}

const started = start(emptyProgress(), "2026-10-19T09:00:00Z");

test("the header says “Get started with Droppen” — 「开始使用 Droppen」, never 「设置 Droppen」", () => {
  const en = screen("welcome", emptyProgress(), undefined);
  assert.match(en, /data-testid="guide-title"[^>]*>Get started with Droppen</);
  const zh = screen("where", started, tools(true, false), "zh-CN");
  assert.match(zh, /data-testid="guide-title"[^>]*>开始使用 Droppen</);
  assert.doesNotMatch(zh, /设置 Droppen/);
  assert.match(screen("tools", started, undefined, "ja"), /Droppen を使い始める/);
});

test("every screen has at most one primary button, and the welcome page's is “Get started”", () => {
  const welcome = screen("welcome", emptyProgress(), tools(true, true));
  assert.equal(primaries(welcome), 1);
  assert.match(welcome, /data-variant="primary"[^>]*>Get started</);
  for (const at of STEPS) {
    for (const report of [undefined, null, tools(false, false), tools(true, false), tools(true, true)]) {
      for (const lang of ["en", "zh-CN"]) {
        const html = screen(at, started, report, lang);
        assert.ok(primaries(html) <= 1, `${at} in ${lang} with ${JSON.stringify(report?.tools.map((x) => x.ready))}: ${primaries(html)} primaries`);
      }
    }
  }
  // With nothing ready, step 01 goes on without a primary: the first card that needs the user holds it.
  const none = screen("tools", started, tools(false, false));
  assert.equal(primaries(none), 1);
  assert.match(none, /data-testid="step-action"[^>]*>Continue without AI tools</);
  assert.match(none, /data-tool="claude"[\s\S]*data-variant="primary"[^>]*>Install…</);
  // Codex still needs the user, so its card keeps the primary; Continue is there, not primary.
  const one = screen("tools", started, tools(true, false));
  assert.equal(primaries(one), 1);
  assert.match(one, /data-variant="secondary"[^>]*data-testid="step-action"[^>]*>Continue</);
  // Both ready: Continue is the primary, and Enter goes on.
  assert.match(screen("tools", started, tools(true, true)), /data-variant="primary"[^>]*data-testid="step-action"[^>]*>Continue</);
});

test("the rail: the current step is ink and aria-current, a done one ticked, a skipped one says so", () => {
  const p = skip(complete(started, "tools"), "where");
  const html = screen("first", p, tools(true, false));
  assert.match(html, /<nav[^>]*data-testid="step-rail"/);
  assert.match(html, /<li data-step="tools" data-state="done">[\s\S]*?done<\/span>/);
  assert.match(html, /<li data-step="where" data-state="skipped">[\s\S]*?skipped<\/span>/);
  assert.match(html, /<li data-step="first" data-state="current"><button[^>]*aria-current="step"[^>]*class="[^"]*bg-fg text-bg/);
  assert.match(html, /<li data-step="wrap" data-state="pending">/);
  assert.equal((html.match(/aria-current="step"/g) ?? []).length, 1);
  assert.match(html, /Step 3 of 4/);
  const zh = screen("first", p, tools(true, false), "zh-CN");
  assert.match(zh, /第 3 步，共 4 步/);
  assert.match(zh, /已跳过/);
  // The step's title takes the focus as it opens.
  assert.match(html, /<h1 tabindex="-1" data-step-heading=""[^>]*>Pick a folder for the AI to work in<\/h1>/);
});

test("the welcome page says what detection found", () => {
  assert.match(screen("welcome", emptyProgress(), undefined), /Already looking for Claude Code and Codex on this device…/);
  assert.match(screen("welcome", emptyProgress(), tools(true, true)), /Found: Claude Code, Codex/);
  assert.match(screen("welcome", emptyProgress(), tools(true, true), "zh-CN"), /已找到：Claude Code、Codex/);
  assert.match(screen("welcome", emptyProgress(), tools(false, false)), /No AI tool is ready on this device yet\./);
  assert.doesNotMatch(screen("welcome", emptyProgress(), null), /welcome-probe/, "an older server without detection: no line");
  assert.doesNotMatch(screen("welcome", emptyProgress(), undefined), /guide-later/, "the welcome page has its own way out");
  assert.match(screen("where", started, undefined), /data-testid="guide-later"[^>]*>Finish later</);
});

test("offline, the top bar says so", () => {
  assert.match(screen("where", started, undefined, "en", false), /data-testid="guide-offline"[^>]*>.*offline</);
  assert.doesNotMatch(screen("where", started, undefined), /guide-offline/);
});

test("step 02: two radios, signing in to Droppen recommended and chosen", () => {
  const html = draw(<WhereStep progress={started} tools={undefined} online onChange={noop} />);
  assert.match(html, /role="radiogroup"/);
  assert.equal((html.match(/role="radio"/g) ?? []).length, 2);
  assert.match(html, /aria-checked="true" tabindex="0" data-choice="droppen"/);
  assert.doesNotMatch(html, /droppen-note/, "online, with no reason to hold it back");
  assert.match(html, /aria-checked="false" tabindex="-1" data-choice="local"/);
  assert.match(html, /Recommended/);
  assert.doesNotMatch(html, /Let browsers on my local network in too/, "the local network waits for its own change");
  assert.match(html, /The Droppen app on this Mac never asks for a password\./);
  assert.match(html, /Change it later in Settings → Access\./);
  // A stored "droppen" is shown as it stands.
  assert.match(draw(<WhereStep progress={{ ...started, where: { choice: "droppen", lan: false } }} tools={undefined} online onChange={noop} />), /aria-checked="true" tabindex="0" data-choice="droppen"/);
  // Offline, signing in cannot be had: this Mac only, with why.
  const offline = draw(<WhereStep progress={started} tools={undefined} online={false} onChange={noop} />);
  assert.match(offline, /data-choice="droppen"[\s\S]*droppen.app needs the internet/);
  assert.match(offline, /aria-checked="true" tabindex="0" data-choice="local"/);
});

const NOW = Date.parse("2026-10-22T09:00:00Z");
const ATTEMPT: Attempt = {
  attempt: "att-1",
  nonce: "n-1",
  hubWeb: "https://droppen.app",
  expiresMs: NOW + 14 * 60_000,
  machine: { hostname: "studio", signKey: "aa", dhKey: "bb", fingerprint: "1a2b 3c4d 5e6f 7a8b 9c0d 1e2f 3a4b 5c6d" },
  handoffUrl: "https://droppen.app/link#droppen-link=x",
};
const status = (state: string, extra: Record<string, unknown> = {}) =>
  ({
    state,
    localAuth: true,
    canManage: true,
    hubWeb: "https://droppen.app",
    machine: { hostname: "studio", fingerprint: "1a2b 3c4d", id: state === "none" ? null : "m-1" },
    account: state === "linked" ? { id: "a-1", email: "ada@example.com", fingerprint: "ff" } : null,
    ...extra,
  }) as unknown as Parameters<typeof whereSignIn>[0];
const signInAt = (state: WhereSignIn, calls: string[] = []): GuideSignIn => ({
  state,
  start: () => void calls.push("start"),
  reopen: () => void calls.push("reopen"),
  cancel: () => void calls.push("cancel"),
});

test("step 02's sign-in: how the device's standing and the page's sign-in read", () => {
  assert.deepEqual(whereSignIn(undefined, { kind: "idle" }), { kind: "unavailable", reason: "loading", owner: null }, "nothing known yet");
  assert.equal(whereSignIn(status("none"), { kind: "idle" }).kind, "none");
  assert.equal(whereSignIn(status("removed"), { kind: "idle" }).kind, "none", "removed: sign in again");
  assert.deepEqual(whereSignIn(status("none", { canManage: false, owner: "ada" }), { kind: "idle" }), { kind: "unavailable", reason: "not_manager", owner: "ada" });
  assert.deepEqual(whereSignIn(status("none", { localAuth: false }), { kind: "idle" }), { kind: "unavailable", reason: "no_local_auth", owner: null });
  assert.deepEqual(whereSignIn(status("broken"), { kind: "idle" }), { kind: "unavailable", reason: "broken", owner: null });
  assert.equal(whereSignIn(status("none"), { kind: "starting" }).kind, "pending");
  assert.equal(whereSignIn(status("none"), { kind: "browser", attempt: ATTEMPT }).kind, "pending");
  assert.equal(whereSignIn(status("enrolling"), { kind: "idle" }).kind, "pending", "waiting for a device to add it");
  // Why it stopped: the hub not reached, the link run out, or a refusal in its own words.
  assert.deepEqual(whereSignIn(status("none"), { kind: "error", message: "connection refused", code: "hub_unreachable" }), { kind: "failed", failure: "unreachable", message: "connection refused" });
  assert.deepEqual(whereSignIn(status("none"), { kind: "error", message: "busy", code: "unavailable" }), { kind: "failed", failure: "unreachable", message: "busy" });
  for (const code of ["already_enrolled", "not_owner", "refused", undefined]) {
    assert.deepEqual(whereSignIn(status("none"), { kind: "error", message: "the hub said no", code }), { kind: "failed", failure: "refused", message: "the hub said no" }, String(code));
  }
  assert.deepEqual(whereSignIn(status("none"), { kind: "expired" }), { kind: "failed", failure: "expired", message: null });
  // In the account wins over whatever the flow last said.
  assert.deepEqual(whereSignIn(status("linked"), { kind: "error", message: "late" }), { kind: "linked", email: "ada@example.com", device: "studio", fingerprint: "1a2b 3c4d" });
  assert.equal(whereSignIn(status("not_hosting"), { kind: "idle" }).kind, "linked");
});

test("step 02 with “Sign in to Droppen” chosen: its primary signs in, keeps going while it is under way, and offers this Mac only when it stopped", () => {
  void i18n.changeLanguage("en");
  const go = { next: () => void calls.push("next"), finish: noop, local: () => void calls.push("local") };
  const calls: string[] = [];
  const action = (state: WhereSignIn, progress: SetupProgress = started) => stepAction("where", progress, undefined, t, go, undefined, undefined, true, signInAt(state, calls))!;

  const none = action({ kind: "none" });
  assert.equal(none.label, "Sign in in the browser");
  none.onClick();
  assert.deepEqual(calls.splice(0), ["start"], "the click itself starts it (the browser opens from it)");
  const pending = action({ kind: "pending", flow: { kind: "browser", attempt: ATTEMPT } });
  assert.equal(pending.label, "Keep going; I'll finish in the browser");
  pending.onClick();
  assert.deepEqual(calls.splice(0), ["next"]);
  assert.equal(action({ kind: "linked", email: "ada@example.com", device: "studio", fingerprint: null }).label, "Continue");
  const failed = action({ kind: "failed", failure: "refused", message: "x" });
  assert.equal(failed.label, "Only on this Mac for now");
  failed.onClick();
  assert.deepEqual(calls.splice(0), ["local"]);
  // This Mac only, or no sign-in to follow: Continue, as before.
  assert.equal(action({ kind: "none" }, { ...started, where: { choice: "local", lan: false } }).label, "Continue");
  assert.equal(stepAction("where", started, undefined, t, go)!.label, "Continue");
  void i18n.changeLanguage("zh-CN");
  assert.equal(stepAction("where", started, undefined, (key) => i18n.t(key), go, undefined, undefined, true, signInAt({ kind: "none" }))!.label, "在浏览器中登录");
  void i18n.changeLanguage("en");
});

test("step 02 shows how the sign-in stands under the choices, with one primary on the page", () => {
  const drawWhere = (state: WhereSignIn) => {
    void i18n.changeLanguage("en");
    const sign = signInAt(state);
    const action = stepAction("where", started, undefined, t, { next: noop, finish: noop, local: noop }, undefined, undefined, true, sign);
    return draw(<GuideScreen screen="where" progress={started} tools={undefined} online onLater={noop} onBack={noop} action={action} signIn={sign} />);
  };
  assert.doesNotMatch(drawWhere({ kind: "none" }), /data-testid="where-sign-in"/, "nothing under the choices before it starts");
  const waiting = drawWhere({ kind: "pending", flow: { kind: "browser", attempt: ATTEMPT } });
  assert.match(waiting, /data-testid="where-sign-in" data-state="pending"[\s\S]*data-testid="droppen-browser-waiting"/);
  assert.match(waiting, /1a2b 3c4d 5e6f 7a8b 9c0d 1e2f 3a4b 5c6d/, "the fingerprint droppen.app shows too");
  assert.match(waiting, /data-testid="droppen-reopen-browser"/);
  assert.equal(primaries(waiting), 1, "the browser's own button is not a second primary");
  assert.match(waiting, /data-variant="primary"[^>]*data-testid="step-action"[^>]*>Keep going; I&#x27;ll finish in the browser</);
  assert.match(drawWhere({ kind: "pending", flow: { kind: "starting" } }), /data-testid="where-progress"[^>]*>[\s\S]*opening droppen.app…/);
  const linked = drawWhere({ kind: "linked", email: "ada@example.com", device: "studio", fingerprint: "1a2b 3c4d" });
  assert.match(linked, /data-testid="where-linked"[^>]*>[\s\S]*This Mac is in your Droppen account/);
  assert.match(linked, /sign in as ada@example\.com\. “studio” is there\./);
  assert.equal(primaries(linked), 1);
  // The hub not reached: "Can't reach droppen.app", what to do, and the reason in small print.
  const unreachable = drawWhere({ kind: "failed", failure: "unreachable", message: "hub_unreachable · 502" });
  assert.match(unreachable, /data-testid="where-failed"[\s\S]*Can&#x27;t reach droppen.app[\s\S]*When you&#x27;re online[\s\S]*hub_unreachable · 502[\s\S]*data-testid="where-retry"/);
  assert.match(unreachable, /data-variant="primary"[^>]*data-testid="step-action"[^>]*>Only on this Mac for now</);
  // The link ran out: that sentence is the heading, and nothing about being online.
  const expired = drawWhere({ kind: "failed", failure: "expired", message: null });
  assert.match(expired, /data-failure="expired"[\s\S]*<p class="[^"]*font-semibold">The sign-in ran out of time\./);
  assert.doesNotMatch(expired, /Can&#x27;t reach|When you&#x27;re online/);
  // A refusal says only what refused it.
  const refused = drawWhere({ kind: "failed", failure: "refused", message: "this device is in another account already" });
  assert.match(refused, /data-failure="refused"[\s\S]*<p class="[^"]*font-semibold">this device is in another account already<\/p>[\s\S]*data-testid="where-retry"/);
  assert.doesNotMatch(refused, /Can&#x27;t reach|When you&#x27;re online/);
  // The panel is read out as it changes, there before anything is in it.
  assert.match(drawWhere({ kind: "none" }), /<div role="status" aria-live="polite" data-testid="where-sign-in-live"><\/div>/);
  assert.match(waiting, /role="status" aria-live="polite" data-testid="where-sign-in-live"><div data-testid="where-sign-in"/);
  // This Mac only: no sign-in shown, whatever stands.
  assert.doesNotMatch(
    draw(<WhereStep progress={{ ...started, where: { choice: "local", lan: false } }} tools={undefined} online onChange={noop} signIn={signInAt({ kind: "pending", flow: { kind: "starting" } })} />),
    /where-sign-in"/,
  );
  assert.match(drawWhere({ kind: "linked", email: "ada@example.com", device: "studio", fingerprint: null }), /data-choice="droppen"/);
  const zh = draw(<WhereStep progress={started} tools={undefined} online onChange={noop} signIn={signInAt({ kind: "linked", email: "ada@example.com", device: "studio", fingerprint: null })} />, "zh-CN");
  assert.match(zh, /这台 Mac 已加入你的 Droppen 账户/);
});

test("step 02 says why it cannot sign in from here, and that a Mac in an account stays there on “Only on this Mac”", () => {
  const where = (state: WhereSignIn, choice: "droppen" | "local" = "droppen", lang = "en") =>
    draw(<WhereStep progress={{ ...started, where: { choice, lan: false } }} tools={undefined} online onChange={noop} signIn={signInAt(state)} />, lang);
  assert.doesNotMatch(where({ kind: "unavailable", reason: "loading", owner: null }), /where-unavailable/, "nothing while it loads");
  assert.match(where({ kind: "unavailable", reason: "broken", owner: null }), /data-testid="where-unavailable" data-reason="broken"[^>]*>this device&#x27;s Droppen state does not load</);
  assert.match(where({ kind: "unavailable", reason: "not_manager", owner: "ada" }), /data-reason="not_manager"[^>]*>Only ada can sign this device in\.</);
  assert.match(where({ kind: "unavailable", reason: "not_manager", owner: null }), /data-reason="not_manager"[^>]*>This window cannot sign this device in\. See Settings → Access → Droppen\.</);
  assert.match(where({ kind: "unavailable", reason: "no_local_auth", owner: null }), /data-reason="no_local_auth"[^>]*>Create a local account for this device first/);
  // "Only on this Mac" for a Mac in an account: it stays in it, and where to take it out.
  const stays = where({ kind: "linked", email: "ada@example.com", device: "studio", fingerprint: null }, "local");
  assert.match(stays, /data-testid="where-stays-linked"[^>]*>This Mac stays in your Droppen account\. To take it out, use Settings → Access → Droppen\.</);
  assert.doesNotMatch(stays, /where-linked/);
  assert.doesNotMatch(where({ kind: "none" }, "local"), /where-stays-linked/);
  assert.match(where({ kind: "linked", email: "ada@example.com", device: "studio", fingerprint: null }, "local", "zh-CN"), /这台 Mac 仍在你的 Droppen 账户里/);
});

test("choosing “Only on this Mac” ends a sign-in under way or one that stopped, and nothing else", () => {
  for (const state of [
    { kind: "pending", flow: { kind: "browser", attempt: ATTEMPT } },
    { kind: "failed", failure: "refused", message: "no" },
  ] as WhereSignIn[]) {
    const calls: string[] = [];
    const next = chooseWhere(started, "local", signInAt(state, calls));
    assert.equal(next.where.choice, "local");
    assert.deepEqual(calls, ["cancel"], state.kind);
  }
  for (const [state, choice] of [
    [{ kind: "linked", email: null, device: null, fingerprint: null }, "local"],
    [{ kind: "none" }, "local"],
    [{ kind: "failed", failure: "expired", message: null }, "droppen"],
  ] as [WhereSignIn, "local" | "droppen"][]) {
    const calls: string[] = [];
    assert.equal(chooseWhere(started, choice, signInAt(state, calls)).where.choice, choice);
    assert.deepEqual(calls, [], `${state.kind} → ${choice}`);
  }
  assert.equal(chooseWhere(started, "local").where.choice, "local", "no sign-in to follow");
});

test("the guide's toast once this Mac is in the account: only after it was seen under way, and not on step 02", () => {
  assert.deepEqual(linkedToast(false, "pending", "where"), { toast: false, pending: true });
  assert.deepEqual(linkedToast(true, "linked", "first"), { toast: true, pending: false }, "kept going, then it landed");
  assert.deepEqual(linkedToast(true, "linked", "wrap"), { toast: true, pending: false });
  assert.deepEqual(linkedToast(true, "linked", "where"), { toast: false, pending: false }, "step 02 says so itself");
  assert.deepEqual(linkedToast(false, "linked", "first"), { toast: false, pending: false }, "in the account all along: no news");
  assert.deepEqual(linkedToast(true, "failed", "first"), { toast: false, pending: false }, "it stopped: forgotten");
  assert.deepEqual(linkedToast(true, "none", "first"), { toast: false, pending: false }, "cancelled: forgotten");
  // The words are the guide's, in every language.
  assert.equal(LINKED_TOAST, "This Mac is in your Droppen account");
  assert.equal(welcomeZhCN[LINKED_TOAST], "这台 Mac 已加入你的 Droppen 账户");
});

test("a double click and a held Enter press one primary, not the next step's too", () => {
  assert.equal(repeatedClick({ detail: 1 }), false);
  assert.equal(repeatedClick({ detail: 0 }), false, "a click from the keyboard");
  assert.equal(repeatedClick({ detail: 2 }), true);
  let presses = 0;
  const click = onceClicked(() => presses++);
  // A double click: the first goes through, the second (detail 2) lands on whatever is there now and is dropped.
  click({ detail: 1 });
  click({ detail: 2 });
  click({ detail: 3 });
  click({ detail: 0 });
  assert.equal(presses, 2);
  const enter = { key: "Enter", repeat: false, defaultPrevented: false, isComposing: false, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false };
  assert.equal(pressesPrimary(enter), true);
  assert.equal(pressesPrimary({ ...enter, repeat: true }), false, "a held Enter's repeats");
  assert.equal(pressesPrimary({ ...enter, isComposing: true }), false);
  assert.equal(pressesPrimary({ ...enter, shiftKey: true }), false);
  assert.equal(pressesPrimary({ ...enter, defaultPrevented: true }), false);
  assert.equal(pressesPrimary({ ...enter, key: "a" }), false);
});

test("step 04 says what the sign-in came to", () => {
  const p = complete(complete({ ...started, where: { choice: "droppen", lan: false } }, "tools"), "where");
  const linked = draw(<WrapStep progress={p} tools={undefined} online onChange={noop} signIn={signInAt({ kind: "linked", email: "ada@example.com", device: "studio", fingerprint: null })} />);
  assert.match(linked, /data-testid="wrap-summary"[\s\S]*Droppen · ada@example\.com/);
  assert.match(linked, /data-testid="wrap-phone"[\s\S]*Open droppen.app and sign in as ada@example\.com\./);
  const pending = draw(<WrapStep progress={p} tools={undefined} online onChange={noop} signIn={signInAt({ kind: "pending", flow: { kind: "browser", attempt: ATTEMPT } })} />);
  assert.match(pending, /Droppen · Waiting for droppen.app…/);
  assert.match(pending, /Want it on your phone too\?/);
  assert.match(draw(<WrapStep progress={p} tools={undefined} online onChange={noop} signIn={signInAt({ kind: "failed", failure: "expired", message: null })} />), /Droppen · not signed in/);
  // In the account even when "only on this Mac" was the choice: that is what stands.
  const local = complete(complete({ ...started, where: { choice: "local", lan: false } }, "tools"), "where");
  assert.match(draw(<WrapStep progress={local} tools={undefined} online onChange={noop} signIn={signInAt({ kind: "linked", email: "ada@example.com", device: "studio", fingerprint: null })} />), /Droppen · ada@example\.com/);
});

test("step 04: start at login points at the menu bar to turn it on or off, no notifications row, Finish — or the first session once step 03 saved one", () => {
  const p = complete(complete({ ...started, where: { choice: "local", lan: false } }, "tools"), "where");
  const html = draw(<WrapStep progress={p} tools={tools(true, false)} online onChange={noop} />);
  assert.match(html, /Start Droppen at login/);
  assert.match(html, /Turn it on or off any time from the Droppen icon in the menu bar: Start at Login\./);
  assert.doesNotMatch(html, /role="switch"/, "the menu bar switches it, not the guide");
  assert.doesNotMatch(html, /Notifications/);
  assert.match(html, /data-testid="wrap-phone"[\s\S]*Want it on your phone too\?/);
  assert.match(html, /data-testid="wrap-summary"[\s\S]*Claude Code[\s\S]*Max 20x[\s\S]*Codex[\s\S]*not installed[\s\S]*Only on this Mac/);

  assert.match(screen("wrap", p, tools(true, false)), /data-variant="primary"[^>]*>Finish</);
  const withFolder = { ...complete(p, "first"), first: { cwd: "/Users/demo/app", agent: "claude" as const, prompt: null } };
  const finishing = screen("wrap", withFolder, tools(true, false));
  assert.match(finishing, /data-variant="primary"[^>]*>Finish and open my first session</);
  assert.match(finishing, /\/Users\/demo\/app/);
  assert.match(screen("wrap", withFolder, tools(true, false), "zh-CN"), /完成，打开第一个会话/);
});

test("the bars above the session list", () => {
  const resume = draw(<SetupBarView bar={{ kind: "resume", left: 3 }} onOpen={noop} onDismiss={noop} />);
  assert.match(resume, /data-testid="setup-resume-bar"/);
  assert.match(resume, /Setup: 3 steps left/);
  assert.match(resume, />Continue</);
  assert.match(resume, /aria-label="Don&#x27;t remind me"/);
  assert.equal(primaries(resume), 0, "a bar never takes the list's primary");
  assert.match(draw(<SetupBarView bar={{ kind: "resume", left: 1 }} onOpen={noop} onDismiss={noop} />), /Setup: 1 step left/);
  assert.match(draw(<SetupBarView bar={{ kind: "resume", left: 3 }} onOpen={noop} onDismiss={noop} />, "zh-CN"), /设置还差 3 步[\s\S]*继续[\s\S]*不再提醒/);
  const noTool = draw(<SetupBarView bar={{ kind: "noTool" }} onOpen={noop} onDismiss={noop} />);
  assert.match(noTool, /data-testid="setup-no-tool-bar"/);
  assert.match(noTool, /No AI tool is ready on this device yet\.[\s\S]*Get one ready/);
  assert.doesNotMatch(noTool, /remind/);
  assert.equal(draw(<SetupBarView bar={null} onOpen={noop} onDismiss={noop} />), "");
});

test("Settings → General → Setup runs the guide again", () => {
  const html = draw(<SetupSectionView onRerun={noop} />);
  assert.match(html, /data-label=""[^>]*>Setup</);
  assert.match(html, /data-testid="setup-rerun"[^>]*>Run setup again</);
  assert.match(draw(<SetupSectionView onRerun={noop} />, "zh-CN"), /设置引导[\s\S]*重新运行/);
});
