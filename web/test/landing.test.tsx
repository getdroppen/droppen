// The landing page (`features/landing`): what a signed-out visitor sees at `/`
// in cloud mode, and the paths that move sign-in and sign-up off the root.
//
// What these pin: the page says what it is, in zh-CN first and in en, zh-TW
// and ja, with every string on it translated and in its own words (never an
// app translation of the same key); it shows the product first (the tour of
// screenshots) and says end-to-end encryption once, near the end; it calls
// only the client open source; it carries exactly one primary action, and
// its links go to /signup and /signin; its numbers are the measured ones and
// it makes no absolute claims; only the bare root is the landing page; and
// the diagrams' moving parts keep their promises (a sealed line keeps its
// width, the direct path's demo delivers every key in order).
//
// `react-dom/server` with no DOM, like the other render tests.
import * as React from "react";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import test from "node:test";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { landingBundles } from "../src/features/landing/copy.js";
import { MEASURED, schedule } from "../src/features/landing/direct-path.js";
import { TOUR_SHOTS, TOUR_TABS, tourLanguage, tourShot } from "../src/features/landing/product-tour.js";
import { DIRECT_HUB, directLayout, directWires } from "../src/features/landing/direct-path.js";
import { offlineLayout, offlineWires } from "../src/features/landing/offline-copy.js";
import LandingPage from "../src/features/landing/landing.js";
import { PLAN_NUMBERS } from "../src/features/landing/pricing-plans.js";
import { cipherOf, corners, pointAt, type Pt, seal, wide } from "../src/features/landing/motion.js";
import { APP_PATH, bootRoute, isAuthPath, isLandingPath, publicPage } from "../src/features/landing/route.js";
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
    // As the page adds them: the app's wording wins where both have a key.
    "zh-CN": { translation: { ...landingBundles["zh-CN"], ...app["zh-CN"] } },
    "zh-TW": { translation: { ...landingBundles["zh-TW"], ...app["zh-TW"] } },
    ja: { translation: { ...landingBundles.ja, ...app.ja } },
  },
  interpolation: { escapeValue: false },
  keySeparator: false,
  nsSeparator: false,
});

function draw(lng: string, { signedIn = false, signup }: { signedIn?: boolean; signup?: SignupMode } = {}): string {
  void i18n.changeLanguage(lng);
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <LandingPage signedIn={signedIn} signup={signup} />
    </I18nextProvider>,
  );
}

const text = (html: string) => html.replace(/<[^>]+>/g, " ").replace(/\s+/g, " ");

test("the landing page leads with what you get, in zh-CN first", () => {
  const html = draw("zh-CN");
  const words = text(html);
  for (const line of [
    // The selling points: your computer's AI on your phone, every session in one list, code only you can read.
    "电脑上的 AI，手机上接着用。",
    "每个会话，实时在眼前",
    "三步装好，走到哪都能接手。",
    "本来要走回工位才能做的事，手机上都能做。",
    "所有会话，一个列表",
    "你的代码，只有你看得到。",
    // The deep sections, each headed by what it gets you.
    "在手机上打字，照样跟手。",
    "电脑合上了，会话照样能看。",
    "免费开始",
    "登录",
    "公开测试版",
  ]) {
    assert.ok(words.includes(line), `missing: ${line}`);
  }
  // The sections the header links to, in the page's order: the product first, encryption last.
  const at = (id: string) => html.indexOf(`id="${id}"`);
  const order = ["tour", "how", "features", "direct", "offline", "security"];
  for (const id of order) assert.ok(at(id) > 0, id);
  assert.deepEqual([...order].sort((a, b) => at(a) - at(b)), order);
  for (const id of order) assert.match(html, new RegExp(`<a href="#${id}"|id="${id}"`));
});

test("privacy is said once: not in the header, not in the hero's numbers, one section near the end and the rest on /security", () => {
  for (const lng of ["en", "zh-CN", "zh-TW", "ja"]) {
    const html = draw(lng);
    const header = html.slice(0, html.indexOf("<main"));
    assert.doesNotMatch(header, /href="#journey"|href="#privacy"/, lng);
    // The hero, up to the tour: it sells, and encryption is one word in its proof line.
    const hero = text(html.slice(html.indexOf("<main"), html.indexOf('id="tour"')));
    assert.doesNotMatch(hero, /recovery words|keys on our servers|恢复词|復原詞|リカバリーワード|密文|ciphertext|暗号文/, lng);
    // The one section on it comes after everything the product does, and links to the whole account of it.
    assert.ok(html.indexOf('id="security"') > html.indexOf('id="offline"'), lng);
    assert.match(html, /<a href="\/security"[^>]*data-testid="landing-security"/, lng);
  }
  // No line-by-line ledger of what we can and cannot see: that is /security's.
  assert.doesNotMatch(text(draw("en")), /What we can see|What we cannot see|Who holds the keys/);
});

test("one primary action, and the calls to action go to sign-up and sign-in", () => {
  const html = draw("en");
  const primaries = html.match(/data-variant="primary"/g) ?? [];
  assert.equal(primaries.length, 1, "one primary per page (docs/ui-spec.md V10)");
  assert.match(html, /<a href="\/signup"[^>]*data-variant="primary"[^>]*>Start for free<\/a>/);
  // Signing in is the header's and the end's; the hero does not offer it a second time.
  assert.equal((html.match(/<a href="\/signin"/g) ?? []).length, 2, "sign-in in the header and at the end");
  // The page ends on the same call to action, as a secondary.
  assert.equal((html.match(/<a href="\/signup"[^>]*>Start for free<\/a>/g) ?? []).length, 2, "start for free in the hero and at the end");
});

test("signed in, the landing page is still the landing page, with a way back into the app instead of sign-in and sign-up", () => {
  const app = (label: string, primary: boolean) =>
    new RegExp(`<a href="${APP_PATH}"[^>]*${primary ? 'data-variant="primary"[^>]*' : ""}>${label}</a>`);
  for (const [lng, open, how] of [
    ["zh-CN", "打开应用", "工作原理"],
    ["en", "Open app", "How it works"],
    ["zh-TW", "打開應用", "運作方式"],
    ["ja", "アプリを開く", "しくみ"],
  ] as const) {
    const html = draw(lng, { signedIn: true });
    // The header's way in and the hero's primary both open the app.
    assert.equal((html.match(new RegExp(`<a href="${APP_PATH}"`, "g")) ?? []).length, 2, lng);
    assert.match(html, app(open, true), `${lng}: the hero's primary opens the app`);
    assert.equal((html.match(/data-variant="primary"/g) ?? []).length, 1, `${lng}: one primary`);
    assert.match(html, new RegExp(`<a href="#how"[^>]*>${how}</a>`), `${lng}: the secondary reads on through the page`);
    // Nothing asks someone signed in to sign in or sign up.
    assert.doesNotMatch(html, /href="\/sign(in|up)"/, lng);
    // The page itself is all there.
    for (const id of ["tour", "how", "features", "direct", "offline", "security"]) assert.match(html, new RegExp(`id="${id}"`), lng);
  }
  const words = text(draw("zh-CN", { signedIn: true }));
  for (const gone of ["台电脑以内免费。", "免费开始", "AI 接着干活，你在哪都能接手。"]) assert.ok(!words.includes(gone), `still there: ${gone}`);
  assert.ok(!text(draw("zh-CN", { signedIn: true, signup: "invite" })).includes("注册需要邀请码。"), "by invite too");
  assert.ok(text(draw("zh-CN")).includes("台电脑以内免费。"), "signed out keeps the sign-up prompt");
});

test("routing: `/` is the landing page signed in or not; the app keeps its own routes and starts at its start", () => {
  // `/` never goes on to the app by itself.
  assert.deepEqual(bootRoute("/", true), { show: "landing" });
  assert.deepEqual(bootRoute("/", false), { show: "landing" });
  assert.deepEqual(bootRoute("", true), { show: "landing" });
  // Signed in: deep links into the app open where they point; the sign-in pages give way to the app's start.
  assert.deepEqual(bootRoute("/sessions", true), { show: "app" });
  assert.deepEqual(bootRoute("/sessions/claude/abc", true), { show: "app" });
  assert.deepEqual(bootRoute("/terminals", true), { show: "app" });
  assert.deepEqual(bootRoute("/signin", true), { show: "app", replace: APP_PATH });
  assert.deepEqual(bootRoute("/signup/", true), { show: "app", replace: APP_PATH });
  // The verification mail's link opens its own page, signed in or not.
  assert.deepEqual(bootRoute("/verify", true), { show: "verify" });
  assert.deepEqual(bootRoute("/verify", false), { show: "verify" });
  assert.equal(publicPage("/verify/"), "verify");
  assert.ok(!isAuthPath("/verify"));
  // Signed out: the forms, in place on a deep link.
  assert.deepEqual(bootRoute("/signin", false), { show: "auth" });
  assert.deepEqual(bootRoute("/sessions/claude/abc", false), { show: "auth" });
  assert.ok(isLandingPath("/") && !isLandingPath(APP_PATH) && !isLandingPath("/signin"));
  // The app's start is a route the app knows, not the landing page or a sign-in page.
  assert.equal(APP_PATH, "/sessions");
  assert.equal(bootRoute(APP_PATH, true).show, "app");
  // The installed app starts in the app, never on the landing page.
  const manifest = JSON.parse(readFileSync(new URL("../public/manifest.webmanifest", import.meta.url), "utf8")) as { start_url: string };
  assert.equal(manifest.start_url, APP_PATH);
  assert.equal(bootRoute(manifest.start_url, true).show, "app");
  // Sign-out still lands on the sign-in page.
  const signOut = readFileSync(new URL("../src/features/cloud/sign-out.ts", import.meta.url), "utf8");
  assert.match(signOut, /location\.replace\(SIGN_IN_PATH\)/);
});

test("the numbers are the measured ones, and nothing claims more than the build does", () => {
  const words = text(draw("en"));
  for (const n of ["63–73 ms", "215–227 ms", "1.5 s", "two in three", "30 days"]) assert.ok(words.includes(n), `missing: ${n}`);
  assert.match(words, /internal security reviews, not an external audit/);
  assert.match(words, /Droppen is in beta\./);
  for (const claim of [/unhackable/i, /zero risk/i, /\baudited\b/i, /military[- ]grade/i, /100% secure/i]) {
    assert.doesNotMatch(words, claim);
  }
  // The free plan's computers, as the pricing page has them.
  assert.ok(words.includes(`free for ${PLAN_NUMBERS.free.hosts} computers`));
  assert.deepEqual(MEASURED, { same: { direct: [1, 2], hub: [170, 215] }, phone: { direct: [63, 73], hub: [215, 227] } });
});

test("only the client is called open source: the hub never is, and nothing says all of Droppen is", () => {
  const en = text(draw("en"));
  assert.match(en, /open-source client/);
  assert.match(en, /Droppen Hub is not open source/);
  // Every mention is the client's, or says the hub is not.
  for (const mention of en.match(/[^.:;]*open[- ]source[^.:;]*/gi) ?? []) {
    assert.match(mention, /client|program on your computers|Droppen Hub is not/i, mention);
  }
  assert.doesNotMatch(en, /Droppen is open[- ]source|open[- ]source (service|platform)/i);
  for (const [lng, client, all] of [
    ["zh-CN", "客户端开源", /Droppen 开源|全部开源|完全开源/],
    ["zh-TW", "客戶端開源", /Droppen 開源|全部開源|完全開源/],
    ["ja", "クライアントはオープンソース", /Droppen はオープンソース|すべてオープンソース/],
  ] as const) {
    const words = text(draw(lng));
    assert.ok(words.includes(client), lng);
    assert.doesNotMatch(words, all, lng);
  }
  // The link goes to the source.
  assert.match(draw("en"), /<a href="https:\/\/github\.com\/getdroppen\/droppen"[^>]*data-testid="hero-source"/);
});

test("open sign-up by default: no invite wording; the invite wording only where the hub says sign-up is by invite, and \"closed for now\" where it is closed", () => {
  // Open (droppen.app since 2026-10-23), and what the page draws before the hub has answered.
  for (const signup of [undefined, "open"] as const) {
    const html = draw("en", { signup });
    const words = text(html);
    assert.doesNotMatch(words, /invite/i, `${signup}: no invite`);
    assert.ok(words.includes(`Public beta: free for ${PLAN_NUMBERS.free.hosts} computers.`));
    assert.match(html, /<a href="\/pricing"[^>]*data-testid="hero-pricing"[^>]*>See the plans<\/a>/);
    assert.doesNotMatch(html, /data-testid="hero-waitlist"/);
    assert.ok(words.includes("Public beta"));
    assert.ok(words.includes("Sign up with your e-mail, connect your computer, and pick up from your phone."));
    assert.match(html, /<a href="\/signup"[^>]*>Start for free<\/a>/);
  }
  for (const [lng, invite] of [["zh-CN", /邀请/], ["zh-TW", /邀請/], ["ja", /招待/]] as const) {
    assert.doesNotMatch(text(draw(lng)), invite, `${lng}: no invite`);
  }
  assert.ok(text(draw("zh-CN")).includes(`公开测试版，${PLAN_NUMBERS.free.hosts} 台电脑以内免费。`));
  // By invite: as before — the code, and the waitlist for those without one.
  const invited = draw("en", { signup: "invite" });
  assert.ok(text(invited).includes("Invite-only for now: sign-up needs an invite code."));
  assert.match(invited, /<a href="\/pricing#waitlist"[^>]*data-testid="hero-waitlist"[^>]*>No code yet\? Join the waitlist<\/a>/);
  assert.doesNotMatch(invited, /data-testid="hero-pricing"/);
  assert.ok(text(invited).includes("Invite-only beta"));
  assert.ok(text(invited).includes("Sign up with your invite code, connect your computer, and pick up from your phone."));
  assert.ok(text(draw("zh-CN", { signup: "invite" })).includes("邀请制测试版"));
  // Closed: closed for now, and the waitlist; no invite code and nothing to sign up with.
  const closed = draw("en", { signup: "closed" });
  assert.ok(text(closed).includes("Sign-up is closed for now."));
  assert.match(closed, /<a href="\/pricing#waitlist"[^>]*data-testid="hero-waitlist"[^>]*>Join the waitlist<\/a>/);
  assert.doesNotMatch(text(closed), /invite|Public beta|Sign up with your/i);
  assert.ok(text(closed).includes("Beta"));
  // The ways in: the waitlist where "Get started" was (the hero's and the footer's), and sign-in.
  assert.doesNotMatch(closed, /href="\/signup"|Start for free/);
  assert.equal((closed.match(/<a href="\/pricing#waitlist"[^>]*>Join the waitlist<\/a>/g) ?? []).length, 3, "the hero's line, its account action and the footer's");
  assert.match(closed, /<a href="\/signin"[^>]*>Sign in<\/a>/);
  for (const signup of ["open", "invite"] as const) assert.match(draw("en", { signup }), /<a href="\/signup"[^>]*>Start for free<\/a>/, signup);
  for (const [lng, line] of [["zh-CN", "目前暂停注册。"], ["zh-TW", "目前暫停註冊。"], ["ja", "現在、新規登録を停止しています。"]] as const) {
    const words = text(draw(lng, { signup: "closed" }));
    assert.ok(words.includes(line), `${lng}: ${line}`);
    assert.doesNotMatch(words, /邀请|邀請|招待/, `${lng}: no invite`);
  }
  // Signed in: no invitation to sign up whatever the mode.
  for (const signup of ["open", "invite", "closed"] as const) {
    assert.doesNotMatch(draw("en", { signedIn: true, signup }), /hero-(waitlist|pricing)|Public beta|Invite-only|closed for now/);
  }
});

/** Every `t("…")` key in the landing page's sources. */
function landingKeys(): string[] {
  const dir = new URL("../src/features/landing/", import.meta.url);
  const keys = new Set<string>();
  for (const file of readdirSync(dir).filter((f) => f.endsWith(".tsx"))) {
    const source = readFileSync(new URL(file, dir), "utf8");
    for (const m of source.matchAll(/\bt\(\s*"((?:[^"\\]|\\.)*)"/g)) keys.add(m[1]!);
  }
  return [...keys];
}

test("every string on the page is translated into zh-CN, zh-TW and ja", () => {
  const keys = landingKeys();
  assert.ok(keys.length > 60);
  for (const lng of ["zh-CN", "zh-TW", "ja"] as const) {
    const table = { ...landingBundles[lng], ...app[lng] };
    const missing = keys.filter((k) => !(k in table));
    assert.deepEqual(missing, [], `${lng} is missing translations`);
  }
  // The three tables say the same things.
  assert.deepEqual(Object.keys(landingBundles["zh-TW"]).sort(), Object.keys(landingBundles["zh-CN"]).sort());
  assert.deepEqual(Object.keys(landingBundles.ja).sort(), Object.keys(landingBundles["zh-CN"]).sort());
});

test("the home page's words are its own: no key it draws is one the app translates differently", async () => {
  // The landing bundle is added without overwriting (`landing.tsx`), so a key the app already has shows the app's wording.
  const { default: appI18n } = await import("../src/i18n/index.js");
  const home = ["landing.tsx", "product-tour.tsx"].flatMap((file) => [
    ...readFileSync(new URL(`../src/features/landing/${file}`, import.meta.url), "utf8").matchAll(/\bt\(\s*"((?:[^"\\]|\\.)*)"/g),
  ]).map((m) => JSON.parse(`"${m[1]}"`) as string);
  for (const lng of ["zh-CN", "zh-TW", "ja"] as const) {
    const appTable = (appI18n.store.data[lng]?.translation ?? {}) as Record<string, string>;
    for (const key of home) {
      if (key in appTable && key in landingBundles[lng]) assert.equal(appTable[key], landingBundles[lng][key], `${lng}: the app's "${appTable[key]}" hides the page's "${landingBundles[lng][key]}" for ${key}`);
    }
  }
});

test("the page reads in zh-TW and ja", () => {
  const tw = text(draw("zh-TW"));
  assert.ok(tw.includes("電腦上的 AI，手機上接著用。"));
  assert.ok(tw.includes("你的程式碼，只有你看得到。"));
  assert.ok(tw.includes("所有工作階段，一個清單"));
  assert.ok(tw.includes("登入"));
  const jp = text(draw("ja"));
  assert.ok(jp.includes("コンピュータの AI を、スマホで続きから。"));
  assert.ok(jp.includes("あなたのコードは、あなたにしか見えない。"));
  assert.ok(jp.includes("すべてのセッションを、ひとつのリストで"));
  assert.ok(jp.includes("はじめる"));
  for (const words of [tw, jp]) assert.doesNotMatch(words, /Your computer's AI|Your code, for your eyes only|Lid closed|Three steps|Run the migration/);
});

test("the hero sells before it explains: no Hub, relay, NAT, STUN, p50 or ciphertext in its words", () => {
  for (const lng of ["en", "zh-CN", "zh-TW", "ja"]) {
    const html = draw(lng);
    const copy = text(html.slice(html.indexOf("<h1"), html.indexOf('id="tour"')));
    assert.ok(copy.length > 40, lng);
    assert.doesNotMatch(copy, /Hub|relay|中继|中繼|中継|NAT|STUN|p50|ciphertext|密文|暗号文/, lng);
  }
  // The wording rule: a host is a device (设备), the reader's own computer may be 你的电脑, and nothing is a 机器.
  for (const value of Object.values(landingBundles["zh-CN"])) assert.doesNotMatch(value, /机器/, value);
  for (const value of Object.values(landingBundles["zh-TW"])) assert.doesNotMatch(value, /機器/, value);
});

test("only the bare root is the landing page; sign-in and sign-up have their own paths", () => {
  assert.equal(publicPage("/"), "landing");
  assert.equal(publicPage(""), "landing");
  assert.equal(publicPage("/signin"), "signin");
  assert.equal(publicPage("/signin/"), "signin");
  assert.equal(publicPage("/signup"), "signup");
  // A deep link into the app keeps the sign-in form in place, so it opens once the visitor is in.
  assert.equal(publicPage("/sessions/claude/abc"), "signin");
  assert.equal(publicPage("/terminals"), "signin");
  assert.ok(isAuthPath("/signin") && isAuthPath("/signup") && isAuthPath("/signup/"));
  assert.ok(!isAuthPath("/") && !isAuthPath("/sessions"));
});

test("the service worker gives the sign-in pages and 「我的设备」 the offline shell", () => {
  const sw = readFileSync(new URL("../public/sw.js", import.meta.url), "utf8");
  const route = new RegExp(/const SPA_ROUTE =\s*\/(.+)\/;/.exec(sw)![1]!);
  for (const path of ["/", "/signin", "/signup", "/verify", "/devices", "/devices/", "/sessions/claude/x", "/m/d-7f3a", "/m/d-7f3a/sessions/claude/x", "/m/d-7f3a/terminals"]) assert.ok(route.test(path), path);
  assert.ok(!route.test("/p2p-probe.html") && !route.test("/signin.js") && !route.test("/media/x"));
});

test("a line seals into ciphertext of the same shape, and opens back", () => {
  for (const line of ["All tests pass. Ready to merge.", "测试全部通过，可以合并了。", "テストはすべて通りました。"]) {
    const cipher = cipherOf(line, 7);
    assert.match(cipher, /^[0-9a-f ]+$/);
    // Two hex digits under a wide character, one under anything else.
    const width = [...line].reduce((n, ch) => n + (/[　-鿿＀-￯]/.test(ch) ? 2 : 1), 0);
    assert.equal(cipher.length, width);
    assert.equal(seal(line, 0, 7), line);
    assert.equal(seal(line, 1, 7), cipher);
    assert.equal(seal(line, 0.5, 7, 3), seal(line, 0.5, 7, 3), "deterministic");
  }
});

test("a wide character is one a monospace box gives two columns", () => {
  const at = (code: number) => String.fromCharCode(code);
  // Each range's two ends, the CJK punctuation and the ideographic space among them.
  for (const code of [0x1100, 0x115f, 0x2e80, 0x3000, 0x3001, 0x303f, 0x4e2d, 0xa4cf, 0xac00, 0xd7a3, 0xf900, 0xfaff, 0xfe30, 0xfe4f, 0xff00, 0xff60, 0xffe0, 0xffe6])
    assert.ok(wide(at(code)), `U+${code.toString(16)} is wide`);
  for (const code of [0x20, 0x41, 0xe9, 0x10ff, 0x1160, 0x2e7f, 0xa4d0, 0xabff, 0xd7a4, 0xfb00, 0xfe2f, 0xfe50, 0xff61, 0xffdf, 0xffe7])
    assert.ok(!wide(at(code)), `U+${code.toString(16)} is narrow`);
  assert.deepEqual([..."测试、テスト。한글　ok"].map(wide), [true, true, true, true, true, true, true, true, true, true, false, false]);
});

test("packets follow their wires", () => {
  const wire = [{ x: 0, y: 0 }, { x: 10, y: 0 }, { x: 10, y: 10 }];
  assert.deepEqual(pointAt(wire, 0.5), { x: 10, y: 0 });
  assert.deepEqual(pointAt(wire, 0.75), { x: 10, y: 5 });
  assert.deepEqual(pointAt(wire, 2), { x: 10, y: 10 });
});

test("the direct-path demo loses no keystroke: every key lands, in order, and the unconfirmed ones go again through the relay", () => {
  for (const scenario of ["same", "phone"] as const) {
    const keys = schedule(scenario);
    for (let i = 1; i < keys.length; i++) assert.ok(keys[i]!.arrive > keys[i - 1]!.arrive, `${scenario}: key ${i} lands after key ${i - 1}`);
    const resent = keys.filter((k) => k.lostAt !== undefined);
    assert.ok(resent.length >= 1, `${scenario}: a key in flight when the path drops is sent again`);
    assert.ok(resent.every((k) => k.route === "hub"));
    assert.ok(keys.some((k) => k.route === "direct") && keys.some((k) => k.route === "hub"));
  }
});

test("sign-up has its own page: /signup opens the form on its sign-up tab, with a way back to the landing page", async () => {
  const { AuthScreen } = await import("../src/features/cloud/cloud-app.js");
  void i18n.changeLanguage("en");
  const form = (mode?: "signin" | "signup") =>
    renderToStaticMarkup(
      <I18nextProvider i18n={i18n}>
        <AuthScreen api={{} as never} onSignedIn={() => undefined} initialMode={mode} />
      </I18nextProvider>,
    );
  const signup = form("signup");
  assert.match(signup, /aria-selected="true"[^>]*>sign up</);
  // Sign-up is open, as `SIGNUP_CONFIG` says: the optional invite code sits behind a link, no field yet.
  assert.match(text(signup), /have an invite code\?/);
  assert.doesNotMatch(signup, /data-testid="auth-invite"/);
  const signin = form();
  assert.match(signin, /aria-selected="true"[^>]*>sign in</);
  assert.doesNotMatch(text(signin), /invite code/);
  assert.match(signin, /<a href="\/"[^>]*>What is Droppen\?<\/a>/);
});

test("a shared link shows the hero: the share image is committed at 2400×1260 and the shell names it by its content hash", async () => {
  const { createHash } = await import("node:crypto");
  const png = readFileSync(new URL("../public/og.png", import.meta.url));
  assert.equal(png.subarray(0, 8).toString("hex"), "89504e470d0a1a0a");
  assert.deepEqual([png.readUInt32BE(16), png.readUInt32BE(20)], [2400, 1260]);
  const html = readFileSync(new URL("../index.html", import.meta.url), "utf8");
  const v = /<meta property="og:image" content="https:\/\/www\.droppen\.app\/og\.png\?v=([a-f0-9]{12})"/.exec(html)?.[1];
  assert.equal(v, createHash("sha256").update(png).digest("hex").slice(0, 12));
  assert.match(html, /<meta property="og:image:width" content="2400" \/>/);
  assert.match(html, /<meta name="description" content="[^"]+" \/>/);
  assert.match(html, /<meta name="twitter:card" content="summary_large_image" \/>/);
});

// --- Geometry: right angles, one bend at most, and no line through a word ---

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}
const around = (at: Pt, size: { w: number; h: number }): Rect => ({ x: at.x - size.w / 2, y: at.y - size.h / 2, w: size.w, h: size.h });
const inside = (p: Pt, r: Rect) => p.x >= r.x - 0.5 && p.x <= r.x + r.w + 0.5 && p.y >= r.y - 0.5 && p.y <= r.y + r.h + 0.5;

/** Whether an axis-aligned segment runs through the inside of `r` (touching an edge is not crossing). */
function crosses(a: Pt, b: Pt, r: Rect): boolean {
  const [x0, x1] = [Math.min(a.x, b.x), Math.max(a.x, b.x)];
  const [y0, y1] = [Math.min(a.y, b.y), Math.max(a.y, b.y)];
  const ix = Math.min(x1, r.x + r.w - 1) - Math.max(x0, r.x + 1);
  const iy = Math.min(y1, r.y + r.h - 1) - Math.max(y0, r.y + 1);
  return ix >= 0 && iy >= 0;
}

/**
 * Every wire is straight or one L, axis-aligned, and crosses no box except
 * the ones it starts or ends in (a wire ends at, or under, its own nodes).
 */
function checkWires(name: string, wires: Pt[][], boxes: Rect[]) {
  for (const [i, wire] of wires.entries()) {
    assert.ok(wire.length >= 2, `${name}: wire ${i} has two ends`);
    assert.ok(corners(wire) <= 1, `${name}: wire ${i} bends ${corners(wire)} times`);
    for (let k = 1; k < wire.length; k++) {
      const a = wire[k - 1]!;
      const b = wire[k]!;
      assert.ok(a.x === b.x || a.y === b.y, `${name}: wire ${i} runs at an angle`);
    }
    const ends = [wire[0]!, wire[wire.length - 1]!];
    for (const [j, box] of boxes.entries()) {
      if (ends.some((p) => inside(p, box))) continue;
      for (let k = 1; k < wire.length; k++) {
        assert.ok(!crosses(wire[k - 1]!, wire[k]!, box), `${name}: wire ${i} runs through box ${j}`);
      }
    }
  }
}

test("every diagram is right-angled: each wire straight or a single L, through no card or label", () => {
  for (const w of [288, 358, 379, 498, 597, 610, 645, 704, 1100, 1150]) {
    const D = directLayout(w);
    checkWires(`direct ${w}`, directWires(D), [D.device, D.machine, around(D.hub, DIRECT_HUB), ...D.labels]);

    const O = offlineLayout(w);
    checkWires(`offline ${w}`, offlineWires(O), [O.machine, O.device, O.hub, O.store]);
  }
});

test("the tour shows the product: six tabs of real screenshots, each in its frame, in the reader's language and both themes", () => {
  const html = draw("zh-CN");
  const tour = html.slice(html.indexOf('data-testid="product-tour"'), html.indexOf('id="how"'));
  // The tabs, the first one shown.
  assert.equal((tour.match(/role="tab"/g) ?? []).length, TOUR_TABS.length);
  assert.match(tour, /role="tab"[^>]*aria-selected="true"[^>]*data-tab="sessions"/);
  for (const label of ["会话", "分屏", "终端", "云端", "手机", "安全"]) assert.ok(text(tour).includes(label), label);
  // Its picture in both themes, the light one described.
  assert.match(tour, /<img src="\/tour\/app-session-zh-CN-light\.webp" alt="Mac 上的 Droppen[^"]*"/);
  assert.match(tour, /<img src="\/tour\/app-session-zh-CN-dark\.webp" alt="" aria-hidden="true"/);
  // Chinese readers get the Chinese screenshots, every other reader the English ones.
  assert.deepEqual(["zh-CN", "zh-TW", "ja", "en"].map(tourLanguage), ["zh-CN", "zh-CN", "en", "en"]);
  assert.equal(tourShot("phone-session", "ja", "dark"), "/tour/phone-session-en-dark.webp");
});

test("every screenshot the tour can show is in the build, in both languages and both themes", () => {
  const files = new Set(readdirSync(new URL("../public/tour/", import.meta.url)));
  for (const tab of TOUR_TABS) {
    for (const shot of TOUR_SHOTS[tab].shots) {
      for (const lng of ["zh-CN", "en"]) for (const theme of ["light", "dark"] as const) {
        const file = tourShot(shot, lng, theme).replace("/tour/", "");
        assert.ok(files.has(file), `missing: ${file}`);
      }
    }
  }
});

test("the diagrams that carry content mark what is encrypted where: a padlock on every wire leaving the sender", () => {
  const html = draw("zh-CN");
  for (const name of ["direct", "offline"]) {
    const from = html.indexOf(`data-diagram="${name}"`);
    assert.ok(from > 0, name);
    assert.ok(html.slice(from, from + 20000).includes("data-wire-lock"), `${name}: a padlock on the wire leaving the sender`);
  }
  // The offline copy shows the session, not ciphertext: no hex groups anywhere on the page.
  assert.doesNotMatch(text(html), /\b[0-9a-f]{4} [0-9a-f]{4} [0-9a-f]{4}\b/);
  // The speed figure opens on the phone on 4G, the case its headline is about.
  const direct = html.slice(html.indexOf('data-diagram="direct"') - 3000, html.indexOf('data-diagram="direct"'));
  assert.match(direct, /aria-selected="true"[^>]*>4G 网络</);
});

test("no region is named anywhere on the page: the relay is Droppen's, wherever it runs", () => {
  const region = /Singapore|新加坡|シンガポール|Tokyo|东京|東京/;
  // Only the privacy notice says where its providers keep data, as a privacy notice must (`privacy.tsx`).
  const privacy = readFileSync(new URL("../src/features/landing/privacy.tsx", import.meta.url), "utf8");
  const privacyKeys = new Set([...privacy.matchAll(/\bt\(\s*"((?:[^"\\]|\\.)*)"/g)].map((m) => JSON.parse(`"${m[1]}"`) as string));
  assert.ok([...privacyKeys].some((key) => region.test(key)), "the privacy notice names where");
  for (const lng of ["zh-CN", "zh-TW", "ja"] as const) {
    for (const [key, value] of Object.entries(landingBundles[lng])) {
      if (privacyKeys.has(key)) continue;
      assert.doesNotMatch(key, region, `${lng} key: ${key}`);
      assert.doesNotMatch(value, region, `${lng}: ${value}`);
    }
  }
  for (const lng of ["en", "zh-CN", "zh-TW", "ja"]) assert.doesNotMatch(text(draw(lng)), region, lng);
  assert.doesNotMatch(readFileSync(new URL("../index.html", import.meta.url), "utf8"), region);
  // The prose introduces both, where it draws the line: Droppen Hub finds your computers, Droppen relays pass ciphertext on.
  assert.ok(text(draw("zh-CN")).includes("Droppen Hub 只负责找到你的电脑、保管加密副本，Droppen 中继只负责转发密文"));
  assert.ok(text(draw("en")).includes("Droppen Hub only finds your computers and keeps their encrypted copies; Droppen relays only pass ciphertext on"));
});

test("the diagrams show product properties, not implementation trivia, and call the relay a Droppen relay", () => {
  for (const lng of ["en", "zh-CN", "zh-TW", "ja"]) {
    const words = text(draw(lng));
    // No versions, fingerprints, ids, session counts, unread counts or page hashes on the cards.
    assert.doesNotMatch(words, /agent 0\.|m-5vln|d-7c1e|fingerprint|指纹|指紋|unread|未读|未讀|未読|page #/, lng);
    // Never a lower-case "hub"; Droppen Hub is still there, for the sealed pages it keeps.
    assert.doesNotMatch(words, /(?<![A-Za-z])hub(?![A-Za-z])/, `${lng}: lower-case hub`);
    assert.doesNotMatch(words, /Japan(?!ese)|日本(?!語)/, `${lng}: a country`);
    assert.match(words, /Droppen Hub/);
  }
});

test("whatever is about the path names a Droppen relay, never Droppen Hub: the hub forwards nothing", () => {
  const relay = { en: "Droppen relay", "zh-CN": "Droppen 中继", "zh-TW": "Droppen 中繼", ja: "Droppen 中継" } as const;
  for (const lng of ["en", "zh-CN", "zh-TW", "ja"] as const) {
    const html = draw(lng);
    const between = (from: string, to: string) => {
      const start = html.indexOf(from);
      assert.ok(start > 0, `${lng}: ${from}`);
      return text(html.slice(start, html.indexOf(to, start)));
    };
    // The whole direct-path section, its prose, readouts and status: a relay, never the hub.
    const direct = between('id="direct"', 'data-diagram="offline"');
    assert.doesNotMatch(direct, /Hub/, `${lng}: the path goes through a relay`);
    assert.ok(direct.includes(relay[lng]), `${lng}: the relay's card`);
    // The hero names neither: it is about your sessions, not the path.
    assert.doesNotMatch(text(html.slice(html.indexOf("<main"), html.indexOf('id="tour"'))), /Hub|relay|中继|中繼|中継/, `${lng}: the hero`);
  }
});
