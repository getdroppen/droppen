import { type ReactNode, useEffect, useRef, useState } from "react";
import { Tabs } from "@base-ui/react/tabs";
import { useTranslation } from "react-i18next";
import { useIntersectionObserver, useInterval } from "usehooks-ts";
import { cn } from "@/lib/cn";
import { usePrefersReducedMotion } from "@/lib/reduced-motion";
import { useDarkTheme } from "@/lib/theme";

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
    <div className="overflow-hidden border border-border-strong bg-bg">
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

/** One screen of the tour: its caption and the picture behind it. */
export type TourScreen = {
  /** A stable name for the screen, used for its ids and its indicator button. */
  value: string;
  /** The caption's headline and the line that says what the screen is for. */
  title: string;
  body: string;
  /** The screenshot, framed as the device it was taken on. */
  shot: string;
  alt: string;
  device: "app" | "phone";
};

/** How long one screen stays before the next one, when the tour is free to move on. */
const ROTATE_MS = 6000;

/** Where the tour is, announced to a screen reader as it moves on by itself. */
function Live({ index, total }: { index: number; total: number }) {
  const { t } = useTranslation();
  return (
    <span aria-live="polite" className="sr-only">
      {t("Screen {{n}} of {{total}}", { n: index + 1, total })}
    </span>
  );
}

/**
 * The product tour: three screens, one at a time. The picture is the subject; the
 * caption above it is the one belonging to the picture on screen, and nothing else of
 * the set is written on the page. The tour moves on by itself every few seconds at every
 * width — it stops while the reader points at it or is focused in it, while the tab is
 * hidden, while it is scrolled away, and when the reader asked the system for less
 * motion. Small capsule marks under the picture choose a screen directly, and a
 * horizontal touch swipe moves to the next or previous one.
 */
export function ProductTour({ screens }: { screens: readonly TourScreen[] }) {
  const { t } = useTranslation();
  const [value, setValue] = useState(screens[0]!.value);
  const touchStart = useRef<{ x: number; y: number } | null>(null);
  /** The reader is pointing at or focused in the tour, so the rotation waits. */
  const [held, setHeld] = useState(false);
  const reduced = usePrefersReducedMotion();
  /** A background tab keeps the rotation for later. */
  const [away, setAway] = useState(false);
  const [inViewRef, inView] = useIntersectionObserver({ threshold: 0.35 });
  const index = Math.max(
    0,
    screens.findIndex((screen) => screen.value === value),
  );
  const screen = screens[index]!;
  // The stage has a fixed height at every width (see below), so moving on by itself never
  // moves the page under the reader's thumb.
  const rotating = !reduced && !held && !away && inView;

  useEffect(() => {
    const onChange = () => setAway(document.hidden);
    onChange();
    document.addEventListener("visibilitychange", onChange);
    return () => document.removeEventListener("visibilitychange", onChange);
  }, []);

  useInterval(
    () => setValue((current) => screens[(screens.findIndex((entry) => entry.value === current) + 1) % screens.length]!.value),
    rotating ? ROTATE_MS : null,
  );

  const choose = (next: number) => setValue(screens[(next + screens.length) % screens.length]!.value);
  /* The picture is the subject: it takes the full column, with three capsule marks under
     it and the caption above it. */
  const stage = (
    <div ref={inViewRef} className="relative isolate">
      <div className="relative grid place-items-center lg:h-full">
        {screens.map((entry) => (
          <Tabs.Panel
            key={entry.value}
            value={entry.value}
            keepMounted
            className="col-start-1 row-start-1 w-full place-self-center transition-opacity duration-300 ease-out data-[ending-style]:opacity-0 data-[starting-style]:opacity-0 motion-reduce:transition-none"
          >
            {entry.device === "phone" ? (
              <div className="mx-auto w-full max-w-[15rem]">
                <Phone>
                  <ProductShot shot={entry.shot} alt={entry.alt} eager width={640} height={1386} />
                </Phone>
              </div>
            ) : (
              <AppWindow>
                <ProductShot shot={entry.shot} alt={entry.alt} eager width={2560} height={1600} />
              </AppWindow>
            )}
          </Tabs.Panel>
        ))}
      </div>
    </div>
  );

  /* Three compact capsules, not another piece of chrome. The generous button
     around each mark keeps it easy to tap without making it look heavy. */
  const rail = (
    <Tabs.List aria-label={t("Choose a screen")} className="flex items-center justify-center gap-1">
      {screens.map((entry, at) => (
        <Tabs.Tab
          key={entry.value}
          value={entry.value}
          aria-label={t("Screen {{n}} of {{total}}: {{title}}", { n: at + 1, total: screens.length, title: entry.title })}
          className="group/tick flex size-7 cursor-pointer items-center justify-center focus-visible:outline-offset-0"
        >
          <span aria-hidden="true" data-shape="" className="block h-1 w-3 rounded-full bg-border-strong transition-[width,background-color] duration-200 group-hover/tick:bg-muted group-data-[active]:w-6 group-data-[active]:bg-fg" />
        </Tabs.Tab>
      ))}
    </Tabs.List>
  );

  /* A short, stable caption above the picture; the images and their small marks
     are the carousel, with no extra arrow chrome. */
  const caption = (
    <div className="min-h-20 min-w-0">
      <p id={`tour-${screen.value}`} className="text-xl leading-7 font-semibold tracking-tight text-balance text-fg cjk-wrap md:cjk-keep">{screen.title}</p>
      <p className="mt-3 max-w-[44rem] text-sm leading-6 text-pretty text-muted">{screen.body}</p>
    </div>
  );

  return (
    <Tabs.Root
      value={value}
      onValueChange={(next) => setValue(String(next))}
      onPointerEnter={() => setHeld(true)}
      onPointerLeave={() => setHeld(false)}
      onFocusCapture={() => setHeld(true)}
      onBlurCapture={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setHeld(false);
      }}
      onTouchStart={(event) => {
        const touch = event.touches[0];
        touchStart.current = touch ? { x: touch.clientX, y: touch.clientY } : null;
      }}
      onTouchEnd={(event) => {
        const start = touchStart.current;
        const touch = event.changedTouches[0];
        touchStart.current = null;
        if (!start || !touch) return;
        const x = touch.clientX - start.x;
        const y = touch.clientY - start.y;
        // Horizontal intent only: an ordinary vertical scroll must not change the screen.
        if (Math.abs(x) > 48 && Math.abs(x) > Math.abs(y)) choose(index + (x < 0 ? 1 : -1));
      }}
      className="grid touch-pan-y gap-y-5"
    >
      <div className="min-w-0">{caption}</div>
      <div className="min-w-0">
        {stage}
        <div className="mt-2">{rail}</div>
      </div>
      <Live index={index} total={screens.length} />
    </Tabs.Root>
  );
}
