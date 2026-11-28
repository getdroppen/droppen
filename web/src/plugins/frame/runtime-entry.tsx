/**
 * Entry point bundled into a single self-contained ESM asset
 * (`assets/plugin-frame-runtime-<hash>.js`) by `web/build/plugin-frame.ts` and
 * sent over a `MessageChannel` into `/plugin-frame.html`.
 *
 * Holds the frame's single React instance, `react-dom`, `@droppen/plugin-sdk`,
 * request shims (`fetch` / `XMLHttpRequest`), module loader, and the framed
 * `PluginHost` UI/storage/navigation bridge.
 */
import * as React from "react";
import { Component, Suspense, useEffect, useRef, useState, type ReactNode } from "react";
import * as ReactDOM from "react-dom";
import { createRoot } from "react-dom/client";
import * as ReactJsxRuntime from "react/jsx-runtime";
import * as PluginSdk from "@droppen/plugin-sdk";
import {
  localize,
  PluginHostProvider,
  PluginStorageError,
  SDK_VERSION,
  type Doc,
  type DocMeta,
  type DocWrite,
  type Locale,
  type PaneState,
  type PluginContributions,
  type PluginHost,
  type RouteContribution,
} from "@droppen/plugin-sdk";
import { loadPluginPackage } from "./module-loader";
import {
  PLUGIN_PAGE_PANE_ID,
  type FrameToParentMessage,
  type HttpRequestParams,
  type HttpResponseResult,
  type ParentToFrameMessage,
  type SerializedContributions,
  type SerializedError,
} from "./protocol";
import { installRequestShims } from "./shims";

// ---------------------------------------------------------------------------
// Frame styling & theme sync

function applyTheme(themeMode: "light" | "dark", cssVars: Record<string, string>) {
  const root = document.documentElement;
  root.classList.toggle("dark", themeMode === "dark");
  root.style.colorScheme = themeMode;
  for (const [name, value] of Object.entries(cssVars)) {
    if (name.startsWith("--")) root.style.setProperty(name, value);
  }
}

function injectBaseStyles() {
  const style = document.createElement("style");
  style.textContent = `
    *, *::before, *::after { box-sizing: border-box; }
    html, body, #root {
      margin: 0;
      padding: 0;
      width: 100%;
      height: 100%;
      background: var(--color-bg, #ffffff);
      color: var(--color-fg, #1a1e23);
      font-family: var(--font-sans, system-ui, -apple-system, sans-serif);
      font-size: 14px;
      line-height: 1.5;
    }
    #root { display: flex; flex-direction: column; min-height: 0; min-width: 0; }
    @keyframes droppen-spin { to { transform: rotate(360deg); } }
    .droppen-frame-spinner {
      width: 16px;
      height: 16px;
      border: 2px solid var(--color-border, #e5e5e5);
      border-top-color: var(--color-fg, #1a1e23);
      border-radius: 9999px;
      animation: droppen-spin 0.7s linear infinite;
    }
    .droppen-frame-backdrop {
      position: fixed;
      inset: 0;
      z-index: 50;
      display: flex;
      align-items: center;
      justify-content: center;
      background: rgba(0, 0, 0, 0.35);
      padding: 16px;
    }
    .droppen-frame-dialog {
      width: 100%;
      max-width: 28rem;
      max-height: 85vh;
      overflow: auto;
      background: var(--color-bg, #ffffff);
      color: var(--color-fg, #1a1e23);
      border: 1px solid var(--color-border-strong, #a3a3a3);
      padding: 14px 16px;
    }
    .droppen-frame-dialog-head {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 8px;
      margin-bottom: 10px;
      font-size: 13px;
      font-weight: 600;
    }
    .droppen-frame-dialog-close {
      border: none;
      background: transparent;
      color: var(--color-muted, #666666);
      cursor: pointer;
      font-size: 14px;
      padding: 2px 6px;
    }
    .droppen-frame-dialog-close:hover {
      color: var(--color-fg, #1a1e23);
      background: var(--color-hover, #f0f0f0);
    }
  `;
  document.head.appendChild(style);
}

// ---------------------------------------------------------------------------
// Frame UI components (Dialog, DialogContent, Spinner)

const DialogCloseContext = React.createContext<(() => void) | null>(null);

function FrameDialog({ open, onOpenChange, children }: { open: boolean; onOpenChange(open: boolean): void; children: ReactNode }) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onOpenChange(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onOpenChange]);

  if (!open) return null;
  return (
    <DialogCloseContext.Provider value={() => onOpenChange(false)}>
      <div
        className="droppen-frame-backdrop"
        onClick={(e) => {
          if (e.target === e.currentTarget) onOpenChange(false);
        }}
      >
        {children}
      </div>
    </DialogCloseContext.Provider>
  );
}

function FrameDialogContent({
  title,
  className,
  closeLabel = "Close",
  children,
}: {
  title: string;
  className?: string;
  closeLabel?: string;
  children: ReactNode;
}) {
  const close = React.useContext(DialogCloseContext);
  return (
    <div role="dialog" aria-modal="true" aria-label={title} className={`droppen-frame-dialog ${className ?? ""}`}>
      <div className="droppen-frame-dialog-head">
        <span>{title}</span>
        {close && (
          <button type="button" className="droppen-frame-dialog-close" aria-label={closeLabel} onClick={close}>
            ×
          </button>
        )}
      </div>
      {children}
    </div>
  );
}

function FrameSpinner({ className }: { className?: string }) {
  return (
    <div
      className={className}
      style={{ display: "flex", alignItems: "center", justifyContent: "center", padding: "12px" }}
      role="status"
      aria-label="Loading"
    >
      <div className="droppen-frame-spinner" />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Error boundary inside frame

class FrameErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  render() {
    if (this.state.error) {
      return (
        <div style={{ padding: "16px", fontFamily: "monospace", fontSize: "12px", color: "var(--color-danger, #c62828)" }}>
          {this.state.error.message}
        </div>
      );
    }
    return this.props.children;
  }
}

// ---------------------------------------------------------------------------
// Route matching (same longest-prefix rule as web/src/plugins/routing.ts)

function matchRoute(routes: RouteContribution[], subpath: string): RouteContribution | null {
  const clean = subpath.replace(/^\/+|\/+$/g, "");
  let best: RouteContribution | null = null;
  let bestLen = -1;
  for (const route of routes) {
    const prefix = route.path.replace(/^\/+|\/+$/g, "");
    const matches = prefix === "" || clean === prefix || clean.startsWith(`${prefix}/`);
    if (matches && prefix.length > bestLen) {
      best = route;
      bestLen = prefix.length;
    }
  }
  return best;
}

// ---------------------------------------------------------------------------
// File open inside frame (works without allow-forms)

function openFileInFrame(accept: string[]): Promise<File | null> {
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
    document.body.appendChild(input);
    input.click();
  });
}

// ---------------------------------------------------------------------------
// RPC helper over MessagePort

function reviveError(err: SerializedError): Error {
  if (err.kind === "storage") {
    if (err.code === "conflict") {
      return new PluginStorageError({
        code: "conflict",
        message: err.message,
        doc: err.doc ?? "",
        current: (err.current as Doc<unknown> | null | undefined) ?? null,
      });
    }
    if (err.code === "too_large") {
      return new PluginStorageError({ code: "too_large", message: err.message, limit: err.limit ?? 0 });
    }
    return new PluginStorageError({ code: err.code, message: err.message });
  }
  return new Error(err.message);
}

function interpolate(template: string, vars?: Record<string, unknown>): string {
  if (!vars) return template;
  return template.replace(/\{\{\s*(\w+)\s*\}\}/g, (_, key: string) => (key in vars ? String(vars[key]) : `{{${key}}}`));
}

// ---------------------------------------------------------------------------
// Root frame React component

interface FrameAppProps {
  boot: Extract<ParentToFrameMessage, { type: "boot" }>;
  contributions: PluginContributions;
  localeListeners: Set<() => void>;
  routeListeners: Set<(route: { subpath: string; search: string }) => void>;
  paneStateListeners: Set<(state: PaneState) => void>;
  post: (msg: FrameToParentMessage) => void;
}

function FrameApp({ boot, contributions, localeListeners, routeListeners, paneStateListeners, post }: FrameAppProps) {
  const [, setLangTick] = useState(0);
  const [routeState, setRouteState] = useState(boot.route ?? { subpath: "", search: "" });
  const [paneState, setPaneState] = useState<PaneState>(boot.pane?.state ?? {});

  useEffect(() => {
    const bump = () => setLangTick((n) => n + 1);
    localeListeners.add(bump);
    return () => {
      localeListeners.delete(bump);
    };
  }, [localeListeners]);

  useEffect(() => {
    routeListeners.add(setRouteState);
    return () => {
      routeListeners.delete(setRouteState);
    };
  }, [routeListeners]);

  useEffect(() => {
    paneStateListeners.add(setPaneState);
    return () => {
      paneStateListeners.delete(setPaneState);
    };
  }, [paneStateListeners]);

  if (boot.mode === "route") {
    const matched = matchRoute(contributions.routes ?? [], routeState.subpath);
    if (!matched) {
      return <div style={{ padding: "16px", fontFamily: "monospace", fontSize: "12px" }}>No page at /{routeState.subpath}</div>;
    }
    const Page = matched.component;
    return <Page subpath={routeState.subpath} search={new URLSearchParams(routeState.search)} />;
  }

  if (boot.mode === "pane") {
    const paneId = boot.pane?.id ?? "";
    const matchedPane = (contributions.panes ?? []).find((p) => p.id === paneId);
    if (matchedPane) {
      const Pane = matchedPane.component;
      return (
        <Pane
          state={paneState}
          setState={(next) => {
            setPaneState(next);
            post({ type: "pane:state", state: next });
          }}
          openPage={(path = "") => post({ type: "pane:open-page", path })}
        />
      );
    }
    // Issue #299: render a plugin's page directly inside a Screen pane.
    if (paneId === PLUGIN_PAGE_PANE_ID) {
      const subpath = typeof paneState.subpath === "string" ? paneState.subpath : "";
      const matchedRoute = matchRoute(contributions.routes ?? [], subpath);
      if (matchedRoute) {
        const Page = matchedRoute.component;
        return <Page subpath={subpath} search={new URLSearchParams()} />;
      }
    }
    return <div style={{ padding: "16px", fontFamily: "monospace", fontSize: "12px" }}>Pane {paneId} is not available</div>;
  }

  if (boot.mode === "settings") {
    const block = (contributions.settings ?? [])[boot.settingsIndex ?? 0];
    if (!block) return null;
    const Block = block.component;
    return <Block />;
  }

  return null;
}

// ---------------------------------------------------------------------------
// Main boot entry invoked by plugin-frame.html bootstrap

export async function bootFrame(port: MessagePort, boot: Extract<ParentToFrameMessage, { type: "boot" }>): Promise<void> {
  injectBaseStyles();
  applyTheme(boot.themeMode, boot.cssVars);

  let nextRpcId = 1;
  const pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();
  const docWatchers = new Map<string, Set<(rev: number | null) => void>>();
  let currentLocale: Locale = boot.locale;
  let pluginI18n: Partial<Record<Locale, Record<string, string>>> = {};
  const localeListeners = new Set<() => void>();
  const routeListeners = new Set<(next: { subpath: string; search: string }) => void>();
  const paneStateListeners = new Set<(next: PaneState) => void>();

  const post = (msg: FrameToParentMessage, transfer: Transferable[] = []) => {
    port.postMessage(msg, transfer);
  };

  const rpc = <T,>(
    method: Extract<FrameToParentMessage, { type: "rpc:req" }>["method"],
    params: unknown,
    transfer: Transferable[] = [],
  ): Promise<T> => {
    const id = nextRpcId++;
    return new Promise<T>((resolve, reject) => {
      pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
      post({ type: "rpc:req", id, method, params } as FrameToParentMessage, transfer);
    });
  };

  port.onmessage = (event: MessageEvent<ParentToFrameMessage>) => {
    const msg = event.data;
    if (!msg || typeof msg !== "object") return;
    switch (msg.type) {
      case "rpc:res": {
        const waiter = pending.get(msg.id);
        if (!waiter) return;
        pending.delete(msg.id);
        if (msg.ok) waiter.resolve(msg.result);
        else waiter.reject(reviveError(msg.error));
        break;
      }
      case "update:theme":
        applyTheme(msg.themeMode, msg.cssVars);
        break;
      case "update:locale":
        currentLocale = msg.locale;
        for (const fn of [...localeListeners]) fn();
        break;
      case "update:route":
        for (const fn of [...routeListeners]) fn({ subpath: msg.subpath, search: msg.search });
        break;
      case "update:pane-state":
        for (const fn of [...paneStateListeners]) fn(msg.state);
        break;
      case "doc:changed":
        if (msg.plugin === "*") {
          for (const set of docWatchers.values()) {
            for (const cb of [...set]) cb(null);
          }
        } else if (msg.plugin === boot.pluginId) {
          const set = docWatchers.get(msg.name);
          if (set) {
            for (const cb of [...set]) cb(msg.revision);
          }
        }
        break;
    }
  };
  port.start();

  // 1. Install `fetch` & `XMLHttpRequest` shims BEFORE importing any plugin code.
  installRequestShims((params: HttpRequestParams, signal?: AbortSignal | null) => {
    if (signal?.aborted) {
      return Promise.reject(new DOMException("The operation was aborted.", "AbortError"));
    }
    const id = nextRpcId++;
    const transfer = params.body ? [params.body] : [];
    return new Promise<HttpResponseResult>((resolve, reject) => {
      const onAbort = () => {
        pending.delete(id);
        post({ type: "http:abort", id });
        reject(new DOMException("The operation was aborted.", "AbortError"));
      };
      if (signal) signal.addEventListener("abort", onAbort, { once: true });
      pending.set(id, {
        resolve: (value) => {
          signal?.removeEventListener("abort", onAbort);
          resolve(value as HttpResponseResult);
        },
        reject: (err) => {
          signal?.removeEventListener("abort", onAbort);
          reject(err);
        },
      });
      post({ type: "rpc:req", id, method: "http:request", params }, transfer);
    });
  });

  // 2. Build the framed PluginHost.
  const host: PluginHost = {
    sdkVersion: SDK_VERSION,
    pluginId: boot.pluginId,
    storage: {
      get: <T,>(name: string) => rpc<Doc<T> | null>("storage:get", { name }),
      list: (prefix: string) => rpc<DocMeta[]>("storage:list", { prefix }),
      put: <T,>(name: string, value: T, expectedRevision: number | null) =>
        rpc<Doc<T>>("storage:put", { name, value, expectedRevision }),
      batch: (writes: DocWrite[]) => rpc<Doc<unknown>[]>("storage:batch", { writes }),
      watch: (name, onChange) => {
        let set = docWatchers.get(name);
        if (!set) {
          set = new Set();
          docWatchers.set(name, set);
        }
        set.add(onChange);
        return () => {
          set?.delete(onChange);
          if (set && set.size === 0) docWatchers.delete(name);
        };
      },
    },
    ui: {
      Dialog: FrameDialog,
      DialogContent: FrameDialogContent,
      Spinner: FrameSpinner,
      confirm: (message, options) => rpc<boolean>("ui:confirm", { message, options }),
    },
    i18n: {
      language: () => currentLocale,
      t: (key, vars) => {
        const dict =
          pluginI18n[currentLocale] ??
          (currentLocale === "zh-TW" ? pluginI18n["zh-CN"] : undefined) ??
          pluginI18n.en;
        return interpolate(dict?.[key] ?? key, vars);
      },
      text: (value) => localize(value, currentLocale),
    },
    navigation: {
      navigate: (path = "") => post({ type: "nav:go", path: path.replace(/^\/+/, "") }),
      useLeaveGuard: (active: boolean, message: string) => {
        const prev = useRef<{ active: boolean; message: string }>({ active: false, message: "" });
        useEffect(() => {
          if (prev.current.active !== active || prev.current.message !== message) {
            prev.current = { active, message };
            post({ type: "nav:guard", active, message });
          }
        }, [active, message]);
        useEffect(() => {
          return () => {
            if (prev.current.active) post({ type: "nav:guard", active: false, message: "" });
          };
        }, []);
      },
    },
    capabilities: {
      print: false,
      clipboard: boot.clipboardSupported,
    },
    files: {
      save: async (data: Blob, suggestedName: string) => {
        const bytes = await data.arrayBuffer();
        await rpc<void>("files:save", { bytes, mimeType: data.type || "application/octet-stream", suggestedName }, [bytes]);
      },
      open: openFileInFrame,
    },
    clipboard: {
      writeText: (text: string) => rpc<void>("clipboard:write", { text }),
    },
    print: async () => {
      throw new Error("printing is not available on droppen.app");
    },
  };

  // 3. Load the plugin package via blob URLs + runtime import map.
  const shared: Record<string, Record<string, unknown>> = {
    react: React as unknown as Record<string, unknown>,
    "react/jsx-runtime": ReactJsxRuntime as unknown as Record<string, unknown>,
    "react-dom": ReactDOM as unknown as Record<string, unknown>,
    "@droppen/plugin-sdk": PluginSdk as unknown as Record<string, unknown>,
  };

  try {
    const definition = await loadPluginPackage(boot.packageData, shared);
    const contributions: PluginContributions = definition.activate(host) ?? {};
    pluginI18n = contributions.i18n ?? {};

    if (boot.mode === "activate") {
      const serialized: SerializedContributions = {
        nav: (contributions.nav ?? []).map((n) => ({ label: n.label, ...(n.path !== undefined ? { path: n.path } : {}) })),
        routes: (contributions.routes ?? []).map((r) => ({ path: r.path })),
        panes: (contributions.panes ?? []).map((p) => ({
          id: p.id,
          label: p.label,
          ...(p.description !== undefined ? { description: p.description } : {}),
        })),
        settings: (contributions.settings ?? []).map((s) => ({ title: s.title })),
        ...(contributions.i18n ? { i18n: contributions.i18n } : {}),
      };
      post({ type: "activated", contributions: serialized });
      return;
    }

    const rootEl = document.getElementById("root") ?? document.body.appendChild(document.createElement("div"));
    rootEl.id = "root";

    createRoot(rootEl).render(
      <PluginHostProvider value={host}>
        <FrameErrorBoundary>
          <Suspense fallback={<FrameSpinner />}>
            <FrameApp
              boot={boot}
              contributions={contributions}
              localeListeners={localeListeners}
              routeListeners={routeListeners}
              paneStateListeners={paneStateListeners}
              post={post}
            />
          </Suspense>
        </FrameErrorBoundary>
      </PluginHostProvider>,
    );
    post({ type: "mounted" });
  } catch (error) {
    post({ type: "error", message: error instanceof Error ? error.message : String(error) });
  }
}
