/**
 * Loading and activating a runtime-installed plugin: once per machine,
 * plugin and build, for the life of the page, whatever remounts.
 *
 * A plugin's host is bound to a machine — its storage reads and writes that
 * machine's documents (`httpStorage(id, machine)`). The machine is the home
 * (`lib/machines/registry.ts`), and on droppen.app the home moves when the
 * person switches machines; an activation kept by plugin alone would go on
 * reading and writing the first machine's documents after the switch. So an
 * activation is kept per (machine, plugin, build), and the page's other
 * machine gets its own host when it is the home. A machine closed on the page
 * takes its activations with it (`perMachine`).
 *
 * The build is in the key because the entry URL names one exact build
 * (`/plugins/<key>/<id>/<build>/…`): a reinstall with other bytes is other
 * code, and must never be answered with the module already in memory.
 */
import type { PluginContributions, PluginDefinition, PluginHost } from "@droppen/plugin-sdk";
import i18n from "i18next";
import type { MachineKey } from "@/lib/machines/key";
import { perMachine } from "@/lib/machines/registry";
import { createPluginHost, pluginNamespace } from "./host";
import type { PluginStatus } from "./types";

export type Activation = { ok: true; definition: PluginDefinition; host: PluginHost; contributions: PluginContributions } | { ok: false; error: string };

/** What a plugin's entry module must at least be: a `definePlugin(...)` default export. */
export interface PluginModule {
  default?: PluginDefinition;
}

/** How an entry URL becomes a module; the dynamic import, or a test's stand-in. */
export type PluginLoader = (entry: string) => Promise<PluginModule>;

/**
 * The entry URL as the page will import it, or a refusal. Only this page's
 * own server serves plugin code, under `/plugins/` (`docs/plugins/runtime.md`
 * §2), so an entry anywhere else — another origin, a `data:` or `blob:` URL,
 * another path of this server — is not imported, whatever the status said.
 */
export function pluginEntryUrl(entry: string, origin: string | undefined = globalThis.location?.origin): string {
  const url = origin ? new URL(entry, origin) : null;
  if (!url || url.origin !== origin || !url.pathname.startsWith("/plugins/"))
    throw new Error(`refusing to load plugin code from ${entry}: not this server's /plugins/`);
  return url.href;
}

/** The browser's way: the instance serves the code, the import map lends it React and the SDK. */
export const importPluginEntry: PluginLoader = (entry) => import(/* @vite-ignore */ pluginEntryUrl(entry));

/** The statuses' code parts: which plugin, at which build, from where. */
export type PluginSource = Pick<PluginStatus, "id" | "build"> & { entry: string };

/** The activation's key: one per plugin and build (the machine is the map). */
export function activationKey(source: Pick<PluginStatus, "id" | "build">): string {
  return `${source.id}@${source.build}`;
}

/** Each machine's activations, by plugin id and build; dropped when the machine is closed. */
const activationsOf = perMachine(() => new Map<string, Promise<Activation>>());

/** The activation of `source` on `machine`: loaded, checked and activated on first ask, then the same one. */
export function activatePlugin(
  source: PluginSource,
  navigate: (id: string, path: string) => void,
  machine: MachineKey,
  load: PluginLoader = importPluginEntry,
): Promise<Activation> {
  const activations = activationsOf(machine);
  const key = activationKey(source);
  let pending = activations.get(key);
  if (!pending) {
    pending = (async (): Promise<Activation> => {
      try {
        const mod = await load(source.entry);
        const definition = mod.default;
        if (!definition || typeof definition.activate !== "function") throw new Error("the entry module has no default definePlugin(...) export");
        if (definition.manifest?.id !== source.id)
          throw new Error(`the package says it is "${definition.manifest?.id}", the instance installed it as "${source.id}"`);
        const host = createPluginHost(source.id, { navigate: (path) => navigate(source.id, path), machine });
        const contributions = definition.activate(host) ?? {};
        for (const [locale, strings] of Object.entries(contributions.i18n ?? {})) {
          if (strings) i18n.addResourceBundle(locale, pluginNamespace(source.id), strings, true, true);
        }
        return { ok: true, definition, host, contributions };
      } catch (error) {
        // A chunk that failed to download may succeed on the next attempt.
        activations.delete(key);
        const message = error instanceof Error ? error.message : String(error);
        console.error(`plugin ${source.id} failed to start:`, error);
        return { ok: false, error: message };
      }
    })();
    activations.set(key, pending);
  }
  return pending;
}
