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
  return {
    name: "rh-dev-tools",
    apply: "serve",
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const path = (req.url ?? "").split(/[?#]/, 1)[0]!;
        const type = FILES[path];
        if (!type) return next();
        res.setHeader("content-type", type);
        res.setHeader("cache-control", "no-store");
        res.end(readFileSync(join(dir, path.slice(1))));
      });
    },
  };
}
