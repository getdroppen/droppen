/*
 * The entries list (`docs/a2a.md` §4): one row per A2A agent of this device —
 * its switch, name, agent and directory, the URL a client is given, and the
 * way to edit or delete it. Switching one off or deleting it cancels its
 * unfinished tasks (§6); the row's confirmation says so.
 */
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { CheckIcon, CopyIcon, PencilSimpleIcon, TrashIcon } from "@phosphor-icons/react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ApiError, errorText } from "@/api/errors";
import { useFeedSettings } from "@/app/global-feed";
import { SourceIcon } from "@/components/source-icon";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { SettingsNote } from "@/features/settings/layout";
import { useCopy } from "@/lib/clipboard";
import { cn } from "@/lib/cn";
import { shortenHome } from "@/lib/format";
import { useApi, useMachine } from "@/lib/machines/context";
import { a2aAllTasksKey, a2aEntriesKey, a2aTokensKey } from "./api";
import { EntryFormView } from "./entry-form";
import { cardUrl, isBypass, type A2aEntry } from "./model";

/** One entry. `editing` opens its form under it; the section keeps one form open at a time. */
export function EntryRow({
  entry,
  base,
  defaultTemplate,
  editing,
  onEdit,
  onDone,
}: {
  entry: A2aEntry;
  /** Where its URL starts (`cardBase`), or `null` when this page cannot say. */
  base: string | null;
  defaultTemplate: string;
  editing: boolean;
  onEdit: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const api = useApi();
  const machine = useMachine();
  const queryClient = useQueryClient();
  const homeDir = useFeedSettings()?.homeDir ?? "";
  const [confirming, setConfirming] = useState(false);
  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: a2aEntriesKey(machine) });
    void queryClient.invalidateQueries({ queryKey: a2aAllTasksKey(machine) });
  };

  const toggle = useMutation({
    mutationFn: async (enabled: boolean) => {
      const res = await api.PATCH("/api/a2a/entries/{id}", { params: { path: { id: entry.id } }, body: { enabled } });
      if (res.error) throw new ApiError(res.error);
    },
    onSuccess: refresh,
  });
  const remove = useMutation({
    mutationFn: async () => {
      const res = await api.DELETE("/api/a2a/entries/{id}", { params: { path: { id: entry.id } } });
      if (res.error) throw new ApiError(res.error);
    },
    onSuccess: () => {
      refresh();
      // It leaves every token's scope.
      void queryClient.invalidateQueries({ queryKey: a2aTokensKey(machine) });
    },
  });

  const facts = [
    entry.permission ?? t("CLI default"),
    entry.isolation === "worktree" ? t("worktree") : t("no worktree"),
    t("{{n}} at once", { n: entry.maxConcurrent }),
    ...(entry.allowRemoteAnswers ? [] : [t("questions answered in Droppen only")]),
  ];

  return (
    <div data-a2a-entry={entry.id} className="flex flex-col gap-1 border-b border-border py-2">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <Switch
          on={entry.enabled}
          blocked={toggle.isPending}
          label={entry.enabled ? t("enabled") : t("disabled")}
          title={entry.enabled ? t("takes tasks — switching it off cancels its unfinished ones") : t("takes no tasks")}
          onChange={(on) => toggle.mutate(on)}
          className="-ml-1 font-sans-text text-xs font-medium"
        />
        <span className="min-w-0 truncate text-[13px] font-medium text-fg">{entry.name}</span>
        <span className="font-mono text-2xs text-faint">{entry.id}</span>
        {entry.source !== "tck" && <SourceIcon source={entry.source} acpAgent={entry.acpAgent} className="text-muted" />}
        <span className="min-w-0 max-w-64 truncate font-mono text-2xs text-faint" title={entry.cwd}>
          {shortenHome(entry.cwd, homeDir)}
        </span>
        <div className="ml-auto flex items-center gap-1">
          <Button
            variant="ghost"
            title={t("edit entry")}
            aria-label={t("edit entry")}
            aria-expanded={editing}
            onClick={editing ? onDone : onEdit}
            className={cn("size-7 px-0", editing && "bg-hover text-fg")}
          >
            <PencilSimpleIcon className="size-3.5" />
          </Button>
          <Button
            variant="ghost"
            title={t("delete entry")}
            aria-label={t("delete entry")}
            onClick={() => setConfirming(true)}
            data-signal="failed"
            className="size-7 px-0 hover:text-danger"
          >
            <TrashIcon className="size-3.5" />
          </Button>
        </div>
      </div>
      <CardUrl base={base} id={entry.id} />
      <div className={cn("font-mono text-2xs text-faint", isBypass(entry.permission) && "text-danger")} data-signal={isBypass(entry.permission) ? "failed" : undefined}>
        {facts.join(" · ")}
      </div>
      {confirming && (
        <div data-a2a-entry-confirm="" className="flex flex-wrap items-center gap-2 pt-1">
          <SettingsNote tone="warn" mono={false} className="text-xs">
            {t("Delete {{name}}? Its unfinished tasks are cancelled; their sessions stay.", { name: entry.name })}
          </SettingsNote>
          <Button variant="danger" data-signal="failed" disabled={remove.isPending} onClick={() => remove.mutate()}>
            {remove.isPending ? t("removing…") : t("delete")}
          </Button>
          <Button onClick={() => setConfirming(false)}>{t("cancel")}</Button>
        </div>
      )}
      {toggle.isError && <SettingsNote tone="danger">{errorText(toggle.error)}</SettingsNote>}
      {remove.isError && <SettingsNote tone="danger">{errorText(remove.error)}</SettingsNote>}
      {editing && <EntryFormView initial={entry} defaultTemplate={defaultTemplate} onDone={onDone} />}
    </div>
  );
}

/**
 * The URL an A2A client is given for the entry, to copy. Its Agent Card is
 * under it (`/.well-known/agent-card.json`). With no base this page can name
 * (it is not the device's own page), the path alone, for the device's address.
 */
export function CardUrl({ base, id }: { base: string | null; id: string }) {
  const { t } = useTranslation();
  const { copied, copy } = useCopy();
  const url = cardUrl(base, id);
  return (
    <div data-a2a-card-url={url} className="flex min-w-0 items-center gap-1.5">
      <span className="shrink-0 text-xs text-muted">{t("Agent Card URL")}</span>
      <code className="min-w-0 truncate font-mono text-2xs text-fg" title={`${url}/.well-known/agent-card.json`}>
        {url}
      </code>
      <Button
        variant="ghost"
        aria-label={t("copy the Agent Card URL")}
        title={copied ? t("copied") : t("copy the Agent Card URL")}
        onClick={() => copy(url)}
        className="size-6 shrink-0 px-0"
      >
        {copied ? <CheckIcon className="size-3.5" /> : <CopyIcon className="size-3.5" />}
      </Button>
      {base === null && <span className="font-mono text-2xs text-faint">{t("after the device's own address")}</span>}
    </div>
  );
}
