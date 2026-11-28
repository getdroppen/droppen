// Relative ESM import rewriting (`web/src/plugins/frame/rewrite-imports.ts`):
// rewrites static, dynamic, and re-export relative specifiers to `@droppen-pkg/<path>`
// so modules loaded from `blob:` URLs inside `/plugin-frame.html` can resolve
// sibling chunks and cycles via a runtime import map.
import assert from "node:assert/strict";
import test from "node:test";
import { resolvePackagePath, rewriteRelativeImports } from "../src/plugins/frame/rewrite-imports.js";

test("resolvePackagePath resolves ./ and ../ cleanly against the importer's package path", () => {
  assert.equal(resolvePackagePath("dist/index.js", "./chunk.js"), "dist/chunk.js");
  assert.equal(resolvePackagePath("dist/ui/view.js", "../lib/util.js"), "dist/lib/util.js");
  assert.equal(resolvePackagePath("index.js", "./a/b/../c.js"), "a/c.js");
  assert.equal(resolvePackagePath("dist/index.js", "./x.js?v=1#hash"), "dist/x.js");
});

test("rewrites static imports, side-effect imports, re-exports, and dynamic imports", () => {
  const src = [
    'import React from "react";',
    'import { a, b as c } from "./sub/helper.js";',
    'import "./styles.js";',
    'export * from "../shared.js";',
    'export { default as Foo } from "./foo.js";',
    'const lazy = () => import("./lazy/chunk.js");',
  ].join("\n");

  const out = rewriteRelativeImports(src, "dist/pages/main.js");
  assert.ok(out.includes('import React from "react";'), "bare specifiers stay untouched");
  assert.ok(out.includes('from "@droppen-pkg/dist/pages/sub/helper.js"'));
  assert.ok(out.includes('import "@droppen-pkg/dist/pages/styles.js"'));
  assert.ok(out.includes('from "@droppen-pkg/dist/shared.js"'));
  assert.ok(out.includes('from "@droppen-pkg/dist/pages/foo.js"'));
  assert.ok(out.includes('import("@droppen-pkg/dist/pages/lazy/chunk.js")'));
});

test("ignores import-like text inside strings, template literals, and comments", () => {
  const src = [
    '// import foo from "./comment.js";',
    '/* export * from "./block.js"; */',
    'const s = "import x from \\"./str.js\\"";',
    'const t = `import("./tpl.js")`;',
    'import real from "./real.js";',
  ].join("\n");

  const out = rewriteRelativeImports(src, "dist/index.js");
  assert.ok(out.includes('./comment.js'), "line comment untouched");
  assert.ok(out.includes('./block.js'), "block comment untouched");
  assert.ok(out.includes('./str.js'), "string literal untouched");
  assert.ok(out.includes('./tpl.js'), "template literal untouched");
  assert.ok(out.includes('"@droppen-pkg/dist/real.js"'), "real import rewritten");
});

test("handles cyclic imports without losing specifiers", () => {
  const a = 'import { b } from "./b.js"; export const a = () => b();';
  const b = 'import { a } from "./a.js"; export const b = () => a();';
  assert.equal(rewriteRelativeImports(a, "dist/a.js"), 'import { b } from "@droppen-pkg/dist/b.js"; export const a = () => b();');
  assert.equal(rewriteRelativeImports(b, "dist/b.js"), 'import { a } from "@droppen-pkg/dist/a.js"; export const b = () => a();');
});
