/*
 * Settings › Agents & accounts: what a new session can run on and under
 * which login, one tab per provider — Claude Code, Codex, then the ACP
 * agents. (The section's id stays `accounts`, so old links still land here.)
 *
 * - The tab is the address's (`?section=accounts&tab=claude|codex|acp`), else
 *   the one this browser used last, else Claude Code; `&agent=<id>` opens an
 *   ACP agent's details. Switching tabs rewrites the address in place.
 * - Each label says what its tab holds that needs the person: a 7px square and
 *   a count, amber for "needs you", red for "cannot run" (`agents-tabs.ts`).
 * - The tabs are words over a rule, not a segmented control: on a phone the
 *   settings categories above are already one.
 */
import { useSearch } from "@tanstack/react-router";
import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { useMachineNavigate } from "@/app/machine-route";
import { GlyphIcon } from "@/components/glyph";
import { Sheet } from "@/components/sheet";
import { SourceIcon } from "@/components/source-icon";
import { Tabs, TextTab, TextTabsList } from "@/components/ui/tabs";
import { useAccounts, useQuota } from "@/features/accounts/use-accounts";
import { useAcpAgents } from "@/features/sessions/acp-agent";
import { ACP_GLYPH } from "@/features/sessions/acp-agent-icons";
import { AccountsSection } from "@/features/settings/accounts-section";
import { cn } from "@/lib/cn";
import { AcpTab } from "./acp-tab";
import {
  accountsSignal,
  acpSignal,
  agentsSearch,
  AGENTS_TABS,
  LAST_TAB_KEY,
  openingTab,
  TAB_NAME,
  type AgentsTab,
  type TabSignal,
} from "./agents-tabs";

function storedTab(): string | null {
  try {
    return localStorage.getItem(LAST_TAB_KEY);
  } catch {
    return null;
  }
}

/** A tab label's square and count; nothing when the tab holds nothing that needs the person. */
export function SignalCount({ signal }: { signal: TabSignal }) {
  const { t } = useTranslation();
  const tone = signal.danger > 0 ? "danger" : signal.warn > 0 ? "warn" : null;
  if (!tone) return null;
  const parts = [
    signal.danger > 0 ? t("{{n}} cannot run", { n: signal.danger }) : null,
    signal.warn > 0 ? t("{{n}} need you", { n: signal.warn }) : null,
  ].filter(Boolean);
  return (
    <span
      data-signal={tone === "danger" ? "failed" : "warn"}
      title={parts.join(" · ")}
      aria-label={parts.join(" · ")}
      className="flex items-center gap-1 font-mono text-2xs text-muted tabular-nums"
    >
      <span aria-hidden="true" className={cn("size-[7px] shrink-0", tone === "danger" ? "bg-danger" : "bg-warn")} />
      {signal.danger + signal.warn}
    </span>
  );
}

/** The provider tabs, each with its mark and its signal. */
export function AgentsTabStrip({
  value,
  onChange,
  signals,
}: {
  value: AgentsTab;
  onChange: (tab: AgentsTab) => void;
  signals: Record<AgentsTab, TabSignal>;
}) {
  const { t } = useTranslation();
  return (
    <Tabs value={value} onValueChange={(next) => onChange(next as AgentsTab)}>
      <TextTabsList aria-label={t("agents & accounts")} className="px-3">
        {AGENTS_TABS.map((tab) => (
          <TextTab key={tab} value={tab} data-tab={tab}>
            {tab === "acp" ? <GlyphIcon glyph={ACP_GLYPH} className="size-3.5" /> : <SourceIcon source={tab} className="size-3.5" />}
            {TAB_NAME[tab]}
            <SignalCount signal={signals[tab]} />
          </TextTab>
        ))}
      </TextTabsList>
    </Tabs>
  );
}

export function AgentsSection() {
  const search = useSearch({ strict: false }) as { tab?: string; agent?: string };
  const navigate = useMachineNavigate();
  const tab = openingTab(search.tab, storedTab());
  const { data: accounts } = useAccounts();
  const { data: quota } = useQuota();
  const { data: acp } = useAcpAgents();
  const signals: Record<AgentsTab, TabSignal> = {
    claude: accountsSignal(accounts, quota, "claude"),
    codex: accountsSignal(accounts, quota, "codex"),
    acp: acpSignal(acp?.agents),
  };

  // The tab shown is the one this browser used last, deep link or not.
  useEffect(() => {
    try {
      localStorage.setItem(LAST_TAB_KEY, tab);
    } catch {
      // storage unavailable: the page still opens on the address's tab
    }
  }, [tab]);

  const go = (next: AgentsTab, agent?: string | null) =>
    void navigate({ to: "/settings", search: agentsSearch(next, agent), replace: true });

  return (
    <Sheet>
      <AgentsTabStrip value={tab} onChange={(next) => go(next)} signals={signals} />
      {tab === "acp" ? (
        <AcpTab openAgent={search.agent ?? null} onOpenAgent={(agent) => go("acp", agent)} />
      ) : (
        <AccountsSection key={tab} provider={tab} />
      )}
    </Sheet>
  );
}
