import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";
import { cloudBuild, cloudConfig } from "./build/cloud-mode";
import { devTools } from "./build/dev-tools";
import { droppenPlugins, isPluginModule } from "./build/droppen-plugins";
import { fillServiceWorker } from "./build/service-worker";
import { thirdPartyNotices } from "./build/third-party-notices.mjs";

/**
 * Which server `pnpm dev` talks to.
 *
 * The default is the one a developer normally has running. `RH_DEV_PROXY`
 * points it somewhere else — most usefully at a `remote-hand-server replay`
 * rebuilt from a flight log on its own port, which is how a UI change gets
 * tried against real transcripts without going near a live instance.
 */
const proxyTarget = process.env.RH_DEV_PROXY ?? "http://127.0.0.1:8700";

/**
 * The hub the dev server's probe page (`build/dev-tools.ts`) signals through:
 * its `/v1/probe/rooms/…` is proxied there. Unset, the page has no room.
 */
const probeHub = process.env.RH_DEV_HUB;

/**
 * `DROPPEN_WEB_MODE=cloud`: also write the static-hosting output (the shell's
 * `rh-hub` marker, `/.well-known/droppen-hub` and `_headers`) into `dist`,
 * after the normal build (`build/cloud-mode.ts`). `null` leaves the build
 * exactly as it always is — the one a release attests and a server embeds.
 */
const cloud = cloudConfig();

/**
 * Mermaid (`lib/mermaid-render.ts`, fetched by the first ```mermaid fence a
 * message draws) and every package only it imports: d3, the dagre and
 * cytoscape layouts, its parser's chevrotain, DOMPurify and the rest — read
 * off the module graph of the version pinned in package.json. Barrels count
 * too (`d3` re-exports every `d3-*`): a module left to `vendor` takes its
 * static imports in with it. A package listed here that the app itself comes
 * to import leaves `vendor` for the chunk that imports it.
 */
const MERMAID_PACKAGES = [
  "mermaid/",
  "@mermaid-js/",
  "@braintree/sanitize-url/",
  "@chevrotain/",
  "chevrotain/",
  "@iconify/utils/",
  "@upsetjs/venn.js/",
  "cose-base/",
  "layout-base/",
  "cytoscape/",
  "cytoscape-",
  "d3/",
  "d3-",
  "dagre-d3-es/",
  "dayjs/",
  "delaunator/",
  "dompurify/",
  "es-toolkit/",
  "internmap/",
  "khroma/",
  "lodash-es/",
  "marked/",
  "robust-predicates/",
  "roughjs/",
  "stylis/",
  "ts-dedent/",
  "uuid/",
];

/**
 * Third-party code in a chunk of its own.
 *
 * Every release rehashes the app chunk; if the libraries lived in it too, each
 * deploy would make every browser fetch ~1.8 MB again, which on a weak link is
 * the whole wait. Split out, they keep their hash across releases and stay in
 * the browser's immutable cache.
 *
 * Deliberately coarse: finer library groups import each other, and a cycle
 * between chunks runs one before the other's exports exist (a finer split
 * failed to boot). xterm imports nothing else, so it alone gets its own chunk.
 * The QR encoder and decoder (pairing a device, cloud mode only) import
 * nothing either and are loaded on demand, so they stay out of it altogether,
 * as do compatibility mode's curves (`@noble/curves` and its hashes, reached
 * only from `lib/e2e/soft25519.ts`, on a browser without them natively),
 * math: KaTeX and the parsing around it (`components/markdown-math.ts`),
 * fetched by the first message that may hold a formula, and diagrams
 * ({@link MERMAID_PACKAGES}).
 */
const LAZY_PACKAGES = [
  "jsqr/",
  "qrcode-generator/",
  "@noble/",
  "katex/",
  "remark-math/",
  "micromark-extension-math/",
  "mdast-util-math/",
  "hast-util-from-html-isomorphic/",
  "hast-util-from-dom/",
  ...MERMAID_PACKAGES,
];

/**
 * Bundled plugins (`build/droppen-plugins.ts`) live in `node_modules` too, but
 * they are lazy chunks of their own: in `vendor` they would be downloaded by
 * every page, enabled or not. `droppenPlugins()` fails the build if one ever
 * ends up statically reachable from the entry.
 */
function vendorChunk(id: string): string | undefined {
  if (isPluginModule(id)) return undefined;
  const at = id.lastIndexOf("/node_modules/");
  if (at < 0) return undefined;
  const pkg = id.slice(at + "/node_modules/".length);
  if (LAZY_PACKAGES.some((p) => pkg.startsWith(p))) return undefined;
  return pkg.startsWith("@xterm/") ? "vendor-xterm" : "vendor";
}

/**
 * KaTeX's stylesheet offers each of its twenty fonts as woff2, woff and ttf.
 * Every browser the app runs in takes woff2, so the other forty would be
 * files the build emits, the server embeds and the worker hashes, and no page
 * ever fetches. Before `vite:css`, so the fonts it no longer names are never
 * emitted.
 */
function katexWoff2Only(): Plugin {
  return {
    name: "rh-katex-woff2-only",
    apply: "build",
    enforce: "pre",
    transform(code, id) {
      if (!/\/katex\/dist\/katex(?:\.min)?\.css(?:\?|$)/.test(id)) return null;
      return code.replace(/,\s*url\([^)]*\.(?:woff|ttf)\)\s*format\("(?:woff|truetype)"\)/g, "");
    },
  };
}

/**
 * Tell the service worker (`public/sw.js`) which assets this build boots from,
 * and the sha256 of every asset and of the shell (`build/service-worker.ts`).
 *
 * The worker caches hashed assets as they are fetched, which helps every load
 * but the first one after a release: that one still waits on the new app chunk
 * over whatever link the reader is on. Listing the boot set in `sw.js` changes
 * its bytes every release, so the browser installs the new worker when it next
 * checks — on returning to the tab, or when the version banner appears — and
 * the install fetches the new chunks in the background, before the reload that
 * needs them. Only the entry chunk, its static imports and their CSS: lazy
 * chunks are fetched when used, not on every install over a weak link. The
 * hashes are what the worker checks every asset against before it caches or
 * serves it.
 */
function precacheBootSet(): Plugin {
  let boot: string[] = [];
  return {
    name: "rh-precache-boot-set",
    apply: "build",
    generateBundle(_options, bundle) {
      const files = new Set<string>();
      const visit = (fileName: string) => {
        const chunk = bundle[fileName];
        if (!chunk || chunk.type !== "chunk" || files.has(fileName)) return;
        files.add(fileName);
        for (const css of chunk.viteMetadata?.importedCss ?? []) files.add(css);
        for (const imported of chunk.imports) visit(imported);
      };
      for (const chunk of Object.values(bundle)) {
        if (chunk.type === "chunk" && chunk.isEntry) visit(chunk.fileName);
      }
      boot = [...files].sort().map((file) => `/${file}`);
    },
    // After every file is written and the public directory copied, so `sw.js`
    // is there to edit and the hashes are of the bytes as served. In a cloud
    // build the worker is told the marker origin too; the marker itself goes
    // in later (`cloudBuild`), so the shell it hashes is the build's own.
    writeBundle(options) {
      fillServiceWorker(options.dir ?? "dist", boot, { hubOrigin: cloud?.hubOrigin });
    },
  };
}

export default defineConfig({
  build: {
    rollupOptions: {
      output: { manualChunks: vendorChunk },
    },
  },
  plugins: [
    droppenPlugins({ webRoot: fileURLToPath(new URL(".", import.meta.url)) }),
    react(),
    tailwindcss(),
    katexWoff2Only(),
    precacheBootSet(),
    // Static-hosting output, only with `DROPPEN_WEB_MODE=cloud` (a no-op otherwise).
    cloudBuild(cloud),
    // `THIRD_PARTY_NOTICES.txt`: the licences of the npm packages in this build.
    thirdPartyNotices(),
    // Developer pages for `vite dev` only (the direct-path probe): never in a build.
    devTools(fileURLToPath(new URL("./dev", import.meta.url))),
  ],
  resolve: {
    alias: [
      { find: "@", replacement: fileURLToPath(new URL("./src", import.meta.url)) },
      // Mermaid registers its ELK layouts behind a lazy loader that imports
      // elkjs, which is EPL-2.0: not a licence this app ships (the allow-list
      // in `build/third-party-notices.mjs`). The diagrams are laid out with
      // dagre (`lib/mermaid-render.ts`); this stub is what that loader gets,
      // and it refuses, so the block stays its code.
      { find: /^elkjs(?:\/.*)?$/, replacement: fileURLToPath(new URL("./src/lib/elk-stub.ts", import.meta.url)) },
    ],
  },
  server: {
    port: Number(process.env.RH_DEV_PORT ?? 5180),
    proxy: {
      "/api": { target: proxyTarget, ws: true },
      "/embed": { target: proxyTarget, ws: true },
      ...(probeHub ? { "/v1/probe": { target: probeHub, ws: true, changeOrigin: true } } : {}),
    },
  },
});
