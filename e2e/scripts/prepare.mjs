// Build what the harness serves: the web app (in cloud mode when the tree has
// it), the three host fixtures, and a throwaway TLS certificate. Idempotent;
// the Playwright `webServer` runs it through `scripts/serve.mjs`.
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { buildFixtures, DIST, ORIGINS, OUT, REPO_ROOT } from "./build-fixtures.mjs";
import { ensureCertificates } from "./certs.mjs";

function run(command, args, env = process.env) {
  const result = spawnSync(command, args, { cwd: REPO_ROOT, env, stdio: "inherit" });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} ${args.join(" ")} exited ${result.status}`);
}

function pnpm(args, env = process.env) {
  const local = spawnSync("pnpm", ["--version"], { cwd: REPO_ROOT, env, stdio: "ignore" });
  if (local.status === 0) return run("pnpm", args, env);
  return run("npx", ["-y", "pnpm@10.6.3", ...args], env);
}

/** `DROPPEN_WEB_MODE=cloud` support is not on `main` yet: detect it, don't assume it. */
function cloudBuildAvailable() {
  return existsSync(join(REPO_ROOT, "web", "build", "cloud-mode.ts"));
}

function buildWeb() {
  if (process.env.E2E_SKIP_BUILD === "1" && existsSync(join(REPO_ROOT, "web", "dist", "index.html"))) return;
  const cloud = cloudBuildAvailable();
  console.log(`e2e: building the web app (${cloud ? "cloud mode: marker and headers from the build" : "normal + fixture marker and API shim"})`);
  const env = {
    ...process.env,
    ...(cloud
      ? {
          DROPPEN_WEB_MODE: "cloud",
          DROPPEN_WEB_HUB_ORIGIN: ORIGINS.hub,
          DROPPEN_WEB_APP_ORIGIN: ORIGINS.app,
          DROPPEN_WEB_SITE_ORIGIN: ORIGINS.site,
        }
      : {}),
  };
  pnpm(["--filter", "@remote-hand/web", "build"], env);
}

export function prepare() {
  buildWeb();
  ensureCertificates(join(OUT, "certs"));
  buildFixtures();
  console.log(`e2e: fixtures in ${DIST}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  prepare();
}
