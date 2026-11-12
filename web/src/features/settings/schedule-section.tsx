import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { SourceIcon } from "@/components/source-icon";
import { FolderSimpleIcon, CircleNotchIcon, PencilSimpleIcon, PlusIcon, TrashIcon } from "@phosphor-icons/react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "@/api/schema";
import { useFeedSettings } from "@/app/global-feed";
import { Button } from "@/components/ui/button";
import { Combobox } from "@/components/ui/combobox";
import { CheckboxField } from "@/components/ui/checkbox-field";
import { Input } from "@/components/ui/input";
import { SelectField } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsList, TabsTab } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { localLoginHint } from "@/features/accounts/account-picker";
import { accountsForProvider, localLoginOf, localLoginSignedOut, useAccounts, useStartAccount } from "@/features/accounts/use-accounts";
import {
  checkCron,
  describeCron,
  DEFAULT_PRESET,
  matchPreset,
  nextOccurrences,
  presetExpr,
  PRESETS,
  type CronPreset,
  type PresetState,
} from "@/features/schedule/cron-model";
import {
  effortsOf,
  MODEL_FALLBACK,
  ModelSelect,
  PERMISSION_OPTIONS,
  type CliSource,
} from "@/features/sessions/composer-controls";
import { DirBrowser } from "@/features/sessions/dir-browser";
import { cn } from "@/lib/cn";
import { formatUntil, shortenHome } from "@/lib/format";
import { Sheet } from "@/components/sheet";
import {
  SettingField,
  SettingsColumn,
  SettingsHint,
  SettingsNote,
} from "@/features/settings/layout";
import { InlineEdit } from "@/features/settings/inline-edit";
import { useApi, useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";
import type { MachineKey } from "@/lib/machines/key";

type ScheduledTask = components["schemas"]["ScheduledTask"];
type TaskStatus = components["schemas"]["TaskStatus"];
// The agents a schedule can run: the ACP agent is not one of them (yet).
type Source = CliSource;

const scheduledTasksKey = (machine: MachineKey) => mq(machine, "scheduled-tasks");

/** Sunday first, because `Date.getDay` and cron both number it zero. */
const WEEKDAY_KEYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/**
 * How loudly the last outcome reads.
 *
 * A skip is not a failure — the scheduler decided not to start a session and
 * said why — so it is amber rather than red; the two red states are the ones
 * where the user asked for a run and did not get one.
 */
const STATUS_TONE: Record<NonNullable<TaskStatus>, string> = {
  ok: "text-ok",
  skippedBusy: "text-warn-fg",
  skippedMissed: "text-warn-fg",
  failed: "text-danger",
  invalidExpression: "text-danger",
};

function isLoud(status: TaskStatus): boolean {
  return status === "failed" || status === "invalidExpression";
}

function hhmm(hour: number, minute: number): string {
  return `${String(hour).padStart(2, "0")}:${String(minute).padStart(2, "0")}`;
}

function parseHhmm(text: string): { hour: number; minute: number } | null {
  const parts = /^(\d{1,2}):(\d{2})$/.exec(text);
  if (!parts) return null;
  const hour = Number(parts[1] ?? "");
  const minute = Number(parts[2] ?? "");
  if (hour > 23 || minute > 59) return null;
  return { hour, minute };
}

function clampNumber(text: string, min: number, max: number, fallback: number): number {
  const n = Number(text);
  if (!Number.isFinite(n)) return fallback;
  return Math.min(max, Math.max(min, Math.trunc(n)));
}

/** The dot that says what happened last time, and carries the detail in its title. */
function StatusDot({ task }: { task: ScheduledTask }) {
  const { t } = useTranslation();
  const label = ((): string => {
    switch (task.lastStatus) {
      case "ok":
        return t("last run started a session");
      case "skippedBusy":
        return t("skipped: the session the previous run started was still working");
      case "skippedMissed":
        return t("skipped: the time passed while the server was not running");
      case "failed":
        return t("the last run could not start a session");
      case "invalidExpression":
        return t("the server could not read this expression");
      default:
        return t("has not run yet");
    }
  })();
  const tone = task.lastStatus ? STATUS_TONE[task.lastStatus] : "text-faint";
  return (
    <span
      title={task.lastError ? `${label} — ${task.lastError}` : label}
      aria-label={label}
      className={cn("size-1.5 shrink-0 rounded-full bg-current", tone, !task.lastStatus && "opacity-40")}
    />
  );
}

function TaskName({ task }: { task: ScheduledTask }) {
  const machine = useMachine();
  const api = useApi();
  const { t } = useTranslation();
  const queryClient = useQueryClient();

  const rename = useMutation({
    mutationFn: async (name: string) => {
      const res = await api.PATCH("/api/scheduled-tasks/{id}", {
        params: { path: { id: task.id } },
        body: { name },
      });
      if (res.error) throw new Error(res.error.error);
      return res.data;
    },
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: scheduledTasksKey(machine) }),
  });

  return (
    <InlineEdit
      value={task.name}
      title={t("rename task")}
      onCommit={(name) => rename.mutateAsync(name)}
    />
  );
}

/** The expression in words, in the current language, or an admission that it is not. */
function ScheduleSummary({ expr }: { expr: string }) {
  const { t } = useTranslation();
  const described = describeCron(expr);
  if (!described) return t("this expression cannot be previewed here");
  switch (described.kind) {
    case "everyMinute":
      return t("every minute");
    case "everyNMinutes":
      return t("every {{n}} minutes", { n: described.n });
    case "hourly":
      return t("every hour at :{{minute}}", { minute: String(described.minute).padStart(2, "0") });
    case "everyNHours":
      return t("every {{n}} hours, at :{{minute}}", {
        n: described.n,
        minute: String(described.minute).padStart(2, "0"),
      });
    case "daily":
      return t("every day at {{time}}", { time: hhmm(described.hour, described.minute) });
    case "weekly":
      return t("every {{weekday}} at {{time}}", {
        weekday: t(WEEKDAY_KEYS[described.weekday] ?? ""),
        time: hhmm(described.hour, described.minute),
      });
    default:
      return t("a custom schedule");
  }
}

/**
 * Create and edit are the same form, because they ask the same eleven questions.
 *
 * The only difference is the verb and the route, and keeping them one component
 * is what stops the edit form from quietly falling behind when a field is added
 * — which is the failure mode of every duplicated settings form.
 */
function TaskForm({ initial, onDone }: { initial?: ScheduledTask; onDone: () => void }) {
  const api = useApi();
  const machine = useMachine();
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const settings = useFeedSettings();
  const homeDir = settings?.homeDir ?? "";

  const [source, setSource] = useState<Source>(initial?.source === "codex" ? "codex" : "claude");
  const [name, setName] = useState(initial?.name ?? "");
  const [cwd, setCwd] = useState(initial?.cwd ?? "");
  const [browsing, setBrowsing] = useState(false);
  const [prompt, setPrompt] = useState(initial?.prompt ?? "");
  const [model, setModel] = useState(initial?.model ?? "");
  const [effort, setEffort] = useState(initial?.effort ?? "");
  const [permission, setPermission] = useState(initial?.permission ?? "");
  const [skipIfBusy, setSkipIfBusy] = useState(initial?.skipIfBusy ?? true);

  // A stored expression that is one of the presets opens on the preset controls;
  // only the ones that are not send the user to the raw field, which is the whole
  // point of [`matchPreset`].
  const opening = initial ? matchPreset(initial.expr) : null;
  const [preset, setPreset] = useState<PresetState>(opening ?? DEFAULT_PRESET);
  const [advanced, setAdvanced] = useState(initial != null && opening === null);
  const [rawExpr, setRawExpr] = useState(initial?.expr ?? presetExpr(DEFAULT_PRESET));
  const expr = advanced ? rawExpr : presetExpr(preset);
  const verdict = checkCron(expr);
  const upcoming = nextOccurrences(expr, new Date(), 3);

  // A schedule binds no account of its own: the runner spawns with
  // `use_default_account`, so the task runs wherever the server's start rule
  // sends an unnamed start at three in the morning — the configured default
  // (moved off a rest), else the local login, else, while that is signed out
  // and nobody chose it, an account in the auto-rotation pool. This form asks
  // the server where that is now, for the model the task names, and lists
  // models for that login. Codex has no account to pick, so its key stays
  // account-free.
  const { data: accountList } = useAccounts();
  // The task sends its model, or none (the CLI's default): placed as it is.
  const { start } = useStartAccount(source, model || null);
  const startAccountId = start?.accountId ?? null;
  const modelAccount = source === "claude" ? startAccountId : null;
  // The cases where the login a task runs on is not simply the one the user
  // set: the local login signed out, with or without a stand-in. The task still
  // runs, so the form says on what, rather than letting three in the morning
  // find out.
  const localSignedOut = localLoginSignedOut(localLoginOf(accountList, source));
  const standInLabel =
    start?.reason === "fallback" ? (accountList?.accounts.find((a) => a.id === startAccountId)?.label ?? null) : null;
  const runsSignedOut = localSignedOut && !startAccountId;
  const noStandIn =
    start?.reason === "local_signed_out" && accountsForProvider(accountList?.accounts ?? [], source).length > 0;
  const { data: modelData } = useQuery({
    // The composer's own cache: same route, same source, same account, so
    // opening this form costs nothing on a page that has already asked.
    queryKey: mq(machine, "models", source, modelAccount),
    queryFn: async () =>
      (await api.GET("/api/models", {
        params: { query: { source, ...(modelAccount ? { accountId: modelAccount } : {}) } },
      })).data,
    staleTime: 5 * 60_000,
  });
  // Deliberately not `useComposerChoice`: that hook writes every pick back to
  // /api/composer-prefs, and a schedule is not the composer — choosing opus for
  // a nightly task must not change what the next chat window opens with.
  const models = modelData?.models.length ? modelData.models : MODEL_FALLBACK[source];
  // A saved task may name a model this list has never carried — one typed into
  // the box here, or one the account's env block named before it changed. It is
  // still what the task runs with, so it stays selected, marked as typed-in.
  const customModel = model !== "" && !models.some((m) => m.id === model) ? model : null;
  const efforts = effortsOf(
    models.find((m) => m.id === model),
    source,
  );
  // A model with no ladder refuses `--effort` outright, so the rung is hidden
  // rather than emptied, and nothing is sent for it.
  const effortToSend = efforts.includes(effort) ? effort : "";

  const { data: recent } = useQuery({
    queryKey: mq(machine, "recent-cwds", source),
    queryFn: async () =>
      (await api.GET("/api/sessions/recent-cwds", { params: { query: { source } } })).data,
    staleTime: 30_000,
  });
  const recentCwds = (recent?.cwds ?? []).slice(0, 6);

  const save = useMutation({
    mutationFn: async () => {
      const common = {
        source,
        expr: expr.trim(),
        cwd: cwd.trim(),
        prompt: prompt.trim(),
        skipIfBusy,
      };
      if (initial) {
        const res = await api.PATCH("/api/scheduled-tasks/{id}", {
          params: { path: { id: initial.id } },
          // Sent unconditionally, empty string included: `""` is how the API
          // spells "back to the CLI's default", and leaving the field out would
          // mean "unchanged" — so an omitted model could never be unset again.
          body: {
            ...common,
            model,
            effort: effortToSend,
            permission,
            ...(name.trim() !== "" ? { name: name.trim() } : {}),
          },
        });
        if (res.error) throw new Error(res.error.error);
        return;
      }
      const res = await api.POST("/api/scheduled-tasks", {
        // On create the empty string has no meaning to add: absent already is
        // the CLI's default, and the server names an unnamed task itself.
        body: {
          ...common,
          ...(name.trim() !== "" ? { name: name.trim() } : {}),
          ...(model !== "" ? { model } : {}),
          ...(effortToSend !== "" ? { effort: effortToSend } : {}),
          ...(permission !== "" ? { permission } : {}),
        },
      });
      if (res.error) {
        // 409 is the only refusal with a fixed meaning — the fifty-task cap —
        // and it is worth saying in the user's own language. Everything else is
        // the server's own sentence about the flags it just rejected, and that
        // sentence is the point: it names the rungs the chosen model actually
        // has, which nothing in the browser knows.
        throw new Error(
          res.response.status === 409
            ? t("this instance already holds as many scheduled tasks as it will take")
            : res.error.error,
        );
      }
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: scheduledTasksKey(machine) });
      onDone();
    },
  });

  const ready =
    cwd.trim() !== "" && prompt.trim() !== "" && verdict.kind !== "invalid" && !save.isPending;

  const switchSource = (next: Source) => {
    setSource(next);
    // The two CLIs share no model ids, no effort ladder and no permission
    // names, so carrying a pick across would offer the new CLI a flag it has
    // never heard of — which the server refuses with a 400 rather than a
    // fallback. Back to "CLI default" on both sides of the switch.
    setModel("");
    setEffort("");
    setPermission("");
  };

  return (
    <SettingsColumn className="mt-2 flex w-full flex-col gap-2 border border-border bg-panel p-2">
      <Tabs value={source} onValueChange={(v) => switchSource(v as Source)}>
        <TabsList className="w-full">
          <TabsTab value="claude">claude</TabsTab>
          <TabsTab value="codex">codex</TabsTab>
        </TabsList>
      </Tabs>
      {(standInLabel || runsSignedOut) && (
        <div data-signal="warn" data-local-signed-out="" className="font-mono text-2xs text-warn-fg">
          {standInLabel ? (
            // Codex has no reading to heal its login by: only a turn that goes
            // through on it does, and a task on the stand-in is not one.
            source === "codex"
              ? t("your own codex login on this device was refused, so this task runs on {{label}}; after `codex login`, run one chat or session on the local login to switch back", { label: standInLabel })
              : t("your own login on this device is signed out, so this task runs on {{label}} until you sign in again", { label: standInLabel })
          ) : (
            <>
              {localLoginHint(source, t)}
              {noStandIn && <>{" — "}{t("and no account in auto-rotation is free to take its place")}</>}
            </>
          )}
        </div>
      )}

      <div className="flex flex-col gap-1">
        <span className="text-xs text-muted">{t("schedule")}</span>
        {advanced ? (
          <Input
            value={rawExpr}
            onChange={(e) => setRawExpr(e.target.value)}
            spellCheck={false}
            placeholder="0 9 * * *"
            className="font-mono"
          />
        ) : (
          <div className="flex flex-wrap items-center gap-1.5">
            <SelectField<CronPreset>
              aria-label={t("schedule")}
              value={preset.preset}
              onValueChange={(next) => setPreset({ ...preset, preset: next })}
              options={PRESETS.map((p) => ({
                value: p,
                label:
                  p === "daily"
                    ? t("every day at")
                    : p === "everyNHours"
                      ? t("every N hours")
                      : t("every week on"),
              }))}
              className="w-auto"
            />
            {preset.preset === "weekly" && (
              <SelectField
                aria-label={t("every week on")}
                value={String(preset.weekday)}
                onValueChange={(next) => setPreset({ ...preset, weekday: Number(next) })}
                options={WEEKDAY_KEYS.map((key, index) => ({ value: String(index), label: t(key) }))}
                className="w-auto"
              />
            )}
            {preset.preset === "everyNHours" ? (
              <>
                <Input
                  type="number"
                  min={1}
                  max={23}
                  value={preset.n}
                  onChange={(e) => setPreset({ ...preset, n: clampNumber(e.target.value, 1, 23, 1) })}
                  className="w-16"
                />
                <span className="text-xs text-muted">{t("hours, at minute")}</span>
                <Input
                  type="number"
                  min={0}
                  max={59}
                  value={preset.minute}
                  onChange={(e) =>
                    setPreset({ ...preset, minute: clampNumber(e.target.value, 0, 59, 0) })
                  }
                  className="w-16"
                />
              </>
            ) : (
              <Input
                type="time"
                value={hhmm(preset.hour, preset.minute)}
                onChange={(e) => {
                  const picked = parseHhmm(e.target.value);
                  if (picked) setPreset({ ...preset, ...picked });
                }}
                className="w-28"
              />
            )}
          </div>
        )}
        <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5">
          <span className="font-mono text-2xs text-muted">{expr}</span>
          <span className="text-xs text-muted">
            <ScheduleSummary expr={expr} />
          </span>
          <Button
            variant="ghost"
            onClick={() => {
              // Leaving the presets seeds the raw field with what they built, so
              // the advanced view starts from the schedule that was on screen.
              if (!advanced) setRawExpr(expr);
              setAdvanced(!advanced);
            }}
            className="ml-auto h-auto px-1.5 py-0.5 text-xs"
          >
            {advanced ? t("use the presets") : t("advanced")}
          </Button>
        </div>
        {verdict.kind === "invalid" && (
          <SettingsNote tone="danger">
            {verdict.reason === "fieldCount"
              ? t("a schedule is five fields: minute hour day-of-month month day-of-week")
              : verdict.reason === "tooLong"
                ? t("this expression is too long")
                : verdict.reason === "empty"
                  ? t("the schedule is empty")
                  : t("one of the five fields is not a value cron accepts")}
          </SettingsNote>
        )}
        {upcoming.length > 0 && (
          <SettingsNote tone="muted" className="text-xs">
            {t("next: {{times}}", {
              // The weekday earns its place here: "Tue Sep 8, 09:00" is the
              // reading that catches a `* * * * 2` meant as `* * * * 1`, which
              // a bare date does not.
              times: upcoming
                .map((d) =>
                  d.toLocaleString(undefined, {
                    weekday: "short",
                    month: "short",
                    day: "numeric",
                    hour: "2-digit",
                    minute: "2-digit",
                  }),
                )
                .join(" · "),
            })}
          </SettingsNote>
        )}
        <SettingsHint>
          {t(
            "The schedule is read in the server's time zone. The times above are drawn in this browser's ({{zone}}), so they only match if the server keeps the same clock.",
            { zone: Intl.DateTimeFormat().resolvedOptions().timeZone },
          )}
        </SettingsHint>
      </div>

      <div className="flex flex-col gap-1">
        <span className="text-xs text-muted">{t("working directory")}</span>
        <div className="flex items-center gap-1">
          <div className="min-w-0 flex-1">
            <Combobox
              value={cwd}
              onValueChange={setCwd}
              items={recent?.cwds ?? []}
              placeholder="/path/to/project"
            />
          </div>
          <button
            type="button"
            title={t("browse folders")}
            onClick={() => setBrowsing(!browsing)}
            className={cn(
              "flex size-6.5 shrink-0 items-center justify-center rounded border border-border hover:bg-hover",
              browsing ? "bg-hover text-fg" : "bg-bg text-muted",
            )}
          >
            <FolderSimpleIcon className="size-3.5" />
          </button>
        </div>
        {recentCwds.length > 0 && (
          <div className="flex flex-wrap gap-1">
            {recentCwds.map((path) => (
              <button
                key={path}
                type="button"
                title={path}
                onClick={() => setCwd(path)}
                className={cn(
                  "max-w-full truncate rounded border px-1.5 py-0.5 font-mono text-2xs",
                  path === cwd.trim()
                    ? "border-fg bg-hover text-fg"
                    : "border-border bg-bg text-muted hover:bg-hover hover:text-fg",
                )}
              >
                {shortenHome(path, homeDir)}
              </button>
            ))}
          </div>
        )}
        {browsing && (
          <DirBrowser
            start={cwd.trim()}
            onPick={(path) => {
              setCwd(path);
              setBrowsing(false);
            }}
          />
        )}
      </div>

      <SettingField label={t("prompt")}>
        <Textarea
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          rows={4}
          placeholder={t("what should the agent do each time?")}
        />
      </SettingField>

      <div className="flex flex-wrap items-end gap-2">
        <SettingField label={t("model")} className="min-w-32 flex-1">
          <ModelSelect
            models={models}
            value={model}
            customId={customModel}
            onChange={setModel}
            className="cursor-pointer"
            leading={{ value: "", label: t("CLI default") }}
          />
        </SettingField>
        {efforts.length > 0 && (
          <SettingField label={t("effort")} className="min-w-24 flex-1">
            <SelectField
              value={effortToSend}
              onValueChange={setEffort}
              options={[{ value: "", label: t("CLI default") }, ...efforts.map((e) => ({ value: e, label: e }))]}
            />
          </SettingField>
        )}
        <SettingField label={t("permission")} className="min-w-32 flex-1">
          <SelectField
            value={permission}
            onValueChange={setPermission}
            options={[{ value: "", label: t("CLI default") }, ...PERMISSION_OPTIONS[source].map((p) => ({ value: p, label: p }))]}
          />
        </SettingField>
      </div>

      <CheckboxField
        checked={skipIfBusy}
        onCheckedChange={setSkipIfBusy}
        className="text-xs"
        labelClassName="text-xs text-muted"
        label={t(
          "Skip a run while the session the previous one started is still working, instead of stacking a second session on top of it.",
        )}
      />

      <SettingField label={t("name (optional)")}>
        <Input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder={t("nightly review")}
        />
      </SettingField>

      {save.isError && <SettingsNote tone="danger">{save.error.message}</SettingsNote>}

      <div className="flex items-center gap-1.5">
        <Button variant="primary" disabled={!ready} onClick={() => save.mutate()}>
          {save.isPending && <CircleNotchIcon className="size-3 animate-spin" />}
          {initial ? t("save") : t("add task")}
        </Button>
        <Button onClick={onDone}>{t("cancel")}</Button>
      </div>
    </SettingsColumn>
  );
}

function TaskRow({ task }: { task: ScheduledTask }) {
  const machine = useMachine();
  const api = useApi();
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const settings = useFeedSettings();
  const homeDir = settings?.homeDir ?? "";
  const [confirming, setConfirming] = useState(false);
  const [editing, setEditing] = useState(false);

  const toggle = useMutation({
    mutationFn: async () => {
      const res = await api.PATCH("/api/scheduled-tasks/{id}", {
        params: { path: { id: task.id } },
        body: { enabled: !task.enabled },
      });
      if (res.error) throw new Error(res.error.error);
      return res.data;
    },
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: scheduledTasksKey(machine) }),
  });

  const remove = useMutation({
    mutationFn: async () => {
      const res = await api.DELETE("/api/scheduled-tasks/{id}", {
        params: { path: { id: task.id } },
      });
      if (res.error) throw new Error(res.error.error);
    },
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: scheduledTasksKey(machine) }),
  });

  return (
    <div className="flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-border/40 py-1.5">
      <Button
        variant="ghost"
        disabled={toggle.isPending}
        onClick={() => toggle.mutate()}
        title={task.enabled ? t("enabled — click to pause") : t("paused — click to enable")}
        aria-label={task.enabled ? t("enabled — click to pause") : t("paused — click to enable")}
        className="size-4 p-0"
      >
        <span
          className={cn(
            "size-2 rounded-full bg-current",
            task.enabled ? "text-accent" : "text-faint opacity-50",
          )}
        />
      </Button>
      <TaskName task={task} />
      <SourceIcon source={task.source} className="text-muted" />
      {/* The directory is what says which project a nightly task acts on, and
          the row is the only place it is visible without opening the form. */}
      <span className="max-w-48 truncate font-mono text-2xs text-faint" title={task.cwd}>
        {shortenHome(task.cwd, homeDir)}
      </span>
      <span className="font-mono text-2xs text-faint">
        {task.expr}
        {" · "}
        {!task.enabled
          ? t("paused")
          : task.nextRunAt
            ? t("next in {{when}}", { when: formatUntil(task.nextRunAt) })
            : // Enabled with nothing scheduled is the one state that needs a
              // reason: the expression either did not parse or never comes round.
              t("no next run — the server could not use this expression")}
      </span>
      <StatusDot task={task} />
      <div className="ml-auto flex items-center gap-1">
        <Button
          variant="ghost"
          title={t("edit task")}
          aria-label={t("edit task")}
          onClick={() => setEditing(!editing)}
          className={cn("size-6 p-0", editing ? "text-accent" : "hover:text-fg")}
        >
          <PencilSimpleIcon className="size-3.5" />
        </Button>
        {confirming ? (
          <>
            <SettingsNote tone="warn">{t("delete this task?")}</SettingsNote>
            <Button
              variant="ghost"
              disabled={remove.isPending}
              onClick={() => remove.mutate()}
              data-signal="failed"
              className="h-auto px-1.5 py-0.5 font-mono text-2xs text-danger hover:bg-danger/15"
            >
              {remove.isPending ? t("removing…") : t("remove")}
            </Button>
            <Button
              variant="ghost"
              onClick={() => setConfirming(false)}
              className="h-auto px-1.5 py-0.5 font-mono text-2xs"
            >
              {t("cancel")}
            </Button>
          </>
        ) : (
          <Button
            variant="ghost"
            title={t("delete task")}
            aria-label={t("delete task")}
            onClick={() => setConfirming(true)}
            data-signal="failed"
            className="size-6 p-0 hover:text-danger"
          >
            <TrashIcon className="size-3.5" />
          </Button>
        )}
      </div>
      {/* A failure has to be readable without a hover: on a phone there is none. */}
      {isLoud(task.lastStatus) && task.lastError && (
        <SettingsNote tone="danger" className="w-full break-all">{task.lastError}</SettingsNote>
      )}
      {toggle.isError && (
        <SettingsNote tone="danger" className="w-full">{toggle.error.message}</SettingsNote>
      )}
      {remove.isError && (
        <SettingsNote tone="danger" className="w-full">{remove.error.message}</SettingsNote>
      )}
      {editing && <TaskForm initial={task} onDone={() => setEditing(false)} />}
    </div>
  );
}

/**
 * Cron for this instance: expressions that start a brand new session, on the
 * server's own clock, with no catch-up for the ones that were missed.
 */
export function ScheduleSection() {
  const machine = useMachine();
  const api = useApi();
  const { t } = useTranslation();
  const [adding, setAdding] = useState(false);
  const { data, isPending } = useQuery({
    queryKey: scheduledTasksKey(machine),
    // The route declares no failure: a task file that cannot be read is an
    // empty list, not an error, so there is nothing here to throw about.
    queryFn: async () => (await api.GET("/api/scheduled-tasks")).data,
  });
  const tasks = data?.tasks ?? [];

  return (
    <Sheet className="p-3">
      <SettingsColumn>
        <SettingsHint>
          {t(
            "Each task starts a new session on its own schedule — there is no injecting a message into a session you already have open. The expression is read in the server's time zone, not this browser's, and a run whose time passed while the server was down is not made up afterwards: it simply waits for the next one.",
          )}
        </SettingsHint>
      </SettingsColumn>

      <SettingsColumn className="mt-3">
        {isPending && <Spinner />}
        {data && (
          <>
            {tasks.map((task) => (
              <TaskRow key={task.id} task={task} />
            ))}
            {tasks.length === 0 && (
              <p className="py-2 text-xs text-muted">{t("no scheduled tasks yet")}</p>
            )}
          </>
        )}
      </SettingsColumn>

      <SettingsColumn className="mt-3">
        {adding ? (
          <TaskForm onDone={() => setAdding(false)} />
        ) : (
          <Button variant="primary" onClick={() => setAdding(true)}>
            <PlusIcon className="size-3" />
            {t("add scheduled task")}
          </Button>
        )}
      </SettingsColumn>
    </Sheet>
  );
}
