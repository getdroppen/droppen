import { useEffect, useState } from "react";
import { type Availability, type DownloadOs } from "./get-droppen";

export interface EnvironmentHints {
  userAgent: string;
  platform?: string | null;
  mobile?: boolean | null;
  maxTouchPoints?: number | null;
}

export type DeviceKind = "mac" | "ipad" | "iphone" | "android" | "windows" | "linux" | "chromeos" | "unknown";

export function detectKind(hints: EnvironmentHints): DeviceKind {
  const ua = hints.userAgent;
  const platform = (hints.platform ?? "").toLowerCase();
  const touch = (hints.maxTouchPoints ?? 0) > 1;
  if (/iPhone|iPod/.test(ua)) return "iphone";
  if (/iPad/.test(ua)) return "ipad";
  if (platform === "android" || /Android/.test(ua)) return "android";
  if (platform === "ios") return "iphone";
  if (platform === "chrome os" || platform === "chromeos" || /CrOS/.test(ua)) return "chromeos";
  if (platform === "macos" || /Macintosh|Mac OS X/.test(ua)) {
    // iPadOS can report macOS when requesting the desktop site.
    return touch || hints.mobile === true ? "ipad" : "mac";
  }
  if (platform === "windows" || /Windows/.test(ua)) return "windows";
  if (platform === "linux" || /Linux|X11/.test(ua)) return hints.mobile === true ? "android" : "linux";
  if (hints.mobile === true) return "android";
  return "unknown";
}

export function osForKind(kind: DeviceKind): DownloadOs | null {
  switch (kind) {
    case "unknown": return null;
    case "mac": return "macos-app";
    case "iphone":
    case "ipad":
    case "android": return "mobile";
    case "windows": return "windows";
    case "linux":
    case "chromeos": return "linux";
  }
}

export const DEFAULT_OS: DownloadOs = "macos-app";
export type HeroOffer = "mac" | "linux" | "windows" | "phone" | "all";

export function heroOffer(kind: DeviceKind, availability: Pick<Availability, "app" | "linux">): HeroOffer {
  switch (kind) {
    case "mac": return availability.app ? "mac" : "all";
    case "linux": return availability.linux ? "linux" : "all";
    case "windows": return "windows";
    case "iphone":
    case "ipad":
    case "android": return "phone";
    case "chromeos":
    case "unknown": return "all";
  }
}

export const CPU_HINTS = ["architecture", "bitness"] as const;

interface UserAgentData {
  platform?: string;
  mobile?: boolean;
  getHighEntropyValues?: (hints: string[]) => Promise<{ architecture?: string; bitness?: string }>;
}

export interface NavigatorLike {
  userAgent: string;
  maxTouchPoints?: number;
  userAgentData?: UserAgentData;
}

const theNavigator = (): NavigatorLike | null => (typeof navigator === "undefined" ? null : (navigator as unknown as NavigatorLike));

export function browserHints(nav: NavigatorLike | null = theNavigator()): EnvironmentHints | null {
  if (!nav) return null;
  const data = nav.userAgentData;
  return { userAgent: nav.userAgent, platform: data?.platform ?? null, mobile: data?.mobile ?? null, maxTouchPoints: nav.maxTouchPoints ?? 0 };
}

export const LINUX_CPUS = ["x86_64", "aarch64"] as const;

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

export async function browserCpu(nav: NavigatorLike | null = theNavigator()): Promise<string | null> {
  if (!nav) return null;
  let values: { architecture?: string; bitness?: string } | null = null;
  if (nav.userAgentData?.getHighEntropyValues) {
    try {
      values = await nav.userAgentData.getHighEntropyValues([...CPU_HINTS]);
    } catch {
      // A refused hint request still has the user agent as a fallback.
    }
  }
  return cpuOf(values, nav.userAgent);
}

/**
 * The CPU this browser reports, when the page needs it and the caller has not
 * said. A page handed its environment (`hints`) never reads the browser: the
 * render tests and the server have none.
 */
export function useBrowserCpu({ ask, cpu, hints }: { ask: boolean; cpu?: string | null; hints?: EnvironmentHints }): string | null {
  const [detected, setDetected] = useState<string | null>(null);
  useEffect(() => {
    if (!ask || cpu !== undefined) return;
    let live = true;
    void browserCpu(hints ? null : undefined).then((value) => live && setDetected(value));
    return () => {
      live = false;
    };
  }, [ask, cpu, hints]);
  return cpu === undefined ? detected : cpu;
}
