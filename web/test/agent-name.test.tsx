// The name other agent sessions message a Claude Code session by (`agentName`,
// read off its running CLI's registration): the header's chip that copies it
// (`AgentNameChip`, `session-meta.tsx`) and the session row's context menu
// (`SessionRowMenu`, `sessions-layout.tsx`), driven in a real DOM (jsdom).
//
// What is pinned: the chip shows the name in the header's mono meta type,
// says what it is for in its tooltip, copies the name — never the session id,
// which no agent can address — and says it did, the way every copy control
// does; the menu offers "copy agent name" only on a row that has one, and not
// on another device's row, whose name is its copy's guess.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after, afterEach } from "node:test";
import { installJsdom } from "./jsdom.js";
import { makeI18n } from "./render.js";

const { window, document, container } = installJsdom();
// The context menu's trigger listens with an `AbortSignal`, and jsdom takes
// only its own: Node's, the global one, is refused.
for (const name of ["AbortController", "AbortSignal"]) {
  Object.defineProperty(globalThis, name, { configurable: true, writable: true, value: (window as unknown as Record<string, unknown>)[name] });
}
after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});

// After the DOM: react-dom and Base UI look at the page as they load.
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { I18nextProvider } = await import("react-i18next");
const { AgentNameChip } = await import("../src/features/sessions/session-meta.js");
const { SessionRowMenu } = await import("../src/features/sessions/sessions-layout.js");
const { ContextMenu, ContextMenuTrigger } = await import("../src/components/ui/context-menu.js");
const { zhCN } = await import("../src/i18n/zh-CN.js");
type SessionSummary = import("../src/api/schema.js").components["schemas"]["SessionSummary"];

const i18n = makeI18n({ lng: "en", resources: { en: { translation: {} }, "zh-CN": { translation: zhCN } } });

/** What the page wrote to the clipboard, in order. */
let written: string[] = [];
Object.defineProperty(window.navigator, "clipboard", {
  configurable: true,
  value: { writeText: async (text: string) => void written.push(text) },
});

type Root = ReturnType<typeof createRoot>;
let root: Root | null = null;
afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  written = [];
  await act(async () => void (await i18n.changeLanguage("en")));
  // A menu left open by a test would answer the next one's queries.
  document.body.querySelectorAll("[data-base-ui-portal], [data-base-ui-floating-portal]").forEach((n) => n.remove());
});

async function render(node: React.ReactNode): Promise<void> {
  root = createRoot(container);
  await act(async () => root!.render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>));
}

const settle = (ms = 20) => act(async () => void (await new Promise((r) => setTimeout(r, ms))));

async function click(el: Element): Promise<void> {
  await act(async () => {
    el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
  });
  await settle();
}

const row = (extra: Partial<SessionSummary> = {}) =>
  ({
    source: "claude",
    id: "49c15a9d-11d8-4d7b-b4f6-611b10366b2e",
    title: "claude运维",
    pinned: false,
    ...extra,
  }) as SessionSummary;

test("the chip shows the name in the header's mono meta type, and its tooltip says what it is for", async () => {
  await render(<AgentNameChip name="k8s-configs-24" />);
  const chip = container.querySelector<HTMLButtonElement>("button[data-agent-name]");
  assert.ok(chip, "a button");
  assert.equal(chip.textContent, "k8s-configs-24");
  assert.equal(chip.type, "button");
  assert.equal(chip.title, "other agent sessions can message this session by this name");
  for (const cls of ["font-mono", "text-2xs", "text-muted", "h-5"]) assert.ok(chip.classList.contains(cls), `${cls} on the chip`);
  assert.ok(chip.querySelector("span.truncate"), "a name too long for the row is cut, not the row widened");

  await act(async () => void (await i18n.changeLanguage("zh-CN")));
  assert.equal(chip.title, "其他 agent 会话可以用这个名称给这个会话发消息");
});

test("pressing the chip copies the name, and says so for a moment", async () => {
  await render(<AgentNameChip name="k8s-configs-24" />);
  const chip = container.querySelector<HTMLButtonElement>("button[data-agent-name]")!;
  const icon = chip.querySelector("svg")!.outerHTML;
  await click(chip);
  assert.deepEqual(written, ["k8s-configs-24"], "the name, not the session id");
  assert.equal(chip.title, "copied");
  assert.notEqual(chip.querySelector("svg")!.outerHTML, icon, "the check in place of the @");
  assert.equal(chip.textContent, "k8s-configs-24", "the name stays on the chip");
});

/** The row's menu, opened the way a person opens it: a right click on the row. */
async function openMenu(session: SessionSummary, readOnly = false): Promise<HTMLElement[]> {
  const noop = () => undefined;
  await render(
    <ContextMenu>
      <ContextMenuTrigger render={<div data-row="">{session.title}</div>} />
      <SessionRowMenu session={session} readOnly={readOnly} pinPending={false} onPin={noop} onRename={noop} onArchive={noop} />
    </ContextMenu>,
  );
  const trigger = container.querySelector("[data-row]")!;
  await act(async () => {
    trigger.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, button: 2, clientX: 10, clientY: 10 }));
  });
  await settle();
  const items = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')];
  assert.ok(items.length > 0, "the menu is open");
  return items;
}

const labels = (items: HTMLElement[]) => items.map((item) => item.textContent);

test("a row whose CLI runs offers to copy its agent name, which copies the name", async () => {
  const items = await openMenu(row({ agentName: "k8s-configs-fc" }));
  assert.deepEqual(labels(items), ["pin session", "rename", "copy session id", "copy agent name", "archive session"]);
  await click(items.find((item) => item.textContent === "copy agent name")!);
  assert.deepEqual(written, ["k8s-configs-fc"]);
});

test("a row with no agent name offers only the session id", async () => {
  const items = await openMenu(row());
  assert.deepEqual(labels(items), ["pin session", "rename", "copy session id", "archive session"]);
  await click(items.find((item) => item.textContent === "copy session id")!);
  assert.deepEqual(written, ["49c15a9d-11d8-4d7b-b4f6-611b10366b2e"]);
});

test("another device's row, read from its copy, offers no agent name", async () => {
  const items = await openMenu(row({ agentName: "k8s-configs-b7" }), true);
  assert.deepEqual(labels(items), ["copy session id"]);
});

test("the menu says it in Chinese", async () => {
  await act(async () => void (await i18n.changeLanguage("zh-CN")));
  const items = await openMenu(row({ agentName: "k8s-configs-24" }));
  assert.ok(labels(items).includes("复制 agent 名称"), `${labels(items).join(" / ")}`);
});
