import { type ReactNode, useEffect, useState } from "react";
import { Tabs } from "@base-ui/react/tabs";
import { ArrowLeft, ArrowRight, Pause, Play } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useIntersectionObserver, useInterval, useMediaQuery } from "usehooks-ts";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/cn";
import { usePrefersReducedMotion } from "@/lib/reduced-motion";
import { useDarkTheme } from "@/lib/theme";
import { gridPaper } from "./figures/paper";

/* Real screenshots in HTML-drawn frames; each is per language and theme. */

/** Every screenshot the build ships, for the test that checks none is missing; the page picks the ones it draws. */
export const TOUR_SHOTS = ["app-session", "app-split", "app-terminal", "cloud-devices", "phone-needs-you", "phone-session", "security-pair"] as const;

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

/** One screen of the tour: its index entry and the picture behind it. */
export type TourScreen = {
  value: string;
  /** The entry's headline and the line that says what the screen is for. */
  title: string;
  body: string;
  /** The screenshot, framed as the device it was taken on. */
  shot: string;
  alt: string;
  device: "app" | "phone";
};

/** How long one screen stays before the next one, when nothing holds it. */
const ROTATE_MS = 6000;

/**
 * The product tour: three screens, one at a time. The index at the left names
 * each one and says what it is for; the frame at the right shows it, framed as
 * the device it was taken on. It moves on by itself every few seconds until
 * the reader points at it, focuses it or stops it — or asks the system for less
 * motion — and the index, the arrow keys and the buttons beside it move it by
 * hand, one screen at a time.
 */
export function ProductTour({ screens }: { screens: readonly TourScreen[] }) {
  const { t } = useTranslation();
  const [value, setValue] = useState(screens[0]!.value);
  /** The reader is pointing at or focused in the tour, so the rotation waits. */
  const [held, setHeld] = useState(false);
  /** The reader stopped the rotation with its button: only they start it again. */
  const [stopped, setStopped] = useState(false);
  const reduced = usePrefersReducedMotion();
  /**
   * The two-column layout (the stage beside the index) has a fixed height, so a
   * screen swap moves nothing on the page. Below it the frame is as tall as the
   * screen it shows, and a carousel that moved the page on its own would be a
   * nuisance: there the tour waits for the reader's tabs and arrows.
   */
  const sideBySide = useMediaQuery("(min-width: 64rem)", { defaultValue: true });
  /** A background tab, or the tour scrolled away, keeps the rotation for later. */
  const [away, setAway] = useState(false);
  const [inViewRef, inView] = useIntersectionObserver({ threshold: 0.35 });
  const index = Math.max(
    0,
    screens.findIndex((screen) => screen.value === value),
  );
  const rotating = sideBySide && !reduced && !held && !stopped && !away && inView;
  const counter = (n: number) => String(n).padStart(2, "0");

  useEffect(() => {
    const onChange = () => setAway(document.hidden);
    onChange();
    document.addEventListener("visibilitychange", onChange);
    return () => document.removeEventListener("visibilitychange", onChange);
  }, []);

  useInterval(
    () => setValue((current) => screens[(screens.findIndex((screen) => screen.value === current) + 1) % screens.length]!.value),
    rotating ? ROTATE_MS : null,
  );

  const step = (delta: number) => setValue(screens[(index + delta + screens.length) % screens.length]!.value);

  return (
    <Tabs.Root
      value={value}
      onValueChange={(next) => setValue(String(next))}
      orientation="vertical"
      onPointerEnter={() => setHeld(true)}
      onPointerLeave={() => setHeld(false)}
      onFocusCapture={() => setHeld(true)}
      onBlurCapture={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setHeld(false);
      }}
      className="grid gap-10 max-md:gap-8 lg:grid-cols-[minmax(0,0.9fr)_minmax(0,1.1fr)] lg:items-center lg:gap-16"
    >
      <Tabs.List aria-label={t("Choose a screen")} activateOnFocus className="flex flex-col border-t border-border">
        {screens.map((screen, at) => (
          <Tabs.Tab
            key={screen.value}
            value={screen.value}
            aria-label={screen.title}
            aria-describedby={`tour-${screen.value}`}
            className={cn(
              "group flex w-full items-start gap-4 border-b border-l-2 border-l-transparent border-b-border py-5 pr-2 pl-4 text-left transition-colors",
              "hover:border-l-border-strong data-[active]:border-l-fg",
            )}
          >
            <span aria-hidden="true" className="mt-0.5 font-mono text-xs text-faint tabular-nums group-hover:text-muted group-data-[active]:text-fg">
              {counter(at + 1)}
            </span>
            <span className="flex min-w-0 flex-col gap-1.5">
              <span className="text-lg leading-6 font-semibold tracking-tight text-balance text-muted group-hover:text-fg group-data-[active]:text-fg cjk-wrap md:cjk-keep">{screen.title}</span>
              <span id={`tour-${screen.value}`} className="text-sm leading-6 text-pretty text-faint group-data-[active]:text-muted">
                {screen.body}
              </span>
            </span>
          </Tabs.Tab>
        ))}
      </Tabs.List>

      <div className="min-w-0">
        <div ref={inViewRef} className="relative isolate lg:h-[36rem]">
          <div
            aria-hidden="true"
            className="pointer-events-none absolute inset-0"
            style={{
              ...gridPaper(22),
              maskImage: "radial-gradient(72% 82% at 50% 48%, #000 28%, transparent 100%)",
              WebkitMaskImage: "radial-gradient(72% 82% at 50% 48%, #000 28%, transparent 100%)",
              opacity: 0.7,
            }}
          />
          <div className="relative grid h-full place-items-center px-5 py-6 lg:px-8 lg:py-0">
            {screens.map((screen) => (
              <Tabs.Panel
                key={screen.value}
                value={screen.value}
                keepMounted
                className="col-start-1 row-start-1 w-full place-self-center transition-opacity duration-200 ease-out data-[ending-style]:opacity-0 data-[starting-style]:opacity-0 motion-reduce:transition-none"
              >
                {screen.device === "phone" ? (
                  <div className="mx-auto w-full max-w-[250px]">
                    <Phone>
                      <ProductShot shot={screen.shot} alt={screen.alt} eager width={640} height={1386} />
                    </Phone>
                  </div>
                ) : (
                  <AppWindow>
                    <ProductShot shot={screen.shot} alt={screen.alt} eager width={2560} height={1600} />
                  </AppWindow>
                )}
              </Tabs.Panel>
            ))}
          </div>
        </div>
        <div className="mt-2 flex items-center justify-between gap-4 border-t border-border pt-4">
          <span aria-hidden="true" className="font-mono text-xs text-muted tabular-nums">
            {counter(index + 1)} / {counter(screens.length)}
          </span>
          <div className="flex items-center gap-2">
            {!reduced && sideBySide && (
              <Button
                variant="secondary"
                size="lg"
                className="size-9 px-0"
                aria-label={stopped ? t("Play the rotation") : t("Pause the rotation")}
                onClick={() => setStopped((was) => !was)}
              >
                {stopped ? <Play aria-hidden="true" /> : <Pause aria-hidden="true" />}
              </Button>
            )}
            <Button variant="secondary" size="lg" className="size-9 px-0" aria-label={t("Previous screen")} onClick={() => step(-1)}>
              <ArrowLeft aria-hidden="true" />
            </Button>
            <Button variant="secondary" size="lg" className="size-9 px-0" aria-label={t("Next screen")} onClick={() => step(1)}>
              <ArrowRight aria-hidden="true" />
            </Button>
          </div>
        </div>
      </div>
    </Tabs.Root>
  );
}
