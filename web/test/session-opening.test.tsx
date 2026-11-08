// What a session being opened with nothing to show says under its spinner
// (`features/sessions/session-opening.tsx`): the line for each stage — the
// socket connecting, the server's answer, the offline copy's download from
// the hub with its pages or without, another try after `closed` — in English
// and Chinese; the thin bar only where there is a total; that the line waits
// a moment before it is said and then stays a moment, under a clock the test
// moves, so a fast open is the spinner alone; and that on droppen.app a feed
// waiting while the channel to the device is made says it is connecting. The
// UI audit's session-opening fixture measures that each line shows whole at
// 390 px.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after, afterEach, mock } from "node:test";
import { installJsdom } from "./jsdom.js";

const { window, container } = installJsdom();
after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});

const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { I18nextProvider } = await import("react-i18next");
const { OPENING_DELAY_MS, OPENING_MIN_MS, openingShown, openingText, SessionOpening } = await import(
  "../src/features/sessions/session-opening.js"
);
const { zhCN } = await import("../src/i18n/zh-CN.js");
const { makeI18n } = await import("./render.js");
type FeedOpening = import("../src/features/sessions/session-feed-pool.js").FeedOpening;
type Live = import("../src/lib/machines/runtime.js").MachineInfo["live"];

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} }, "zh-CN": { translation: zhCN } },
  keySeparator: false,
  nsSeparator: false,
});
const en = i18n.getFixedT("en");
const zh = i18n.getFixedT("zh-CN");

const CONNECTING: FeedOpening = { stage: "connecting" };
const LOADING: FeedOpening = { stage: "loading" };
const COPY: FeedOpening = { stage: "copy" };
const COPY_PAGES: FeedOpening = { stage: "copy", done: 12, total: 256 };
const READ_BYTES: FeedOpening = { stage: "loading", done: 40_000_000, total: 127_000_000 };
const RETRYING: FeedOpening = { stage: "retrying" };

test("the line for each stage, in English and Chinese", () => {
  const cases: [FeedOpening, string, string][] = [
    [CONNECTING, "connecting…", "连接中…"],
    [LOADING, "loading session…", "读取会话…"],
    [COPY_PAGES, "from hub · 12/256", "从云端下载 · 12/256"],
    [COPY, "from hub…", "从云端下载…"],
    [RETRYING, "cannot open · retrying", "打不开会话 · 重试中"],
    // A read's bytes are the bar's to show, not the line's.
    [READ_BYTES, "loading session…", "读取会话…"],
  ];
  for (const [opening, english, chinese] of cases) {
    assert.equal(openingText(opening, en), english);
    assert.equal(openingText(opening, zh), chinese);
  }
});

test("on droppen.app, a feed the copy has not answered while the channel is made is connecting", () => {
  assert.deepEqual(openingShown(LOADING, "connecting"), CONNECTING);
  for (const live of ["live", "copy", "stale", "refused"] satisfies Live[]) {
    assert.equal(openingShown(LOADING, live), LOADING, `${live}: the answer is what it waits for`);
  }
  assert.equal(openingShown(COPY_PAGES, "connecting"), COPY_PAGES, "the copy's pages say more");
  assert.equal(openingShown(READ_BYTES, "connecting"), READ_BYTES);
  assert.equal(openingShown(RETRYING, "connecting"), RETRYING);
});

// ---------------------------------------------------------------------------
// The view

let root: ReturnType<typeof createRoot> | null = null;
afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  mock.timers.reset();
});

async function mount(opening: FeedOpening | null, live: Live = "live", lng = "en"): Promise<void> {
  if (!root) root = createRoot(container);
  await act(async () => {
    await i18n.changeLanguage(lng);
    root!.render(
      <I18nextProvider i18n={i18n}>
        <SessionOpening opening={opening} live={live} />
      </I18nextProvider>,
    );
  });
}
const tick = (ms: number) => act(async () => mock.timers.tick(ms));
const line = () => container.querySelector("[data-opening-line]")?.textContent ?? null;
const bar = () => container.querySelector<HTMLElement>("[data-opening-bar]");
const spinner = () => container.querySelector("svg.animate-spin");

test("the line waits a moment before it is said, and stays a moment once it is", async () => {
  mock.timers.enable({ apis: ["setTimeout", "Date"], now: 0 });
  await mount(CONNECTING);
  assert.ok(spinner(), "the spinner from the start");
  await tick(OPENING_DELAY_MS - 1);
  assert.equal(line(), null, "a fast open is the spinner alone");
  await tick(1);
  assert.equal(line(), "connecting…");
  await mount(LOADING);
  assert.equal(line(), "loading session…", "a new stage is said at once");
  await tick(100);
  // Nothing waited for any more, with the view still up: the last words stay.
  await mount(null);
  await tick(OPENING_MIN_MS - 101);
  assert.equal(line(), "loading session…", "over just after it was said: kept, not blinked");
  await tick(1);
  assert.equal(line(), null);
  assert.ok(spinner(), "and the spinner stays where it was");

  // Over before the delay: never said.
  await mount(CONNECTING);
  await tick(OPENING_DELAY_MS - 200);
  await mount(null);
  await tick(5_000);
  assert.equal(line(), null);
});

test("each stage's line, and the bar only where there is a total", async () => {
  mock.timers.enable({ apis: ["setTimeout", "Date"], now: 0 });
  const cases: [FeedOpening, string, string | null][] = [
    [CONNECTING, "connecting…", null],
    [LOADING, "loading session…", null],
    [COPY, "from hub…", null],
    [COPY_PAGES, "from hub · 12/256", "4.6875%"],
    [RETRYING, "cannot open · retrying", null],
    [READ_BYTES, "loading session…", `${(100 * 40) / 127}%`],
  ];
  for (const [opening, words, width] of cases) {
    await mount(opening);
    await tick(OPENING_DELAY_MS);
    const view = container.querySelector<HTMLElement>("[data-session-opening]");
    assert.equal(view?.dataset.sessionOpening, opening.stage);
    assert.equal(line(), words);
    const b = bar();
    if (width === null) {
      assert.equal(b, null, `${words}: no bar without a total`);
    } else {
      assert.ok(b, `${words}: a bar`);
      assert.equal(b.getAttribute("role"), "progressbar");
      assert.equal(b.getAttribute("aria-label"), words);
      assert.equal(b.getAttribute("aria-valuenow"), String(opening.done));
      assert.equal(b.getAttribute("aria-valuemax"), String(opening.total));
      assert.equal((b.firstElementChild as HTMLElement).style.width, width);
    }
    // A fresh view for the next stage, which starts its own wait.
    await act(async () => root?.unmount());
    root = null;
  }
});

test("the copy's download in Chinese, and connecting while the channel is made", async () => {
  mock.timers.enable({ apis: ["setTimeout", "Date"], now: 0 });
  await mount(COPY_PAGES, "connecting", "zh-CN");
  await tick(OPENING_DELAY_MS);
  assert.equal(line(), "从云端下载 · 12/256");
  assert.ok(bar());
  await mount(LOADING, "connecting", "zh-CN");
  assert.equal(line(), "连接中…");
  assert.equal(bar(), null);
  await mount(RETRYING, "connecting", "zh-CN");
  assert.equal(line(), "打不开会话 · 重试中");
});
