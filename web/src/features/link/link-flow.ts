import { useSyncExternalStore } from "react";
import { type QueryClient, useQueryClient } from "@tanstack/react-query";
import { ApiError } from "@/api/errors";
import type { ApiClient } from "@/api/client";
import { useApi, useMachine } from "@/lib/machines/context";
import { type MachineKey } from "@/lib/machines/key";
import { mq } from "@/lib/machines/query";
import { createStore, type WritableStore } from "@/lib/machines/store";
import { type Attempt, type HandoffStatus, LinkOpener, type OpenerState, type OpenerWindow } from "./opener";

/*
 * The local page's one sign-in to Droppen (`opener.ts`), per device and for
 * the life of the page: Settings → Access → Droppen (`link-settings.tsx`) and the
 * first-run card on the sessions page (`first-run.tsx`) start and show the
 * same attempt, so 「登录 Droppen」 on the card opens the droppen.app window in
 * the click itself and Settings → Access → Droppen goes on showing how it stands.
 */

/** `GET /api/cloud`'s cache entry: the device's Droppen state. */
export const cloudKey = (machine: MachineKey) => mq(machine, "cloud");

/** The local API's error as a sentence, carrying its machine-readable `code` too (`opener.ts`'s `errorCode`). */
export async function failure(res: { error?: unknown; response: Response }): Promise<Error & { code?: string }> {
  const body = (res.error ?? {}) as Record<string, unknown>;
  const message = typeof body.message === "string" ? body.message : null;
  const error = new ApiError(body);
  return Object.assign(new Error(message ?? error.code ?? `HTTP ${res.response.status}`), { code: error.code || undefined });
}

interface LinkFlow {
  opener: LinkOpener;
  /** What the page shows of the attempt (the opener's state, or set back to `idle` once it is over). */
  state: WritableStore<OpenerState>;
}

const flows = new Map<MachineKey, LinkFlow>();

/** Outside a browser (the render tests): no window to open or hear from. */
const NO_WINDOW: OpenerWindow = { open: () => null, addEventListener: () => undefined, removeEventListener: () => undefined };

/** The sign-in of `machine`'s page: made on first use, kept while the page lives. */
export function linkFlowFor(machine: MachineKey, api: ApiClient, queryClient: QueryClient): LinkFlow {
  const known = flows.get(machine);
  if (known) return known;
  const state = createStore<OpenerState>({ kind: "idle" });
  const opener = new LinkOpener({
    start: async (body) => {
      const res = await api.POST("/api/cloud/link", { body: { hub: body.hub ?? null, aggregate: body.aggregate } });
      if (!res.data) throw await failure(res);
      return res.data as Attempt;
    },
    finish: async (body) => {
      const res = await api.POST("/api/cloud/link/finish", { body: { ...body, email: body.email ?? null } });
      if (!res.data) throw await failure(res);
      return res.data;
    },
    cancel: async (attempt) => {
      await api.POST("/api/cloud/link/cancel", { body: { attempt } });
    },
    // The browser hand-off follows the attempt here; shared with Settings → Access → Droppen's own polling.
    status: async () =>
      (await queryClient.fetchQuery({
        queryKey: cloudKey(machine),
        queryFn: async () => {
          const res = await api.GET("/api/cloud");
          if (!res.data) throw await failure(res);
          return res.data;
        },
        staleTime: 500,
      })) as HandoffStatus,
    win: typeof window === "undefined" ? NO_WINDOW : undefined,
    onState: (next) => {
      state.set(next);
      if (next.kind === "enrolling" || next.kind === "done" || next.kind === "idle" || next.kind === "browser") void queryClient.invalidateQueries({ queryKey: cloudKey(machine) });
    },
  });
  const flow = { opener, state };
  flows.set(machine, flow);
  return flow;
}

/** The page's device's sign-in: its state as it changes, a way to set what is shown, and the opener. */
export function useLinkFlow(): { flow: OpenerState; setFlow: (next: OpenerState) => void; opener: LinkOpener } {
  const machine = useMachine();
  const api = useApi();
  const queryClient = useQueryClient();
  const { opener, state } = linkFlowFor(machine, api, queryClient);
  const flow = useSyncExternalStore(state.subscribe, state.get, state.get);
  return { flow, setFlow: state.set, opener };
}
