import { Menu } from "@base-ui-components/react/menu";
import { Check, Globe, Monitor, Moon, Sun } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { applyLanguage, browserLanguages, LANGUAGE_NAMES, pickLanguage, storedLanguagePref, type LanguagePref } from "@/i18n";
import { cn } from "@/lib/cn";
import { applyThemeMode, storedThemeMode, type ThemeMode } from "@/lib/theme";

/*
 * Language and theme for the public pages (the landing page, sign-in and
 * sign-up), at the right of their header beside the way in: two compact
 * 36px controls with hairline frames. Language opens a square menu: auto
 * (the browser's language, named beside it) and the four languages, each in
 * its own script; theme cycles auto → light → dark, as the app's own toggle
 * does.
 */

const NEXT_MODE: Record<ThemeMode, ThemeMode> = { auto: "light", light: "dark", dark: "auto" };
const MODE_ICONS: Record<ThemeMode, typeof Sun> = { auto: Monitor, light: Sun, dark: Moon };

const control = "flex h-9 shrink-0 items-center justify-center gap-1.5 border border-border-strong bg-bg text-muted hover:bg-hover hover:text-fg";

export function PrefsControls() {
  const { t, i18n } = useTranslation();
  const [theme, setTheme] = useState<ThemeMode>(() => {
    try {
      return storedThemeMode();
    } catch {
      return "auto";
    }
  });
  const [language, setLanguage] = useState<LanguagePref>(storedLanguagePref);
  const active = LANGUAGE_NAMES.find((l) => l.id === i18n.language) ?? LANGUAGE_NAMES[LANGUAGE_NAMES.length - 1]!;
  const browser = LANGUAGE_NAMES.find((l) => l.id === pickLanguage(browserLanguages()))!;
  const chooseLanguage = (value: LanguagePref) => {
    void applyLanguage(value);
    setLanguage(value);
  };
  const Icon = MODE_ICONS[theme];
  const themeLabel = t("theme: {{mode}} (click for {{next}})", { mode: t(theme), next: t(NEXT_MODE[theme]) });
  return (
    <div data-prefs="" className="flex shrink-0 items-center gap-2">
      <Menu.Root>
        <Menu.Trigger aria-label={t("language")} title={t("language")} className={cn(control, "px-2.5 font-mono text-2xs")}>
          <Globe className="size-3.5" />
          <span>{active.id}</span>
        </Menu.Trigger>
        <Menu.Portal>
          <Menu.Positioner side="bottom" align="end" sideOffset={4} className="z-50 outline-none">
            <Menu.Popup className="popup-motion max-h-[min(var(--available-height),20rem)] min-w-[10rem] max-w-[20rem] overflow-y-auto border border-border-strong bg-bg py-1.5 outline-none">
              <Menu.RadioGroup value={language} onValueChange={(value: LanguagePref) => chooseLanguage(value)}>
                <Menu.RadioItem
                  value="auto"
                  closeOnClick
                  className="flex min-h-8 w-full cursor-pointer items-center gap-2 px-3 py-1 text-left text-[13px] text-fg outline-none pointer-coarse:min-h-11 data-[highlighted]:bg-hover"
                >
                  <span className="flex size-3.5 shrink-0 items-center justify-center">
                    <Menu.RadioItemIndicator>
                      <Check className="size-3.5" />
                    </Menu.RadioItemIndicator>
                  </span>
                  {t("auto")}
                  <span lang={browser.id} className="text-faint">
                    {browser.label}
                  </span>
                </Menu.RadioItem>
                {LANGUAGE_NAMES.map((l) => (
                  <Menu.RadioItem
                    key={l.id}
                    value={l.id}
                    lang={l.id}
                    closeOnClick
                    className="flex min-h-8 w-full cursor-pointer items-center gap-2 px-3 py-1 text-left text-[13px] text-fg outline-none pointer-coarse:min-h-11 data-[highlighted]:bg-hover"
                  >
                    <span className="flex size-3.5 shrink-0 items-center justify-center">
                      <Menu.RadioItemIndicator>
                        <Check className="size-3.5" />
                      </Menu.RadioItemIndicator>
                    </span>
                    {l.label}
                  </Menu.RadioItem>
                ))}
              </Menu.RadioGroup>
            </Menu.Popup>
          </Menu.Positioner>
        </Menu.Portal>
      </Menu.Root>
      <button
        type="button"
        onClick={() => {
          const next = NEXT_MODE[theme];
          applyThemeMode(next);
          setTheme(next);
        }}
        title={themeLabel}
        aria-label={themeLabel}
        className={cn(control, "w-9")}
      >
        <Icon className="size-3.5" />
      </button>
    </div>
  );
}
