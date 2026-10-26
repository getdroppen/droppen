/*
 * The hooks the cloud screens read the page's cloud state with: the hub
 * link's state and presence, a machine's channel, paths and copy (whether a
 * view shows the copy is `features/machines/copy-mode.ts`, which every mode
 * reads). What there is to read — the account, the link, every machine's
 * channel and copy — is droppen.app's shell (`hub-shell.ts`, `hubShell()`),
 * `null` in local mode, which is how the cloud components render nothing
 * there.
 */
import { useMemo, useSyncExternalStore } from "react";
import { type AgentKeyState, agentKeyState, onAgentKeyState } from "@/lib/hub/deliver";
import type { HubLink, LinkState, PresenceState } from "@/lib/hub/link";
import type { PathStats } from "@/lib/e2e";
import type { MachineChannel, MachineStatus } from "@/lib/hub/machine";
import type { MachineMirror } from "@/lib/hub/mirror";
import type { RelayPool } from "@/lib/hub/relays";
import type { MachineKey } from "@/lib/machines/key";
import { hubOnly, onHubOnly } from "@/lib/transport/p2p";
import { hubShell } from "@/features/machines/shells";

/** `machine`'s offline copy, when droppen.app has it open and this device holds a key for it. */
export function mirrorFor(machine: MachineKey): MachineMirror | null {
  return hubShell()?.machine(machine)?.mirror ?? null;
}

/** The copies droppen.app's shell holds, as a lookup that is a new one when the shell is another (an effect that reads it re-runs). */
export function useShellCopies(): (machine: MachineKey) => MachineMirror | null {
  const shell = hubShell();
  return useMemo(() => (machine: MachineKey) => shell?.machine(machine)?.mirror ?? null, [shell]);
}

export function usePresence(link: HubLink | null | undefined, machine: string | null | undefined): PresenceState | null {
  return useSyncExternalStore(
    (notify) => (link ? link.onPresence(notify) : () => undefined),
    () => (link && machine ? (link.presence.get(machine)?.state ?? null) : null),
  );
}

/**
 * A machine's presence as this page can tell (relay-and-plans §2.9): online
 * when the hub says so, or when a relay this page is connected to has it
 * signed in — so a hub restart does not flip the machine to its offline copy.
 */
export function useMachinePresence(link: HubLink | null | undefined, relays: RelayPool | null | undefined, machine: string | null | undefined): PresenceState | null {
  const hub = usePresence(link, machine);
  const onRelay = useSyncExternalStore(
    (notify) => (relays ? relays.onChange(notify) : () => undefined),
    () => (relays && machine ? relays.online(machine) : false),
    () => false,
  );
  return onRelay ? "online" : hub;
}

/** Re-render when the relay pool changes (its list, round trips, connections, the manual choice). */
export function useRelayPool(relays: RelayPool | null | undefined): number {
  return useSyncExternalStore(
    (notify) => (relays ? relays.onChange(notify) : () => undefined),
    () => relays?.version ?? 0,
    () => 0,
  );
}

export function usePresenceVersion(link: HubLink | null | undefined): number {
  return useSyncExternalStore(
    (notify) => {
      if (!link) return () => undefined;
      const a = link.onPresence(() => {
        version++;
        notify();
      });
      return a;
    },
    () => version,
    () => version,
  );
}
let version = 0;

export function useLinkState(link: HubLink | null | undefined): LinkState {
  return useSyncExternalStore(
    (notify) => (link ? link.onState(notify) : () => undefined),
    () => link?.state ?? "closed",
  );
}

export function useChannelStatus(channel: MachineChannel | null | undefined): MachineStatus {
  return useSyncExternalStore(
    (notify) => (channel ? channel.onStatus(notify) : () => undefined),
    () => channel?.status ?? "idle",
  );
}

/** How the open channel reaches its machine (direct or via the hub, and the round trips); `null` before it is open, or with an agent that does not sequence frames. */
export function usePaths(channel: MachineChannel | null | undefined): PathStats | null {
  return useSyncExternalStore(
    (notify) => (channel ? channel.onPaths(notify) : () => undefined),
    () => channel?.paths ?? null,
    () => null,
  );
}

/** This device's "hub only" setting (no direct paths). */
export function useHubOnly(): boolean {
  return useSyncExternalStore(onHubOnly, hubOnly, () => false);
}

export interface CopyView {
  /** The device copy has been read. */
  loaded: boolean;
  hasCopy: boolean;
  /** How current it is (the newest machine-signed head of the index), ms. */
  freshestMs: number | null;
}

const NO_COPY: CopyView = { loaded: false, hasCopy: false, freshestMs: null };
let copyView: { mirror: MachineMirror | null; view: CopyView } = { mirror: null, view: NO_COPY };

function viewOf(mirror: MachineMirror | null | undefined): CopyView {
  if (!mirror) return NO_COPY;
  const next = { loaded: mirror.loaded, hasCopy: mirror.hasCopy, freshestMs: mirror.freshestMs };
  const held = copyView.mirror === mirror ? copyView.view : null;
  if (held && held.loaded === next.loaded && held.hasCopy === next.hasCopy && held.freshestMs === next.freshestMs) return held;
  copyView = { mirror, view: next };
  return next;
}

/** The offline copy's state, for the notice that says how old it is. */
export function useCopyView(mirror: MachineMirror | null | undefined): CopyView {
  return useSyncExternalStore(
    (notify) => {
      if (!mirror) return () => undefined;
      const off = mirror.onChange((change) => {
        if (change.kind === "index") notify();
      });
      void mirror.whenLoaded().then(notify);
      return off;
    },
    () => viewOf(mirror),
  );
}

/** What the last check on this page said about `machine`'s agent and its key (`undefined`: not checked). */
export function useAgentKeyState(machine: string | null | undefined): AgentKeyState | undefined {
  return useSyncExternalStore(
    (notify) => onAgentKeyState(notify),
    () => (machine ? agentKeyState(machine) : undefined),
    () => undefined,
  );
}
