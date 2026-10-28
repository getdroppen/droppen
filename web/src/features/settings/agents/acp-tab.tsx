/*
 * Settings › Agents & accounts › ACP: the ACP agents this device runs
 * sessions on, as a grid of cards.
 *
 * - A card is an agent's launch description (command line, the names of its
 *   variables) and what its last check found. The name and the status open
 *   its details; the 「⋯」 menu holds every action.
 * - The details are a dialog, not an expanding card: the check's facts, the
 *   raw error when it failed, and the actions — check again, edit, remove,
 *   whose confirmation takes the dialog's footer.
 * - The grid ends with an "add" card. With no agent at all, the built-in
 *   presets are one-click starts, each saying whether its command is on this
 *   device's PATH, and so is any command.
 * - Adding and editing are one dialog: pick a preset or a custom command,
 *   then the form, then 「add and check」. It stays open on a failed check,
 *   with the error, so the fix is made where the problem is shown.
 *
 * The views take their data as props, so `web/ui-audit` renders them without
 * an API; `AcpTab` is the part that talks to the device.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronRight, ExternalLink, Plus, RefreshCw } from "lucide-react";
import { useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { ApiError, errorText } from "@/api/errors";
import type { components } from "@/api/schema";
import { useFeedSessions } from "@/app/global-feed";
import { GlyphIcon } from "@/components/glyph";
import { Badge } from "@/components/ui/badge";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { RowMenu, type RowMenuItem } from "@/features/cloud/device-row";
import { acpAgentsKey, splitArgs, useAcpAgents, useAcpCheck, type AcpAgentInfo } from "@/features/sessions/acp-agent";
import { ACP_GLYPH, acpAgentGlyph } from "@/features/sessions/acp-agent-icons";
import { applyPreset, CUSTOM_PRESET, newAgentId, type AcpAgentDraft, type AcpAgentPreset } from "@/features/sessions/acp-agent-presets";
import {
  ROW_HINT,
  SETTINGS_ACTION,
  SETTINGS_ACTION_DANGER,
  SETTINGS_ACTION_PRIMARY,
  SETTINGS_COLUMN,
  SETTINGS_FIELD,
} from "@/features/settings/layout";
import { cn } from "@/lib/cn";
import { useApi, useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";
import { acpStatus, agoKey, checkFailed, type AcpCheck, type AcpStatus } from "./agents-tabs";
import { StatusWord } from "./status-word";

type ModelList = components["schemas"]["ModelList"];

/** Cards on the sheet's full width: three columns on a desktop sheet, one on a phone. */
export const CARD_GRID = "grid grid-cols-[repeat(auto-fill,minmax(17.5rem,1fr))] gap-3";
const CARD = "group/card flex min-w-0 flex-col gap-3 border border-border p-3 hover:border-border-strong";
const DIALOG_WIDTH = "w-[min(32rem,calc(100vw-2rem))]";
const mono = cn(SETTINGS_FIELD, "font-mono");

export function commandLine(agent: { command: string; args: readonly string[] }): string {
  return [agent.command, ...agent.args].join(" ");
}

/** "5 min ago", in the page's language. */
function useAgo() {
  const { t } = useTranslation();
  return (iso: string) => {
    const { key, n } = agoKey(iso);
    return t(key, { n });
  };
}

/** A check's outcome as a word: ready, checking, not checked yet, or why not. */
export function AcpStatusWord({ status }: { status: AcpStatus }) {
  const { t } = useTranslation();
  switch (status.kind) {
    case "ready":
      return <StatusWord tone="ok">{t("ready to run")}</StatusWord>;
    case "checking":
      return <StatusWord tone="muted">{t("checking…")}</StatusWord>;
    case "unchecked":
      return <StatusWord tone="faint">{t("not checked yet")}</StatusWord>;
    case "not_found":
      return <StatusWord tone="danger">{t("command not found")}</StatusWord>;
    case "failed":
      return <StatusWord tone="danger">{t("does not start")}</StatusWord>;
  }
}

/** The raw error of a failed check: the agent's own words, red, monospaced, selectable. */
function CheckError({ check }: { check: AcpCheck }) {
  if (!check.error) return null;
  return (
    <pre data-signal="failed" className="max-h-40 overflow-y-auto bg-raised px-2 py-1 font-mono text-2xs leading-4 break-words whitespace-pre-wrap text-danger select-text">
      {check.error}
    </pre>
  );
}

export interface AcpCardActions {
  onOpen: (id: string) => void;
  onCheck: (id: string) => void;
  onEdit: (id: string) => void;
  onRemove: (id: string) => void;
}

export function AcpAgentCard({ agent, status, actions }: { agent: AcpAgentInfo; status: AcpStatus; actions: AcpCardActions }) {
  const { t } = useTranslation();
  const ago = useAgo();
  const items: RowMenuItem[] = [
    { id: `details-${agent.id}`, label: t("details…"), blocked: null, onSelect: () => actions.onOpen(agent.id) },
    { id: `check-${agent.id}`, label: t("check again"), blocked: null, busy: status.kind === "checking", onSelect: () => actions.onCheck(agent.id) },
    { id: `edit-${agent.id}`, label: t("edit…"), blocked: null, onSelect: () => actions.onEdit(agent.id) },
    { id: `remove-${agent.id}`, label: t("delete…"), blocked: null, danger: true, onSelect: () => actions.onRemove(agent.id) },
  ];
  const checkedAt = "check" in status ? status.check.checkedAt : null;
  return (
    <li data-acp-card={agent.id} className={CARD}>
      <div className="flex min-w-0 items-start gap-2">
        <GlyphIcon glyph={acpAgentGlyph(agent.id)} className="mt-0.5 size-4 text-muted" />
        <div className="min-w-0 flex-1">
          <button
            type="button"
            onClick={() => actions.onOpen(agent.id)}
            className="block max-w-full truncate text-left text-[13px] leading-5 font-medium text-fg group-hover/card:underline group-hover/card:underline-offset-2"
          >
            {agent.name}
          </button>
          <div className="mt-0.5 truncate font-mono text-2xs leading-4 text-faint" title={commandLine(agent)}>
            {commandLine(agent)}
          </div>
          <div className="truncate font-mono text-2xs leading-4 text-faint" title={agent.envNames.join(", ")}>
            {agent.envNames.length ? agent.envNames.join(", ") : t("no environment variables")}
          </div>
        </div>
        <span className="-mt-1 -mr-1.5">
          <RowMenu id={`acp-${agent.id}`} items={items} />
        </span>
      </div>
      <div className="mt-auto flex min-h-7 min-w-0 items-center justify-between gap-2">
        {/* The status is the way in: the details are where the fix is. */}
        <button type="button" onClick={() => actions.onOpen(agent.id)} className="-mx-1 flex min-w-0 px-1 py-0.5 hover:bg-hover">
          <AcpStatusWord status={status} />
        </button>
        {checkedAt && <span className="shrink-0 font-mono text-2xs text-faint">{t("checked {{ago}}", { ago: ago(checkedAt) })}</span>}
      </div>
    </li>
  );
}

/** The last card of a grid: a dashed-free frame with a plus, and a hint when the grid is empty. */
export function AddCard({ label, hint, onClick }: { label: string; hint?: string; onClick: () => void }) {
  return (
    <li className="flex min-w-0">
      <button
        type="button"
        data-add-card=""
        onClick={onClick}
        className="flex min-h-[5.5rem] w-full flex-col items-center justify-center gap-1 border border-border px-3 text-xs font-medium text-muted hover:border-border-strong hover:text-fg"
      >
        <span className="flex items-center gap-1.5">
          <Plus className="size-3.5" />
          {label}
        </span>
        {hint && <span className="text-center text-2xs font-normal text-faint">{hint}</span>}
      </button>
    </li>
  );
}

/** Whether a preset's command is on this device's PATH, or already added. */
function PresetAvailability({ preset, added }: { preset: AcpAgentPreset; added: boolean }) {
  const { t } = useTranslation();
  if (added) {
    return (
      <Badge variant="outline" className="tracking-[0.08em] uppercase">
        {t("added")}
      </Badge>
    );
  }
  return preset.onPath ? <StatusWord tone="ok">{t("on PATH")}</StatusWord> : <StatusWord tone="faint">{t("not on PATH")}</StatusWord>;
}

/** No agent yet: the presets as one-click starts, then any command. */
export function AcpEmpty({ presets, onPick }: { presets: readonly AcpAgentPreset[]; onPick: (preset: string) => void }) {
  const { t } = useTranslation();
  return (
    <div>
      <p className="mb-2 text-[13px] leading-5 text-fg">{t("Start from a known agent, or any command:")}</p>
      <ul className={CARD_GRID}>
        {presets.map((preset) => (
          <li key={preset.id} data-acp-preset={preset.id} className="flex min-w-0">
            <button
              type="button"
              onClick={() => onPick(preset.id)}
              className="group/card flex min-h-[5.5rem] w-full min-w-0 flex-col gap-3 border border-border p-3 text-left hover:border-border-strong"
            >
              <span className="flex min-w-0 items-start gap-2">
                <GlyphIcon glyph={acpAgentGlyph(preset.id)} className="mt-0.5 size-4 text-muted" />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[13px] leading-5 font-medium text-fg group-hover/card:underline group-hover/card:underline-offset-2">
                    {preset.name}
                  </span>
                  <span className="mt-0.5 block truncate font-mono text-2xs leading-4 text-faint">{commandLine(preset)}</span>
                </span>
              </span>
              <span className="mt-auto flex min-h-5 items-center justify-between gap-2">
                <PresetAvailability preset={preset} added={false} />
                <span className="flex items-center gap-1 text-xs font-medium text-muted group-hover/card:text-fg">
                  <Plus className="size-3" />
                  {t("add")}
                </span>
              </span>
            </button>
          </li>
        ))}
        <AddCard label={t("custom command")} hint={t("any program that speaks ACP over stdio")} onClick={() => onPick(CUSTOM_PRESET)} />
      </ul>
    </div>
  );
}

/** The tab's whole view: the hint, the count, then the grid or the empty state. */
export function AcpTabView({
  agents,
  presets,
  statusOf,
  actions,
  onAdd,
}: {
  agents: readonly AcpAgentInfo[];
  presets: readonly AcpAgentPreset[];
  statusOf: (agent: AcpAgentInfo) => AcpStatus;
  actions: AcpCardActions;
  onAdd: (preset?: string) => void;
}) {
  const { t } = useTranslation();
  const failing = agents.filter((agent) => checkFailed(agent.lastCheck)).length;
  return (
    <div data-tab-panel="acp" className="p-3">
      <p className={cn(SETTINGS_COLUMN, ROW_HINT)}>
        {t("Any program that speaks the Agent Client Protocol over stdio. It has no accounts: an API key goes in its own environment. New sessions offer each one beside Claude Code and Codex.")}
      </p>
      <div className="mt-3 mb-2 border-t border-border pt-3 font-mono text-2xs text-faint">
        {agents.length > 0 ? t("{{n}} agents", { n: agents.length }) : t("no ACP agents yet")}
        {failing > 0 && <> · {t("{{n}} cannot run", { n: failing })}</>}
      </div>
      {agents.length > 0 ? (
        <ul data-acp-grid="" className={CARD_GRID}>
          {agents.map((agent) => (
            <AcpAgentCard key={agent.id} agent={agent} status={statusOf(agent)} actions={actions} />
          ))}
          <AddCard label={t("add agent")} onClick={() => onAdd()} />
        </ul>
      ) : (
        <AcpEmpty presets={presets} onPick={(preset) => onAdd(preset)} />
      )}
    </div>
  );
}

/** One fact of the details: a label and its value. */
function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="contents">
      <dt className="text-faint">{label}</dt>
      <dd className="min-w-0 break-words text-fg">{children}</dd>
    </div>
  );
}

/** What the last check found, as facts. */
export function CheckFacts({ agent, status, sessions, models }: { agent: AcpAgentInfo; status: AcpStatus; sessions: number; models: number | null }) {
  const { t } = useTranslation();
  const ago = useAgo();
  const check = "check" in status ? status.check : null;
  const ready = check?.state === "ready" ? check : null;
  const caps = ready?.capabilities;
  const can = caps
    ? [
        caps.loadSession && t("load session"),
        caps.image && t("images"),
        caps.audio && t("audio"),
        caps.embeddedContext && t("embedded context"),
        (caps.mcpHttp || caps.mcpSse) && "MCP",
      ].filter((cap): cap is string => !!cap)
    : [];
  const info = ready?.agentInfo;
  return (
    <dl data-acp-facts="" className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-0.5 font-mono text-xs">
      <Fact label={t("command")}>{ready?.resolvedPath ? commandLine({ command: ready.resolvedPath, args: agent.args }) : commandLine(agent)}</Fact>
      {ready && (info || ready.protocolVersion) && (
        <Fact label={t("agent")}>
          {[info ? [info.title ?? info.name, info.version].filter(Boolean).join(" ") : null, ready.protocolVersion ? `ACP v${ready.protocolVersion}` : null]
            .filter(Boolean)
            .join(" · ")}
        </Fact>
      )}
      {ready && can.length > 0 && (
        <Fact label={t("can")}>
          <span className="flex flex-wrap gap-1">
            {can.map((cap) => (
              <Badge key={cap}>{cap}</Badge>
            ))}
          </span>
        </Fact>
      )}
      {ready?.authMethods && ready.authMethods.length > 0 && (
        <Fact label={t("sign-in")}>{ready.authMethods.map((method) => method.name).join(", ")}</Fact>
      )}
      {models !== null && <Fact label={t("models")}>{t("{{n}} offered by the agent", { n: models })}</Fact>}
      <Fact label={t("environment")}>{agent.envNames.length ? agent.envNames.join(", ") : "—"}</Fact>
      <Fact label={t("sessions")}>{String(sessions)}</Fact>
      {check && <Fact label={t("checked")}>{ago(check.checkedAt)}</Fact>}
    </dl>
  );
}

/** The details dialog's body: facts, the error, the preset's note, then the actions or the remove confirmation. */
export function AcpDetailsBody({
  agent,
  status,
  sessions,
  models,
  confirming,
  busy,
  error,
  errorWithheld,
  onCheck,
  onEdit,
  onAskRemove,
  onConfirmRemove,
  onCancelRemove,
}: {
  agent: AcpAgentInfo;
  status: AcpStatus;
  sessions: number;
  models: number | null;
  confirming: boolean;
  busy: boolean;
  error: string | null;
  /** Why the check failed is kept from this device: it does not manage the computer. */
  errorWithheld: boolean;
  onCheck: () => void;
  onEdit: () => void;
  onAskRemove: () => void;
  onConfirmRemove: () => void;
  onCancelRemove: () => void;
}) {
  const { t } = useTranslation();
  const check = "check" in status ? status.check : null;
  return (
    <div data-acp-details={agent.id} className="flex flex-col gap-3">
      <div className="flex items-center gap-2">
        <GlyphIcon glyph={acpAgentGlyph(agent.id)} className="size-4 text-muted" />
        <AcpStatusWord status={status} />
      </div>
      <CheckFacts agent={agent} status={status} sessions={sessions} models={models} />
      {check && checkFailed(check) && <CheckError check={check} />}
      {check && checkFailed(check) && errorWithheld && (
        <p data-acp-withheld="" className={ROW_HINT}>
          {t("only a full device can see why it failed")}
        </p>
      )}
      {(agent.note || agent.homepage) && (
        <p className="text-2xs leading-4 text-faint">
          {agent.note}
          {agent.note && agent.homepage && " · "}
          {agent.homepage && (
            <a href={agent.homepage} target="_blank" rel="noreferrer" className="underline hover:text-fg">
              {t("learn more")}
            </a>
          )}
        </p>
      )}
      {error && (
        <p data-signal="failed" className="text-xs text-danger">
          {error}
        </p>
      )}
      {confirming ? (
        // The confirmation takes the footer's place: what stays, what it takes to send again.
        <div data-acp-remove-confirm="" className="flex flex-col gap-2 border-t border-border pt-3">
          <p className="text-xs leading-[18px] text-muted">
            {t("Delete {{name}}? Its {{n}} sessions stay readable; sending to them needs an agent with this id again.", {
              name: agent.name,
              n: sessions,
            })}
          </p>
          <div className="flex flex-wrap items-center gap-1.5">
            <button type="button" disabled={busy} onClick={onConfirmRemove} className={SETTINGS_ACTION_DANGER}>
              {t("delete")}
            </button>
            <button type="button" disabled={busy} onClick={onCancelRemove} className={SETTINGS_ACTION}>
              {t("cancel")}
            </button>
          </div>
        </div>
      ) : (
        <div className="flex flex-wrap items-center gap-1.5 border-t border-border pt-3">
          <button type="button" disabled={busy || status.kind === "checking"} onClick={onCheck} className={SETTINGS_ACTION}>
            <RefreshCw className="size-3" />
            {t("check again")}
          </button>
          <button type="button" disabled={busy} onClick={onEdit} className={SETTINGS_ACTION}>
            {t("edit…")}
          </button>
          <button type="button" disabled={busy} onClick={onAskRemove} className={cn(SETTINGS_ACTION_DANGER, "ml-auto")}>
            {t("delete…")}
          </button>
        </div>
      )}
    </div>
  );
}

/** Step one of adding: a preset, or any command. */
export function PresetChoices({
  presets,
  added,
  onPick,
}: {
  presets: readonly AcpAgentPreset[];
  added: ReadonlySet<string>;
  onPick: (preset: string) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-2">
      <p className={ROW_HINT}>{t("Start from a known agent or any command; every field stays editable.")}</p>
      <div className="flex flex-col">
        {[...presets, null].map((preset) => (
          <button
            key={preset?.id ?? CUSTOM_PRESET}
            type="button"
            data-acp-choice={preset?.id ?? CUSTOM_PRESET}
            onClick={() => onPick(preset?.id ?? CUSTOM_PRESET)}
            className="relative -mt-px flex min-h-12 w-full min-w-0 items-center gap-2.5 border border-border bg-bg px-2.5 py-2 text-left first:mt-0 hover:bg-hover"
          >
            <GlyphIcon glyph={preset ? acpAgentGlyph(preset.id) : ACP_GLYPH} className="size-4 text-muted" />
            <span className="min-w-0 flex-1">
              <span className="block truncate text-[13px] leading-5 font-medium text-fg">{preset ? preset.name : t("custom command")}</span>
              <span className="block truncate font-mono text-2xs leading-4 text-faint">
                {preset ? commandLine(preset) : t("any program that speaks ACP over stdio")}
              </span>
              {preset?.note && <span className="block text-2xs leading-4 text-faint">{preset.note}</span>}
            </span>
            {preset && <PresetAvailability preset={preset} added={added.has(preset.id)} />}
            <ChevronRight aria-hidden="true" className="size-3 shrink-0 text-faint" />
          </button>
        ))}
      </div>
    </div>
  );
}

/** The add / edit form's fields, with what the last check found under them. */
export function AgentFields({
  draft,
  args,
  env,
  envNames,
  onDraft,
  onEnv,
  onClearEnv,
  check,
  status,
}: {
  draft: { name: string; command: string };
  args: string;
  env: string;
  envNames: readonly string[];
  onDraft: (next: { name?: string; command?: string; args?: string }) => void;
  onEnv: (env: string) => void;
  /** Forget every saved variable, at once. */
  onClearEnv?: () => void;
  check: AcpCheck | null;
  status: AcpStatus | null;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-2">
      <label className="flex flex-col gap-1">
        <span className="text-xs text-muted">{t("name")}</span>
        <input
          value={draft.name}
          onChange={(e) => onDraft({ name: e.target.value })}
          placeholder={t("ACP agent")}
          autoComplete="off"
          spellCheck={false}
          className={SETTINGS_FIELD}
        />
      </label>
      <div className="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)] gap-2 max-md:grid-cols-1">
        <label className="flex flex-col gap-1">
          <span className="text-xs text-muted">{t("command")}</span>
          <input
            value={draft.command}
            onChange={(e) => onDraft({ command: e.target.value })}
            placeholder="/path/to/agent"
            autoComplete="off"
            spellCheck={false}
            className={mono}
          />
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-xs text-muted">{t("arguments (separated by spaces)")}</span>
          <input value={args} onChange={(e) => onDraft({ args: e.target.value })} placeholder="acp" autoComplete="off" spellCheck={false} className={mono} />
        </label>
      </div>
      <label className="flex flex-col gap-1">
        <span className="text-xs text-muted">
          {t("environment (one KEY=VALUE per line)")}
          {envNames.length > 0 && <> {t("· saved: {{names}} (a new block replaces them all)", { names: envNames.join(", ") })}</>}
          {envNames.length > 0 && onClearEnv && (
            <>
              {" · "}
              <button type="button" onClick={onClearEnv} className="underline hover:text-fg">
                {t("clear variables")}
              </button>
            </>
          )}
        </span>
        <textarea
          rows={3}
          value={env}
          onChange={(e) => onEnv(e.target.value)}
          placeholder={envNames.length > 0 ? "••••••••" : "KEY=…"}
          autoComplete="off"
          spellCheck={false}
          className={cn(mono, "h-auto resize-y py-1.5 max-md:h-auto")}
        />
      </label>
      {status && (
        <div data-acp-check="" className="mt-1 flex flex-col gap-1.5 border-t border-border pt-2">
          <div className="flex items-center justify-between gap-2">
            <span className="text-xs font-medium text-fg">{t("agent check")}</span>
            <AcpStatusWord status={status} />
          </div>
          <p className="text-2xs leading-4 text-faint">
            {t("Starts the command with its environment, sends `initialize`, reads what the agent says it can do, and stops it. Nothing is sent to a model.")}
          </p>
          {check && checkFailed(check) && <CheckError check={check} />}
        </div>
      )}
    </div>
  );
}

/** What the add / edit dialog is doing. */
type Editing =
  | { mode: "add"; step: "pick" }
  | { mode: "add"; step: "form"; preset: string }
  | { mode: "edit"; id: string };

/** The body of a check, as the server answers it, or why it was not run. */
async function runCheck(api: ReturnType<typeof useApi>, id: string): Promise<AcpCheck> {
  const res = await api.POST("/api/acp/agents/{id}/check", { params: { path: { id } } });
  if (!res.data) throw new ApiError(res.error);
  return res.data;
}

/** The add / edit dialog: pick, fill in, save and check; open until the check passes or it is closed. */
function AgentDialog({
  editing,
  agents,
  presets,
  onClose,
  onStep,
}: {
  editing: Editing;
  agents: readonly AcpAgentInfo[];
  presets: readonly AcpAgentPreset[];
  onClose: () => void;
  onStep: (next: Editing) => void;
}) {
  const { t } = useTranslation();
  const api = useApi();
  const machine = useMachine();
  const queryClient = useQueryClient();
  // The agent once saved: from then on the dialog edits it, whatever it was
  // opened for, and reads it from the list as soon as the list has it.
  const [saved, setSaved] = useState<AcpAgentInfo | null>(null);
  const editId = saved?.id ?? (editing.mode === "edit" ? editing.id : null);
  const existing = editId ? (agents.find((agent) => agent.id === editId) ?? saved ?? undefined) : undefined;
  const preset = editing.mode === "add" && editing.step === "form" ? editing.preset : CUSTOM_PRESET;
  const [draft, setDraft] = useState<AcpAgentDraft>(() =>
    editing.mode === "add" && editing.step === "form" ? applyPreset({ name: null, command: null, args: null }, editing.preset, presets) : { name: null, command: null, args: null },
  );
  const [env, setEnv] = useState("");
  const name = draft.name ?? existing?.name ?? "";
  const command = draft.command ?? existing?.command ?? "";
  const args = draft.args ?? existing?.args.join(" ") ?? "";
  const taken = new Set(agents.map((agent) => agent.id));
  const id = existing?.id ?? newAgentId(name, taken, preset);
  const described = existing ?? presets.find((candidate) => candidate.id === preset);

  const [check, setCheck] = useState<AcpCheck | null>(existing?.lastCheck ?? null);
  const save = useMutation({
    mutationFn: async () => {
      const target = id;
      const body =
        existing
          ? {
              ...(draft.name !== null ? { name } : {}),
              ...(draft.command !== null ? { command } : {}),
              ...(draft.args !== null ? { args: splitArgs(args) } : {}),
              ...(env ? { env } : {}),
            }
          : { name, command, args: splitArgs(args), ...(env ? { env } : {}) };
      const res = await api.PUT("/api/acp/agents/{id}", { params: { path: { id: target } }, body });
      if (!res.data) throw new ApiError(res.error);
      setSaved(res.data);
      setDraft({ name: null, command: null, args: null });
      setEnv("");
      void queryClient.invalidateQueries({ queryKey: mq(machine, "models", "acp", target) });
      return runCheck(api, target);
    },
    onSuccess: (result) => {
      setCheck(result);
      void queryClient.invalidateQueries({ queryKey: acpAgentsKey(machine) });
      // A passing check is the end of it; a failing one stays on screen with its error.
      if (!checkFailed(result)) onClose();
    },
  });
  const clearEnv = useMutation({
    mutationFn: async () => {
      const res = await api.PUT("/api/acp/agents/{id}", { params: { path: { id: existing!.id } }, body: { env: "" } });
      if (!res.data) throw new ApiError(res.error);
    },
    onSuccess: () => {
      setCheck(null);
      void queryClient.invalidateQueries({ queryKey: acpAgentsKey(machine) });
    },
  });
  const recheck = useMutation({
    mutationFn: () => runCheck(api, existing!.id),
    onSuccess: (result) => {
      setCheck(result);
      void queryClient.invalidateQueries({ queryKey: acpAgentsKey(machine) });
    },
  });

  const title = existing ? t("edit {{name}}", { name: existing.name }) : t("add an ACP agent");
  const status = save.isPending || recheck.isPending ? acpStatus(null, true) : check ? acpStatus(check) : null;
  const dirty = draft.name !== null || draft.command !== null || draft.args !== null || env !== "";
  const error = save.error ?? recheck.error ?? clearEnv.error;
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent title={title} className={DIALOG_WIDTH}>
        {editing.mode === "add" && editing.step === "pick" ? (
          <PresetChoices presets={presets} added={taken} onPick={(next) => onStep({ mode: "add", step: "form", preset: next })} />
        ) : (
          <div data-acp-form={existing?.id ?? "new"} className="flex flex-col gap-2">
            {!existing && <div className="font-mono text-2xs text-faint">{t("id: {{id}}", { id })}</div>}
            <AgentFields
              draft={{ name, command }}
              args={args}
              env={env}
              envNames={existing?.envNames ?? []}
              onDraft={(next) =>
                setDraft({
                  name: next.name !== undefined ? next.name : draft.name,
                  command: next.command !== undefined ? next.command : draft.command,
                  args: next.args !== undefined ? next.args : draft.args,
                })
              }
              onEnv={setEnv}
              onClearEnv={existing ? () => clearEnv.mutate() : undefined}
              check={check}
              status={status}
            />
            {error && (
              <p data-signal="failed" className="text-xs text-danger">
                {errorText(error)}
              </p>
            )}
            <div className="mt-1 flex flex-wrap items-center gap-1.5">
              <button
                type="button"
                data-variant="primary"
                disabled={!command.trim() || save.isPending || (!!existing && !dirty && !checkFailed(check))}
                onClick={() => save.mutate()}
                className={SETTINGS_ACTION_PRIMARY}
              >
                {existing ? t("save and check") : t("add and check")}
              </button>
              {existing && (
                <button type="button" disabled={recheck.isPending || save.isPending} onClick={() => recheck.mutate()} className={SETTINGS_ACTION}>
                  <RefreshCw className="size-3" />
                  {t("check again")}
                </button>
              )}
              <button type="button" onClick={onClose} className={SETTINGS_ACTION}>
                {t("cancel")}
              </button>
              {described?.homepage && (
                <a href={described.homepage} target="_blank" rel="noreferrer" className="ml-auto flex items-center gap-1 text-2xs text-muted underline hover:text-fg">
                  {t("learn more")}
                  <ExternalLink className="size-3" />
                </a>
              )}
            </div>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}

/**
 * The details dialog, wired to the device: check, edit, remove. `checkError`
 * is why the last check of this agent was not run — the device refused it,
 * or could not be reached — which no `lastCheck` records. Why a check failed
 * is not in the list: it is read from the check's own route, which answers
 * only a device that manages the computer.
 */
function DetailsDialog({
  agent,
  status,
  confirmFirst,
  checking,
  checkError,
  onCheck,
  onEdit,
  onClose,
}: {
  agent: AcpAgentInfo;
  status: AcpStatus;
  confirmFirst: boolean;
  checking: boolean;
  checkError: string | null;
  onCheck: () => void;
  onEdit: () => void;
  onClose: () => void;
}) {
  const api = useApi();
  const machine = useMachine();
  const queryClient = useQueryClient();
  const [confirming, setConfirming] = useState(confirmFirst);
  const sessions = (useFeedSessions() ?? []).filter((session) => session.source === "acp" && session.acpAgent === agent.id).length;
  // The model list only as far as it was already read: opening the details must not start the agent.
  const { data: models } = useQuery<ModelList | undefined>({
    queryKey: mq(machine, "models", "acp", agent.id),
    queryFn: async () => undefined,
    enabled: false,
  });
  const failed = !!agent.lastCheck && checkFailed(agent.lastCheck);
  const full = useAcpCheck(agent.id, failed);
  // The full check only while it is the one the list names; a newer one is on its way.
  const fullCheck = full.data && full.data.checkedAt === agent.lastCheck?.checkedAt ? full.data : null;
  const withheld = full.error instanceof ApiError && full.error.code === "limited_device";
  const fullError = full.error && !withheld ? errorText(full.error) : null;
  const remove = useMutation({
    mutationFn: async () => {
      const res = await api.DELETE("/api/acp/agents/{id}", { params: { path: { id: agent.id } } });
      if (!res.data) throw new ApiError(res.error);
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: acpAgentsKey(machine) });
      onClose();
    },
  });
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent title={agent.name} className={DIALOG_WIDTH}>
        <AcpDetailsBody
          agent={agent}
          status={checking ? acpStatus(null, true) : fullCheck ? acpStatus(fullCheck) : status}
          sessions={sessions}
          models={models?.models.length ? models.models.length : null}
          confirming={confirming}
          busy={remove.isPending}
          error={remove.error ? errorText(remove.error) : (checkError ?? fullError)}
          errorWithheld={withheld}
          onCheck={onCheck}
          onEdit={onEdit}
          onAskRemove={() => setConfirming(true)}
          onConfirmRemove={() => remove.mutate()}
          onCancelRemove={() => {
            setConfirming(false);
            remove.reset();
          }}
        />
      </DialogContent>
    </Dialog>
  );
}

/**
 * The ACP tab, wired to the device. `openAgent` is the agent whose details the
 * address names (`&agent=<id>`); `onOpenAgent` keeps the address in step.
 */
export function AcpTab({ openAgent, onOpenAgent }: { openAgent: string | null; onOpenAgent: (id: string | null) => void }) {
  const api = useApi();
  const machine = useMachine();
  const queryClient = useQueryClient();
  const { data } = useAcpAgents();
  const agents = data?.agents ?? [];
  const presets = data?.presets ?? [];
  const [editing, setEditing] = useState<Editing | null>(null);
  const [confirmFirst, setConfirmFirst] = useState(false);
  const [checking, setChecking] = useState<ReadonlySet<string>>(new Set());

  const check = useMutation({
    mutationFn: (id: string) => runCheck(api, id),
    onMutate: (id) => setChecking((now) => new Set(now).add(id)),
    onSettled: (_result, _error, id) => {
      setChecking((now) => {
        const next = new Set(now);
        next.delete(id);
        return next;
      });
      void queryClient.invalidateQueries({ queryKey: acpAgentsKey(machine) });
    },
  });

  const statusOf = (agent: AcpAgentInfo) => acpStatus(agent.lastCheck, checking.has(agent.id));
  const open = openAgent ? agents.find((agent) => agent.id === openAgent) : undefined;
  const actions: AcpCardActions = {
    onOpen: (id) => {
      setConfirmFirst(false);
      onOpenAgent(id);
    },
    onCheck: (id) => check.mutate(id),
    onEdit: (id) => setEditing({ mode: "edit", id }),
    onRemove: (id) => {
      setConfirmFirst(true);
      onOpenAgent(id);
    },
  };

  return (
    <>
      <AcpTabView
        agents={agents}
        presets={presets}
        statusOf={statusOf}
        actions={actions}
        onAdd={(preset) => setEditing(preset ? { mode: "add", step: "form", preset } : { mode: "add", step: "pick" })}
      />
      {open && !editing && (
        <DetailsDialog
          key={`${open.id}-${confirmFirst}`}
          agent={open}
          status={statusOf(open)}
          confirmFirst={confirmFirst}
          checking={checking.has(open.id)}
          checkError={check.isError && check.variables === open.id ? errorText(check.error) : null}
          onCheck={() => check.mutate(open.id)}
          onEdit={() => setEditing({ mode: "edit", id: open.id })}
          onClose={() => {
            setConfirmFirst(false);
            onOpenAgent(null);
          }}
        />
      )}
      {editing && (
        <AgentDialog
          key={editing.mode === "edit" ? `edit-${editing.id}` : editing.step === "form" ? `add-${editing.preset}` : "add"}
          editing={editing}
          agents={agents}
          presets={presets}
          onStep={setEditing}
          onClose={() => setEditing(null)}
        />
      )}
    </>
  );
}
