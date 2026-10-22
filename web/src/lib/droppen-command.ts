/*
 * The command a person types on a device to run its Droppen server, for the
 * hints that ask them to (`<command> user add <name>`).
 *
 * The server says where it is: `/api/auth/status`'s `droppenPath` is the
 * `droppen` link beside its binary when there is one (the installer's
 * `~/.local/share/remote-hand/bin/droppen`), else the binary itself, with the
 * home directory as `~`. A server older than that field says nothing, and
 * answers to its old name only. Its `droppenEnv` is what the command needs
 * before it to act on that server's instance — `APP_INSTANCE=bob` for a second
 * instance, the data and configuration directories of a second install — and
 * is empty for the usual one: without it, `setup-code` on such a device would
 * show another server's code.
 */

/** What a server that reports no command answers to: its old name. */
export const LEGACY_COMMAND = "remote-hand-server";

/** Characters a POSIX shell takes literally in an unquoted word. */
const PLAIN = /^[A-Za-z0-9_@%+=:,./~-]+$/;

/**
 * `path` as one shell word: as it is when nothing in it is special, else in
 * single quotes — after a leading `~/`, which only expands unquoted.
 */
export function shellWord(path: string): string {
  if (PLAIN.test(path)) return path;
  const quote = (text: string) => `'${text.replace(/'/g, `'\\''`)}'`;
  return path.startsWith("~/") ? `~/${quote(path.slice(2))}` : quote(path);
}

/** What a server says its commands need in their environment. */
export interface CommandEnv {
  name: string;
  value: string;
}

/**
 * The variables a command may be shown with: which instance, and where its
 * files are — today's names, and the `DROPPEN_` ones the rename moves the
 * directories to (`APP_INSTANCE` is the
 * framework's and keeps its name). Anything else a server sends is dropped:
 * a page that shows another device's commands must not let that device put
 * `DYLD_INSERT_LIBRARIES` or `BASH_ENV` in front of one a person copies.
 */
const COMMAND_ENV = new Set(["APP_INSTANCE", "REMOTE_HAND_DATA_DIR", "REMOTE_HAND_CONFIG_DIR", "DROPPEN_DATA_DIR", "DROPPEN_CONFIG_DIR"]);

/** The command to show for a server, from its `/api/auth/status` answer. */
export function droppenCommand(
  status: { droppenPath?: string | null; droppenEnv?: readonly CommandEnv[] | null } | null | undefined,
): string {
  const path = status?.droppenPath?.trim();
  const command = path ? shellWord(path) : LEGACY_COMMAND;
  const env = (status?.droppenEnv ?? []).filter((pair) => COMMAND_ENV.has(pair.name));
  return [...env.map((pair) => `${pair.name}=${shellWord(pair.value)}`), command].join(" ");
}
