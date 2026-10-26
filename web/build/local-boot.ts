/**
 * `localBoot()` — keep cloud mode's code out of the local app's boot, and
 * fetch it early on a page a hub serves.
 *
 * One build serves both modes: which one runs is decided in the page, by the
 * hub's marker in its shell (`src/lib/hub/config.ts`). The local app — a
 * server's own page — boots from the entry chunk and the chunks it imports
 * statically. Cloud mode (`features/cloud`, the hub client in `lib/hub`, the
 * end-to-end protocol in `lib/e2e`, the browser key move, the hub origin's
 * move notice) is reached through `src/hub-pages.ts`, which `main.tsx`
 * imports only when a hub serves the page. A static import that pulls any of
 * it back into the boot fails the build, with the chain of imports that does
 * (#169).
 *
 * In cloud mode those chunks would then be a second round trip: the entry has
 * to load and run before it asks for them. So the shell asks for them at once
 * when it carries a hub's marker — a small inline script
 * ({@link cloudPreloadScript}) that adds a `<link rel="modulepreload">` for
 * each. The shell stays the same file in both modes (the marker is the only
 * thing a hub, or the cloud build, adds to it: docs/web-integrity.md), and a
 * local page fetches none of them. The service worker precaches the same set
 * where a hub serves the page (`PRECACHE_CLOUD`, vite.config.ts
 * `precacheBootSet`).
 */
import { relative } from "node:path";
import type { Plugin, Rollup } from "vite";

type OutputBundle = Rollup.OutputBundle;

/** What a page a hub serves runs: the chunk `main.tsx` imports in cloud mode. */
export const HUB_PAGES = "/src/hub-pages.ts";

/**
 * What the local app's boot may not hold, and why. The few modules it needs
 * are named: whether the page is in cloud mode at all, which of a hub's hosts
 * it is on and the move popup's path (`config`, `hosts`, `boot-url`,
 * `lib/move/types`, read by `main.tsx` before anything else), and the hex and
 * SHA-256 of the local half of a computer's sign-in to Droppen
 * (`features/link/handoff.ts`).
 */
const CLOUD_CODE: [pattern: RegExp, what: string][] = [
  [/\/src\/features\/cloud\//, "cloud mode's app"],
  [/\/src\/lib\/hub\/(?!(?:config|hosts|boot-url)\.ts$)/, "the hub client"],
  [/\/src\/lib\/e2e\/(?!(?:bytes|keys)\.ts$)/, "the end-to-end protocol"],
  [/\/src\/(?:features\/move\/|lib\/move\/(?!types\.ts$))/, "the browser key move"],
  [/\/src\/features\/hosts\//, "the hub origin's move notice"],
];

/** What of cloud mode `id` is when the local app's boot may not hold it, else `null`. */
export function cloudCode(id: string): string | null {
  const file = id.split("?")[0]!;
  return CLOUD_CODE.find(([pattern]) => pattern.test(file))?.[1] ?? null;
}

/** The chunks `from` loads before it runs (it, and every chunk it imports statically), with their CSS. */
export function staticSet(bundle: OutputBundle, from: readonly string[]): Set<string> {
  const files = new Set<string>();
  const visit = (fileName: string) => {
    const chunk = bundle[fileName];
    if (!chunk || chunk.type !== "chunk" || files.has(fileName)) return;
    files.add(fileName);
    for (const css of chunk.viteMetadata?.importedCss ?? []) files.add(css);
    for (const imported of chunk.imports) visit(imported);
  };
  for (const fileName of from) visit(fileName);
  return files;
}

/** The entry chunks: the local app's boot starts from them. */
function entries(bundle: OutputBundle): string[] {
  return Object.values(bundle).flatMap((c) => (c.type === "chunk" && c.isEntry ? [c.fileName] : []));
}

/**
 * `hub-pages.ts`'s chunk and what it loads before it runs that the entry has
 * not loaded already (`/assets/…`, sorted): what a page a hub serves fetches
 * besides the local app's boot.
 */
export function hubPagesSet(bundle: OutputBundle): string[] {
  // Its chunk need not have it as its facade: nothing imports it but by `import()`.
  const app = Object.values(bundle).find((c) => c.type === "chunk" && c.moduleIds.some((id) => id.split("?")[0]!.endsWith(HUB_PAGES)));
  if (!app || app.type !== "chunk" || app.isEntry) fail(`no chunk of its own for ${HUB_PAGES}: main.tsx imports it with import()`);
  const boot = staticSet(bundle, entries(bundle));
  return [...staticSet(bundle, [app.fileName])].filter((f) => !boot.has(f)).sort().map((f) => `/${f}`);
}

/**
 * The shell's inline script that, on a page a hub serves (the marker as
 * `lib/hub/config.ts` reads it), starts fetching `files`, the cloud chunks
 * (CSS among them is left to the import, which fetches it too). It never
 * spells the marker out (`name="rh-hub"`): a shell with that in it is one a
 * hub has marked already (`build/cloud-mode.ts` `markShell`).
 */
export function cloudPreloadScript(files: readonly string[]): string {
  const scripts = files.filter((f) => f.endsWith(".js"));
  return (
    `(function(){var hub=document.querySelector("meta[name=rh-hub]");` +
    `if(!hub||!/^wss?:\\/\\/[^/]+$/.test(hub.getAttribute("content")||""))return;` +
    `${JSON.stringify(scripts)}.forEach(function(href){var link=document.createElement("link");link.rel="modulepreload";link.crossOrigin="";link.href=href;document.head.appendChild(link)})})()`
  );
}

function fail(message: string): never {
  throw new Error(`[local-boot] ${message}`);
}

/** The local app's boot (the entry and its static imports) holds no cloud code; the shell preloads cloud mode's. */
export function localBoot(): Plugin {
  let root = process.cwd();
  return {
    name: "droppen-local-boot",
    apply: "build",
    configResolved(config) {
      root = config.root;
    },
    generateBundle(_options, bundle) {
      const boot = staticSet(bundle, entries(bundle));
      const leaked: string[] = [];
      for (const fileName of boot) {
        const chunk = bundle[fileName];
        if (chunk?.type === "chunk") leaked.push(...chunk.moduleIds.filter((id) => cloudCode(id)));
      }
      if (!leaked.length) return;
      // The shortest chain of static imports from an entry to one of them:
      // the import to cut is its last step.
      const cloud = new Set(leaked);
      const from = new Map<string, string | null>();
      const queue: string[] = [];
      for (const id of this.getModuleIds()) {
        if (this.getModuleInfo(id)?.isEntry) {
          from.set(id, null);
          queue.push(id);
        }
      }
      let found = leaked[0]!;
      search: while (queue.length) {
        const id = queue.shift()!;
        for (const next of this.getModuleInfo(id)?.importedIds ?? []) {
          if (from.has(next)) continue;
          from.set(next, id);
          if (cloud.has(next)) {
            found = next;
            break search;
          }
          queue.push(next);
        }
      }
      const chain: string[] = [];
      for (let at: string | null | undefined = found; at; at = from.get(at)) chain.unshift(relative(root, at));
      fail(
        `${leaked.length} module(s) of cloud mode are in the local app's boot, through ${cloudCode(found)}: ${chain.join(" → ")}. ` +
          `Import it with import(), or move what the local app needs out of it (build/local-boot.ts).`,
      );
    },
    transformIndexHtml: {
      order: "post",
      handler(_html, ctx) {
        if (!ctx.bundle) return;
        return [{ tag: "script", children: cloudPreloadScript(hubPagesSet(ctx.bundle)), injectTo: "head" }];
      },
    },
  };
}
