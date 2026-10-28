/*
 * What the new-session view decides without drawing anything: which
 * directories it offers, what its draft keeps, which device a new session
 * starts on, and the chord that opens it.
 */
import type { components } from "@/api/schema";
import { shortenHome } from "@/lib/format";
import type { MachineKey } from "@/lib/machines/key";
import { readMachineItem, removeMachineItem, writeMachineItem, type KeyValueStorage } from "@/lib/machines/storage";
import type { StoredAttachment } from "./composer-box";
import { isSessionSource, parseAgentKey, type CreationAttempt, type SessionSource } from "./new-session-flow";

type SessionSummary = Pick<components["schemas"]["SessionSummary"], "cwd" | "createdAt" | "updatedAt">;

/** How many directories {@link recentDirectories} returns unless told otherwise (`RECENT_CWDS_LIMIT` on the server). */
export const RECENT_DIRECTORIES = 8;

export interface RecentDirectory {
  /** The directory, absolute. */
  cwd: string;
  /** How many of the device's sessions ran there. */
  sessions: number;
  /** When any of them last did anything, ISO-8601. */
  lastActivity: string;
  /** When the newest of them was started there, ISO-8601: what the list is ordered by. */
  lastStarted: string;
}

/**
 * The directories a device's sessions ran in, the one most recently *started
 * in* first — where work was begun rather than what is loudest, as the
 * server's own recent-directories list ranks them — with how many sessions
 * each holds and when one of them last did anything.
 */
export function recentDirectories(sessions: readonly SessionSummary[] | null, limit = RECENT_DIRECTORIES): RecentDirectory[] {
  const byCwd = new Map<string, RecentDirectory>();
  for (const session of sessions ?? []) {
    const cwd = session.cwd;
    if (!cwd) continue;
    const seen = byCwd.get(cwd);
    if (!seen) {
      byCwd.set(cwd, { cwd, sessions: 1, lastActivity: session.updatedAt, lastStarted: session.createdAt });
      continue;
    }
    seen.sessions += 1;
    if (session.updatedAt > seen.lastActivity) seen.lastActivity = session.updatedAt;
    if (session.createdAt > seen.lastStarted) seen.lastStarted = session.createdAt;
  }
  return [...byCwd.values()]
    .sort((a, b) => (a.lastStarted === b.lastStarted ? a.cwd.localeCompare(b.cwd) : a.lastStarted < b.lastStarted ? 1 : -1))
    .slice(0, limit);
}

/** How many recent directories the empty view lists before 「显示全部（N）」. */
export const SHOWN_DIRECTORIES = 6;

/** The directory rows the empty view draws. */
export interface DirectoryRows {
  /** Recent directories, newest start first; the pick among them even when it is further down. */
  rows: RecentDirectory[];
  /** The pick when no session ran there yet (typed or browsed to): a row of its own, last. */
  other: string | null;
  /** How many recent directories the collapsed list leaves out. */
  hidden: number;
}

/**
 * The rows the directory list shows: the first `shown` recent directories (all
 * of them when `expanded`), and the pick wherever it is — a recent one further
 * down the list, or a path no session ran in yet — so the choice is always on
 * screen.
 */
export function directoryRows(recent: readonly RecentDirectory[], cwd: string, expanded: boolean, shown = SHOWN_DIRECTORIES): DirectoryRows {
  const rows = expanded ? [...recent] : recent.slice(0, shown);
  const picked = cwd ? recent.find((dir) => dir.cwd === cwd) : undefined;
  if (picked && !rows.includes(picked)) rows.push(picked);
  return {
    rows,
    other: cwd && !picked ? cwd : null,
    hidden: expanded ? 0 : Math.max(0, recent.length - shown),
  };
}

/**
 * The directory ↑ (`step` −1) or ↓ (+1) moves the pick to, among the rows on
 * screen (`paths`, in order); null when it stays where it is. From no pick, or
 * one not on screen, ↓ takes the first row and ↑ the last.
 */
export function stepDirectory(paths: readonly string[], cwd: string, step: 1 | -1): string | null {
  if (paths.length === 0) return null;
  const at = paths.indexOf(cwd);
  if (at < 0) return (step > 0 ? paths[0] : paths[paths.length - 1]) ?? null;
  const next = Math.min(paths.length - 1, Math.max(0, at + step));
  return next === at ? null : (paths[next] ?? null);
}

/**
 * A directory as its row reads: its name, and where it is — the parent, the
 * home written `~` — in two parts, a head that gives way first when the row is
 * narrow and the last folder kept whole, so a long path is cut in its middle
 * (`~/Documents/Co…/2026-10-15`), never at the end that tells two apart.
 */
export function directoryLabel(cwd: string, homeDir: string): { name: string; head: string; tail: string } {
  const short = shortenHome(cwd.length > 1 ? cwd.replace(/\/+$/, "") : cwd, homeDir);
  const cut = short.lastIndexOf("/");
  if (short === "~" || short === "/" || cut < 0) return { name: short, head: "", tail: "" };
  const name = short.slice(cut + 1);
  const parent = cut === 0 ? "/" : short.slice(0, cut);
  const split = parent.lastIndexOf("/");
  if (split < 0 || parent === "/") return { name, head: "", tail: parent };
  return { name, head: parent.slice(0, split + 1), tail: parent.slice(split + 1) };
}

/**
 * The directory a new session starts in: the one picked (`picked`, which an
 * opening "new session here" sets), else the one last used — the head of the
 * recent list — else none yet.
 */
export function startingDirectory(picked: string | null, recent: readonly RecentDirectory[]): string {
  if (picked !== null && picked.trim() !== "") return picked;
  return recent[0]?.cwd ?? "";
}

/**
 * What the new-session view keeps on this browser for a device, across reloads,
 * until a session is started with it: the message and its uploaded attachments,
 * and the choices made around it. Model, effort and permission are not here —
 * they are the CLI's stored default, on the device itself.
 */
export interface NewSessionDraft {
  text: string;
  attachments: StoredAttachment[];
  source: SessionSource;
  /** The ACP agent picked, by id, when `source` is `acp`; absent is none picked yet. */
  acpAgent?: string;
  /** Picked by hand; null follows the directory last used. */
  cwd: string | null;
  /** Picked by hand: an account id, or "" for the local login; null follows the CLI's default. */
  account: string | null;
  /**
   * The start already sent with this draft, if one was: pressing send again
   * after a reload goes under the same message id, and is answered with the
   * session the first press started instead of starting a second.
   */
  attempt: CreationAttempt | null;
}

export const EMPTY_NEW_SESSION_DRAFT: NewSessionDraft = { text: "", attachments: [], source: "claude", cwd: null, account: null, attempt: null };

/** The draft's name on its device (`rh.draft.new-session` on the local one). */
const DRAFT = "draft.new-session";

const isString = (value: unknown): value is string => typeof value === "string";

function readAttachments(value: unknown): StoredAttachment[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((a: unknown) => {
    const { path, name, isImage } = (a ?? {}) as Record<string, unknown>;
    return isString(path) && path && isString(name) && typeof isImage === "boolean" ? [{ path, name, isImage }] : [];
  });
}

/** A device's draft, or the empty one; anything unreadable in it is dropped, not trusted. */
export function readNewSessionDraft(storage: KeyValueStorage, machine: MachineKey): NewSessionDraft {
  let raw: string | null;
  try {
    raw = readMachineItem(storage, machine, DRAFT);
  } catch {
    return EMPTY_NEW_SESSION_DRAFT;
  }
  if (!raw) return EMPTY_NEW_SESSION_DRAFT;
  try {
    const stored = JSON.parse(raw) as Record<string, unknown>;
    const attempt = stored.attempt as Partial<CreationAttempt> | null | undefined;
    // Only an id the server could have, and only for an ACP pick.
    const acpAgent = stored.source === "acp" && isString(stored.acpAgent) ? parseAgentKey(`acp:${stored.acpAgent}`)?.acpAgent : null;
    return {
      text: isString(stored.text) ? stored.text : "",
      attachments: readAttachments(stored.attachments),
      source: isSessionSource(stored.source) ? stored.source : "claude",
      ...(acpAgent ? { acpAgent } : {}),
      cwd: isString(stored.cwd) ? stored.cwd : null,
      account: isString(stored.account) ? stored.account : null,
      attempt: attempt && isString(attempt.key) && isString(attempt.clientMessageId) ? { key: attempt.key, clientMessageId: attempt.clientMessageId } : null,
    };
  } catch {
    return EMPTY_NEW_SESSION_DRAFT;
  }
}

/** Whether a draft holds nothing worth keeping (the defaults, and nothing typed or attached). */
export function isEmptyNewSessionDraft(draft: NewSessionDraft): boolean {
  return !draft.text && draft.attachments.length === 0 && draft.source === "claude" && !draft.acpAgent && draft.cwd === null && draft.account === null && draft.attempt === null;
}

/** Keep `draft` as the device's; an empty one removes the key rather than storing defaults. */
export function writeNewSessionDraft(storage: KeyValueStorage, machine: MachineKey, draft: NewSessionDraft): void {
  try {
    if (isEmptyNewSessionDraft(draft)) removeMachineItem(storage, machine, DRAFT);
    else writeMachineItem(storage, machine, DRAFT, JSON.stringify(draft));
  } catch {
    // storage unavailable: the view still works, it only forgets on reload
  }
}

/**
 * After a start went through: the message and the attempt are spent; the
 * choices around them (agent, directory, account) stay for the next one.
 */
export function clearNewSessionDraft(storage: KeyValueStorage, machine: MachineKey): void {
  const draft = readNewSessionDraft(storage, machine);
  writeNewSessionDraft(storage, machine, { ...draft, text: "", attachments: [], attempt: null });
}

/**
 * What the view can be opened with — a directory, a first message, an agent —
 * by `/sessions/new?cwd=…&draft=…&agent=…` (`/m/<device>/sessions/new?…`) or
 * `useOpenNewSession()({ cwd, draft, source })`. Nothing is sent: the person
 * reads it and presses send.
 */
export interface NewSessionPrefill {
  /** The directory to start in, absolute. */
  cwd?: string;
  /** The message, in place of whatever was typed (and attached) before; "" empties the composer. */
  text?: string;
  /** Which agent. */
  source?: SessionSource;
  /** Which ACP agent, by id, when `source` is `acp`. */
  acpAgent?: string;
}

/** `draft` with `prefill` taken into it: what is named replaces what was there, the rest stays. */
export function applyNewSessionPrefill(draft: NewSessionDraft, prefill: NewSessionPrefill | null | undefined): NewSessionDraft {
  if (!prefill) return draft;
  return {
    ...draft,
    ...(prefill.cwd ? { cwd: prefill.cwd } : {}),
    // An account picked for the other CLI cannot start this one.
    ...(prefill.source && prefill.source !== draft.source ? { source: prefill.source, account: null } : {}),
    // Which ACP agent, when it names one; the draft's own pick is read only while it is an ACP one.
    ...(prefill.source === "acp" && prefill.acpAgent ? { acpAgent: prefill.acpAgent } : {}),
    // A different message is a different start: the old one's attempt and attachments go with it.
    ...(prefill.text !== undefined ? { text: prefill.text, attachments: [], attempt: null } : {}),
  };
}

/**
 * Fill a device's new-session draft before its view opens (another device's
 * page, or code that navigates by itself); a view already open takes a prefill
 * through its `prefill` prop instead.
 */
export function prefillNewSessionDraft(storage: KeyValueStorage, machine: MachineKey, prefill: NewSessionPrefill): void {
  writeNewSessionDraft(storage, machine, applyNewSessionPrefill(readNewSessionDraft(storage, machine), prefill));
}

/**
 * The prefill an address names (`?cwd=…&draft=…&agent=…`, the agent as
 * `claude`, `codex` or `acp:<id>`), or null when it names none.
 */
export function prefillFromSearch(search: { cwd?: string; draft?: string; agent?: string }): NewSessionPrefill | null {
  const agent = parseAgentKey(search.agent);
  const prefill: NewSessionPrefill = {
    ...(search.cwd ? { cwd: search.cwd } : {}),
    ...(search.draft !== undefined ? { text: search.draft } : {}),
    ...(agent ? { source: agent.source, ...(agent.acpAgent ? { acpAgent: agent.acpAgent } : {}) } : {}),
  };
  return Object.keys(prefill).length > 0 ? prefill : null;
}

/** The device the last session was started on from this browser (droppen.app, several devices). */
const LAST_DEVICE = "rh.hub.new-session-device";

export function lastDevice(storage: KeyValueStorage): MachineKey | null {
  try {
    return storage.getItem(LAST_DEVICE);
  } catch {
    return null;
  }
}

export function rememberDevice(storage: KeyValueStorage, machine: MachineKey): void {
  try {
    storage.setItem(LAST_DEVICE, machine);
  } catch {
    // nothing to remember it in: the next one starts on its default
  }
}

/** The parts of a `KeyboardEvent` the chord reads. */
export interface ChordKey {
  key: string;
  code?: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  isComposing?: boolean;
}

/**
 * The chords that open a new session.
 *
 * Cmd+Shift+O on a Mac, Ctrl+Shift+O elsewhere, everywhere: a browser lets a
 * page take it (Cmd/Ctrl+N it keeps for a new window and never hands over).
 * In the desktop app's window (`desktop`), whose webview does hand it over,
 * Cmd+N (Ctrl+N) as well. Read the way the find chord is (`isFindChord`): the
 * letter off `key` on a Latin layout and off the physical key elsewhere, Ctrl
 * on a Mac being the text fields' own (Ctrl+N is "next line" there), Alt
 * never.
 */
export function isNewSessionChord(event: ChordKey, isMac: boolean, desktop = false): boolean {
  if (event.isComposing || event.altKey) return false;
  if (!(isMac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey)) return false;
  const is = (letter: "n" | "o") => (/^[a-z]$/i.test(event.key) ? event.key.toLowerCase() === letter : event.code === `Key${letter.toUpperCase()}`);
  return event.shiftKey ? is("o") : desktop && is("n");
}

/** How to write the chord that works here: the one {@link isNewSessionChord} takes and the browser does not keep. */
export function newSessionShortcut(isMac: boolean, desktop: boolean): string {
  if (desktop) return isMac ? "⌘N" : "Ctrl+N";
  return isMac ? "⇧⌘O" : "Ctrl+Shift+O";
}
