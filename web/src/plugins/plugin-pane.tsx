/**
 * Plugin content in a split pane, and the picker's plugin entries.
 */
import { localize, type PaneContribution, type PaneState } from "@droppen/plugin-sdk";
import { useRouter } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { Spinner } from "@/components/ui/spinner";
import { isCloud } from "@/lib/hub/config";
import { PluginBoundary } from "./boundary";
import { hostLocale } from "./host";
import { validPaneState, type PluginPaneContent } from "./pane-content";
import { usePluginRegistry, type ActivePlugin } from "./registry";

/** Every pane the running plugins offer, in plugin order. */
export function usePluginPaneOptions(): { plugin: ActivePlugin; pane: PaneContribution }[] {
  const { active } = usePluginRegistry();
  return active.flatMap((plugin) => (plugin.contributions.panes ?? []).map((pane) => ({ plugin, pane })));
}

/** The pane header's title for plugin content. */
export function usePluginPaneTitle(content: PluginPaneContent): string {
  const registry = usePluginRegistry();
  const locale = hostLocale();
  const active = registry.active.find((p) => p.id === content.plugin);
  const pane = active?.contributions.panes?.find((p) => p.id === content.pane);
  if (pane) return localize(pane.label, locale);
  const status = registry.statuses?.find((s) => s.id === content.plugin);
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
  const { t } = useTranslation();
  const router = useRouter();
  const registry = usePluginRegistry();
  // A page a hub serves shows no plugin content at all (§5): a layout may
  // have been mirrored from the machine with a plugin pane in it.
  if (isCloud()) return null;
  if (registry.statuses === null) return <Spinner className="flex-1" />;
  const status = registry.statuses.find((s) => s.id === content.plugin);
  if (!status) return <Placeholder note={t("this pane's plugin is not installed on this instance")} onReset={onReset} />;
  const name = localize(status.name, hostLocale());
  if (registry.loading.includes(content.plugin)) return <Spinner className="flex-1" />;
  const active = registry.active.find((p) => p.id === content.plugin);
  if (registry.errors[content.plugin]) return <Placeholder note={t("the {{name}} plugin could not start", { name })} onReset={onReset} />;
  if (!status.compatible) return <Placeholder note={t("{{name}} does not work with this version of Droppen", { name })} onReset={onReset} />;
  if (!active) return <Placeholder note={t("the {{name}} plugin is not enabled", { name })} onReset={onReset} />;
  const pane = active.contributions.panes?.find((p) => p.id === content.pane);
  if (!pane) return <Placeholder note={t("{{name}} has no pane called {{pane}}", { name, pane: content.pane })} onReset={onReset} />;
  const Pane = pane.component;
  const openPage = (path = "") =>
    void router.navigate({ to: "/p/$plugin/$", params: { plugin: content.plugin, _splat: path.replace(/^\/+/, "") } });
  const notices = active.status.notices ?? (active.status.notice ? [active.status.notice] : []);
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-auto" data-plugin={content.plugin}>
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
