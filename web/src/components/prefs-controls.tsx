import { Globe } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { activeLanguage, LanguageMenu } from "@/components/language-menu";
import { Button, buttonVariants } from "@/components/ui/button";
import { DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { applyLanguage, storedLanguagePref, type LanguagePref } from "@/i18n";
import { cn } from "@/lib/cn";
import { applyThemeMode, MODE_ICONS, NEXT_MODE, storedThemeMode, type ThemeMode } from "@/lib/theme";

/*
 * Language and theme for the public pages (the landing page, sign-in and
 * sign-up), at the right of their header beside the way in: two compact
 * 36px controls with hairline frames. Language opens a square menu: auto
 * (the browser's language, named beside it) and the four languages, each in
 * its own script; theme cycles auto → light → dark, as the app's own toggle
 * does.
 */

const control = cn(
  buttonVariants({ variant: "secondary" }),
  "flex h-9 p-0 text-muted transition-colors hover:text-fg data-[popup-open]:bg-hover data-[popup-open]:text-fg",
);

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
  const active = activeLanguage(i18n.language);
  const chooseLanguage = (value: LanguagePref) => {
    void applyLanguage(value);
    setLanguage(value);
  };
  const Icon = MODE_ICONS[theme];
  const themeLabel = t("theme: {{mode}} (click for {{next}})", { mode: t(theme), next: t(NEXT_MODE[theme]) });
  const langLabel = `${t("language")}: ${active.label}`;
  return (
    <div data-prefs="" className="flex shrink-0 items-center gap-2">
      <LanguageMenu value={language} onChoose={chooseLanguage} look="public">
        <DropdownMenuTrigger aria-label={langLabel} title={langLabel} className={cn(control, "w-9")}>
          <Globe className="size-3.5" />
        </DropdownMenuTrigger>
      </LanguageMenu>
      <Button
        variant="secondary"
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
      </Button>
    </div>
  );
}
