/*
 * Which machine — which device that runs AI — something belongs to.
 *
 * Everything the app asks for lives on a machine: a session, a terminal, the
 * prefs, a file. Until now the page assumed exactly one, "the server", and
 * nothing said which. Here every address says it (`docs/local-aggregator.md`
 * §6.1), so that one page can hold several machines at once — droppen.app
 * running every device side by side, the local app listing another device's
 * sessions — without two of them ever meeting under one key.
 *
 * - In the local shell the page's own server is {@link LOCAL}: the install that
 *   served the page, whatever its device id is on the account.
 * - On droppen.app there is no `local`; a machine is its device id.
 */

/** A machine the page can reach: {@link LOCAL}, or a device id. */
export type MachineKey = string;

/** The page's own server, in the local shell. Never a device id: those are minted by the hub. */
export const LOCAL: MachineKey = "local";

/** One session, wherever it lives. */
export interface SessionRef {
  readonly machine: MachineKey;
  readonly source: string;
  readonly id: string;
}

export function sessionRef(machine: MachineKey, source: string, id: string): SessionRef {
  return { machine, source, id };
}

/**
 * A session's key wherever sessions from more than one machine can meet: maps,
 * sets, React keys, the reading registry. `local|claude/abc`.
 *
 * The machine goes first and the rest is the `source/id` the server itself
 * files a row under (a `sessions:delta`'s `removed`, the prefs' per-session
 * map), which never holds a `|`; {@link parseSessionKey} splits at the last one,
 * so a device id may.
 */
export function sessionKey(ref: SessionRef): string {
  return `${ref.machine}|${ref.source}/${ref.id}`;
}

/** The ref a {@link sessionKey} was made from, or `null` for anything else. */
export function parseSessionKey(key: string): SessionRef | null {
  const bar = key.lastIndexOf("|");
  if (bar <= 0) return null;
  const rest = key.slice(bar + 1);
  const slash = rest.indexOf("/");
  if (slash <= 0 || slash === rest.length - 1) return null;
  return { machine: key.slice(0, bar), source: rest.slice(0, slash), id: rest.slice(slash + 1) };
}
