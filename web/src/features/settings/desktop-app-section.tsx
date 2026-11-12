import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import {
  type DesktopSettingsState,
  desktopSettingsState,
  isDesktopWindow,
  openInBrowser,
  openLoginItems,
  refreshDesktopSettings,
  setDesktopStartAtLogin,
  startAtLoginOffered,
} from "@/lib/desktop-app";
import { SettingRow, SettingsNote, SettingsSection } from "./layout";

/** The app's Start at Login state, following what it pushes; null before the first push. */
function useDesktopSettings(): DesktopSettingsState | null {
  const [state, setState] = useState<DesktopSettingsState | null>(desktopSettingsState);
  useEffect(() => {
    const read = () => setState(desktopSettingsState());
    read();
    window.addEventListener("droppen:settings", read);
    // The platform can change it while the page is open (System Settings →
    // Login Items): read again when the section opens and the window returns.
    refreshDesktopSettings();
    const onFocus = () => refreshDesktopSettings();
    window.addEventListener("focus", onFocus);
    return () => {
      window.removeEventListener("droppen:settings", read);
      window.removeEventListener("focus", onFocus);
    };
  }, []);
  return state;
}

/**
 * Settings → Desktop app: the app's own actions, in the app's own window — the
 * tray's *Open in Browser* and *Start at Login*, with or without a tray host
 * (stock GNOME has none). The page is the server's, so what it asks rides the
 * bridge its init script sets up (`desktop/src-tauri/src/init.js`, part 9):
 * four navigations the app cancels, with a token the script keeps in a
 * closure, and the state the app pushes back. A browser on the device has no
 * bridge and never shows this section (`sections.ts`).
 *
 * Whether Start at Login is on is the app's to say, not the page's: the switch
 * draws the pushed state. A turn-on macOS takes but needs approved shows the
 * Login Items line the tray shows, and an App Translocation copy blocks the
 * switch with the move note. An app that does not offer Start at Login (an
 * end-to-end build on Linux) has no row at all.
 */
export function DesktopAppSection() {
  const { t } = useTranslation();
  const state = useDesktopSettings();
  if (!isDesktopWindow()) return null;
  const offered = startAtLoginOffered();
  const blocked = state ? !state.enabled : true;
  return (
    <SettingsSection title={t("Desktop app")} data-testid="desktop-app-section">
      <SettingRow
        data-audit-row="desktop-open"
        data-testid="desktop-open-in-browser"
        label={t("Open in Browser")}
        hint={t("Open this device's Droppen page in your browser, signed in.")}
      >
        <Button variant="secondary" onClick={() => openInBrowser()}>
          {t("Open in Browser")}
        </Button>
      </SettingRow>
      {offered && state && (
        <SettingRow
          data-audit-row="desktop-login"
          data-testid="desktop-start-at-login-row"
          label={t("Start Droppen at login")}
          hint={t("Starts Droppen when you log in, so this device stays reachable after a restart.")}
          labelExtra={
            <>
              {!state.enabled && (
                <SettingsNote mono={false} className="mt-0.5 block">
                  {t("Move Droppen to Applications first.")}
                </SettingsNote>
              )}
              {state.note && (
                <SettingsNote tone="warn" mono={false} className="mt-1 flex flex-wrap items-center gap-2">
                  {t("Allow Droppen in Login Items.")}
                  <Button variant="secondary" onClick={() => openLoginItems()}>
                    {t("Open Login Items")}
                  </Button>
                </SettingsNote>
              )}
            </>
          }
        >
          <Switch
            on={state.checked}
            blocked={blocked}
            label={t("Start Droppen at login")}
            labelHidden
            onChange={(on) => setDesktopStartAtLogin(on)}
            testId="desktop-start-at-login"
          />
        </SettingRow>
      )}
    </SettingsSection>
  );
}
