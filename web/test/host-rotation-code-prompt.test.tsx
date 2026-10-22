// The code prompt of a person's renewal (`features/cloud/renewal.tsx`
// `RenewPanel`, docs/host-identity-rotation.md §3.7), in a real DOM: the panel
// closing while it waits for the code — another row's menu opened, say —
// gives the prompt up, so the renewal ends (cancelled) and the renewal
// runner, which does one thing at a time and checks the hub's disputes in the
// same queue, goes on. Typed or cancelled, the prompt answers once.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after, afterEach } from "node:test";
import { installJsdom } from "./jsdom.js";

const { window, container } = installJsdom();
after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});

const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { I18nextProvider } = await import("react-i18next");
const { default: i18n } = await import("../src/i18n/index.js");
const { RenewPanel } = await import("../src/features/cloud/renewal.js");
const { RenewalError } = await import("../src/lib/hub/account.js");
type RenewalRunner = import("../src/lib/hub/renewal.js").RenewalRunner;

type Root = ReturnType<typeof createRoot>;
let root: Root | null = null;
afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
});

const byTestId = (id: string) => container.querySelector<HTMLElement>(`[data-testid="${id}"]`);
const click = (el: HTMLElement | null) =>
  act(async () => {
    assert.ok(el, "the control is on the page");
    el.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
  });

/** A runner that asks for the code as the real one does, and notes each answer and how the renewal ended. */
function runnerAsking() {
  const answers: (string | null)[] = [];
  const ended: string[] = [];
  const runner: Pick<RenewalRunner, "renew"> = {
    renew: async (host, how) => {
      try {
        const code = await how.askCode!(host);
        answers.push(code);
        if (code === null) throw new RenewalError("cancelled", "no code was typed");
        ended.push("renewed");
      } catch (error) {
        ended.push(error instanceof RenewalError ? error.kind : String(error));
        throw error;
      }
    },
  };
  return { runner, answers, ended };
}

async function within(what: string, check: () => boolean, ms = 1_000): Promise<void> {
  const deadline = Date.now() + ms;
  while (!check()) {
    if (Date.now() > deadline) assert.fail(`timed out waiting for ${what}`);
    await act(async () => new Promise((resolve) => setTimeout(resolve, 5)));
  }
}

async function openPrompt(runner: Pick<RenewalRunner, "renew">): Promise<void> {
  root = createRoot(container);
  await act(async () =>
    root!.render(
      <I18nextProvider i18n={i18n}>
        <RenewPanel host="m-1" runner={runner} fixed onDone={() => undefined} />
      </I18nextProvider>,
    ),
  );
  await click(byTestId("renew-start-m-1"));
  await within("the code prompt", () => !!byTestId("renew-code-m-1"));
}

test("the panel closed while it waits for the code gives the prompt up: the renewal ends, cancelled", async () => {
  const { runner, answers, ended } = runnerAsking();
  await openPrompt(runner);
  assert.deepEqual(answers, [], "waiting for the code");
  // Another row's menu opens: this panel goes.
  await act(async () => root!.unmount());
  root = null;
  await within("the prompt to be given up", () => ended.length > 0);
  assert.deepEqual(answers, [null]);
  assert.deepEqual(ended, ["cancelled"]);
});

test("cancel in the prompt gives it up once; the panel closing after that answers nothing more", async () => {
  const { runner, answers, ended } = runnerAsking();
  await openPrompt(runner);
  const cancel = [...byTestId("renew-code-m-1")!.querySelectorAll("button")].find((b) => b.textContent === "cancel") ?? null;
  await click(cancel);
  await within("the renewal to end", () => ended.length > 0);
  assert.equal(byTestId("renew-code-m-1"), null, "the prompt is gone");
  await act(async () => root!.unmount());
  root = null;
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.deepEqual(answers, [null]);
  assert.deepEqual(ended, ["cancelled"]);
});
