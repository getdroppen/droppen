// The theme hook the landing page's screenshots use: it re-renders when
// `applyThemeMode` paints the other theme, and stops listening when the
// component that asked is gone.
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
const { applyThemeMode, useDarkTheme } = await import("../src/lib/theme.js");

type Root = ReturnType<typeof createRoot>;
let root: Root | null = null;

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
});

function Probe() {
  return <span data-testid="dark">{String(useDarkTheme())}</span>;
}

const painted = (): string | null => container.querySelector('[data-testid="dark"]')?.textContent ?? null;

test("useDarkTheme follows applyThemeMode, and stops after unmount", async () => {
  document.documentElement.classList.remove("dark");
  root = createRoot(container);
  await act(async () => root!.render(<Probe />));
  assert.equal(painted(), "false");

  await act(async () => applyThemeMode("dark"));
  assert.equal(painted(), "true");
  assert.equal(document.documentElement.classList.contains("dark"), true);

  await act(async () => applyThemeMode("light"));
  assert.equal(painted(), "false");
  assert.equal(document.documentElement.classList.contains("dark"), false);

  await act(async () => root!.unmount());
  root = null;
  // The subscriber is gone: painting the other theme still works, and does not
  // try to update an unmounted tree.
  await act(async () => applyThemeMode("dark"));
  assert.equal(document.documentElement.classList.contains("dark"), true);
});
