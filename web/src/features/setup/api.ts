/*
 * The guide's requests (`types.ts`), on this view's device, cached by it:
 * `GET/PUT /api/setup/progress`, `GET /api/setup/tools` and, for step 01,
 * the whitelisted commands and the install, update, sign-in, rescan and
 * "use this copy" actions — the server runs only its own commands; the page
 * picks one by name.
 */
import { type QueryClient, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useRef } from "react";
import type { ApiClient } from "@/api/client";
import { useApi, useMachine } from "@/lib/machines/context";
import type { MachineKey } from "@/lib/machines/key";
import { mq } from "@/lib/machines/query";
import type { LoginMode, SetupProgress, SetupTask, SetupTool, ToolId, ToolsReport } from "./types";

export const setupProgressKey = (machine: MachineKey) => mq(machine, "setup", "progress");
export const setupToolsKey = (machine: MachineKey) => mq(machine, "setup", "tools");

/** The stored progress; `null` from a server without the route (an older one), which is read as "no guide". */
export async function fetchProgress(api: ApiClient): Promise<SetupProgress | null> {
  return (await api.GET("/api/setup/progress")).data ?? null;
}

export async function fetchTools(api: ApiClient): Promise<ToolsReport | null> {
  return (await api.GET("/api/setup/tools")).data ?? null;
}

export function useSetupProgress(enabled = true) {
  const api = useApi();
  const machine = useMachine();
  return useQuery({ queryKey: setupProgressKey(machine), queryFn: () => fetchProgress(api), enabled, staleTime: 30_000 });
}

/**
 * Which AI tools are ready. Detection asks each CLI for its version and its
 * sign-in, which takes a moment: asked once and kept for a few minutes, not
 * on every focus.
 */
export function useSetupTools(enabled = true, poll: number | false = false) {
  const api = useApi();
  const machine = useMachine();
  return useQuery({
    queryKey: setupToolsKey(machine),
    queryFn: () => fetchTools(api),
    enabled,
    staleTime: 5 * 60_000,
    refetchOnWindowFocus: false,
    // While an install or a sign-in runs (step 01): its end, re-detected. Not
    // while the page is hidden — React Query holds an interval then.
    refetchInterval: poll,
  });
}

/** The whitelisted commands, for step 01's confirm boxes; `null` from a server without them. */
export function useSetupCommands(enabled = true) {
  const api = useApi();
  const machine = useMachine();
  return useQuery({
    queryKey: mq(machine, "setup", "commands"),
    queryFn: async () => (await api.GET("/api/setup/commands")).data ?? null,
    enabled,
    staleTime: 5 * 60_000,
    refetchOnWindowFocus: false,
  });
}

/** Step 03's recent folders: from the device's session lists, folder names only; empty from a server without the route. */
export function useRecentFolders(enabled = true) {
  const api = useApi();
  const machine = useMachine();
  return useQuery({
    queryKey: mq(machine, "setup", "recent"),
    queryFn: async () => (await api.GET("/api/setup/projects/recent")).data?.folders ?? [],
    enabled,
    staleTime: 60_000,
    refetchOnWindowFocus: false,
  });
}

/** Make `~/Droppen/first-session` (or `-2`, …; never one that is there): its path. */
export function usePracticeFolder() {
  const api = useApi();
  return useMutation({ mutationFn: async () => (await answered(await api.POST("/api/setup/practice-folder"))).path });
}

/** A refusal of a step-01 action: the server's `error` code (`busy`, `app_only`, `unavailable`…), and its HTTP status. */
export class SetupActionError extends Error {
  constructor(
    readonly code: string,
    readonly status: number,
  ) {
    super(code);
  }
}

/** The body of a request that worked, else a {@link SetupActionError} with the server's `error` code. */
async function answered<T>(res: { data?: T; error?: unknown; response: Response }): Promise<NonNullable<T>> {
  if (res.data != null && !res.error) return res.data;
  const code = (res.error as { error?: unknown } | undefined)?.error;
  throw new SetupActionError(typeof code === "string" ? code : "failed", res.response?.status ?? 0);
}

/**
 * Step 01's actions, as plain requests (the hook below adds the cache): start
 * an install or update with one whitelisted method, sign in, look again, adopt
 * a copy found elsewhere, ask Apple's installer for git, stop a task's
 * terminal, and the one cheap sign-in check that polling makes.
 */
export function setupActions(api: ApiClient) {
  const at = (tool: ToolId) => ({ params: { path: { tool } } });
  return {
    run: async (tool: ToolId, kind: "install" | "update", method: string): Promise<SetupTask> =>
      kind === "install"
        ? answered(await api.POST("/api/setup/tools/{tool}/install", { ...at(tool), body: { method } }))
        : answered(await api.POST("/api/setup/tools/{tool}/update", { ...at(tool), body: { method } })),
    /** `apiKey` (mode `apikey`) goes in this body and nowhere else. */
    login: async (tool: ToolId, mode: LoginMode, apiKey?: string): Promise<SetupTask> =>
      answered(await api.POST("/api/setup/tools/{tool}/login", { ...at(tool), body: apiKey === undefined ? { mode } : { mode, apiKey } })),
    loginStatus: async (tool: ToolId): Promise<SetupTool["login"]> => answered(await api.GET("/api/setup/tools/{tool}/login", at(tool))),
    rescan: async (patient: boolean) => answered(await api.POST("/api/setup/tools/rescan", { body: { patient } })),
    use: async (tool: ToolId, path: string) => answered(await api.POST("/api/setup/tools/{tool}/use", { ...at(tool), body: { path } })),
    installGit: async () => answered(await api.POST("/api/setup/git/install")),
    stop: async (terminalId: string) => {
      const res = await api.DELETE("/api/terminals/{id}", { params: { path: { id: terminalId } } });
      if (res.error) throw new SetupActionError("failed", res.response.status);
    },
  };
}

export type SetupActions = ReturnType<typeof setupActions>;

/** A report with one tool's task replaced: a task just started shows at once, before the next detection. */
export function withTask(report: ToolsReport, task: SetupTask): ToolsReport {
  return { ...report, tools: report.tools.map((tool) => (tool.id === task.tool ? { ...tool, task } : tool)) };
}

/**
 * A report with one tool's sign-in as the cheap check just read it: signed in
 * (with a plan that includes it) is ready once the version is supported —
 * the server's rule — so the card turns ready before the next detection.
 */
export function withLogin(report: ToolsReport, id: ToolId, login: SetupTool["login"]): ToolsReport {
  return {
    ...report,
    tools: report.tools.map((tool) => {
      if (tool.id !== id) return tool;
      const signedIn = login.state === "signed-in" && login.usable !== false;
      return { ...tool, login, ready: tool.supported && (signedIn || tool.droppenAccounts.total > 0) };
    }),
  };
}

/** {@link setupActions} on this view's device, keeping the tools report in the cache current. */
export function useSetupActions() {
  const api = useApi();
  const machine = useMachine();
  const queryClient = useQueryClient();
  return useMemo(() => boundActions(setupActions(api), queryClient, machine), [api, machine, queryClient]);
}

function boundActions(actions: SetupActions, queryClient: QueryClient, machine: MachineKey) {
  const report = () => queryClient.getQueryData<ToolsReport | null>(setupToolsKey(machine)) ?? null;
  const started = (task: SetupTask) => {
    const now = report();
    if (now) queryClient.setQueryData(setupToolsKey(machine), withTask(now, task));
    void queryClient.invalidateQueries({ queryKey: setupToolsKey(machine) });
    return task;
  };
  const replaced = (next: ToolsReport) => {
    queryClient.setQueryData(setupToolsKey(machine), next);
    return next;
  };
  return {
    run: async (tool: ToolId, kind: "install" | "update", method: string) => started(await actions.run(tool, kind, method)),
    login: async (tool: ToolId, mode: LoginMode, apiKey?: string) => started(await actions.login(tool, mode, apiKey)),
    loginStatus: async (tool: ToolId) => {
      const login = await actions.loginStatus(tool);
      const now = report();
      if (now && login.state === "signed-in") {
        queryClient.setQueryData(setupToolsKey(machine), withLogin(now, tool, login));
        void queryClient.invalidateQueries({ queryKey: setupToolsKey(machine) });
      }
      return login;
    },
    rescan: async (patient: boolean) => replaced(await actions.rescan(patient)),
    use: async (tool: ToolId, path: string) => replaced(await actions.use(tool, path)),
    installGit: actions.installGit,
    stop: async (terminalId: string) => {
      await actions.stop(terminalId);
      void queryClient.invalidateQueries({ queryKey: setupToolsKey(machine) });
    },
    /** Detection again, after something outside the guide may have changed it (an account added). */
    refresh: () => void queryClient.invalidateQueries({ queryKey: setupToolsKey(machine) }),
  };
}

/**
 * Store a new progress. The cache takes it at once — the rail, the bars and
 * a navigation right after read it without waiting — and the server's answer
 * (its `flow` and `autoOpen`) replaces it when it comes, unless a newer save
 * was made meanwhile.
 */
export function useSaveProgress(): (next: SetupProgress) => Promise<SetupProgress | null> {
  const api = useApi();
  const machine = useMachine();
  const queryClient = useQueryClient();
  const latest = useRef(0);
  const mutation = useMutation({
    mutationFn: async ({ next }: { next: SetupProgress; seq: number }) => {
      const res = await api.PUT("/api/setup/progress", { body: next });
      return res.data ?? null;
    },
    onSuccess: (stored, { seq }) => {
      if (stored && seq === latest.current) queryClient.setQueryData(setupProgressKey(machine), stored);
    },
  });
  return (next) => {
    queryClient.setQueryData(setupProgressKey(machine), next);
    const seq = ++latest.current;
    return mutation.mutateAsync({ next, seq }).catch(() => null);
  };
}
