// What only the owner can say on droppen.app's privacy notice
// (`features/landing/privacy.tsx`): how long copies in backups and logs live
// (`BACKUPS_EXPIRE_AFTER`).
//
// What these pin: until it is filled in, it shows on the page as a marked
// placeholder, in every language, and nowhere unmarked. The last test fails
// while it is left, so the notice cannot ship unfilled by accident: set
// `BACKUPS_EXPIRE_AFTER` (e.g. "30 days").
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { landingBundles } from "../src/features/landing/copy.js";
import PrivacyPage, { BACKUPS_EXPIRE_AFTER, marked, PLACEHOLDER } from "../src/features/landing/privacy.js";
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

test("the backups' period, still to supply, shows on the notice marked, in every language, and nowhere unmarked", () => {
  for (const lng of LANGS) {
    const html = draw(lng);
    if (PLACEHOLDER.test(BACKUPS_EXPIRE_AFTER)) {
      assert.ok(html.includes(`<mark data-placeholder="" class="bg-warn/25 px-0.5 font-mono text-warn-fg">${BACKUPS_EXPIRE_AFTER}</mark>`), `${lng}: marked`);
    } else {
      assert.ok(html.includes(BACKUPS_EXPIRE_AFTER), `${lng}: said`);
    }
    assert.doesNotMatch(unmarked(html), PLACEHOLDER, `${lng}: a placeholder unmarked`);
  }
});

// Fails until the owner sets `BACKUPS_EXPIRE_AFTER` (web/src/features/landing/privacy.tsx):
// the privacy notice must not ship with a placeholder on the page.
test("the owner's backup retention is filled in: no placeholder left on the privacy notice", () => {
  assert.doesNotMatch(BACKUPS_EXPIRE_AFTER, PLACEHOLDER, "still to fill in: BACKUPS_EXPIRE_AFTER (how long copies in backups and logs live)");
  for (const lng of LANGS) assert.doesNotMatch(draw(lng), PLACEHOLDER, `${lng}: a placeholder on the page`);
});
