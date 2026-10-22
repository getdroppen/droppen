/*
 * The first-run guide's rules, as plain functions of its progress: the four
 * steps and their order,
 * what Continue, Skip and Back do to them, how many are left, where to pick
 * up again, what a rerun clears, when the guide opens by itself and which bar
 * the session list shows. No React and no requests: `welcome-view.tsx` and
 * `bars.tsx` draw these, the tests run them.
 */
import type { SetupProgress, SetupTool, StepId, StepState, ToolsReport } from "./types";

/** The steps, in order. */
export const STEPS: readonly StepId[] = ["tools", "where", "first", "wrap"];

/** What the guide shows: the welcome page, or one of the steps. */
export type Screen = "welcome" | StepId;

/** The flow version this web app draws (the server's `flow`). */
export const FLOW = 1;

/** The route of the guide. */
export const WELCOME_PATH = "/welcome";

export interface WelcomeSearch {
  /** `1`: "Run setup again" — every step back to pending, from step 01. */
  rerun?: 1;
}

export function validateWelcomeSearch(search: Record<string, unknown>): WelcomeSearch {
  return search.rerun === 1 || search.rerun === "1" ? { rerun: 1 } : {};
}

/** The step names in the rail and on the welcome page (`shell.step.*`). */
export const STEP_LABEL: Record<StepId, string> = {
  tools: "AI tools",
  where: "Where to use it",
  first: "First session",
  wrap: "Finishing up",
};

/** Nothing stored yet: what `GET /api/setup/progress` answers on a new install. */
export function emptyProgress(): SetupProgress {
  return {
    flow: FLOW,
    startedAt: null,
    finishedAt: null,
    dismissed: false,
    steps: { tools: "pending", where: "pending", first: "pending", wrap: "pending" },
    stepFlow: { tools: 0, where: 0, first: 0, wrap: 0 },
    where: { choice: null, lan: false },
    first: { cwd: null, agent: null, prompt: null },
    wrap: { startAtLogin: null },
    autoOpen: false,
  };
}

/** 1–4. */
export function stepNumber(step: StepId): number {
  return STEPS.indexOf(step) + 1;
}

/** The step after `step`, `null` after the last. */
export function nextStep(step: StepId): StepId | null {
  return STEPS[STEPS.indexOf(step) + 1] ?? null;
}

/** The step before `step`, `null` before the first (Back is not offered there: the welcome page is not a step). */
export function prevStep(step: StepId): StepId | null {
  return STEPS[STEPS.indexOf(step) - 1] ?? null;
}

function withStep(progress: SetupProgress, step: StepId, state: StepState): SetupProgress {
  return { ...progress, steps: { ...progress.steps, [step]: state } };
}

/** "Get started", "Skip setup", or any first save: the guide has started (it never opens by itself again). */
export function start(progress: SetupProgress, now: string): SetupProgress {
  return progress.startedAt ? progress : { ...progress, startedAt: now };
}

/** Continue on `step`: it is done, in this flow's version of it. */
export function complete(progress: SetupProgress, step: StepId): SetupProgress {
  return { ...withStep(progress, step, "done"), stepFlow: { ...progress.stepFlow, [step]: FLOW } };
}

/** "Skip this step": marked skipped, unless it was done before (coming back to a done step and skipping keeps it done). */
export function skip(progress: SetupProgress, step: StepId): SetupProgress {
  return progress.steps[step] === "done" ? progress : withStep(progress, step, "skipped");
}

/** How many steps are not done yet — pending or skipped: the resume bar's "N steps left". */
export function stepsLeft(progress: SetupProgress): number {
  return STEPS.filter((step) => progress.steps[step] !== "done").length;
}

/** Where to pick up: the first pending step, else the first skipped one, else the last. */
export function resumeStep(progress: SetupProgress): StepId {
  return (
    STEPS.find((step) => progress.steps[step] === "pending") ??
    STEPS.find((step) => progress.steps[step] === "skipped") ??
    STEPS[STEPS.length - 1]!
  );
}

/**
 * "Run setup again" (`?rerun=1`): every step pending and the guide not
 * finished, from step 01. What was set up stays set up — the tools, the
 * device's account, start at login — and the steps show it as it is now; the
 * choices stay as defaults, only the first session is picked again.
 */
export function rerun(progress: SetupProgress, now: string): SetupProgress {
  return {
    ...progress,
    startedAt: progress.startedAt ?? now,
    finishedAt: null,
    dismissed: false,
    steps: { tools: "pending", where: "pending", first: "pending", wrap: "pending" },
    first: { cwd: null, agent: null, prompt: null },
  };
}

/**
 * Finish on step 04: done, and the guide is over. Start at login is not the
 * guide's to set: the app turns it on by itself at a new
 * install's first launch and the menu bar changes it, so `wrap.startAtLogin`
 * stays as it was — `null`, never chosen here — and nothing reads it.
 */
export function finish(progress: SetupProgress, now: string): SetupProgress {
  const done = complete(start(progress, now), "wrap");
  return { ...done, finishedAt: now };
}

/** "Don't remind me" on the resume bar. */
export function dismiss(progress: SetupProgress): SetupProgress {
  return { ...progress, dismissed: true };
}

/**
 * Whether Finish opens the first session step 03 saved — the empty-session
 * view with its folder, tool and first message filled in
 * (`first-session.ts`) — rather than the session list. Its label says so.
 */
export function opensFirstSession(progress: SetupProgress): boolean {
  return progress.steps.first === "done" && !!progress.first.cwd && !!progress.first.agent;
}

/**
 * Whether this page load opens the guide by itself: only in the
 * app, only on the page load's first route and only when that is the home
 * (`/` or `/sessions`, never a deep link), and only when the server says the
 * guide never started on an install with no sessions of its own
 * (`autoOpen`) — an existing install never gets it.
 */
export function shouldAutoOpen(input: {
  desktopApp: boolean;
  firstLoad: boolean;
  pathname: string;
  progress: Pick<SetupProgress, "autoOpen"> | null;
}): boolean {
  const home = input.pathname === "/" || input.pathname === "/sessions" || input.pathname === "/sessions/";
  return input.desktopApp && input.firstLoad && home && input.progress?.autoOpen === true;
}

/** The bar above the session list, if any. */
export type SetupBar = { kind: "resume"; left: number } | { kind: "noTool" } | null;

/**
 * Which bar the session list shows: in the app only. A guide
 * started and left unfinished, not dismissed: "Setup: N steps left". An
 * install that never started the guide, when no AI tool is ready: "No AI
 * tool is ready on this device yet". Nothing while either answer is missing.
 */
export function whichBar(input: { desktopApp: boolean; progress: SetupProgress | null | undefined; tools: ToolsReport | null | undefined }): SetupBar {
  const { desktopApp, progress, tools } = input;
  if (!desktopApp || !progress) return null;
  if (progress.startedAt) {
    if (progress.finishedAt || progress.dismissed) return null;
    return { kind: "resume", left: stepsLeft(progress) };
  }
  if (!tools) return null;
  return tools.tools.some((tool) => tool.ready) ? null : { kind: "noTool" };
}

/** A tool's product name: never translated. */
export const TOOL_NAME: Record<SetupTool["id"], string> = { claude: "Claude Code", codex: "Codex" };

/** Where a tool stands, in step 01's words (`tools.st.*`). */
export type ToolStatus = "ready" | "notInstalled" | "tooOld" | "signedOut" | "plan";

export function toolStatus(tool: SetupTool): ToolStatus {
  if (tool.ready) return "ready";
  if (!tool.found) return "notInstalled";
  if (!tool.supported) return "tooOld";
  if (tool.login.state === "signed-in" && tool.login.usable === false) return "plan";
  return "signedOut";
}

/** The status words, as i18n keys. */
export const TOOL_STATUS_LABEL: Record<ToolStatus, string> = {
  ready: "ready",
  notInstalled: "not installed",
  tooOld: "update needed",
  signedOut: "not signed in",
  plan: "plan doesn't include it",
};

/** The tools this device has, by name, for the welcome page's "Found: …". */
export function foundTools(report: ToolsReport): string[] {
  return report.tools.filter((tool) => tool.found).map((tool) => TOOL_NAME[tool.id]);
}
