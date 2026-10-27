// The local app's boot carries no cloud code (build/local-boot.ts, #169): a
// static import that pulls it back in fails the build, and a shell a hub
// serves starts fetching the cloud app's chunks as it loads.
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { build, type Rollup } from "vite";
import { markShell } from "../build/cloud-mode.js";
import { cloudCode, cloudPreloadScript, HUB_PAGES, hubPagesSet, localBoot } from "../build/local-boot.js";

test("what of cloud mode the local app's boot may not hold, and the few modules it needs", () => {
  for (const id of [
    "/w/web/src/features/cloud/plan.tsx",
    "/w/web/src/features/cloud/hub-shell.ts",
    "/w/web/src/lib/hub/account.ts",
    "/w/web/src/lib/hub/mirror.ts?x",
    "/w/web/src/lib/e2e/index.ts",
    "/w/web/src/lib/e2e/chain.ts",
    "/w/web/src/lib/move/receiver.ts",
    "/w/web/src/features/move/index.ts",
    "/w/web/src/features/hosts/legacy.tsx",
    "/w/web/src/hub-pages.ts",
    "/w/web/src/features/landing/site-app.tsx",
    "/w/web/src/features/landing/front-pages.tsx",
    "/w/web/src/features/landing/security/security.tsx",
    "/w/web/src/features/link/link-popup.tsx",
    "/w/web/src/features/link/add-host.ts",
    "/w/web/src/lib/machines/shells/hub.ts",
    "/w/web/src/lib/transport/p2p.ts",
    "/w/web/src/lib/transport/switching.ts",
    "/w/web/src/api/hub.ts",
  ]) {
    assert.ok(cloudCode(id), id);
  }
  for (const id of [
    "/w/web/src/lib/hub/config.ts",
    "/w/web/src/lib/hub/hosts.ts",
    "/w/web/src/lib/hub/boot-url.ts",
    "/w/web/src/lib/e2e/bytes.ts",
    "/w/web/src/lib/e2e/keys.ts",
    "/w/web/src/lib/move/types.ts",
    "/w/web/src/features/landing/route.ts",
    "/w/web/src/features/landing/get-droppen.ts",
    "/w/web/src/features/link/handoff.ts",
    "/w/web/src/lib/machines/shells/gateway.ts",
    "/w/web/src/lib/transport/direct.ts",
    "/w/web/src/api/client.ts",
    // Shared with the local settings page (push notifications from this device's server).
    "/w/web/src/lib/push.ts",
    "/w/web/src/features/sessions/session-detail.tsx",
    "/w/web/node_modules/react/index.js",
  ]) {
    assert.equal(cloudCode(id), null, id);
  }
});

/** A chunk of a fake bundle: `imports` are its static ones. */
function chunk(fileName: string, opts: { imports?: string[]; css?: string[]; modules?: string[]; entry?: boolean } = {}) {
  return {
    type: "chunk",
    fileName,
    isEntry: opts.entry === true,
    isDynamicEntry: !!opts.modules,
    moduleIds: opts.modules ?? [],
    imports: opts.imports ?? [],
    viteMetadata: { importedCss: new Set(opts.css ?? []), importedAssets: new Set() },
  };
}

test("what a hub's page fetches besides the boot: hub-pages' chunk first, then its static imports, with their CSS, less what the entry loads", () => {
  const bundle = Object.fromEntries(
    [
      chunk("assets/index-1.js", { entry: true, imports: ["assets/vendor-1.js"], css: ["assets/index-1.css"] }),
      chunk("assets/vendor-1.js"),
      chunk("assets/hub-pages-1.js", { modules: [`/w/web${HUB_PAGES}`, "/w/web/src/features/cloud/cloud-app.tsx"], imports: ["assets/vendor-1.js", "assets/ui-1.js", "assets/api-1.js"] }),
      chunk("assets/ui-1.js", { imports: ["assets/vendor-1.js", "assets/api-1.js"], css: ["assets/ui-1.css"] }),
      chunk("assets/api-1.js"),
      chunk("assets/settings-1.js", { modules: ["/w/web/src/features/settings/settings-view.tsx"], imports: ["assets/vendor-1.js"] }),
    ].map((c) => [c.fileName, c]),
  ) as unknown as Rollup.OutputBundle;
  assert.deepEqual(hubPagesSet(bundle), ["/assets/hub-pages-1.js", "/assets/api-1.js", "/assets/ui-1.css", "/assets/ui-1.js"], "its own chunk first");
  delete (bundle as Record<string, unknown>)["assets/hub-pages-1.js"];
  assert.throws(() => hubPagesSet(bundle), /no chunk of its own for \/src\/hub-pages\.ts/);
});

/** Run the preload script in a page whose `<head>` holds `marker` (or none): the links it added. */
function preloadIn(script: string, marker: string | null) {
  const links: Record<string, string>[] = [];
  const document = {
    querySelector: (selector: string) =>
      selector === "meta[name=rh-hub]" && marker !== null ? { getAttribute: (name: string) => (name === "content" ? marker : null) } : null,
    createElement: () => ({}) as Record<string, string>,
    head: { appendChild: (link: Record<string, string>) => void links.push({ ...link }) },
  };
  new Function("document", script)(document);
  return links;
}

test("the shell's preload asks for the cloud app's scripts on a page a hub serves, and for nothing on a local one", () => {
  const script = cloudPreloadScript(["/assets/api-1.js", "/assets/cloud-app-1.js", "/assets/ui-1.css"]);
  assert.doesNotMatch(script, /<\/script/i);
  // Not the marker itself: a shell that says `name="rh-hub"` is one a hub has marked (`markShell`).
  assert.equal(script.includes('name="rh-hub"'), false);
  assert.deepEqual(preloadIn(script, "wss://api.droppen.app"), [
    { rel: "modulepreload", crossOrigin: "", href: "/assets/api-1.js" },
    { rel: "modulepreload", crossOrigin: "", href: "/assets/cloud-app-1.js" },
  ]);
  assert.equal(preloadIn(script, "ws://127.0.0.1:8787").length, 2, "a test hub on this computer");
  assert.deepEqual(preloadIn(script, null), [], "a server's own page");
  // A marker the app would not take as one (`lib/hub/config.ts`): the page runs in local mode.
  for (const odd of ["", "javascript:alert(1)", "https://api.droppen.app", "wss://api.droppen.app/x"]) assert.deepEqual(preloadIn(script, odd), [], odd);
});

/** A small app in a temporary directory, built with the plugin: `files` are its sources under `src/`. */
async function buildApp(files: Record<string, string>) {
  const root = mkdtempSync(join(tmpdir(), "local-boot-"));
  try {
    writeFileSync(join(root, "index.html"), '<!doctype html><html><head><title>t</title></head><body><script type="module" src="/src/main.ts"></script></body></html>');
    for (const [path, text] of Object.entries(files)) {
      mkdirSync(dirname(join(root, "src", path)), { recursive: true });
      writeFileSync(join(root, "src", path), text);
    }
    const out = (await build({ root, configFile: false, logLevel: "silent", plugins: [localBoot()], build: { write: false } })) as Rollup.RollupOutput;
    return out.output;
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

const APP_FILES = {
  "lib/hub/config.ts": 'export const isCloud = () => document.querySelector("meta[name=rh-hub]") !== null;\n',
  "lib/hub/api.ts": "export const api = () => fetch('/v1/me');\n",
  "features/cloud/cloud-app.tsx": 'import { api } from "../../lib/hub/api";\nexport const CloudApp = () => api();\n',
  "app/view.ts": "export const view = () => 'sessions';\n",
  "hub-pages.ts": 'export { CloudApp } from "./features/cloud/cloud-app";\n',
};

test("a build whose entry imports cloud mode only with import() passes, and its shell preloads the cloud app", async () => {
  const output = await buildApp({
    ...APP_FILES,
    "main.ts": 'import { isCloud } from "./lib/hub/config";\nimport { view } from "./app/view";\nif (isCloud()) void import("./hub-pages").then((m) => m.CloudApp());\nelse console.log(view());\n',
  });
  const html = output.find((f) => f.fileName === "index.html");
  assert.ok(html && html.type === "asset");
  const app = output.find((f) => f.type === "chunk" && f.moduleIds.some((id) => id.endsWith(HUB_PAGES)));
  assert.ok(app, "what a hub serves is a chunk of its own");
  assert.match(String(html.source), new RegExp(`<script>\\(function\\(\\)\\{var hub=document\\.querySelector[^<]*"/${app.fileName.replace(/[.]/g, "\\.")}"`));
  // The cloud build can still mark it.
  assert.doesNotThrow(() => markShell(String(html.source), "wss://api.droppen.app"));
});

test("a build whose entry reaches cloud code through a static import fails, naming the chain", async () => {
  await assert.rejects(
    buildApp({
      ...APP_FILES,
      "app/view.ts": 'import { api } from "../lib/hub/api";\nexport const view = () => api();\n',
      "main.ts": 'import { view } from "./app/view";\nvoid import("./hub-pages");\nconsole.log(view());\n',
    }),
    (error: Error) => /cloud mode are in the local app's boot, through the hub client: .*src\/main\.ts → src\/app\/view\.ts → src\/lib\/hub\/api\.ts\./.test(error.message),
  );
});

test("a static import of any module only cloud mode loads fails the build, not only the hub client's", async () => {
  await assert.rejects(
    buildApp({
      ...APP_FILES,
      "features/landing/site-app.tsx": "export const SiteApp = () => 'the public pages';\n",
      "main.ts": 'import { SiteApp } from "./features/landing/site-app";\nvoid import("./hub-pages");\nconsole.log(SiteApp());\n',
    }),
    (error: Error) => /cloud mode are in the local app's boot, through the public pages: .*src\/main\.ts → src\/features\/landing\/site-app\.tsx\./.test(error.message),
  );
});

test("a module only cloud mode loads that the guard does not name fails the build until it is named", async () => {
  await assert.rejects(
    buildApp({
      ...APP_FILES,
      "features/elsewhere/thing.ts": "export const thing = () => 'cloud only';\n",
      "hub-pages.ts": 'export { CloudApp } from "./features/cloud/cloud-app";\nexport { thing } from "./features/elsewhere/thing";\n',
      "main.ts": 'import { view } from "./app/view";\nvoid import("./hub-pages").then((m) => m.thing());\nconsole.log(view());\n',
    }),
    (error: Error) => /1 module\(s\) only a page a hub serves loads are not named in CLOUD_CODE \(build\/local-boot\.ts\): src\/features\/elsewhere\/thing\.ts\./.test(error.message),
  );
});

