/**
 * Every device's session list as the page already has it (§6.4 "行从哪来"):
 * the device the app is live on from its own feed, every other one from its
 * offline copy's index — which the page follows anyway (droppen.app follows
 * every machine's index and status, `hub-shell.ts`) — so listing them, and
 * working out what needs you across them (`needs-you.ts`), dials nothing and
 * subscribes to nothing more.
 *
 * In the local app there is only `local`, from its feed: the same list, and
 * so the same numbers, as before there were devices — unless its gateway
 * shows other devices too (`features/gateway`, S1-7), each from its copy as
 * the local install holds it.
 */
import { useCallback, useRef, useSyncExternalStore } from "react";
import type { MachineKey } from "@/lib/machines/key";
import type { MachineInfo } from "@/lib/machines/runtime";
import { type Store, useStore } from "@/lib/machines/store";
import { gatewayShell, gatewayShellStore, hubShell, useGatewayMachines, useShellMachines } from "./shells";
import type { DeviceSessions } from "./needs-you";

/**
 * A device's list in the page's shell: droppen.app's machines' copies, or —
 * in the local app — another device's copy through the gateway (none for
 * `local`, whose list is its feed).
 */
function shellSessions(machine: MachineKey): Store<DeviceSessions> | null {
  return hubShell()?.machine(machine)?.sessions ?? gatewayShell()?.machine(machine)?.sessions ?? null;
}

/**
 * Every device the page shows, as each changes: droppen.app's machines, or
 * the local app's when its gateway shows other devices (this one first) —
 * empty in the local app alone.
 */
export function useShellDevices(): readonly MachineInfo[] {
  const hub = useShellMachines();
  const gateway = useGatewayMachines();
  return hubShell() ? hub : gateway;
}

const NONE: readonly MachineInfo[] = [];

/** The same array as before when every list in it is the same one. */
function sameLists(a: readonly DeviceSessions[], b: readonly DeviceSessions[]): boolean {
  return a.length === b.length && a.every((list, i) => list === b[i]);
}

/**
 * Every device's session list, in the shell's order, as each changes.
 *
 * `live` is the list of the device the app is live on (its feed, from inside
 * the app): it stands in for that device's copy, which trails it. In the local
 * app it is the only one. Without it (droppen.app's dashboard, where no app is
 * open) every device is read off its copy.
 */
export function useDeviceSessions(live: DeviceSessions | null = null, enabled = true): readonly DeviceSessions[] {
  // Not wanted (a list of one device's sessions): nothing followed, so no
  // other device's change re-renders the page.
  const all = useShellDevices();
  const machines = enabled ? all : NONE;
  const gateway = useStore(gatewayShellStore());
  const shell = hubShell() ?? gateway;
  const keys = machines.map((m) => m.key);
  // The stores as the shell has them now: a machine closed and opened again
  // has a new store under the same key, so they are compared one by one.
  const now = keys.map((key) => shellSessions(key));
  const stores = useRef<{ list: (Store<DeviceSessions> | null)[]; version: number }>({ list: [], version: 0 });
  if (now.length !== stores.current.list.length || now.some((store, i) => store !== stores.current.list[i])) {
    stores.current = { list: now, version: stores.current.version + 1 };
  }
  const version = stores.current.version;
  const held = useRef<readonly DeviceSessions[]>([]);
  const subscribe = useCallback(
    (notify: () => void) => {
      const offs = stores.current.list.map((store) => store?.subscribe(notify) ?? (() => undefined));
      return () => {
        for (const off of offs) off();
      };
    },
    // Re-subscribed when the stores change (another device, the shell gone, a device opened again).
    [version, shell],
  );
  const snapshot = (): readonly DeviceSessions[] => {
    const lists: DeviceSessions[] = [];
    if (keys.length === 0) {
      if (live) lists.push(live);
    } else {
      keys.forEach((key, i) => {
        const store = stores.current.list[i];
        // The live list stands in for the device's copy once it has rows: a
        // feed that has not had its first snapshot yet (the app just came up
        // on a new home) would count the device as nothing in the meantime.
        if (live && live.machine === key && (live.rows !== null || !store)) lists.push(live);
        else if (store) lists.push(store.get());
      });
    }
    if (sameLists(held.current, lists)) return held.current;
    held.current = lists;
    return lists;
  };
  return useSyncExternalStore(subscribe, snapshot, snapshot);
}
