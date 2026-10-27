/**
 * Old top-level plugin paths (`/tasks`), now learnt from the instance's
 * statuses rather than from a build-time list (`docs/plugins/runtime.md` §6).
 *
 * The shell's own top-level routes are not aliases: the server refuses an
 * install whose `droppen.aliases` names one (`server-rs/crates/droppen-plugin-sdk`'s
 * reserved paths, which `RESERVED` used to mirror on the web side). So a
 * match here is always a plugin's page under `/p/<id>/`.
 *
 * A page a hub serves has no aliases, whatever list it might be given.
 */
import type { PluginStatus } from "./types";

/**
 * The plugin whose old path `pathname` is, if any. Aliases are one lower-case
 * segment (`/tasks`), so a longer path is never one; the trailing slash of a
 * pasted address does not matter.
 */
export function pluginAlias(statuses: PluginStatus[] | null, pathname: string, cloud = false): string | undefined {
  if (cloud || !statuses) return undefined;
  const path = pathname.replace(/\/+$/, "") || "/";
  if (!path.startsWith("/") || path.indexOf("/", 1) !== -1) return undefined;
  return statuses.find((status) => status.aliases?.includes(path))?.id;
}
