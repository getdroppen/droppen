// The public header's account menu, driven in a real DOM: the way in when
// signed out (sign in, create an account), the way back when signed in, and
// the keyboard path through both. The menu is the header's only control with
// a popup, so it is where a public page's focus handling is pinned.
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
const { PublicHeader } = await import("../src/features/landing/page-kit.js");

const i18n = makeI18n({ lng: "en", resources: { en: { translation: {} } } });
type Root = ReturnType<typeof createRoot>;
let root: Root | null = null;

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  document.body.querySelectorAll("[data-base-ui-portal], [data-base-ui-floating-portal]").forEach((n) => n.remove());
});

async function render(signedIn: boolean): Promise<void> {
  root = createRoot(container);
  await act(async () => root!.render(<I18nextProvider i18n={i18n}><PublicHeader signedIn={signedIn} /></I18nextProvider>));
}

const settle = () => act(async () => void (await new Promise((r) => setTimeout(r, 20))));

const trigger = (name: string): HTMLElement => {
  const found = container.querySelector<HTMLElement>(`button[aria-label="${name}"]`);
  assert.ok(found, `the ${name} trigger is on the page`);
  return found;
};

/** The menu's items that are not inside a hidden positioner. */
const items = (): HTMLElement[] =>
  [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].filter((el) => !el.closest("[hidden]"));

const item = (text: string): HTMLElement => {
  const found = items().find((el) => el.textContent === text);
  assert.ok(found, `a menu item says ${text}`);
  return found;
};

/** A mouse press and release on `el`, as a browser sends them. */
async function press(el: HTMLElement): Promise<void> {
  await act(async () => {
    el.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, cancelable: true, pointerType: "mouse", button: 0 }));
    el.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 }));
  });
  await settle();
  await act(async () => {
    el.dispatchEvent(new PointerEvent("pointerup", { bubbles: true, cancelable: true, pointerType: "mouse", button: 0 }));
    el.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, cancelable: true, button: 0 }));
    el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
  });
  await settle();
}

/** A key pressed on whatever has focus. */
async function key(name: string): Promise<void> {
  const target = (document.activeElement as HTMLElement | null) ?? document.body;
  await act(async () => {
    target.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true }));
  });
  await act(async () => {
    target.dispatchEvent(new KeyboardEvent("keyup", { key: name, bubbles: true, cancelable: true }));
  });
  await settle();
}

test("signed out: the menu offers sign-in and sign-up, and closes on Escape", async () => {
  await render(false);
  const button = trigger("Sign in");
  assert.equal(button.getAttribute("aria-expanded"), "false");

  await press(button);
  assert.equal(button.getAttribute("aria-expanded"), "true");
  assert.equal(item("Sign in").getAttribute("href"), "/signin");
  assert.equal(item("Create a free account").getAttribute("href"), "/signup");

  await key("Escape");
  assert.equal(button.getAttribute("aria-expanded"), "false");
  assert.equal(items().length, 0, "the popup is gone");
});

test("signed out: the keyboard opens the menu and moves through its items", async () => {
  await render(false);
  const button = trigger("Sign in");
  button.focus();
  await key("ArrowDown");
  assert.equal(button.getAttribute("aria-expanded"), "true");

  const focused: string[] = [];
  const record = () => {
    const text = document.activeElement?.textContent?.trim();
    if (text && !focused.includes(text)) focused.push(text);
  };
  record();
  for (let i = 0; i < 4 && focused.length < 2; i++) {
    await key("ArrowDown");
    record();
  }
  assert.deepEqual(focused.slice(0, 2), ["Sign in", "Create a free account"]);
  await key("Escape");
  assert.equal(document.activeElement, button, "Escape returns focus to the trigger");
});

test("signed in: the menu has one item, back into the app", async () => {
  await render(true);
  const button = trigger("Open app");
  await press(button);
  assert.equal(item("Open app").getAttribute("href"), "/sessions");
  assert.equal(items().length, 1, "no sign-in or sign-up once signed in");
  assert.equal(document.querySelector('[role="menuitem"][href="/signin"]'), null);
});
