import { Select as BaseSelect } from "@base-ui-components/react/select";
import { Settings } from "lucide-react";
import { Fragment } from "react";
import { useTranslation } from "react-i18next";
import { GlyphIcon, type Glyph } from "@/components/glyph";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { cn } from "@/lib/cn";

/*
 * The new-session composer's agent chip: Claude Code and Codex, then every
 * configured ACP agent, each by its mark and name, and at the foot the way to
 * the page that manages them. An agent whose last check failed is listed in
 * red with why, and can still be picked: a check is a moment's reading, and
 * the start says what is wrong if it still is.
 */

export interface AgentChoice {
  /** `claude`, `codex` or `acp:<id>` (`agentKey`). */
  key: string;
  label: string;
  glyph: Glyph;
  group: "cli" | "acp";
  /** Why it cannot start, as its last check found: red words beside its name. */
  failing?: string | null;
}

/** The foot row's value: not an agent, the way to Settings › Agents & accounts. */
const MANAGE = "manage";

export function AgentSelect({
  value,
  choices,
  onChange,
  onManage,
}: {
  value: string;
  choices: readonly AgentChoice[];
  onChange: (key: string) => void;
  onManage: () => void;
}) {
  const { t } = useTranslation();
  const selected = choices.find((choice) => choice.key === value);
  const items = [...choices.map(({ key, label }) => ({ value: key, label })), { value: MANAGE, label: t("manage agents and accounts…") }];
  const groups = (["cli", "acp"] as const).filter((group) => choices.some((choice) => choice.group === group));
  return (
    <Select
      value={value}
      items={items}
      onValueChange={(next) => {
        if (next === MANAGE) onManage();
        else if (next !== null && next !== value) onChange(next);
      }}
    >
      <SelectTrigger
        variant="chip"
        aria-label={t("agent")}
        title={selected?.failing ? `${selected.label}: ${selected.failing}` : t("agent")}
        data-chip="agent"
        data-signal={selected?.failing ? "failed" : undefined}
        iconClassName={selected?.failing ? "text-danger" : undefined}
        className={cn(selected?.failing && "text-danger hover:text-danger data-[popup-open]:text-danger")}
      >
        <SelectValue className="max-w-48">
          {() =>
            selected ? (
              <span className="flex min-w-0 items-center gap-1.5">
                <GlyphIcon glyph={selected.glyph} />
                <span className="min-w-0 truncate">{selected.label}</span>
              </span>
            ) : (
              value
            )
          }
        </SelectValue>
      </SelectTrigger>
      <SelectContent variant="chip" className="min-w-[max(var(--anchor-width),16rem)]">
        {groups.map((group, index) => (
          <Fragment key={group}>
            {index > 0 && <BaseSelect.Separator className="my-1.5 h-px bg-border" />}
            <BaseSelect.Group>
              <BaseSelect.GroupLabel data-label="" className="label-caps px-3 pt-0.5 pb-1 text-[10px] text-muted">
                {group === "cli" ? t("CLI") : t("ACP agents")}
              </BaseSelect.GroupLabel>
              {choices
                .filter((choice) => choice.group === group)
                .map((choice) => (
                  <SelectItem key={choice.key} value={choice.key} data-signal={choice.failing ? "failed" : undefined}>
                    <span className="flex min-w-0 items-center gap-2">
                      <GlyphIcon glyph={choice.glyph} className="size-3.5" />
                      <span className="min-w-0 shrink-0 truncate">{choice.label}</span>
                      {choice.failing && <span className="ml-auto min-w-0 truncate pl-2 text-danger">{choice.failing}</span>}
                    </span>
                  </SelectItem>
                ))}
            </BaseSelect.Group>
          </Fragment>
        ))}
        <BaseSelect.Separator className="my-1.5 h-px bg-border" />
        <SelectItem value={MANAGE} data-manage-agents="" className="text-muted">
          <span className="flex min-w-0 items-center gap-2">
            <Settings aria-hidden="true" className="size-3.5 shrink-0" />
            <span className="min-w-0 truncate">{t("manage agents and accounts…")}</span>
          </span>
        </SelectItem>
      </SelectContent>
    </Select>
  );
}
