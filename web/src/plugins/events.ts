/**
 * Plugin news from each machine's global feed, for the host's registry and
 * storage (`docs/plugins/runtime.md` §6, `docs/plugins/cloud.md` §6).
 *
 * The feed (`app/global-feed.tsx`) hands every `plugin-doc` and `plugins` frame
 * here, and says when it has reconnected (a `sessions:snapshot`), after which
 * anything a plugin is watching may have changed unseen and is re-read.
 * Scoped per machine (`pluginEventsFor`) so multi-machine views never cross-talk
 * or leak listeners when a machine closes; `pluginEvents` aliases `LOCAL`.
 */
import { LOCAL, type MachineKey } from "@/lib/machines/key";
import { perMachine } from "@/lib/machines/registry";
import type { PluginStatus } from "./types";

export interface PluginDocEvent {
  plugin: string;
  name: string;
  /** `null` after a reconnect: re-read, the revision is unknown. */
  revision: number | null;
}

type DocListener = (event: PluginDocEvent) => void;
type StatusListener = (statuses: PluginStatus[]) => void;

export interface PluginEventBus {
  onDoc(listener: DocListener): () => void;
  onStatuses(listener: StatusListener): () => void;
  /** A frame off the feed; `true` when it was a plugin frame. */
  takeFrame(frame: { type?: unknown; [key: string]: unknown }): boolean;
  /** The feed reconnected: every watched document may have moved. */
  resync(): void;
  /** Drop all listeners when the owning machine closes. */
  clear(): void;
}

function createPluginEventBus(): PluginEventBus {
  const docListeners = new Set<DocListener>();
  const statusListeners = new Set<StatusListener>();
  return {
    onDoc(listener: DocListener): () => void {
      docListeners.add(listener);
      return () => docListeners.delete(listener);
    },
    onStatuses(listener: StatusListener): () => void {
      statusListeners.add(listener);
      return () => statusListeners.delete(listener);
    },
    takeFrame(frame: { type?: unknown; [key: string]: unknown }): boolean {
      if (frame.type === "plugin-doc") {
        const { plugin, name, revision } = frame;
        if (typeof plugin !== "string" || typeof name !== "string") return true;
        const event = { plugin, name, revision: typeof revision === "number" ? revision : null };
        for (const listener of [...docListeners]) listener(event);
        return true;
      }
      if (frame.type === "plugins") {
        if (Array.isArray(frame.plugins)) {
          for (const listener of [...statusListeners]) listener(frame.plugins as PluginStatus[]);
        }
        return true;
      }
      return false;
    },
    resync(): void {
      for (const listener of [...docListeners]) listener({ plugin: "*", name: "*", revision: null });
    },
    clear(): void {
      docListeners.clear();
      statusListeners.clear();
    },
  };
}

/** Per-machine plugin event bus, disposed automatically when the machine closes. */
export const pluginEventsFor: (machine: MachineKey) => PluginEventBus = perMachine(
  (_machine: MachineKey) => createPluginEventBus(),
  (bus) => bus.clear(),
);

/** Alias for the local machine's plugin event bus. */
export const pluginEvents: PluginEventBus = pluginEventsFor(LOCAL);
