/*
 * Whether a view's machine is shown from its copy, in any mode: cloud mode's
 * transport switches between the machine and its offline copy
 * (`lib/transport/switching.ts`), and the local app shows another
 * device only from its copy, read-only (its gateway, S1-7) — and so whether
 * a session's view holds the machine's own state. Apart from the cloud
 * modules, so the views every mode shares (a session, a terminal) read it
 * without loading them.
 */
import { useSyncExternalStore } from "react";
import type { CloudMode, SwitchingTransport } from "@/lib/transport/switching";
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

/**
 * Whether a session's view holds its machine's own state: what only the
 * machine can say of the session — the model, effort and permission it runs
 * with (`cliChoice`), the account it runs on, its context — and what a pick
 * made here is stored against. The one test for showing any of it, in the
 * composer's row and in the session's header alike. It is not in
 *
 * - while the state is the machine's offline copy's (`fromCopy` on the feed's
 *   state, `session-feed-pool.ts`): a window drawn from the copy while
 *   droppen.app waits for the machine (`feed-from-copy.ts`), or the state the
 *   copy itself serves while the page is on it (`lib/transport/offline.ts`),
 *   until the machine's own state replaces it, a round trip after the page
 *   reaches the machine again;
 * - while the page shows the machine from its copy ({@link useShowingCopy}),
 *   whatever the view still holds from before: nothing picked here reaches
 *   the machine, and the copy's own state is on its way.
 *
 * Without it those values would be the defaults, the local login's quota
 * and "no account", so they are left out rather than shown and then changed
 * under the reader. Never read off a `null` `cliChoice`: a live session can
 * have one.
 */
export function useMachineStateIn(state: { fromCopy?: true } | null | undefined): boolean {
  const showingCopy = useShowingCopy();
  return !!state && state.fromCopy !== true && !showingCopy;
}
