import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin, type Rollup } from "vite";
import { cloudBuild, cloudConfig } from "./build/cloud-mode";
import { devTools } from "./build/dev-tools";
import { droppenPlugins } from "./build/droppen-plugins";
import { fillServiceWorker, type LanguageFiles } from "./build/service-worker";
import { thirdPartyNotices } from "./build/third-party-notices.mjs";
import { guardBootChunks, vendorChunk } from "./build/vendor-chunks";

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
 * Packages and views the app loads on demand, which no boot chunk may hold
 * (`guardBootChunks`). Third-party code goes to the vendor chunks only when
 * the entry reaches it statically (`build/vendor-chunks.ts`), so a package
 * used only by lazy code stays out of the boot set without being listed here.
 * The list is the alarm for these becoming eager by an import in the wrong
 * place: the views loaded when first shown (`src/app/lazy-views.ts`) and the
 * markdown they draw messages with, the terminal (xterm, with the first
 * terminal view), the QR encoder and decoder (pairing a device, cloud mode
 * only), compatibility mode's curves (`@noble/curves` and its hashes, reached
 * only from `lib/e2e/soft25519.ts`, on a browser without them natively), math:
 * KaTeX and the parsing around it (`components/markdown-math.ts`), fetched by
 * the first message that may hold a formula, and diagrams: Mermaid
 * (`lib/mermaid-render.ts`), fetched by the first ```mermaid fence a message
 * draws, with the largest packages only it uses: its parser's chevrotain,
 * cytoscape and cytoscape's layouts.
 */
const ON_DEMAND = [
  "src/features/sessions/session-detail.tsx",
  "src/features/sessions/new-session-view.tsx",
  "src/features/chat/chat-home.tsx",
  "src/features/chat/claude-chat-view.tsx",
  "src/features/chat/api-chat-view.tsx",
  "src/features/files/files-view.tsx",
  "src/features/terminals/terminals-view.tsx",
  "src/features/screens/screen-view.tsx",
  "src/features/apps/app-view.tsx",
  "src/features/auth/login-view.tsx",
  "src/features/auth/setup-view.tsx",
  "src/components/markdown.tsx",
  "react-markdown",
  "remark-gfm",
  "@xterm/",
  "jsqr",
  "qrcode-generator",
  "@noble/",
  "katex",
  "remark-math",
  "micromark-extension-math",
  "mdast-util-math",
  "hast-util-from-html-isomorphic",
  "hast-util-from-dom",
  "mermaid",
  "@mermaid-js/",
  "chevrotain",
  "cytoscape",
  "layout-base",
  "cose-base",
];

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
 * chunks are fetched when used, not on every install over a weak link. A
 * page also boots from its language's translations, which are a chunk per
 * language: those the worker precaches only for the languages the browser
 * shows ({@link languageChunks}). The hashes are what the worker checks every
 * asset against before it caches or serves it.
 */
function precacheBootSet(): Plugin {
  let boot: string[] = [];
  let languages: LanguageFiles = {};
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
      languages = languageChunks(bundle, files);
    },
    // After every file is written and the public directory copied, so `sw.js`
    // is there to edit and the hashes are of the bytes as served. In a cloud
    // build the worker is told the marker origin too; the marker itself goes
    // in later (`cloudBuild`), so the shell it hashes is the build's own.
    writeBundle(options) {
      fillServiceWorker(options.dir ?? "dist", boot, { hubOrigin: cloud?.hubOrigin, languages });
    },
  };
}

/**
 * Each language's translations (`src/i18n/lang-*.ts`, a chunk each): its
 * files beyond the boot set, which a page in that language boots from too,
 * and how the chunk's name starts in any build, by which the worker knows an
 * earlier build's chunk of the same language in its cache (`public/sw.js`).
 * A language with no chunk fails the build: the worker would never keep it.
 */
function languageChunks(bundle: Rollup.OutputBundle, boot: ReadonlySet<string>): LanguageFiles {
  const out: LanguageFiles = {};
  for (const chunk of Object.values(bundle)) {
    if (chunk.type !== "chunk" || !chunk.isDynamicEntry) continue;
    const lang = /\/src\/i18n\/lang-([\w-]+)\.ts$/.exec(chunk.facadeModuleId ?? "")?.[1];
    if (!lang) continue;
    const prefix = `assets/lang-${lang}-`;
    if (!chunk.fileName.startsWith(prefix)) throw new Error(`${chunk.fileName}: the ${lang} translations' chunk is not named ${prefix}…`);
    const files: string[] = [];
    const visit = (fileName: string) => {
      const c = bundle[fileName];
      if (!c || c.type !== "chunk" || boot.has(fileName) || files.includes(`/${fileName}`)) return;
      files.push(`/${fileName}`);
      for (const imported of c.imports) visit(imported);
    };
    visit(chunk.fileName);
    out[lang] = { prefix: `/${prefix}`, files };
  }
  const languages = readdirSync(fileURLToPath(new URL("./src/i18n", import.meta.url))).flatMap((file) => /^lang-([\w-]+)\.ts$/.exec(file)?.[1] ?? []);
  const missing = languages.filter((lang) => !out[lang]);
  if (languages.length === 0 || missing.length > 0) throw new Error(`no chunk for the translations of ${missing.join(", ") || "any language"} (src/i18n/lang-*.ts)`);
  return out;
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
    guardBootChunks(ON_DEMAND),
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
