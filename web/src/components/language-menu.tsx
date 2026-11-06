/*
 * The language menu: auto (the browser's language, named beside it) and the
 * four languages, each in its own script. Shared by the public pages' header
 * and the setup guide's top bar; those differ only in the trigger they draw
 * and in how roomy the popup is.
 */
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { DropdownMenu, DropdownMenuContent, DropdownMenuRadioGroup, DropdownMenuRadioItem } from "@/components/ui/dropdown-menu";
import { browserLanguages, LANGUAGE_NAMES, pickLanguage, type LanguagePref } from "@/i18n";

/** The popup's two looks: the public pages' roomy one, the guide's compact one. */
const LOOK = {
  public: {
    popup: undefined,
    item: "gap-2 text-sm",
  },
  guide: {
    popup: "min-w-36 py-1",
    item: "gap-2",
  },
} as const;

/** The language the UI is in; a code outside the table reads as English (the last entry). */
export function activeLanguage(language: string): { id: LanguagePref; label: string } {
  return LANGUAGE_NAMES.find((l) => l.id === language) ?? LANGUAGE_NAMES[LANGUAGE_NAMES.length - 1]!;
}

/** The language the browser prefers, shown in its own script beside "auto". */
function browserLanguage(): { id: LanguagePref; label: string } {
  return LANGUAGE_NAMES.find((l) => l.id === pickLanguage(browserLanguages()))!;
}

/** The menu itself; `children` is the caller's trigger. */
export function LanguageMenu({
  value,
  onChoose,
  look,
  children,
}: {
  value: LanguagePref;
  onChoose: (language: LanguagePref) => void;
  look: keyof typeof LOOK;
  children: ReactNode;
}) {
  const { t } = useTranslation();
  const browser = browserLanguage();
  return (
    <DropdownMenu>
      {children}
      <DropdownMenuContent align="end" side="bottom" sideOffset={4} className={LOOK[look].popup}>
        <DropdownMenuRadioGroup value={value} onValueChange={(next: LanguagePref) => onChoose(next)}>
          <DropdownMenuRadioItem value="auto" closeOnClick className={LOOK[look].item}>
            {t("auto")}
            <span lang={browser.id} className="text-faint">
              {browser.label}
            </span>
          </DropdownMenuRadioItem>
          {LANGUAGE_NAMES.map((l) => (
            <DropdownMenuRadioItem key={l.id} value={l.id} lang={l.id} closeOnClick className={LOOK[look].item}>
              {l.label}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
