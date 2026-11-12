// Settings → Desktop app: the app's own window's section, drawn for real. It
// shows Open in Browser and Start at Login (with the state the app pushes),
// hides Start at Login where the app offers none, blocks the switch on an App
// Translocation copy, and carries macOS's Login Items line. A browser has no
// bridge, so the section — and its tab — are not there at all.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { find, installLiveDom, propsOf, waitFor, type FakeNode } from "./live-dom.js";

const container = installLiveDom({ act: true });

const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { default: i18n } = await import("./i18n-all.js");
const { DesktopAppSection } = await import("../src/features/settings/desktop-app-section.js");
const { askedSection, sectionsFor } = await import("../src/features/settings/sections.js");
const { isDesktopWindow, parseDesktopSettings, desktopSettingsState } = await import("../src/lib/desktop-app.js");

await i18n.changeLanguage("en");

type Bridge = Record<string, unknown>;

function setBridge(bridge: Bridge | undefined): void {
  const page = window as unknown as { __droppenDesktop?: Bridge };
  if (bridge === undefined) delete page.__droppenDesktop;
  else page.__droppenDesktop = bridge;
}

/** A bridge whose settings state is `state`, recording what the section asked. */
function fakeBridge(state: Record<string, unknown>, startAtLogin = true) {
  const calls: string[] = [];
  return {
    calls,
    bridge: {
      startAtLogin,
      trayHost: true,
      settingsState: () => state,
      openInBrowser: () => calls.push("open"),
      setStartAtLogin: (on: boolean) => calls.push(`set:${on}`),
      openLoginItems: () => calls.push("loginItems"),
      refreshDesktopSettings: () => calls.push("refresh"),
    },
  };
}

const section = () => find(container, (node) => node.getAttribute("data-testid") === "desktop-app-section");
const byTestId = (id: string) => find(container, (node) => node.getAttribute("data-testid") === id);
const text = () => (container as FakeNode).textContent ?? "";

/** Mount the real section with `bridge` on the page, and wait for its state. */
async function mount(bridge: Bridge) {
  setBridge(bridge);
  const root = createRoot(container as never);
  await act(async () => {
    root.render(<DesktopAppSection />);
  });
  await waitFor(() => section() !== null);
  return root;
}

async function unmount(root: ReturnType<typeof createRoot>) {
  await act(async () => root.unmount());
  setBridge(undefined);
}

test("outside the app's window the section and its tab are not there", () => {
  setBridge(undefined);
  assert.equal(isDesktopWindow(), false);
  assert.equal(desktopSettingsState(), null);
  assert.doesNotMatch(sectionsFor(false).join(","), /desktop app/);
  assert.equal(askedSection("desktop app", false, false), undefined);
  assert.equal(renderToStaticMarkup(<DesktopAppSection />), "");
  setBridge({ startAtLogin: true });
  try {
    assert.equal(isDesktopWindow(), true);
    assert.match(sectionsFor(false, true).join(","), /desktop app/);
    assert.equal(askedSection("desktop app", false, true), "desktop app");
  } finally {
    setBridge(undefined);
  }
});

test("the pushed state is read as the section's shape, and nothing else", () => {
  assert.deepEqual(parseDesktopSettings({ checked: true, enabled: true, note: null }), { checked: true, enabled: true, note: null });
  assert.deepEqual(parseDesktopSettings({ checked: false, enabled: false, note: "needsApproval" }), {
    checked: false,
    enabled: false,
    note: "needsApproval",
  });
  assert.deepEqual(parseDesktopSettings({ checked: false, enabled: true, note: "turnOnFailed" })?.note, "turnOnFailed");
  assert.deepEqual(parseDesktopSettings({ checked: false, enabled: true, note: "other" })?.note, null);
  for (const bad of [null, undefined, "on", 1, {}, { checked: true }, { checked: "yes", enabled: true }]) {
    assert.equal(parseDesktopSettings(bad), null, JSON.stringify(bad));
  }
});

test("Open in Browser asks the app; the switch asks for the other way", async () => {
  const { bridge, calls } = fakeBridge({ checked: true, enabled: true, note: null });
  const root = await mount(bridge);
  try {
    assert.match(text(), /Open in Browser/);
    assert.match(text(), /Start Droppen at login/);
    assert.equal(byTestId("desktop-start-at-login")!.getAttribute("aria-checked"), "true");
    propsOf(find(section()!, (n) => n.tagName === "BUTTON" && n.textContent === "Open in Browser")!).onClick();
    assert.deepEqual(calls.filter((call) => call !== "refresh"), ["open"]);
    propsOf(byTestId("desktop-start-at-login")!).onClick();
    assert.deepEqual(calls.filter((call) => call !== "refresh"), ["open", "set:false"]);
    assert.ok(calls.includes("refresh"), "the section reads the real state when it opens");
  } finally {
    await unmount(root);
  }
});

test("an app with no Start at Login keeps Open in Browser and drops the row", async () => {
  const { bridge } = fakeBridge({ checked: false, enabled: true, note: null }, false);
  const root = await mount(bridge);
  try {
    assert.match(text(), /Open in Browser/);
    assert.doesNotMatch(text(), /Start Droppen at login/);
    assert.equal(byTestId("desktop-start-at-login"), null);
  } finally {
    await unmount(root);
  }
});

test("an App Translocation copy blocks the switch and says to move the app", async () => {
  const { bridge, calls } = fakeBridge({ checked: false, enabled: false, note: null });
  const root = await mount(bridge);
  try {
    assert.match(text(), /Move Droppen to Applications first\./);
    assert.equal(byTestId("desktop-start-at-login")!.getAttribute("aria-disabled"), "true");
    propsOf(byTestId("desktop-start-at-login")!).onClick();
    assert.deepEqual(calls.filter((call) => call !== "refresh"), [], "a blocked switch asks for nothing");
  } finally {
    await unmount(root);
  }
});

test("macOS waiting for approval says so and opens Login Items", async () => {
  const { bridge, calls } = fakeBridge({ checked: true, enabled: true, note: "needsApproval" });
  const root = await mount(bridge);
  try {
    assert.match(text(), /Allow Droppen in Login Items\./);
    propsOf(find(section()!, (n) => n.tagName === "BUTTON" && n.textContent === "Open Login Items")!).onClick();
    assert.ok(calls.includes("loginItems"));
  } finally {
    await unmount(root);
  }
});
