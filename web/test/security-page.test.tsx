// droppen.app/security (`features/landing/security/security.tsx`): how the
// end-to-end encryption works, for a technical reader.
//
// What these pin: `/security` is a public page of its own, signed in or not,
// linked from every public page's footer, from the landing page's security
// section and from the privacy notice; it reads in en, zh-CN, zh-TW and ja
// with every string translated and nothing left in English; its sections come
// in order, each with an anchor the table of contents links, and the identity
// renewal's parts have anchors of their own; what is not live, or still in
// testing, is marked so (the renewal "in testing", switched on at droppen.app
// and tested there, never called stable; a passkey letting a device in on its
// own "not live"), passkeys are described as shipped (sign-in, and a join a
// full device approves by pairing), and the page says
// where the source is; it says plainly that
// a modified page can do anything the browser can, keys included; the
// cross-references are links to anchors that exist; its enumerations are
// lists and its key terms bold, by a markup that leaves nothing unrendered,
// with every translation in its English line's shape; every path in the
// repository it names links there on GitHub; vulnerabilities go to the
// security address; it names no Hub setting.
import * as React from "react";
import assert from "node:assert/strict";
import { readFileSync, statSync } from "node:fs";
import test from "node:test";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { landingBundles } from "../src/features/landing/copy.js";
import DownloadPage from "../src/features/landing/download.js";
import LandingPage from "../src/features/landing/landing.js";
import PricingPage from "../src/features/landing/pricing.js";
import PrivacyPage from "../src/features/landing/privacy.js";
import { bootRoute, frontPage, isAuthPath, isFrontPath, publicPage, SECURITY_PATH } from "../src/features/landing/route.js";
import SecurityPage, { linked, RENEWAL_PARTS, SECURITY_EMAIL, SECURITY_SECTIONS, SECURITY_UPDATED } from "../src/features/landing/security/security.js";
import { buildCommit, repoHref } from "../src/features/landing/security/repo-link.js";
import { securityBundles } from "../src/features/landing/security/security-copy.js";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "../src/i18n/cloud-devices.js";
import { ja } from "../src/i18n/ja.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";

const app = { "zh-CN": { ...cloudDevicesZhCN, ...zhCN }, "zh-TW": { ...cloudDevicesZhTW, ...zhTW }, ja: { ...cloudDevicesJa, ...ja } };
const LANGS = ["zh-CN", "zh-TW", "ja"] as const;
const i18n = i18next.createInstance();
void i18n.use(initReactI18next).init({
  lng: "en",
  resources: {
    en: { translation: {} },
    // As the page adds them: the landing tables first, then its own; the app's wording wins where both have a key.
    ...Object.fromEntries(LANGS.map((lng) => [lng, { translation: { ...securityBundles[lng], ...landingBundles[lng], ...app[lng] } }])),
  },
  interpolation: { escapeValue: false },
  keySeparator: false,
  nsSeparator: false,
});

// The page reads `#…` off the address once mounted; the static render never does.
Object.assign(globalThis, { location: { search: "", pathname: SECURITY_PATH, origin: "https://droppen.app", hash: "" } });

function render(element: React.ReactElement, lng: string): string {
  void i18n.changeLanguage(lng);
  return renderToStaticMarkup(<I18nextProvider i18n={i18n}>{element}</I18nextProvider>);
}
const draw = (lng: string, signedIn = false) => render(<SecurityPage signedIn={signedIn} />, lng);

// A key term's `<strong>` and a link sit inside their sentence: no space for them.
const text = (html: string) =>
  html
    .replace(/<\/?(?:strong|a)\b[^>]*>/g, "")
    // A word that explains itself (`withConcepts`) is part of its sentence.
    .replace(/<span role="button"[^>]*data-concept="[^"]*"[^>]*>([^<]*)<\/span>/g, "$1")
    .replace(/<[^>]+>/g, " ")
    .replace(/&#x27;/g, "'")
    .replace(/&quot;/g, '"')
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/\s+/g, " ");

/** Every `t("…")` key in the page's source: the page and its formulas (`formula.tsx`). */
function pageKeys(): string[] {
  const source = ["security.tsx", "formula.tsx"].map((file) => readFileSync(new URL(`../src/features/landing/security/${file}`, import.meta.url), "utf8")).join("\n");
  return [...new Set([...source.matchAll(/\bt\(\s*"((?:[^"\\]|\\.)*)"/g)].map((m) => JSON.parse(`"${m[1]}"`) as string))];
}

test("`/security` is a public page of its own, signed in or not", () => {
  for (const path of [SECURITY_PATH, `${SECURITY_PATH}/`]) {
    assert.equal(publicPage(path), "security");
    assert.equal(frontPage(path), "security");
    assert.deepEqual(bootRoute(path, false), { show: "security" });
    assert.deepEqual(bootRoute(path, true), { show: "security" });
    assert.ok(isFrontPath(path) && !isAuthPath(path));
  }
  assert.match(draw("en"), /data-testid="security"/);
  // Signed in: the way back into the app instead of sign-in.
  assert.match(draw("en", true), /href="\/sessions"[^>]*>Open app</);
});

test("every public page links it: the footer everywhere, the landing page's security section and the privacy notice", () => {
  const pages = {
    landing: <LandingPage />,
    download: <DownloadPage availability={{ app: true, linux: true }} hints={{ userAgent: "" }} />,
    pricing: <PricingPage api={{ joinWaitlist: async () => undefined }} />,
    privacy: <PrivacyPage />,
    security: <SecurityPage />,
  };
  for (const [name, element] of Object.entries(pages)) {
    assert.match(render(element, "en"), /<a href="\/security"[^>]*data-testid="footer-security"[^>]*>Security<\/a>/, `${name}: the footer`);
  }
  assert.match(render(<LandingPage />, "en"), /<a href="\/security"[^>]*data-testid="landing-security"[^>]*>How the encryption works, in detail<\/a>/);
  assert.match(render(<PrivacyPage />, "en"), /<a href="\/security"[^>]*data-testid="privacy-security"[^>]*>How the encryption works, in detail<\/a>/);
  assert.match(render(<PrivacyPage />, "zh-CN"), /data-testid="privacy-security"[^>]*>加密是怎么做的：详细说明<\/a>/);
  // On its own page the footer link says where you are.
  assert.match(draw("en"), /data-testid="footer-security"/);
});

test("the sections come in order, each with an anchor the table of contents links; the renewal's parts have their own", () => {
  const html = draw("en");
  let at = 0;
  for (const id of SECURITY_SECTIONS) {
    const found = html.indexOf(`id="${id}"`);
    assert.ok(found > at, `section ${id} after the one before`);
    at = found;
  }
  const contents = html.slice(html.indexOf('data-testid="security-contents"'), html.indexOf('id="overview"'));
  assert.deepEqual([...contents.matchAll(/href="#([a-z-]+)"/g)].map((m) => m[1]), [...SECURITY_SECTIONS]);
  const renewal = html.slice(html.indexOf('id="renewal"'), html.indexOf('id="threats"'));
  for (const id of RENEWAL_PARTS) assert.match(renewal, new RegExp(`id="${id}"`), id);
  assert.equal(RENEWAL_PARTS.length, 7);
  // Each heading a reader can find: numbered 01–08 in the contents and on the sections.
  const words = text(html);
  for (const [i, title] of ["Overview", "The device chain", "Keys", "Channels and relays", "Offline copies", "Identity renewal", "Threat model", "Verify it yourself"].entries()) {
    assert.ok(words.includes(`0${i + 1} · ${title}`), `section label: ${title}`);
  }
  assert.ok(words.includes(`Last updated ${SECURITY_UPDATED}`));
});

test("every string on the page is translated into zh-CN, zh-TW and ja, and the three tables say the same things", () => {
  const keys = pageKeys();
  assert.ok(keys.length > 200);
  for (const lng of LANGS) {
    const table = { ...securityBundles[lng], ...landingBundles[lng], ...app[lng] };
    assert.deepEqual(
      keys.filter((k) => !(k in table)),
      [],
      `${lng} is missing translations`,
    );
  }
  assert.deepEqual(Object.keys(securityBundles["zh-TW"]).sort(), Object.keys(securityBundles["zh-CN"]).sort());
  assert.deepEqual(Object.keys(securityBundles.ja).sort(), Object.keys(securityBundles["zh-CN"]).sort());
  // Nothing in the tables that the page does not say.
  assert.deepEqual(
    Object.keys(securityBundles["zh-CN"]).filter((k) => !keys.includes(k)),
    [],
  );
});

test("the page reads in each language, with nothing left in English but the code it quotes", () => {
  const lines = {
    en: ["How Droppen's end-to-end encryption works.", "Your devices hold the keys. Our servers carry ciphertext.", "One signed list decides who is in your account.", "Renewing the identity key of a device that runs AI.", "Don't take our word for it."],
    "zh-CN": ["Droppen 的端到端加密，是怎么做的。", "钥匙在你手里，我们只经手密文。", "谁属于你的账户，一份签名名单说了算。", "更换可运行 AI 的设备的身份密钥。", "别只听我们说。", "测试中"],
    "zh-TW": ["Droppen 的端對端加密，是怎麼做的。", "金鑰在你手上，我們只經手密文。", "更換可執行 AI 的裝置的身分金鑰。", "測試中"],
    ja: ["Droppen のエンドツーエンド暗号化のしくみ。", "鍵はあなたのデバイスに。私たちのサーバーは暗号文を運ぶだけ。", "AI を実行できるデバイスの ID 鍵を更新する。", "テスト中"],
  };
  for (const [lng, want] of Object.entries(lines)) {
    const words = text(draw(lng));
    for (const line of want) assert.ok(words.includes(line), `${lng} missing: ${line}`);
  }
  for (const lng of LANGS) {
    const words = text(draw(lng));
    assert.doesNotMatch(words, /Your devices hold the keys|What it never sees|In testing|Threat model|Verify it yourself|Report a vulnerability|devices? that runs? AI/, lng);
  }
});

test("only what ships is described as live: the renewal is in testing, a passkey letting a device in on its own not live; the source is where it says", () => {
  for (const lng of ["en", ...LANGS]) {
    const html = draw(lng);
    // The renewal's head carries the chip, in the reader's language.
    const head = html.slice(html.indexOf('id="renewal"'), html.indexOf('id="renewal-why"'));
    const chip = { en: "In testing", "zh-CN": "测试中", "zh-TW": "測試中", ja: "テスト中" }[lng]!;
    assert.ok(text(head).includes(chip), `${lng}: the renewal is marked`);
    // So does its line in the table of contents.
    const contents = html.slice(html.indexOf('data-testid="security-contents"'), html.indexOf('id="overview"'));
    assert.ok(text(contents).includes(chip), `${lng}: the contents mark it too`);
  }
  const page = draw("en");
  const words = text(page);
  // Switched on at droppen.app, in testing, and what its test there showed; never called stable, nor "not live" any more.
  const renewal = text(page.slice(page.indexOf('id="renewal"'), page.indexOf('id="renewal-why"')));
  assert.ok(renewal.includes("Switched on at droppen.app since 2026-10-23, in early testing; Droppen 0.2.127 has the web app's side and that of the device that runs AI. It is still being tested"));
  assert.ok(renewal.includes("A test on droppen.app on 2026-10-23 showed:"));
  for (const seen of ["the window was 2 minutes", "was revoked, on relays that can revoke one role's tickets alone; its ticket for running AI stayed valid until the entry was stored", "an entry written before the window was over was refused", "an automatic renewal and one confirmed with the code on the device both went through"]) {
    assert.ok(renewal.includes(seen), seen);
  }
  assert.doesNotMatch(words, /\bstable\b|Not switched on|not live yet, will|renewal in place is not live/i);
  assert.ok(words.includes("In testing: renewing the identity key of a device that runs AI in place"));
  assert.ok(words.includes("Not live: a passkey letting a new device in without a full device's approval"));
  // What has shipped: a passkey's slot lets a new browser ask; a full device approves it by pairing.
  assert.ok(words.includes("It gets no key until a full device pairs with it, by its 8-digit code or QR code, and approves exactly the keys on its screen"));
  // On by default only for a passkey added on a full device (with PRF); the sign-up passkey only once switched on.
  assert.ok(words.includes("when you add one on a full device it does, unless you untick that (it needs a passkey that supports PRF); the passkey you signed up with does only once you switch it on"));
  assert.ok(words.includes("no app writes such a slot"));
  assert.ok(words.includes("A record of the last 30 days' changes to how you sign in"));
  assert.ok(words.includes("A passkey holds no encryption key and unlocks none: a slot only lets a device ask."));
  // No e-mail recovery, password reset or start-over exists.
  assert.doesNotMatch(words, /e-mail recovery|gets you back into the account|password reset/i);
  assert.ok(words.includes("starting the account over is not offered yet"));
  assert.ok(words.includes("their source code is at github.com/getdroppen/droppen. Droppen Hub is not open source."));
  assert.doesNotMatch(words, /will be published|available on request|until the repository is public/i);
  assert.ok(words.includes("Internal security reviews only; no external audit yet."));
  for (const claim of [/unhackable/i, /zero risk/i, /\baudited\b/i, /military[- ]grade/i, /100% secure/i, /is open[- ]source/i, /are open[- ]source/i]) assert.doesNotMatch(words, claim);
});

test("the web-delivery gap is said plainly: a modified page can do anything the browser can, and export its keys", () => {
  const words = text(draw("en"));
  assert.ok(words.includes("WebCrypto's non-extractable keys do not stop code on the same origin"));
  assert.ok(words.includes("Served to you once, do anything this browser can: read all history"));
  assert.ok(words.includes("export this browser's private keys and user keys"));
  // The Hub's own row says it serves the page, and its Cannot list holds only through the protocol.
  assert.ok(words.includes("Serve a modified page to a browser that uses droppen.app"));
  assert.ok(words.includes("Through the protocol alone: decrypt content"));
  assert.ok(words.includes("use the desktop app, signed with Apple's Developer ID and notarised, or the local web app that the droppen program serves from its own binary: neither loads code from Droppen Hub"));
  // The 7-day control bound and its documented limit.
  assert.ok(words.includes("if the Hub also freezes every other full device on that same head, they keep vouching for it, and a removed device keeps control until one of them notices it is frozen"));
  assert.ok(words.includes("and longer if it also freezes every other full device on the same head until one of them notices"));
});

test("the labels it quotes are the protocol's own, and it names no Hub setting or private path", () => {
  const html = draw("en");
  const words = text(html);
  for (const label of ["rh/v1 chain-entry", "rh/v1 chain-attest", "rh/v1 stream-head", "rh/v1 stream-manifest", "rh/v1 host-rotate", "rh/v1 rotate-ack", "rh/v1 seal-auth", "rh/v1 key-commit", "Noise_IK_25519_AESGCM_SHA256"]) {
    assert.ok(words.includes(label), label);
  }
  // The same labels the Rust code signs under.
  const sig = readFileSync(new URL("../../server-rs/crates/rh-proto/src/sig.rs", import.meta.url), "utf8");
  for (const m of words.matchAll(/rh\/v1 ([a-z-]+)(?= ·| rh\/v1|$)/g)) assert.ok(sig.includes(`b"rh/v1 ${m[1]}\\0"`), `sig.rs signs under rh/v1 ${m[1]}`);
  // No Hub setting and no path into the Hub's code (spelled so the open-boundary check does not take the test for one).
  assert.doesNotMatch(html, /(?:DROPPEN|RH)_HUB_|rh[-_]hub/);
});

test("every cross-reference is a link to an anchor on the page, in every language", () => {
  for (const lng of ["en", ...LANGS]) {
    const html = draw(lng);
    assert.doesNotMatch(html, /\[\[|\]\]/, `${lng}: an unrendered reference`);
    const targets = [...html.matchAll(/<a href="#([a-z0-9-]+)"/g)].map((m) => m[1]!);
    assert.ok(targets.length > 15, lng);
    for (const id of targets) assert.match(html, new RegExp(`id="${id}"`), `${lng}: #${id}`);
  }
});

/** Block elements opened inside phrasing-only ones (a `<ul>` in a `<p>`, a `<p>` in a `<span>`): invalid HTML, which a browser re-parents, so hydration would not match. */
function misnested(html: string): string[] {
  const phrasing = new Set(["p", "span", "a", "strong", "em", "code", "label", "button", "h1", "h2", "h3"]);
  const block = new Set(["div", "p", "ul", "ol", "li", "dl", "dt", "dd", "nav", "section", "main", "header", "footer", "h1", "h2", "h3", "table"]);
  const open: string[] = [];
  const bad: string[] = [];
  for (const [, closing, tag, self] of html.matchAll(/<(\/?)([a-z][a-z0-9-]*)\b[^>]*?(\/?)>/g)) {
    if (closing) {
      const at = open.lastIndexOf(tag!);
      if (at >= 0) open.length = at;
      continue;
    }
    if (self || tag === "br" || tag === "img" || tag === "input" || tag === "hr") continue;
    if (block.has(tag!)) {
      const inside = [...open].reverse().find((t) => phrasing.has(t));
      if (inside) bad.push(`<${tag}> in <${inside}>`);
    }
    open.push(tag!);
  }
  return bad;
}

/** A translated line's shape: its lines, list items, key terms and links. */
const shape = (line: string) => ({
  lines: line.split("\n").length,
  items: (line.match(/(?:^|\n)- /g) ?? []).length,
  bold: (line.match(/\*\*/g) ?? []).length / 2,
  links: (line.match(/\[\[/g) ?? []).length,
});

test("the markup: key terms bold, a reason after a term muted, list items in a real list after their lead-in", () => {
  const html = renderToStaticMarkup(<div>{linked("Lead with **a term** and [[a link|t2-6]]:\n- **one** (why, see [[2.6|t2-6]])\n- **二**（理由（内））\n- three\nA closing line.")}</div>);
  assert.equal(
    html,
    '<div><div class="flex flex-col gap-2"><p>Lead with <strong class="font-semibold text-fg">a term</strong> and <a href="#t2-6" class="text-fg underline underline-offset-4 hover:no-underline">a link</a>:</p>' +
      '<ul class="flex flex-col gap-2">' +
      '<li class="flex gap-2.5"><span class="size-1.5 shrink-0 bg-fg mt-2"></span><span class="min-w-0"><strong class="font-semibold text-fg">one</strong> <span class="font-normal text-muted">(why, see <a href="#t2-6" class="text-fg underline underline-offset-4 hover:no-underline">2.6</a>)</span></span></li>' +
      '<li class="flex gap-2.5"><span class="size-1.5 shrink-0 bg-fg mt-2"></span><span class="min-w-0"><strong class="font-semibold text-fg">二</strong><span class="font-normal text-muted">（理由（内））</span></span></li>' +
      '<li class="flex gap-2.5"><span class="size-1.5 shrink-0 bg-fg mt-2"></span><span class="min-w-0">three</span></li>' +
      "</ul><p>A closing line.</p></div></div>",
  );
  // One line stays inline, and a line with no markup stays as it is.
  assert.equal(renderToStaticMarkup(<p>{linked("A **term** here.")}</p>), '<p>A <strong class="font-semibold text-fg">term</strong> here.</p>');
  assert.equal(linked("Plain words (and a parenthesis)."), "Plain words (and a parenthesis).");
});

test("enumerations render as lists and key terms in bold: the Hub's Cannot cell, in en and zh-CN", () => {
  const cases = [
    { lng: "en", lead: "Through the protocol alone:", first: "decrypt content", term: "forge commands", why: " (only devices on the chain are admitted, and the first message asks for nothing)" },
    { lng: "zh-CN", lead: "仅凭协议本身：", first: "解密内容", term: "伪造命令", why: "（只有链上的设备能被准入，首包不带请求）" },
  ];
  for (const { lng, lead, first, term, why } of cases) {
    const html = draw(lng);
    const at = html.indexOf(`<p>${lead}</p><ul`);
    assert.ok(at > 0, `${lng}: the lead-in, then a list`);
    // The whole of a table cell, which is a block, not a span: its (narrow-screen) head, then the lead-in and the list.
    assert.match(html.slice(html.lastIndexOf('<div role="cell"', at), at), /^<div role="cell"[^>]*><span[^>]*>[^<]+<\/span><div class="flex flex-col gap-1\.5">$/, lng);
    const list = html.slice(at, html.indexOf("</ul>", at));
    assert.equal(list.match(/<li\b/g)?.length, 6, `${lng}: six items`);
    assert.match(list, new RegExp(`<li[^>]*><span[^>]*></span><span[^>]*><strong[^>]*>${first}</strong></span></li>`), `${lng}: ${first}`);
    const reason = why.trimStart();
    assert.ok(list.includes(`>${term}</strong>${why.startsWith(" ") ? " " : ""}<span class="font-normal text-muted">${reason}</span>`), `${lng}: ${term}, its reason muted`);
  }
});

test("the markup is all rendered in every language, as valid HTML", () => {
  for (const lng of ["en", ...LANGS]) {
    const html = draw(lng);
    assert.doesNotMatch(html, /\*\*/, `${lng}: an unrendered **`);
    assert.doesNotMatch(html, /\[\[|\]\]/, `${lng}: an unrendered reference`);
    assert.doesNotMatch(html, /\n- |>- /, `${lng}: an unrendered list item`);
    assert.deepEqual(misnested(html), [], `${lng}: a block inside a <p> or a <span>`);
    // Lists and bold terms are there in this language too.
    assert.ok((html.match(/<strong\b/g) ?? []).length > 100, `${lng}: key terms`);
    assert.ok((html.match(/<ul\b/g) ?? []).length > 25, `${lng}: lists`);
  }
});

test("every translation has its English line's shape: the same lines, list items, key terms and links", () => {
  const keys = pageKeys();
  assert.ok(keys.filter((k) => k.includes("\n- ") || k.startsWith("- ")).length >= 25);
  assert.ok(keys.filter((k) => k.includes("**")).length >= 100);
  for (const lng of LANGS) {
    for (const key of keys) {
      const line = securityBundles[lng][key];
      if (line !== undefined) assert.deepEqual(shape(line), shape(key), `${lng}: ${key.slice(0, 80)}`);
    }
  }
});

const REPO = "https://github.com/getdroppen/droppen";

test("every repository path on the page links to it on GitHub, at `main` until the served build names its commit; the repository's name links to the repository", () => {
  // Which commit a `/.well-known/droppen-build` answer names, and where a path links at a revision.
  assert.equal(buildCommit({ commit: "0123abcd", repo: "getdroppen/droppen", web: { digest: "sha256:00" } }), "0123abcd");
  for (const answer of [{ commit: null, repo: "getdroppen/droppen" }, { commit: "main", repo: "getdroppen/droppen" }, { commit: "0123abcd/../x", repo: "getdroppen/droppen" }, { commit: "0123ab", repo: "getdroppen/droppen" }, {}, null, "0123abcd"]) assert.equal(buildCommit(answer), null);
  // A commit the answer does not tie to this repository (a Hub's own, say) would lead nowhere: `main` instead.
  for (const repo of [undefined, "someone/fork", "getdroppen/droppen-fork"]) assert.equal(buildCommit({ commit: "0123abcd", repo }), null, String(repo));
  assert.equal(repoHref("scripts/verify-web.mjs", "0123abcd"), `${REPO}/blob/0123abcd/scripts/verify-web.mjs`);
  assert.equal(repoHref("server-rs/crates/rh-proto", "main"), `${REPO}/tree/main/server-rs/crates/rh-proto`);
  assert.equal(repoHref("/", "0123abcd"), REPO);

  const root = new URL("../../", import.meta.url);
  for (const lng of ["en", ...LANGS]) {
    // Up to the footer, whose own "Source code" link is not a path.
    const html = draw(lng);
    const page = html.slice(0, html.lastIndexOf("<a ", html.indexOf('data-testid="footer-source"')));
    const links = [...page.matchAll(/<a href="(https:\/\/github\.com\/[^"]*)"[^>]*>([^<]*)<\/a>/g)].map(([, href, label]) => ({ href: href!, label: label! }));
    const paths = links.filter((l) => l.href !== REPO);
    assert.ok(paths.length >= 11, `${lng}: ${paths.length} paths`);
    for (const { href, label } of paths) {
      // A static render never asks the server: `main`; `tree/` for a directory, `blob/` for a file, that exists.
      const [, kind, path] = href.match(/^https:\/\/github\.com\/getdroppen\/droppen\/(tree|blob)\/main\/(.+)$/) ?? [];
      assert.ok(kind && path, `${lng}: ${href}`);
      assert.ok(path.endsWith(label), `${lng}: "${label}" links ${path}`);
      assert.equal(kind, statSync(new URL(path, root)).isDirectory() ? "tree" : "blob", path);
    }
    // The repository's name, wherever the page gives it (the source in 8.3, its Security tab in 8.6).
    const repo = links.filter((l) => l.href === REPO);
    assert.equal(repo.length, 2, lng);
    assert.ok(repo.every((l) => l.label === "github.com/getdroppen/droppen"), lng);
    // No path-looking word left unlinked: only the protocol's labels and a release's asset, which are not paths in it.
    const words = text(page.replace(/<a href="https:\/\/github\.com\/[^"]*"[^>]*>[^<]*<\/a>/g, " "));
    const left = [...words.matchAll(/(?<![\w./-])(?:[\w-]+\/)+[\w.-]+|[\w-]+\.(?:md|mjs|sh|json|rs|tsx?)\b|github\.com\/\S+/g)].map((m) => m[0]);
    assert.deepEqual([...new Set(left)].filter((w) => w !== "rh/v1" && w !== "web-manifest.json"), [], lng);
  }
});

test("a vulnerability report has somewhere to go", () => {
  assert.equal(SECURITY_EMAIL, "security@droppen.app");
  const html = draw("en");
  assert.match(html, new RegExp(`<a href="mailto:${SECURITY_EMAIL}"[^>]*data-testid="security-email"`));
  assert.ok(text(html).includes("GitHub's private vulnerability reporting"));
});
