// Modules rendered into the diagram worker's own bundle.
//
// Vite builds `new Worker(new URL(…))` in a separate Rollup pass and emits the
// result as an asset of the main build. The notices recorder walks the main
// bundle, so it sees the worker's source file but not the packages inside it
// (Pele and its diagram chunks). This plugin runs inside that worker pass and
// keeps their module ids for `thirdPartyNotices` and `scripts/notices`, which
// add them to the same package/licence collection as the main chunks.
//
// Node >= 20, no dependencies. Vite bundles the config (and this file with
// it), while `scripts/notices/web-notices.mjs` imports it directly; the two
// are different module instances of one Node process. The registry is on
// globalThis so both, and the config's own notices plugin, see one set.

const KEY = Symbol.for("droppen.workerNotices");

function recorded() {
  return (globalThis[KEY] ??= new Set());
}

export function recordWorkerModules() {
  return {
    name: "droppen-worker-notices-record",
    apply: "build",
    generateBundle(_options, bundle) {
      for (const output of Object.values(bundle)) {
        if (output.type !== "chunk") continue;
        for (const [id, info] of Object.entries(output.modules)) {
          // A module Rollup rendered to nothing (a type-only re-export, say)
          // is not in the worker's bytes.
          if (info.renderedLength > 0 || /\.css($|\?)/.test(id)) recorded().add(id);
        }
      }
    },
  };
}

/** The worker bundle's module ids recorded so far. */
export function workerNotices() {
  return recorded();
}
