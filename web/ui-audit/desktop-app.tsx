/*
 * Settings → Desktop app (`features/settings/desktop-app-section.tsx`), as the
 * app's own window shows it, from fixed state and no server: the section for
 * each system (`&platform=darwin|linux`), with or without a tray host
 * (`&tray=1|0`) and with one of the Start at Login states (`&state=on|off|
 * approval|translocated|no-login`), at `&lang=`.
 *
 * The state is what the app pushes over its bridge (`init.js`, part 9); the
 * fixture's bridge records nothing and asks for nothing.
 */
import { createRoot } from "react-dom/client";
import { DesktopAppSection } from "@/features/settings/desktop-app-section";
import { applyLanguage, type LanguagePref } from "@/i18n";

/** What the app says of Start at Login, per state; `no-login` offers none. */
const STATES: Record<string, { checked: boolean; enabled: boolean; note: string | null }> = {
  on: { checked: true, enabled: true, note: null },
  off: { checked: false, enabled: true, note: null },
  approval: { checked: true, enabled: true, note: "needsApproval" },
  failed: { checked: false, enabled: true, note: "turnOnFailed" },
  translocated: { checked: false, enabled: false, note: null },
  "no-login": { checked: false, enabled: true, note: null },
};

export async function mountDesktopApp(query: URLSearchParams): Promise<void> {
  const platform = query.get("platform") ?? "darwin";
  const tray = query.get("tray") ?? "1";
  const state = STATES[query.get("state") ?? "on"] ?? STATES.on;
  const lang = (query.get("lang") as LanguagePref | null) ?? "en";
  Object.defineProperty(window, "__droppenDesktop", {
    value: Object.freeze({
      startAtLogin: query.get("state") !== "no-login",
      trayHost: tray !== "0",
      settingsState: () => state,
      openInBrowser: () => undefined,
      setStartAtLogin: () => undefined,
      openLoginItems: () => undefined,
      refreshDesktopSettings: () => undefined,
    }),
  });
  await applyLanguage(lang);
  createRoot(document.getElementById("root")!).render(
    <div data-fixture-root="" data-system={platform} data-tray={tray} className="@container/settings mx-auto max-w-3xl p-4">
      <header className="mb-3 flex h-8 items-center border-b border-border px-4 text-[13px] font-semibold">Settings · Desktop app</header>
      <DesktopAppSection />
    </div>,
  );
}
