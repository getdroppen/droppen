import type { Plugin, Rollup } from "vite";
import { staticSet } from "./local-boot";

/** What the app's code says in place of the list (`src/app/view-prefetch.ts`). */
export const VIEW_FILES = "__RH_VIEW_FILES__";

/** A module that calls this loads routes' views: every module it loads with `import()` is one. */
const ROUTE_VIEWS = /\blazyRouteComponent\s*\(/;

/** The exceptions to "every view, in the order the code loads them": modules by their path from the web root. */
export interface ViewPrefetchOptions {
  /** Views fetched before the others, in this order. */
  first?: readonly string[];
  /** Views never fetched ahead. */
  skip?: readonly string[];
  /** Modules a view loads on demand rather than a route, fetched after every view. */
  last?: readonly string[];
}

/**
 * `viewPrefetch(options)` — write into the app the files each view it loads on
 * demand is made of, for the page to fetch them ahead of use
 * (`src/app/view-prefetch.ts`).
 *
 * The views are found, not listed: every module that a module calling
 * `lazyRouteComponent` (`src/app/lazy-views.ts`, `src/app/router.tsx`) loads
 * with `import()`, in the order the code loads them. A route added there is
 * fetched ahead without anyone naming it; only the exceptions are named
 * ({@link ViewPrefetchOptions}).
 *
 * Only the build knows which files a view is made of: a chunk's name carries
 * its hash, and a view's chunk imports others it shares with other views. So
 * the code names {@link VIEW_FILES}, and once the chunks have their names the
 * build puts the list in its place: per view, the chunk that holds it, the
 * chunks that chunk imports statically and their CSS — less the boot set,
 * which the page has already, and what a view before it brings. A view with
 * nothing left is left out.
 *
 * An exception that names no view (or for `last`, a module in no chunk or in
 * a boot chunk) fails the build, and so does a build whose code never names
 * the marker or loads no route's view: a list nobody checks rots.
 *
 * Vite fills in each `import()`'s preload list the same way, after the chunks
 * are named. A chunk's name is then the hash of its code before the list went
 * in — and, as it reaches every view by `import()`, of their names too.
 */
export function viewPrefetch(options: ViewPrefetchOptions = {}): Plugin {
  return {
    name: "rh-view-prefetch",
    apply: "build",
    generateBundle(_options, bundle) {
      const chunks = Object.values(bundle).filter((file): file is Rollup.OutputChunk => file.type === "chunk");
      const boot = staticSet(bundle, chunks.filter((chunk) => chunk.isEntry).map((chunk) => chunk.fileName));
      const named = (id: string, name: string) => id.split("?")[0]!.endsWith(`/${name}`);

      const views: string[] = [];
      for (const id of [...this.getModuleIds()].sort()) {
        if (id.startsWith("\0") || id.includes("/node_modules/")) continue;
        const info = this.getModuleInfo(id);
        if (!info?.code || !ROUTE_VIEWS.test(info.code)) continue;
        for (const view of info.dynamicallyImportedIds) if (!views.includes(view)) views.push(view);
      }
      if (views.length === 0) this.error("no module loads a route's view with lazyRouteComponent(() => import(…)): there is nothing to fetch ahead");
      const view = (name: string, option: string) => {
        const id = views.find((v) => named(v, name));
        if (!id) this.error(`${name} (\`${option}\` of PREFETCH in vite.config.ts) is not a view a route loads with lazyRouteComponent — take it off`);
        return id;
      };
      const first = (options.first ?? []).map((name) => view(name, "first"));
      const skip = new Set((options.skip ?? []).map((name) => view(name, "skip")));
      const order = [...first, ...views.filter((id) => !first.includes(id) && !skip.has(id))].map((id) => chunks.find((c) => c.moduleIds.includes(id)));
      for (const name of options.last ?? []) {
        const chunk = chunks.find((c) => c.moduleIds.some((id) => named(id, name)));
        if (!chunk) this.error(`${name} (\`last\` of PREFETCH in vite.config.ts) is in no chunk of this build — take it off`);
        if (boot.has(chunk.fileName)) this.error(`${name} (\`last\` of PREFETCH in vite.config.ts) is in a boot chunk (${chunk.fileName}), so there is nothing to fetch ahead — take it off`);
        order.push(chunk);
      }

      const listed = new Set(boot);
      const list: string[][] = [];
      for (const chunk of order) {
        if (!chunk) continue;
        const files = [...staticSet(bundle, [chunk.fileName])].filter((file) => !listed.has(file));
        for (const file of files) listed.add(file);
        if (files.length > 0) list.push(files.map((file) => `/${file}`));
      }
      const marked = chunks.filter((chunk) => chunk.code.includes(VIEW_FILES));
      if (marked.length === 0) this.error(`no chunk names ${VIEW_FILES} (src/app/view-prefetch.ts): the page would fetch none of the views`);
      for (const chunk of marked) chunk.code = chunk.code.replaceAll(VIEW_FILES, JSON.stringify(list));
    },
  };
}
