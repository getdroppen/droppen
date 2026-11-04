import { type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Lock } from "lucide-react";
import { cn } from "@/lib/cn";
import { useDarkTheme } from "@/lib/theme";

/* Real screenshots in HTML-drawn frames; each is per language and theme. */

export const TOUR_TABS = ["sessions", "split", "terminal", "cloud", "phone", "security"] as const;
export type TourTab = (typeof TOUR_TABS)[number];

type FrameKind = "app" | "browser" | "phone";

export const TOUR_SHOTS: Record<TourTab, { frame: FrameKind; shots: string[] }> = {
  sessions: { frame: "app", shots: ["app-session"] },
  split: { frame: "app", shots: ["app-split"] },
  terminal: { frame: "app", shots: ["app-terminal"] },
  cloud: { frame: "browser", shots: ["cloud-devices"] },
  phone: { frame: "phone", shots: ["phone-needs-you", "phone-session"] },
  security: { frame: "phone", shots: ["security-pair"] },
};

export function tourLanguage(language: string): "zh-CN" | "en" {
  return language.startsWith("zh") ? "zh-CN" : "en";
}

export function tourShot(shot: string, language: string, theme: "light" | "dark"): string {
  return `/tour/${shot}-${tourLanguage(language)}-${theme}.webp`;
}

export function ProductShot({
  shot,
  alt,
  className,
  eager,
  width,
  height,
}: {
  shot: string;
  alt: string;
  className?: string;
  eager?: boolean;
  width: number;
  height: number;
}) {
  const { i18n } = useTranslation();
  const dark = useDarkTheme();
  // Only the theme on screen is fetched eagerly; the other waits, hidden, so the page does not pay for two 2560px images at first load.
  const loading = (shown: boolean) => (eager && shown ? "eager" : "lazy");
  return (
    <>
      <img
        src={tourShot(shot, i18n.language, "light")}
        alt={alt}
        width={width}
        height={height}
        loading={loading(!dark)}
        decoding="async"
        className={cn("block h-auto w-full dark:hidden", className)}
      />
      <img
        src={tourShot(shot, i18n.language, "dark")}
        alt={alt}
        width={width}
        height={height}
        loading={loading(dark)}
        decoding="async"
        className={cn("hidden h-auto w-full dark:block", className)}
      />
    </>
  );
}

export function AppWindow({ children }: { children: ReactNode }) {
  return (
    <div className="overflow-hidden rounded-[10px] border border-border-strong bg-bg shadow-[0_24px_60px_-28px_rgb(0_0_0/0.35)]">
      <div className="relative flex h-8 items-center gap-2 bg-raised px-3">
        {[0, 1, 2].map((i) => (
          <span key={i} className="size-3 rounded-full border border-border-strong bg-bg" />
        ))}
        <span className="absolute inset-x-0 text-center text-xs font-medium text-muted">Droppen</span>
      </div>
      {children}
    </div>
  );
}

export function BrowserWindow({ children }: { children: ReactNode }) {
  return (
    <div className="overflow-hidden rounded-[10px] border border-border-strong bg-bg shadow-[0_24px_60px_-28px_rgb(0_0_0/0.35)]">
      <div className="flex h-9 items-center gap-3 bg-raised px-3">
        <span className="flex gap-2">
          {[0, 1, 2].map((i) => (
            <span key={i} className="size-3 rounded-full border border-border-strong bg-bg" />
          ))}
        </span>
        <span className="mx-auto flex h-6 w-full max-w-[22rem] items-center justify-center gap-1.5 rounded-md border border-border bg-bg font-mono text-xs text-muted">
          <Lock className="size-3" />
          my.droppen.app
        </span>
        <span className="w-[52px] max-sm:hidden" />
      </div>
      {children}
    </div>
  );
}

export function Phone({ children }: { children: ReactNode }) {
  return (
    <div className="w-full max-w-[340px] rounded-[44px] border border-border-strong bg-raised p-[9px] shadow-[0_24px_60px_-28px_rgb(0_0_0/0.35)]">
      <div className="relative overflow-hidden rounded-[36px] border border-border bg-bg">
        <div aria-hidden="true" className="relative flex h-9 items-center justify-between px-6 text-xs font-semibold text-fg tabular-nums">
          <span>9:41</span>
          <span className="absolute top-2 left-1/2 h-[22px] w-[78px] -translate-x-1/2 rounded-full bg-fg" />
          {/* Signal and battery, drawn as the system draws them: bars and a pill. */}
          <span className="flex items-center gap-1">
            <span className="flex items-end gap-px">
              {[4, 6, 8, 10].map((h) => (
                <span key={h} className="w-[3px] rounded-[1px] bg-fg" style={{ height: h }} />
              ))}
            </span>
            <span className="ml-1 flex h-[10px] w-[20px] items-center rounded-[3px] border border-fg p-px">
              <span className="h-full w-3/4 rounded-[1px] bg-fg" />
            </span>
          </span>
        </div>
        {children}
      </div>
    </div>
  );
}
