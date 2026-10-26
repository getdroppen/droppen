/**
 * Which plugins this page runs, and what they contribute.
 *
 * The build decides which plugins exist (`virtual:droppen/bundled-plugins`,
 * generated from the distribution manifest — none in the public build); the
 * instance decides which of those are on (`PluginStatus`, from the feed's
 * `settings` frame and `plugins` frames, or `GET /api/plugins`). Only a plugin
 * that is bundled, enabled and compatible is loaded: its entry chunk is fetched,
 * its `manifest.id` checked against the build's, and `activate(host)` called
 * once per page and machine inside a try/catch (`activation.ts`) — a plugin that throws is skipped and named
 * in Settings › Plugins, and the shell carries on.
 */
import type { PluginContributions, PluginDefinition, PluginHost } from "@droppen/plugin-sdk";
import { useQuery } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { bundledPlugins, type BundledPlugin } from "virtual:droppen/bundled-plugins";
import { useFeedSettings } from "@/app/global-feed";
import { ConfirmHost } from "./confirm";
import { pluginEvents } from "./events";
import { type Activation, activatePlugin } from "./activation";
import type { PluginStatus } from "./types";
import { useApi, useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";

/** A plugin that is running on this page. */
export interface ActivePlugin {
  id: string;
  bundled: BundledPlugin;
  status: PluginStatus;
  definition: PluginDefinition;
  host: PluginHost;
  contributions: PluginContributions;
}

export interface PluginRegistry {
  /** What the build ships. */
  bundled: BundledPlugin[];
  /** This instance's view of them; `null` until the server has said. */
  statuses: PluginStatus[] | null;
  /** Bundled, enabled, compatible and activated. */
  active: ActivePlugin[];
  /** Enabled plugins whose code is still on its way. */
  loading: string[];
  /** Plugins that failed to load or activate, with the reason. */
  errors: Record<string, string>;
  /** Adopt a status the server just answered with (after a switch). */
  adopt(status: PluginStatus): void;
}

const EMPTY: PluginRegistry = { bundled: [], statuses: [], active: [], loading: [], errors: {}, adopt: () => {} };
const RegistryContext = createContext<PluginRegistry>(EMPTY);

export const usePluginRegistry = () => useContext(RegistryContext);

export function PluginRegistryProvider({ children }: { children: ReactNode }) {
  const api = useApi();
  const machine = useMachine();
  const router = useRouter();
  const routerRef = useRef(router);
  routerRef.current = router;
  const feedSettings = useFeedSettings() as { plugins?: PluginStatus[] } | null;
  // Not cached across page loads on purpose: a stale "enabled" would fetch a
  // disabled plugin's code, and a disabled plugin leaves no trace.
  const [statuses, setStatuses] = useState<PluginStatus[] | null>(bundledPlugins.length ? null : []);
  const [results, setResults] = useState<Record<string, Activation>>({});

  const remember = useCallback((next: PluginStatus[]) => setStatuses(next), []);

  // The feed's `settings` frame is the primary source: it arrives with every
  // (re)connect, on droppen.app too, and from a machine's mirror when offline.
  useEffect(() => {
    if (feedSettings && Array.isArray(feedSettings.plugins)) remember(feedSettings.plugins);
    else if (feedSettings && !bundledPlugins.length) remember([]);
  }, [feedSettings, remember]);
  useEffect(() => pluginEvents.onStatuses(remember), [remember]);
  // A server too old to put `plugins` in its settings frame still answers this.
  const fallback = useQuery({
    queryKey: mq(machine, "plugins"),
    enabled: bundledPlugins.length > 0 && !!feedSettings && !Array.isArray(feedSettings.plugins),
    queryFn: async () => (await api.GET("/api/plugins")).data?.plugins ?? [],
    staleTime: 60_000,
  });
  useEffect(() => {
    if (fallback.data) remember(fallback.data as PluginStatus[]);
  }, [fallback.data, remember]);

  const wanted = useMemo(
    () => bundledPlugins.filter((b) => statuses?.some((s) => s.id === b.id && s.enabled && s.compatible)),
    [statuses],
  );
  useEffect(() => {
    let live = true;
    const navigate = (id: string, path: string) =>
      void routerRef.current.navigate({ to: "/p/$plugin/$", params: { plugin: id, _splat: path } });
    for (const bundled of wanted) {
      void activatePlugin(bundled, navigate, machine).then((result) => {
        if (live) setResults((old) => (old[bundled.id] === result ? old : { ...old, [bundled.id]: result }));
      });
    }
    return () => {
      live = false;
    };
  }, [wanted, machine]);

  const adopt = useCallback(
    (status: PluginStatus) => remember([...(statuses ?? []).filter((s) => s.id !== status.id), status]),
    [statuses, remember],
  );

  const value = useMemo<PluginRegistry>(() => {
    const active: ActivePlugin[] = [];
    const loading: string[] = [];
    const errors: Record<string, string> = {};
    for (const bundled of wanted) {
      const result = results[bundled.id];
      const status = statuses?.find((s) => s.id === bundled.id);
      if (!result || !status) loading.push(bundled.id);
      else if (result.ok) active.push({ id: bundled.id, bundled, status, ...result });
      else errors[bundled.id] = result.error;
    }
    return { bundled: bundledPlugins, statuses, active, loading, errors, adopt };
  }, [wanted, results, statuses, adopt]);

  return (
    <RegistryContext.Provider value={value}>
      {children}
      <ConfirmHost />
    </RegistryContext.Provider>
  );
}
