// Adding a device without anyone to walk you through it, and seeing devices
// side by side (droppen.app's 「添加可运行 AI 的设备」 guide, the hints, the
// split screen it makes) — and the local page's first-run card.
//
// What these pin: the guide says how to install Droppen (the agreed command,
// the download page), how the device signs itself in (its own page, 「登录
// Droppen」, the droppen.app window, a browser new to the account paired from
// here), and waits by watching the shell's list — never dialling; a second
// device turns into 「放进同一个屏幕」, which makes a screen on the main device
// with one pane per device and opens it; the hints show with one device, the
// tip with two, each put away for good; and the local card shows only for a
// device never signed in, on the local page only.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AddComputer, NewDeviceReady } from "../src/features/cloud/add-computer.js";
import { setHubShell } from "../src/features/cloud/hub-shell.js";
import { addAsked, dismissHint, hintDismissed, openAddDevice, partnerFor, watchNewHost } from "../src/features/cloud/onboarding.js";
import { AddDeviceHint, SideBySideTip } from "../src/features/cloud/onboarding-hints.js";
import { makeSideBySide, paneFor, type ScreenMaker, SideBySideError, sideBySideLayout, whenLive, whenReady } from "../src/features/cloud/side-by-side.js";
import { APP_DOWNLOAD_URL, DOWNLOAD_PATH, INSTALL_COMMAND, otherHub, RELEASES_URL, UNINSTALL_COMMAND } from "../src/features/landing/get-droppen.js";
import { LinkFirstRun } from "../src/features/link/first-run.js";
import { cloudKey } from "../src/features/link/link-flow.js";
import { LOCAL } from "../src/lib/machines/key.js";
import type { MachineInfo } from "../src/lib/machines/runtime.js";
import { constantStore } from "../src/lib/machines/store.js";
import { cloudDevicesZhCN } from "../src/i18n/cloud-devices.js";
import { linkZhCN } from "../src/i18n/link.js";
import { onboardingJa, onboardingZhCN, onboardingZhTW } from "../src/i18n/onboarding.js";
import { zhCN } from "../src/i18n/zh-CN.js";

class MemoryStorage {
  values = new Map<string, string>();
  getItem(key: string) {
    return this.values.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    this.values.set(key, value);
  }
  removeItem(key: string) {
    this.values.delete(key);
  }
}
const storage = new MemoryStorage();
Object.assign(globalThis, { localStorage: storage });

const i18n = i18next.createInstance();
void i18n.use(initReactI18next).init({
  lng: "en",
  resources: { en: { translation: {} }, "zh-CN": { translation: { ...onboardingZhCN, ...linkZhCN, ...cloudDevicesZhCN, ...zhCN } } },
  interpolation: { escapeValue: false },
  keySeparator: false,
  nsSeparator: false,
});

function draw(node: React.ReactElement, lng = "en"): string {
  void i18n.changeLanguage(lng);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } } });
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={client}>{node}</QueryClientProvider>
    </I18nextProvider>,
  );
}

const text = (html: string) => html.replace(/<[^>]+>/g, " ").replace(/&#x27;/g, "'").replace(/&quot;/g, '"').replace(/&amp;/g, "&").replace(/\s+/g, " ");

function info(key: string, state: MachineInfo["state"] = "online", name = key): MachineInfo {
  return { key, name, state, since: 0, canManage: true, live: "copy" };
}

function account(hosts: string[], { admin = true }: { admin?: boolean } = {}) {
  const record = (id: string, host: boolean) => ({ info: { id }, host, admin: !host, status: { type: "active" } });
  const devices = [record("d-0", false), ...hosts.map((id) => record(id, true))];
  return {
    account: "a-1",
    deviceId: "d-0",
    isAdmin: admin,
    session: { token: "t" },
    api: { me: () => new Promise(() => undefined) },
    chain: { isV3: false, hosts: () => devices.filter((d) => d.host), allDevices: () => devices },
    onChange: () => () => undefined,
  };
}

const link = new Proxy({ presence: new Map() } as Record<string, unknown>, { get: (t, k) => (k in t ? t[k as string] : () => () => undefined) });

/** A hub shell of stubs: the machines listed, the main device, and what was asked of it. */
function shell(machines: MachineInfo[], acct = account(machines.map((m) => m.key)), home = machines[0]?.key ?? "m-none") {
  const asked: string[] = [];
  return {
    asked,
    shell: {
      account: acct,
      home,
      lastShown: null,
      machines: () => constantStore(machines),
      machine: () => ({ sessions: constantStore({ machine: "x", rows: null, from: "copy", asOf: null }) }),
      select: (key: string | null) => void asked.push(`select ${key}`),
    },
  };
}

test("the agreed way onto a computer: the install line, the release page, and the uninstall flag", () => {
  assert.equal(INSTALL_COMMAND, "curl -fsSL https://droppen.app/install.sh | sh");
  assert.equal(RELEASES_URL, "https://github.com/getdroppen/droppen/releases/latest");
  assert.equal(UNINSTALL_COMMAND, "curl -fsSL https://droppen.app/install.sh | sh -s -- --uninstall");
  assert.equal(DOWNLOAD_PATH, "/download");
  assert.ok(!otherHub("https://droppen.app") && !otherHub("https://droppen.app/"));
  assert.ok(otherHub("http://127.0.0.1:61135"));
});

test("the guide, before the App is out: install there (the command, the download page's background tab), sign it in from its own page, and wait — every step on screen", () => {
  const { shell: s } = shell([info("m-1")]);
  setHubShell(s as never);
  try {
    const html = draw(<AddComputer account={account(["m-1"]) as never} link={link as never} origin="wss://droppen.app" onDone={() => undefined} pairWith={() => <i>pairing</i>} availability={{ app: false, appSignIn: false, linux: false }} />);
    const words = text(html);
    assert.match(html, /data-testid="add-computer" data-step="waiting"/);
    assert.ok(words.includes(INSTALL_COMMAND), "the command itself");
    assert.match(html, /<a href="\/download\?os=macos-bg" target="_blank"[^>]*data-testid="add-download-link"/, "the download page's tab, in a new tab: the guide keeps waiting");
    assert.doesNotMatch(html, /data-testid="add-app-download"/, "no link to an App that is not out");
    for (const line of [
      "01 · install Droppen on that device",
      "The Droppen App for Mac is coming soon",
      "02 · on its page, sign in to Droppen",
      "click Sign in to Droppen",
      "A droppen.app window opens there: sign in to this account, check that the fingerprint matches",
      "Is that window's browser new to your account?",
      "add a browser or phone",
      "03 · it comes online here",
      "waiting for the new device to come online…",
      "Keep this page open: it notices the new device by itself.",
      "Server or running in the background?",
      "show a one-time code",
    ]) assert.ok(words.includes(line), `missing: ${line}`);
    // The server way: the download page's tabs, and the one-time code.
    assert.match(html, /<a href="\/download\?os=macos-bg"[^>]*data-testid="add-server-macos"/);
    assert.match(html, /<a href="\/download\?os=linux"[^>]*data-testid="add-server-linux"/);
    // On droppen.app, no word about another hub.
    assert.doesNotMatch(html, /add-other-hub/);
    // A hub of its own: the device's page needs it named.
    const other = text(draw(<AddComputer account={account(["m-1"]) as never} link={link as never} origin="ws://127.0.0.1:61135" onDone={() => undefined} availability={{ app: false, appSignIn: false, linux: false }} />));
    assert.ok(other.includes("This account is on http://127.0.0.1:61135, not droppen.app"));
    // zh-CN reads as the app does.
    const zh = text(draw(<AddComputer account={account(["m-1"]) as never} link={link as never} origin="wss://droppen.app" onDone={() => undefined} availability={{ app: false, appSignIn: false, linux: false }} />, "zh-CN"));
    for (const line of ["正在等新设备上线…", "「登录 Droppen」", "「加入这台设备」", "显示一次性注册码", "服务器或后台运行？"]) assert.ok(zh.includes(line), `zh-CN missing: ${line}`);
  } finally {
    setHubShell(null);
  }
});

test("the guide once the App is out but cannot sign itself in yet: the background install and its own page's 「登录 Droppen」, never a step for a button the App does not have", () => {
  const { shell: s } = shell([info("m-1")]);
  setHubShell(s as never);
  try {
    for (const availability of [{ app: true, appSignIn: false, linux: true }, { app: true, appSignIn: false, linux: false }]) {
      const html = draw(<AddComputer account={account(["m-1"]) as never} link={link as never} origin="wss://droppen.app" onDone={() => undefined} availability={availability} />);
      const words = text(html);
      assert.match(html, /data-testid="add-step-install" data-app="false"/);
      assert.ok(words.includes(INSTALL_COMMAND), "the way in that signs in today");
      assert.match(html, /<a href="\/download\?os=macos-bg" target="_blank"[^>]*data-testid="add-download-link"/);
      assert.doesNotMatch(html, /data-testid="add-app-download"/, "the App would leave the device out of the account");
      assert.match(html, /data-testid="add-app-soon"/);
      for (const line of [
        "The Droppen App cannot sign in to Droppen yet (that is coming in an update): to add a Mac now, run Droppen in the background.",
        "02 · on its page, sign in to Droppen",
        "sign in with its local login and click Sign in to Droppen",
      ]) assert.ok(words.includes(line), `missing: ${line}`);
      for (const line of ["02 · in the App", "In the Droppen App on that device", "The Droppen App for Mac is coming soon"]) assert.ok(!words.includes(line), `still says: ${line}`);
    }
    const zh = text(draw(<AddComputer account={account(["m-1"]) as never} link={link as never} origin="wss://droppen.app" onDone={() => undefined} availability={{ app: true, appSignIn: false, linux: true }} />, "zh-CN"));
    assert.ok(zh.includes("Droppen App 暂时还不能登录 Droppen（会在后续更新中推出）"));
    assert.ok(zh.includes("02 · 在它的页面上登录 Droppen"));
    assert.ok(!zh.includes("在 App 里点「登录 Droppen」"));
  } finally {
    setHubShell(null);
  }
});

test("the guide once the App is out and signs itself in: step 1 is the App's download, step 2 「在 App 里点「登录 Droppen」」; the server way stays second", () => {
  const { shell: s } = shell([info("m-1")]);
  setHubShell(s as never);
  try {
    const html = draw(<AddComputer account={account(["m-1"]) as never} link={link as never} origin="wss://droppen.app" onDone={() => undefined} availability={{ app: true, appSignIn: true, linux: false }} />, "zh-CN");
    const words = text(html);
    assert.match(html, new RegExp(`<a href="${APP_DOWNLOAD_URL.replace(/[.?]/g, "\\$&")}"[^>]*data-testid="add-app-download"`));
    assert.match(html, /<a href="\/download\?os=macos-app"[^>]*data-testid="add-download-link"/);
    for (const line of ["01 · 在那台设备上安装 Droppen App", "下载 Droppen App（macOS）", "02 · 在 App 里点「登录 Droppen」", "不需要登录页", "服务器或后台运行？", "显示一次性注册码"]) assert.ok(words.includes(line), `missing: ${line}`);
    assert.doesNotMatch(html, /data-testid="install-command"/, "the command is the server way's, behind its link");
  } finally {
    setHubShell(null);
  }
});

test("the last step watches the shell's list: a device the account did not have, then it online — or, past patience, as it is", () => {
  const before = new Set(["m-1"]);
  assert.deepEqual(watchNewHost({ before, machines: [info("m-1")] }), { kind: "waiting" });
  assert.equal(watchNewHost({ before, machines: [info("m-1"), info("m-2", "unknown")], joinedAt: 1_000, now: 2_000 }).kind, "joined");
  assert.equal(watchNewHost({ before, machines: [info("m-1"), info("m-2", "online")] }).kind, "ready");
  const late = watchNewHost({ before, machines: [info("m-1"), info("m-2", "offline")], joinedAt: 1_000, now: 30_000 });
  assert.equal(late.kind, "ready");
  assert.equal(late.kind === "ready" && late.info.key, "m-2");
  // Of several new ones, an online one first.
  const several = watchNewHost({ before, machines: [info("m-1"), info("m-2", "unknown"), info("m-3", "online")] });
  assert.equal(several.kind === "ready" && several.info.key, "m-3");
  // A device leaving is not a new one.
  assert.deepEqual(watchNewHost({ before: new Set(["m-1", "m-2"]), machines: [info("m-1")] }), { kind: "waiting" });
});

test("ready with another device: 「现在可以把两台设备放进同一个屏幕」 and the button; alone: open it", () => {
  const two = [info("m-1", "online", "studio"), info("m-2", "online", "laptop")];
  const { shell: s, asked } = shell(two);
  setHubShell(s as never);
  try {
    const html = draw(<NewDeviceReady account={account(["m-1", "m-2"]) as never} added={two[1]!} machines={two} onDone={() => undefined} />, "zh-CN");
    const words = text(html);
    assert.ok(words.includes("laptop 已上线"));
    assert.ok(words.includes("现在可以把两台设备放进同一个屏幕"));
    assert.ok(words.includes("studio 和 laptop 并排"));
    assert.ok(words.includes("最近的会话"));
    assert.match(html, /data-testid="side-by-side"[^>]*>.*放进同一个屏幕/);
    assert.equal((html.match(/data-variant="primary"/g) ?? []).length, 1, "one primary: put them on one screen");
    const alone = draw(<NewDeviceReady account={account(["m-2"]) as never} added={two[1]!} machines={[two[1]!]} onDone={() => undefined} />);
    assert.doesNotMatch(alone, /data-testid="side-by-side"/);
    assert.match(alone, /data-variant="primary" data-testid="open-new-device"[^>]*>open laptop</);
    assert.ok(text(alone).includes("Add another device the same way to see both on one screen."));
    assert.deepEqual(asked, []);
  } finally {
    setHubShell(null);
  }
});

test("its partner: the device last shown, else the main device, else the first other", () => {
  assert.equal(partnerFor("m-3", { lastShown: "m-2", home: "m-1", machines: ["m-1", "m-2", "m-3"] }), "m-2");
  assert.equal(partnerFor("m-3", { lastShown: null, home: "m-1", machines: ["m-1", "m-2", "m-3"] }), "m-1");
  assert.equal(partnerFor("m-3", { lastShown: "m-3", home: "m-3", machines: ["m-2", "m-3"] }), "m-2");
  assert.equal(partnerFor("m-3", { lastShown: null, home: null, machines: ["m-3"] }), null);
});

test("the split: one pane per device, the main device's naming none (as screens store it); a session, else a terminal, else the picker", () => {
  const layout = sideBySideLayout(paneFor("m-1", { session: { source: "claude", id: "s-1" } }), paneFor("m-2", { terminal: "t-9" }), "m-1");
  assert.equal(layout.type, "split");
  if (layout.type !== "split") return;
  assert.equal(layout.direction, "row");
  assert.equal(layout.ratio, 0.5);
  const [left, right] = layout.children;
  assert.deepEqual(left.type === "leaf" && left.content, { kind: "session", source: "claude", id: "s-1" });
  assert.deepEqual(right.type === "leaf" && right.content, { kind: "terminal", id: "t-9", machine: "m-2" });
  assert.notEqual(left.id, right.id);
  assert.deepEqual(paneFor("m-2", {}), { kind: "picker", machine: "m-2" }, "nothing of it to show: the picker, on it");
});

test("making it: each pane on something of its own device, then the screen on the main device, the layout on the revision it was made at, opened — and every device let go of, however it ends", async () => {
  const calls: string[] = [];
  let released = 0;
  let laid: unknown = null;
  const maker = (opts: { fail?: "post" | "live"; sessions?: Record<string, { source: "claude" | "codex"; id: string }>; terminals?: Record<string, string | null> } = {}): ScreenMaker => ({
    home: "m-1",
    api: {
      POST: (async (path: string, init: { body: unknown }) => {
        calls.push(`POST ${path} ${JSON.stringify(init.body)}`);
        if (opts.fail === "post") return { error: { error: "no room" }, response: new Response(null, { status: 500 }) };
        return { data: { id: "scr-1", revision: 4 }, response: new Response() };
      }) as never,
      PATCH: (async (path: string, init: { params: unknown; body: { baseRevision: number; layout: unknown } }) => {
        calls.push(`PATCH ${path} ${JSON.stringify(init.params)} base ${init.body.baseRevision}`);
        laid = init.body.layout;
        return { data: { id: "scr-1", revision: 5 }, response: new Response() };
      }) as never,
    },
    live: () => (opts.fail === "live" ? Promise.reject(new SideBySideError("unreachable", "x")) : Promise.resolve()),
    newest: async (machine) => opts.sessions?.[machine] ?? null,
    terminal: async (machine) => {
      calls.push(`terminal ${machine}`);
      const id = opts.terminals?.[machine];
      if (id === undefined) throw new SideBySideError("unreachable", "offline");
      return id;
    },
    invalidate: () => void calls.push("invalidate"),
    open: (path) => void calls.push(`open ${path}`),
    release: () => void released++,
  });
  const leaves = () => ((laid as { children: { content: unknown }[] }).children ?? []).map((c) => c.content);
  // A session the copy knows on one device; nothing on the other: a terminal of it.
  assert.equal(await makeSideBySide(maker({ sessions: { "m-1": { source: "claude", id: "s-1" } }, terminals: { "m-2": "t-2" } }), { title: "studio · laptop", left: "m-1", right: "m-2" }), "scr-1");
  assert.deepEqual(calls, ["terminal m-2", 'POST /api/screens {"title":"studio · laptop"}', 'PATCH /api/screens/{id} {"path":{"id":"scr-1"}} base 4', "invalidate", "open /screens/scr-1"]);
  assert.deepEqual(leaves(), [{ kind: "session", source: "claude", id: "s-1" }, { kind: "terminal", id: "t-2", machine: "m-2" }]);
  assert.equal(released, 1);
  // Both have sessions: no terminal is opened anywhere.
  calls.length = 0;
  await makeSideBySide(maker({ sessions: { "m-1": { source: "claude", id: "s-1" }, "m-2": { source: "codex", id: "s-2" } } }), { title: "t", left: "m-1", right: "m-2" });
  assert.ok(!calls.some((c) => c.startsWith("terminal")));
  assert.deepEqual(leaves(), [{ kind: "session", source: "claude", id: "s-1" }, { kind: "session", source: "codex", id: "s-2", machine: "m-2" }]);
  // A device that cannot be reached for a terminal: its pane is the picker, on it.
  await makeSideBySide(maker({ sessions: { "m-1": { source: "claude", id: "s-1" } } }), { title: "t", left: "m-1", right: "m-2" });
  assert.deepEqual(leaves()[1], { kind: "picker", machine: "m-2" });
  calls.length = 0;
  await assert.rejects(makeSideBySide(maker({ fail: "post" }), { title: "t", left: "m-1", right: "m-2" }), /no room/);
  assert.ok(!calls.some((c) => c.startsWith("open")), "nothing opened");
  await assert.rejects(makeSideBySide(maker({ fail: "live" }), { title: "t", left: "m-1", right: "m-2" }), (e: unknown) => e instanceof SideBySideError && e.kind === "unreachable");
  assert.equal(released, 5, "let go of every time");
});

test("waiting for a device's copy: at once when it has one, on its next change, or nothing past the time given", async () => {
  let value: { rows: unknown[] | null } = { rows: null };
  const fns = new Set<() => void>();
  const store = { get: () => value, subscribe: (fn: () => void) => (fns.add(fn), () => fns.delete(fn)) };
  const waiting = whenReady(store, (v) => v.rows !== null, 1_000);
  value = { rows: [] };
  for (const fn of [...fns]) fn();
  assert.deepEqual(await waiting, { rows: [] });
  assert.equal(fns.size, 0, "unsubscribed");
  assert.equal(await whenReady({ get: () => ({ rows: null }), subscribe: () => () => undefined }, (v) => v.rows !== null, 10), null);
});

test("waiting for the main device to be live: at once when it is, on its next switch, or not past the time given", async () => {
  await whenLive({ current: "live", onMode: () => () => undefined });
  let fire: ((mode: "live" | "offline") => void) | null = null;
  const waiting = whenLive({ current: "offline", onMode: (fn) => ((fire = fn), () => (fire = null)) }, 1_000);
  fire!("offline");
  fire!("live");
  await waiting;
  await assert.rejects(whenLive({ current: "offline", onMode: () => () => undefined }, 10), (e: unknown) => e instanceof SideBySideError && e.kind === "unreachable");
});

test("the hint with one device that runs AI, put away for good with its ×; none with two, none for a device that cannot add one", () => {
  storage.values.clear();
  const one = shell([info("m-1")]);
  setHubShell(one.shell as never);
  try {
    const row = draw(<AddDeviceHint variant="row" />, "zh-CN");
    assert.match(row, /data-testid="add-device-hint"/);
    assert.ok(text(row).includes("再加一台设备，就能在一个屏里同时看"));
    assert.ok(text(row).includes("添加可运行 AI 的设备"));
    assert.match(draw(<AddDeviceHint variant="line" />), /data-testid="add-device-hint-add"/);
    dismissHint("add-device", "a-1");
    assert.ok(hintDismissed("add-device", "a-1"));
    assert.equal(draw(<AddDeviceHint variant="row" />), "", "put away");
    assert.ok(!hintDismissed("add-device", "a-2"), "per account");
    storage.values.clear();
    setHubShell(shell([info("m-1")], account(["m-1"], { admin: false })).shell as never);
    assert.equal(draw(<AddDeviceHint variant="row" />), "", "not a device that can add one");
    setHubShell(shell([info("m-1"), info("m-2")]).shell as never);
    assert.equal(draw(<AddDeviceHint variant="row" />), "", "two already");
  } finally {
    setHubShell(null);
  }
  // A device's own page (no hub shell): never.
  assert.equal(draw(<AddDeviceHint variant="row" />), "");
  assert.equal(draw(<SideBySideTip onScreen={() => undefined} />), "");
});

test("with two devices, the side-by-side tip once: a line of its own (never over a pane), with a new screen; put away for good", () => {
  storage.values.clear();
  setHubShell(shell([info("m-1"), info("m-2")]).shell as never);
  try {
    const html = draw(<SideBySideTip onScreen={() => undefined} />, "zh-CN");
    assert.match(html, /data-testid="side-by-side-tip" role="note"/);
    assert.doesNotMatch(html, /class="[^"]*\b(fixed|absolute)\b/, "in the layout, not over it");
    assert.ok(text(html).includes("每个格子都可以放不同设备的内容"));
    assert.match(html, /data-testid="side-by-side-tip-screen"/);
    dismissHint("side-by-side", "a-1");
    assert.equal(draw(<SideBySideTip onScreen={() => undefined} />), "");
    storage.values.clear();
    setHubShell(shell([info("m-1")]).shell as never);
    assert.equal(draw(<SideBySideTip onScreen={() => undefined} />), "", "one device: nothing to put side by side");
  } finally {
    storage.values.clear();
    setHubShell(null);
  }
});

test("the hints' way in: 「我的设备」 with the guide open", () => {
  const selected: (string | null)[] = [];
  setHubShell({ select: (key: string | null) => void selected.push(key) } as never);
  try {
    openAddDevice();
    assert.equal(addAsked.get(), true);
    assert.deepEqual(selected, [null]);
  } finally {
    addAsked.set(false);
    setHubShell(null);
  }
});

test("the onboarding table translates every key into zh-TW and ja too, and says 设备 / 裝置 / デバイス", () => {
  const keys = Object.keys(onboardingZhCN).sort();
  assert.deepEqual(Object.keys(onboardingZhTW).sort(), keys);
  assert.deepEqual(Object.keys(onboardingJa).sort(), keys);
  // The words the user asked for, as they are.
  assert.equal(onboardingZhCN["waiting for the new device to come online…"], "正在等新设备上线…");
  assert.equal(onboardingZhCN["Now you can put the two devices on one screen."], "现在可以把两台设备放进同一个屏幕");
  assert.equal(onboardingZhCN["put them on one screen"], "放进同一个屏幕");
  assert.equal(onboardingZhCN["Add another device to see both on one screen."], "再加一台设备，就能在一个屏里同时看");
  assert.equal(onboardingZhCN["Each pane can show a different device's content."], "每个格子都可以放不同设备的内容");
});

test("the local first-run card: only for a device never signed in to Droppen, with 「登录 Droppen」 and why, and gone once put away", () => {
  const status = (state: string) => ({
    state,
    localAuth: true,
    canManage: true,
    hub: "wss://droppen.app",
    hubWeb: "https://droppen.app",
    owner: null,
    remoteAccess: [],
    aggregator: { enabled: false, state: "off" },
    account: null,
    machine: { hostname: "laptop", id: null, fingerprint: null },
    link: null,
    attempt: null,
  });
  const card = (state: string) => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } } });
    client.setQueryData(cloudKey(LOCAL), status(state));
    void i18n.changeLanguage("zh-CN");
    return renderToStaticMarkup(
      <I18nextProvider i18n={i18n}>
        <QueryClientProvider client={client}>
          <LinkFirstRun />
        </QueryClientProvider>
      </I18nextProvider>,
    );
  };
  storage.values.clear();
  const html = card("none");
  assert.match(html, /data-testid="droppen-first-run"/);
  assert.ok(text(html).includes("登录 Droppen，就能在这里看到你的其它设备，也能在手机上用这一台。"));
  assert.match(html, /data-variant="primary" data-testid="droppen-first-run-sign-in"[^>]*>登录 Droppen</);
  for (const state of ["linked", "enrolling", "not_hosting", "removed", "broken"]) assert.equal(card(state), "", state);
  // Put away ("以后再说"): remembered for this device on this browser.
  storage.setItem("rh.first-run.droppen", "1");
  assert.equal(card("none"), "");
  storage.values.clear();
});
