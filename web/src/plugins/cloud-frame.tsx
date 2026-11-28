/**
 * Sandboxed iframe host for cloud-enabled plugins on `droppen.app`
 * (`docs/plugins/cloud.md` §2–§5).
 *
 * Each mounted view runs inside `<iframe sandbox="allow-scripts" src="/plugin-frame.html">`
 * (opaque origin, `connect-src 'none'`, no keys or tokens). All plugin code and
 * data arrive through the parent's `MessageChannel`, and every action is
 * forwarded through `transportFor(machine)` with `X-Rh-Plugin: <pluginId>`.
 */
import { localize, type PaneState } from "@droppen/plugin-sdk";
import { PuzzlePieceIcon } from "@phosphor-icons/react";
import { useBlocker } from "@tanstack/react-router";
import i18n from "i18next";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useMachineNavigate } from "@/app/machine-route";
import { Spinner } from "@/components/ui/spinner";
import { useInfoOf } from "@/lib/machines/context";
import type { MachineKey } from "@/lib/machines/key";
import { perMachine } from "@/lib/machines/registry";
import {
  fetchPluginPackage,
  forwardFrameHttp,
  handleFrameStorage,
  readHostTheme,
  serializeError,
} from "./cloud-bridge";
import { requestConfirm } from "./confirm";
import { pluginEventsFor } from "./events";
import type {
  FrameMode,
  FrameToParentMessage,
  ParentToFrameMessage,
  SerializedContributions,
} from "./frame/protocol";
import { fetchFrameRuntimeCode } from "./frame-runtime-source";
import { hostLocale, saveFile } from "./host";
import { validPaneState } from "./pane-content";
import type { PluginStatus } from "./types";

export type CloudActivation =
  | { ok: true; contributions: SerializedContributions }
  | { ok: false; error: string };

const ACTIVATION_TIMEOUT_MS = 15_000;

const cloudActivationsOf = perMachine(() => new Map<string, Promise<CloudActivation>>());

/**
 * Activate a cloud-enabled plugin once per `(machine, id, build)` inside a
 * hidden sandboxed iframe to discover its serializable contributions
 * (nav items, routes, panes, settings titles, i18n).
 */
export function activateCloudPlugin(
  machine: MachineKey,
  status: Pick<PluginStatus, "id" | "build">,
): Promise<CloudActivation> {
  const cache = cloudActivationsOf(machine);
  const key = `${status.id}@${status.build}`;
  let pending = cache.get(key);
  if (!pending) {
    pending = runHiddenActivation(machine, status).then((result) => {
      if (!result.ok) cache.delete(key);
      return result;
    });
    cache.set(key, pending);
  }
  return pending;
}

async function runHiddenActivation(
  machine: MachineKey,
  status: Pick<PluginStatus, "id" | "build">,
): Promise<CloudActivation> {
  if (typeof document === "undefined") {
    return { ok: false, error: "no DOM available for sandboxed plugin frame" };
  }
  try {
    const [runtimeCode, packageData] = await Promise.all([
      fetchFrameRuntimeCode(),
      fetchPluginPackage(machine, status.id, status.build),
    ]);

    return await new Promise<CloudActivation>((resolve) => {
      const iframe = document.createElement("iframe");
      iframe.setAttribute("sandbox", "allow-scripts");
      iframe.setAttribute("aria-hidden", "true");
      iframe.tabIndex = -1;
      iframe.src = "/plugin-frame.html";
      Object.assign(iframe.style, {
        position: "fixed",
        width: "0",
        height: "0",
        border: "0",
        visibility: "hidden",
        pointerEvents: "none",
      });

      let settled = false;
      let port: MessagePort | null = null;
      const inflight = new Map<number, AbortController>();

      const finish = (result: CloudActivation) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        for (const ctrl of inflight.values()) ctrl.abort();
        inflight.clear();
        port?.close();
        iframe.remove();
        resolve(result);
      };

      const timer = setTimeout(() => {
        finish({ ok: false, error: "timed out activating plugin in sandboxed frame" });
      }, ACTIVATION_TIMEOUT_MS);

      iframe.addEventListener("error", () => {
        finish({ ok: false, error: "failed to load /plugin-frame.html" });
      });

      iframe.addEventListener("load", () => {
        if (settled) return;
        const win = iframe.contentWindow;
        if (!win) {
          finish({ ok: false, error: "sandboxed frame has no contentWindow" });
          return;
        }
        const channel = new MessageChannel();
        port = channel.port1;
        port.onmessage = (event: MessageEvent<FrameToParentMessage>) => {
          const msg = event.data;
          if (!msg || typeof msg !== "object") return;
          if (msg.type === "activated") {
            finish({ ok: true, contributions: msg.contributions });
          } else if (msg.type === "error") {
            finish({ ok: false, error: msg.message || "plugin activation failed" });
          } else if (msg.type === "rpc:req") {
            // During hidden activation we refuse interactive RPCs cleanly; storage/http still work if needed.
            if (msg.method === "http:request") {
              const ctrl = new AbortController();
              inflight.set(msg.id, ctrl);
              void forwardFrameHttp(machine, status.id, msg.params, ctrl.signal)
                .then((result) => {
                  inflight.delete(msg.id);
                  const transfer = result.body ? [result.body] : [];
                  port?.postMessage({ type: "rpc:res", id: msg.id, ok: true, result } satisfies ParentToFrameMessage, transfer);
                })
                .catch((error) => {
                  inflight.delete(msg.id);
                  port?.postMessage({ type: "rpc:res", id: msg.id, ok: false, error: serializeError(error) } satisfies ParentToFrameMessage);
                });
            } else if (
              msg.method === "storage:get" ||
              msg.method === "storage:list" ||
              msg.method === "storage:put" ||
              msg.method === "storage:batch"
            ) {
              void handleFrameStorage(machine, status.id, msg.method, msg.params as Record<string, unknown>)
                .then((result) => {
                  port?.postMessage({ type: "rpc:res", id: msg.id, ok: true, result } satisfies ParentToFrameMessage);
                })
                .catch((error) => {
                  port?.postMessage({ type: "rpc:res", id: msg.id, ok: false, error: serializeError(error) } satisfies ParentToFrameMessage);
                });
            } else {
              port?.postMessage({
                type: "rpc:res",
                id: msg.id,
                ok: false,
                error: { kind: "error", message: `${msg.method} is not available during plugin activation` },
              } satisfies ParentToFrameMessage);
            }
          } else if (msg.type === "http:abort") {
            inflight.get(msg.id)?.abort();
            inflight.delete(msg.id);
          }
        };
        port.start();

        const { themeMode, cssVars } = readHostTheme();
        const boot: Extract<ParentToFrameMessage, { type: "boot" }> = {
          type: "boot",
          pluginId: status.id,
          mode: "activate",
          packageData,
          locale: hostLocale(),
          themeMode,
          cssVars,
          clipboardSupported: false,
        };
        // Sandboxed frame without `allow-same-origin` has an opaque `"null"` origin,
        // so `"*"` is required to deliver the initial MessagePort handshake.
        win.postMessage({ type: "droppen-plugin-init", runtimeCode, boot }, "*", [channel.port2]);
      });

      document.body.appendChild(iframe);
    });
  } catch (error) {
    return { ok: false, error: error instanceof Error ? error.message : String(error) };
  }
}

export interface CloudPluginFrameProps {
  machine: MachineKey;
  status: PluginStatus;
  mode: Exclude<FrameMode, "activate">;
  route?: { subpath: string; search: string };
  pane?: {
    id: string;
    state: PaneState;
    onState: (state: PaneState) => void;
    onOpenPage: (path: string) => void;
  };
  settingsIndex?: number;
  /** Compact header banner for split panes / settings blocks. */
  compactBanner?: boolean;
}

/**
 * Visible sandboxed iframe rendering a cloud-enabled plugin's route, pane, or
 * settings block, paired with an outside-the-frame provenance bar naming both
 * the plugin and the machine it runs on (`docs/plugins/cloud.md` §7).
 */
export function CloudPluginFrame({
  machine,
  status,
  mode,
  route,
  pane,
  settingsIndex,
  compactBanner = false,
}: CloudPluginFrameProps) {
  const { t } = useTranslation();
  const machineInfo = useInfoOf(machine);
  const navigateOnMachine = useMachineNavigate(machine);
  const iframeRef = useRef<HTMLIFrameElement | null>(null);
  const portRef = useRef<MessagePort | null>(null);
  const lastSentPaneState = useRef<string>("");

  const [mounted, setMounted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [leaveGuard, setLeaveGuard] = useState<{ active: boolean; message: string }>({
    active: false,
    message: "",
  });

  const pluginName = localize(status.name, hostLocale());
  const machineLabel = machineInfo.name || machine;

  // Keep latest callbacks in refs so the iframe does not reload when parent closures change.
  const paneRef = useRef(pane);
  paneRef.current = pane;
  const routeRef = useRef(route);
  routeRef.current = route;
  const navRef = useRef(navigateOnMachine);
  navRef.current = navigateOnMachine;
  const labelRef = useRef({ pluginName, machineLabel });
  labelRef.current = { pluginName, machineLabel };

  // Wire `host.navigation.useLeaveGuard` reported by the frame to the parent router.
  useBlocker({
    shouldBlockFn: async () => {
      if (!leaveGuard.active) return false;
      const { pluginName: pName, machineLabel: mLabel } = labelRef.current;
      return !(await requestConfirm(leaveGuard.message, {
        title: `${pName} · ${mLabel}`,
      }));
    },
    enableBeforeUnload: leaveGuard.active,
    disabled: !leaveGuard.active,
  });

  // Boot the iframe whenever the machine, plugin id, build, mode, pane id, or settings index changes.
  const paneId = pane?.id;
  useEffect(() => {
    let disposed = false;
    let port: MessagePort | null = null;
    const inflight = new Map<number, AbortController>();
    let themeObserver: MutationObserver | null = null;
    let offDoc: (() => void) | null = null;
    let onLang: (() => void) | null = null;

    setMounted(false);
    setError(null);
    setLeaveGuard({ active: false, message: "" });

    const bootResources = Promise.all([
      fetchFrameRuntimeCode(),
      fetchPluginPackage(machine, status.id, status.build),
    ]);

    const iframe = iframeRef.current;
    if (!iframe) return;

    const startHandshake = () => {
      void bootResources
        .then(([runtimeCode, packageData]) => {
          if (disposed) return;
          const win = iframe.contentWindow;
          if (!win) {
            setError("sandboxed frame has no contentWindow");
            return;
          }
          const channel = new MessageChannel();
          port = channel.port1;
          portRef.current = port;

          const send = (message: ParentToFrameMessage, transfer?: Transferable[]) => {
            if (disposed || !port) return;
            if (transfer && transfer.length > 0) port.postMessage(message, transfer);
            else port.postMessage(message);
          };

          port.onmessage = (event: MessageEvent<FrameToParentMessage>) => {
            if (disposed) return;
            const msg = event.data;
            if (!msg || typeof msg !== "object") return;

            switch (msg.type) {
              case "mounted":
                setMounted(true);
                break;
              case "error":
                setError(msg.message || "plugin failed to render");
                break;
              case "http:abort":
                inflight.get(msg.id)?.abort();
                inflight.delete(msg.id);
                break;
              case "nav:go":
                void navRef.current({
                  to: "/p/$plugin/$",
                  params: { plugin: status.id, _splat: (msg.path ?? "").replace(/^\/+/, "") },
                });
                break;
              case "nav:guard":
                setLeaveGuard({ active: Boolean(msg.active), message: String(msg.message ?? "") });
                break;
              case "pane:state":
                if (validPaneState(msg.state)) {
                  lastSentPaneState.current = JSON.stringify(msg.state);
                  paneRef.current?.onState(msg.state);
                } else {
                  console.warn(`plugin ${status.id}: pane state must be flat and at most 2 KB; not saved`);
                }
                break;
              case "pane:open-page":
                paneRef.current?.onOpenPage((msg.path ?? "").replace(/^\/+/, ""));
                break;
              case "rpc:req": {
                const { id, method } = msg;
                if (method === "http:request") {
                  const ctrl = new AbortController();
                  inflight.set(id, ctrl);
                  void forwardFrameHttp(machine, status.id, msg.params, ctrl.signal)
                    .then((result) => {
                      inflight.delete(id);
                      const transfer = result.body ? [result.body] : [];
                      send({ type: "rpc:res", id, ok: true, result }, transfer);
                    })
                    .catch((err) => {
                      inflight.delete(id);
                      send({ type: "rpc:res", id, ok: false, error: serializeError(err) });
                    });
                } else if (
                  method === "storage:get" ||
                  method === "storage:list" ||
                  method === "storage:put" ||
                  method === "storage:batch"
                ) {
                  void handleFrameStorage(machine, status.id, method, msg.params as Record<string, unknown>)
                    .then((result) => send({ type: "rpc:res", id, ok: true, result }))
                    .catch((err) => send({ type: "rpc:res", id, ok: false, error: serializeError(err) }));
                } else if (method === "ui:confirm") {
                  const { pluginName: pName, machineLabel: mLabel } = labelRef.current;
                  const baseTitle = `${pName} · ${mLabel}`;
                  const title = msg.params.options?.title
                    ? `${baseTitle} — ${msg.params.options.title}`
                    : baseTitle;
                  void requestConfirm(msg.params.message, { ...msg.params.options, title })
                    .then((yes) => send({ type: "rpc:res", id, ok: true, result: yes }))
                    .catch((err) => send({ type: "rpc:res", id, ok: false, error: serializeError(err) }));
                } else if (method === "files:save") {
                  const blob = new Blob([msg.params.bytes], {
                    type: msg.params.mimeType || "application/octet-stream",
                  });
                  void saveFile(blob, msg.params.suggestedName)
                    .then(() => send({ type: "rpc:res", id, ok: true, result: undefined }))
                    .catch((err) => send({ type: "rpc:res", id, ok: false, error: serializeError(err) }));
                } else if (method === "clipboard:write") {
                  if (typeof navigator === "undefined" || !navigator.clipboard?.writeText) {
                    send({
                      type: "rpc:res",
                      id,
                      ok: false,
                      error: { kind: "error", message: "clipboard is not available" },
                    });
                  } else {
                    void navigator.clipboard
                      .writeText(msg.params.text)
                      .then(() => send({ type: "rpc:res", id, ok: true, result: undefined }))
                      .catch((err) => send({ type: "rpc:res", id, ok: false, error: serializeError(err) }));
                  }
                }
                break;
              }
            }
          };
          port.start();

          // Live theme sync
          if (typeof MutationObserver !== "undefined") {
            themeObserver = new MutationObserver(() => {
              const next = readHostTheme();
              send({ type: "update:theme", themeMode: next.themeMode, cssVars: next.cssVars });
            });
            themeObserver.observe(document.documentElement, {
              attributes: true,
              attributeFilter: ["class", "style"],
            });
          }

          // Live locale sync
          onLang = () => send({ type: "update:locale", locale: hostLocale() });
          i18n.on("languageChanged", onLang);

          // Live doc:changed events from this machine's feed
          offDoc = pluginEventsFor(machine).onDoc((event) => {
            if (event.plugin === "*" || event.plugin === status.id) {
              send({
                type: "doc:changed",
                plugin: event.plugin,
                name: event.name,
                revision: event.revision,
              });
            }
          });

          const { themeMode, cssVars } = readHostTheme();
          const initialPaneState = paneRef.current?.state ?? {};
          lastSentPaneState.current = JSON.stringify(initialPaneState);
          const boot: Extract<ParentToFrameMessage, { type: "boot" }> = {
            type: "boot",
            pluginId: status.id,
            mode,
            packageData,
            locale: hostLocale(),
            themeMode,
            cssVars,
            clipboardSupported:
              typeof navigator !== "undefined" &&
              Boolean(navigator.clipboard?.writeText) &&
              (typeof window === "undefined" || window.isSecureContext),
            ...(mode === "route" ? { route: routeRef.current ?? { subpath: "", search: "" } } : {}),
            ...(mode === "pane" && paneId ? { pane: { id: paneId, state: initialPaneState } } : {}),
            ...(mode === "settings" ? { settingsIndex: settingsIndex ?? 0 } : {}),
          };
          win.postMessage({ type: "droppen-plugin-init", runtimeCode, boot }, "*", [channel.port2]);
        })
        .catch((err) => {
          if (!disposed) setError(err instanceof Error ? err.message : String(err));
        });
    };

    const onLoad = () => startHandshake();
    const onError = () => {
      if (!disposed) setError("failed to load /plugin-frame.html");
    };
    iframe.addEventListener("load", onLoad);
    iframe.addEventListener("error", onError);
    // Reload src cleanly so `load` always fires for a fresh boot.
    iframe.src = "/plugin-frame.html";

    return () => {
      disposed = true;
      iframe.removeEventListener("load", onLoad);
      iframe.removeEventListener("error", onError);
      themeObserver?.disconnect();
      if (onLang) i18n.off("languageChanged", onLang);
      offDoc?.();
      for (const ctrl of inflight.values()) ctrl.abort();
      inflight.clear();
      if (portRef.current === port) portRef.current = null;
      port?.close();
    };
  }, [machine, status.id, status.build, mode, paneId, settingsIndex]);

  // Push route changes to an already-booted route frame without reloading the iframe.
  const routeSubpath = route?.subpath;
  const routeSearch = route?.search;
  useEffect(() => {
    if (mode !== "route" || !portRef.current) return;
    portRef.current.postMessage({
      type: "update:route",
      subpath: routeSubpath ?? "",
      search: routeSearch ?? "",
    } satisfies ParentToFrameMessage);
  }, [mode, routeSubpath, routeSearch]);

  // Push external pane state changes into an already-booted pane frame.
  const serializedPaneState = pane ? JSON.stringify(pane.state) : "";
  useEffect(() => {
    if (mode !== "pane" || !pane || !portRef.current) return;
    if (serializedPaneState === lastSentPaneState.current) return;
    lastSentPaneState.current = serializedPaneState;
    portRef.current.postMessage({
      type: "update:pane-state",
      state: pane.state,
    } satisfies ParentToFrameMessage);
  }, [mode, pane, serializedPaneState]);

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col" data-plugin-cloud-frame={status.id}>
      <div
        data-plugin-provenance=""
        className={
          compactBanner
            ? "flex shrink-0 items-center gap-1.5 border-b border-border bg-panel px-2.5 py-1 font-mono text-[10px] text-faint"
            : "flex shrink-0 items-center gap-2 border-b border-border bg-panel px-3 py-1.5 font-mono text-2xs text-muted"
        }
      >
        <PuzzlePieceIcon className="size-3 shrink-0 text-faint" />
        <span className="truncate text-fg">{pluginName}</span>
        <span className="shrink-0 text-faint">v{status.version}</span>
        <span className="text-faint">·</span>
        <span className="min-w-0 truncate text-faint">
          {t("running on {{device}}", { device: machineLabel })}
        </span>
      </div>
      <div className="relative flex min-h-0 min-w-0 flex-1 flex-col">
        {error ? (
          <div role="alert" className="flex min-h-0 flex-1 flex-col items-center justify-center gap-1.5 p-4 text-center">
            <span className="text-xs text-danger">{t("the {{name}} plugin could not start", { name: pluginName })}</span>
            <span className="max-w-md font-mono text-2xs break-words text-faint">{error}</span>
          </div>
        ) : (
          <>
            {!mounted && (
              <div className="absolute inset-0 z-10 flex items-center justify-center bg-bg">
                <Spinner />
              </div>
            )}
            <iframe
              ref={iframeRef}
              title={`${pluginName} (${machineLabel})`}
              sandbox="allow-scripts"
              className="h-full w-full flex-1 border-0 bg-bg"
            />
          </>
        )}
      </div>
    </div>
  );
}
