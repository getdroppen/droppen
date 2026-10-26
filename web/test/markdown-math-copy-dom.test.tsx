// A typeset formula copies its TeX when clicked (getdroppen/droppen#167).
//
// What is pinned here: a typeset formula is a focusable button carrying its
// TeX; a click anywhere in it, or Enter or Space on it, writes exactly that
// TeX (no delimiters) to the clipboard and says "copied" on the formula for
// 1.5s; a click that ends a text selection, a formula KaTeX could not read and
// a message drawn with the setting off copy nothing. A real DOM (jsdom),
// because this is about events.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after, afterEach } from "node:test";
import { installJsdom } from "./jsdom.js";

const { window, document, container } = installJsdom();
after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});

const written: string[] = [];
Object.defineProperty(window.navigator, "clipboard", {
  configurable: true,
  value: { writeText: async (text: string) => void written.push(text) },
});

const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { default: i18next } = await import("i18next");
const { I18nextProvider, initReactI18next } = await import("react-i18next");
const { installMath, Markdown, MathRendering } = await import("../src/components/markdown.js");
installMath(await import("../src/components/markdown-math.js"));

const i18n = i18next.createInstance();
await i18n.use(initReactI18next).init({
  lng: "en",
  resources: { en: { translation: {} } },
  keySeparator: false,
  nsSeparator: false,
  interpolation: { escapeValue: false },
});

let root: ReturnType<typeof createRoot> | null = null;
afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  written.length = 0;
  window.getSelection()?.removeAllRanges();
});

async function render(text: string, on = true) {
  root = createRoot(container);
  await act(async () =>
    root!.render(
      <I18nextProvider i18n={i18n}>
        <MathRendering.Provider value={on}>
          <Markdown text={text} />
        </MathRendering.Provider>
      </I18nextProvider>,
    ),
  );
}

/** Dispatch `event` on `el` and let the clipboard's promise settle, without a timer. */
async function fire(el: Element, event: Event) {
  await act(async () => {
    el.dispatchEvent(event);
    for (let i = 0; i < 5; i++) await Promise.resolve();
  });
  return event;
}
const click = (el: Element) => fire(el, new window.MouseEvent("click", { bubbles: true, cancelable: true }));
const key = (el: Element, k: string) => fire(el, new window.KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));

const formulas = () => [...container.querySelectorAll<HTMLElement>("[data-tex]")];

test("a typeset formula is a focusable button that carries its TeX", async () => {
  await render("energy $E = mc^2$ and\n\n$$\n\\sum_i x_i\n$$");
  const [inline, display] = formulas();
  assert.equal(inline?.dataset.tex, "E = mc^2");
  assert.equal(display?.dataset.tex, "\\sum_i x_i");
  for (const formula of [inline!, display!]) {
    assert.equal(formula.getAttribute("role"), "button");
    assert.equal(formula.tabIndex, 0);
    assert.equal(formula.title, "click to copy the TeX");
  }
});

test("a click anywhere in it copies the TeX, without delimiters, and says so for 1.5s", async (t) => {
  await render("energy $E = mc^2$ here");
  const [formula] = formulas();
  t.mock.timers.enable({ apis: ["setTimeout"] });
  // The innermost glyph, as a real click would land.
  const glyph = formula!.querySelector(".katex-html .mord") ?? formula!;
  await click(glyph);
  assert.deepEqual(written, ["E = mc^2"]);
  assert.equal(formula!.dataset.copied, "copied");
  t.mock.timers.tick(1499);
  assert.equal(formula!.dataset.copied, "copied");
  t.mock.timers.tick(1);
  assert.equal(formula!.dataset.copied, undefined, "and then no longer");
});

test("Enter or Space on a focused formula copies it; other keys do not", async () => {
  await render("$$\n\\frac{1}{k^2}\n$$");
  const [formula] = formulas();
  await key(formula!, "a");
  assert.deepEqual(written, []);
  await key(formula!, "Enter");
  await key(formula!, " ");
  assert.deepEqual(written, ["\\frac{1}{k^2}", "\\frac{1}{k^2}"]);
});

test("a click that ends a text selection is someone selecting: nothing is copied", async () => {
  await render("energy $E = mc^2$ here");
  const [formula] = formulas();
  window.getSelection()!.selectAllChildren(formula!);
  await click(formula!);
  assert.deepEqual(written, []);
});

test("a formula in a link's text copies instead of following the link", async () => {
  await render("[the identity $e^{i\\pi} = -1$](https://example.com)");
  const [formula] = formulas();
  const event = await click(formula!);
  assert.deepEqual(written, ["e^{i\\pi} = -1"]);
  assert.equal(event.defaultPrevented, true);
});

test("a formula KaTeX could not read, and a message with the setting off, are not buttons", async () => {
  await render("see $\\notamacro{x}$ here");
  assert.equal(formulas().length, 0);
  await click(container.querySelector(".math-error")!);
  assert.deepEqual(written, []);
  await act(async () => root?.unmount());

  await render("energy $E = mc^2$ here", false);
  assert.equal(formulas().length, 0);
  await click(container.querySelector("p")!);
  assert.deepEqual(written, []);
});

test("nothing in a message without math is touched by a click", async () => {
  await render("plain **text** with `code`");
  const event = await click(container.querySelector("strong")!);
  assert.equal(event.defaultPrevented, false);
  assert.deepEqual(written, []);
  assert.equal(document.querySelectorAll("[data-copied]").length, 0);
});
