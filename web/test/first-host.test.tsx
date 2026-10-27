// 「我的设备」's first-run card (`features/cloud/first-host.tsx`), while the
// account has no device that runs AI: what to do next, for the device the
// page is open on.
//
// What these pin: a Mac gets the App's .dmg itself (the release page until
// the hub names it) and three steps ending in 「登录 Droppen」 in the App;
// Linux the install command and its page's 「登录 Droppen」, with the one-time
// code for a device with no browser; Windows says plainly there is no app yet
// and what works; a phone gets the link to open on the device the AI runs on,
// shared or copied. Every way says first that the device needs Claude Code or
// Codex, installed and signed in; each has at most one primary action; another
// kind of device is a click away; and it reads in en, zh-CN, zh-TW and ja.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { I18nextProvider } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { FirstHostCard, firstHostWay, setupLink } from "../src/features/cloud/first-host.js";
import { type EnvironmentHints } from "../src/features/landing/detect.js";
import { type Availability, AVAILABILITY, INSTALL_COMMAND, RELEASES_URL } from "../src/features/landing/get-droppen.js";
import { type LatestRelease, parseLatestRelease } from "../src/lib/hub/releases.js";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "../src/i18n/cloud-devices.js";
import { linkJa, linkZhCN, linkZhTW } from "../src/i18n/link.js";
import { onboardingJa, onboardingZhCN, onboardingZhTW } from "../src/i18n/onboarding.js";
import { ja } from "../src/i18n/ja.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";
import { makeI18n } from "./render.js";

const i18n = makeI18n({
  lng: "en",
  resources: {
    en: { translation: {} },
    "zh-CN": { translation: { ...onboardingZhCN, ...linkZhCN, ...cloudDevicesZhCN, ...zhCN } },
    "zh-TW": { translation: { ...onboardingZhTW, ...linkZhTW, ...cloudDevicesZhTW, ...zhTW } },
    ja: { translation: { ...onboardingJa, ...linkJa, ...cloudDevicesJa, ...ja } }
  },
  keySeparator: false,
  nsSeparator: false
});

const UA = {
  macChrome: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36",
  macSafari: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15",
  linuxFirefox: "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0",
  chromebook: "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36",
  windowsEdge: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36 Edg/129.0.0.0",
  iphone: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
  android: "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Mobile Safari/537.36",
};

const HINTS = {
  mac: { userAgent: UA.macChrome, platform: "macOS", mobile: false, maxTouchPoints: 0 },
  linux: { userAgent: UA.linuxFirefox, maxTouchPoints: 0 },
  windows: { userAgent: UA.windowsEdge, platform: "Windows", mobile: false, maxTouchPoints: 0 },
  iphone: { userAgent: UA.iphone, maxTouchPoints: 5 },
  // iPadOS asks for the desktop site and says it is a Mac, with touch.
  ipad: { userAgent: UA.macSafari, maxTouchPoints: 5 },
  android: { userAgent: UA.android, platform: "Android", mobile: true, maxTouchPoints: 5 },
  chromebook: { userAgent: UA.chromebook, platform: "Chrome OS", mobile: false },
  unknown: { userAgent: "" },
} satisfies Record<string, EnvironmentHints>;

/** The hub's answer for the latest release (docs/hub-api/http.md), as GitHub names its files. */
const DL = "https://github.com/getdroppen/droppen/releases/download/v0.2.131/";
const RELEASE = parseLatestRelease({
  version: "0.2.131",
  tag: "v0.2.131",
  published_at: "2026-10-24T09:30:00Z",
  releases_page: "https://github.com/getdroppen/droppen/releases/tag/v0.2.131",
  platforms: { macos: { name: "Droppen-0.2.131.dmg", url: `${DL}Droppen-0.2.131.dmg`, size: 48_000_000 } },
  fetched_ms: 1_793_000_000_000,
})!;

const text = (html: string) => html.replace(/<[^>]+>/g, " ").replace(/&#x27;/g, "'").replace(/&quot;/g, '"').replace(/&amp;/g, "&").replace(/\s+/g, " ");
const primaries = (html: string) => (html.match(/data-variant="primary"/g) ?? []).length;
const esc = (s: string) => s.replace(/[.?*+^$()[\]{}|/\\]/g, "\\$&");

type CardProps = React.ComponentProps<typeof FirstHostCard>;

function card(lng: string, hints: EnvironmentHints, props: Partial<CardProps> = {}): string {
  void i18n.changeLanguage(lng);
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <FirstHostCard onGuide={() => undefined} onCode={() => undefined} pairWith={() => <i>pairing</i>} hints={hints} release={RELEASE} share={false} {...props} />
    </I18nextProvider>,
  );
}

type El = React.ReactElement<Record<string, unknown>>;
function elements(node: unknown, out: El[] = []): El[] {
  if (Array.isArray(node)) for (const n of node) elements(n, out);
  else if (node && typeof node === "object" && "props" in node) {
    out.push(node as El);
    elements((node as El).props.children, out);
  }
  return out;
}

/** A click on the element with test id `id` in the card's own tree. */
const press = (id: string) => (tree: El[]) => {
  const el = tree.find((e) => e.props["data-testid"] === id);
  assert.ok(el, `${id} is there to click`);
  (el.props.onClick as () => void)();
};

/** A way's own part (a component of its own, not drawn in the card's tree) asked for `name`, as its button would. */
const ask = (name: string, ...args: unknown[]) => (tree: El[]) => {
  const el = tree.find((e) => typeof e.props[name] === "function");
  assert.ok(el, `a part that takes ${name}`);
  (el.props[name] as (...a: unknown[]) => void)(...args);
};

/**
 * The card rendered inline in a probe, its returned tree walked, and at render
 * `i` the step `steps[i]` (a click) made as the page would: a click that
 * moves its state renders it again at once.
 */
function click(hints: EnvironmentHints, steps: ((tree: El[]) => void)[], props: Partial<CardProps> = {}): string {
  let i = 0;
  function Probe() {
    const out = FirstHostCard({ onGuide: () => undefined, hints, release: RELEASE, share: false, ...props } as CardProps);
    if (i < steps.length) steps[i++]!(elements(out));
    return out;
  }
  void i18n.changeLanguage("en");
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <Probe />
    </I18nextProvider>,
  );
}

test("the way for each kind of device: a Mac's, Linux's (a Chromebook's too), Windows', a phone's or tablet's — and the Mac's for one it cannot tell", () => {
  const table: [keyof typeof HINTS, Parameters<typeof firstHostWay>[0], string][] = [
    ["mac", "mac", "mac"],
    ["linux", "linux", "linux"],
    ["chromebook", "chromeos", "linux"],
    ["windows", "windows", "windows"],
    ["iphone", "iphone", "phone"],
    ["ipad", "ipad", "phone"],
    ["android", "android", "phone"],
    ["unknown", "unknown", "mac"],
  ];
  for (const [device, kind, way] of table) {
    const html = card("en", HINTS[device]);
    assert.match(html, new RegExp(`data-testid="first-host" data-way="${way}" data-detected="${way}"`), device);
    assert.equal(firstHostWay(kind), way, device);
  }
});

test("every way says first what the device needs: Claude Code or Codex, installed and signed in", () => {
  for (const hints of [HINTS.mac, HINTS.linux, HINTS.windows, HINTS.iphone]) {
    const html = card("en", hints);
    assert.match(html, /data-testid="first-host-needs"/);
    const words = text(html);
    assert.ok(words.includes("You need: Claude Code or Codex, installed and signed in on that device."), words);
    assert.ok(words.indexOf("You need:") < words.indexOf("Another kind of device?"));
    assert.ok(words.includes("Connect the device your AI runs on"));
  }
});

test("on a Mac: the App's .dmg itself, the one primary action, then install it, open it, 「登录 Droppen」 in the App", () => {
  const html = card("en", HINTS.mac, { pairWith: () => <i>pairing</i> });
  const words = text(html);
  assert.match(html, /data-testid="first-host-mac" data-app="true"/);
  assert.match(html, new RegExp(`<a href="${esc(`${DL}Droppen-0.2.131.dmg`)}" data-variant="primary" data-testid="first-host-download"[^>]*>.*Download Droppen for Mac</a>`));
  assert.equal(primaries(html), 1, "one primary: the download");
  assert.ok(words.includes("v0.2.131 · 48 MB · macOS 13 or later"));
  const steps = html.match(/data-testid="first-host-steps"[^]*?<\/ol>/)?.[0] ?? "";
  assert.equal((steps.match(/<li/g) ?? []).length, 3);
  for (const line of [
    "Install it",
    "Open the .dmg and drag Droppen to Applications.",
    "Open Droppen",
    "It opens a short setup guide, four steps.",
    "Sign in to Droppen in the App",
    // Where the App's guide has it: its step 02, the card the App names so — and where it is after the guide.
    "In the guide's step 02 (Where to use it), choose “On my phone and other devices too”, then Sign in to Droppen (later: Settings → Access → Droppen). A droppen.app window opens: sign in, then click add this device.",
    "Keep this page open: it notices the new device by itself.",
    "Is that window's browser new to your account?",
  ])
    assert.ok(words.includes(line), `missing: ${line}`);
  assert.doesNotMatch(html, /first-host-command|first-host-link/, "no command, no link to send");
  // The other kinds of device, and the full guide.
  for (const way of ["linux", "windows", "phone"]) assert.match(html, new RegExp(`data-testid="first-host-way-${way}"`));
  assert.doesNotMatch(html, /data-testid="first-host-way-mac"/);
  assert.match(html, /data-testid="first-host-guide"/);
  // Until the hub names the file (or when it cannot): the release page, in a new tab.
  for (const release of [null, undefined]) {
    const page = card("en", HINTS.mac, { release });
    assert.match(page, new RegExp(`<a href="${esc(RELEASES_URL)}" target="_blank" rel="noopener noreferrer" data-variant="primary" data-testid="first-host-download"`));
    assert.ok(text(page).includes("macOS 13 or later, Apple silicon or Intel"));
  }
  // Off (`APP_SIGN_IN_AVAILABLE`, a preview): the background install, which signs in from its page — never a button the App does not have.
  const off = card("en", HINTS.mac, { availability: { ...AVAILABILITY, appSignIn: false } as Availability });
  assert.match(off, /data-testid="first-host-mac" data-app="false"/);
  assert.doesNotMatch(off, /first-host-download|Sign in to Droppen in the App/);
  assert.ok(text(off).includes(INSTALL_COMMAND));
  assert.ok(text(off).includes("Open Terminal on that Mac and run:"));
  assert.equal(primaries(off), 1, "the command's copy");
});

test("on Linux: the install command, its copy the one primary, then its page's 「登录 Droppen」 — and a one-time code for a device with no browser", () => {
  let coded = 0;
  const html = card("en", HINTS.linux, { onCode: () => coded++ });
  const words = text(html);
  assert.match(html, /data-testid="first-host-linux"/);
  assert.match(html, new RegExp(`data-testid="first-host-command"[^>]*>${esc(INSTALL_COMMAND)}</code>`));
  assert.match(html, /data-variant="primary" data-testid="first-host-command-copy"[^>]*>.*copy command</);
  assert.equal(primaries(html), 1);
  for (const line of [
    "Run this in a terminal on that device:",
    "Run the command",
    "It installs and starts Droppen, and makes a local login for it.",
    "Sign in on its page",
    "On the page it opens, sign in with that login, then click Sign in to Droppen.",
    "Add it",
    "No browser on that device, like a server?",
    "use a one-time code",
  ])
    assert.ok(words.includes(line), `missing: ${line}`);
  assert.doesNotMatch(html, /\.dmg|first-host-download/, "no Mac file for a Linux device");
  // The code's link opens the guide on its one-time code.
  click(HINTS.linux, [ask("onCode")], { onCode: () => coded++ });
  assert.equal(coded, 1);
  // With no way to the code (none given), no link to it.
  assert.doesNotMatch(card("en", HINTS.linux, { onCode: undefined }), /first-host-code/);
  // A Chromebook: the same, once its Linux is turned on — said first; on Linux itself, nothing of the kind.
  const chromebook = card("en", HINTS.chromebook);
  assert.match(chromebook, /data-testid="first-host-chromebook"[^>]*>On a Chromebook, turn on Linux first: in Settings, search for “Linux development environment”\.</);
  assert.ok(chromebook.indexOf("first-host-chromebook") < chromebook.indexOf("first-host-command"));
  assert.doesNotMatch(html, /first-host-chromebook/);
  assert.doesNotMatch(click(HINTS.mac, [press("first-host-way-linux")]), /first-host-chromebook/, "a Mac choosing Linux is no Chromebook");
  assert.ok(text(card("zh-CN", HINTS.chromebook)).includes("Chromebook 上要先开启 Linux：在「设置」里搜索「Linux 开发环境」。"));
});

test("on Windows: no app yet, said plainly, and what works — droppen.app in this browser once a Mac or Linux device (WSL too) is connected", () => {
  const html = card("en", HINTS.windows);
  const words = text(html);
  assert.match(html, /data-testid="first-host-windows"/);
  assert.ok(words.includes("There is no Windows app yet."));
  // WSL only with systemd on: the installer runs Droppen as a systemd user service, and stops where there is none.
  assert.ok(words.includes("Your AI needs a Mac or a Linux device with Droppen on it (WSL works too, with systemd turned on). Once one is connected, use droppen.app right here in this browser to watch and answer it."));
  assert.match(html, /<a href="https:\/\/learn\.microsoft\.com\/windows\/wsl\/systemd" target="_blank" rel="noopener noreferrer" data-testid="first-host-wsl-systemd"[^>]*>how to turn on systemd in WSL<\/a>/);
  assert.equal(primaries(html), 0, "nothing to download here");
  assert.doesNotMatch(html, /first-host-download|first-host-command|\.dmg|first-host-waiting/, "no file, no command, nothing to wait for");
  // Its two ways on: the Mac's and Linux's.
  assert.match(click(HINTS.windows, [ask("onWay", "mac")]), /data-testid="first-host" data-way="mac" data-detected="windows"/);
  assert.match(click(HINTS.windows, [ask("onWay", "linux")]), /data-testid="first-host" data-way="linux" data-detected="windows"/);
  assert.match(html, /data-testid="first-host-to-mac"[^>]*>set up a Mac</);
  assert.match(html, /data-testid="first-host-to-linux"[^>]*>set up Linux or WSL</);
});

test("on a phone or tablet: set it up on the device the AI runs on — the link to open there, shared or copied", () => {
  for (const hints of [HINTS.iphone, HINTS.android, HINTS.ipad]) {
    const html = card("en", hints, { share: true });
    const words = text(html);
    assert.match(html, /data-testid="first-host-phone"/);
    assert.ok(words.includes("Set it up on the device your AI runs on."));
    // The same words for a phone and a tablet: "this one", "from here".
    assert.ok(words.includes("Droppen goes on the Mac or Linux device where your AI runs, not on this one. Open this link there:"));
    assert.ok(words.includes("Once it is connected, you can watch and answer your AI from here too."));
    assert.doesNotMatch(words, /this phone|this tablet/);
    assert.match(html, /data-testid="first-host-link"[^>]*>https:\/\/www\.droppen\.app\/download</, "the site host: the device opens the download page there");
    // The share sheet where there is one, the primary; the copy beside it.
    assert.match(html, /data-variant="primary" data-testid="first-host-share"[^>]*>.*share link</);
    assert.match(html, /<button type="button" data-testid="first-host-copy-link"[^>]*>.*copy link</);
    assert.equal(primaries(html), 1);
    assert.ok(words.includes("Droppen there opens a sign-in window. If it shows a code first, add it from here:"), "the device's sign-in window may show a code to add from here");
    assert.ok(!words.includes("Is that window's browser new"), "no window this page did not name");
    assert.doesNotMatch(html, /first-host-download|first-host-command/);
  }
  // No share sheet: the copy is the primary.
  const noShare = card("en", HINTS.iphone, { share: false });
  assert.doesNotMatch(noShare, /first-host-share/);
  assert.match(noShare, /data-variant="primary" data-testid="first-host-copy-link"/);
  assert.equal(primaries(noShare), 1);
  // The link: the download page, absolute — it opens on that device's way.
  assert.equal(setupLink("https://my.droppen.app"), "https://my.droppen.app/download");
  assert.equal(setupLink("https://my.droppen.app/"), "https://my.droppen.app/download");
});

test("another kind of device is a click away, and the full guide behind it", () => {
  assert.match(click(HINTS.mac, [press("first-host-way-linux")]), /data-testid="first-host" data-way="linux" data-detected="mac"/);
  assert.match(click(HINTS.mac, [press("first-host-way-phone")]), /data-testid="first-host" data-way="phone" data-detected="mac"/);
  assert.match(click(HINTS.iphone, [press("first-host-way-mac")]), /data-testid="first-host" data-way="mac" data-detected="phone"/);
  let guided = 0;
  click(HINTS.mac, [press("first-host-guide")], { onGuide: () => guided++ });
  assert.equal(guided, 1);
  // The pairing for a browser new to the account opens in place.
  assert.match(click(HINTS.mac, [press("first-host-pair")], { pairWith: () => <i data-testid="pairing-here" /> }), /data-testid="pairing-here"/);
});

test("the card reads in zh-CN, zh-TW and ja, on every kind of device, and says 设备 / 裝置 / デバイス", () => {
  const said: Record<string, Record<string, string[]>> = {
    "zh-CN": {
      mac: [
        "连接跑 AI 的那台设备",
        "那台设备上已安装并登录 Claude Code 或 Codex。",
        "下载 Mac 版 Droppen",
        "在 App 里点「登录 Droppen」",
        "它会打开一个四步的设置向导。",
        // The App's own words for its step 02 and its card (i18n/welcome.ts).
        "在向导第 02 步「在哪里用」选「在手机和其它设备上也能用」，再点「登录 Droppen」",
        "会弹出 droppen.app 窗口：登录后点「加入这台设备」。",
      ],
      linux: ["在那台设备的终端里运行：", "复制命令", "再点「登录 Droppen」", "那台设备没有浏览器（比如服务器）？", "用一次性注册码"],
      windows: ["暂时还没有 Windows 版 App。", "开启了 systemd 的 WSL 也可以", "如何在 WSL 里开启 systemd", "在 Mac 上安装", "在 Linux 或 WSL 上安装"],
      phone: ["装在跑 AI 的那台设备上。", "不是这台。", "在这里也能看 AI 干活", "复制链接", "那台设备上的 Droppen 会弹出登录窗口"],
    },
    "zh-TW": {
      mac: ["連接執行 AI 的那台裝置", "那台裝置上已安裝並登入 Claude Code 或 Codex。", "下載 Mac 版 Droppen", "在 App 裡點「登入 Droppen」"],
      linux: ["在那台裝置的終端機裡執行：", "複製指令", "用一次性註冊碼"],
      windows: ["暫時還沒有 Windows 版 App。", "開啟了 systemd 的 WSL 也可以"],
      phone: ["裝在執行 AI 的那台裝置上。", "複製連結"],
    },
    ja: {
      mac: ["AI を動かすデバイスをつなぐ", "そのデバイスに Claude Code か Codex がインストール済みで、サインインしていること。", "Mac 版 Droppen をダウンロード", "アプリで「Droppen にサインイン」", "ガイドのステップ 02「使う場所」で「スマートフォンやほかのデバイスでも使う」を選び", "ここやスマートフォンから"],
      linux: ["そのデバイスのターミナルで実行します：", "コマンドをコピー", "ワンタイムコードを使う"],
      windows: ["Windows 版アプリはまだありません。", "systemd を有効にした WSL でも可"],
      phone: ["AI を動かすデバイスに入れましょう。", "リンクをコピー"],
    },
  };
  const hintsOf = { mac: HINTS.mac, linux: HINTS.linux, windows: HINTS.windows, phone: HINTS.iphone };
  for (const [lng, ways] of Object.entries(said)) {
    for (const [way, lines] of Object.entries(ways)) {
      const words = text(card(lng, hintsOf[way as keyof typeof hintsOf]));
      for (const line of lines) assert.ok(words.includes(line), `${lng} ${way}: ${line}`);
      assert.doesNotMatch(words, /Connect the device|You need:|Download Droppen for Mac|There is no Windows app|Open this link there|Another kind of device/, `${lng} ${way}: English left`);
      assert.doesNotMatch(words, /机器|电脑|本机|機器|電腦|本機|マシン|コンピュータ|端末/, `${lng} ${way}`);
      // Japanese says スマートフォン, as the app does.
      assert.doesNotMatch(words, /スマホ/, `${lng} ${way}`);
    }
  }
  // English never says "computer" or "machine" on screen either (test/ui-wording.test.ts checks the source too).
  for (const hints of Object.values(hintsOf)) assert.doesNotMatch(text(card("en", hints)), /\bcomputers?\b|\bmachines?\b|\bPCs?\b/i);
});
