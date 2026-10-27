// Where a select's list lands (`components/ui/select.tsx`). jsdom has no
// layout, so this file gives it just enough: a viewport, a box for the trigger
// and for the composer card around it, and a size for the list, which honours
// the popup's `max-h-[min(var(--available-height),20rem)]` the way the browser
// would. Floating UI does the rest exactly as in the app.
//
// What is pinned: the list is against the trigger — 4px off it, left edges
// together — below it when there is room and above it when not, moved along
// the edge rather than off the screen, never over the trigger, and never
// pushed past the composer card (the list a composer chip opened used to float
// a whole card's height above the chip).
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after, afterEach } from "node:test";
import { installJsdom } from "./jsdom.js";
import { makeI18n } from "./render.js";

const { window, document, container } = installJsdom();
after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});

type Box = { x: number; y: number; w: number; h: number };
/** The page as these tests lay it out; every other element is a 0×0 box. */
const layout = {
  viewport: { w: 1000, h: 800 },
  trigger: { x: 0, y: 0, w: 0, h: 0 } as Box,
  card: null as Box | null,
  /** The list's natural size, before its max-height. */
  list: { w: 160, h: 130 },
};

const rect = ({ x, y, w, h }: Box) =>
  ({ x, y, left: x, top: y, width: w, height: h, right: x + w, bottom: y + h, toJSON: () => ({}) }) as DOMRect;
const isPositioner = (el: Element) => !!el.querySelector(':scope > [role="listbox"]');
const px = (value: string) => (value.trim() === "" ? Infinity : parseFloat(value));

const Html = window.HTMLElement.prototype;
Object.defineProperty(window.Element.prototype, "getBoundingClientRect", {
  configurable: true,
  value(this: Element) {
    if (this.getAttribute("role") === "combobox") return rect(layout.trigger);
    if (this.hasAttribute("data-composer") && layout.card) return rect(layout.card);
    return rect({ x: 0, y: 0, w: 0, h: 0 });
  },
});
Object.defineProperty(Html, "offsetWidth", {
  configurable: true,
  get(this: HTMLElement) {
    return isPositioner(this) ? layout.list.w : 0;
  },
});
Object.defineProperty(Html, "offsetHeight", {
  configurable: true,
  get(this: HTMLElement) {
    if (!isPositioner(this)) return 0;
    // The popup's `max-h-[min(var(--available-height),20rem)]`.
    return Math.min(layout.list.h, px(this.style.getPropertyValue("--available-height")), 320);
  },
});
for (const [key, read] of [
  ["clientWidth", () => layout.viewport.w],
  ["clientHeight", () => layout.viewport.h],
] as const) {
  Object.defineProperty(document.documentElement, key, { configurable: true, get: read });
}
for (const [key, read] of [
  ["innerWidth", () => layout.viewport.w],
  ["innerHeight", () => layout.viewport.h],
] as const) {
  Object.defineProperty(window, key, { configurable: true, get: read });
}

const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { I18nextProvider } = await import("react-i18next");
const { SelectField } = await import("../src/components/ui/select.js");
const { ComposerSelect } = await import("../src/features/sessions/composer-controls.js");

const i18n = makeI18n({ lng: "en", resources: { en: { translation: {} } } });

let root: ReturnType<typeof createRoot> | null = null;
afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  document.body.querySelectorAll("[data-base-ui-portal], [data-base-ui-floating-portal]").forEach((n) => n.remove());
  layout.card = null;
});

const settle = () => act(async () => void (await new Promise((r) => setTimeout(r, 30))));

const EFFORTS = ["low", "medium", "high", "xhigh", "max"];

/** Render `node`, open its select with a mouse press and say where the list landed. */
async function open(node: React.ReactNode): Promise<{ list: Box; side: string | null; align: string | null; popup: HTMLElement }> {
  root = createRoot(container);
  await act(async () => root!.render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>));
  const trigger = container.querySelector<HTMLElement>('[role="combobox"]');
  assert.ok(trigger, "the trigger is on the page");
  await act(async () => {
    trigger.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, cancelable: true, pointerType: "mouse", button: 0 }));
    trigger.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 }));
  });
  await settle();
  await act(async () => {
    trigger.dispatchEvent(new PointerEvent("pointerup", { bubbles: true, cancelable: true, pointerType: "mouse", button: 0 }));
    trigger.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, cancelable: true, button: 0 }));
    trigger.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
  });
  // Floating UI measures, flips, resizes and measures again.
  await settle();
  await settle();
  const popup = [...document.querySelectorAll<HTMLElement>('[role="listbox"]')].find((l) => !l.closest("[hidden]"));
  assert.ok(popup, "the list is open");
  const positioner = popup.parentElement!;
  const style = positioner.style;
  const moved = /translate\((-?[\d.]+)px,\s*(-?[\d.]+)px\)/.exec(style.transform);
  const x = (moved ? parseFloat(moved[1]) : 0) + (parseFloat(style.left) || 0);
  const y = (moved ? parseFloat(moved[2]) : 0) + (parseFloat(style.top) || 0);
  return {
    list: { x, y, w: positioner.offsetWidth, h: positioner.offsetHeight },
    side: positioner.getAttribute("data-side"),
    align: positioner.getAttribute("data-align"),
    popup,
  };
}

/** The composer's effort chip, inside the composer card. */
const chip = (
  <div data-composer="">
    <ComposerSelect ghost label="effort" value="xhigh" options={EFFORTS} onChange={() => {}} />
  </div>
);

const overlaps = (a: Box, b: Box) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;

test("a chip at the bottom of the composer opens its list right above the chip, not above the whole card", async () => {
  layout.viewport = { w: 1000, h: 800 };
  layout.card = { x: 60, y: 680, w: 900, h: 100 };
  layout.trigger = { x: 100, y: 748, w: 80, h: 24 };
  layout.list = { w: 160, h: 130 };
  const { list, side } = await open(chip);
  assert.equal(side, "top", "no room below: flipped above");
  assert.equal(list.y + list.h, layout.trigger.y - 4, "4px above the chip");
  assert.notEqual(list.y + list.h, layout.card.y - 4, "not lifted past the card");
  assert.equal(list.x, layout.trigger.x, "left edges together");
  assert.ok(!overlaps(list, layout.trigger), "the chip stays visible");
});

test("with room below, the list hangs 4px under its trigger, left edges together", async () => {
  layout.viewport = { w: 1000, h: 800 };
  layout.trigger = { x: 200, y: 120, w: 180, h: 26 };
  layout.list = { w: 200, h: 130 };
  const { list, side } = await open(<SelectField aria-label="effort" value="xhigh" options={EFFORTS.map((v) => ({ value: v, label: v }))} onValueChange={() => {}} />);
  assert.equal(side, "bottom");
  assert.equal(list.y, layout.trigger.y + layout.trigger.h + 4);
  assert.equal(list.x, layout.trigger.x);
});

test("a composer high on the page (the new-session view) opens the chip's list downward, against the chip", async () => {
  layout.viewport = { w: 1000, h: 800 };
  layout.card = { x: 60, y: 300, w: 900, h: 100 };
  layout.trigger = { x: 100, y: 368, w: 80, h: 24 };
  layout.list = { w: 160, h: 130 };
  const { list, side } = await open(chip);
  assert.equal(side, "bottom");
  assert.equal(list.y, layout.trigger.y + layout.trigger.h + 4, "under the chip, inside the card's span");
  assert.equal(list.x, layout.trigger.x);
});

test("near the right edge the list lines up with the trigger's right edge instead of leaving the screen", async () => {
  layout.viewport = { w: 1000, h: 800 };
  layout.trigger = { x: 900, y: 120, w: 80, h: 24 };
  layout.list = { w: 160, h: 130 };
  const { list, align } = await open(chip);
  assert.equal(align, "end");
  assert.equal(list.x + list.w, layout.trigger.x + layout.trigger.w);
  assert.ok(list.x + list.w <= layout.viewport.w - 8, "inside the 8px margin");
});

test("a long list in a short window takes the roomier side, stops at the screen's margin and scrolls inside", async () => {
  layout.viewport = { w: 1000, h: 300 };
  layout.trigger = { x: 100, y: 250, w: 80, h: 24 };
  layout.list = { w: 160, h: 31 * 24 + 10 };
  const { list, side, popup } = await open(chip);
  assert.equal(side, "top");
  assert.equal(list.y + list.h, layout.trigger.y - 4, "still against the trigger");
  assert.ok(list.y >= 8, "not past the top margin");
  assert.ok(list.h < layout.list.h, "cut to the room there is");
  assert.match(popup.className, /max-h-\[min\(var\(--available-height\),20rem\)\]/, "capped by the room the positioner found");
  assert.match(popup.className, /overflow-y-auto/, "the rest scrolls inside the list");
  assert.ok(!overlaps(list, layout.trigger));
});

test("the list is the menus' sheet: hairline frame, no shadow, square, 32px rows set in the trigger's font", async () => {
  layout.viewport = { w: 1000, h: 800 };
  layout.trigger = { x: 200, y: 120, w: 180, h: 26 };
  layout.list = { w: 200, h: 130 };
  const { popup } = await open(chip);
  assert.match(popup.className, /\bborder\b/);
  assert.match(popup.className, /border-border-strong/);
  assert.match(popup.className, /\bbg-bg\b/);
  assert.match(popup.className, /\bpy-1\.5\b/, "6px above the first row");
  assert.match(popup.className, /font-mono/, "the chip's mono");
  assert.match(popup.className, /min-w-\[max\(var\(--anchor-width\),10rem\)\]/, "never a sliver under a short chip");
  assert.match(popup.className, /max-w-\[min\(var\(--available-width\),20rem\)\]/, "never wider than 20rem");
  assert.doesNotMatch(popup.className, /shadow|rounded/);
  const row = popup.querySelector<HTMLElement>('[role="option"]')!;
  assert.match(row.className, /\bmin-h-8\b/, "32px rows");
  assert.match(row.className, /\bpx-3\b/, "text 12px in, under a field's own text");
  assert.match(row.className, /pointer-coarse:min-h-11/, "44px under a finger");
});
