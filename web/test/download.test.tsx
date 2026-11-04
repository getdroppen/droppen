// droppen.app/download (`features/landing/download.tsx`).
//
// What these pin: the hero has one action, the Mac App, with the other
// platforms behind its `▾`; the server section opens on the detected (or
// `?os=`) platform and keeps its three steps behind one tab; a browser installs
// nothing; update, uninstall and verification are folded after both; a file
// link is the release's own (or the release page), never an invented one; the
// page reads in zh-CN, en, zh-TW and ja with every key translated; `/download`
// is public signed in or not, and the landing page links it from its footer.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { I18nextProvider } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { landingBundles } from "../src/features/landing/copy.js";
import { downloadJa, downloadZhCN, downloadZhTW } from "../src/features/landing/download-copy.js";
import { detectKind, type EnvironmentHints, initialOs, osForKind } from "../src/features/landing/detect.js";
import DownloadPage from "../src/features/landing/download.js";
import { parseLatestRelease, type LatestRelease } from "../src/lib/hub/releases.js";
import {
  APP_AVAILABLE,
  APP_SIGN_IN_AVAILABLE,
  type Availability,
  AVAILABILITY,
  ATTESTATION_COMMAND,
  CHECKSUM_COMMAND,
  codeInstallCommand,
  DOWNLOAD_OSES,
  downloadHref,
  type DownloadOs,
  INSTALL_COMMAND,
  LINUX_AVAILABLE,
  LINGER_COMMAND,
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

const place = { search: "", origin: "https://droppen.app", hash: "" };
Object.assign(globalThis, { location: place });

const MAC_CHROME: EnvironmentHints = {
  userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36",
  platform: "macOS",
  mobile: false,
  maxTouchPoints: 0,
};
const LINUX_FIREFOX: EnvironmentHints = {
  userAgent: "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0",
  maxTouchPoints: 0,
};
const IPHONE: EnvironmentHints = {
  userAgent: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
  maxTouchPoints: 5,
};

const ALL_AVAILABLE: Availability = { app: true, appSignIn: true, linux: true };

function draw(
  lng: string,
  opts: { signedIn?: boolean; availability?: Availability; hints?: EnvironmentHints; os?: DownloadOs | null; cpu?: string | null; release?: LatestRelease | null } = {},
): string {
  void i18n.changeLanguage(lng);
  place.search = opts.os ? `?os=${opts.os}` : "";
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <DownloadPage
        signedIn={opts.signedIn}
        availability={opts.availability ?? ALL_AVAILABLE}
        hints={opts.hints ?? MAC_CHROME}
        cpu={opts.cpu}
        release={opts.release}
      />
    </I18nextProvider>,
  );
}

const text = (html: string) => html.replace(/<[^>]+>/g, " ").replace(/&#x27;/g, "'").replace(/&amp;/g, "&").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/\s+/g, " ");
const primaries = (html: string) => (html.match(/data-variant="primary"/g) ?? []).length;
const tag = (html: string, testId: string) => html.match(new RegExp(`<(?:details|div|button|a|ol|dl)[^>]*data-testid="${testId}"[^>]*>`))?.[0] ?? "";
const block = (html: string, testId: string, end: string) => html.match(new RegExp(`data-testid="${testId}"[^]*?${end}`))?.[0] ?? "";

/** The hub's answer for the latest release, as GitHub names its files. */
const DL = "https://github.com/getdroppen/droppen/releases/download/v0.2.129/";
const ANSWER = {
  version: "0.2.129",
  tag: "v0.2.129",
  published_at: "2026-10-24T09:30:00Z",
  releases_page: "https://github.com/getdroppen/droppen/releases/tag/v0.2.129",
  platforms: {
    macos: { name: "Droppen-0.2.129.dmg", url: `${DL}Droppen-0.2.129.dmg`, size: 48_000_000 },
    "linux-x86_64": { name: "droppen-v0.2.129-linux-x86_64.tar.gz", url: `${DL}droppen-v0.2.129-linux-x86_64.tar.gz`, size: 12_300_000 },
    "linux-aarch64": { name: "droppen-v0.2.129-linux-aarch64.tar.gz", url: `${DL}droppen-v0.2.129-linux-aarch64.tar.gz`, size: 11_000_000 },
  },
  fetched_ms: 1_793_000_000_000,
};
const RELEASE = parseLatestRelease(ANSWER)!;

test("detection, over real user agents: the device's kind, and the state it opens on", () => {
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
    { name: "nothing to go on", hints: { userAgent: "" }, kind: "unknown", os: null },
  ];
  for (const row of table) {
    const kind = detectKind(row.hints);
    assert.equal(kind, row.kind, row.name);
    assert.equal(osForKind(kind), row.os, row.name);
  }
});

test("the state: the address wins, then the session's choice, then the detected one", () => {
  assert.equal(initialOs({ search: "?os=linux", remembered: "windows", detected: "macos-app" }), "linux");
  assert.equal(initialOs({ search: "", remembered: "windows", detected: "macos-app" }), "windows");
  assert.equal(initialOs({ search: "?os=nope", remembered: null, detected: "mobile" }), "mobile");
  assert.equal(initialOs({ search: "", remembered: null, detected: null }), "macos-app", "the Mac by default");
  for (const os of DOWNLOAD_OSES) {
    assert.equal(osOfSearch(downloadHref(os).replace("/download", "")), os, "the address names it, and reads back");
  }
  assert.equal(downloadHref(), "/download");
  assert.equal(downloadHref("macos-bg"), "/download?os=macos-bg");
  assert.equal(osOfSearch("?os=macos"), null);
});

test("the page opens on the address's platform, else the detected one", () => {
  assert.match(draw("zh-CN"), /data-testid="environments" data-os="macos-app" data-detected="macos-app"/);
  assert.match(draw("zh-CN", { os: "linux" }), /data-os="linux" data-detected="macos-app"/);
  assert.match(draw("zh-CN", { os: "windows" }), /data-os="windows"/);
  assert.match(draw("zh-CN", { os: "macos-bg" }), /data-os="macos-bg"/);
  assert.match(draw("zh-CN", { hints: IPHONE }), /data-os="mobile" data-detected="mobile"/);
  // Nothing detected: the Mac by default, and no `data-detected` at all.
  assert.match(draw("en", { hints: { userAgent: "" } }), /data-testid="environments" data-os="macos-app" class=/);
  // The Mac button and its menu are the first screen's only actions.
  const html = draw("en");
  const hero = html.slice(0, html.indexOf('id="server"'));
  assert.equal(primaries(hero), 1, "one primary action: the download");
  assert.equal((hero.match(/data-testid="platform-menu-trigger"/g) ?? []).length, 1);
  assert.match(tag(html, "platform-menu-trigger"), /aria-haspopup="menu"/);
});

test("the Mac App: one download and its requirements; the other platforms are behind the arrow", () => {
  const html = draw("en", { release: RELEASE });
  assert.match(html, new RegExp(`<a href="${DL}Droppen-0\\.2\\.129\\.dmg" data-variant="primary" data-testid="app-download"`));
  assert.match(html, /data-testid="app-file-facts"[^>]*>v0\.2\.129 · 48 MB/);
  assert.ok(text(html).includes("macOS 13 or later · Apple silicon and Intel"));
  assert.match(block(html, "app-download", "</a>"), /<svg/, "the Apple mark leads the button");
  // Without an answer: the release page, in a new tab, and no invented version.
  const asking = draw("en");
  assert.match(asking, new RegExp(`<a href="${RELEASES_URL}" target="_blank" rel="noopener noreferrer" data-variant="primary" data-testid="app-download"`));
  assert.doesNotMatch(asking, /data-testid="app-file-facts"/);
  // No App yet: the command is the way in, and it is the one action.
  const noApp = draw("zh-CN", { availability: { app: false, appSignIn: false, linux: true } });
  assert.match(noApp, /data-testid="app-soon"/);
  assert.ok(text(noApp).includes("App 即将推出"));
  assert.ok(text(noApp).includes(INSTALL_COMMAND), "the way in today");
  assert.doesNotMatch(noApp, /data-testid="app-download"/);
  assert.match(tag(noApp, "install-command-copy"), /data-variant="primary"/);
  // Sign-in from the App still coming: said once, honestly.
  const off = draw("en", { availability: { app: true, appSignIn: false, linux: true } });
  assert.match(off, /data-testid="app-sign-in-soon"/);
  assert.ok(text(off).includes("signing in to Droppen from the app is coming in an update"));
  assert.doesNotMatch(draw("en"), /data-testid="app-sign-in-soon"/);
});

test("the server: three tabs, opening on the device's platform, with one set of three steps", () => {
  const opens: [DownloadOs, string, string][] = [
    ["macos-app", "macos", "macOS 13 or later · Apple silicon and Intel"],
    ["linux", "linux", "Linux · x86_64 or aarch64 · glibc 2.28+ · systemd"],
    ["windows", "windows", "Windows · WSL 2"],
  ];
  for (const [os, tab, requirement] of opens) {
    const html = draw("en", { os });
    const tabs = [...html.matchAll(/data-testid="server-tab-(\w+)"/g)].map((m) => m[1]);
    assert.deepEqual(tabs, ["macos", "linux", "windows"], `${os}: every platform is offered`);
    for (const other of ["macos", "linux", "windows"]) {
      assert.match(tag(html, `server-tab-${other}`), new RegExp(`aria-selected="${other === tab}"`), `${os}: ${other}`);
    }
    const chosen = block(html, `server-tab-${tab}`, "</button>");
    assert.match(chosen, /<svg/, `${os}: the tab carries the platform's mark`);
    assert.ok(text(html).includes(requirement), `${os}: ${requirement}`);
    const steps = block(html, "server-steps", "</ol>");
    assert.equal((steps.match(/<li/g) ?? []).length, 3, `${os}: three steps`);
    for (const title of ["Get a one-time code", "Install it", "Check it is online"]) assert.ok(text(steps).includes(title), `${os}: ${title}`);
    const items = steps.split("<li").slice(1);
    assert.match(items[1]!, /data-testid="install-command"/, `${os}: the command is inside step 2`);
    assert.ok(text(items[1]!).includes(codeInstallCommand("<code>")));
    if (tab === "macos") {
      assert.doesNotMatch(html, /data-testid="linger-command"/, "macOS's launchd agent needs no lingering");
    } else {
      assert.match(items[2]!, /data-testid="linger-command"/, `${os}: lingering is step 3's action`);
    }
  }
  // WSL says how to turn systemd on.
  const windows = draw("en", { os: "windows" });
  assert.match(windows, /<a href="https:\/\/learn\.microsoft\.com\/windows\/wsl\/systemd" target="_blank" rel="noopener noreferrer" data-testid="wsl-systemd"/);
  assert.doesNotMatch(draw("en", { os: "macos-app" }), /data-testid="wsl-systemd"/);
});

test("Linux says which processor this browser reports, and when there is no build for it", () => {
  const built = draw("en", { os: "linux", hints: LINUX_FIREFOX, cpu: "x86_64" });
  assert.match(built, /data-testid="linux-cpu"/);
  assert.ok(text(built).includes("This browser reports Linux on x86_64."));
  assert.ok(!text(built).includes("Droppen's Linux builds are for x86_64 and aarch64."));
  const other = draw("zh-CN", { os: "linux", hints: LINUX_FIREFOX, cpu: "armv7l" });
  assert.ok(text(other).includes("Droppen 的 Linux 版本支持 x86_64 和 aarch64。"));
  assert.equal((text(other).match(/Droppen 的 Linux 版本支持/g) ?? []).length, 1, "said once");
  // Read on a Mac, the Mac's processor says nothing about a server's.
  assert.doesNotMatch(draw("en", { os: "linux", hints: MAC_CHROME, cpu: "aarch64" }), /data-testid="linux-cpu"/);
  // A phone that landed on the server section says nothing about a processor.
  assert.doesNotMatch(draw("en", { hints: IPHONE, cpu: "aarch64" }), /data-testid="linux-cpu"/);
});

test("with Linux not yet available, the server tab says so instead of a command that would not work", () => {
  const off: Availability = { app: true, appSignIn: true, linux: false };
  const linux = draw("en", { os: "linux", availability: off });
  assert.ok(text(linux).includes("Linux support, with no browser needed on the server, is on its way."));
  assert.ok(!text(linux).includes("DROPPEN_CODE"));
  assert.doesNotMatch(linux, /data-testid="server-steps"/);
  const windows = draw("en", { os: "windows", availability: off });
  assert.ok(text(windows).includes("Once Linux is supported: a Linux server, or WSL."));
  assert.doesNotMatch(windows, /data-testid="server-steps"/);
  // macOS keeps its steps: the install script still runs.
  assert.match(draw("en", { os: "macos-app", availability: off }), /data-testid="server-steps"/);
});

test("the browser: nothing to install, approved by a device already in the account", () => {
  const html = draw("en");
  assert.ok(html.indexOf('id="browser"') > 0);
  assert.match(html, /id="phone"/);
  const steps = block(html, "phone-steps", "</ol>");
  assert.equal((steps.match(/<li/g) ?? []).length, 2);
  for (const title of ["Sign in", "Approve this browser"]) assert.ok(text(steps).includes(title), title);
  assert.ok(html.indexOf('data-testid="phone-steps"') > html.indexOf('id="server"'), "after the server");
});

test("maintenance is folded after both paths, and the old anchors open the part they name", () => {
  const html = draw("en", { release: RELEASE });
  const manage = html.slice(html.indexOf('id="manage"'));
  for (const id of ["update", "uninstall", "verify"]) {
    const item = tag(manage, `manage-${id}`);
    assert.ok(item, `#${id} is an item`);
    assert.ok(item.includes(`id="${id}"`), `#${id} keeps its anchor`);
    assert.ok(item.includes("scroll-mt-14"), `#${id} clears the 56px header`);
  }
  assert.equal((manage.match(/aria-expanded="false"/g) ?? []).length, 3, "all three folded");
  assert.doesNotMatch(manage, /aria-expanded="true"/);
  assert.match(html, /data-testid="update-command"[^>]*>[^<]*.*curl -fsSL https:\/\/droppen\.app\/install\.sh \| sh/s);
  const steps = block(html, "uninstall-steps", "</ol>");
  assert.equal((steps.match(/<li/g) ?? []).length, 3);
  for (const line of ["Remove it from your account", "Disconnect it", "Remove the program"]) assert.ok(text(steps).includes(line), line);
  assert.ok(text(html).includes(UNINSTALL_COMMAND));
  assert.ok(text(html).includes(CHECKSUM_COMMAND));
  assert.ok(text(html).includes(ATTESTATION_COMMAND));
  assert.match(html, /data-testid="download-verify"/);
  assert.match(html, /<a href="\/security"[^>]*data-testid="download-security"/);
  assert.match(html, /data-testid="verify-download-link"/);
  assert.ok(html.indexOf('data-testid="phone-steps"') < html.indexOf('id="manage"'));
});

test("the switches degrade honestly: no App is the command, no Linux is a line — never a link to a file that is not there", () => {
  assert.equal(APP_AVAILABLE, true, "the signed and notarised App ships from v0.2.123");
  assert.equal(LINUX_AVAILABLE, true, "Linux tarballs and DROPPEN_CODE ship from v0.2.121");
  assert.equal(APP_SIGN_IN_AVAILABLE, true);
  assert.equal(AVAILABILITY.appSignIn, APP_SIGN_IN_AVAILABLE, "the page reads the switch");
});

test("it reads in zh-CN, zh-TW and ja, with nothing left in English", () => {
  const cn = text(draw("zh-CN"));
  assert.ok(cn.includes("下载 Droppen") && cn.includes("运行服务器") && cn.includes("管理安装"));
  const tw = text(draw("zh-TW"));
  assert.ok(tw.includes("下載 Droppen") && tw.includes("執行伺服器") && tw.includes("管理安裝"));
  const jp = text(draw("ja"));
  assert.ok(jp.includes("Droppen をダウンロード") && jp.includes("サーバーを動かす") && jp.includes("インストールの管理"));
  for (const words of [cn, tw, jp]) {
    assert.doesNotMatch(words, /Download Droppen|Run the server|Manage your installation|Nothing to install: any browser works/);
  }
});

test("every key the download page uses is translated in all three tables, with the same placeholders", () => {
  const source = readFileSync(new URL("../src/features/landing/download.tsx", import.meta.url), "utf8");
  const used = new Set<string>();
  for (const match of source.matchAll(/(?<![A-Za-z0-9_$.])t\("([^"]+)"/g)) used.add(match[1]!);
  // Passed through `t(label)` by the server tabs, so the scanner cannot see them.
  for (const label of ["macOS", "Linux", "Windows / WSL"]) used.add(label);
  assert.ok(used.size > 60, "the page's strings are found");
  const placeholders = (value: string) => [...value.matchAll(/\{\{\w+\}\}/g)].map((m) => m[0]).sort().join(",");
  for (const key of used) {
    assert.ok(downloadZhCN[key], `zh-CN has ${JSON.stringify(key)}`);
    for (const [lng, table] of [["zh-TW", downloadZhTW], ["ja", downloadJa]] as const) {
      assert.ok(table[key], `${lng} has ${JSON.stringify(key)}`);
      assert.equal(placeholders(table[key]!), placeholders(key), `${lng}: ${JSON.stringify(key)} keeps its placeholders`);
    }
  }
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
