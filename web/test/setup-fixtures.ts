// What the first-run guide's tests feed it: tools in each state of step 01,
// a report around them, the server's whitelisted commands and a quota
// reading. Shapes as `features/setup/types.ts` has them.
import type { Quota } from "../src/features/accounts/use-accounts.js";
import type { SetupCommand, SetupTask, SetupTool, ToolId, ToolsReport } from "../src/features/setup/types.js";

export const HOME = "/Users/lin";
export const STARTED = "2026-10-19T09:00:00Z";

const SIGNED_OUT: SetupTool["login"] = { state: "signed-out", method: null, plan: null, email: null, usable: null };

/** The official installers as the server's whitelist has them: downloaded, then run. */
const INSTALLER = {
  claude: `/bin/sh -c 't="$(mktemp)" && curl --proto =https --tlsv1.2 -fsSLo "$t" https://claude.ai/install.sh && bash "$t"; s=$?; rm -f "$t"; exit $s'`,
  codex: `/bin/sh -c 't="$(mktemp)" && curl --proto =https --tlsv1.2 -fsSLo "$t" https://chatgpt.com/codex/install.sh && sh "$t"; s=$?; rm -f "$t"; exit $s'`,
} as const;

/** A task; one that has finished is `failed` as the server would say for its exit code, unless `over` says otherwise. */
export function task(tool: ToolId, kind: SetupTask["kind"], over: Partial<SetupTask> = {}): SetupTask {
  const finished = over.running === false;
  const exitCode = over.exitCode ?? null;
  return {
    tool,
    kind,
    method: kind === "login" ? (tool === "claude" ? "claudeai" : "chatgpt") : "native",
    command: kind === "login" ? `${tool === "claude" ? "claude auth" : "codex"} login` : INSTALLER[tool],
    terminalId: `t-${tool}-${kind}`,
    startedAt: STARTED,
    running: true,
    exitCode: null,
    failed: finished && exitCode !== null && exitCode !== 0,
    failure: null,
    tail: [],
    ...over,
  };
}

/** One tool in a named state. */
export function tool(id: ToolId, state: "missing" | "signedOut" | "ready" | "tooOld" | "plan" | "fallback" | "viaAccount", over: Partial<SetupTool> = {}): SetupTool {
  const installed = {
    found: { path: `${HOME}/.local/bin/${id}`, via: "path" as const, method: "native" as const },
    version: id === "claude" ? "2.1.292" : "0.163.0",
    supported: true,
  };
  const base: SetupTool = {
    id,
    found: null,
    version: null,
    minVersion: id === "claude" ? "2.0.0" : "0.100.0",
    supported: false,
    login: { state: "unknown", method: null, plan: null, email: null, usable: null },
    shellKey: false,
    droppenAccounts: { total: 0, defaultLabel: null },
    ready: false,
    task: null,
  };
  const signedIn: SetupTool["login"] = { state: "signed-in", method: id === "claude" ? "claude.ai" : "chatgpt", plan: id === "claude" ? "Max 20x" : "ChatGPT Plus", email: "lin@example.com", usable: true };
  const states: Record<typeof state, Partial<SetupTool>> = {
    missing: {},
    signedOut: { ...installed, login: SIGNED_OUT },
    ready: { ...installed, login: signedIn, ready: true },
    tooOld: { ...installed, found: { path: `${HOME}/.npm-global/bin/${id}`, via: "path", method: "npm" }, version: "1.0.88", supported: false, login: signedIn },
    plan: { ...installed, login: { ...signedIn, plan: "Free", usable: false } },
    fallback: { ...installed, found: { path: `${HOME}/.nvm/versions/node/v22.12.0/bin/${id}`, via: "fallback", method: "npm" }, login: signedIn, ready: true },
    viaAccount: { ...installed, login: SIGNED_OUT, droppenAccounts: { total: 1, defaultLabel: "Company gateway" }, ready: true },
  };
  return { ...base, ...states[state], ...over };
}

export function report(claude: SetupTool, codex: SetupTool, over: Partial<ToolsReport> = {}): ToolsReport {
  return {
    pathRead: "ok",
    os: { platform: "darwin", macos: "15.3", arch: "arm64" },
    helpers: { brew: true, node: "v22.12.0", git: "ready" },
    canInstall: true,
    tools: [claude, codex],
    ...over,
  };
}

function command(tool: SetupCommand["tool"], kind: SetupCommand["kind"], method: string, text: string, over: Partial<SetupCommand> = {}): SetupCommand {
  return {
    tool,
    kind,
    method,
    command: text,
    available: true,
    reason: null,
    recommended: method === "native" || method === "claudeai" || method === "chatgpt",
    docs: tool === "claude" ? "https://code.claude.com/docs/en/setup" : "https://github.com/openai/codex",
    ...over,
  };
}

/** The whitelist; `brew: false` is a Mac without Homebrew. */
export function commands({ brew = true, oldMac = false }: { brew?: boolean; oldMac?: boolean } = {}): SetupCommand[] {
  const noBrew = brew ? {} : { available: false, reason: "no_brew" };
  return [
    command("claude", "install", "native", INSTALLER.claude, oldMac ? { available: false, reason: "macos_too_old" } : {}),
    command("claude", "install", "brew", "brew install --cask claude-code", noBrew),
    command("claude", "install", "npm", "npm install -g @anthropic-ai/claude-code"),
    command("claude", "update", "native", "claude update"),
    command("claude", "update", "brew", "brew upgrade claude-code", noBrew),
    command("claude", "update", "npm", "npm install -g @anthropic-ai/claude-code@latest"),
    command("codex", "install", "native", INSTALLER.codex),
    command("codex", "install", "brew", "brew install --cask codex", noBrew),
    command("codex", "install", "npm", "npm install -g @openai/codex"),
    command("codex", "update", "native", INSTALLER.codex),
    command("codex", "update", "brew", "brew upgrade --cask codex", noBrew),
    command("codex", "update", "npm", "npm install -g @openai/codex@latest"),
    command("git", "install", "apple", "xcode-select --install"),
    command("claude", "login", "claudeai", "claude auth login"),
    command("claude", "login", "console", "claude auth login --console"),
    command("claude", "login", "sso", "claude auth login --sso"),
    command("codex", "login", "chatgpt", "codex login"),
    command("codex", "login", "device", "codex login --device-auth"),
    command("codex", "login", "apikey", "codex login --with-api-key"),
  ];
}

/** A quota reading with both windows for both tools. */
export function quota(): Quota {
  const window = (label: string, minutes: number, used: number) => ({
    label,
    windowMinutes: minutes,
    usedPercent: used,
    remainingPercent: 100 - used,
    resetsAt: "2026-10-19T16:40:00Z",
  });
  return {
    accounts: [],
    claude: { plan: "Max 20x", tier: null, updatedAt: STARTED, source: "session", windows: [window("5h", 300, 12), window("weekly", 10_080, 30)] },
    codex: { plan: "plus", blocked: false, source: "app_server", updatedAt: STARTED, windows: [window("5h", 300, 8), window("weekly", 10_080, 21)] },
  } as Quota;
}
