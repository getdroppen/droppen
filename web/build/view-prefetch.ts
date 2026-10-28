import type { Plugin, Rollup } from "vite";
import { staticSet } from "./local-boot";

/** What the app's code says in place of the list (`src/app/view-prefetch.ts`). */
export const VIEW_FILES = "__RH_VIEW_FILES__";

/**
 * `viewPrefetch(views)` — write into the app the files each view it loads on
 * demand is made of, for the page to fetch them ahead of use
 * (`src/app/view-prefetch.ts`).
 *
 * Only the build knows which files those are: a chunk's name carries its
 * hash, and a view's chunk imports others it shares with other views. So the
 * code names {@link VIEW_FILES}, and once the chunks have their names the
 * build puts the list in its place: for each of `views` (modules by their
 * path from the web root, in the order the page is to fetch them), the chunk
 * that holds it, the chunks that chunk imports statically and their CSS —
 * less the boot set, which the page has already, and what a view earlier in
 * the list brings. A view with nothing left is left out.
 *
 * A view in no chunk or in a boot chunk fails the build, and so does a build
 * whose code never names the marker: a list nobody checks would fetch views
 * that are gone and miss the ones that are there.
 *
 * Vite fills in each `import()`'s preload list the same way, after the chunks
 * are named. A chunk's name is then the hash of its code before the list went
 * in — and, as it reaches every view by `import()`, of their names too.
 */
export function viewPrefetch(views: readonly string[]): Plugin {
  return {
    name: "rh-view-prefetch",
    apply: "build",
    generateBundle(_options, bundle) {
      const chunks = Object.values(bundle).filter((file): file is Rollup.OutputChunk => file.type === "chunk");
      const boot = staticSet(bundle, chunks.filter((chunk) => chunk.isEntry).map((chunk) => chunk.fileName));
      const listed = new Set(boot);
      const list: string[][] = [];
      for (const view of views) {
        const chunk = chunks.find((c) => c.moduleIds.some((id) => id.split("?")[0]!.endsWith(`/${view}`)));
        if (!chunk) this.error(`${view} is in no chunk of this build — take it off PREFETCH in vite.config.ts`);
        if (boot.has(chunk.fileName)) this.error(`${view} is in a boot chunk (${chunk.fileName}), so there is nothing to fetch ahead — take it off PREFETCH in vite.config.ts`);
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
