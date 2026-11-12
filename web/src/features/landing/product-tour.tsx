import { type CSSProperties, type PointerEvent, type ReactNode, useEffect, useRef, useState } from "react";
import { Tabs } from "@base-ui/react/tabs";
import { useTranslation } from "react-i18next";
import { useIntersectionObserver, useInterval } from "usehooks-ts";
import { cn } from "@/lib/cn";
import { usePrefersReducedMotion } from "@/lib/reduced-motion";
import { useDarkTheme } from "@/lib/theme";

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
  priority,
  width,
  height,
}: {
  shot: string;
  alt: string;
  className?: string;
  eager?: boolean;
  priority?: boolean;
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
        fetchPriority={priority && !dark ? "high" : "low"}
        decoding="async"
        className={cn("block h-auto w-full dark:hidden", className)}
      />
      <img
        src={tourShot(shot, i18n.language, "dark")}
        alt={alt}
        width={width}
        height={height}
        loading={loading(dark)}
        fetchPriority={priority && dark ? "high" : "low"}
        decoding="async"
        className={cn("hidden h-auto w-full dark:block", className)}
      />
    </>
  );
}

export function AppWindow({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div className={cn("overflow-hidden border border-border-strong bg-bg", className)}>
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

function StatusMarks({ android = false }: { android?: boolean }) {
  return (
    <span aria-hidden="true" className="flex items-center gap-1">
      <span className="flex items-end gap-px">
        {[4, 6, 8, 10].map((h) => (
          <span key={h} className="w-[3px] rounded-[1px] bg-fg" style={{ height: h }} />
        ))}
      </span>
      {android ? (
        <svg viewBox="0 0 16 12" className="size-3 fill-fg">
          <path d="M8 11.2 5.4 8.3a3.7 3.7 0 0 1 5.2 0L8 11.2Zm-4.4-4.9a7.4 7.4 0 0 1 8.8 0l1.2-1.4a9.4 9.4 0 0 0-11.2 0l1.2 1.4Z" />
        </svg>
      ) : (
        <svg viewBox="0 0 16 12" className="size-3 fill-fg">
          <path d="M8 11.2 5.4 8.3a3.7 3.7 0 0 1 5.2 0L8 11.2Zm-2.9-3.2-1-1.1a6 6 0 0 1 7.8 0l-1 1.1a4.5 4.5 0 0 0-5.8 0Zm-2-2.2-1-1.1a9 9 0 0 1 11.8 0l-1 1.1a7.5 7.5 0 0 0-9.8 0Z" />
        </svg>
      )}
      <span className={android ? "ml-0.5 flex h-[10px] w-[6px] flex-col items-center" : "ml-1 flex h-[10px] w-[20px] items-center rounded-[3px] border border-fg p-px"}>
        {android ? (
          <>
            <span className="h-[2px] w-[3px] bg-fg" />
            <span className="mt-px h-full w-full rounded-[1px] border border-fg p-px">
              <span className="block h-full w-3/4 bg-fg" />
            </span>
          </>
        ) : (
          <span className="h-full w-3/4 rounded-[1px] bg-fg" />
        )}
      </span>
    </span>
  );
}

/** The screen is 9:19.5, the ratio of the shots in `public/tour`. */
function IPhone({ children, className }: { children: ReactNode; className?: string }) {
  return (
    // The shell is its 6px rim around a 9:19.5 screen (the shots' ratio). Given only a
    // height, a shell takes its width from that ratio, so two of them can be fitted into
    // the tour's frame; given only a width, they fill it and keep the same proportions.
    <div className={cn("relative aspect-[657/1440] rounded-[3rem] border border-border-strong bg-raised p-[6px] shadow-[0_24px_60px_-28px_rgb(0_0_0/0.35)] w-full", className)}>
      <span aria-hidden="true" className="absolute top-[22%] -left-px h-[7%] w-[2px] rounded-l bg-border-strong" />
      <span aria-hidden="true" className="absolute top-[33%] -left-px h-[11%] w-[2px] rounded-l bg-border-strong" />
      <span aria-hidden="true" className="absolute top-[47%] -left-px h-[11%] w-[2px] rounded-l bg-border-strong" />
      <span aria-hidden="true" className="absolute top-[36%] -right-px h-[16%] w-[2px] rounded-r bg-border-strong" />
      <div className="absolute inset-[6px] overflow-hidden rounded-[2.6rem] border border-border bg-bg">
        {/* The bar keeps its two ends clear of the island: the island is a share of the
            shell's width, so a narrow shell gets a narrow island rather than one that
            swallows the clock. */}
        <div aria-hidden="true" className="relative flex h-8 items-center justify-between px-4 text-xs font-semibold text-fg tabular-nums">
          <span>9:41</span>
          <span className="absolute top-1/2 left-1/2 h-[1.1em] w-[38%] max-w-[68px] -translate-x-1/2 -translate-y-1/2 rounded-full bg-fg" />
          <StatusMarks />
        </div>
        {children}
      </div>
    </div>
  );
}

export function Android({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div className={cn("relative aspect-[657/1440] rounded-[2rem] border border-border-strong bg-raised p-[6px] shadow-[0_24px_60px_-28px_rgb(0_0_0/0.35)] w-full", className)}>
      <span aria-hidden="true" className="absolute top-[28%] -right-px h-[14%] w-[2px] rounded-r bg-border-strong" />
      <div className="absolute inset-[6px] overflow-hidden rounded-[1.7rem] border border-border bg-bg">
        <div aria-hidden="true" className="relative flex h-8 items-center justify-between px-3.5 text-xs font-semibold text-fg tabular-nums">
          <span>9:41</span>
          <span className="absolute top-1/2 left-1/2 size-[7px] -translate-x-1/2 -translate-y-1/2 rounded-full bg-fg" />
          <StatusMarks android />
        </div>
        {children}
      </div>
    </div>
  );
}

type TourScreen = {
  value: string;
  title: string;
  body: string;
  shot: string;
  alt: string;
  device: "app" | "phones";
};

const ROTATE_MS = 6000;

function Live({ index, total }: { index: number; total: number }) {
  const { t } = useTranslation();
  return (
    <span aria-live="polite" className="sr-only">
      {t("Screen {{n}} of {{total}}", { n: index + 1, total })}
    </span>
  );
}

/**
 * Automatic rotation pauses for hover, focus, hidden tabs, offscreen content and reduced motion.
 * The rail uses 44px targets around its smaller visual marks.
 */
export function ProductTour({ screens }: { screens: readonly TourScreen[] }) {
  const { t } = useTranslation();
  const [value, setValue] = useState(screens[0]!.value);
  const touchStart = useRef<{ x: number; y: number } | null>(null);
  const [held, setHeld] = useState(false);
  const reduced = usePrefersReducedMotion();
  const [away, setAway] = useState(false);
  const [inViewRef, inView] = useIntersectionObserver({ threshold: 0.35 });
  const index = Math.max(
    0,
    screens.findIndex((screen) => screen.value === value),
  );
  const screen = screens[index]!;
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
  const fill = reduced ? "still" : rotating ? "running" : "paused";
  // Fixed-height frames keep automatic rotation from moving the page under the reader.
  const stage = (
    <div ref={inViewRef} className="relative isolate min-w-0">
      {/* Size both device types from the fixed frame so phone shells cannot stretch it. */}
      <div className="relative grid aspect-[16/10] place-items-center lg:aspect-auto lg:h-[var(--tour-picture,44rem)]">
        {screens.map((entry, at) => (
          <Tabs.Panel
            key={entry.value}
            value={entry.value}
            keepMounted
            className="col-start-1 row-start-1 flex size-full min-h-0 min-w-0 items-center justify-center transition-opacity duration-300 ease-out data-[ending-style]:opacity-0 data-[starting-style]:opacity-0 motion-reduce:transition-none"
          >
            {entry.device === "phones" ? (
              <div className="flex h-full w-full items-center justify-center gap-3 max-[22rem]:gap-2 lg:gap-6">
                <IPhone className="h-full w-auto shrink-0">
                  <ProductShot shot={entry.shot} alt={entry.alt} eager priority={at === 0} width={640} height={1386} />
                </IPhone>
                <Android className="h-full w-auto shrink-0">
                  <ProductShot shot={entry.shot} alt={entry.alt} eager priority={at === 0} width={640} height={1386} />
                </Android>
              </div>
            ) : (
              <AppWindow className="w-full lg:h-full lg:max-w-[calc(var(--tour-picture,44rem)*1.6)]">
                <ProductShot shot={entry.shot} alt={entry.alt} eager priority={at === 0} width={2560} height={1600} />
              </AppWindow>
            )}
          </Tabs.Panel>
        ))}
      </div>
    </div>
  );

  const rail = (
    <Tabs.List
      aria-label={t("Choose a screen")}
      data-rail=""
      // Holding a mark with a finger is not a hold: the tap already chose a screen, and a tour
      // waiting on it would look broken. Under reduced motion there is nothing to hold.
      onPointerEnter={(event: PointerEvent<HTMLDivElement>) => {
        if (!reduced && event.pointerType !== "touch") setHeld(true);
      }}
      onPointerLeave={(event: PointerEvent<HTMLDivElement>) => {
        if (!reduced && event.pointerType !== "touch") setHeld(false);
      }}
      className="mx-auto mt-1 flex w-fit items-center justify-center"
    >
      {screens.map((entry, at) => (
        <Tabs.Tab
          key={entry.value}
          value={entry.value}
          aria-label={t("Screen {{n}} of {{total}}: {{title}}", { n: at + 1, total: screens.length, title: entry.title })}
          className="group/tick flex h-11 w-11 cursor-pointer items-center justify-center px-1 outline-offset-0"
        >
          <span
            aria-hidden="true"
            data-shape=""
            className="relative block h-1 w-6 rounded-full bg-border transition-colors duration-200 group-hover/tick:bg-border-strong"
          >
            <span
              key={entry.value}
              aria-hidden="true"
              data-shape=""
              data-fill={fill}
              style={reduced ? undefined : { animationName: "tour-fill", animationPlayState: rotating ? "running" : "paused" }}
              className={cn(
                "absolute inset-y-0 left-0 rounded-full bg-fg",
                "hidden group-data-[active]/tick:block",
                "w-full motion-safe:w-0 motion-safe:[animation-duration:var(--tour-rotate)] motion-safe:[animation-timing-function:linear] motion-safe:[animation-fill-mode:both]",
              )}
            />
          </span>
        </Tabs.Tab>
      ))}
    </Tabs.List>
  );

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
      className="grid touch-pan-y gap-y-5 lg:grid-cols-[minmax(0,17rem)_minmax(0,1fr)] lg:items-center lg:gap-x-10 lg:gap-y-0"
      // The one place `ROTATE_MS` reaches the stylesheet: the rail's fill runs on the tour's
      // own interval, so the bar and the timer that moves the tour on cannot disagree.
      style={{ "--tour-rotate": `${ROTATE_MS}ms` } as CSSProperties}
    >
      <div className="min-w-0">{caption}</div>
      <div className="min-w-0">
        {stage}
        {rail}
      </div>
      <Live index={index} total={screens.length} />
    </Tabs.Root>
  );
}
