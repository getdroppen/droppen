// The first-run guide's words (`i18n/welcome.ts`): every string in all three
// languages with the same
// placeholders, the header that never reads as Settings, no translation the
// app already has changed by merging this table in, and every string the
// guide's source puts on screen translated.
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import ts from "typescript";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "../src/i18n/cloud-devices.js";
import { crossDeviceJa, crossDeviceZhCN, crossDeviceZhTW } from "../src/i18n/cross-device.js";
import { ja } from "../src/i18n/ja.js";
import { linkJa, linkZhCN, linkZhTW } from "../src/i18n/link.js";
import { onboardingJa, onboardingZhCN, onboardingZhTW } from "../src/i18n/onboarding.js";
import { setupJa, setupZhCN, setupZhTW } from "../src/i18n/setup.js";
import { welcomeJa, welcomeZhCN, welcomeZhTW } from "../src/i18n/welcome.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";
import { GUIDE_TITLE } from "../src/features/setup/frame.js";
import { STEP_LABEL, TOOL_STATUS_LABEL } from "../src/features/setup/model.js";
import { STEP_SUMMARY } from "../src/features/setup/screens.js";
import { STEP_HEADERS } from "../src/features/setup/welcome-view.js";
import { STEP01_STRINGS } from "../src/features/setup/tools-step.js";
import { LANDED, PROMPTS } from "../src/features/setup/first-model.js";

const SETUP = { "zh-CN": welcomeZhCN, "zh-TW": welcomeZhTW, ja: welcomeJa } as const;
/** What the app had before this table: the feature tables (the setup page's, `i18n/setup.ts`, included) and the main one, as `i18n/lang-*.ts` merge them. */
const EXISTING = {
  "zh-CN": { ...onboardingZhCN, ...crossDeviceZhCN, ...linkZhCN, ...cloudDevicesZhCN, ...setupZhCN, ...zhCN },
  "zh-TW": { ...onboardingZhTW, ...crossDeviceZhTW, ...linkZhTW, ...cloudDevicesZhTW, ...setupZhTW, ...zhTW },
  ja: { ...onboardingJa, ...crossDeviceJa, ...linkJa, ...cloudDevicesJa, ...setupJa, ...ja },
} as const;
const MERGED = {
  "zh-CN": { ...EXISTING["zh-CN"], ...welcomeZhCN, ...zhCN },
  "zh-TW": { ...EXISTING["zh-TW"], ...welcomeZhTW, ...zhTW },
  ja: { ...EXISTING.ja, ...welcomeJa, ...ja },
} as const;

const placeholders = (s: string) => [...s.matchAll(/\{\{(\w+)\}\}/g)].map((m) => m[1]).sort();

test("every string of the guide is in all three languages, with the same placeholders", () => {
  const keys = Object.keys(welcomeZhCN);
  assert.ok(keys.length > 200, `the whole design is there (${keys.length})`);
  for (const [lang, table] of Object.entries(SETUP)) {
    assert.deepEqual(Object.keys(table), keys, `${lang} has exactly the same keys, in the same order`);
    for (const [key, value] of Object.entries(table)) {
      assert.ok(value.trim().length > 0, `${lang}: ${key}`);
      assert.deepEqual(placeholders(value), placeholders(key), `${lang} keeps the placeholders of ${key}`);
      assert.doesNotMatch(key, /(^|[^{])\{\w+\}(?!\})/, `i18next placeholders are {{name}}: ${key}`);
    }
  }
});

test("the header is “Get started with Droppen”, never 「设置 Droppen」, which reads as Settings", () => {
  assert.equal(GUIDE_TITLE, "Get started with Droppen");
  assert.equal(welcomeZhCN[GUIDE_TITLE], "开始使用 Droppen");
  assert.equal(welcomeZhTW[GUIDE_TITLE], "開始使用 Droppen");
  assert.equal(welcomeJa[GUIDE_TITLE], "Droppen を使い始める");
  for (const [lang, table] of Object.entries(MERGED)) assert.equal(table[GUIDE_TITLE], SETUP[lang as keyof typeof SETUP][GUIDE_TITLE], `${lang}: no other table says otherwise`);
  const settings = /设置 ?Droppen|設定 ?Droppen|Droppen ?の ?セットアップ/;
  for (const [lang, table] of Object.entries(SETUP)) {
    assert.deepEqual(Object.values(table).filter((v) => settings.test(v)), [], lang);
  }
  assert.ok(!("Set up Droppen" in welcomeZhCN));
});

test("merging the guide's table changes no translation the app already has", () => {
  for (const [lang, table] of Object.entries(SETUP)) {
    const existing = EXISTING[lang as keyof typeof EXISTING] as Record<string, string>;
    const changed = Object.entries(table).filter(([key, value]) => key in existing && existing[key] !== value).map(([key, value]) => `${key}: ${existing[key]} → ${value}`);
    assert.deepEqual(changed, [], lang);
  }
});

/** The `t("…")` keys in the guide's source. */
function sourceKeys(): string[] {
  const dir = fileURLToPath(new URL("../src/features/setup/", import.meta.url));
  const keys: string[] = [];
  for (const name of readdirSync(dir).filter((n) => /\.tsx?$/.test(n))) {
    const sf = ts.createSourceFile(name, readFileSync(dir + name, "utf8"), ts.ScriptTarget.Latest, true, name.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
    const visit = (n: ts.Node): void => {
      if (ts.isCallExpression(n) && n.expression.getText(sf) === "t" && n.arguments[0] && ts.isStringLiteral(n.arguments[0])) keys.push(n.arguments[0].text);
      ts.forEachChild(n, visit);
    };
    visit(sf);
  }
  return keys;
}

test("every string the guide puts on screen is translated", () => {
  const keys = new Set([
    ...sourceKeys(),
    GUIDE_TITLE,
    ...Object.values(STEP_LABEL),
    ...Object.values(TOOL_STATUS_LABEL),
    ...Object.values(STEP_SUMMARY),
    ...Object.values(STEP_HEADERS).flatMap((h) => [h.kicker, h.title, ...(h.body ? [h.body] : [])]),
    ...STEP01_STRINGS,
    ...Object.values(PROMPTS),
    LANDED,
  ]);
  assert.ok(keys.size > 40, `found the guide's strings (${keys.size})`);
  for (const [lang, table] of Object.entries(MERGED)) {
    const missing = [...keys].filter((key) => !(key in table));
    assert.deepEqual(missing, [], `${lang} translates them all`);
  }
});
