import { useParams } from "@tanstack/react-router";
import { Bot, ChevronLeft } from "lucide-react";
import { useCallback, useEffect, useMemo } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "@/api/schema";
import { feedRow, useGlobalFeedSelector, type GlobalFeedState } from "@/app/global-feed";
import { MachineLink } from "@/app/machine-route";
import { SourceIcon } from "@/components/source-icon";
import { Spinner } from "@/components/ui/spinner";
import { AgentTranscriptBody, useAgentTranscript, type SubagentCtx } from "@/features/sessions/session-detail";
import { SessionHeader } from "@/features/sessions/session-header";
import { ChildStatusMark } from "@/features/sessions/subagent-marks";
import { useMachine } from "@/lib/machines/context";
import { browserPresence } from "@/lib/presence";
import { apiFor } from "@/lib/machines/registry";

type Source = components["schemas"]["Source"];
type SessionChild = components["schemas"]["SessionChild"];

/** Where an agent sits in its session's tree: the agents above it, outermost first, then itself. */
export function agentPath(children: readonly SessionChild[], agentId: string): SessionChild[] | null {
  for (const child of children) {
    if (child.id === agentId) return [child];
    const below = agentPath(child.children, agentId);
    if (below) return [child, ...below];
  }
  return null;
}

/**
 * Mark the agent read while its page is being looked at: on arrival, when it
 * writes again, and when the window comes back. Gated on somebody being at
 * the page, as the session's own report is (`use-seen.ts`).
 */
function useReportAgentSeen(machine: string, source: Source, id: string, agentId: string, updatedAt: string | null) {
  const presence = browserPresence();
  useEffect(() => {
    const post = () => {
      if (!presence.present()) return;
      void apiFor(machine)
        .POST("/api/sessions/{source}/{id}/agents/{agentId}/seen", { params: { path: { source, id, agentId } } })
        .catch(() => undefined);
    };
    post();
    window.addEventListener("focus", post);
    return () => window.removeEventListener("focus", post);
  }, [machine, source, id, agentId, updatedAt, presence]);
}

/**
 * One Claude subagent's conversation, on a page of its own (`/sessions/$source/$id/agents/$agentId`).
 *
 * Read-only: an agent takes no messages, its session does. The header leads
 * back to whatever delegated to it — its session, or the agent above it.
 */
export function SubagentDetail() {
  const { t } = useTranslation();
  // Only Claude and Codex sessions have subagents; an ACP agent's have none.
  const { source, id, agentId } = useParams({ strict: false }) as { source: SubagentCtx["source"]; id: string; agentId: string };
  const machine = useMachine();
  // Two slices that each keep their identity until the row is replaced.
  const selectTitle = useCallback((state: GlobalFeedState) => feedRow(state, { machine, source, id })?.title, [machine, source, id]);
  const selectTree = useCallback((state: GlobalFeedState) => feedRow(state, { machine, source, id })?.children, [machine, source, id]);
  const sessionTitle = useGlobalFeedSelector(selectTitle);
  const tree = useGlobalFeedSelector(selectTree);
  const path = useMemo(() => (tree ? agentPath(tree, agentId) : null), [tree, agentId]);
  const self = path?.at(-1) ?? null;
  const parent = path && path.length > 1 ? path.at(-2)! : null;
  const query = useAgentTranscript({ source, id }, agentId, true);
  useReportAgentSeen(machine, source, id, agentId, self?.updatedAt ?? null);

  const name = self?.name ?? query.data?.description ?? agentId;
  const role = self?.role ?? query.data?.agentType ?? null;
  const back: { to: string; params: Record<string, string> } = parent
    ? { to: "/sessions/$source/$id/agents/$agentId", params: { source, id, agentId: parent.id } }
    : { to: "/sessions/$source/$id", params: { source, id } };
  const backName = parent?.name ?? sessionTitle ?? id.slice(0, 8);

  return (
    <div data-subagent-page="" className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SessionHeader
        titleRow={<>
          <MachineLink
            {...back}
            title={t("back to {{name}}", { name: backName })}
            aria-label={t("back to {{name}}", { name: backName })}
            className="flex size-5 shrink-0 items-center justify-center text-muted hover:bg-hover hover:text-fg max-md:size-9"
          >
            <ChevronLeft className="size-3.5 max-md:size-4" />
          </MachineLink>
          {self && <ChildStatusMark status={self.status} />}
          <span className="min-w-0 flex-1 truncate text-[13px] font-semibold max-md:text-[15px]" title={name}>{name}</span>
        </>}
        metadata={<>
          <SourceIcon source={source} className="text-muted" />
          <span className="flex min-w-0 items-center gap-1">
            <Bot aria-hidden="true" className="size-3 shrink-0" />
            <span className="shrink-0">{t("subagent of")}</span>
            <MachineLink {...back} className="min-w-0 truncate text-fg hover:underline" title={backName}>
              {backName}
            </MachineLink>
          </span>
          {role && <span className="shrink-0 truncate">{role}</span>}
          {query.data && <span className="ml-auto shrink-0 tabular-nums text-faint">{t("{{n}} items", { n: query.data.totalItems })}</span>}
        </>}
      />
      <div className="min-h-0 flex-1 overflow-y-auto">
        {query.data || query.error ? (
          <AgentTranscriptBody query={query} session={{ source, id }} flush />
        ) : (
          <Spinner className="py-4" />
        )}
      </div>
    </div>
  );
}
