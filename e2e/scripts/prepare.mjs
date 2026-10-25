// Build what the harness serves: the real web role outputs when the tree has
// them (`DROPPEN_WEB_MODE=cloud` + `DROPPEN_WEB_ROLE`), the three host
// fixtures, and a throwaway TLS certificate. Idempotent; the Playwright
// `webServer` runs it through `scripts/serve.mjs`.
import { spawnSync } from "node:child_process";
import { cpSync, existsSync, readFileSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { BUILDS, buildFixtures, DIST, ORIGINS, OUT, REPO_ROOT } from "./build-fixtures.mjs";
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

const CLOUD_MODE = join(REPO_ROOT, "web", "build", "cloud-mode.ts");

/** `DROPPEN_WEB_MODE=cloud` is on the tree. */
function cloudBuildAvailable() {
  return existsSync(CLOUD_MODE);
}

/** The build has `DROPPEN_WEB_ROLE=app|site` (the per-host static outputs). */
function roleBuildAvailable() {
  return cloudBuildAvailable() && readFileSync(CLOUD_MODE, "utf8").includes("DROPPEN_WEB_ROLE");
}

function buildWeb(role) {
  const cloud = cloudBuildAvailable();
  console.log(`e2e: building the web app (${role ? `cloud mode, role ${role}` : cloud ? "cloud mode (default role)" : "normal + fixture marker and API shim"})`);
  const env = {
    ...process.env,
    ...(cloud
      ? {
          DROPPEN_WEB_MODE: "cloud",
          DROPPEN_WEB_HUB_ORIGIN: ORIGINS.hub,
          DROPPEN_WEB_APP_ORIGIN: ORIGINS.app,
          DROPPEN_WEB_SITE_ORIGIN: ORIGINS.site,
          ...(role ? { DROPPEN_WEB_ROLE: role } : {}),
        }
      : {}),
  };
  pnpm(["--filter", "@remote-hand/web", "build"], env);
}

function copyRoleBuild(name) {
  const dest = join(BUILDS, name);
  rmSync(dest, { recursive: true, force: true });
  cpSync(join(REPO_ROOT, "web", "dist"), dest, { recursive: true });
}

function roleBuildsPresent() {
  return existsSync(join(BUILDS, "app", "index.html")) && existsSync(join(BUILDS, "site", "index.html"));
}

export function prepare() {
  ensureCertificates(join(OUT, "certs"));
  const roles = roleBuildAvailable();
  if (roles) {
    if (process.env.E2E_SKIP_BUILD === "1" && roleBuildsPresent()) {
      console.log("e2e: reusing the app and site builds in .e2e/builds (E2E_SKIP_BUILD=1)");
    } else {
      buildWeb("app");
      copyRoleBuild("app");
      buildWeb("site");
      copyRoleBuild("site");
    }
    buildFixtures({ cloud: true });
  } else {
    if (!(process.env.E2E_SKIP_BUILD === "1" && existsSync(join(REPO_ROOT, "web", "dist", "index.html")))) buildWeb(null);
    buildFixtures({ cloud: false });
  }
  console.log(`e2e: fixtures in ${DIST}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  prepare();
}
