// Photograph the staged demo with Playwright: every shot, light and dark, in
// one language. Files are <shot>-<lang>-<theme>.png in --out.
//
//   node capture.mjs --lang en|zh-CN --url http://127.0.0.1:8787 --state /opt/demo/state --out /work/out [--tz <zone>] [--only <shot,…>]
//
// The theme and the language are the app's own preferences (`rh.theme` and
// `rh.lang` in localStorage; stage.mjs set the server's language), so the
// pages are exactly what a user who chose them sees.

import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { CODE, IDS, newSessionDraft } from "./scenario.mjs";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const { chromium } = createRequire(path.join(repo, "e2e/package.json"))("@playwright/test");

const { values: opts } = parseArgs({
  options: {
    lang: { type: "string" },
    url: { type: "string" },
    state: { type: "string" },
    out: { type: "string" },
    tz: { type: "string", default: "UTC" },
    only: { type: "string" },
  },
});
const { lang, url, out, tz } = opts;
const staged = JSON.parse(fs.readFileSync(path.join(opts.state, "staged.json"), "utf8"));
const only = opts.only ? new Set(opts.only.split(",")) : null;
fs.mkdirSync(out, { recursive: true });

// The homepage frames the desktop shots about 1280 px wide: at 1280 the app's
// text is at its real size there.
const DESKTOP = { viewport: { width: 1280, height: 800 }, deviceScaleFactor: 2 };
const PHONE = {
  viewport: { width: 390, height: 844 },
  deviceScaleFactor: 3,
  isMobile: true,
  hasTouch: true,
  userAgent: "Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Mobile Safari/537.36",
};

// No scrollbars and no text caret in a picture.
const CLEAN = `
  *, *::before, *::after { scrollbar-width: none !important; caret-color: transparent !important; }
  ::-webkit-scrollbar { display: none !important; width: 0 !important; height: 0 !important; }
  .xterm-cursor-layer, .xterm .xterm-cursor { visibility: hidden !important; }
`;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Unfold a transcript's folded tool calls the way a reader clicks them open:
 * the last run of steps in `scope` (or every run with `all`), and in it the
 * edits' diffs (`edits: "last"` for only the newest, `"none"` for none).
 */
function unfold(page, scope, { all = false, edits = "all" } = {}) {
  return page.evaluate(
    ([scope, all, edits]) => {
      const root = document.querySelector(scope) ?? document;
      const groups = [...root.querySelectorAll("details.group\\/steps")];
      for (const group of all ? groups : groups.slice(-1)) {
        group.open = true;
        const diffs = [...group.querySelectorAll("details")].filter((d) => d.querySelector("[data-diff]"));
        for (const d of edits === "last" ? diffs.slice(-1) : edits === "none" ? [] : diffs) d.open = true;
      }
    },
    [scope, all, edits],
  );
}

/** Scroll so `selector` sits at the bottom of the transcript, as it does when it has just arrived. */
const toBottom = (page, selector) => page.locator(selector).first().evaluate((el) => el.scrollIntoView({ block: "end" }));

/** Type into the focused terminal and give the command time to print. */
async function run(page, command, wait) {
  await page.keyboard.type(command, { delay: 12 });
  await page.keyboard.press("Enter");
  await sleep(wait);
}

const SHOTS = [
  {
    name: "app-session",
    device: DESKTOP,
    path: `/sessions/claude/${IDS.ask}`,
    ready: async (page) => {
      await page.locator('[data-question="pending"]').first().waitFor();
      await unfold(page, "body", { edits: "last" });
      await sleep(300);
      await toBottom(page, '[data-question="pending"]');
    },
  },
  {
    name: "app-split",
    device: DESKTOP,
    path: `/screens/${staged.screen}`,
    ready: async (page, first) => {
      await page.locator('[data-pane-id="pane-term"] .xterm').first().waitFor();
      await page.locator('[data-pane-id="pane-claude"] details').first().waitFor({ state: "attached" });
      await unfold(page, '[data-pane-id="pane-claude"]', { all: true });
      await unfold(page, '[data-pane-id="pane-codex"]', { all: true, edits: "none" });
      if (first) {
        await page.locator('[data-pane-id="pane-term"] .xterm').first().click();
        await run(page, "npm run dev", 4500);
      }
    },
  },
  {
    name: "app-terminal",
    device: DESKTOP,
    path: `/terminals?id=${staged.terminal}`,
    ready: async (page, first) => {
      await page.locator(".xterm").first().waitFor();
      if (first) {
        await page.locator(".xterm").first().click();
        await run(page, "git log --oneline --graph --decorate -8", 1200);
        await run(page, "npm test", 4000);
      }
    },
  },
  {
    name: "app-new-session",
    device: DESKTOP,
    path: "/sessions/new",
    ready: async (page) => {
      const box = page.locator("textarea").first();
      await box.waitFor();
      await box.fill(newSessionDraft(lang));
    },
  },
  {
    name: "app-files",
    device: DESKTOP,
    path: `/files?path=${encodeURIComponent(`${CODE}/shop-api`)}&file=${encodeURIComponent(`${CODE}/shop-api/README.md`)}`,
    ready: (page) => page.getByText("shop-api").first().waitFor(),
  },
  {
    name: "phone-session",
    device: PHONE,
    path: `/sessions/claude/${IDS.ask}`,
    ready: (page) => page.locator('[data-question="pending"]').first().waitFor(),
  },
  {
    name: "phone-list",
    device: PHONE,
    path: "/sessions",
    ready: (page) => page.getByText(/rate limiting|限流/).first().waitFor(),
  },
];

// Without WebGL the terminal draws with xterm's DOM renderer, the app's own
// fallback: headless Chromium's software WebGL comes out blank at 2x.
const browser = await chromium.launch({ args: ["--disable-3d-apis"] });
const typed = new Set();
try {
  for (const theme of ["light", "dark"]) {
    for (const shot of SHOTS) {
      if (only && !only.has(shot.name)) continue;
      const context = await browser.newContext({
        ...shot.device,
        colorScheme: theme,
        locale: lang === "zh-CN" ? "zh-CN" : "en-US",
        timezoneId: tz,
        reducedMotion: "reduce",
      });
      await context.addInitScript(
        ([theme, lang]) => {
          localStorage.setItem("rh.theme", theme);
          localStorage.setItem("rh.lang", lang);
          // Somebody who said "not now" to signing in to Droppen long ago.
          localStorage.setItem("rh.first-run.droppen", "1");
        },
        [theme, lang],
      );
      const page = await context.newPage();
      await page.goto(url + shot.path, { waitUntil: "networkidle" });
      await page.addStyleTag({ content: CLEAN });
      const first = !typed.has(shot.name);
      await shot.ready(page, first);
      typed.add(shot.name);
      await page.addStyleTag({ content: CLEAN });
      // Off everything hoverable, and nothing focused but what the shot wants.
      await page.mouse.move(shot.device.viewport.width - 2, shot.device.viewport.height - 2);
      await sleep(800);
      const file = path.join(out, `${shot.name}-${lang}-${theme}.png`);
      await page.screenshot({ path: file, animations: "disabled" });
      console.log(`captured ${path.basename(file)}`);
      await context.close();
    }
  }
} finally {
  await browser.close();
}
