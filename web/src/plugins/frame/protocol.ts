/**
 * MessageChannel protocol between the parent page (`droppen.app`) and a
 * sandboxed plugin iframe (`/plugin-frame.html`, `docs/plugins/cloud.md` §2–§5).
 *
 * The frame runs in an opaque origin with `connect-src 'none'` and holds no
 * keys, session or machine handles. Every network request, storage call,
 * confirmation dialog, clipboard write and navigation crosses this single port,
 * and the parent binds every call to the `(machine, pluginId)` it opened the
 * frame for.
 */
import type { Locale, LocalizedText, PaneState } from "@droppen/plugin-sdk";

export type FrameMode = "activate" | "route" | "pane" | "settings";

export interface FramePackageFile {
  path: string;
  contentType: string;
  encoding: "utf8" | "base64";
  data: string;
}

export interface FramePackageData {
  id: string;
  build: string;
  entry: string;
  files: FramePackageFile[];
}

export interface SerializedNav {
  label: LocalizedText;
  path?: string;
}

export interface SerializedRoute {
  path: string;
}

export interface SerializedPane {
  id: string;
  label: LocalizedText;
  description?: LocalizedText;
}

export interface SerializedSettings {
  title: LocalizedText;
}

export interface SerializedContributions {
  nav: SerializedNav[];
  routes: SerializedRoute[];
  panes: SerializedPane[];
  settings: SerializedSettings[];
  i18n?: Partial<Record<Locale, Record<string, string>>>;
}

export interface SerializedStorageError {
  kind: "storage";
  code: "conflict" | "too_large" | "disabled" | "offline" | "failed";
  message: string;
  doc?: string;
  current?: unknown;
  limit?: number;
}

export interface SerializedGenericError {
  kind: "error";
  message: string;
}

export type SerializedError = SerializedStorageError | SerializedGenericError;

export type ParentToFrameMessage =
  | {
      type: "boot";
      pluginId: string;
      mode: FrameMode;
      packageData: FramePackageData;
      locale: Locale;
      themeMode: "light" | "dark";
      cssVars: Record<string, string>;
      clipboardSupported: boolean;
      route?: { subpath: string; search: string };
      pane?: { id: string; state: PaneState };
      settingsIndex?: number;
    }
  | { type: "update:theme"; themeMode: "light" | "dark"; cssVars: Record<string, string> }
  | { type: "update:locale"; locale: Locale }
  | { type: "update:route"; subpath: string; search: string }
  | { type: "update:pane-state"; state: PaneState }
  | { type: "doc:changed"; plugin: string; name: string; revision: number | null }
  | { type: "rpc:res"; id: number; ok: true; result: unknown }
  | { type: "rpc:res"; id: number; ok: false; error: SerializedError };

export interface HttpRequestParams {
  url: string;
  method: string;
  headers: [string, string][];
  body: ArrayBuffer | null;
}

export interface HttpResponseResult {
  status: number;
  statusText: string;
  url: string;
  headers: [string, string][];
  body: ArrayBuffer | null;
}

export type FrameToParentMessage =
  | { type: "activated"; contributions: SerializedContributions }
  | { type: "mounted" }
  | { type: "error"; message: string }
  | { type: "rpc:req"; id: number; method: "http:request"; params: HttpRequestParams }
  | { type: "rpc:req"; id: number; method: "storage:get"; params: { name: string } }
  | { type: "rpc:req"; id: number; method: "storage:list"; params: { prefix: string } }
  | { type: "rpc:req"; id: number; method: "storage:put"; params: { name: string; value: unknown; expectedRevision: number | null } }
  | { type: "rpc:req"; id: number; method: "storage:batch"; params: { writes: { name: string; value: unknown; expectedRevision: number | null }[] } }
  | {
      type: "rpc:req";
      id: number;
      method: "ui:confirm";
      params: { message: string; options?: { title?: string; danger?: boolean; confirmLabel?: string; cancelLabel?: string } };
    }
  | { type: "rpc:req"; id: number; method: "files:save"; params: { bytes: ArrayBuffer; mimeType: string; suggestedName: string } }
  | { type: "rpc:req"; id: number; method: "clipboard:write"; params: { text: string } }
  | { type: "http:abort"; id: number }
  | { type: "nav:go"; path: string }
  | { type: "nav:guard"; active: boolean; message: string }
  | { type: "pane:state"; state: PaneState }
  | { type: "pane:open-page"; path: string };

/**
 * Reserved pane id used when a plugin's page (`RouteContribution`) is placed
 * directly inside a Screen pane (Issue #299). Matches `^[a-z][a-z0-9-]{0,31}$`
 * so even older servers preserve it unchanged in saved layouts.
 */
export const PLUGIN_PAGE_PANE_ID = "droppen-page";
