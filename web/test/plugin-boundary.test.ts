// scripts/check-plugin-boundary.mjs keeps the open SDK and host off the hosted
// service's code. These cases run it against a scratch tree.
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
// @ts-expect-error — a plain .mjs script, without types.
import { checkBoundary } from "../../scripts/check-plugin-boundary.mjs";

function tree(files: Record<string, string>): string {
  const root = mkdtempSync(join(tmpdir(), "droppen-boundary-"));
  for (const [path, text] of Object.entries(files)) {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    writeFileSync(join(root, path), text);
  }
  return root;
}

test("the real tree is clean", () => {
  assert.deepEqual(checkBoundary(resolve(import.meta.dirname, "../..")), []);
});

test("the SDK may import react and its own files only; a relative path may not leave the package", () => {
  const root = tree({
    "packages/plugin-sdk/src/index.ts": 'import { useState } from "react";\nexport * from "./semver";\nimport "../package.json";\n',
    "packages/plugin-sdk/src/semver.ts": "export const x = 1;\n",
    "packages/plugin-sdk/src/escape.ts": 'import { api } from "../../../web/src/api/client";\n',
    "packages/plugin-sdk/src/deep/up.ts": 'import "../../../plugin-sdk/src/semver";\nimport "../../../other/x";\n',
    "packages/plugin-sdk/src/lib.ts": 'import "lodash";\n',
  });
  const problems: string[] = checkBoundary(root);
  assert.equal(problems.length, 3, problems.join("\n"));
  assert.ok(problems.some((p) => p.includes("escape.ts") && p.includes("leaves packages/plugin-sdk")));
  assert.ok(problems.some((p) => p.includes("deep/up.ts") && p.includes("../../../other/x")));
  assert.ok(problems.some((p) => p.includes("lib.ts") && p.includes("only react")));
});

test("the host may not reach the hosted service's code by alias or by relative path", () => {
  const root = tree({
    "web/src/plugins/a.ts": 'import { api } from "@/api/client";\nimport { hub } from "@/lib/hub/client";\n',
    "web/src/plugins/b.ts": 'import { t } from "../lib/transport";\nimport { c } from "../features/cloud/cloud-bar";\nimport { cn } from "../lib/cn";\n',
    // The shell's marker reader is the one hub file the plugin host may use.
    "web/src/plugins/d.ts": 'import { isCloud } from "@/lib/hub/config";\nimport { hubOrigin } from "../lib/hub/config";\n',
    "web/src/app/c.ts": 'const p = import("@droppen/plugin-tasks");\nimport "@droppen/plugin-sdk";\n',
  });
  const problems: string[] = checkBoundary(root);
  assert.deepEqual(
    problems.map((p) => p.split(":")[0] + " " + p.split('"')[1]).sort(),
    [
      "web/src/app/c.ts @droppen/plugin-tasks",
      "web/src/plugins/a.ts @/lib/hub/client",
      "web/src/plugins/b.ts ../features/cloud/cloud-bar",
      "web/src/plugins/b.ts ../lib/transport",
    ],
  );
});

test("require() and export … from are imports too", () => {
  const root = tree({
    "packages/plugin-sdk/src/a.ts": 'const client = require("../../../web/src/api/client");\nexport { x } from "../../../web/src/lib/cn";\nexport * from "./b";\n',
    "packages/plugin-sdk/src/b.ts": 'const p = require.resolve("lodash");\nexport const y = 1;\n',
    "web/src/plugins/c.ts": 'const hub = require("@/lib/hub/client");\nexport * from "../features/landing/route";\n',
  });
  const problems: string[] = checkBoundary(root);
  assert.deepEqual(
    problems.map((p) => p.split(":")[0] + " " + p.split('"')[1]).sort(),
    [
      "packages/plugin-sdk/src/a.ts ../../../web/src/api/client",
      "packages/plugin-sdk/src/a.ts ../../../web/src/lib/cn",
      "packages/plugin-sdk/src/b.ts lodash",
      "web/src/plugins/c.ts ../features/landing/route",
      "web/src/plugins/c.ts @/lib/hub/client",
    ],
  );
});
