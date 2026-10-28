// The views loaded on demand are fetched ahead of use (src/app/view-prefetch.ts):
// the build writes in the files each one is made of (build/view-prefetch.ts).
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { build, type Rollup } from "vite";
import { VIEW_FILES, viewPrefetch, type ViewPrefetchOptions } from "../build/view-prefetch.js";
import { fetchViews } from "../src/app/view-prefetch.js";

/** A small app in a temporary directory, built with the plugin: `files` are its sources under `src/`. */
async function buildApp(files: Record<string, string>, options: ViewPrefetchOptions = {}) {
  // Its real path: Vite takes a root through a symlink (macOS's /var) for one outside the build.
  const root = realpathSync(mkdtempSync(join(tmpdir(), "view-prefetch-")));
  try {
    writeFileSync(join(root, "index.html"), '<!doctype html><html><head><title>t</title></head><body><script type="module" src="/src/main.ts"></script></body></html>');
    for (const [path, text] of Object.entries(files)) {
      mkdirSync(dirname(join(root, "src", path)), { recursive: true });
      writeFileSync(join(root, "src", path), text);
    }
    const out = (await build({ root, configFile: false, logLevel: "silent", plugins: [viewPrefetch(options)], build: { write: false } })) as Rollup.RollupOutput;
    return out.output.filter((file): file is Rollup.OutputChunk => file.type === "chunk");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

/**
 * Routes whose views load with `lazyRouteComponent` (`routes.ts`), as the
 * app's do, and a view that loads a module of its own with `import()`
 * (the session view's terminal pane), which is not a route's.
 */
const APP_FILES = {
  "main.ts": [
    'import { shell } from "./shell";',
    'import { routes } from "./routes";',
    `const files = typeof ${VIEW_FILES} === "undefined" ? null : ${VIEW_FILES};`,
    "console.log(shell(), routes, files);",
  ].join("\n"),
  "router-lib.ts": "export const lazyRouteComponent = (load: () => Promise<unknown>) => ({ preload: load });\n",
  "routes.ts": [
    'import { lazyRouteComponent } from "./router-lib";',
    'const session = () => import("./views/session");',
    "export const routes = [",
    '  lazyRouteComponent(() => import("./views/settings")),',
    "  lazyRouteComponent(session),",
    '  lazyRouteComponent(() => import("./views/login")),',
    '  lazyRouteComponent(() => import("./views/files")),',
    "];",
  ].join("\n"),
  "shell.ts": "export const shell = () => 'the list';\n",
  "composer.ts": "export const composer = () => 'a composer';\n",
  "views/session.ts": [
    'import { composer } from "../composer";',
    'import { shell } from "../shell";',
    "export const session = () => [composer(), shell(), \"a session\"];",
    'export const terminal = () => import("../terminal/pane");',
  ].join("\n"),
  "views/settings.ts": 'import { composer } from "../composer";\nexport const settings = () => [composer(), "settings"];\n',
  "views/login.ts": "export const login = () => 'login';\n",
  "views/files.ts": "export const files = () => 'files';\n",
  "terminal/pane.ts": "export const pane = () => 'xterm';\n",
};

const chunkOf = (chunks: Rollup.OutputChunk[], module: string) => {
  const chunk = chunks.find((c) => c.moduleIds.some((id) => id.endsWith(`/src/${module}`)));
  assert.ok(chunk, module);
  return `/${chunk.fileName}`;
};

/** The list the build wrote into the entry, in place of the marker. */
function written(chunks: Rollup.OutputChunk[], expected: string[][]) {
  const entry = chunks.find((c) => c.isEntry)!;
  assert.equal(entry.code.includes(VIEW_FILES), false, "the marker is gone");
  assert.ok(entry.code.includes(JSON.stringify(expected)), entry.code);
}

test("every view a route loads is found and written in, in the order the code loads them, less the boot and what a view before it brings", async () => {
  const chunks = await buildApp(APP_FILES);
  const settings = chunkOf(chunks, "views/settings.ts");
  const composer = chunkOf(chunks, "composer.ts");
  assert.notEqual(composer, settings, "shared by two views, the composer is a chunk of its own");
  // Not the terminal pane: a view loads it, not a route.
  written(chunks, [[chunkOf(chunks, "views/session.ts"), composer], [settings], [chunkOf(chunks, "views/login.ts")], [chunkOf(chunks, "views/files.ts")]]);
});

test("only the exceptions are named: views first, views skipped, and modules a view loads, last", async () => {
  const chunks = await buildApp(APP_FILES, { first: ["src/views/files.ts"], skip: ["src/views/login.ts"], last: ["src/terminal/pane.ts"] });
  written(chunks, [
    [chunkOf(chunks, "views/files.ts")],
    [chunkOf(chunks, "views/session.ts"), chunkOf(chunks, "composer.ts")],
    [chunkOf(chunks, "views/settings.ts")],
    [chunkOf(chunks, "terminal/pane.ts")],
  ]);
});

test("an exception that names no view, or a module with nothing to fetch, fails the build", async () => {
  await assert.rejects(buildApp(APP_FILES, { first: ["src/terminal/pane.ts"] }), /src\/terminal\/pane\.ts \(`first` of PREFETCH in vite\.config\.ts\) is not a view a route loads/);
  await assert.rejects(buildApp(APP_FILES, { skip: ["src/views/gone.ts"] }), /src\/views\/gone\.ts \(`skip` of PREFETCH in vite\.config\.ts\) is not a view a route loads/);
  await assert.rejects(buildApp(APP_FILES, { last: ["src/views/gone.ts"] }), /src\/views\/gone\.ts \(`last` of PREFETCH in vite\.config\.ts\) is in no chunk of this build/);
  await assert.rejects(buildApp(APP_FILES, { last: ["src/shell.ts"] }), /src\/shell\.ts \(`last` of PREFETCH in vite\.config\.ts\) is in a boot chunk/);
});

test("a build that loads no route's view, or whose code never names the marker, fails", async () => {
  // `import()` alone is not a route's view: nothing calls lazyRouteComponent.
  await assert.rejects(
    buildApp({ ...APP_FILES, "routes.ts": 'export const routes = [() => import("./views/session"), () => import("./views/files")];\n' }),
    /no module loads a route's view with lazyRouteComponent/,
  );
  await assert.rejects(
    buildApp({ ...APP_FILES, "main.ts": APP_FILES["main.ts"].replace(/^const files = .*$/m, "const files = null;") }),
    new RegExp(`no chunk names ${VIEW_FILES}`),
  );
});

/** `fetchViews` with fetches that come when told to, and a record of what it did. */
function harness(fetchedAlready: readonly string[] = [], failing: readonly string[] = []) {
  const events: string[] = [];
  const pending = new Map<string, () => void>();
  const io = {
    idle: async () => void events.push("idle"),
    fetched: (path: string) => fetchedAlready.includes(path),
    fetchFile: (path: string) =>
      new Promise<boolean>((resolve) => {
        events.push(`fetch ${path}`);
        pending.set(path, () => resolve(!failing.includes(path)));
      }),
  };
  /** Let the run take every step it can without a fetch coming. */
  const flush = async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  };
  /** The fetches in flight come, and the run goes on. */
  const finish = async () => {
    for (const [path, done] of pending) {
      pending.delete(path);
      done();
    }
    await flush();
  };
  return { io, events, pending, flush, finish };
}

test("a view's files are fetched together, after idle, and the next view only once they have come", async () => {
  const { io, events, pending, flush, finish } = harness(["/assets/shell.js"]);
  const run = fetchViews([["/assets/new-session.js", "/assets/shell.js", "/assets/composer.js"], ["/assets/session.js"]], io);
  await flush();
  assert.deepEqual(events, ["idle", "fetch /assets/new-session.js", "fetch /assets/composer.js"], "both at once, and not what the page has already");
  assert.equal(pending.size, 2);
  await finish();
  assert.deepEqual(events.slice(3), ["idle", "fetch /assets/session.js"]);
  await finish();
  await run;
});

test("a view whose fetch fails ends the run: the rest load when they are opened", async () => {
  const { io, events, flush, finish } = harness([], ["/assets/session.js"]);
  const run = fetchViews([["/assets/new-session.js"], ["/assets/session.js"], ["/assets/settings.js"]], io);
  await flush();
  for (let i = 0; i < 3; i++) await finish();
  await run;
  assert.deepEqual(events, ["idle", "fetch /assets/new-session.js", "idle", "fetch /assets/session.js"]);
});
