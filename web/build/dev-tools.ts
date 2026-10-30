import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { Plugin } from "vite";

/**
 * Developer tools that `vite dev` serves and no build ever contains: the
 * direct-path measurement page (`web/dev/p2p-probe.html` and its script), the
 * browser end of `rh-p2p-probe` (server-rs/crates/rh-devkit). It used to sit
 * in `public/` and so shipped in every build, droppen.app's included: a page
 * of the app's origin that is not the app. Here it is reached at
 * `<dev server>/p2p-probe.html#<room>`, its signaling room proxied to the hub
 * named by `RH_DEV_HUB` (vite.config.ts).
 */
const FILES: Record<string, string> = {
  "/p2p-probe.html": "text/html; charset=utf-8",
  "/p2p-probe.js": "text/javascript; charset=utf-8",
};

export function devTools(dir: string): Plugin {
  // `pnpm dev:web:landing` serves the public pages alone; this dev-only marker
  // points them at the production hub's config, since no local hub is running.
  // `apply: "serve"` keeps both the marker and the well-known route out of
  // every build.
  const landingMode = process.env.DROPPEN_DEV_LANDING === "1" || process.env.DROPPEN_WEB_ROLE === "site";
  return {
    name: "rh-dev-tools",
    apply: "serve",
    transformIndexHtml(html) {
      if (landingMode && !html.includes('name="rh-hub"')) {
        return html.replace("<head>", '<head>\n    <meta name="rh-hub" content="wss://api.droppen.app" />');
      }
      return html;
    },
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const path = (req.url ?? "").split(/[?#]/, 1)[0]!;
        if (landingMode && path === "/.well-known/droppen-hub") {
          res.setHeader("content-type", "application/json");
          res.setHeader("cache-control", "no-store");
          res.end(JSON.stringify({ origin: "wss://api.droppen.app", app: null, site: null, apex_mode: "app" }));
          return;
        }
        const type = FILES[path];
        if (!type) return next();
        res.setHeader("content-type", type);
        res.setHeader("cache-control", "no-store");
        res.end(readFileSync(join(dir, path.slice(1))));
      });
    },
  };
}
