import { useQuery } from "@tanstack/react-query";
import { ApiError } from "@/api/errors";
import type { components } from "@/api/schema";
import { useApi, useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";
import type { MachineKey } from "@/lib/machines/key";

export type AcpAgentInfo = components["schemas"]["AcpAgentInfo"];
export type AcpAgentList = components["schemas"]["AcpAgentList"];

/** Where this device's ACP agents are cached. */
export const acpAgentsKey = (machine: MachineKey) => mq(machine, "acp-agents");

/**
 * The ACP agents this device runs sessions on (`agents`, each with the names
 * of its variables), and the built-in descriptions offered when one is added
 * (`presets`). `enabled: false` asks nothing (a row of another device's).
 */
export function useAcpAgents(enabled = true) {
  const api = useApi();
  const machine = useMachine();
  return useQuery({
    queryKey: acpAgentsKey(machine),
    queryFn: async () => (await api.GET("/api/acp/agents")).data,
    staleTime: 60_000,
    enabled,
  });
}

/** Where an agent's last check, in full, is cached: under the list's key, so what refreshes the list refreshes it too. */
export const acpCheckKey = (machine: MachineKey, id: string) => mq(machine, "acp-agents", id, "check");

/**
 * The agent `id`'s last check in full, its error included, which the list
 * leaves out. Only a device that manages the computer is answered; another
 * is refused, 403 `limited_device`.
 */
export function useAcpCheck(id: string, enabled: boolean) {
  const api = useApi();
  const machine = useMachine();
  return useQuery({
    queryKey: acpCheckKey(machine, id),
    queryFn: async () => {
      const res = await api.GET("/api/acp/agents/{id}/check", { params: { path: { id } } });
      if (!res.data) throw new ApiError(res.error);
      return res.data;
    },
    enabled,
    retry: false,
  });
}

/** Arguments as typed into one field: separated by whitespace, no quoting. */
export function splitArgs(text: string): string[] {
  return text.split(/\s+/).filter(Boolean);
}
