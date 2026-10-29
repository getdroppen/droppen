import { useSyncExternalStore } from "react";
import { Monitor, Moon, Sun } from "lucide-react";

export type ThemeMode = "auto" | "light" | "dark";

const KEY = "rh.theme";

/** --color-bg for each theme. index.html's boot script resolves the same pair
 *  for first paint; this is the only other place they appear. */
const THEME_COLOR = { dark: "#0f1115", light: "#ffffff" } as const;

export function storedThemeMode(): ThemeMode {
  const v = localStorage.getItem(KEY);
  return v === "light" || v === "dark" ? v : "auto";
}

export function applyThemeMode(mode: ThemeMode): void {
  try {
    localStorage.setItem(KEY, mode);
  } catch {
    // storage unavailable: theme still applies for this page
  }
  const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  const dark = mode === "dark" || (mode === "auto" && prefersDark);
  document.documentElement.classList.toggle("dark", dark);
  // The installed app's chrome (Android status bar, desktop title bar) is
  // painted from this meta, which index.html only sets at boot — a runtime
  // toggle has to move it too or the frame around the app keeps the old theme.
  document
    .querySelector('meta[name="theme-color"]')
    ?.setAttribute("content", dark ? THEME_COLOR.dark : THEME_COLOR.light);
}

export const NEXT_MODE: Record<ThemeMode, ThemeMode> = {
  auto: "light",
  light: "dark",
  dark: "auto",
};

/** The icon of each mode in the theme toggle. */
export const MODE_ICONS: Record<ThemeMode, typeof Sun> = {
  auto: Monitor,
  light: Sun,
  dark: Moon,
};

/*
 * The theme actually painted (the `.dark` class on `<html>`), for components
 * that must not fetch what the other theme would show: the landing page's
 * screenshots, one image per theme, both in the DOM so a toggle is instant.
 * `applyThemeMode` is the only writer; the observer catches its class change
 * (and the boot script's), so a reader's toggle reaches every subscriber.
 */
const themeListeners = new Set<() => void>();
let themeObserver: MutationObserver | null = null;

function subscribeTheme(listener: () => void): () => void {
  themeListeners.add(listener);
  if (!themeObserver) {
    themeObserver = new MutationObserver(() => {
      for (const notify of themeListeners) notify();
    });
    themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
  }
  return () => {
    themeListeners.delete(listener);
    if (themeListeners.size === 0) {
      themeObserver?.disconnect();
      themeObserver = null;
    }
  };
}

/** Whether the page is painted dark now; follows the theme toggle. */
export function useDarkTheme(): boolean {
  return useSyncExternalStore(
    subscribeTheme,
    () => document.documentElement.classList.contains("dark"),
    () => false,
  );
}
