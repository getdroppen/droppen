/**
 * What the page does with the instance's plugin list, as plain functions:
 * which statuses it runs and whether the list moved under it
 * (`docs/plugins/runtime.md` §6).
 *
 * The page starts from the list it first sees. A plugin that was installed,
 * upgraded or uninstalled after that (the CLI, another tab) is not adopted:
 * a module cannot be unloaded, so only a reload settles it. The statuses the
 * page keeps running are therefore the first list's, at the builds it
 * actually loaded — while the switch stays live: turning a known plugin on
 * or off needs no reload.
 *
 * On a page a hub serves none of this happens: the registry is not mounted,
 * and the helpers answer nothing for `cloud` even if they were asked.
 */
import type { PluginStatus } from "./types";

/** What the page first saw: each installed plugin and the build it started at. */
export type PluginBaseline = Map<string, { build: string; status: PluginStatus }>;

/** The baseline for the first list the page sees. */
export function baselineOf(statuses: readonly PluginStatus[]): PluginBaseline {
  return new Map(statuses.map((status) => [status.id, { build: status.build, status }]));
}

/**
 * The statuses this page runs, in list order: on and compatible, with code to
 * fetch, at the build the page started from. A plugin's status carries the
 * build the page should run; after an upgrade elsewhere that is the baseline's
 * old status (the module already loaded), not the new one.
 */
export function runningStatuses(
  statuses: PluginStatus[] | null,
  baseline: PluginBaseline | null,
  cloud = false,
): PluginStatus[] {
  if (cloud || !statuses) return [];
  const running: PluginStatus[] = [];
  for (const status of statuses) {
    if (!status.enabled || !status.compatible) continue;
    const started = baseline?.get(status.id);
    // Installed after the page knew its list: it waits for the reload.
    if (baseline && !started) continue;
    const code = started && started.build !== status.build ? started.status : status;
    if (code.entry) running.push(code);
  }
  return running;
}

/**
 * The plugins the user has switched on that this host cannot run — typically
 * left behind by a host update (`docs/plugins/runtime.md` §3). They are not
 * loaded, but the page names them (a rail entry, their page, their pane)
 * rather than letting them vanish; switching one off puts it away (§6). From
 * the live list, not the baseline: the switch needs no reload.
 */
export function strandedStatuses(statuses: readonly PluginStatus[] | null, cloud = false): PluginStatus[] {
  if (cloud || !statuses) return [];
  return statuses.filter((status) => status.switchedOn && !status.compatible);
}

/**
 * Whether the list differs from the page's baseline in a way only a reload
 * can apply: a plugin appeared, disappeared, or its build changed. A switch
 * is not: it leaves every build and every id as it was.
 */
export function changedUnderPage(baseline: PluginBaseline, statuses: readonly PluginStatus[]): boolean {
  const present = new Set(statuses.map((status) => status.id));
  for (const [id, started] of baseline) {
    if (!present.has(id)) return true;
    const now = statuses.find((status) => status.id === id);
    if (now && now.build !== started.build) return true;
  }
  return statuses.some((status) => !baseline.has(status.id));
}
