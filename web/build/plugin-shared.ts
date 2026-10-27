/**
 * `pluginShared()` — the host's React and plugin SDK, lent to runtime
 * plugins through the shell's import map (`docs/plugins/runtime.md` §3).
 *
 * A plugin is built as a library and leaves `react`, `react/jsx-runtime`,
 * `react-dom` and `@droppen/plugin-sdk` as bare imports. The page resolves
 * them with one import map, written into `index.html` before any module
 * script, and each name points at a facade chunk: a chunk of its own that
 * re-exports the module the host itself loaded. A plugin therefore runs
 * against the host's one React (one hook dispatcher) and one
 * `@droppen/plugin-sdk` (one `PluginHostContext`), not a second copy.
 *
 * The facades are emitted with a strict entry signature: their exports keep
 * the module's own names (`useState`, `jsx`, `definePlugin`, …) instead of
 * Rollup's shortened internal ones, because nothing in the host imports them
 * and Rollup would otherwise shape them for an importer that does not exist.
 * The build fails if an emitted facade has lost one of the names it must
 * carry.
 *
 * React, ReactDOM and the JSX runtime are CommonJS, and `export *` does not
 * see a CommonJS module's named exports — Rollup only knows the `default`
 * interop object. Their facades are therefore written out name by name from
 * the module the build process itself loads, so every name the installed
 * React exposes is there. The SDK is ESM and gets `export *`.
 *
 * The import map is inline, so it is one of the shell's inline scripts: the
 * service worker and the cloud build's `_headers` already allow those by
 * sha256 (`build/service-worker.ts`, `build/cloud-mode.ts`), and it is inert
 * where no plugin loads.
 */
import { createRequire } from "node:module";
import { join } from "node:path";
import type { Plugin, Rollup } from "vite";

/** What a plugin's build leaves bare, and what the import map names. */
export const SHARED_MODULES = ["react", "react/jsx-runtime", "react-dom", "@droppen/plugin-sdk"] as const;
export type SharedModule = (typeof SHARED_MODULES)[number];

const VIRTUAL_PREFIX = "virtual:droppen/plugin-shared/";
const RESOLVED_PREFIX = "\0" + VIRTUAL_PREFIX;

/** The chunk name a facade is emitted under; the hash is Vite's. */
export function facadeName(specifier: SharedModule): string {
  return `plugin-shared-${specifier.replace(/^@/, "").replace(/[\/.]/g, "-")}`;
}

/** The virtual module id a facade is emitted from. */
const virtualId = (specifier: SharedModule) => VIRTUAL_PREFIX + specifier;
const resolvedId = (specifier: SharedModule) => RESOLVED_PREFIX + specifier;

/** The SDK facade's exports must at least include what every plugin gets from it. */
const SDK_EXPORTS = ["definePlugin", "PluginHostContext", "usePluginHost"];

/**
 * The names a CommonJS module exposes to `import { … }`, as the build
 * process's own `require` sees them. Only identifiers can be re-exported.
 */
function cjsExportNames(webRoot: string, specifier: string): string[] {
  const require = createRequire(join(webRoot, "package.json"));
  const mod = require(specifier) as Record<string, unknown>;
  return Object.keys(mod).filter((name) => name !== "default" && /^[A-Za-z_$][\w$]*$/.test(name));
}

/**
 * The facade module's source: the host's module instance, under the names a
 * plugin imports. `default` is the module itself, so `import React from
 * "react"` and `React.useState` work too.
 */
export function facadeSource(webRoot: string, specifier: SharedModule): string {
  if (specifier === "@droppen/plugin-sdk") return `export * from ${JSON.stringify(specifier)};\n`;
  const names = cjsExportNames(webRoot, specifier);
  return (
    `import shared from ${JSON.stringify(specifier)};\n` +
    names.map((name) => `export const ${name} = shared.${name};\n`).join("") +
    "export default shared;\n"
  );
}

/** The emitted facade chunks, by the specifier each one answers. */
export function facadeChunks(bundle: Rollup.OutputBundle): Map<SharedModule, Rollup.OutputChunk> {
  const out = new Map<SharedModule, Rollup.OutputChunk>();
  for (const specifier of SHARED_MODULES) {
    const name = facadeName(specifier);
    const chunk = Object.values(bundle).find((file): file is Rollup.OutputChunk => file.type === "chunk" && file.name === name);
    if (!chunk) throw new Error(`[plugin-shared] no chunk ${name} for "${specifier}" — an import map entry would point at nothing`);
    out.set(specifier, chunk);
  }
  return out;
}

/**
 * The import map as the shell carries it: every shared specifier to the
 * emitted facade's hashed URL. Throws if a facade is missing.
 */
export function importMap(bundle: Rollup.OutputBundle): { imports: Record<string, string> } {
  const imports: Record<string, string> = {};
  for (const [specifier, chunk] of facadeChunks(bundle)) imports[specifier] = `/${chunk.fileName}`;
  return { imports };
}

/** What a facade chunk must export, for the build to trust it. */
function expectedExports(webRoot: string, specifier: SharedModule): string[] {
  return specifier === "@droppen/plugin-sdk" ? SDK_EXPORTS : [...cjsExportNames(webRoot, specifier), "default"];
}

export interface PluginSharedOptions {
  /** `web/`, from which `react` and the SDK resolve. */
  webRoot: string;
}

export function pluginShared(options: PluginSharedOptions): Plugin {
  const webRoot = options.webRoot;
  const entries = new Map<SharedModule, string>();
  return {
    name: "droppen-plugin-shared",
    apply: "build",
    resolveId(source) {
      const specifier = SHARED_MODULES.find((s) => source === virtualId(s));
      return specifier ? resolvedId(specifier) : null;
    },
    load(id) {
      const specifier = SHARED_MODULES.find((s) => id === resolvedId(s));
      return specifier ? facadeSource(webRoot, specifier) : null;
    },
    buildStart() {
      entries.clear();
      for (const specifier of SHARED_MODULES) {
        entries.set(
          specifier,
          this.emitFile({
            type: "chunk",
            id: virtualId(specifier),
            name: facadeName(specifier),
            // Keep the exports a plugin imports under their own names: nothing
            // in the host imports these chunks, so Rollup must not shape them
            // for an importer (docs/plugins/runtime.md §3).
            preserveSignature: "strict",
          }),
        );
      }
    },
    generateBundle(_options, bundle) {
      const chunks = facadeChunks(bundle);
      for (const [specifier, chunk] of chunks) {
        const missing = expectedExports(webRoot, specifier).filter((name) => !chunk.exports.includes(name));
        if (missing.length)
          this.error(
            `[plugin-shared] ${chunk.fileName}: "${specifier}" does not export ${missing.join(", ")} — a plugin importing them would fail to load`,
          );
        // The chunk must be its own file, high up in the bundle, not merged.
        if (!chunk.isEntry) this.error(`[plugin-shared] ${chunk.fileName} is not an entry chunk`);
      }
      for (const [specifier, reference] of entries) {
        if (this.getFileName(reference) !== chunks.get(specifier)!.fileName)
          this.error(`[plugin-shared] "${specifier}" was emitted as ${this.getFileName(reference)} but is in ${chunks.get(specifier)!.fileName}`);
      }
    },
    transformIndexHtml: {
      order: "post",
      handler(_html, ctx) {
        if (!ctx.bundle) return;
        return [
          {
            tag: "script",
            attrs: { type: "importmap" },
            children: JSON.stringify(importMap(ctx.bundle)),
            // Before every module script (and everything else in the head),
            // as an import map must be: https://html.spec.whatwg.org/#import-maps.
            injectTo: "head-prepend",
          },
        ];
      },
    },
  };
}
