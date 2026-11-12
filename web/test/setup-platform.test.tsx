// The desktop app's first-run guide on each system the app is built for
// (`lib/desktop-app.ts`): the server says which (`GET /api/auth/status`,
// `platform`; the tools report's `os` for steps 01 and 03), and an older
// server that says none is a Mac's. On Linux the guide names the app's
// edition, says "this device", points at the system tray and leaves out what
// only macOS does (the folder prompt, Apple's installer for git); on a Mac it
// reads as it did, "this device" for "this Mac". Whether the app has Start at
// Login is its window's to say; a page without the window's word takes yes,
// as both apps have it.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { HOME, report, task, tool } from "./setup-fixtures.js";

/** What `GET /api/auth/status` answers: the app's own server. */
let statusAnswer: Record<string, unknown> = {};

// As in `setup-view.test.tsx`: node cannot parse the app's relative URLs, so
// `Request` keeps what the fake server reads. In place before the client loads.
class FakeRequest {
  url: string;
  method: string;
  headers: Headers;
  constructor(url: string, init: { method?: string; headers?: HeadersInit } = {}) {
    this.url = url;
    this.method = init.method ?? "GET";
    this.headers = new Headers(init.headers);
  }
}
(globalThis as unknown as { Request: unknown }).Request = FakeRequest;
(globalThis as unknown as { fetch: unknown }).fetch = async (req: FakeRequest) =>
  req.url === "/api/auth/status"
    ? new Response(JSON.stringify(statusAnswer), { status: 200, headers: { "content-type": "application/json" } })
    : new Response(JSON.stringify({ error: `unexpected request: ${req.url}` }), { status: 404 });

const { default: i18n } = await import("./i18n-all.js");
const { authStatus, forgetAuthStatus } = await import("../src/lib/auth.js");
const { desktopSystem, desktopSystemOf, startAtLoginOffered, trayHostPresent } = await import("../src/lib/desktop-app.js");
const { emptyProgress, complete, start } = await import("../src/features/setup/model.js");
const { WhereStep, WrapStep } = await import("../src/features/setup/screens.js");
const { GuideScreen } = await import("../src/features/setup/welcome-view.js");
const { SetupPage } = await import("../src/features/auth/setup-view.js");
const { GitRow } = await import("../src/features/setup/tools-advanced.js");
const { FirstStepView } = await import("../src/features/setup/first-step.js");
const { EMPTY_FIRST_UI } = await import("../src/features/setup/first-model.js");
const { IDLE_STATUS, ToolsStepView } = await import("../src/features/setup/tools-step.js");
const { IDLE_CARD, IDLE_TOOLS_UI } = await import("../src/features/setup/tools-model.js");
const { LOCAL } = await import("../src/lib/machines/key.js");
const { mq } = await import("../src/lib/machines/query.js");

const noop = () => undefined;
const acts = { ui: noop, run: noop, login: noop, loginWithKey: noop, stop: noop, dismiss: noop, rescan: noop, use: noop, installGit: noop, openAdvanced: noop };

function draw(node: React.ReactNode, lang = "en"): string {
  void i18n.changeLanguage(lang);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } } });
  client.setQueryData(mq(LOCAL, "prefs"), { sessionGrouping: "flat", transcriptStyle: "flat", fontSize: "default", language: lang, composerSend: "enter" });
  const html = renderToStaticMarkup(<QueryClientProvider client={client}>{node}</QueryClientProvider>);
  client.clear();
  void i18n.changeLanguage("en");
  return html;
}

/** The app's own server on `platform` (none: a server from before the field). */
async function onServer(platform: string | undefined): Promise<void> {
  statusAnswer = { authRequired: true, setupRequired: true, authOff: false, user: "droppen-app", desktopApp: true, droppenPath: "/opt/droppen/droppen", ...(platform ? { platform } : {}) };
  forgetAuthStatus();
  await authStatus();
}

/** What only a Mac has, as the guide would say it: never on the Linux app's pages. */
const MAC_ONLY = /\bMacs?\b|macOS|menu.bar|Apple|xcode-select|~\/\.zprofile|~\/\.zshrc|菜单栏|選單列|メニューバー/;

const linuxOs = { platform: "linux", macos: null, arch: "x86_64" };
const started = start(emptyProgress(), "2026-11-09T09:00:00Z");
const atWrap = complete(complete({ ...started, where: { choice: "local" as const, lan: false } }, "tools"), "where");

test("the app's system: Linux as itself, the Mac otherwise — and a server from before `platform` was a Mac's", async () => {
  assert.equal(desktopSystemOf("linux"), "linux");
  assert.equal(desktopSystemOf("darwin"), "mac");
  assert.equal(desktopSystemOf(null), "mac");
  assert.equal(desktopSystemOf(undefined), "mac");
  try {
    await onServer("linux");
    assert.equal(desktopSystem(), "linux");
    await onServer(undefined);
    assert.equal(desktopSystem(), "mac");
  } finally {
    forgetAuthStatus();
  }
});

test("Start at Login: what the app's window says; without its word, yes — the Mac app and the Linux app both have it", () => {
  assert.equal(startAtLoginOffered(true), true);
  assert.equal(startAtLoginOffered(false), false);
  assert.equal(startAtLoginOffered(undefined), true, "a browser on the device");
});

test("the tray host: what the app's window says; without its word, yes — a browser on the device", () => {
  assert.equal(trayHostPresent(true), true);
  assert.equal(trayHostPresent(false), false);
  assert.equal(trayHostPresent(undefined), true, "a browser on the device");
  assert.equal(inWindow(false, () => trayHostPresent(), false), false, "the window's word, for a page the platform cannot tell");
});

/** Draw with the app's window saying `startAtLogin` and `trayHost` (`undefined`: no window, a browser on the device). */
function inWindow<T>(startAtLogin: boolean | undefined, fn: () => T, trayHost?: boolean): T {
  const global = globalThis as unknown as { window?: unknown };
  const bridge: Record<string, boolean> = {};
  if (startAtLogin !== undefined) bridge.startAtLogin = startAtLogin;
  if (trayHost !== undefined) bridge.trayHost = trayHost;
  if (Object.keys(bridge).length) global.window = { __droppenDesktop: bridge };
  try {
    return fn();
  } finally {
    delete global.window;
  }
}

test("on Linux with no tray host, the guide points at Settings → Desktop app and step 04 says closing quits", async () => {
  try {
    await onServer("linux");
    // The Linux app's window: Start at Login, and no tray host.
    const noTray = <T,>(fn: () => T): T => inWindow(true, fn, false);
    const noTrayWords = /system tray|closing the window doesn.t stop it|startup applications|系统托盘|关掉窗口不会停止它|启动应用程序|系統匣|關掉視窗不會停止它|啟動應用程式|システムトレイ|ウインドウを閉じても止まりません|スタートアップアプリケーション/;
    for (const lang of ["en", "zh-CN", "zh-TW", "ja"]) {
      const where = noTray(() => draw(<WhereStep progress={started} tools={undefined} online={false} onChange={noop} />, lang));
      assert.doesNotMatch(where, noTrayWords, `step 02 in ${lang}`);
      const wrap = noTray(() => draw(<WrapStep progress={atWrap} tools={undefined} online onChange={noop} />, lang));
      assert.match(wrap, /data-testid="wrap-login"/, `Start at Login stays offered in ${lang}`);
      assert.doesNotMatch(wrap, noTrayWords, `step 04 in ${lang}`);
      const setup = noTray(() => draw(<SetupPage owner={false} />, lang));
      assert.doesNotMatch(setup, noTrayWords, `the setup page in ${lang}`);
    }
    const where = noTray(() => draw(<WhereStep progress={started} tools={undefined} online={false} onChange={noop} />));
    assert.match(where, /choose Open in Browser in Settings → Desktop app: it opens signed in\./);
    const wrap = noTray(() => draw(<WrapStep progress={atWrap} tools={undefined} online onChange={noop} />));
    assert.match(wrap, /Closing the window quits Droppen; start it again from the app launcher\./);
    assert.match(wrap, /Turn it on or off any time in Settings → Desktop app\./);
    assert.match(noTray(() => draw(<WrapStep progress={atWrap} tools={undefined} online onChange={noop} />, "zh-CN")), /关掉窗口会退出 Droppen/);
    assert.match(noTray(() => draw(<WrapStep progress={atWrap} tools={undefined} online onChange={noop} />, "zh-TW")), /關掉視窗會結束 Droppen/);
    assert.match(noTray(() => draw(<WrapStep progress={atWrap} tools={undefined} online onChange={noop} />, "ja")), /ウインドウを閉じると Droppen が終了します/);
    assert.match(noTray(() => draw(<WrapStep progress={atWrap} tools={undefined} online onChange={noop} />, "zh-CN")), /随时可以在 设置 → 桌面 App 里打开或关闭。/);
    const setup = noTray(() => draw(<SetupPage owner={false} />));
    assert.match(setup, /In Settings → Desktop app, choose Open in Browser/);
    assert.match(setup, /Or create the account on this device, without a code:/);
    // The note follows the platform, not the tray: a browser on Linux reads it too.
    assert.match(draw(<WhereStep progress={started} tools={undefined} online={false} onChange={noop} />), /choose Open in Browser in Settings → Desktop app/);
  } finally {
    forgetAuthStatus();
  }
});

test("on Linux the guide never names the Mac: the edition, this device, the system tray", async () => {
  try {
    await onServer("linux");
    for (const said of [true, undefined]) {
      for (const lang of ["en", "zh-CN", "zh-TW", "ja"]) {
        const welcome = inWindow(said, () => draw(<GuideScreen screen="welcome" progress={emptyProgress()} tools={report(tool("claude", "ready"), tool("codex", "missing"), { os: linuxOs })} online onLater={noop} />, lang));
        const where = inWindow(said, () => draw(<WhereStep progress={started} tools={undefined} online={false} onChange={noop} />, lang));
        const wrap = inWindow(said, () => draw(<WrapStep progress={atWrap} tools={undefined} online onChange={noop} />, lang));
        for (const [name, html] of Object.entries({ welcome, where, wrap })) assert.doesNotMatch(html, MAC_ONLY, `${name} in ${lang}, window says ${said}`);
        assert.match(wrap, /data-testid="wrap-login"/, `Start at Login in ${lang}, window says ${said}`);
      }
    }
    const welcome = draw(<GuideScreen screen="welcome" progress={emptyProgress()} tools={undefined} online onLater={noop} />);
    assert.match(welcome, /Droppen for Linux/);
    assert.match(welcome, /Droppen is running on this device\./);
    assert.match(welcome, /Only on this device, or on your phone and other devices too\./);
    assert.match(welcome, /Start at login, notifications, your phone\./);
    assert.match(draw(<GuideScreen screen="welcome" progress={emptyProgress()} tools={undefined} online onLater={noop} />, "zh-CN"), /Droppen · Linux 版/);
    const where = draw(<WhereStep progress={started} tools={undefined} online={false} onChange={noop} />);
    assert.match(where, /choose Open in Browser in Settings → Desktop app: it opens signed in\./);
    assert.match(where, /Choose “Only on this device” for now/);
    assert.match(draw(<WhereStep progress={started} tools={undefined} online onChange={noop} />, "zh-CN"), /在 设置 → 桌面 App 里选「在浏览器中打开」/);
    // The Linux app's window, and a browser on the device: the row points at the system tray and the section.
    for (const said of [true, undefined]) {
      const wrap = inWindow(said, () => draw(<WrapStep progress={atWrap} tools={undefined} online onChange={noop} />));
      assert.match(wrap, /Droppen lives in the system tray; closing the window doesn&#x27;t stop it\./, `window says ${said}`);
      assert.match(wrap, /Turn it on or off any time from the Droppen icon in the system tray or in Settings → Desktop app\./);
      assert.match(wrap, /data-testid="wrap-phone"/);
      assert.match(wrap, /Only on this device/);
      assert.match(inWindow(said, () => draw(<WrapStep progress={atWrap} tools={undefined} online onChange={noop} />, "zh-CN")), /随时可以在系统托盘的 Droppen 图标或 设置 → 桌面 App 里打开或关闭。/);
    }
  } finally {
    forgetAuthStatus();
  }
});

test("on a Mac, and from a server too old to say, the guide reads as before, with “this device” for “this Mac”", async () => {
  try {
    for (const platform of ["darwin", undefined]) {
      await onServer(platform);
      const welcome = draw(<GuideScreen screen="welcome" progress={emptyProgress()} tools={undefined} online onLater={noop} />);
      assert.match(welcome, /Droppen for Mac/, `${platform}`);
      assert.match(welcome, /Droppen is running on this device\./);
      assert.match(welcome, /Start at login, notifications, your phone\./);
      assert.match(draw(<WhereStep progress={started} tools={undefined} online onChange={noop} />), /choose Open in Browser in the app&#x27;s menu-bar menu or in Settings → Desktop app/);
      const wrap = draw(<WrapStep progress={atWrap} tools={undefined} online onChange={noop} />);
      assert.match(wrap, /data-testid="wrap-login"/);
      assert.match(wrap, /reach this device after a restart\. Droppen lives in the menu bar/);
      assert.match(wrap, /Turn it on or off any time from the Droppen icon in the menu bar or in Settings → Desktop app\./);
      assert.match(draw(<WrapStep progress={atWrap} tools={undefined} online onChange={noop} />, "zh-CN"), /随时可以在菜单栏的 Droppen 图标或 设置 → 桌面 App 里打开或关闭。/);
    }
  } finally {
    forgetAuthStatus();
  }
});

test("an app that says it has no Start at Login gets no row, and the welcome line for step 04 names the phone and the settings", async () => {
  try {
    for (const platform of ["linux", "darwin"]) {
      await onServer(platform);
      const wrap = inWindow(false, () => draw(<WrapStep progress={atWrap} tools={undefined} online onChange={noop} />));
      assert.doesNotMatch(wrap, /data-testid="wrap-login"|Start at Login/, platform);
      assert.match(wrap, /data-testid="wrap-phone"/);
      const welcome = (lang: string) => inWindow(false, () => draw(<GuideScreen screen="welcome" progress={emptyProgress()} tools={undefined} online onLater={noop} />, lang));
      assert.match(welcome("en"), /Your phone, and a last look at your settings\./, platform);
      assert.doesNotMatch(welcome("en"), /Start at login/);
      assert.match(welcome("zh-CN"), /手机，再确认一下设置。/);
      assert.match(welcome("zh-TW"), /手機，再確認一下設定。/);
      assert.match(welcome("ja"), /スマートフォンと、設定の最終確認。/);
    }
  } finally {
    forgetAuthStatus();
  }
});

test("git on Linux: the system's package manager and a look again, never Apple's installer", () => {
  const git = (system: "mac" | "linux", canInstall: boolean, lang = "en") =>
    draw(<GitRow git="missing" system={system} canInstall={canInstall} online command="xcode-select --install" started={false} checking={false} onInstall={noop} onCheck={noop} />, lang);
  const app = git("linux", true);
  assert.match(app, /Install it with your system&#x27;s package manager\./);
  assert.match(app, /data-testid="git-check"[^>]*>Check again</);
  assert.doesNotMatch(app, /git-install|xcode-select|Apple|macOS/);
  // A browser cannot ask the server to look again (`app_only`), and has no command to copy.
  const browser = git("linux", false);
  assert.doesNotMatch(browser, /git-check|git-install|xcode-select/);
  assert.match(git("linux", true, "zh-CN"), /请用系统的包管理器安装/);
  // A Mac keeps Apple's installer.
  assert.match(git("mac", true), /macOS interrupts the first session[\s\S]*data-testid="git-install"[^>]*>Install \(Apple\)</);
});

test("step 01 on Linux: Codex's PATH line names no zsh file, Claude's goes to ~/.bashrc", () => {
  const linux = (claude: ReturnType<typeof tool>, codex: ReturnType<typeof tool>) => report(claude, codex, { os: linuxOs });
  // The confirm box draws the facts of the command it runs: the whitelist's, here Codex's official installer.
  const withCommand = draw(
    <ToolsStepView
      report={linux(tool("claude", "ready"), tool("codex", "missing"))}
      commands={[{ tool: "codex", kind: "install", method: "native", command: "sh install.sh", available: true, reason: null, recommended: true, docs: "https://github.com/openai/codex" }]}
      quota={null}
      homeDir={HOME}
      online
      ui={{ ...IDLE_TOOLS_UI, codex: { ...IDLE_CARD, open: "confirm", kind: "install" } }}
      status={IDLE_STATUS}
      acts={acts}
      advanced={{ open: false, kind: "token", onOpen: noop, onKind: noop }}
    />,
  );
  assert.match(withCommand, /Adds ~\/\.local\/bin to PATH in your shell&#x27;s startup file so your own terminal finds codex too\./);
  assert.doesNotMatch(withCommand, /zprofile/);
  const installed = tool("claude", "ready", { task: task("claude", "install", { running: false, exitCode: 0 }) });
  const hint = draw(
    <ToolsStepView
      report={linux(installed, tool("codex", "ready"))}
      commands={[]}
      quota={null}
      homeDir={HOME}
      online
      ui={IDLE_TOOLS_UI}
      status={IDLE_STATUS}
      acts={acts}
      advanced={{ open: false, kind: "token", onOpen: noop, onKind: noop }}
    />,
  );
  assert.match(hint, /data-testid="path-hint"[\s\S]*&gt;&gt; ~\/\.bashrc/);
  assert.doesNotMatch(hint, /zshrc/);
});

test("step 03 on Linux: no macOS prompt for a folder in Documents", () => {
  const ui = { ...EMPTY_FIRST_UI, cwd: `${HOME}/Documents/notes` };
  const at = (os: { platform: string; macos: string | null; arch: string }) =>
    draw(<FirstStepView report={report(tool("claude", "ready"), tool("codex", "missing"), { os })} ui={ui} onUi={noop} recent={[]} onPractice={noop} practice={{ state: "idle", path: null }} homeDir={HOME} onGoto={noop} />);
  assert.match(at({ platform: "darwin", macos: "15.3", arch: "arm64" }), /data-testid="first-tcc"/);
  assert.doesNotMatch(at(linuxOs), /data-testid="first-tcc"|macOS/);
});
