/**
 * `/p/$plugin/$` (and `/m/$machine/p/$plugin/$`) — every plugin page.
 *
 * Locally, routed directly into the host's matched `RouteContribution`
 * (`docs/plugins/runtime.md` §6). On `droppen.app`, rendered inside a sandboxed
 * `/plugin-frame.html` iframe bound to the target machine (`docs/plugins/cloud.md` §2–§6).
 */
import { localize } from "@droppen/plugin-sdk";
import { Link, useParams, useRouterState } from "@tanstack/react-router";
import { PuzzlePieceIcon } from "@phosphor-icons/react";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/cn";
import { isCloud } from "@/lib/hub/config";
import { useHoldMachine, useInfoOf, useMachine } from "@/lib/machines/context";
import { PluginBoundary } from "./boundary";
import { CloudPluginFrame } from "./cloud-frame";
import { hostLocale } from "./host";
import { ReloadNotice } from "./reload-notice";
import { useMachinePlugins, usePluginRegistry } from "./registry";
import type { PluginStatus } from "./types";
import { matchRoute } from "./routing";

/**
 * A plugin page that cannot be shown, and why. `hint` says what to do about
 * it; `warn` marks a problem the user did not choose (a plugin they switched
 * on that this host cannot run), not a plugin that is merely off.
 */
export function PluginUnavailable({
  message,
  detail,
  hint,
  warn,
  hideSettingsLink = false,
}: {
  message: string;
  detail?: string;
  hint?: string;
  warn?: boolean;
  hideSettingsLink?: boolean;
}) {
  const { t } = useTranslation();
  return (
    <div
      role={warn ? "alert" : "status"}
      data-signal={warn ? "warn" : undefined}
      className="flex min-h-0 min-w-0 flex-1 flex-col items-center justify-center gap-2 p-6 text-center"
    >
      <PuzzlePieceIcon className={cn("size-5", warn ? "text-warn-fg" : "text-faint")} />
      <span className="text-xs text-fg">{message}</span>
      {detail && <span className="max-w-md font-mono text-2xs break-words text-faint">{detail}</span>}
      {hint && <span className="max-w-md text-2xs text-muted">{hint}</span>}
      {!hideSettingsLink && !isCloud() && (
        <Link
          to="/settings"
          search={{ section: "plugins" }}
          className="font-mono text-2xs text-muted underline-offset-2 hover:text-fg hover:underline"
        >
          {t("Settings")} › {t("plugins")}
        </Link>
      )}
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
  if (isCloud()) return <CloudPluginRoute />;
  return <LocalPluginRoute />;
}

function LocalPluginRoute() {
  const { t } = useTranslation();
  const params = useParams({ strict: false }) as { plugin?: string; _splat?: string };
  const id = params.plugin ?? "";
  const subpath = params._splat ?? "";
  const searchStr = useRouterState({ select: (s) => s.location.searchStr });
  const search = useMemo(() => new URLSearchParams(searchStr), [searchStr]);
  const registry = usePluginRegistry();
  // A page that cannot be shown, with the reload prompt above it when the
  // reason may be a change made elsewhere (docs/plugins/runtime.md §6).
  const unavailable = (message: string, detail?: string, more: { hint?: string; warn?: boolean } = {}) => (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      {registry.reloadNeeded && <ReloadNotice className="m-3 mb-0" />}
      <PluginUnavailable message={message} detail={detail} {...more} />
    </div>
  );
  if (registry.statuses === null) return <Spinner className="flex-1" />;
  const status = registry.statuses.find((s) => s.id === id);
  if (!status) return unavailable(t("there is no plugin called {{id}} on this instance", { id }));
  const name = localize(status.name, hostLocale());
  if (!status.compatible)
    return unavailable(t("{{name}} does not work with this version of Droppen", { name }), status.reason ?? undefined, {
      hint: t("Install a build of it made for this version, or switch it off."),
      warn: status.switchedOn,
    });
  if (registry.loading.includes(id)) return <Spinner className="flex-1" />;
  if (registry.errors[id]) return unavailable(t("the {{name}} plugin could not start", { name }), registry.errors[id]);
  const active = registry.active.find((p) => p.id === id);
  if (!status.enabled || !active) return unavailable(t("the {{name}} plugin is not enabled", { name }), status.reason ?? undefined);
  const route = matchRoute(active.contributions.routes ?? [], subpath);
  if (!route) return unavailable(t("{{name}} has no page here", { name }), `/p/${id}/${subpath}`);
  const Page = route.component;
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col" data-plugin={id}>
      {registry.reloadNeeded && <ReloadNotice className="m-3 mb-0" />}
      <PluginNotices status={status} />
      <div className="flex min-h-0 min-w-0 flex-1">
        <PluginBoundary host={active.host} name={name}>
          <Page subpath={subpath} search={search} />
        </PluginBoundary>
      </div>
    </div>
  );
}

function CloudPluginRoute() {
  const { t } = useTranslation();
  const machine = useMachine();
  useHoldMachine(machine);
  const info = useInfoOf(machine);
  const params = useParams({ strict: false }) as { plugin?: string; _splat?: string };
  const id = params.plugin ?? "";
  const subpath = params._splat ?? "";
  const searchStr = useRouterState({ select: (s) => s.location.searchStr });
  const registry = useMachinePlugins(machine);

  const connecting =
    info.state === "unknown" || info.live === "connecting" || (info.state === "online" && info.live === "copy");
  if (connecting) return <Spinner className="flex-1" />;
  if (info.state !== "online" || info.live !== "live") {
    return (
      <PluginUnavailable
        message={t("{{device}} is offline", { device: info.name || machine })}
        detail={t("Plugins run live on their device and are not available while it is offline.")}
        hideSettingsLink
      />
    );
  }
  if (registry.statuses === null) return <Spinner className="flex-1" />;
  const status = registry.statuses.find((s) => s.id === id);
  if (!status || !status.enabled || !status.cloud) {
    return (
      <PluginUnavailable
        message={t("the {{name}} plugin is not available on droppen.app", { name: status ? localize(status.name, hostLocale()) : id })}
        hint={t("Turn on \"Show on droppen.app\" in Settings › Plugins on the device itself.")}
        hideSettingsLink
      />
    );
  }
  const name = localize(status.name, hostLocale());
  if (!status.compatible) {
    return (
      <PluginUnavailable
        message={t("{{name}} does not work with this version of Droppen", { name })}
        detail={status.reason ?? undefined}
        warn
        hideSettingsLink
      />
    );
  }
  if (registry.errors[id]) {
    return (
      <PluginUnavailable
        message={t("the {{name}} plugin could not start", { name })}
        detail={registry.errors[id]}
        hideSettingsLink
      />
    );
  }
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col" data-plugin={id}>
      <PluginNotices status={status} />
      <CloudPluginFrame
        machine={machine}
        status={status}
        mode="route"
        route={{ subpath, search: searchStr }}
      />
    </div>
  );
}
