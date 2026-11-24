/*
 * The A2A management API of the view's device (`/api/a2a/*`, `docs/a2a.md`
 * §2): its entries, tokens and tasks, cached per device. Entries and tokens
 * are posture — a device reaching another over a channel needs to be a full
 * device to read or change them — and the task list is no more than the
 * session list shows.
 */
import { useQuery } from "@tanstack/react-query";
import { ApiError } from "@/api/errors";
import { useApi, useMachine } from "@/lib/machines/context";
import type { MachineKey } from "@/lib/machines/key";
import { mq } from "@/lib/machines/query";
import { a2aMarks, type A2aMark } from "./model";

export const a2aEntriesKey = (machine: MachineKey) => mq(machine, "a2a", "entries");
export const a2aTokensKey = (machine: MachineKey) => mq(machine, "a2a", "tokens");
export const a2aTasksKey = (machine: MachineKey, entry = "", state = "") => mq(machine, "a2a", "tasks", entry, state);
/** Every cached task list of the device, whatever it was filtered by. */
export const a2aAllTasksKey = (machine: MachineKey) => mq(machine, "a2a", "tasks");

/** The entries, and the template an entry without its own uses. */
export function useA2aEntries() {
  const api = useApi();
  const machine = useMachine();
  return useQuery({
    queryKey: a2aEntriesKey(machine),
    queryFn: async () => {
      // The route declares no failure, but a device reached over a channel
      // can refuse it (a limited device), and an older server has no route.
      const { data, error } = await api.GET("/api/a2a/entries");
      if (!data) throw new ApiError(error);
      return data;
    },
  });
}

/** Every token, revoked ones too, newest first, never with a secret. */
export function useA2aTokens() {
  const api = useApi();
  const machine = useMachine();
  return useQuery({
    queryKey: a2aTokensKey(machine),
    queryFn: async () => {
      // The route declares no failure, but a device reached over a channel
      // can refuse it (a limited device), and an older server has no route.
      const { data, error } = await api.GET("/api/a2a/tokens");
      if (!data) throw new ApiError(error);
      return data;
    },
  });
}

/** The tasks, most recently changed first, filtered by entry and state (`""`: all). */
export function useA2aTasks(entry: string, state: string, refetchInterval: number | false = 10_000) {
  const api = useApi();
  const machine = useMachine();
  return useQuery({
    queryKey: a2aTasksKey(machine, entry, state),
    queryFn: async () => {
      const res = await api.GET("/api/a2a/tasks", {
        params: { query: { ...(entry ? { entry } : {}), ...(state ? { state } : {}) } },
      });
      if (res.error) throw new ApiError(res.error);
      return res.data;
    },
    refetchInterval,
  });
}

/**
 * Which of the view's device's sessions were started over A2A, for the
 * session list's mark, from the task list (its newest 500 tasks). Asked again
 * when the list gains or loses a session (`sessions`), so a session an A2A
 * client just started is marked as soon as it is listed. A device whose
 * server has no A2A server answers no list, and marks nothing.
 */
export function useA2aMarks(sessions: number): ReadonlyMap<string, A2aMark> {
  const api = useApi();
  const machine = useMachine();
  const { data } = useQuery({
    queryKey: mq(machine, "a2a", "marks", sessions),
    queryFn: async () => {
      const res = await api.GET("/api/a2a/tasks");
      return res.data ? a2aMarks(res.data.tasks) : NO_MARKS;
    },
    placeholderData: (previous) => previous,
    staleTime: 60_000,
    retry: false,
  });
  return data ?? NO_MARKS;
}

const NO_MARKS: ReadonlyMap<string, A2aMark> = new Map();
