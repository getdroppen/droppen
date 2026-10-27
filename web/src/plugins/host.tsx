/**
 * The browser's `PluginHost`: what a plugin gets in the desktop app's window
 * and in a browser on the server's own page (the desktop shell will swap the
 * file, print and clipboard parts). One per plugin per page, made by the
 * registry; a page a hub serves has no plugin host at all
 * (`docs/plugins/runtime.md` §5).
 *
 * Storage speaks `docs/plugins/storage-protocol.md` through the typed API
 * client of the machine the plugin's documents live on — the home machine
 * (`lib/machines/registry.ts`) — so it travels whichever way that machine's
 * API does. Hosted-service failures are translated to the SDK's generic codes
 * here — a plugin never sees a relay or a plan.
 */
import {
  localize,
  PluginStorageError,
  SDK_VERSION,
  type Doc,
  type DocMeta,
  type DocWrite,
  type Locale,
  type PluginHost,
  type PluginStorage,
  type StorageError,
} from "@droppen/plugin-sdk";
import { useBlocker } from "@tanstack/react-router";
import i18n from "i18next";
import type { MachineKey } from "@/lib/machines/key";
import { apiFor } from "@/lib/machines/registry";
import { matchLanguage } from "@/i18n/match";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Spinner } from "@/components/ui/spinner";
import { requestConfirm } from "./confirm";
import { pluginEvents } from "./events";

/** The i18next namespace a plugin's strings live in. */
export const pluginNamespace = (id: string) => `plugin:${id}`;

/** The page's language as the SDK names it. */
export function hostLocale(language = i18n.language): Locale {
  return (language && matchLanguage(language)) || "en";
}

interface Failure {
  response?: Response;
  error?: unknown;
}

/** An HTTP answer that is not a success, as the SDK's error. */
export function storageFailure(id: string, { response, error }: Failure): StorageError {
  const body = (error && typeof error === "object" ? error : {}) as Record<string, unknown>;
  const code = typeof body.error === "string" ? body.error : "";
  const message = typeof body.message === "string" ? body.message : code || `HTTP ${response?.status ?? "?"}`;
  const status = response?.status ?? 0;
  if (status === 409 && code === "conflict") {
    return {
      code: "conflict",
      message,
      doc: typeof body.doc === "string" ? body.doc : "",
      current: (body.current as Doc<unknown> | null | undefined) ?? null,
    };
  }
  if (status === 413) return { code: "too_large", message, limit: typeof body.limit === "number" ? body.limit : 0 };
  if (status === 404 && (code === "plugin_disabled" || code === "plugin_not_found"))
    return { code: "disabled", message: `the ${id} plugin is not enabled here` };
  // The bridge to a machine that is not there, or a gateway in between.
  if (status === 503 || status === 502 || status === 504 || code === "machine_offline") return { code: "offline", message };
  return { code: "failed", message };
}

/** Run one request, turning a thrown network error into `offline`. */
async function send<T>(id: string, call: () => Promise<{ data?: T; error?: unknown; response: Response }>): Promise<{ data?: T; response: Response }> {
  let result: { data?: T; error?: unknown; response: Response };
  try {
    result = await call();
  } catch (error) {
    throw new PluginStorageError({ code: "offline", message: error instanceof Error ? error.message : String(error) });
  }
  if (result.response.ok) return result;
  if (result.response.status === 404 && (result.error as { error?: string } | undefined)?.error === "doc_not_found") return result;
  throw new PluginStorageError(storageFailure(id, result));
}

/** `host.storage` over the HTTP protocol, on `machine`. */
export function httpStorage(id: string, machine: MachineKey): PluginStorage {
  const api = apiFor(machine);
  return {
    async get<T>(name: string) {
      const { data, response } = await send(id, () =>
        api.GET("/api/plugins/{id}/docs/{name}", { params: { path: { id, name } } }),
      );
      if (response.status === 404) return null;
      return (data as Doc<T> | undefined) ?? null;
    },
    async list(prefix: string) {
      const { data } = await send(id, () => api.GET("/api/plugins/{id}/docs", { params: { path: { id }, query: { prefix } } }));
      return (data?.docs ?? []) as DocMeta[];
    },
    async put<T>(name: string, value: T, expectedRevision: number | null) {
      const { data } = await send(id, () =>
        api.PUT("/api/plugins/{id}/docs/{name}", {
          params: { path: { id, name } },
          body: { expectedRevision, value },
        }),
      );
      return data as unknown as Doc<T>;
    },
    async batch(writes: DocWrite[]) {
      const { data } = await send(id, () => api.POST("/api/plugins/{id}/batch", { params: { path: { id } }, body: { writes } }));
      return (data?.docs ?? []) as Doc<unknown>[];
    },
    watch(name, onChange) {
      return pluginEvents.onDoc((event) => {
        if (event.plugin === "*" || (event.plugin === id && event.name === name)) onChange(event.revision);
      });
    },
  };
}

/** `<a download>`, the browser's way to hand over a file. */
export async function saveFile(data: Blob, suggestedName: string): Promise<void> {
  const url = URL.createObjectURL(data);
  const a = document.createElement("a");
  a.href = url;
  a.download = suggestedName;
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}

/** `<input type=file>`; resolves `null` when the picker is dismissed. */
export function openFile(accept: string[]): Promise<File | null> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = accept.join(",");
    input.style.display = "none";
    let settled = false;
    const finish = (file: File | null) => {
      if (settled) return;
      settled = true;
      input.remove();
      resolve(file);
    };
    input.addEventListener("change", () => finish(input.files?.[0] ?? null));
    input.addEventListener("cancel", () => finish(null));
    document.body.append(input);
    input.click();
  });
}

const escapeHtml = (text: string) => text.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);

/**
 * Print a standalone document rather than the page: a hidden frame holding
 * only what the plugin handed over, so the app around it never reaches paper.
 */
export function printDocument({ title, html }: { title: string; html: string }): Promise<void> {
  return new Promise((resolve, reject) => {
    const frame = document.createElement("iframe");
    // No scripts in the printed document, ever: the plugin hands over markup.
    // Same origin so this page can fill it in; modals so it may print.
    frame.setAttribute("sandbox", "allow-same-origin allow-modals");
    frame.setAttribute("aria-hidden", "true");
    frame.tabIndex = -1;
    Object.assign(frame.style, { position: "fixed", right: "0", bottom: "0", width: "0", height: "0", border: "0", visibility: "hidden" });
    document.body.append(frame);
    const win = frame.contentWindow;
    const doc = frame.contentDocument;
    if (!win || !doc) {
      frame.remove();
      reject(new Error("printing is not available here"));
      return;
    }
    doc.open();
    doc.write(`<!doctype html><html><head><meta charset="utf-8"><title>${escapeHtml(title)}</title></head><body>${html}</body></html>`);
    doc.close();
    const done = () => {
      setTimeout(() => frame.remove(), 1000);
      resolve();
    };
    win.addEventListener("afterprint", done, { once: true });
    // Let layout settle before the dialog freezes it.
    setTimeout(() => {
      try {
        win.focus();
        win.print();
      } catch (error) {
        frame.remove();
        reject(error instanceof Error ? error : new Error(String(error)));
        return;
      }
      // Some browsers return from print() only after the dialog; some never fire afterprint.
      setTimeout(done, 60_000);
    }, 50);
  });
}

export interface HostDeps {
  /** Go to an app path (already prefixed with `/p/<id>/`). */
  navigate(to: string): void;
  /** Where its documents live: the home machine. */
  machine: MachineKey;
  storage?: PluginStorage;
}

/** Whether this page is the desktop app's own window (its init script defines `window.__droppenDesktop`). */
function desktopWindow(): boolean {
  if (typeof window === "undefined") return false;
  return (window as unknown as { __droppenDesktop?: unknown }).__droppenDesktop !== undefined;
}

/** Build the host one plugin gets for the life of the page. */
export function createPluginHost(id: string, deps: HostDeps): PluginHost {
  const ns = pluginNamespace(id);
  const root = `/p/${id}/`;
  const confirm = requestConfirm;
  return {
    sdkVersion: SDK_VERSION,
    pluginId: id,
    storage: deps.storage ?? httpStorage(id, deps.machine),
    ui: {
      Dialog: ({ open, onOpenChange, children }) => (
        <Dialog open={open} onOpenChange={(next) => onOpenChange(next)}>
          {children}
        </Dialog>
      ),
      DialogContent,
      Spinner,
      confirm,
    },
    i18n: {
      language: () => hostLocale(),
      t: (key, vars) => i18n.t(key, { ...vars, ns, defaultValue: key }) as string,
      text: (value) => localize(value, hostLocale()),
    },
    navigation: {
      navigate(path = "") {
        deps.navigate(root + path.replace(/^\/+/, ""));
      },
      useLeaveGuard(active: boolean, message: string) {
        useBlocker({
          shouldBlockFn: async () => !(await confirm(message)),
          enableBeforeUnload: active,
          disabled: !active,
        });
      },
    },
    capabilities: {
      // wry 0.57's WKWebView UI delegate implements no print callback, so
      // window.print() silently does nothing in the app's window. A browser
      // on the same server prints — `isDesktopApp()` is true there too, which
      // is why the app's own marker decides instead.
      print: typeof window !== "undefined" && typeof window.print === "function" && !desktopWindow(),
      clipboard: typeof navigator !== "undefined" && !!navigator.clipboard?.writeText && window.isSecureContext,
    },
    files: { save: saveFile, open: openFile },
    clipboard: {
      async writeText(text) {
        await navigator.clipboard.writeText(text);
      },
    },
    print: printDocument,
  };
}
