// droppen.app/pricing (`features/landing/pricing.tsx`), its waitlist form
// (`waitlist.tsx`) and the privacy notice (`privacy.tsx`).
//
// What these pin: the page leads with what you get and reads in zh-CN, en,
// zh-TW and ja; its numbers are the plans' own (`pricing-plans.ts`, which
// the hub's own tests hold to its plans) and Pro is $10 a month or $96 a
// year; Pro's button is "Join the waitlist" — no checkout anywhere — and,
// like Free's, it brings you to the form with that plan chosen; one primary
// action in each part of the page; the form says how it went (on the list;
// not an address; a throwaway address; too many tries, and for how long;
// out of reach) and sends nothing it cannot; the hub client posts
// `/v1/waitlist` without a session; `/pricing` and `/privacy` are public
// pages signed in or not, linked from every public page's header or footer;
// the privacy notice says what is kept, why, how long, how to be removed, and
// what our servers can and cannot see.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { landingBundles } from "../src/features/landing/copy.js";
import DownloadPage from "../src/features/landing/download.js";
import LandingPage from "../src/features/landing/landing.js";
import PricingPage, { planOfSearch } from "../src/features/landing/pricing.js";
import { gb, PLAN_NUMBERS, PRO_PRICE, THROTTLED_KB_PER_SEC, yearlySaving } from "../src/features/landing/pricing-plans.js";
import PrivacyPage, { PRIVACY_UPDATED } from "../src/features/landing/privacy.js";
import { bootRoute, frontPage, isAuthPath, isFrontPath, PRICING_PATH, PRIVACY_PATH, publicPage } from "../src/features/landing/route.js";
import { looksLikeEmail, PRIVACY_EMAIL, submitWaitlist, type WaitlistState, waitlistProblem } from "../src/features/landing/waitlist.js";
import { HubApi, HubApiError, type WaitlistPlan } from "../src/lib/hub/api.js";
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

// The page reads `?plan=` off the address.
const place = { search: "", pathname: "/pricing", origin: "https://droppen.app", hash: "" };
Object.assign(globalThis, { location: place });

/** A hub that never answers: the static render never sends. */
const silent = { joinWaitlist: async () => undefined };

function draw(lng: string, opts: { signedIn?: boolean; waitlist?: WaitlistState; search?: string } = {}): string {
  void i18n.changeLanguage(lng);
  place.search = opts.search ?? "";
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <PricingPage signedIn={opts.signedIn} api={silent} waitlist={opts.waitlist} />
    </I18nextProvider>,
  );
}

function drawPrivacy(lng: string): string {
  void i18n.changeLanguage(lng);
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <PrivacyPage />
    </I18nextProvider>,
  );
}

const text = (html: string) =>
  html
    .replace(/<[^>]+>/g, " ")
    .replace(/&#x27;/g, "'")
    .replace(/&quot;/g, '"')
    .replace(/&amp;/g, "&")
    .replace(/\s+/g, " ");
const primaries = (html: string) => (html.match(/data-variant="primary"/g) ?? []).length;
/** The part of `html` from `id="from"` to `id="to"`. */
const part = (html: string, from: string, to?: string) => {
  const start = html.indexOf(`id="${from}"`);
  assert.ok(start > 0, from);
  return html.slice(start, to ? html.indexOf(`id="${to}"`, start) : undefined);
};

test("the pricing page leads with what you get, in zh-CN first", () => {
  const words = text(draw("zh-CN"));
  for (const line of [
    "先免费用起来，不够再升 Pro。",
    "每个方案都端到端加密，哪个方案我们都看不到你的代码。",
    "仅本地",
    "免费版",
    "不管人在哪，手机上都能接着用 AI。",
    "免费版的全部，再加上：",
    "3 台可运行 AI 的设备",
    "20 台可运行 AI 的设备",
    "设备总共 8 台，手机、浏览器都算在内",
    "设备关机时，手机上能看最近 7 天的会话",
    "设备关机时，手机上能看最近 30 天的会话",
    "每月经我们服务器转发 10 GB，超出后限速",
    "每月经我们服务器转发 100 GB，超出部分按量计费，不限速",
    "或每年 $96，省 20%",
    "加入等候名单",
    "每个方案包含什么",
    "留下邮箱，轮到你时我们写信给你。",
    "每月的流量用完了会怎样？",
    "「仅本地」是什么意思？",
    "可以自己部署吗？",
    "什么时候能付费开通 Pro？",
  ]) {
    assert.ok(words.includes(line), `missing: ${line}`);
  }
  assert.match(words, /\$10 美元 \/ 月/);
});

test("in English, and every number on the page is a plan's own", () => {
  const html = draw("en");
  const words = text(html);
  const { free, pro } = PLAN_NUMBERS;
  for (const line of [
    "Start free. Go Pro when you need more.",
    "Local only",
    "Everything in Free, plus:",
    `${free.hosts} devices that run AI`,
    `${pro.hosts} devices that run AI`,
    `${free.devices} devices in all, phones and browsers included`,
    `${pro.devices} devices in all, phones and browsers included`,
    `Read the last ${free.retentionDays} days on your phone while a device is off`,
    `Read the last ${pro.retentionDays} days on your phone while a device is off`,
    `${gb(free.transferBytes)} GB a month through our servers, then slower`,
    `${gb(pro.transferBytes)} GB a month through our servers, then billed by use, at full speed`,
    `$${PRO_PRICE.monthly} USD / month`,
    `or $${PRO_PRICE.yearly} a year: save ${yearlySaving()}%`,
    `$${PRO_PRICE.monthly} a month, or $${PRO_PRICE.yearly} a year`,
    `past ${gb(free.transferBytes)} GB that traffic slows to ${THROTTLED_KB_PER_SEC} KB/s`,
    `past ${gb(pro.transferBytes)} GB is billed by use`,
    "End-to-end encrypted: we can't see your code",
    "Prices in US dollars.",
  ]) {
    assert.ok(words.includes(line), `missing: ${line}`);
  }
  // The user's decision of 2026-10-19: $10 a month, $96 a year.
  assert.deepEqual(PRO_PRICE, { monthly: 10, yearly: 96 });
  assert.equal(yearlySaving(), 20);
  // Every plan says it is end-to-end encrypted: the compare table's last row has a mark in each column.
  const compare = part(html, "compare", "waitlist");
  const encrypted = compare.slice(compare.lastIndexOf('role="rowheader"'));
  assert.equal((encrypted.match(/>included</g) ?? []).length, 3);
  // Section ids the header's anchors and the cards' buttons go to.
  for (const id of ["plans", "compare", "waitlist", "faq"]) assert.match(html, new RegExp(`id="${id}"`));
});

test("nothing can be bought yet: Pro's button joins the waitlist, and no page offers a checkout", () => {
  for (const lng of ["en", "zh-CN", "zh-TW", "ja"]) {
    const html = draw(lng);
    assert.match(html, /<a href="#waitlist"[^>]*data-variant="primary"[^>]*data-testid="plan-pro-waitlist"/, lng);
    assert.match(html, /<a href="#waitlist"[^>]*data-testid="plan-free-waitlist"/, lng);
    assert.doesNotMatch(html, /stripe|checkout|buy now|subscribe/i, lng);
  }
  const words = text(draw("en"));
  assert.ok(words.includes("Payments are not open yet: we'll e-mail you when they are."));
  assert.ok(words.includes("Join the waitlist"));
});

test("one primary action in each part of the page: Pro's among the plans, the form's own below the comparison", () => {
  for (const lng of ["en", "zh-CN"]) {
    const html = draw(lng);
    assert.equal(primaries(html), 2, lng);
    assert.equal(primaries(part(html, "plans", "compare")), 1, `${lng}: the plans`);
    assert.equal(primaries(part(html, "compare", "waitlist")), 0, `${lng}: the comparison keeps them apart`);
    assert.equal(primaries(part(html, "waitlist", "faq")), 1, `${lng}: the form`);
    assert.match(part(html, "waitlist", "faq"), /<button type="submit" data-variant="primary" data-testid="waitlist-submit"/);
  }
});

test("the form starts on Free, or on Pro when the address says so; both cards' buttons choose their plan", () => {
  assert.equal(planOfSearch(""), "free");
  assert.equal(planOfSearch("?plan=pro"), "pro");
  assert.equal(planOfSearch("?plan=internal"), "free");
  const checked = (html: string) => /<input type="radio"[^>]*name="plan" checked=""[^>]*value="(\w+)"/.exec(html)?.[1];
  assert.equal(checked(draw("en")), "free");
  assert.equal(checked(draw("en", { search: "?plan=pro" })), "pro");
  // The plan choice is two native radios in a fieldset, named for screen readers.
  const html = draw("en");
  assert.equal((html.match(/<input type="radio"[^>]*name="plan"/g) ?? []).length, 2);
  assert.match(html, /<fieldset[^>]*><legend[^>]*><span[^>]*>Plan<\/span><\/legend>/);
  assert.match(html, /<input id="waitlist-email" type="email" autoComplete="email" inputMode="email" required=""[^>]*name="email"/);
  // The form links the privacy notice.
  assert.match(html, /<a href="\/privacy"[^>]*data-testid="waitlist-privacy"/);
});

test("signed out, Free offers sign-up with an invite; signed in, the way back into the app", () => {
  const out = draw("en");
  assert.match(out, /<a href="\/signup"[^>]*data-testid="plan-free-signup"[^>]*>Have an invite code\? Sign up<\/a>/);
  assert.match(out, /<a href="\/download"[^>]*data-testid="plan-local-download"/);
  const signedIn = draw("en", { signedIn: true });
  assert.doesNotMatch(signedIn, /href="\/sign(in|up)"/);
  assert.match(signedIn, /<a href="\/sessions"[^>]*data-testid="plan-free-open"[^>]*>Open app<\/a>/);
  // Pro's waitlist stays: an account cannot buy Pro yet either.
  assert.match(signedIn, /data-testid="plan-pro-waitlist"/);
});

test("joined: the form says you're on the list and what comes next, in each language", () => {
  const joined = (lng: string, plan: WaitlistPlan) => text(draw(lng, { waitlist: { kind: "done", plan } }));
  assert.ok(joined("en", "free").includes("You're on the list: we'll e-mail you an invite."));
  assert.ok(joined("en", "pro").includes("You're on the list: we'll e-mail you an invite when Pro opens."));
  assert.ok(joined("en", "pro").includes(`To be taken off the list, write to ${PRIVACY_EMAIL}.`));
  assert.ok(joined("zh-CN", "free").includes("你已在名单上，我们会把邀请发到你的邮箱。"));
  assert.ok(joined("zh-CN", "pro").includes("你已在名单上，Pro 开放时我们会把邀请发到你的邮箱。"));
  assert.ok(joined("zh-TW", "free").includes("你已在名單上，我們會把邀請寄到你的信箱。"));
  assert.ok(joined("ja", "pro").includes("登録しました。Pro が始まったら招待をメールでお送りします。"));
  const html = draw("en", { waitlist: { kind: "done", plan: "pro" } });
  assert.match(html, /role="status" data-testid="waitlist-done" data-plan="pro"/);
  assert.doesNotMatch(html, /data-testid="waitlist-form"/, "the form gives way to the answer");
});

test("refused: each reason in words, as an alert by the field", () => {
  const refused = (lng: string, state: WaitlistState) => {
    const html = draw(lng, { waitlist: state });
    assert.match(html, /role="alert" data-signal="failed" data-testid="waitlist-error"/);
    assert.match(html, /aria-describedby="waitlist-error"/);
    return text(html);
  };
  assert.ok(refused("en", { kind: "error", reason: "email" }).includes("That does not look like an e-mail address. Check it and try again."));
  assert.ok(refused("en", { kind: "error", reason: "disposable" }).includes("Use an address you keep: throwaway mail providers are not accepted."));
  assert.ok(refused("en", { kind: "error", reason: "rate", retryMinutes: 12 }).includes("Too many tries from your network. Try again in 12 min."));
  assert.ok(refused("en", { kind: "error", reason: "unreachable" }).includes("Could not reach Droppen. Check your connection and try again."));
  assert.ok(refused("zh-CN", { kind: "error", reason: "rate", retryMinutes: 3 }).includes("你所在的网络尝试次数太多，请 3 分钟后再试。"));
  assert.ok(refused("zh-CN", { kind: "error", reason: "email" }).includes("这看起来不像邮箱地址，检查一下再试。"));
  assert.ok(refused("ja", { kind: "error", reason: "disposable" }).includes("使い捨てメールは受け付けていません。"));
  assert.match(draw("en", { waitlist: { kind: "error", reason: "email" } }), /aria-invalid="true"/);
  assert.doesNotMatch(draw("en", { waitlist: { kind: "error", reason: "unreachable" } }), /aria-invalid/);
  // Sending: the button waits.
  assert.match(draw("en", { waitlist: { kind: "sending" } }), /data-testid="waitlist-submit" disabled=""/);
});

test("submitting: what is not an address is never sent; the hub's refusals become reasons", async () => {
  const sent: unknown[] = [];
  const ok = { joinWaitlist: async (input: unknown) => void sent.push(input) };
  assert.deepEqual(await submitWaitlist(ok, { email: "not an address", plan: "free" }), { kind: "error", reason: "email" });
  assert.deepEqual(await submitWaitlist(ok, { email: "", plan: "pro" }), { kind: "error", reason: "email" });
  assert.equal(sent.length, 0);
  assert.deepEqual(await submitWaitlist(ok, { email: "  ada@example.com ", plan: "pro", locale: "zh-CN" }), { kind: "done", plan: "pro" });
  assert.deepEqual(sent, [{ email: "ada@example.com", plan: "pro", locale: "zh-CN" }]);
  const refusing = (error: unknown) => ({
    joinWaitlist: async () => {
      throw error;
    },
  });
  assert.deepEqual(await submitWaitlist(refusing(new HubApiError(429, "rate", "too many", { retryAfterMs: 11 * 60_000 + 1 })), { email: "a@b.co", plan: "free" }), {
    kind: "error",
    reason: "rate",
    retryMinutes: 12,
  });
  assert.deepEqual(waitlistProblem(new HubApiError(429, "rate", "too many")), { reason: "rate", retryMinutes: 1 });
  assert.deepEqual(waitlistProblem(new HubApiError(400, "disposable_email", "x")), { reason: "disposable" });
  assert.deepEqual(waitlistProblem(new HubApiError(400, "bad_request", "that is not an e-mail address")), { reason: "email" });
  assert.deepEqual(waitlistProblem(new HubApiError(503, "unavailable", "x")), { reason: "unreachable" });
  assert.deepEqual(waitlistProblem(new TypeError("Failed to fetch")), { reason: "unreachable" });
  for (const good of ["a@b.co", "First.Last+droppen@gmail.com", " x@sub.example.org "]) assert.ok(looksLikeEmail(good), good);
  for (const bad of ["", "a", "a@b", "a b@c.de", "@c.de", "a@.de", `${"a".repeat(250)}@c.de`]) assert.ok(!looksLikeEmail(bad), bad);
});

test("the hub client joins with POST /v1/waitlist, with no session and only what was given", async () => {
  const sent: { url: string; method: string; headers: Record<string, string>; body: unknown }[] = [];
  const answers = [{ status: 200, body: { ok: true } }, { status: 429, body: { error: "rate", message: "too many", retry_after_ms: 90_000 } }];
  const fetch = async (input: string, init?: RequestInit): Promise<Response> => {
    sent.push({ url: input, method: init?.method ?? "GET", headers: (init?.headers ?? {}) as Record<string, string>, body: init?.body ? JSON.parse(init.body as string) : undefined });
    const a = answers.shift()!;
    return new Response(JSON.stringify(a.body), { status: a.status, headers: { "content-type": "application/json" } });
  };
  const api = new HubApi("https://hub.test", fetch);
  await api.joinWaitlist({ email: "ada@example.com", plan: "pro", locale: "ja" });
  assert.deepEqual(sent[0], {
    url: "https://hub.test/v1/waitlist",
    method: "POST",
    headers: { "content-type": "application/json" },
    body: { email: "ada@example.com", plan: "pro", locale: "ja" },
  });
  await assert.rejects(api.joinWaitlist({ email: "ada@example.com", plan: "free" }), (e: unknown) => e instanceof HubApiError && e.status === 429 && e.retryAfterMs === 90_000);
  assert.deepEqual(sent[1]!.body, { email: "ada@example.com", plan: "free" }, "no locale when none is given");
});

test("the page reads in zh-TW and ja, with nothing left in English", () => {
  const tw = text(draw("zh-TW"));
  for (const line of ["先免費用起來，不夠再升 Pro。", "僅本地", "加入等候名單", "3 台可執行 AI 的裝置", "每個方案包含什麼", "可以自己架設嗎？"]) assert.ok(tw.includes(line), `zh-TW missing: ${line}`);
  const jp = text(draw("ja"));
  for (const line of ["まずは無料で。足りなくなったら Pro へ。", "ローカルのみ", "ウェイトリストに登録", "AI を実行できるデバイス 3 台", "各プランの内容", "セルフホストできますか？"]) assert.ok(jp.includes(line), `ja missing: ${line}`);
  for (const words of [tw, jp, text(draw("zh-CN"))]) {
    assert.doesNotMatch(words, /Join the waitlist|Start free|Local only|Everything in Free|What each plan includes|Before you choose|devices that run AI/);
  }
});

test("product words, not implementation: no relay, no plan keys, and never a machine or a computer", () => {
  for (const lng of ["en", "zh-CN", "zh-TW", "ja"]) {
    for (const words of [text(draw(lng)), text(drawPrivacy(lng))]) {
      assert.doesNotMatch(words, /\brelays?\b|hosts\.max|devices\.max|entitlement|中继|中繼|中継|机器|機器|电脑|電腦|マシン|コンピュータ|\bmachines?\b|\bcomputers?\b/i, lng);
      // Never a lower-case "hub": Droppen Hub is a name.
      assert.doesNotMatch(words, /(?<![A-Za-z])hub(?![A-Za-z])/, `${lng}: lower-case hub`);
    }
  }
});

test("`/pricing` and `/privacy` are public pages of their own, signed in or not, linked from every public page", () => {
  for (const [path, page] of [
    [PRICING_PATH, "pricing"],
    [PRIVACY_PATH, "privacy"],
  ] as const) {
    assert.equal(publicPage(path), page);
    assert.equal(publicPage(`${path}/`), page);
    assert.equal(frontPage(path), page);
    assert.deepEqual(bootRoute(path, false), { show: page });
    assert.deepEqual(bootRoute(path, true), { show: page });
    assert.ok(isFrontPath(path) && !isAuthPath(path));
  }
  assert.equal(frontPage("/sessions"), null);
  assert.equal(frontPage("/signin"), null);
  void i18n.changeLanguage("en");
  const pages = {
    landing: <LandingPage />,
    download: <DownloadPage availability={{ app: true, linux: true }} hints={{ userAgent: "" }} />,
    pricing: <PricingPage api={silent} />,
    privacy: <PrivacyPage />,
  };
  for (const [name, element] of Object.entries(pages)) {
    const html = renderToStaticMarkup(<I18nextProvider i18n={i18n}>{element}</I18nextProvider>);
    assert.match(html, /<a href="\/pricing"[^>]*data-testid="header-pricing"[^>]*>Pricing<\/a>/, `${name}: the header`);
    assert.match(html, /<a href="\/pricing"[^>]*data-testid="footer-pricing"/, `${name}: the footer`);
    assert.match(html, /<a href="\/privacy"[^>]*data-testid="footer-privacy"[^>]*>Privacy notice<\/a>/, `${name}: the footer`);
  }
  // Below 640px the header has no room for it: there, the landing page's hero offers the waitlist itself.
  assert.match(renderToStaticMarkup(<I18nextProvider i18n={i18n}><LandingPage /></I18nextProvider>), /<a href="\/pricing#waitlist"[^>]*data-testid="hero-waitlist"[^>]*>No code yet\? Join the waitlist<\/a>/);
  assert.doesNotMatch(renderToStaticMarkup(<I18nextProvider i18n={i18n}><LandingPage signedIn /></I18nextProvider>), /data-testid="hero-waitlist"/);
  void i18n.changeLanguage("zh-CN");
  const cn = renderToStaticMarkup(<I18nextProvider i18n={i18n}><LandingPage /></I18nextProvider>);
  assert.match(cn, /data-testid="header-pricing"[^>]*>定价<\/a>/);
  assert.match(cn, /data-testid="footer-privacy"[^>]*>隐私说明<\/a>/);
  assert.match(cn, /data-testid="hero-waitlist"[^>]*>还没有邀请码？加入等候名单<\/a>/);
});

test("the privacy notice: what the waitlist keeps and why, for how long, how to be removed, and what our servers can and cannot see", () => {
  const html = drawPrivacy("en");
  const words = text(html);
  for (const line of [
    "Your e-mail address, the plan you chose (Free or Pro), the language of the page and when you joined.",
    "We use them only to e-mail you an invite, or to tell you when Pro opens.",
    "Not your IP address, nor anything else about your browser.",
    "never more than 12 months after you last joined: older entries are deleted automatically.",
    `To be taken off, write to ${PRIVACY_EMAIL} from that address and we will delete it.`,
    "Your sessions and terminals are encrypted on your devices, and only your devices hold the keys.",
    "Which of your devices are online, when they connect and from which IP address",
    "What kind of device each one is and which browser, as a short name such as “iPhone · Chrome” made from its User-Agent; it does not keep the User-Agent itself",
    "What is said in a session, or typed and shown in a terminal",
    "The names of your devices, your files and their names",
    // Stored data is padded; live connections are not (as /security says).
    "What we store is padded to fixed sizes; live connections are not, so while our servers carry one they see its timing, its volume and the rhythm of typing in a terminal",
    `Last updated ${PRIVACY_UPDATED}`,
    // Deleting an account: what goes, and what is kept, for how long.
    "If you delete your account",
    "It is gone at once and for good: nobody, us included, can bring it back.",
    "Your address also leaves the waitlist, if it was on it.",
    "A keyed hash of your e-mail address, never the address itself, for 7 days, so the same inbox cannot sign up again straight away. How long is a setting of Droppen Hub: 7 days on droppen.app.",
    "For the same 7 days, your account's and your devices' random ids and your devices' public keys, which open nothing",
  ]) {
    assert.ok(words.includes(line), `missing: ${line}`);
  }
  assert.doesNotMatch(words, /in padded sizes/);
  assert.doesNotMatch(words, /a matter of minutes/, "the ids are kept as long as the address's hash, not minutes");
  assert.equal(PRIVACY_UPDATED, "2026-10-23");
  assert.match(html, new RegExp(`<a href="mailto:${PRIVACY_EMAIL}"`));
  assert.equal(primaries(html), 0, "nothing to do here but read");
  const cn = text(drawPrivacy("zh-CN"));
  for (const line of ["我们保存什么，又看不到什么。", "不保存你的 IP 地址", "最长不超过你最后一次加入后的 12 个月", "我们的服务器看得到", "看不到", "如果你删除了账号", "保留 7 天"]) assert.ok(cn.includes(line), `zh-CN missing: ${line}`);
  for (const lng of ["zh-TW", "ja"]) assert.doesNotMatch(text(drawPrivacy(lng)), /What we keep|If you joined the waitlist|What our servers can see|If you delete your account|A keyed hash/, lng);
});
