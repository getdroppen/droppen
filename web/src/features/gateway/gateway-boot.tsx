/*
 * When the local app shows the account's other devices (S1-7): while the
 * local install's `GET /api/cloud` says this local account sees them
 * (`seesOtherDevices`: the local account that signed this device in to
 * Droppen, or one it allowed — never anyone with local sign-in off) and its aggregator
 * runs, the page opens the gateway (`/api/agg/ws`) and the gateway shell
 * (`gateway-shell.ts`); otherwise it is the local app alone, exactly as
 * before.
 *
 * The status is the settings page's own query (`cloudKey`), looked at again
 * every minute: signing in, turning the aggregator on or off, an allowance
 * given or taken back show here at the next look (at once, when it was done
 * on this page). The gateway sending the page away (`bye`) — the allowance
 * taken back from another page — ends it at once and looks again; when the
 * status still says to show them (taken back and given again meanwhile), a
 * new gateway starts.
 */
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { cloudKey, failure } from "@/features/link/link-flow";
import { currentUser } from "@/lib/auth";
import { GatewayLink } from "@/lib/gateway/link";
import { LOCAL } from "@/lib/machines/key";
import { apiFor, transportFor } from "@/lib/machines/registry";
import { stopGatewayShell } from "@/features/machines/shells";

/** How often the page looks again at whether it should show other devices. */
export const RECHECK_MS = 60_000;

/** Whether `GET /api/cloud` says this page shows the other devices. */
export function showsOtherDevices(status: { seesOtherDevices?: boolean; aggregator?: { enabled?: boolean; state?: string } } | undefined): boolean {
  return status?.seesOtherDevices === true && status.aggregator?.enabled === true && status.aggregator.state === "on";
}

/**
 * The shell and the copies it reads: a chunk of their own, fetched only by a
 * page that shows other devices.
 */
const loadGatewayShell = () => import("./gateway-shell");

/**
 * `loadShell` and `retryMs` stand in for the shell's chunk and the wait
 * before it is fetched again when it did not come (tests); the page uses its
 * own and {@link RECHECK_MS}.
 */
export function GatewayBoot({ loadShell = loadGatewayShell, retryMs = RECHECK_MS }: { loadShell?: () => Promise<Pick<typeof import("./gateway-shell"), "startGatewayShell">>; retryMs?: number } = {}) {
  const queryClient = useQueryClient();
  const { data } = useQuery({
    queryKey: cloudKey(LOCAL),
    queryFn: async () => {
      const res = await apiFor(LOCAL).GET("/api/cloud");
      if (!res.data) throw await failure(res);
      return res.data;
    },
    refetchInterval: RECHECK_MS,
    retry: false,
  });
  const show = showsOtherDevices(data);
  // Bumped after a `bye` once the status was looked at again, and when the
  // shell's chunk did not come: a new gateway, if still shown.
  const [round, setRound] = useState(0);
  useEffect(() => {
    if (!show) {
      stopGatewayShell();
      return;
    }
    const lookAgain = () => void queryClient.invalidateQueries({ queryKey: cloudKey(LOCAL) });
    const local = transportFor(LOCAL);
    const link = new GatewayLink({
      socketUrl: () => local.socketUrl("/api/agg/ws"),
      openSocket: (url) => local.openSocket(url),
      onTrouble: lookAgain,
    });
    let gone = false;
    let again: ReturnType<typeof setTimeout> | undefined;
    const offBye = link.onBye(() => {
      stopGatewayShell();
      void queryClient.refetchQueries({ queryKey: cloudKey(LOCAL) }).finally(() => {
        if (!gone) setRound((n) => n + 1);
      });
    });
    void loadShell().then(
      ({ startGatewayShell }) => {
        if (!gone) startGatewayShell(link, { user: currentUser() ?? null });
      },
      () => {
        // The chunk did not come (the link dropped, or the server moved to
        // another build): fetched again at the next look.
        if (!gone) again = setTimeout(() => setRound((n) => n + 1), retryMs);
      },
    );
    return () => {
      gone = true;
      clearTimeout(again);
      offBye();
      stopGatewayShell();
    };
  }, [show, round, queryClient, loadShell, retryMs]);
  return null;
}
