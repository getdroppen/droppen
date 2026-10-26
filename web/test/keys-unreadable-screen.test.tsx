// "This browser could not open its device keys" (`features/cloud/cloud-app.tsx`
// `KeysUnreadableScreen`), driven in a real DOM: using the browser as a new
// device is asked about in a question of its own — a double tap on the button
// that opens it never reaches the confirm button, the question takes the
// focus and can be cancelled — and the screen says what it knows: keys still
// stored only when they are, and "use as a new device" first when the keys
// will never open again.
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

const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { I18nextProvider } = await import("react-i18next");
const { default: i18n } = await import("./i18n-all.js");
const { KeysUnreadableScreen } = await import("../src/features/cloud/cloud-app.js");

type Root = ReturnType<typeof createRoot>;
let root: Root | null = null;
afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  await i18n.changeLanguage("en");
});

async function render(node: React.ReactNode): Promise<void> {
  root = createRoot(container);
  await act(async () => root!.render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>));
}

const byTestId = (id: string) => container.querySelector<HTMLElement>(`[data-testid="${id}"]`);
const click = (el: HTMLElement | null) =>
  act(async () => {
    assert.ok(el, "the control is on the page");
    el.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
  });

test("a double tap on “use this browser as a new device” asks; only the question's own button replaces the keys", async () => {
  await i18n.changeLanguage("en");
  let replaced = 0;
  let retried = 0;
  await render(
    <KeysUnreadableScreen reason="stored" message="m" onRetry={() => void retried++} onNewDevice={() => void replaced++} />,
  );
  const ask = byTestId("keys-new-device");
  await click(ask);
  await click(ask);
  assert.equal(replaced, 0, "two taps on the same spot replace nothing");
  const question = byTestId("keys-new-device-confirm");
  assert.ok(question, "the question is open");
  assert.equal(question.getAttribute("role"), "alertdialog");
  assert.equal(document.activeElement, question, "the question takes the focus");
  const confirm = byTestId("keys-new-device-confirm-button");
  assert.ok(confirm && confirm !== ask, "a button of its own");
  assert.match(confirm.className, /danger/);
  // It is described by its warning.
  const described = question.getAttribute("aria-describedby");
  assert.ok(described, "aria-describedby is set");
  assert.match(document.getElementById(described)?.textContent ?? "", /replaces these keys for good/);
  // Cancel closes it, gives the focus back to the button that opened it, and nothing was replaced.
  const cancel = [...question.querySelectorAll("button")].find((b) => b.textContent === "cancel") as HTMLElement;
  await click(cancel);
  assert.equal(byTestId("keys-new-device-confirm"), null);
  assert.equal(document.activeElement, byTestId("keys-new-device"), "the focus is back on the button");
  assert.equal(replaced, 0);
  // Asked again and confirmed: replaced, once.
  await click(byTestId("keys-new-device"));
  await click(byTestId("keys-new-device-confirm-button"));
  assert.equal(replaced, 1);
  assert.equal(retried, 0);
});

test("the screen says what it knows: still stored, not answering, or gone for good (and then a new device comes first)", async () => {
  await i18n.changeLanguage("en");
  await render(<KeysUnreadableScreen reason="stored" message="m" onRetry={() => undefined} onNewDevice={() => undefined} />);
  assert.match(byTestId("keys-unreadable-why")!.textContent!, /still stored in this browser/);
  assert.equal(byTestId("keys-retry")!.getAttribute("data-variant"), "primary", "trying again comes first");
  await act(async () => root?.unmount());

  root = null;
  await render(<KeysUnreadableScreen reason="store" message="m" onRetry={() => undefined} onNewDevice={() => undefined} />);
  const store = byTestId("keys-unreadable-why")!.textContent!;
  assert.match(store, /storage did not answer/);
  assert.doesNotMatch(store, /still stored/, "not known to be stored: not said");
  await act(async () => root?.unmount());

  root = null;
  await i18n.changeLanguage("zh-CN");
  await render(<KeysUnreadableScreen reason="gone" message="m" onRetry={() => undefined} onNewDevice={() => undefined} />);
  assert.match(byTestId("keys-unreadable-why")!.textContent!, /不会再回来/);
  assert.equal(byTestId("keys-new-device")!.getAttribute("data-variant"), "primary", "a new device comes first");
  assert.notEqual(byTestId("keys-retry")!.getAttribute("data-variant"), "primary");
  assert.ok(byTestId("keys-retry"), "trying again stays");
});
