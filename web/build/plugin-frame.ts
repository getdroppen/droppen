/**
 * `pluginFrame()` — builds the sandboxed cloud plugin iframe's static page
 * (`plugin-frame.html`) and its self-contained runtime bundle
 * (`assets/plugin-frame-runtime-<hash>.js`, `docs/plugins/cloud.md` §2–§3).
 *
 * The runtime holds React, `react-dom`, `react/jsx-runtime`,
 * `@droppen/plugin-sdk`, the request shims (`fetch` / `XMLHttpRequest`) and
 * the `blob:` module loader in one hashed file under `/assets/` so the service
 * worker's integrity table and the release manifest cover it automatically.
 *
 * `plugin-frame.html` has one tiny inline bootstrap script that waits for its
 * parent to transfer a `MessagePort` and the runtime's bytes, instantiates the
 * runtime as a `blob:` module, and hands it the port.
 */
import { createHash } from "node:crypto";
import { createRequire } from "node:module";
import { join } from "node:path";
import type { Plugin } from "vite";

const RUNTIME_URL_PLACEHOLDER = "__DROPPEN_PLUGIN_FRAME_RUNTIME_URL__";

/** The CSP served with `/plugin-frame.html` (`docs/plugins/cloud.md` §2). */
export const PLUGIN_FRAME_CSP =
  "sandbox allow-scripts; default-src 'none'; script-src 'unsafe-inline' blob:; style-src 'unsafe-inline' blob:; img-src blob: data:; font-src blob: data:; connect-src 'none'; frame-ancestors 'self'";

/**
 * The static `/plugin-frame.html` shipped in every build.
 * Deterministic bytes; receives the runtime source and boot payload over its
 * transferred `MessagePort` so the frame never touches the network itself.
 */
export const PLUGIN_FRAME_HTML = `<!doctype html>
<html lang="en">
<head>
<meta charset="UTF-8" />
<meta name="viewport" content="width=device-width, initial-scale=1.0" />
<title>Droppen Plugin</title>
<script>
(function () {
  var booted = false;
  window.addEventListener("message", function (event) {
    if (booted) return;
    if (event.source !== window.parent) return;
    var data = event.data;
    if (!data || data.type !== "droppen-plugin-init" || !event.ports || !event.ports[0]) return;
    booted = true;
    var port = event.ports[0];
    var runtimeUrl = URL.createObjectURL(new Blob([data.runtimeCode], { type: "text/javascript" }));
    import(runtimeUrl)
      .then(function (mod) {
        URL.revokeObjectURL(runtimeUrl);
        return mod.bootFrame(port, data.boot);
      })
      .catch(function (err) {
        port.postMessage({ type: "error", message: err && err.message ? err.message : String(err) });
      });
  });
})();
</script>
</head>
<body><div id="root"></div></body>
</html>
`;

/** Bundle `src/plugins/frame/runtime-entry.tsx` into one self-contained ESM string with Vite's esbuild. */
async function bundleFrameRuntime(webRoot: string): Promise<string> {
  const webRequire = createRequire(join(webRoot, "package.json"));
  // `esbuild` is a dependency of `vite`, not a direct dependency of `@droppen/web`,
  // so under pnpm's strict node_modules (e.g. `--filter "@droppen/web..."` in CI
  // and release notices jobs) it must be resolved from Vite's package root.
  const viteRequire = createRequire(webRequire.resolve("vite/package.json"));
  const esbuild = viteRequire("esbuild") as {
    build(options: Record<string, unknown>): Promise<{ outputFiles: { text: string }[] }>;
  };
  const result = await esbuild.build({
    entryPoints: [join(webRoot, "src/plugins/frame/runtime-entry.tsx")],
    bundle: true,
    format: "esm",
    platform: "browser",
    target: ["es2022"],
    jsx: "automatic",
    minify: true,
    write: false,
    legalComments: "none",
    define: {
      "process.env.NODE_ENV": '"production"',
    },
  });
  const file = result.outputFiles[0];
  if (!file) throw new Error("[plugin-frame] esbuild produced no runtime output");
  return file.text;
}

export interface PluginFrameOptions {
  webRoot: string;
}

export function pluginFrame(options: PluginFrameOptions): Plugin {
  const webRoot = options.webRoot;
  let runtimeSource = "";
  let runtimeFileName = "assets/plugin-frame-runtime.js";

  return {
    name: "droppen-plugin-frame",
    transform(code, id) {
      const cleanId = id.split("?")[0] ?? id;
      if (!cleanId.endsWith("/frame-runtime-source.ts") || !code.includes(RUNTIME_URL_PLACEHOLDER)) return null;
      return {
        code: code.replaceAll(RUNTIME_URL_PLACEHOLDER, `/${runtimeFileName}`),
        map: null,
      };
    },
    configureServer(server) {
      server.middlewares.use(async (req, res, next) => {
        if (req.url === "/plugin-frame.html") {
          res.statusCode = 200;
          res.setHeader("content-type", "text/html; charset=utf-8");
          res.setHeader("content-security-policy", PLUGIN_FRAME_CSP);
          res.end(PLUGIN_FRAME_HTML);
          return;
        }
        if (req.url === `/${runtimeFileName}`) {
          try {
            const code = await bundleFrameRuntime(webRoot);
            res.statusCode = 200;
            res.setHeader("content-type", "text/javascript; charset=utf-8");
            res.end(code);
          } catch (error) {
            res.statusCode = 500;
            res.end(error instanceof Error ? error.message : String(error));
          }
          return;
        }
        next();
      });
    },
    async buildStart() {
      runtimeSource = await bundleFrameRuntime(webRoot);
      const hash = createHash("sha256").update(runtimeSource).digest("hex").slice(0, 8);
      runtimeFileName = `assets/plugin-frame-runtime-${hash}.js`;
    },
    generateBundle() {
      this.emitFile({
        type: "asset",
        fileName: runtimeFileName,
        source: runtimeSource,
      });
      this.emitFile({
        type: "asset",
        fileName: "plugin-frame.html",
        source: PLUGIN_FRAME_HTML,
      });
    },
  };
}
