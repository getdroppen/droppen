/*
 * Settings › Agents & accounts: which tab opens, and what each tab's label
 * says about what it holds. Plain functions, so the rules are tested without
 * a page.
 */
import type { components } from "@/api/schema";
import { activeRests } from "@/features/accounts/account-rest";
import {
  accountsForProvider,
  accountUsageState,
  localLoginOf,
  localLoginSignedOut,
  usageNotice,
  type AccountList,
  type AccountProvider,
  type Quota,
} from "@/features/accounts/use-accounts";
import type { AcpAgentInfo } from "@/features/sessions/acp-agent";

export type AcpCheck = components["schemas"]["AcpCheck"];

/** The page's tabs, in order: the two CLIs, then every ACP agent. */
export const AGENTS_TABS = ["claude", "codex", "acp"] as const;
export type AgentsTab = (typeof AGENTS_TABS)[number];

export function isAgentsTab(value: unknown): value is AgentsTab {
  return AGENTS_TABS.includes(value as AgentsTab);
}

/** What each tab is called: the provider's own name, never translated. */
export const TAB_NAME: Record<AgentsTab, string> = { claude: "Claude Code", codex: "Codex", acp: "ACP" };

/** Where this browser keeps the tab it used last (a browser's own key, not a device's). */
export const LAST_TAB_KEY = "rh.settings.agents-tab";

/**
 * The tab the page opens on: the one the address names, else the one this
 * browser used last, else Claude Code. Never "the first with a problem": an
 * amber state can last for weeks, and every tab's label already says it.
 */
export function openingTab(asked: unknown, stored: unknown): AgentsTab {
  if (isAgentsTab(asked)) return asked;
  if (isAgentsTab(stored)) return stored;
  return "claude";
}

/** The page's link to a tab, and to one ACP agent's details. */
export function agentsSearch(tab: AgentsTab, agent?: string | null): { section: "accounts"; tab: AgentsTab; agent?: string } {
  return { section: "accounts", tab, ...(agent ? { agent } : {}) };
}

/** What a tab's label says: how many things need the person, and how many cannot run. */
export interface TabSignal {
  /** Amber: a signed-out local login, a usage login that expired behind old numbers. */
  warn: number;
  /** Red: a credential the provider refused, an ACP agent whose last check failed. */
  danger: number;
}

export const NO_SIGNAL: TabSignal = { warn: 0, danger: 0 };

/**
 * A CLI tab's signal. Not counted: a limit rest (it ends by itself), usage
 * that was never enabled, an expiry that only affects the idle-time fallback,
 * and numbers that are only old.
 */
export function accountsSignal(list: AccountList | undefined, quota: Quota | undefined, provider: AccountProvider, now = Date.now()): TabSignal {
  if (!list) return NO_SIGNAL;
  let warn = localLoginSignedOut(localLoginOf(list, provider)) ? 1 : 0;
  let danger = 0;
  for (const account of accountsForProvider(list.accounts, provider)) {
    if (activeRests(account, now).some((rest) => rest.class === "auth")) {
      danger++;
      continue;
    }
    const q = quota?.accounts.find((a) => a.id === account.id);
    const usage = accountUsageState(account.kind, account.usageEnabled, q?.error);
    if (usageNotice(usage, q?.updatedAt, (q?.windows.length ?? 0) > 0, now) === "login_expired") warn++;
  }
  return { warn, danger };
}

/** The ACP tab's signal: every agent whose last check found it cannot start. */
export function acpSignal(agents: readonly AcpAgentInfo[] | undefined): TabSignal {
  return { warn: 0, danger: (agents ?? []).filter((agent) => checkFailed(agent.lastCheck)).length };
}

/** Whether a check found the agent cannot start. */
export function checkFailed(check: AcpCheck | null | undefined): boolean {
  return !!check && check.state !== "ready";
}

/** What an agent's card and the picker say about it: its last check, or one under way. */
export type AcpStatus = { kind: "unchecked" } | { kind: "checking" } | { kind: "ready" | "not_found" | "failed"; check: AcpCheck };

export function acpStatus(check: AcpCheck | null | undefined, checking = false): AcpStatus {
  if (checking) return { kind: "checking" };
  if (!check) return { kind: "unchecked" };
  return { kind: check.state, check };
}

/** How long ago `iso` was, as the words a check's time is said in. */
export function agoKey(iso: string, now = Date.now()): { key: string; n?: number } {
  const minutes = Math.floor((now - new Date(iso).getTime()) / 60_000);
  if (!(minutes >= 1)) return { key: "just now" };
  if (minutes < 60) return { key: "{{n}} min ago", n: minutes };
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return { key: "{{n}} h ago", n: hours };
  return { key: "{{n}} d ago", n: Math.floor(hours / 24) };
}
