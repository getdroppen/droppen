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
 *   --fixture <f>      settings | devices | agents | both (default both: every fixture)
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
const views =
  flag("viewport", "both") === "both"
    ? [
        { name: "desktop", width: 1440, height: 1000 },
        { name: "mobile", width: 390, height: 844 },
      ]
    : [{ name: flag("viewport", "desktop"), width: 390, height: 844 }];
const channel = flag("channel", process.env.UI_AUDIT_CHANNEL ?? "");
const liveUrl = flag("url", "");
const check = has("check");
const which = flag("fixture", "both");
const runSettings = which === "both" || which === "settings";
const runDevices = which === "both" || which === "devices";
const runAgents = which === "both" || which === "agents";

await fs.mkdir(out, { recursive: true });

/** Start the app's own Vite server (fixture mode only). */
let server = null;
let base = liveUrl;
if (!liveUrl) {
  process.env.RH_DEV_PROXY ??= "http://127.0.0.1:9";
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
  await page.locator('[data-audit-row="checkbox"] input').focus();
  const checkboxFocus = await page.evaluate(() => {
    const input = document.querySelector('[data-audit-row="checkbox"] input');
    const box = input?.nextElementSibling;
    const s = box ? getComputedStyle(box) : null;
    return {
      focusVisible: input?.matches(":focus-visible") ?? null,
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
    // lucide `<svg>` itself.
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

async function auditLive(view) {
  const page = await browser.newPage({ viewport: { width: view.width, height: view.height }, deviceScaleFactor: 1 });
  await page.goto(liveUrl, { waitUntil: "networkidle" });
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(300);
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
  await page.screenshot({ path: `${out}/live-${view.name}.png`, fullPage: true });
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
      const hint = document.querySelector('[data-tab-panel="acp"] > p');
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
      if ((before.hint?.w ?? 0) > 768.5) fail("agents.text-column", before.hint?.w, "text within 48rem");
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

try {
  if (liveUrl) {
    for (const view of views) await auditLive(view);
  } else {
    for (const theme of themes) {
      for (const view of views) {
        if (runSettings) await auditSettings(theme, view);
        if (runDevices) await auditDevices(theme, view);
        if (runAgents) await auditAgents(theme, view);
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
