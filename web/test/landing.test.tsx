// Server-rendered public landing page, routing, translation, and content contracts.
import * as React from "react";
import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import test from "node:test";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { AGENTS } from "../src/features/landing/agent-marks.js";
import { landingBundles } from "../src/features/landing/copy.js";
import { TOUR_SHOTS, tourLanguage, tourShot } from "../src/features/landing/product-tour.js";
import LandingPage from "../src/features/landing/landing.js";
import { PLAN_NUMBERS } from "../src/features/landing/pricing-plans.js";
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
    // The selling points: your computer's Agent on your phone, every session in one list, code only you can read.
    "电脑上的 Agent，在手机上接着使用",
    "Claude Code 和 Codex 仍在你自己的电脑上运行。",
    "所有电脑，一个列表",
    "安全与端到端加密",
    // The deep sections, each headed by what it gets you.
    "直连优先，网络受限时自动中继",
    "免费注册",
  ]) {
    assert.ok(words.includes(line), `missing: ${line}`);
  }
  assert.ok(html.includes("登录"), "header includes sign in");
  // The sections the header links to, in the page's order: the product first, encryption last.
  const at = (id: string) => html.indexOf(`id="${id}"`);
  const order = ["tour", "features", "direct", "security"];
  for (const id of order) assert.ok(at(id) > 0, id);
  assert.deepEqual([...order].sort((a, b) => at(a) - at(b)), order);
  for (const id of order) assert.match(html, new RegExp(`<a href="#${id}"|id="${id}"`));
});

test("the FAQ is one vertical list at every width", () => {
  const html = draw("en");
  const faq = html.slice(html.indexOf('id="faq"'), html.indexOf('id="start"'));
  assert.equal((faq.match(/data-testid="faq-item"/g) ?? []).length, 4);
  const list = /<div[^>]*data-testid="landing-faq"[^>]*>/.exec(faq)?.[0];
  assert.ok(list, "the FAQ list");
  // The questions are one card (`components/ui/card`, shadcn): it draws its own edge on
  // all four sides, so the block reads as a card rather than as more of the page's hairlines.
  assert.match(list, /data-slot="card"/);
  assert.match(list, /\bring-1\b/);
  assert.doesNotMatch(list, /grid-cols|flex-row/);
  // Each question is separated from the next by a line.
  assert.match(faq, /data-testid="faq-item"[^>]*class="[^"]*border-b border-border/);
  // The heading is the wide window's at every width — left-aligned, never centred over the
  // card — and the two columns are for lg and up.
  assert.match(faq, /<h2 class="[^"]*">FAQ<\/h2>/);
  assert.doesNotMatch(faq, /<h2 class="[^"]*text-center/);
  assert.match(faq, /lg:grid-cols-\[minmax\(0,0\.45fr\)_minmax\(0,1fr\)\]/);
});

test("every public page's header links the site's other pages and the client's source, not the page's own sections", () => {
  for (const lng of ["en", "zh-CN"]) {
    const header = draw(lng).slice(0, draw(lng).indexOf("<main"));
    for (const [path, testId] of [["/download", "header-download"], ["/pricing", "header-pricing"], ["/security", "header-security"]]) {
      assert.match(header, new RegExp(`<a href="${path}"[^>]*data-testid="${testId}"`), `${lng}: ${path}`);
    }
    assert.match(header, /<a href="https:\/\/github\.com\/getdroppen\/droppen"[^>]*data-testid="header-source"/, lng);
    assert.doesNotMatch(header, /href="#/, `${lng}: no anchors into the page`);
  }
  assert.match(draw("zh-CN").slice(0, draw("zh-CN").indexOf("<main")), /data-testid="header-download"[^>]*>[\s\S]*?<span[^>]*>下载<\/span><\/a>/);
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
    assert.ok(html.indexOf('id="security"') > html.indexOf('id="direct"'), lng);
    assert.match(html.slice(html.indexOf('id="security"')), /<a href="\/security"[^>]*data-testid="landing-security"/, lng);
  }
  // No line-by-line ledger of what we can and cannot see: that is /security's.
  assert.doesNotMatch(text(draw("en")), /What we can see|What we cannot see|Who holds the keys/);
});

test("one primary action, and the calls to action go to sign-up and sign-in", () => {
  const html = draw("en");
  const primaries = html.match(/data-variant="primary"/g) ?? [];
  assert.equal(primaries.length, 1, "one primary per page (docs/ui-spec.md V10)");
  assert.match(html, /<a (?=[^>]*href="\/signup")(?=[^>]*data-variant="primary")[^>]*>Create a free account<\/a>/);
  // Sign-in is the header's user menu and nothing else: the page ends on sign-up alone.
  assert.match(html, /aria-label="Sign in"/, "sign-in in the header user menu");
  assert.equal((html.match(/<a href="\/signin"/g) ?? []).length, 0, "no sign-in link on the page");
  // The page ends on the same call to action, as a secondary.
  assert.match(html, /<a (?=[^>]*href="\/signup")[^>]*>[\s\S]*Create a free account[\s\S]*<\/a>/, "sign-up at the end");
});

test("signed in, the landing page is still the landing page, with a way back into the app instead of sign-in and sign-up", () => {
  const app = (label: string, primary: boolean) =>
    new RegExp(`<a (?=[^>]*href="${APP_PATH}")${primary ? '(?=[^>]*data-variant="primary")' : ""}[^>]*>${label}</a>`);
  for (const [lng, open, how] of [
    ["zh-CN", "打开应用", "工作原理"],
    ["en", "Open app", "How it works"],
    ["zh-TW", "打開應用", "運作方式"],
    ["ja", "アプリを開く", "しくみ"],
  ] as const) {
    const html = draw(lng, { signedIn: true });
    // The hero's primary opens the app; the header offers entry via user menu.
    assert.match(html, new RegExp(`aria-label="${open}"`), `${lng}: header user menu opens app`);
    assert.equal((html.match(new RegExp(`<a (?=[^>]*href="${APP_PATH}")[^>]*>`, "g")) ?? []).length, 1, lng);
    assert.match(html, app(open, true), `${lng}: the hero's primary opens the app`);
    assert.equal((html.match(/data-variant="primary"/g) ?? []).length, 1, `${lng}: one primary`);
    assert.match(html, new RegExp(`<a (?=[^>]*href="#tour")[^>]*>${how}</a>`), `${lng}: the secondary reads on through the page`);
    // Nothing asks someone signed in to sign in or sign up.
    assert.doesNotMatch(html, /href="\/sign(in|up)"/, lng);
    // The page itself is all there.
    for (const id of ["tour", "features", "direct", "security"]) assert.match(html, new RegExp(`id="${id}"`), lng);
  }
  const words = text(draw("zh-CN", { signedIn: true }));
  for (const gone of ["台运行 AI 的电脑免费", "免费注册", "AI 在你的电脑上继续工作"]) assert.ok(!words.includes(gone), `still there: ${gone}`);
  assert.ok(!text(draw("zh-CN", { signedIn: true, signup: "invite" })).includes("注册需要邀请码。"), "by invite too");
  assert.ok(text(draw("zh-CN")).includes("免费注册"), "signed out keeps the sign-up prompt");
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
});

test("the numbers are the measured ones, and nothing claims more than the build does", () => {
  const words = text(draw("en"));
  for (const n of ["63–73 ms", "215–227 ms", "1.5 s"]) assert.ok(words.includes(n), `missing: ${n}`);
  // What a phone can read while the computer is off: the plan's window (the agent syncs min(30 days, the plan's retention)).
  assert.ok(words.includes(`the last ${PLAN_NUMBERS.free.retentionDays} days (${PLAN_NUMBERS.pro.retentionDays} on Pro)`));
  for (const claim of [/unhackable/i, /zero risk/i, /\baudited\b/i, /military[- ]grade/i, /100% secure/i]) {
    assert.doesNotMatch(words, claim);
  }
});

test("the hero is its action, not a screenshot: the agents' marks ring it as decoration", () => {
  for (const lng of ["en", "zh-CN"]) {
    const html = draw(lng);
    const hero = html.slice(html.indexOf("<main"), html.indexOf('id="tour"'));
    // The screens are gone from the hero: the first screen is the promise and the way in.
    assert.doesNotMatch(hero, /<img\b/, `${lng}: no screenshots in the hero`);
    assert.doesNotMatch(hero, /\/tour\//, `${lng}: no product shot in the hero`);
    assert.match(hero, /data-testid="hero-get"/, `${lng}: the action is still there`);
    // The marks of the agents the app follows ring that action, as decoration: one field
    // hidden from assistive technology and taking no pointer. Its marks are the library's
    // SVGs, which only Vite can read (`import.meta.glob`), so this test reads the same files
    // from disk: every agent `agent-marks.tsx` lists has one.
    assert.match(hero, /<div aria-hidden="true" data-allow-color=""[^>]*class="pointer-events-none absolute inset-0"/, `${lng}: a hidden, inert field`);
    assert.ok(AGENTS.length >= 30, "the ring carries the agents a terminal can run, not just two");
    const icons = new URL("../node_modules/@lobehub/icons-static-svg/icons/", import.meta.url);
    for (const agent of AGENTS) {
      const files = [`${agent.icon.slug}-color.svg`, `${agent.icon.slug}.svg`];
      assert.ok(files.some((f) => existsSync(new URL(f, icons))), `${lng}: ${agent.name} has a mark in the icon set`);
    }
  }
});

test("the hero is the first thing on the page: the beta's caveat is /security's, not a note above it", () => {
  for (const lng of ["en", "zh-CN", "zh-TW", "ja"]) {
    const html = draw(lng);
    const main = html.slice(html.indexOf("<main"));
    assert.doesNotMatch(main.slice(0, main.indexOf("<h1")), /<p\b|<svg\b/, `${lng}: nothing above the hero's headline`);
    assert.doesNotMatch(text(html), /external audit|外部审计|外部稽核|外部監査/, lng);
  }
});

test("only the client is called open source: the hub never is, and nothing says all of Droppen is", () => {
  const en = text(draw("en"));
  assert.match(en, /open-source client/i);
  assert.match(en, /Droppen Hub is not open source/);
  // Every mention is the client's, or says the hub is not.
  for (const mention of en.match(/[^.:;]*open[- ]source[^.:;]*/gi) ?? []) {
    assert.match(mention, /client|command-line program|Droppen Hub is not/i, mention);
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
  assert.match(draw("en"), /<a href="https:\/\/github\.com\/getdroppen\/droppen"[^>]*data-testid="landing-source"/);
});

test("open sign-up by default: no invite wording; the invite wording only where the hub says sign-up is by invite, and \"closed for now\" where it is closed", () => {
  // Open (droppen.app since 2026-10-23), and what the page draws before the hub has answered.
  for (const signup of [undefined, "open"] as const) {
    const html = draw("en", { signup });
    const words = text(html);
    assert.doesNotMatch(words, /invite/i, `${signup}: no invite`);
    // The first screen sells the product, not its price: no plans, no link to them (the header and the footer have one).
    assert.doesNotMatch(words, /Public beta|Free for up to|See pricing/, `${signup}: no plans in the hero`);
    assert.doesNotMatch(html, /data-testid="hero-pricing"/);
    assert.doesNotMatch(html, /data-testid="hero-waitlist"/);
    assert.match(html, /<a (?=[^>]*href="\/signup")[^>]*>Create a free account<\/a>/);
  }
  for (const [lng, invite] of [["zh-CN", /邀请/], ["zh-TW", /邀請/], ["ja", /招待/]] as const) {
    assert.doesNotMatch(text(draw(lng)), invite, `${lng}: no invite`);
  }
  for (const lng of ["zh-CN", "zh-TW", "ja"]) assert.doesNotMatch(text(draw(lng)), /公开测试版|公開測試版|パブリックベータ|查看定价|查看定價|料金を見る/, `${lng}: no plans in the hero`);
  // By invite: as before — the code, and the waitlist for those without one.
  const invited = draw("en", { signup: "invite" });
  assert.ok(text(invited).includes("Invite-only for now: sign-up needs an invite code."));
  assert.match(invited, /<a href="\/pricing#waitlist"[^>]*data-testid="hero-waitlist"[^>]*>No code yet\? Join the waitlist<\/a>/);
  assert.doesNotMatch(invited, /data-testid="hero-pricing"/);
  // Closed: closed for now, and the waitlist; no invite code and nothing to sign up with.
  const closed = draw("en", { signup: "closed" });
  assert.ok(text(closed).includes("Sign-up is closed for now."));
  assert.match(closed, /<a href="\/pricing#waitlist"[^>]*data-testid="hero-waitlist"[^>]*>Join the waitlist<\/a>/);
  assert.doesNotMatch(text(closed), /invite|Public beta|Sign up with your/i);
  // The ways in: the waitlist where "Get started" was (the hero's and the footer's), and sign-in.
  assert.doesNotMatch(closed, /href="\/signup"|Create a free account/);
  assert.equal((closed.match(/<a (?=[^>]*href="\/pricing#waitlist")[^>]*>[\s\S]*?Join the waitlist[\s\S]*?<\/a>/g) ?? []).length, 3, "the hero's line, its account action and the footer's");
  assert.match(closed, /aria-label="Sign in"/, "sign-in stays in the header's user menu");
  for (const signup of ["open", "invite"] as const) assert.match(draw("en", { signup }), /<a (?=[^>]*href="\/signup")[^>]*>Create a free account<\/a>/, signup);
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
  // Every language loaded: the app fetches a language's tables only when it first shows it.
  const { default: appI18n } = await import("./i18n-all.js");
  const home = ["landing.tsx", "product-tour.tsx"].flatMap((file) => [
    ...readFileSync(new URL(`../src/features/landing/${file}`, import.meta.url), "utf8").matchAll(/\bt\(\s*"((?:[^"\\]|\\.)*)"/g),
  ]).map((m) => JSON.parse(`"${m[1]}"`) as string);
  for (const lng of ["zh-CN", "zh-TW", "ja"] as const) {
    const appTable = (appI18n.store.data[lng]?.translation ?? {}) as Record<string, string>;
    assert.ok(Object.keys(appTable).length > 500, `${lng}: the app's table is loaded`);
    for (const key of home) {
      if (key in appTable && key in landingBundles[lng]) assert.equal(appTable[key], landingBundles[lng][key], `${lng}: the app's "${appTable[key]}" hides the page's "${landingBundles[lng][key]}" for ${key}`);
    }
  }
});

test("the page reads in zh-TW and ja", () => {
  const twHtml = draw("zh-TW");
  const tw = text(twHtml);
  assert.ok(tw.includes("電腦上的 Agent，在手機上接著使用"));
  assert.ok(tw.includes("安全與端對端加密"));
  assert.ok(tw.includes("所有電腦，一個清單"));
  assert.ok(twHtml.includes("登入"));
  const jpHtml = draw("ja");
  const jp = text(jpHtml);
  assert.ok(jp.includes("コンピュータの Agent を、スマホで続きから。"));
  assert.ok(jp.includes("セキュリティとエンドツーエンド暗号化"));
  assert.ok(jp.includes("すべてのコンピュータを、ひとつのリストで"));
  assert.ok(jp.includes("無料で登録"));
  for (const words of [tw, jp]) assert.doesNotMatch(words, /Your computer's Agent|Security and encryption|stay readable|Three steps|Run the migration/);
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

test("the product showcase shows the product: three screens in their frames, in the page's language (Chinese or English) and both themes", () => {
  const html = draw("zh-CN");
  const tour = html.slice(html.indexOf('id="tour"'), html.indexOf('id="features"'));
  // No section head over the pictures: the screenshots and their captions say what the
  // product is, without a line announcing them.
  for (const [lng, head] of [["zh-CN", "应用里的三个界面。"], ["zh-TW", "應用程式裡的三個介面。"], ["ja", "アプリの 3 つの画面。"], ["en", "The app, in three screens."]] as const) {
    assert.ok(!text(draw(lng)).includes(head), `${lng}: the showcase has no section head`);
    if (lng !== "en") assert.ok(!("The app, in three screens." in landingBundles[lng]), `${lng}: the removed section head`);
  }
  // Three screens, and only the caption of the one on screen is written: the server render
  // opens the first, so its headline and body are there and the other two are not.
  const entries = [
    ["sessions", "所有电脑，一个列表。", "将多台 Mac 与 Linux 电脑接入同一个账户。会话状态实时汇总，一目了然定位正在运行或等待确认的任务。"],
    ["split", "左边代码，右边终端。", "AI 会话与主机原生终端同屏并排运行。实时查看代码改动、测试日志与命令输出，无需在窗口间频繁切换。"],
    ["phone", "离开电脑，在手机上接手。", "在手机浏览器打开 droppen.app 即时同步会话。「需要你」待处理任务优先置顶，随时随地审批命令、回复提问或派发新任务。"],
  ] as const;
  const [first] = entries;
  assert.ok(text(tour).includes(first[1]), `${first[0]}: the open screen's headline`);
  assert.ok(text(tour).includes(first[2]), `${first[0]}: the open screen's body`);
  assert.ok(tour.includes(`id="tour-${first[0]}"`), `${first[0]}: the body carries its id`);
  // The captions of the screens not on screen are nowhere on the page.
  for (const [value, title, body] of entries.slice(1)) {
    assert.ok(!text(tour).includes(title), `${value}: its headline waits for its turn`);
    assert.ok(!text(tour).includes(body), `${value}: its body waits for its turn`);
  }
  // No 01/02/03 numbering beside the entries, and no list of the entries as an index.
  assert.doesNotMatch(tour, />0[123]<\/span>/, "no numbered entries");
  // A carousel of exactly three screens: three ticks on its rail over three panels. Every
  // panel is kept mounted (so all three screenshots below are in the markup, and the
  // swap is a style change rather than a reload), but the server render opens the first
  // one and leaves the other two `hidden` so the page is usable before hydration.
  assert.equal((tour.match(/role="tab"/g) ?? []).length, 3, "three ticks, as tabs");
  assert.equal((tour.match(/role="tabpanel"/g) ?? []).length, 3, "three screens, one panel each");
  const firstTab = /<button[^>]*role="tab"[^>]*>/.exec(tour)?.[0];
  assert.ok(firstTab, "the first tick");
  assert.match(firstTab, /aria-selected="true"/, "the first screen is open in the server render");
  assert.equal((tour.match(/role="tab"[^>]*aria-selected="true"/g) ?? []).length, 1, "only one tick filled");
  assert.equal((tour.match(/role="tab"[^>]*aria-selected="false"/g) ?? []).length, 2, "the other two ticks are closed");
  // Three compact capsules, not a second toolbar or a full-width rail. Each has a
  // generous tap target and names the screen it goes to.
  // The rail is the ui-spec's (§83): a centred, w-fit list (three 44px taps, 132px); a 24px track
  // in each; and, in the tick on show, an ink fill that runs the tour's own interval.
  assert.match(tour, /data-shape="" class="relative block h-1 w-6 rounded-full bg-border[^"]*"/, "compact capsule tracks");
  assert.equal((tour.match(/data-shape="" class="relative block h-1 w-6/g) ?? []).length, 3, "one track per screen");
  assert.match(tour, /role="tablist" aria-label="选择界面"[^>]*class="[^"]*flex[^"]*w-fit[^"]*justify-center"/, "three compact marks");
  assert.equal((tour.match(/class="group\/tick flex h-11 w-11 /g) ?? []).length, 3, "a 44px tap target per mark");
  const fill = /<span[^>]*data-fill="[^"]+"[^>]*class="[^"]*bg-fg[^"]*"[^>]*>/.exec(tour)?.[0];
  assert.ok(fill, "the tick on show carries a fill");
  assert.match(fill, /hidden group-data-\[active\]\/tick:block/, "only the tick on show is inked");
  assert.match(fill, /animation-duration:var\(--tour-rotate\)/, "the fill runs the tour's interval");
  assert.match(firstTab, /aria-label="第 1 个界面，共 3 个：所有电脑，一个列表。"/, "the tick names its screen");
  assert.equal((tour.match(/aria-label="第 \d 个界面，共 3 个：/g) ?? []).length, 3, "one tick per screen");
  const panels = [...tour.matchAll(/<div[^>]*role="tabpanel"[^>]*>/g)].map((m) => m[0]);
  assert.equal(panels.length, 3, "one panel per screen");
  assert.doesNotMatch(panels[0]!, /\shidden=""/, "the first panel is open");
  for (const panel of panels.slice(1)) assert.match(panel, /\shidden=""/, "the other panels start hidden");
  // Nothing counts the screens on the page: the capsules say where the tour is, and the
  // caption says which screen it belongs to. No `01 / 03`, no `01`/`02`/`03` entries.
  assert.doesNotMatch(text(tour), /\b0\d\s*\/\s*0\d\b/, "no position counter");
  // One screenshot per screen, in both themes, in the page's language: the app window,
  // its split view and the phone, with the alt texts that describe them.
  const shots: Record<string, string> = {
    "app-session": "Mac 上的 Droppen：Claude Code 和 Codex 的会话列表，以及一个等你回答的会话",
    "app-split": "Mac 上的 Droppen 分成三栏：两个会话和一个终端",
    "phone-needs-you": "手机上的 Droppen：「需要你」清单，以及一个会话的提问",
  };
  for (const [shot, alt] of Object.entries(shots)) {
    for (const theme of ["light", "dark"] as const) {
      assert.ok(tour.includes(`src="/tour/${shot}-zh-CN-${theme}.webp" alt="${alt}"`), `${shot} (${theme}): ${alt}`);
    }
  }
  // The dark variant is a real picture with the same alt, not hidden from assistive
  // technology: the theme swap is CSS (`dark:hidden` / `dark:block`), so a reader gets
  // one image either way, never none.
  assert.match(tour, /<img src="\/tour\/app-session-zh-CN-light\.webp" alt="Mac 上的 Droppen[^"]*"/);
  assert.doesNotMatch(tour, /<img src="\/tour\/app-session-zh-CN-dark\.webp"[^>]*aria-hidden/);
  // The old showcase's amber Focus callouts are gone: no signal marker, no warn border,
  // and none of the callout labels. 终端 survives only inside the split view's body
  // copy, so the removed key is what the test pins, not the word.
  assert.doesNotMatch(tour, /data-signal="waiting"/);
  assert.doesNotMatch(tour, /border-warn/);
  for (const label of ["等你回答 / 工作中", "需要你处理"]) assert.ok(!text(tour).includes(label), `still there: ${label}`);
  for (const lng of ["zh-CN", "zh-TW", "ja"] as const) {
    for (const key of ["Waiting for you / working", "Terminal", "Needs you"]) {
      assert.ok(!(key in landingBundles[lng]), `${lng}: the removed callout key ${key}`);
    }
  }
  // The pictures rotate by themselves, the small marks choose one directly, and touch
  // swipes move between them. No arrow buttons or play/pause switch crowd the picture, and
  // their words are not kept in the tables for a control that no longer exists.
  assert.doesNotMatch(tour, /aria-label="暂停轮播"/, "no pause switch: the tour stops itself while it is being read");
  assert.doesNotMatch(tour, /aria-label="上一个界面"|aria-label="下一个界面"/, "no arrow buttons");
  for (const lng of ["zh-CN", "zh-TW", "ja"] as const) {
    for (const key of ["Previous screen", "Next screen", "Pause the rotation", "Play the rotation"]) {
      assert.ok(!(key in landingBundles[lng]), `${lng}: the removed tour key ${key}`);
    }
  }
  // Chinese pages show the app in Chinese, every other page in English, in both themes.
  assert.deepEqual(["zh-CN", "zh-TW", "ja", "en"].map(tourLanguage), ["zh-CN", "zh-CN", "en", "en"]);
  for (const [lng, lang] of [["zh-TW", "zh-CN"], ["ja", "en"]] as const) {
    const localized = draw(lng);
    const section = localized.slice(localized.indexOf('id="tour"'), localized.indexOf('id="features"'));
    for (const shot of Object.keys(shots)) {
      for (const theme of ["light", "dark"] as const) {
        assert.ok(section.includes(`/tour/${shot}-${lang}-${theme}.webp`), `${lng}: ${shot} (${theme})`);
      }
    }
  }
  assert.match(draw("en"), /<img src="\/tour\/app-session-en-light\.webp" alt="Droppen on a Mac[^"]*"/);
});

test("every screenshot the tour can show is in the build, in both languages and both themes, and nothing it cannot", () => {
  const files = readdirSync(new URL("../public/tour/", import.meta.url)).sort();
  const shown = TOUR_SHOTS.flatMap((shot) =>
    ["zh-CN", "en"].flatMap((lng) => (["light", "dark"] as const).map((theme) => tourShot(shot, lng, theme).replace("/tour/", ""))),
  );
  assert.deepEqual(files, [...new Set(shown)].sort());
});

test("the network section draws the path: direct first, the relay as the fallback, with the measured times", () => {
  const html = draw("en");
  const network = html.slice(html.indexOf('id="direct"'), html.indexOf('id="security"'));
  const words = text(network);
  assert.equal((network.match(/role="img"/g) ?? []).length, 1, "one responsive figure");
  for (const line of ["Direct · 63–73 ms", "Relay · 215–227 ms", "Switches within 1.5 s", "Keys you type are sent again in order", "Droppen relay", "Encrypted data only"]) {
    assert.ok(words.includes(line), `missing: ${line}`);
  }
  assert.doesNotMatch(network, /data-wire-lock|aria-pressed/);
  // Every diagram has a real viewBox.
  for (const [, box] of network.matchAll(/viewBox="([^"]+)"/g)) assert.match(box!, /^-?\d+(\.\d+)?\s+-?\d+(\.\d+)?\s+\d+(\.\d+)?\s+\d+(\.\d+)?$/, box);
  // The diagram is about the path, not the payload: no ciphertext hex groups anywhere.
  assert.doesNotMatch(text(html), /\b[0-9a-f]{4} [0-9a-f]{4} [0-9a-f]{4}\b/);
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
  // The encryption card draws the line between them: Droppen Hub keeps the encrypted copies, Droppen relays pass encrypted data on.
  assert.ok(text(draw("zh-CN")).includes("Droppen Hub 保存的只是加密副本，Droppen 中继转发的只是加密数据"));
  assert.ok(text(draw("en")).includes("Droppen Hub stores only encrypted copies and Droppen relays forward only encrypted data"));
});

test("the network diagram shows product properties, not implementation trivia, and calls the relay a Droppen relay", () => {
  for (const lng of ["en", "zh-CN", "zh-TW", "ja"]) {
    const html = draw(lng);
    const words = text(html);
    // No versions, fingerprints, ids, session counts, unread counts or page hashes on the diagrams' cards.
    const diagrams = text(html.slice(html.indexOf('id="direct"'), html.indexOf('id="security"')));
    assert.doesNotMatch(diagrams, /agent 0\.|m-5vln|d-7c1e|fingerprint|指纹|指紋|unread|未读|未讀|未読|page #/, lng);
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
    // The whole network section, its prose, readouts and diagram: a relay, never the hub.
    const direct = between('id="direct"', 'id="security"');
    assert.doesNotMatch(direct, /Hub/, `${lng}: the path goes through a relay`);
    assert.ok(direct.includes(relay[lng]), `${lng}: the relay's card`);
    // The hero names neither: it is about your sessions, not the path.
    assert.doesNotMatch(text(html.slice(html.indexOf("<main"), html.indexOf('id="tour"'))), /Hub|relay|中继|中繼|中継/, `${lng}: the hero`);
  }
});
