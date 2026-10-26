/*
 * The guide's frame: the whole window, no
 * rail of the app's. A 40px top bar (the mark · "Get started with Droppen" ·
 * offline · language · "Finish later"); under it a document frame up to
 * 1120px wide between two hairlines, holding the step rail (232px) and the
 * step, with a 52px action bar at the foot (Skip this step · Step N of 4 ·
 * Back · one primary). Registration marks only where the top bar's line meets
 * the frame's lines and the rail's divider. Under 960px the rail is a row of
 * four cells across the top, and no marks are drawn.
 */
import { Menu } from "@base-ui-components/react/menu";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, ChevronDown, Globe, Info } from "lucide-react";
import { forwardRef, useEffect, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "@/api/schema";
import { BrandMark } from "@/components/brand-mark";
import { RegMark } from "@/components/reg-mark";
import { Button } from "@/components/ui/button";
import { applyLanguage, browserLanguages, LANGUAGE_NAMES, pickLanguage, storedLanguagePref, type LanguagePref } from "@/i18n";
import { cn } from "@/lib/cn";
import { useApi, useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";
import { STEP_LABEL, STEPS, stepNumber } from "./model";
import type { SetupProgress, StepId } from "./types";

type Prefs = components["schemas"]["Prefs"];

/** The header: never 「设置 Droppen」, which reads as Settings. */
export const GUIDE_TITLE = "Get started with Droppen";

/** The frame's widths (px): the document column and the step rail. */
const FRAME_WIDTH = 1120;
const RAIL_WIDTH = 232;

/** Whether the browser says it is online, as it changes. */
export function useOnline(): boolean {
  const [online, setOnline] = useState(() => (typeof navigator === "undefined" ? true : navigator.onLine !== false));
  useEffect(() => {
    const update = () => setOnline(navigator.onLine !== false);
    window.addEventListener("online", update);
    window.addEventListener("offline", update);
    return () => {
      window.removeEventListener("online", update);
      window.removeEventListener("offline", update);
    };
  }, []);
  return online;
}

/** Auto (the browser's language) and the four languages, each in its own script; the choice is this device's language pref (`/api/prefs`). */
function LanguagePicker() {
  const { t, i18n } = useTranslation();
  const api = useApi();
  const machine = useMachine();
  const queryClient = useQueryClient();
  const { data: prefs } = useQuery({
    queryKey: mq(machine, "prefs"),
    queryFn: async () => (await api.GET("/api/prefs")).data,
    staleTime: 30_000,
  });
  const [chosen, setChosen] = useState<LanguagePref>(storedLanguagePref);
  const active = LANGUAGE_NAMES.find((l) => l.id === i18n.language) ?? LANGUAGE_NAMES[LANGUAGE_NAMES.length - 1]!;
  const browser = LANGUAGE_NAMES.find((l) => l.id === pickLanguage(browserLanguages()))!;
  const choose = (language: LanguagePref) => {
    void applyLanguage(language);
    setChosen(language);
    if (!prefs) return;
    const next = { ...prefs, language } as Prefs;
    queryClient.setQueryData(mq(machine, "prefs"), next);
    void api.PUT("/api/prefs", { body: next }).then((res) => {
      if (res.data) queryClient.setQueryData(mq(machine, "prefs"), res.data);
    });
  };
  return (
    <Menu.Root>
      <Menu.Trigger
        aria-label={t("Language")}
        title={t("Language")}
        className="flex h-6 shrink-0 items-center gap-1 px-2 font-mono text-2xs text-muted hover:bg-hover hover:text-fg"
      >
        <Globe className="size-3" />
        <span lang={active.id}>{active.label}</span>
        <ChevronDown className="size-2.5" />
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Positioner side="bottom" align="end" sideOffset={4} className="z-50 outline-none">
          <Menu.Popup className="min-w-36 border border-border-strong bg-bg py-1 outline-none">
            <Menu.RadioGroup value={prefs?.language ?? chosen} onValueChange={(value: LanguagePref) => choose(value)}>
              <Menu.RadioItem
                value="auto"
                closeOnClick
                className="flex w-full cursor-pointer items-center gap-2 px-3 py-1 text-left text-xs text-fg outline-none data-[highlighted]:bg-hover"
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
                  className="flex w-full cursor-pointer items-center gap-2 px-3 py-1 text-left text-xs text-fg outline-none data-[highlighted]:bg-hover"
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
  );
}

/** The 40px bar across the top. "Finish later" is not on the welcome page, which has its own way out. */
export function GuideTopBar({ onLater, online }: { onLater: (() => void) | null; online: boolean }) {
  const { t } = useTranslation();
  return (
    <header className="flex h-10 shrink-0 items-center gap-2.5 border-b border-border pr-3 pl-4">
      <span className="flex shrink-0 items-center gap-2 text-[13px] leading-none font-semibold">
        <BrandMark size={20} />
        Droppen
      </span>
      <span aria-hidden="true" className="h-3.5 w-px shrink-0 bg-border" />
      <span data-label="" data-testid="guide-title" className="label-caps min-w-0 truncate text-2xs text-muted">
        {t(GUIDE_TITLE)}
      </span>
      {!online && (
        <span
          data-signal="offline"
          data-testid="guide-offline"
          role="status"
          title={t("Offline. Installing, signing in and Droppen need the internet; everything else works.")}
          className="ml-1 flex shrink-0 items-center gap-1 font-mono text-2xs text-danger"
        >
          <span className="size-1.5 bg-danger" />
          {t("offline")}
        </span>
      )}
      <span className="ml-auto flex shrink-0 items-center gap-1.5">
        <LanguagePicker />
        {onLater && (
          <Button
            variant="ghost"
            size="sm"
            data-testid="guide-later"
            title={t("What you've done is kept. Continue from the bar above the session list, or from Settings → General → Setup.")}
            onClick={onLater}
          >
            {t("Finish later")}
          </Button>
        )}
      </span>
    </header>
  );
}

/** The state word beside a step in the rail. */
function railWord(state: SetupProgress["steps"][StepId]): string | null {
  return state === "done" ? "done" : state === "skipped" ? "skipped" : null;
}

/**
 * The step rail: an ordered list, the current step ink-filled and
 * `aria-current="step"`, a done one ticked, a skipped one says so. Any step
 * can be opened from it.
 */
export function StepRail({ current, progress, onGoto }: { current: StepId; progress: SetupProgress; onGoto: (step: StepId) => void }) {
  const { t } = useTranslation();
  return (
    <nav
      aria-label={t(GUIDE_TITLE)}
      data-testid="step-rail"
      className="flex min-h-0 flex-col border-r border-border max-[960px]:border-r-0 max-[960px]:border-b"
    >
      <h2 data-label="" className="label-caps flex h-9 shrink-0 items-center border-b border-border px-4 text-2xs text-muted max-[960px]:hidden">
        {t(GUIDE_TITLE)}
      </h2>
      <ol className="max-[960px]:grid max-[960px]:grid-cols-4">
        {STEPS.map((step) => {
          const on = step === current;
          const state = progress.steps[step];
          const word = on ? null : railWord(state);
          return (
            <li key={step} data-step={step} data-state={on ? "current" : state}>
              <button
                type="button"
                aria-current={on ? "step" : undefined}
                onClick={() => onGoto(step)}
                className={cn(
                  "grid w-full grid-cols-[22px_minmax(0,1fr)_auto] items-center gap-x-2 border-b border-border py-2.5 pr-3.5 pl-4 text-left",
                  "max-[960px]:grid-cols-[auto_minmax(0,1fr)_auto] max-[960px]:border-r max-[960px]:border-b-0 max-[960px]:px-2.5 max-[960px]:py-2",
                  on ? "bg-fg text-bg" : "hover:bg-hover",
                )}
              >
                <span className={cn("font-mono text-2xs tabular-nums", on ? "text-bg/70" : "text-faint")}>{String(stepNumber(step)).padStart(2, "0")}</span>
                <span className="truncate text-[13px] leading-[18px]">{t(STEP_LABEL[step])}</span>
                <span className={cn("flex items-center gap-1 font-mono text-[10px] leading-[14px]", state === "done" ? "text-fg" : "text-faint")}>
                  {!on && state === "done" && <Check className="size-3" aria-hidden="true" />}
                  {word && <span className="max-[960px]:sr-only">{t(word)}</span>}
                </span>
              </button>
            </li>
          );
        })}
      </ol>
      <p className="mt-auto border-t border-border px-4 pt-3.5 pb-4 text-2xs leading-[17px] text-faint max-[960px]:hidden">
        {t("Every step can be skipped and changed later in Settings.")}
      </p>
    </nav>
  );
}

/** What the action bar's right end does on a step: one button, primary or not. */
export interface StepAction {
  label: string;
  onClick: () => void;
  /** The page's one primary (V10); a step whose own content holds the primary passes `false`. */
  primary: boolean;
  disabled?: boolean;
}

/**
 * Whether a click is the second (or a later) one of a double click. The first
 * already acted, and what is under the pointer since may be the next step's
 * primary — "Sign in in the browser", then "Keep going…": it is not pressed.
 * A click from the keyboard has `detail` 0.
 */
export function repeatedClick(event: { detail: number }): boolean {
  return event.detail > 1;
}

/** `act` for a guide button's click, but not for the rest of a double click ({@link repeatedClick}). */
export function onceClicked(act: () => void): (event: { detail: number }) => void {
  return (event) => {
    if (!repeatedClick(event)) act();
  };
}

/**
 * Whether a key press is the page's primary: Enter alone, not while composing,
 * and not the repeats of a held key, which would go on pressing each next
 * step's primary as it appears.
 */
export function pressesPrimary(event: {
  key: string;
  repeat: boolean;
  defaultPrevented: boolean;
  isComposing: boolean;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}): boolean {
  return (
    event.key === "Enter" && !event.repeat && !event.defaultPrevented && !event.isComposing && !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey
  );
}

/** The 52px bar at the foot of a step. */
export function ActionBar({ step, onSkip, onBack, action }: { step: StepId; onSkip: () => void; onBack: (() => void) | null; action: StepAction | null }) {
  const { t } = useTranslation();
  return (
    <footer data-testid="action-bar" className="flex h-[52px] shrink-0 items-center gap-2 border-t border-border pr-4 pl-6 max-[960px]:px-3">
      <Button variant="ghost" data-testid="skip-step" onClick={onceClicked(onSkip)}>
        {t("Skip this step")}
      </Button>
      <span className="font-mono text-2xs text-faint tabular-nums max-[960px]:hidden">{t("Step {{n}} of 4", { n: stepNumber(step) })}</span>
      <span className="ml-auto flex items-center gap-2">
        {onBack && (
          <Button variant="ghost" data-testid="step-back" onClick={onceClicked(onBack)}>
            {t("Back")}
          </Button>
        )}
        {action && (
          <Button
            variant={action.primary ? "primary" : "secondary"}
            data-testid="step-action"
            data-guide-primary={action.primary ? "" : undefined}
            disabled={action.disabled}
            onClick={onceClicked(action.onClick)}
          >
            {action.label}
          </Button>
        )}
      </span>
    </footer>
  );
}

/** A step's kicker, title and lead: the title takes the focus when the step opens. */
export const StepHeader = forwardRef<HTMLHeadingElement, { kicker: string; title: string; body?: string }>(function StepHeader({ kicker, title, body }, ref) {
  return (
    <>
      <span data-label="" className="label-caps mb-2 block text-2xs text-muted">
        {kicker}
      </span>
      <h1 ref={ref} tabIndex={-1} data-step-heading="" className="mb-1.5 text-[20px] leading-[26px] font-semibold tracking-[-0.02em] outline-none">
        {title}
      </h1>
      {body ? <p className="mb-5 max-w-[46em] text-[13px] leading-[21px] text-muted">{body}</p> : <div className="mb-5" />}
    </>
  );
});

/** A note under a step's choices: an info mark and a line of muted text. */
export function FootNote({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div className={cn("mt-[18px] grid max-w-[720px] grid-cols-[14px_minmax(0,1fr)] gap-2 text-xs leading-[18px] text-muted", className)}>
      <Info className="mt-0.5 size-3.5" aria-hidden="true" />
      <span>{children}</span>
    </div>
  );
}

/**
 * The window under the top bar: the framed column and its marks. `rail` is
 * drawn left of `children` (a step); without it the column is one piece (the
 * welcome page).
 */
export function GuideSheet({ rail, children }: { rail: ReactNode | null; children: ReactNode }) {
  const half = FRAME_WIDTH / 2;
  return (
    <div className="relative min-h-0 flex-1">
      <div
        data-sheet=""
        data-testid="guide-frame"
        className={cn(
          "mx-auto grid h-full w-full border-x border-border max-[960px]:border-x-0",
          "grid-rows-[minmax(0,1fr)]",
          rail ? "grid-cols-[232px_minmax(0,1fr)] max-[960px]:grid-cols-[minmax(0,1fr)] max-[960px]:grid-rows-[auto_minmax(0,1fr)]" : "grid-cols-[minmax(0,1fr)]",
        )}
        style={{ maxWidth: FRAME_WIDTH }}
      >
        {rail}
        {children}
      </div>
      {/* The top bar's line × the frame's sides: only while the column is narrower than the window, so there is a seam. */}
      <RegMark className="guide-mark-side -top-[5px]" style={{ left: `calc(50% - ${half + 4}px)` }} />
      <RegMark className="guide-mark-side -top-[5px]" style={{ left: `calc(50% + ${half - 5}px)` }} />
      {/* × the rail's divider, wherever the frame starts. */}
      {rail && (
        <RegMark
          className="guide-mark-rail -top-[5px]"
          style={{ left: `calc(max(0px, 50% - ${half}px) + ${RAIL_WIDTH - 4}px)` }}
        />
      )}
    </div>
  );
}

/** A step's page: its content scrolls, the action bar stays. */
export function StepPage({ children, bar }: { children: ReactNode; bar: ReactNode }) {
  return (
    <div className="grid min-h-0 grid-rows-[minmax(0,1fr)_auto]">
      <div className="min-h-0 overflow-y-auto">
        <div data-testid="step-content" className="max-w-[840px] px-8 pt-7 pb-10 max-[960px]:px-4 max-[960px]:pt-5 max-[960px]:pb-8">
          {children}
        </div>
      </div>
      {bar}
    </div>
  );
}
