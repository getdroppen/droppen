import { type KeyboardEvent, type ReactNode, useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Lock } from "lucide-react";
import { cn } from "@/lib/cn";
import { useReducedMotion } from "./motion";

/*
 * The product, shown: real screenshots of Droppen behind a row of tabs —
 * sessions, split screen, terminal, the cloud's every-device page, the phone,
 * letting a device in. Each picture sits in the frame it is seen in (the Mac
 * app's window, a browser at my.droppen.app, a phone), drawn here in HTML so
 * the frames follow the theme; the pictures themselves are per language and
 * theme (`tourShot`).
 *
 * The screenshots are made from the real app on a demo home with made-up
 * sessions (`scripts/landing-shots/`), never from anyone's account.
 *
 * The tabs advance by themselves while the tour is on screen, until the
 * reader picks one; under reduced motion they never do.
 */

export const TOUR_TABS = ["sessions", "split", "terminal", "cloud", "phone", "security"] as const;
export type TourTab = (typeof TOUR_TABS)[number];

/** How long a tab is shown before the next, while nobody has picked one. */
export const TOUR_ADVANCE_MS = 7000;

type FrameKind = "app" | "browser" | "phone";

/** Which screenshots each tab shows, and in what frame. */
export const TOUR_SHOTS: Record<TourTab, { frame: FrameKind; shots: string[] }> = {
  sessions: { frame: "app", shots: ["app-session"] },
  split: { frame: "app", shots: ["app-split"] },
  terminal: { frame: "app", shots: ["app-terminal"] },
  cloud: { frame: "browser", shots: ["cloud-devices"] },
  phone: { frame: "phone", shots: ["phone-needs-you", "phone-session"] },
  security: { frame: "phone", shots: ["security-pair"] },
};

/** The screenshots' languages: Chinese pages get the Chinese ones, every other the English. */
export function tourLanguage(language: string): "zh-CN" | "en" {
  return language.startsWith("zh") ? "zh-CN" : "en";
}

/** Where a screenshot is, for a language and a theme. */
export function tourShot(shot: string, language: string, theme: "light" | "dark"): string {
  return `/tour/${shot}-${tourLanguage(language)}-${theme}.webp`;
}

/** One screenshot in both themes: the page's theme (the `dark` class) shows one of them. */
function Shot({ shot, alt, className, eager }: { shot: string; alt: string; className?: string; eager?: boolean }) {
  const { i18n } = useTranslation();
  const loading = eager ? "eager" : "lazy";
  return (
    <>
      <img src={tourShot(shot, i18n.language, "light")} alt={alt} loading={loading} decoding="async" className={cn("block w-full dark:hidden", className)} />
      <img src={tourShot(shot, i18n.language, "dark")} alt="" aria-hidden="true" loading={loading} decoding="async" className={cn("hidden w-full dark:block", className)} />
    </>
  );
}

/** The Mac app's window: a title bar with its three buttons, and the app. */
function AppWindow({ children }: { children: ReactNode }) {
  return (
    <div className="overflow-hidden rounded-[10px] border border-border-strong bg-bg shadow-[0_24px_60px_-28px_rgb(0_0_0/0.35)]">
      <div className="relative flex h-8 items-center gap-2 border-b border-border bg-raised px-3">
        {[0, 1, 2].map((i) => (
          <span key={i} className="size-3 rounded-full border border-border-strong bg-bg" />
        ))}
        <span className="absolute inset-x-0 text-center text-xs font-medium text-muted">Droppen</span>
      </div>
      {children}
    </div>
  );
}

/** A browser at my.droppen.app: the address bar, and the page. */
function BrowserWindow({ children }: { children: ReactNode }) {
  return (
    <div className="overflow-hidden rounded-[10px] border border-border-strong bg-bg shadow-[0_24px_60px_-28px_rgb(0_0_0/0.35)]">
      <div className="flex h-9 items-center gap-3 border-b border-border bg-raised px-3">
        <span className="flex gap-2">
          {[0, 1, 2].map((i) => (
            <span key={i} className="size-3 rounded-full border border-border-strong bg-bg" />
          ))}
        </span>
        <span className="mx-auto flex h-6 w-full max-w-[22rem] items-center justify-center gap-1.5 rounded-md border border-border bg-bg font-mono text-[11px] text-muted">
          <Lock className="size-3" />
          my.droppen.app
        </span>
        <span className="w-[52px] max-sm:hidden" />
      </div>
      {children}
    </div>
  );
}

/** A phone: the bezel, the status bar with its island, and the screen under it. */
function Phone({ children }: { children: ReactNode }) {
  return (
    <div className="w-full max-w-[340px] rounded-[44px] border border-border-strong bg-raised p-[9px] shadow-[0_24px_60px_-28px_rgb(0_0_0/0.35)]">
      <div className="relative overflow-hidden rounded-[36px] border border-border bg-bg">
        <div aria-hidden="true" className="relative flex h-9 items-center justify-between px-6 text-[11px] font-semibold text-fg tabular-nums">
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

function Framed({ tab, alt, eager }: { tab: TourTab; alt: string; eager?: boolean }) {
  const { frame, shots } = TOUR_SHOTS[tab];
  if (frame === "phone") {
    return (
      <div className="flex items-start justify-center gap-10 max-sm:gap-4">
        {shots.map((shot, i) => (
          <div key={shot} className={cn("flex flex-1 justify-center", i > 0 && "max-sm:hidden")}>
            <Phone>
              <Shot shot={shot} alt={i === 0 ? alt : ""} eager={eager} />
            </Phone>
          </div>
        ))}
      </div>
    );
  }
  const Window = frame === "app" ? AppWindow : BrowserWindow;
  return (
    <Window>
      <Shot shot={shots[0]!} alt={alt} eager={eager} />
    </Window>
  );
}

export function ProductTour() {
  const { t } = useTranslation();
  const id = useId();
  const reduced = useReducedMotion();
  const [active, setActive] = useState<TourTab>("sessions");
  // Picked by the reader: the tour stops advancing by itself for good.
  const [picked, setPicked] = useState(false);
  const [visible, setVisible] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const tabs = useRef<Partial<Record<TourTab, HTMLButtonElement | null>>>({});

  const copy: Record<TourTab, { label: string; title: string; body: string; alt: string }> = {
    sessions: {
      label: t("Sessions"),
      title: t("Every session, as it runs"),
      body: t("The transcript, every tool it ran, and the question it is waiting on. Answer it, and it carries on."),
      alt: t("Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer"),
    },
    split: {
      label: t("Split screen"),
      title: t("Sessions and terminals, side by side"),
      body: t("Lay out several panes at once: a session, a terminal, another session, each on whichever computer you choose."),
      alt: t("Droppen with three panes: two sessions and a terminal"),
    },
    terminal: {
      label: t("Terminal"),
      title: t("A real terminal on any of your computers"),
      body: t("Open a shell on any computer of yours, from the browser on your phone or anywhere else."),
      alt: t("A terminal in Droppen"),
    },
    cloud: {
      label: t("Cloud"),
      title: t("Every computer on one account"),
      body: t("my.droppen.app shows every computer on your account, and one list of what needs you across all of them."),
      alt: t("my.droppen.app: three computers and the list of what needs you"),
    },
    phone: {
      label: t("Phone"),
      title: t("Made for the phone in your hand"),
      body: t("No app to install: open droppen.app, add it to your home screen, and be told when an agent needs you."),
      alt: t("Droppen on a phone: what needs you, and a session's question"),
    },
    security: {
      label: t("Security"),
      title: t("Only the devices you let in"),
      body: t("A new phone or browser gets in only when a device you already have lets it in, with a QR code. End-to-end encrypted."),
      alt: t("Letting a new device in with a QR code"),
    },
  };

  // Advance only while some of the tour is on screen.
  useEffect(() => {
    const el = root.current;
    if (!el || typeof IntersectionObserver !== "function") return;
    const io = new IntersectionObserver((entries) => setVisible(entries.some((e) => e.isIntersecting)), { threshold: 0.3 });
    io.observe(el);
    return () => io.disconnect();
  }, []);
  useEffect(() => {
    if (reduced || picked || !visible) return;
    const timer = setTimeout(() => setActive((tab) => TOUR_TABS[(TOUR_TABS.indexOf(tab) + 1) % TOUR_TABS.length]!), TOUR_ADVANCE_MS);
    return () => clearTimeout(timer);
  }, [active, reduced, picked, visible]);
  // On a phone the tabs scroll sideways: keep the active one in view, moving only the tab list, never the page.
  useEffect(() => {
    const tab = tabs.current[active];
    const list = tab?.parentElement;
    if (!tab || !list || list.scrollWidth <= list.clientWidth) return;
    list.scrollTo({ left: tab.offsetLeft - (list.clientWidth - tab.offsetWidth) / 2, behavior: reduced ? "auto" : "smooth" });
  }, [active, reduced]);

  const pick = (tab: TourTab, focus = false) => {
    setPicked(true);
    setActive(tab);
    if (focus) tabs.current[tab]?.focus();
  };
  // The tab list's keys: left and right move between tabs, Home and End to the ends.
  const onKey = (e: KeyboardEvent) => {
    const at = TOUR_TABS.indexOf(active);
    const to = e.key === "ArrowRight" ? at + 1 : e.key === "ArrowLeft" ? at - 1 : e.key === "Home" ? 0 : e.key === "End" ? TOUR_TABS.length - 1 : null;
    if (to === null) return;
    e.preventDefault();
    pick(TOUR_TABS[(to + TOUR_TABS.length) % TOUR_TABS.length]!, true);
  };

  const now = copy[active];
  return (
    <div ref={root} data-testid="product-tour">
      <div role="tablist" aria-label={t("What Droppen looks like")} onKeyDown={onKey} className="relative grid grid-cols-6 border-b border-border max-md:flex max-md:overflow-x-auto max-md:[scrollbar-width:none]">
        {TOUR_TABS.map((tab, i) => {
          const on = tab === active;
          return (
            <button
              key={tab}
              ref={(el) => void (tabs.current[tab] = el)}
              type="button"
              role="tab"
              id={`${id}-tab-${tab}`}
              aria-selected={on}
              aria-controls={`${id}-panel`}
              tabIndex={on ? 0 : -1}
              data-tab={tab}
              onClick={() => pick(tab)}
              className={cn(
                "relative flex h-12 items-center justify-center px-4 font-mono text-xs whitespace-nowrap transition-colors max-md:shrink-0",
                i > 0 && "border-l border-border",
                on ? "bg-raised text-fg" : "text-muted hover:text-fg",
              )}
            >
              {copy[tab].label}
              {/* The active tab's rule; while the tour advances by itself, it fills up to the next tab. */}
              {on && (
                <span
                  key={`${tab}-${picked || reduced}`}
                  className={cn("absolute inset-x-0 -bottom-px h-0.5 origin-left bg-fg", !picked && !reduced && visible && "animate-[tour-progress_linear_forwards]")}
                  style={!picked && !reduced && visible ? { animationDuration: `${TOUR_ADVANCE_MS}ms` } : undefined}
                />
              )}
            </button>
          );
        })}
      </div>
      <div role="tabpanel" id={`${id}-panel`} aria-labelledby={`${id}-tab-${active}`} className="flex flex-col">
        <div className="flex flex-col gap-1 px-6 pt-6 max-md:px-4">
          <span className="text-[17px] leading-6 font-semibold text-fg">{now.title}</span>
          <span className="max-w-[46em] text-sm leading-6 text-muted">{now.body}</span>
        </div>
        <div
          className="px-10 pt-6 pb-12 max-md:px-4 max-md:pt-5 max-md:pb-8"
          style={{
            backgroundImage: "linear-gradient(var(--color-border) 1px, transparent 1px), linear-gradient(90deg, var(--color-border) 1px, transparent 1px)",
            backgroundSize: "22px 22px",
            backgroundPosition: "-1px -1px",
          }}
        >
          <Framed key={active} tab={active} alt={now.alt} eager={active === "sessions"} />
        </div>
      </div>
    </div>
  );
}
