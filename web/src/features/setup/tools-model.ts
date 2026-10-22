/*
 * Step 01's rules: which state each tool's card is in, which card holds the
 * page's one primary, which
 * whitelisted command a confirm box shows, how a failed install is said in
 * plain words, when to poll, and where F6 goes. No React and no requests:
 * `tools-step.tsx` draws these, the tests run them.
 */
import type { QuotaWindow } from "@/features/accounts/use-accounts";
import type { SetupCommand, SetupTask, SetupTool, ToolId, ToolsReport } from "./types";

/** Every state a tool's card can be in, plus the forms a card opens. */
export type CardState =
  | "checking"
  | "notInstalled"
  | "confirm"
  | "installing"
  | "failed"
  | "signedOut"
  | "signingIn"
  | "ready"
  | "tooOld"
  | "notFound"
  | "fallback"
  | "plan"
  | "apiKey"
  | "choosePath";

/** What the user opened on a card, kept by the step while it is on screen. */
export interface CardUi {
  /** The confirm box (install or update), the API key form (Codex), the path field (not found). */
  open: "none" | "confirm" | "apiKey" | "choosePath";
  /** What the confirm box runs. */
  kind: "install" | "update";
  /** The method picked in the confirm box; `null` takes the default. */
  method: string | null;
  /** "Browser didn't open?" was pressed for the sign-in running now. */
  noBrowser: boolean;
  /** A task the user put away (stopped, cancelled, retried), by its `startedAt`: no longer drawn. */
  dismissed: string | null;
}

export type ToolsUi = Record<ToolId, CardUi>;

export const IDLE_CARD: CardUi = { open: "none", kind: "install", method: null, noBrowser: false, dismissed: null };
export const IDLE_TOOLS_UI: ToolsUi = { claude: IDLE_CARD, codex: IDLE_CARD };

/** How often a running task's tool is detected again, and a running sign-in checked. */
export const TASK_POLL_MS = 2_000;
export const LOGIN_POLL_MS = 2_000;
/** A sign-in is checked for at most this long. */
export const LOGIN_POLL_LIMIT_MS = 10 * 60_000;

/** The official guides, when the server's commands do not say. */
export const TOOL_DOCS: Record<ToolId, string> = {
  claude: "https://code.claude.com/docs/en/setup",
  codex: "https://github.com/openai/codex",
};

/** Where the Claude plans are (the plan doesn't include Claude Code). */
export const CLAUDE_PLANS_URL = "https://claude.ai/upgrade";

/** The tool's task, unless the user put it away. */
export function liveTask(tool: SetupTool, ui: CardUi): SetupTask | null {
  const task = tool.task;
  return task && task.startedAt !== ui.dismissed ? task : null;
}

/**
 * An install or update that ended badly, and the tool still isn't there (or
 * still too old). The server says `failed` for a non-zero exit, and for an
 * installer that exited 0 without installing anything.
 */
export function failedInstall(tool: SetupTool, task: SetupTask | null): task is SetupTask {
  if (!task || task.running || task.kind === "login" || !task.failed) return false;
  return task.kind === "install" ? !tool.found : !tool.supported;
}

/** A sign-in that ended badly with the tool still signed out: said on the signed-out card. */
export function failedLogin(tool: SetupTool, task: SetupTask | null): task is SetupTask {
  if (!task || task.running || task.kind !== "login") return false;
  return task.failed && tool.login.state !== "signed-in";
}

/** The state of one tool's card: `tool` is `undefined` while detection runs. */
export function cardState(tool: SetupTool | undefined, report: Pick<ToolsReport, "pathRead"> | undefined, ui: CardUi): CardState {
  if (!tool || !report) return "checking";
  const task = liveTask(tool, ui);
  if (task?.running) return task.kind === "login" ? "signingIn" : "installing";
  if (ui.open === "confirm") return "confirm";
  if (ui.open === "apiKey") return "apiKey";
  if (ui.open === "choosePath") return "choosePath";
  if (failedInstall(tool, task)) return "failed";
  // Found only outside the PATH sessions use: adopt it first, or no session finds it.
  if (tool.found?.via === "fallback") return "fallback";
  if (tool.ready) return "ready";
  if (!tool.found) return report.pathRead === "timeout" || report.pathRead === "failed" ? "notFound" : "notInstalled";
  if (!tool.supported) return "tooOld";
  if (tool.login.state === "signed-in" && tool.login.usable === false) return "plan";
  return "signedOut";
}

/** A card the user is in the middle of: its button outranks every other card's. */
export function engaged(state: CardState, ui: CardUi, id: ToolId): boolean {
  return state === "confirm" || state === "apiKey" || state === "choosePath" || (state === "signingIn" && ui.noBrowser && id === "codex");
}

const NEEDS_USER: ReadonlySet<CardState> = new Set(["notInstalled", "confirm", "failed", "signedOut", "tooOld", "notFound", "fallback", "plan", "apiKey", "choosePath"]);

/** Whether a card waits for the user (it has a main action to take). */
export function needsUser(state: CardState, ui: CardUi, id: ToolId): boolean {
  return NEEDS_USER.has(state) || engaged(state, ui, id);
}

/**
 * The card that holds the page's one primary (V10): one the user is in
 * the middle of, else the first that waits for the user, else none — then
 * "Continue" is primary once a tool is ready.
 */
export function primaryOwner(cards: { id: ToolId; state: CardState; ui: CardUi }[]): ToolId | null {
  return cards.find((c) => engaged(c.state, c.ui, c.id))?.id ?? cards.find((c) => needsUser(c.state, c.ui, c.id))?.id ?? null;
}

/** The cards of a report, in order, with their states. */
export function cardsOf(report: ToolsReport | null | undefined, ui: ToolsUi): { id: ToolId; tool: SetupTool | undefined; state: CardState; ui: CardUi }[] {
  if (report === null) return [];
  const ids: ToolId[] = report ? report.tools.map((t) => t.id) : ["claude", "codex"];
  return ids.map((id) => {
    const tool = report?.tools.find((t) => t.id === id);
    return { id, tool, state: cardState(tool, report ?? undefined, ui[id]), ui: ui[id] };
  });
}

/** Whether step 01's cards hold the page's primary (the action bar's Continue then is not). */
export function cardsOwnPrimary(report: ToolsReport | null | undefined, ui: ToolsUi): boolean {
  return primaryOwner(cardsOf(report, ui)) !== null;
}

const METHOD_ORDER = ["native", "brew", "npm"];

/** One tool's commands of one kind, the official installer first: the ones this device can use, and the others with their reasons. */
export function methodChoices(commands: readonly SetupCommand[], tool: ToolId, kind: "install" | "update"): { available: SetupCommand[]; unavailable: SetupCommand[] } {
  const all = commands.filter((c) => c.tool === tool && c.kind === kind).sort((a, b) => METHOD_ORDER.indexOf(a.method) - METHOD_ORDER.indexOf(b.method));
  return { available: all.filter((c) => c.available), unavailable: all.filter((c) => !c.available) };
}

/**
 * The methods a confirm box offers: for an update, only the way the tool was
 * installed when that is known and usable (an npm copy is updated with npm);
 * otherwise every usable one.
 */
export function offeredMethods(commands: readonly SetupCommand[], tool: SetupTool, kind: "install" | "update"): SetupCommand[] {
  const { available } = methodChoices(commands, tool.id, kind);
  if (kind === "update") {
    const same = available.find((c) => c.method === tool.found?.method);
    if (same) return [same];
  }
  return available;
}

/** The command a confirm box shows and runs: the one picked, else the recommended, else the first usable. */
export function chosenCommand(commands: readonly SetupCommand[], tool: SetupTool, ui: CardUi): SetupCommand | null {
  const offered = offeredMethods(commands, tool, ui.kind);
  return offered.find((c) => c.method === ui.method) ?? offered.find((c) => c.recommended) ?? offered[0] ?? null;
}

/** The sign-in command for a mode (shown and copied where this page cannot run it). */
export function loginCommand(commands: readonly SetupCommand[], tool: ToolId, mode: string): SetupCommand | null {
  return commands.find((c) => c.tool === tool && c.kind === "login" && c.method === mode) ?? null;
}

/** A tool's official guide. */
export function docsFor(commands: readonly SetupCommand[], tool: ToolId): string {
  return commands.find((c) => c.tool === tool && c.docs)?.docs ?? TOOL_DOCS[tool];
}

/** The sign-in a tool's main "Sign in" runs: a Claude subscription, or ChatGPT. */
export function defaultLoginMode(tool: ToolId): "claudeai" | "chatgpt" {
  return tool === "claude" ? "claudeai" : "chatgpt";
}

/** Why a method is not offered (i18n keys). */
export const UNAVAILABLE_WORDS = {
  macos_too_old: "The official installer needs macOS 13 or later.",
  no_brew: "Homebrew isn't on this Mac.",
  no_node: "npm needs Node.js, which isn't on this Mac.",
  other: "Not available on this device.",
} as const;

/** Why a method is not offered, as an i18n key. */
export function unavailableReason(command: Pick<SetupCommand, "method" | "reason">): string {
  const reason = command.reason ?? (command.method === "brew" ? "no_brew" : command.method === "npm" ? "no_node" : "other");
  return UNAVAILABLE_WORDS[reason as keyof typeof UNAVAILABLE_WORDS] ?? UNAVAILABLE_WORDS.other;
}

/** A failed install or update in words (i18n keys). */
export const FAILURE_WORDS = {
  install: "The install didn't finish (exit code {{code}}).",
  update: "The update didn't finish (exit code {{code}}).",
  installedNothing: "The installer finished, but the tool still isn't on this device.",
  updatedNothing: "The update finished, but the version is still too old.",
  network: "Couldn't reach the download server. Check the connection and try again, or run the command in your own Terminal.",
  brewPermission: "Permission denied. Homebrew on this Mac may belong to another user; the official installer installs into your home folder instead.",
  permission: "Permission denied: it couldn't write where it installs. Try the official installer, which installs into your home folder, or run the command in your own Terminal.",
  disk: "The disk is full. Free some space, then try again.",
  other: "The last lines are below. Try again, or run the command in your own Terminal.",
} as const;

/**
 * What went wrong with a finished install or update, as i18n keys: the
 * headline with its exit code (or, for one that exited 0, that it did not do
 * its job), and the reason in plain words — the network, a permission (a
 * Homebrew owned by another user on a shared Mac), a full disk, or
 * anything else, read from the last lines.
 */
export function failureWords(task: SetupTask): { title: string; code: number | null; why: string } {
  const why =
    task.failure === "network"
      ? FAILURE_WORDS.network
      : task.failure === "permission"
        ? task.method === "brew"
          ? FAILURE_WORDS.brewPermission
          : FAILURE_WORDS.permission
        : task.failure === "disk"
          ? FAILURE_WORDS.disk
          : FAILURE_WORDS.other;
  const update = task.kind === "update";
  const title =
    task.exitCode === 0 ? (update ? FAILURE_WORDS.updatedNothing : FAILURE_WORDS.installedNothing) : update ? FAILURE_WORDS.update : FAILURE_WORDS.install;
  return { title, code: task.exitCode, why };
}

/** How often to detect again: every 2 s while any task runs, else not at all. */
export function toolsPoll(report: ToolsReport | null | undefined): number | false {
  return report?.tools.some((tool) => tool.task?.running) ? TASK_POLL_MS : false;
}

/**
 * Whether to check a tool's sign-in now: only while its sign-in runs,
 * the page is visible, and for at most ten minutes from its start.
 */
export function pollsLogin(tool: SetupTool | undefined, ui: CardUi, visible: boolean, now: number): boolean {
  if (!tool || !visible) return false;
  const task = liveTask(tool, ui);
  if (!task?.running || task.kind !== "login") return false;
  const started = Date.parse(task.startedAt);
  return Number.isNaN(started) || now - started < LOGIN_POLL_LIMIT_MS;
}

/** A task's running time, `m:ss` (`h:mm:ss` past an hour). */
export function elapsed(startedAt: string, now: number): string {
  const started = Date.parse(startedAt);
  const total = Number.isNaN(started) ? 0 : Math.max(0, Math.floor((now - started) / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = String(total % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${String(m).padStart(2, "0")}:${s}`;
}

/** The two windows a ready card draws: the 5-hour one and the weekly one, when the reading has them. */
export function readyWindows(windows: readonly QuotaWindow[] | null | undefined): { fiveHour: QuotaWindow | null; week: QuotaWindow | null } {
  const list = windows ?? [];
  const fiveHour = list.find((w) => w.windowMinutes === 300) ?? list.find((w) => /^5\s*h/i.test(w.label)) ?? null;
  const week = list.find((w) => w.windowMinutes === 10_080 && !/opus|sonnet/i.test(w.label)) ?? list.find((w) => /week/i.test(w.label)) ?? null;
  return { fiveHour, week };
}

/**
 * Where F6 goes: through the regions in order — each embedded
 * terminal, then the action bar — and round again; Shift+F6 the other way.
 * From anywhere else on the page it goes to the first terminal.
 */
export function f6Next(regions: number, current: number, back: boolean): number {
  if (regions <= 0) return -1;
  if (current < 0) return 0;
  return (current + (back ? regions - 1 : 1)) % regions;
}
