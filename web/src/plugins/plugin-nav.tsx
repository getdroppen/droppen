import { localize } from "@droppen/plugin-sdk";
import { Link } from "@tanstack/react-router";
import { PuzzlePieceIcon } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import { MachineLink } from "@/app/machine-route";
import { RailHint } from "@/components/ui/rail-hint";
import { isCloud } from "@/lib/hub/config";
import type { MachineKey } from "@/lib/machines/key";
import { PluginIcon } from "./boundary";
import { hostLocale } from "./host";
import { useMachinePlugins, usePluginRegistry } from "./registry";
import { cloudRunningStatuses, strandedStatuses } from "./statuses";

/**
 * The running plugins' navigation entries, in the shell's rail style.
 *
 * Locally, shows every activated plugin's nav entries plus one marked entry for
 * each stranded plugin (`docs/plugins/runtime.md` §6). On `droppen.app`, shows
 * the selected machine's cloud-enabled plugins (`docs/plugins/cloud.md` §6),
 * linking to `/p/<plugin>/…` (or `/m/<machine>/p/<plugin>/…`).
 */
export function PluginNav({ className, machine }: { className: string; machine: MachineKey }) {
  if (isCloud()) return <CloudPluginNav className={className} machine={machine} />;
  return <LocalPluginNav className={className} />;
}

function LocalPluginNav({ className }: { className: string }) {
  const { t } = useTranslation();
  const { active, statuses } = usePluginRegistry();
  const locale = hostLocale();
  const running = active.flatMap((plugin) =>
    (plugin.contributions.nav ?? []).map((item, index) => {
      const label = localize(item.label, locale);
      return (
        <RailHint
          key={`${plugin.id}-${index}`}
          label={label}
          render={
            <Link
              to="/p/$plugin/$"
              params={{ plugin: plugin.id, _splat: (item.path ?? "").replace(/^\/+/, "") }}
              aria-label={label}
              data-plugin={plugin.id}
              className={className}
              activeProps={{ className: "!bg-fg !text-bg" }}
            />
          }
        >
          <PluginIcon host={plugin.host} icon={item.icon} className="size-4" />
        </RailHint>
      );
    }),
  );
  const stranded = strandedStatuses(statuses).map((status) => {
    const label = t("{{name}} does not work with this version of Droppen", { name: localize(status.name, locale) });
    return (
      <RailHint
        key={`${status.id}-stranded`}
        label={label}
        render={
          <Link
            to="/p/$plugin/$"
            params={{ plugin: status.id, _splat: "" }}
            aria-label={label}
            data-plugin={status.id}
            data-plugin-stranded=""
            className={className}
            activeProps={{ className: "!bg-fg !text-bg" }}
          />
        }
      >
        <span data-signal="warn" className="relative flex items-center justify-center">
          <PuzzlePieceIcon className="size-4 text-warn-fg" />
          <span className="absolute -top-0.5 -right-0.5 size-[3px] bg-warn" />
        </span>
      </RailHint>
    );
  });
  return [...running, ...stranded];
}

function CloudPluginNav({ className, machine }: { className: string; machine: MachineKey }) {
  const { active, statuses, loading } = useMachinePlugins(machine);
  const locale = hostLocale();
  const activatedIds = new Set(active.map((p) => p.id));

  const activatedEntries = active.flatMap((plugin) =>
    (plugin.contributions.nav ?? []).map((item, index) => {
      const label = localize(item.label, locale);
      return (
        <RailHint
          key={`${machine}-${plugin.id}-${index}`}
          label={label}
          render={
            <MachineLink
              machine={machine}
              to="/p/$plugin/$"
              params={{ plugin: plugin.id, _splat: (item.path ?? "").replace(/^\/+/, "") }}
              aria-label={label}
              data-plugin={plugin.id}
              className={className}
              activeProps={{ className: "!bg-fg !text-bg" }}
            />
          }
        >
          <PuzzlePieceIcon className="size-4" />
        </RailHint>
      );
    }),
  );

  // While a cloud-enabled plugin's package is still activating, show a placeholder
  // entry so the rail stays steady instead of popping in after download.
  const pendingEntries = cloudRunningStatuses(statuses)
    .filter((status) => !activatedIds.has(status.id) && loading.includes(status.id))
    .map((status) => {
      const label = localize(status.name, locale);
      return (
        <RailHint
          key={`${machine}-${status.id}-pending`}
          label={label}
          render={
            <MachineLink
              machine={machine}
              to="/p/$plugin/$"
              params={{ plugin: status.id, _splat: "" }}
              aria-label={label}
              data-plugin={status.id}
              className={className}
              activeProps={{ className: "!bg-fg !text-bg" }}
            />
          }
        >
          <PuzzlePieceIcon className="size-4 opacity-60" />
        </RailHint>
      );
    });

  return [...activatedEntries, ...pendingEntries];
}
