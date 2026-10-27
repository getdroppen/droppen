import type { Plugin, Rollup } from "vite";

/**
 * Third-party code in chunks of its own (`output.manualChunks`).
 *
 * Every release rehashes the app chunk; if the libraries lived in it too, each
 * deploy would make every browser fetch them again, which on a weak link is
 * the whole wait. Split out, they keep their hash across releases and stay in
 * the browser's immutable cache.
 *
 * - `vendor-react`: React, ReactDOM and the scheduler, with the CommonJS
 *   helpers they are wrapped with. They import nothing else.
 * - `vendor`: every other package module the entry reaches by static imports,
 *   which is what the app boots with. It imports only `vendor-react`.
 * - `vendor-xterm`: the terminal, loaded with the first terminal view and
 *   never at boot. It imports nothing else either.
 *
 * A package module the entry does not reach statically is used only by code
 * loaded on demand: the autocomplete only Settings shows, math's KaTeX and
 * parsers, pairing's QR codes, compatibility mode's curves. It is left to
 * Rollup, which puts it in or beside the lazy chunks that use it. In `vendor`
 * it would be downloaded with the boot set; this way a new dependency of a
 * lazy view stays out of it without anyone listing it.
 *
 * Lucide (its icons, and the index that imports every one of them) is left to
 * Rollup too, so an icon lands in the app chunk, or in the lazy chunk that
 * alone uses it. In `vendor`, every icon the app starts to use would rehash
 * it; and the index there would take every icon with it, as a manual chunk
 * takes the modules it imports that no other manual chunk has.
 *
 * Deliberately coarse otherwise: finer library groups import each other, and a
 * cycle between chunks runs one before the other's exports exist (a finer
 * split failed to boot). `guardBootChunks` fails the build if the chunks above
 * ever import anything but `vendor-react`.
 */
export function vendorChunk(id: string, meta: Rollup.ManualChunkMeta): string | undefined {
  const pkg = packageOf(id);
  if (pkg?.startsWith("@xterm/")) return "vendor-xterm";
  if (!bootModules(meta).has(id)) return undefined;
  if (id === COMMONJS_HELPERS || (pkg !== null && REACT.has(pkg))) return "vendor-react";
  if (pkg === null || pkg === "lucide-react") return undefined;
  return "vendor";
}

const REACT = new Set(["react", "react-dom", "scheduler"]);

/**
 * `@rollup/plugin-commonjs`'s helpers, which every CommonJS package (React's
 * among them) imports. Left alone, they would go to the manual chunk of the
 * first CommonJS module to run. That is React today; but once a CommonJS
 * package in `vendor` runs before it (the entry importing it first), the
 * helpers would land in `vendor`, and `vendor-react` would import `vendor`,
 * which imports `vendor-react`.
 */
const COMMONJS_HELPERS = "\0commonjsHelpers.js";

/**
 * Every module the entries reach by static imports alone, walked once per
 * build: Rollup asks `manualChunks` about each module with the same `meta`.
 */
const bootSets = new WeakMap<Rollup.ManualChunkMeta, Set<string>>();

function bootModules(meta: Rollup.ManualChunkMeta): Set<string> {
  let boot = bootSets.get(meta);
  if (boot) return boot;
  boot = new Set();
  const pending = [...meta.getModuleIds()].filter((id) => meta.getModuleInfo(id)?.isEntry);
  for (let id = pending.pop(); id !== undefined; id = pending.pop()) {
    if (boot.has(id)) continue;
    boot.add(id);
    pending.push(...(meta.getModuleInfo(id)?.importedIds ?? []));
  }
  bootSets.set(meta, boot);
  return boot;
}

/** The npm package a module id is in (`react-dom`, `@xterm/xterm`), or null for the app's own code. */
function packageOf(id: string): string | null {
  const at = id.lastIndexOf("/node_modules/");
  if (at < 0) return null;
  const [name = "", sub = ""] = id.slice(at + "/node_modules/".length).split("/");
  return name.startsWith("@") ? `${name}/${sub}` : name;
}

/**
 * Fail the build when the boot chunks (the entry and what it imports
 * statically, as the shell preloads and the service worker precaches them)
 * hold code or a stylesheet the app loads on demand (`onDemand`: package
 * names, a trailing `/` for a scope; or the app's own modules by their path
 * from the web root, `src/features/files/files-view.tsx`), or when a vendor
 * chunk imports anything but `vendor-react`. A stylesheet's JS proxy renders
 * to nothing, but its CSS ships with the chunk as a render-blocking `<link>`.
 *
 * `vendorChunk` keeps lazy-only packages out of `vendor` by itself. This is
 * the alarm for one of them becoming eager by a static import in the wrong
 * place — or one of the app's lazy views (`src/app/lazy-views.ts`) — and for
 * the chunk cycle a finer split once failed to boot with.
 */
export function guardBootChunks(onDemand: readonly string[]): Plugin {
  let root = "";
  /** A module of the app's own by its path from the web root, without a query. */
  const appModuleOf = (id: string) => (root && id.startsWith(`${root}/`) ? id.slice(root.length + 1).replace(/[?#].*$/, "") : null);
  const isOnDemand = (id: string) => {
    const name = packageOf(id) ?? appModuleOf(id);
    return name !== null && onDemand.some((p) => (p.endsWith("/") ? name.startsWith(p) : name === p));
  };
  return {
    name: "rh-guard-boot-chunks",
    apply: "build",
    configResolved(config) {
      root = config.root;
    },
    generateBundle(_options, bundle) {
      const chunks = new Map<string, Rollup.OutputChunk>();
      for (const file of Object.values(bundle)) if (file.type === "chunk") chunks.set(file.fileName, file);
      const boot = new Set<string>();
      const visit = (fileName: string) => {
        const chunk = chunks.get(fileName);
        if (!chunk || boot.has(fileName)) return;
        boot.add(fileName);
        for (const imported of chunk.imports) visit(imported);
      };
      for (const chunk of chunks.values()) if (chunk.isEntry) visit(chunk.fileName);
      for (const fileName of boot) {
        const eager = Object.entries(chunks.get(fileName)!.modules)
          .filter(([id, m]) => (m.renderedLength > 0 || /\.css($|\?)/.test(id)) && isOnDemand(id))
          .map(([id]) => id);
        if (eager.length)
          this.error(
            `${fileName} is a boot chunk, but holds code or CSS the app loads on demand: ${eager.slice(0, 3).join(", ")} — import it only from a module loaded with import(), or take it off the on-demand list in vite.config.ts`,
          );
      }
      for (const chunk of chunks.values()) {
        if (!["vendor", "vendor-react", "vendor-xterm"].includes(chunk.name)) continue;
        const allowed = chunk.name === "vendor-react" ? [] : ["vendor-react"];
        const other = chunk.imports.filter((fileName) => !allowed.includes(chunks.get(fileName)?.name ?? ""));
        if (other.length)
          this.error(`${chunk.fileName} imports ${other.join(", ")} — a vendor chunk may import only vendor-react (see vendorChunk in build/vendor-chunks.ts)`);
      }
    },
  };
}
