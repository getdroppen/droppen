/*
 * The Droppen app's first-run guide (`features/setup/`) and the setup page's
 * notice for the app's own server (`features/auth/setup-view.tsx`), as the app
 * on each system shows them, from fixed data and no server:
 * `?fixture=welcome&screen=welcome|tools|where|first|wrap|setup`, with
 * `&platform=darwin|linux` (what `GET /api/auth/status` says the server runs
 * on), `&bridge=1|0` (`startAtLogin` in the app window's bridge, `init.js`;
 * left out, a browser on the device, with no bridge), `&tray=1|0` (`trayHost`
 * on the same bridge; left out, yes, as a browser on the device takes it) and
 * `&lang=`.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import i18next from "i18next";
import { createRoot } from "react-dom/client";
import { useTranslation } from "react-i18next";
import { SetupPage } from "@/features/auth/setup-view";
import { FirstStepView } from "@/features/setup/first-step";
import { EMPTY_FIRST_UI } from "@/features/setup/first-model";
import { ActionBar, GuideSheet, GuideTopBar, StepHeader, StepPage, StepRail } from "@/features/setup/frame";
import { emptyProgress } from "@/features/setup/model";
import { IDLE_STATUS, type ToolActs } from "@/features/setup/tool-card";
import { IDLE_CARD, IDLE_TOOLS_UI } from "@/features/setup/tools-model";
import { ToolsStepView } from "@/features/setup/tools-step";
import type { RecentFolder, SetupCommand, SetupProgress, SetupTool, StepId, ToolsReport } from "@/features/setup/types";
import { GuideScreen, STEP_HEADERS, stepAction } from "@/features/setup/welcome-view";
import { applyLanguage, type LanguagePref } from "@/i18n";
import { authStatus } from "@/lib/auth";
import { MachineProvider } from "@/lib/machines/context";
import { LOCAL } from "@/lib/machines/key";
import { mq } from "@/lib/machines/query";

const noop = () => undefined;

/** One system's facts, as its server and its detection say them. */
function system(platform: string) {
  const mac = platform !== "linux";
  const home = mac ? "/Users/lin" : "/home/lin";
  return {
    home,
    os: { platform, macos: mac ? "15.3" : null, arch: mac ? "arm64" : "x86_64" },
    droppen: mac ? "/Applications/Droppen.app/Contents/MacOS/droppen" : "/opt/droppen/droppen",
  };
}

function tool(id: SetupTool["id"], ready: boolean, home: string): SetupTool {
  return {
    id,
    found: ready ? { path: `${home}/.local/bin/${id}`, via: "path", method: "native" } : null,
    version: ready ? "2.1.292" : null,
    minVersion: "2.0.0",
    supported: ready,
    login: ready
      ? { state: "signed-in", method: "claude.ai", plan: "Max 20x", email: "lin@example.com", usable: true }
      : { state: "unknown", method: null, plan: null, email: null, usable: null },
    shellKey: false,
    droppenAccounts: { total: 0, defaultLabel: null },
    ready,
    task: null,
  };
}

/** The whitelist as the server lists it on each system: no Homebrew either way; Apple's installer only on a Mac. */
function commands(mac: boolean): SetupCommand[] {
  const row = (tool: SetupCommand["tool"], kind: SetupCommand["kind"], method: string, command: string, over: Partial<SetupCommand> = {}): SetupCommand => ({
    tool,
    kind,
    method,
    command,
    available: true,
    reason: null,
    recommended: method === "native",
    docs: tool === "codex" ? "https://github.com/openai/codex" : "https://code.claude.com/docs/en/setup",
    ...over,
  });
  return [
    row("codex", "install", "native", "/bin/sh -c 'curl -fsSL https://chatgpt.com/codex/install.sh | sh'"),
    row("codex", "install", "brew", "brew install --cask codex", { available: false, reason: "no_brew" }),
    row("codex", "install", "npm", "npm install -g @openai/codex", { available: false, reason: "no_node" }),
    row("git", "install", "apple", "xcode-select --install", mac ? {} : { available: false, reason: "not_macos" }),
  ];
}

const acts: ToolActs = { ui: noop, run: noop, login: noop, loginWithKey: noop, stop: noop, dismiss: noop, rescan: noop, use: noop, installGit: noop, openAdvanced: noop };

/** Steps 01 and 03 drawn in the guide's frame from their views, which take their data as props. */
function StepFrame({ step, progress, report, children }: { step: StepId; progress: SetupProgress; report: ToolsReport; children: React.ReactNode }) {
  const { t } = useTranslation();
  const header = STEP_HEADERS[step];
  const action = stepAction(step, progress, report, t, { next: noop, finish: noop });
  return (
    <div data-testid="guide" data-screen={step} className="flex h-full flex-col bg-bg text-fg">
      <GuideTopBar onLater={noop} online />
      <GuideSheet rail={<StepRail current={step} progress={progress} onGoto={noop} />}>
        <StepPage bar={<ActionBar step={step} onSkip={noop} onBack={noop} action={action} />}>
          <StepHeader kicker={t(header.kicker)} title={t(header.title)} body={header.body ? t(header.body) : undefined} />
          {children}
        </StepPage>
      </GuideSheet>
    </div>
  );
}

export async function mountWelcome(query: URLSearchParams): Promise<void> {
  const platform = query.get("platform") ?? "darwin";
  const { home, os, droppen } = system(platform);
  const mac = os.macos !== null;
  const screen = query.get("screen") ?? "welcome";
  const lang = (query.get("lang") as LanguagePref | null) ?? "en";
  const bridge = query.get("bridge");
  const tray = query.get("tray");
  if (bridge !== null) Object.defineProperty(window, "__droppenDesktop", { value: Object.freeze({ startAtLogin: bridge === "1", trayHost: tray !== "0" }) });

  // The app's own server: the status the page asks before anything is drawn.
  const status = {
    authRequired: true,
    setupRequired: true,
    authOff: false,
    user: screen === "setup" ? null : "droppen-app",
    desktopApp: true,
    platform,
    droppenPath: droppen,
    droppenEnv: [],
  };
  const realFetch = globalThis.fetch.bind(globalThis);
  globalThis.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(input instanceof Request ? input.url : String(input), location.href);
    if (url.pathname === "/api/auth/status") return new Response(JSON.stringify(status), { headers: { "content-type": "application/json" } });
    if (url.pathname.startsWith("/api/")) return new Response(JSON.stringify({ error: "fixture" }), { status: 404 });
    return realFetch(input, init);
  };
  await authStatus();
  await applyLanguage(lang);

  const report: ToolsReport = {
    pathRead: "ok",
    os,
    helpers: { brew: false, node: null, git: "missing" },
    canInstall: true,
    tools: [tool("claude", true, home), tool("codex", false, home)],
  };
  const started = "2026-11-09T09:00:00Z";
  const base = { ...emptyProgress(), startedAt: started };
  const project = `${home}/Documents/acme-api`;
  const recent: RecentFolder[] = [
    { path: project, name: "acme-api", tools: ["claude"], lastUsed: new Date(Date.now() - 2 * 86_400_000).toISOString(), sessions: 3 },
    { path: `${home}/Projects/droppen-site`, name: "droppen-site", tools: ["claude"], lastUsed: new Date(Date.now() - 5 * 86_400_000).toISOString(), sessions: 1 },
  ];
  const t = i18next.t.bind(i18next);

  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  client.setQueryData(mq(LOCAL, "prefs"), { sessionGrouping: "flat", transcriptStyle: "flat", fontSize: "default", language: lang, composerSend: "enter" });

  let page: React.ReactNode;
  switch (screen) {
    case "setup":
      page = <SetupPage owner={false} />;
      break;
    case "tools":
      page = (
        <StepFrame step="tools" progress={base} report={report}>
          <ToolsStepView
            report={report}
            commands={commands(mac)}
            quota={null}
            homeDir={home}
            online
            ui={{ ...IDLE_TOOLS_UI, codex: { ...IDLE_CARD, open: "confirm", kind: "install" } }}
            status={IDLE_STATUS}
            acts={acts}
            advanced={{ open: false, kind: "token", onOpen: noop, onKind: noop }}
          />
        </StepFrame>
      );
      break;
    case "first": {
      const progress: SetupProgress = { ...base, steps: { ...base.steps, tools: "done", where: "done" }, where: { ...base.where, choice: "local" } };
      page = (
        <StepFrame step="first" progress={progress} report={report}>
          <FirstStepView
            report={report}
            ui={{ ...EMPTY_FIRST_UI, cwd: project, agent: "claude", prompt: "repo1" }}
            onUi={noop}
            recent={recent}
            onPractice={noop}
            practice={{ state: "idle", path: null }}
            homeDir={home}
            onGoto={noop}
          />
        </StepFrame>
      );
      break;
    }
    case "where":
      page = <GuideScreen screen="where" progress={{ ...base, steps: { ...base.steps, tools: "done" } }} tools={report} online onLater={noop} action={stepAction("where", base, report, t, { next: noop, finish: noop })} />;
      break;
    case "wrap": {
      const progress: SetupProgress = {
        ...base,
        steps: { tools: "done", where: "done", first: "done", wrap: "pending" },
        where: { ...base.where, choice: "local" },
        first: { cwd: project, agent: "claude", prompt: "Explain this project." },
      };
      page = <GuideScreen screen="wrap" progress={progress} tools={report} online onLater={noop} action={stepAction("wrap", progress, report, t, { next: noop, finish: noop })} />;
      break;
    }
    default:
      page = <GuideScreen screen="welcome" progress={emptyProgress()} tools={report} online onLater={noop} />;
  }

  createRoot(document.getElementById("root")!).render(
    <QueryClientProvider client={client}>
      <MachineProvider machine={LOCAL}>
        <div data-fixture-root className="h-full">
          {page}
        </div>
      </MachineProvider>
    </QueryClientProvider>,
  );
}
