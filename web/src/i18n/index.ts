import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "./cloud-devices";
import { crossDeviceJa, crossDeviceZhCN, crossDeviceZhTW } from "./cross-device";
import { hostsJa, hostsZhCN, hostsZhTW } from "./hosts";
import { ja } from "./ja";
import { linkJa, linkZhCN, linkZhTW } from "./link";
import { moveJa, moveZhCN, moveZhTW } from "./move";
import { onboardingJa, onboardingZhCN, onboardingZhTW } from "./onboarding";
import { setupJa, setupZhCN, setupZhTW } from "./setup";
import { signOutJa, signOutZhCN, signOutZhTW } from "./sign-out";
import { updatesJa, updatesZhCN, updatesZhTW } from "./updates";
import { welcomeJa, welcomeZhCN, welcomeZhTW } from "./welcome";
import { zhCN } from "./zh-CN";
import { zhTW } from "./zh-TW";
import { pickLanguage, type Language } from "./match";

export { matchLanguage, pickLanguage, type Language } from "./match";

/**
 * Translation keys ARE the English source strings, so English needs no
 * resource file and a missing translation falls back to readable English.
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

/** The server pref is the source of truth; localStorage caches it for boot. */
export function applyLanguage(pref: LanguagePref): void {
  try {
    localStorage.setItem(CACHE_KEY, pref);
  } catch {
    // storage unavailable: language still applies for this page
  }
  const lang = resolveLanguage(pref);
  if (i18n.language !== lang) void i18n.changeLanguage(lang);
  setDocumentLanguage(lang);
}

const initial = resolveLanguage(storedLanguagePref());

void i18n.use(initReactI18next).init({
  resources: {
    "zh-CN": { translation: { ...onboardingZhCN, ...crossDeviceZhCN, ...linkZhCN, ...cloudDevicesZhCN, ...signOutZhCN, ...updatesZhCN, ...hostsZhCN, ...moveZhCN, ...setupZhCN, ...welcomeZhCN, ...zhCN } },
    "zh-TW": { translation: { ...onboardingZhTW, ...crossDeviceZhTW, ...linkZhTW, ...cloudDevicesZhTW, ...signOutZhTW, ...updatesZhTW, ...hostsZhTW, ...moveZhTW, ...setupZhTW, ...welcomeZhTW, ...zhTW } },
    ja: { translation: { ...onboardingJa, ...crossDeviceJa, ...linkJa, ...cloudDevicesJa, ...signOutJa, ...updatesJa, ...hostsJa, ...moveJa, ...setupJa, ...welcomeJa, ...ja } },
  },
  lng: initial,
  fallbackLng: false,
  keySeparator: false,
  nsSeparator: false,
  interpolation: { escapeValue: false },
});

// `<html lang>` says the page's language from the first paint on (the shell
// is `en` until a script runs), and follows every change after.
setDocumentLanguage(initial);
i18n.on("languageChanged", setDocumentLanguage);

// `auto` follows the browser when its languages change while the page is open.
if (typeof window !== "undefined" && typeof window.addEventListener === "function") {
  window.addEventListener("languagechange", () => {
    if (storedLanguagePref() === "auto") applyLanguage("auto");
  });
}

export default i18n;
