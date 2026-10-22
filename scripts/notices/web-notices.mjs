#!/usr/bin/env node
// Third-party notices for the npm packages the web app bundles: what actually
// reaches `web/dist` (and so the server binary, which embeds it), not
// everything `package.json` lists. Node >= 20, no dependencies of its own.
//
//   node scripts/notices/web-notices.mjs <web-dir> <out-file>
//
// The app is built once more, with its own configuration and the caller's
// environment (so the same distribution), into a scratch directory, with one
// extra plugin that records every module rollup rendered into a chunk, every
// stylesheet, every asset's source file and every `/*! name vX` banner in the
// CSS. Each is mapped to the npm package it came from (the package directory
// after the last `node_modules/`). First-party code — `web/src` and the
// workspace packages — is not listed.
//
// Exits 1 when a package's licence is missing or not on the allow-list
// (AGPL-3.0-compatible permissive licences only), after listing them all.
//
// The recording, the allow-list and the text are web/build/third-party-notices.mjs,
// which also writes THIRD_PARTY_NOTICES.txt into every build of the app.

import { mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { collect, recorder, render } from "../../web/build/third-party-notices.mjs";

async function main() {
  const [webArg, outArg] = process.argv.slice(2);
  if (!webArg || !outArg) {
    console.error("usage: node scripts/notices/web-notices.mjs <web-dir> <out-file>");
    process.exit(2);
  }
  // The real path: Vite resolves modules to real paths, and a root reached
  // through a symlink (macOS's /tmp is /private/tmp) would not contain them.
  const webDir = realpathSync(resolve(webArg));
  const require = createRequire(join(webDir, "package.json"));
  const viteDir = dirname(require.resolve("vite/package.json"));
  const viteManifest = JSON.parse(readFileSync(join(viteDir, "package.json"), "utf8"));
  const entry = viteManifest.exports["."].import;
  const vite = await import(pathToFileURL(join(viteDir, typeof entry === "string" ? entry : entry.default)).href);

  const seen = recorder();
  const record = {
    name: "droppen-third-party-notices-record",
    apply: "build",
    enforce: "post",
    generateBundle(_options, bundle) {
      seen.record(bundle);
    },
  };

  const outDir = mkdtempSync(join(tmpdir(), "droppen-notices-"));
  try {
    await vite.build({
      root: webDir,
      configFile: join(webDir, "vite.config.ts"),
      logLevel: "error",
      build: { outDir, emptyOutDir: true },
      plugins: [record],
    });
  } finally {
    rmSync(outDir, { recursive: true, force: true });
  }

  const found = collect(webDir, seen);
  writeFileSync(
    outArg,
    render(found, ["The web app is embedded in the server binary. Every package below has code", "or styles in it; each licence follows in full as the package ships it."]),
  );
  const { entries, failures, unusual, withoutText, summary } = found;

  console.error(`web notices: ${entries.length} bundled npm packages (${summary})`);
  if (unusual.length) console.error(`  outside MIT/Apache/BSD/ISC: ${unusual.join("; ")}`);
  if (withoutText.length) console.error(`  no licence file shipped: ${withoutText.join("; ")}`);
  if (failures.length) {
    console.error(`web notices: ${failures.length} package(s) fail the licence check:`);
    for (const f of failures) console.error(`  ${f}`);
    process.exit(1);
  }
}

await main();
