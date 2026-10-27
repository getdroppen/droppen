#!/usr/bin/env node
/*
 * Screenshots of droppen.app (the cloud app) for the homepage's product tour.
 *
 * Renders the fixture in `web/tour/` — the real components in their real
 * composition, with made-up data and no hub — through the app's own
 * Vite/Tailwind setup, and captures each shot in English (the homepage shows
 * those in every language) and both themes with Playwright:
 * `<shot>-<lang>-<theme>.png`, no browser chrome, no
 * scrollbars, nothing moving.
 *
 *   node scripts/landing-shots/cloud/run.mjs
 *
 * Options:
 *   --out <dir>        where the PNGs go (default /tmp/droppen-tour/shots/cloud)
 *   --shot <name>      cloud-devices | phone-needs-you | security-pair (default: all)
 *   --lang <l>         en | zh-CN | both (default: en)
 *   --theme <t>        light | dark (default: both)
 *   --channel <name>   Playwright browser channel, e.g. chrome (default: bundled Chromium)
 */
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import fs from "node:fs/promises";
import path from "node:path";

const repo = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const web = path.join(repo, "web");
const requireFromWeb = createRequire(path.join(web, "package.json"));
const requireFromE2e = createRequire(path.join(repo, "e2e/package.json"));

const argv = process.argv.slice(2);
const flag = (name, fallback) => {
  const i = argv.indexOf(`--${name}`);
  return i >= 0 && argv[i + 1] && !argv[i + 1].startsWith("--") ? argv[i + 1] : fallback;
};
if (argv.includes("--help")) {
  console.log(await fs.readFile(fileURLToPath(import.meta.url), "utf8").then((s) => s.split("*/")[0] + "*/"));
  process.exit(0);
}

const desktop = { width: 1280, height: 800, deviceScaleFactor: 2, mobile: false };
const phone = { width: 390, height: 844, deviceScaleFactor: 3, mobile: true };

/**
 * Each shot: the fixture's query, the viewport, what to wait for, and whether
 * the page's own scroller is taken to its end (`scrollEnd`: on a phone the
 * ledger sits under the device list; the cloud bar stays, it is sticky).
 */
const SHOTS = {
  "cloud-devices": { query: "shot=devices", view: desktop, ready: '[data-testid="needs-you-ledger"]' },
  "phone-needs-you": { query: "shot=devices&as=phone", view: phone, ready: '[data-testid="needs-you-ledger"]', scrollEnd: true },
  "security-pair": { query: "shot=pair", view: phone, ready: '[data-testid="pairing-qr"]' },
};

const out = path.resolve(flag("out", "/tmp/droppen-tour/shots/cloud"));
const only = flag("shot", "all");
const shots = only === "all" ? Object.keys(SHOTS) : [only];
for (const name of shots) if (!SHOTS[name]) throw new Error(`unknown shot ${name}: one of ${Object.keys(SHOTS).join(", ")}`);
const langs = flag("lang", "en") === "both" ? ["zh-CN", "en"] : [flag("lang", "en")];
const themes = flag("theme", "both") === "both" ? ["light", "dark"] : [flag("theme")];
const channel = flag("channel", process.env.LANDING_SHOTS_CHANNEL ?? "");

await fs.mkdir(out, { recursive: true });

// The app's own Vite server; nothing is proxied anywhere (the fixture makes no request).
process.env.RH_DEV_PROXY ??= "http://127.0.0.1:9";
const { createServer } = await import(requireFromWeb.resolve("vite"));
const server = await createServer({
  configFile: path.join(web, "vite.config.ts"),
  root: web,
  server: { host: "127.0.0.1", port: 0 },
  logLevel: "warn",
});
await server.listen();
const base = server.resolvedUrls?.local?.[0];
if (!base) throw new Error("the Vite server did not report a local URL");

// The e2e workspace already pins Playwright; no second copy for a screenshot tool.
const { chromium } = requireFromE2e("@playwright/test");
const browser = await chromium.launch({ headless: true, ...(channel ? { channel } : {}) });

// Overlay scrollbars on every platform: none drawn, no gutter kept for one.
const NO_SCROLLBARS = "* { scrollbar-width: none !important; } *::-webkit-scrollbar { display: none !important; }";

const written = [];
try {
  for (const name of shots) {
    const { query, view, ready, scrollEnd } = SHOTS[name];
    for (const lang of langs) {
      for (const theme of themes) {
        const context = await browser.newContext({
          viewport: { width: view.width, height: view.height },
          deviceScaleFactor: view.deviceScaleFactor,
          isMobile: view.mobile,
          hasTouch: view.mobile,
          colorScheme: theme,
          // The live dots hold still, lit (`LiveDot`): the same frame every run.
          reducedMotion: "reduce",
          locale: lang,
        });
        const page = await context.newPage();
        const errors = [];
        page.on("pageerror", (err) => errors.push(err.message));
        await page.goto(`${base}tour/index.html?${query}&lang=${lang}&theme=${theme}`);
        await page.addStyleTag({ content: NO_SCROLLBARS });
        await page.waitForSelector(ready);
        await page.evaluate(() => document.fonts.ready);
        if (scrollEnd) {
          await page.evaluate(() => {
            // `CloudPage`'s scroller, the one under the sticky bar.
            const scroller = document.querySelector("header")?.closest(".overflow-y-auto");
            if (!scroller) throw new Error("no page scroller");
            scroller.scrollTop = scroller.scrollHeight;
          });
        }
        await page.waitForTimeout(300);
        if (errors.length) throw new Error(`${name} ${lang} ${theme}: ${errors.join("; ")}`);
        const file = path.join(out, `${name}-${lang}-${theme}.png`);
        await page.screenshot({ path: file, animations: "disabled", caret: "hide" });
        written.push(file);
        await context.close();
      }
    }
  }
} finally {
  await browser.close();
  await server.close();
}
for (const file of written) console.log(file);
