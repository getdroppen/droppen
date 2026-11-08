// What the cloud bar says while the page is not live on the device
// (`features/cloud/cloud-bar.tsx`): which step a channel being opened is on,
// and — between tries, of the hub link or of the channel — that it cannot
// reach it and when it tries again, counting down; the countdown is the
// "retry now" button too. A refusal gets a short label of its own (the notice
// under the bar says it in full). Every new string is in every shipped
// language.
//
// The words are drawn with `react-dom/server`; the countdown and the button
// need a real DOM (jsdom), a clock the test moves, and a click.
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
const { barState, ConnectionLabel, secondsUntil } = await import("../src/features/cloud/cloud-bar.js");
const { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } = await import("../src/i18n/cloud-devices.js");
const { ja } = await import("../src/i18n/ja.js");
const { zhCN } = await import("../src/i18n/zh-CN.js");
const { zhTW } = await import("../src/i18n/zh-TW.js");
const { makeI18n, pageDraw } = await import("./render.js");

const i18n = makeI18n({
  lng: "en",
  resources: {
    en: { translation: {} },
    "zh-CN": { translation: { ...cloudDevicesZhCN, ...zhCN } },
    ja: { translation: { ...cloudDevicesJa, ...ja } },
  },
  keySeparator: false,
  nsSeparator: false,
});
const draw = pageDraw(i18n);

const copy = { loaded: true, hasCopy: true, freshestMs: Date.UTC(2026, 9, 15, 9, 30) };
const base = { link: "open" as const, presence: "online" as const, channel: "error" as const, mode: "offline" as const, copy };

test("between tries the bar says when the next one is; a try under way, or a reason, says something else", () => {
  const hub = barState({ ...base, link: "closed", linkRetryAt: 5_000 });
  assert.deepEqual([hub.label, hub.retryAt], ["hubRetry", 5_000], "the hub link waits out its backoff");
  const dialling = barState({ ...base, link: "connecting" });
  assert.deepEqual([dialling.label, dialling.retryAt], ["hub", null], "a connection to the hub being made is not a wait");
  const device = barState({ ...base, channelRetryAt: 8_000 });
  assert.deepEqual([device.label, device.retryAt], ["deviceRetry", 8_000]);
  assert.equal(device.notice, true, "the copy's notice under it, as for any channel that keeps failing");
  const trying = barState({ ...base, channel: "connecting", channelRetryAt: 8_000 });
  assert.deepEqual([trying.label, trying.retryAt], ["connecting", null], "a try started before the backoff was out says its step");
  const plain = barState({ ...base, channel: "connecting" });
  assert.deepEqual([plain.label, plain.retryAt], ["connecting", null]);
  // What outranks a wait: live, the hub's word that the device is offline, a refusal, the hub link.
  assert.equal(barState({ ...base, channel: "open", mode: "live", linkRetryAt: 5_000, channelRetryAt: 8_000 }).label, "e2e");
  const offline = barState({ ...base, presence: "offline", channelRetryAt: 8_000 });
  assert.deepEqual([offline.label, offline.retryAt], ["offline", null], "the device is offline: nothing is retried, so nothing is counted down");
  assert.equal(barState({ ...base, refusal: "notAccepted", channelRetryAt: 8_000 }).label, "refused");
  assert.equal(barState({ ...base, link: "closed", linkRetryAt: 5_000, channelRetryAt: 8_000 }).label, "hubRetry", "the hub first: no channel is tried without it");
});

test("the countdown rounds up and never says 0 s while a try is still waited for", () => {
  assert.equal(secondsUntil(12_000, 0), 12);
  assert.equal(secondsUntil(12_000, 999), 12, "11.001 s left reads 12");
  assert.equal(secondsUntil(12_000, 1_000), 11);
  assert.equal(secondsUntil(12_000, 11_999), 1);
  assert.equal(secondsUntil(12_000, 12_000), 1, "due now: still 1 s, not 0");
  assert.equal(secondsUntil(12_000, 15_000), 1, "overdue (timers held in a background tab): still 1 s");
});

// The words are short enough to show whole at 390 px beside a short device
// name and the device switcher, in every language: the UI audit's cloud-bar
// fixture measures that.
test("the bar's words for each state, in English, Chinese and Japanese", () => {
  const now = Date.now();
  const say = (node: React.ReactElement, lng = "en") => draw(node, lng).replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim();
  assert.equal(say(<ConnectionLabel label="hub" />), "reaching the hub…");
  assert.equal(say(<ConnectionLabel label="hubRetry" retryAt={now + 12_000} />), "no hub · retry 12 s");
  assert.equal(say(<ConnectionLabel label="connecting" step={{ kind: "presence" }} />), "finding the device…");
  assert.equal(say(<ConnectionLabel label="connecting" step={{ kind: "relay", city: null }} />), "reaching a relay…");
  assert.equal(say(<ConnectionLabel label="connecting" step={{ kind: "relay", city: "tokyo" }} />), "Tokyo relay…");
  assert.equal(say(<ConnectionLabel label="connecting" step={{ kind: "relay", city: "san-francisco" }} />), "San Francisco relay…");
  assert.equal(say(<ConnectionLabel label="connecting" step={{ kind: "handshake" }} />), "handshake…");
  assert.equal(say(<ConnectionLabel label="connecting" />), "connecting…", "between steps");
  assert.equal(say(<ConnectionLabel label="deviceRetry" retryAt={now + 7_400} />), "no reply · retry 8 s");
  assert.equal(say(<ConnectionLabel label="offline" />), "device offline", "as before");
  // A refusal's short label: the notice under the bar says it in full, and so does the device switcher.
  assert.equal(say(<ConnectionLabel label="refused" />), "refuses this browser");
  assert.equal(say(<ConnectionLabel label="unconfirmed" />), "confirmation pending");
  assert.equal(say(<ConnectionLabel label="unsaved" />), "access not saved");
  assert.match(draw(<ConnectionLabel label="refused" />), /data-signal="warn"/);
  assert.equal(draw(<ConnectionLabel label="e2e" />), "", "live: no words, the path indicator says it");

  assert.equal(say(<ConnectionLabel label="hub" />, "zh-CN"), "正在连接云端…");
  assert.equal(say(<ConnectionLabel label="hubRetry" retryAt={now + 12_000} />, "zh-CN"), "连不上云端 · 12 秒后重试");
  assert.equal(say(<ConnectionLabel label="connecting" step={{ kind: "presence" }} />, "zh-CN"), "正在查找设备…");
  assert.equal(say(<ConnectionLabel label="connecting" step={{ kind: "relay", city: "tokyo" }} />, "zh-CN"), "正在连接东京中继…");
  assert.equal(say(<ConnectionLabel label="connecting" step={{ kind: "relay", city: null }} />, "zh-CN"), "正在连接中继…");
  assert.equal(say(<ConnectionLabel label="connecting" step={{ kind: "handshake" }} />, "zh-CN"), "握手中…");
  assert.equal(say(<ConnectionLabel label="deviceRetry" retryAt={now + 7_400} />, "zh-CN"), "连不上设备 · 8 秒后重试");
  assert.equal(say(<ConnectionLabel label="refused" />, "zh-CN"), "不接受这个浏览器");
  assert.equal(say(<ConnectionLabel label="unconfirmed" />, "zh-CN"), "等待确认");
  assert.equal(say(<ConnectionLabel label="unsaved" />, "zh-CN"), "存不下连接记录");

  assert.equal(say(<ConnectionLabel label="hubRetry" retryAt={now + 12_000} />, "ja"), "ハブ不通 · 12 秒で再試行");
  assert.equal(say(<ConnectionLabel label="connecting" step={{ kind: "relay", city: "san-francisco" }} />, "ja"), "サンフランシスコの中継…");
  assert.equal(say(<ConnectionLabel label="deviceRetry" retryAt={now + 15_600} />, "ja"), "応答なし · 16 秒で再試行");
  assert.equal(say(<ConnectionLabel label="refused" />, "ja"), "操作を受け付けません");
  assert.equal(say(<ConnectionLabel label="unconfirmed" />, "ja"), "確認待ち");
  assert.equal(say(<ConnectionLabel label="unsaved" />, "ja"), "記録を保存できません");

  const html = draw(<ConnectionLabel label="deviceRetry" retryAt={now + 7_400} />);
  assert.match(html, /<button type="button" data-testid="retry-now"[^>]*aria-label="retry now · next try in 8 s"/, "a real button, named for what it does and when the try comes");
  assert.match(html, /title="retry now · next try in 8 s"/);
  assert.match(draw(<ConnectionLabel label="deviceRetry" retryAt={now + 7_400} />, "zh-CN"), /aria-label="立即重试 · 8 秒后自动重试"/);
  assert.doesNotMatch(html, /outline-none/, "with the page's own focus ring");
  assert.doesNotMatch(draw(<ConnectionLabel label="connecting" step={{ kind: "handshake" }} />), /<button/, "nothing to retry while a try is under way");
  assert.match(draw(<ConnectionLabel label="offline" />), /data-signal="offline"/);
});

test("every new string is in every shipped language", () => {
  const keys = [
    "reaching the hub…",
    "no hub",
    "no reply",
    "finding the device…",
    "{{city}} relay…",
    "reaching a relay…",
    "handshake…",
    "retry {{n}} s",
    "retry now · next try in {{n}} s",
    "refuses this browser",
    "confirmation pending",
    "access not saved",
  ];
  for (const [lang, table] of Object.entries({ "zh-CN": zhCN, "zh-TW": zhTW, ja })) {
    for (const key of keys) {
      const value = (table as Record<string, string>)[key];
      assert.ok(value, `${lang} has "${key}"`);
      for (const slot of key.match(/\{\{\w+\}\}/g) ?? []) assert.ok(value.includes(slot), `${lang} keeps ${slot} in "${key}"`);
    }
  }
  // The relay's city is said in each language's own name for it.
  for (const table of [cloudDevicesZhCN, cloudDevicesZhTW, cloudDevicesJa]) assert.ok((table as Record<string, string>).Tokyo);
});

// ---------------------------------------------------------------------------
// The countdown and the button, in a DOM
// ---------------------------------------------------------------------------

let root: ReturnType<typeof createRoot> | null = null;
afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  mock.timers.reset();
});

async function mount(node: React.ReactElement): Promise<void> {
  root = createRoot(container);
  await act(async () => root!.render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>));
}

const button = () => container.querySelector<HTMLButtonElement>('[data-testid="retry-now"]');

test("the countdown ticks down once a second, on the second, and stays at 1 s until the try", async () => {
  void i18n.changeLanguage("en");
  mock.timers.enable({ apis: ["setTimeout", "Date"], now: 100_000 });
  await mount(<ConnectionLabel label="deviceRetry" retryAt={100_000 + 2_500} controller={{ poke: () => undefined }} />);
  const shown: string[] = [button()!.textContent!];
  const tick = async (ms: number) => {
    await act(async () => mock.timers.tick(ms));
    shown.push(button()!.textContent!);
  };
  await tick(499);
  await tick(1);
  await tick(999);
  await tick(1);
  await tick(1_000);
  await tick(5_000);
  assert.deepEqual(shown, ["retry 3 s", "retry 3 s", "retry 2 s", "retry 2 s", "retry 1 s", "retry 1 s", "retry 1 s"]);
  assert.equal(button()!.getAttribute("aria-label"), "retry now · next try in 1 s", "its name counts down with it");
});

test("\"retry now\" tries at once: the hub link's wake while the hub cannot be reached, the channel's poke while the device cannot", async () => {
  void i18n.changeLanguage("en");
  const calls: string[] = [];
  const link = { wake: () => void calls.push("wake") };
  const controller = { poke: () => void calls.push("poke") };
  const click = async () => {
    const b = button();
    assert.ok(b, "the button is there");
    assert.match(b.getAttribute("aria-label") ?? "", /^retry now · next try in [1-9]\d* s$/);
    await act(async () => void b.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true })));
  };

  await mount(<ConnectionLabel label="hubRetry" retryAt={Date.now() + 12_000} link={link} controller={controller} />);
  await click();
  assert.deepEqual(calls, ["wake"]);
  await act(async () => root!.unmount());

  await mount(<ConnectionLabel label="deviceRetry" retryAt={Date.now() + 8_000} link={link} controller={controller} />);
  await click();
  assert.deepEqual(calls, ["wake", "poke"]);
});
