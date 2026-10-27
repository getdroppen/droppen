/**
 * Plugin panes in layouts.
 *
 * A pane that shows plugin content is `{kind: "plugin", plugin, pane, state?}`.
 * Layouts saved before plugins existed hold `{kind: "tasks"}`; that is read as
 * the tasks plugin's checklist pane, forever. The checklist is also *saved* in
 * the old form, so a layout saved now still means something to a previous
 * binary (whose sanitizer turns unknown kinds into an empty picker).
 */
import type { PaneState } from "@droppen/plugin-sdk";

export interface PluginPaneContent {
  kind: "plugin";
  plugin: string;
  pane: string;
  state?: PaneState;
}

/** Old layout kinds and the plugin pane each one now is. */
const LEGACY: Record<string, { plugin: string; pane: string }> = {
  tasks: { plugin: "tasks", pane: "checklist" },
};

/** Content as the pane renders it: a legacy kind becomes its plugin pane. */
export function readPaneContent<C extends { kind: string }>(content: C): C | PluginPaneContent {
  const legacy = LEGACY[content.kind];
  return legacy ? { kind: "plugin", ...legacy } : content;
}

/** Content as the layout stores it: the legacy form while a rollback may still need it. */
export function storePaneContent(content: PluginPaneContent): PluginPaneContent | { kind: string } {
  const empty = !content.state || Object.keys(content.state).length === 0;
  for (const [kind, legacy] of Object.entries(LEGACY)) {
    if (empty && legacy.plugin === content.plugin && legacy.pane === content.pane) return { kind };
  }
  return content;
}

/** A pane state the server will keep: flat, primitive values, ≤ 2 KB as JSON. */
export function validPaneState(state: unknown): state is PaneState {
  if (!state || typeof state !== "object" || Array.isArray(state)) return false;
  const flat = Object.values(state).every((v) => v === null || ["string", "number", "boolean"].includes(typeof v));
  return flat && JSON.stringify(state).length <= 2048;
}
