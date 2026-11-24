/*
 * react-query keys, by machine.
 *
 * Every cache entry is something one machine said: two machines' prefs, two
 * machines' terminal lists, are two entries. So every key starts with the
 * machine — `["m", machine, ...]` — built here and nowhere else
 * (`scripts/check-machine-keys.mjs` refuses a key written out by hand), which
 * also makes "everything this machine said" one prefix to invalidate when the
 * page switches between it and its offline copy.
 */
import type { QueryClient } from "@tanstack/react-query";
import type { MachineKey } from "./key";

/** The first element of every key built here. */
export const MACHINE_QUERY = "m";

/** The key of something `machine` says: `mq(machine, "prefs")`. */
export function mq<const T extends readonly unknown[]>(machine: MachineKey, ...parts: T): readonly [typeof MACHINE_QUERY, MachineKey, ...T] {
  return [MACHINE_QUERY, machine, ...parts];
}

/** The prefix every one of `machine`'s keys shares. */
export function machineQueries(machine: MachineKey): readonly [typeof MACHINE_QUERY, MachineKey] {
  return [MACHINE_QUERY, machine];
}

/** How many of `machine`'s queries something watches now: those {@link invalidateMachine} asks again at once (for the connection log). */
export function machineQueryCount(client: QueryClient, machine: MachineKey): number {
  try {
    return client.getQueryCache().findAll({ queryKey: machineQueries(machine), type: "active" }).length;
  } catch {
    return -1;
  }
}

/** Ask everything `machine` said again, and nothing any other machine said. */
export function invalidateMachine(client: QueryClient, machine: MachineKey): Promise<void> {
  return client.invalidateQueries({ queryKey: machineQueries(machine) });
}
