#!/usr/bin/env node
/**
 * The server's layers: every top-level module of `server-rs/src`, and the
 * binary's own `main` and its modules, sits in one layer of
 * `server-rs/layers.json`, bottom up, and may use its own layer and the ones
 * below it. One module reaching up is enough to tie the layers into a single
 * dependency cycle, and then no part of the server can be read, tested or
 * split off without all the rest.
 *
 * A use is a `crate::<module>` path in code (in the binary, also the
 * library's own name, `remote_hand_server::<module>`). Comments, string
 * literals and `#[cfg(test)]` items do not count: a test may reach anywhere.
 * The check fails on
 * - a module that `layers.json` does not place, places twice, or places but
 *   is gone;
 * - an upward use that `allowed` does not list;
 * - an `allowed` edge that is no longer an upward use. The list only shrinks:
 *   an exception that is gone may not come back unnoticed.
 * It also prints the largest dependency cycle (strongly connected group) and
 * the most used modules, the numbers a change to the layers is judged by.
 *
 *   node scripts/check-module-layers.mjs [--json]     (pnpm check:layers)
 */
import { existsSync, readFileSync } from "node:fs";
import { basename, dirname, join, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { codeOnly } from "./check-open-boundary.mjs";

const blank = (text) => text.replace(/[^\n]/g, " ");
const IDENT = /[A-Za-z0-9_]/;
const NAME = "(?:r#)?([A-Za-z_][A-Za-z0-9_]*)";

/**
 * `code` (comments already blanked) with the contents of every string and
 * char literal blanked too, quotes kept, so that a `{` or a `crate::` in text
 * is not taken for code. Newlines stay, so lines still count.
 */
export function withoutLiterals(code) {
  const n = code.length;
  let out = "";
  let i = 0;
  while (i < n) {
    const c = code[i];
    // raw strings: r"…", r#"…"#, br"…", cr#"…"#
    const raw = (i === 0 || !IDENT.test(code[i - 1])) && /^(?:b|c)?r(#*)"/.exec(code.slice(i, i + 40));
    if (raw) {
      const close = '"' + raw[1];
      const end = code.indexOf(close, i + raw[0].length);
      if (end < 0) return out + blank(code.slice(i));
      out += raw[0] + blank(code.slice(i + raw[0].length, end)) + close;
      i = end + close.length;
      continue;
    }
    if (c === '"') {
      let j = i + 1;
      while (j < n && code[j] !== '"') j += code[j] === "\\" ? 2 : 1;
      out += '"' + blank(code.slice(i + 1, j)) + '"';
      i = j + 1;
      continue;
    }
    if (c === "'") {
      // a char literal ('x', '\n', '\''), or a lifetime ('a), which stays
      if (code[i + 1] === "\\") {
        const end = code.indexOf("'", i + 3);
        const stop = end < 0 ? n : end + 1;
        out += "'" + blank(code.slice(i + 1, stop - 1)) + "'";
        i = stop;
        continue;
      }
      const width = (code.codePointAt(i + 1) ?? 0) > 0xffff ? 2 : 1;
      if (code[i + 1 + width] === "'") {
        out += "'" + " ".repeat(width) + "'";
        i += 2 + width;
        continue;
      }
    }
    out += c;
    i++;
  }
  return out;
}

/** The index just past the bracket group that opens at `at` (`(`, `[` or `{`). */
function closing(code, at) {
  let depth = 0;
  for (let i = at; i < code.length; i++) {
    if ("([{".includes(code[i])) depth++;
    else if (")]}".includes(code[i]) && --depth === 0) return i + 1;
  }
  return code.length;
}

/** `cfg(test)` or `cfg(all(…, test, …))`: compiled for tests only. */
const testOnly = (predicate) => {
  const p = predicate.replace(/\s+/g, "");
  return p === "test" || (p.startsWith("all(") && p.slice(4, -1).split(",").includes("test"));
};

/**
 * `code` (comments and literals blanked) with every item, statement or field
 * under a test-only `#[cfg(…)]` blanked, attribute included.
 */
export function withoutTests(code) {
  let out = code;
  const cfg = /#\s*\[\s*cfg\s*\(/g;
  let m;
  while ((m = cfg.exec(out))) {
    const open = m.index + m[0].length - 1;
    const close = closing(out, open);
    if (!testOnly(out.slice(open + 1, close - 1))) continue;
    // the attribute's `]`, then any other attributes on the same item
    let i = out.indexOf("]", close) + 1;
    for (;;) {
      while (/\s/.test(out[i] ?? "")) i++;
      if (out[i] !== "#") break;
      i = closing(out, out.indexOf("[", i));
    }
    // The item ends at its `;`, at the `}` of its body, at a `,` (a field) or
    // at the `}` that closes the block around it. `<…>` is counted so that the
    // comma in a field's `HashMap<K, V>` does not end it.
    let depth = 0;
    let angles = 0;
    let end = out.length;
    for (let j = i; j < out.length; j++) {
      const c = out[j];
      if (c === "(" || c === "[") depth++;
      else if (c === ")" || c === "]") depth--;
      else if (c === "<") angles++;
      else if (c === ">" && out[j - 1] !== "-" && out[j - 1] !== "=") angles = Math.max(0, angles - 1);
      else if (depth === 0 && c === "{") {
        end = closing(out, j);
        if (/^\s*;/.test(out.slice(end))) end = out.indexOf(";", end) + 1;
        break;
      } else if (depth === 0 && (c === ";" || (c === "," && angles === 0))) {
        end = j + 1;
        break;
      } else if (depth === 0 && c === "}") {
        end = j;
        break;
      }
    }
    out = out.slice(0, m.index) + blank(out.slice(m.index, end)) + out.slice(end);
    cfg.lastIndex = m.index + 1;
  }
  return out;
}

/** What the scan reads of a Rust source: the code that is built outside tests. */
export const builtCode = (src) => withoutTests(withoutLiterals(codeOnly(src, "rs")));

const lineAt = (text, at) => text.slice(0, at).split("\n").length;
const MOD_DECL = new RegExp(`(?<![A-Za-z0-9_])mod\\s+${NAME}\\s*;`, "g");

/**
 * The files of the module whose root file is `file`, following its `mod x;`
 * declarations that are built outside tests (`x.rs` or `x/mod.rs` next to a
 * `mod.rs`, else under the file's own directory). `missing` lists a
 * declaration with no file, which the check reports rather than skips.
 */
function moduleFiles(file, out = [], missing = []) {
  const code = builtCode(readFileSync(file, "utf8"));
  out.push({ file, code });
  const name = basename(file);
  const dir = ["mod.rs", "lib.rs", "main.rs"].includes(name) ? dirname(file) : join(dirname(file), name.slice(0, -3));
  for (const m of code.matchAll(MOD_DECL)) {
    const child = [join(dir, `${m[1]}.rs`), join(dir, m[1], "mod.rs")].find(existsSync);
    if (child) moduleFiles(child, out, missing);
    else missing.push({ file, line: lineAt(code, m.index), name: m[1] });
  }
  return { files: out, missing };
}

/**
 * The top-level modules a path prefix (`crate`, `remote_hand_server`) names
 * in `code`, grouped imports (`crate::{a::b, c}`) included: `[{ name, at }]`.
 */
export function namedModules(code, prefix) {
  const found = [];
  const re = new RegExp(`(?<![A-Za-z0-9_:])\\$?${prefix}\\s*::\\s*(?:(\\{)|${NAME})`, "g");
  for (const m of code.matchAll(re)) {
    if (m[2]) {
      found.push({ name: m[2], at: m.index });
      continue;
    }
    const open = m.index + m[0].length - 1;
    const group = code.slice(open + 1, closing(code, open) - 1);
    let depth = 0;
    let part = "";
    for (const c of group + ",") {
      if (c === "{") depth++;
      if (c === "}") depth--;
      if (c === "," && depth === 0) {
        const name = new RegExp(`^\\s*${NAME}`).exec(part);
        if (name) found.push({ name: name[1], at: m.index });
        part = "";
      } else part += c;
    }
  }
  return found;
}

/** The library's crate name, as the binary's code spells it. */
const libName = (manifest) => {
  const lib = /^\[lib\][^[]*?^name\s*=\s*"([^"]+)"/m.exec(manifest);
  const pkg = /^\[package\][^[]*?^name\s*=\s*"([^"]+)"/m.exec(manifest);
  return (lib?.[1] ?? pkg?.[1] ?? "").replace(/-/g, "_");
};

/**
 * The server's module graph, read from `<serverDir>/src`: the library's
 * top-level modules, and the binary's root (`main`) and its modules.
 * `{ modules, files, edges: Map<from, Map<to, { count, where }>>, problems }`.
 */
export function moduleGraph(serverDir) {
  const src = join(serverDir, "src");
  const lib = libName(readFileSync(join(serverDir, "Cargo.toml"), "utf8"));
  const problems = [];
  const owners = new Map();
  const names = { lib: new Set(), bin: new Set() };
  for (const [rootFile, kind] of [
    ["lib.rs", "lib"],
    ["main.rs", "bin"],
  ]) {
    const root = join(src, rootFile);
    if (!existsSync(root)) continue;
    const rootCode = builtCode(readFileSync(root, "utf8"));
    if (kind === "bin") owners.set("main", { files: [{ file: root, code: rootCode }], kind });
    for (const m of rootCode.matchAll(MOD_DECL)) {
      const name = m[1];
      if (owners.has(name)) problems.push(`${name}: a module of both the library and the binary`);
      names[kind].add(name);
      const file = [join(src, `${name}.rs`), join(src, name, "mod.rs")].find(existsSync);
      if (!file) {
        problems.push(`src/${rootFile}:${lineAt(rootCode, m.index)}: mod ${name}; has no file`);
        continue;
      }
      const { files, missing } = moduleFiles(file);
      for (const x of missing) problems.push(`${relative(serverDir, x.file)}:${x.line}: mod ${x.name}; has no file`);
      owners.set(name, { files, kind });
    }
  }
  const edges = new Map();
  for (const [from, { files, kind }] of owners) {
    const out = new Map();
    for (const { file, code } of files) {
      const uses =
        kind === "bin"
          ? [...namedModules(code, "crate").filter((u) => names.bin.has(u.name)), ...namedModules(code, lib).filter((u) => names.lib.has(u.name))]
          : namedModules(code, "crate").filter((u) => names.lib.has(u.name));
      for (const { name, at } of uses) {
        if (name === from) continue;
        const edge = out.get(name) ?? { count: 0, where: `${relative(serverDir, file)}:${lineAt(code, at)}` };
        edge.count++;
        out.set(name, edge);
      }
    }
    edges.set(from, out);
  }
  const files = [...owners.values()].flatMap((o) => o.files.map((f) => relative(serverDir, f.file)));
  return { modules: [...owners.keys()].sort(), files, edges, problems };
}

/** The strongly connected groups of more than one module, largest first (Tarjan's algorithm). */
export function cycles(modules, edges) {
  let next = 0;
  const index = new Map();
  const low = new Map();
  const stack = [];
  const onStack = new Set();
  const groups = [];
  const visit = (v) => {
    index.set(v, next);
    low.set(v, next++);
    stack.push(v);
    onStack.add(v);
    for (const w of edges.get(v)?.keys() ?? []) {
      if (!index.has(w)) {
        visit(w);
        low.set(v, Math.min(low.get(v), low.get(w)));
      } else if (onStack.has(w)) low.set(v, Math.min(low.get(v), index.get(w)));
    }
    if (low.get(v) === index.get(v)) {
      const group = [];
      let w;
      do {
        w = stack.pop();
        onStack.delete(w);
        group.push(w);
      } while (w !== v);
      if (group.length > 1) groups.push(group.sort());
    }
  };
  for (const v of modules) if (!index.has(v)) visit(v);
  return groups.sort((a, b) => b.length - a.length);
}

/** How many other modules use each module, most used first. */
export function fanIn(edges) {
  const users = new Map();
  for (const out of edges.values()) for (const to of out.keys()) users.set(to, (users.get(to) ?? 0) + 1);
  return new Map([...users].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])));
}

/**
 * `graph` checked against `config` (`layers.json`: `layers`, bottom up, each
 * `{ name, modules }`; `allowed`, each `{ why, edges: ["from -> to", …] }`):
 * `{ problems, upward, stats }`.
 */
export function checkLayers(graph, config) {
  const problems = [...graph.problems];
  const layerOf = new Map();
  config.layers.forEach(({ name, modules }, level) => {
    for (const m of modules) {
      if (layerOf.has(m)) problems.push(`layers.json places ${m} twice`);
      layerOf.set(m, { name, level });
    }
  });
  for (const m of graph.modules) {
    if (!layerOf.has(m)) problems.push(`${m}: not in layers.json; place it in the lowest layer that holds everything it uses`);
  }
  for (const m of layerOf.keys()) {
    if (!graph.modules.includes(m)) problems.push(`layers.json places ${m}, which is no longer a module: drop it`);
  }

  const upward = [];
  for (const [from, out] of graph.edges) {
    for (const [to, { count, where }] of out) {
      const a = layerOf.get(from);
      const b = layerOf.get(to);
      if (a && b && b.level > a.level) upward.push({ edge: `${from} -> ${to}`, from: a.name, to: b.name, count, where });
    }
  }
  upward.sort((a, b) => a.edge.localeCompare(b.edge));
  const allowed = new Set();
  for (const { edges = [] } of config.allowed ?? []) {
    for (const edge of edges) {
      if (allowed.has(edge)) problems.push(`allowed lists ${edge} twice`);
      allowed.add(edge);
    }
  }
  for (const u of upward) {
    if (!allowed.has(u.edge)) problems.push(`${u.edge}: ${u.from} uses ${u.to} (${u.count} use(s), the first at ${u.where})`);
  }
  const actual = new Set(upward.map((u) => u.edge));
  for (const edge of allowed) {
    if (!actual.has(edge)) problems.push(`allowed lists ${edge}, which is no longer an upward use: drop it`);
  }

  const users = fanIn(graph.edges);
  const stats = {
    modules: graph.modules.length,
    files: graph.files.length,
    edges: [...graph.edges.values()].reduce((n, out) => n + out.size, 0),
    upward: upward.length,
    cycle: cycles(graph.modules, graph.edges)[0] ?? [],
    fanIn: Object.fromEntries(users),
  };
  return { problems, upward, stats };
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const serverDir = resolve(dirname(fileURLToPath(import.meta.url)), "..", "server-rs");
  const config = JSON.parse(readFileSync(join(serverDir, "layers.json"), "utf8"));
  const { problems, upward, stats } = checkLayers(moduleGraph(serverDir), config);
  if (process.argv.includes("--json")) {
    console.log(JSON.stringify({ ...stats, upwardEdges: upward.map((u) => u.edge), problems }, null, 2));
    process.exit(problems.length ? 1 : 0);
  }
  const top = Object.entries(stats.fanIn)
    .slice(0, 8)
    .map(([m, n]) => `${m} ${n}`)
    .join(", ");
  const summary = [
    `${stats.modules} modules (${stats.files} files), ${stats.edges} edges, ${stats.upward} upward (each in allowed, with its reason)`,
    `largest dependency cycle: ${stats.cycle.length} modules${stats.cycle.length ? ` (${stats.cycle.join(", ")})` : ""}`,
    `most used (modules using each): ${top}`,
  ];
  if (problems.length) {
    console.error(`module layers: ${problems.length} problem(s)`);
    for (const p of problems) console.error(`  - ${p}`);
    console.error(
      "\nA module may use its own layer and the ones below it (server-rs/layers.json). Move the shared piece down to " +
        "where both sides can reach it. An upward use that has to stay goes in allowed, with its reason; one that is " +
        "gone leaves the list.",
    );
    process.exit(1);
  }
  console.log("module layers: ok");
  for (const line of summary) console.log(`  ${line}`);
}
