#!/usr/bin/env node
// The manifest of a public web build (docs/web-integrity.md). Node >= 20, no
// dependencies.
//
//   node scripts/web-manifest.mjs web/dist [--commit SHA] [--ref REF] [--out FILE]
//       write the manifest: every file's path, size and sha256, the digest
//       that names the build, and what it was built from
//   node scripts/web-manifest.mjs web/dist --check web-manifest.json
//       compare a build with a manifest; exit 1 and list what differs
//
// The digest is the sha256 of one `<sha256>  <path>\n` line per file, sorted
// by path bytewise — what `sha256sum` prints — so it can be recomputed with
// nothing but a shell:
//
//   cd web/dist && find . -type f | sed 's|^\./||' | LC_ALL=C sort | xargs sha256sum | sha256sum
//
// The hub reports the same digest at `/.well-known/droppen-build`
// (docs/hub-api/http.md), which is how a verifier picks the manifest to compare
// droppen.app against.
//
// The build's service worker (`sw.js`) carries the sha256 of every file under
// `assets/` and of `index.html`, which it checks the browser's cache against
// (web/build/service-worker.ts). No manifest is written for a build whose table
// disagrees with its files, and `--check` reports it.

import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync, realpathSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

export const FORMAT = "droppen-web-manifest/1";
const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..");

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const bytewise = (a, b) => Buffer.compare(Buffer.from(a.path), Buffer.from(b.path));

/** Every file under `dir`, sorted by path: `{path, size, sha256}`. */
export function filesOf(dir) {
  const out = [];
  const walk = (at) => {
    for (const entry of readdirSync(at, { withFileTypes: true })) {
      const path = join(at, entry.name);
      if (entry.isDirectory()) walk(path);
      else if (entry.isFile()) {
        const body = readFileSync(path);
        out.push({ path: relative(dir, path).split(sep).join("/"), size: body.length, sha256: sha256(body) });
      } else throw new Error(`${path}: neither a file nor a directory`);
    }
  };
  walk(dir);
  return out.sort(bytewise);
}

/** The digest that names a build (see the top of this file). */
export function digestOf(files) {
  const lines = [...files].sort(bytewise).map((f) => `${f.sha256}  ${f.path}\n`);
  return `sha256:${sha256(lines.join(""))}`;
}

/**
 * The service worker's integrity table: `{assets: {"/assets/…": sha256},
 * shell, scripts}` as the build wrote it into `sw.js` — `scripts`, the CSP
 * sources of the shell's inline scripts it serves the offline shell's policy
 * with — or `null` when the text has none.
 */
export function workerTable(text) {
  const assets = /^const INTEGRITY = (\{.*\});$/m.exec(text);
  const shell = /^const SHELL_SHA256 = "([0-9a-f]*)";$/m.exec(text);
  const scripts = /^const SHELL_SCRIPTS = (\[.*\]);$/m.exec(text);
  if (!assets || !shell || !scripts) return null;
  let table, sources;
  try {
    table = JSON.parse(assets[1]);
    sources = JSON.parse(scripts[1]);
  } catch {
    return null;
  }
  if (!table || typeof table !== "object" || Array.isArray(table) || Object.values(table).some((v) => typeof v !== "string")) return null;
  if (!Array.isArray(sources) || sources.some((v) => typeof v !== "string")) return null;
  return { assets: table, shell: shell[1], scripts: sources };
}

/** The CSP sources of `html`'s inline scripts, found as a hub finds them (web/build/service-worker.ts). */
export function inlineScriptSources(html) {
  const out = [];
  let rest = html.toString("utf8");
  for (;;) {
    const start = rest.indexOf("<script");
    if (start < 0) break;
    const openEnd = rest.indexOf(">", start);
    if (openEnd < 0) break;
    const close = rest.indexOf("</script>", openEnd + 1);
    if (close < 0) break;
    if (!rest.slice(start, openEnd).includes("src=")) out.push(`'sha256-${createHash("sha256").update(rest.slice(openEnd + 1, close)).digest("base64")}'`);
    rest = rest.slice(close + "</script>".length);
  }
  return out;
}

/**
 * What the service worker's table (`text`, the build's `sw.js`) gets wrong
 * about `files`: every file under `assets/` must be listed with its sha256,
 * nothing else, the shell must be `index.html`'s, and (given the shell's
 * bytes, `shellHtml`) its inline scripts' sources the shell's. `[]` when it is
 * right.
 */
export function workerProblems(files, text, shellHtml) {
  const table = workerTable(text);
  if (!table) return ["sw.js has no integrity table (`const INTEGRITY = {…};`, `const SHELL_SHA256 = \"…\";` and `const SHELL_SCRIPTS = […];`, written by the build)"];
  const out = [];
  const assets = new Map(files.filter((f) => f.path.startsWith("assets/")).map((f) => [`/${f.path}`, f.sha256]));
  for (const [path, sha] of assets) {
    if (!Object.hasOwn(table.assets, path)) out.push(`sw.js does not list ${path}`);
    else if (table.assets[path] !== sha) out.push(`sw.js lists ${path} as ${table.assets[path]}, the file is ${sha}`);
  }
  for (const path of Object.keys(table.assets)) if (!assets.has(path)) out.push(`sw.js lists ${path}, which is not in the build`);
  const shell = files.find((f) => f.path === "index.html")?.sha256 ?? null;
  if (table.shell !== shell) out.push(`sw.js has the shell as ${table.shell || "(none)"}, index.html is ${shell ?? "missing"}`);
  if (shellHtml !== undefined) {
    const want = inlineScriptSources(shellHtml);
    if (JSON.stringify(table.scripts) !== JSON.stringify(want)) out.push(`sw.js allows the shell's inline scripts ${table.scripts.join(" ") || "(none)"}, index.html has ${want.join(" ") || "none"}`);
  }
  return out;
}

/** {@link workerProblems} of the build in `dist` (none without a `sw.js`). */
function distWorkerProblems(dist, files) {
  if (!files.some((f) => f.path === "sw.js")) return [];
  return workerProblems(files, readFileSync(join(dist, "sw.js"), "utf8"), readFileSync(join(dist, "index.html")));
}

function inputHash(name) {
  const file = join(repo, name);
  return existsSync(file) ? sha256(readFileSync(file)) : null;
}

/** The manifest of the build in `dist`. */
export function manifestOf(dist, { commit = null, ref = null } = {}) {
  const files = filesOf(dist);
  const worker = distWorkerProblems(dist, files);
  if (worker.length) throw new Error(`${dist}: the service worker's integrity table does not match the build:\n  ${worker.join("\n  ")}`);
  const rootPackage = JSON.parse(readFileSync(join(repo, "package.json"), "utf8"));
  return {
    format: FORMAT,
    repository: process.env.GITHUB_REPOSITORY || null,
    commit,
    ref,
    build: {
      command: 'pnpm install --frozen-lockfile --filter "@remote-hand/web..." && pnpm --filter @remote-hand/web build',
      node: process.version,
      packageManager: rootPackage.packageManager ?? null,
      source_date_epoch: process.env.SOURCE_DATE_EPOCH ?? null,
      inputs: Object.fromEntries([".nvmrc", "package.json", "pnpm-lock.yaml"].map((n) => [n, inputHash(n)])),
    },
    digest: digestOf(files),
    files,
  };
}

/** What differs between a build and a manifest: `[]` when nothing does. */
export function compare(files, manifest) {
  const have = new Map(files.map((f) => [f.path, f]));
  const want = new Map(manifest.files.map((f) => [f.path, f]));
  const out = [];
  for (const [path, f] of want) {
    const got = have.get(path);
    if (!got) out.push(`missing   ${path}`);
    else if (got.sha256 !== f.sha256) out.push(`differs   ${path}  (manifest ${f.sha256}, build ${got.sha256})`);
  }
  for (const path of have.keys()) if (!want.has(path)) out.push(`extra     ${path}`);
  return out;
}

function main(argv) {
  const usage = "usage: node scripts/web-manifest.mjs <dist> [--commit SHA] [--ref REF] [--out FILE] | <dist> --check <manifest.json>";
  const opts = {};
  const rest = [];
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (["--commit", "--ref", "--out", "--check"].includes(a)) {
      if (argv[i + 1] === undefined) throw new Error(usage);
      opts[a.slice(2)] = argv[++i];
    } else if (a === "-h" || a === "--help") {
      console.log(usage);
      return 0;
    } else rest.push(a);
  }
  if (rest.length !== 1) {
    console.error(usage);
    return 2;
  }
  const dist = resolve(rest[0]);
  if (opts.check) {
    const manifest = JSON.parse(readFileSync(opts.check, "utf8"));
    if (manifest.format !== FORMAT) throw new Error(`${opts.check}: not a ${FORMAT} manifest`);
    const files = filesOf(dist);
    const diffs = compare(files, manifest);
    diffs.push(...distWorkerProblems(dist, files).map((p) => `worker    ${p}`));
    if (digestOf(manifest.files) !== manifest.digest) diffs.push(`the manifest's digest does not match its own file list`);
    if (diffs.length) {
      console.error(`FAIL: ${dist} is not the build in ${opts.check} (${manifest.digest}):`);
      for (const d of diffs) console.error(`  ${d}`);
      return 1;
    }
    console.log(`OK: ${dist} is byte-identical to ${opts.check}: ${files.length} files, ${manifest.digest}`);
    return 0;
  }
  const manifest = manifestOf(dist, { commit: opts.commit ?? null, ref: opts.ref ?? null });
  const text = JSON.stringify(manifest, null, 2) + "\n";
  if (opts.out) {
    writeFileSync(opts.out, text);
    console.log(`${opts.out}: ${manifest.files.length} files, ${manifest.digest}`);
  } else process.stdout.write(text);
  return 0;
}

// Run as a program (through a symlink too, e.g. macOS /tmp), not when imported.
if (process.argv[1] && realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url))) {
  try {
    process.exitCode = main(process.argv.slice(2));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 2;
  }
}
