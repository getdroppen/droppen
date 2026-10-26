import i18n from "i18next";
import type { Root } from "react-dom/client";
import { initReactI18next } from "react-i18next";
import { languageLoader, rootAfter } from "./load";
import { pickLanguage, type Language } from "./match";

export { matchLanguage, pickLanguage, type Language } from "./match";

/**
 * Translation keys ARE the English source strings, so English needs no
 * resource file and a missing translation falls back to readable English.
 * Every other language is a chunk of its own (`lang-*.ts`), fetched when the
 * page first shows it: an English page downloads none of them, any other
 * page its own alone. So every table is a file of one language
 * (`cloud-devices.ja.ts`; a file two languages share would be a chunk both
 * download), and `cloud-devices.ts` and the like name a feature's three for
 * the tests.
 */
export type LanguagePref = "auto" | Language;

/** The four, each named in its own script, as the language menus list them. */
export const LANGUAGE_NAMES: readonly { id: Language; label: string }[] = [
  { id: "zh-CN", label: "简体中文" },
  { id: "zh-TW", label: "繁體中文" },
  { id: "ja", label: "日本語" },
  { id: "en", label: "English" },
];

const CACHE_KEY = "rh.lang";

function isLanguage(value: unknown): value is Language {
  return value === "en" || value === "zh-CN" || value === "zh-TW" || value === "ja";
}

/**
 * The browser's languages in its order of preference: `navigator.languages`,
 * then `navigator.language`. WebKit lists only the first of the system's
 * languages; the desktop app hands its window the whole list
 * (`desktop/src-tauri/src/init.js`).
 */
export function browserLanguages(): string[] {
  if (typeof navigator === "undefined") return [];
  const list: string[] = Array.isArray(navigator.languages) ? [...navigator.languages] : [];
  if (navigator.language) list.push(navigator.language);
  return list;
}

/** The language a pref shows: an explicit choice as it is; `auto` the first of the browser's languages the app speaks, else English. */
export function resolveLanguage(pref: LanguagePref, languages: readonly string[] = browserLanguages()): Language {
  return pref !== "auto" && isLanguage(pref) ? pref : pickLanguage(languages);
}

/** The pref this browser last applied (`applyLanguage`): an explicit choice, else `auto`. */
export function storedLanguagePref(): LanguagePref {
  try {
    const cached = localStorage.getItem(CACHE_KEY);
    if (isLanguage(cached)) return cached;
  } catch {
    // storage unavailable: auto
  }
  return "auto";
}

function setDocumentLanguage(lang: string): void {
  if (typeof document !== "undefined" && document.documentElement) document.documentElement.lang = lang;
}

const loader = languageLoader(i18n, {
  "zh-CN": () => import("./lang-zh-CN"),
  "zh-TW": () => import("./lang-zh-TW"),
  ja: () => import("./lang-ja"),
});

/** `lang`'s translations into i18next, fetched once (see `LanguageLoader.load`). */
export const loadLanguage = loader.load;

/** Settles once the language asked for last shows: its translations in, or English if they did not load. */
export const languageShown = loader.shown;

/**
 * The server pref is the source of truth; localStorage caches it for boot.
 * The page changes language once that language's translations are in (or to
 * English if they cannot be had), which the returned promise waits for.
 */
export function applyLanguage(pref: LanguagePref): Promise<void> {
  try {
    localStorage.setItem(CACHE_KEY, pref);
  } catch {
    // storage unavailable: language still applies for this page
  }
  return loader.show(resolveLanguage(pref));
}

/**
 * `root` (main.tsx's), drawing its first frame once the page's language
 * shows: in the reader's language from the start, never English first.
 */
export function rootInLanguage(root: Root): Root {
  return rootAfter(root, languageShown());
}

const initial = resolveLanguage(storedLanguagePref());

// Synchronous: with `resources` given (empty: each language comes as its
// chunk loads), init finishes before it returns, in `initial`.
void i18n.use(initReactI18next).init({
  resources: {},
  lng: initial,
  fallbackLng: false,
  keySeparator: false,
  nsSeparator: false,
  interpolation: { escapeValue: false },
});
void loader.show(initial);

// `<html lang>` says the page's language from the first paint on (the shell
// is `en` until a script runs), and follows every change after.
setDocumentLanguage(initial);
i18n.on("languageChanged", setDocumentLanguage);

// `auto` follows the browser when its languages change while the page is open.
if (typeof window !== "undefined" && typeof window.addEventListener === "function") {
  window.addEventListener("languagechange", () => {
    if (storedLanguagePref() === "auto") void applyLanguage("auto");
  });
}

export default i18n;
