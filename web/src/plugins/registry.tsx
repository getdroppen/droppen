/**
 * Which plugins this page runs, and what they contribute.
 *
 * The instance decides which plugins exist and which are on (`PluginStatus`,
 * from the feed's `settings` and `plugins` frames, else `GET /api/plugins`);
 * the page loads every status that has an `entry` — the URL the instance
 * serves that exact build on (`docs/plugins/runtime.md` §2) — with a dynamic
 * `import()`, checks the module's `manifest.id` against the status's, and
 * activates it once per (machine, plugin, build) inside a try/catch
 * (`activation.ts`). A plugin that throws is skipped and named in
 * Settings › Plugins; the shell carries on.
 *
 * A plugin installed, upgraded or uninstalled elsewhere cannot be applied to
 * a page that already loaded it — a module cannot be unloaded — so the page
 * keeps running the builds it started with and asks for a reload
 * (`statuses.ts`). Settings › Plugins reloads the page after its own install
 * and uninstall.
 *
 * On droppen.app (`isCloud()`) none of this exists: the provider below is
 * not mounted, so there is no list to read, no module to import, no alias to
 * redirect and no nav entry or page to show.
 */
import type { PluginContributions, PluginDefinition, PluginHost } from "@droppen/plugin-sdk";
import { useQuery } from "@tanstack/react-query";
import { useRouter, useRouterState } from "@tanstack/react-router";
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useFeedSettings } from "@/app/global-feed";
import { isCloud } from "@/lib/hub/config";
import { useApi, useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";
import { activatePlugin, activationKey, type Activation, type PluginSource } from "./activation";
import { pluginAlias } from "./aliases";
import { ConfirmHost } from "./confirm";
import { pluginEvents } from "./events";
import { baselineOf, changedUnderPage, runningStatuses, type PluginBaseline } from "./statuses";
import type { PluginStatus } from "./types";

/** A plugin that is running on this page. */
export interface ActivePlugin {
  id: string;
  /** The status whose build the page loaded (the baseline's, after an upgrade elsewhere). */
  status: PluginStatus;
  definition: PluginDefinition;
  host: PluginHost;
  contributions: PluginContributions;
}

export interface PluginRegistry {
  /** Every plugin installed on this instance; `null` until the server has said. */
  statuses: PluginStatus[] | null;
  /** On, compatible and activated, at the build the page started with. */
  active: ActivePlugin[];
  /** Plugins whose code is still on its way. */
  loading: string[];
  /** Plugins that failed to load or activate, with the reason. */
  errors: Record<string, string>;
  /** The list moved under the page (an install, upgrade or uninstall elsewhere): reload to finish. */
  reloadNeeded: boolean;
  /** Adopt a status the server just answered with (after a switch). */
  adopt(status: PluginStatus): void;
}

const EMPTY: PluginRegistry = { statuses: [], active: [], loading: [], errors: {}, reloadNeeded: false, adopt: () => {} };
const RegistryContext = createContext<PluginRegistry>(EMPTY);

export const usePluginRegistry = () => useContext(RegistryContext);

/**
 * The registry, or — on a page a hub serves — nothing at all. The cloud
 * branch renders no provider state to read and starts no effect, so no
 * status list is fetched, no `plugins` frame is listened for and no entry is
 * imported (§5).
 */
export function PluginRegistryProvider({ children }: { children: ReactNode }) {
  if (isCloud()) return <RegistryContext.Provider value={EMPTY}>{children}</RegistryContext.Provider>;
  return <InstalledPluginRegistry>{children}</InstalledPluginRegistry>;
}

function InstalledPluginRegistry({ children }: { children: ReactNode }) {
  const api = useApi();
  const machine = useMachine();
  const router = useRouter();
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const routerRef = useRef(router);
  routerRef.current = router;
  const feedSettings = useFeedSettings();
  // Not cached across page loads on purpose: a stale "enabled" would fetch a
  // disabled plugin's code, and a disabled plugin leaves no trace.
  const [statuses, setStatuses] = useState<PluginStatus[] | null>(null);
  const [reloadNeeded, setReloadNeeded] = useState(false);
  const [results, setResults] = useState<Record<string, Activation>>({});
  /** What the page first saw; `null` until the first list. */
  const baseline = useRef<PluginBaseline | null>(null);

  const remember = useCallback((next: PluginStatus[]) => {
    if (baseline.current === null) baseline.current = baselineOf(next);
    else if (changedUnderPage(baseline.current, next)) setReloadNeeded(true);
    setStatuses(next);
  }, []);

  // The feed's `settings` frame is the primary source: it arrives with every
  // (re)connect. Until it does — or on a server that does not put `plugins`
  // in it — the API's own list answers (docs/plugins/runtime.md §6).
  useEffect(() => {
    if (feedSettings && Array.isArray(feedSettings.plugins)) remember(feedSettings.plugins);
  }, [feedSettings, remember]);
  useEffect(() => pluginEvents.onStatuses(remember), [remember]);
  const fallback = useQuery({
    queryKey: mq(machine, "plugins"),
    enabled: statuses === null,
    queryFn: async () => (await api.GET("/api/plugins")).data?.plugins ?? [],
    staleTime: 60_000,
  });
  useEffect(() => {
    // The feed's list wins: a stale API answer must not look like a change.
    if (fallback.data && statuses === null) remember(fallback.data);
  }, [fallback.data, statuses, remember]);

  // An old top-level path a plugin keeps (`/tasks`) redirects to its page as
  // soon as the list says a plugin has it. Only a page the server serves has
  // aliases; the cloud registry is not mounted.
  useEffect(() => {
    if (statuses === null) return;
    const id = pluginAlias(statuses, pathname);
    if (id) void router.navigate({ to: "/p/$plugin/$", params: { plugin: id, _splat: "" }, replace: true });
  }, [statuses, pathname, router]);

  const running = useMemo(() => runningStatuses(statuses, baseline.current), [statuses]);
  useEffect(() => {
    let live = true;
    const navigate = (id: string, path: string) =>
      void routerRef.current.navigate({ to: "/p/$plugin/$", params: { plugin: id, _splat: path } });
    for (const status of running) {
      const entry = status.entry;
      if (!entry) continue;
      const source: PluginSource = { id: status.id, build: status.build, entry };
      void activatePlugin(source, navigate, machine).then((result) => {
        if (!live) return;
        const key = activationKey(source);
        setResults((old) => (old[key] === result ? old : { ...old, [key]: result }));
      });
    }
    return () => {
      live = false;
    };
  }, [running, machine]);

  const adopt = useCallback((status: PluginStatus) => {
    setStatuses((old) => (old ? [...old.filter((s) => s.id !== status.id), status] : [status]));
  }, []);

  const value = useMemo<PluginRegistry>(() => {
    const active: ActivePlugin[] = [];
    const loading: string[] = [];
    const errors: Record<string, string> = {};
    for (const status of running) {
      const result = results[activationKey(status)];
      if (!result) loading.push(status.id);
      else if (result.ok)
        active.push({ id: status.id, status, definition: result.definition, host: result.host, contributions: result.contributions });
      else errors[status.id] = result.error;
    }
    return { statuses, active, loading, errors, reloadNeeded, adopt };
  }, [running, statuses, results, reloadNeeded, adopt]);

  return (
    <RegistryContext.Provider value={value}>
      {children}
      <ConfirmHost />
    </RegistryContext.Provider>
  );
}
