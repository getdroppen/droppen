import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";
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
 * only from `lib/e2e/soft25519.ts`, on a browser without them natively).
 */
const LAZY_PACKAGES = ["jsqr/", "qrcode-generator/", "@noble/"];

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
    // is there to edit and the hashes are of the bytes as served.
    writeBundle(options) {
      fillServiceWorker(options.dir ?? "dist", boot);
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
    precacheBootSet(),
    // `THIRD_PARTY_NOTICES.txt`: the licences of the npm packages in this build.
    thirdPartyNotices(),
  ],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  server: {
    port: Number(process.env.RH_DEV_PORT ?? 5180),
    proxy: {
      "/api": { target: proxyTarget, ws: true },
      "/embed": { target: proxyTarget, ws: true },
    },
  },
});
