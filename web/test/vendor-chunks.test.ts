// Which chunk third-party code goes to (build/vendor-chunks.ts): `vendor` holds
// only what the entry reaches statically, React has `vendor-react` to itself
// (with the CommonJS helpers), icons stay with the app, and a lazy-only package
// is left to the lazy chunks.
// Small fixture apps built against the web app's own node_modules.
import assert from "node:assert/strict";
import { mkdtempSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { build, type Rollup } from "vite";
import { guardBootChunks, vendorChunk } from "../build/vendor-chunks.js";

const webRoot = resolve(import.meta.dirname, "..");

async function bundle(
  files: Record<string, string>,
  opts: { onDemand?: string[]; manualChunks?: Rollup.GetManualChunk } = {},
): Promise<Rollup.OutputChunk[]> {
  const root = mkdtempSync(join(tmpdir(), "droppen-vendor-"));
  symlinkSync(join(webRoot, "node_modules"), join(root, "node_modules"));
  for (const [name, text] of Object.entries(files)) writeFileSync(join(root, name), text);
  const result = await build({
    root,
    configFile: false,
    logLevel: "silent",
    plugins: [guardBootChunks(opts.onDemand ?? [])],
    build: {
      write: false,
      rollupOptions: { input: join(root, "entry.js"), output: { manualChunks: opts.manualChunks ?? vendorChunk } },
    },
  });
  return (result as Rollup.RollupOutput).output.filter((o): o is Rollup.OutputChunk => o.type === "chunk");
}

/** The names of the chunks with code of `pkg` in them. */
function holding(chunks: Rollup.OutputChunk[], pkg: string): string[] {
  const has = (c: Rollup.OutputChunk) => Object.entries(c.modules).some(([id, m]) => m.renderedLength > 0 && id.includes(`/node_modules/${pkg}/`));
  return chunks.filter(has).map((c) => c.name);
}

/** The entry chunk and every chunk it imports statically: what the shell preloads. */
function bootNames(chunks: Rollup.OutputChunk[]): string[] {
  const byFile = new Map(chunks.map((c) => [c.fileName, c]));
  const seen = new Set<Rollup.OutputChunk>();
  const visit = (c: Rollup.OutputChunk | undefined) => {
    if (!c || seen.has(c)) return;
    seen.add(c);
    for (const f of c.imports) visit(byFile.get(f));
  };
  for (const c of chunks) if (c.isEntry) visit(c);
  return [...seen].map((c) => c.name).sort();
}

const chunk = (chunks: Rollup.OutputChunk[], name: string) => chunks.find((c) => c.name === name)!;
const namesOf = (chunks: Rollup.OutputChunk[], files: string[]) => files.map((f) => chunks.find((c) => c.fileName === f)!.name).sort();

const app = (icons: string[]) => ({
  "entry.js": [
    'import { createRoot } from "react-dom/client";',
    'import { jsx } from "react/jsx-runtime";',
    'import { clsx } from "clsx";',
    'import { useVirtualizer } from "@tanstack/react-virtual";',
    `import { ${icons.join(", ")} } from "lucide-react";`,
    `createRoot(document.body).render([${icons.map((i) => `jsx(${i}, { className: clsx("a") })`).join(", ")}]);`,
    // An app build keeps no exports of its entry: only what it runs is bundled.
    'console.log(useVirtualizer, () => import("./settings.js"));',
    "",
  ].join("\n"),
  // Lazy only: a QR decoder and an icon of its own.
  "settings.js": 'import jsQR from "jsqr";\nimport { X } from "lucide-react";\nexport default [jsQR, X];\n',
});

test("vendor holds what the entry reaches statically, React its own chunk, and lazy-only code stays lazy", async () => {
  const chunks = await bundle(app(["Check"]));
  assert.deepEqual(bootNames(chunks), ["entry", "vendor", "vendor-react"]);
  for (const pkg of ["react", "react-dom", "scheduler"]) assert.deepEqual(holding(chunks, pkg), ["vendor-react"], pkg);
  assert.deepEqual(chunk(chunks, "vendor-react").imports, [], "vendor-react imports no other chunk");
  assert.deepEqual(holding(chunks, "clsx"), ["vendor"]);
  assert.deepEqual(holding(chunks, "@tanstack/react-virtual"), ["vendor"]);
  assert.deepEqual(namesOf(chunks, chunk(chunks, "vendor").imports), ["vendor-react"]);
  assert.deepEqual(holding(chunks, "jsqr"), ["settings"], "a package only lazy code uses is not in vendor");
  // Icons and their renderer stay with the code that uses them.
  assert.deepEqual(holding(chunks, "lucide-react"), ["entry", "settings"]);
});

test("the CommonJS helpers go with React, even when another CommonJS package runs first", async () => {
  // jsQR is CommonJS too, and the entry imports it before React: left to
  // Rollup, the helpers would follow it into `vendor`.
  const files = { "entry.js": 'import jsQR from "jsqr";\nimport { createRoot } from "react-dom/client";\nconsole.log(jsQR, createRoot);\n' };
  const chunks = await bundle(files);
  const helpers = chunks.filter((c) => (c.modules["\0commonjsHelpers.js"]?.renderedLength ?? 0) > 0).map((c) => c.name);
  assert.deepEqual(helpers, ["vendor-react"]);
  assert.deepEqual(holding(chunks, "jsqr"), ["vendor"]);
  assert.deepEqual(chunk(chunks, "vendor-react").imports, []);
  assert.deepEqual(namesOf(chunks, chunk(chunks, "vendor").imports), ["vendor-react"]);
});

test("a new icon leaves the vendor chunks' hashes alone", async () => {
  const before = await bundle(app(["Check"]));
  const after = await bundle(app(["Check", "Plus"]));
  for (const name of ["vendor", "vendor-react"]) assert.equal(chunk(after, name).fileName, chunk(before, name).fileName, name);
  assert.notEqual(chunk(after, "entry").fileName, chunk(before, "entry").fileName);
});

test("a boot chunk with code of an on-demand package fails the build", async () => {
  const files = { "entry.js": 'import jsQR from "jsqr";\nconsole.log(jsQR);\n' };
  await assert.rejects(bundle(files, { onDemand: ["jsqr"] }), /is a boot chunk, but holds code or CSS the app loads on demand: .*\/node_modules\/jsqr\//);
  // By the whole package name, or a whole scope.
  await bundle(files, { onDemand: ["jsq", "@jsqr/"] });
});

test("so does one with an on-demand package's stylesheet, which renders no JS", async () => {
  const eager = { "entry.js": 'import "katex/dist/katex.min.css";\nimport { clsx } from "clsx";\nconsole.log(clsx);\n' };
  await assert.rejects(bundle(eager, { onDemand: ["katex"] }), /is a boot chunk, but holds code or CSS the app loads on demand: .*\/node_modules\/katex\/dist\/katex\.min\.css/);
  // Imported by a lazy module, it ships with that module's chunk.
  const lazy = { "entry.js": 'console.log(() => import("./math.js"));\n', "math.js": 'import "katex/dist/katex.min.css";\nexport default 1;\n' };
  await bundle(lazy, { onDemand: ["katex"] });
});

test("a vendor chunk importing anything but vendor-react fails the build", async () => {
  // Lucide imports React: with React in `vendor`, `vendor-react` would import it.
  const manualChunks = (id: string) => (id.includes("/node_modules/lucide-react/") ? "vendor-react" : id.includes("/node_modules/react/") ? "vendor" : undefined);
  const files = { "entry.js": 'import { Check } from "lucide-react";\nconsole.log(Check);\n' };
  await assert.rejects(bundle(files, { manualChunks }), /vendor-react-.*\.js imports .*vendor-.*\.js — a vendor chunk may import only vendor-react/);
});
