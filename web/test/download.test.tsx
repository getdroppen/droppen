// droppen.app/download (`features/landing/download.tsx`): how Droppen gets
// onto a device, the way other vendors' download pages work.
//
// What these pin: which device is reading is detected from what the browser
// says (a table of real user agents, iPadOS-as-a-Mac included) and opens
// its tab, marked; every way is one tab of an ARIA tablist, named in the
// address (`?os=`) — the address wins, then the session's choice, then the
// detected one; the App and Linux degrade to "coming" behind their switches
// and never link a file that is not there, and the App's last step is
// signing it in from inside it; each tab renders its way in, with
// one primary action; the page reads in zh-CN, en, zh-TW and ja; `/download`
// is public signed in or not, and the landing page links it from its footer.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { landingBundles } from "../src/features/landing/copy.js";
import { detectKind, type EnvironmentHints, initialOs, osForKind } from "../src/features/landing/detect.js";
import DownloadPage from "../src/features/landing/download.js";
import {
  APP_AVAILABLE,
  APP_DOWNLOAD_URL,
  APP_SIGN_IN_AVAILABLE,
  type Availability,
  AVAILABILITY,
  CHECKSUM_COMMAND,
  codeInstallCommand,
  DOWNLOAD_OSES,
  downloadHref,
  type DownloadOs,
  INSTALL_COMMAND,
  LINUX_AVAILABLE,
  osOfSearch,
  RELEASES_URL,
  stdinEnrollCommand,
  UNINSTALL_COMMAND,
} from "../src/features/landing/get-droppen.js";
import LandingPage from "../src/features/landing/landing.js";
import { bootRoute, isAuthPath, isFrontPath, isLandingPath, publicPage } from "../src/features/landing/route.js";
import type { SignupMode } from "../src/lib/hub/api.js";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "../src/i18n/cloud-devices.js";
import { ja } from "../src/i18n/ja.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";

const app = { "zh-CN": { ...cloudDevicesZhCN, ...zhCN }, "zh-TW": { ...cloudDevicesZhTW, ...zhTW }, ja: { ...cloudDevicesJa, ...ja } };
const i18n = i18next.createInstance();
void i18n.use(initReactI18next).init({
  lng: "zh-CN",
  resources: {
    en: { translation: {} },
    "zh-CN": { translation: { ...landingBundles["zh-CN"], ...app["zh-CN"] } },
    "zh-TW": { translation: { ...landingBundles["zh-TW"], ...app["zh-TW"] } },
    ja: { translation: { ...landingBundles.ja, ...app.ja } },
  },
  interpolation: { escapeValue: false },
  keySeparator: false,
  nsSeparator: false,
});

// The page reads the address (`?os=`) and the session's choice.
const place = { search: "", origin: "https://droppen.app", hash: "" };
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
const session = new MemoryStorage();
Object.assign(globalThis, { location: place, sessionStorage: session });

const MAC_CHROME: EnvironmentHints = {
  userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36",
  platform: "macOS",
  mobile: false,
  maxTouchPoints: 0,
};

function draw(
  lng: string,
  opts: { signedIn?: boolean; app?: boolean; appSignIn?: boolean; linux?: boolean; availability?: Availability; hints?: EnvironmentHints; os?: DownloadOs | null; signup?: SignupMode } = {},
): string {
  void i18n.changeLanguage(lng);
  place.search = opts.os ? `?os=${opts.os}` : "";
  const availability = opts.availability ?? { app: opts.app ?? false, appSignIn: opts.appSignIn ?? false, linux: opts.linux ?? false };
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <DownloadPage signedIn={opts.signedIn} availability={availability} hints={opts.hints ?? MAC_CHROME} signup={opts.signup} />
    </I18nextProvider>,
  );
}

const text = (html: string) => html.replace(/<[^>]+>/g, " ").replace(/&#x27;/g, "'").replace(/&amp;/g, "&").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/\s+/g, " ");
const primaries = (html: string) => (html.match(/data-variant="primary"/g) ?? []).length;

test("detection, over real user agents: the device's kind, and the tab it opens on", () => {
  const table: { name: string; hints: EnvironmentHints; kind: string; os: DownloadOs | null }[] = [
    {
      name: "Safari on macOS",
      hints: { userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15", maxTouchPoints: 0 },
      kind: "mac",
      os: "macos-app",
    },
    { name: "Chrome on macOS (client hints)", hints: MAC_CHROME, kind: "mac", os: "macos-app" },
    {
      name: "Firefox on macOS",
      hints: { userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 14.6; rv:131.0) Gecko/20100101 Firefox/131.0", maxTouchPoints: 0 },
      kind: "mac",
      os: "macos-app",
    },
    {
      name: "Chrome on Windows",
      hints: { userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36", platform: "Windows", mobile: false },
      kind: "windows",
      os: "windows",
    },
    {
      name: "Edge on Windows",
      hints: { userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36 Edg/129.0.0.0" },
      kind: "windows",
      os: "windows",
    },
    {
      name: "Firefox on Linux",
      hints: { userAgent: "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0", maxTouchPoints: 0 },
      kind: "linux",
      os: "linux",
    },
    {
      name: "Chrome on ChromeOS",
      hints: { userAgent: "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36", platform: "Chrome OS" },
      kind: "chromeos",
      os: "linux",
    },
    {
      name: "Safari on iPhone",
      hints: { userAgent: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1", maxTouchPoints: 5 },
      kind: "iphone",
      os: "mobile",
    },
    {
      name: "Chrome on iPhone",
      hints: { userAgent: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/129.0.6668.69 Mobile/15E148 Safari/604.1", maxTouchPoints: 5 },
      kind: "iphone",
      os: "mobile",
    },
    {
      name: "Chrome on Android (client hints)",
      hints: { userAgent: "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Mobile Safari/537.36", platform: "Android", mobile: true, maxTouchPoints: 5 },
      kind: "android",
      os: "mobile",
    },
    {
      name: "Chrome on an Android tablet (no Mobile token)",
      hints: { userAgent: "Mozilla/5.0 (Linux; Android 14; SM-X710) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36", maxTouchPoints: 10 },
      kind: "android",
      os: "mobile",
    },
    {
      name: "Safari on iPadOS, saying it is a Mac (told apart by touch)",
      hints: { userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15", maxTouchPoints: 5 },
      kind: "ipad",
      os: "mobile",
    },
    // Nothing marked detected; the page opens on the Mac App (`DEFAULT_OS`, test/direct-download.test.tsx).
    { name: "nothing to go on", hints: { userAgent: "" }, kind: "unknown", os: null },
  ];
  for (const row of table) {
    const kind = detectKind(row.hints);
    assert.equal(kind, row.kind, row.name);
    assert.equal(osForKind(kind), row.os, row.name);
  }
});

test("the tab: the address wins, then the session's choice, then the detected one", () => {
  assert.equal(initialOs({ search: "?os=linux", remembered: "windows", detected: "macos-app" }), "linux");
  assert.equal(initialOs({ search: "", remembered: "windows", detected: "macos-app" }), "windows");
  assert.equal(initialOs({ search: "?os=nope", remembered: null, detected: "mobile" }), "mobile");
  for (const os of DOWNLOAD_OSES) {
    assert.equal(osOfSearch(downloadHref(os).replace("/download", "")), os, "the address names it, and reads back");
  }
  assert.equal(downloadHref(), "/download");
  assert.equal(downloadHref("macos-bg"), "/download?os=macos-bg");
  assert.equal(osOfSearch("?os=macos"), null);
});

test("the page opens on the tab the address names, else the session's, else the detected one — marked 「推荐」", () => {
  session.values.clear();
  const detected = draw("zh-CN");
  assert.match(detected, /data-testid="environments" data-os="macos-app" data-detected="macos-app"/);
  assert.match(detected, /role="tablist"[^>]*aria-label="选择你的系统"/);
  assert.equal((detected.match(/data-testid="detected"/g) ?? []).length, 1);
  assert.match(detected, /data-testid="os-macos-app"[^]*?推荐/);
  // The address wins over the detected device.
  assert.match(draw("zh-CN", { os: "linux" }), /data-os="linux"/);
  // A choice made in this session, when the address names none.
  session.setItem("rh.download.os", "mobile");
  assert.match(draw("zh-CN"), /data-os="mobile"/);
  assert.match(draw("zh-CN", { os: "windows" }), /data-os="windows"/, "the address still wins");
  session.values.clear();
  // A phone opens on the phone's tab.
  const phone = draw("zh-CN", { hints: { userAgent: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1" } });
  assert.match(phone, /data-os="mobile" data-detected="mobile"/);
});

test("every way is one tab of a keyboard-operable tablist, and each renders its way in with one primary action", () => {
  session.values.clear();
  for (const os of DOWNLOAD_OSES) {
    const html = draw("en", { os });
    assert.equal((html.match(/role="tab"/g) ?? []).length, DOWNLOAD_OSES.length, os);
    assert.match(html, new RegExp(`data-testid="os-${os}"[^>]*aria-selected="true"|aria-selected="true"[^>]*data-testid="os-${os}"`), `${os} selected`);
    assert.match(html, new RegExp(`data-testid="panel-${os}"`), `${os} panel`);
    assert.equal(primaries(html), 1, `${os}: one primary action`);
  }
  const words = (os: DownloadOs, extra: { app?: boolean; linux?: boolean } = {}) => text(draw("en", { os, ...extra }));
  assert.ok(words("macos-bg").includes(INSTALL_COMMAND));
  assert.ok(words("macos-bg").includes("It installs and starts at login"));
  assert.ok(words("windows").includes("Not supported yet"));
  assert.ok(words("windows").includes("use droppen.app in this browser"), "no dead end");
  const mobile = draw("en", { os: "mobile" });
  assert.ok(text(mobile).includes("On your phone, Droppen is the remote control"));
  assert.ok(text(mobile).includes("Add to Home Screen") && text(mobile).includes("Install app"));
  assert.match(mobile, /data-testid="page-address"[^>]*>.*https:\/\/droppen\.app\/download/, "the address to open on the device the AI runs on");
});

test("the switches degrade: no App yet is 「App 即将推出」 and the command; no Linux yet is 「即将支持」 — never a link to a file that is not there", () => {
  assert.equal(APP_AVAILABLE, true, "the signed and notarised App ships from v0.2.123");
  assert.equal(LINUX_AVAILABLE, true, "Linux tarballs and DROPPEN_CODE ship from v0.2.121");
  const noApp = draw("zh-CN", { os: "macos-app" });
  assert.match(noApp, /data-testid="app-soon"/);
  assert.ok(text(noApp).includes("App 即将推出"));
  assert.ok(text(noApp).includes(INSTALL_COMMAND), "the way in today");
  assert.doesNotMatch(noApp, /data-testid="app-download"/);
  const withApp = draw("zh-CN", { os: "macos-app", app: true });
  assert.match(withApp, new RegExp(`<a href="${APP_DOWNLOAD_URL.replace(/[.?]/g, "\\$&")}"[^>]*data-variant="primary" data-testid="app-download"`));
  assert.ok(text(withApp).includes("下载 Droppen App（macOS）"));
  assert.doesNotMatch(withApp, /data-testid="app-soon"/);
  const noLinux = draw("zh-CN", { os: "linux" });
  assert.match(noLinux, /data-testid="linux-soon"/);
  assert.ok(text(noLinux).includes("即将支持"));
  assert.ok(!text(noLinux).includes("DROPPEN_CODE"), "no command that does not work yet");
  const withLinux = text(draw("en", { os: "linux", linux: true }));
  assert.ok(withLinux.includes(codeInstallCommand("<code>")));
  assert.ok(withLinux.includes("my devices → add a device that runs AI → show a one-time code"));
  // Windows says what works either way.
  assert.ok(text(draw("en", { os: "windows" })).includes("Once Linux is supported"));
  // WSL only with systemd on: the installer runs Droppen as a systemd user service, and stops where there is none.
  const windows = draw("en", { os: "windows", linux: true });
  assert.ok(text(windows).includes("Run the AI on a Linux server, or in WSL with systemd turned on."));
  assert.match(windows, /<a href="https:\/\/learn\.microsoft\.com\/windows\/wsl\/systemd" target="_blank" rel="noopener noreferrer" data-testid="wsl-systemd"[^>]*>how to turn on systemd in WSL<\/a>/);
  assert.ok(text(draw("zh-CN", { os: "windows", linux: true })).includes("或者开启了 systemd 的 WSL 里。"));
});

/** The App tab's panel alone (the page around it says 「登录 Droppen」 for the other ways in). */
const appPanel = (html: string) => {
  const from = html.indexOf('data-testid="panel-macos-app"');
  assert.ok(from >= 0, "the App tab is open");
  return html.slice(from, html.indexOf('id="requirements"', from));
};
const appSteps = (html: string) => (appPanel(html).match(/data-testid="app-steps"[^]*?<\/ol>/)?.[0].match(/<li/g) ?? []).length;

test("signing in from the App: its last step is 「登录 Droppen」 inside it, and nothing says it is coming — in every language", () => {
  assert.equal(APP_SIGN_IN_AVAILABLE, true, "every App published signs itself in (the hand-off came in 0.2.125; the first public .dmg is 0.2.127)");
  assert.equal(AVAILABILITY.appSignIn, APP_SIGN_IN_AVAILABLE, "the page reads the switch");
  const said = {
    en: {
      step: "Click Sign in to Droppen in the app",
      lead: "Then sign it in to Droppen from inside the app.",
      end: "Install it, then sign it in to Droppen.",
      note: "signing in to Droppen from the app is coming in an update",
    },
    "zh-CN": { step: "在 App 里点「登录 Droppen」", lead: "然后在 App 里登录 Droppen 就行", end: "装好，再登录 Droppen。", note: "在 App 里登录 Droppen 的功能会在后续更新中推出" },
    "zh-TW": { step: "在 App 裡點「登入 Droppen」", lead: "接著在 App 裡登入 Droppen 就好", end: "裝好，再登入 Droppen。", note: "在 App 裡登入 Droppen 的功能會在後續更新中推出" },
    ja: { step: "アプリで「Droppen にサインイン」を押す", lead: "あとはアプリから Droppen にサインインするだけ", end: "インストールして、Droppen にサインイン。", note: "アプリからの Droppen へのサインインは、今後のアップデートで対応します" },
  };
  for (const [lng, { step, lead, end, note }] of Object.entries(said)) {
    // The build's own switches, and the same spelled out.
    for (const html of [draw(lng, { os: "macos-app", availability: AVAILABILITY }), draw(lng, { os: "macos-app", app: true, appSignIn: true })]) {
      const panel = text(appPanel(html));
      assert.match(appPanel(html), /data-testid="app-download"/, `${lng}: the App is offered`);
      assert.ok(panel.includes(step), `${lng}: ${step}`);
      assert.ok(panel.includes(lead), `${lng}: ${lead}`);
      assert.equal(appSteps(html), 4, `${lng}: open the .dmg, drag it to Applications, open it, sign it in`);
      assert.doesNotMatch(html, /data-testid="app-sign-in-soon"/, `${lng}: no "coming" note`);
      // The page says one thing: install it, then sign it in — and nowhere that signing in is still to come.
      assert.ok(text(html).includes(end), `${lng}: ${end}`);
      assert.ok(!text(html).includes(note), `${lng} still says: ${note}`);
      assert.equal(primaries(html), 1, `${lng}: one primary action, the download`);
    }
  }
  // Off (an explicit switch, as a preview would pass): no step for a button the App does not have, and a note that it is coming.
  const off = draw("en", { os: "macos-app", app: true, appSignIn: false });
  assert.match(appPanel(off), /data-testid="app-sign-in-soon"/);
  assert.ok(text(appPanel(off)).includes("Using it from your phone: signing in to Droppen from the app is coming in an update."));
  assert.ok(!text(appPanel(off)).includes("Click Sign in to Droppen in the app"));
  assert.equal(appSteps(off), 3);
  assert.ok(text(appPanel(draw("zh-CN", { os: "macos-app", app: true, appSignIn: false }))).includes("想在手机上用？在 App 里登录 Droppen 的功能会在后续更新中推出。"));
});

test("what it runs on, the manual download with its checksums, and the uninstall", () => {
  const html = draw("en");
  const words = text(html);
  for (const line of ["Mac", "Apple silicon or Intel", "Linux server", "Windows", "not yet, on its way", "Claude Code or Codex"]) assert.ok(words.includes(line), line);
  assert.match(html, new RegExp(`<a href="${RELEASES_URL}" target="_blank" rel="noopener noreferrer"`));
  assert.ok(words.includes("SHA-256"));
  assert.ok(words.includes(CHECKSUM_COMMAND), "how to check the file");
  assert.ok(words.includes(UNINSTALL_COMMAND));
  assert.ok(words.includes("Settings → Access → Droppen → Disconnect this device"));
  assert.ok(text(draw("en", { app: true })).includes("quit Droppen and drag it from Applications to the Trash"));
  for (const id of ["get", "requirements", "manual", "uninstall"]) assert.match(html, new RegExp(`id="${id}"`));
});

test("signed out it invites to sign up; signed in it goes back to 「我的设备」 — never asks to sign in again", () => {
  const out = draw("en");
  assert.match(out, /<a href="\/signup"[^>]*>Get started<\/a>/);
  // Open sign-up (the default): no invite code; by invite, the code first.
  assert.ok(text(out).includes("No account yet? Sign up first, or in the"));
  assert.doesNotMatch(text(out), /invite/i);
  assert.ok(text(draw("zh-CN")).includes("还没有账号？先注册，或者在设备登录时弹出的"));
  assert.ok(text(draw("en", { signup: "invite" })).includes("No account yet? Sign up with your invite code first, or in the"));
  // Closed: closed for now, and the waitlist where "Get started" was; sign-in stays.
  const closed = draw("en", { signup: "closed" });
  assert.ok(text(closed).includes("No account yet? Sign-up is closed for now: join the waitlist."));
  assert.match(closed, /<a href="\/pricing#waitlist"[^>]*data-testid="download-waitlist"[^>]*>Join the waitlist<\/a>/);
  assert.doesNotMatch(closed, /href="\/signup"|Get started/);
  assert.match(closed, /<a href="\/signin"[^>]*>Sign in<\/a>/);
  assert.doesNotMatch(text(closed), /invite/i);
  assert.ok(text(draw("ja", { signup: "closed" })).includes("現在は新規登録を停止しています。"));
  const signedIn = draw("en", { signedIn: true, os: "windows" });
  assert.doesNotMatch(signedIn, /href="\/sign(in|up)"/);
  assert.match(signedIn, /<a href="\/devices"[^>]*data-testid="download-open-devices"/);
  assert.match(signedIn, /<a href="\/sessions"[^>]*data-testid="use-in-browser"/);
});

test("it leads with what you get, and reads in zh-CN, zh-TW and ja", () => {
  const cn = text(draw("zh-CN"));
  assert.ok(cn.includes("装一次，你的 AI 就到了手机上。"));
  assert.ok(cn.indexOf("装一次") < cn.indexOf(INSTALL_COMMAND), "the benefit before the command");
  const tw = text(draw("zh-TW", { os: "linux" }));
  assert.ok(tw.includes("裝一次，你的 AI 就到了手機上。"));
  assert.ok(tw.includes("即將支援"));
  const jp = text(draw("ja", { os: "mobile" }));
  assert.ok(jp.includes("一度入れれば、あなたの AI がスマホに。"));
  assert.ok(jp.includes("スマホでは、Droppen はリモコンです"));
  for (const words of [cn, tw, jp]) assert.doesNotMatch(words, /Install it once|Choose your system|Leaving takes three steps/);
});

test("`/download` is a public page of its own, signed in or not, and the landing page links it from its footer", () => {
  assert.equal(publicPage("/download"), "download");
  assert.equal(publicPage("/download/"), "download");
  assert.deepEqual(bootRoute("/download", false), { show: "download" });
  assert.deepEqual(bootRoute("/download", true), { show: "download" });
  assert.ok(isFrontPath("/download") && isFrontPath("/") && !isFrontPath("/sessions") && !isFrontPath("/signin"));
  assert.ok(!isLandingPath("/download") && !isAuthPath("/download"));
  for (const lng of ["zh-CN", "en"]) {
    void i18n.changeLanguage(lng);
    const html = renderToStaticMarkup(
      <I18nextProvider i18n={i18n}>
        <LandingPage />
      </I18nextProvider>,
    );
    assert.match(html, /<a href="\/download"[^>]*data-testid="footer-download"/, lng);
  }
  void i18n.changeLanguage("zh-CN");
  assert.match(renderToStaticMarkup(<I18nextProvider i18n={i18n}><LandingPage /></I18nextProvider>), /data-testid="footer-download"[^>]*>下载 Droppen<\/a>/);
});

test("a one-time code is never on a command line: the installer takes it from its environment, a running server from stdin", () => {
  const code = "RH1-ABCD-EFGH";
  assert.equal(codeInstallCommand(code), `curl -fsSL https://droppen.app/install.sh | DROPPEN_CODE=${code} sh`);
  assert.equal(codeInstallCommand(code, "https://droppen.app"), codeInstallCommand(code), "droppen.app needs no hub");
  assert.equal(
    codeInstallCommand(code, "https://hub.example"),
    `curl -fsSL https://droppen.app/install.sh | DROPPEN_HUB=https://hub.example DROPPEN_CODE=${code} sh`,
  );
  const stdin = stdinEnrollCommand(code, "https://droppen.app");
  assert.equal(stdin, `printf '%s\\n' '${code}' | ~/.local/share/remote-hand/bin/remote-hand-server agent enroll - --hub https://droppen.app`);
  assert.ok(!/--code\s+RH1/.test(codeInstallCommand(code)) && !/enroll RH1/.test(stdin));
  // The add-device dialog builds its commands from these two, and no other.
  const dialog = readFileSync(new URL("../src/features/cloud/add-computer.tsx", import.meta.url), "utf8");
  assert.doesNotMatch(dialog, /agent enroll \$\{/, "no code in an `agent enroll` argument");
  assert.match(dialog, /codeInstallCommand\(code, hubUrl\)/);
});
