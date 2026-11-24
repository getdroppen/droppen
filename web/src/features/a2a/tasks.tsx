/*
 * The A2A tasks (`GET /api/a2a/tasks`, `docs/a2a.md` §3.2): newest change
 * first, filtered by entry and state, each with who sent it, through which
 * entry, when, and the session it runs in once that is known. Asked again
 * every ten seconds while the page is open.
 */
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { errorText } from "@/api/errors";
import { MachineLink } from "@/app/machine-route";
import { SelectField } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { SettingField, SettingsNote } from "@/features/settings/layout";
import { cn } from "@/lib/cn";
import { formatRelative, formatTime } from "@/lib/format";
import { useA2aTasks } from "./api";
import { callerLabel, TASK_STATE_WORD, TASK_STATES, taskSignal, type A2aEntry, type A2aTask } from "./model";

/** The colour of each signal, for the state's word (`taskSignal`). */
const SIGNAL_TEXT = { working: "text-ok", warn: "text-warn-fg", failed: "text-danger" } as const;

export function TaskList({ entries }: { entries: readonly A2aEntry[] }) {
  const { t } = useTranslation();
  const [entry, setEntry] = useState("");
  const [state, setState] = useState("");
  const { data, error, isPending } = useA2aTasks(entry, state);
  const tasks = data?.tasks ?? [];
  const nameOf = (id: string) => entries.find((e) => e.id === id)?.name ?? id;
  return (
    <div data-a2a-tasks="">
      <div className="grid gap-3 @min-[36rem]/settings:grid-cols-2">
        <SettingField label={t("entry")}>
          <SelectField
            aria-label={t("entry")}
            value={entry}
            onValueChange={setEntry}
            options={[{ value: "", label: t("all entries") }, ...entries.map((e) => ({ value: e.id, label: e.name }))]}
          />
        </SettingField>
        <SettingField label={t("state")}>
          <SelectField
            aria-label={t("state")}
            value={state}
            onValueChange={setState}
            options={[{ value: "", label: t("all states") }, ...TASK_STATES.map((s) => ({ value: s, label: t(TASK_STATE_WORD[s]) }))]}
          />
        </SettingField>
      </div>
      <div className="mt-2">
        {isPending && <Spinner />}
        {error && <SettingsNote tone="danger">{errorText(error)}</SettingsNote>}
        {data && tasks.length === 0 && <p className="py-2 text-xs text-muted">{entry || state ? t("no tasks match") : t("no tasks yet")}</p>}
        {tasks.map((task) => (
          <TaskRow key={task.id} task={task} entryName={nameOf(task.entry)} />
        ))}
      </div>
    </div>
  );
}

function TaskRow({ task, entryName }: { task: A2aTask; entryName: string }) {
  const { t } = useTranslation();
  const signal = taskSignal(task.state);
  const caller = callerLabel(task.caller);
  return (
    <div data-a2a-task={task.id} data-state={task.state} className="flex flex-col gap-0.5 border-b border-border py-2">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5">
        <span data-signal={signal ?? undefined} className={cn("shrink-0 font-mono text-2xs", signal ? SIGNAL_TEXT[signal] : "text-muted")}>
          {t(TASK_STATE_WORD[task.state])}
        </span>
        <span className="min-w-0 truncate text-[13px] text-fg">{entryName}</span>
        <span className="min-w-0 truncate font-mono text-2xs text-muted" title={task.caller}>
          {caller.kind === "device" ? t("device {{id}}", { id: caller.name }) : caller.name}
        </span>
        <span className="ml-auto shrink-0 font-mono text-2xs text-faint" title={formatTime(task.updatedAt)}>
          {t("updated {{when}}", { when: formatRelative(task.updatedAt) })}
        </span>
      </div>
      <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5 font-mono text-2xs text-faint">
        <span title={formatTime(task.createdAt)}>{t("created {{when}}", { when: formatRelative(task.createdAt) })}</span>
        {task.branch && <span className="min-w-0 truncate">{task.branch}</span>}
        {task.sessionId && task.source !== "tck" && (
          <MachineLink
            to="/sessions/$source/$id"
            params={{ source: task.source, id: task.sessionId }}
            data-a2a-session={task.sessionId}
            className="text-fg underline underline-offset-2 hover:text-muted"
          >
            {t("open session")}
          </MachineLink>
        )}
      </div>
      {task.statusText && <p className="line-clamp-2 text-xs text-muted">{task.statusText}</p>}
    </div>
  );
}
