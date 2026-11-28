/**
 * Which plugins this page runs, and what they contribute.
 *
 * Locally, the instance decides which plugins exist and which are on
 * (`PluginStatus`, from the feed's `settings` and `plugins` frames, else
 * `GET /api/plugins`); the page loads every status that has an `entry`
 * (`docs/plugins/runtime.md` §2) with a dynamic `import()`, checks the
 * module's `manifest.id` against the status's, and activates it once per
 * (machine, plugin, build) inside a try/catch (`activation.ts`).
 *
 * On droppen.app (`isCloud()`), no plugin code ever runs in the parent origin.
 * Instead, `useMachinePlugins(machine)` tracks each online machine's
 * cloud-enabled plugins (`enabled && compatible && cloud`) and activates them
 * inside hidden sandboxed `/plugin-frame.html` iframes (`cloud-frame.tsx`) to
 * discover their nav, route, pane, and settings contributions.
 */
import type { PluginContributions, PluginDefinition, PluginHost } from "@droppen/plugin-sdk";
import { useQuery } from "@tanstack/react-query";
import { useRouter, useRouterState } from "@tanstack/react-router";
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useFeedSettings, useMachineFeedSelector, type GlobalFeedState } from "@/app/global-feed";
import { isCloud } from "@/lib/hub/config";
import { useApi, useInfoOf, useMachine } from "@/lib/machines/context";
import type { MachineKey } from "@/lib/machines/key";
import { mq } from "@/lib/machines/query";
import { apiFor, isOpen } from "@/lib/machines/registry";
import { activatePlugin, activationKey, type Activation, type PluginSource } from "./activation";
import { pluginAlias } from "./aliases";
import { activateCloudPlugin, type CloudActivation } from "./cloud-frame";
import { ConfirmHost } from "./confirm";
import { pluginEventsFor } from "./events";
import type { SerializedContributions } from "./frame/protocol";
import { baselineOf, changedUnderPage, cloudRunningStatuses, runningStatuses, type PluginBaseline } from "./statuses";
import type { PluginStatus } from "./types";

/** A plugin that is running on a local page. */
export interface ActivePlugin {
  id: string;
  /** The status whose build the page loaded (the baseline's, after an upgrade elsewhere). */
  status: PluginStatus;
  definition: PluginDefinition;
  host: PluginHost;
  contributions: PluginContributions;
}

/** A cloud-enabled plugin activated inside a sandboxed frame for a machine. */
export interface CloudActivePlugin {
  id: string;
  status: PluginStatus;
  contributions: SerializedContributions;
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

export interface CloudPluginRegistry {
  statuses: PluginStatus[] | null;
  active: CloudActivePlugin[];
  loading: string[];
  errors: Record<string, string>;
}

const EMPTY: PluginRegistry = { statuses: [], active: [], loading: [], errors: {}, reloadNeeded: false, adopt: () => {} };
const RegistryContext = createContext<PluginRegistry>(EMPTY);

export const usePluginRegistry = () => useContext(RegistryContext);

/**
 * The local plugin registry, or — on a page a hub serves — the shared
 * `<ConfirmHost />` so sandboxed cloud plugin frames can open confirmation
 * dialogs through the parent shell.
 */
export function PluginRegistryProvider({ children }: { children: ReactNode }) {
  if (isCloud()) {
    return (
      <RegistryContext.Provider value={EMPTY}>
        {children}
        <ConfirmHost />
      </RegistryContext.Provider>
    );
  }
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
  useEffect(() => pluginEventsFor(machine).onStatuses(remember), [machine, remember]);
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

const selectFeedSettings = (state: GlobalFeedState) => state.settings;

/**
 * Cloud-enabled plugins on `machine`, for `droppen.app` (`docs/plugins/cloud.md` §6).
 *
 * Subscribes to `machine`'s feed (`settings.plugins` + `plugins` frames), falls
 * back to `GET /api/plugins` while the machine is online, and activates every
 * cloud-enabled plugin once per `(machine, id, build)` inside a hidden
 * sandboxed iframe. In local mode returns an empty snapshot (use
 * {@link usePluginRegistry} there).
 */
export function useMachinePlugins(machine: MachineKey): CloudPluginRegistry {
  const cloud = isCloud();
  const info = useInfoOf(machine);
  const live = cloud && isOpen(machine) && info.state === "online" && info.live === "live";
  const feedSettings = useMachineFeedSelector(machine, selectFeedSettings);
  const feedPlugins =
    cloud && feedSettings && !feedSettings.fromCopy && Array.isArray(feedSettings.plugins)
      ? feedSettings.plugins
      : null;
  const [statuses, setStatuses] = useState<PluginStatus[] | null>(feedPlugins);
  const [results, setResults] = useState<Record<string, CloudActivation>>({});

  // Reset per-machine local state when switching the watched machine key.
  useEffect(() => {
    setStatuses(feedPlugins);
    setResults({});
    // Only reset on machine switch; live updates are handled by the effects below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [machine]);

  // Primary source: live `settings` frame (ignore offline copy frames).
  useEffect(() => {
    if (feedPlugins !== null) {
      setStatuses(feedPlugins);
    }
  }, [feedPlugins]);

  // Live `plugins` frames from this machine's feed.
  useEffect(() => {
    if (!cloud || !isOpen(machine)) return;
    return pluginEventsFor(machine).onStatuses((next) => setStatuses(next));
  }, [cloud, machine]);

  // Fallback `GET /api/plugins` once the live channel is up and the feed hasn't delivered plugins yet.
  const fallback = useQuery({
    queryKey: mq(machine, "cloud-plugins"),
    enabled: live && statuses === null,
    queryFn: async () => {
      const res = await apiFor(machine).GET("/api/plugins");
      if (res.error || !res.data) {
        throw new Error("failed to load plugins");
      }
      return res.data.plugins ?? [];
    },
    staleTime: 60_000,
  });
  useEffect(() => {
    if (cloud && fallback.data && statuses === null) setStatuses(fallback.data);
  }, [cloud, fallback.data, statuses]);

  const running = useMemo(
    () => (live ? cloudRunningStatuses(statuses) : []),
    [live, statuses],
  );

  useEffect(() => {
    if (!live) return;
    let mounted = true;
    for (const status of running) {
      void activateCloudPlugin(machine, status).then((result) => {
        if (!mounted) return;
        const key = activationKey(status);
        setResults((old) => (old[key] === result ? old : { ...old, [key]: result }));
      });
    }
    return () => {
      mounted = false;
    };
  }, [live, running, machine]);

  return useMemo<CloudPluginRegistry>(() => {
    if (!cloud) return { statuses: [], active: [], loading: [], errors: {} };
    const active: CloudActivePlugin[] = [];
    const loading: string[] = [];
    const errors: Record<string, string> = {};
    for (const status of running) {
      const result = results[activationKey(status)];
      if (!result) loading.push(status.id);
      else if (result.ok) active.push({ id: status.id, status, contributions: result.contributions });
      else errors[status.id] = result.error;
    }
    return { statuses, active, loading, errors };
  }, [cloud, running, statuses, results]);
}
