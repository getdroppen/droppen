/**
 * Plugin content in a split pane / Screen, and the picker's plugin entries.
 *
 * Works both locally (rendering the plugin's pane or full page directly) and
 * on `droppen.app` (rendering inside a sandboxed `/plugin-frame.html` iframe
 * bound to the pane's machine). Plugins that register routes/nav without
 * explicit `contributions.panes` are offered via the reserved pane id
 * `PLUGIN_PAGE_PANE_ID` (`"droppen-page"`), so every plugin can be placed in a
 * Screen (Issue #299).
 */
import { localize, type LocalizedText, type PaneState, type PluginHost } from "@droppen/plugin-sdk";
import { useMemo, type ComponentType } from "react";
import { useTranslation } from "react-i18next";
import { useMachineNavigate } from "@/app/machine-route";
import { Spinner } from "@/components/ui/spinner";
import { isCloud } from "@/lib/hub/config";
import { useHoldMachine, useInfoOf, useMachine } from "@/lib/machines/context";
import type { MachineKey } from "@/lib/machines/key";
import { PluginBoundary } from "./boundary";
import { CloudPluginFrame } from "./cloud-frame";
import { hostLocale } from "./host";
import { PLUGIN_PAGE_PANE_ID, validPaneState, type PluginPaneContent } from "./pane-content";
import { useMachinePlugins, usePluginRegistry } from "./registry";
import { matchRoute } from "./routing";

export interface PluginPaneOption {
  key: string;
  pluginId: string;
  paneId: string;
  label: LocalizedText;
  description?: LocalizedText;
  state?: PaneState;
  /** Present on local pages so the picker can render the plugin's own React icon. */
  local?: { host: PluginHost; icon?: ComponentType<{ className?: string }> };
}

/**
 * Every pane the running plugins on `machine` offer, in plugin order:
 * explicit `contributions.panes`, plus page entries (`PLUGIN_PAGE_PANE_ID`)
 * for plugins that only define `routes`/`nav`.
 */
export function usePluginPaneOptions(machine: MachineKey): PluginPaneOption[] {
  const localRegistry = usePluginRegistry();
  const cloudRegistry = useMachinePlugins(machine);
  const cloud = isCloud();

  return useMemo(() => {
    if (cloud) {
      return cloudRegistry.active.flatMap((plugin): PluginPaneOption[] => {
        const panes = plugin.contributions.panes ?? [];
        if (panes.length > 0) {
          return panes.map((pane) => ({
            key: `${plugin.id}/${pane.id}`,
            pluginId: plugin.id,
            paneId: pane.id,
            label: pane.label,
            description: pane.description,
          }));
        }
        const routes = plugin.contributions.routes ?? [];
        if (routes.length === 0) return [];
        const nav = plugin.contributions.nav ?? [];
        if (nav.length > 0) {
          return nav.map((item, idx) => {
            const subpath = (item.path ?? "").replace(/^\/+/, "");
            return {
              key: `${plugin.id}/${PLUGIN_PAGE_PANE_ID}/${idx}`,
              pluginId: plugin.id,
              paneId: PLUGIN_PAGE_PANE_ID,
              label: item.label,
              state: subpath ? { subpath } : undefined,
            };
          });
        }
        return [
          {
            key: `${plugin.id}/${PLUGIN_PAGE_PANE_ID}`,
            pluginId: plugin.id,
            paneId: PLUGIN_PAGE_PANE_ID,
            label: plugin.status.name,
          },
        ];
      });
    }

    return localRegistry.active.flatMap((plugin): PluginPaneOption[] => {
      const panes = plugin.contributions.panes ?? [];
      if (panes.length > 0) {
        return panes.map((pane) => ({
          key: `${plugin.id}/${pane.id}`,
          pluginId: plugin.id,
          paneId: pane.id,
          label: pane.label,
          description: pane.description,
          local: { host: plugin.host, icon: pane.icon },
        }));
      }
      const routes = plugin.contributions.routes ?? [];
      if (routes.length === 0) return [];
      const nav = plugin.contributions.nav ?? [];
      if (nav.length > 0) {
        return nav.map((item, idx) => {
          const subpath = (item.path ?? "").replace(/^\/+/, "");
          return {
            key: `${plugin.id}/${PLUGIN_PAGE_PANE_ID}/${idx}`,
            pluginId: plugin.id,
            paneId: PLUGIN_PAGE_PANE_ID,
            label: item.label,
            state: subpath ? { subpath } : undefined,
            local: { host: plugin.host, icon: item.icon },
          };
        });
      }
      return [
        {
          key: `${plugin.id}/${PLUGIN_PAGE_PANE_ID}`,
          pluginId: plugin.id,
          paneId: PLUGIN_PAGE_PANE_ID,
          label: plugin.status.name,
          local: { host: plugin.host },
        },
      ];
    });
  }, [cloud, cloudRegistry.active, localRegistry.active]);
}

/** The pane header's title for plugin content. */
export function usePluginPaneTitle(content: PluginPaneContent): string {
  const machine = useMachine();
  const localRegistry = usePluginRegistry();
  const cloudRegistry = useMachinePlugins(machine);
  const locale = hostLocale();

  if (isCloud()) {
    const active = cloudRegistry.active.find((p) => p.id === content.plugin);
    if (content.pane === PLUGIN_PAGE_PANE_ID) {
      const subpath = typeof content.state?.subpath === "string" ? content.state.subpath.replace(/^\/+/, "") : "";
      const nav = active?.contributions.nav?.find((n) => (n.path ?? "").replace(/^\/+/, "") === subpath);
      if (nav) return localize(nav.label, locale);
    } else {
      const pane = active?.contributions.panes?.find((p) => p.id === content.pane);
      if (pane) return localize(pane.label, locale);
    }
    const status = cloudRegistry.statuses?.find((s) => s.id === content.plugin);
    return status ? localize(status.name, locale) : content.plugin;
  }

  const active = localRegistry.active.find((p) => p.id === content.plugin);
  if (content.pane === PLUGIN_PAGE_PANE_ID) {
    const subpath = typeof content.state?.subpath === "string" ? content.state.subpath.replace(/^\/+/, "") : "";
    const nav = active?.contributions.nav?.find((n) => (n.path ?? "").replace(/^\/+/, "") === subpath);
    if (nav) return localize(nav.label, locale);
  } else {
    const pane = active?.contributions.panes?.find((p) => p.id === content.pane);
    if (pane) return localize(pane.label, locale);
  }
  const status = localRegistry.statuses?.find((s) => s.id === content.plugin);
  return status ? localize(status.name, locale) : content.plugin;
}

function Placeholder({ note, onReset }: { note: string; onReset: () => void }) {
  const { t } = useTranslation();
  return (
    <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-2 p-3 text-center">
      <span className="font-mono text-2xs text-muted">{note}</span>
      <button
        type="button"
        onClick={onReset}
        className="flex items-center gap-1 border border-border px-2 py-1 font-mono text-2xs text-fg hover:bg-hover"
      >
        {t("pick new content")}
      </button>
    </div>
  );
}

/**
 * A plugin's pane. A pane whose plugin is off stays in the layout as a
 * placeholder — the layout is the user's, and turning the plugin back on
 * brings the pane back as it was.
 */
export function PluginPane({
  content,
  onState,
  onReset,
}: {
  content: PluginPaneContent;
  onState: (state: PaneState) => void;
  onReset: () => void;
}) {
  if (isCloud()) return <CloudPluginPane content={content} onState={onState} onReset={onReset} />;
  return <LocalPluginPane content={content} onState={onState} onReset={onReset} />;
}

function LocalPluginPane({
  content,
  onState,
  onReset,
}: {
  content: PluginPaneContent;
  onState: (state: PaneState) => void;
  onReset: () => void;
}) {
  const { t } = useTranslation();
  const machine = useMachine();
  const navigateOnMachine = useMachineNavigate(machine);
  const registry = usePluginRegistry();
  const emptySearch = useMemo(() => new URLSearchParams(), []);

  if (registry.statuses === null) return <Spinner className="flex-1" />;
  const status = registry.statuses.find((s) => s.id === content.plugin);
  if (!status) return <Placeholder note={t("this pane's plugin is not installed on this instance")} onReset={onReset} />;
  const name = localize(status.name, hostLocale());
  if (registry.loading.includes(content.plugin)) return <Spinner className="flex-1" />;
  const active = registry.active.find((p) => p.id === content.plugin);
  if (registry.errors[content.plugin]) return <Placeholder note={t("the {{name}} plugin could not start", { name })} onReset={onReset} />;
  if (!status.compatible) return <Placeholder note={t("{{name}} does not work with this version of Droppen", { name })} onReset={onReset} />;
  if (!active) return <Placeholder note={t("the {{name}} plugin is not enabled", { name })} onReset={onReset} />;

  const openPage = (path = "") =>
    void navigateOnMachine({ to: "/p/$plugin/$", params: { plugin: content.plugin, _splat: path.replace(/^\/+/, "") } });
  const notices = active.status.notices ?? (active.status.notice ? [active.status.notice] : []);

  const noticeBar = notices.length > 0 && (
    <button
      type="button"
      role="alert"
      data-signal="failed"
      onClick={() => openPage()}
      title={notices.map((n) => (typeof n === "string" ? n : n.text)).join("\n")}
      className="flex shrink-0 items-center gap-1.5 border-b border-border px-3 py-1.5 text-left text-2xs text-danger hover:bg-hover"
    >
      <span className="min-w-0 flex-1 truncate">{t("{{count}} notices — open {{name}} to read them", { count: notices.length, name })}</span>
    </button>
  );

  // Page-as-pane (Issue #299): render the plugin's matched RouteContribution directly.
  if (content.pane === PLUGIN_PAGE_PANE_ID) {
    const subpath = typeof content.state?.subpath === "string" ? content.state.subpath : "";
    const route = matchRoute(active.contributions.routes ?? [], subpath);
    if (!route) return <Placeholder note={t("{{name}} has no page here", { name })} onReset={onReset} />;
    const Page = route.component;
    return (
      <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-auto" data-plugin={content.plugin}>
        {noticeBar}
        <div className="flex min-h-0 min-w-0 flex-1">
          <PluginBoundary host={active.host} name={name}>
            <Page subpath={subpath} search={emptySearch} />
          </PluginBoundary>
        </div>
      </div>
    );
  }

  const pane = active.contributions.panes?.find((p) => p.id === content.pane);
  if (!pane) return <Placeholder note={t("{{name}} has no pane called {{pane}}", { name, pane: content.pane })} onReset={onReset} />;
  const Pane = pane.component;
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-auto" data-plugin={content.plugin}>
      {noticeBar}
      <PluginBoundary host={active.host} name={name}>
        <Pane
          state={content.state ?? {}}
          setState={(next) => {
            if (validPaneState(next)) onState(next);
            else console.warn(`plugin ${content.plugin}: pane state must be flat and at most 2 KB; not saved`);
          }}
          openPage={openPage}
        />
      </PluginBoundary>
    </div>
  );
}

function CloudPluginPane({
  content,
  onState,
  onReset,
}: {
  content: PluginPaneContent;
  onState: (state: PaneState) => void;
  onReset: () => void;
}) {
  const { t } = useTranslation();
  const machine = useMachine();
  useHoldMachine(machine);
  const info = useInfoOf(machine);
  const navigateOnMachine = useMachineNavigate(machine);
  const registry = useMachinePlugins(machine);

  const connecting =
    info.state === "unknown" || info.live === "connecting" || (info.state === "online" && info.live === "copy");
  if (connecting) return <Spinner className="flex-1" />;
  if (info.state !== "online" || info.live !== "live") {
    return (
      <Placeholder
        note={t("{{device}} is offline — plugins run live on their device", { device: info.name || machine })}
        onReset={onReset}
      />
    );
  }
  if (registry.statuses === null) return <Spinner className="flex-1" />;
  const status = registry.statuses.find((s) => s.id === content.plugin);
  if (!status || !status.enabled || !status.cloud) {
    return (
      <Placeholder
        note={t("the {{name}} plugin is not available on droppen.app", { name: status ? localize(status.name, hostLocale()) : content.plugin })}
        onReset={onReset}
      />
    );
  }
  const name = localize(status.name, hostLocale());
  if (!status.compatible) return <Placeholder note={t("{{name}} does not work with this version of Droppen", { name })} onReset={onReset} />;
  if (registry.errors[content.plugin]) return <Placeholder note={t("the {{name}} plugin could not start", { name })} onReset={onReset} />;

  const openPage = (path = "") =>
    void navigateOnMachine({ to: "/p/$plugin/$", params: { plugin: content.plugin, _splat: path.replace(/^\/+/, "") } });
  const notices = status.notices ?? (status.notice ? [status.notice] : []);

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col" data-plugin={content.plugin}>
      {notices.length > 0 && (
        <button
          type="button"
          role="alert"
          data-signal="failed"
          onClick={() => openPage()}
          title={notices.map((n) => (typeof n === "string" ? n : n.text)).join("\n")}
          className="flex shrink-0 items-center gap-1.5 border-b border-border px-3 py-1.5 text-left text-2xs text-danger hover:bg-hover"
        >
          <span className="min-w-0 flex-1 truncate">{t("{{count}} notices — open {{name}} to read them", { count: notices.length, name })}</span>
        </button>
      )}
      <CloudPluginFrame
        machine={machine}
        status={status}
        mode="pane"
        compactBanner
        pane={{
          id: content.pane,
          state: content.state ?? {},
          onState,
          onOpenPage: openPage,
        }}
      />
    </div>
  );
}
