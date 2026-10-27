// The activity bar's words while a turn runs, mounted in a real DOM (jsdom)
// and driven by draft frames the way the socket drives them: the count of a
// think in flight takes the place of "last activity", moves in place, and
// gives it back when the think is over — counting the think as activity —
// and wakes nothing around it. `stream-draft.test.tsx` pins the markup; this
// pins the behaviour over time.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after, afterEach } from "node:test";
import { installJsdom } from "./jsdom.js";
import { makeI18n } from "./render.js";

const { window, container } = installJsdom();
after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});

// After the DOM: react-dom looks at the page as it loads.
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { I18nextProvider } = await import("react-i18next");
const { createDraftStore, thinkingOf } = await import("../src/features/sessions/stream-draft.js");
const { TurnStatus } = await import("../src/features/sessions/turn-status.js");

const i18n = makeI18n({ lng: "en", resources: { en: { translation: {} } } });

type Root = ReturnType<typeof createRoot>;
let root: Root | null = null;
afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
});

/** What a reader sees: not the invisible copy that holds the count's width. */
function visible(element: Element): string {
  const copy = element.cloneNode(true) as Element;
  copy.querySelectorAll("[data-ghost]").forEach((ghost) => ghost.remove());
  return copy.textContent ?? "";
}

const frame = (blocks: unknown[]) => ({ type: "draft", turn: 1, v: 1, blocks });
const counter = (tokens: number) => ({ index: 0, kind: "thinking", thinkingTokens: tokens });
// What an agent from before also sent: the finished think and the words after it.
const thought = { uuid: "u-think", index: 0, kind: "thinking", thinkingTokens: 1240, done: true };
const said = { uuid: "u-said", index: 0, kind: "text", text: "I'll write the new module now." };

const NOW = Date.parse("2026-10-21T12:00:00Z");
const progress = {
  messages: 28,
  startedAt: new Date(NOW - (7 * 60 + 14) * 1000).toISOString(),
  lastActivityAt: new Date(NOW - 45_000).toISOString(),
};

test("the count takes the place of the last activity, moves in place, and gives it back counting the think", async () => {
  const drafts = createDraftStore();
  let around = 0;
  let now = NOW;
  // Everything the bar is inside of: it must not hear a draft frame.
  function Around({ at }: { at: number }) {
    around++;
    return <TurnStatus drafts={drafts} progress={progress} now={at} />;
  }
  root = createRoot(container);
  const render = () => act(async () => root!.render(<I18nextProvider i18n={i18n}><Around at={now} /></I18nextProvider>));
  await render();
  const text = () => visible(container);
  assert.equal(text(), "28 messages · last activity 45s ago · 7m14s");
  const rendered = around;

  const hear = (blocks: unknown[], at: number) => act(async () => drafts.receive(thinkingOf(frame(blocks)), at));
  await hear([counter(42)], NOW - 20_000);
  assert.equal(text(), "28 messages · thinking… ~42 tokens · 7m14s");
  const slot = container.querySelector("[data-thinking-tokens]");
  assert.ok(slot);

  // A dozen frames a second during a long think: the number changes in the
  // same box, and the words around it stay where they are.
  await hear([counter(850)], NOW - 10_000);
  await hear([counter(1240)], NOW - 5_000);
  assert.equal(text(), "28 messages · thinking… ~1.2k tokens · 7m14s");
  assert.equal(container.querySelector("[data-thinking-tokens]"), slot, "the same slot, not a new one");
  assert.equal(around, rendered, "no draft frame re-rendered what the bar sits in");

  // The think is printed finished and the model moves on — an agent from
  // before sends the finished think and the words beside it, and neither is
  // shown. The bar's last activity is the think's end, not the transcript's
  // 45 seconds of quiet.
  await hear([thought, said], NOW - 2_000);
  assert.equal(text(), "28 messages · last activity 2s ago · 7m14s");
  assert.doesNotMatch(text(), /write the new module|1\.2k/);
  now = NOW + 8_000;
  await render();
  assert.equal(text(), "28 messages · last activity 10s ago · 7m22s", "and it ages like any other");

  // The next think, and then the socket going away.
  await hear([thought, said, counter(9)], NOW + 8_000);
  assert.equal(text(), "28 messages · thinking… ~9 tokens · 7m22s");
  await act(async () => drafts.clear());
  assert.equal(text(), "28 messages · last activity 0s ago · 7m22s");
});

test("a frame that moves only blocks nobody shows wakes nothing at all", async () => {
  const drafts = createDraftStore();
  let renders = 0;
  root = createRoot(container);
  await act(async () =>
    root!.render(
      <I18nextProvider i18n={i18n}>
        <React.Profiler id="turn-status" onRender={() => renders++}>
          <TurnStatus drafts={drafts} progress={{ messages: 3, startedAt: new Date(NOW - 12_000).toISOString(), lastActivityAt: null }} now={NOW} />
        </React.Profiler>
      </I18nextProvider>,
    ),
  );
  await act(async () => drafts.receive(thinkingOf(frame([said, counter(40)])), NOW));
  const before = renders;
  assert.ok(before >= 2, "the count arriving is a render");
  // From an agent from before: a tool call answered beside the same count.
  await act(async () =>
    drafts.receive(thinkingOf(frame([said, { uuid: "u-tool", index: 0, kind: "tool_use", toolName: "Read", done: true }, counter(40)])), NOW + 100),
  );
  assert.equal(visible(container), "3 messages · thinking… ~40 tokens · 12s");
  assert.equal(renders, before);
});
