import type { MouseEvent, RefObject, SVGProps } from "react";
import { ArrowRightIcon, CaretDownIcon, CheckIcon, GlobeIcon, WindowsLogoIcon } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/cn";
import { AppleMark, LinuxMark } from "./brand-marks";
import { DOWNLOAD_PATH } from "./route";

/**
 * The systems Droppen runs on: one list, so the hero's split download button
 * (`release-files.tsx`), the download page's and its platform menu cannot
 * drift apart.
 */
export const PLATFORMS = ["macos", "linux", "windows"] as const;
export type Platform = (typeof PLATFORMS)[number];

/** Which systems have an app build of their own today. */
const APP_BUILD: Record<Platform, "current" | "soon"> = { macos: "current", linux: "soon", windows: "soon" };
const MENU_LABELS: Record<Platform, string> = { macos: "macOS", linux: "Linux", windows: "Windows" };

/**
 * A system's requirement, when it has one worth saying. "Coming soon" already
 * says why Linux and Windows cannot be picked, and the browser needs nothing
 * installed, so those rows say nothing more; a row without a note is one line.
 */
const MENU_NOTES: Record<Platform, string | null> = {
  macos: "macOS 13 or later",
  linux: null,
  windows: null,
};

const MENU_ITEM = "gap-2.5 py-2 text-left text-sm";

/** The platform's mark: Apple, Linux and GitHub come from Simple Icons; Windows from Phosphor. */
export function PlatformMark({ os, className, ...props }: { os: Platform } & SVGProps<SVGSVGElement>) {
  if (os === "macos") return <AppleMark className={className} {...props} />;
  if (os === "linux") return <LinuxMark className={className} {...props} />;
  return <WindowsLogoIcon weight="fill" aria-hidden="true" className={className} {...props} />;
}

function jumpToBrowser(event: MouseEvent<HTMLAnchorElement>) {
  const section = document.getElementById("browser");
  if (!section) return;
  event.preventDefault();
  section.scrollIntoView();
  history.replaceState(null, "", "#browser");
}

/**
 * The caret half of the segmented download button: macOS has the build, Linux
 * and Windows are marked "coming soon", and the browser needs nothing installed.
 *
 * `anchor` is the whole control, not the caret: the list then opens from the
 * button's left edge instead of hanging under the narrow caret segment.
 */
export function PlatformMenu({ anchor }: { anchor?: RefObject<HTMLDivElement | null> }) {
  const { t } = useTranslation();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        aria-label={t("Other platforms")}
        data-testid="platform-menu-trigger"
        className="group flex min-h-9 shrink-0 items-center justify-center border border-fg border-l-bg/25 bg-fg px-2.5 text-bg transition-colors hover:bg-fg/85 data-[popup-open]:bg-fg/85"
      >
        <CaretDownIcon aria-hidden="true" className="size-4 transition-transform duration-150 group-data-[popup-open]:rotate-180" />
      </DropdownMenuTrigger>
      <DropdownMenuContent side="bottom" align="start" sideOffset={4} anchor={anchor} data-testid="platform-menu" className="w-60 py-1">
        {PLATFORMS.map((id) => {
          const current = APP_BUILD[id] === "current";
          const note = MENU_NOTES[id];
          return (
            <DropdownMenuItem key={id} disabled={!current} data-testid={`hero-os-${id}`} className={cn(MENU_ITEM, "items-start")}>
              <PlatformMark os={id} className="mt-0.5 size-4 shrink-0" />
              <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                <span className={cn("truncate", current && "font-medium")}>{t(MENU_LABELS[id])}</span>
                {note && <span className="text-xs leading-4 text-muted">{t(note)}</span>}
              </span>
              {current ? (
                <CheckIcon aria-hidden="true" className="mt-0.5 size-3.5 shrink-0" />
              ) : (
                <span className="mt-0.5 shrink-0 text-xs">{t("Coming soon")}</span>
              )}
            </DropdownMenuItem>
          );
        })}
        <DropdownMenuItem
          render={<a href={`${DOWNLOAD_PATH}#browser`} onClick={jumpToBrowser} />}
          data-testid="hero-browser"
          className={cn(MENU_ITEM, "border-t border-border")}
        >
          <GlobeIcon aria-hidden="true" className="size-4 shrink-0" />
          <span className="flex-1 font-medium">{t("Browser")}</span>
          <ArrowRightIcon aria-hidden="true" className="size-3.5 shrink-0" />
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
