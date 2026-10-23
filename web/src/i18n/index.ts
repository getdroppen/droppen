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
import { welcomeJa, welcomeZhCN, welcomeZhTW } from "./welcome";
import { zhCN } from "./zh-CN";
import { zhTW } from "./zh-TW";

/**
 * Translation keys ARE the English source strings, so English needs no
 * resource file and a missing translation falls back to readable English.
 */
export type LanguagePref = "auto" | "en" | "zh-CN" | "zh-TW" | "ja";

const CACHE_KEY = "rh.lang";

export function resolveLanguage(pref: LanguagePref): string {
  if (pref !== "auto") return pref;
  const nav = navigator.language || "en";
  if (/^zh/i.test(nav)) return /tw|hk|mo|hant/i.test(nav) ? "zh-TW" : "zh-CN";
  if (/^ja/i.test(nav)) return "ja";
  return "en";
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
  document.documentElement.lang = lang;
}

function initialLanguage(): string {
  let pref: LanguagePref = "auto";
  try {
    const cached = localStorage.getItem(CACHE_KEY);
    if (cached === "en" || cached === "zh-CN" || cached === "zh-TW" || cached === "ja") pref = cached;
  } catch {
    // fall through to auto
  }
  return resolveLanguage(pref);
}

void i18n.use(initReactI18next).init({
  resources: {
    "zh-CN": { translation: { ...onboardingZhCN, ...crossDeviceZhCN, ...linkZhCN, ...cloudDevicesZhCN, ...signOutZhCN, ...hostsZhCN, ...moveZhCN, ...setupZhCN, ...welcomeZhCN, ...zhCN } },
    "zh-TW": { translation: { ...onboardingZhTW, ...crossDeviceZhTW, ...linkZhTW, ...cloudDevicesZhTW, ...signOutZhTW, ...hostsZhTW, ...moveZhTW, ...setupZhTW, ...welcomeZhTW, ...zhTW } },
    ja: { translation: { ...onboardingJa, ...crossDeviceJa, ...linkJa, ...cloudDevicesJa, ...signOutJa, ...hostsJa, ...moveJa, ...setupJa, ...welcomeJa, ...ja } },
  },
  lng: initialLanguage(),
  fallbackLng: false,
  keySeparator: false,
  nsSeparator: false,
  interpolation: { escapeValue: false },
});

export default i18n;
