/*
 * The language menu: auto (the browser's language, named beside it) and the
 * four languages, each in its own script. Shared by the public pages' header
 * and the setup guide's top bar; those differ only in the trigger they draw
 * and in how roomy the popup is.
 */
import { Menu } from "@base-ui-components/react/menu";
import { Check } from "lucide-react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { browserLanguages, LANGUAGE_NAMES, pickLanguage, type LanguagePref } from "@/i18n";

/** The popup's two looks: the public pages' roomy one, the guide's compact one. */
const LOOK = {
  public: {
    popup:
      "popup-motion max-h-[min(var(--available-height),20rem)] min-w-[10rem] max-w-[20rem] overflow-y-auto border border-border-strong bg-bg py-1.5 outline-none",
    item: "flex min-h-8 w-full cursor-pointer items-center gap-2 px-3 py-1 text-left text-[13px] text-fg outline-none pointer-coarse:min-h-11 data-[highlighted]:bg-hover",
  },
  guide: {
    popup: "min-w-36 border border-border-strong bg-bg py-1 outline-none",
    item: "flex w-full cursor-pointer items-center gap-2 px-3 py-1 text-left text-xs text-fg outline-none data-[highlighted]:bg-hover",
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

/**
 * The menu itself; `children` is the caller's `Menu.Trigger`, which keeps its
 * own size and chrome (a 36px framed control on the public pages, a 24px bar
 * button in the guide).
 */
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
    <Menu.Root>
      {children}
      <Menu.Portal>
        <Menu.Positioner side="bottom" align="end" sideOffset={4} className="z-50 outline-none">
          <Menu.Popup className={LOOK[look].popup}>
            <Menu.RadioGroup value={value} onValueChange={(next: LanguagePref) => onChoose(next)}>
              <Menu.RadioItem value="auto" closeOnClick className={LOOK[look].item}>
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
                <Menu.RadioItem key={l.id} value={l.id} lang={l.id} closeOnClick className={LOOK[look].item}>
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
  );
}
