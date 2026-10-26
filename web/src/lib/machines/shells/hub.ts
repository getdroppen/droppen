/*
 * droppen.app's machines in the registry (`docs/local-aggregator.md` §6.2,
 * `HubShell`).
 *
 * The shell itself — every machine of the account at once, each through its
 * own end-to-end channel or its offline copy — is built from the cloud
 * modules (`features/cloud/hub-shell.ts`); this is its registry half. There
 * is no {@link LOCAL} here: the page's origin is the hub, and a machine is its
 * device id. The first machine opened is the home until the shell moves it.
 */
import type { Transport } from "@/lib/transport/types";
import { LOCAL, type MachineKey } from "../key";
import { homeMachine, isOpen, type MachineDriver, openMachine, setHome, setShell, shellKind } from "../registry";
import type { MachineRuntime } from "../runtime";

/**
 * Make the page droppen.app's, before any machine is open. Nothing here
 * reaches the page's own origin as a machine: a `local` made before the shell
 * knew better (a stray early ask) goes.
 */
export function enterHubShell(): void {
  if (shellKind() !== "hub") setShell("hub");
}

/** Open `machine` (a device id) on droppen.app, through `transport`. The first machine opened is the home. */
export function openHubMachine(machine: MachineKey, transport: Transport, driver?: MachineDriver): MachineRuntime {
  if (machine === LOCAL) throw new Error(`"${LOCAL}" is not a device id`);
  enterHubShell();
  const runtime = openMachine(machine, transport, driver);
  if (!isOpen(homeMachine())) setHome(machine);
  return runtime;
}
