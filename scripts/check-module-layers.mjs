#!/usr/bin/env node
/**
 * The server's target crates: every module of `server-rs/src`, and the
 * binary's own `main` and its modules, is placed in the crate of
 * `server-rs/layers.json` it is going to, and a module may use another only
 * where that crate may use the other's. One use that reaches the wrong way
 * is enough to tie the crates back into a single dependency cycle, and then
 * no part of the server can be read, tested or split off without all the
 * rest.
 *
 * Crates sit in layers, bottom up. A crate may use the crates its `mayUse`
 * lists, of its own layer or a lower one, and every crate of a lower layer
 * marked `open` (the base and the platform, which every subsystem stands
 * on); the entry's `mayUse` is `*`, every crate below it. Nothing else: a
 * crate never uses a higher layer, and what one crate may use does not pass
 * to the crates that may use it, just as a Cargo dependency's own
 * dependencies are not the dependent's to name. Modules of one crate use
 * one another freely.
 *
 * A top-level module goes to one crate. A module that is going elsewhere
 * than its parent (`inject::resident`, `sessions::codex`) is placed on its
 * own, and its files and the uses that name it follow it; a module whose
 * own items go to two crates goes to the crate that receives most of it,
 * and its crate's `notes` say where the rest goes.
 *
 * A use is a path in code: `crate::<module>…` (in the binary, also the
 * library's own name, `remote_hand_server::<module>…`), `self::…`,
 * `super::…`, or a child module's name. Comments, string literals,
 * `pub(in …)` and `#[cfg(test)]` items do not count: a test may reach
 * anywhere. The check fails on
 * - a module that `layers.json` does not place, places twice, or places but
 *   is gone;
 * - a use between two crates that may not use each other that `allowed`
 *   does not list;
 * - an `allowed` edge that is no longer such a use. The list only shrinks:
 *   an exception that is gone may not come back unnoticed;
 * - more or fewer `Source::Claude|Codex|Acp` matches outside the provider
 *   crates than `sourceMatches` records: outside them `Source` is only the
 *   key that picks a provider, so that number too only goes down.
 * It also prints the largest dependency cycle (strongly connected group) of
 * modules and of crates, each middle crate's size and fan-in, and the most
 * used modules: the numbers a change to the layers is judged by.
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

/** `text` with each `[from, to)` of `ranges` blanked. */
const blankRanges = (text, ranges) =>
  ranges.reduce((out, [from, to]) => out.slice(0, from) + blank(out.slice(from, to)) + out.slice(to), text);

/**
 * The `[from, to)` ranges of `code` (comments and literals blanked) that are
 * an item, statement or field under a test-only `#[cfg(…)]`, attribute
 * included.
 */
function testRanges(code) {
  const ranges = [];
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
    ranges.push([m.index, end]);
    out = out.slice(0, m.index) + blank(out.slice(m.index, end)) + out.slice(end);
    cfg.lastIndex = m.index + 1;
  }
  return ranges;
}

/**
 * `code` (comments and literals blanked) with every item, statement or field
 * under a test-only `#[cfg(…)]` blanked, attribute included.
 */
export const withoutTests = (code) => blankRanges(code, testRanges(code));

/**
 * `pub(in crate::a::b)` names the module an item is visible in, an ancestor
 * of its own: a limit on who may use it, not a use.
 */
const withoutVisibilityPaths = (code) => code.replace(/pub\s*\(\s*in\s[^)]*\)/g, blank);

/** What the scan reads of a Rust source: the code that is built outside tests. */
export const builtCode = (src) => withoutVisibilityPaths(withoutTests(withoutLiterals(codeOnly(src, "rs"))));

/**
 * A source file as the scan reads it (`code`, as {@link builtCode}), and its
 * size: the lines that hold code built outside tests, string literals
 * included (`lines`).
 */
function readSource(file) {
  const comments = codeOnly(readFileSync(file, "utf8"), "rs");
  const literals = withoutLiterals(comments);
  const tests = testRanges(literals);
  const code = withoutVisibilityPaths(blankRanges(literals, tests));
  const lines = blankRanges(comments, tests)
    .split("\n")
    .filter((line) => line.trim()).length;
  return { code, lines };
}

const lineAt = (text, at) => text.slice(0, at).split("\n").length;
const MOD_DECL = new RegExp(`(?<![A-Za-z0-9_])mod\\s+${NAME}\\s*;`, "g");
const INLINE_MOD = new RegExp(`(?<![A-Za-z0-9_])mod\\s+${NAME}\\s*\\{`, "g");

const SEGMENT = /\s*(?:r#)?([A-Za-z_][A-Za-z0-9_]*)/y;
const SEPARATOR = /\s*::/y;
const GROUP = /\s*\{/y;

/**
 * The paths a use tree or a path expression names from `at` on, as lists of
 * segments: `a::b::{c, d::{e, self}}` is `[a,b,c]`, `[a,b,d,e]`, `[a,b,d]`;
 * `a::B::<T>::f()` is `[a,B]`; `a::*` is `[a]`.
 */
function tree(code, at, prefix = []) {
  GROUP.lastIndex = at;
  if (GROUP.test(code)) {
    const open = GROUP.lastIndex - 1;
    const end = closing(code, open) - 1;
    const found = [];
    let depth = 0;
    let start = open + 1;
    for (let j = open + 1; j <= end; j++) {
      if (code[j] === "{") depth++;
      else if (code[j] === "}" && j < end) depth--;
      if ((code[j] === "," && depth === 0) || j === end) {
        if (code.slice(start, j).trim()) found.push(...tree(code, start, prefix));
        start = j + 1;
      }
    }
    return found;
  }
  SEGMENT.lastIndex = at;
  const m = SEGMENT.exec(code);
  if (!m) return [prefix];
  if (m[1] === "self") return [prefix];
  const path = [...prefix, m[1]];
  SEPARATOR.lastIndex = SEGMENT.lastIndex;
  return SEPARATOR.test(code) ? tree(code, SEPARATOR.lastIndex, path) : [path];
}

/**
 * The paths that start with one of `heads` (a regular-expression
 * alternation: `crate`, a crate's name, `super(::super)*`, a module's name)
 * in `code`: `[{ head, path, at }]`, `path` being the segments after the
 * head, grouped imports expanded. A head right after one of `after`'s
 * characters is not a path's start (`a::crate`, or `x.name::<T>()`, a method).
 */
export function pathsFrom(code, heads, after = "A-Za-z0-9_:") {
  const found = [];
  const re = new RegExp(`(?<![${after}])\\$?(${heads})\\s*::`, "g");
  for (const m of code.matchAll(re)) {
    for (const path of tree(code, m.index + m[0].length)) found.push({ head: m[1], path, at: m.index });
  }
  return found;
}

/**
 * The top-level modules a path prefix (`crate`, `remote_hand_server`) names
 * in `code`, grouped imports (`crate::{a::b, c}`) included: `[{ name, at }]`.
 */
export function namedModules(code, prefix) {
  return pathsFrom(code, prefix)
    .filter((p) => p.path.length)
    .map((p) => ({ name: p.path[0], at: p.at }));
}

/** The inline (`mod x { … }`) modules of `code`: `[{ name, from, to }]`. */
function inlineModules(code) {
  return [...code.matchAll(INLINE_MOD)].map((m) => ({ name: m[1], from: m.index, to: closing(code, m.index + m[0].length - 1) }));
}

/** The module path at `at` of a file whose own module path is `path`. */
const pathAt = (path, inline, at) => [...path, ...inline.filter((m) => m.from < at && at < m.to).map((m) => m.name)];

/**
 * The files of the module whose root file is `file` and whose path is
 * `path`, following its `mod x;` declarations that are built outside tests
 * (`x.rs` or `x/mod.rs` next to a `mod.rs`, else under the file's own
 * directory). `missing` lists a declaration with no file, which the check
 * reports rather than skips.
 */
function moduleFiles(file, path, out = [], missing = []) {
  const { code, lines } = readSource(file);
  const children = [...code.matchAll(MOD_DECL)];
  out.push({ file, code, lines, path, children: children.map((m) => m[1]) });
  const name = basename(file);
  const dir = ["mod.rs", "lib.rs", "main.rs"].includes(name) ? dirname(file) : join(dirname(file), name.slice(0, -3));
  for (const m of children) {
    const child = [join(dir, `${m[1]}.rs`), join(dir, m[1], "mod.rs")].find(existsSync);
    if (child) moduleFiles(child, [...path, m[1]], out, missing);
    else missing.push({ file, line: lineAt(code, m.index), name: m[1] });
  }
  return { files: out, missing };
}

/** The library's crate name, as the binary's code spells it. */
const libName = (manifest) => {
  const lib = /^\[lib\][^[]*?^name\s*=\s*"([^"]+)"/m.exec(manifest);
  const pkg = /^\[package\][^[]*?^name\s*=\s*"([^"]+)"/m.exec(manifest);
  return (lib?.[1] ?? pkg?.[1] ?? "").replace(/-/g, "_");
};

const escape = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const SOURCE_MATCH = /(?<![A-Za-z0-9_])Source\s*::\s*(?:Claude|Codex|Acp)(?![A-Za-z0-9_])/g;

/**
 * The server's module graph, read from `<serverDir>/src`: the library's
 * modules, and the binary's root (`main`) and its modules.
 * - `modules`: the top-level modules; `paths`: every module path (`a`,
 *   `a::b`, inline ones too); `files`;
 * - `uses`: every use, `{ from, to, where }`, `from` and `to` module paths as
 *   segment lists (`to` may go on into an item: `[sessions, Source]`);
 * - `edges`: the top-level modules' graph, `Map<from, Map<to, { count, where }>>`;
 * - `sizes`: per file, `{ file, path, lines, sources }`: its code lines and
 *   where it matches `Source::Claude|Codex|Acp`;
 * - `problems`.
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
    const { code: rootCode, lines } = readSource(root);
    if (kind === "bin") owners.set("main", { files: [{ file: root, code: rootCode, lines, path: ["main"], children: [] }], kind });
    for (const m of rootCode.matchAll(MOD_DECL)) {
      const name = m[1];
      if (owners.has(name)) problems.push(`${name}: a module of both the library and the binary`);
      names[kind].add(name);
      const file = [join(src, `${name}.rs`), join(src, name, "mod.rs")].find(existsSync);
      if (!file) {
        problems.push(`src/${rootFile}:${lineAt(rootCode, m.index)}: mod ${name}; has no file`);
        continue;
      }
      const { files, missing } = moduleFiles(file, [name]);
      for (const x of missing) problems.push(`${relative(serverDir, x.file)}:${x.line}: mod ${x.name}; has no file`);
      owners.set(name, { files, kind });
    }
  }
  // The binary's root declares its own modules: `crate::export` from
  // `main.rs` is a use of `export`, so `main`'s children are those.
  if (owners.has("main")) owners.get("main").files[0].children = [...names.bin];

  const paths = new Set();
  const uses = [];
  const sizes = [];
  for (const [, { files, kind }] of owners) {
    for (const { file, code, lines, path, children } of files) {
      const rel = relative(serverDir, file);
      const inline = inlineModules(code);
      paths.add(path.join("::"));
      for (const m of inline) paths.add([...pathAt(path, inline, m.from), m.name].join("::"));
      // `main.rs` is the binary's root: its paths start at the crate's top.
      const base = (at) => (kind === "bin" && path[0] === "main" ? pathAt([], inline, at) : pathAt(path, inline, at));
      const known = (to, ns) => to.length > 0 && names[ns].has(to[0]);
      const add = (at, to) => uses.push({ from: pathAt(path, inline, at), to, where: `${rel}:${lineAt(code, at)}` });
      const heads = ["crate", "self", "super(?:\\s*::\\s*super)*", ...(kind === "bin" ? [escape(lib)] : [])].join("|");
      const childNames = [...children, ...inline.map((m) => m.name)].map(escape).join("|");
      const found = [...pathsFrom(code, heads), ...(childNames ? pathsFrom(code, childNames, "A-Za-z0-9_:.") : [])];
      for (const { head, path: rest, at } of found) {
        const here = base(at);
        let to;
        let ns = kind;
        if (head === "crate") to = rest;
        else if (head === lib && kind === "bin") (to = rest), (ns = "lib");
        else if (head === "self") to = [...here, ...rest];
        else if (/^super(\s*::\s*super)*$/.test(head)) {
          const up = head.split("super").length - 1;
          if (up > here.length) continue;
          to = [...here.slice(0, here.length - up), ...rest];
        } else to = [...here, head, ...rest];
        if (known(to, ns)) add(at, to);
      }
      const sources = [...code.matchAll(SOURCE_MATCH)].map((m) => `${rel}:${lineAt(code, m.index)}`);
      sizes.push({ file: rel, path, lines, sources });
    }
  }

  const edges = new Map([...owners.keys()].map((m) => [m, new Map()]));
  for (const { from, to, where } of uses) {
    if (from[0] === to[0]) continue;
    const out = edges.get(from[0]);
    const edge = out.get(to[0]) ?? { count: 0, where };
    edge.count++;
    out.set(to[0], edge);
  }
  const files = [...owners.values()].flatMap((o) => o.files.map((f) => relative(serverDir, f.file)));
  return { modules: [...owners.keys()].sort(), paths, files, uses, edges, sizes, problems };
}

/** The strongly connected groups of more than one node, largest first (Tarjan's algorithm). */
export function cycles(nodes, edges) {
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
  for (const v of nodes) if (!index.has(v)) visit(v);
  return groups.sort((a, b) => b.length - a.length);
}

/** How many others use each node, most used first. */
export function fanIn(edges) {
  const users = new Map();
  for (const out of edges.values()) for (const to of out.keys()) users.set(to, (users.get(to) ?? 0) + 1);
  return new Map([...users].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])));
}

/** `edges`'s `{ count, where }` for `from -> to`, counted. */
const count = (edges, from, to, where) => {
  const out = edges.get(from) ?? new Map();
  const edge = out.get(to) ?? { count: 0, where };
  edge.count++;
  out.set(to, edge);
  edges.set(from, out);
};

/**
 * `graph` checked against `config` (`layers.json`):
 * - `layers`, bottom up, each `{ name, open?, middle?, crates: [{ name,
 *   modules, mayUse?, notes? }] }`: the crates of each layer, the modules
 *   (`a`, `a::b`) going to each, and the crates each may use besides those
 *   of the open layers below it (`*`: every crate below it);
 * - `allowed`, each `{ why, edges: ["from -> to", …] }`, between modules;
 * - `sourceMatches`, `{ providers: [crate, …], count }`.
 * Returns `{ problems, violations, stats }`.
 */
export function checkLayers(graph, config) {
  const problems = [...graph.problems];
  const crates = new Map();
  const crateOf = new Map();
  config.layers.forEach((layer, level) => {
    for (const c of layer.crates ?? []) {
      if (crates.has(c.name)) problems.push(`layers.json lists the crate ${c.name} twice`);
      crates.set(c.name, {
        name: c.name,
        layer: layer.name,
        level,
        open: Boolean(layer.open),
        middle: Boolean(layer.middle),
        mayUse: c.mayUse ?? [],
        modules: c.modules ?? [],
      });
      for (const m of c.modules ?? []) {
        if (crateOf.has(m)) problems.push(`layers.json places ${m} twice`);
        crateOf.set(m, c.name);
      }
    }
  });
  for (const c of crates.values()) {
    if (c.mayUse === "*") continue;
    for (const u of c.mayUse) {
      const other = crates.get(u);
      if (!other) problems.push(`${c.name} may use ${u}, which layers.json does not list`);
      else if (other.level > c.level) problems.push(`${c.name} lists ${u} in mayUse: a crate of a higher layer, which it may never use`);
      else if (other.open && other.level < c.level) problems.push(`${c.name} lists ${u} in mayUse: every crate above its layer, ${other.layer}, may use it already`);
    }
  }
  for (const m of graph.modules) {
    if (!crateOf.has(m)) problems.push(`${m}: not in layers.json; place it in the crate it is going to`);
  }
  for (const m of crateOf.keys()) {
    if (!graph.paths.has(m)) problems.push(`layers.json places ${m}, which is no longer a module: drop it`);
  }

  const mayUse = (a, b) => {
    const [x, y] = [crates.get(a), crates.get(b)];
    if (a === b) return true;
    if (y.level < x.level && (y.open || x.mayUse === "*")) return true;
    return x.mayUse !== "*" && x.mayUse.includes(b) && y.level <= x.level;
  };
  const unitOf = (path) => {
    for (let n = path.length; n > 0; n--) {
      const unit = path.slice(0, n).join("::");
      if (crateOf.has(unit)) return unit;
    }
    return undefined;
  };

  const unitEdges = new Map();
  const crateEdges = new Map();
  for (const { from, to, where } of graph.uses) {
    const [a, b] = [unitOf(from), unitOf(to)];
    if (!a || !b || a === b) continue;
    count(unitEdges, a, b, where);
    if (crateOf.get(a) !== crateOf.get(b)) count(crateEdges, crateOf.get(a), crateOf.get(b), where);
  }

  const violations = [];
  for (const [from, out] of unitEdges) {
    for (const [to, { count: n, where }] of out) {
      const [a, b] = [crateOf.get(from), crateOf.get(to)];
      if (mayUse(a, b)) continue;
      const [x, y] = [crates.get(a), crates.get(b)];
      const why = y.level > x.level ? `${b} is in a higher layer (${y.layer})` : `${a}'s mayUse does not list ${b}`;
      violations.push({ edge: `${from} -> ${to}`, from: a, to: b, why, count: n, where });
    }
  }
  violations.sort((p, q) => p.edge.localeCompare(q.edge));
  const allowed = new Set();
  for (const { edges = [] } of config.allowed ?? []) {
    for (const edge of edges) {
      if (allowed.has(edge)) problems.push(`allowed lists ${edge} twice`);
      allowed.add(edge);
    }
  }
  for (const v of violations) {
    if (!allowed.has(v.edge)) problems.push(`${v.edge}: ${v.from} may not use ${v.to}: ${v.why} (${v.count} use(s), the first at ${v.where})`);
  }
  const actual = new Set(violations.map((v) => v.edge));
  for (const edge of allowed) {
    if (!actual.has(edge)) problems.push(`allowed lists ${edge}, which is no longer a use between crates that may not use each other: drop it`);
  }

  // Source matches outside the provider crates.
  const providers = new Set(config.sourceMatches?.providers ?? []);
  for (const p of providers) if (!crates.has(p)) problems.push(`sourceMatches names ${p}, which layers.json does not list`);
  const sources = graph.sizes
    .filter((f) => !providers.has(crateOf.get(unitOf(f.path))))
    .flatMap((f) => f.sources.map((where) => ({ crate: crateOf.get(unitOf(f.path)), where })));
  const recorded = config.sourceMatches?.count ?? 0;
  if (sources.length !== recorded) {
    const files = new Map();
    for (const { where } of sources) files.set(where.split(":")[0], (files.get(where.split(":")[0]) ?? 0) + 1);
    const list = [...files].sort((a, b) => b[1] - a[1]).map(([f, n]) => `${f} ${n}`).join(", ");
    problems.push(
      sources.length > recorded
        ? `${sources.length} matches of Source::Claude|Codex|Acp outside the provider crates, more than the ${recorded} sourceMatches records: dispatch through the provider instead (${list})`
        : `${sources.length} matches of Source::Claude|Codex|Acp outside the provider crates, fewer than the ${recorded} sourceMatches records: lower it to ${sources.length}`,
    );
  }

  const code = new Map();
  for (const f of graph.sizes) {
    const c = crateOf.get(unitOf(f.path));
    if (c) code.set(c, (code.get(c) ?? 0) + f.lines);
  }
  const crateUsers = fanIn(crateEdges);
  const stats = {
    modules: graph.modules.length,
    files: graph.files.length,
    edges: [...graph.edges.values()].reduce((n, out) => n + out.size, 0),
    crateEdges: [...crateEdges.values()].reduce((n, out) => n + out.size, 0),
    violations: violations.length,
    cycle: cycles(graph.modules, graph.edges)[0] ?? [],
    crateCycle: cycles([...crates.keys()], crateEdges)[0] ?? [],
    middle: [...crates.values()]
      .filter((c) => c.middle && c.modules.length)
      .map((c) => ({ crate: c.name, code: code.get(c.name) ?? 0, fanIn: crateUsers.get(c.name) ?? 0 })),
    sourceMatches: sources.length,
    fanIn: Object.fromEntries(fanIn(graph.edges)),
  };
  return { problems, violations, stats };
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const serverDir = resolve(dirname(fileURLToPath(import.meta.url)), "..", "server-rs");
  const config = JSON.parse(readFileSync(join(serverDir, "layers.json"), "utf8"));
  const { problems, violations, stats } = checkLayers(moduleGraph(serverDir), config);
  if (process.argv.includes("--json")) {
    console.log(JSON.stringify({ ...stats, violationEdges: violations.map((v) => v.edge), problems }, null, 2));
    process.exit(problems.length ? 1 : 0);
  }
  const top = Object.entries(stats.fanIn)
    .slice(0, 8)
    .map(([m, n]) => `${m} ${n}`)
    .join(", ");
  const summary = [
    `${stats.modules} modules (${stats.files} files), ${stats.edges} edges between them, ${stats.crateEdges} between their crates`,
    `${stats.violations} uses between crates that may not use each other (each in allowed, with its reason)`,
    `largest dependency cycle: ${stats.cycle.length} modules${stats.cycle.length ? ` (${stats.cycle.join(", ")})` : ""}`,
    `largest crate cycle: ${stats.crateCycle.length} crates${stats.crateCycle.length ? ` (${stats.crateCycle.join(", ")})` : ""}`,
    `middle crates (code lines, crates using each): ${stats.middle.map((c) => `${c.crate} ${c.code} / ${c.fanIn}`).join(", ")}`,
    `Source::Claude|Codex|Acp outside the provider crates: ${stats.sourceMatches}`,
    `most used modules (modules using each): ${top}`,
  ];
  if (problems.length) {
    console.error(`module layers: ${problems.length} problem(s)`);
    for (const p of problems) console.error(`  - ${p}`);
    console.error(
      "\nA module may use the modules of its own crate, the crates its crate's mayUse lists, and the crates of the open " +
        "layers below it (server-rs/layers.json). Move the shared piece down to where both sides can reach it. A use " +
        "that has to stay goes in allowed, with its reason; one that is gone leaves the list.",
    );
    process.exit(1);
  }
  console.log("module layers: ok");
  for (const line of summary) console.log(`  ${line}`);
}
