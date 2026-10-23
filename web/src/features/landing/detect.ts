import { type Availability, type DownloadOs, osOfSearch } from "./get-droppen";

/*
 * Which device is reading the home page or the download page, so the right
 * way onto it is offered before anyone looks for it (the hero's action in
 * `landing.tsx`, the tab `download.tsx` opens on). From what the browser says
 * of itself, in the browser, with no request: `navigator.userAgentData` where
 * there is one (platform and mobile), else the user agent — and the touch
 * points, because an iPad's Safari says it is a Mac. A Mac's processor is
 * never asked: the App is universal. Linux's is read from the high-entropy
 * hints (`architecture`, `bitness`) where the browser offers them, else from
 * the user agent, and only said on its tab, with a word when the release has
 * no build for it.
 */

/** What the browser says of itself (all of it optional but the user agent). */
export interface EnvironmentHints {
  userAgent: string;
  /** `navigator.userAgentData.platform`: "macOS", "Windows", "Linux", "Android", "Chrome OS", … */
  platform?: string | null;
  /** `navigator.userAgentData.mobile`. */
  mobile?: boolean | null;
  /** `navigator.maxTouchPoints`: an iPad's Safari reports a Mac, with touch. */
  maxTouchPoints?: number | null;
}

/** What kind of device it is. */
export type DeviceKind = "mac" | "ipad" | "iphone" | "android" | "windows" | "linux" | "chromeos" | "unknown";

/** The kind of device `hints` describe. */
export function detectKind(hints: EnvironmentHints): DeviceKind {
  const ua = hints.userAgent ?? "";
  const platform = (hints.platform ?? "").toLowerCase();
  const touch = (hints.maxTouchPoints ?? 0) > 1;
  if (/iPhone|iPod/.test(ua)) return "iphone";
  if (/iPad/.test(ua)) return "ipad";
  if (platform === "android" || /Android/.test(ua)) return "android";
  if (platform === "ios") return "iphone";
  if (platform === "chrome os" || platform === "chromeos" || /CrOS/.test(ua)) return "chromeos";
  if (platform === "macos" || /Macintosh|Mac OS X/.test(ua)) {
    // iPadOS asks for the desktop site and says it is a Mac; a Mac has no touch screen.
    return touch || hints.mobile === true ? "ipad" : "mac";
  }
  if (platform === "windows" || /Windows/.test(ua)) return "windows";
  if (platform === "linux" || /Linux|X11/.test(ua)) return hints.mobile === true ? "android" : "linux";
  if (hints.mobile === true) return "android";
  return "unknown";
}

/**
 * The tab a kind of device is detected for; `null` for a device it cannot be
 * told, for which nothing is marked detected and the page opens on the Mac
 * App ({@link DEFAULT_OS}).
 */
export function osForKind(kind: DeviceKind): DownloadOs | null {
  switch (kind) {
    case "unknown":
      return null;
    case "mac":
      return "macos-app";
    case "iphone":
    case "ipad":
    case "android":
      return "mobile";
    case "windows":
      return "windows";
    case "linux":
    case "chromeos":
      return "linux";
  }
}

/** The tab the download page opens on when the address names none and the device cannot be told: the Mac App, the way most people come in. */
export const DEFAULT_OS: DownloadOs = "macos-app";

/**
 * What the home page's hero offers as its action:
 * - `mac`: the App's download, at once (one universal `.dmg`);
 * - `linux`: the install command, to copy;
 * - `windows`: no app for Windows yet — droppen.app in this browser, and the
 *   ways onto a Mac or a Linux server;
 * - `phone`: droppen.app in the phone's browser; the AI runs on a Mac or Linux;
 * - `all`: a device it cannot tell (or one the build has no way onto yet):
 *   every way, each one click away.
 */
export type HeroOffer = "mac" | "linux" | "windows" | "phone" | "all";

/** The hero's offer for a kind of device, as far as the build's switches go. */
export function heroOffer(kind: DeviceKind, availability: Pick<Availability, "app" | "linux">): HeroOffer {
  switch (kind) {
    case "mac":
      return availability.app ? "mac" : "all";
    case "linux":
      return availability.linux ? "linux" : "all";
    case "windows":
      return "windows";
    case "iphone":
    case "ipad":
    case "android":
      return "phone";
    case "chromeos":
    case "unknown":
      return "all";
  }
}

/** The high-entropy hints asked for, and nothing else: the processor family and its width. */
export const CPU_HINTS = ["architecture", "bitness"] as const;

/** `navigator.userAgentData`, as far as it is read here (Chromium's; Safari and Firefox have none). */
export interface UserAgentData {
  platform?: string;
  mobile?: boolean;
  getHighEntropyValues?: (hints: string[]) => Promise<{ architecture?: string; bitness?: string }>;
}

/** The part of `navigator` read here (tests pass their own). */
export interface NavigatorLike {
  userAgent: string;
  maxTouchPoints?: number;
  userAgentData?: UserAgentData;
}

const theNavigator = (): NavigatorLike | null => (typeof navigator === "undefined" ? null : (navigator as unknown as NavigatorLike));

/** What this browser says of itself, synchronously (`null` outside a browser). */
export function browserHints(nav: NavigatorLike | null = theNavigator()): EnvironmentHints | null {
  if (!nav) return null;
  const data = nav.userAgentData;
  return { userAgent: nav.userAgent ?? "", platform: data?.platform ?? null, mobile: data?.mobile ?? null, maxTouchPoints: nav.maxTouchPoints ?? 0 };
}

/** The processors the release has a Linux build for (`droppen-<version>-linux-<cpu>.tar.gz`). */
export const LINUX_CPUS = ["x86_64", "aarch64"] as const;

/**
 * The processor, by the name `uname -m` gives it (x86_64, aarch64, i686,
 * armv7l): from the high-entropy hints where there are some, else from the
 * user agent (Firefox names it there; Chrome's reduced one always says
 * x86_64 on Linux, so it is read only without hints); `null` when neither
 * says.
 */
export function cpuOf(values: { architecture?: string; bitness?: string } | null, userAgent: string): string | null {
  const family = values?.architecture?.toLowerCase();
  const bits = values?.bitness;
  if (family === "x86" && (bits === "64" || bits === "32")) return bits === "64" ? "x86_64" : "i686";
  if (family === "arm" && (bits === "64" || bits === "32")) return bits === "64" ? "aarch64" : "armv7l";
  if (/\b(?:x86_64|amd64|x64|Win64|WOW64)\b/i.test(userAgent)) return "x86_64";
  if (/\b(?:aarch64|arm64)\b/i.test(userAgent)) return "aarch64";
  if (/\barmv[67]l?\b/i.test(userAgent)) return "armv7l";
  if (/\bi[3-6]86\b/.test(userAgent)) return "i686";
  return null;
}

/** This browser's processor ({@link cpuOf}), asking for the high-entropy hints where it can; `null` outside a browser or when nothing says. */
export async function browserCpu(nav: NavigatorLike | null = theNavigator()): Promise<string | null> {
  if (!nav) return null;
  let values: { architecture?: string; bitness?: string } | null = null;
  if (nav.userAgentData?.getHighEntropyValues) {
    try {
      values = await nav.userAgentData.getHighEntropyValues([...CPU_HINTS]);
    } catch {
      values = null;
    }
  }
  return cpuOf(values, nav.userAgent ?? "");
}

/**
 * The tab the page opens on: the one the address names (shared, or linked
 * from the home page, the docs and the add-device guide), else the one chosen
 * in this tab earlier (`remembered`, the session's), else the detected one,
 * else {@link DEFAULT_OS}.
 */
export function initialOs(input: { search: string; remembered: DownloadOs | null; detected: DownloadOs | null }): DownloadOs {
  return osOfSearch(input.search) ?? input.remembered ?? input.detected ?? DEFAULT_OS;
}
