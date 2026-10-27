/*
 * The shell the page runs besides its own machine, as the views every mode
 * shares read it (the unified list, 「需要你」, a session or a terminal on
 * another device): droppen.app's (`features/cloud/hub-shell.ts`, set by the
 * cloud app once the page is past sign-in and device setup) or the local
 * app's gateway (`features/gateway/gateway-shell.ts`, while it shows other
 * devices). Both live in one slot in `lib/machines/registry.ts`; the typed
 * accessors here are what cloud-only and gateway-only code reads. Only where
 * each is held: the shells themselves are built from modules the local app's
 * boot does not carry (`build/local-boot.ts`).
 */
import type { HubShell } from "@/features/cloud/hub-shell";
import type { GatewayShell } from "@/features/gateway/gateway-shell";
import type { MachineKey } from "@/lib/machines/key";
import { currentShell, currentShellStore, setCurrentShell } from "@/lib/machines/registry";
import type { MachineInfo } from "@/lib/machines/runtime";
import { constantStore, type Store, useStore } from "@/lib/machines/store";

/** droppen.app's shell, once the page is past sign-in and device setup; `null` in local mode. */
export function hubShell(): HubShell | null {
  const shell = currentShell();
  return shell?.kind === "hub" ? (shell as HubShell) : null;
}

export function setHubShell(next: HubShell | null): void {
  setCurrentShell(next);
}

const NO_FOCUS = constantStore<MachineKey | null>(null);

/** The machine droppen.app has in view (`null`: 「我的设备」, or local mode), as it changes. */
export function useFocusedMachine(): MachineKey | null {
  const shell = hubShell();
  return useStore(shell?.focus ?? NO_FOCUS);
}

const NO_MACHINES = constantStore<readonly MachineInfo[]>([]);

/** Every machine the page's shell has, as each changes (empty in the local app alone). */
export function useShellMachines(): readonly MachineInfo[] {
  const shell = useStore(currentShellStore());
  return useStore(shell?.machines() ?? NO_MACHINES);
}

/** The local app's gateway shell, while it shows other devices; `null` otherwise (and on droppen.app). */
export function gatewayShell(): GatewayShell | null {
  const shell = currentShell();
  return shell?.kind === "gateway" ? (shell as GatewayShell) : null;
}

/** The gateway shell as it comes and goes, for `useStore` (stable: one store for the one slot). */
const gatewayStore: Store<GatewayShell | null> = {
  get: () => gatewayShell(),
  subscribe: (notify) => currentShellStore().subscribe(notify),
};

export function gatewayShellStore(): Store<GatewayShell | null> {
  return gatewayStore;
}

/** The gateway shell that just started (`startGatewayShell`). */
export function setGatewayShell(shell: GatewayShell): void {
  setCurrentShell(shell);
}

/** Stop the gateway shell, if one runs. */
export function stopGatewayShell(): void {
  const shell = gatewayShell();
  if (!shell) return;
  setCurrentShell(null);
  shell.stop();
}
