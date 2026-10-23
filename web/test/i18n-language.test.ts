// The page's language (`i18n/index.ts`): an explicit choice as it is; `auto`
// the first of the browser's languages, in its order, that the app speaks
// (en, zh-CN, zh-TW, ja), else English. `<html lang>` says it from the start.
import assert from "node:assert/strict";
import test from "node:test";

/** A `Storage` kept in memory. */
class MemoryStorage {
  private values = new Map<string, string>();
  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }
  setItem(key: string, value: string): void {
    this.values.set(key, String(value));
  }
  removeItem(key: string): void {
    this.values.delete(key);
  }
  clear(): void {
    this.values.clear();
  }
}

function stubGlobal(name: string, value: unknown): void {
  Object.defineProperty(globalThis, name, { value, configurable: true, writable: true });
}

function setBrowser(languages: string[] | undefined, language: string): void {
  stubGlobal("navigator", { languages, language, userAgent: "node" });
}

// Before the module loads: a French-first reader who also reads Hong Kong Chinese, nothing stored.
const storage = new MemoryStorage();
const html = { lang: "en" };
stubGlobal("localStorage", storage);
stubGlobal("document", { documentElement: html });
stubGlobal("window", new EventTarget());
setBrowser(["fr-FR", "zh-HK", "en-US"], "fr-FR");

const { default: i18n, applyLanguage, browserLanguages, matchLanguage, pickLanguage, resolveLanguage, storedLanguagePref } = await import("../src/i18n/index.js");

test("the first page is in the reader's language, and <html lang> says so from the start", () => {
  assert.equal(i18n.language, "zh-TW");
  assert.equal(html.lang, "zh-TW");
});

test("each tag maps to a language the app speaks, or to none", () => {
  for (const tag of ["zh-Hant", "zh-TW", "zh-HK", "zh-MO", "zh-Hant-TW", "zh-Hant-HK", "zh-Hant-CN", "zh_TW", "ZH-hk"]) {
    assert.equal(matchLanguage(tag), "zh-TW", tag);
  }
  for (const tag of ["zh", "zh-CN", "zh-SG", "zh-Hans", "zh-Hans-CN", "zh-Hans-HK", "zh-Hans-MO", "zh-Hans-JP", "zh_CN"]) {
    assert.equal(matchLanguage(tag), "zh-CN", tag);
  }
  for (const tag of ["ja", "ja-JP", "JA-jp"]) assert.equal(matchLanguage(tag), "ja", tag);
  for (const tag of ["en", "en-US", "en-GB", "en-JP"]) assert.equal(matchLanguage(tag), "en", tag);
  for (const tag of ["fr-FR", "de", "ko-KR", "yue-Hant-HK", "jav", "eng", "zhx", ""]) assert.equal(matchLanguage(tag), null, tag);
});

test("auto takes the first of the browser's languages that the app speaks, in the browser's order", () => {
  assert.equal(pickLanguage(["fr-FR", "ja-JP", "zh-CN"]), "ja", "French is skipped, Japanese is next");
  assert.equal(pickLanguage(["de-DE", "zh-Hant-TW", "ja"]), "zh-TW");
  assert.equal(pickLanguage(["en-US", "zh-CN"]), "en", "English, when it comes first, is a match like any other");
  assert.equal(pickLanguage(["zh-SG", "en-US"]), "zh-CN");
  assert.equal(pickLanguage([undefined, null, "", "ko", "ja-JP"]), "ja");
});

test("auto falls back to English when the browser speaks none of the app's languages", () => {
  assert.equal(pickLanguage(["fr-FR", "de-DE", "ko-KR"]), "en");
  assert.equal(pickLanguage([]), "en");
  assert.equal(resolveLanguage("auto", ["es-ES"]), "en");
  assert.equal(resolveLanguage("auto", []), "en");
});

test("an explicit choice wins over the browser's languages", () => {
  assert.equal(resolveLanguage("ja", ["zh-CN", "en-US"]), "ja");
  assert.equal(resolveLanguage("en", ["ja-JP"]), "en");
  assert.equal(resolveLanguage("zh-TW", ["zh-CN"]), "zh-TW");
  assert.equal(resolveLanguage("zh-CN", []), "zh-CN");
  assert.equal(resolveLanguage("auto", ["fr", "zh-Hant"]), "zh-TW");
  // A pref the app does not know (an older or newer server) is read as auto.
  assert.equal(resolveLanguage("ko" as never, ["ja"]), "ja");
});

test("the browser's languages are navigator.languages in order, then navigator.language", () => {
  setBrowser(["fr-FR", "ja", "en"], "fr-FR");
  assert.deepEqual(browserLanguages(), ["fr-FR", "ja", "en", "fr-FR"]);
  assert.equal(resolveLanguage("auto"), "ja");
  // WebKit lists one language; a browser without the list still has navigator.language.
  setBrowser(undefined, "zh-HK");
  assert.deepEqual(browserLanguages(), ["zh-HK"]);
  assert.equal(resolveLanguage("auto"), "zh-TW");
  setBrowser([], "");
  assert.equal(resolveLanguage("auto"), "en");
});

test("a choice is kept for the next boot, wins over the browser, and auto gives the browser back", () => {
  setBrowser(["fr-FR", "zh-CN"], "fr-FR");
  applyLanguage("ja");
  assert.equal(storage.getItem("rh.lang"), "ja");
  assert.equal(storedLanguagePref(), "ja");
  assert.equal(resolveLanguage(storedLanguagePref()), "ja", "what the next boot shows");
  assert.equal(i18n.language, "ja");
  assert.equal(html.lang, "ja");

  applyLanguage("auto");
  assert.equal(storedLanguagePref(), "auto");
  assert.equal(i18n.language, "zh-CN");
  assert.equal(html.lang, "zh-CN");

  storage.setItem("rh.lang", "klingon");
  assert.equal(storedLanguagePref(), "auto", "a value the app does not know is auto");
});

test("auto follows the browser when its languages change; an explicit choice does not", () => {
  applyLanguage("auto");
  setBrowser(["ja-JP"], "ja-JP");
  window.dispatchEvent(new Event("languagechange"));
  assert.equal(i18n.language, "ja");
  assert.equal(html.lang, "ja");

  applyLanguage("en");
  setBrowser(["zh-TW"], "zh-TW");
  window.dispatchEvent(new Event("languagechange"));
  assert.equal(i18n.language, "en");
  assert.equal(html.lang, "en");
});

test("<html lang> follows every change of language", async () => {
  await i18n.changeLanguage("zh-TW");
  assert.equal(html.lang, "zh-TW");
  await i18n.changeLanguage("en");
  assert.equal(html.lang, "en");
});
