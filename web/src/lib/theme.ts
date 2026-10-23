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
