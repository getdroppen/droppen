// Third-party notices for the npm packages the web app bundles: what actually
// reaches `web/dist`, not everything `package.json` lists. Node >= 20, no
// dependencies of its own; inside `web/` so every build of the app can use it
// (the Hub's image builds `web/` alone).
//
// Two users:
//   - `thirdPartyNotices()`, a Vite plugin in `vite.config.ts`: every build
//     writes `THIRD_PARTY_NOTICES.txt` beside `index.html`, so droppen.app,
//     the local program and the desktop app all serve the notices of the very
//     packages in the page (and the release manifest lists the file);
//   - `scripts/notices/web-notices.mjs`, which builds the app once more and
//     writes the web part of a release's THIRD_PARTY_NOTICES.
//
// What counts: every module rollup rendered into a chunk, every stylesheet,
// every asset's source file and every `/*! name vX` banner in the CSS, each
// mapped to the npm package it came from (the package directory after the
// last `node_modules/`). First-party code — `web/src` and the workspace
// packages — is not listed. A package whose licence is missing or not on the
// allow-list below (AGPL-3.0-compatible permissive licences only) fails.

import { existsSync, readdirSync, readFileSync, realpathSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";

/** SPDX ids the server binary may carry. Keep in step with `about.toml`. */
export const ALLOWED = new Set([
  "MIT",
  "MIT-0",
  "Apache-2.0",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "0BSD",
  "ISC",
  "Zlib",
  "Unicode-3.0",
  "Unicode-DFS-2016",
  "CC0-1.0",
  "BSL-1.0",
  "Unlicense",
  "MPL-2.0",
  "CDLA-Permissive-2.0",
]);

/** Licences outside the everyday MIT / Apache / BSD / ISC set, to be named in the summary. */
const USUAL = new Set(["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC"]);

/** The file the plugin writes, at the root of `web/dist`. */
export const NOTICES_FILE = "THIRD_PARTY_NOTICES.txt";

/** Whether an SPDX expression can be satisfied with allowed licences, and the ids it names. */
export function evaluate(expression) {
  const tokens = expression.replace(/[()]/g, (p) => ` ${p} `).trim().split(/\s+/);
  let at = 0;
  const ids = [];
  const primary = () => {
    const token = tokens[at++];
    if (token === "(") {
      const value = or();
      if (tokens[at++] !== ")") throw new Error(`unbalanced: ${expression}`);
      return value;
    }
    if (!token || ["AND", "OR", "WITH", ")"].includes(token)) throw new Error(`unexpected ${token ?? "end"} in ${expression}`);
    let id = token.replace(/\+$/, "");
    if (tokens[at] === "WITH") {
      at++;
      id = `${id} WITH ${tokens[at++]}`;
    }
    ids.push(id);
    return ALLOWED.has(id) || id === "Apache-2.0 WITH LLVM-exception";
  };
  const and = () => {
    let value = primary();
    while (tokens[at] === "AND") {
      at++;
      value = primary() && value;
    }
    return value;
  };
  const or = () => {
    let value = and();
    while (tokens[at] === "OR") {
      at++;
      value = and() || value;
    }
    return value;
  };
  const ok = or();
  if (at !== tokens.length) throw new Error(`trailing tokens in ${expression}`);
  return { ok, ids };
}

/** The declared licence of a package.json, as one SPDX expression, or null. */
function declared(pkg) {
  if (typeof pkg.license === "string" && pkg.license.trim()) return pkg.license.trim();
  if (pkg.license && typeof pkg.license.type === "string") return pkg.license.type;
  if (Array.isArray(pkg.licenses) && pkg.licenses.length) {
    return pkg.licenses.map((l) => (typeof l === "string" ? l : l.type)).join(" OR ");
  }
  return null;
}

/** `<...>/node_modules/[@scope/]name` for a file inside a package, or null. */
function packageDirOf(file) {
  const at = file.lastIndexOf("/node_modules/");
  if (at < 0) return null;
  const rest = file.slice(at + "/node_modules/".length).split("/");
  const parts = rest[0].startsWith("@") ? rest.slice(0, 2) : rest.slice(0, 1);
  return file.slice(0, at) + "/node_modules/" + parts.join("/");
}

const LICENSE_FILE = /^(licen[cs]e|copying|notice|copyright)([-._].*)?$/i;

/**
 * What a build bundles, recorded from its output: `record` is a rollup
 * `generateBundle` hook (it must run after the CSS has been emitted, so in a
 * plugin with `enforce: "post"`), `files` and `banners` what it saw.
 */
export function recorder() {
  const files = new Set();
  const banners = new Set();
  const record = (bundle) => {
    for (const output of Object.values(bundle)) {
      if (output.type === "chunk") {
        for (const [id, info] of Object.entries(output.modules)) {
          // A stylesheet's JS proxy renders to nothing; its CSS still ships.
          if (info.renderedLength > 0 || /\.css($|\?)/.test(id)) files.add(id);
        }
      } else {
        for (const name of output.originalFileNames ?? []) files.add(name);
        if (output.fileName.endsWith(".css")) {
          const css = typeof output.source === "string" ? output.source : Buffer.from(output.source).toString("utf8");
          for (const m of css.matchAll(/\/\*!\s*(@?[a-z0-9][\w./-]*)\s+v\d[\w.+-]*\s*\|/gi)) banners.add(m[1]);
        }
      }
    }
  };
  return { files, banners, record };
}

/**
 * The bundled packages, each with its licence and licence files, from what a
 * build recorded. `webDir` is the app's real path (Vite resolves modules to
 * real paths).
 */
export function collect(webDir, { files, banners }) {
  const require = createRequire(join(webDir, "package.json"));
  const viteDir = dirname(require.resolve("vite/package.json"));
  const packages = new Map(); // dir -> null
  const helpers = new Set();
  for (const raw of files) {
    // `\0` marks a virtual module: a CommonJS wrapper around a real file
    // (which still names it), a bundler helper, or one of the app's own.
    const virtual = raw.startsWith("\0");
    const id = (virtual ? raw.slice(1) : raw).split("?")[0];
    const dir = packageDirOf(resolve(webDir, id));
    if (dir) packages.set(dir, null);
    else if (virtual && /^(commonjsHelpers|vite\/)/.test(id)) helpers.add(id);
  }
  for (const name of banners) {
    try {
      packages.set(dirname(require.resolve(`${name}/package.json`)), null);
    } catch {
      let at = webDir;
      for (;;) {
        const candidate = join(at, "node_modules", name);
        if (existsSync(join(candidate, "package.json"))) {
          packages.set(candidate, null);
          break;
        }
        if (dirname(at) === at) throw new Error(`a CSS banner names ${name}, which is not installed`);
        at = dirname(at);
      }
    }
  }
  if (helpers.size) {
    // The helpers are vite's (it bundles rollup's commonjs plugin): attribute them to vite.
    packages.set(viteDir, null);
  }

  const failures = [];
  const unusual = [];
  const withoutText = [];
  const counts = new Map();
  const entries = [];
  for (const dir of packages.keys()) {
    const pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
    const license = declared(pkg);
    const label = `${pkg.name}@${pkg.version}`;
    let verdict;
    try {
      verdict = license ? evaluate(license) : { ok: false, ids: [] };
    } catch (error) {
      verdict = { ok: false, ids: [String(error.message)] };
    }
    if (!license) failures.push(`${label}: no licence declared`);
    else if (!verdict.ok) failures.push(`${label}: ${license} is not on the allow-list`);
    if (license && verdict.ids.some((id) => !USUAL.has(id))) unusual.push(`${label}: ${license}`);
    counts.set(license ?? "(none)", (counts.get(license ?? "(none)") ?? 0) + 1);
    const texts = readdirSync(dir)
      .filter((name) => LICENSE_FILE.test(name) && statSync(join(dir, name)).isFile())
      .sort()
      .map((name) => ({ name, text: readFileSync(join(dir, name), "utf8").trimEnd() }));
    if (!texts.length) withoutText.push(`${label} (${license})`);
    const repository = typeof pkg.repository === "string" ? pkg.repository : pkg.repository?.url;
    entries.push({ label, license, repository, homepage: pkg.homepage, texts });
  }
  entries.sort((a, b) => (a.label < b.label ? -1 : a.label > b.label ? 1 : 0));
  const summary = [...counts.entries()].sort((a, b) => b[1] - a[1]).map(([l, n]) => `${n} ${l}`).join(", ");
  return { entries, helpers: [...helpers].sort(), failures, unusual, withoutText, summary };
}

/** The notices as text: `intro` (lines), then every package and its licence files in full. */
export function render({ entries, helpers }, intro) {
  const rule = "-".repeat(80);
  const lines = [
    `npm packages bundled into the web app (${entries.length})`,
    "",
    ...intro,
    helpers.length ? `Bundler runtime helpers (${helpers.join(", ")}) are attributed to vite.` : "",
    "",
  ];
  for (const e of entries) {
    lines.push(rule, `${e.label}  ${e.license}`);
    if (e.repository || e.homepage) lines.push(String(e.repository ?? e.homepage).replace(/^git\+/, ""));
    lines.push("");
    if (e.texts.length) {
      for (const t of e.texts) lines.push(`[${t.name}]`, t.text, "");
    } else {
      lines.push(`(The package ships no licence file. Its package.json declares ${e.license}.)`, "");
    }
  }
  return lines.filter((line, i) => line !== "" || lines[i - 1] !== "").join("\n") + "\n";
}

/** The licence check's verdict as error text, or null when every package passes. */
export function failureText({ failures }) {
  if (!failures.length) return null;
  return `${failures.length} npm package(s) bundled into the web app fail the licence check (allow-list: web/build/third-party-notices.mjs):\n${failures.map((f) => `  ${f}`).join("\n")}`;
}

/**
 * The Vite plugin: `THIRD_PARTY_NOTICES.txt` in every build, for the packages
 * in that build. The build fails when one of them fails the licence check.
 */
export function thirdPartyNotices() {
  let root = "";
  return {
    name: "droppen-third-party-notices",
    apply: "build",
    // After the CSS is emitted, as the recorder needs.
    enforce: "post",
    configResolved(config) {
      root = config.root;
    },
    generateBundle(_options, bundle) {
      const seen = recorder();
      seen.record(bundle);
      // Vite's root may be reached through a symlink (macOS's /tmp is
      // /private/tmp); the modules it records are real paths.
      const found = collect(realpathSync(resolve(root)), seen);
      const failed = failureText(found);
      if (failed) this.error(failed);
      this.emitFile({
        type: "asset",
        fileName: NOTICES_FILE,
        source: render(found, [
          "The Droppen web app is licensed under the GNU Affero General Public License,",
          "version 3 only (AGPL-3.0-only). Every package below has code or styles in this",
          "build of it; each licence follows in full as the package ships it.",
        ]),
      });
    },
  };
}
