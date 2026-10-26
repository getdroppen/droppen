/*
 * The router's half of addressing machines (`lib/machines/routes.ts`,
 * `docs/local-aggregator.md` §6.7 M-2): the `/m/$machine` layout every other
 * machine's pages sit under, what a view needs to know about the address it
 * is at, and links and navigations that stay on the view's machine.
 *
 * A view never writes `/m/…` itself: it says where it goes as the home would
 * (`/sessions/$source/$id`), and {@link MachineLink} / {@link useMachineNavigate}
 * put it on the machine the view is about (`useMachine()`).
 */
import { Link, Navigate, Outlet, useNavigate, useParams, useRouterState } from "@tanstack/react-router";
import { useCallback, useState, type ComponentProps, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { MachineProvider, useHoldMachine, useHomeMachine, useIsOpen, useMachine } from "@/lib/machines/context";
import type { MachineKey } from "@/lib/machines/key";
import { machineOfPath, machineTarget, plainHomeAddress, splitMachinePath } from "@/lib/machines/routes";
import { GlobalFeedProvider } from "./global-feed";

/**
 * The machine the address is about, as it changes: the one `/m/<machine>`
 * names, else the home — and the home for a machine the page does not have
 * (its route says so; nothing else follows it there).
 */
export function useRouteMachine(): MachineKey {
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const home = useHomeMachine();
  const named = machineOfPath(pathname, home);
  return useIsOpen(named) ? named : home;
}

/**
 * Whether the router's address is the address bar's: the path of its `href`
 * — as the browser writes it, percent-encoded — against `location.pathname`.
 * (The router's `pathname` is decoded: `/sessions/claude/a b` for the bar's
 * `/sessions/claude/a%20b`, and would never match.)
 */
export function routerCaughtUp(routerHref: string, browserPathname: string): boolean {
  const bare = (path: string) => (path.length > 1 ? path.replace(/\/+$/, "") : path);
  return bare(routerHref.split(/[?#]/, 1)[0] ?? "") === bare(browserPathname);
}

/**
 * Whether the router has caught up with the address bar. On droppen.app the
 * app's tree goes while 「我的设备」 is shown and the shell moves the address
 * under it; mounted again, the router renders the page it last had until it
 * has loaded the address — another device's page, perhaps, whose views would
 * ask for that device live. Until then nothing below should render. Once
 * caught up, it stays so: a navigation moves the router before the address
 * bar (its history writes are queued), and that gap is the router's own.
 */
export function useRouterCaughtUp(): boolean {
  const href = useRouterState({ select: (s) => s.location.href });
  const [caught, setCaught] = useState(() => typeof location === "undefined" || routerCaughtUp(href, location.pathname));
  if (!caught && typeof location !== "undefined" && routerCaughtUp(href, location.pathname)) setCaught(true);
  return caught;
}

/** The address's path as the home would write it: without its `/m/<machine>`. */
export function useSectionPath(): string {
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  return splitMachinePath(pathname).path;
}

/**
 * Everything under this is about `machine`: its provider and its session
 * list (`GlobalFeedProvider`, shared with every other view of it), and —
 * `hold` — the machine held while this is mounted (`useHoldMachine`). Inside
 * a view already about `machine`, only the hold.
 */
export function MachineView({ machine, hold = false, children }: { machine: MachineKey; hold?: boolean; children: ReactNode }) {
  const outer = useMachine();
  useHoldMachine(hold ? machine : null);
  if (machine === outer) return <>{children}</>;
  return (
    <MachineProvider key={machine} machine={machine}>
      <GlobalFeedProvider>{children}</GlobalFeedProvider>
    </MachineProvider>
  );
}

/**
 * `/m/$machine`: another machine's pages. The home's go to their plain
 * address (one address per page); a machine the page does not have says so.
 */
export function MachineRoute() {
  const { machine } = useParams({ strict: false }) as { machine?: string };
  const home = useHomeMachine();
  const open = useIsOpen(machine ?? null);
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const search = useRouterState({ select: (s) => s.location.searchStr });
  const plain = plainHomeAddress(pathname, search, home);
  if (plain !== null) return <Navigate {...({ href: plain, replace: true } as ComponentProps<typeof Navigate>)} />;
  if (!machine || !open) return <NotOnThisPage machine={machine ?? ""} />;
  return (
    <MachineView machine={machine} hold>
      <Outlet />
    </MachineView>
  );
}

/** An address of a device this page does not have: removed from the account, or a link from another one. */
function NotOnThisPage({ machine }: { machine: MachineKey }) {
  const { t } = useTranslation();
  return (
    <div data-machine-missing={machine} className="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center">
      <div className="font-mono text-xs text-muted">{t("this device is not on this page")}</div>
      <div className="max-w-sm font-mono text-2xs text-faint">{t("It may have been removed from the account, or the link is from another account.")}</div>
      <Link to="/sessions" className="font-mono text-2xs text-accent hover:underline">
        {t("back to sessions")}
      </Link>
    </div>
  );
}

type LinkProps = ComponentProps<typeof Link>;

/**
 * A `<Link>` to `to` — written as the home's route (`/sessions/$source/$id`)
 * — on `machine` (default: the view's). Everything else is the link's own.
 */
export function MachineLink({
  to,
  params,
  machine,
  ...rest
}: Omit<LinkProps, "to" | "params"> & { to: string; params?: Record<string, string>; machine?: MachineKey }) {
  const viewMachine = useMachine();
  const home = useHomeMachine();
  const target = machineTarget(machine ?? viewMachine, to, params, home);
  return <Link {...(rest as LinkProps)} to={target.to as never} params={target.params as never} />;
}

/** What {@link useMachineNavigate} takes: a navigation written as the home's. */
export interface MachineNavigation {
  to: string;
  /** Another machine than the hook's to go to. */
  machine?: MachineKey;
  params?: Record<string, string>;
  search?: Record<string, unknown>;
  replace?: boolean;
}

/** Navigate on the view's machine (or `machine`): `to` is written as the home's route. */
export function useMachineNavigate(machine?: MachineKey): (go: MachineNavigation) => Promise<void> {
  const viewMachine = useMachine();
  const home = useHomeMachine();
  const navigate = useNavigate();
  const on = machine ?? viewMachine;
  return useCallback(
    ({ machine: other, ...go }: MachineNavigation) => {
      const target = machineTarget(other ?? on, go.to, go.params, home);
      return navigate({ ...go, to: target.to, params: target.params } as never);
    },
    [on, home, navigate],
  );
}
