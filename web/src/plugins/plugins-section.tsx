/**
 * Settings › Plugins: the plugins installed in this instance, whether each is
 * on, and the settings blocks the running ones contribute
 * (`docs/plugins/runtime.md` §1, §6).
 *
 * Any signed-in user of the instance may switch a plugin or install and
 * uninstall one (there are no roles); a switch the configuration pins
 * (`[plugins] enable/disable`) is read-only and says so. Switching off keeps
 * the plugin's data. Installing or uninstalling reloads the page: the plugin
 * list and the modules it loaded cannot change under a running page.
 *
 * An install asks first, and says what it means: a plugin runs unsandboxed
 * in this page, as the signed-in user (§8). A plugin switched on that this
 * host cannot run shows its switch on and its problem as a warning; switching
 * it off is how the user puts it away (§6).
 *
 * A page a hub serves has no such section (§5).
 */
import { localize } from "@droppen/plugin-sdk";
import { useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsList, TabsTab } from "@/components/ui/tabs";
import { SETTINGS_ACTION, SETTINGS_ACTION_DANGER } from "@/features/settings/layout";
import { cn } from "@/lib/cn";
import { isCloud } from "@/lib/hub/config";
import { useApi } from "@/lib/machines/context";
import { PluginBoundary } from "./boundary";
import { requestConfirm } from "./confirm";
import { hostLocale, openFile } from "./host";
import { installPlugin, uninstallPlugin } from "./install-api";
import { ReloadNotice } from "./reload-notice";
import { usePluginRegistry } from "./registry";
import type { PluginStatus } from "./types";

const row =
  "flex max-w-xl flex-col items-start gap-2 @min-[36rem]/settings:flex-row @min-[36rem]/settings:items-center @min-[36rem]/settings:justify-between @min-[36rem]/settings:gap-4";

/** The page after an install or uninstall: the modules already loaded cannot be replaced. */
const reloadPage = () => window.location.reload();

function PluginRow({ status }: { status: PluginStatus }) {
  const api = useApi();
  const { t } = useTranslation();
  const registry = usePluginRegistry();
  const name = localize(status.name, hostLocale());
  const toggle = useMutation({
    mutationFn: async (enabled: boolean) => {
      const res = await api.PUT("/api/plugins/{id}/enabled", { params: { path: { id: status.id } }, body: { enabled } });
      if (!res.data) {
        const body = res.error as { error?: string; message?: string } | undefined;
        throw new Error(body?.message ?? body?.error ?? `HTTP ${res.response.status}`);
      }
      return res.data;
    },
    onSuccess: (next) => registry.adopt(next),
  });
  const uninstall = useMutation({ mutationFn: () => uninstallPlugin(api, status.id), onSuccess: reloadPage });

  const askUninstall = async () => {
    const yes = await requestConfirm(
      t("uninstall the {{name}} plugin? its documents are kept for a later install", { name }),
      { danger: true, confirmLabel: t("uninstall") },
    );
    if (yes) uninstall.mutate();
  };

  const failure = registry.errors[status.id];
  const problem = uninstall.isError ? uninstall.error.message : toggle.isError ? toggle.error.message : null;
  // Switched on and cannot run: not the user's choice, so it is a warning.
  const stranded = status.switchedOn && !status.compatible;
  const notes = [
    !status.compatible ? { text: t("not compatible with this version of Droppen"), signal: stranded ? "warn" : null } : null,
    status.reason ? { text: status.reason, signal: stranded ? "warn" : null } : null,
    status.notice ? { text: status.notice, signal: null } : null,
    failure ? { text: t("could not start: {{error}}", { error: failure }), signal: "failed" } : null,
    problem ? { text: problem, signal: "failed" } : null,
  ].filter((note) => note !== null);
  return (
    <div className={cn(row, "mt-3 first:mt-0")} data-plugin-row={status.id}>
      <div className="min-w-0">
        <div className="flex items-baseline gap-2 text-xs">
          <span>{name}</span>
          <span className="font-mono text-2xs text-faint tabular-nums">
            {status.id} · v{status.version}
          </span>
        </div>
        {notes.map((note) => (
          <div
            key={note.text}
            data-signal={note.signal ?? undefined}
            className={cn("text-2xs break-words", note.signal === "failed" ? "text-danger" : note.signal === "warn" ? "text-warn-fg" : "text-faint")}
          >
            {note.text}
          </div>
        ))}
        {status.forced && <div className="text-2xs text-faint">{t("set by the server configuration")}</div>}
      </div>
      <div className="flex max-w-full shrink-0 items-center gap-2">
        <Tabs
          className="max-w-full"
          value={status.switchedOn ? "on" : "off"}
          onValueChange={(v) => {
            if (!status.forced && !toggle.isPending) toggle.mutate(v === "on");
          }}
        >
          <TabsList
            className={cn("w-40", (status.forced || !status.compatible) && "opacity-50")}
            aria-label={t("{{name}} plugin", { name })}
          >
            <TabsTab value="off" disabled={status.forced || toggle.isPending}>
              {t("disabled")}
            </TabsTab>
            <TabsTab value="on" disabled={status.forced || !status.compatible || toggle.isPending}>
              {t("enabled")}
            </TabsTab>
          </TabsList>
        </Tabs>
        <button
          type="button"
          disabled={uninstall.isPending || toggle.isPending}
          onClick={() => void askUninstall()}
          className={SETTINGS_ACTION_DANGER}
        >
          {uninstall.isPending ? t("uninstalling…") : t("uninstall")}
        </button>
      </div>
    </div>
  );
}

export function PluginsSection() {
  const { t } = useTranslation();
  const api = useApi();
  const registry = usePluginRegistry();
  const install = useMutation({ mutationFn: (file: File) => installPlugin(api, file), onSuccess: reloadPage });

  if (isCloud()) return null;

  const pickAndInstall = async () => {
    const file = await openFile([".tgz", "application/gzip", "application/x-gzip"]);
    if (!file) return;
    const yes = await requestConfirm(
      t(
        "A plugin runs inside this page with no sandbox, signed in as you: it can do anything you can do here, terminals included. Install only one you trust as much as a program you would run in this device's terminal.",
      ),
      { title: t("install {{file}}?", { file: file.name }), confirmLabel: t("install") },
    );
    if (yes) install.mutate(file);
  };

  const settings = registry.active.flatMap((plugin) =>
    (plugin.contributions.settings ?? []).map((block, index) => ({ plugin, block, key: `${plugin.id}-${index}` })),
  );
  const statuses = registry.statuses;
  return (
    <>
      <section className="border-b border-border p-3">
        <h2 data-label="" className="label-caps -mx-3 -mt-3 mb-3 flex h-9 items-center border-b border-border px-3 text-2xs text-fg">
          {t("plugins")}
        </h2>
        {registry.reloadNeeded && <ReloadNotice className="mb-3" />}
        <p className="mb-3 max-w-xl text-2xs text-faint">
          {t("Plugins installed on this instance. Turning one off hides it here and keeps its data.")}
        </p>
        <div className="mb-3 flex max-w-xl flex-wrap items-center gap-2">
          <button type="button" disabled={install.isPending} onClick={() => void pickAndInstall()} className={SETTINGS_ACTION}>
            {install.isPending ? t("installing…") : t("Install from file…")}
          </button>
          {install.isError && (
            <span data-signal="failed" className="min-w-0 text-2xs break-words text-danger">
              {install.error.message}
            </span>
          )}
        </div>
        {statuses === null ? (
          <Spinner />
        ) : statuses.length === 0 ? (
          <p className="max-w-xl text-2xs text-faint">{t("No plugins are installed on this instance.")}</p>
        ) : (
          statuses.map((status) => <PluginRow key={status.id} status={status} />)
        )}
      </section>
      {settings.map(({ plugin, block, key }) => {
        const Block = block.component;
        const title = localize(block.title, hostLocale());
        return (
          <section key={key} className="border-b border-border p-3" data-plugin={plugin.id}>
            <h2 data-label="" className="label-caps -mx-3 -mt-3 mb-3 flex h-9 items-center border-b border-border px-3 text-2xs text-fg">
              {title}
            </h2>
            <PluginBoundary host={plugin.host} name={localize(plugin.status.name, hostLocale())}>
              <Block />
            </PluginBoundary>
          </section>
        );
      })}
    </>
  );
}
