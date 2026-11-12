#!/usr/bin/env node
/*
 * Droppen UI audit — the executable half of `.claude/skills/ui-consistency`.
 *
 * Renders the fixture in `web/ui-audit/` with the real Vite/Tailwind setup and
 * the real production components, then measures computed styles and captures
 * screenshots with Playwright. It is a reporting tool by default; `--check`
 * turns the target invariants in `docs/ui-spec.md` into a pass/fail gate for
 * UI changes.
 *
 *   pnpm ui-audit                     # report + screenshots
 *   pnpm ui-audit --check             # fail on a spec violation
 *   pnpm ui-audit --url http://127.0.0.1:8700/settings
 *                                     # measure a running page (no fixture)
 *
 * Options:
 *   --out <dir>        where measurements/screenshots go (default /tmp/droppen-ui-audit-<date>)
 *   --theme <t>        light | dark | both (default both)
 *   --viewport <v>     desktop | mobile | both (default both)
 *   --channel <name>   Playwright browser channel, e.g. chrome (default: bundled Chromium)
 *   --fixture <f>      settings | devices | agents | new-chat | cloud-bar | session-opening | no-copy | session-from-copy | welcome | desktop-app | both (default both: every fixture)
 *   --url <url>        skip the fixture and audit a running page
 *   --check            exit 1 if any target invariant fails
 */
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import fs from "node:fs/promises";
import path from "node:path";

const repo = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const web = path.join(repo, "web");
const requireFromWeb = createRequire(path.join(web, "package.json"));
const requireFromE2e = createRequire(path.join(repo, "e2e/package.json"));

const argv = process.argv.slice(2);
const flag = (name, fallback) => {
  const i = argv.indexOf(`--${name}`);
  return i >= 0 && argv[i + 1] && !argv[i + 1].startsWith("--") ? argv[i + 1] : fallback;
};
const has = (name) => argv.includes(`--${name}`);
if (has("help")) {
  console.log(await fs.readFile(fileURLToPath(import.meta.url), "utf8").then((s) => s.split("*/")[0] + "*/"));
  process.exit(0);
}

const out = path.resolve(flag("out", `/tmp/droppen-ui-audit-${new Date().toISOString().slice(0, 10).replaceAll("-", "")}`));
const themes = flag("theme", "both") === "both" ? ["light", "dark"] : [flag("theme", "light")];
const VIEWPORTS = {
  desktop: { name: "desktop", width: 1440, height: 1000 },
  mobile: { name: "mobile", width: 390, height: 844 },
};
const viewportArg = flag("viewport", "both");
const views =
  viewportArg === "both"
    ? [VIEWPORTS.desktop, VIEWPORTS.mobile]
    : Object.hasOwn(VIEWPORTS, viewportArg)
      ? [VIEWPORTS[viewportArg]]
      : [];
if (!views.length) {
  console.error(`ui-audit: unknown --viewport "${viewportArg}" (desktop, mobile, or both)`);
  process.exit(2);
}
const channel = flag("channel", process.env.UI_AUDIT_CHANNEL ?? "");
const liveUrl = flag("url", "");
const check = has("check");
const which = flag("fixture", "both");
const runSettings = which === "both" || which === "settings";
const runDevices = which === "both" || which === "devices";
const runAgents = which === "both" || which === "agents";
const runNewChat = which === "both" || which === "new-chat";
const runCloudBar = which === "both" || which === "cloud-bar";
const runSessionOpening = which === "both" || which === "session-opening";
const runNoCopy = which === "both" || which === "no-copy";
const runSessionFromCopy = which === "both" || which === "session-from-copy";
const runWelcome = which === "both" || which === "welcome";
const runDesktopApp = which === "both" || which === "desktop-app";

await fs.mkdir(out, { recursive: true });

/** Start the app's own Vite server (fixture mode only). */
let server = null;
let base = liveUrl;
if (!liveUrl) {
  process.env.DROPPEN_DEV_PROXY ??= "http://127.0.0.1:9";
  const { createServer } = await import(requireFromWeb.resolve("vite"));
  server = await createServer({
    configFile: path.join(web, "vite.config.ts"),
    root: web,
    server: { host: "127.0.0.1", port: 0 },
  });
  await server.listen();
  const urls = server.resolvedUrls?.local ?? [];
  base = urls[0];
  if (!base) throw new Error("the Vite server did not report a local URL");
  console.log(`ui-audit: fixture at ${base}ui-audit/index.html`);
}

// The e2e workspace already pins Playwright; no second copy for an audit tool.
const { chromium } = requireFromE2e("@playwright/test");
const browser = await chromium.launch({ headless: true, ...(channel ? { channel } : {}) });

const results = [];
const failures = [];

/** Everything measured on one page load. */
async function measure(page) {
  return page.evaluate(() => {
    const rect = (e) => {
      if (!e) return null;
      const r = e.getBoundingClientRect();
      return { x: +r.x.toFixed(1), y: +r.y.toFixed(1), w: +r.width.toFixed(1), h: +r.height.toFixed(1) };
    };
    const style = (e) => {
      if (!e) return null;
      const s = getComputedStyle(e);
      return {
        font: s.fontFamily,
        size: s.fontSize,
        weight: s.fontWeight,
        line: s.lineHeight,
        color: s.color,
        bg: s.backgroundColor,
        border: s.borderColor,
        borderW: s.borderWidth,
        radius: s.borderRadius,
        padding: s.padding,
        outline: `${s.outlineWidth} ${s.outlineStyle} ${s.outlineColor}`,
        outlineColor: s.outlineColor,
        outlineOffset: s.outlineOffset,
        shadow: s.boxShadow,
        transition: s.transition,
        transform: s.transform,
        opacity: s.opacity,
      };
    };
    const root = document.querySelector("[data-audit-row]")?.closest("body") ?? document.body;
    const rows = [...document.querySelectorAll("[data-audit-row]")].map((r) => ({
      id: r.dataset.auditRow,
      rect: rect(r),
      label: style(r.querySelector("[data-audit-label] > div")),
      hint: style(r.querySelector("[data-audit-label] > div:nth-child(2)")),
      control: rect(r.querySelector("[data-audit-control]")),
    }));
    const trigger = document.querySelector('[data-audit-row="model"] button');
    const effort = document.querySelector('[data-audit-row="effort"] button');
    const empty = document.querySelector('[data-audit-row="empty"] button');
    const input = document.querySelector('[data-audit-row="name"] input');
    const activeTab = document.querySelector('[data-audit-row="grouping"] [data-active]');
    const visible = (e) => e && e.getBoundingClientRect().width > 0;
    const borders = [...root.querySelectorAll("button,input,[role=tab],[role=listbox],[role=option]")]
      .filter(visible)
      .map((e) => ({ tag: e.tagName, radius: getComputedStyle(e).borderRadius, shadow: getComputedStyle(e).boxShadow }))
      .filter((x) => parseFloat(x.radius) > 0 || (x.shadow !== "none" && !x.shadow.includes("inset")));
    return {
      rows,
      header: rect(document.querySelector("header")),
      sectionTitle: rect(document.querySelector("h2")),
      trigger: { rect: rect(trigger), style: style(trigger) },
      effort: { style: style(effort) },
      input: { rect: rect(input), style: style(input) },
      activeTab: { rect: rect(activeTab), style: style(activeTab), focus: activeTab?.matches(":focus-visible") ?? null },
      switchTrack: rect(document.querySelector("[data-audit-switch-track]")),
      switchButton: rect(document.querySelector("[data-audit-switch]")),
      checkboxBox: rect(document.querySelector("[data-audit-checkbox]")),
      checkboxBoxStyle: style(document.querySelector("[data-audit-checkbox]")),
      checkboxCheck: (() => {
        const svg = document.querySelector("[data-audit-checkbox] svg");
        return svg ? { opacity: getComputedStyle(svg).opacity, color: getComputedStyle(svg).color } : null;
      })(),
      pageBg: getComputedStyle(document.body).backgroundColor,
      triggerAttrs: trigger ? Object.fromEntries([...trigger.attributes].map((a) => [a.name, a.value])) : null,
      chevronClass: trigger?.querySelector("svg")?.getAttribute("class") ?? null,
      invalidBorders: borders,
    };
  });
}

async function auditSettings(theme, view) {
  const page = await browser.newPage({
    viewport: { width: view.width, height: view.height },
    deviceScaleFactor: 1,
    // A phone is a coarse pointer, not just a narrow window: the spec's
    // touch sizes key off `pointer: coarse`, which a 390px desktop context
    // does not match.
    hasTouch: view.name === "mobile",
    isMobile: view.name === "mobile",
  });
  await page.goto(`${base}ui-audit/index.html?fixture=settings&theme=${theme}`);
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(200);

  // Keyboard focus, as a keyboard user gets it.
  await page.locator('[data-audit-row="name"] input').focus();
  const inputFocus = await page.evaluate(() => {
    const e = document.querySelector('[data-audit-row="name"] input');
    const s = getComputedStyle(e);
    return { focusVisible: e.matches(":focus-visible"), width: s.outlineWidth, style: s.outlineStyle, color: s.outlineColor, offset: s.outlineOffset };
  });
  await page.locator('[data-audit-row="grouping"] [data-active]').focus();
  const tabFocus = await page.evaluate(() => {
    const e = document.querySelector('[data-audit-row="grouping"] [data-active]');
    const s = getComputedStyle(e);
    return { focusVisible: e.matches(":focus-visible"), width: s.outlineWidth, style: s.outlineStyle, color: s.outlineColor, offset: s.outlineOffset, bg: s.backgroundColor };
  });
  await page.locator('[data-audit-row="checkbox"] [data-audit-checkbox]').focus();
  await page.waitForTimeout(200);
  const checkboxFocus = await page.evaluate(() => {
    const box = document.querySelector('[data-audit-row="checkbox"] [data-audit-checkbox]');
    const s = box ? getComputedStyle(box) : null;
    return {
      focusVisible: box?.matches(":focus-visible") ?? null,
      width: s?.outlineWidth ?? null,
      style: s?.outlineStyle ?? null,
      color: s?.outlineColor ?? null,
      offset: s?.outlineOffset ?? null,
    };
  });

  const before = await measure(page);

  const trigger = page.locator('[data-audit-row="model"] button');
  const baseBorder = await trigger.evaluate((e) => getComputedStyle(e).borderColor);
  await trigger.hover();
  await page.waitForTimeout(200);
  const hoverBorder = await trigger.evaluate((e) => getComputedStyle(e).borderColor);
  await trigger.click();
  await page.waitForTimeout(250);
  // Move the highlight off the selected row: a selected row must stay
  // distinguishable from an unhighlighted normal one, not only while focused.
  await page.keyboard.press("ArrowDown");
  await page.waitForTimeout(150);
  const open = await page.evaluate(() => {
    const style = (e) => {
      const s = getComputedStyle(e);
      return { font: s.fontFamily, size: s.fontSize, color: s.color, bg: s.backgroundColor, padding: s.padding, radius: s.borderRadius, transform: s.transform, borderLeft: `${s.borderLeftWidth} ${s.borderLeftColor}` };
    };
    const rect = (e) => {
      const r = e.getBoundingClientRect();
      return { x: +r.x.toFixed(1), y: +r.y.toFixed(1), w: +r.width.toFixed(1), h: +r.height.toFixed(1) };
    };
    const lists = [...document.querySelectorAll('[role="listbox"]')];
    const list = lists.at(-1);
    const options = [...(list?.querySelectorAll('[role="option"]') ?? [])];
    const selected = options.find((o) => o.hasAttribute("data-selected"));
    const normal = options.find((o) => !o.hasAttribute("data-selected") && !o.hasAttribute("data-highlighted"));
    const chevron = document.querySelector('[data-audit-row="model"] button svg');
    // The rotation lives on the icon's wrapper (`[&>svg]:size-…`), not on the
    // Phosphor `<svg>` itself.
    const chevronIcon = chevron?.parentElement;
    const trigger = document.querySelector('[data-audit-row="model"] button');
    return {
      list: { rect: rect(list), style: style(list), text: (list?.innerText ?? "").trim() },
      options: options.map((o) => ({ text: o.textContent?.trim(), rect: rect(o), style: style(o) })),
      selected: selected ? { style: style(selected), check: !!selected.querySelector("svg") } : null,
      normal: normal ? { style: style(normal) } : null,
      // Tailwind v4 turns `rotate-180` into the individual `rotate` property,
      // so `transform` stays "none" on a rotated element.
      chevron: chevron && chevronIcon ? { transform: getComputedStyle(chevronIcon).transform, rotate: getComputedStyle(chevronIcon).rotate, class: chevronIcon.getAttribute("class") } : null,
      triggerAttrs: trigger ? Object.fromEntries([...trigger.attributes].map((a) => [a.name, a.value])) : null,
    };
  });
  await page.screenshot({ path: `${out}/select-open-${theme}-${view.name}.png` });
  await page.keyboard.press("Escape");
  await page.waitForTimeout(350);
  await page.waitForFunction(() => !document.querySelector('[role="listbox"]'), undefined, { timeout: 2000 }).catch(() => undefined);

  // The empty state: open the select with no options.
  await page.locator('[data-audit-row="empty"] button').click();
  await page.waitForTimeout(250);
  const empty = await page.evaluate(() => {
    const lists = [...document.querySelectorAll('[role="listbox"]')];
    const list = lists.at(-1);
    return { text: (list?.innerText ?? "").trim(), html: (list?.innerHTML ?? "").trim(), open: lists.length > 0 };
  });
  await page.screenshot({ path: `${out}/select-empty-${theme}-${view.name}.png` });
  await page.keyboard.press("Escape");

  await page.screenshot({ path: `${out}/settings-${theme}-${view.name}.png` });

  const record = { mode: "fixture", theme, viewport: view.name, inputFocus, tabFocus, checkboxFocus, before, select: { baseBorder, hoverBorder, open }, empty };
  results.push(record);

  // ---- target invariants (docs/ui-spec.md) ----
  const fail = (id, actual, expected) => failures.push({ theme, viewport: view.name, id, actual, expected });
  const desktop = view.name === "desktop";
  const fieldH = desktop ? 32 : 40;
  const rowH = desktop ? 32 : 44;
  const focusVisible = (f) => f.style !== "none" && parseFloat(f.width) >= 2 && f.color !== "rgba(0, 0, 0, 0)";
  if (!focusVisible(inputFocus)) fail("focus.input", inputFocus, "2px visible outline");
  // A ring outside the element sits on the page, not on the element's own
  // ink fill: compare it against the page background when it is offset out.
  const tabRingVisible =
    tabFocus.style !== "none" &&
    parseFloat(tabFocus.width) >= 2 &&
    (parseFloat(tabFocus.offset) >= 0 ? tabFocus.color !== before.pageBg : tabFocus.color !== tabFocus.bg);
  if (!tabRingVisible) fail("focus.tab", tabFocus, "visible ring outside the ink fill");
  const switchTrack = before.switchTrack;
  if (!switchTrack || Math.round(switchTrack.w) !== 24 || Math.round(switchTrack.h) !== 14) {
    fail("choice.switch-track", switchTrack, "24 x 14 px");
  }
  if (!desktop && (!before.switchButton || before.switchButton.h < 36)) fail("choice.switch-touch", before.switchButton?.h, ">= 36px on a coarse pointer");
  const wantBox = desktop ? 14 : 16;
  if (!before.checkboxBox || before.checkboxBox.w < wantBox - 1 || before.checkboxBox.h < wantBox - 1) {
    fail("choice.checkbox", before.checkboxBox, `${wantBox} x ${wantBox} px`);
  }
  if (!before.checkboxBoxStyle || before.checkboxBoxStyle.bg === "rgba(0, 0, 0, 0)" || before.checkboxCheck?.opacity !== "1") {
    fail("choice.checkbox-checked", { bg: before.checkboxBoxStyle?.bg, check: before.checkboxCheck }, "an ink box with a visible check when chosen");
  }
  if (checkboxFocus.focusVisible && (checkboxFocus.style === "none" || parseFloat(checkboxFocus.width) < 2 || checkboxFocus.color === before.pageBg)) {
    fail("choice.checkbox-focus", checkboxFocus, "a visible 2px ring on the box");
  }
  if (before.trigger.rect.h < fieldH) fail("size.trigger", before.trigger.rect.h, `>= ${fieldH}px`);
  if (before.input.rect.h < fieldH) fail("size.input", before.input.rect.h, `>= ${fieldH}px`);
  if (parseFloat(before.effort.style.size) < 13) fail("type.value", before.effort.style.size, ">= 13px");
  if (/mono|Menlo|Consolas|SFMono/i.test(before.effort.style.font)) fail("type.value-family", before.effort.style.font, "sans for a plain value");
  if (desktop) {
    const widths = before.rows.filter((r) => r.control).map((r) => r.control.w);
    const spread = Math.max(...widths) - Math.min(...widths);
    if (spread > 1) fail("layout.control-widths", widths, "one control column, spread <= 1px");
  }
  // A coarse pointer has no hover: the phone's state is open/pressed instead.
  if (desktop && baseBorder === hoverBorder) fail("select.hover", hoverBorder, "a border change on hover");
  const chevronTurned = open.chevron && (open.chevron.rotate === "180deg" || /matrix\(-1/.test(open.chevron.transform));
  if (!chevronTurned) fail("select.chevron", open.chevron, "rotated ~180deg while open");
  if (!open.options.length) fail("select.options", 0, "at least one option");
  const itemH = open.options[0]?.rect.h ?? 0;
  if (itemH < rowH) fail("select.row-height", itemH, `>= ${rowH}px`);
  const selected = open.selected?.style;
  const normal = open.normal?.style;
  if (!selected || !normal) fail("select.selected", null, "a selected and a normal option");
  else if (selected.bg === normal.bg && selected.weight === normal.weight && selected.color === normal.color && selected.borderLeft === normal.borderLeft) {
    fail("select.selected", { bg: selected.bg, weight: selected.weight }, "a selected row that stays distinct from an unhighlighted normal one");
  }
  if (!open.selected?.check) fail("select.check", null, "a check on the selected row");
  if (!empty.open || !empty.text) fail("select.empty", empty, "an open list with a sentence when there are no options");
  if (before.invalidBorders.length) fail("visual.radius-shadow", before.invalidBorders, "no radius, no drop shadow");
  await page.close();
}

/**
 * The element the page scrolls, and its size. The app keeps `body` fixed and
 * scrolls a container inside it; a plain page scrolls the window instead.
 * `fullPage` cannot see that container — it leaves everything below the first
 * viewport blank — so the live screenshots walk it in viewport-sized steps.
 */
async function liveScroller(page) {
  const handle = await page.evaluateHandle(() => {
    const scrollers = [...document.querySelectorAll("*")].filter((e) => {
      const style = getComputedStyle(e);
      return /(auto|scroll)/.test(style.overflowY) && e.scrollHeight > e.clientHeight + 1;
    });
    // The outermost scroller is the page's; a nested one is a panel inside it.
    return scrollers.find((e) => !scrollers.some((other) => other !== e && other.contains(e))) ?? null;
  });
  const element = handle.asElement();
  const size = element
    ? await element.evaluate((e) => ({ height: e.scrollHeight, viewport: e.clientHeight }))
    : await page.evaluate(() => ({ height: document.body.scrollHeight, viewport: window.innerHeight }));
  // Sticky bars (the header, and the security page's contents bar) cover the
  // top of every scrolled viewport. Step by less than a viewport so what they
  // hide was already visible at the previous slice's bottom.
  const overlap = await page.evaluate(() => {
    const sticky = [...document.querySelectorAll("*")].filter((e) => {
      if (e === document.body || e === document.documentElement) return false;
      const style = getComputedStyle(e);
      const rect = e.getBoundingClientRect();
      const top = parseFloat(style.top);
      // The header sticks at 0, a second bar at 56: both are at their own top.
      const stuck = rect.top <= 1 || (Number.isFinite(top) && Math.abs(rect.top - top) <= 1);
      return (
        (style.position === "sticky" || style.position === "fixed") &&
        stuck &&
        rect.height > 0 &&
        // Not the fixed body or a full-height rail: a top bar is short and wide.
        rect.height < window.innerHeight &&
        rect.width >= window.innerWidth / 2
      );
    });
    // Stacked bars (the header and the security page's contents bar) add up;
    // overlapping ones count once. Summing heights would double a nested bar.
    const spans = sticky
      .map((e) => {
        const rect = e.getBoundingClientRect();
        return [Math.max(0, rect.top), Math.min(window.innerHeight, rect.bottom)];
      })
      .filter(([top, bottom]) => bottom > top)
      .sort((a, b) => a[0] - b[0]);
    let total = 0;
    let end = 0;
    for (const [top, bottom] of spans) {
      const from = Math.max(top, end);
      if (bottom > from) total += bottom - from;
      end = Math.max(end, bottom);
    }
    return total;
  });
  return { handle, element, ...size, overlap };
}

/** Scroll the page's own scroller (or the window) to `top` and let it paint. */
async function scrollLive(page, scroller, top) {
  if (scroller) await scroller.evaluate((e, top) => (e.scrollTop = top), top);
  else await page.evaluate((top) => window.scrollTo(0, top), top);
  await page.waitForTimeout(100);
}

async function auditLive(view) {
  const page = await browser.newPage({ viewport: { width: view.width, height: view.height }, deviceScaleFactor: 1 });
  await page.goto(liveUrl, { waitUntil: "networkidle" });
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(300);
  const { handle, element, height, viewport, overlap } = await liveScroller(page);
  // Measure and capture the same initial state, even when the URL has a hash.
  await scrollLive(page, element, 0);
  const snapshot = await page.evaluate(() => {
    const rect = (e) => {
      const r = e.getBoundingClientRect();
      return { x: +r.x.toFixed(1), y: +r.y.toFixed(1), w: +r.width.toFixed(1), h: +r.height.toFixed(1) };
    };
    const style = (e) => {
      const s = getComputedStyle(e);
      return { size: s.fontSize, weight: s.fontWeight, color: s.color, bg: s.backgroundColor, border: s.borderColor, radius: s.borderRadius, padding: s.padding, shadow: s.boxShadow, outline: `${s.outlineWidth} ${s.outlineStyle} ${s.outlineColor}` };
    };
    return [...document.querySelectorAll("button,input,select,textarea,[role=tab],[role=combobox],h1,h2")].filter((e) => e.getBoundingClientRect().width > 0).map((e) => ({
      tag: e.tagName,
      role: e.getAttribute("role"),
      text: (e.innerText || e.getAttribute("placeholder") || "").trim().replace(/\s+/g, " ").slice(0, 80),
      rect: rect(e),
      style: style(e),
    }));
  });
  results.push({ mode: "live", url: liveUrl, viewport: view.name, controls: snapshot });
  // One image per viewport, from the top: a `fullPage` image of this app is
  // 1440×20602 and mostly blank, because the scroll container inside the fixed
  // body never paints below the fold, and it is too tall for a model to accept.
  try {
    await page.screenshot({ path: `${out}/live-${view.name}.png` });
    const step = Math.max(64, viewport - Math.ceil(overlap));
    if (height > viewport && viewport > 0) {
      let part = 2;
      for (let top = step; top < height; top += step) {
        await scrollLive(page, element, Math.min(top, height - viewport));
        await page.screenshot({ path: `${out}/live-${view.name}-${part}.png` });
        part += 1;
      }
    }
  } finally {
    await handle.dispose();
  }
  await page.close();
}

async function auditDevices(theme, view) {
  const page = await browser.newPage({
    viewport: { width: view.width, height: view.height },
    deviceScaleFactor: 1,
    hasTouch: view.name === "mobile",
    isMobile: view.name === "mobile",
  });
  await page.goto(`${base}ui-audit/index.html?fixture=devices&theme=${theme}`);
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(250);

  const snapshot = () =>
    page.evaluate(() => {
      const rect = (e) => {
        if (!e) return null;
        const r = e.getBoundingClientRect();
        return { x: +r.x.toFixed(1), y: +r.y.toFixed(1), w: +r.width.toFixed(1), h: +r.height.toFixed(1) };
      };
      const cs = (e) => (e ? getComputedStyle(e) : null);
      const list = document.querySelector('[data-testid="device-list"]');
      const card = document.querySelector('li[data-testid^="device-"]');
      const summary = card?.querySelector("summary");
      const name = card?.querySelector('[data-testid^="name-"]');
      return {
        list: { rect: rect(list), columns: cs(list)?.gridTemplateColumns ?? "", gap: cs(list)?.gap ?? "" },
        count: document.querySelectorAll('li[data-testid^="device-"]').length,
        card: card
          ? {
              rect: rect(card),
              borderW: cs(card).borderTopWidth,
              border: cs(card).borderTopColor,
              padding: cs(card).padding,
              radius: cs(card).borderRadius,
              bg: cs(card).backgroundColor,
            }
          : null,
        summary: summary ? { rect: rect(summary), bg: cs(summary).backgroundColor } : null,
        name: name ? { decoration: cs(name).textDecorationLine } : null,
        heads: [...document.querySelectorAll('li[data-testid^="device-"]')]
          .filter((c) => c.dataset.status === "active")
          .map((c) => {
            const headName = c.querySelector('[data-testid^="name-"]');
            const group = c.querySelector('[data-testid="row-badges"]');
            const box = group?.getBoundingClientRect() ?? null;
            return {
              name: (headName?.textContent ?? "").trim(),
              nameWidth: headName?.clientWidth ?? 0,
              nameFull: headName ? headName.scrollWidth <= headName.clientWidth + 1 : true,
              badges: group
                ? [...group.children]
                    .filter((b) => b.getBoundingClientRect().width > 0)
                    .map((b) => {
                      const r = b.getBoundingClientRect();
                      return { text: (b.textContent ?? "").trim(), width: +r.width.toFixed(1), whole: box ? r.left >= box.left - 0.5 && r.right <= box.right + 0.5 && r.top >= box.top - 0.5 && r.bottom <= box.bottom + 0.5 : true };
                    })
                : [],
            };
          }),
        cards: [...document.querySelectorAll('li[data-testid^="device-"]')].map((c) => rect(c)),
        overflow: {
          doc: document.documentElement.scrollWidth,
          win: window.innerWidth,
          listScroll: list?.scrollWidth ?? 0,
          listClient: list?.clientWidth ?? 0,
        },
        pageBg: cs(document.body).backgroundColor,
        root: (() => {
          const r = document.querySelector("[data-fixture-root]");
          const s = r ? getComputedStyle(r) : null;
          return r ? { w: r.getBoundingClientRect().width, type: s.containerType, name: s.containerName } : null;
        })(),
        cardClass: card?.className ?? null,
      };
    });

  const before = await snapshot();
  await page.screenshot({ path: `${out}/devices-${theme}-${view.name}.png` });
  await page.locator('li[data-testid^="device-"] summary').first().hover();
  await page.waitForTimeout(200);
  const hover = await snapshot();
  await page.screenshot({ path: `${out}/devices-hover-${theme}-${view.name}.png` });
  results.push({ mode: "devices", theme, viewport: view.name, before, hover });

  const fail = (id, actual, expected) => failures.push({ theme, viewport: view.name, fixture: "devices", id, actual, expected });
  const desktop = view.name === "desktop";
  const columns = before.list.columns.split(/\s+/).filter(Boolean).length;
  if (!before.card || before.count < 2) fail("devices.cards", before.count, "the five fixture devices");
  if (desktop) {
    if (columns < 2) fail("devices.columns", before.list.columns, ">= 2 card columns");
    if (before.card?.borderW !== "1px") fail("devices.card-frame", before.card?.borderW, "a 1px card frame");
    if (before.card?.padding !== "16px") fail("devices.card-padding", before.card?.padding, "16px");
    if (parseFloat(before.card?.radius ?? "1") !== 0) fail("devices.card-radius", before.card?.radius, "0");
    if ((before.card?.rect.h ?? 0) < 88) fail("devices.card-height", before.card?.rect.h, ">= 88px");
    if (!(before.cards[0]?.h > before.cards[1]?.h)) {
      fail("devices.card-self-start", before.cards.slice(0, 3), "a taller card does not stretch its row");
    }
    if (before.cards[0]?.y !== before.cards[1]?.y) fail("devices.card-top", before.cards.slice(0, 2), "cards in a row start together");
    if (before.card?.border === hover.card?.border) fail("devices.hover-frame", hover.card?.border, "a border change on hover");
    if (hover.summary?.bg !== before.summary?.bg) fail("devices.hover-fill", hover.summary?.bg, "no row-wide fill on hover");
    if (hover.name?.decoration !== "underline") fail("devices.hover-name", hover.name?.decoration, "an underlined name on hover");
    for (const head of before.heads) {
      if (!head.nameFull) fail("devices.head-name", { name: head.name, width: head.nameWidth }, "the name fits its head, not truncated");
      for (const badge of head.badges) if (!badge.whole) fail("devices.head-badge", badge, "the badge is drawn whole inside the head");
    }
  }
  if (before.overflow.doc > before.overflow.win + 1 || before.overflow.listScroll > before.overflow.listClient + 1) {
    fail("devices.overflow", before.overflow, "no horizontal overflow");
  }
  if (!desktop) {
    if (columns !== 1) fail("devices.mobile-columns", before.list.columns, "one column");
    if (before.card?.borderW !== "0px") fail("devices.mobile-row", before.card?.borderW, "no card frame below 44rem");
    if (Math.round(before.summary?.rect.h ?? 0) !== 56) fail("devices.mobile-row-height", before.summary?.rect.h, "56px");
  }
  await page.close();
}

/**
 * The instant-chat home (`features/chat/chat-home.tsx`, docs/ui-spec.md §3):
 * the new-session layout — the question about the middle on a desktop, the
 * composer pinned to the bottom on a phone, one 48rem column — with the
 * backend switch above the composer and the warnings beside it. Six states
 * are captured: claude selected, the api backend with no provider, the
 * signed-out local login, the desktop `/chat` beside the list, claude with
 * one picture and one file attached (uploaded into the fixture through the
 * composer's own file inputs), and the refusal of a binary file the fixture's
 * chat start answers `400` for. The api tab is clicked, because no query
 * selects a backend.
 */
async function auditNewChat(theme, view) {
  const page = await browser.newPage({
    viewport: { width: view.width, height: view.height },
    deviceScaleFactor: 1,
    hasTouch: view.name === "mobile",
    isMobile: view.name === "mobile",
  });
  const fail = (id, actual, expected) => failures.push({ theme, viewport: view.name, fixture: "new-chat", id, actual, expected });
  const snapshots = {};
  for (const state of ["claude", "api", "signed-out", "list", "attached", "refused"]) {
    await page.goto(`${base}ui-audit/index.html?fixture=new-chat&state=${state}&theme=${theme}`);
    await page.evaluate(() => document.fonts.ready);
    // The form is drawn once the accounts' and composer's answers are in; the
    // list, once its chats are.
    await page.waitForSelector("textarea", { timeout: 5_000 });
    await page.waitForFunction(() => (document.body.textContent ?? "").includes("Release notes for 0.6"), undefined, { timeout: 5_000 }).catch(() => undefined);
    await page.waitForTimeout(200);
    if (state === "api") {
      await page.getByRole("tab", { name: "api" }).click();
      await page.waitForTimeout(200);
    }
    if (state === "attached") {
      // The composer's own inputs, the way a picker would fill them: one text
      // file and one picture drawn in the page (a synthetic photo, no asset).
      await page.evaluate(async () => {
        const attach = (input, file) => {
          const transfer = new DataTransfer();
          transfer.items.add(file);
          input.files = transfer.files;
          input.dispatchEvent(new Event("change", { bubbles: true }));
        };
        const inputs = [...document.querySelectorAll("[data-composer] input[type=file]")];
        const canvas = document.createElement("canvas");
        canvas.width = 96;
        canvas.height = 96;
        const ctx = canvas.getContext("2d");
        const gradient = ctx.createLinearGradient(0, 0, 96, 96);
        gradient.addColorStop(0, "#7aa7ff");
        gradient.addColorStop(1, "#1b3a67");
        ctx.fillStyle = gradient;
        ctx.fillRect(0, 0, 96, 96);
        ctx.fillStyle = "#fff";
        ctx.font = "600 14px sans-serif";
        ctx.fillText("chart", 24, 54);
        const blob = await new Promise((resolve) => canvas.toBlob(resolve, "image/png"));
        attach(inputs[0], new File(["release notes\n"], "release-notes.txt", { type: "text/plain" }));
        attach(inputs[1], new File([blob], "chart.png", { type: "image/png" }));
      });
      await page.waitForSelector('[data-attachment="image"]', { timeout: 5_000 });
      await page.waitForSelector('[data-attachment="file"]', { timeout: 5_000 });
      await page.waitForTimeout(250);
    }
    if (state === "refused") {
      // A binary file: uploaded, then refused by the chat start, and the
      // server's own sentence is the notice the state is about.
      await page.evaluate(() => {
        const input = document.querySelector("[data-composer] input[type=file]");
        const transfer = new DataTransfer();
        transfer.items.add(new File([new Uint8Array([0xff, 0xfe, 0x00])], "binary.bin", { type: "application/octet-stream" }));
        input.files = transfer.files;
        input.dispatchEvent(new Event("change", { bubbles: true }));
      });
      await page.waitForSelector('[data-attachment="file"]', { timeout: 5_000 });
      await page.locator("[data-composer] textarea").fill("take this");
      await page.waitForFunction(() => !document.querySelector('[data-variant="primary"]')?.hasAttribute("disabled"), undefined, {
        timeout: 5_000,
      });
      await page.locator('[data-variant="primary"]').click();
      await page.waitForSelector('[data-chat-new-column] [data-signal="failed"]', { timeout: 5_000 });
      await page.waitForTimeout(250);
    }
    snapshots[state] = await page.evaluate(() => {
      const rect = (e) => {
        if (!e) return null;
        const r = e.getBoundingClientRect();
        return { x: +r.x.toFixed(1), y: +r.y.toFixed(1), w: +r.width.toFixed(1), h: +r.height.toFixed(1) };
      };
      const cs = (e) => (e ? getComputedStyle(e) : null);
      const composer = document.querySelector("[data-composer]");
      const region = document.querySelector("[data-chat-new-composer]");
      const column = document.querySelector("[data-chat-new-column]");
      const tabs = document.querySelector('[role="tablist"]');
      const send = document.querySelector('[data-variant="primary"]');
      const warnings = [...document.querySelectorAll('[data-signal="warn"]')].map((e) => (e.textContent ?? "").trim());
      return {
        composer: composer
          ? { rect: rect(composer), borderW: cs(composer).borderTopWidth, radius: cs(composer).borderRadius, shadow: cs(composer).boxShadow }
          : null,
        region: region ? { rect: rect(region) } : null,
        column: column ? { rect: rect(column) } : null,
        tabs: tabs ? { rect: rect(tabs), radius: cs(tabs).borderTopLeftRadius } : null,
        send: send ? { rect: rect(send), bg: cs(send).backgroundColor, radius: cs(send).borderRadius } : null,
        textarea: rect(document.querySelector("[data-composer] textarea")),
        attachments: [...document.querySelectorAll("[data-attachment]")].map((e) => e.getAttribute("data-attachment")),
        failed: [...document.querySelectorAll('[data-chat-new-column] [data-signal="failed"]')].map((e) => (e.textContent ?? "").trim()),
        hint: (document.querySelector("[data-attachment-hint]")?.textContent ?? "").trim(),
        warnings,
        overflow: { doc: document.documentElement.scrollWidth, win: window.innerWidth },
      };
    });
    await page.screenshot({ path: `${out}/new-chat-${state}-${theme}-${view.name}.png` });
  }
  results.push({ mode: "new-chat", theme, viewport: view.name, snapshots });

  const claude = snapshots.claude;
  if (!claude.composer) fail("new-chat.composer", null, "the composer box");
  else {
    if (claude.composer.borderW !== "1px") fail("new-chat.composer-border", claude.composer.borderW, "1px");
    if (parseFloat(claude.composer.radius) !== 0) fail("new-chat.composer-radius", claude.composer.radius, "0");
    if (claude.composer.shadow !== "none") fail("new-chat.composer-shadow", claude.composer.shadow, "none");
  }
  if (!claude.textarea) fail("new-chat.textarea", null, "a textarea inside the composer");
  if (!claude.tabs || Math.round(claude.tabs.rect.h) !== 32) fail("new-chat.segmented-height", claude.tabs?.rect.h, "32px");
  if (parseFloat(claude.tabs?.radius ?? "1") !== 0) fail("new-chat.segmented-radius", claude.tabs?.radius, "0");
  if (!claude.send || Math.round(claude.send.rect.w) !== 26 || Math.round(claude.send.rect.h) !== 26) {
    fail("new-chat.send-size", claude.send?.rect, "26 x 26 px");
  }
  if (parseFloat(claude.send?.radius ?? "1") !== 0) fail("new-chat.send-radius", claude.send?.radius, "0");
  if (claude.overflow.doc > claude.overflow.win + 1) fail("new-chat.overflow", claude.overflow, "no horizontal overflow");
  if (view.name === "desktop") {
    if (!claude.column || claude.column.rect.w > 768.5) fail("new-chat.column", claude.column?.rect, "<= 48rem");
  } else {
    // The composer region is the last thing in the view: its bottom is the viewport's.
    if (!claude.region || Math.abs(claude.region.rect.y + claude.region.rect.h - view.height) > 1) {
      fail("new-chat.pinned", claude.region?.rect, "the composer at the bottom of the view");
    }
  }
  if (!snapshots.api.warnings.some((text) => /no API provider configured/.test(text))) {
    fail("new-chat.api-warning", snapshots.api.warnings, "the no-provider warning on the api backend");
  }
  if (!snapshots["signed-out"].warnings.some((text) => /signed out/.test(text))) {
    fail("new-chat.signed-out-warning", snapshots["signed-out"].warnings, "the signed-out local login warning");
  }
  // The attachments are the session composer's own chips, one of each kind,
  // and the untouched states have none.
  const attached = [...(snapshots.attached?.attachments ?? [])].sort().join(",");
  if (attached !== "file,image") fail("new-chat.attachments", snapshots.attached?.attachments, "one file and one image chip");
  if ((snapshots.claude.attachments ?? []).length !== 0) {
    fail("new-chat.no-attachments", snapshots.claude.attachments, "no chips before anything is attached");
  }
  // A claude chat says what it takes before the picker is used; the api
  // backend has no attachments at all and shows no hint.
  if (!/PNG\/JPEG\/GIF\/WebP/.test(snapshots.claude.hint ?? "")) {
    fail("new-chat.attachment-hint", snapshots.claude.hint, "the claude chat's accepted-file note");
  }
  if ((snapshots.api.hint ?? "") !== "") fail("new-chat.api-hint", snapshots.api.hint, "no attachment note on the api backend");
  // And a refused file is the server's sentence, in the failed notice style.
  if (!(snapshots.refused?.failed ?? []).some((text) => /not a UTF-8 text file/.test(text))) {
    fail("new-chat.refused", snapshots.refused?.failed, "the refusal notice for a binary file");
  }
  await page.close();
}

/**
 * Settings › Agents & accounts (docs/ui-spec.md §3): text tabs, not a second
 * segmented strip; a card grid on the sheet's full width — three columns on a
 * desktop sheet, one on a phone — with the text kept to the 48rem column; the
 * details dialog with its error well; the empty state's preset cards.
 */
async function auditAgents(theme, view) {
  const page = await browser.newPage({
    viewport: { width: view.width, height: view.height },
    deviceScaleFactor: 1,
    hasTouch: view.name === "mobile",
    isMobile: view.name === "mobile",
  });
  const fail = (id, actual, expected) => failures.push({ theme, viewport: view.name, fixture: "agents", id, actual, expected });
  const desktop = view.name === "desktop";
  const snapshot = () =>
    page.evaluate(() => {
      const rect = (e) => {
        if (!e) return null;
        const r = e.getBoundingClientRect();
        return { x: +r.x.toFixed(1), y: +r.y.toFixed(1), w: +r.width.toFixed(1), h: +r.height.toFixed(1) };
      };
      const cs = (e) => (e ? getComputedStyle(e) : null);
      const root = document.querySelector("[data-fixture-root]");
      const list = document.querySelector('[role="tablist"]');
      const active = document.querySelector('[role="tab"][aria-selected="true"]');
      const idle = document.querySelector('[role="tab"][aria-selected="false"]');
      const grid = document.querySelector("[data-acp-grid]") ?? document.querySelector("[data-acp-preset]")?.closest("ul");
      const cards = [...document.querySelectorAll("[data-acp-card], [data-acp-preset] > button, [data-add-card]")];
      const card = document.querySelector("[data-acp-card]") ?? document.querySelector("[data-acp-preset] > button");
      // The tab's lead: a `SettingsHint` in the panel's `SettingsColumn`.
      const hint = document.querySelector('[data-tab-panel="acp"] > div:first-child > p');
      const visible = (e) => e && e.getBoundingClientRect().width > 0;
      const invalid = [...(root ?? document.body).querySelectorAll("button,input,[role=tab],li,pre,dl")]
        .filter(visible)
        .map((e) => ({ tag: e.tagName, radius: getComputedStyle(e).borderRadius, shadow: getComputedStyle(e).boxShadow }))
        .filter((x) => parseFloat(x.radius) > 0 || (x.shadow !== "none" && !x.shadow.includes("inset")));
      const details = document.querySelector("[data-acp-details]");
      const well = details?.querySelector('pre[data-signal="failed"]');
      return {
        tabs: {
          list: list ? { rect: rect(list), borderTop: cs(list).borderTopWidth, borderBottom: cs(list).borderBottomWidth, bg: cs(list).backgroundColor } : null,
          active: active ? { rect: rect(active), bg: cs(active).backgroundColor, color: cs(active).color, shadow: cs(active).boxShadow, size: cs(active).fontSize } : null,
          idle: idle ? { color: cs(idle).color } : null,
        },
        grid: grid ? { rect: rect(grid), columns: cs(grid).gridTemplateColumns } : null,
        cards: cards.map((c) => rect(c)),
        card: card ? { borderW: cs(card).borderTopWidth, border: cs(card).borderTopColor, radius: cs(card).borderRadius, padding: cs(card).padding } : null,
        hint: rect(hint),
        details: details ? { rect: rect(details.closest('[role="dialog"]') ?? details), well: well ? { color: cs(well).color, font: cs(well).fontFamily } : null } : null,
        presets: document.querySelectorAll("[data-acp-preset]").length,
        overflow: { doc: document.documentElement.scrollWidth, win: window.innerWidth },
        invalid,
      };
    });
  for (const which of ["cards", "details", "empty"]) {
    await page.goto(`${base}ui-audit/index.html?fixture=agents&view=${which}&theme=${theme}`);
    await page.evaluate(() => document.fonts.ready);
    await page.waitForTimeout(250);
    const before = await snapshot();
    await page.screenshot({ path: `${out}/agents-${which}-${theme}-${view.name}.png` });
    results.push({ mode: "agents", view: which, theme, viewport: view.name, before });
    if (which === "cards") {
      const tabs = before.tabs;
      if (!tabs.list || tabs.list.borderTop !== "0px" || tabs.list.borderBottom !== "1px") fail("agents.tabs-rule", tabs.list, "words over one 1px rule, no frame");
      if (!tabs.active || tabs.active.bg !== "rgba(0, 0, 0, 0)") fail("agents.tabs-fill", tabs.active?.bg, "no ink fill on the chosen tab");
      if (!tabs.active || !/0px -2px 0px 0px|inset 0px -2px/.test(tabs.active.shadow)) fail("agents.tabs-underline", tabs.active?.shadow, "a 2px underline on the chosen tab");
      if (tabs.active && tabs.idle && tabs.active.color === tabs.idle.color) fail("agents.tabs-ink", tabs.active.color, "the chosen tab in ink, the rest muted");
      const columns = before.grid?.columns.split(/\s+/).filter(Boolean).length ?? 0;
      if (desktop && columns < 3) fail("agents.columns", before.grid?.columns, ">= 3 card columns on a desktop sheet");
      if (!desktop && columns !== 1) fail("agents.mobile-columns", before.grid?.columns, "one column on a phone");
      if (before.card?.borderW !== "1px") fail("agents.card-frame", before.card?.borderW, "a 1px card frame");
      if (parseFloat(before.card?.radius ?? "1") !== 0) fail("agents.card-radius", before.card?.radius, "0");
      const firstRow = before.cards.filter((c) => c && Math.abs(c.y - before.cards[0].y) < 1);
      if (new Set(firstRow.map((c) => Math.round(c.h))).size > 1) fail("agents.card-height", firstRow, "one height per row");
      if (!before.hint || before.hint.w > 768.5) fail("agents.text-column", before.hint?.w ?? null, "the tab's lead, within 48rem");
      if (desktop) {
        await page.locator("[data-acp-card]").first().hover();
        await page.waitForTimeout(200);
        const hover = await snapshot();
        if (hover.card?.border === before.card?.border) fail("agents.hover-frame", hover.card?.border, "a border change on hover");
      }
    }
    if (which === "details") {
      if (!before.details?.well) fail("agents.details-error", before.details, "the failed check's error in a well");
      else if (!/mono|Menlo|Consolas|SFMono/i.test(before.details.well.font)) fail("agents.details-error-font", before.details.well.font, "monospaced");
      if ((before.details?.rect.w ?? 0) > 512.5) fail("agents.details-width", before.details?.rect.w, "<= 32rem");
    }
    if (which === "empty" && before.presets < 2) fail("agents.presets", before.presets, "the built-in presets as start cards");
    if (before.overflow.doc > before.overflow.win + 1) fail("agents.overflow", before.overflow, "no horizontal overflow");
    if (before.invalid.length) fail("agents.radius-shadow", before.invalid, "no radius, no drop shadow");
  }
  await page.close();
}

/**
 * The cloud bar (`features/cloud/cloud-bar.tsx`) in each state of its
 * connection, in every language the app ships: one 28px strip, with the
 * device switcher drawn (an account of two devices, as most are); every
 * state's words and the device's name whole, with no ellipsis, at a phone's
 * width as on a desktop (the fixture draws each state at its widest: two-digit
 * countdowns, the longest relay city); between tries, a countdown that is a
 * real button with a name and the 2px focus ring, never "0 s", and whole in
 * the strip before the buttons; while a channel is opened, the step in words;
 * live, the path indicator and no words. Captured: the whole fixture in each
 * language, and in English each state's strip on its own and the focused
 * countdown. Each state's free space (the strip's spacer) is in
 * measurements.json.
 */
const CLOUD_BAR_LANGS = ["en", "zh-CN", "zh-TW", "ja"];
/** The English words the bar says for each fixture state. */
const CLOUD_BAR_WORDS = {
  hub: "reaching the hub…",
  "hub-retry": "no hub",
  presence: "finding the device…",
  relay: "Tokyo relay…",
  "relay-long": "San Francisco relay…",
  handshake: "handshake…",
  connecting: "connecting…",
  "device-retry": "no reply",
  offline: "device offline",
  refused: "refuses this browser",
  unconfirmed: "confirmation pending",
  unsaved: "access not saved",
};
/** The countdown in each language: a whole number of seconds, at least 1. */
const CLOUD_BAR_COUNTDOWN = {
  en: /^retry [1-9]\d* s$/,
  "zh-CN": /^[1-9]\d* 秒后重试$/,
  "zh-TW": /^[1-9]\d* 秒後重試$/,
  ja: /^[1-9]\d* 秒で再試行$/,
};

/**
 * What the cloud-bar fixture's page draws, state by state (`[data-audit-row]`):
 * the strip and its parts — the device's name and the words, each with
 * whether it is cut short; the switcher; the countdown; the strip's free
 * space — and, for the catch-up states, the catch-up's words wherever they are
 * drawn (in the offline notice, in the strip, or on a line of their own under
 * it) with the box that would cut them.
 */
function cloudBarSnapshot(page) {
  return page.evaluate(() => {
    const rect = (e) => {
      if (!e) return null;
      const r = e.getBoundingClientRect();
      return { x: +r.x.toFixed(1), y: +r.y.toFixed(1), w: +r.width.toFixed(1), h: +r.height.toFixed(1), right: +r.right.toFixed(1) };
    };
    // Cut short by its box: an ellipsis (`truncate`) or a clip. The text's
    // own width (a range over it) and the box's scroll width both say so.
    const cut = (e) => {
      if (!e) return null;
      const range = document.createRange();
      range.selectNodeContents(e);
      const natural = range.getBoundingClientRect().width;
      const shown = e.getBoundingClientRect().width;
      return { natural: +natural.toFixed(1), shown: +shown.toFixed(1), truncated: e.scrollWidth > e.clientWidth || natural > shown + 0.5 };
    };
    const visible = (e) => e && e.getBoundingClientRect().width > 0;
    return {
      states: [...document.querySelectorAll("[data-audit-row]")].map((section) => {
        const bar = section.querySelector("[data-cloud-bar]");
        const label = bar?.querySelector("[data-connection]");
        const words = label?.querySelector(".truncate");
        const button = bar?.querySelector('[data-testid="retry-now"]');
        const next = bar?.querySelector('[data-testid="my-devices"]');
        // The catch-up's words: the notice has them in its line at desktop width and on a line of their own below it,
        // the strip after its words at desktop width and on a line under it below that — the one drawn.
        const catchUp = [...section.querySelectorAll('[data-testid="catch-up"]')].find(visible) ?? null;
        const clip = catchUp?.closest(".truncate") ?? null;
        const box = catchUp?.closest("[data-catch-up]") ?? null;
        const spacer = bar?.querySelector(":scope > .flex-1");
        // What the words' line has left after them: in the strip, its spacer; on a line of their own, up to its padding; in
        // the notice's line, up to its end (the read-only sentence after them gives way first).
        const after = () => {
          const end = catchUp.getBoundingClientRect().right;
          if (box?.dataset.catchUp === "bar") return spacer?.getBoundingClientRect().width ?? 0;
          if (box?.dataset.catchUp === "line") return box.getBoundingClientRect().right - parseFloat(getComputedStyle(box).paddingRight) - end;
          return (clip?.getBoundingClientRect().right ?? end) - end;
        };
        return {
          name: section.dataset.auditRow,
          bar: rect(bar),
          device: cut(bar?.querySelector('[data-testid="bar-machine-name"]')),
          switcher: rect(bar?.querySelector('[data-testid="machine-switcher"]')),
          label: label ? { kind: label.dataset.connection, rect: rect(label), text: (words?.textContent ?? "").trim(), words: cut(words) } : null,
          // What the strip has left over: nothing had to give way while it is above 0.
          free: +(bar?.querySelector(":scope > .flex-1")?.getBoundingClientRect().width ?? 0).toFixed(1),
          path: bar?.querySelector("[data-path]")?.getAttribute("data-path") ?? null,
          button: button
            ? { tag: button.tagName, type: button.getAttribute("type"), name: button.getAttribute("aria-label"), text: (button.textContent ?? "").trim(), rect: rect(button) }
            : null,
          next: rect(next),
          catchUp: catchUp
            ? {
                where: box?.getAttribute("data-catch-up") ?? null,
                text: (catchUp.textContent ?? "").trim(),
                title: catchUp.getAttribute("title"),
                rect: rect(catchUp),
                clip: rect(clip),
                // A line of its own, or the strip's span: nothing else in its box, so the box must not be cut either.
                cut: clip && clip.textContent.trim() === (catchUp.textContent ?? "").trim() ? cut(clip) : null,
                free: +after().toFixed(1),
              }
            : null,
        };
      }),
      overflow: { doc: document.documentElement.scrollWidth, win: window.innerWidth },
      invalid: [...document.querySelectorAll("[data-cloud-bar] button")]
        .filter(visible)
        .map((e) => ({ text: (e.textContent ?? "").trim(), radius: getComputedStyle(e).borderRadius, shadow: getComputedStyle(e).boxShadow }))
        .filter((x) => parseFloat(x.radius) > 0 || (x.shadow !== "none" && !x.shadow.includes("inset"))),
    };
  });
}

/**
 * The checks every state of the strip passes, whatever else it shows: 28px;
 * the switcher drawn; the device's name and the state's words whole; live,
 * the path indicator and no words; a countdown only between tries (`-retry`
 * states), a real button, its words in the language, whole in the strip
 * before the buttons.
 */
function checkCloudBarState(state, where, view, fail) {
  if (Math.round(state.bar?.h ?? 0) !== 28) fail("cloud-bar.height", { ...where, h: state.bar?.h }, "a 28px strip");
  if (!state.switcher || state.switcher.w < 1) fail("cloud-bar.switcher", { ...where, switcher: state.switcher }, "the device switcher drawn, as for an account of two devices");
  if (state.device?.truncated) fail("cloud-bar.name-truncated", { ...where, ...state.device }, "the device's name whole, no ellipsis");
  if (state.name === "live") {
    if (state.label) fail("cloud-bar.live-words", { ...where, label: state.label }, "no words while live");
    if (state.path !== "relay") fail("cloud-bar.live-path", { ...where, path: state.path }, "the path indicator");
    return;
  }
  if (!state.label) {
    fail("cloud-bar.label", where, "words for the state");
    return;
  }
  if (state.label.words?.truncated) fail("cloud-bar.words-truncated", { ...where, text: state.label.text, ...state.label.words }, "the state's words whole, no ellipsis");
  if (where.lang === "en" && CLOUD_BAR_WORDS[state.name] && state.label.text !== CLOUD_BAR_WORDS[state.name]) {
    fail("cloud-bar.words", { ...where, text: state.label.text }, CLOUD_BAR_WORDS[state.name]);
  }
  const waits = /(^|-)retry$/.test(state.name);
  if (!waits) {
    if (state.button) fail("cloud-bar.no-retry", where, "no countdown while a try is under way");
    return;
  }
  const b = state.button;
  if (!b || b.tag !== "BUTTON" || b.type !== "button" || !b.name) fail("cloud-bar.retry-button", { ...where, button: b }, "a <button type=button> with an aria-label");
  if (!b) return;
  if (!CLOUD_BAR_COUNTDOWN[where.lang].test(b.text)) fail("cloud-bar.countdown", { ...where, text: b.text }, `${CLOUD_BAR_COUNTDOWN[where.lang]}`);
  const inside = b.rect.x >= (state.bar?.x ?? 0) - 0.5 && b.rect.right <= Math.min(state.next?.x ?? Infinity, view.width) + 0.5;
  if (!inside || b.rect.w < 1) fail("cloud-bar.countdown-visible", { ...where, button: b.rect, next: state.next }, "the whole countdown in the strip, before the buttons");
}

/** Load the cloud-bar fixture (`states`: another set of its states) in `lang`, as tall as it is, and measure it. */
async function loadCloudBar(page, view, theme, lang, states = null) {
  await page.goto(`${base}ui-audit/index.html?fixture=cloud-bar${states ? `&states=${states}` : ""}&lang=${lang}&theme=${theme}`);
  await page.evaluate(() => document.fonts.ready);
  if (states) await page.waitForSelector("[data-fixture-root]");
  // Every state in one image: the page as tall as the fixture, since the
  // body is fixed and nothing below the fold paints. Only the width lays
  // the strip out.
  const tall = await page.evaluate(() => Math.ceil(document.querySelector("[data-fixture-root]")?.getBoundingClientRect().height ?? 0));
  if (tall > view.height) await page.setViewportSize({ width: view.width, height: tall });
  await page.waitForTimeout(250);
  return cloudBarSnapshot(page);
}

async function auditCloudBar(theme, view) {
  const page = await browser.newPage({
    viewport: { width: view.width, height: view.height },
    deviceScaleFactor: 1,
    hasTouch: view.name === "mobile",
    isMobile: view.name === "mobile",
  });
  const fail = (id, actual, expected) => failures.push({ theme, viewport: view.name, fixture: "cloud-bar", id, actual, expected });

  for (const lang of CLOUD_BAR_LANGS) {
    const before = await loadCloudBar(page, view, theme, lang);
    await page.screenshot({ path: `${out}/cloud-bar${lang === "en" ? "" : `-${lang}`}-${theme}-${view.name}.png` });
    let focus = null;
    if (lang === "en") {
      for (const state of before.states) {
        await page.locator(`[data-audit-row="${state.name}"] [data-cloud-bar]`).screenshot({ path: `${out}/cloud-bar-${state.name}-${theme}-${view.name}.png` });
      }
      // Keyboard focus on the countdown, as a keyboard user gets it.
      const retry = page.locator('[data-audit-row="device-retry"] [data-testid="retry-now"]');
      await retry.focus();
      focus = await retry.evaluate((e) => {
        const s = getComputedStyle(e);
        return { focusVisible: e.matches(":focus-visible"), width: s.outlineWidth, style: s.outlineStyle, color: s.outlineColor, offset: s.outlineOffset };
      });
      await page.locator('[data-audit-row="device-retry"] [data-cloud-bar]').screenshot({ path: `${out}/cloud-bar-focus-${theme}-${view.name}.png` });
      if (!focus.focusVisible || focus.style === "none" || parseFloat(focus.width) < 2) fail("cloud-bar.focus", focus, "2px visible outline on the countdown");
    }
    results.push({ mode: "cloud-bar", lang, theme, viewport: view.name, before, focus });

    for (const state of before.states) checkCloudBarState(state, { lang, state: state.name }, view, fail);
    if (before.overflow.doc > before.overflow.win + 1) fail("cloud-bar.overflow", { lang, ...before.overflow }, "no horizontal overflow");
    if (before.invalid.length) fail("cloud-bar.radius-shadow", { lang, invalid: before.invalid }, "no radius, no drop shadow");
  }
  await page.close();
}

/** The catch-up's words for each kind of catch-up state, in every language the app ships. */
const CATCH_UP_WORDS = {
  en: { index: "updating from hub · session list", sessions: "updating from hub · sessions 3/7", pages: "updating from hub · sessions 3/7 · 42%" },
  "zh-CN": { index: "正在从云端更新 · 会话列表", sessions: "正在从云端更新 · 会话 3/7", pages: "正在从云端更新 · 会话 3/7 · 42%" },
  "zh-TW": { index: "正在從雲端更新 · 工作階段清單", sessions: "正在從雲端更新 · 工作階段 3/7", pages: "正在從雲端更新 · 工作階段 3/7 · 42%" },
  ja: { index: "ハブから更新中 · セッション一覧", sessions: "ハブから更新中 · セッション 3/7", pages: "ハブから更新中 · セッション 3/7 · 42%" },
};

/**
 * The cloud bar while the offline copy catches up from the hub
 * (`?fixture=cloud-bar&states=catch-up`), drawn as the connection's states
 * are (the switcher, two devices) and held to the same checks, in every
 * language the app ships — and its words whole, with no ellipsis, at a
 * phone's width as on a desktop: in the offline notice after the copy's time
 * at desktop width and on a line of their own at a phone's; with no notice,
 * after the strip's words at desktop width and on a line of their own under
 * the strip at a phone's (where the strip's words leave them no room); the
 * retry countdown beside them whole. Captured: the whole fixture in each
 * language, and in English and Chinese each state's strip with what is under
 * it. Each state's words' free space in their line is in measurements.json.
 */
async function auditCatchUp(theme, view) {
  const page = await browser.newPage({
    viewport: { width: view.width, height: view.height },
    deviceScaleFactor: 1,
    hasTouch: view.name === "mobile",
    isMobile: view.name === "mobile",
  });
  const fail = (id, actual, expected) => failures.push({ theme, viewport: view.name, fixture: "cloud-bar-catch-up", id, actual, expected });
  for (const lang of CLOUD_BAR_LANGS) {
    const before = await loadCloudBar(page, view, theme, lang, "catch-up");
    const tag = lang === "en" ? "" : `-${lang}`;
    await page.screenshot({ path: `${out}/cloud-bar-catch-up${tag}-${theme}-${view.name}.png` });
    if (lang === "en" || lang === "zh-CN") {
      for (const state of before.states) {
        await page.locator(`[data-audit-row="${state.name}"] [data-catch-up-shot]`).screenshot({ path: `${out}/cloud-bar-${state.name}${tag}-${theme}-${view.name}.png` });
      }
    }
    results.push({ mode: "cloud-bar-catch-up", lang, theme, viewport: view.name, before });
    for (const state of before.states) {
      const where = { lang, state: state.name };
      checkCloudBarState(state, where, view, fail);
      const kind = state.name.endsWith("index") ? "index" : state.name.endsWith("sessions") ? "sessions" : "pages";
      const noticed = !state.name.includes("-bar-") && state.name !== "catch-up-retry";
      const expectedWhere = noticed ? "notice" : view.name === "mobile" ? "line" : "bar";
      const c = state.catchUp;
      if (!c || c.where !== expectedWhere) {
        fail("catch-up.where", { ...where, where: c?.where ?? null }, expectedWhere);
        continue;
      }
      if (c.text !== CATCH_UP_WORDS[lang][kind]) fail("catch-up.words", { ...where, text: c.text }, CATCH_UP_WORDS[lang][kind]);
      const whole = c.clip && c.rect.x >= c.clip.x - 0.5 && c.rect.right <= c.clip.right + 0.5 && !c.cut?.truncated;
      if (!whole) fail("catch-up.truncated", { ...where, text: c.text, words: c.rect, clip: c.clip, cut: c.cut }, "the catch-up's words whole, no ellipsis");
      if (kind !== "index" && !c.title) fail("catch-up.title", where, "a tooltip saying what the counts are");
    }
    if (before.overflow.doc > before.overflow.win + 1) fail("catch-up.overflow", { lang, ...before.overflow }, "no horizontal overflow");
  }
  await page.close();
}

/** The line under the spinner for each stage of the session-opening fixture, in every language the app ships. */
const SESSION_OPENING_WORDS = {
  en: { connecting: "connecting…", loading: "loading session…", "copy-pages": "from hub · 128/256", copy: "from hub…", retrying: "cannot open · retrying" },
  "zh-CN": { connecting: "连接中…", loading: "读取会话…", "copy-pages": "从云端下载 · 128/256", copy: "从云端下载…", retrying: "打不开会话 · 重试中" },
  "zh-TW": { connecting: "連線中…", loading: "讀取工作階段…", "copy-pages": "從雲端下載 · 128/256", copy: "從雲端下載…", retrying: "無法開啟工作階段 · 重試中" },
  ja: { connecting: "接続中…", loading: "セッションを読み込み中…", "copy-pages": "ハブから取得 · 128/256", copy: "ハブから取得中…", retrying: "セッションを開けません · 再試行中" },
};

/**
 * What the session-opening fixture draws, stage by stage (`[data-audit-row]`):
 * the view, its spinner, the line under it — its words, type, and the room
 * the view's padding leaves beside it — and the bar.
 */
function sessionOpeningSnapshot(page) {
  return page.evaluate(() => {
    const rect = (e) => {
      if (!e) return null;
      const r = e.getBoundingClientRect();
      return { x: +r.x.toFixed(1), y: +r.y.toFixed(1), w: +r.width.toFixed(1), h: +r.height.toFixed(1), right: +r.right.toFixed(1) };
    };
    return {
      states: [...document.querySelectorAll("[data-audit-row]")].map((section) => {
        const opening = section.querySelector("[data-session-opening]");
        const line = section.querySelector("[data-opening-line]");
        const bar = section.querySelector("[data-opening-bar]");
        // The room the line has: the view, inside its own padding.
        const box = opening?.getBoundingClientRect();
        const pad = opening ? parseFloat(getComputedStyle(opening).paddingLeft) : 0;
        const room = box ? { left: box.left + pad, right: box.right - pad } : null;
        let words = null;
        if (line && room) {
          const r = line.getBoundingClientRect();
          const range = document.createRange();
          range.selectNodeContents(line);
          const natural = range.getBoundingClientRect().width;
          const s = getComputedStyle(line);
          words = {
            text: (line.textContent ?? "").trim(),
            rect: rect(line),
            size: s.fontSize,
            font: s.fontFamily,
            // Cut short (an ellipsis or a clip) or out of the view's padded box.
            cut: line.scrollWidth > line.clientWidth || natural > r.width + 0.5,
            inside: r.left >= room.left - 0.5 && r.right <= room.right + 0.5,
            free: +(room.right - room.left - r.width).toFixed(1),
          };
        }
        return {
          name: section.dataset.auditRow,
          stage: opening?.getAttribute("data-session-opening") ?? null,
          view: rect(section.querySelector("[data-session-view]")),
          spinner: rect(opening?.querySelector("svg")),
          words,
          bar: bar ? { rect: rect(bar), radius: getComputedStyle(bar).borderRadius, fill: rect(bar.firstElementChild) } : null,
        };
      }),
      overflow: { doc: document.documentElement.scrollWidth, win: window.innerWidth },
    };
  });
}

/**
 * A session opening with nothing to show yet (`?fixture=session-opening`), in
 * each stage of its wait, in every language the app ships: the spinner where
 * it always was, in the middle of the view; under it, one line in mono at
 * 11px saying the stage's words, whole (no ellipsis, no clip) inside the
 * view's padding at a phone's width as on a desktop; and the thin bar, 2px
 * tall and square, only where there is a total (the copy's pages), filled to
 * its share. Captured: the whole fixture in each language, and in English
 * and Chinese each stage's view on its own (the spinner held still). Each
 * stage's free space beside its line is in measurements.json.
 */
async function auditSessionOpening(theme, view) {
  const page = await browser.newPage({
    viewport: { width: view.width, height: view.height },
    deviceScaleFactor: 1,
    hasTouch: view.name === "mobile",
    isMobile: view.name === "mobile",
  });
  const fail = (id, actual, expected) => failures.push({ theme, viewport: view.name, fixture: "session-opening", id, actual, expected });
  for (const lang of CLOUD_BAR_LANGS) {
    await page.setViewportSize({ width: view.width, height: view.height });
    await page.goto(`${base}ui-audit/index.html?fixture=session-opening&lang=${lang}&theme=${theme}`);
    await page.evaluate(() => document.fonts.ready);
    await page.waitForSelector("[data-fixture-root]");
    // The line is said only once the wait has gone on for a moment (`OPENING_DELAY_MS`).
    await page.waitForFunction(() => document.querySelectorAll("[data-opening-line]").length === document.querySelectorAll("[data-audit-row]").length);
    const tall = await page.evaluate(() => Math.ceil(document.querySelector("[data-fixture-root]")?.getBoundingClientRect().height ?? 0));
    if (tall > view.height) await page.setViewportSize({ width: view.width, height: tall });
    await page.waitForTimeout(250);
    const before = await sessionOpeningSnapshot(page);
    const tag = lang === "en" ? "" : `-${lang}`;
    await page.screenshot({ path: `${out}/session-opening${tag}-${theme}-${view.name}.png`, animations: "disabled" });
    if (lang === "en" || lang === "zh-CN") {
      for (const state of before.states) {
        await page.locator(`[data-audit-row="${state.name}"] [data-session-view]`).screenshot({ path: `${out}/session-opening-${state.name}${tag}-${theme}-${view.name}.png`, animations: "disabled" });
      }
    }
    results.push({ mode: "session-opening", lang, theme, viewport: view.name, before });
    for (const state of before.states) {
      const where = { lang, state: state.name };
      const stage = state.name.startsWith("copy") ? "copy" : state.name;
      if (state.stage !== stage) fail("session-opening.stage", { ...where, stage: state.stage }, stage);
      // The spinner in the middle of the view, the line or none: it does not move when the words come.
      const centred = state.spinner && state.view
        && Math.abs(state.spinner.x + state.spinner.w / 2 - (state.view.x + state.view.w / 2)) <= 1
        && Math.abs(state.spinner.y + state.spinner.h / 2 - (state.view.y + state.view.h / 2)) <= 1;
      if (!centred) fail("session-opening.spinner", { ...where, spinner: state.spinner, view: state.view }, "the spinner in the middle of the view");
      const w = state.words;
      if (!w) {
        fail("session-opening.line", where, "a line under the spinner");
        continue;
      }
      if (w.text !== SESSION_OPENING_WORDS[lang][state.name]) fail("session-opening.words", { ...where, text: w.text }, SESSION_OPENING_WORDS[lang][state.name]);
      if (w.size !== "11px" || !/mono/i.test(w.font)) fail("session-opening.type", { ...where, size: w.size, font: w.font }, "mono 11px (text-2xs)");
      if (w.cut || !w.inside) fail("session-opening.truncated", { ...where, text: w.text, words: w.rect, free: w.free }, "the line whole, inside the view's padding");
      const bottom = Math.max(w.rect.y + w.rect.h, state.bar ? state.bar.rect.y + state.bar.rect.h : 0);
      if (state.view && bottom > state.view.y + state.view.h + 0.5) fail("session-opening.below", { ...where, bottom, view: state.view }, "the line and the bar inside the view");
      if (state.name === "copy-pages") {
        const b = state.bar;
        if (!b || Math.round(b.rect.h) !== 2 || parseFloat(b.radius) > 0) fail("session-opening.bar", { ...where, bar: b }, "a 2px square bar");
        else if (Math.abs(b.fill.w - b.rect.w / 2) > 0.5) fail("session-opening.bar-fill", { ...where, bar: b }, "filled to 128 of 256");
      } else if (state.bar) {
        fail("session-opening.no-bar", { ...where, bar: state.bar }, "no bar without a total");
      }
    }
    if (before.overflow.doc > before.overflow.win + 1) fail("session-opening.overflow", { lang, ...before.overflow }, "no horizontal overflow");
  }
  await page.close();
}

/** The reason a session with no copy here gives, by state, in every language the app ships (`offlineMessages`). */
const NO_COPY_WORDS = {
  en: { offline: "the device is offline, and there is no copy of this session here yet", "no-copy": "there is no copy of this session on this device yet" },
  "zh-CN": { offline: "设备离线，本设备上还没有这个会话的副本", "no-copy": "这台设备上还没有这个会话的副本" },
  "zh-TW": { offline: "裝置離線，本裝置上還沒有這個工作階段的副本", "no-copy": "這台裝置上還沒有這個工作階段的副本" },
  ja: { offline: "デバイスはオフラインで、ここにはまだこのセッションのコピーがありません", "no-copy": "このデバイスにはまだこのセッションのコピーがありません" },
};

/**
 * The foot of a session with no copy on this device yet
 * (`?fixture=no-copy`): the session composer's box, read-only, the reason
 * its placeholder — the device offline (the hub says so), or only the copy
 * missing (online, or not said yet) — in every language the app ships.
 * Checked: the words themselves, the box disabled, its 1px square frame, and
 * the reason whole in the box at either width — on as many lines as it takes
 * (a phone sets form text at 16px), none of them cut off: the words laid out
 * in the textarea's font and width are no taller than the room inside its
 * padding. Captured: `no-copy[-<lang>]-<theme>-<viewport>.png`, the fixture
 * alone.
 */
async function auditNoCopy(theme, view) {
  const page = await browser.newPage({
    viewport: { width: view.width, height: view.height },
    deviceScaleFactor: 1,
    hasTouch: view.name === "mobile",
    isMobile: view.name === "mobile",
  });
  const fail = (id, actual, expected) => failures.push({ theme, viewport: view.name, fixture: "no-copy", id, actual, expected });
  for (const [lang, words] of Object.entries(NO_COPY_WORDS)) {
    await page.goto(`${base}ui-audit/index.html?fixture=no-copy&lang=${lang}&theme=${theme}`);
    await page.waitForSelector("[data-fixture-root] textarea");
    await page.evaluate(() => document.fonts.ready);
    await page.waitForTimeout(250);
    const before = await page.evaluate(() => ({
      rows: [...document.querySelectorAll("[data-audit-row]")].map((row) => {
        const area = row.querySelector("textarea");
        const box = row.querySelector("[data-composer]");
        const cs = area ? getComputedStyle(area) : null;
        const bs = box ? getComputedStyle(box) : null;
        // The placeholder can be neither selected nor measured: the same words, laid out as the textarea lays them out.
        let text = null;
        if (area && cs) {
          const probe = document.createElement("div");
          const width = area.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight);
          Object.assign(probe.style, { position: "absolute", visibility: "hidden", width: `${width}px`, font: cs.font, lineHeight: cs.lineHeight, letterSpacing: cs.letterSpacing, whiteSpace: "pre-wrap", overflowWrap: "break-word" });
          probe.textContent = area.placeholder;
          document.body.append(probe);
          const height = probe.getBoundingClientRect().height;
          probe.remove();
          const lineHeight = parseFloat(cs.lineHeight);
          text = { lines: Math.round(height / lineHeight), height: +height.toFixed(1), room: +(area.clientHeight - parseFloat(cs.paddingTop) - parseFloat(cs.paddingBottom)).toFixed(1), fontSize: cs.fontSize, width: +width.toFixed(1) };
        }
        return {
          name: row.getAttribute("data-audit-row"),
          placeholder: area?.placeholder ?? null,
          disabled: area?.disabled ?? null,
          text,
          box: bs ? { borderW: bs.borderTopWidth, radius: bs.borderRadius, shadow: bs.boxShadow } : null,
        };
      }),
      overflow: { doc: document.documentElement.scrollWidth, win: window.innerWidth },
    }));
    await page.locator("[data-fixture-root]").screenshot({ path: `${out}/no-copy${lang === "en" ? "" : `-${lang}`}-${theme}-${view.name}.png` });
    results.push({ mode: "no-copy", lang, theme, viewport: view.name, before });
    for (const [name, text] of Object.entries(words)) {
      const row = before.rows.find((r) => r.name === name);
      const where = { lang, state: name };
      if (row?.placeholder !== text) fail("no-copy.words", { ...where, text: row?.placeholder ?? null }, text);
      if (row?.disabled !== true) fail("no-copy.read-only", where, "a disabled composer");
      if (!row?.box || row.box.borderW !== "1px" || parseFloat(row.box.radius) !== 0 || row.box.shadow !== "none") fail("no-copy.box", { ...where, box: row?.box ?? null }, "a 1px square frame, no shadow");
      if (!row?.text || row.text.height > row.text.room + 0.5) fail("no-copy.whole", { ...where, text: row?.text ?? null }, "the reason whole in the box, no line cut off");
    }
    if (before.overflow.doc > before.overflow.win + 1) fail("no-copy.overflow", { lang, ...before.overflow }, "no horizontal overflow");
  }
  await page.close();
}

/** The banner over a session drawn from the device's copy, in the two languages its screenshots are taken in. */
const COPY_BANNER_WORDS = {
  en: "showing the copy saved on this device — updating…",
  "zh-CN": "正在显示这台设备上保存的副本 — 更新中…",
};
/** What the composer says while it waits for the machine's state, which it needs before it sends anything (#571's words). */
const COPY_COMPOSER_WORDS = { en: "loading session…", "zh-CN": "读取会话…" };
/** What the read-only composer says while the page is on the copy, the machine away: the copy's own reason. */
const COPY_OFFLINE_WORDS = { en: "the device is offline — this is a read-only copy", "zh-CN": "设备离线 — 这是只读副本" };
/** The session the fixture draws from its copy: its first and last items' words. */
const COPY_FIRST_WORDS = "The relay drops idle sockets after a minute.";
const COPY_LAST_WORDS = "three beats go unanswered";
/**
 * What the composer's pickers and gauges would say over the copy without the
 * machine's state: the list's first model, its default effort, the CLI's
 * default permission, the local login's quota. None of it is the session's.
 */
const COPY_DEFAULT_WORDS = ["fable", "medium", "acceptEdits", "88%", "64%"];
/** What they say under the machine's own state: the session's model, effort and permission, its account's quota, its context. */
const COPY_OWN_WORDS = ["sonnet", "low", "plan", "37%", "21%", "42%"];
/** The account the session runs on, which only the machine's state names, and what the header's badge says of a session bound to none. */
const COPY_ACCOUNT_WORDS = "work";
const COPY_UNBOUND_WORDS = { en: "no account", "zh-CN": "无账号" };

/**
 * droppen.app on the machine itself, opening a session from the device's copy
 * (`?fixture=session-from-copy`): the real session view drawn at once, the
 * transcript from its first item to its last, under the banner that says it
 * is the copy saved on this device — its existing words, mono at 11px, whole
 * (no ellipsis, no clip) at a phone's width as on a desktop — and no loading
 * line anywhere. Under it the composer waits for the machine's state: the
 * text editable, the send button disabled, and the box saying it is loading
 * the session, not that the session is busy. Nothing the machine's state
 * alone can say is shown in the meantime: no model, effort or permission
 * picker, no quota and no context in the composer's row, and no account in
 * the header. Then the same session once the machine has answered
 * (`&state=live`): the banner gone, and those showing the session's own
 * values. How far the composer and the header's metadata moved between the
 * two is in measurements.json (`shift`). And the same session with the
 * machine away and the page on its copy (`&state=offline`): the transcript
 * the copy serves, the composer read-only with the copy's reason, and again
 * nothing only the machine's state can say — no model, effort or permission
 * (which would be the static fallback list's), no quota (the local login's)
 * and no context in the row, nothing there to press or open, and no account
 * in the header. Captured in English and Chinese.
 */
async function auditSessionFromCopy(theme, view) {
  const page = await browser.newPage({
    viewport: { width: view.width, height: view.height },
    deviceScaleFactor: 1,
    hasTouch: view.name === "mobile",
    isMobile: view.name === "mobile",
  });
  const fail = (id, actual, expected) => failures.push({ theme, viewport: view.name, fixture: "session-from-copy", id, actual, expected });
  for (const lang of Object.keys(COPY_BANNER_WORDS)) {
    const drawn = {};
    for (const variant of ["copy", "live", "offline"]) {
      await page.goto(`${base}ui-audit/index.html?fixture=session-from-copy&lang=${lang}&theme=${theme}${variant === "copy" ? "" : `&state=${variant}`}`);
      await page.evaluate(() => document.fonts.ready);
      if (variant === "live") {
        await page.waitForFunction(
          (waiting) => {
            const box = document.querySelector("[data-session-view] textarea");
            return !!box && box.getAttribute("placeholder") !== waiting && !document.querySelector('[data-session-view] [data-signal="reconnecting"]');
          },
          COPY_COMPOSER_WORDS[lang],
        );
      } else if (variant === "offline") {
        await page.waitForFunction((reason) => document.querySelector("[data-session-view] textarea")?.getAttribute("placeholder") === reason, COPY_OFFLINE_WORDS[lang]);
      } else {
        await page.waitForSelector('[data-session-view] [data-signal="reconnecting"]');
      }
      await page.waitForFunction((words) => document.body.innerText.includes(words), COPY_LAST_WORDS);
      await page.waitForTimeout(250);
      drawn[variant] = await auditSessionFromCopyState(page, theme, view, lang, variant, fail);
    }
    const { copy, live } = drawn;
    // Where the state coming in moved things: the composer's top edge (the
    // transcript's bottom), its row of chips, and the header's metadata row
    // and the first thing after the account in it (the git branch).
    const shift = (a, b, key) => (a && b ? +(b[key] - a[key]).toFixed(1) : null);
    results.push({
      mode: "session-from-copy-shift",
      lang,
      theme,
      viewport: view.name,
      shift: {
        composerTop: shift(copy.composerBox, live.composerBox, "y"),
        chipsRowHeight: shift(copy.chips?.rect, live.chips?.rect, "h"),
        metadataRowHeight: shift(copy.metadata?.rect, live.metadata?.rect, "h"),
        branchX: shift(copy.metadata?.branch, live.metadata?.branch, "x"),
      },
    });
  }
  await page.close();
}

/**
 * One state of the session-from-copy fixture, measured, captured and checked:
 * drawn from the copy while the machine is asked (`copy`), once it has
 * answered (`live`), or on the copy while the machine is away (`offline`).
 * Returns the boxes the shift between the first two is read from.
 */
async function auditSessionFromCopyState(page, theme, view, lang, variant, fail) {
  const live = variant === "live";
  const state = await page.evaluate(() => {
    const rect = (e) => {
      if (!e) return null;
      const r = e.getBoundingClientRect();
      return { x: +r.x.toFixed(1), y: +r.y.toFixed(1), w: +r.width.toFixed(1), h: +r.height.toFixed(1) };
    };
    const banner = document.querySelector('[data-session-view] [data-signal="reconnecting"]');
    let words = null;
    if (banner) {
      const s = getComputedStyle(banner);
      const range = document.createRange();
      range.selectNodeContents(banner);
      const natural = range.getBoundingClientRect();
      const box = banner.getBoundingClientRect();
      words = {
        text: (banner.textContent ?? "").trim(),
        rect: rect(banner),
        size: s.fontSize,
        font: s.fontFamily,
        cut: banner.scrollWidth > banner.clientWidth || natural.right > box.right - parseFloat(s.paddingRight) + 0.5,
        lines: Math.round(natural.height / parseFloat(s.lineHeight)),
      };
    }
    const box = document.querySelector("[data-session-view] textarea");
    const send = document.querySelector('[data-session-view] button[data-variant="primary"]');
    const composerBox = document.querySelector("[data-session-view] [data-composer]");
    // The composer's one toolbar row: attach, the chips, send.
    const chips = composerBox?.lastElementChild ?? null;
    // The header's second row: the session's source, account, branch, model drift and size.
    const metadata = document.querySelector("[data-session-view] header")?.children[1] ?? null;
    return {
      banner: words,
      composer: box ? { placeholder: box.getAttribute("placeholder"), editable: !box.disabled, send: send ? !send.disabled : null } : null,
      composerBox: rect(composerBox),
      chips: chips
        ? {
            rect: rect(chips),
            text: chips.innerText.replace(/\s+/g, " ").trim(),
            // Every control in the row that opens something: a picker, a gauge's overview.
            openers: chips.querySelectorAll('[role="combobox"], [aria-haspopup], [aria-expanded]').length,
            // What in the row can still be pressed, and any mark it carries (the model list's fallback warning).
            enabled: [...chips.querySelectorAll("button")].filter((b) => !b.disabled).length,
            marks: chips.querySelectorAll("[data-signal]").length,
            // What the row holds, and where: a value arriving moves what comes after it.
            items: [...chips.children].map((e) => ({ text: e.innerText.trim(), ...rect(e) })).filter((e) => e.w > 1),
          }
        : null,
      metadata: metadata
        ? { rect: rect(metadata), text: metadata.innerText.replace(/\s+/g, " ").trim(), branch: rect(metadata.querySelector('[title="main"]')) }
        : null,
      opening: document.querySelectorAll("[data-session-opening]").length,
      text: document.body.innerText,
      overflow: { doc: document.documentElement.scrollWidth, win: window.innerWidth },
    };
  });
  const tag = `${variant === "copy" ? "" : `-${variant}`}${lang === "en" ? "" : `-${lang}`}`;
  await page.screenshot({ path: `${out}/session-from-copy${tag}-${theme}-${view.name}.png`, animations: "disabled" });
  const { text, ...measured } = state;
  results.push({ mode: `session-from-copy${variant === "copy" ? "" : `-${variant}`}`, lang, theme, viewport: view.name, ...measured });
  const where = { lang, state: variant };
  if (!text.includes(COPY_FIRST_WORDS) || !text.includes(COPY_LAST_WORDS)) fail("session-from-copy.transcript", where, "the copy's transcript drawn, first item to last");
  if (state.opening > 0) fail("session-from-copy.opening", { ...where, opening: state.opening }, "no loading line over a drawn transcript");
  if (state.overflow.doc > state.overflow.win + 1) fail("session-from-copy.overflow", { ...where, ...state.overflow }, "no horizontal overflow");
  const c = state.composer;
  const chipWords = state.chips?.text ?? "";
  const header = state.metadata?.text ?? "";
  if (variant === "offline") {
    if (!c || c.placeholder !== COPY_OFFLINE_WORDS[lang] || c.editable || c.send !== false) {
      fail("session-from-copy.offline-composer", { ...where, composer: c }, `a read-only box saying "${COPY_OFFLINE_WORDS[lang]}", its send button disabled`);
    }
    // The row as it is while the machine's state is not in, with attach (and
    // the camera, on a phone) and send disabled as the read-only box has them.
    const shownWords = [...COPY_DEFAULT_WORDS, ...COPY_OWN_WORDS].filter((w) => chipWords.includes(w));
    if (!state.chips || chipWords !== "" || state.chips.openers > 0 || state.chips.enabled > 0 || state.chips.marks > 0) {
      fail("session-from-copy.offline-chips", { ...where, chips: state.chips, shown: shownWords }, "no model, effort, permission, quota or context in the composer's row, no mark, and nothing there to press or open");
    }
    if (header.includes(COPY_ACCOUNT_WORDS) || header.includes(COPY_UNBOUND_WORDS[lang])) {
      fail("session-from-copy.offline-account", { ...where, header }, "no account in the header while the page is on the copy");
    }
    return state;
  }
  if (live) {
    if (state.banner) fail("session-from-copy.live-banner", { ...where, banner: state.banner.text }, "no banner once the machine has answered");
    if (!c || c.placeholder === COPY_COMPOSER_WORDS[lang] || !c.editable) {
      fail("session-from-copy.live-composer", { ...where, composer: c }, "an editable box that no longer waits for the session");
    }
    const missing = COPY_OWN_WORDS.filter((w) => !chipWords.includes(w));
    const defaults = COPY_DEFAULT_WORDS.filter((w) => chipWords.includes(w));
    if (missing.length || defaults.length || (state.chips?.openers ?? 0) < 3) {
      fail("session-from-copy.live-chips", { ...where, chips: state.chips, missing, defaults }, `the session's own ${COPY_OWN_WORDS.join(", ")}, and its model, effort and permission pickers`);
    }
    if (!header.includes(COPY_ACCOUNT_WORDS)) fail("session-from-copy.live-account", { ...where, header }, `the account the session runs on, "${COPY_ACCOUNT_WORDS}"`);
    return state;
  }
  if (!state.banner) {
    fail("session-from-copy.banner", where, "the banner over a transcript drawn from the copy");
  } else {
    if (state.banner.text !== COPY_BANNER_WORDS[lang]) fail("session-from-copy.words", { ...where, text: state.banner.text }, COPY_BANNER_WORDS[lang]);
    if (state.banner.size !== "11px" || !/mono/i.test(state.banner.font)) fail("session-from-copy.type", { ...where, size: state.banner.size, font: state.banner.font }, "mono 11px (text-2xs)");
    if (state.banner.cut) fail("session-from-copy.truncated", { ...where, banner: state.banner }, "the banner's words whole");
  }
  if (!c || c.placeholder !== COPY_COMPOSER_WORDS[lang] || !c.editable || c.send !== false) {
    fail("session-from-copy.composer", { ...where, composer: c }, `an editable box saying "${COPY_COMPOSER_WORDS[lang]}", its send button disabled`);
  }
  // Neither the defaults nor anything else: the row holds attach and send alone.
  const shown = [...COPY_DEFAULT_WORDS, ...COPY_OWN_WORDS].filter((w) => chipWords.includes(w));
  if (!state.chips || chipWords !== "" || state.chips.openers > 0) {
    fail("session-from-copy.chips", { ...where, chips: state.chips, shown }, "no model, effort, permission, quota or context in the composer's row, and nothing there to pick");
  }
  if (header.includes(COPY_ACCOUNT_WORDS) || header.includes(COPY_UNBOUND_WORDS[lang])) {
    fail("session-from-copy.account", { ...where, header }, "no account in the header before the machine has named it");
  }
  return state;
}

/** The desktop app's guide screens and its server's setup page (`?fixture=welcome&screen=`). */
const WELCOME_SCREENS = ["welcome", "tools", "where", "first", "wrap", "setup"];
/**
 * Each system the app is built for, with what its window's bridge says of
 * Start at Login (both apps have it) and of a tray host (yes, except stock
 * GNOME without the AppIndicator extension), and an app that says it has
 * neither, on the screens that change for that.
 */
const WELCOME_SYSTEMS = [
  { name: "darwin", platform: "darwin", bridge: "1", tray: "1", screens: WELCOME_SCREENS, edition: { en: "Droppen for Mac", "zh-CN": "Droppen · Mac 版" } },
  { name: "linux", platform: "linux", bridge: "1", tray: "1", screens: WELCOME_SCREENS, edition: { en: "Droppen for Linux", "zh-CN": "Droppen · Linux 版" } },
  { name: "linux-no-login", platform: "linux", bridge: "0", tray: "1", screens: ["welcome", "wrap"], edition: { en: "Droppen for Linux", "zh-CN": "Droppen · Linux 版" } },
  { name: "linux-no-tray", platform: "linux", bridge: "1", tray: "0", screens: ["where", "wrap", "setup"], edition: { en: "Droppen for Linux", "zh-CN": "Droppen · Linux 版" } },
];
/** Start at Login named on the welcome page's line for step 04, in the languages captured. */
const START_AT_LOGIN_WORDS = /Start at login|登录时启动/;
/** What only a Mac has, in the languages captured: never on the Linux app's pages. */
const MAC_ONLY = /\bMacs?\b|macOS|menu.bar|Apple|xcode-select|菜单栏|访达/;
/** What a page with no tray host must not promise: the tray icon, or that closing keeps the app running. */
const NO_TRAY_WORDS = /system tray|系统托盘|系統匣|システムトレイ|closing the window doesn.t stop it|关掉窗口不会停止它|關掉視窗不會停止它|ウインドウを閉じても止まりません/;
/** What a page with no tray host says instead of the tray lines. */
const NO_TRAY_QUIT = /Closing the window quits Droppen|关掉窗口会退出 Droppen/;

/**
 * The desktop app's first-run guide and its server's setup page
 * (`?fixture=welcome`), as the app on each system shows them, from the
 * platform the server reports and the window's bridge: the welcome page names
 * the app's edition; the Linux app's pages never say Mac, macOS, the menu bar
 * or Apple's installer; step 04, and the welcome page's line for it, name
 * Start at Login only where the app offers it; and with no tray host the
 * pages promise neither the tray icon nor Open in Browser, and say that
 * closing the window quits. Captured: every screen on each system, in English
 * and Chinese.
 */
async function auditWelcome(theme, view) {
  const page = await browser.newPage({
    viewport: { width: view.width, height: view.height },
    deviceScaleFactor: 1,
    hasTouch: view.name === "mobile",
    isMobile: view.name === "mobile",
  });
  const fail = (id, actual, expected) => failures.push({ theme, viewport: view.name, fixture: "welcome", id, actual, expected });
  for (const system of WELCOME_SYSTEMS) {
    for (const lang of ["en", "zh-CN"]) {
      for (const screen of system.screens) {
        await page.goto(`${base}ui-audit/index.html?fixture=welcome&screen=${screen}&platform=${system.platform}&bridge=${system.bridge}&tray=${system.tray}&lang=${lang}&theme=${theme}`);
        await page.evaluate(() => document.fonts.ready);
        await page.waitForSelector("[data-fixture-root] > *");
        const seen = await page.evaluate(() => ({
          text: document.querySelector("[data-fixture-root]")?.innerText ?? "",
          kicker: document.querySelector("[data-testid='welcome'] [data-label]")?.textContent ?? null,
          startAtLogin: document.querySelector("[data-testid='wrap-login']") !== null,
        }));
        const tag = lang === "en" ? "" : `-${lang}`;
        await page.screenshot({ path: `${out}/welcome-${screen}-${system.name}${tag}-${theme}-${view.name}.png`, animations: "disabled" });
        results.push({ mode: "welcome", screen, system: system.name, lang, theme, viewport: view.name, kicker: seen.kicker, startAtLogin: seen.startAtLogin });
        const where = { screen, system: system.name, lang };
        if (system.platform === "linux" && MAC_ONLY.test(seen.text)) fail("welcome.mac-words", { ...where, said: seen.text.match(MAC_ONLY)[0] }, "no Mac, macOS, menu bar or Apple on the Linux app's pages");
        if (system.tray === "0" && NO_TRAY_WORDS.test(seen.text)) fail("welcome.no-tray-words", { ...where, said: seen.text.match(NO_TRAY_WORDS)[0] }, "no tray icon, or closing that keeps the app running, where there is no tray host");
        if (system.tray === "0" && screen === "wrap" && !NO_TRAY_QUIT.test(seen.text)) fail("welcome.no-tray-quit", { ...where }, "closing the window quits Droppen");
        if (screen === "welcome" && seen.kicker?.trim() !== system.edition[lang]) fail("welcome.edition", { ...where, kicker: seen.kicker }, system.edition[lang]);
        if (screen === "welcome" && START_AT_LOGIN_WORDS.test(seen.text) !== (system.bridge === "1")) fail("welcome.summary-start-at-login", { ...where, text: seen.text.match(START_AT_LOGIN_WORDS)?.[0] ?? null }, system.bridge === "1" ? "step 04's line names Start at Login" : "step 04's line does not name Start at Login");
        if (screen === "wrap" && seen.startAtLogin !== (system.bridge === "1")) fail("welcome.start-at-login", { ...where, shown: seen.startAtLogin }, system.bridge === "1" ? "Start at Login, which the app offers" : "no Start at Login, which the app does not offer");
      }
    }
  }
  await page.close();
}

/** Settings → Desktop app (`?fixture=desktop-app`), on each system and in each Start at Login state. */
const DESKTOP_APP_CASES = [
  { name: "darwin-on", platform: "darwin", tray: "1", state: "on" },
  { name: "darwin-off", platform: "darwin", tray: "1", state: "off" },
  { name: "darwin-approval", platform: "darwin", tray: "1", state: "approval" },
  { name: "darwin-failed", platform: "darwin", tray: "1", state: "failed" },
  { name: "linux-on", platform: "linux", tray: "1", state: "on" },
  { name: "linux-no-tray", platform: "linux", tray: "0", state: "on" },
  { name: "linux-translocated", platform: "linux", tray: "1", state: "translocated" },
  { name: "linux-no-login", platform: "linux", tray: "1", state: "no-login" },
];
/** Start at Login is on for these; off for the rest (a failed turn-on leaves it off). */
const DESKTOP_APP_ON = new Set(["on", "approval"]);
const DESKTOP_APP_OPEN = { en: "Open in Browser", "zh-CN": "在浏览器中打开" };

/**
 * The app window's Settings → Desktop app section
 * (`?fixture=desktop-app`), as each system and Start at Login state shows it:
 * Open in Browser always; Start at Login with the app's own state, the Login
 * Items line when macOS waits for approval, and no row where the app offers
 * none. Desktop width only: the section is a settings page, never a phone's.
 * Captured: every case in English and Chinese, in each theme.
 */
async function auditDesktopApp(theme, view) {
  if (view.name !== "desktop") return;
  const page = await browser.newPage({ viewport: { width: view.width, height: view.height }, deviceScaleFactor: 1 });
  const fail = (id, actual, expected) => failures.push({ theme, viewport: view.name, fixture: "desktop-app", id, actual, expected });
  for (const one of DESKTOP_APP_CASES) {
    for (const lang of ["en", "zh-CN"]) {
      await page.goto(`${base}ui-audit/index.html?fixture=desktop-app&platform=${one.platform}&tray=${one.tray}&state=${one.state}&lang=${lang}&theme=${theme}`);
      await page.evaluate(() => document.fonts.ready);
      await page.waitForSelector("[data-fixture-root] [data-testid='desktop-app-section']");
      const seen = await page.evaluate(() => {
        const section = document.querySelector("[data-testid='desktop-app-section']");
        const toggle = document.querySelector("[data-testid='desktop-start-at-login']");
        const rect = (element) => {
          if (!element) return null;
          const r = element.getBoundingClientRect();
          return { x: +r.x.toFixed(1), w: +r.width.toFixed(1), h: +r.height.toFixed(1) };
        };
        return {
          text: section?.innerText ?? "",
          open: document.querySelector("[data-testid='desktop-open-in-browser'] button")?.textContent?.trim() ?? null,
          loginRow: document.querySelector("[data-testid='desktop-start-at-login-row']") !== null,
          checked: toggle?.getAttribute("aria-checked") ?? null,
          blocked: toggle?.getAttribute("aria-disabled") ?? null,
          openRow: rect(document.querySelector("[data-audit-row='desktop-open']")),
          loginGeometry: rect(document.querySelector("[data-audit-row='desktop-login']")),
          button: rect(document.querySelector("[data-testid='desktop-open-in-browser'] button")),
        };
      });
      const tag = lang === "en" ? "" : `-${lang}`;
      await page.screenshot({ path: `${out}/desktop-app-${one.name}${tag}-${theme}.png`, animations: "disabled" });
      results.push({ mode: "desktop-app", ...one, lang, theme, ...seen });
      const where = { case: one.name, lang };
      const wantsLogin = one.state !== "no-login";
      if (seen.open !== DESKTOP_APP_OPEN[lang]) fail("desktop-app.open", { ...where, open: seen.open }, DESKTOP_APP_OPEN[lang]);
      if (seen.loginRow !== wantsLogin) fail("desktop-app.login-row", { ...where, shown: seen.loginRow }, wantsLogin ? "the Start at Login row" : "no Start at Login row");
      if (wantsLogin && seen.checked !== String(DESKTOP_APP_ON.has(one.state))) fail("desktop-app.checked", { ...where, checked: seen.checked }, String(DESKTOP_APP_ON.has(one.state)));
      if (seen.blocked !== (one.state === "translocated" ? "true" : null)) fail("desktop-app.blocked", { ...where, blocked: seen.blocked }, one.state === "translocated" ? "a blocked switch" : "an enabled switch");
      if (one.state === "approval" && !/Allow Droppen in Login Items|在登录项里允许 Droppen/.test(seen.text)) fail("desktop-app.approval", { ...where }, "the Login Items line");
      if (one.state === "failed" && !/Allow Droppen in Login Items|在登录项里允许 Droppen/.test(seen.text)) fail("desktop-app.failed", { ...where }, "the Login Items line");
      if (one.state === "translocated" && !/Move Droppen to Applications first|先把 Droppen 移到/.test(seen.text)) fail("desktop-app.translocated", { ...where }, "the move note");
      if (!wantsLogin && /Start Droppen at login|登录时启动 Droppen/.test(seen.text)) fail("desktop-app.no-login", { ...where }, "no Start at Login where the app offers none");
      // The shared skeleton's measurements (docs/ui-spec.md §3/§4): 28px
      // actions, 56px rows with a hint, both rows on the same column edges.
      if (seen.button?.h !== 28) fail("desktop-app.button", { ...where, button: seen.button }, "a 28px action");
      if (!seen.openRow || seen.openRow.h < 56) fail("desktop-app.row", { ...where, row: seen.openRow }, "a 56px row with its hint");
      if (seen.loginGeometry && seen.openRow && (seen.loginGeometry.x !== seen.openRow.x || seen.loginGeometry.w !== seen.openRow.w)) {
        fail("desktop-app.columns", { ...where, open: seen.openRow, login: seen.loginGeometry }, "the same label/control column edges");
      }
    }
  }
  await page.close();
}

try {
  if (liveUrl) {
    for (const view of views) await auditLive(view);
  } else {
    for (const theme of themes) {
      for (const view of views) {
        if (runSettings) await auditSettings(theme, view);
        if (runDevices) await auditDevices(theme, view);
        if (runAgents) await auditAgents(theme, view);
        if (runNewChat) await auditNewChat(theme, view);
        if (runCloudBar) await auditCloudBar(theme, view);
        if (runCloudBar) await auditCatchUp(theme, view);
        if (runSessionOpening) await auditSessionOpening(theme, view);
        if (runNoCopy) await auditNoCopy(theme, view);
        if (runSessionFromCopy) await auditSessionFromCopy(theme, view);
        if (runWelcome) await auditWelcome(theme, view);
        if (runDesktopApp) await auditDesktopApp(theme, view);
      }
    }
  }
} finally {
  await fs.writeFile(`${out}/measurements.json`, JSON.stringify(results, null, 2));
  const lines = [];
  if (check) {
    if (failures.length === 0) lines.push("ui-audit: PASS (all target invariants)");
    else {
      lines.push(`ui-audit: FAIL (${failures.length})`);
      for (const f of failures) lines.push(`  ✗ ${f.id} [${f.theme}/${f.viewport}] actual=${JSON.stringify(f.actual)} expected=${f.expected}`);
    }
  } else {
    lines.push("ui-audit: report only (pass --check to gate)");
  }
  lines.push(`ui-audit: ${out}`);
  await fs.writeFile(`${out}/report.txt`, lines.join("\n") + "\n");
  console.log(lines.join("\n"));
  await browser.close();
  if (server) await server.close();
  if (check && failures.length) process.exitCode = 1;
}
