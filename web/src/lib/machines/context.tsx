/*
 * Which machine a view is about, handed down the tree.
 *
 * A component reads its machine with {@link useMachine} and talks to it with
 * {@link useApi} or {@link useMachineRuntime}; it never reaches for "the"
 * server. One {@link MachineProvider} at the root says the home machine, so
 * every view is about it unless something nearer says otherwise: another
 * machine's pages (`/m/<machine>/…`, `app/machine-route.tsx`) and a pane
 * showing another machine's session or terminal (`docs/local-aggregator.md`
 * §6.5) are each a provider around them, and nothing inside has to change.
 * The home itself moves only when a new main device is chosen, and the
 * root's tree then starts over on it ({@link useHomeMachine}).
 */
import { createContext, useContext, useEffect, useMemo, type ReactNode } from "react";
import type { ApiClient } from "@/api/client";
import { sessionRef, type MachineKey, type SessionRef } from "./key";
import { homeMachine, homeMachineStore, isOpen, openMachinesStore, runtimeFor, type MachineInfo, type MachineRuntime } from "./registry";
import { constantStore, type Store, useStore } from "./store";

const MachineContext = createContext<MachineKey | null>(null);

export function MachineProvider({ machine, children }: { machine: MachineKey; children: ReactNode }) {
  return <MachineContext.Provider value={machine}>{children}</MachineContext.Provider>;
}

/** The machine this view is about: the nearest {@link MachineProvider}'s, or the home machine. */
export function useMachine(): MachineKey {
  return useContext(MachineContext) ?? homeMachine();
}

export function useMachineRuntime(): MachineRuntime {
  return runtimeFor(useMachine());
}

/** The home machine, re-rendering when it moves (droppen.app, switching machines). */
export function useHomeMachine(): MachineKey {
  return useStore(homeMachineStore());
}

/** What the page knows about this view's machine, as it changes. */
export function useMachineInfo(): MachineInfo {
  return useStore(useMachineRuntime().info());
}

/**
 * This view is about to use its machine itself (a session's transcript, a
 * terminal): ask for the live connection, which a machine shown only from its
 * copy does not have, and keep it for as long as the view is there
 * (`MachineRuntime.hold`). Nothing on a machine that is always live (the
 * local page's own server).
 */
export function useEnsureLive(): void {
  useHoldMachine(useMachine());
}

/**
 * Hold `machine` while this view is mounted (`MachineRuntime.hold`): a pane
 * or a page of it is on the page, so it is in use. `null`, or a machine the
 * page does not have (any more): nothing.
 */
export function useHoldMachine(machine: MachineKey | null): void {
  const open = useIsOpen(machine);
  useEffect(() => {
    // Again here, not only as rendered: the machine may have gone between the
    // commit and this effect (a chain change landing in between). Then there
    // is nothing to hold, and the view re-renders without it.
    if (machine === null || !open || !isOpen(machine)) return;
    return runtimeFor(machine).hold();
  }, [machine, open]);
}

/** Every machine open on the page, home first, as they come and go. */
export function useOpenMachines(): readonly MachineKey[] {
  return useStore(openMachinesStore());
}

/** Whether `machine` is open on the page, as machines come and go (`null`: no). */
export function useIsOpen(machine: MachineKey | null): boolean {
  useOpenMachines();
  return machine !== null && isOpen(machine);
}

const UNKNOWN = new Map<MachineKey, Store<MachineInfo>>();

/**
 * What the page knows about `machine`, as it changes — also for a machine
 * that is not open (any more): it reads as unknown, by its key.
 */
export function useInfoOf(machine: MachineKey): MachineInfo {
  const open = useIsOpen(machine);
  let store: Store<MachineInfo>;
  if (open) store = runtimeFor(machine).info();
  else {
    store = UNKNOWN.get(machine) ?? constantStore<MachineInfo>({ key: machine, name: machine, state: "unknown", since: 0, canManage: false, live: "copy" });
    UNKNOWN.set(machine, store);
  }
  return useStore(store);
}

/** The typed client of this view's machine. */
export function useApi(): ApiClient {
  return useMachineRuntime().api;
}

/** The session `source/id` on this view's machine, as one stable ref. */
export function useSessionRef(source: string, id: string): SessionRef {
  const machine = useMachine();
  return useMemo(() => sessionRef(machine, source, id), [machine, source, id]);
}
