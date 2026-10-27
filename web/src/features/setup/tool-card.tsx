/*
 * Step 01's cards: the state words and facts that describe one AI tool, the
 * small forms it opens (an API key, a path, the confirm box), the card body
 * for each state, and the card's header. `tools-step.tsx` owns the live step:
 * polling, the actions and the view around these cards.
 */
import { Menu } from "@base-ui-components/react/menu";
import { Ellipsis, KeyRound } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { LiveDot } from "@/components/live-dot";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { Quota, QuotaWindow } from "@/features/accounts/use-accounts";
import { shortenHome } from "@/lib/format";
import { SetupActionError } from "./api";
import { TOOL_NAME } from "./model";
import { ADVANCED_LABELS, type AdvancedKind } from "./tools-advanced";
import {
  type CardState,
  type CardUi,
  CLAUDE_PLANS_URL,
  chosenCommand,
  defaultLoginMode,
  docsFor,
  failedLogin,
  FAILURE_WORDS,
  failureWords,
  liveTask,
  loginCommand,
  methodChoices,
  offeredMethods,
  readyWindows,
  UNAVAILABLE_WORDS,
  unavailableReason,
} from "./tools-model";
import {
  Actions,
  Blocked,
  CommandWell,
  CopyCommandButton,
  DocsLink,
  LinkButton,
  MainButton,
  QuotaBar,
  Segmented,
  StatusChip,
  TailBlock,
  TaskTerminal,
  type ChipTone,
} from "./tools-parts";
import type { LoginMode, SetupCommand, SetupTool, ToolId, ToolsReport } from "./types";

/** The status words (`tools.st.*`) and colours of each state. */
const CHIP: Record<CardState, { label: string; tone: ChipTone }> = {
  checking: { label: "checking", tone: "neutral" },
  notInstalled: { label: "not installed", tone: "neutral" },
  confirm: { label: "not installed", tone: "neutral" },
  installing: { label: "installing", tone: "running" },
  failed: { label: "failed", tone: "failed" },
  signedOut: { label: "not signed in", tone: "neutral" },
  signingIn: { label: "signing in", tone: "running" },
  ready: { label: "ready", tone: "ready" },
  tooOld: { label: "update needed", tone: "warn" },
  notFound: { label: "not found", tone: "neutral" },
  fallback: { label: "found elsewhere", tone: "neutral" },
  plan: { label: "plan doesn't include it", tone: "failed" },
  apiKey: { label: "not signed in", tone: "neutral" },
  choosePath: { label: "not found", tone: "neutral" },
};

/** The words for a state that depend on the task's kind (an update is not an install). */
function chipOf(state: CardState, tool: SetupTool | undefined, ui: CardUi): { label: string; tone: ChipTone } {
  const task = tool ? liveTask(tool, ui) : null;
  if (state === "installing" && task?.kind === "update") return { label: "updating", tone: "running" };
  if (state === "confirm" && ui.kind === "update") return CHIP.tooOld;
  return CHIP[state];
}

const SIGNED_OUT: Record<ToolId, string> = {
  claude: "Installed. Sign in once and Droppen can start Claude Code sessions. Your browser opens; sign in with your Claude account.",
  codex: "Installed. Sign in once and Droppen can start Codex sessions. Your browser opens; sign in with ChatGPT.",
};
const NOT_INSTALLED: Record<ToolId, string> = { claude: "Claude Code isn't on this device yet.", codex: "Codex isn't on this device yet." };
const NEEDS: Record<ToolId, string> = {
  claude: "Needs a Claude Pro, Max, Team or Enterprise plan, or an Anthropic Console account.",
  codex: "Works with a ChatGPT plan (Plus, Pro, Business, Edu, Enterprise; Free and Go with lower limits) or an OpenAI API key.",
};
const INSTALLING: Record<ToolId, string> = {
  claude: "Installing Claude Code. It keeps going if you leave this step.",
  codex: "Installing Codex. It keeps going if you leave this step.",
};
const UPDATING: Record<ToolId, string> = {
  claude: "Updating Claude Code. It keeps going if you leave this step.",
  codex: "Updating Codex. It keeps going if you leave this step.",
};
const TOO_OLD: Record<ToolId, string> = {
  claude: "Claude Code {{v}} is too old for Droppen; it needs {{min}} or later.",
  codex: "Codex {{v}} is too old for Droppen; it needs {{min}} or later.",
};
const NO_BROWSER: Record<ToolId, string> = {
  claude: "Press c in the terminal to copy the sign-in link, open it in any browser, then paste the code it shows back into the terminal.",
  codex: "Use a one-time code instead: Droppen runs codex login --device-auth and shows a code to enter at chatgpt.com.",
};
const SHELL_KEY: Record<ToolId, string> = {
  claude: "Your Terminal sets ANTHROPIC_API_KEY. Droppen doesn't take keys from your shell settings; to use that key here, add it to Droppen below.",
  codex: "Your Terminal sets OPENAI_API_KEY. Droppen doesn't take keys from your shell settings; to use that key here, add it to Droppen below.",
};
/** The toast when a sign-in lands. */
export const SIGNED_IN_TOAST: Record<ToolId, string> = {
  claude: "Claude Code is signed in. You can go back to Droppen.",
  codex: "Codex is signed in. You can go back to Droppen.",
};
/** The facts under an install command: from where, into where, needing what. */
function factsOf(command: SetupCommand, node: string | null): { key: string; vars?: Record<string, string> }[] {
  if (command.method === "brew") return [{ key: "Uses your Homebrew. Homebrew copies don't update themselves; Droppen offers the update when one is out." }];
  if (command.method === "npm") return [{ key: "Uses your Node.js ({{node}}). Only if you already manage tools with npm.", vars: { node: node ?? "Node.js" } }];
  return command.tool === "claude"
    ? [
        { key: "From Anthropic (claude.ai): the command in Claude Code's own install guide." },
        { key: "Installs into your home folder (~/.local/bin/claude). No administrator password, no Node.js." },
        { key: "Updates itself in the background afterwards. Takes about a minute." },
      ]
    : [
        { key: "From OpenAI (chatgpt.com): the command in Codex's own README." },
        { key: "Installs into ~/.local/bin/codex and checks every download's SHA-256. No administrator password, no Node.js." },
        { key: "Adds ~/.local/bin to PATH in ~/.zprofile so your own Terminal finds codex too. That is what the official installer does anyway." },
      ];
}

const METHOD_LABEL: Record<string, string> = { native: "Official installer", brew: "Homebrew", npm: "npm" };

/** The official installer's own advice after it ran: Droppen finds claude itself; the user's own Terminal may not. */
const PATH_HINT = `echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc`;

/** What the step's actions are, for the cards to call. */
export interface ToolActs {
  /** Change what is open on a card. */
  ui: (id: ToolId, patch: Partial<CardUi>) => void;
  /** Run the confirm box's command (an install or update). */
  run: (id: ToolId, kind: "install" | "update", method: string) => void;
  login: (id: ToolId, mode: LoginMode) => void;
  /** Sign Codex in with an API key: it is in this call and nowhere else. */
  loginWithKey: (id: ToolId, apiKey: string) => void;
  /** Stop a running task's terminal and put the task away. */
  stop: (id: ToolId) => void;
  /** Put a finished task away. */
  dismiss: (id: ToolId) => void;
  rescan: (patient: boolean) => void;
  use: (id: ToolId, path: string) => void;
  installGit: () => void;
  openAdvanced: (kind: AdvancedKind) => void;
}

/** What is happening besides the report: requests in flight and refusals, per tool. */
export interface ToolsStatus {
  busy: Partial<Record<ToolId, boolean>>;
  errors: Partial<Record<ToolId, string>>;
  rescanning: boolean;
  git: "idle" | "started";
}

export const IDLE_STATUS: ToolsStatus = { busy: {}, errors: {}, rescanning: false, git: "idle" };

/** One tool's quota reading for its ready card (`GET /api/stats/quota`, the local login). */
function quotaOf(quota: Quota | null | undefined, id: ToolId): { plan: string | null; windows: QuotaWindow[] } {
  const reading = id === "claude" ? quota?.claude : quota?.codex;
  return { plan: reading?.plan ?? null, windows: reading?.windows ?? [] };
}

/** A refused action in words (i18n keys). */
export const REFUSALS = {
  busy: "Something is already running for this tool. Wait for it, or stop it.",
  appOnly: "These run from the Droppen app on this Mac. Copy the command and run it in your own Terminal.",
  path: "That isn't a copy Droppen can use. Pick the program itself, named claude or codex.",
  other: "That didn't work. Try again.",
} as const;

/** Every string step 01 shows through a variable (the i18n test checks them all). */
export const STEP01_STRINGS: string[] = [
  ...Object.values(CHIP).map((c) => c.label),
  "updating",
  ...[SIGNED_OUT, NOT_INSTALLED, NEEDS, INSTALLING, UPDATING, TOO_OLD, NO_BROWSER, SHELL_KEY, SIGNED_IN_TOAST].flatMap((words) => Object.values(words)),
  ...Object.values(METHOD_LABEL),
  ...Object.values(REFUSALS),
  ...Object.values(FAILURE_WORDS),
  ...Object.values(UNAVAILABLE_WORDS),
  ...Object.values(ADVANCED_LABELS),
  ...(["claude", "codex"] as const).flatMap((tool) =>
    (["native", "brew", "npm"] as const).flatMap((method) => factsOf({ tool, method } as SetupCommand, null).map((f) => f.key)),
  ),
  "5 hours",
  "this week",
];

/** A refusal in words. */
export function refusal(error: unknown): string {
  if (error instanceof SetupActionError) {
    if (error.code === "busy") return REFUSALS.busy;
    if (error.code === "app_only") return REFUSALS.appOnly;
    if (error.status === 400) return REFUSALS.path;
  }
  return REFUSALS.other;
}


/** The API key form of Codex's "Use an OpenAI API key instead": the key is cleared from the field as it is sent. */
export function ApiKeyForm({ primary, online, onSubmit, onCancel }: { primary: boolean; online: boolean; onSubmit: (key: string) => void; onCancel: () => void }) {
  const { t } = useTranslation();
  const [key, setKey] = useState("");
  const send = () => {
    const value = key.trim();
    if (!value) return;
    // Out of this component's state before the request even starts.
    setKey("");
    onSubmit(value);
  };
  return (
    <div data-testid="apikey-form" className="mt-1 border border-fg p-3">
      <label className="flex max-w-[28rem] flex-col gap-1">
        <span className="text-xs font-semibold">{t("OpenAI API key")}</span>
        <Input
          type="password"
          autoComplete="off"
          spellCheck={false}
          data-testid="apikey-input"
          value={key}
          placeholder="sk-…"
          onChange={(e) => setKey(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") send();
          }}
        />
      </label>
      <p className="mt-1.5 max-w-[46em] text-2xs leading-4 text-muted">{t("Droppen hands the key to codex login --with-api-key once, through its input. Codex keeps it; Droppen doesn't.")}</p>
      <Actions>
        <MainButton primary={primary} testId="apikey-submit" disabled={!online || key.trim() === ""} onClick={send}>
          {t("Sign in")}
        </MainButton>
        <Button variant="ghost" onClick={onCancel}>
          {t("Cancel")}
        </Button>
      </Actions>
      <Blocked online={online} canInstall />
    </div>
  );
}

/** "Show where it is…": a path to adopt (`POST /api/setup/tools/{tool}/use`). */
function ChoosePathForm({ id, primary, onUse, onCancel, busy }: { id: ToolId; primary: boolean; onUse: (path: string) => void; onCancel: () => void; busy: boolean }) {
  const { t } = useTranslation();
  const [path, setPath] = useState("");
  return (
    <div data-testid="choose-path" className="mt-1 border border-fg p-3">
      <label className="flex flex-col gap-1">
        <span className="text-xs font-semibold">{t("Where is it?")}</span>
        <Input
          autoFocus
          spellCheck={false}
          value={path}
          placeholder={id === "claude" ? "~/.nvm/versions/node/v22.12.0/bin/claude" : "~/.volta/bin/codex"}
          className="font-mono"
          onChange={(e) => setPath(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && path.trim()) onUse(path.trim());
          }}
        />
      </label>
      <p className="mt-1.5 text-2xs leading-4 text-muted">{t("In your own Terminal, `which {{name}}` prints it.", { name: id })}</p>
      <Actions>
        <MainButton primary={primary} disabled={busy || path.trim() === ""} onClick={() => onUse(path.trim())}>
          {t("Use this one")}
        </MainButton>
        <Button variant="ghost" onClick={onCancel}>
          {t("Cancel")}
        </Button>
      </Actions>
    </div>
  );
}

/** The confirm box: the method, the command verbatim, three facts, and then — only then — Run it here. */
function ConfirmBox({
  tool,
  ui,
  commands,
  report,
  primary,
  online,
  busy,
  acts,
}: {
  tool: SetupTool;
  ui: CardUi;
  commands: SetupCommand[];
  report: ToolsReport;
  primary: boolean;
  online: boolean;
  busy: boolean;
  acts: ToolActs;
}) {
  const { t } = useTranslation();
  const offered = offeredMethods(commands, tool, ui.kind);
  const chosen = chosenCommand(commands, tool, ui);
  const { unavailable } = methodChoices(commands, tool.id, ui.kind);
  const docs = docsFor(commands, tool.id);
  return (
    <div data-testid="confirm-box" className="border border-fg p-3.5">
      {offered.length > 1 && (
        <div className="mb-3 flex flex-wrap items-center gap-3">
          <span className="text-xs font-semibold">{t("How to install")}</span>
          <Segmented
            label={t("How to install")}
            value={chosen?.method ?? ""}
            options={offered.map((c) => ({
              value: c.method,
              label: (
                <>
                  {t(METHOD_LABEL[c.method] ?? c.method)}
                  {c.recommended && <span className="opacity-70"> · {t("recommended")}</span>}
                </>
              ),
            }))}
            onChange={(method) => acts.ui(tool.id, { method })}
          />
        </div>
      )}
      {chosen ? (
        <>
          <div className="mb-2 text-[13px] font-semibold">{t("Droppen will run this command")}</div>
          <CommandWell command={chosen.command} testId="confirm-command" />
          {ui.kind === "install" && (
            <ul className="mt-2.5 flex flex-col gap-1">
              {factsOf(chosen, report.helpers.node).map((fact) => (
                <li key={fact.key} className="grid grid-cols-[14px_minmax(0,1fr)] gap-2 text-xs leading-[18px] text-muted">
                  <span aria-hidden="true" className="text-fg">
                    ✓
                  </span>
                  <span>{t(fact.key, fact.vars)}</span>
                </li>
              ))}
            </ul>
          )}
        </>
      ) : (
        <p data-testid="confirm-none" className="text-xs leading-[18px] text-fg">
          {t("Droppen can't install it on this device. The official guide lists the other ways.")}
        </p>
      )}
      {unavailable.length > 0 && (
        <ul data-testid="confirm-unavailable" className="mt-2 flex flex-col gap-0.5">
          {unavailable.map((c) => (
            <li key={c.method} className="font-mono text-2xs leading-4 text-faint">
              {t(METHOD_LABEL[c.method] ?? c.method)}: {t(unavailableReason(c))}
            </li>
          ))}
        </ul>
      )}
      <Actions>
        {chosen && report.canInstall && (
          <MainButton primary={primary} testId="run-here" disabled={!online || busy} onClick={() => acts.run(tool.id, ui.kind, chosen.method)}>
            {t("Run it here")}
          </MainButton>
        )}
        {chosen && <CopyCommandButton command={chosen.command} primary={primary && !report.canInstall} />}
        <Button variant="ghost" onClick={() => acts.ui(tool.id, { open: "none", method: null })}>
          {t("Cancel")}
        </Button>
        <DocsLink href={docs} label={t("Official guide")} className="ml-auto" />
      </Actions>
      {chosen && <Blocked online={online} canInstall={report.canInstall} />}
    </div>
  );
}

/** The body of one card, by its state. */
function CardBody({
  id,
  tool,
  state,
  ui,
  report,
  commands,
  quota,
  primary,
  online,
  status,
  acts,
}: {
  id: ToolId;
  tool: SetupTool | undefined;
  state: CardState;
  ui: CardUi;
  report: ToolsReport | undefined;
  commands: SetupCommand[];
  quota: Quota | null | undefined;
  primary: boolean;
  online: boolean;
  status: ToolsStatus;
  acts: ToolActs;
}) {
  const { t, i18n } = useTranslation();
  if (state === "checking" || !tool || !report) {
    return (
      <div className="flex items-center gap-2 text-xs text-muted">
        <LiveDot className="text-muted" />
        {t("Already looking for Claude Code and Codex on this device…")}
      </div>
    );
  }
  const busy = status.busy[id] === true;
  const error = status.errors[id];
  const docs = docsFor(commands, id);
  const task = liveTask(tool, ui);
  const can = report.canInstall;
  const loginMode = defaultLoginMode(id);
  const loginCmd = loginCommand(commands, id, loginMode);
  const shellKey =
    tool.shellKey && !tool.ready ? (
      <div data-testid="shell-key" className="mt-3 grid grid-cols-[14px_minmax(0,1fr)] items-start gap-2 border-t border-border pt-3 text-xs leading-[18px] text-muted">
        <KeyRound className="mt-0.5 size-3.5 text-warn-fg" aria-hidden="true" />
        <div>
          <span>{t(SHELL_KEY[id])}</span>{" "}
          <LinkButton onClick={() => acts.openAdvanced(id === "claude" ? "env" : "codex")}>{t("Add it to Droppen")}</LinkButton>
        </div>
      </div>
    ) : null;
  const errorLine = error ? (
    <p data-signal="failed" role="alert" className="mt-2 text-2xs leading-4 text-danger">
      {t(error)}
    </p>
  ) : null;
  const pathHint =
    id === "claude" && task && !task.running && task.kind === "install" && task.method === "native" && task.exitCode === 0 ? (
      <div data-testid="path-hint" className="mt-3 border-t border-border pt-3">
        <p className="mb-1.5 text-2xs leading-4 text-muted">{t("Want claude in your own Terminal too? The installer leaves that to you; this line adds ~/.local/bin to your PATH:")}</p>
        <CommandWell command={PATH_HINT} />
      </div>
    ) : null;

  switch (state) {
    case "notInstalled":
      return (
        <>
          <p className="text-xs leading-[18px]">{t(NOT_INSTALLED[id])}</p>
          <p className="mt-1 text-xs leading-[18px] text-muted">{t(NEEDS[id])}</p>
          <Actions>
            <MainButton primary={primary} testId="install" onClick={() => acts.ui(id, { open: "confirm", kind: "install", method: null })}>
              {t("Install…")}
            </MainButton>
            <DocsLink href={docs} label={t("Official guide")} />
          </Actions>
        </>
      );
    case "confirm":
      return (
        <>
          <ConfirmBox tool={tool} ui={ui} commands={commands} report={report} primary={primary} online={online} busy={busy} acts={acts} />
          {errorLine}
        </>
      );
    case "installing":
    case "signingIn": {
      if (!task) return null;
      const signing = state === "signingIn";
      // The API key sign-in runs without a terminal: nothing to watch but the check.
      if (!task.terminalId) {
        return (
          <p className="flex items-center gap-2 text-xs leading-[18px] text-muted">
            <LiveDot className="bg-ok" />
            {t("Signing in with the API key…")}
          </p>
        );
      }
      return (
        <>
          <p className="text-xs leading-[18px]">{t(signing ? "Finish signing in in your browser, then come back. Droppen notices by itself." : task.kind === "update" ? UPDATING[id] : INSTALLING[id])}</p>
          <TaskTerminal terminalId={task.terminalId} startedAt={task.startedAt} onStop={signing ? null : () => acts.stop(id)} />
          {signing ? (
            <>
              <Actions>
                <span className="inline-flex items-center gap-2 text-xs text-muted">
                  <LiveDot className="bg-ok" />
                  {t("Waiting for sign-in…")}
                </span>
                {!ui.noBrowser && <LinkButton testId="no-browser-link" onClick={() => acts.ui(id, { noBrowser: true })}>{t("Browser didn't open?")}</LinkButton>}
                <Button variant="ghost" className="ml-auto" onClick={() => acts.stop(id)}>
                  {t("Cancel")}
                </Button>
              </Actions>
              {ui.noBrowser && (
                <div data-testid="no-browser" className="mt-2.5 border border-fg p-3">
                  <p className="text-xs leading-[18px]">{t(NO_BROWSER[id])}</p>
                  {id === "codex" && (
                    <Actions>
                      <MainButton primary={primary} testId="use-code" disabled={!online || busy} onClick={() => acts.login(id, "device")}>
                        {t("Sign in with a code")}
                      </MainButton>
                    </Actions>
                  )}
                </div>
              )}
            </>
          ) : null}
          <p className="mt-2 font-mono text-2xs text-faint">{t("F6 moves between the terminal and the buttons")}</p>
          {errorLine}
        </>
      );
    }
    case "failed": {
      if (!task) return null;
      const words = failureWords(task);
      return (
        <>
          <p data-signal="failed" className="text-xs leading-[18px] text-danger">
            {t(words.title, { code: words.code ?? "?" })}
            {/^(zh|ja)/.test(i18n.language) ? "" : " "}
            {t(words.why)}
          </p>
          <TailBlock lines={task.tail} />
          <Actions>
            {can && (
              <MainButton
                primary={primary}
                testId="retry"
                disabled={!online || busy}
                onClick={() => {
                  acts.dismiss(id);
                  acts.run(id, task.kind === "update" ? "update" : "install", task.method);
                }}
              >
                {t("Try again")}
              </MainButton>
            )}
            <CopyCommandButton command={task.command} primary={primary && !can} />
            <DocsLink href={docs} label={t("Official guide")} />
          </Actions>
          <Blocked online={online} canInstall={can} />
          {errorLine}
        </>
      );
    }
    case "signedOut": {
      const failed = failedLogin(tool, task) ? task : null;
      return (
        <>
          <p className="text-xs leading-[18px]">{t(SIGNED_OUT[id])}</p>
          <p className="mt-1 text-xs leading-[18px] text-muted">{t(NEEDS[id])}</p>
          {failed && (
            <p data-signal="failed" className="mt-1.5 text-xs leading-[18px] text-danger">
              {t("Signing in didn't finish (exit code {{code}}). Try again.", { code: failed.exitCode ?? "?" })}
            </p>
          )}
          {can ? (
            <Actions>
              <MainButton primary={primary} testId="sign-in" disabled={!online || busy} onClick={() => acts.login(id, loginMode)}>
                {t("Sign in")}
              </MainButton>
              {id === "claude" ? (
                <>
                  <LinkButton disabled={!online || busy} onClick={() => acts.login(id, "console")}>
                    {t("Use an Anthropic Console account instead")}
                  </LinkButton>
                  <MoreMenu disabled={!online || busy} onSso={() => acts.login(id, "sso")} />
                </>
              ) : (
                <LinkButton testId="use-api-key" disabled={busy} onClick={() => acts.ui(id, { open: "apiKey" })}>
                  {t("Use an OpenAI API key instead")}
                </LinkButton>
              )}
            </Actions>
          ) : (
            loginCmd && (
              <>
                <div className="mt-3">
                  <CommandWell command={loginCmd.command} />
                </div>
                <Actions>
                  <CopyCommandButton command={loginCmd.command} primary={primary} />
                </Actions>
              </>
            )
          )}
          <Blocked online={online} canInstall={can} />
          {errorLine}
          {pathHint}
          {shellKey}
        </>
      );
    }
    case "apiKey":
      return (
        <>
          <p className="text-xs leading-[18px]">{t(SIGNED_OUT[id])}</p>
          <ApiKeyForm primary={primary} online={online} onSubmit={(key) => acts.loginWithKey(id, key)} onCancel={() => acts.ui(id, { open: "none" })} />
          {errorLine}
        </>
      );
    case "ready": {
      const reading = quotaOf(quota, id);
      const { fiveHour, week } = readyWindows(reading.windows);
      const viaAccount = tool.login.state !== "signed-in" && tool.droppenAccounts.total > 0;
      if (viaAccount) {
        return (
          <p data-testid="via-account" className="text-xs leading-[18px]">
            {t("Ready through the Droppen account “{{label}}”", { label: tool.droppenAccounts.defaultLabel ?? t("Droppen account") })}
          </p>
        );
      }
      return (
        <>
          <div className="grid grid-cols-[minmax(0,1fr)_auto] items-start gap-x-6 gap-y-2 max-[960px]:grid-cols-1">
            <div className="min-w-0">
              <div className="text-[13px] leading-[18px] font-semibold">{tool.login.plan ?? reading.plan ?? t("Signed in")}</div>
              {tool.login.email && <div className="truncate font-mono text-2xs leading-4 text-faint">{tool.login.email}</div>}
            </div>
            {fiveHour || week ? (
              <div data-testid="ready-quota" className="grid grid-cols-[repeat(2,minmax(120px,auto))] gap-x-6">
                {fiveHour && <QuotaBar label="5 hours" window={fiveHour} />}
                {week && <QuotaBar label="this week" window={week} />}
              </div>
            ) : (
              <p data-testid="quota-later" className="font-mono text-2xs leading-4 text-faint">
                {t("Quota shows up after the first session.")}
              </p>
            )}
          </div>
          {can && (
            <Actions className="mt-2">
              <LinkButton testId="switch-account" disabled={!online || busy} onClick={() => acts.login(id, loginMode)}>
                {t("Use a different account")}
              </LinkButton>
            </Actions>
          )}
          {errorLine}
          {pathHint}
        </>
      );
    }
    case "tooOld": {
      const update = offeredMethods(commands, tool, "update")[0] ?? null;
      return (
        <>
          <p className="text-xs leading-[18px]">{t(TOO_OLD[id], { v: tool.version ?? "?", min: tool.minVersion })}</p>
          <Actions>
            <MainButton primary={primary} testId="update" onClick={() => acts.ui(id, { open: "confirm", kind: "update", method: null })}>
              {t("Update")}
            </MainButton>
            {update && <CopyCommandButton command={update.command} />}
          </Actions>
          {shellKey}
        </>
      );
    }
    case "notFound":
      return (
        <>
          <p className="text-xs leading-[18px]">
            {t("Droppen couldn't read your Terminal's settings in time (over 5 s), so a copy installed with nvm, volta or similar may be hiding. Look again, or show Droppen where it is.")}
          </p>
          <Actions>
            <MainButton primary={primary} testId="look-again" disabled={status.rescanning} onClick={() => acts.rescan(true)}>
              {status.rescanning ? t("Looking… (up to 20 s)") : t("Look again (wait longer)")}
            </MainButton>
            <Button data-testid="choose-path-open" disabled={status.rescanning} onClick={() => acts.ui(id, { open: "choosePath" })}>
              {t("Show where it is…")}
            </Button>
            <Button variant="ghost" onClick={() => acts.ui(id, { open: "confirm", kind: "install", method: null })}>
              {t("Install a fresh copy")}
            </Button>
          </Actions>
          {errorLine}
        </>
      );
    case "choosePath":
      return (
        <>
          <ChoosePathForm id={id} primary={primary} busy={busy} onUse={(path) => acts.use(id, path)} onCancel={() => acts.ui(id, { open: "none" })} />
          {errorLine}
        </>
      );
    case "fallback":
      return (
        <>
          <p className="text-xs leading-[18px]">{t("Found {{name}} at {{path}}, outside the places Droppen's sessions look. Use this copy?", { name: TOOL_NAME[id], path: tool.found?.path ?? "" })}</p>
          <Actions>
            <MainButton primary={primary} testId="use-found" disabled={busy} onClick={() => tool.found && acts.use(id, tool.found.path)}>
              {t("Found it, use it")}
            </MainButton>
            <Button variant="ghost" onClick={() => acts.ui(id, { open: "choosePath" })}>
              {t("Show where it is…")}
            </Button>
          </Actions>
          {errorLine}
        </>
      );
    case "plan":
      return (
        <>
          <p data-signal="failed" className="text-xs leading-[18px] text-danger">
            {tool.login.email
              ? t("Signed in as {{email}}, but the free Claude plan doesn't include Claude Code. Upgrade at claude.ai, or sign in with a Console account.", { email: tool.login.email })
              : t("The free Claude plan doesn't include Claude Code. Upgrade at claude.ai, or sign in with a Console account.")}
          </p>
          <Actions>
            <DocsLink href={CLAUDE_PLANS_URL} label={t("Plans at claude.ai")} primary={primary} />
            {can && (
              <Button disabled={!online || busy} onClick={() => acts.login(id, "console")}>
                {t("Use an Anthropic Console account instead")}
              </Button>
            )}
          </Actions>
          {errorLine}
          {shellKey}
        </>
      );
  }
}

/** Claude's "More": the enterprise single sign-on. */
function MoreMenu({ onSso, disabled }: { onSso: () => void; disabled: boolean }) {
  const { t } = useTranslation();
  return (
    <Menu.Root>
      <Menu.Trigger disabled={disabled} aria-label={t("More")} title={t("More")} className="flex h-7 items-center gap-1 px-1.5 text-xs text-muted hover:bg-hover hover:text-fg disabled:opacity-40">
        <Ellipsis className="size-3.5" />
        {t("More")}
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Positioner side="bottom" align="start" sideOffset={4} className="z-50 outline-none">
          <Menu.Popup className="min-w-44 border border-border-strong bg-bg py-1 outline-none">
            <Menu.Item onClick={onSso} className="flex w-full cursor-pointer items-center px-3 py-1 text-left text-xs text-fg outline-none data-[highlighted]:bg-hover">
              {t("Sign in with single sign-on (SSO)")}
            </Menu.Item>
          </Menu.Popup>
        </Menu.Positioner>
      </Menu.Portal>
    </Menu.Root>
  );
}

/** One tool's card: its header (name · maker · version · path · status chip), then its state. */
export function ToolCard(props: Parameters<typeof CardBody>[0] & { homeDir: string }) {
  const { t } = useTranslation();
  const { id, tool, state, ui, homeDir } = props;
  const chip = chipOf(state, tool, ui);
  const installed = tool?.found && !["notInstalled", "notFound"].includes(state);
  const meta = installed ? [tool.version, shortenHome(tool.found!.path, homeDir)].filter(Boolean).join(" · ") : "";
  return (
    <section data-tool={id} data-state={state} aria-labelledby={`setup-tool-${id}`} className="mt-4 border border-border first:mt-0">
      <header className="flex min-h-10 items-center gap-2.5 border-b border-border px-3.5 py-2">
        <span id={`setup-tool-${id}`} className="text-[13px] leading-[18px] font-semibold">
          {TOOL_NAME[id]}
        </span>
        <span className="font-mono text-2xs whitespace-nowrap text-faint max-[720px]:hidden">{id === "claude" ? t("by Anthropic") : t("by OpenAI")}</span>
        <span className="ml-auto flex min-w-0 items-center gap-2.5">
          {meta && (
            <span className="truncate font-mono text-2xs text-faint tabular-nums" title={tool?.found?.path}>
              {meta}
            </span>
          )}
          <StatusChip tone={chip.tone}>{t(chip.label)}</StatusChip>
        </span>
      </header>
      <div className="px-3.5 pt-3 pb-3.5">
        <CardBody {...props} />
      </div>
    </section>
  );
}
