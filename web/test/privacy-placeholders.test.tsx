// What only the owner can say on droppen.app's pages: owner placeholders,
// such as the privacy notice's backup retention was until the owner gave it
// (`BACKUPS_EXPIRE_DAYS` in `features/landing/privacy.tsx`, 2026-10-23).
//
// What these pin: a placeholder shows on the page marked, in every
// language, and nowhere unmarked; and every
// placeholder the source writes is in `web/owner-placeholders.txt`, the list
// a release checks its built web app against
// (`scripts/release-text-check.sh`, `release_text_owner_placeholders`): main
// stays green with a placeholder left, a release cannot ship one.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { landingBundles } from "../src/features/landing/copy.js";
import PrivacyPage, { BACKUPS_EXPIRE_DAYS, marked, PLACEHOLDER } from "../src/features/landing/privacy.js";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "../src/i18n/cloud-devices.js";
import { ja } from "../src/i18n/ja.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";

const app = { "zh-CN": { ...cloudDevicesZhCN, ...zhCN }, "zh-TW": { ...cloudDevicesZhTW, ...zhTW }, ja: { ...cloudDevicesJa, ...ja } };
const LANGS = ["en", "zh-CN", "zh-TW", "ja"] as const;
const i18n = i18next.createInstance();
void i18n.use(initReactI18next).init({
  lng: "en",
  resources: {
    en: { translation: {} },
    ...Object.fromEntries((["zh-CN", "zh-TW", "ja"] as const).map((lng) => [lng, { translation: { ...landingBundles[lng], ...app[lng] } }])),
  },
  interpolation: { escapeValue: false },
  keySeparator: false,
  nsSeparator: false,
});

Object.assign(globalThis, { location: { search: "", pathname: "/privacy", origin: "https://droppen.app", hash: "" } });

function draw(lng: string): string {
  void i18n.changeLanguage(lng);
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <PrivacyPage />
    </I18nextProvider>,
  );
}

const unmarked = (html: string) => html.replace(/<mark data-placeholder=""[^>]*>[^<]*<\/mark>/g, "");

test("a placeholder is a value in capitals and brackets, marked where it is said; a filled-in period is plain", () => {
  assert.match("[BACKUP RETENTION]", PLACEHOLDER);
  assert.doesNotMatch("30 days", PLACEHOLDER);
  assert.equal(marked("after 30 days."), "after 30 days.");
  const html = renderToStaticMarkup(<p>{marked("after [BACKUP RETENTION].")}</p>);
  assert.equal(html, '<p>after <mark data-placeholder="" class="bg-warn/25 px-0.5 font-mono text-warn-fg">[BACKUP RETENTION]</mark>.</p>');
});

test("the backups' period is said on the notice in every language, and no placeholder is left unmarked", () => {
  assert.equal(BACKUPS_EXPIRE_DAYS, 30);
  const said = { en: "expire within 30 days", "zh-CN": "会在 30 天内过期", "zh-TW": "會在 30 天內過期", ja: "30 日以内に期限切れ" };
  for (const lng of LANGS) {
    const html = draw(lng);
    assert.ok(html.includes(said[lng]), `${lng}: said`);
    assert.doesNotMatch(unmarked(html), PLACEHOLDER, `${lng}: a placeholder unmarked`);
  }
});

/** Every file under `dir`. */
function files(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? files(path) : [path];
  });
}

test("every owner placeholder the source writes is on the list a release checks its build against", () => {
  const root = fileURLToPath(new URL("..", import.meta.url));
  const listed = readFileSync(join(root, "owner-placeholders.txt"), "utf8")
    .split("\n")
    .map((l) => l.replace(/\r$/, ""))
    .filter((l) => l !== "" && !l.startsWith("#"));
  for (const marker of listed) assert.match(marker, new RegExp(`^${PLACEHOLDER.source}$`), `listed: ${marker}`);
  assert.ok(listed.includes("[BACKUP RETENTION]"));
  // A placeholder is a string literal of the source that is one: "[IN CAPITALS]" (comments aside).
  const written = new Set<string>();
  for (const file of files(join(root, "src")).filter((f) => /\.(ts|tsx)$/.test(f))) {
    for (const line of readFileSync(file, "utf8").split("\n")) {
      if (/^\s*(\/\/|\*|\/\*)/.test(line)) continue;
      for (const m of line.matchAll(new RegExp(`"(${PLACEHOLDER.source})"`, "g"))) written.add(m[1]);
    }
  }
  for (const marker of written) assert.ok(listed.includes(marker), `${marker} is in the source but not in web/owner-placeholders.txt`);
});
