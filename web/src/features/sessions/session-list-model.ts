import type { components } from "@/api/schema";
import { basename, shortenHome } from "@/lib/format";
import { sessionKey, type MachineKey } from "@/lib/machines/key";
import { activitySortRank } from "@/features/sessions/session-activity";

type SessionSummary = components["schemas"]["SessionSummary"];
type Source = components["schemas"]["Source"];
type SessionChild = components["schemas"]["SessionChild"];

/** The group a session with no working directory at all falls into. */
export const NO_PROJECT = "(no project)";

/** How many rows the list will ever render. */
export const SECTION_LIMIT = 300;

export type SessionListRow =
  | {
      type: "header";
      dir: string;
      /** What the fold is remembered under; see [`collapseKey`]. */
      collapseKey: string;
      list: SessionSummary[];
      isOpen: boolean;
      /** The device its sessions are on, in a list of several devices' (absent: the list's own). */
      machine?: MachineKey;
    }
  | {
      type: "session";
      session: SessionSummary;
      dirLabel: string | null;
      /** The device it is on, in a list of several devices' (absent: the list's own). */
      machine?: MachineKey;
      /** What its subagents' fold is remembered under (absent: they are not listed here); see [`expandKey`]. */
      expandKey?: string;
      /** Its subagents are listed under it. */
      isOpen?: boolean;
    }
  | {
      /** A subagent the session above delegated to, or one of that one's own. */
      type: "child";
      child: SessionChild;
      /** The listed session it hangs off, however deep. */
      root: SessionSummary;
      /** 1 for the session's own subagents, 2 for theirs, … */
      depth: number;
      expandKey: string;
      isOpen: boolean;
      machine?: MachineKey;
      /** The last of its siblings: its connector ends here (`└`) rather than running on (`├`). */
      last: boolean;
      /**
       * For each level above it, 1 to `depth - 1`: whether that level's line
       * runs on past this row — the ancestor there has a sibling still to come.
       */
      through: boolean[];
    };

export interface SessionListInput {
  /** The whole feed, newest (and unread) first; both scopes arrive here. */
  sessions: readonly SessionSummary[];
  source: "all" | Source;
  query: string;
  /** The `session_grouping` pref, resolved to a boolean. */
  grouped: boolean;
  /** Groups the user folded away, keyed by [`collapseKey`]. */
  collapsedDirs: ReadonlySet<string>;
  /** Sessions and subagents whose subagents are listed, keyed by [`expandKey`]; folded by default. */
  expanded?: ReadonlySet<string>;
  /** Used to shorten a row's own cwd to `~/…` under a repository heading. */
  homeDir: string;
  limit?: number;
}

export interface SessionListModel {
  rows: SessionListRow[];
  /** Rows rendered, and how many matched before the cap. */
  ownedShown: number;
  ownedTotal: number;
}

/**
 * The checkout a repository key names.
 *
 * `repoKey` is `git rev-parse --git-common-dir`, so it points at the shared
 * `.git` directory rather than at the project folder holding it. Every worktree
 * of one repository still agrees on the answer — which is the whole contract of
 * the key — and the heading gets to say `remote-hand` instead of `.git`.
 */
export function repoDir(repoKey: string): string {
  const trimmed = repoKey.replace(/\/+$/, "");
  if (!trimmed.endsWith("/.git")) return trimmed;
  return trimmed.slice(0, -"/.git".length) || "/";
}

/**
 * What a session groups under: its repository if it has one, else its own cwd.
 *
 * The reason the server sends `repoKey` at all — four worktrees of one project
 * are one heading here rather than four, and a session outside any repository
 * keeps grouping the way it always did.
 */
export function groupKeyOf(session: SessionSummary): string {
  if (session.repoKey) return repoDir(session.repoKey);
  return session.cwd ?? NO_PROJECT;
}

/** Case-insensitive substring match over the two fields the filter box covers. */
export function matchesQuery(session: SessionSummary, needle: string): boolean {
  if (!needle) return true;
  return (
    session.title.toLowerCase().includes(needle) || (session.cwd ?? "").toLowerCase().includes(needle)
  );
}

/**
 * What a group's fold is remembered under.
 *
 * The `owned:` prefix is a leftover kept on purpose. Headings used to be
 * namespaced by section, because one repository could have rows in both the
 * owned list and the external one and folding it away in the first said nothing
 * about the second. The external list is gone; the folds saved under these keys
 * are not, and dropping the prefix would forget every one of them.
 */
export function collapseKey(dir: string): string {
  return `owned:${dir}`;
}

/**
 * What a session's subagents' fold is remembered under, and each subagent's
 * below it: the session (`claude/<id>`), then the path down to the subagent.
 * A Claude agent's id is only unique within its session, hence the path rather
 * than the id alone. Another device's has that device in front, as
 * [`deviceCollapseKey`] does for its projects.
 */
export function expandKey(source: Source, id: string, path: readonly string[] = [], machine: MachineKey | null = null): string {
  const own = [`${source}/${id}`, ...path].join("/");
  return machine ? `${machine}|${own}` : own;
}

/** Stable identity for the virtualizer, on a list of `machine`'s rows (a row naming its own device: that one's); headers, sessions and subagents must never collide. */
export function rowKey(row: SessionListRow, machine: MachineKey): string {
  if (row.type === "header") return `h:${row.collapseKey}`;
  if (row.type === "child") return `c:${row.machine ?? machine}|${row.expandKey}`;
  return sessionKey({ machine: row.machine ?? machine, source: row.session.source, id: row.session.id });
}

const NONE: ReadonlySet<string> = new Set();

/**
 * A session's row, then, while it is unfolded, its subagents' — each one's own
 * below it while that one is unfolded too. Nothing about a subagent decides
 * whether its session is listed: the filter, the pins and the cap all see the
 * session alone, and its subagents come along under it.
 */
function pushSession(
  rows: SessionListRow[],
  session: SessionSummary,
  dirLabel: string | null,
  expanded: ReadonlySet<string>,
  machine?: MachineKey,
  /** Another device's (its folds are its own). */
  remote: MachineKey | null = null,
): void {
  const key = expandKey(session.source, session.id, [], remote);
  const isOpen = expanded.has(key);
  rows.push({ type: "session", session, dirLabel, expandKey: key, isOpen, ...(machine ? { machine } : {}) });
  if (isOpen) pushChildren(rows, session, session.children ?? [], 1, key, expanded, machine);
}

function pushChildren(
  rows: SessionListRow[],
  root: SessionSummary,
  children: readonly SessionChild[],
  depth: number,
  parentKey: string,
  expanded: ReadonlySet<string>,
  machine?: MachineKey,
  through: readonly boolean[] = [],
): void {
  children.forEach((child, at) => {
    const key = `${parentKey}/${child.id}`;
    const isOpen = expanded.has(key);
    const last = at === children.length - 1;
    rows.push({ type: "child", child, root, depth, expandKey: key, isOpen, last, through: [...through], ...(machine ? { machine } : {}) });
    if (isOpen) pushChildren(rows, root, child.children, depth + 1, key, expanded, machine, [...through, !last]);
  });
}

/**
 * The list's order (`app/global-feed.tsx` keeps every device's feed in it):
 * moving first, then unread, then the newest. A list of several devices'
 * rows is sorted by it as one list.
 */
export function compareSessions(a: SessionSummary, b: SessionSummary): number {
  return activitySortRank(a) - activitySortRank(b) || Number(b.unread) - Number(a.unread) || b.updatedAt.localeCompare(a.updatedAt);
}

/**
 * The row under a session's title.
 *
 * Flat: the basename, as it always was. Grouped: the session's *own* cwd, since
 * the heading above it may be a repository shared by several worktrees and the
 * basename alone would not say which checkout this is.
 */
function dirLabelFor(session: SessionSummary, grouped: boolean, homeDir: string): string | null {
  if (!session.cwd) return null;
  return grouped ? shortenHome(session.cwd, homeDir) : basename(session.cwd);
}

/** The newest session start in a group: what its heading is ranked by ([`compareGroups`]). */
function newestStart(list: readonly SessionSummary[]): string {
  let newest = "";
  for (const session of list) if (session.createdAt > newest) newest = session.createdAt;
  return newest;
}

/**
 * The order directory headings sit in: the one most recently *started in*
 * first — where work was begun, not what is loudest. A message arriving in a
 * directory's existing sessions moves its rows (`compareSessions` still orders
 * them inside the group) but never the heading, so groups do not trade places
 * while they stream. Ties fall back to `tie` (a directory, or a device and one)
 * so the order does not depend on which group happened to be listed first.
 */
function compareGroups(
  a: readonly SessionSummary[],
  b: readonly SessionSummary[],
  tieA: string,
  tieB: string,
): number {
  const startedA = newestStart(a);
  const startedB = newestStart(b);
  if (startedA !== startedB) return startedA < startedB ? 1 : -1;
  return tieA.localeCompare(tieB);
}

function pushRows(
  rows: SessionListRow[],
  sessions: readonly SessionSummary[],
  input: SessionListInput,
): void {
  const expanded = input.expanded ?? NONE;
  if (!input.grouped) {
    for (const session of sessions) pushSession(rows, session, dirLabelFor(session, false, input.homeDir), expanded);
    return;
  }
  const groups = new Map<string, SessionSummary[]>();
  for (const session of sessions) {
    const key = groupKeyOf(session);
    const bucket = groups.get(key);
    if (bucket) bucket.push(session);
    else groups.set(key, [session]);
  }
  // Headings keep their place as messages stream; the rows inside them do not
  // (`compareGroups`).
  const order = [...groups.entries()].sort(([a, al], [b, bl]) => compareGroups(al, bl, a, b));
  for (const [dir, list] of order) {
    const key = collapseKey(dir);
    const isOpen = !input.collapsedDirs.has(key);
    rows.push({ type: "header", dir, collapseKey: key, list, isOpen });
    if (!isOpen) continue;
    for (const session of list) pushSession(rows, session, dirLabelFor(session, true, input.homeDir), expanded);
  }
}

/**
 * The flattened row model the sidebar virtualizes.
 *
 * Owned rows only, deliberately. Sessions this server merely found on the
 * machine used to sit behind a collapsed `External (n)` divider here, and on a
 * machine that has been used for a while that section was most of the list:
 * rows that looked like every other row and then refused `send`, `rename` and
 * `archive` with `external_session`. Do not restore the section.
 *
 * Filtering is local because the feed already streams every row, both scopes, in
 * one stream: the source tab and the query narrow what is listed, and scope
 * decides whether a row is listed at all.
 */
export function buildSessionList(input: SessionListInput): SessionListModel {
  const limit = input.limit ?? SECTION_LIMIT;
  const needle = input.query.trim().toLowerCase();
  const owned: SessionSummary[] = [];
  for (const session of input.sessions) {
    if (input.source !== "all" && session.source !== input.source) continue;
    if (session.scope === "external") continue;
    owned.push(session);
  }

  const matched = owned.filter((s) => matchesQuery(s, needle));
  // Partition before the cap and before grouping: a pin must not disappear in
  // a folded directory or below 300 newer sessions. Keep existing order within
  // each section, and never bypass source/search/scope filters.
  const ordered = [...matched.filter((s) => s.pinned), ...matched.filter((s) => !s.pinned)];
  const shown = ordered.slice(0, limit);

  const rows: SessionListRow[] = [];
  for (const session of shown.filter((s) => s.pinned)) {
    pushSession(rows, session, dirLabelFor(session, input.grouped, input.homeDir), input.expanded ?? NONE);
  }
  pushRows(rows, shown.filter((s) => !s.pinned), input);

  return { rows, ownedShown: shown.length, ownedTotal: matched.length };
}

/** One device's rows, for a list of several devices' (`buildDeviceList`). */
export interface DeviceRows {
  readonly machine: MachineKey;
  /** Newest (and moving, and unread) first, as the device's feed or copy has them; `null`: none yet. */
  readonly rows: readonly SessionSummary[] | null;
  /** Its home directory, to shorten a row's own cwd under a heading. */
  readonly homeDir?: string;
}

export interface DeviceListInput {
  /** Every device's rows, the page's home first. */
  lists: readonly DeviceRows[];
  /** The home (`local`): its folds are remembered under the keys from before devices (`owned:<dir>`). */
  home: MachineKey;
  /** Only one device's rows; `all`: every device's. */
  device: "all" | MachineKey;
  query: string;
  grouped: boolean;
  collapsedDirs: ReadonlySet<string>;
  /** As in [`SessionListInput`]; another device's keys carry it in front ([`expandKey`]). */
  expanded?: ReadonlySet<string>;
  limit?: number;
}

/**
 * What a project's fold is remembered under in a list of several devices':
 * the home's as it always was ([`collapseKey`]), another device's with that
 * device in front — the same path on two devices is two projects.
 */
export function deviceCollapseKey(machine: MachineKey, home: MachineKey, dir: string): string {
  return machine === home ? collapseKey(dir) : `owned:${machine}|${dir}`;
}

/**
 * The row model of several devices' sessions as one list (the local app with
 * its gateway's devices, S1-7): the same rules as [`buildSessionList`] — owned
 * rows only, the filter, pins first, the 300-row cut — over every device's
 * rows sorted as one list ([`compareSessions`]), or one device's (`device`).
 * Grouped, each device's projects are headings of their own, in the same
 * start-time order as the single-device list ([`compareGroups`]). Every row says
 * its device. One device's list is exactly what [`buildSessionList`] makes
 * of it, folds and all.
 */
export function buildDeviceList(input: DeviceListInput): SessionListModel {
  const limit = input.limit ?? SECTION_LIMIT;
  const needle = input.query.trim().toLowerCase();
  const tagged: { machine: MachineKey; session: SessionSummary }[] = [];
  const homeDirs = new Map<MachineKey, string>();
  for (const list of input.lists) {
    homeDirs.set(list.machine, list.homeDir ?? "");
    if (input.device !== "all" && list.machine !== input.device) continue;
    for (const session of list.rows ?? []) {
      if (session.scope === "external" || !matchesQuery(session, needle)) continue;
      tagged.push({ machine: list.machine, session });
    }
  }
  // Stable: each device's own order (the same rule) holds within the merge.
  if (input.device === "all") tagged.sort((a, b) => compareSessions(a.session, b.session));
  const ordered = [...tagged.filter((t) => t.session.pinned), ...tagged.filter((t) => !t.session.pinned)];
  const shown = ordered.slice(0, limit);
  const rows: SessionListRow[] = [];
  const label = (t: { machine: MachineKey; session: SessionSummary }, grouped: boolean) =>
    dirLabelFor(t.session, grouped, homeDirs.get(t.machine) ?? "");
  const push = (t: { machine: MachineKey; session: SessionSummary }, grouped: boolean) =>
    pushSession(rows, t.session, label(t, grouped), input.expanded ?? NONE, t.machine, t.machine === input.home ? null : t.machine);
  for (const t of shown.filter((t) => t.session.pinned)) push(t, input.grouped);
  const rest = shown.filter((t) => !t.session.pinned);
  if (!input.grouped) {
    for (const t of rest) push(t, false);
  } else {
    const groups = new Map<string, { machine: MachineKey; dir: string; items: typeof rest }>();
    for (const t of rest) {
      const dir = groupKeyOf(t.session);
      const key = `${t.machine}\u0000${dir}`;
      const group = groups.get(key);
      if (group) group.items.push(t);
      else groups.set(key, { machine: t.machine, dir, items: [t] });
    }
    // Sorted by when work was started in each project, like the single-device
    // list: loudness moves rows inside a group, never the headings.
    const order = [...groups.entries()].sort(([keyA, a], [keyB, b]) =>
      compareGroups(a.items.map((t) => t.session), b.items.map((t) => t.session), keyA, keyB),
    );
    for (const [, { machine, dir, items }] of order) {
      const key = deviceCollapseKey(machine, input.home, dir);
      const isOpen = !input.collapsedDirs.has(key);
      rows.push({ type: "header", dir, collapseKey: key, list: items.map((t) => t.session), isOpen, machine });
      if (!isOpen) continue;
      for (const t of items) push(t, true);
    }
  }
  return { rows, ownedShown: shown.length, ownedTotal: ordered.length };
}

/** What a session's subagents are up to, every level of them, for its folded row. */
export interface ChildTally {
  total: number;
  working: number;
  waiting: number;
  failed: number;
  unread: number;
}

export function childTally(children: readonly SessionChild[]): ChildTally {
  const out: ChildTally = { total: 0, working: 0, waiting: 0, failed: 0, unread: 0 };
  const visit = (list: readonly SessionChild[]) => {
    for (const child of list) {
      out.total += 1;
      if (child.status === "working") out.working += 1;
      else if (child.status === "waiting") out.waiting += 1;
      else if (child.status === "failed") out.failed += 1;
      if (child.unread) out.unread += 1;
      visit(child.children);
    }
  };
  visit(children);
  return out;
}
