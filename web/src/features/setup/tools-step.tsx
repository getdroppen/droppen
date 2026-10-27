/*
 * Step 01 · AI tools: a card each for Claude Code and Codex in every state
 * — checking, not installed, the confirm box with the server's command shown
 * verbatim, installing in an embedded terminal, failed, signed out, signing
 * in (and when the browser didn't open), ready with the plan and the quota,
 * update needed, not found, found elsewhere, the plan doesn't include it, a
 * key in the shell — then the developer tools row and the advanced accounts.
 *
 * The cards' states are `tools-model.ts`'s; what the user opened on a card
 * (`ToolsUi`) lives in the guide, which also asks it who holds the page's one
 * primary. Installing and signing in run only from the app on this Mac
 * (`canInstall`); elsewhere the cards show the command to copy.
 */

import { useQuery } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useFeedSettings } from "@/app/global-feed";
import { quotaKey, type Quota } from "@/features/accounts/use-accounts";
import { useApi, useMachine } from "@/lib/machines/context";
import { SetupActionError, useSetupActions, useSetupCommands, useSetupTools } from "./api";
import { FootNote } from "./frame";
import type { StepProps } from "./screens";
import { showSetupToast } from "./toast";
import { IDLE_STATUS, SIGNED_IN_TOAST, ToolCard, refusal, type ToolActs, type ToolsStatus } from "./tool-card";
import { AdvancedAccounts, GitRow, type AdvancedKind } from "./tools-advanced";
import { IDLE_TOOLS_UI, LOGIN_POLL_MS, cardsOf, f6Next, liveTask, pollsLogin, primaryOwner, toolsPoll, type CardUi, type ToolsUi } from "./tools-model";
import type { SetupCommand, ToolId, ToolsReport } from "./types";

export { ApiKeyForm, IDLE_STATUS, REFUSALS, SIGNED_IN_TOAST, STEP01_STRINGS } from "./tool-card";
export type { ToolActs, ToolsStatus } from "./tool-card";

/** What step 01 is given besides {@link StepProps}: the cards' opened forms, kept by the guide. */
export interface ToolsStepUi {
  toolsUi?: ToolsUi;
  onToolsUi?: (next: ToolsUi) => void;
}

/** Step 01 as drawn, given everything (the render tests draw it directly). */
export function ToolsStepView({
  report,
  commands,
  quota,
  homeDir,
  online,
  ui,
  status,
  acts,
  advanced,
}: {
  report: ToolsReport | null | undefined;
  commands: SetupCommand[];
  quota: Quota | null | undefined;
  homeDir: string;
  online: boolean;
  ui: ToolsUi;
  status: ToolsStatus;
  acts: ToolActs;
  advanced: { open: boolean; kind: AdvancedKind; onOpen: (open: boolean) => void; onKind: (kind: AdvancedKind) => void };
}) {
  const { t } = useTranslation();
  const cards = cardsOf(report, ui);
  const owner = primaryOwner(cards);
  const gitCommand = commands.find((c) => c.tool === "git" && c.kind === "install")?.command ?? null;
  const noneReady = report != null && !report.tools.some((tool) => tool.ready);
  return (
    <div data-testid="tools-step" className="max-w-[760px]">
      {cards.map((card) => (
        <ToolCard
          key={card.id}
          id={card.id}
          tool={card.tool}
          state={card.state}
          ui={card.ui}
          report={report ?? undefined}
          commands={commands}
          quota={quota}
          primary={owner === card.id}
          online={online}
          status={status}
          acts={acts}
          homeDir={homeDir}
        />
      ))}
      {report && (
        <GitRow
          git={report.helpers.git}
          canInstall={report.canInstall}
          online={online}
          command={gitCommand}
          started={status.git === "started"}
          checking={status.rescanning}
          onInstall={acts.installGit}
          onCheck={() => acts.rescan(false)}
        />
      )}
      <AdvancedAccounts open={advanced.open} onOpen={advanced.onOpen} kind={advanced.kind} onKind={advanced.onKind} online={online} />
      {noneReady && <FootNote>{t("Without Claude Code or Codex, Droppen can show files and open terminals but can't run AI. Come back any time from Settings → General → Setup.")}</FootNote>}
    </div>
  );
}

/** Whether the page is visible, as it changes (polling holds while it is not). */
function useVisible(): boolean {
  const [visible, setVisible] = useState(() => typeof document === "undefined" || document.visibilityState !== "hidden");
  useEffect(() => {
    const update = () => setVisible(document.visibilityState !== "hidden");
    document.addEventListener("visibilitychange", update);
    return () => document.removeEventListener("visibilitychange", update);
  }, []);
  return visible;
}

/**
 * F6 moves the focus between each embedded terminal and the action bar; a
 * terminal never takes it by itself. Caught before xterm sees the
 * key, so the CLI is never sent it.
 */
function useF6(active: boolean) {
  useEffect(() => {
    if (!active) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "F6" || event.altKey || event.metaKey || event.ctrlKey) return;
      const terminals = [...document.querySelectorAll<HTMLElement>("[data-setup-terminal]")];
      const bar = document.querySelector<HTMLElement>("[data-testid='action-bar']");
      const regions = bar ? [...terminals, bar] : terminals;
      if (regions.length === 0) return;
      event.preventDefault();
      event.stopPropagation();
      const focused = document.activeElement;
      const current = regions.findIndex((region) => focused instanceof Node && region.contains(focused));
      const next = regions[f6Next(regions.length, current, event.shiftKey)];
      if (!next) return;
      if (next === bar) {
        (bar.querySelector<HTMLElement>("[data-guide-primary]") ?? bar.querySelector<HTMLElement>("button:not(:disabled)"))?.focus();
      } else {
        // xterm's own input: focusing it is focusing the terminal.
        next.querySelector<HTMLElement>("textarea")?.focus();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [active]);
}

/**
 * Step 01, live: the report polled while a task runs, each running sign-in
 * checked every 2 s while the page is visible (at most ten minutes), the
 * toast when one lands, and the actions.
 */
export function ToolsStep({ tools, online, toolsUi = IDLE_TOOLS_UI, onToolsUi }: StepProps & ToolsStepUi) {
  const api = useApi();
  const machine = useMachine();
  const actions = useSetupActions();
  // The report as the guide has it; polled here while a task runs.
  useSetupTools(true, toolsPoll(tools));
  const commands = useSetupCommands(tools != null).data?.commands ?? [];
  const anyReady = tools?.tools.some((tool) => tool.ready) ?? false;
  // The reading every composer shows: asked only once a card has bars to draw; never a probe of its own.
  const quota = useQuery({
    queryKey: quotaKey(machine),
    queryFn: async () => (await api.GET("/api/stats/quota")).data ?? null,
    enabled: anyReady,
    staleTime: 60_000,
  }).data;
  const homeDir = useFeedSettings()?.homeDir ?? "";
  const visible = useVisible();
  const [localUi, setLocalUi] = useState<ToolsUi>(toolsUi);
  const ui = onToolsUi ? toolsUi : localUi;
  const uiRef = useRef(ui);
  uiRef.current = ui;
  const setUi = (next: ToolsUi) => (onToolsUi ? onToolsUi(next) : setLocalUi(next));
  const patch = (id: ToolId, change: Partial<CardUi>) => setUi({ ...uiRef.current, [id]: { ...uiRef.current[id], ...change } });
  const [status, setStatus] = useState<ToolsStatus>(IDLE_STATUS);
  const setBusy = (id: ToolId, busy: boolean, error?: string) =>
    setStatus((s) => ({ ...s, busy: { ...s.busy, [id]: busy }, errors: { ...s.errors, [id]: error } }));
  const [advanced, setAdvanced] = useState<{ open: boolean; kind: AdvancedKind }>({ open: false, kind: "token" });
  const report = tools ?? undefined;

  // Each running sign-in, checked while the page is visible.
  const signingIn = (report?.tools ?? []).filter((tool) => pollsLogin(tool, ui[tool.id], visible, Date.now())).map((tool) => tool.id);
  const signingKey = signingIn.join(",");
  useEffect(() => {
    if (!signingKey) return;
    const ids = signingKey.split(",") as ToolId[];
    let stopped = false;
    const timer = setInterval(() => {
      for (const id of ids) {
        void actions.loginStatus(id).then(
          (login) => {
            if (stopped || login.state !== "signed-in") return;
            showSetupToast(SIGNED_IN_TOAST[id]);
            patch(id, { noBrowser: false, open: "none" });
          },
          () => undefined,
        );
      }
    }, LOGIN_POLL_MS);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
    // `patch` reads the latest UI through its ref.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [signingKey, actions]);

  useF6((report?.tools ?? []).some((tool) => liveTask(tool, ui[tool.id])?.running && liveTask(tool, ui[tool.id])?.terminalId));

  const attempt = async (id: ToolId, work: () => Promise<unknown>) => {
    setBusy(id, true);
    try {
      await work();
      setBusy(id, false);
    } catch (error) {
      setBusy(id, false, refusal(error));
    }
  };
  const taskOf = (id: ToolId) => report?.tools.find((tool) => tool.id === id)?.task ?? null;

  const acts: ToolActs = {
    ui: patch,
    run: (id, kind, method) =>
      void attempt(id, async () => {
        await actions.run(id, kind, method);
        patch(id, { open: "none", method: null });
      }),
    login: (id, mode) =>
      void attempt(id, async () => {
        const running = taskOf(id);
        // A sign-in already running gives way to the new one ("Sign in with a code").
        if (running?.running && running.terminalId) {
          await actions.stop(running.terminalId).catch(() => undefined);
          await startLogin(() => actions.login(id, mode));
        } else {
          await actions.login(id, mode);
        }
        patch(id, { open: "none", noBrowser: false, dismissed: null });
      }),
    loginWithKey: (id, apiKey) =>
      void attempt(id, async () => {
        patch(id, { open: "none" });
        await actions.login(id, "apikey", apiKey);
      }),
    stop: (id) => {
      const task = taskOf(id);
      if (!task) return;
      patch(id, { dismissed: task.startedAt, noBrowser: false });
      if (task.terminalId) void attempt(id, () => actions.stop(task.terminalId!));
    },
    dismiss: (id) => {
      const task = taskOf(id);
      if (task) patch(id, { dismissed: task.startedAt });
    },
    rescan: (patient) => {
      // Re-reading the login shell is the app's own window's to ask (it
      // rewrites every child's PATH); anywhere else, detection runs again.
      if (!report?.canInstall) {
        actions.refresh();
        return;
      }
      setStatus((s) => ({ ...s, rescanning: true }));
      void actions.rescan(patient).finally(() => setStatus((s) => ({ ...s, rescanning: false })));
    },
    use: (id, path) =>
      void attempt(id, async () => {
        await actions.use(id, path);
        patch(id, { open: "none" });
      }),
    installGit: () => {
      void actions.installGit().then(
        () => setStatus((s) => ({ ...s, git: "started" })),
        () => setStatus((s) => ({ ...s, git: "started" })),
      );
    },
    openAdvanced: (kind) => setAdvanced({ open: true, kind }),
  };

  return (
    <ToolsStepView
      report={tools}
      commands={commands}
      quota={quota}
      homeDir={homeDir}
      online={online}
      ui={ui}
      status={status}
      acts={acts}
      advanced={{ ...advanced, onOpen: (open) => setAdvanced((a) => ({ ...a, open })), onKind: (kind) => setAdvanced((a) => ({ ...a, kind })) }}
    />
  );
}

/** Start a sign-in right after stopping one: the old task may still be finishing (409 busy) for a moment. */
async function startLogin(start: () => Promise<unknown>): Promise<void> {
  for (let tries = 0; ; tries++) {
    try {
      await start();
      return;
    } catch (error) {
      if (!(error instanceof SetupActionError) || error.code !== "busy" || tries >= 6) throw error;
      await new Promise((resolve) => setTimeout(resolve, 500));
    }
  }
}
