// The App downloaded from the home page and the download page, directly:
// `features/landing/hero-get.tsx`, `download.tsx`, `release-files.tsx`,
// `detect.ts` and the hub's answer, `lib/hub/releases.ts`.
//
// What these pin: the latest release is asked of this page's hub
// (`GET /v1/releases/latest`, same-origin: droppen.app's CSP stays as it is)
// and checked again here (the tag, each file's exact name and address); every
// download button is then a plain link to the file itself, which downloads
// in place, with the version and size beside it — and on any failure a link
// to the release page; the release page is beside it for checksums and
// other files; Linux gets the tarball for its processor after the command;
// the device is told from
// `navigator.userAgentData` (platform, and the `architecture` / `bitness`
// high-entropy hints) or else the user agent, with no request; and the hero
// offers what fits it — the download on a Mac, the install command on Linux,
// droppen.app in the browser on Windows and on a phone, every way on a device
// it cannot tell — with one primary action, in en, zh-CN, zh-TW and ja.
// A hub that counts downloads names each file's `download_path`
// (`/dl/<platform>`, its own `302` to the file, counted per platform and
// day); then the buttons link that, on this page's origin, with the same
// name, version and size, and a hub without it keeps the direct links.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { I18nextProvider } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { landingBundles } from "../src/features/landing/copy.js";
import {
  browserCpu,
  browserHints,
  CPU_HINTS,
  cpuOf,
  DEFAULT_OS,
  detectKind,
  type EnvironmentHints,
  heroOffer,
  initialOs,
  type NavigatorLike,
  osForKind,
} from "../src/features/landing/detect.js";
import DownloadPage from "../src/features/landing/download.js";
import { APP_DOWNLOAD_URL, type Availability, AVAILABILITY, INSTALL_COMMAND, RELEASES_URL } from "../src/features/landing/get-droppen.js";
import LandingPage from "../src/features/landing/landing.js";
import { downloadPath, fetchLatestRelease, formatSize, LATEST_RELEASE_PATH, type LatestRelease, parseLatestRelease } from "../src/lib/hub/releases.js";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "../src/i18n/cloud-devices.js";
import { ja } from "../src/i18n/ja.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";
import { makeI18n } from "./render.js";

const app = { "zh-CN": { ...cloudDevicesZhCN, ...zhCN }, "zh-TW": { ...cloudDevicesZhTW, ...zhTW }, ja: { ...cloudDevicesJa, ...ja } };
const i18n = makeI18n({
  lng: "en",
  resources: {
    en: { translation: {} },
    "zh-CN": { translation: { ...landingBundles["zh-CN"], ...app["zh-CN"] } },
    "zh-TW": { translation: { ...landingBundles["zh-TW"], ...app["zh-TW"] } },
    ja: { translation: { ...landingBundles.ja, ...app.ja } }
  },
  keySeparator: false,
  nsSeparator: false
});

// The download page reads the address (`?os=`) and the session's choice.
const place = { search: "", origin: "https://droppen.app", hash: "", pathname: "/" };
const session = new Map<string, string>();
Object.assign(globalThis, {
  location: place,
  sessionStorage: { getItem: (k: string) => session.get(k) ?? null, setItem: (k: string, v: string) => void session.set(k, v), removeItem: (k: string) => void session.delete(k) },
});

const UA = {
  macSafari: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15",
  macChrome: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36",
  linuxFirefox: "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0",
  linuxFirefoxArm: "Mozilla/5.0 (X11; Linux aarch64; rv:131.0) Gecko/20100101 Firefox/131.0",
  linuxChrome: "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36",
  windowsEdge: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36 Edg/129.0.0.0",
  iphone: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
  android: "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Mobile Safari/537.36",
  raspberry: "Mozilla/5.0 (X11; Linux armv7l; rv:115.0) Gecko/20100101 Firefox/115.0",
};

const HINTS: Record<"mac" | "linux" | "windows" | "phone" | "unknown", EnvironmentHints> = {
  mac: { userAgent: UA.macChrome, platform: "macOS", mobile: false, maxTouchPoints: 0 },
  linux: { userAgent: UA.linuxFirefox, maxTouchPoints: 0 },
  windows: { userAgent: UA.windowsEdge, platform: "Windows", mobile: false, maxTouchPoints: 0 },
  phone: { userAgent: UA.iphone, maxTouchPoints: 5 },
  unknown: { userAgent: "" },
};

// `&amp;` last, so an escaped entity (`&amp;lt;`) is not decoded twice.
const text = (html: string) => html.replace(/<[^>]+>/g, " ").replace(/&#x27;/g, "'").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&amp;/g, "&").replace(/\s+/g, " ");
const primaries = (html: string) => (html.match(/data-variant="primary"/g) ?? []).length;
const esc = (s: string) => s.replace(/[.?*+^$()[\]{}|/\\]/g, "\\$&");

/** The hub's answer for the latest release (docs/hub-api/http.md), as GitHub names its files. */
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
/** The same, from a hub that counts downloads: each file names its `/dl/<platform>`. */
const COUNTED_ANSWER = {
  ...ANSWER,
  platforms: {
    macos: { ...ANSWER.platforms.macos, download_path: "/dl/macos" },
    "linux-x86_64": { ...ANSWER.platforms["linux-x86_64"], download_path: "/dl/linux-x86_64" },
    "linux-aarch64": { ...ANSWER.platforms["linux-aarch64"], download_path: "/dl/linux-aarch64" },
  },
};
const COUNTED = parseLatestRelease(COUNTED_ANSWER)!;

function landing(lng: string, opts: { hints?: EnvironmentHints; signedIn?: boolean; availability?: Availability; release?: LatestRelease | null; cpu?: string | null } = {}): string {
  void i18n.changeLanguage(lng);
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <LandingPage
        signedIn={opts.signedIn}
        hints={opts.hints ?? HINTS.unknown}
        availability={opts.availability ?? AVAILABILITY}
        release={opts.release === undefined ? RELEASE : opts.release}
        cpu={opts.cpu === undefined ? null : opts.cpu}
      />
    </I18nextProvider>,
  );
}

/** The hero's actions alone. */
function hero(html: string): string {
  const from = html.indexOf('data-testid="hero-get"');
  assert.ok(from >= 0, "the hero's actions are there");
  return html.slice(html.lastIndexOf("<div", from), html.indexOf('data-diagram="hero"', from));
}

function download(
  lng: string,
  opts: { hints?: EnvironmentHints; os?: string; cpu?: string | null; availability?: Availability; signedIn?: boolean; release?: LatestRelease | null } = {},
): string {
  void i18n.changeLanguage(lng);
  place.search = opts.os ? `?os=${opts.os}` : "";
  session.clear();
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <DownloadPage
        signedIn={opts.signedIn}
        hints={opts.hints ?? HINTS.mac}
        cpu={opts.cpu}
        availability={opts.availability ?? AVAILABILITY}
        release={opts.release === undefined ? RELEASE : opts.release}
      />
    </I18nextProvider>,
  );
}

/** A stand-in for `fetch`: answers `status` with `body`, or throws; records what was asked. */
function fakeFetch(status: number, body: unknown, asked: { url: string; init?: RequestInit }[] = []): typeof fetch {
  return (async (url: string | URL | Request, init?: RequestInit) => {
    asked.push({ url: String(url), init });
    if (body instanceof Error) throw body;
    return new Response(typeof body === "string" ? body : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
  }) as typeof fetch;
}

test("the latest release is asked of this page's hub, same-origin, and checked again: the tag, each file's exact name and address", async () => {
  const asked: { url: string; init?: RequestInit }[] = [];
  const release = await fetchLatestRelease(fakeFetch(200, ANSWER, asked));
  assert.deepEqual(release, RELEASE);
  assert.equal(LATEST_RELEASE_PATH, "/v1/releases/latest");
  assert.equal(asked[0]!.url, "/v1/releases/latest", "a relative address: this page's own origin");
  assert.equal(asked[0]!.init?.credentials, "omit");
  assert.equal(RELEASE.version, "0.2.129");
  assert.equal(RELEASE.releasesPage, "https://github.com/getdroppen/droppen/releases/tag/v0.2.129");
  assert.equal(RELEASE.files.macos?.url, `${DL}Droppen-0.2.129.dmg`);
  assert.equal(RELEASE.files["linux-aarch64"]?.size, 11_000_000);
  // Any failure is none: the buttons link the release page.
  for (const [status, body] of [
    [503, { error: "release_unavailable", message: "not yet" }],
    [404, { error: "not_found", message: "not found" }],
    [429, { error: "rate", message: "slow down" }],
    [200, "not json"],
    [200, new TypeError("Failed to fetch")],
  ] as const) {
    assert.equal(await fetchLatestRelease(fakeFetch(status, body)), null, `${status} ${String(body)}`);
  }
  // A file at another address, or under another name, is left out; with none left, there is no release.
  const at = (platform: string, change: Record<string, unknown>) =>
    parseLatestRelease({ ...ANSWER, platforms: { ...ANSWER.platforms, [platform]: { ...ANSWER.platforms[platform as keyof typeof ANSWER.platforms], ...change } } });
  for (const url of [
    "https://evil.example/Droppen-0.2.129.dmg",
    "http://github.com/getdroppen/droppen/releases/download/v0.2.129/Droppen-0.2.129.dmg",
    "https://github.com/other/droppen/releases/download/v0.2.129/Droppen-0.2.129.dmg",
    "https://github.com/getdroppen/droppen/releases/download/v0.2.128/Droppen-0.2.129.dmg",
    "https://github.com/getdroppen/droppen/releases/download/v0.2.129/../Droppen-0.2.129.dmg",
    "javascript:alert(1)",
  ]) {
    const checked = at("macos", { url });
    assert.equal(checked?.files.macos, undefined, url);
    assert.ok(checked?.files["linux-x86_64"], "the other files stay");
  }
  assert.equal(at("macos", { name: "Droppen.dmg", url: `${DL}Droppen.dmg` })?.files.macos, undefined, "an odd name");
  assert.equal(at("linux-x86_64", { name: "remote-hand-v0.2.129-linux-x86_64.tar.gz", url: `${DL}remote-hand-v0.2.129-linux-x86_64.tar.gz` })?.files["linux-x86_64"], undefined, "the legacy name");
  assert.equal(at("macos", { size: 0 })?.files.macos, undefined);
  assert.equal(at("macos", { size: "48 MB" })?.files.macos, undefined);
  assert.equal(parseLatestRelease({ ...ANSWER, platforms: {} }), null);
  // The release itself: a vX.Y.Z tag, its version.
  assert.equal(parseLatestRelease({ ...ANSWER, tag: "v0.2.129-rc.1" }), null);
  assert.equal(parseLatestRelease({ ...ANSWER, version: "0.2.130" }), null);
  assert.equal(parseLatestRelease(null), null);
  assert.equal(parseLatestRelease([ANSWER]), null);
  // The release page is made from the tag, never taken from the answer.
  assert.equal(parseLatestRelease({ ...ANSWER, releases_page: "https://evil.example/" })?.releasesPage, "https://github.com/getdroppen/droppen/releases/tag/v0.2.129");
  assert.deepEqual([formatSize(48_000_000), formatSize(9_500_000), formatSize(640_000), formatSize(1_200_000_000)], ["48 MB", "9.5 MB", "640 KB", "1.2 GB"]);
});

test("with the hub's answer, every download button is a plain link to the file itself, with its version and size", () => {
  const links = [
    ...hero(landing("en", { hints: HINTS.mac })).matchAll(/<a [^>]*data-testid="hero-download"[^>]*>/g),
    ...hero(landing("en", { hints: HINTS.unknown })).matchAll(/<a [^>]*data-testid="hero-download"[^>]*>/g),
    ...download("en", { os: "macos-app" }).matchAll(/<a [^>]*data-testid="app-download"[^>]*>/g),
  ].map((m) => m[0]);
  assert.equal(links.length, 3);
  for (const a of links) {
    assert.match(a, new RegExp(`^<a href="${esc(`${DL}Droppen-0.2.129.dmg`)}" `), a);
    assert.doesNotMatch(a, /target=|rel=|download=/, a);
  }
  assert.ok(text(hero(landing("en", { hints: HINTS.mac }))).includes("v0.2.129 · 48 MB · macOS 13 or later · Apple silicon and Intel"));
  const page = download("en", { os: "macos-app" });
  assert.ok(text(page).includes("v0.2.129 · 48 MB · macOS 13 or later · Universal: Apple silicon and Intel"));
  // Downloaded, opened, then signed in from inside the App (`APP_SIGN_IN_AVAILABLE`): never "coming in an update".
  assert.ok(text(page).includes("Click Sign in to Droppen in the app"));
  assert.doesNotMatch(page, /data-testid="app-sign-in-soon"|coming in an update/);
  // The release page beside it, for the checksums and the other files: that release's.
  assert.match(page, /<a href="https:\/\/github\.com\/getdroppen\/droppen\/releases\/tag\/v0\.2\.129" target="_blank" rel="noopener noreferrer" data-testid="app-release-link"/);
  // The add-device guide's button too (its source: the guide needs an account to render).
  const guide = readFileSync(new URL("../src/features/cloud/add-computer.tsx", import.meta.url), "utf8");
  assert.match(guide, /<a \{\.\.\.fileLink\(latest\?\.files\.macos\)\} data-testid="add-app-download"/);
});

test("a hub that counts downloads: each button links its /dl/<platform> on this page's origin, with the same name, version and size; without it, the file on GitHub", () => {
  // Taken only as this page would make it.
  assert.equal(COUNTED.files.macos?.href, "/dl/macos");
  assert.equal(COUNTED.files["linux-x86_64"]?.href, "/dl/linux-x86_64");
  assert.equal(COUNTED.files["linux-aarch64"]?.href, "/dl/linux-aarch64");
  assert.equal(downloadPath("macos"), "/dl/macos");
  assert.equal(COUNTED.files.macos?.url, `${DL}Droppen-0.2.129.dmg`, "the file it goes on to is still checked");
  assert.equal(RELEASE.files.macos?.href, RELEASE.files.macos?.url, "a hub before it: the file itself");
  const at = (path: unknown) =>
    parseLatestRelease({ ...COUNTED_ANSWER, platforms: { ...COUNTED_ANSWER.platforms, macos: { ...COUNTED_ANSWER.platforms.macos, download_path: path } } })?.files.macos?.href;
  for (const odd of ["/dl/linux-x86_64", "/dl/macos/", "/dl/macos?x=1", "//evil.example/dl/macos", "https://evil.example/dl/macos", "https://droppen.app/dl/macos", "dl/macos", "/DL/macos", "javascript:alert(1)", 7, null]) {
    assert.equal(at(odd), `${DL}Droppen-0.2.129.dmg`, `${String(odd)}: the file itself`);
  }
  // Every button: the Mac's (hero, download page), the Linux tarballs (hero, download page).
  const mac = [
    ...hero(landing("en", { hints: HINTS.mac, release: COUNTED })).matchAll(/<a [^>]*data-testid="hero-download"[^>]*>/g),
    ...hero(landing("en", { hints: HINTS.unknown, release: COUNTED })).matchAll(/<a [^>]*data-testid="hero-download"[^>]*>/g),
    ...download("en", { os: "macos-app", release: COUNTED }).matchAll(/<a [^>]*data-testid="app-download"[^>]*>/g),
  ].map((m) => m[0]);
  assert.equal(mac.length, 3);
  for (const a of mac) {
    assert.match(a, /^<a href="\/dl\/macos" rel="nofollow" /, a);
    assert.doesNotMatch(a, /target=|download=/, a);
  }
  assert.ok(text(hero(landing("en", { hints: HINTS.mac, release: COUNTED }))).includes("v0.2.129 · 48 MB · macOS 13 or later · Apple silicon and Intel"));
  assert.ok(text(download("en", { os: "macos-app", release: COUNTED })).includes("v0.2.129 · 48 MB · macOS 13 or later · Universal: Apple silicon and Intel"));
  const linuxHero = hero(landing("en", { hints: HINTS.linux, cpu: "x86_64", release: COUNTED }));
  assert.deepEqual([...linuxHero.matchAll(/<a href="([^"]+)" data-testid="hero-tarballs-file"[^>]*>([^<]+)</g)].map((m) => [m[1], m[2]]), [["/dl/linux-x86_64", "droppen-v0.2.129-linux-x86_64.tar.gz"]]);
  assert.match(linuxHero, /<a href="\/dl\/linux-x86_64" data-testid="hero-tarballs-file" class="[^"]*" rel="nofollow">/, "crawlers leave the count alone");
  assert.ok(text(linuxHero).includes("Or download it: droppen-v0.2.129-linux-x86_64.tar.gz (12 MB)"));
  const linuxTab = download("en", { os: "linux", hints: HINTS.linux, release: COUNTED });
  assert.deepEqual(
    [...linuxTab.matchAll(/<a href="([^"]+)" data-testid="linux-tarballs-file"[^>]*>([^<]+)</g)].map((m) => [m[1], m[2]]),
    [
      ["/dl/linux-x86_64", "droppen-v0.2.129-linux-x86_64.tar.gz"],
      ["/dl/linux-aarch64", "droppen-v0.2.129-linux-aarch64.tar.gz"],
    ],
  );
  // The release page beside it is still GitHub's, for the checksums and the other files.
  assert.match(download("en", { os: "macos-app", release: COUNTED }), /<a href="https:\/\/github\.com\/getdroppen\/droppen\/releases\/tag\/v0\.2\.129" target="_blank" rel="noopener noreferrer" data-testid="app-release-link"/);
  // A hub without it: the files on GitHub, as before, and no nofollow on them.
  const direct = hero(landing("en", { hints: HINTS.mac })) + download("en", { os: "linux", hints: HINTS.linux });
  assert.doesNotMatch(direct, /href="\/dl\//);
  assert.doesNotMatch(direct, /nofollow/);
  // And crawlers are asked to leave /dl/ alone: the web app's robots.txt, served on every host of the hub.
  const robots = readFileSync(new URL("../public/robots.txt", import.meta.url), "utf8");
  assert.match(robots, /^User-agent: \*\nDisallow: \/dl\/\n/m);
  assert.doesNotMatch(robots.replace(/^#.*\n/gm, ""), /Disallow: \/(?!dl\/)/, "nothing else is disallowed");
});

test("without it (no route, a 503, a failure, or not yet), every download button links the release page, in a new tab", () => {
  assert.equal(APP_DOWNLOAD_URL, RELEASES_URL);
  assert.equal(RELEASES_URL, "https://github.com/getdroppen/droppen/releases/latest");
  // None (the hub said nothing usable), and not yet (asked, no answer: as the page first draws).
  void i18n.changeLanguage("en");
  const asking = renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <LandingPage hints={HINTS.mac} />
    </I18nextProvider>,
  );
  const pages = [hero(landing("en", { hints: HINTS.mac, release: null })), hero(landing("en", { hints: HINTS.unknown, release: null })), download("en", { os: "macos-app", release: null }), hero(asking)];
  for (const html of pages) {
    const a = html.match(/<a [^>]*data-testid="(hero|app)-download"[^>]*>/)![0];
    assert.match(a, new RegExp(`^<a href="${esc(RELEASES_URL)}" target="_blank" rel="noopener noreferrer"`), a);
    assert.doesNotMatch(html, /v0\.2\.129|github\.com\/getdroppen\/droppen\/releases\/download/);
  }
  const page = download("en", { os: "macos-app", release: null });
  assert.ok(text(page).includes("macOS 13 or later · Universal: Apple silicon and Intel · Droppen-<version>.dmg"));
  assert.match(page, new RegExp(`<a href="${esc(RELEASES_URL)}" target="_blank" rel="noopener noreferrer" data-testid="app-release-link"`));
  assert.doesNotMatch(hero(landing("en", { hints: HINTS.linux, release: null, cpu: "x86_64" })), /data-testid="hero-tarballs"/, "no tarball named");
});

test("droppen.app's CSP stays as it is: nothing in the web app asks GitHub for anything, and the page carries no policy of its own", () => {
  const src = fileURLToPath(new URL("../src", import.meta.url));
  const files: string[] = [];
  const walk = (dir: string) => {
    for (const name of readdirSync(dir)) {
      const path = join(dir, name);
      if (statSync(path).isDirectory()) walk(path);
      else if (/\.(ts|tsx)$/.test(name)) files.push(path);
    }
  };
  walk(src);
  assert.ok(files.length > 100);
  for (const file of files) {
    const source = readFileSync(file, "utf8");
    assert.doesNotMatch(source, /api\.github\.com|objects\.githubusercontent\.com/, file);
    // A request (fetch, XHR, EventSource, a beacon, a WebSocket) to GitHub would need connect-src: the downloads are links.
    assert.doesNotMatch(source, /(fetch|XMLHttpRequest|EventSource|sendBeacon|WebSocket)\([^)]*github/i, file);
  }
  // The one request for the files: the hub's route, on the page's own origin.
  const releases = readFileSync(new URL("../src/lib/hub/releases.ts", import.meta.url), "utf8");
  assert.deepEqual([...releases.matchAll(/fetcher\(([^,]+),/g)].map((m) => m[1]), ["LATEST_RELEASE_PATH"]);
  assert.match(releases, /export const LATEST_RELEASE_PATH = "\/v1\/releases\/latest";/);
  const detect = readFileSync(new URL("../src/features/landing/detect.ts", import.meta.url), "utf8");
  assert.doesNotMatch(detect, /\bfetch\(|XMLHttpRequest/, "detection asks nobody");
  // The policy is the hub's header (connect-src 'self' and the relays); the shell adds none.
  const shell = readFileSync(new URL("../index.html", import.meta.url), "utf8");
  assert.doesNotMatch(shell, /Content-Security-Policy/i);
});

test("detection, from navigator.userAgentData where there is one, else the user agent", () => {
  const nav = (userAgent: string, data?: NavigatorLike["userAgentData"], maxTouchPoints = 0): NavigatorLike => ({ userAgent, userAgentData: data, maxTouchPoints });
  const table: { name: string; nav: NavigatorLike; kind: string; os: string | null; offer: string }[] = [
    { name: "Chrome on macOS: userAgentData says macOS", nav: nav(UA.macChrome, { platform: "macOS", mobile: false }), kind: "mac", os: "macos-app", offer: "mac" },
    { name: "Safari on macOS: no userAgentData, the user agent", nav: nav(UA.macSafari), kind: "mac", os: "macos-app", offer: "mac" },
    { name: "iPadOS Safari says Mac, with touch", nav: nav(UA.macSafari, undefined, 5), kind: "ipad", os: "mobile", offer: "phone" },
    { name: "Chrome on Linux: userAgentData says Linux", nav: nav(UA.linuxChrome, { platform: "Linux", mobile: false }), kind: "linux", os: "linux", offer: "linux" },
    { name: "Firefox on Linux: the user agent", nav: nav(UA.linuxFirefox), kind: "linux", os: "linux", offer: "linux" },
    { name: "Edge on Windows: userAgentData says Windows", nav: nav(UA.windowsEdge, { platform: "Windows", mobile: false }), kind: "windows", os: "windows", offer: "windows" },
    { name: "Windows, from the user agent alone", nav: nav(UA.windowsEdge), kind: "windows", os: "windows", offer: "windows" },
    { name: "Safari on iPhone", nav: nav(UA.iphone, undefined, 5), kind: "iphone", os: "mobile", offer: "phone" },
    { name: "Chrome on Android: userAgentData says Android, mobile", nav: nav(UA.android, { platform: "Android", mobile: true }, 5), kind: "android", os: "mobile", offer: "phone" },
    // userAgentData wins over a user agent that says otherwise (a reduced or spoofed one).
    { name: "userAgentData says macOS under a Windows user agent", nav: nav(UA.windowsEdge, { platform: "macOS", mobile: false }), kind: "mac", os: "macos-app", offer: "mac" },
    { name: "Chrome OS", nav: nav(UA.linuxChrome.replace("Linux x86_64", "CrOS x86_64 14541.0.0"), { platform: "Chrome OS", mobile: false }), kind: "chromeos", os: "linux", offer: "all" },
    { name: "nothing to go on", nav: nav(""), kind: "unknown", os: null, offer: "all" },
  ];
  for (const row of table) {
    const hints = browserHints(row.nav);
    assert.ok(hints, row.name);
    const kind = detectKind(hints);
    assert.equal(kind, row.kind, row.name);
    assert.equal(osForKind(kind), row.os, row.name);
    assert.equal(heroOffer(kind, AVAILABILITY), row.offer, row.name);
  }
  assert.equal(browserHints(null), null, "outside a browser");
  // Behind the build's switches: no App yet, a Mac is offered every way; no Linux yet, Linux too.
  assert.equal(heroOffer("mac", { app: false, linux: true }), "all");
  assert.equal(heroOffer("linux", { app: true, linux: false }), "all");
  // An unknown device opens on the Mac App, marked as nothing.
  assert.equal(DEFAULT_OS, "macos-app");
  assert.equal(initialOs({ search: "", remembered: null, detected: null }), "macos-app");
  assert.equal(initialOs({ search: "", remembered: null, detected: "linux" }), "linux");
  assert.equal(initialOs({ search: "?os=windows", remembered: null, detected: "linux" }), "windows", "the address wins");
});

test("the processor: the architecture and bitness high-entropy hints, else the user agent — asked for nothing else", async () => {
  const asked: string[][] = [];
  const data = (values: { architecture?: string; bitness?: string } | Error) => ({
    platform: "Linux",
    mobile: false,
    getHighEntropyValues: async (hints: string[]) => {
      asked.push(hints);
      if (values instanceof Error) throw values;
      return values;
    },
  });
  assert.equal(await browserCpu({ userAgent: UA.linuxChrome, userAgentData: data({ architecture: "arm", bitness: "64" }) }), "aarch64", "Chrome's frozen user agent says x86_64; the hints know better");
  assert.equal(await browserCpu({ userAgent: UA.linuxChrome, userAgentData: data({ architecture: "x86", bitness: "64" }) }), "x86_64");
  assert.equal(await browserCpu({ userAgent: UA.linuxChrome, userAgentData: data({ architecture: "x86", bitness: "32" }) }), "i686");
  assert.equal(await browserCpu({ userAgent: UA.linuxChrome, userAgentData: data({ architecture: "arm", bitness: "32" }) }), "armv7l");
  assert.deepEqual(asked, Array(4).fill([...CPU_HINTS]));
  assert.deepEqual([...CPU_HINTS], ["architecture", "bitness"]);
  // Refused, or empty: the user agent.
  assert.equal(await browserCpu({ userAgent: UA.linuxFirefoxArm, userAgentData: data(new Error("NotAllowedError")) }), "aarch64");
  assert.equal(await browserCpu({ userAgent: UA.linuxFirefox, userAgentData: data({}) }), "x86_64");
  // No userAgentData at all (Firefox, Safari).
  assert.equal(await browserCpu({ userAgent: UA.linuxFirefoxArm }), "aarch64");
  assert.equal(await browserCpu({ userAgent: UA.raspberry }), "armv7l");
  assert.equal(await browserCpu({ userAgent: "Mozilla/5.0 (X11; Linux)" }), null);
  assert.equal(await browserCpu(null), null);
  assert.equal(cpuOf({ architecture: "x86", bitness: "64" }, UA.linuxFirefoxArm), "x86_64", "the hints win over the user agent");
});

test("the Linux tab says the processor and links its tarball after the command, and says when the release has no build for it", () => {
  const supported = download("en", { os: "linux", hints: HINTS.linux, cpu: "aarch64" });
  assert.ok(text(supported).includes("This browser reports Linux on aarch64."));
  assert.doesNotMatch(text(supported), /builds are for/);
  const tarballs = (html: string) => [...html.matchAll(/data-testid="linux-tarballs-file"[^>]*>([^<]+)</g)].map((m) => m[1]);
  assert.deepEqual(tarballs(supported), ["droppen-v0.2.129-linux-aarch64.tar.gz"]);
  assert.match(supported, new RegExp(`<a href="${esc(`${DL}droppen-v0.2.129-linux-aarch64.tar.gz`)}" data-testid="linux-tarballs-file"`));
  assert.ok(supported.indexOf('data-testid="install-command"') < supported.indexOf('data-testid="linux-tarballs"'), "the command first");
  const other = download("zh-CN", { os: "linux", hints: HINTS.linux, cpu: "armv7l" });
  assert.ok(text(other).includes("Droppen 的 Linux 版本支持 x86_64 和 aarch64。"));
  assert.equal((text(other).match(/Droppen 的 Linux 版本支持/g) ?? []).length, 1, "said once");
  assert.deepEqual(tarballs(other), ["droppen-v0.2.129-linux-x86_64.tar.gz", "droppen-v0.2.129-linux-aarch64.tar.gz"]);
  const unknown = download("en", { os: "linux", hints: HINTS.linux });
  assert.doesNotMatch(unknown, /data-testid="linux-cpu"/, "nothing said when nothing is known");
  assert.equal(tarballs(unknown).length, 2, "both, when the processor is not known");
  assert.deepEqual(tarballs(download("en", { os: "linux", hints: HINTS.linux, cpu: "x86_64", release: null })), [], "none without the hub's answer");
});

test("the download page opens on the detected tab without ?os=; a device it cannot tell opens on the Mac App, marked as nothing", () => {
  assert.match(download("en", { hints: HINTS.linux }), /data-os="linux" data-detected="linux"/);
  assert.match(download("en", { hints: HINTS.windows }), /data-os="windows" data-detected="windows"/);
  assert.match(download("en", { hints: HINTS.phone }), /data-os="mobile" data-detected="mobile"/);
  assert.match(download("en", { hints: HINTS.linux, os: "macos-app" }), /data-os="macos-app" data-detected="linux"/, "the address wins");
  const unknown = download("en", { hints: HINTS.unknown });
  assert.match(unknown, /data-testid="environments" data-os="macos-app"/);
  assert.doesNotMatch(unknown, /data-detected=|data-testid="detected"/);
  assert.ok(text(unknown).includes("Pick the device your AI runs on: every way is one click away."));
  assert.ok(!text(unknown).includes("We picked the way for the device you are on"));
  // Windows: no app, droppen.app in this browser, and the Mac and Linux ways for a server.
  const windows = download("en", { hints: HINTS.windows });
  assert.match(windows, /data-testid="to-macos-app"/);
  assert.match(windows, /data-testid="to-linux"[^>]*>the Linux server options</);
  assert.equal(primaries(windows), 1);
});

test("the hero on a Mac: 「Download for Mac」 is the one primary action, the file itself; the release page and the other systems beside it", () => {
  const html = landing("en", { hints: HINTS.mac });
  const h = hero(html);
  assert.match(h, /data-offer="mac"/);
  assert.match(h, new RegExp(`<a href="${esc(`${DL}Droppen-0.2.129.dmg`)}" data-variant="primary" data-testid="hero-download"[^>]*>.*Download for Mac</a>`));
  assert.equal(primaries(html), 1, "one primary on the page");
  assert.ok(text(h).includes("v0.2.129 · 48 MB · macOS 13 or later · Apple silicon and Intel"));
  assert.match(h, new RegExp(`<a href="${esc(RELEASE.releasesPage)}" target="_blank" rel="noopener noreferrer" data-testid="hero-release-link"`));
  assert.match(h, /<a href="\/download" data-testid="hero-other-systems"/);
  // The account's ways in stay, as secondaries.
  assert.match(h, /<a href="\/signup" class="[^"]*">Get started<\/a>/);
  assert.match(h, /<a href="\/signin"/);
  assert.doesNotMatch(h, /data-testid="hero-install-command"/);
  // Signed in: the download, and back into the app.
  const signedIn = hero(landing("en", { hints: HINTS.mac, signedIn: true }));
  assert.match(signedIn, /data-variant="primary" data-testid="hero-download"/);
  assert.match(signedIn, /<a href="\/sessions" class="[^"]*">Open app<\/a>/);
  assert.doesNotMatch(signedIn, /href="\/sign(in|up)"/);
});

test("the hero on Linux: the install command first, its copy button the primary, then the tarball for this processor and the Linux tab", () => {
  const html = landing("en", { hints: HINTS.linux, cpu: "x86_64" });
  const h = hero(html);
  assert.match(h, /data-offer="linux"/);
  assert.match(h, new RegExp(`data-testid="hero-install-command"[^>]*>.*${esc(INSTALL_COMMAND)}</code>`));
  assert.match(h, /data-variant="primary" data-testid="hero-install-command-copy"/);
  assert.equal(primaries(html), 1);
  assert.match(h, /<a href="\/download\?os=linux" data-testid="hero-linux-tab"/);
  const tarballs = (html: string) => [...html.matchAll(/<a href="([^"]+)" data-testid="hero-tarballs-file"/g)].map((m) => m[1]);
  assert.deepEqual(tarballs(h), [`${DL}droppen-v0.2.129-linux-x86_64.tar.gz`]);
  assert.ok(text(h).includes("Or download it: droppen-v0.2.129-linux-x86_64.tar.gz (12 MB)"));
  assert.ok(h.indexOf('data-testid="hero-install-command"') < h.indexOf('data-testid="hero-tarballs"'), "the command first");
  assert.deepEqual(tarballs(hero(landing("en", { hints: HINTS.linux, cpu: "aarch64" }))), [`${DL}droppen-v0.2.129-linux-aarch64.tar.gz`]);
  assert.equal(tarballs(hero(landing("en", { hints: HINTS.linux, cpu: null }))).length, 2, "both, when the processor is not known");
  const pi = hero(landing("en", { hints: HINTS.linux, cpu: "armv7l" }));
  assert.ok(text(pi).includes("Droppen's Linux builds are for x86_64 and aarch64."));
  assert.equal(tarballs(pi).length, 2);
  assert.doesNotMatch(h, /Droppen-0\.2\.129\.dmg/, "no Mac file for a Linux device");
});

test("the hero on Windows and on a phone: droppen.app in the browser is the primary, and where the AI runs", () => {
  const windows = landing("en", { hints: HINTS.windows });
  const w = hero(windows);
  assert.match(w, /data-offer="windows"/);
  assert.ok(text(w).includes("No Windows app yet. Droppen works in this browser"));
  assert.match(w, /<a href="\/signup" data-variant="primary"[^>]*>Get started<\/a>/);
  assert.match(w, /<a href="\/download\?os=macos-app" data-testid="hero-mac-tab"/);
  assert.match(w, /<a href="\/download\?os=linux" data-testid="hero-linux-tab"/);
  assert.doesNotMatch(w, /Droppen-0\.2\.129\.dmg|github\.com/, "no Mac file for a Windows device");
  assert.equal(primaries(windows), 1);
  const phone = landing("en", { hints: HINTS.phone });
  const p = hero(phone);
  assert.match(p, /data-offer="phone"/);
  assert.ok(text(p).includes("On your phone, Droppen works in the browser"));
  assert.ok(text(p).includes("Your AI runs on a Mac or on Linux"));
  assert.match(p, /<a href="\/download\?os=mobile" data-testid="hero-mobile-tab"/);
  assert.doesNotMatch(p, /Droppen-0\.2\.129\.dmg|github\.com/);
  assert.equal(primaries(phone), 1);
  // Signed in, the way back into the app is the primary.
  assert.match(hero(landing("en", { hints: HINTS.phone, signedIn: true })), /<a href="\/sessions" data-variant="primary"/);
});

test("the hero on a device it cannot tell: every way, the Mac's the file itself — or its tab while the App is not out", () => {
  const html = landing("en", { hints: HINTS.unknown });
  const h = hero(html);
  assert.match(h, /data-offer="all"/);
  assert.match(h, /<a href="\/signup" data-variant="primary"/);
  assert.match(h, new RegExp(`<a href="${esc(`${DL}Droppen-0.2.129.dmg`)}" data-testid="hero-download"`));
  for (const os of ["linux", "windows", "mobile"]) assert.match(h, new RegExp(`<a href="/download\\?os=${os}"`), os);
  assert.equal(primaries(html), 1);
  const noApp = hero(landing("en", { hints: HINTS.mac, availability: { app: false, appSignIn: false, linux: true } }));
  assert.match(noApp, /data-offer="all"/);
  assert.doesNotMatch(noApp, /Droppen-0\.2\.129\.dmg|data-testid="hero-download"/, "never a link to a file that is not there");
  assert.match(noApp, /<a href="\/download\?os=macos-app" data-testid="hero-mac-tab"/);
});

test("the hero reads in en, zh-CN, zh-TW and ja on every device, and sells before it explains", () => {
  const said: Record<string, Record<string, string[]>> = {
    en: { mac: ["Download for Mac"], linux: ["Paste it into a terminal on this device", "Or download it:"], windows: ["No Windows app yet."], phone: ["On your phone, Droppen works in the browser"], unknown: ["Get Droppen:"] },
    "zh-CN": { mac: ["下载 Mac 版", "其他系统", "校验值和其他文件"], linux: ["在这台设备的终端里粘贴运行", "复制命令", "也可以直接下载："], windows: ["Windows 版 App 还没有。"], phone: ["在手机上，Droppen 直接在浏览器里用"], unknown: ["获取 Droppen：", "下载 Mac 版"] },
    "zh-TW": { mac: ["下載 Mac 版", "其他系統"], linux: ["在這台裝置的終端機裡貼上執行", "也可以直接下載："], windows: ["Windows 版 App 還沒有。"], phone: ["在手機上，Droppen 直接在瀏覽器裡用"], unknown: ["取得 Droppen："] },
    ja: { mac: ["Mac 版をダウンロード", "ほかの OS"], linux: ["このデバイスのターミナルに貼り付けて実行すると", "直接ダウンロードもできます："], windows: ["Windows 版アプリはまだありません。"], phone: ["スマホでは、Droppen はブラウザで使います。"], unknown: ["Droppen を入手："] },
  };
  for (const [lng, devices] of Object.entries(said)) {
    for (const [device, lines] of Object.entries(devices)) {
      const words = text(hero(landing(lng, { hints: HINTS[device as keyof typeof HINTS] })));
      for (const line of lines) assert.ok(words.includes(line), `${lng} ${device}: ${line}`);
      if (lng !== "en") assert.doesNotMatch(words, /Download for Mac|No Windows app yet|Paste it into|Get Droppen:|other systems|Or download it/, `${lng} ${device}`);
      assert.doesNotMatch(words, /Hub|relay|中继|中繼|中継|NAT|STUN|p50|ciphertext|密文|暗号文/, `${lng} ${device}`);
    }
  }
});
