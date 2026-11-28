// The connection log on screen (`features/cloud/connection-log.tsx`), driven
// in a real DOM: opened, it lists what was logged — time, name, fields — and
// takes in what is logged while it is open; "problems" keeps the warnings and
// errors alone; Copy puts the view on the clipboard with its header and
// nothing that names a device; Clear forgets it; and the ways to it (the bar's
// icon, the row under 「这台设备」) say what they open, in every language.
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

let copied: string | null = null;
Object.defineProperty(window.navigator, "clipboard", {
  configurable: true,
  value: {
    writeText: async (text: string) => {
      copied = text;
    },
  },
});

const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { I18nextProvider } = await import("react-i18next");
const { default: i18n } = await import("./i18n-all.js");
const { ConnectionLog, maskId } = await import("../src/lib/connection-log.js");
const { ConnectionLogButton, ConnectionLogDialog, ConnectionLogHost, headerNow, openConnectionLog } = await import("../src/features/cloud/connection-log.js");
const { ConnectionLogSetting } = await import("../src/features/cloud/this-device.js");

type Root = ReturnType<typeof createRoot>;
let root: Root | null = null;
const container = document.body.appendChild(document.createElement("div"));
afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  await i18n.changeLanguage("en");
});

async function render(node: React.ReactNode): Promise<void> {
  root = createRoot(container);
  await act(async () => root!.render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>));
}

const byTestId = (id: string) => document.querySelector<HTMLElement>(`[data-testid="${id}"]`);
const rows = () => [...document.querySelectorAll<HTMLElement>("[data-testid='connection-log-list'] li[data-event]")];
const click = (el: Element | null) =>
  act(async () => {
    assert.ok(el, "the control is on the page");
    el.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
  });
const settle = (ms = 20) => act(async () => new Promise((resolve) => setTimeout(resolve, ms)));

/** A log with a weak network's minute in it: a channel through a relay, a probe that went unanswered, the close, the way back. */
function incident() {
  const log = new ConnectionLog();
  const device = maskId("m-studio");
  log.emit("hub.open", { attempt: 1, ms: 180 });
  log.emit("channel.open", { device, on: "ch:nrt-1/3", via: "relay:nrt-1", handshakeMs: 412 });
  log.emit("sample", { "ch:nrt-1/3.rx": 131072, "ch:nrt-1/3.inflight": 524288, "hub.buf": 0 });
  log.emit("probe.sent", { on: "ch:nrt-1/3", path: "relay", nonce: 7 });
  log.emit("probe.late", { on: "ch:nrt-1/3", path: "relay", nonce: 7, ageMs: 5001, quietMs: 40, unsent: false });
  log.emit("timeout", { device, timer: "RELAY_SILENT_MS", limitMs: 10_000, elapsedMs: 10_002, on: "ch:nrt-1/3" });
  log.emit("channel.close", { device, on: "ch:nrt-1/3", code: 4000, by: "local", rule: "relay_silent", rxBytes: 4_194_304 });
  log.emit("channel.retry", { device, waitMs: 2000, attempt: 1 });
  return log;
}

test("opened, the log lists every event with its time, name and fields, and takes in what is logged while it is open", async () => {
  const log = incident();
  await render(<ConnectionLogDialog open onOpenChange={() => undefined} log={log} connection={() => ({ hub: "open" })} />);
  await settle();
  assert.equal(document.querySelector("[role=dialog] h2")?.textContent, "Connection log");
  // It opens on the events, without the every-second samples and probes: what a copy is for.
  assert.equal(byTestId("connection-log-filter-events")?.getAttribute("aria-selected"), "true");
  assert.deepEqual(
    rows().map((r) => r.dataset.event),
    ["hub.open", "channel.open", "probe.late", "timeout", "channel.close", "channel.retry"],
  );
  assert.match(byTestId("connection-log-count")?.textContent ?? "", /^6 of 8 events$/);
  await click(byTestId("connection-log-filter-all"));
  await settle();
  const names = rows().map((r) => r.dataset.event);
  assert.deepEqual(names, ["hub.open", "channel.open", "sample", "probe.sent", "probe.late", "timeout", "channel.close", "channel.retry"]);
  const before = rows()[0];
  await settle(650);
  assert.equal(rows()[0], before, "a refresh keeps the rows it drew (keyed by seq), it does not draw them anew");
  const timeout = rows().find((r) => r.dataset.event === "timeout")!;
  assert.match(timeout.textContent ?? "", /^\d\d:\d\d:\d\d\.\d{3} \+\d+\.\d{3} timeout .*timer=RELAY_SILENT_MS limitMs=10000 elapsedMs=10002/);
  assert.equal(timeout.querySelector("[data-signal]")?.getAttribute("data-signal"), "failed", "an error in the danger colour, marked as a signal");
  assert.equal(rows().find((r) => r.dataset.event === "probe.late")!.querySelector("[data-signal]")?.getAttribute("data-signal"), "warn");
  assert.match(byTestId("connection-log-count")?.textContent ?? "", /^8 of 8 events$/);

  // Live: what comes while it is open shows within half a second.
  log.emit("channel.open", { on: "ch:nrt-1/4", via: "relay:nrt-1", sinceLastCloseMs: 2100 });
  await settle(650);
  assert.equal(rows().at(-1)?.dataset.event, "channel.open");
  assert.match(rows().at(-1)?.textContent ?? "", /sinceLastCloseMs=2100/);
});

test("'problems' keeps the warnings and errors; Copy puts the view on the clipboard with its header; Clear forgets the log", async () => {
  const log = incident();
  await render(<ConnectionLogDialog open onOpenChange={() => undefined} log={log} connection={() => ({ hub: "open", device: "#3fa9c1", via: "relay:nrt-1" })} />);
  await settle();
  await click(byTestId("connection-log-filter-problems"));
  await settle();
  assert.deepEqual(
    rows().map((r) => r.dataset.event),
    ["probe.late", "timeout", "channel.close"],
  );
  await click(byTestId("connection-log-copy"));
  await settle();
  assert.ok(copied, "something was copied");
  const lines = copied.split("\n");
  assert.match(lines[0]!, /^Droppen connection log · web build \S+ · device agent - · exported \d{4}-\d\d-\d\dT/);
  assert.match(lines[2]!, /^browser: /);
  assert.match(lines[3]!, /^network: online=/);
  assert.equal(lines[4], "connection: hub=open device=#3fa9c1 via=relay:nrt-1");
  assert.equal(lines[5], "events: 3 of 8 (problems)");
  assert.match(lines[6]!, /^privacy: device ids are salted hashes; /);
  assert.equal(lines.filter((l) => / [IWE] /.test(l)).length, 3, "the three problems, one line each");
  assert.ok(!copied.includes("m-studio"), "no device id");
  assert.match(byTestId("connection-log-copy")?.textContent ?? "", /copied/);

  await click(byTestId("connection-log-clear"));
  await settle(650);
  assert.equal(log.size, 0);
  assert.equal(rows().length, 0);
  assert.match(byTestId("connection-log-list")?.textContent ?? "", /Nothing logged yet\./);
});

test("the ways to the log say what they open, in every language, and open the page's one dialog", async () => {
  await render(
    <>
      <ConnectionLogButton />
      <ConnectionLogSetting onOpen={openConnectionLog} />
      <ConnectionLogHost />
    </>,
  );
  const button = byTestId("connection-log-open")!;
  assert.equal(button.getAttribute("aria-label"), "connection log");
  assert.match(button.className, /max-sm:hidden/, "the strip's icon only where the strip has room");
  assert.match(byTestId("connection-log-setting")?.textContent ?? "", /^Connection log.*copy it or download it to send\.open$/);
  for (const [lang, title, open] of [
    ["zh-CN", "连接日志", "打开"],
    ["zh-TW", "連線日誌", "打開"],
    ["ja", "接続ログ", "開く"],
  ] as const) {
    await act(async () => void (await i18n.changeLanguage(lang)));
    assert.equal(byTestId("connection-log-open")!.getAttribute("aria-label"), title, lang);
    const row = byTestId("connection-log-setting")!.textContent ?? "";
    assert.ok(row.startsWith(title) && row.endsWith(open), `${lang}: ${row}`);
    assert.ok(!/[A-Za-z]{4,}/.test(row), `${lang} has no English left: ${row}`);
  }
  await act(async () => void (await i18n.changeLanguage("en")));

  // Both open the one dialog the page has.
  assert.equal(byTestId("connection-log"), null, "closed at first");
  await click(byTestId("connection-log-open"));
  await settle();
  assert.ok(byTestId("connection-log"), "the bar's icon opens it");
  await click(document.querySelector("[aria-label='close']"));
  await settle(200);
  assert.equal(byTestId("connection-log"), null, "closed again");
  await click(byTestId("connection-log-setting-open"));
  await settle();
  assert.ok(byTestId("connection-log"), "and so does 「这台设备」's row");
  assert.equal(document.querySelectorAll("[data-testid='connection-log']").length, 1, "one dialog");
});

test("headerNow runs build, agentVersion, and userAgent through ConnectionLog.clean", () => {
  const log = new ConnectionLog();
  const deviceId = "Dev-Ada-Studio-99";
  log.hide(deviceId);
  const bad = headerNow(
    log,
    "all",
    () => null,
    () => "192.168.1.23",
    () => `${deviceId} studio.tail1234.ts.net`,
    () => `Mozilla/5.0 (${deviceId}; 2001:db8:85a3::1; https://ada.example.com/secret)`,
  );
  assert.equal(bad.agentVersion, null, "an IP is not an agent version");
  assert.ok(!bad.build.includes(deviceId) && !bad.build.includes("studio") && !bad.build.includes("tail1234"), bad.build);
  assert.ok(!bad.userAgent.includes(deviceId) && !bad.userAgent.includes("2001:db8") && !bad.userAgent.includes("ada.example.com"), bad.userAgent);
  assert.ok(bad.userAgent.includes(maskId(deviceId)!), "hidden ids are masked in header fields too");

  const good = headerNow(
    log,
    "events",
    () => null,
    () => "0.2.143",
    () => "index-B3x9aQ",
    () => "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/131.0.0.0 Safari/537.36",
  );
  assert.equal(good.build, "index-B3x9aQ");
  assert.equal(good.agentVersion, "0.2.143");
  assert.equal(good.userAgent, "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/131.0.0.0 Safari/537.36");
});
