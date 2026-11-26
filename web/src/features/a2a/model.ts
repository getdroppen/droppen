/*
 * The A2A settings, as data (`docs/a2a.md` §2, §4, §6): what a new entry
 * starts with, which permission modes an entry may offer, the bodies the
 * management API takes, an entry's Agent Card URL, and what the task list
 * says about sessions. No React here, so the rules can be tested on their own.
 */
import type { components } from "@/api/schema";
import { DANGER_PERMISSIONS, PERMISSION_OPTIONS } from "@/features/sessions/composer-controls";

export type A2aEntry = components["schemas"]["A2aEntry"];
export type A2aEntryCreate = components["schemas"]["A2aEntryCreate"];
export type A2aEntryUpdate = components["schemas"]["A2aEntryUpdate"];
export type A2aToken = components["schemas"]["A2aToken"];
export type A2aTask = components["schemas"]["A2aTaskSummary"];
export type TaskState = components["schemas"]["TaskState"];
export type RunAccount = components["schemas"]["RunAccount"];
export type Isolation = components["schemas"]["Isolation"];

/** The agents an entry can run: the two CLIs, or one configured ACP agent (`tck` is the TCK's alone). */
export type EntrySource = "claude" | "codex" | "acp";

/** An entry's id, as it goes into its URL (`^[a-z0-9][a-z0-9-]{0,39}$`). */
const ENTRY_ID = /^[a-z0-9][a-z0-9-]{0,39}$/;

/** A token's name: letters, digits, `.`, `_` and `-`, at most 64. */
const TOKEN_NAME = /^[A-Za-z0-9._-]{1,64}$/;

/** At most this many tasks may work at once in one entry (`maxConcurrent`). */
export const MAX_CONCURRENT = 8;

/**
 * The permission a new entry starts with: edits accepted, but no bypass
 * (§6). Codex's `workspace-write` is the sandbox the composer calls `auto`.
 */
export const ENTRY_DEFAULT_PERMISSION = { claude: "acceptEdits", codex: "workspace-write" } as const;

/** The modes each CLI offers an entry: the composer's, with Codex's `auto` under its sandbox name. */
const ENTRY_PERMISSIONS: Record<"claude" | "codex", readonly string[]> = {
  claude: PERMISSION_OPTIONS.claude,
  codex: PERMISSION_OPTIONS.codex.map((mode) => (mode === "auto" ? ENTRY_DEFAULT_PERMISSION.codex : mode)),
};

/** Whether `permission` drops the guard rails (`bypassPermissions`, `danger-full-access` and their kin). */
export function isBypass(permission: string | null | undefined): boolean {
  return !!permission && DANGER_PERMISSIONS.has(permission);
}

/**
 * The modes an entry of `source` may be given. A bypass mode needs the entry
 * to name the proxies it takes (`allowedDevices`): an empty list is every
 * proxy, and a device that reaches it from afar must not be able to run
 * anything at all here. The mode it holds
 * now is kept in the list, so the form shows what is saved, even when it is
 * one the rule now refuses ({@link entryProblems} says so).
 */
export function permissionOptions(source: "claude" | "codex", allowedDevices: readonly string[], current = ""): string[] {
  const offered = ENTRY_PERMISSIONS[source].filter((mode) => allowedDevices.length > 0 || !isBypass(mode));
  return current && !offered.includes(current) ? [...offered, current] : offered;
}

/** The entry form, as typed: empty strings are "not set" (the CLI's default, the default template). */
export interface EntryForm {
  id: string;
  /** The id was typed, rather than made from the name. */
  idTyped: boolean;
  name: string;
  description: string;
  source: EntrySource;
  acpAgent: string | null;
  cwd: string;
  account: RunAccount;
  model: string;
  effort: string;
  permission: string;
  isolation: Isolation;
  maxConcurrent: number;
  template: string;
  allowRemoteAnswers: boolean;
  /** The push origins, one per line. */
  pushOrigins: string;
  /** The a2a devices that may call it (§7); read here, chosen in a later milestone. */
  allowedDevices: string[];
}

/**
 * A new entry: a worktree per context, edits accepted but nothing bypassed,
 * questions answerable over A2A, one task at a time — and off until it is
 * saved complete and switched on ({@link createBody}).
 */
export function newEntryForm(source: EntrySource = "claude", acpAgent: string | null = null): EntryForm {
  return {
    id: "",
    idTyped: false,
    name: "",
    description: "",
    source,
    acpAgent: source === "acp" ? acpAgent : null,
    cwd: "",
    account: { kind: "default" },
    model: "",
    effort: "",
    permission: source === "acp" ? "" : ENTRY_DEFAULT_PERMISSION[source],
    isolation: "worktree",
    maxConcurrent: 1,
    template: "",
    allowRemoteAnswers: true,
    pushOrigins: "",
    allowedDevices: [],
  };
}

/** The form for an entry already saved. */
export function entryFormOf(entry: A2aEntry): EntryForm {
  const source: EntrySource = entry.source === "codex" || entry.source === "acp" ? entry.source : "claude";
  return {
    id: entry.id,
    idTyped: true,
    name: entry.name,
    description: entry.description,
    source,
    acpAgent: entry.acpAgent,
    cwd: entry.cwd,
    account: entry.account,
    model: entry.model ?? "",
    effort: entry.effort ?? "",
    permission: entry.permission ?? "",
    isolation: entry.isolation,
    maxConcurrent: entry.maxConcurrent,
    template: entry.template ?? "",
    allowRemoteAnswers: entry.allowRemoteAnswers,
    pushOrigins: entry.pushOrigins.join("\n"),
    allowedDevices: [...entry.allowedDevices],
  };
}

/**
 * The form on another agent. The CLIs share no model ids, effort ladders,
 * permission names or accounts, so those go back to the new agent's
 * defaults; an ACP agent takes none of them.
 */
export function withSource(form: EntryForm, source: EntrySource, acpAgent: string | null = null): EntryForm {
  const fresh = newEntryForm(source, acpAgent);
  return { ...form, source, acpAgent: fresh.acpAgent, account: fresh.account, model: "", effort: "", permission: fresh.permission };
}

/** An id made from a name: lower case, dashes for the rest, 40 at most. Empty when nothing of the name fits. */
export function idFromName(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+/, "")
    .slice(0, 40)
    .replace(/-+$/, "");
}

/** The form with a new name, and the id made from it while the id was never typed. */
export function withName(form: EntryForm, name: string): EntryForm {
  return { ...form, name, id: form.idTyped ? form.id : idFromName(name) };
}

/** The push origins typed, split into the ones that are an origin (`https://host[:port]`) and the rest. */
export function parseOrigins(text: string): { origins: string[]; bad: string[] } {
  const origins: string[] = [];
  const bad: string[] = [];
  for (const word of text.split(/[\s,]+/)) {
    const value = word.trim().replace(/\/+$/, "");
    if (!value) continue;
    if (/^https:\/\/[^/\s?#@]+$/i.test(value)) {
      if (!origins.includes(value)) origins.push(value);
    } else bad.push(value);
  }
  return { origins, bad };
}

/** What stops the form from being saved, in the order the form asks. */
export type EntryProblem = "id" | "name" | "acpAgent" | "cwd" | "bypassWithDevices" | "pushOrigins";

/** What each problem says (the translation key), beside the save it holds back. */
export const ENTRY_PROBLEM_WORD: Record<EntryProblem, string> = {
  id: "the id is 1–40 lower-case letters, digits and dashes, starting with a letter or digit",
  name: "give it a name",
  acpAgent: "choose the ACP agent",
  cwd: "choose the directory its sessions start in",
  bypassWithDevices: "a mode that bypasses permissions needs the entry to name the A2A devices it takes (an empty list is every device)",
  pushOrigins: "a push origin is https://host or https://host:port",
};

export function entryProblems(form: EntryForm): EntryProblem[] {
  const problems: EntryProblem[] = [];
  if (!ENTRY_ID.test(form.id)) problems.push("id");
  if (!form.name.trim()) problems.push("name");
  if (form.source === "acp" && !form.acpAgent) problems.push("acpAgent");
  if (!form.cwd.trim().startsWith("/")) problems.push("cwd");
  if (form.source !== "acp" && form.allowedDevices.length === 0 && isBypass(form.permission)) problems.push("bypassWithDevices");
  if (parseOrigins(form.pushOrigins).bad.length > 0) problems.push("pushOrigins");
  return problems;
}

/** `POST /api/a2a/entries`: off, always — it is switched on in the list once saved. */
export function createBody(form: EntryForm): A2aEntryCreate {
  const cli = form.source !== "acp";
  const description = form.description.trim();
  const template = form.template.trim();
  const { origins } = parseOrigins(form.pushOrigins);
  return {
    id: form.id,
    name: form.name.trim(),
    source: form.source,
    enabled: false,
    cwd: form.cwd.trim(),
    isolation: form.isolation,
    maxConcurrent: form.maxConcurrent,
    allowRemoteAnswers: form.allowRemoteAnswers,
    ...(description ? { description } : {}),
    ...(!cli && form.acpAgent ? { acpAgent: form.acpAgent } : {}),
    ...(cli ? { account: form.account } : {}),
    ...(cli && form.model ? { model: form.model } : {}),
    ...(cli && form.effort ? { effort: form.effort } : {}),
    ...(cli && form.permission ? { permission: form.permission } : {}),
    ...(template ? { template: form.template } : {}),
    ...(origins.length ? { pushOrigins: origins } : {}),
  };
}

/**
 * `PATCH /api/a2a/entries/{id}`: every field the form edits, `""` where it
 * goes back to the default (the API's way to clear a nullable field). Not
 * `enabled` (the list's switch) and not `allowedDevices` (not chosen here yet).
 */
export function updateBody(form: EntryForm): A2aEntryUpdate {
  const cli = form.source !== "acp";
  return {
    name: form.name.trim(),
    description: form.description.trim(),
    source: form.source,
    acpAgent: cli ? "" : (form.acpAgent ?? ""),
    cwd: form.cwd.trim(),
    account: cli ? form.account : { kind: "default" },
    model: cli ? form.model : "",
    effort: cli ? form.effort : "",
    permission: cli ? form.permission : "",
    isolation: form.isolation,
    maxConcurrent: form.maxConcurrent,
    template: form.template.trim() ? form.template : "",
    allowRemoteAnswers: form.allowRemoteAnswers,
    pushOrigins: parseOrigins(form.pushOrigins).origins,
  };
}

/** The base an Agent Card's URL starts with: the configured public base, else this page's origin (or none). */
export function cardBase(publicBase: string | null | undefined, origin: string | null): string | null {
  const base = publicBase?.trim().replace(/\/+$/, "");
  return base || origin?.replace(/\/+$/, "") || null;
}

/** The URL an A2A client is given for an entry: its card is under it, at `/.well-known/agent-card.json`. */
export function cardUrl(base: string | null, id: string): string {
  return `${base ?? ""}/a2a/agents/${id}`;
}

/** Whether a token's name is one the server takes. */
export function validTokenName(name: string): boolean {
  return TOKEN_NAME.test(name.trim());
}

/** The task states, in the order a person reads them: open first, then how it ended. */
export const TASK_STATES = [
  "TASK_STATE_SUBMITTED",
  "TASK_STATE_WORKING",
  "TASK_STATE_INPUT_REQUIRED",
  "TASK_STATE_AUTH_REQUIRED",
  "TASK_STATE_COMPLETED",
  "TASK_STATE_FAILED",
  "TASK_STATE_REJECTED",
  "TASK_STATE_CANCELED",
] as const satisfies readonly TaskState[];

/** A state's word (the translation key). */
export const TASK_STATE_WORD: Record<TaskState, string> = {
  TASK_STATE_UNSPECIFIED: "unknown",
  TASK_STATE_SUBMITTED: "queued",
  TASK_STATE_WORKING: "working",
  TASK_STATE_INPUT_REQUIRED: "waiting for an answer",
  TASK_STATE_AUTH_REQUIRED: "waiting for sign-in",
  TASK_STATE_COMPLETED: "completed",
  TASK_STATE_FAILED: "failed",
  TASK_STATE_REJECTED: "rejected",
  TASK_STATE_CANCELED: "canceled",
};

/**
 * The state's colour, where it has one (docs/ui-spec.md §1): working is
 * `ok`, waiting on a person is `warn`, an end that did not do the work is
 * `danger`; queued, done and canceled stay ink.
 */
export function taskSignal(state: TaskState): "working" | "warn" | "failed" | null {
  switch (state) {
    case "TASK_STATE_WORKING":
      return "working";
    case "TASK_STATE_INPUT_REQUIRED":
    case "TASK_STATE_AUTH_REQUIRED":
      return "warn";
    case "TASK_STATE_FAILED":
    case "TASK_STATE_REJECTED":
      return "failed";
    default:
      return null;
  }
}

/** Who a caller is, as a person reads it: a token by its name, an a2a device by its id. */
export function callerLabel(caller: string): { kind: "token" | "device" | "other"; name: string } {
  if (caller.startsWith("a2a:token:")) return { kind: "token", name: caller.slice("a2a:token:".length) };
  if (caller.startsWith("a2a:device:")) return { kind: "device", name: caller.slice("a2a:device:".length) };
  return { kind: "other", name: caller };
}

/** What marks a session row as started over A2A: who sent it, through which entry. */
export interface A2aMark {
  caller: string;
  entry: string;
}

/** A session's key in {@link a2aMarks}. */
export const sessionMarkKey = (source: string, id: string) => `${source}/${id}`;

/**
 * The sessions the tasks ran in, by `source/id`, each with the caller and
 * entry of its newest task (the list is newest first). A task whose session
 * is not known yet marks nothing.
 */
export function a2aMarks(tasks: readonly A2aTask[]): Map<string, A2aMark> {
  const marks = new Map<string, A2aMark>();
  for (const task of tasks) {
    if (!task.sessionId) continue;
    const key = sessionMarkKey(task.source, task.sessionId);
    if (!marks.has(key)) marks.set(key, { caller: task.caller, entry: task.entry });
  }
  return marks;
}
