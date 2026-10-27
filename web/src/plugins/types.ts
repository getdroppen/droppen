import type { LocalizedText } from "@droppen/plugin-sdk";

/**
 * A plugin as this instance sees it (`GET /api/plugins`, the `settings` frame).
 * See `docs/plugins/storage-protocol.md`.
 */
export interface PluginStatus {
  id: string;
  version: string;
  name: LocalizedText;
  /** On for this instance, and compatible. */
  enabled: boolean;
  compatible: boolean;
  /** The configuration pins it; the switch is read-only. */
  forced: boolean;
  reason: string | null;
  /** Every notice's text, joined; `notices` has them one by one. */
  notice: string | null;
  /** Oldest first. Absent from servers before notices were listed. */
  notices?: PluginNotice[];
}

/** One thing the host wants the user to see about a plugin. */
export interface PluginNotice {
  id: string;
  /** `storeReset`. */
  kind: string;
  text: string;
  at: string;
  /** A store reset is not: it goes when the user clears its file. */
  dismissible: boolean;
}
