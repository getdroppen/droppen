/**
 * `/p/$plugin/$` — every plugin page, routed by the longest matching
 * `RouteContribution.path` prefix of the rest of the path.
 */
import { localize } from "@droppen/plugin-sdk";
import { Link, useParams, useRouterState } from "@tanstack/react-router";
import { Puzzle } from "lucide-react";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { Spinner } from "@/components/ui/spinner";
import { PluginBoundary } from "./boundary";
import { hostLocale } from "./host";
import { usePluginRegistry } from "./registry";
import type { PluginStatus } from "./types";
import { matchRoute } from "./routing";

/** A plugin page that cannot be shown, and why. */
export function PluginUnavailable({ message, detail }: { message: string; detail?: string }) {
  const { t } = useTranslation();
  return (
    <div role="status" className="flex min-h-0 min-w-0 flex-1 flex-col items-center justify-center gap-2 p-6 text-center">
      <Puzzle className="size-5 text-faint" strokeWidth={1.5} />
      <span className="text-xs text-fg">{message}</span>
      {detail && <span className="max-w-md font-mono text-2xs break-words text-faint">{detail}</span>}
      <Link
        to="/settings"
        search={{ section: "plugins" }}
        className="font-mono text-2xs text-muted underline-offset-2 hover:text-fg hover:underline"
      >
        {t("Settings")} › {t("plugins")}
      </Link>
    </div>
  );
}

/**
 * A plugin's notices (a store that had to be reset), above its own page where
 * they cannot be missed. A problem still there stays until it is fixed.
 */
export function PluginNotices({ status }: { status: PluginStatus }) {
  const { t } = useTranslation();
  const notices = status.notices ?? (status.notice ? [{ id: "", kind: "", text: status.notice, at: "", dismissible: false }] : []);
  if (!notices.length) return null;
  return (
    <div role="alert" data-signal="failed" data-plugin-notices="" className="flex shrink-0 flex-col border-b border-border">
      {notices.map((notice) => (
        <div key={notice.id || notice.text} className="flex items-start gap-2 border-b border-border px-3 py-2 text-xs text-danger last:border-b-0">
          <span className="min-w-0 flex-1 break-words whitespace-pre-wrap">{notice.text}</span>
          <span className="shrink-0 font-mono text-2xs text-faint">{t("until fixed")}</span>
        </div>
      ))}
    </div>
  );
}

export function PluginRoute() {
  const { t } = useTranslation();
  const params = useParams({ strict: false }) as { plugin?: string; _splat?: string };
  const id = params.plugin ?? "";
  const subpath = params._splat ?? "";
  const searchStr = useRouterState({ select: (s) => s.location.searchStr });
  const search = useMemo(() => new URLSearchParams(searchStr), [searchStr]);
  const registry = usePluginRegistry();
  const bundled = registry.bundled.find((b) => b.id === id);
  if (!bundled) return <PluginUnavailable message={t("there is no plugin called {{id}} in this build", { id })} />;
  const name = localize(bundled.manifest.name, hostLocale());
  const status = registry.statuses?.find((s) => s.id === id);
  if (registry.statuses === null || registry.loading.includes(id)) return <Spinner className="flex-1" />;
  if (registry.errors[id]) return <PluginUnavailable message={t("the {{name}} plugin could not start", { name })} detail={registry.errors[id]} />;
  const active = registry.active.find((p) => p.id === id);
  if (!status?.enabled || !active)
    return <PluginUnavailable message={t("the {{name}} plugin is not enabled", { name })} detail={status?.reason ?? undefined} />;
  const route = matchRoute(active.contributions.routes ?? [], subpath);
  if (!route) return <PluginUnavailable message={t("{{name}} has no page here", { name })} detail={`/p/${id}/${subpath}`} />;
  const Page = route.component;
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col" data-plugin={id}>
      <PluginNotices status={status} />
      <div className="flex min-h-0 min-w-0 flex-1">
        <PluginBoundary host={active.host} name={name}>
          <Page subpath={subpath} search={search} />
        </PluginBoundary>
      </div>
    </div>
  );
}
