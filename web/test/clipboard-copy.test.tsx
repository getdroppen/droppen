// `lib/clipboard.ts`'s `useCopy`: the one "copied" state the copy controls
// share, and its promise to the caller about failures.
//
// What is pinned here: a copy whose Clipboard API is missing (an insecure
// context), has no `writeText`, throws synchronously, or rejects calls
// `onError` — the files browser's "cannot copy path" notice and its focus on
// the address bar depend on it — while a successful write says `copied` and
// calls `onSuccess`. A real DOM (jsdom), because this is a click and a hook.
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
const { useCopy } = await import("../src/lib/clipboard.js");

/** Put `value` behind `navigator.clipboard` (or remove it, with `undefined`). */
const setClipboard = (value: unknown) =>
  Object.defineProperty(window.navigator, "clipboard", { configurable: true, value });

function Probe({ onSuccess, onError }: { onSuccess?: () => void; onError?: () => void }) {
  const { copied, copy } = useCopy({ onSuccess, onError });
  return (
    <button type="button" data-copied={copied ? "yes" : "no"} onClick={() => copy("value")}>
      copy
    </button>
  );
}

let root: ReturnType<typeof createRoot> | null = null;
afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  setClipboard(undefined);
});

async function mount(props: { onSuccess?: () => void; onError?: () => void }) {
  root = createRoot(container);
  await act(async () => root!.render(<Probe {...props} />));
  return container.querySelector("button")!;
}

/** Click the probe and let a settled clipboard promise run its handlers. */
async function click(button: HTMLButtonElement) {
  await act(async () => {
    button.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
    for (let i = 0; i < 5; i++) await Promise.resolve();
  });
  return button;
}

test("with no Clipboard API, the copy reports the failure and does not claim to have copied", async () => {
  setClipboard(undefined);
  let errors = 0;
  const button = await mount({ onError: () => errors++ });
  await click(button);
  assert.equal(errors, 1);
  assert.equal(button.dataset.copied, "no");
});

test("a clipboard without writeText is the same failure", async () => {
  setClipboard({});
  let errors = 0;
  const button = await mount({ onError: () => errors++ });
  await click(button);
  assert.equal(errors, 1);
  assert.equal(button.dataset.copied, "no");
});

test("a writeText that throws synchronously reports the failure", async () => {
  setClipboard({
    writeText: () => {
      throw new Error("denied");
    },
  });
  let errors = 0;
  const button = await mount({ onError: () => errors++ });
  await click(button);
  assert.equal(errors, 1);
  assert.equal(button.dataset.copied, "no");
});

test("a writeText that rejects reports the failure", async () => {
  setClipboard({ writeText: () => Promise.reject(new Error("denied")) });
  let errors = 0;
  const button = await mount({ onError: () => errors++ });
  await click(button);
  assert.equal(errors, 1);
  assert.equal(button.dataset.copied, "no");
});

test("a successful write says copied and calls onSuccess", async () => {
  const written: string[] = [];
  setClipboard({ writeText: async (text: string) => void written.push(text) });
  let successes = 0;
  const button = await mount({ onSuccess: () => successes++ });
  await click(button);
  assert.deepEqual(written, ["value"]);
  assert.equal(successes, 1);
  assert.equal(button.dataset.copied, "yes");
});
