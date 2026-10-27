/**
 * `@droppen/plugin-sdk` — the whole contract between Droppen and a plugin.
 *
 * A plugin is an npm package whose `package.json` carries a `droppen` manifest
 * and whose entry module default-exports {@link definePlugin}'s result. The
 * host loads that module only when the plugin is enabled on the instance, calls
 * {@link PluginDefinition.activate} once with a {@link PluginHost}, and renders
 * whatever the plugin contributed (navigation, routes, panes, settings) inside a
 * boundary that provides the same host to {@link usePluginHost}.
 *
 * Everything a plugin does to the outside world goes through `host.*`: storage,
 * dialogs, files, printing, the clipboard and navigation. That is what lets one
 * plugin build run unchanged in the desktop app's window and in a browser on
 * the server's own page (droppen.app loads no plugin code). The SDK depends on
 * React and nothing else, and it names nothing that belongs to the hosted
 * service (accounts, plans, relays, keys): a host translates those into the
 * generic errors below.
 *
 * `host.*` is the supported way, not a wall: a plugin runs unsandboxed in the
 * host's page, and installing one trusts it fully
 * (`docs/plugins/runtime.md` §8).
 *
 * Licensed MIT OR Apache-2.0, independently of the application that hosts it.
 */
import { createContext, useContext, type ComponentType, type LazyExoticComponent, type ReactNode } from "react";

export { compareVersions, parseVersion, satisfies, validRange, type Version } from "./semver";

/**
 * The SDK version this package implements. A plugin's `engines.droppen` is a
 * range over it. It versions the host's React too: on 0.x a minor is a
 * breaking change, and a React major takes one (`docs/plugins/runtime.md` §3).
 */
export const SDK_VERSION = "0.1.0";

/** The languages a host may run in. */
export type Locale = "en" | "zh-CN" | "zh-TW" | "ja";

/** One string, or one per language with English as the fallback. */
export type LocalizedText = string | ({ en: string } & Partial<Record<Locale, string>>);

/**
 * What a plugin says it uses. **Declared, not enforced**: the host checks the
 * names at install and nothing at run time holds a plugin to them — it runs
 * with the page's own rights (`docs/plugins/runtime.md` §8). Only `storage`
 * names something the host offers in 0.1; the other two are reserved so a
 * manifest can name them without a breaking change later.
 */
export type Permission = "storage" | "sessions:read" | "notify";

/** Plugin ids: lower-case, start with a letter, 2-32 characters. */
export const PLUGIN_ID = /^[a-z][a-z0-9-]{1,31}$/;

/** Whether `id` is usable as a plugin id (it becomes a URL, an API prefix and a directory name). */
export const isPluginId = (id: string): boolean => PLUGIN_ID.test(id);

/**
 * Document names: 1-200 characters of `A-Z a-z 0-9 . _ -` in `/`-separated
 * segments, no empty, `.` or `..` segment.
 */
export const isDocName = (name: string): boolean =>
  name.length > 0 &&
  name.length <= 200 &&
  name.split("/").every((segment) => /^[A-Za-z0-9._-]+$/.test(segment) && segment !== "." && segment !== "..");

/** The manifest, as it appears under `droppen` in the plugin's `package.json` (plus `version`). */
export interface PluginManifest {
  /** See {@link PLUGIN_ID}. */
  id: string;
  /** The package version. */
  version: string;
  /** Display name. */
  name: LocalizedText;
  /** The SDK range this plugin was built against, e.g. `^0.1.0`. */
  engines: { droppen: string };
  /** What it says it uses: declared, not enforced (see {@link Permission}). */
  permissions?: Permission[];
  /**
   * Old top-level paths that should keep working, e.g. `["/tasks"]`. The host
   * redirects each to the plugin's root, `/p/<id>/`.
   */
  aliases?: string[];
  /** Enabled on an instance that has never decided. Normally `false`. */
  defaultEnabled?: boolean;
}

/** Icons are components; the host passes a size class and nothing else. */
export type Icon = ComponentType<{ className?: string; size?: number | string; "aria-hidden"?: boolean | "true" | "false" }>;

/** A component, or a `React.lazy` one (preferred: it keeps the plugin's code off the critical path). */
export type Lazy<P> = ComponentType<P> | LazyExoticComponent<ComponentType<P>>;

/** An entry in the main navigation, placed after the host's own. */
export interface NavContribution {
  label: LocalizedText;
  icon: Icon;
  /** Relative to the plugin root `/p/<id>/`; empty for the root. */
  path?: string;
}

/** What a routed page is given. */
export interface RouteProps {
  /** The rest of the path below the plugin root, without a leading slash. */
  subpath: string;
  /** The page's query string. */
  search: URLSearchParams;
}

/** A page under `/p/<id>/`. The longest matching `path` prefix wins; `""` matches everything. */
export interface RouteContribution {
  path: string;
  component: Lazy<RouteProps>;
}

/** A pane's own state, saved in the layout. Flat, and at most 2 KB as JSON. */
export type PaneState = Record<string, string | number | boolean | null>;

/** What a pane is given. */
export interface PaneProps {
  state: PaneState;
  setState(next: PaneState): void;
  /** Open one of the plugin's pages in the main view (relative to `/p/<id>/`). */
  openPage(path?: string): void;
}

/** Something a split pane can show. */
export interface PaneContribution {
  /** Stable within the plugin; saved in layouts. */
  id: string;
  label: LocalizedText;
  description?: LocalizedText;
  icon: Icon;
  component: Lazy<PaneProps>;
}

/** A block in Settings, under the host's plugin list. */
export interface SettingsContribution {
  title: LocalizedText;
  component: Lazy<object>;
}

/** Everything a plugin adds to the host. All optional. */
export interface PluginContributions {
  nav?: NavContribution[];
  routes?: RouteContribution[];
  panes?: PaneContribution[];
  settings?: SettingsContribution[];
  /** Strings for {@link PluginHost.i18n}, registered in the plugin's own namespace. */
  i18n?: Partial<Record<Locale, Record<string, string>>>;
}

/** A plugin: its manifest and the function that registers its contributions. */
export interface PluginDefinition {
  manifest: PluginManifest;
  /** Register contributions. Called once per page; must not do I/O. */
  activate(host: PluginHost): PluginContributions;
}

/** Identity, for type inference at the definition site. */
export function definePlugin(definition: PluginDefinition): PluginDefinition {
  return definition;
}

// ---------------------------------------------------------------------------
// Storage

/** A stored document. */
export interface Doc<T> {
  name: string;
  /** Bumped by every write; the first write makes it 1. */
  revision: number;
  /** ISO-8601. */
  updatedAt: string;
  value: T;
}

/** A document's name and revision, without its value. */
export type DocMeta = Omit<Doc<never>, "value">;

/** The failures storage reports. Told apart by `code`, never by `instanceof`. */
export type StorageErrorCode = "conflict" | "too_large" | "disabled" | "offline" | "failed";

/** A storage failure, as thrown by every {@link PluginStorage} method. */
export type StorageError<T = unknown> =
  | {
      code: "conflict";
      message: string;
      /** Which document conflicted (the first one, for a batch). */
      doc: string;
      /** That document as it is now; `null` when it does not exist. */
      current: Doc<T> | null;
    }
  | { code: "too_large"; message: string; limit: number }
  | { code: "disabled" | "offline" | "failed"; message: string };

/**
 * The `Error` hosts throw. A plain object with the same fields is equally
 * valid; check with {@link isStorageError}, which looks at `code` only.
 */
export class PluginStorageError extends Error {
  readonly code: StorageErrorCode;
  readonly current?: Doc<unknown> | null;
  readonly doc?: string;
  readonly limit?: number;
  constructor(error: StorageError) {
    super(error.message);
    this.name = "PluginStorageError";
    this.code = error.code;
    if (error.code === "conflict") {
      this.current = error.current;
      this.doc = error.doc;
    }
    if (error.code === "too_large") this.limit = error.limit;
  }
}

const CODES: ReadonlySet<string> = new Set(["conflict", "too_large", "disabled", "offline", "failed"]);

/** Whether `error` is a storage failure, optionally of one kind. */
export function isStorageError<C extends StorageErrorCode>(
  error: unknown,
  code?: C,
): error is Extract<StorageError, { code: C }> & { message: string } {
  if (typeof error !== "object" || error === null) return false;
  const found = (error as { code?: unknown }).code;
  return typeof found === "string" && CODES.has(found) && (code === undefined || found === code);
}

/** The document a conflict carries, or `null`. */
export function conflictCurrent<T>(error: unknown): Doc<T> | null {
  return isStorageError(error, "conflict") ? ((error as { current?: Doc<T> | null }).current ?? null) : null;
}

/** One write in a {@link PluginStorage.batch}. */
export interface DocWrite<T = unknown> {
  name: string;
  value: T;
  /** The revision the write is based on; `null` means "must not exist yet". */
  expectedRevision: number | null;
}

/**
 * The plugin's private document store: JSON values under names, each with a
 * revision used for compare-and-set. The host decides where it lives (the
 * instance today); the plugin never sees a path or a URL.
 */
export interface PluginStorage {
  /** The document, or `null` when there is none. */
  get<T>(name: string): Promise<Doc<T> | null>;
  /** Every document whose name starts with `prefix` (`""` for all), without values. */
  list(prefix: string): Promise<DocMeta[]>;
  /** Write one document if its revision is still `expectedRevision`. */
  put<T>(name: string, value: T, expectedRevision: number | null): Promise<Doc<T>>;
  /** Write several documents atomically: if any one conflicts, none is written. */
  batch(writes: DocWrite[]): Promise<Doc<unknown>[]>;
  /**
   * Be told when `name` changes anywhere — another tab, pane or device. The
   * argument is the new revision, or `null` when the host lost track (after a
   * reconnect) and the document should simply be re-read. Returns the
   * unsubscribe call.
   */
  watch(name: string, onChange: (revision: number | null) => void): () => void;
}

// ---------------------------------------------------------------------------
// Host services

/** The host's own dialog and feedback components, so a plugin looks like the app it is in. */
export interface HostUi {
  Dialog: ComponentType<{ open: boolean; onOpenChange(open: boolean): void; children: ReactNode }>;
  DialogContent: ComponentType<{ title: string; className?: string; closeLabel?: string; children: ReactNode }>;
  Spinner: ComponentType<{ className?: string }>;
  /** A yes/no question in the host's dialog. Replaces `window.confirm`. */
  confirm(message: string, options?: { title?: string; danger?: boolean; confirmLabel?: string; cancelLabel?: string }): Promise<boolean>;
}

export interface HostI18n {
  language(): Locale;
  /** Look `key` up in the plugin's own strings; falls back to the key. */
  t(key: string, vars?: Record<string, unknown>): string;
  /** Pick the current language's text. */
  text(value: LocalizedText): string;
}

export interface HostNavigation {
  /** Go to a page of this plugin, relative to `/p/<id>/`. */
  navigate(path?: string): void;
  /**
   * A React hook: while `active`, leaving the page (in-app or closing the tab)
   * asks `message` first, in the host's own dialog. Replaces `useBlocker` and
   * `beforeunload`.
   */
  useLeaveGuard(active: boolean, message: string): void;
}

/** What this host can do. A plugin hides what is unavailable rather than sniffing the platform. */
export interface HostCapabilities {
  print: boolean;
  clipboard: boolean;
}

export interface HostFiles {
  /** Offer `data` to the user as a file. Replaces `<a download>`. */
  save(data: Blob, suggestedName: string): Promise<void>;
  /** Ask the user for one file; `null` when they cancel. Replaces `<input type=file>`. */
  open(accept: string[]): Promise<File | null>;
}

/** Everything the host offers a plugin. One per plugin per page. */
export interface PluginHost {
  readonly sdkVersion: string;
  readonly pluginId: string;
  readonly storage: PluginStorage;
  readonly ui: HostUi;
  readonly i18n: HostI18n;
  readonly navigation: HostNavigation;
  readonly capabilities: HostCapabilities;
  readonly files: HostFiles;
  readonly clipboard: { writeText(text: string): Promise<void> };
  /** Print a standalone HTML document (with its own `<style>`), not the page. */
  print(document: { title: string; html: string }): Promise<void>;
}

/** Provided by the host around every plugin component. */
export const PluginHostContext = createContext<PluginHost | null>(null);

/** The provider hosts (and tests) wrap plugin components in. */
export const PluginHostProvider = PluginHostContext.Provider;

/** The host this component was rendered by. */
export function usePluginHost(): PluginHost {
  const host = useContext(PluginHostContext);
  if (!host) throw new Error("usePluginHost() outside a plugin boundary: render plugin components through the host");
  return host;
}

/** Pick a language's text out of a {@link LocalizedText}. */
export function localize(value: LocalizedText, locale: Locale): string {
  if (typeof value === "string") return value;
  return value[locale] ?? (locale === "zh-TW" ? value["zh-CN"] : undefined) ?? value.en;
}
