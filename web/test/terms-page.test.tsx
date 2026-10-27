// droppen.app/terms (`features/landing/terms/terms.tsx`): the terms of service
// of the hosted service.
//
// What these pin: `/terms` is a public page of its own, signed in or not,
// linked from every public page's footer and from the sign-up form (with the
// privacy notice, each in a new tab; never on sign-in); it reads in en,
// zh-CN, zh-TW and ja with every string translated and nothing left in
// English; it says what the product is — end-to-end encrypted, the terms
// covering the hosted service only and the code under its own licences, the
// Free plan's numbers as the plans have them, "as is" with no SLA and the
// identity renewal in testing, the addresses for an account and for
// security, and the owner's facts (Droppen operates droppen.app; the laws
// and courts of Singapore, the consumer's own law kept; 16 and over; no
// postal address) — in product words (a device, our servers; never a machine
// or a relay), with no owner placeholder left.
import * as React from "react";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { I18nextProvider } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { AuthScreen } from "../src/features/cloud/cloud-app.js";
import { landingBundles } from "../src/features/landing/copy.js";
import DownloadPage from "../src/features/landing/download.js";
import LandingPage from "../src/features/landing/landing.js";
import { SECURITY_EMAIL } from "../src/features/landing/page-kit.js";
import PricingPage from "../src/features/landing/pricing.js";
import { gb, PLAN_NUMBERS } from "../src/features/landing/pricing-plans.js";
import PrivacyPage from "../src/features/landing/privacy.js";
import { bootRoute, frontPage, isAuthPath, isFrontPath, publicPage, TERMS_PATH } from "../src/features/landing/route.js";
import SecurityPage, { SECURITY_EMAIL as SECURITY_PAGE_EMAIL } from "../src/features/landing/security/security.js";
import TermsPage, { MINIMUM_AGE, TERMS_UPDATED } from "../src/features/landing/terms/terms.js";
import { termsBundles } from "../src/features/landing/terms/terms-copy.js";
import { PRIVACY_EMAIL } from "../src/features/landing/waitlist.js";
import { setHostMode } from "../src/lib/hub/hosts.js";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "../src/i18n/cloud-devices.js";
import { ja } from "../src/i18n/ja.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";
import { makeI18n } from "./render.js";

const app = { "zh-CN": { ...cloudDevicesZhCN, ...zhCN }, "zh-TW": { ...cloudDevicesZhTW, ...zhTW }, ja: { ...cloudDevicesJa, ...ja } };
const LANGS = ["zh-CN", "zh-TW", "ja"] as const;
const i18n = makeI18n({
  lng: "en",
  resources: {
    en: { translation: {} },
    // As the page adds them: the landing tables, then its own; the app's wording wins where both have a key.
    ...Object.fromEntries(LANGS.map((lng) => [lng, { translation: { ...termsBundles[lng], ...landingBundles[lng], ...app[lng] } }]))
  },
  keySeparator: false,
  nsSeparator: false
});

// The pricing page reads `?plan=` off the address; the static render never moves.
Object.assign(globalThis, { location: { search: "", pathname: TERMS_PATH, origin: "https://droppen.app", hash: "" } });

function render(element: React.ReactElement, lng: string): string {
  void i18n.changeLanguage(lng);
  return renderToStaticMarkup(<I18nextProvider i18n={i18n}>{element}</I18nextProvider>);
}
const draw = (lng: string, signedIn = false) => render(<TermsPage signedIn={signedIn} />, lng);

// A link and a marked placeholder sit inside their sentence: no space for them.
const text = (html: string) =>
  html
    .replace(/<\/?(?:a|mark)\b[^>]*>/g, "")
    .replace(/<[^>]+>/g, " ")
    .replace(/&#x27;/g, "'")
    .replace(/&quot;/g, '"')
    .replace(/&amp;/g, "&")
    .replace(/\s+/g, " ");

/** Every `t("…")` key in the page's source. */
function pageKeys(): string[] {
  const source = readFileSync(new URL("../src/features/landing/terms/terms.tsx", import.meta.url), "utf8");
  return [...new Set([...source.matchAll(/\bt\(\s*"((?:[^"\\]|\\.)*)"/g)].map((m) => JSON.parse(`"${m[1]}"`) as string))];
}

const SIGN_UP_LINE = "By signing up you agree to the <terms>Terms of Service</terms> and have read the <privacy>Privacy notice</privacy>.";

test("`/terms` is a public page of its own, signed in or not", () => {
  for (const path of [TERMS_PATH, `${TERMS_PATH}/`]) {
    assert.equal(publicPage(path), "terms");
    assert.equal(frontPage(path), "terms");
    assert.deepEqual(bootRoute(path, false), { show: "terms" });
    assert.deepEqual(bootRoute(path, true), { show: "terms" });
    assert.ok(isFrontPath(path) && !isAuthPath(path));
  }
  assert.match(draw("en"), /data-testid="terms"/);
  // Signed in: the way back into the app instead of sign-in.
  assert.match(draw("en", true), /href="\/sessions"[^>]*>Open app</);
});

test("every public page's footer links it", () => {
  const pages = {
    landing: <LandingPage />,
    download: <DownloadPage availability={{ app: true, linux: true }} hints={{ userAgent: "" }} />,
    pricing: <PricingPage api={{ joinWaitlist: async () => undefined }} />,
    privacy: <PrivacyPage />,
    security: <SecurityPage />,
    terms: <TermsPage />,
  };
  for (const [name, element] of Object.entries(pages)) {
    assert.match(render(element, "en"), /<a href="\/terms"[^>]*data-testid="footer-terms"[^>]*>Terms of Service<\/a>/, `${name}: the footer`);
  }
  assert.match(render(<LandingPage />, "zh-CN"), /data-testid="footer-terms"[^>]*>服务条款<\/a>/);
  assert.match(render(<LandingPage />, "zh-TW"), /data-testid="footer-terms"[^>]*>服務條款<\/a>/);
  assert.match(render(<LandingPage />, "ja"), /data-testid="footer-terms"[^>]*>利用規約<\/a>/);
});

test("the sign-up form links the terms and the privacy notice, each in a new tab; sign-in does not", () => {
  const form = (mode: "signin" | "signup", lng: string) => render(<AuthScreen api={{} as never} onSignedIn={() => undefined} initialMode={mode} />, lng);
  const signup = form("signup", "en");
  const line = signup.slice(signup.indexOf('data-testid="signup-terms"'));
  assert.match(line, /<a href="\/terms" target="_blank" rel="noopener"[^>]*data-testid="signup-terms-link"[^>]*>Terms of Service<\/a>/);
  assert.match(line, /<a href="\/privacy" target="_blank" rel="noopener"[^>]*data-testid="signup-privacy-link"[^>]*>Privacy notice<\/a>/);
  assert.ok(text(signup).includes("By signing up you agree to the Terms of Service and have read the Privacy notice."));
  // Above the button that signs up.
  assert.ok(signup.indexOf('data-testid="signup-terms"') < signup.indexOf('data-testid="password-submit"'));
  assert.doesNotMatch(form("signin", "en"), /signup-terms/);
  const cn = form("signup", "zh-CN");
  assert.ok(text(cn).includes("注册即表示你同意服务条款，并已阅读隐私说明。"));
  assert.match(cn, /<a href="\/terms"[^>]*>服务条款<\/a>/);
  assert.match(cn, /<a href="\/privacy"[^>]*>隐私说明<\/a>/);
  assert.ok(text(form("signup", "zh-TW")).includes("註冊即表示你同意服務條款，並已閱讀隱私權聲明。"));
  assert.ok(text(form("signup", "ja")).includes("登録すると、利用規約に同意し、プライバシーについてを読んだものとみなされます。"));
  // On the app host of a hub that keeps the public pages apart, both go to the site host.
  setHostMode({ origin: "wss://droppen.app", app: "https://my.droppen.app", site: "https://www.droppen.app", apexMode: "app" }, "app");
  try {
    const apart = form("signup", "en");
    assert.match(apart, /<a href="https:\/\/www\.droppen\.app\/terms" target="_blank"[^>]*data-testid="signup-terms-link"/);
    assert.match(apart, /<a href="https:\/\/www\.droppen\.app\/privacy" target="_blank"[^>]*data-testid="signup-privacy-link"/);
  } finally {
    setHostMode(null);
  }
  // Every language keeps both links.
  for (const table of [zhCN, zhTW, ja]) {
    const said = (table as Record<string, string>)[SIGN_UP_LINE];
    assert.ok(said, "translated");
    assert.match(said!, /<terms>[^<]+<\/terms>/);
    assert.match(said!, /<privacy>[^<]+<\/privacy>/);
  }
});

test("every string on the page is translated into zh-CN, zh-TW and ja, and the three tables say the same things", () => {
  const keys = pageKeys();
  assert.ok(keys.length > 60);
  for (const lng of LANGS) {
    const table = { ...termsBundles[lng], ...landingBundles[lng], ...app[lng] };
    assert.deepEqual(
      keys.filter((k) => !(k in table)),
      [],
      `${lng} is missing translations`,
    );
    // The same interpolations in every language.
    for (const [key, value] of Object.entries(termsBundles[lng])) {
      const names = (s: string) => [...s.matchAll(/\{\{(\w+)\}\}/g)].map((m) => m[1]).sort();
      assert.deepEqual(names(value), names(key), `${lng}: ${key}`);
    }
  }
  assert.deepEqual(Object.keys(termsBundles["zh-TW"]).sort(), Object.keys(termsBundles["zh-CN"]).sort());
  assert.deepEqual(Object.keys(termsBundles.ja).sort(), Object.keys(termsBundles["zh-CN"]).sort());
  // Nothing in the tables that the page does not say.
  assert.deepEqual(
    Object.keys(termsBundles["zh-CN"]).filter((k) => !keys.includes(k)),
    [],
  );
});

test("the page reads in each language, with nothing left in English", () => {
  const lines = {
    "zh-CN": ["服务条款", "我们承诺什么，也请你遵守什么。", "一人一个账号", "你的设备，你的 AI 工具", "测试版，按现状提供", "离开，以及删除账号", "最后更新：2026-10-23"],
    "zh-TW": ["服務條款", "我們承諾什麼，也請你遵守什麼。", "一人一個帳號", "你的裝置，你的 AI 工具", "測試版，依現狀提供", "離開，以及刪除帳號", "最後更新：2026-10-23"],
    ja: ["利用規約", "私たちがお約束すること、お願いすること。", "1 人に 1 アカウント", "あなたのデバイス、あなたの AI ツール", "ベータ版、現状のままでの提供", "退会とアカウントの削除", "最終更新：2026-10-23"],
  };
  for (const lng of LANGS) {
    const words = text(draw(lng));
    for (const line of lines[lng]) assert.ok(words.includes(line), `${lng} missing: ${line}`);
    assert.doesNotMatch(words, /What you can expect|Acceptable use|as is|Liability|Who we are|devices that run AI|our servers|Last updated/, lng);
  }
});

test("the terms say what the product is: encrypted, the hosted service only, the plans' own numbers, as is, and where to write", () => {
  const html = draw("en");
  const words = text(html);
  const free = PLAN_NUMBERS.free;
  for (const line of [
    "Your sessions and terminals are end-to-end encrypted on your devices, and only your devices hold the keys. We cannot read them",
    "These terms cover only the hosted service at droppen.app",
    "licensed under the AGPL-3.0 (some of their libraries under MIT or Apache-2.0), with their source code at github.com/getdroppen/droppen. Those licences, not these terms, govern the code.",
    "You are responsible for",
    "One account per person and per e-mail inbox",
    "An account still unverified after 14 days cannot add devices",
    `The Free plan includes ${free.hosts} devices that run AI, ${free.devices} devices in all, the last ${free.retentionDays} days readable while a device is off, and ${gb(free.transferBytes)} GB a month through our servers, then slower.`,
    "We may change plans, their limits and their prices.",
    "Use our servers as a general-purpose tunnel, proxy or VPN, or as file storage",
    "it is provided “as is” and “as available”: there is no service-level agreement",
    "Features marked as in testing, such as renewing the identity key of a device that runs AI, may change or be withdrawn.",
    "We may suspend or close an account that breaks these terms",
    "delete your account yourself, in its settings: it is gone at once",
    "These terms are the agreement between you and Droppen (“we”), which operates droppen.app.",
    `You must be at least ${MINIMUM_AGE} years old to use droppen.app.`,
    "Droppen operates droppen.app and the service on it.",
    "These terms are governed by the laws of Singapore, and the courts of Singapore have jurisdiction over any dispute about them. If you are a consumer, you keep the protection of the law where you live.",
    `Report it privately: to ${SECURITY_EMAIL}`,
    "As far as the law allows, our total liability to you is limited to",
    "we tell you by e-mail before it takes effect",
    `Last updated ${TERMS_UPDATED}`,
  ]) {
    assert.ok(words.includes(line), `missing: ${line}`);
  }
  assert.equal(TERMS_UPDATED, "2026-10-23");
  assert.equal(MINIMUM_AGE, 16);
  assert.doesNotMatch(words, /Postal address/, "no postal address is published");
  assert.equal(SECURITY_EMAIL, SECURITY_PAGE_EMAIL, "the security page's address");
  assert.match(html, new RegExp(`<a href="mailto:${PRIVACY_EMAIL.replace(/[[\]]/g, "\\$&")}"[^>]*data-testid="terms-email"`));
  assert.match(html, new RegExp(`<a href="mailto:${SECURITY_EMAIL}"[^>]*data-testid="terms-security-email"`));
  assert.match(html, /<a href="\/pricing"[^>]*data-testid="terms-pricing"/);
  assert.match(html, /<a href="\/privacy"[^>]*data-testid="terms-privacy"/);
  assert.doesNotMatch(html, /data-variant="primary"/, "nothing to do here but read");
});

test("product words, not implementation: no relay, and never a machine or a computer", () => {
  for (const lng of ["en", ...LANGS]) {
    const words = text(draw(lng));
    assert.doesNotMatch(words, /\brelays?\b|hosts\.max|devices\.max|entitlement|中继|中繼|中継|机器|機器|电脑|電腦|マシン|コンピュータ|\bmachines?\b|\bcomputers?\b/i, lng);
    // Never a lower-case "hub": Droppen Hub is a name.
    assert.doesNotMatch(words, /(?<![A-Za-z])hub(?![A-Za-z])/, `${lng}: lower-case hub`);
  }
});

test("no owner placeholder is left on the page, in any language", () => {
  for (const lng of ["en", ...LANGS]) assert.doesNotMatch(draw(lng), /\[[A-Z][A-Z ]*\]/, lng);
  const cn = text(draw("zh-CN"));
  for (const line of ["本条款是你与 Droppen（下称「我们」，droppen.app 的运营方）之间的协议。", "年满 16 岁才能使用 droppen.app。", "本条款受新加坡法律管辖"]) assert.ok(cn.includes(line), `zh-CN missing: ${line}`);
});
