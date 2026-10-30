// Security-page navigation in a real DOM: a late font swap must not pull the
// reader back to the initial hash after they navigate or scroll, and the mobile
// contents sheet must leave focus at a chosen chapter, except when dismissed.
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

const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { I18nextProvider } = await import("react-i18next");
const { default: SecurityPage } = await import("../src/features/landing/security/security.js");

const i18n = makeI18n({ lng: "en", resources: { en: { translation: {} } }, keySeparator: false, nsSeparator: false });
type Root = ReturnType<typeof createRoot>;
let root: Root | null = null;
const scrolls: string[] = [];
HTMLElement.prototype.scrollIntoView = function () {
  if (this.id) scrolls.push(this.id);
};

const settle = (ms = 30) => act(async () => void (await new Promise((resolve) => setTimeout(resolve, ms))));

async function renderPage(): Promise<void> {
  root = createRoot(container);
  await act(async () => root!.render(<I18nextProvider i18n={i18n}><SecurityPage /></I18nextProvider>));
  await settle();
}

/** Hold the font swap until the test lets it finish. */
function holdFonts(): () => Promise<void> {
  let release!: () => void;
  const ready = new Promise<void>((resolve) => {
    release = resolve;
  });
  Object.defineProperty(document, "fonts", { configurable: true, value: { ready } });
  return async () => {
    release();
    await ready;
  };
}

async function renderAtInitialHash(): Promise<() => Promise<void>> {
  scrolls.length = 0;
  window.history.replaceState(null, "", "/security#renewal");
  const finishFonts = holdFonts();
  await renderPage();
  assert.deepEqual(scrolls, ["renewal"], "the initial hash scrolls once the page mounts");
  return finishFonts;
}

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  document.body.querySelectorAll("[data-base-ui-portal], [data-base-ui-floating-portal]").forEach((node) => node.remove());
  window.history.replaceState(null, "", "/security");
});

test("a late font swap does not scroll to the initial hash after navigation", async () => {
  const finishFonts = await renderAtInitialHash();
  window.history.replaceState(null, "", "/security#keys");
  await act(finishFonts);
  assert.deepEqual(scrolls, ["renewal"], "the delayed correction must not overwrite the newer #keys location");
});

for (const [interaction, event] of [
  ["wheel", () => new window.Event("wheel", { bubbles: true })],
  ["touch", () => new window.Event("touchstart", { bubbles: true })],
  ["keyboard", () => new window.KeyboardEvent("keydown", { key: "PageDown", bubbles: true })],
] as const) {
  test(`a late font swap is cancelled by ${interaction} even when the hash is unchanged`, async () => {
    const finishFonts = await renderAtInitialHash();
    await act(async () => {
      document.querySelector('[data-testid="security"]')!.dispatchEvent(event());
    });
    await act(finishFonts);
    assert.deepEqual(scrolls, ["renewal"], "the reader's own scrolling wins over the delayed correction");
  });
}

test("a late font swap still corrects the initial hash without interaction", async () => {
  const finishFonts = await renderAtInitialHash();
  await act(finishFonts);
  assert.deepEqual(scrolls, ["renewal", "renewal"]);
});

async function press(element: HTMLElement): Promise<void> {
  await act(async () => {
    element.dispatchEvent(new window.PointerEvent("pointerdown", { bubbles: true, cancelable: true, pointerType: "mouse", button: 0 }));
    element.dispatchEvent(new window.MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 }));
  });
  await act(async () => {
    element.dispatchEvent(new window.PointerEvent("pointerup", { bubbles: true, cancelable: true, pointerType: "mouse", button: 0 }));
    element.dispatchEvent(new window.MouseEvent("mouseup", { bubbles: true, cancelable: true, button: 0 }));
    element.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
  });
  await settle();
}

async function key(name: string): Promise<void> {
  const target = (document.activeElement as HTMLElement | null) ?? document.body;
  await act(async () => {
    target.dispatchEvent(new window.KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true }));
    target.dispatchEvent(new window.KeyboardEvent("keyup", { key: name, bubbles: true, cancelable: true }));
  });
  await settle();
}

const trigger = () => document.querySelector<HTMLElement>('[data-testid="security-toc-trigger"]')!;

async function openSheet(): Promise<HTMLElement> {
  const button = trigger();
  button.focus();
  await press(button);
  assert.ok(document.querySelector('[data-testid="security-contents-sheet"]'), "the contents sheet is open");
  return button;
}

test("choosing a chapter in the contents sheet leaves focus on that section", async () => {
  window.history.replaceState(null, "", "/security");
  await renderPage();
  const button = await openSheet();
  const keys = document.querySelector<HTMLElement>('[data-testid="security-contents-sheet"] a[href="#keys"]');
  assert.ok(keys, "the sheet links to Keys");
  await press(keys);
  assert.ok(document.activeElement === document.getElementById("keys"), "focus must be on Keys, not the contents trigger");
  assert.ok(document.activeElement !== button);
});

test("Escape and the close button still restore focus to the contents trigger", async () => {
  window.history.replaceState(null, "", "/security");
  await renderPage();
  let button = await openSheet();
  await key("Escape");
  assert.ok(document.activeElement === button, "Escape restores the trigger");

  button = await openSheet();
  const close = document.querySelector<HTMLElement>('[role="dialog"] button[aria-label="close"]');
  assert.ok(close, "the sheet has a close button");
  await press(close);
  assert.ok(document.activeElement === button, "the close button restores the trigger");
});
