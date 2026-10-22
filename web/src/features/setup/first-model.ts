/*
 * Step 03's rules: which
 * tools can run the first session, which first messages are offered for the
 * folder picked, when macOS will ask about a protected folder, and what
 * Continue stores — `progress.first = { cwd, agent, prompt }`. No React and
 * no requests: `first-step.tsx` draws these, the tests run them.
 */
import type { SetupProgress, ToolId, ToolsReport } from "./types";

/** A suggested first message, or the user's own. */
export type PromptChoice = "repo1" | "repo2" | "practice" | "own";

/** What step 03 has picked so far, kept by the guide while it is open. */
export interface FirstUi {
  /** The folder the session starts in. */
  cwd: string | null;
  /** The folder is the practice folder the guide made (its first messages differ). */
  practice: boolean;
  /** The tool picked; `null` follows the first ready one. */
  agent: ToolId | null;
  prompt: PromptChoice | null;
  /** "Write my own". */
  own: string;
  /** "Choose another folder…" is open. */
  browsing: boolean;
}

export const EMPTY_FIRST_UI: FirstUi = { cwd: null, practice: false, agent: null, prompt: null, own: "", browsing: false };

/** The suggestions (i18n keys): in a project they only ask the AI to look; in the practice folder it makes something. */
export const PROMPTS: Record<Exclude<PromptChoice, "own">, string> = {
  repo1: "Explain this project: what it does and how to run it. Don't change any files yet.",
  repo2: "List the three things most worth fixing here first. Just list them; don't change anything.",
  practice: "Make a one-page website that shows today's date and a short quote, then tell me how to open it.",
};

/** The note over the empty-session view the guide lands on, an i18n key. */
export const LANDED = "Press Send when you're ready. Model, effort and permission are in the row below.";

/** The practice folder's place (the server adds -2, -3… when it is taken). */
export const PRACTICE_PATH = "~/Droppen/first-session";

/** Whether a path is a practice folder the guide made (`~/Droppen/first-session`, `-2`, …). */
export function isPracticeFolder(path: string): boolean {
  return /\/Droppen\/first-session(-\d+)?\/?$/.test(path);
}

/** The tools a first session can run with: the ready ones, Claude Code first. */
export function readyAgents(report: ToolsReport | null | undefined): ToolId[] {
  return (report?.tools ?? []).filter((tool) => tool.ready).map((tool) => tool.id);
}

/** Whether step 03 has nothing to start with (detection answered, nothing ready). */
export function noToolReady(report: ToolsReport | null | undefined): boolean {
  return report !== undefined && readyAgents(report).length === 0;
}

/** The tool the session runs with: the one picked while it is ready, else the first ready one. */
export function agentOf(ui: FirstUi, report: ToolsReport | null | undefined): ToolId | null {
  const ready = readyAgents(report);
  return ui.agent && ready.includes(ui.agent) ? ui.agent : (ready[0] ?? null);
}

/** The first messages offered for the folder: two that only look in a project, one that builds in the practice folder; and the user's own. */
export function promptChoices(practice: boolean): PromptChoice[] {
  return practice ? ["practice", "own"] : ["repo1", "repo2", "own"];
}

/** The choice shown: the one picked while it is offered, else the first offered. */
export function promptOf(ui: FirstUi): PromptChoice {
  const offered = promptChoices(ui.practice);
  return ui.prompt && offered.includes(ui.prompt) ? ui.prompt : offered[0]!;
}

/** The message itself, in the reader's language (`t`), or the user's own words. */
export function promptText(ui: FirstUi, t: (key: string) => string): string {
  const choice = promptOf(ui);
  return choice === "own" ? ui.own.trim() : t(PROMPTS[choice]);
}

/** The folders macOS guards (TCC): asked once whether Droppen may open them. */
const PROTECTED = ["Documents", "Desktop", "Downloads"];

/** Whether `path` is in ~/Documents, ~/Desktop or ~/Downloads (or is one of them). */
export function protectedFolder(path: string | null, home: string): boolean {
  if (!path || !home) return false;
  const under = (dir: string) => path === `${home}/${dir}` || path.startsWith(`${home}/${dir}/`);
  return PROTECTED.some(under);
}

/** What Continue stores, or `null` while something is missing: a folder, a ready tool, and a message (unless the user's own is empty). */
export function firstOf(ui: FirstUi, report: ToolsReport | null | undefined, t: (key: string) => string): SetupProgress["first"] | null {
  const agent = agentOf(ui, report);
  const prompt = promptText(ui, t);
  if (!ui.cwd || !agent || !prompt) return null;
  return { cwd: ui.cwd, agent, prompt };
}

/** Coming back to step 03: what it stored, the message as the user's own words. */
export function firstUiFrom(progress: SetupProgress): FirstUi {
  const { cwd, agent, prompt } = progress.first;
  if (!cwd) return EMPTY_FIRST_UI;
  return { ...EMPTY_FIRST_UI, cwd, practice: isPracticeFolder(cwd), agent, prompt: prompt ? "own" : null, own: prompt ?? "" };
}

/** How long ago, in the reader's language ("3 days ago", 「3 天前」). */
export function ago(iso: string | null, language: string, now = Date.now()): string {
  if (!iso) return "";
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "";
  const minutes = Math.round((then - now) / 60_000);
  const format = new Intl.RelativeTimeFormat(language, { numeric: "auto" });
  if (Math.abs(minutes) < 60) return format.format(minutes, "minute");
  const hours = Math.round(minutes / 60);
  if (Math.abs(hours) < 24) return format.format(hours, "hour");
  const days = Math.round(hours / 24);
  if (Math.abs(days) < 14) return format.format(days, "day");
  const weeks = Math.round(days / 7);
  if (Math.abs(weeks) < 9) return format.format(weeks, "week");
  return format.format(Math.round(days / 30), "month");
}
