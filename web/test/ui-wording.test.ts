// The interface calls every device a device (docs/local-aggregator.md §3.1
// 用词, §6; the user's decision): 「设备」 / 「裝置」 / 「デバイス」 /
// "device", a device that runs AI included — 「可运行 AI 的设备」 where it has
// to be told apart. "Machine" and "computer" stay in the code (`Machine*`,
// `mq`, `machine` params), never on screen, in any of the four languages the
// app ships: English (the keys themselves, `i18n/index.ts`), zh-CN, zh-TW, ja.
// The device a page runs on included (the user's decision of 2026-10-18):
// "this device", 「这台设备」 / 「這台裝置」 / 「このデバイス」 — never "this
// computer", 本机 / 本機 or このコンピュータ — and "local" is 本地.
//
// Checked: every translation table, and every piece of text the interface
// source puts on screen — JSX text, the `aria-label` / `title` / `placeholder`
// / `alt` attributes, `{"…"}` and template literals in JSX, `t("…")` keys —
// read from the TypeScript syntax tree, so comments and identifiers never
// count. Chinese and Japanese words are looked for in every string literal.
//
// Known limits of the English check: a constant (or a template) holding the
// words and rendered somewhere else, and `new Error("…")` messages shown
// through `errorText`, are not followed — the error messages a device's
// channel and an enrollment end in are i18n keys, checked below.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "../src/i18n/cloud-devices.js";
import { crossDeviceJa, crossDeviceZhCN, crossDeviceZhTW } from "../src/i18n/cross-device.js";
import { ja } from "../src/i18n/ja.js";
import { linkJa, linkZhCN, linkZhTW } from "../src/i18n/link.js";
import { onboardingJa, onboardingZhCN, onboardingZhTW } from "../src/i18n/onboarding.js";
import { setupJa, setupZhCN, setupZhTW } from "../src/i18n/setup.js";
import { signOutJa, signOutZhCN, signOutZhTW } from "../src/i18n/sign-out.js";
import { hostsJa, hostsZhCN, hostsZhTW } from "../src/i18n/hosts.js";
import { moveJa, moveZhCN, moveZhTW } from "../src/i18n/move.js";
import { welcomeJa, welcomeZhCN, welcomeZhTW } from "../src/i18n/welcome.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";
import { downloadJa, downloadZhCN, downloadZhTW } from "../src/features/landing/download-copy.js";
import { pricingJa, pricingZhCN, pricingZhTW } from "../src/features/landing/pricing-copy.js";
import { securityJa, securityZhCN, securityZhTW } from "../src/features/landing/security/security-copy.js";
import { termsJa, termsZhCN, termsZhTW } from "../src/features/landing/terms/terms-copy.js";
import { sourceFiles, sourceTexts } from "./screen-text.js";

/**
 * Source not checked, with why: the landing page's selling copy
 * (`features/landing/copy.ts` and the sections that draw it) is marketing the
 * user approved, about the computer the AI CLIs run on (「手机上接着用电脑上的
 * AI」), not the interface; the i18n tables are checked as tables below. The
 * download page, the pricing page with its waitlist, the privacy notice, the
 * security design, the terms of service and the frame around them all
 * (`download*.ts(x)`, `pricing*.ts(x)`, `waitlist.tsx`, `privacy.tsx`,
 * `security/*`, `terms/*`, `page-kit.tsx`) are the product's own words, and are checked like any other
 * interface source — their tables below.
 */
const LANDING_MARKETING = ["copy.ts", "landing.tsx", "hero-a.tsx", "hero-diagram.tsx", "journey.tsx", "direct-path.tsx", "offline-copy.tsx", "kit.tsx", "motion.ts", "story.ts"];
const NOT_INTERFACE = [...LANDING_MARKETING.map((file) => `features/landing/${file}`), "i18n/"];

const words = (s: string) => s.replace(/\{\{[^}]*\}\}/g, "");
const ENGLISH = { test: (s: string) => /\bmachines?\b|\bcomputers?\b/i.test(s) || /\bPCs?\b/.test(s) };
// 本机 / 本機 is "this machine": the device the page runs on is 「这台设备」 / 「這台裝置」, and "local" is 本地.
const CJK = /机器|機器|マシン|电脑|電腦|コンピュータ|端末|本机|本機/;

const ZH_CN = /机器|电脑|本机/;
const ZH_TW = /機器|電腦|本機/;
const JA = /マシン|コンピュータ|端末/;

const TABLES = {
  "zh-CN": { table: { ...onboardingZhCN, ...crossDeviceZhCN, ...linkZhCN, ...cloudDevicesZhCN, ...signOutZhCN, ...hostsZhCN, ...moveZhCN, ...setupZhCN, ...welcomeZhCN, ...zhCN }, forbidden: ZH_CN },
  "zh-TW": { table: { ...onboardingZhTW, ...crossDeviceZhTW, ...linkZhTW, ...cloudDevicesZhTW, ...signOutZhTW, ...hostsZhTW, ...moveZhTW, ...setupZhTW, ...welcomeZhTW, ...zhTW }, forbidden: ZH_TW },
  ja: { table: { ...onboardingJa, ...crossDeviceJa, ...linkJa, ...cloudDevicesJa, ...signOutJa, ...hostsJa, ...moveJa, ...setupJa, ...welcomeJa, ...ja }, forbidden: JA },
};

/** The download page's own table (`features/landing/download-copy.ts`): the product's words, not the landing's selling copy. */
const DOWNLOAD = { "zh-CN": { table: downloadZhCN, forbidden: ZH_CN }, "zh-TW": { table: downloadZhTW, forbidden: ZH_TW }, ja: { table: downloadJa, forbidden: JA } };
/** The pricing page's, the waitlist's and the privacy notice's (`features/landing/pricing-copy.ts`): product words too. */
const PRICING = { "zh-CN": { table: pricingZhCN, forbidden: ZH_CN }, "zh-TW": { table: pricingZhTW, forbidden: ZH_TW }, ja: { table: pricingJa, forbidden: JA } };
/** The security design's (`features/landing/security/security-copy.ts`): a device that runs AI, never a computer, there too. */
const SECURITY = { "zh-CN": { table: securityZhCN, forbidden: ZH_CN }, "zh-TW": { table: securityZhTW, forbidden: ZH_TW }, ja: { table: securityJa, forbidden: JA } };
/** The terms of service's (`features/landing/terms/terms-copy.ts`): a device, never a computer, there too. */
const TERMS = { "zh-CN": { table: termsZhCN, forbidden: ZH_CN }, "zh-TW": { table: termsZhTW, forbidden: ZH_TW }, ja: { table: termsJa, forbidden: JA } };

test("no translation says 机器 / 电脑 / 本机, 機器 / 電腦 / 本機, or マシン / コンピューター / 端末", () => {
  for (const tables of [TABLES, DOWNLOAD, PRICING, SECURITY, TERMS]) {
    for (const [lang, { table, forbidden }] of Object.entries(tables)) {
      const said = Object.entries(table).filter(([, value]) => forbidden.test(words(value)));
      assert.deepEqual(said, [], `${lang} says it`);
    }
  }
});

test('no English string says "machine", "computer" or "PC" (a key is the English text)', () => {
  const keys = new Set([TABLES, DOWNLOAD, PRICING, SECURITY, TERMS].flatMap((tables) => Object.values(tables).flatMap(({ table }) => Object.keys(table))));
  const said = [...keys].filter((key) => ENGLISH.test(words(key)));
  assert.deepEqual(said, []);
});

/** Each piece of on-screen text in `source` that says it, as `line: text`. Chinese or Japanese words are on-screen text wherever they are. */
function sayings(file: string, source: string): string[] {
  return sourceTexts(file, source)
    .filter(({ text, shown }) => CJK.test(words(text)) || (shown && ENGLISH.test(words(text))))
    .map(({ line, text }) => `${line}: ${text.trim().slice(0, 100)}`);
}

test("no interface source puts 「机器」 / machine / 「电脑」 / computer on screen", () => {
  const src = fileURLToPath(new URL("../src", import.meta.url));
  const files = sourceFiles(src);
  assert.ok(files.length > 100);
  const said: string[] = [];
  for (const file of files) {
    const rel = file.slice(src.length + 1);
    if (NOT_INTERFACE.some((dir) => rel.startsWith(dir))) continue;
    for (const s of sayings(rel, readFileSync(file, "utf8"))) said.push(`${rel}:${s}`);
  }
  assert.deepEqual(said, []);
});

// The user's decision of 2026-10-20: every browser or phone is a full
// device, so nothing on screen speaks of 「可管理账号」 / managing the account
// any more — a browser or phone is a full device, or a limited one to make
// full. (The chain's own rule texts, matched against the verifiers, stay in
// the code; the one that can reach a person is translated without it.)
const MANAGES_EN = /\bmanag(?:e|es|ing) (?:the|this) account\b|\bcan manage\b/i;
const MANAGES_CJK = /管理账号|管理账户|管理帳號|管理帳戶|アカウントを管理|アカウント管理|アカウントの管理/;

test("no interface text and no translation speaks of managing the account", () => {
  for (const [lang, { table }] of Object.entries(TABLES)) {
    const said = Object.entries(table).filter(([, value]) => MANAGES_CJK.test(value));
    assert.deepEqual(said, [], `${lang} says it`);
  }
  const src = fileURLToPath(new URL("../src", import.meta.url));
  const said: string[] = [];
  for (const file of sourceFiles(src)) {
    const rel = file.slice(src.length + 1);
    if (NOT_INTERFACE.some((dir) => rel.startsWith(dir))) continue;
    for (const { line, text, shown } of sourceTexts(rel, readFileSync(file, "utf8"))) {
      if ((shown && MANAGES_EN.test(text)) || MANAGES_CJK.test(text)) said.push(`${rel}:${line}: ${text.trim().slice(0, 100)}`);
    }
  }
  assert.deepEqual(said, []);
});

test("the scan catches each way text reaches the screen, and nothing that is code", () => {
  const hits = (source: string) => sayings("probe.tsx", source).length;
  assert.equal(hits(`const a = <span>机器</span>;`), 1, "JSX text, Chinese");
  assert.equal(hits(`const a = <span>Machines</span>;`), 1, "JSX text, English");
  assert.equal(hits(`const a = <b aria-label="machine name" />;`), 1, "aria-label");
  assert.equal(hits(`const a = <b title="the computer" placeholder="x" alt="y" />;`), 1, "title");
  assert.equal(hits(`const a = <img alt="a computer" />;`), 1, "alt");
  assert.equal(hits("const a = <b title={`${n} machines`} />;"), 1, "a template literal in an attribute");
  assert.equal(hits(`const a = <b>{"machine"}</b>;`), 1, '{"…"}');
  assert.equal(hits("const a = <b>{`${n} computers`}</b>;"), 1, "a template literal in JSX");
  assert.equal(hits(`const a = t("电脑离线");`), 1, "Chinese inside t()");
  assert.equal(hits(`const a = t("computer offline");`), 1, "English t() key");
  assert.equal(hits(`const a = tr(\n  "the machine is offline");`), 1, "a renamed t, over two lines");
  assert.equal(hits(`const m = "电脑";`), 1, "Chinese in any string");
  // Never: comments, identifiers, data attributes, English code strings.
  assert.equal(hits("// the machine\nconst machine = 1; /* computer */ const k = \"machine key epoch\"; const a = <b data-testid=\"add-machine\" id={`machine-${id}`} />;"), 0);
  assert.equal(sayings("features/link/link-settings.tsx", `const a = t("this computer");`).length, 1, "the device's own local settings included");
  assert.equal(hits(`const a = t("本机");`), 1, "本机 is 「这台设备」");
  assert.equal(hits(`const a = t("What works from this PC today:");`), 1, "a PC is a device too");
  assert.equal(hits(`const a = <Field label="computer name" />;`), 1, "a label prop");
  assert.equal(hits(`const a = <b aria-description="the machine" />;`), 1, "aria-description");
  assert.equal(hits(`toast("computer added");`), 1, "a toast");
  assert.equal(hits(`confirm("remove this machine?");`), 1, "confirm()");
});

test("errors a device's channel, a relay or an enrollment can end in read in the reader's language, never as “machine”", async () => {
  const { default: i18n } = await import("../src/i18n/index.js");
  const { errorText, knownText } = await import("../src/features/cloud/ui.js");
  const { AccountError } = await import("../src/lib/hub/account.js");
  const { MachineClosedError } = await import("../src/features/cloud/cloud-transport.js");
  const { ChannelBlocked } = await import("../src/lib/hub/machine.js");
  const source = (file: string) => readFileSync(new URL(`../src/${file}`, import.meta.url), "utf8");
  // What the channel, the relays and the enrollment throw or note, as written.
  const thrown = [...source("lib/hub/machine.ts").matchAll(/(?:new Error|noteFailure\([^,]+,)\(?\s*"([^"]+)"/g)].map((m) => m[1]!);
  assert.ok(thrown.includes("the device did not answer") && thrown.includes("the device is not on this relay") && thrown.includes("the device refused the channel"), thrown.join(" | "));
  await i18n.changeLanguage("zh-CN");
  try {
    for (const message of thrown) {
      const said = errorText(new Error(message));
      assert.notEqual(said, message, `“${message}” has words in zh-CN`);
      assert.doesNotMatch(said, /机器|电脑/);
    }
    assert.equal(errorText(new Error("the device is offline")), "设备离线");
    assert.equal(knownText("the device is not on this relay"), "设备不在这个中继上", "the relay picker's “last failure”");
    assert.equal(knownText("closed (1006)"), "closed (1006)", "anything else as it is");
    assert.equal(errorText(new MachineClosedError()), "这台设备已不在本页打开");
    assert.equal(errorText(new ChannelBlocked("agentTooOld")), "设备的 agent 版本过旧，无法连接");
    assert.equal(errorText(new AccountError("proof", "the device's enrollment proof does not verify: it was not added")), "设备的注册证明校验不通过：没有添加它");
    // D1's refusal, in words that do not speak of managing the account.
    const { hostAdminRefused } = await import("../src/lib/hub/account.js");
    assert.equal(errorText(hostAdminRefused()), "可运行 AI 的设备暂时不能成为完整设备");
    // The relay's or the hub's own reason reaches the reader, inside their sentence.
    const { ChannelRefused } = await import("../src/lib/hub/machine.js");
    const refused = new ChannelRefused("hub", "this device was removed from the account");
    assert.equal(errorText(refused), "Hub 拒绝了通道：this device was removed from the account");
    assert.equal(refused.message, "the hub refused the channel: this device was removed from the account", "and the diagnostics");
    assert.equal(errorText(new ChannelRefused("relay", null)), "中继拒绝了通道");
    assert.match(source("lib/hub/machine.ts"), /new ChannelRefused\("relay", first\.reason \|\| null\)/);
    assert.match(source("lib/hub/machine.ts"), /new ChannelRefused\("hub", first\.reason \|\| null\)/);
    const named = new AccountError("notInChain", "{{host}} is not a device of the account that runs AI", undefined, undefined, { host: "m-1" });
    assert.equal(named.message, "m-1 is not a device of the account that runs AI", "the English, filled in, for the logs");
    assert.equal(errorText(named), "m-1 不是这个账户里可运行 AI 的设备");
  } finally {
    await i18n.changeLanguage("en");
  }
  assert.equal(errorText(new Error("the device did not answer")), "the device did not answer");
});
