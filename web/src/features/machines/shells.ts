/*
 * The shell the page runs besides its own machine, as the views every mode
 * shares read it (the unified list, 「需要你」, a session or a terminal on
 * another device): droppen.app's (`features/cloud/hub-shell.ts`, set by the
 * cloud app once the page is past sign-in and device setup) or the local
 * app's gateway (`features/gateway/gateway-shell.ts`, while it shows other
 * devices). Only where each is held: the shells themselves are built from
 * modules the local app's boot does not carry (`build/local-boot.ts`).
 */
import type { HubShell } from "@/features/cloud/hub-shell";
import type { GatewayShell } from "@/features/gateway/gateway-shell";
import type { MachineKey } from "@/lib/machines/key";
import type { MachineInfo } from "@/lib/machines/runtime";
import { constantStore, createStore, type Store, useStore } from "@/lib/machines/store";

let hub: HubShell | null = null;

/** droppen.app's shell, once the page is past sign-in and device setup; `null` in local mode. */
export function hubShell(): HubShell | null {
  return hub;
}

export function setHubShell(next: HubShell | null): void {
  hub = next;
}

const NO_FOCUS = constantStore<MachineKey | null>(null);

/** The machine droppen.app has in view (`null`: 「我的设备」, or local mode), as it changes. */
export function useFocusedMachine(): MachineKey | null {
  return useStore(hub?.focus ?? NO_FOCUS);
}

/** Every machine droppen.app has, as each changes (empty in local mode). */
export function useShellMachines(): readonly MachineInfo[] {
  return useStore(hub?.machines() ?? NO_MACHINES);
}
const NO_MACHINES = constantStore<readonly MachineInfo[]>([]);

const gateway = createStore<GatewayShell | null>(null);

/** The local app's gateway shell, while it shows other devices; `null` otherwise (and on droppen.app). */
export function gatewayShell(): GatewayShell | null {
  return gateway.get();
}

/** The gateway shell as it comes and goes. */
export function gatewayShellStore(): Store<GatewayShell | null> {
  return gateway;
}

/** The gateway shell that just started (`startGatewayShell`). */
export function setGatewayShell(shell: GatewayShell): void {
  gateway.set(shell);
}

/** Stop the gateway shell, if one runs. */
export function stopGatewayShell(): void {
  const shell = gateway.get();
  if (!shell) return;
  gateway.set(null);
  shell.stop();
}

const NO_DEVICES = constantStore<readonly MachineInfo[]>([]);

/** The devices the local app shows (this one first), as they change; empty without other devices. */
export function useGatewayMachines(): readonly MachineInfo[] {
  const shell = useStore(gateway);
  return useStore(shell?.machines() ?? NO_DEVICES);
}
