/*
 * Step 01's last two blocks: the optional developer tools (git) row, and the
 * folded "Add accounts to
 * Droppen (advanced)" — the existing setup-token wizard, the existing env
 * block for a key or a company gateway, and a Codex OpenAI API key stored as
 * a Droppen account (the custom provider's env key, which the user never
 * sees). Every form posts to the same `POST /api/accounts` the Settings page
 * uses, which checks the credential before it keeps it.
 */
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import { Check, ChevronDown, ChevronRight, KeyRound } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "@/api/schema";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { EnvBlockField } from "@/features/accounts/env-block-field";
import { parseEnvBlock } from "@/features/accounts/env-block";
import { accountsKey, quotaKey } from "@/features/accounts/use-accounts";
import { SetupTokenDialog } from "@/features/settings/account-wizards";
import { cn } from "@/lib/cn";
import { useApi, useMachine } from "@/lib/machines/context";
import { setupToolsKey } from "./api";
import { CopyCommandButton, StatusChip } from "./tools-parts";
import type { ToolsReport } from "./types";

type AccountConflict = components["schemas"]["AccountConflict"];

/** The developer tools row: ✓ when `xcode-select -p` says so, else why and Apple's installer. */
export function GitRow({
  git,
  canInstall,
  online,
  command,
  started,
  checking,
  onInstall,
  onCheck,
}: {
  git: ToolsReport["helpers"]["git"];
  canInstall: boolean;
  online: boolean;
  /** The whitelisted `xcode-select --install`, for copying. */
  command: string | null;
  /** Apple's installer was asked for. */
  started: boolean;
  checking: boolean;
  onInstall: () => void;
  onCheck: () => void;
}) {
  const { t } = useTranslation();
  if (git === "unknown") return null;
  return (
    <div data-testid="tools-git" className="mt-4 grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-6 gap-y-2 border border-border px-3.5 py-3 max-[960px]:grid-cols-1">
      <div className="min-w-0">
        <div className="text-[13px] leading-[18px] font-semibold">
          {t("Developer tools (git)")} <span className="font-mono text-2xs font-normal text-faint">· {t("optional")}</span>
        </div>
        {git === "missing" && (
          <p className="mt-0.5 max-w-[46em] text-xs leading-[18px] text-muted">
            {started ? t("macOS opened Apple's installer. Finish it there, then check again.") : t("Claude Code and Codex use git in code projects. Without it, macOS interrupts the first session to install Apple's Command Line Tools; you can get that done now.")}
          </p>
        )}
      </div>
      <div className="flex items-center gap-2">
        {git === "ready" ? (
          <StatusChip tone="ready">{t("installed")}</StatusChip>
        ) : started ? (
          <Button data-testid="git-check" disabled={checking} onClick={onCheck}>
            {t("Check again")}
          </Button>
        ) : canInstall ? (
          <Button data-testid="git-install" disabled={!online} onClick={onInstall}>
            {t("Install (Apple)")}
          </Button>
        ) : command ? (
          <CopyCommandButton command={command} />
        ) : null}
      </div>
    </div>
  );
}

/** The three ways to add a Droppen account from here. */
export type AdvancedKind = "token" | "env" | "codex";

/** A label nobody typed: the kind of account, so a form can be one field shorter. */
export const ADVANCED_LABELS: Record<Exclude<AdvancedKind, "token">, string> = { env: "Claude · environment", codex: "Codex · API key" };

/** `POST /api/accounts` with an env block, the way the Settings page's form does. */
function useAddEnvAccount(onAdded: (label: string, unverified: string | null) => void) {
  const api = useApi();
  const machine = useMachine();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({ label, provider, env }: { label: string; provider: "claude" | "codex"; env: string }) => {
      const res = await api.POST("/api/accounts", { body: { label, provider, kind: "env", env } });
      if (res.error) {
        if (res.response.status === 409) throw new Error(`dup:${(res.error as AccountConflict).existingLabel}`);
        throw new Error(res.error.error);
      }
      return { label, unverified: res.data?.unverified ?? null };
    },
    onSuccess: ({ label, unverified }) => {
      void queryClient.invalidateQueries({ queryKey: accountsKey(machine) });
      void queryClient.invalidateQueries({ queryKey: quotaKey(machine) });
      // "Ready through the Droppen account …": detection counts it.
      void queryClient.invalidateQueries({ queryKey: setupToolsKey(machine) });
      onAdded(label, unverified);
    },
  });
}

function AddError({ error }: { error: Error | null }) {
  const { t } = useTranslation();
  if (!error) return null;
  const dup = error.message.startsWith("dup:") ? error.message.slice(4) : null;
  return (
    <span data-signal="failed" className="font-mono text-2xs text-danger">
      {dup !== null ? t("Already registered as {{label}}", { label: dup }) : error.message}
    </span>
  );
}

/**
 * "Add accounts to Droppen (advanced)", folded until asked for. `kind` and
 * `open` are the step's, so the shell-key line on a card can open it on the
 * right form.
 */
export function AdvancedAccounts({
  open,
  onOpen,
  kind,
  onKind,
  online,
}: {
  open: boolean;
  onOpen: (open: boolean) => void;
  kind: AdvancedKind;
  onKind: (kind: AdvancedKind) => void;
  online: boolean;
}) {
  const { t } = useTranslation();
  // Absent where the step is drawn on its own (the render tests): the link is then a plain one.
  const router = useRouter({ warn: false });
  const [wizard, setWizard] = useState(false);
  const [label, setLabel] = useState("");
  const [env, setEnv] = useState("");
  const [key, setKey] = useState("");
  const [added, setAdded] = useState<{ label: string; unverified: string | null } | null>(null);
  const add = useAddEnvAccount((name, unverified) => {
    setAdded({ label: name, unverified });
    setLabel("");
  });
  const block = parseEnvBlock(env, "claude");
  const options: { id: AdvancedKind; title: string; body: string | null }[] = [
    { id: "token", title: t("Claude · long-lived sign-in (setup-token)"), body: t("Sign in once in the browser; Droppen keeps a one-year token. Needed for automatic account switching.") },
    { id: "env", title: t("Environment variables (API key, gateway)"), body: t("One KEY=VALUE per line, given only to the sessions this account starts.") },
    { id: "codex", title: t("Codex · OpenAI API key"), body: null },
  ];
  const submit = (provider: "claude" | "codex") => {
    const name = label.trim() || t(ADVANCED_LABELS[provider === "claude" ? "env" : "codex"]);
    // The secret leaves the form's state as it is sent: never kept for a re-render.
    const body = provider === "claude" ? env : `OPENAI_API_KEY=${key.trim()}`;
    if (provider === "claude") setEnv("");
    else setKey("");
    setAdded(null);
    add.mutate({ label: name, provider, env: body });
  };
  return (
    <div data-testid="tools-advanced" className="mt-4 border border-border">
      <button
        type="button"
        data-testid="advanced-toggle"
        aria-expanded={open}
        onClick={() => onOpen(!open)}
        className="flex h-10 w-full items-center gap-2 px-3.5 text-left text-[13px] font-semibold hover:bg-hover"
      >
        {open ? <ChevronDown className="size-3" aria-hidden="true" /> : <ChevronRight className="size-3" aria-hidden="true" />}
        <span className="min-w-0 flex-1">{t("Add accounts to Droppen (advanced)")}</span>
        <KeyRound className="size-3.5 text-faint" aria-hidden="true" />
      </button>
      {open && (
        <div className="border-t border-border px-3.5 pt-3 pb-3.5">
          <p className="max-w-[46em] text-xs leading-[18px] text-muted">
            {t("For switching accounts automatically when one runs out of quota, or paying through an API key or a company gateway. Most people don't need this. Keys stay on this device and are never shown again.")}
          </p>
          <div role="radiogroup" aria-label={t("Add accounts to Droppen (advanced)")} className="mt-3 flex flex-col">
            {options.map((option) => {
              const on = option.id === kind;
              return (
                <button
                  key={option.id}
                  type="button"
                  role="radio"
                  aria-checked={on}
                  data-kind={option.id}
                  onClick={() => {
                    onKind(option.id);
                    add.reset();
                    setAdded(null);
                  }}
                  className={cn("-mt-px grid grid-cols-[13px_minmax(0,1fr)] items-start gap-x-3 border px-3 py-2.5 text-left first:mt-0", on ? "relative z-[1] border-fg" : "border-border hover:bg-hover")}
                >
                  <span aria-hidden="true" className={cn("mt-[3px] flex size-[13px] items-center justify-center rounded-full border", on ? "border-fg" : "border-border-strong")}>
                    {on && <span className="size-[7px] rounded-full bg-fg" />}
                  </span>
                  <span className="text-xs leading-[18px] font-semibold">{option.title}</span>
                  {option.body && <span className="col-start-2 text-2xs leading-4 text-muted">{option.body}</span>}
                </button>
              );
            })}
          </div>
          <div className="mt-3 flex max-w-[34rem] flex-col gap-2">
            {kind === "token" ? (
              <div>
                <Button data-testid="adv-token-start" disabled={!online} onClick={() => setWizard(true)}>
                  {t("Start")}
                </Button>
              </div>
            ) : (
              <>
                <label className="flex flex-col gap-1">
                  <span className="text-2xs text-faint">{t("Name")}</span>
                  <Input value={label} placeholder={t(ADVANCED_LABELS[kind])} onChange={(e) => setLabel(e.target.value)} />
                </label>
                {kind === "env" ? (
                  <EnvBlockField provider="claude" value={env} onChange={setEnv} issue={block.issue} warning={block.warning} />
                ) : (
                  <label className="flex flex-col gap-1">
                    <span className="font-mono text-2xs text-faint">OPENAI_API_KEY</span>
                    <Input type="password" autoComplete="off" spellCheck={false} value={key} placeholder="sk-…" onChange={(e) => setKey(e.target.value)} />
                  </label>
                )}
                <div className="flex items-center gap-2.5">
                  <Button
                    data-testid="adv-add"
                    disabled={add.isPending || (kind === "env" ? block.issue !== null : key.trim() === "")}
                    onClick={() => submit(kind === "env" ? "claude" : "codex")}
                  >
                    {t("Add account")}
                  </Button>
                  <AddError error={add.error} />
                  {added && !added.unverified && (
                    <span className="flex items-center gap-1 font-mono text-2xs text-ok">
                      <Check className="size-3" aria-hidden="true" />
                      {t("Added to Droppen as “{{label}}”.", { label: added.label })}
                    </span>
                  )}
                  {added?.unverified && (
                    <span data-signal="warn" className="font-mono text-2xs text-warn-fg">
                      {t("saved, but it could not be checked: {{why}}", { why: added.unverified })}
                    </span>
                  )}
                </div>
              </>
            )}
          </div>
          <a
            href="/settings?section=accounts"
            onClick={(event) => {
              if (!router) return;
              event.preventDefault();
              void router.navigate({ to: "/settings", search: { section: "accounts" } });
            }}
            className="mt-3 inline-block font-mono text-2xs text-muted underline underline-offset-2 hover:text-fg">
            {t("Rotation and the full list are in Settings → Accounts.")}
          </a>
        </div>
      )}
      <SetupTokenDialog open={wizard} onOpenChange={setWizard} />
    </div>
  );
}
