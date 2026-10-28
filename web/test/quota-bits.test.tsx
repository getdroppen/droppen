// What the quota readouts draw once an account's numbers come mainly from its
// own sessions and only fall back to the /usage probe when they go quiet.
//
// The rules are pinned in `account-quota.test.ts`; what is pinned here is the
// markup: the note says where a reading came from, a window older than its
// siblings is dimmed with its age on hover, and a broken fallback probe behind
// a fresh reading neither hides the numbers nor raises the amber line.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { I18nextProvider } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { createMemoryHistory, createRootRoute, createRouter, RouterContextProvider } from "@tanstack/react-router";
import { QuotaBlock, QuotaReadingNote, QuotaWindowRow } from "../src/features/accounts/quota-bits.js";
import type { QuotaWindow } from "../src/features/accounts/use-accounts.js";
import { makeI18n } from "./render.js";

// English keys are the source strings, so an empty resource bundle renders the
// same text the untranslated UI does — and asserting on it asserts on the key.
const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} } }
});

// The usage lines link to Settings, and a router `Link` needs a router to build
// its href from; a bare root route in memory is enough, and nothing navigates.
const router = createRouter({ routeTree: createRootRoute(), history: createMemoryHistory() });

function draw(node: React.ReactElement): string {
  return renderToStaticMarkup(
    <RouterContextProvider router={router}>
      <I18nextProvider i18n={i18n}>{node}</I18nextProvider>
    </RouterContextProvider>,
  );
}

/** An ISO time this many minutes before now: the components date against the real clock. */
function ago(minutes: number): string {
  return new Date(Date.now() - minutes * 60_000).toISOString();
}

function win(over: Partial<QuotaWindow> = {}): QuotaWindow {
  return { label: "5h", usedPercent: 40, remainingPercent: 60, windowMinutes: 300, resetsAt: null, ...over };
}

test("the reading note says whether a Claude reading came from a session or the /usage probe", () => {
  assert.match(draw(<QuotaReadingNote updatedAt={ago(1)} source="session" />), /from sessions/);
  const probe = draw(<QuotaReadingNote updatedAt={ago(1)} source="probe" />);
  assert.match(probe, /from \/usage/);
  assert.doesNotMatch(probe, /from sessions/);
  // No source (an older server) says neither, and Codex's own labels are untouched.
  const bare = draw(<QuotaReadingNote updatedAt={ago(1)} />);
  assert.doesNotMatch(bare, /from sessions|from \/usage/);
  assert.match(draw(<QuotaReadingNote updatedAt={ago(1)} source="session_log" />), /from session logs/);
  assert.doesNotMatch(draw(<QuotaReadingNote updatedAt={ago(1)} source="app_server" />), /from /);
});

test("a window with its own stale time is dimmed and says how old it is", () => {
  const stale = draw(<QuotaWindowRow w={win({ label: "weekly", updatedAt: ago(120) })} />);
  assert.match(stale, /^<div class="opacity-50" title="read 2h ago">/);
  assert.match(stale, /60% left/, "the number is still shown, only dimmed");
});

test("a fresh or undated window is drawn as before", () => {
  for (const w of [win({ updatedAt: ago(1) }), win()]) {
    const html = draw(<QuotaWindowRow w={w} />);
    assert.doesNotMatch(html, /opacity-50/);
    assert.doesNotMatch(html, /^<div[^>]*title=/);
  }
});

test("a failed probe behind a fresh session reading shows the windows without the unavailable line", () => {
  const fresh = draw(
    <QuotaBlock title="work" windows={[win()]} current={false} usage="unavailable" updatedAt={ago(1)} source="session" />,
  );
  assert.doesNotMatch(fresh, /usage unavailable/);
  assert.match(fresh, /60% left/);
  assert.match(fresh, /from sessions/);

  const stale = draw(
    <QuotaBlock title="work" windows={[win()]} current={false} usage="unavailable" updatedAt={ago(120)} source="probe" />,
  );
  assert.match(stale, /usage unavailable/);
  assert.match(stale, /60% left/, "old numbers are still drawn, dated");
});

test("an expired /usage login behind a fresh reading reads faint, as what it costs", () => {
  const fresh = draw(
    <QuotaBlock title="work" windows={[win()]} current={false} usage="login_expired" updatedAt={ago(1)} source="session" />,
  );
  assert.match(fresh, /<div class="font-mono text-\[10px\] text-faint">usage login expired · only affects the idle-time fallback · <a href="\/settings\?section=accounts&amp;tab=claude"/);
  assert.doesNotMatch(fresh, /text-warn/);
  assert.match(fresh, /60% left/);
});

test("an expired /usage login behind an aged reading keeps today's amber line", () => {
  const stale = draw(
    <QuotaBlock title="work" windows={[win()]} current={false} usage="login_expired" updatedAt={ago(120)} source="probe" />,
  );
  assert.match(stale, /<div class="font-mono text-\[10px\] text-warn-fg">usage login expired · <a href="\/settings\?section=accounts&amp;tab=claude"/);
  assert.doesNotMatch(stale, /idle-time fallback/);
  assert.match(stale, /60% left/, "the last good numbers are still drawn, dated");
});

test("a never-enabled /usage login with session windows shows them, with a faint fallback note", () => {
  const html = draw(
    <QuotaBlock title="work" windows={[win()]} current={false} usage="login_needed" updatedAt={ago(1)} source="session" />,
  );
  assert.match(html, /<div class="font-mono text-\[10px\] text-faint">usage not enabled · only affects the idle-time fallback · <a href="\/settings\?section=accounts&amp;tab=claude"/);
  assert.match(html, /60% left/);
  assert.match(html, /from sessions/);
});

test("an env account with session windows shows them and no usage line", () => {
  const html = draw(
    <QuotaBlock title="relay" windows={[win()]} current={false} usage="not_applicable" updatedAt={ago(1)} source="session" />,
  );
  assert.match(html, /60% left/);
  assert.doesNotMatch(html, /usage (not enabled|unavailable|login expired)|no window data/);
});

test("with no windows, every usage state reads as it did before", () => {
  const block = (usage: "login_needed" | "not_applicable" | "login_expired" | "ready") =>
    draw(<QuotaBlock title="w" windows={[]} current={false} usage={usage} updatedAt={null} />);
  assert.match(block("login_needed"), /<div class="font-mono text-\[10px\] text-faint">usage not enabled · <a/);
  assert.doesNotMatch(block("login_needed"), /idle-time/);
  assert.match(block("login_expired"), /text-warn-fg">usage login expired · <a/);
  assert.match(block("ready"), /no window data/);
  assert.doesNotMatch(block("not_applicable"), /usage|no window data|left/);
});
