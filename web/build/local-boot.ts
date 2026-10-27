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
 * What the local app's boot may not hold, and why: every module only a page a
 * hub serves loads, by name. Named rather than worked out from the build,
 * because a build cannot tell a module that leaked into the boot from one
 * the two modes share: once a static import pulls it in, the local app
 * reaches it like any other. The build checks the names are complete instead
 * ({@link unnamedCloudCode}): a module only cloud mode loads that none of
 * them covers fails it. The local app's lazy pages may share a module of
 * these (the gateway shell reads the hub client's copies); its boot may not.
 *
 * The few the boot needs are left out by name: whether the page is in cloud
 * mode at all, which of a hub's hosts it is on, the public pages' paths and
 * the move popup's (`lib/hub/config`, `hosts`, `boot-url`,
 * `features/landing/route`, `get-droppen`, `lib/move/types`, read by
 * `main.tsx` before anything else), the hex and SHA-256 of the local half of a
 * computer's sign-in to Droppen (`features/link/handoff.ts`), and the field
 * arithmetic its key check uses (`lib/e2e/field.ts`, which imports nothing).
 */
const CLOUD_CODE: [pattern: RegExp, what: string][] = [
  [/\/src\/hub-pages\.ts$/, "what a hub serves"],
  [/\/src\/(?:features\/cloud\/|lib\/machines\/shells\/hub\.ts$|features\/machines\/device-facts\.ts$|lib\/page-resume\.ts$|components\/prefs-controls\.tsx$)/, "cloud mode's app"],
  [/\/src\/(?:lib\/hub\/(?!(?:config|hosts|boot-url)\.ts$)|api\/hub\.ts$)/, "the hub client"],
  [/\/src\/(?:lib\/e2e\/(?!(?:bytes|keys|field)\.ts$)|lib\/transport\/p2p\.ts$)/, "the end-to-end protocol"],
  [/\/src\/features\/link\/(?:link-popup\.tsx|add-host\.ts)$/, "droppen.app's half of a computer's sign-in"],
  [/\/src\/features\/landing\/(?!(?:route|get-droppen)\.ts$)/, "the public pages"],
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
 * `hub-pages.ts`'s chunk, then what it loads before it runs that the entry
 * has not loaded already (`/assets/…`, sorted): what a page a hub serves
 * fetches besides the local app's boot. Its own chunk first: no local page
 * loads it, so the worker tells a hub's page by it (`public/sw.js`
 * `servedByHub`).
 */
export function hubPagesSet(bundle: OutputBundle): string[] {
  const boot = staticSet(bundle, entries(bundle));
  const hub = hubPagesChunk(bundle);
  const rest = [...staticSet(bundle, [hub])].filter((f) => f !== hub && !boot.has(f)).sort();
  return [hub, ...rest].map((f) => `/${f}`);
}

/** The chunk `hub-pages.ts` is in (not necessarily as its facade: nothing imports it but by `import()`). */
function hubPagesChunk(bundle: OutputBundle): string {
  const chunk = Object.values(bundle).find((c) => c.type === "chunk" && c.moduleIds.some((id) => id.split("?")[0]!.endsWith(HUB_PAGES)));
  if (!chunk || chunk.type !== "chunk" || chunk.isEntry) fail(`no chunk of its own for ${HUB_PAGES}: main.tsx imports it with import()`);
  return chunk.fileName;
}

/** Every chunk `from` can load, now or later (static imports and `import()`), short of `skip`. */
function reachable(bundle: OutputBundle, from: readonly string[], skip?: string): Set<string> {
  const files = new Set<string>();
  const visit = (fileName: string) => {
    const chunk = bundle[fileName];
    if (!chunk || chunk.type !== "chunk" || files.has(fileName) || fileName === skip) return;
    files.add(fileName);
    for (const imported of [...chunk.imports, ...chunk.dynamicImports]) visit(imported);
  };
  for (const fileName of from) visit(fileName);
  return files;
}

/**
 * This app's modules that only a page a hub serves ever loads — in a chunk
 * `hub-pages.ts` reaches that the local app cannot reach without it — and
 * {@link CLOUD_CODE} does not name. The boot is checked against the names, so
 * a module missing from them could slip into it unseen.
 */
export function unnamedCloudCode(bundle: OutputBundle): string[] {
  const hub = hubPagesChunk(bundle);
  const local = reachable(bundle, entries(bundle), hub);
  const out = new Set<string>();
  for (const fileName of reachable(bundle, [hub])) {
    const chunk = bundle[fileName];
    if (local.has(fileName) || chunk?.type !== "chunk") continue;
    for (const id of chunk.moduleIds) if (/\/src\//.test(id) && !id.includes("/node_modules/") && !id.startsWith("\0") && !cloudCode(id)) out.add(id);
  }
  return [...out].sort();
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

/**
 * The shortest chain of static imports from an entry to one of `targets`:
 * the import to cut is its last step.
 */
function shortestChain(ctx: Pick<Rollup.PluginContext, "getModuleIds" | "getModuleInfo">, targets: Set<string>): string[] {
  const from = new Map<string, string | null>();
  const queue: string[] = [];
  for (const id of ctx.getModuleIds()) {
    if (ctx.getModuleInfo(id)?.isEntry) {
      from.set(id, null);
      queue.push(id);
    }
  }
  let found: string | null = null;
  search: while (queue.length) {
    const id = queue.shift()!;
    for (const next of ctx.getModuleInfo(id)?.importedIds ?? []) {
      if (from.has(next)) continue;
      from.set(next, id);
      if (targets.has(next)) {
        found = next;
        break search;
      }
      queue.push(next);
    }
  }
  const chain: string[] = [];
  for (let at = found ?? [...targets][0]!; at; at = from.get(at) ?? "") chain.unshift(at);
  return chain;
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
      const leaked: string[] = [];
      for (const fileName of staticSet(bundle, entries(bundle))) {
        const chunk = bundle[fileName];
        if (chunk?.type === "chunk") leaked.push(...chunk.moduleIds.filter((id) => cloudCode(id)));
      }
      if (leaked.length) {
        const chain = shortestChain(this, new Set(leaked));
        fail(
          `${leaked.length} module(s) of cloud mode are in the local app's boot, through ${cloudCode(chain.at(-1)!)}: ${chain.map((id) => relative(root, id)).join(" → ")}. ` +
            `Import it with import(), or move what the local app needs out of it (build/local-boot.ts).`,
        );
      }
      const unnamed = unnamedCloudCode(bundle);
      if (unnamed.length)
        fail(
          `${unnamed.length} module(s) only a page a hub serves loads are not named in CLOUD_CODE (build/local-boot.ts): ` +
            `${unnamed.map((id) => relative(root, id)).join(", ")}. Name them there, so that the local app's boot is checked for them.`,
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
