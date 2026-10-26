/*
 * Whether a view's machine is shown from its copy, in any mode: cloud mode's
 * transport switches between the machine and its offline copy
 * (`features/cloud/cloud-transport.ts`), and the local app shows another
 * device only from its copy, read-only (its gateway, S1-7). Apart from the
 * cloud modules, so the views every mode shares (a session, a terminal) read
 * it without loading them.
 */
import { useSyncExternalStore } from "react";
import type { CloudMode, SwitchingTransport } from "@/features/cloud/cloud-transport";
import type { Transport } from "@/lib/transport/types";
import { useMachineRuntime } from "@/lib/machines/context";
import { useStore } from "@/lib/machines/store";

/** Whether the page is on the machine (`live`) or its copy (`offline`); `live` outside cloud mode. */
export function useCloudMode(transport: SwitchingTransport | null | undefined): CloudMode {
  return useSyncExternalStore(
    (notify) => (transport ? transport.onMode(notify) : () => undefined),
    () => transport?.current ?? "live",
  );
}

/** A transport that switches between a machine and its offline copy (cloud mode's), or `null`. */
function switching(transport: Transport): SwitchingTransport | null {
  return "onMode" in transport ? (transport as SwitchingTransport) : null;
}

/**
 * Whether this view's machine is being shown from its offline copy right now
 * (any component, cloud mode or not) — also another device the local app
 * only ever shows from its copy, read-only (its gateway, S1-7).
 */
export function useShowingCopy(): boolean {
  const runtime = useMachineRuntime();
  const mode = useCloudMode(switching(runtime.transport));
  const readOnly = useStore(runtime.info()).readOnly === true;
  return mode === "offline" || readOnly;
}
