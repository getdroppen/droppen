/*
 * An A2A entry's form (`docs/a2a.md` §4): create and edit are one form, as
 * the scheduled task's are, so the edit form cannot fall behind when a field
 * is added. The pickers are the new-session view's own — agent, directory
 * (recent ones and the folder browser), account, model, effort, permission —
 * at the settings page's 32px.
 *
 * What a new entry starts with is the model's (`newEntryForm`): a worktree per
 * context, edits accepted and nothing bypassed, questions answerable over
 * A2A, one task at a time, and off until it is switched on in the list.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CaretDownIcon, CircleNotchIcon, FolderSimpleIcon } from "@phosphor-icons/react";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { ApiError, errorText } from "@/api/errors";
import { useFeedSessions, useFeedSettings } from "@/app/global-feed";
import { useMachineNavigate } from "@/app/machine-route";
import { cliGlyph } from "@/components/source-icon";
import { Button } from "@/components/ui/button";
import { CheckboxField } from "@/components/ui/checkbox-field";
import { Combobox } from "@/components/ui/combobox";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { SelectField } from "@/components/ui/select";
import { Tabs, TabsList, TabsTab } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { AccountRows } from "@/features/accounts/account-picker";
import { accountsForProvider, localLoginLabel, localLoginOf, useAccounts } from "@/features/accounts/use-accounts";
import { useAcpAgents } from "@/features/sessions/acp-agent";
import { acpAgentGlyph } from "@/features/sessions/acp-agent-icons";
import { AgentSelect, type AgentChoice } from "@/features/sessions/agent-select";
import { effortsOf, MODEL_FALLBACK, ModelSelect } from "@/features/sessions/composer-controls";
import { DirBrowser } from "@/features/sessions/dir-browser";
import { inTempDir, recentDirectories } from "@/features/sessions/new-session";
import { agentKey, parseAgentKey } from "@/features/sessions/new-session-flow";
import { agentsSearch, TAB_NAME } from "@/features/settings/agents/agents-tabs";
import { SettingField, SettingsColumn, SettingsHint, SettingsNote } from "@/features/settings/layout";
import { cn } from "@/lib/cn";
import { shortenHome } from "@/lib/format";
import { useApi, useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";
import { a2aEntriesKey } from "./api";
import {
  createBody,
  entryFormOf,
  ENTRY_PROBLEM_WORD,
  entryProblems,
  isBypass,
  MAX_CONCURRENT,
  newEntryForm,
  parseOrigins,
  permissionOptions,
  updateBody,
  withName,
  withSource,
  type A2aEntry,
  type EntryForm,
  type RunAccount,
} from "./model";

/** The two CLIs, by their marks and names; then each configured ACP agent (as the new-session view lists them). */
const CLI_CHOICES: AgentChoice[] = (["claude", "codex"] as const).map((source) => ({
  key: source,
  label: TAB_NAME[source],
  glyph: cliGlyph(source),
  group: "cli",
}));

/** The 32px field a composer's bordered select becomes in a settings form. */
const FIELD_SELECT = "h-8 w-full px-3 text-[13px] text-fg max-md:h-10 max-md:text-base";

/**
 * The entry form: `initial` to edit one, none to make one. `defaultTemplate`
 * is the server's, shown where the entry has no template of its own.
 */
export function EntryFormView({
  initial,
  defaultTemplate,
  onDone,
}: {
  initial?: A2aEntry;
  defaultTemplate: string;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const api = useApi();
  const machine = useMachine();
  const queryClient = useQueryClient();
  const manage = useMachineNavigate();
  const [form, setForm] = useState<EntryForm>(() => (initial ? entryFormOf(initial) : newEntryForm()));
  const [advanced, setAdvanced] = useState(() => !!initial?.template);
  const set = (patch: Partial<EntryForm>) => setForm((now) => ({ ...now, ...patch }));

  const { data: acpList } = useAcpAgents();
  const acpAgents = acpList?.agents ?? [];
  const choices: AgentChoice[] = [
    ...CLI_CHOICES,
    ...acpAgents.map((agent): AgentChoice => ({ key: agentKey({ source: "acp", acpAgent: agent.id }), label: agent.name, glyph: acpAgentGlyph(agent.id), group: "acp" })),
    // An entry's agent since removed from the list still shows as itself.
    ...(form.source === "acp" && form.acpAgent && !acpAgents.some((agent) => agent.id === form.acpAgent)
      ? [{ key: agentKey({ source: "acp", acpAgent: form.acpAgent }), label: form.acpAgent, glyph: acpAgentGlyph(form.acpAgent), group: "acp" as const }]
      : []),
  ];
  const cli = form.source === "acp" ? null : form.source;

  const problems = entryProblems(form);
  // Said under the form: an empty id is the name's to fill in, so it is not
  // named until either has been typed.
  const said = problems.filter((problem) => problem !== "id" || form.id !== "" || form.name.trim() !== "");
  const save = useMutation({
    mutationFn: async () => {
      const res = initial
        ? await api.PATCH("/api/a2a/entries/{id}", { params: { path: { id: initial.id } }, body: updateBody(form) })
        : await api.POST("/api/a2a/entries", { body: createBody(form) });
      if (res.error) throw new ApiError(res.error);
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: a2aEntriesKey(machine) });
      onDone();
    },
  });

  return (
    <SettingsColumn data-a2a-entry-form={initial?.id ?? ""} className="mt-2 flex w-full flex-col gap-3 border border-border p-3">
      <div className="grid gap-3 @min-[36rem]/settings:grid-cols-2">
        <SettingField label={t("name")}>
          <Input
            value={form.name}
            onChange={(e) => {
              const name = e.target.value;
              setForm((now) => withName(now, name));
            }}
            placeholder={t("e.g. dont-do tasks")}
          />
        </SettingField>
        <SettingField label={t("id")} hint={initial ? t("· in its URL, cannot change") : t("· in its URL")}>
          <Input
            value={form.id}
            readOnly={!!initial}
            onChange={(e) => set({ id: e.target.value.toLowerCase(), idTyped: true })}
            spellCheck={false}
            autoComplete="off"
            placeholder="dont-do"
            className={cn("font-mono", initial && "bg-raised text-muted")}
          />
        </SettingField>
      </div>
      <SettingField label={t("description")} hint={t("· on its Agent Card")}>
        <Input value={form.description} onChange={(e) => set({ description: e.target.value })} placeholder={t("what to hand this agent")} />
      </SettingField>

      <SettingField label={t("agent")}>
        <AgentSelect
          variant="field"
          value={agentKey({ source: form.source, acpAgent: form.acpAgent })}
          choices={choices}
          onChange={(key) => {
            const next = parseAgentKey(key);
            if (next) setForm((now) => withSource(now, next.source, next.acpAgent));
          }}
          onManage={() => void manage({ to: "/settings", search: agentsSearch(form.source) })}
        />
      </SettingField>

      <DirectoryField cwd={form.cwd} onPick={(cwd) => set({ cwd })} />

      {cli && <CliFields form={form} cli={cli} set={set} />}

      <div className="grid gap-3 @min-[36rem]/settings:grid-cols-2">
        {/* Not a `<label>`: clicking its words would pick the first tab. */}
        <div className="flex flex-col gap-1">
          <span className="text-xs text-muted">
            {t("isolation")} {form.isolation === "worktree" ? t("· a git worktree per conversation") : t("· its sessions share the directory")}
          </span>
          <Tabs value={form.isolation} onValueChange={(value) => set({ isolation: value as EntryForm["isolation"] })}>
            <TabsList className="w-full" aria-label={t("isolation")}>
              <TabsTab value="worktree">{t("worktree")}</TabsTab>
              <TabsTab value="none">{t("in the directory")}</TabsTab>
            </TabsList>
          </Tabs>
        </div>
        <SettingField label={t("tasks at once")}>
          <SelectField
            aria-label={t("tasks at once")}
            value={String(form.maxConcurrent)}
            onValueChange={(value) => set({ maxConcurrent: Number(value) })}
            options={Array.from({ length: MAX_CONCURRENT }, (_, i) => ({ value: String(i + 1), label: String(i + 1) }))}
          />
        </SettingField>
      </div>

      <CheckboxField
        checked={form.allowRemoteAnswers}
        onCheckedChange={(checked) => set({ allowRemoteAnswers: checked })}
        className="text-xs"
        labelClassName="text-xs text-muted"
        label={t("Let the client answer a session's questions. Off, a question waits for you in Droppen.")}
      />

      <SettingField label={t("push origins")} hint={t("· one per line; a task's updates may be pushed only there")}>
        <Textarea
          value={form.pushOrigins}
          onChange={(e) => set({ pushOrigins: e.target.value })}
          rows={2}
          spellCheck={false}
          placeholder="https://dont-do.example.com"
          className="font-mono"
        />
      </SettingField>

      {/* TODO(M2, docs/a2a.md §7): choose the a2a devices from the device chain. Read-only until then, and hidden while none is listed. */}
      {form.allowedDevices.length > 0 && (
        <div data-a2a-allowed-devices="" className="flex flex-col gap-1">
          <span className="text-xs text-muted">{t("devices that may call it")}</span>
          <span className="font-mono text-2xs break-all text-fg">{form.allowedDevices.join(", ")}</span>
        </div>
      )}

      <div className="flex flex-col gap-1">
        <button
          type="button"
          aria-expanded={advanced}
          onClick={() => setAdvanced(!advanced)}
          className="-ml-1.5 flex h-7 w-fit items-center gap-1 px-1.5 text-xs text-muted hover:bg-hover hover:text-fg"
        >
          <CaretDownIcon className={cn("size-3 transition-transform", !advanced && "-rotate-90")} />
          {t("prompt template (advanced)")}
        </button>
        {advanced && (
          <>
            <Textarea
              value={form.template}
              onChange={(e) => set({ template: e.target.value })}
              rows={7}
              spellCheck={false}
              placeholder={defaultTemplate}
              className="font-mono text-xs"
            />
            <SettingsHint>
              {/* The placeholders go in as a value, so the translation never has to carry them. */}
              {t("Empty uses the default shown. {{names}} are filled in; the request is a client's words, so keep it between the fences.", {
                names: "{{caller}}, {{fence}}, {{request}}",
              })}
            </SettingsHint>
          </>
        )}
      </div>

      {said.length > 0 && (
        <ul data-a2a-problems="" className="flex flex-col gap-0.5">
          {said.map((problem) => (
            <li key={problem}>
              <SettingsNote tone={problem === "bypassWithDevices" ? "danger" : "muted"} mono={false} className="text-xs">
                {t(ENTRY_PROBLEM_WORD[problem])}
              </SettingsNote>
            </li>
          ))}
        </ul>
      )}
      {save.isError && <SettingsNote tone="danger">{errorText(save.error)}</SettingsNote>}
      {!initial && <SettingsHint>{t("A new entry is saved switched off: switch it on in the list when you are ready.")}</SettingsHint>}

      <div className="flex items-center gap-1.5">
        <Button variant="primary" disabled={problems.length > 0 || save.isPending} onClick={() => save.mutate()}>
          {save.isPending && <CircleNotchIcon className="size-3 animate-spin" />}
          {initial ? t("save") : t("add entry")}
        </Button>
        <Button onClick={onDone}>{t("cancel")}</Button>
      </div>
    </SettingsColumn>
  );
}

/** The directory: typed (with the device's recent ones offered), browsed to, or one of the recent ones. */
function DirectoryField({ cwd, onPick }: { cwd: string; onPick: (cwd: string) => void }) {
  const { t } = useTranslation();
  const settings = useFeedSettings();
  const homeDir = settings?.homeDir ?? "";
  const sessions = useFeedSessions();
  const [browsing, setBrowsing] = useState(false);
  // The new-session view's list: where this device's sessions ran, newest
  // start first, leaving out scratch space under the temporary directories.
  const tempDirs = settings?.tempDirs;
  const recent = useMemo(() => recentDirectories(sessions, 50, (dir) => inTempDir(dir, tempDirs ?? [])), [sessions, tempDirs]);
  return (
    <div className="flex flex-col gap-1">
      <span className="text-xs text-muted">{t("working directory")}</span>
      <div className="flex items-center gap-1">
        <div className="min-w-0 flex-1">
          <Combobox value={cwd} onValueChange={onPick} items={recent.map((dir) => dir.cwd)} placeholder="/path/to/project" />
        </div>
        <Button
          title={t("browse folders")}
          aria-label={t("browse folders")}
          aria-expanded={browsing}
          onClick={() => setBrowsing(!browsing)}
          className={cn("size-8 px-0 max-md:size-10", browsing && "bg-hover")}
        >
          <FolderSimpleIcon className="size-3.5" />
        </Button>
      </div>
      {recent.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {recent.slice(0, 6).map((dir) => (
            <button
              key={dir.cwd}
              type="button"
              title={dir.cwd}
              aria-pressed={dir.cwd === cwd.trim()}
              onClick={() => onPick(dir.cwd)}
              className={cn(
                "max-w-full truncate border px-1.5 py-0.5 font-mono text-2xs",
                dir.cwd === cwd.trim() ? "border-fg text-fg" : "border-border text-muted hover:bg-hover hover:text-fg",
              )}
            >
              {shortenHome(dir.cwd, homeDir)}
            </button>
          ))}
        </div>
      )}
      {browsing && (
        <DirBrowser
          start={cwd.trim()}
          onPick={(path) => {
            onPick(path);
            setBrowsing(false);
          }}
        />
      )}
    </div>
  );
}

/** Account, model, effort and permission: a CLI's alone; an ACP agent chooses its own. */
function CliFields({ form, cli, set }: { form: EntryForm; cli: "claude" | "codex"; set: (patch: Partial<EntryForm>) => void }) {
  const { t } = useTranslation();
  const api = useApi();
  const machine = useMachine();
  // The models of the account it runs under, as the scheduled task's form asks:
  // the composer's own cache entry, so a page that has asked pays nothing.
  const modelAccount = cli === "claude" && form.account.kind === "id" ? form.account.id : null;
  const { data: modelData } = useQuery({
    queryKey: mq(machine, "models", cli, modelAccount),
    queryFn: async () =>
      (await api.GET("/api/models", { params: { query: { source: cli, ...(modelAccount ? { accountId: modelAccount } : {}) } } })).data,
    staleTime: 5 * 60_000,
  });
  const models = modelData?.models.length ? modelData.models : MODEL_FALLBACK[cli];
  const customModel = form.model !== "" && !models.some((m) => m.id === form.model) ? form.model : null;
  // A model with no ladder refuses `--effort`: the rung is hidden, and none is sent.
  const efforts = effortsOf(models.find((m) => m.id === form.model), cli);
  const effort = efforts.includes(form.effort) ? form.effort : "";
  const permissions = permissionOptions(cli, form.allowedDevices, form.permission);
  return (
    <div className="grid gap-3 @min-[36rem]/settings:grid-cols-2">
      <SettingField label={t("account")}>
        <RunAccountPicker provider={cli} value={form.account} onChange={(account) => set({ account, model: "", effort: "" })} />
      </SettingField>
      <SettingField label={t("model")}>
        <ModelSelect
          models={models}
          value={form.model}
          customId={customModel}
          onChange={(model) => set({ model })}
          className={FIELD_SELECT}
          leading={{ value: "", label: t("CLI default") }}
        />
      </SettingField>
      {efforts.length > 0 && (
        <SettingField label={t("effort")}>
          <SelectField
            aria-label={t("effort")}
            value={effort}
            onValueChange={(value) => set({ effort: value })}
            options={[{ value: "", label: t("CLI default") }, ...efforts.map((e) => ({ value: e, label: e }))]}
          />
        </SettingField>
      )}
      <SettingField label={cli === "codex" ? t("sandbox") : t("permission")}>
        <SelectField
          aria-label={cli === "codex" ? t("sandbox") : t("permission")}
          value={form.permission}
          onValueChange={(value) => set({ permission: value })}
          options={[{ value: "", label: t("CLI default") }, ...permissions.map((p) => ({ value: p, label: p }))]}
        />
        {isBypass(form.permission) && (
          <SettingsNote tone="danger" mono={false} className="mt-1 block text-xs">
            {t("No guard rails: a client with a token for this entry can run any command here, as you.")}
          </SettingsNote>
        )}
      </SettingField>
    </div>
  );
}

/** What the account picker's rows are given while the entry follows the default: a value no row has. */
const NO_ROW = "\u0000default";

/**
 * Which login the entry's sessions run on: the instance's default at the time
 * (what a start that names no account gets), the CLI's own local login, or
 * one stored account. The rows are the account picker's, with their quota.
 */
function RunAccountPicker({ provider, value, onChange }: { provider: "claude" | "codex"; value: RunAccount; onChange: (account: RunAccount) => void }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const { data: list } = useAccounts();
  const accounts = accountsForProvider(list?.accounts ?? [], provider);
  const label =
    value.kind === "default"
      ? t("the default account")
      : value.kind === "local"
        ? localLoginLabel(localLoginOf(list, provider), t)
        : (accounts.find((a) => a.id === value.id)?.label ?? value.id);
  const pick = (account: RunAccount) => {
    onChange(account);
    setOpen(false);
  };
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger
        aria-label={t("account")}
        className="group flex h-8 w-full min-w-0 cursor-pointer items-center justify-between gap-2 border border-border bg-bg px-3 text-left text-[13px] text-fg hover:border-border-strong data-[popup-open]:border-fg max-md:h-10 max-md:text-base"
      >
        <span className="min-w-0 truncate">{label}</span>
        <CaretDownIcon className="size-3.5 shrink-0 text-faint transition-transform duration-150 ease-out group-data-[popup-open]:rotate-180" />
      </PopoverTrigger>
      <PopoverContent side="bottom" className="max-h-[50vh] w-72 max-w-[calc(100vw-2rem)] overflow-y-auto">
        <button
          type="button"
          data-run-account="default"
          disabled={value.kind === "default"}
          onClick={() => pick({ kind: "default" })}
          className={cn(
            "mb-0.5 flex w-full flex-col gap-0.5 px-1.5 py-1 text-left font-mono text-2xs",
            value.kind === "default" ? "bg-hover text-fg" : "text-fg hover:bg-hover",
          )}
        >
          <span>{t("the default account")}</span>
          <span className="text-[10px] text-faint">{t("whichever a new session would start on")}</span>
        </button>
        <AccountRows
          accounts={accounts}
          value={value.kind === "default" ? NO_ROW : value.kind === "local" ? null : value.id}
          allowLocal
          provider={provider}
          onSelect={(id) => pick(id === null ? { kind: "local" } : { kind: "id", id })}
        />
      </PopoverContent>
    </Popover>
  );
}
