// The sign-out question (`features/cloud/sign-out-dialog.tsx`), driven in a
// real DOM: both ways are offered; the line under "remove this browser" says
// plainly what it does here — for the account's last full device, whether the
// account has recovery words at all — and for that device a second, explicit
// confirmation is needed before anything is deleted. A screen that could not
// open the account offers the removal too, as a local wipe.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after, afterEach } from "node:test";
import { installJsdom } from "./jsdom.js";

const { window, document } = installJsdom();
after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});

const replaced: string[] = [];
(globalThis as { location?: unknown }).location = { replace: (url: string) => void replaced.push(url), pathname: "/account", search: "", hash: "", origin: "http://localhost" };

const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { I18nextProvider } = await import("react-i18next");
const { default: i18n } = await import("../src/i18n/index.js");
const { MemoryKeyStore } = await import("../src/lib/e2e/index.js");
const { SignOutDialog } = await import("../src/features/cloud/sign-out-dialog.js");
type Account = import("../src/lib/hub/account.js").CloudAccount;

type Root = ReturnType<typeof createRoot>;
let root: Root | null = null;
const container = document.body.appendChild(document.createElement("div"));
afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
});

async function render(node: React.ReactNode): Promise<void> {
  root = createRoot(container);
  await act(async () => root!.render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>));
}

// The dialog is portalled into the body.
const byTestId = (id: string) => document.querySelector<HTMLElement>(`[data-testid="${id}"]`);
const click = (el: HTMLElement | null) =>
  act(async () => {
    assert.ok(el, "the control is on the page");
    el.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
  });
const settle = () => act(async () => new Promise((resolve) => setTimeout(resolve, 20)));

/** An account as far as the question and a sign-out read it. */
function fakeAccount(why: string, words: boolean) {
  const calls: string[] = [];
  const account = {
    selfRemoval: () => why,
    standing: "active",
    chain: { currentRecoveryKey: words ? new Uint8Array(32) : null },
    removeSelf: async () => void calls.push("removeSelf"),
    wipe: async () => void calls.push("wipe"),
    stopSession: () => undefined,
    store: new MemoryKeyStore(),
    api: { logout: async () => void calls.push("logout") },
    session: { token: "t" },
  };
  return { account: account as unknown as Account, calls };
}

test("the last full device: said plainly when the account has no recovery words, and deleted only on a second, explicit confirmation", async () => {
  await i18n.changeLanguage("en");
  replaced.length = 0;
  const { account, calls } = fakeAccount("lastAdmin", false);
  await render(<SignOutDialog account={account} onOpenChange={() => undefined} />);
  assert.ok(byTestId("sign-out-keep") && byTestId("sign-out-remove"), "both ways are offered");
  assert.match(byTestId("sign-out-remove-why")!.textContent ?? "", /The account has no recovery words/);
  await click(byTestId("sign-out-remove"));
  await settle();
  assert.deepEqual(calls, [], "the first click deletes nothing");
  assert.deepEqual(replaced, []);
  const confirm = byTestId("sign-out-remove-confirm");
  assert.ok(confirm, "it asks again");
  assert.match(confirm.textContent ?? "", /last full device, and the account has no recovery words/);
  // Cancelled: the question goes, nothing happened.
  const cancel = [...confirm.querySelectorAll("button")].find((b) => b.textContent === "cancel") as HTMLElement;
  await click(cancel);
  assert.equal(byTestId("sign-out-remove-confirm"), null);
  assert.deepEqual(calls, []);
  // Asked again and confirmed: wiped (never taken off: it is the last), then the sign-in page.
  await click(byTestId("sign-out-remove"));
  await click(byTestId("sign-out-remove-anyway"));
  for (let i = 0; i < 50 && !replaced.length; i++) await settle();
  assert.deepEqual(calls.filter((c) => c !== "logout"), ["wipe"]);
  assert.deepEqual(replaced, ["/signin"]);
});

test("with recovery words the last full device's line says they bring a full device back; any other device is removed on one click", async () => {
  await i18n.changeLanguage("en");
  const last = fakeAccount("lastAdmin", true);
  await render(<SignOutDialog account={last.account} onOpenChange={() => undefined} />);
  assert.match(byTestId("sign-out-remove-why")!.textContent ?? "", /only your recovery words bring back a full device/);
  await act(async () => root?.unmount());
  root = null;
  replaced.length = 0;
  const other = fakeAccount("removable", true);
  await render(<SignOutDialog account={other.account} onOpenChange={() => undefined} />);
  await click(byTestId("sign-out-remove"));
  for (let i = 0; i < 50 && !replaced.length; i++) await settle();
  assert.equal(byTestId("sign-out-remove-confirm"), null, "no second question");
  assert.deepEqual(other.calls.slice(0, 2), ["removeSelf", "wipe"]);
  assert.deepEqual(replaced, ["/signin"]);
});

test("a screen that could not open the account offers the removal too, as a local wipe", async () => {
  await i18n.changeLanguage("en");
  const held = { api: { logout: async () => undefined }, token: "t", account: "a-1", store: new MemoryKeyStore() };
  await render(<SignOutDialog held={held} onOpenChange={() => undefined} />);
  assert.ok(byTestId("sign-out-remove"));
  assert.equal(byTestId("sign-out-remove-why")!.getAttribute("data-why"), "unopened");
  assert.match(byTestId("sign-out-remove-why")!.textContent ?? "", /The account could not be opened, so it stays on your account's devices/);
});
