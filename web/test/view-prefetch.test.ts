// The views loaded on demand are fetched ahead of use (src/app/view-prefetch.ts):
// the build writes in the files each one is made of (build/view-prefetch.ts).
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { build, type Rollup } from "vite";
import { VIEW_FILES, viewPrefetch } from "../build/view-prefetch.js";
import { fetchViews } from "../src/app/view-prefetch.js";

/** A small app in a temporary directory, built with the plugin: `files` are its sources under `src/`. */
async function buildApp(files: Record<string, string>, views: readonly string[]) {
  // Its real path: Vite takes a root through a symlink (macOS's /var) for one outside the build.
  const root = realpathSync(mkdtempSync(join(tmpdir(), "view-prefetch-")));
  try {
    writeFileSync(join(root, "index.html"), '<!doctype html><html><head><title>t</title></head><body><script type="module" src="/src/main.ts"></script></body></html>');
    for (const [path, text] of Object.entries(files)) {
      mkdirSync(dirname(join(root, "src", path)), { recursive: true });
      writeFileSync(join(root, "src", path), text);
    }
    const out = (await build({ root, configFile: false, logLevel: "silent", plugins: [viewPrefetch(views)], build: { write: false } })) as Rollup.RollupOutput;
    return out.output.filter((file): file is Rollup.OutputChunk => file.type === "chunk");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

const APP_FILES = {
  "main.ts": [
    'import { shell } from "./shell";',
    `const files = typeof ${VIEW_FILES} === "undefined" ? null : ${VIEW_FILES};`,
    "console.log(shell(), files);",
    'document.body.onclick = () => void import("./views/session").then((m) => m.session());',
    'document.body.onkeydown = () => void import("./views/settings").then((m) => m.settings());',
    'document.body.onfocus = () => void import("./views/files").then((m) => m.files());',
  ].join("\n"),
  "shell.ts": "export const shell = () => 'the list';\n",
  "composer.ts": "export const composer = () => 'a composer';\n",
  "views/session.ts": 'import { composer } from "../composer";\nimport { shell } from "../shell";\nexport const session = () => [composer(), shell(), "a session"];\n',
  "views/settings.ts": 'import { composer } from "../composer";\nexport const settings = () => [composer(), "settings"];\n',
  "views/files.ts": "export const files = () => 'files';\n",
};

const chunkOf = (chunks: Rollup.OutputChunk[], module: string) => {
  const chunk = chunks.find((c) => c.moduleIds.some((id) => id.endsWith(`/src/${module}`)));
  assert.ok(chunk, module);
  return `/${chunk.fileName}`;
};

test("the build writes in each view's files in the order given, less the boot and what an earlier view brings", async () => {
  const chunks = await buildApp(APP_FILES, ["src/views/settings.ts", "src/views/session.ts", "src/views/files.ts"]);
  const entry = chunks.find((c) => c.isEntry)!;
  assert.equal(entry.code.includes(VIEW_FILES), false, "the marker is gone");
  const settings = chunkOf(chunks, "views/settings.ts");
  const composer = chunkOf(chunks, "composer.ts");
  assert.notEqual(composer, settings, "shared by two views, the composer is a chunk of its own");
  const expected = [[settings, composer], [chunkOf(chunks, "views/session.ts")], [chunkOf(chunks, "views/files.ts")]];
  assert.ok(entry.code.includes(JSON.stringify(expected)), entry.code);
});

test("a view the build does not have, or one in the boot, fails the build", async () => {
  await assert.rejects(buildApp(APP_FILES, ["src/views/session.ts", "src/views/gone.ts"]), /src\/views\/gone\.ts is in no chunk of this build/);
  await assert.rejects(buildApp(APP_FILES, ["src/shell.ts"]), /src\/shell\.ts is in a boot chunk/);
});

test("a build whose code never names the marker fails", async () => {
  await assert.rejects(
    buildApp({ ...APP_FILES, "main.ts": APP_FILES["main.ts"].replace(/^const files = .*$/m, "const files = null;") }, ["src/views/session.ts"]),
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
