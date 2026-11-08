// What the cloud bar says while the offline copy catches up from the hub
// (`features/cloud/cloud-bar.tsx`): the words for each case — the session
// list, sessions x/y, and a percentage while a big checkpoint download runs —
// in English and Chinese; that they are said only once a catch-up has gone on
// for a moment and then stay a moment, under a clock the test moves; that they
// are never said live, with the hub link down, or for a stream that only
// waits out a backoff; and where they go: after the copy's time in the
// offline notice, or after the connection's words in the strip — each with a
// line of its own for where there is no room (CSS picks; the UI audit
// measures that they show whole at 390 px).
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
const { BarCatchUp, BarCatchUpLine, BIG_DOWNLOAD_MS, CATCH_UP_DELAY_MS, CATCH_UP_MIN_MS, catchUpText, OfflineNotice, useCatchUpShown } = await import("../src/features/cloud/cloud-bar.js");
const { useShownAfter } = await import("../src/lib/use-shown-after.js");
const { NO_PROGRESS } = await import("../src/lib/hub/streams.js");
const { ja } = await import("../src/i18n/ja.js");
const { zhCN } = await import("../src/i18n/zh-CN.js");
const { zhTW } = await import("../src/i18n/zh-TW.js");
const { makeI18n } = await import("./render.js");
type StreamProgress = import("../src/lib/hub/streams.js").StreamProgress;
type CloudMode = import("../src/lib/transport/switching.js").CloudMode;
type LinkState = import("../src/lib/hub/link.js").LinkState;

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} }, "zh-CN": { translation: zhCN } },
  keySeparator: false,
  nsSeparator: false,
});
const en = i18n.getFixedT("en");
const zh = i18n.getFixedT("zh-CN");

const counts = (index: number, status: number, session: number) => ({ index, status, session });
const progress = (p: Partial<StreamProgress>): StreamProgress => ({ ...NO_PROGRESS, ...p });
const INDEX = progress({ catchingUp: counts(1, 0, 0), synced: counts(0, 1, 0) });
const SESSIONS = progress({ catchingUp: counts(0, 0, 4), synced: counts(1, 1, 3) });
const NOW = 1_000_000;
const downloading = (base: StreamProgress, done: number, total: number, sinceMs = 100) => ({
  ...base,
  pages: { done, total },
  downloadingSince: NOW - sinceMs,
  episode: { done, total },
});

// ---------------------------------------------------------------------------
// The words
// ---------------------------------------------------------------------------

test("nothing is said while nothing is being caught up on: all synced, only waiting, or only waiting out a backoff", () => {
  assert.equal(catchUpText(NO_PROGRESS, en, NOW), null);
  assert.equal(catchUpText(progress({ synced: counts(1, 1, 5) }), en, NOW), null, "caught up");
  assert.equal(catchUpText(progress({ waiting: counts(1, 1, 5) }), en, NOW), null, "the link is down: the bar's own words say that");
  assert.equal(catchUpText(progress({ retrying: counts(0, 0, 2), synced: counts(1, 1, 3) }), en, NOW), null, "a stream that keeps failing is not a catch-up");
});

test("the session list while only the index catches up; sessions x/y once sessions do", () => {
  assert.deepEqual(catchUpText(INDEX, en, NOW), { text: "updating from hub · session list", title: null });
  assert.equal(catchUpText(INDEX, zh, NOW)?.text, "正在从云端更新 · 会话列表");

  const sessions = catchUpText(SESSIONS, en, NOW);
  assert.equal(sessions?.text, "updating from hub · sessions 3/7");
  assert.equal(sessions?.title, "recent sessions kept on this device: 3 of 7 up to date", "the tooltip says what the 7 is");
  assert.equal(catchUpText(SESSIONS, zh, NOW)?.text, "正在从云端更新 · 会话 3/7");
  assert.equal(catchUpText(SESSIONS, zh, NOW)?.title, "本设备保留的最近会话：7 个中 3 个已是最新");

  // y is every session followed now, a failing or waiting one too; the index catching up as well does not hide the count.
  const mixed = progress({ catchingUp: counts(1, 0, 2), waiting: counts(0, 0, 1), retrying: counts(0, 0, 1), synced: counts(0, 1, 3) });
  assert.equal(catchUpText(mixed, en, NOW)?.text, "updating from hub · sessions 3/7");
  // Only the status catching up: nothing more to name.
  assert.equal(catchUpText(progress({ catchingUp: counts(0, 1, 0), synced: counts(1, 0, 2) }), en, NOW)?.text, "updating from hub");
});

test("a percentage while a big download runs: big by its pages or by how long it takes, counted over the whole catch-up", () => {
  const big = catchUpText(downloading(SESSIONS, 17, 40), en, NOW);
  assert.equal(big?.text, "updating from hub · sessions 3/7 · 42%", "17 of 40 pages: rounded down, never ahead of itself");
  assert.equal(big?.title, "recent sessions kept on this device: 3 of 7 up to date · 17 of 40 pieces fetched");
  assert.equal(catchUpText(downloading(SESSIONS, 17, 40), zh, NOW)?.text, "正在从云端更新 · 会话 3/7 · 42%");
  assert.equal(catchUpText(downloading(SESSIONS, 17, 40), zh, NOW)?.title, "本设备保留的最近会话：7 个中 3 个已是最新 · 40 块中已取回 17 块");
  assert.equal(catchUpText(downloading(INDEX, 4, 10), en, NOW)?.text, "updating from hub · session list · 40%");

  // A small one says no percentage, until it has run for a while.
  assert.equal(catchUpText(downloading(SESSIONS, 1, 3, 500), en, NOW)?.text, "updating from hub · sessions 3/7");
  assert.equal(catchUpText(downloading(SESSIONS, 1, 3, BIG_DOWNLOAD_MS), en, NOW)?.text, "updating from hub · sessions 3/7 · 33%");
  // Pages counted, but no download running now: no percentage.
  assert.equal(catchUpText({ ...SESSIONS, episode: { done: 20, total: 40 } }, en, NOW)?.text, "updating from hub · sessions 3/7");
  // A download on a synced stream (a live checkpoint) is a catch-up too.
  assert.equal(catchUpText(downloading(progress({ synced: counts(1, 1, 7) }), 10, 20), en, NOW)?.text, "updating from hub · 50%");
  // A total that really grows is not hidden: the share drops.
  assert.equal(catchUpText({ ...downloading(SESSIONS, 20, 40), episode: { done: 20, total: 40 } }, en, NOW)?.text.endsWith("· 50%"), true);
  assert.equal(catchUpText({ ...downloading(SESSIONS, 0, 20), episode: { done: 20, total: 60 } }, en, NOW)?.text.endsWith("· 33%"), true);
});

test("every new string is in every shipped language", () => {
  const keys = [
    "updating from hub",
    "session list",
    "sessions {{done}}/{{total}}",
    "recent sessions kept on this device: {{done}} of {{total}} up to date",
    "{{done}} of {{total}} pieces fetched",
  ];
  for (const [lang, table] of Object.entries({ "zh-CN": zhCN, "zh-TW": zhTW, ja })) {
    for (const key of keys) {
      const value = (table as Record<string, string>)[key];
      assert.ok(value, `${lang} has "${key}"`);
      for (const slot of key.match(/\{\{\w+\}\}/g) ?? []) assert.ok(value.includes(slot), `${lang} keeps ${slot} in "${key}"`);
    }
  }
  // "The hub" is 云端 in Chinese, as in 「连不上云端」.
  assert.ok(zhCN["updating from hub"]!.includes("云端"));
  assert.ok(zhTW["updating from hub"]!.includes("雲端"));
});

// ---------------------------------------------------------------------------
// When, under a clock the test moves
// ---------------------------------------------------------------------------

let root: ReturnType<typeof createRoot> | null = null;
afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  mock.timers.reset();
});

async function mount(node: React.ReactElement): Promise<void> {
  if (!root) root = createRoot(container);
  await act(async () => root!.render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>));
}
const tick = (ms: number) => act(async () => mock.timers.tick(ms));
const shown = () => container.querySelector("[data-shown]")?.getAttribute("data-shown");

function Delayed({ active }: { active: boolean }) {
  return <span data-shown={String(useShownAfter(active, CATCH_UP_DELAY_MS, CATCH_UP_MIN_MS))} />;
}

test("said only once it has held for a moment, and kept a moment once said", async () => {
  mock.timers.enable({ apis: ["setTimeout", "Date"], now: 0 });
  await mount(<Delayed active />);
  await tick(CATCH_UP_DELAY_MS - 1);
  assert.equal(shown(), "false", "a plain resume, caught up in a moment, never flashes");
  await tick(1);
  assert.equal(shown(), "true");
  await tick(100);
  await mount(<Delayed active={false} />);
  await tick(CATCH_UP_MIN_MS - 101);
  assert.equal(shown(), "true", "over just after it was said: kept up, not blinked");
  await tick(1);
  assert.equal(shown(), "false");

  // Over before the delay: never said.
  await mount(<Delayed active />);
  await tick(CATCH_UP_DELAY_MS - 300);
  await mount(<Delayed active={false} />);
  await tick(5_000);
  assert.equal(shown(), "false");

  // Said long enough already: gone as soon as it is over. (One tick per timer:
  // the mocked clock reads the end of a tick inside every timer it runs.)
  await mount(<Delayed active />);
  await tick(CATCH_UP_DELAY_MS);
  await tick(3_000);
  assert.equal(shown(), "true");
  await mount(<Delayed active={false} />);
  await tick(1);
  assert.equal(shown(), "false");
});

/** A machine's copy source that the test moves. */
function source(initial: StreamProgress) {
  const listeners = new Set<() => void>();
  const s = {
    progress: initial,
    onProgress(fn: () => void) {
      listeners.add(fn);
      return () => void listeners.delete(fn);
    },
    async set(next: StreamProgress) {
      await act(async () => {
        s.progress = next;
        for (const fn of [...listeners]) fn();
      });
    },
  };
  return s;
}

function Gate({ src, mode, link }: { src: ReturnType<typeof source>; mode: CloudMode; link: LinkState }) {
  return <span data-shown={String(useCatchUpShown(src, { mode, link }))} />;
}

test("never said live, with the hub link down, or for streams that only wait out a backoff", async () => {
  mock.timers.enable({ apis: ["setTimeout", "Date"], now: 0 });
  const busy = source(SESSIONS);
  await mount(<Gate src={busy} mode="live" link="open" />);
  await tick(5_000);
  assert.equal(shown(), "false", "live: the copy is not what is shown");
  await mount(<Gate src={busy} mode="offline" link="closed" />);
  await tick(5_000);
  assert.equal(shown(), "false", "the hub link is down: the bar says that");
  await mount(<Gate src={busy} mode="offline" link="connecting" />);
  await tick(5_000);
  assert.equal(shown(), "false");
  const failing = source(progress({ retrying: counts(0, 0, 3), synced: counts(1, 1, 4) }));
  await mount(<Gate src={failing} mode="offline" link="open" />);
  await tick(60_000);
  assert.equal(shown(), "false", "a stream that keeps failing cannot keep it on screen");
});

test("said on the copy over an open link; gone at once with the link or into live, after its minimum when the catch-up ends", async () => {
  mock.timers.enable({ apis: ["setTimeout", "Date"], now: 0 });
  const src = source(SESSIONS);
  await mount(<Gate src={src} mode="offline" link="open" />);
  await tick(CATCH_UP_DELAY_MS);
  assert.equal(shown(), "true");
  await mount(<Gate src={src} mode="offline" link="closed" />);
  assert.equal(shown(), "false", "the link went: not kept for its minimum");
  await tick(5_000);
  await mount(<Gate src={src} mode="offline" link="open" />);
  await tick(CATCH_UP_DELAY_MS - 1);
  assert.equal(shown(), "false", "back on the link, after a while: it waits its moment again");
  await tick(1);
  assert.equal(shown(), "true");
  await mount(<Gate src={src} mode="live" link="open" />);
  assert.equal(shown(), "false", "live: gone at once");
  await tick(5_000);

  await mount(<Gate src={src} mode="offline" link="open" />);
  await tick(CATCH_UP_DELAY_MS);
  await tick(1_000);
  assert.equal(shown(), "true");
  await src.set(progress({ synced: counts(1, 1, 7) }));
  await tick(1);
  assert.equal(shown(), "false", "shown long enough: gone when the catch-up is over");
  await src.set(SESSIONS);
  await tick(CATCH_UP_DELAY_MS - 1);
  assert.equal(shown(), "false", "a new catch-up waits its moment again");
  await tick(1);
  assert.equal(shown(), "true");
});

// ---------------------------------------------------------------------------
// Where
// ---------------------------------------------------------------------------

const words = () => [...container.querySelectorAll('[data-testid="catch-up"]')].map((e) => e.textContent);

test("in the offline notice, after the copy's time; the count and the percentage kept current; the last words kept while it stays up", async () => {
  mock.timers.enable({ apis: ["setTimeout", "Date"], now: NOW });
  void i18n.changeLanguage("en");
  const src = source(SESSIONS);
  await mount(<OfflineNotice freshestMs={Date.UTC(2026, 10, 8, 9, 30)} catchUp={src} />);
  const notice = container.querySelector('[data-testid="offline-notice"]')!;
  // In the time's line at desktop width, and on a line of its own at a phone's (CSS picks one).
  assert.deepEqual(words(), ["updating from hub · sessions 3/7", "updating from hub · sessions 3/7"]);
  assert.ok(notice.textContent!.indexOf("as of") < notice.textContent!.indexOf("updating from hub"), "after the time it moves on");
  assert.ok(notice.textContent!.indexOf("updating from hub") < notice.textContent!.indexOf("read-only"));
  assert.equal(container.querySelector('[data-testid="catch-up"]')!.getAttribute("title"), "recent sessions kept on this device: 3 of 7 up to date");

  await src.set(progress({ catchingUp: counts(0, 0, 3), synced: counts(1, 1, 4) }));
  assert.equal(words()[0], "updating from hub · sessions 4/7");
  // A small download: its percentage once it has run for a while.
  await src.set({ ...progress({ catchingUp: counts(0, 0, 3), synced: counts(1, 1, 4) }), pages: { done: 1, total: 3 }, downloadingSince: Date.now(), episode: { done: 1, total: 3 } });
  assert.equal(words()[0], "updating from hub · sessions 4/7");
  await tick(BIG_DOWNLOAD_MS);
  assert.equal(words()[0], "updating from hub · sessions 4/7 · 33%", "drawn again when it has lasted, with no page to prompt it");

  // Over, while the bar still keeps it up for its minimum: the last words, not nothing.
  await src.set(progress({ synced: counts(1, 1, 7) }));
  assert.equal(words()[0], "updating from hub · sessions 4/7 · 33%");

  // Without a catch-up to say, the notice is as it was.
  await mount(<OfflineNotice freshestMs={Date.UTC(2026, 10, 8, 9, 30)} />);
  assert.deepEqual(words(), []);
});

test("with no notice: in the strip after the connection's words where it has room, else on a line of its own under it; nothing at all while there are no words", async () => {
  void i18n.changeLanguage("zh-CN");
  const busy = source(downloading(SESSIONS, 17, 40, 5_000));
  await mount(
    <>
      <BarCatchUp source={busy} />
      <BarCatchUpLine source={busy} />
    </>,
  );
  const bar = container.querySelector('[data-catch-up="bar"]');
  assert.ok(bar, "in the strip");
  assert.equal(bar.textContent, "·正在从云端更新 · 会话 3/7 · 42%");
  assert.match(bar.className, /max-lg:hidden/, "only where the strip has room for it beside its longest words");
  assert.match(bar.className, /shrink-\[999\]/, "the first thing in the strip to give way, before the retry countdown");
  const line = container.querySelector('[data-catch-up="line"]');
  assert.ok(line, "and a line of its own under the strip");
  assert.equal(line.textContent, "正在从云端更新 · 会话 3/7 · 42%");
  assert.match(line.className, /lg:hidden/, "where the strip has no room");
  assert.doesNotMatch(line.className, /max-lg:hidden/);
  // Another machine's source with nothing to say: not the last machine's words, and no lone dot or empty line.
  const idle = source(NO_PROGRESS);
  await mount(
    <>
      <BarCatchUp source={idle} />
      <BarCatchUpLine source={idle} />
    </>,
  );
  assert.ok(container.querySelector("[data-catch-up]") === null, "nothing drawn");
  assert.equal(container.textContent, "");
  void i18n.changeLanguage("en");
});
