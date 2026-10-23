/*
 * What the guide's frame holds: the welcome
 * page, and steps 02 and 04. Step 01 (AI tools) is `tools-step.tsx`, step 03
 * (first session) `first-step.tsx`: each is one component taking
 * {@link StepProps}, put in place in `welcome-view.tsx`.
 */
import { ArrowRight, Check } from "lucide-react";
import { type KeyboardEvent, useRef } from "react";
import { useTranslation } from "react-i18next";
import { LiveDot } from "@/components/live-dot";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/cn";
import { DESKTOP_SIGN_IN_READY } from "./flags";
import { FootNote, onceClicked } from "./frame";
import { foundTools, STEP_LABEL, STEPS, TOOL_STATUS_LABEL, toolStatus } from "./model";
import type { SetupProgress, ToolsReport } from "./types";
import { type GuideSignIn, WhereSignInPanel } from "./where-sign-in";

/** What every step is given. */
export interface StepProps {
  progress: SetupProgress;
  /** `undefined` while detection runs, `null` when it is not there (an older server). */
  tools: ToolsReport | null | undefined;
  online: boolean;
  /** Store a change to the progress (the step's own choices). */
  onChange: (next: SetupProgress) => void;
  /** This Mac's sign-in to Droppen (steps 02 and 04); absent, the steps show none. */
  signIn?: GuideSignIn;
}

export const STEP_SUMMARY: Record<(typeof STEPS)[number], string> = {
  tools: "Find Claude Code and Codex; install and sign in where needed.",
  where: "Only on this Mac, or on your phone and other devices too.",
  first: "Pick a folder and send a first message.",
  wrap: "Start at login, notifications, your phone.",
};

/** A list in the reader's language: 「、」 in Chinese and Japanese, ", " otherwise. */
function listOf(items: string[], language: string): string {
  return items.join(/^(zh|ja)/.test(language) ? "、" : ", ");
}

/** The welcome page's foot line: still looking, what was found, or that nothing is ready. */
function ProbeLine({ tools }: { tools: ToolsReport | null | undefined }) {
  const { t, i18n } = useTranslation();
  if (tools === null) return null;
  const found = tools ? foundTools(tools) : [];
  return (
    <div data-testid="welcome-probe" role="status" className="mt-auto flex items-center gap-2 border-t border-border px-5 py-3.5 font-mono text-2xs leading-4 text-muted">
      {tools === undefined ? (
        <>
          <LiveDot className="text-muted" />
          {t("Already looking for Claude Code and Codex on this device…")}
        </>
      ) : found.length > 0 ? (
        <>
          <Check className="size-3 shrink-0 text-fg" aria-hidden="true" />
          {t("Found: {{list}}", { list: listOf(found, i18n.language) })}
        </>
      ) : (
        t("No AI tool is ready on this device yet.")
      )}
    </div>
  );
}

/**
 * The welcome page: what Droppen is doing here, the four steps in a
 * line each, detection already running. "Get started" is the page's one
 * primary and has the focus.
 */
export function WelcomeScreen({ tools, onStart, onSkipAll }: { tools: ToolsReport | null | undefined; onStart: () => void; onSkipAll: () => void }) {
  const { t } = useTranslation();
  return (
    <div data-testid="welcome" className="min-h-0 overflow-y-auto">
      <div className="grid min-h-full grid-cols-[minmax(0,1fr)_360px] max-[960px]:grid-cols-[minmax(0,1fr)]">
        <div className="pt-[72px] pr-10 pb-10 pl-12 max-[960px]:px-5 max-[960px]:pt-10 max-[960px]:pb-8">
          <span data-label="" className="label-caps text-2xs text-muted">
            {t("Droppen for Mac")}
          </span>
          <h1 className="mt-3.5 mb-4 max-w-[18em] text-[40px] leading-[1.02] font-semibold tracking-[-0.045em] max-[960px]:text-[30px] [:lang(ja)_&]:tracking-[-0.03em] [:lang(zh)_&]:tracking-[-0.03em]">
            {t("Droppen is running on this Mac.")}
          </h1>
          <p className="mb-7 max-w-[34em] text-[13px] leading-[21px] text-muted">
            {t("About three minutes to put it to work: get Claude Code or Codex ready, decide where you'll use it, then start your first session. Skip anything you like; all of it can be changed later in Settings.")}
          </p>
          <div className="flex flex-wrap items-center gap-2.5">
            <Button autoFocus variant="primary" data-testid="welcome-start" data-guide-primary="" onClick={onceClicked(onStart)}>
              {t("Get started")}
            </Button>
            <Button variant="ghost" data-testid="welcome-skip" onClick={onceClicked(onSkipAll)}>
              {t("Skip setup and open Droppen")}
            </Button>
          </div>
        </div>
        <aside className="flex flex-col border-l border-border max-[960px]:border-t max-[960px]:border-l-0">
          <h2 data-label="" className="label-caps flex h-9 shrink-0 items-center border-b border-border px-5 text-2xs text-muted">
            {t("Get started with Droppen")}
          </h2>
          <ol>
            {STEPS.map((step, i) => (
              <li key={step} className="grid grid-cols-[26px_minmax(0,1fr)] gap-x-2 gap-y-0.5 border-b border-border px-5 py-3.5">
                <span className="font-mono text-2xs leading-[18px] text-faint tabular-nums">{`0${i + 1}`}</span>
                <span className="text-[13px] leading-[18px] font-semibold">{t(STEP_LABEL[step])}</span>
                <span className="col-start-2 text-xs leading-[18px] text-muted">{t(STEP_SUMMARY[step])}</span>
              </li>
            ))}
          </ol>
          <ProbeLine tools={tools} />
        </aside>
      </div>
    </div>
  );
}

/** Where to use it: the two outcomes. */
export type WhereChoice = "droppen" | "local";

/** The choice step 02 shows: the stored one, else the recommended one when it can be had, else this Mac only. */
export function whereChoice(progress: SetupProgress, online: boolean, signInReady = DESKTOP_SIGN_IN_READY): WhereChoice {
  const droppen = signInReady && online;
  if (progress.where.choice === "local") return "local";
  if (progress.where.choice === "droppen") return droppen ? "droppen" : "local";
  return droppen ? "droppen" : "local";
}

/**
 * Step 02's choice made: the progress with it. "Only on this Mac" also ends a
 * sign-in under way or one that stopped — the page's one, which Settings →
 * Droppen would otherwise go on showing as such.
 */
export function chooseWhere(progress: SetupProgress, choice: WhereChoice, signIn?: GuideSignIn): SetupProgress {
  if (choice === "local" && signIn && (signIn.state.kind === "pending" || signIn.state.kind === "failed")) signIn.cancel();
  return { ...progress, where: { ...progress.where, choice } };
}

function WhereCard({
  choice,
  checked,
  disabled,
  title,
  action,
  body,
  note,
  recommended,
  onPick,
}: {
  choice: WhereChoice;
  checked: boolean;
  disabled: boolean;
  title: string;
  action: string;
  body: string;
  note: string | null;
  recommended: boolean;
  onPick: () => void;
}) {
  const { t } = useTranslation();
  return (
    <button
      type="button"
      role="radio"
      aria-checked={checked}
      disabled={disabled}
      tabIndex={checked ? 0 : -1}
      data-choice={choice}
      onClick={onPick}
      className={cn(
        "-mt-px grid grid-cols-[13px_minmax(0,1fr)_auto] items-start gap-x-3.5 gap-y-1 border p-4 text-left first:mt-0",
        checked ? "relative z-[1] border-fg" : "border-border hover:bg-hover",
        disabled && "cursor-not-allowed hover:bg-transparent",
      )}
    >
      <span
        data-shape="radio"
        aria-hidden="true"
        className={cn("mt-[3px] flex size-[13px] items-center justify-center rounded-full border", checked ? "border-fg" : "border-border-strong", disabled && "opacity-45")}
      >
        {checked && <span data-shape="radio" className="size-[7px] rounded-full bg-fg" />}
      </span>
      <span className={cn("flex min-w-0 flex-wrap items-center gap-2 text-[14px] leading-5 font-semibold", disabled && "opacity-45")}>
        {title}
        {recommended && <Badge className="tracking-[0.06em] text-fg uppercase shadow-[inset_0_0_0_1px_var(--color-fg)]">{t("Recommended")}</Badge>}
      </span>
      <span className={cn("inline-flex items-center gap-1 font-mono text-2xs leading-5 font-medium whitespace-nowrap text-fg", disabled && "opacity-45")}>
        {action}
        <ArrowRight className="size-3" aria-hidden="true" />
      </span>
      <span className="col-start-2 col-end-4 max-w-[52em] text-xs leading-[19px] text-muted">
        <span className={cn(disabled && "opacity-45")}>{body}</span>
        {note && (
          <span data-testid={`${choice}-note`} className="mt-1 block text-fg">
            {note}
          </span>
        )}
      </span>
    </button>
  );
}

/**
 * Step 02 · Where to use it, minimal: on the phone and other devices
 * too (signing in to Droppen; recommended), or only on this Mac. The first
 * is off without the app's sign-in through the system browser
 * ({@link DESKTOP_SIGN_IN_READY}) and while offline; chosen, the step signs
 * in from its primary and shows how that stands under the choices
 * (`where-sign-in.tsx`). The local-network option ("Also") waits for the
 * local-access change and the app ↔ server channel.
 */
export function WhereStep({ progress, online, onChange, signIn, signInReady = DESKTOP_SIGN_IN_READY }: StepProps & { signInReady?: boolean }) {
  const { t } = useTranslation();
  const chosen = whereChoice(progress, online, signInReady);
  const droppenOff = !signInReady || !online;
  const group = useRef<HTMLDivElement>(null);
  const pick = (choice: WhereChoice) => onChange(chooseWhere(progress, choice, signIn));
  const enabled: WhereChoice[] = droppenOff ? ["local"] : ["droppen", "local"];
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const step = event.key === "ArrowDown" || event.key === "ArrowRight" ? 1 : event.key === "ArrowUp" || event.key === "ArrowLeft" ? -1 : 0;
    if (!step) return;
    event.preventDefault();
    const next = enabled[(enabled.indexOf(chosen) + step + enabled.length) % enabled.length]!;
    pick(next);
    group.current?.querySelector<HTMLButtonElement>(`[data-choice="${next}"]`)?.focus();
  };
  return (
    <>
      <div ref={group} role="radiogroup" aria-label={t("Where will you use Droppen?")} onKeyDown={onKeyDown} className="flex max-w-[720px] flex-col">
        <WhereCard
          choice="droppen"
          checked={chosen === "droppen"}
          disabled={droppenOff}
          title={t("On my phone and other devices too")}
          action={t("Sign in to Droppen")}
          body={t("Open droppen.app on your phone or any other device and carry on with this Mac's sessions. Content is end-to-end encrypted; droppen.app can't read it. Your browser opens to sign in or create an account.")}
          note={
            !signInReady
              ? t("Signing in from the app is coming soon")
              : !online
                ? t("droppen.app needs the internet. Choose “Only on this Mac” for now; sign in later from Settings → Access → Droppen.")
                : null
          }
          recommended
          onPick={() => pick("droppen")}
        />
        <WhereCard
          choice="local"
          checked={chosen === "local"}
          disabled={false}
          title={t("Only on this Mac")}
          action={t("Skip")}
          body={t("Everything works without an account. Nothing leaves this Mac except what Claude Code and Codex send to their own services.")}
          note={null}
          recommended={false}
          onPick={() => pick("local")}
        />
      </div>
      {signIn && <WhereSignInPanel signIn={signIn} chosen={chosen} />}
      <FootNote>
        {t("The Droppen app on this Mac never asks for a password. For a browser on this Mac, choose Open in Browser in the app's menu-bar menu: it opens signed in. Once this device has a password, a browser asks for it.")}{" "}
        {t("Change it later in Settings → Access.")}
      </FootNote>
    </>
  );
}

/** One row of step 04: what it is, why, and its control. */
function WrapRow({ title, body, children, testId }: { title: string; body: React.ReactNode; children?: React.ReactNode; testId: string }) {
  return (
    <div data-testid={testId} className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-6 border-b border-border px-4 py-3.5 last:border-b-0">
      <div className="min-w-0">
        <div className="text-[13px] leading-[18px] font-semibold">{title}</div>
        <div className="max-w-[46em] text-xs leading-[18px] text-muted">{body}</div>
      </div>
      {children && <div className="flex items-center gap-2.5">{children}</div>}
    </div>
  );
}

/** A tool's line in "What's set". */
function toolSummary(tools: ToolsReport | null | undefined, id: "claude" | "codex", t: (key: string) => string) {
  const tool = tools?.tools.find((it) => it.id === id);
  if (!tool) return <span className="font-mono text-2xs text-faint">—</span>;
  const status = toolStatus(tool);
  if (status === "ready") {
    return (
      <span className="inline-flex items-center gap-1.5 font-mono text-xs">
        <Check className="size-3" aria-hidden="true" />
        {tool.login.plan ?? t("ready")}
      </span>
    );
  }
  return <span className="font-mono text-2xs text-faint">{t(TOOL_STATUS_LABEL[status])}</span>;
}

/**
 * Step 04 · Finishing up, as the fallback while the web app cannot reach the
 * app: start at login points at the menu bar's own
 * switch to turn it on or off — whether it is on is the app's to say (a new
 * install's first launch turns it on), so the guide claims neither; no
 * notifications row until the app can ask for them, the phone row,
 * and what is set.
 */
export function WrapStep({ progress, tools, signIn, signInReady = DESKTOP_SIGN_IN_READY }: StepProps & { signInReady?: boolean }) {
  const { t } = useTranslation();
  const skipped = <span className="font-mono text-2xs text-faint">{t("skipped")}</span>;
  // What this Mac's sign-in came to, when the guide follows it: in the account, under way, or not (yet).
  const linked = signIn?.state.kind === "linked" ? signIn.state : null;
  const droppen = linked
    ? linked.email
      ? t("Droppen · {{email}}", { email: linked.email })
      : "Droppen"
    : !signIn
      ? "Droppen"
      : signIn.state.kind === "pending"
        ? `Droppen · ${t("Waiting for droppen.app…")}`
        : `Droppen · ${t("not signed in")}`;
  const where = linked
    ? droppen
    : progress.steps.where === "skipped"
      ? skipped
      : progress.where.choice === "droppen"
        ? droppen
        : progress.where.choice === "local"
          ? t("Only on this Mac")
          : "—";
  const cwd = progress.steps.first === "done" ? progress.first.cwd : null;
  return (
    <>
      <div className="max-w-[760px] border border-border">
        <WrapRow
          testId="wrap-login"
          title={t("Start Droppen at login")}
          body={
            <>
              {t("So sessions, your phone and scheduled tasks can reach this Mac after a restart. Droppen lives in the menu bar; closing the window doesn't stop it.")}
              <span className="mt-1 block text-fg">{t("Turn it on or off any time from the Droppen icon in the menu bar: Start at Login.")}</span>
            </>
          }
        />
        <WrapRow
          testId="wrap-phone"
          title={t("On your phone")}
          body={
            linked?.email
              ? t("Open droppen.app and sign in as {{email}}.", { email: linked.email })
              : signInReady
                ? t("Want it on your phone too? Sign in to Droppen any time in Settings → Access → Droppen.")
                : t("Signing in from the app is coming soon")
          }
        />
      </div>
      <div className="mt-5 max-w-[760px]">
        <h2 data-label="" className="label-caps mb-2 text-2xs text-muted">
          {t("What's set")}
        </h2>
        <table data-testid="wrap-summary" className="w-full border-collapse text-xs">
          <tbody>
            {(
              [
                ["Claude Code", toolSummary(tools, "claude", t)],
                ["Codex", toolSummary(tools, "codex", t)],
                [t("Where to use it"), where],
                [t("First session"), cwd ? <span className="font-mono">{cwd}</span> : progress.steps.first === "skipped" ? skipped : "—"],
              ] as const
            ).map(([label, value]) => (
              <tr key={label} className="h-8 border-y border-border">
                <td className="w-40 pr-3 text-muted">{label}</td>
                <td className="px-3">{value}</td>
              </tr>
            ))}
          </tbody>
        </table>
        <p className="mt-2.5 font-mono text-2xs text-faint">{t("Run this again from Settings → General → Setup.")}</p>
      </div>
    </>
  );
}
