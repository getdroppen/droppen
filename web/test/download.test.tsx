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
  opts: { signedIn?: boolean; app?: boolean; appSignIn?: boolean; linux?: boolean; availability?: Availability; hints?: EnvironmentHints; os?: DownloadOs | null } = {},
): string {
  void i18n.changeLanguage(lng);
  place.search = opts.os ? `?os=${opts.os}` : "";
  const availability = opts.availability ?? { app: opts.app ?? false, appSignIn: opts.appSignIn ?? false, linux: opts.linux ?? false };
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <DownloadPage signedIn={opts.signedIn} availability={availability} hints={opts.hints ?? MAC_CHROME} />
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
  // The Mac's two ways, flat in the tab: no switch, nothing hidden behind one.
  const mac = draw("en", { os: "macos-app", app: true });
  assert.match(mac, /data-testid="way-panel-macos-app"/);
  assert.match(mac, /data-testid="way-panel-macos-bg"/, "the command line is shown beside the App");
  assert.doesNotMatch(mac, /data-testid="way-macos-app"|data-testid="way-macos-bg"/, "no switch");
  assert.equal(primaries(mac), 1, "one primary action: the App's download");
  const words = (os: DownloadOs, extra: { app?: boolean; linux?: boolean } = {}) => text(draw("en", { os, ...extra }));
  assert.ok(words("macos-bg").includes(INSTALL_COMMAND));
  assert.ok(words("macos-bg").includes("It installs and starts at login"));
  assert.ok(words("windows").includes("On Windows, Droppen runs in the browser:"));
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
  assert.ok(text(draw("zh-CN", { os: "windows", linux: true })).includes("在 Linux 服务器或已开启 systemd 的 WSL 中运行 AI。"));
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
  // Where the sign-in is in the App: its setup guide's Where to use it step, by the App's own names (i18n/welcome.ts), through the browser, then Settings — as the first-run card says it.
  const said = {
    en: {
      step: "Sign in to Droppen from the app",
      opens: "It opens a short setup guide, four steps.",
      where: "In the setup guide, choose “On my phone and other devices too”, then sign in: your browser opens droppen.app, where you check the fingerprint and approve this Mac.",
      lead: "To use it from your phone and other devices, sign in to Droppen from the app.",
      note: "signing in to Droppen from the app is coming in an update",
      gone: "Nothing to type.",
    },
    "zh-CN": {
      step: "在 App 内登录 Droppen",
      opens: "它会打开一个四步的设置向导。",
      where: "在设置向导里选择「在手机和其他设备上也能用」，然后登录：浏览器会打开 droppen.app，核对指纹后批准这台 Mac。",
      lead: "要在手机和其他设备上使用，请在 App 内登录 Droppen",
      note: "在 App 内登录 Droppen 的功能将在后续更新中提供",
      gone: "不用输入任何东西",
    },
    "zh-TW": {
      step: "在 App 內登入 Droppen",
      opens: "它會打開一個四步的設定引導。",
      where: "在設定精靈裡選擇「在手機和其他裝置上也能用」，然後登入：瀏覽器會開啟 droppen.app，核對指紋後批准這台 Mac。",
      lead: "要在手機和其他裝置上使用，請在 App 內登入 Droppen",
      note: "在 App 內登入 Droppen 的功能將在後續更新中提供",
      gone: "不用輸入任何東西",
    },
    ja: {
      step: "アプリから Droppen にサインイン",
      opens: "4 ステップの短いセットアップガイドが開きます。",
      where: "セットアップガイドで「スマホやほかのデバイスからも使う」を選び、サインインします。ブラウザが droppen.app を開くので、指紋を照合してこの Mac を承認します。",
      lead: "スマホやほかのデバイスから使うには、アプリから Droppen にサインインします",
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
  // The step's opening line is the card's, in every language; the page now summarizes the sign-in walkthrough the card spells out.
  for (const key of ["It opens a short setup guide, four steps."]) {
    for (const [lng, page, card] of [["zh-CN", downloadZhCN, onboardingZhCN], ["zh-TW", downloadZhTW, onboardingZhTW], ["ja", downloadJa, onboardingJa]] as const) {
      assert.ok(page[key], `${lng}: the page has ${key}`);
      assert.equal(page[key], card[key], `${lng}: as the card says it`);
    }
  }
  // Off (an explicit switch, as a preview would pass): no step for a button the App does not have, and a note that it is coming.
  const off = draw("en", { os: "macos-app", app: true, appSignIn: false });
  assert.match(appPanel(off), /data-testid="app-sign-in-soon"/);
  assert.ok(text(appPanel(off)).includes("Using it from your phone: signing in to Droppen from the app is coming in an update."));
  assert.ok(!text(appPanel(off)).includes("Sign in to Droppen from the app"));
  assert.equal(appSteps(off), 3);
  assert.ok(text(appPanel(draw("zh-CN", { os: "macos-app", app: true, appSignIn: false }))).includes("在手机上使用：在 App 内登录 Droppen 的功能将在后续更新中提供。"));
});

test("in a customer's order: what to have, the install, the phone, what gets installed and how to check it, updating and uninstalling", () => {
  const html = draw("en", { app: true });
  const words = text(html);
  // What to have: the panel's own system line, and no account line of its own under the tabs.
  assert.doesNotMatch(html, /data-testid="download-needs"/, "no needs band under the tabs");
  for (const line of ["macOS 13 or later: Apple silicon or Intel"]) assert.ok(words.includes(line), line);
  // The order of the parts: the platform's install, update, uninstall and reference, then the phone.
  const at = (marker: string) => html.indexOf(marker);
  const order = ['id="get"', 'id="update"', 'id="uninstall"', 'id="verify"', 'id="phone"'];
  assert.deepEqual([...order].sort((a, b) => at(a) - at(b)), order);
  for (const marker of order) assert.ok(at(marker) > 0, marker);
  // What gets installed, as the install script and the release say it, and how to check the files.
  for (const line of ["Signed with a Developer ID and notarized by Apple", "never as root", "SHA-256", "a systemd user service on Linux", "--system makes it a system service that starts at boot and still runs as you", "Listens only on 127.0.0.1", "no inbound port to open", "port 8700, or the next free port", "SHA256SUMS", "neither Droppen Hub nor the Droppen relays hold a key"]) assert.ok(words.includes(line), line);
  const script = readFileSync(new URL("../../scripts/install.sh", import.meta.url), "utf8");
  assert.match(script, /It does not run as root/);
  assert.match(script, /127\.0\.0\.1:8700 or the next free port/);
  assert.match(script, /--system is for Linux/);
  assert.match(script, /--uninstall {3}stop and remove the service and the server; keep the data/);
  assert.match(html, new RegExp(`<a href="${RELEASES_URL}" target="_blank" rel="noopener noreferrer"`));
  assert.ok(words.includes(CHECKSUM_COMMAND), "how to check the file");
  assert.match(html, /<a href="\/security"[^>]*data-testid="download-security"/);
  assert.ok(!text(draw("en", { app: false })).includes("notarized"), "no App, nothing said about it");
  // Updating and uninstalling.
  assert.ok(words.includes("The Mac app checks for updates by itself."));
  assert.ok(words.includes(UNINSTALL_COMMAND));
  assert.ok(words.includes("It stops and removes the service and the program and keeps your data"));
  assert.ok(words.includes("Settings → Access → Droppen → Disconnect this device"));
  assert.ok(words.includes("quit Droppen and drag it from Applications to the Trash"));
  // Said once: no table of systems repeating the tabs, no closing call to install it again.
  assert.doesNotMatch(html, /id="requirements"|id="manual"|Ready when you are|Install it, then sign it in/);
});

test("no account line of its own: sign-in stays in the header, signed in or not", () => {
  const out = draw("en");
  assert.doesNotMatch(out, /data-testid="download-(signup|waitlist|open-devices)"/, "no account link under the tabs");
  assert.doesNotMatch(text(out), /installed and signed in on the device that runs it/, "no needs line");
  assert.match(out, /aria-label="Sign in"/, "signed out, the header offers sign-in");
  const signedIn = draw("en", { signedIn: true, os: "windows" });
  assert.match(signedIn, /aria-label="Open app"/, "signed in, the header offers the app instead");
  assert.match(signedIn, /<a href="\/sessions"[^>]*data-testid="use-in-browser"/);
});

test("it leads with where Droppen goes, and reads in zh-CN, zh-TW and ja", () => {
  const cn = text(draw("zh-CN", { os: "macos-bg" }));
  assert.ok(cn.includes("在运行 AI 的设备上安装 Droppen"));
  assert.ok(cn.includes("手机和其他设备无需安装"));
  assert.ok(cn.indexOf("在运行 AI 的设备上安装") < cn.indexOf(INSTALL_COMMAND), "where it goes before the command");
  const tw = text(draw("zh-TW", { os: "linux" }));
  assert.ok(tw.includes("在執行 AI 的裝置上安裝 Droppen"));
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

test("the one-time-code hint names the placeholder the command has, in every language", () => {
  for (const [lng, table] of [
    ["zh-CN", downloadZhCN],
    ["zh-TW", downloadZhTW],
    ["ja", downloadJa],
  ] as const) {
    assert.ok(
      table["Replace <code> with the one-time code."].includes(table["<code>"]),
      `${lng}: the hint names ${table["<code>"]}`,
    );
  }
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
