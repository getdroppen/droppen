import { localize } from "@droppen/plugin-sdk";
import { Link } from "@tanstack/react-router";
import { Puzzle } from "lucide-react";
import { useTranslation } from "react-i18next";
import { RailHint } from "@/components/ui/rail-hint";
import { PluginIcon } from "./boundary";
import { hostLocale } from "./host";
import { usePluginRegistry } from "./registry";
import { strandedStatuses } from "./statuses";

/**
 * The running plugins' navigation entries, in the shell's rail style — then
 * one plain entry, marked, for each plugin switched on that this host cannot
 * run (`docs/plugins/runtime.md` §6). Its own icon and label are in code that
 * is not loaded, so it shows a generic one and its name; the entry leads to
 * its page, which says why.
 */
export function PluginNav({ className }: { className: string }) {
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
          <Puzzle className="size-4 text-warn-fg" strokeWidth={1.5} />
          <span className="absolute -top-0.5 -right-0.5 size-[3px] bg-warn" />
        </span>
      </RailHint>
    );
  });
  return [...running, ...stranded];
}
