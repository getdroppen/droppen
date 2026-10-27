// droppen.app/download (`features/landing/download.tsx`): how Droppen gets
// onto a device, the way other vendors' download pages work.
//
// What these pin: the page in a customer's order (where it goes and what to
// have, the install, the phone, what gets installed, updating and
// uninstalling); which device is reading is detected from what the browser
// says (a table of real user agents, iPadOS-as-a-Mac included) and opens its
// computer's tab, marked (a phone marks none); the computers are one ARIA
// tablist, the Mac's two ways a switch inside its tab, named in the address
// (`?os=`) — the address wins, then the session's choice, then the detected
// one; the App and Linux degrade to "coming" behind their switches and never
// link a file that is not there, and the App's last step is signing it in
// from inside it; each way renders with one primary action; the page reads
// in zh-CN, en, zh-TW and ja; `/download` is public signed in or not, and the
// landing page links it from its footer.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { I18nextProvider } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { landingBundles } from "../src/features/landing/copy.js";
import { downloadJa, downloadZhCN, downloadZhTW } from "../src/features/landing/download-copy.js";
import { onboardingJa, onboardingZhCN, onboardingZhTW } from "../src/i18n/onboarding.js";
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
import { MemoryStorage } from "./storage.js";
import { makeI18n } from "./render.js";

const app = { "zh-CN": { ...cloudDevicesZhCN, ...zhCN }, "zh-TW": { ...cloudDevicesZhTW, ...zhTW }, ja: { ...cloudDevicesJa, ...ja } };
const i18n = makeI18n({
  lng: "zh-CN",
  resources: {
    en: { translation: {} },
    "zh-CN": { translation: { ...landingBundles["zh-CN"], ...app["zh-CN"] } },
    "zh-TW": { translation: { ...landingBundles["zh-TW"], ...app["zh-TW"] } },
    ja: { translation: { ...landingBundles.ja, ...app.ja } }
  },
  keySeparator: false,
  nsSeparator: false
});

// The page reads the address (`?os=`) and the session's choice.
const place = { search: "", origin: "https://droppen.app", hash: "" };

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

test("the page opens on the tab the address names, else the session's, else the detected one — marked 「当前设备」", () => {
  session.values.clear();
  const detected = draw("zh-CN");
  assert.match(detected, /data-testid="environments" data-os="macos-app" data-detected="macos-app"/);
  assert.match(detected, /role="tablist"[^>]*aria-label="选择你的系统"/);
  assert.equal((detected.match(/data-testid="detected"/g) ?? []).length, 1);
  assert.match(detected, /data-testid="os-macos-app"[^]*?当前设备/);
  // The address wins over the detected device.
  assert.match(draw("zh-CN", { os: "linux" }), /data-os="linux"/);
  // A choice made in this session, when the address names none.
  session.setItem("rh.download.os", "mobile");
  assert.match(draw("zh-CN"), /data-os="mobile"/);
  assert.match(draw("zh-CN", { os: "windows" }), /data-os="windows"/, "the address still wins");
  session.values.clear();
  // A phone: on the phone's way, which is no tab — the Mac's is open, nothing marked, and the page says where to open it.
  const phone = draw("zh-CN", { hints: { userAgent: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1" } });
  assert.match(phone, /data-os="mobile" data-detected="mobile"/);
  assert.match(phone, /data-testid="os-macos-app"[^>]*aria-selected="true"|aria-selected="true"[^>]*data-testid="os-macos-app"/);
  assert.doesNotMatch(phone, /data-testid="detected"/);
  assert.match(phone, /data-testid="page-address"[^>]*>.*https:\/\/droppen\.app\/download/, "the address to open on the computer");
});

test("the computers are one keyboard-operable tablist, the Mac's two ways a switch in its tab, and each way renders with one primary action", () => {
  session.values.clear();
  const tabOf: Record<DownloadOs, string> = { "macos-app": "macos-app", "macos-bg": "macos-app", linux: "linux", windows: "windows", mobile: "macos-app" };
  for (const os of DOWNLOAD_OSES) {
    const html = draw("en", { os, app: true });
    assert.deepEqual([...html.matchAll(/role="tab"[^>]*data-testid="os-([a-z-]+)"|data-testid="os-([a-z-]+)"[^>]*role="tab"/g)].map((m) => m[1] ?? m[2]), ["macos-app", "linux", "windows"], `${os}: Mac, Linux, Windows`);
    const tab = tabOf[os];
    assert.match(html, new RegExp(`data-testid="os-${tab}"[^>]*aria-selected="true"|aria-selected="true"[^>]*data-testid="os-${tab}"`), `${os}: the ${tab} tab`);
    assert.match(html, new RegExp(`data-testid="panel-${tab}"`), `${os} panel`);
    assert.equal(primaries(html), 1, `${os}: one primary action`);
  }
  // The Mac's ways: the App or the command line, a switch in the Mac's tab.
  const bg = draw("en", { os: "macos-bg", app: true });
  assert.match(bg, /data-testid="way-macos-bg"[^>]*aria-pressed="true"|aria-pressed="true"[^>]*data-testid="way-macos-bg"/);
  assert.match(bg, /data-testid="way-panel-macos-bg"/);
  assert.match(draw("en", { os: "macos-app", app: true }), /data-testid="way-panel-macos-app"/);
  const words = (os: DownloadOs, extra: { app?: boolean; linux?: boolean } = {}) => text(draw("en", { os, ...extra }));
  assert.ok(words("macos-bg").includes(INSTALL_COMMAND));
  assert.ok(words("macos-bg").includes("It installs and starts at login"));
  assert.ok(words("windows").includes("Not supported yet"));
  assert.ok(words("windows").includes("use droppen.app in this browser"), "no dead end");
  // The phone is a part of the page, after the install, whatever the tab: nothing to install there.
  for (const os of DOWNLOAD_OSES) {
    const html = draw("en", { os });
    assert.ok(html.indexOf('id="get"') < html.indexOf('id="phone"'), `${os}: the install, then the phone`);
    assert.ok(text(html).includes("Then, use it from your phone") && text(html).includes("Add to Home Screen") && text(html).includes("Install app"), os);
  }
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
  return html.slice(from, html.indexOf('id="phone"', from));
};
const appSteps = (html: string) => (appPanel(html).match(/data-testid="app-steps"[^]*?<\/ol>/)?.[0].match(/<li/g) ?? []).length;

test("signing in from the App: its last step is 「登录 Droppen」 inside it, and nothing says it is coming — in every language", () => {
  assert.equal(APP_SIGN_IN_AVAILABLE, true, "every App published signs itself in (the hand-off came in 0.2.125; the first public .dmg is 0.2.127)");
  assert.equal(AVAILABILITY.appSignIn, APP_SIGN_IN_AVAILABLE, "the page reads the switch");
  // Where the sign-in is in the App: its setup guide's step 02, by the App's own names (i18n/welcome.ts), then Settings — as the first-run card says it.
  const said = {
    en: {
      step: "Click Sign in to Droppen in the app",
      opens: "It opens a short setup guide, four steps.",
      where: "In the guide's step 02 (Where to use it), choose “On my phone and other devices too”, then Sign in to Droppen (later: Settings → Access → Droppen). A droppen.app window opens: sign in, then click add this device.",
      lead: "Then sign it in to Droppen from inside the app.",
      note: "signing in to Droppen from the app is coming in an update",
      gone: "Nothing to type.",
    },
    "zh-CN": {
      step: "在 App 里点「登录 Droppen」",
      opens: "它会打开一个四步的设置向导。",
      where: "在向导第 02 步「在哪里用」选「在手机和其它设备上也能用」，再点「登录 Droppen」（之后也可以在「设置 → 访问 → Droppen」里找到）。会弹出 droppen.app 窗口：登录后点「加入这台设备」。",
      lead: "然后在 App 里登录 Droppen 就行",
      note: "在 App 里登录 Droppen 的功能会在后续更新中推出",
      gone: "不用输入任何东西",
    },
    "zh-TW": {
      step: "在 App 裡點「登入 Droppen」",
      opens: "它會打開一個四步的設定引導。",
      where: "在引導第 02 步「在哪裡用」選「在手機和其他裝置上也能用」，再點「登入 Droppen」（之後也可以在「設定 → 存取 → Droppen」裡找到）。",
      lead: "接著在 App 裡登入 Droppen 就好",
      note: "在 App 裡登入 Droppen 的功能會在後續更新中推出",
      gone: "不用輸入任何東西",
    },
    ja: {
      step: "アプリで「Droppen にサインイン」を押す",
      opens: "4 ステップの短いセットアップガイドが開きます。",
      where: "ガイドのステップ 02「使う場所」で「スマートフォンやほかのデバイスでも使う」を選び、「Droppen にサインイン」を押します（あとからは「設定 → アクセス → Droppen」）。",
      lead: "あとはアプリから Droppen にサインインするだけ",
      note: "アプリからの Droppen へのサインインは、今後のアップデートで対応します",
      gone: "入力するものはありません",
    },
  };
  for (const [lng, { step, opens, where, lead, note, gone }] of Object.entries(said)) {
    // The build's own switches, and the same spelled out.
    for (const html of [draw(lng, { os: "macos-app", availability: AVAILABILITY }), draw(lng, { os: "macos-app", app: true, appSignIn: true })]) {
      const panel = text(appPanel(html));
      assert.match(appPanel(html), /data-testid="app-download"/, `${lng}: the App is offered`);
      assert.ok(panel.includes(step), `${lng}: ${step}`);
      assert.ok(panel.includes(opens), `${lng}: ${opens}`);
      assert.ok(panel.includes(where), `${lng}: ${where}`);
      assert.ok(panel.indexOf(opens) < panel.indexOf(where), `${lng}: open it, then sign it in`);
      assert.ok(!panel.includes(gone), `${lng} still says: ${gone}`);
      assert.ok(panel.includes(lead), `${lng}: ${lead}`);
      assert.equal(appSteps(html), 4, `${lng}: open the .dmg, drag it to Applications, open it, sign it in`);
      assert.doesNotMatch(html, /data-testid="app-sign-in-soon"/, `${lng}: no "coming" note`);
      // Nowhere does the page say that signing in is still to come.
      assert.ok(!text(html).includes(note), `${lng} still says: ${note}`);
      assert.equal(primaries(html), 1, `${lng}: one primary action, the download`);
    }
  }
  // The download page and 「我的设备」's first-run card say it in the same words, in every language.
  for (const key of [
    "It opens a short setup guide, four steps.",
    "In the guide's step 02 (Where to use it), choose “On my phone and other devices too”, then Sign in to Droppen (later: Settings → Access → Droppen). A {{app}} window opens: sign in, then click add this device.",
  ]) {
    for (const [lng, page, card] of [["zh-CN", downloadZhCN, onboardingZhCN], ["zh-TW", downloadZhTW, onboardingZhTW], ["ja", downloadJa, onboardingJa]] as const) {
      assert.ok(page[key], `${lng}: the page has ${key}`);
      assert.equal(page[key], card[key], `${lng}: as the card says it`);
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

test("in a customer's order: what to have, the install, the phone, what gets installed and how to check it, updating and uninstalling", () => {
  const html = draw("en", { app: true });
  const words = text(html);
  // What to have, before the install.
  const needs = text(html.slice(html.indexOf('data-testid="download-needs"'), html.indexOf('id="get"')));
  for (const line of ["Claude Code or Codex, installed and signed in on that device", "macOS 13 or later (Apple silicon or Intel), or Linux (x86_64 or aarch64)", "A free Droppen account"]) assert.ok(needs.includes(line), line);
  // The order of the parts.
  const at = (marker: string) => html.indexOf(marker);
  const order = ['data-testid="download-needs"', 'id="get"', 'id="phone"', 'id="verify"', 'id="uninstall"'];
  assert.deepEqual([...order].sort((a, b) => at(a) - at(b)), order);
  for (const marker of order) assert.ok(at(marker) > 0, marker);
  // What gets installed, as the install script and the release say it, and how to check the files.
  for (const line of ["Signed with a Developer ID and notarized by Apple", "never as root", "SHA-256", "a systemd user service on Linux", "127.0.0.1 (port 8700 by default)", "no inbound port to open"]) assert.ok(words.includes(line), line);
  const script = readFileSync(new URL("../../scripts/install.sh", import.meta.url), "utf8");
  assert.match(script, /It does not run as root/);
  assert.match(script, /127\.0\.0\.1:8700/);
  assert.match(html, new RegExp(`<a href="${RELEASES_URL}" target="_blank" rel="noopener noreferrer"`));
  assert.ok(words.includes(CHECKSUM_COMMAND), "how to check the file");
  assert.match(html, /<a href="\/security"[^>]*data-testid="download-security"/);
  assert.ok(!text(draw("en", { app: false })).includes("notarized"), "no App, nothing said about it");
  // Updating and uninstalling.
  assert.ok(words.includes("The Mac app checks for updates by itself."));
  assert.ok(words.includes(UNINSTALL_COMMAND));
  assert.ok(words.includes("Settings → Access → Droppen → Disconnect this device"));
  assert.ok(words.includes("quit Droppen and drag it from Applications to the Trash"));
  // Said once: no table of systems repeating the tabs, no closing call to install it again.
  assert.doesNotMatch(html, /id="requirements"|id="manual"|Ready when you are|Install it, then sign it in/);
});

test("signed out the account it needs is a link to sign up; signed in it goes back to 「我的设备」 — never asks to sign in again", () => {
  const out = draw("en");
  assert.match(out, /<a href="\/signup"[^>]*data-testid="download-signup"[^>]*>Create a free account<\/a>/);
  // Open sign-up (the default): no invite code; by invite, the code first.
  assert.ok(text(out).includes("A free Droppen account. You can also create it when you sign the device in."));
  assert.doesNotMatch(text(out), /invite/i);
  assert.ok(text(draw("zh-CN")).includes("一个免费的 Droppen 账户，也可以在设备登录时创建。"));
  assert.ok(text(draw("en", { signup: "invite" })).includes("A Droppen account. Sign-up needs an invite code."));
  // Closed: closed for now, and the waitlist; sign-in stays in the header.
  const closed = draw("en", { signup: "closed" });
  assert.ok(text(closed).includes("A Droppen account. Sign-up is closed for now."));
  assert.match(closed, /<a href="\/pricing#waitlist"[^>]*data-testid="download-waitlist"[^>]*>Join the waitlist<\/a>/);
  assert.doesNotMatch(closed, /href="\/signup"|Create a free account/);
  assert.match(closed, /<a href="\/signin"[^>]*>Sign in<\/a>/);
  assert.doesNotMatch(text(closed), /invite/i);
  assert.ok(text(draw("ja", { signup: "closed" })).includes("現在、新規登録を停止しています。"));
  const signedIn = draw("en", { signedIn: true, os: "windows" });
  assert.doesNotMatch(signedIn, /href="\/sign(in|up)"/);
  assert.match(signedIn, /<a href="\/devices"[^>]*data-testid="download-open-devices"/);
  assert.match(signedIn, /<a href="\/sessions"[^>]*data-testid="use-in-browser"/);
});

test("it leads with where Droppen goes, and reads in zh-CN, zh-TW and ja", () => {
  const cn = text(draw("zh-CN", { os: "macos-bg" }));
  assert.ok(cn.includes("在运行 AI 的设备上安装 Droppen。"));
  assert.ok(cn.includes("手机和其他设备无需安装"));
  assert.ok(cn.indexOf("在运行 AI 的设备上安装") < cn.indexOf(INSTALL_COMMAND), "where it goes before the command");
  const tw = text(draw("zh-TW", { os: "linux" }));
  assert.ok(tw.includes("在執行 AI 的裝置上安裝 Droppen。"));
  assert.ok(tw.includes("即將支援"));
  const jp = text(draw("ja", { os: "mobile" }));
  assert.ok(jp.includes("AI が動くデバイスに Droppen をインストール。"));
  assert.ok(jp.includes("インストールしたら、スマホで使う"));
  for (const words of [cn, tw, jp]) assert.doesNotMatch(words, /Install Droppen on the device|Choose your system|Updating and uninstalling|Then, use it from your phone/);
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
